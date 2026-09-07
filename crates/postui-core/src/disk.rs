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
    Io {
        op: &'static str,
        path: String,
        source: std::io::Error,
    },
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
        move |source| DiskError::Io {
            op,
            path: path.to_string(),
            source,
        }
    }
}

/// What `changed` compares: mtime and length, or absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    Absent,
    Present { mtime: Option<SystemTime>, len: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// One trashed path: where it was and the slot it sits in now
/// (`.local/trash/<n>/<original>`). Undo of a delete is a rename back.
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
}

impl Disk {
    pub fn new(root: PathBuf) -> Disk {
        Disk {
            root,
            stamps: HashMap::new(),
            private: HashSet::new(),
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
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|e| DiskError::Io {
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
        std::fs::create_dir_all(parent).map_err(DiskError::io("create the directory of", rel))?;
        let private = self.private.contains(rel);
        let tmp = Self::staged(parent, contents, rel, private)?;
        if !private && let Ok(existing) = std::fs::metadata(&target) {
            std::fs::set_permissions(tmp.path(), existing.permissions())
                .map_err(DiskError::io("write", rel))?;
        }
        tmp.persist(&target)
            .map_err(|e| DiskError::io("write", rel)(e.error))?;
        self.record(rel);
        Ok(())
    }

    /// Creates the file with `text`; `AlreadyExists` when it is there.
    /// Atomic like `write`: the content is staged in a sibling temp file
    /// and linked into place only if nothing is there, so a failed write
    /// leaves no partial file and a refused one leaves no residue.
    pub fn write_new(&mut self, rel: &RelPath, text: &str) -> Result<(), DiskError> {
        let path = self.abs(rel);
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(DiskError::io("create the directory of", rel))?;
        let tmp = Self::staged(parent, text.as_bytes(), rel, self.private.contains(rel))?;
        match tmp.persist_noclobber(&path) {
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
        let mut tmp = builder
            .tempfile_in(parent)
            .map_err(DiskError::io("write", rel))?;
        std::io::Write::write_all(&mut tmp, contents).map_err(DiskError::io("write", rel))?;
        tmp.as_file()
            .sync_all()
            .map_err(DiskError::io("write", rel))?;
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
        std::fs::create_dir_all(self.abs(rel)).map_err(DiskError::io("create", rel))?;
        self.record(rel);
        Ok(())
    }

    /// The fresh stamp of `rel`, straight from disk.
    pub fn stamp(&self, rel: &RelPath) -> Stamp {
        match std::fs::metadata(self.abs(rel)) {
            Ok(m) => Stamp::Present {
                mtime: m.modified().ok(),
                len: m.len(),
            },
            Err(_) => Stamp::Absent,
        }
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
            std::fs::create_dir_all(parent)
                .map_err(DiskError::io("create the directory of", to))?;
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

    pub const TRASH_DIR: &'static str = ".local/trash";

    fn trash_root() -> RelPath {
        RelPath::new(Self::TRASH_DIR).expect("constant path")
    }

    /// The next free numbered slot: one more than the largest existing
    /// numeric entry, starting at 1.
    fn next_trash_slot(&mut self) -> Result<RelPath, DiskError> {
        let root = Self::trash_root();
        let max = self
            .list(&root)?
            .iter()
            .filter_map(|e| e.name.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        root.join(&(max + 1).to_string())
    }

    /// Renames a file or directory into a fresh trash slot. One rename,
    /// so the cost is independent of size.
    pub fn trash(&mut self, rel: &RelPath) -> Result<Ticket, DiskError> {
        let slot = self.next_trash_slot()?.join(rel.as_str())?;
        self.rename(rel, &slot)?;
        Ok(Ticket {
            original: rel.clone(),
            slot,
        })
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

    pub fn empty_trash(&mut self) -> Result<(), DiskError> {
        self.remove_dir_all(&Self::trash_root())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk() -> (tempfile::TempDir, Disk) {
        let dir = tempfile::tempdir().unwrap();
        let disk = Disk::new(dir.path().to_path_buf());
        (dir, disk)
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
        assert!(
            !RelPath::new("requests/main2/x.toml")
                .unwrap()
                .starts_with(&p)
        );
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
        assert_eq!(
            std::fs::metadata(&abs).unwrap().permissions().mode() & 0o777,
            0o664
        );
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
        disk.write(&RelPath::new("config.toml").unwrap(), "new")
            .unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn new_files_take_the_umask_like_a_plain_write_would() {
        use std::os::unix::fs::PermissionsExt;
        let (d, mut disk) = disk();
        std::fs::write(d.path().join("control"), b"").unwrap();
        let control = std::fs::metadata(d.path().join("control"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let a = RelPath::new("project.toml").unwrap();
        let b = RelPath::new("environments/dev.toml").unwrap();
        disk.write(&a, "").unwrap();
        disk.write_new(&b, "").unwrap();
        for rel in [&a, &b] {
            let mode = std::fs::metadata(disk.abs(rel))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode, control,
                "{rel} should have the umask mode, not the temp file's"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only_even_when_the_existing_file_is_wider() {
        use std::os::unix::fs::PermissionsExt;
        let (d, mut disk) = disk();
        let secrets = RelPath::new(".local/secrets.toml").unwrap();
        disk.mark_private(secrets.clone());
        disk.write(&secrets, "[dev]\n").unwrap();
        let abs = d.path().join(".local/secrets.toml");
        assert_eq!(
            std::fs::metadata(&abs).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // A file already leaked wider is tightened, not preserved.
        std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o644)).unwrap();
        disk.write(&secrets, "[qa]\n").unwrap();
        assert_eq!(
            std::fs::metadata(&abs).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let fresh = RelPath::new(".local/other.toml").unwrap();
        disk.mark_private(fresh.clone());
        disk.write_new(&fresh, "").unwrap();
        assert_eq!(
            std::fs::metadata(disk.abs(&fresh))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn a_refused_write_new_leaves_no_temp_file_behind() {
        let (d, mut disk) = disk();
        let p = RelPath::new("environments/dev.toml").unwrap();
        disk.write_new(&p, "first").unwrap();
        assert!(matches!(
            disk.write_new(&p, "second"),
            Err(DiskError::AlreadyExists(_))
        ));
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
        assert!(matches!(
            disk.write_new(&p, ""),
            Err(DiskError::AlreadyExists(_))
        ));
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
        assert!(matches!(
            disk.rename(&a, &b),
            Err(DiskError::AlreadyExists(_))
        ));
        let d1 = RelPath::new("requests/auth").unwrap();
        let d2 = RelPath::new("requests/login").unwrap();
        disk.rename(&d1, &d2).unwrap();
        assert!(disk.is_file(&RelPath::new("requests/login/a.toml").unwrap()));
    }

    #[test]
    fn list_is_sorted_marks_dirs_and_treats_a_missing_dir_as_empty() {
        let (_d, mut disk) = disk();
        disk.write(&RelPath::new("environments/qa.toml").unwrap(), "")
            .unwrap();
        disk.write(&RelPath::new("environments/dev.toml").unwrap(), "")
            .unwrap();
        disk.create_dir(&RelPath::new("environments/sub").unwrap())
            .unwrap();
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
        assert!(
            disk.list(&RelPath::new("nope").unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn walk_files_is_recursive_sorted_and_filtered_by_extension() {
        let (_d, mut disk) = disk();
        disk.write(&RelPath::new("requests/main/b.toml").unwrap(), "")
            .unwrap();
        disk.write(&RelPath::new("requests/main/sub/a.toml").unwrap(), "")
            .unwrap();
        disk.write(&RelPath::new("requests/main/notes.md").unwrap(), "")
            .unwrap();
        disk.write(&RelPath::new("requests/auth/c.toml").unwrap(), "")
            .unwrap();
        let (files, warning) = disk.walk_files(&RelPath::new("requests").unwrap(), "toml");
        let files: Vec<&str> = files.iter().map(|p| p.as_str()).collect();
        assert_eq!(
            files,
            vec![
                "requests/auth/c.toml",
                "requests/main/b.toml",
                "requests/main/sub/a.toml"
            ]
        );
        assert!(warning.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_reports_an_unreadable_subdirectory_and_keeps_going() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut disk) = disk();
        disk.write(&RelPath::new("requests/main/a.toml").unwrap(), "")
            .unwrap();
        disk.write(&RelPath::new("requests/locked/b.toml").unwrap(), "")
            .unwrap();
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
        assert_eq!(t.slot.as_str(), ".local/trash/1/requests/main/a.toml");
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
        assert_eq!(t1.slot.as_str(), ".local/trash/1/requests/auth");
        assert_eq!(t2.slot.as_str(), ".local/trash/2/requests/auth");
        assert_eq!(
            disk.read(&t1.slot.join("a.toml").unwrap())
                .unwrap()
                .as_deref(),
            Some("1")
        );
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

    #[test]
    fn empty_trash_removes_everything_and_tolerates_a_missing_dir() {
        let (_d, mut disk) = disk();
        disk.empty_trash().unwrap();
        let p = RelPath::new("requests/main/a.toml").unwrap();
        disk.write(&p, "x").unwrap();
        disk.trash(&p).unwrap();
        disk.empty_trash().unwrap();
        assert!(!disk.exists(&RelPath::new(".local/trash").unwrap()));
    }
}
