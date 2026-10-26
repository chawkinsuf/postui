//! The one place in the workspace that touches the filesystem for project,
//! local and config files. Every path is relative to a root; every write
//! is atomic; every read and write records a stamp for `changed`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A path relative to a `Disk` root: forward slashes, no empty, `.` or
/// `..` components, never absolute. Built by document units from slugs,
/// so nothing outside `Disk` can name a file the unit does not own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelPath(String);

impl RelPath {
    pub fn new(s: impl AsRef<str>) -> Result<RelPath, DiskError> {
        let s = s.as_ref();
        if s.starts_with('/') || s.starts_with('\\') {
            return Err(DiskError::BadPath(s.to_string()));
        }
        let mut parts: Vec<&str> = Vec::new();
        for seg in s.split(['/', '\\']) {
            match seg {
                "" => continue,
                "." | ".." => return Err(DiskError::BadPath(s.to_string())),
                seg => parts.push(seg),
            }
        }
        if parts.is_empty() {
            return Err(DiskError::BadPath(s.to_string()));
        }
        Ok(RelPath(parts.join("/")))
    }

    pub fn join(&self, s: &str) -> Result<RelPath, DiskError> {
        RelPath::new(format!("{}/{s}", self.0))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parent(&self) -> Option<RelPath> {
        self.0.rsplit_once('/').map(|(p, _)| RelPath(p.to_string()))
    }

    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// `self` is `other` or sits under it.
    pub fn starts_with(&self, other: &RelPath) -> bool {
        self == other || self.0.starts_with(&format!("{}/", other.0))
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    #[error("could not {op} {path}: {source}")]
    Io { op: &'static str, path: String, source: std::io::Error },
    #[error("invalid path {0:?}")]
    BadPath(String),
    #[error("{0} already exists")]
    AlreadyExists(String),
    #[error("{0} not found")]
    NotFound(String),
}

impl DiskError {
    fn io<'a>(
        op: &'static str,
        path: &'a RelPath,
    ) -> impl FnOnce(std::io::Error) -> DiskError + 'a {
        move |source| DiskError::Io { op, path: path.to_string(), source }
    }
}

/// What `changed` compares: mtime and length, absence, or a stat that
/// failed for some other reason.
///
/// `Present` is mtime + length, which is coarse on purpose: it costs one
/// stat and no read. On a filesystem with 1-2 s mtime resolution an
/// outside write of the same length in the same tick as ours is
/// indistinguishable from no write at all, and a byte-identical rewrite
/// (a git checkout, a formatter, `touch`) reads as a change. Consumers
/// that must not be fooled either way need a content hash beside this.
///
/// `Unreadable` is kept apart from `Absent` so a consumer can tell "the
/// file is gone" from "the file is there and something stopped me looking
/// at it" (EACCES, EIO, ENOTDIR) — the project rule is that a file which
/// exists but cannot be read is reported truthfully, never treated as
/// missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    Absent,
    Unreadable,
    Present { mtime: Option<SystemTime>, len: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// One trashed path: where it was and the slot it sits in now
/// (`<TRASH_DIR>/<n>/<original>`). Undo of a delete is a rename back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub original: RelPath,
    pub slot: RelPath,
}

pub struct Disk {
    root: PathBuf,
    stamps: HashMap<RelPath, Stamp>,
    /// Files written owner-only (0600) regardless of the umask or the
    /// mode already on disk — the secrets file.
    private: HashSet<RelPath>,
    /// The directory that gets a `.gitignore` of `*` when a write here
    /// creates it (task #143): a project's local folder.
    ignored: Option<RelPath>,
    /// Where `trash` puts deleted paths: a project's trash folder. A
    /// `Disk` given none (the config directory's) never trashes.
    trash: Option<RelPath>,
}

/// What [`Disk::ignore_when_created`] writes: git skips every file in the
/// directory, this one included, whatever the repository's own
/// `.gitignore` says.
const IGNORE_ALL: &str = "# Written by postui: keeps this folder (secrets, open tabs, layout) out of\n\
                          # git. Delete this file if you do want the folder committed.\n\
                          *\n";

impl Disk {
    pub fn new(root: PathBuf) -> Disk {
        Disk { root, stamps: HashMap::new(), private: HashSet::new(), ignored: None, trash: None }
    }

    /// Names the folder `trash` moves deleted paths into, numbered slots
    /// under it.
    pub fn trash_in(&mut self, dir: RelPath) {
        self.trash = Some(dir);
    }

    /// Names the directory whose creation through this `Disk` puts a
    /// `.gitignore` of `*` in it (task #143). Only the creation does: a
    /// directory that already exists without one (an older project, or a
    /// user who deleted it to commit the folder) is left as it is, since
    /// "never had one" and "removed on purpose" look the same from here
    /// (the user: "if the user wants to remove the .gitignore and commit,
    /// we should let them").
    pub fn ignore_when_created(&mut self, dir: RelPath) {
        self.ignored = Some(dir);
    }

    /// `create_dir_all(dir)` for a write of `rel`: every write, create and
    /// rename makes its directories here, so this is where the ignored
    /// directory is seen coming into being, however it does (a state
    /// save, the trash's rename), and is sealed as it does. That is why
    /// the rule lives in `Disk` and not in `Project`, which never sees the
    /// trash's directories made. `op` is what a failure says was being
    /// done to `rel`.
    fn create_dirs(
        &mut self,
        dir: &Path,
        rel: &RelPath,
        op: &'static str,
    ) -> Result<(), DiskError> {
        if let Some(d) = self.ignored_dir_to_make(dir, rel) {
            self.make_sealed(&d, rel, op)?;
        }
        std::fs::create_dir_all(dir).map_err(DiskError::io(op, rel))
    }

    /// The ignored directory, when making `dir` for a write of `rel` may
    /// have to make it: `rel` is inside it, or is it and `dir` is it (a
    /// rename *onto* the ignored path makes only its parent). `None` for
    /// one already there, the common case, at the cost of one stat per
    /// write. That stat is asked fresh each time rather than cached as
    /// "seen": a cached flag would go stale when the folder is deleted
    /// while the project is open, and the next write would then make it
    /// unsealed.
    fn ignored_dir_to_make(&self, dir: &Path, rel: &RelPath) -> Option<RelPath> {
        let d = self.ignored.as_ref()?;
        let inside = rel.starts_with(d) && (rel != d || dir == self.abs(d));
        (inside && !self.abs(d).is_dir()).then(|| d.clone())
    }

    /// Makes the ignored directory `d` with its `.gitignore`. `create_dir`
    /// answers atomically whether this call made it, so of two writers
    /// racing to make it exactly one seals it. That is all the race gets:
    /// the other writer can put its file in before the `.gitignore` lands,
    /// and if that seal then fails, the directory is no longer empty and
    /// stays unsealed. A crash between making the directory and placing
    /// the `.gitignore` leaves it unsealed the same way. Both are left
    /// there (the user, 2026-10-26: "That sounds like a super rare edge
    /// case").
    ///
    /// A seal that fails fails the write that wanted the directory, rather
    /// than letting it land unsealed: a save the user sees fail beats
    /// secrets sitting where `git add` would take them. The new directory
    /// comes back out while it is still empty (never a file another writer
    /// put there meanwhile), so the next write tries again instead of
    /// finding an unsealed directory it will never seal. A `.gitignore`
    /// that is already there by then (another writer's) seals it just the
    /// same.
    ///
    /// A failure to make the directory itself is reported as the caller's
    /// (`op` on `rel`, the file the user's write was for); one placing the
    /// `.gitignore` names the `.gitignore`.
    fn make_sealed(
        &mut self,
        d: &RelPath,
        rel: &RelPath,
        op: &'static str,
    ) -> Result<(), DiskError> {
        // Named before anything is made, so nothing between making the
        // directory and placing the file can return without the cleanup.
        let gitignore = d.join(".gitignore")?;
        let abs = self.abs(d);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(DiskError::io(op, rel))?;
        }
        match std::fs::create_dir(&abs) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(e) => return Err(DiskError::io(op, rel)(e)),
        }
        match self.place_new(&abs.join(".gitignore"), &gitignore, IGNORE_ALL.as_bytes()) {
            Ok(()) | Err(DiskError::AlreadyExists(_)) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_dir(&abs);
                Err(e)
            }
        }
    }

    /// Marks `rel` private: every write of it lands 0600, tightening a
    /// file that is already wider rather than keeping its mode.
    pub fn mark_private(&mut self, rel: RelPath) {
        self.private.insert(rel);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path — for display and for the legacy free functions
    /// during migration only; nothing new should read or write through it.
    pub fn abs(&self, rel: &RelPath) -> PathBuf {
        let mut p = self.root.clone();
        for seg in rel.as_str().split('/') {
            p.push(seg);
        }
        p
    }

    pub fn exists(&self, rel: &RelPath) -> bool {
        self.abs(rel).exists()
    }

    pub fn is_file(&self, rel: &RelPath) -> bool {
        self.abs(rel).is_file()
    }

    pub fn is_dir(&self, rel: &RelPath) -> bool {
        self.abs(rel).is_dir()
    }

    /// The file's text; `None` when it does not exist. Records a stamp.
    pub fn read(&mut self, rel: &RelPath) -> Result<Option<String>, DiskError> {
        match self.read_bytes(rel)? {
            None => Ok(None),
            Some(bytes) => String::from_utf8(bytes).map(Some).map_err(|e| DiskError::Io {
                op: "read",
                path: rel.to_string(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            }),
        }
    }

    /// `read` without recording a stamp: for scans that must not make a
    /// later `poll` think the file was seen.
    pub fn peek(&self, rel: &RelPath) -> Result<Option<String>, DiskError> {
        match std::fs::read_to_string(self.abs(rel)) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DiskError::io("read", rel)(e)),
        }
    }

    pub fn read_bytes(&mut self, rel: &RelPath) -> Result<Option<Vec<u8>>, DiskError> {
        let result = match std::fs::read(self.abs(rel)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DiskError::io("read", rel)(e)),
        };
        self.record(rel);
        result
    }

    /// Atomic write: sibling temp file, fsynced, mode copied from the
    /// existing file (a new file takes the umask, as a plain create
    /// would; a private one is always 0600), written through a symlink,
    /// then renamed over. Creates the parent directory. Records a stamp.
    pub fn write(&mut self, rel: &RelPath, text: &str) -> Result<(), DiskError> {
        self.write_bytes(rel, text.as_bytes())
    }

    pub fn write_bytes(&mut self, rel: &RelPath, contents: &[u8]) -> Result<(), DiskError> {
        let path = self.abs(rel);
        let target = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                std::fs::canonicalize(&path).map_err(DiskError::io("resolve", rel))?
            }
            Ok(_) => path.clone(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => path.clone(),
            Err(e) => return Err(DiskError::io("stat", rel)(e)),
        };
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        self.create_dirs(parent, rel, "create the directory of")?;
        let private = self.private.contains(rel);
        let tmp = Self::staged(parent, contents, rel, private)?;
        if !private && let Ok(existing) = std::fs::metadata(&target) {
            std::fs::set_permissions(tmp.path(), existing.permissions())
                .map_err(DiskError::io("write", rel))?;
        }
        // Stamped from the staged file BEFORE the rename, which carries
        // mtime and length across with the inode: the recorded stamp then
        // describes the bytes this write put there, and an outside write
        // landing a moment after the rename shows up as a change instead
        // of quietly becoming the recorded state.
        let staged = tmp.as_file().metadata().ok().map(|m| Self::stamp_of(&m));
        tmp.persist(&target).map_err(|e| DiskError::io("write", rel)(e.error))?;
        match staged {
            Some(s) => {
                self.stamps.insert(rel.clone(), s);
            }
            None => self.record(rel),
        }
        Ok(())
    }

    /// Creates the file with `text`; `AlreadyExists` when it is there.
    /// Atomic like `write`: the content is staged in a sibling temp file
    /// and linked into place only if nothing is there, so a failed write
    /// leaves no partial file and a refused one leaves no residue.
    pub fn write_new(&mut self, rel: &RelPath, text: &str) -> Result<(), DiskError> {
        let path = self.abs(rel);
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        self.create_dirs(parent, rel, "create the directory of")?;
        self.place_new(&path, rel, text.as_bytes())
    }

    /// `write_new` once the directory is there: stages `contents` beside
    /// `path` and links it in only if nothing is there. Shared with
    /// `make_sealed`, which places the `.gitignore` in the directory it
    /// just made and must not come back through `create_dirs`.
    fn place_new(&mut self, path: &Path, rel: &RelPath, contents: &[u8]) -> Result<(), DiskError> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let tmp = Self::staged(parent, contents, rel, self.private.contains(rel))?;
        match tmp.persist_noclobber(path) {
            Ok(_) => {}
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(DiskError::AlreadyExists(rel.to_string()));
            }
            Err(e) => return Err(DiskError::io("create", rel)(e.error)),
        }
        self.record(rel);
        Ok(())
    }

    /// A sibling temp file in `parent` holding `contents`, synced to the
    /// device so the rename that follows can never surface an empty file
    /// after a power loss. Created with the mode a plain create would get
    /// (0666 filtered by the umask) rather than `NamedTempFile`'s
    /// owner-only default — unless `private`, which keeps owner-only.
    fn staged(
        parent: &Path,
        contents: &[u8],
        rel: &RelPath,
        private: bool,
    ) -> Result<tempfile::NamedTempFile, DiskError> {
        let mut builder = tempfile::Builder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if private { 0o600 } else { 0o666 };
            builder.permissions(std::fs::Permissions::from_mode(mode));
        }
        let mut tmp = builder.tempfile_in(parent).map_err(DiskError::io("write", rel))?;
        std::io::Write::write_all(&mut tmp, contents).map_err(DiskError::io("write", rel))?;
        tmp.as_file().sync_all().map_err(DiskError::io("write", rel))?;
        Ok(tmp)
    }

    /// Removes a file. A missing file is success.
    pub fn remove(&mut self, rel: &RelPath) -> Result<(), DiskError> {
        match std::fs::remove_file(self.abs(rel)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(DiskError::io("remove", rel)(e)),
        }
        self.record(rel);
        Ok(())
    }

    pub fn create_dir(&mut self, rel: &RelPath) -> Result<(), DiskError> {
        self.create_dirs(&self.abs(rel), rel, "create")?;
        self.record(rel);
        Ok(())
    }

    /// The fresh stamp of `rel`, straight from disk.
    pub fn stamp(&self, rel: &RelPath) -> Stamp {
        match std::fs::metadata(self.abs(rel)) {
            Ok(m) => Self::stamp_of(&m),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Stamp::Absent,
            Err(_) => Stamp::Unreadable,
        }
    }

    fn stamp_of(m: &std::fs::Metadata) -> Stamp {
        Stamp::Present { mtime: m.modified().ok(), len: m.len() }
    }

    /// The stamp recorded at the last read or write of `rel`; `None` for a
    /// path never recorded. A caller that has just written through this
    /// `Disk` should read the seed stamp from here rather than statting
    /// again: this one describes the bytes the write actually put there.
    pub fn recorded(&self, rel: &RelPath) -> Option<Stamp> {
        self.stamps.get(rel).copied()
    }

    fn record(&mut self, rel: &RelPath) {
        let s = self.stamp(rel);
        self.stamps.insert(rel.clone(), s);
    }

    /// Renames a file or a whole directory. `NotFound` when `from` is
    /// missing, `AlreadyExists` when `to` is occupied; `to`'s parent is
    /// created. Records both stamps.
    pub fn rename(&mut self, from: &RelPath, to: &RelPath) -> Result<(), DiskError> {
        let from_abs = self.abs(from);
        let to_abs = self.abs(to);
        if !from_abs.exists() {
            return Err(DiskError::NotFound(from.to_string()));
        }
        if to_abs.exists() {
            return Err(DiskError::AlreadyExists(to.to_string()));
        }
        if let Some(parent) = to_abs.parent() {
            self.create_dirs(parent, to, "create the directory of")?;
        }
        std::fs::rename(&from_abs, &to_abs).map_err(DiskError::io("move", from))?;
        self.record(from);
        self.record(to);
        Ok(())
    }

    /// The entries of a directory, sorted by name. A missing directory
    /// lists as empty; an entry that fails to read is skipped rather than
    /// failing the whole listing (a directory that fails to open still
    /// is). Records the directory's stamp.
    pub fn list(&mut self, dir: &RelPath) -> Result<Vec<DirEntry>, DiskError> {
        let mut out = Vec::new();
        match std::fs::read_dir(self.abs(dir)) {
            Ok(entries) => {
                for e in entries {
                    let Ok(e) = e else { continue };
                    let is_dir = e.path().is_dir();
                    out.push(DirEntry {
                        name: e.file_name().to_string_lossy().to_string(),
                        is_dir,
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(DiskError::io("list", dir)(e)),
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        self.record(dir);
        Ok(out)
    }

    /// Every file under `dir` (recursively) whose extension is `ext`,
    /// sorted by path. The first directory that fails to list is reported
    /// as the warning and skipped; everything else is still returned.
    pub fn walk_files(&mut self, dir: &RelPath, ext: &str) -> (Vec<RelPath>, Option<String>) {
        let mut files = Vec::new();
        let mut warning = None;
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let entries = match self.list(&d) {
                Ok(e) => e,
                Err(e) => {
                    warning.get_or_insert(e.to_string());
                    continue;
                }
            };
            for e in entries {
                let Ok(p) = d.join(&e.name) else { continue };
                if e.is_dir {
                    stack.push(p);
                } else if e.name.rsplit_once('.').is_some_and(|(_, x)| x == ext) {
                    files.push(p);
                }
            }
        }
        files.sort();
        (files, warning)
    }

    pub fn remove_dir_all(&mut self, rel: &RelPath) -> Result<(), DiskError> {
        match std::fs::remove_dir_all(self.abs(rel)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(DiskError::io("remove", rel)(e)),
        }
        self.record(rel);
        Ok(())
    }

    /// Whether `rel` differs from the stamp recorded at the last read or
    /// write. `false` for a path never recorded.
    pub fn changed(&self, rel: &RelPath) -> bool {
        match self.stamps.get(rel) {
            None => false,
            Some(recorded) => *recorded != self.stamp(rel),
        }
    }

    /// Drops every recorded stamp, so the next `poll` sees everything as
    /// unchanged until it is read again — the "force a full reload" hook.
    pub fn forget_stamps(&mut self) {
        self.stamps.clear();
    }

    /// The next free numbered slot: one more than the largest existing
    /// numeric entry, starting at 1.
    fn next_trash_slot(&mut self, root: &RelPath) -> Result<RelPath, DiskError> {
        let max =
            self.list(root)?.iter().filter_map(|e| e.name.parse::<u64>().ok()).max().unwrap_or(0);
        root.join(&(max + 1).to_string())
    }

    /// Renames a file or directory into a fresh trash slot. One rename,
    /// so the cost is independent of size. An error, touching nothing, on
    /// a `Disk` given no trash folder.
    pub fn trash(&mut self, rel: &RelPath) -> Result<Ticket, DiskError> {
        let Some(root) = self.trash.clone() else {
            let none = std::io::Error::new(std::io::ErrorKind::Unsupported, "no trash folder");
            return Err(DiskError::io("trash", rel)(none));
        };
        let slot = self.next_trash_slot(&root)?.join(rel.as_str())?;
        self.rename(rel, &slot)?;
        Ok(Ticket { original: rel.clone(), slot })
    }

    /// Renames a trashed path back. `AlreadyExists` when the original is
    /// occupied; never clobbers.
    pub fn restore(&mut self, t: &Ticket) -> Result<(), DiskError> {
        self.rename(&t.slot, &t.original)
    }

    /// The redo half of `restore`: back into the recorded slot.
    pub fn retrash(&mut self, t: &Ticket) -> Result<(), DiskError> {
        self.rename(&t.original, &t.slot)
    }

    /// Removes the trash folder and everything in it; nothing to do for a
    /// `Disk` with no trash.
    pub fn empty_trash(&mut self) -> Result<(), DiskError> {
        match self.trash.clone() {
            Some(root) => self.remove_dir_all(&root),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{LAYOUT_TOML, LOCAL_DIR, SECRETS_TOML, STATE_TOML, TRASH_DIR};

    /// A `Disk` with the project's trash folder, as `Project` builds it.
    fn disk() -> (tempfile::TempDir, Disk) {
        let dir = tempfile::tempdir().unwrap();
        let mut disk = Disk::new(dir.path().to_path_buf());
        disk.trash_in(RelPath::new(TRASH_DIR).unwrap());
        (dir, disk)
    }

    /// `file` in the local folder.
    fn local(file: &str) -> RelPath {
        RelPath::new(LOCAL_DIR).unwrap().join(file).unwrap()
    }

    #[test]
    fn relpath_rejects_absolute_empty_and_parent_components() {
        assert!(RelPath::new("").is_err());
        assert!(RelPath::new("/etc/passwd").is_err());
        assert!(RelPath::new("../x").is_err());
        assert!(RelPath::new("a/../b").is_err());
        assert!(RelPath::new("a/./b").is_err());
        assert_eq!(RelPath::new("a/b.toml").unwrap().as_str(), "a/b.toml");
        assert_eq!(RelPath::new("a//b").unwrap().as_str(), "a/b");
        assert_eq!(RelPath::new("a/b/").unwrap().as_str(), "a/b");
    }

    #[test]
    fn relpath_join_parent_and_prefix() {
        let p = RelPath::new("requests/main").unwrap();
        let f = p.join("ping.toml").unwrap();
        assert_eq!(f.as_str(), "requests/main/ping.toml");
        assert_eq!(f.parent().unwrap().as_str(), "requests/main");
        assert_eq!(p.parent().unwrap().as_str(), "requests");
        assert!(RelPath::new("requests").unwrap().parent().is_none());
        assert!(f.starts_with(&p));
        assert!(!RelPath::new("requests/main2/x.toml").unwrap().starts_with(&p));
        assert_eq!(f.file_name(), "ping.toml");
        assert!(p.join("../x").is_err());
    }

    #[test]
    fn read_of_a_missing_file_is_none_and_write_creates_parents() {
        let (_d, mut disk) = disk();
        let p = RelPath::new("a/b/c.toml").unwrap();
        assert_eq!(disk.read(&p).unwrap(), None);
        disk.write(&p, "x = 1\n").unwrap();
        assert_eq!(disk.read(&p).unwrap().as_deref(), Some("x = 1\n"));
        assert!(disk.is_file(&p));
        assert!(disk.is_dir(&RelPath::new("a/b").unwrap()));
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let (dir, mut disk) = disk();
        let p = RelPath::new("project.toml").unwrap();
        disk.write(&p, "name = \"x\"\n").unwrap();
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["project.toml"]);
    }

    #[cfg(unix)]
    #[test]
    fn write_keeps_the_existing_files_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut disk) = disk();
        let p = RelPath::new("project.toml").unwrap();
        let abs = dir.path().join("project.toml");
        std::fs::write(&abs, "old").unwrap();
        std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o664)).unwrap();
        disk.write(&p, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&abs).unwrap(), "new");
        assert_eq!(std::fs::metadata(&abs).unwrap().permissions().mode() & 0o777, 0o664);
    }

    #[cfg(unix)]
    #[test]
    fn write_goes_through_a_symlink_instead_of_replacing_it() {
        let (dir, mut disk) = disk();
        let real = dir.path().join("dotfiles").join("config.toml");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "old").unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        disk.write(&RelPath::new("config.toml").unwrap(), "new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn new_files_take_the_umask_like_a_plain_write_would() {
        use std::os::unix::fs::PermissionsExt;
        let (d, mut disk) = disk();
        std::fs::write(d.path().join("control"), b"").unwrap();
        let control =
            std::fs::metadata(d.path().join("control")).unwrap().permissions().mode() & 0o777;
        let a = RelPath::new("project.toml").unwrap();
        let b = RelPath::new("environments/dev.toml").unwrap();
        disk.write(&a, "").unwrap();
        disk.write_new(&b, "").unwrap();
        for rel in [&a, &b] {
            let mode = std::fs::metadata(disk.abs(rel)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, control, "{rel} should have the umask mode, not the temp file's");
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only_even_when_the_existing_file_is_wider() {
        use std::os::unix::fs::PermissionsExt;
        let (d, mut disk) = disk();
        let secrets = RelPath::new(SECRETS_TOML).unwrap();
        disk.mark_private(secrets.clone());
        disk.write(&secrets, "[dev]\n").unwrap();
        let abs = d.path().join(SECRETS_TOML);
        assert_eq!(std::fs::metadata(&abs).unwrap().permissions().mode() & 0o777, 0o600);
        // A file already leaked wider is tightened, not preserved.
        std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o644)).unwrap();
        disk.write(&secrets, "[qa]\n").unwrap();
        assert_eq!(std::fs::metadata(&abs).unwrap().permissions().mode() & 0o777, 0o600);
        let fresh = local("other.toml");
        disk.mark_private(fresh.clone());
        disk.write_new(&fresh, "").unwrap();
        assert_eq!(
            std::fs::metadata(disk.abs(&fresh)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    /// Task #143: every way a write can create the ignored directory puts
    /// a `.gitignore` of `*` in it, so git skips the whole directory
    /// whatever the repository's own `.gitignore` says.
    #[test]
    fn creating_the_ignored_directory_puts_a_gitignore_in_it() {
        let dir = RelPath::new(LOCAL_DIR).unwrap();
        let gitignore = local(".gitignore");
        type Make = fn(&mut Disk);
        let makers: [(&str, Make); 4] = [
            ("write", |d| d.write(&RelPath::new(STATE_TOML).unwrap(), "").unwrap()),
            ("write_new", |d| d.write_new(&RelPath::new(LAYOUT_TOML).unwrap(), "").unwrap()),
            ("create_dir", |d| d.create_dir(&RelPath::new(TRASH_DIR).unwrap()).unwrap()),
            ("trash", |d| {
                d.write(&RelPath::new("requests/a.toml").unwrap(), "").unwrap();
                d.trash(&RelPath::new("requests/a.toml").unwrap()).unwrap();
            }),
        ];
        for (how, make) in makers {
            let (_d, mut disk) = disk();
            disk.ignore_when_created(dir.clone());
            make(&mut disk);
            let text = disk.peek(&gitignore).unwrap().unwrap_or_default();
            assert!(text.lines().any(|l| l == "*"), "{how}: {text:?}");
        }
    }

    /// Task #143 (the user: "if the user wants to remove the .gitignore
    /// and commit, we should let them"): only creating the directory
    /// writes the file. One already there without it, deleted on purpose
    /// or older than the rule, stays as it is.
    #[test]
    fn an_existing_ignored_directory_never_gets_the_gitignore_back() {
        let (_d, mut disk) = disk();
        disk.ignore_when_created(RelPath::new(LOCAL_DIR).unwrap());
        disk.create_dir(&RelPath::new(LOCAL_DIR).unwrap()).unwrap();
        disk.remove(&local(".gitignore")).unwrap();
        disk.write(&RelPath::new(STATE_TOML).unwrap(), "").unwrap();
        disk.create_dir(&RelPath::new(TRASH_DIR).unwrap()).unwrap();
        assert!(!disk.exists(&local(".gitignore")));
    }

    /// A rename *onto* the ignored path makes only its parent: sealing
    /// first would put a directory where the rename has to land.
    #[test]
    fn a_rename_onto_the_ignored_path_is_not_sealed_first() {
        let (_d, mut disk) = disk();
        disk.ignore_when_created(RelPath::new(LOCAL_DIR).unwrap());
        disk.write(&RelPath::new("old").unwrap(), "x").unwrap();
        disk.rename(&RelPath::new("old").unwrap(), &RelPath::new(LOCAL_DIR).unwrap()).unwrap();
        assert!(disk.is_file(&RelPath::new(LOCAL_DIR).unwrap()));
    }

    /// Only the directory named gets one: a `Disk` that names none (the
    /// config directory's) writes no `.gitignore` anywhere.
    #[test]
    fn no_gitignore_without_an_ignored_directory() {
        let (_d, mut disk) = disk();
        disk.write(&RelPath::new(STATE_TOML).unwrap(), "").unwrap();
        assert!(!disk.exists(&local(".gitignore")));
    }

    /// `create_dir` names the directory it was making, not "the directory
    /// of" it as a file write does.
    #[test]
    fn a_failed_create_dir_says_what_it_was_making() {
        let (d, mut disk) = disk();
        std::fs::write(d.path().join("blocker"), "").unwrap();
        let err = disk.create_dir(&RelPath::new("blocker/sub").unwrap()).unwrap_err();
        assert!(err.to_string().starts_with("could not create blocker/sub:"), "{err}");
    }

    /// A write that can't make the ignored directory names its own file,
    /// as any write that can't make its directory does.
    #[cfg(unix)]
    #[test]
    fn a_failed_seal_names_the_file_being_written() {
        use std::os::unix::fs::PermissionsExt;
        let (d, mut disk) = disk();
        disk.ignore_when_created(RelPath::new(LOCAL_DIR).unwrap());
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let written = disk.write(&RelPath::new(STATE_TOML).unwrap(), "");
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root ignores the mode, so there the write lands: nothing to check.
        let Err(err) = written else { return };
        let want = format!("could not create the directory of {STATE_TOML}:");
        assert!(err.to_string().starts_with(&want), "{err}");
    }

    /// Other directories a write makes get none, ignored directory or not.
    #[test]
    fn only_the_ignored_directory_gets_a_gitignore() {
        let (_d, mut disk) = disk();
        disk.ignore_when_created(RelPath::new(LOCAL_DIR).unwrap());
        disk.write(&RelPath::new("other/x.toml").unwrap(), "").unwrap();
        assert!(!disk.exists(&RelPath::new("other/.gitignore").unwrap()));
        assert!(!disk.exists(&RelPath::new(LOCAL_DIR).unwrap()));
    }

    #[test]
    fn a_refused_write_new_leaves_no_temp_file_behind() {
        let (d, mut disk) = disk();
        let p = RelPath::new("environments/dev.toml").unwrap();
        disk.write_new(&p, "first").unwrap();
        assert!(matches!(disk.write_new(&p, "second"), Err(DiskError::AlreadyExists(_))));
        let names: Vec<String> = std::fs::read_dir(d.path().join("environments"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["dev.toml"]);
        assert_eq!(std::fs::read_to_string(disk.abs(&p)).unwrap(), "first");
    }

    #[test]
    fn write_new_refuses_an_existing_file_and_remove_tolerates_a_missing_one() {
        let (_d, mut disk) = disk();
        let p = RelPath::new("environments/dev.toml").unwrap();
        disk.write_new(&p, "").unwrap();
        assert!(matches!(disk.write_new(&p, ""), Err(DiskError::AlreadyExists(_))));
        disk.remove(&p).unwrap();
        assert!(!disk.exists(&p));
        disk.remove(&p).unwrap();
    }

    #[test]
    fn io_errors_name_the_relative_path_and_the_operation() {
        let (_d, mut disk) = disk();
        let dir = RelPath::new("requests").unwrap();
        disk.create_dir(&dir).unwrap();
        // Reading a directory as a file fails; the error must say where.
        let err = disk.read(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("requests"), "{msg}");
        assert!(msg.contains("read"), "{msg}");
    }

    #[test]
    fn rename_moves_files_and_directories_and_refuses_clobbering() {
        let (_d, mut disk) = disk();
        let a = RelPath::new("requests/main/a.toml").unwrap();
        let b = RelPath::new("requests/auth/a.toml").unwrap();
        disk.write(&a, "x").unwrap();
        disk.rename(&a, &b).unwrap();
        assert!(!disk.exists(&a));
        assert_eq!(disk.read(&b).unwrap().as_deref(), Some("x"));
        assert!(matches!(disk.rename(&a, &b), Err(DiskError::NotFound(_))));
        disk.write(&a, "y").unwrap();
        assert!(matches!(disk.rename(&a, &b), Err(DiskError::AlreadyExists(_))));
        let d1 = RelPath::new("requests/auth").unwrap();
        let d2 = RelPath::new("requests/login").unwrap();
        disk.rename(&d1, &d2).unwrap();
        assert!(disk.is_file(&RelPath::new("requests/login/a.toml").unwrap()));
    }

    #[test]
    fn list_is_sorted_marks_dirs_and_treats_a_missing_dir_as_empty() {
        let (_d, mut disk) = disk();
        disk.write(&RelPath::new("environments/qa.toml").unwrap(), "").unwrap();
        disk.write(&RelPath::new("environments/dev.toml").unwrap(), "").unwrap();
        disk.create_dir(&RelPath::new("environments/sub").unwrap()).unwrap();
        let names: Vec<(String, bool)> = disk
            .list(&RelPath::new("environments").unwrap())
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("dev.toml".to_string(), false),
                ("qa.toml".to_string(), false),
                ("sub".to_string(), true)
            ]
        );
        assert!(disk.list(&RelPath::new("nope").unwrap()).unwrap().is_empty());
    }

    #[test]
    fn walk_files_is_recursive_sorted_and_filtered_by_extension() {
        let (_d, mut disk) = disk();
        disk.write(&RelPath::new("requests/main/b.toml").unwrap(), "").unwrap();
        disk.write(&RelPath::new("requests/main/sub/a.toml").unwrap(), "").unwrap();
        disk.write(&RelPath::new("requests/main/notes.md").unwrap(), "").unwrap();
        disk.write(&RelPath::new("requests/auth/c.toml").unwrap(), "").unwrap();
        let (files, warning) = disk.walk_files(&RelPath::new("requests").unwrap(), "toml");
        let files: Vec<&str> = files.iter().map(|p| p.as_str()).collect();
        assert_eq!(
            files,
            vec!["requests/auth/c.toml", "requests/main/b.toml", "requests/main/sub/a.toml"]
        );
        assert!(warning.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_reports_an_unreadable_subdirectory_and_keeps_going() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut disk) = disk();
        disk.write(&RelPath::new("requests/main/a.toml").unwrap(), "").unwrap();
        disk.write(&RelPath::new("requests/locked/b.toml").unwrap(), "").unwrap();
        let locked = dir.path().join("requests/locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (files, warning) = disk.walk_files(&RelPath::new("requests").unwrap(), "toml");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(files.len(), 1);
        assert!(warning.unwrap().contains("requests/locked"));
    }

    #[test]
    fn changed_compares_the_recorded_stamp_against_disk() {
        let (dir, mut disk) = disk();
        let p = RelPath::new("variables.toml").unwrap();
        assert!(!disk.changed(&p), "nothing recorded yet");
        disk.write(&p, "a = 1\n").unwrap();
        assert!(!disk.changed(&p));
        // An outside write with a different length is seen even on a
        // coarse-mtime filesystem.
        std::fs::write(dir.path().join("variables.toml"), "a = 12\n").unwrap();
        assert!(disk.changed(&p));
        disk.read(&p).unwrap();
        assert!(!disk.changed(&p), "a read re-records");
        std::fs::remove_file(dir.path().join("variables.toml")).unwrap();
        assert!(disk.changed(&p), "absence is a change");
        disk.forget_stamps();
        assert!(!disk.changed(&p));
    }

    #[cfg(unix)]
    #[test]
    fn a_stat_that_fails_for_any_reason_but_absence_stamps_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut disk) = disk();
        let p = RelPath::new("requests/locked/a.toml").unwrap();
        disk.write(&p, "x").unwrap();
        assert_eq!(disk.stamp(&RelPath::new("nope.toml").unwrap()), Stamp::Absent);
        let locked = dir.path().join("requests/locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let seen = disk.stamp(&p);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            seen,
            Stamp::Unreadable,
            "a file that exists but cannot be statted is not the same as a deleted one"
        );
    }

    #[test]
    fn a_write_records_the_stamp_of_the_bytes_it_staged() {
        // The stamp is taken from the temp file before the rename, which
        // preserves mtime and length — so it describes exactly what this
        // write put there, never something that landed a moment later.
        let (_d, mut disk) = disk();
        let p = RelPath::new("requests/main/a.toml").unwrap();
        disk.write(&p, "hello").unwrap();
        assert_eq!(disk.recorded(&p), Some(disk.stamp(&p)));
        assert!(matches!(disk.recorded(&p), Some(Stamp::Present { len: 5, .. })));
        disk.write(&p, "hello again").unwrap();
        assert_eq!(disk.recorded(&p), Some(disk.stamp(&p)));
    }

    #[test]
    fn peek_reads_without_recording_a_stamp() {
        let (dir, mut d) = disk();
        let p = RelPath::new("a.txt").unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        assert_eq!(d.peek(&p).unwrap().as_deref(), Some("hi"));
        // `changed` reads `false` both for "never seen" and for "seen and
        // unchanged", so assert on the table itself: `peek` recorded nothing.
        assert!(!d.stamps.contains_key(&p), "no stamp was recorded");
        d.read(&p).unwrap();
        assert!(d.stamps.contains_key(&p));
        assert!(!d.changed(&p));
        assert_eq!(d.peek(&RelPath::new("nope.txt").unwrap()).unwrap(), None);
    }

    #[test]
    fn trash_moves_under_a_numbered_slot_keeping_the_relative_path() {
        let (_d, mut disk) = disk();
        let p = RelPath::new("requests/main/a.toml").unwrap();
        disk.write(&p, "x").unwrap();
        let t = disk.trash(&p).unwrap();
        assert_eq!(t.original, p);
        assert_eq!(t.slot.as_str(), format!("{TRASH_DIR}/1/requests/main/a.toml"));
        assert!(!disk.exists(&p));
        assert_eq!(disk.read(&t.slot).unwrap().as_deref(), Some("x"));
    }

    #[test]
    fn two_trashes_of_the_same_path_get_distinct_slots_and_dirs_move_whole() {
        let (_d, mut disk) = disk();
        let d = RelPath::new("requests/auth").unwrap();
        disk.write(&d.join("a.toml").unwrap(), "1").unwrap();
        let t1 = disk.trash(&d).unwrap();
        disk.write(&d.join("a.toml").unwrap(), "2").unwrap();
        let t2 = disk.trash(&d).unwrap();
        assert_eq!(t1.slot.as_str(), format!("{TRASH_DIR}/1/requests/auth"));
        assert_eq!(t2.slot.as_str(), format!("{TRASH_DIR}/2/requests/auth"));
        assert_eq!(disk.read(&t1.slot.join("a.toml").unwrap()).unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn restore_puts_it_back_refuses_an_occupied_original_and_retrash_round_trips() {
        let (_d, mut disk) = disk();
        let p = RelPath::new("environments/dev.toml").unwrap();
        disk.write(&p, "x").unwrap();
        let t = disk.trash(&p).unwrap();
        disk.restore(&t).unwrap();
        assert_eq!(disk.read(&p).unwrap().as_deref(), Some("x"));
        assert!(!disk.exists(&t.slot));
        disk.retrash(&t).unwrap();
        assert!(!disk.exists(&p));
        assert!(disk.exists(&t.slot));
        disk.write(&p, "other").unwrap();
        assert!(matches!(disk.restore(&t), Err(DiskError::AlreadyExists(_))));
    }

    /// A `Disk` given no trash folder (the config directory's) has none
    /// to empty, and refuses to trash rather than guess one.
    #[test]
    fn without_a_trash_folder_nothing_is_trashed_or_emptied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(TRASH_DIR)).unwrap();
        let mut disk = Disk::new(dir.path().to_path_buf());
        disk.empty_trash().unwrap();
        assert!(dir.path().join(TRASH_DIR).is_dir());
        let p = RelPath::new("config.toml").unwrap();
        disk.write(&p, "x").unwrap();
        assert!(disk.trash(&p).is_err());
        assert!(disk.is_file(&p), "a refused trash leaves the file");
    }

    #[test]
    fn empty_trash_removes_everything_and_tolerates_a_missing_dir() {
        let (_d, mut disk) = disk();
        disk.empty_trash().unwrap();
        let p = RelPath::new("requests/main/a.toml").unwrap();
        disk.write(&p, "x").unwrap();
        disk.trash(&p).unwrap();
        disk.empty_trash().unwrap();
        assert!(!disk.exists(&RelPath::new(TRASH_DIR).unwrap()));
    }
}
