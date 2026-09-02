//! `.local/trash/`: deletes are renames into a per-project trash so undo
//! is a rename back, whatever the size of what was deleted. Emptied when a
//! project is opened (`ProjectContext::open`), so it only ever backs the
//! current session's undo history.

use std::path::{Path, PathBuf};

/// One trashed path: where it was, and where it sits in the trash now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    pub original: PathBuf,
    pub trashed: PathBuf,
}

/// `root/.local/trash`.
pub fn trash_dir(root: &Path) -> PathBuf {
    root.join(".local").join("trash")
}

/// The next free numbered slot under the trash dir: one more than the
/// largest existing numeric entry, starting at 1. Two deletes of the same
/// path therefore never collide.
fn next_slot(root: &Path) -> std::io::Result<PathBuf> {
    let dir = trash_dir(root);
    let mut max = 0u64;
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for e in entries.filter_map(|e| e.ok()) {
                if let Some(n) = e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) {
                    max = max.max(n);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(dir.join((max + 1).to_string()))
}

/// Renames `path` (a file or a whole directory) into a fresh trash slot,
/// keeping its path relative to `root`. A single same-filesystem rename,
/// so the cost is independent of size. `InvalidInput` for a path that
/// isn't under `root`.
pub fn trash(root: &Path, path: &Path) -> std::io::Result<Trashed> {
    let rel = path.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not inside the project", path.display()),
        )
    })?;
    let slot = next_slot(root)?;
    let dest = slot.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(path, &dest)?;
    Ok(Trashed {
        original: path.to_path_buf(),
        trashed: dest,
    })
}

/// Renames a trashed path back to its original location. Never clobbers:
/// `AlreadyExists` when something now occupies the original path.
pub fn restore(t: &Trashed) -> std::io::Result<()> {
    if t.original.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", t.original.display()),
        ));
    }
    if let Some(parent) = t.original.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&t.trashed, &t.original)
}

/// The redo half of [`restore`]: renames the original back into its
/// recorded trash slot. `AlreadyExists` when the slot is occupied.
pub fn retrash(t: &Trashed) -> std::io::Result<()> {
    if t.trashed.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", t.trashed.display()),
        ));
    }
    if let Some(parent) = t.trashed.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&t.original, &t.trashed)
}

/// Removes the whole trash directory. A missing directory is success.
pub fn empty(root: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(trash_dir(root)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn trash_moves_a_file_under_a_numbered_slot_keeping_its_relative_path() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file = root.join("requests/auth/login.toml");
        write(&file, "url = \"x\"\n");
        let t = trash(root, &file).unwrap();
        assert!(!file.exists());
        assert_eq!(t.original, file);
        assert_eq!(
            t.trashed,
            trash_dir(root).join("1").join("requests/auth/login.toml")
        );
        assert_eq!(
            std::fs::read_to_string(&t.trashed).unwrap(),
            "url = \"x\"\n"
        );
    }

    #[test]
    fn trash_moves_a_directory_whole() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join("requests/auth/a.toml"), "a");
        write(&root.join("requests/auth/deep/b.toml"), "b");
        let t = trash(root, &root.join("requests/auth")).unwrap();
        assert!(!root.join("requests/auth").exists());
        assert!(t.trashed.join("deep/b.toml").is_file());
    }

    #[test]
    fn two_trashes_of_the_same_path_get_distinct_slots() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file = root.join("requests/main/x.toml");
        write(&file, "1");
        let t1 = trash(root, &file).unwrap();
        write(&file, "2");
        let t2 = trash(root, &file).unwrap();
        assert_ne!(t1.trashed, t2.trashed);
        assert_eq!(std::fs::read_to_string(&t1.trashed).unwrap(), "1");
        assert_eq!(std::fs::read_to_string(&t2.trashed).unwrap(), "2");
    }

    #[test]
    fn restore_puts_it_back_and_refuses_an_occupied_original() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file = root.join("requests/main/x.toml");
        write(&file, "1");
        let t = trash(root, &file).unwrap();
        restore(&t).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "1");
        assert!(!t.trashed.exists());

        // Occupied original: refuse, leave both sides alone.
        let t = trash(root, &file).unwrap();
        write(&file, "new");
        let err = restore(&t).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        assert!(t.trashed.is_file());
    }

    #[test]
    fn retrash_round_trips_into_the_recorded_slot() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let file = root.join("requests/main/x.toml");
        write(&file, "1");
        let t = trash(root, &file).unwrap();
        restore(&t).unwrap();
        retrash(&t).unwrap();
        assert!(!file.exists());
        assert!(t.trashed.is_file());
        // Already there: refuse rather than clobber.
        write(&file, "again");
        assert_eq!(
            retrash(&t).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn empty_removes_everything_and_tolerates_a_missing_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        empty(root).unwrap(); // nothing there yet
        let file = root.join("requests/main/x.toml");
        write(&file, "1");
        trash(root, &file).unwrap();
        assert!(trash_dir(root).is_dir());
        empty(root).unwrap();
        assert!(!trash_dir(root).exists());
    }

    #[test]
    fn trash_rejects_a_path_outside_the_project() {
        let dir = tempdir().unwrap();
        let other = tempdir().unwrap();
        let file = other.path().join("x.toml");
        write(&file, "1");
        let err = trash(dir.path(), &file).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(file.is_file());
    }
}
