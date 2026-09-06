//! The only place in the TUI crate that touches files outside a project
//! or the config dir: an export to a user-chosen path, the file picker's
//! directory listing, and the external editor's temp file. Everything
//! else goes through `postui_core::disk::Disk` (see CLAUDE.md).

use std::path::{Path, PathBuf};

/// One raw entry of a listed directory, before the picker's filtering
/// and sorting.
pub struct HostEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: PathBuf,
    pub attributes: u32,
}

/// Writes `bytes` to `path`, creating parent directories. Overwrites.
pub fn write_user_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

/// The entries of `dir`, unsorted, with the platform hidden-attribute bits.
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<HostEntry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        // One unreadable entry (a dangling symlink, a race with a delete)
        // is skipped rather than failing the whole listing.
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        // `metadata` follows symlinks, so a link to a folder lists as one.
        let Ok(meta) = std::fs::metadata(item.path()) else {
            continue;
        };
        let is_dir = meta.is_dir();
        entries.push(HostEntry {
            name,
            is_dir,
            path: item.path(),
            attributes: platform_attributes(&meta),
        });
    }
    Ok(entries)
}

#[cfg(windows)]
fn platform_attributes(meta: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn platform_attributes(_meta: &std::fs::Metadata) -> u32 {
    0
}

/// A kept temp file holding `text`, for `$EDITOR`.
pub fn editor_tempfile(prefix: &str, suffix: &str, text: &str) -> anyhow::Result<PathBuf> {
    use std::io::Write;
    let file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile()?;
    let (mut handle, path) = file.keep()?;
    handle.write_all(text.as_bytes())?;
    handle.flush()?;
    Ok(path)
}

/// The edited text, read back after the editor exits.
pub fn read_tempfile(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

/// Best-effort removal of the temp file.
pub fn remove_tempfile(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_user_file_creates_parents_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/out.txt");
        write_user_file(&p, b"one").unwrap();
        write_user_file(&p, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
    }
}
