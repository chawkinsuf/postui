//! The one place in the workspace that touches the filesystem for project,
//! local and config files. Every path is relative to a root; every write
//! is atomic; every read and write records a stamp for `changed`.

use std::collections::HashMap;
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
    fn io<'a>(op: &'static str, path: &'a RelPath) -> impl FnOnce(std::io::Error) -> DiskError + 'a {
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

pub struct Disk {
    root: PathBuf,
    stamps: HashMap<RelPath, Stamp>,
}

impl Disk {
    pub fn new(root: PathBuf) -> Disk {
        Disk {
            root,
            stamps: HashMap::new(),
        }
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

    pub fn read_bytes(&mut self, rel: &RelPath) -> Result<Option<Vec<u8>>, DiskError> {
        let result = match std::fs::read(self.abs(rel)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DiskError::io("read", rel)(e)),
        };
        self.record(rel);
        result
    }

    /// Atomic write: sibling temp file, mode copied from the existing
    /// file, written through a symlink, then renamed over. Creates the
    /// parent directory. Records a stamp.
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
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(DiskError::io("write", rel))?;
        std::io::Write::write_all(&mut tmp, contents).map_err(DiskError::io("write", rel))?;
        if let Ok(existing) = std::fs::metadata(&target) {
            std::fs::set_permissions(tmp.path(), existing.permissions())
                .map_err(DiskError::io("write", rel))?;
        }
        tmp.persist(&target)
            .map_err(|e| DiskError::io("write", rel)(e.error))?;
        self.record(rel);
        Ok(())
    }

    /// Creates the file with `text`; `AlreadyExists` when it is there.
    /// Check and create are one atomic step (`create_new`).
    pub fn write_new(&mut self, rel: &RelPath, text: &str) -> Result<(), DiskError> {
        let path = self.abs(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(DiskError::io("create the directory of", rel))?;
        }
        let mut f = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(DiskError::AlreadyExists(rel.to_string()));
            }
            Err(e) => return Err(DiskError::io("create", rel)(e)),
        };
        std::io::Write::write_all(&mut f, text.as_bytes()).map_err(DiskError::io("write", rel))?;
        self.record(rel);
        Ok(())
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
}
