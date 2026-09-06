//! What is left of the app's own project-file access: one atomic writer,
//! used by `App::apply_undo_step` to replay a raw `FileStates` step's text
//! onto disk. Everything else now goes through
//! [`postui_core::project::Project`], and this file goes with the last
//! legacy write.

use std::path::Path;

/// Atomic write via temp file + rename in `path`'s own directory, matching
/// `storage::save_request`'s pattern (spec §5: writes atomic + immediate).
pub(crate) fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().expect("path always has a parent");
    std::fs::create_dir_all(parent)?;
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
