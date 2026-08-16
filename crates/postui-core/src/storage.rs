//! File-based storage for HTTP requests: project layout, atomic saves, and
//! slug-addressed CRUD over `root/requests/**/*.toml`.

use crate::model::HttpRequest;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse request: {0}")]
    Parse(String),
    #[error("invalid slug {0:?}")]
    InvalidSlug(String),
    #[error("request not found: {0:?}")]
    NotFound(String),
    #[error("request already exists: {0:?}")]
    AlreadyExists(String),
}

/// Builds a closure that wraps an `io::Error` with the path it concerns,
/// for use as `.map_err(io_err(&path))`.
fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> StorageError + '_ {
    move |source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestListing {
    pub slug: String,
    pub broken: Option<String>,
}

/// The default project directory: `<config dir>/<APP_NAME>/default`.
pub fn default_project_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", crate::APP_NAME)
        .map(|dirs| dirs.config_dir().join("default"))
}

/// Ensures `root/requests/` exists.
pub fn ensure_project(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(requests_dir(root))
}

fn requests_dir(root: &Path) -> PathBuf {
    root.join("requests")
}

pub fn validate_slug(slug: &str) -> Result<(), StorageError> {
    let ok = !slug.is_empty()
        && !slug.starts_with('/')
        && !slug.ends_with('/')
        && slug.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        });
    if ok {
        Ok(())
    } else {
        Err(StorageError::InvalidSlug(slug.to_string()))
    }
}

fn request_path(root: &Path, slug: &str) -> PathBuf {
    requests_dir(root).join(format!("{slug}.toml"))
}

/// Recursively walks `dir`, invoking `f` with each `.toml` file's path.
fn walk_toml_files(dir: &Path, f: &mut dyn FnMut(PathBuf)) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk_toml_files(&path, f)?;
        } else if path.extension().is_some_and(|ext| ext == "toml") {
            f(path);
        }
    }
    Ok(())
}

/// Lists all requests under `root/requests`, sorted by slug. Files that fail
/// to parse are included with `broken` set to a description of the error,
/// rather than causing the whole listing to fail. The second element of the
/// return is the first directory-walk IO error encountered (e.g. a
/// permission-denied subdirectory), if any — the listing itself still
/// contains everything that *was* successfully walked.
pub fn list_requests(root: &Path) -> (Vec<RequestListing>, Option<String>) {
    let base = requests_dir(root);
    let mut out = Vec::new();
    let walk_err = walk_toml_files(&base, &mut |path| {
        let rel = path.strip_prefix(&base).unwrap_or(&path);
        let slug = rel.with_extension("");
        let slug = slug
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let broken = match std::fs::read_to_string(&path) {
            Ok(contents) => match HttpRequest::from_toml_str(&contents) {
                Ok(_) => None,
                Err(e) => Some(e.to_string()),
            },
            Err(e) => Some(e.to_string()),
        };
        out.push(RequestListing { slug, broken });
    })
    .err()
    .map(|e| e.to_string());
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    (out, walk_err)
}

/// Whether `root/requests/<slug>.toml` exists, without attempting to parse
/// it — used for exists-checks that shouldn't reject a present-but-broken
/// file the way [`load_request`] would.
pub fn request_exists(root: &Path, slug: &str) -> bool {
    request_path(root, slug).is_file()
}

pub fn load_request(root: &Path, slug: &str) -> Result<HttpRequest, StorageError> {
    validate_slug(slug)?;
    let path = request_path(root, slug);
    if !path.is_file() {
        return Err(StorageError::NotFound(slug.to_string()));
    }
    let contents = std::fs::read_to_string(&path).map_err(io_err(&path))?;
    HttpRequest::from_toml_str(&contents).map_err(|e| StorageError::Parse(e.to_string()))
}

/// Atomically writes `req` to `root/requests/<slug>.toml` (temp file in the
/// same directory, then rename), creating parent directories as needed.
pub fn save_request(root: &Path, slug: &str, req: &HttpRequest) -> Result<(), StorageError> {
    validate_slug(slug)?;
    let path = request_path(root, slug);
    let parent = path.parent().expect("request path always has a parent");
    std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(io_err(parent))?;
    use std::io::Write;
    tmp.write_all(req.to_toml_string().as_bytes())
        .map_err(io_err(&path))?;
    tmp.persist(&path).map_err(|e| StorageError::Io {
        path: path.clone(),
        source: e.error,
    })?;
    Ok(())
}

pub fn rename_request(root: &Path, from: &str, to: &str) -> Result<(), StorageError> {
    validate_slug(from)?;
    validate_slug(to)?;
    if from == to {
        // Renaming onto the same slug is a no-op, not a conflict: there is
        // nothing to move, so treat it as trivially successful rather than
        // reporting "already exists" against itself.
        return Ok(());
    }
    let from_path = request_path(root, from);
    let to_path = request_path(root, to);
    if !from_path.is_file() {
        return Err(StorageError::NotFound(from.to_string()));
    }
    if to_path.is_file() {
        return Err(StorageError::AlreadyExists(to.to_string()));
    }
    if let Some(parent) = to_path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    std::fs::rename(&from_path, &to_path).map_err(io_err(&from_path))?;
    Ok(())
}

pub fn delete_request(root: &Path, slug: &str) -> Result<(), StorageError> {
    validate_slug(slug)?;
    let path = request_path(root, slug);
    if !path.is_file() {
        return Err(StorageError::NotFound(slug.to_string()));
    }
    std::fs::remove_file(&path).map_err(io_err(&path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn req() -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: "https://x.test".into(),
            substitute_body: false,
            params: Default::default(),
            headers: Default::default(),
            body: None,
        }
    }

    #[test]
    fn save_load_list_roundtrip_with_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "auth/login", &req()).unwrap();
        save_request(dir.path(), "get-user", &req()).unwrap();
        let (listing, walk_err) = list_requests(dir.path());
        assert!(walk_err.is_none());
        let slugs: Vec<&str> = listing.iter().map(|l| l.slug.as_str()).collect();
        assert_eq!(
            slugs,
            ["auth/login", "get-user"],
            "sorted, subdir path as slug"
        );
        assert!(listing.iter().all(|l| l.broken.is_none()));
        assert_eq!(load_request(dir.path(), "auth/login").unwrap(), req());
    }

    #[test]
    fn broken_file_is_listed_with_error_and_load_reports_line() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("requests/bad.toml"),
            "url = \"x\"\nurl = \"dup\"\n",
        )
        .unwrap();
        let (listing, walk_err) = list_requests(dir.path());
        assert!(walk_err.is_none());
        assert_eq!(listing[0].slug, "bad");
        assert!(listing[0].broken.is_some());
        let err = load_request(dir.path(), "bad").unwrap_err().to_string();
        assert!(
            err.contains('2') || err.to_lowercase().contains("duplicate"),
            "error should locate/describe the duplicate key: {err}"
        );
    }

    #[test]
    fn io_error_display_includes_the_offending_path() {
        let err = StorageError::Io {
            path: PathBuf::from("/nonexistent/requests/x.toml"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("/nonexistent/requests/x.toml"),
            "message should include the offending path: {msg}"
        );
    }

    #[test]
    fn load_of_missing_root_reports_not_found_not_io() {
        // Sanity check: a missing file surfaces as NotFound (checked up front),
        // not as a bare Io error lacking path context.
        let dir = tempfile::tempdir().unwrap();
        let err = load_request(dir.path(), "missing").unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[test]
    fn slug_validation_rejects_traversal_and_bad_chars() {
        for bad in [
            "",
            "../etc",
            "a//b",
            "/abs",
            "trailing/",
            "Has Space",
            "UPPER",
            "dot.dot",
        ] {
            assert!(validate_slug(bad).is_err(), "{bad:?} should be invalid");
        }
        for good in ["login", "auth/login", "a-b_c/d0"] {
            assert!(validate_slug(good).is_ok(), "{good:?} should be valid");
        }
    }

    #[test]
    fn save_is_atomic_no_temp_left_and_rename_delete_work() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "a", &req()).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("requests"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_none_or(|x| x != "toml"))
            .collect();
        assert!(leftovers.is_empty(), "no temp files left behind");
        rename_request(dir.path(), "a", "sub/b").unwrap();
        assert!(load_request(dir.path(), "a").is_err());
        assert_eq!(load_request(dir.path(), "sub/b").unwrap(), req());
        delete_request(dir.path(), "sub/b").unwrap();
        assert!(list_requests(dir.path()).0.is_empty());
    }

    #[test]
    fn rename_onto_itself_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "a", &req()).unwrap();
        assert!(rename_request(dir.path(), "a", "a").is_ok());
        assert_eq!(load_request(dir.path(), "a").unwrap(), req());
    }

    #[test]
    fn request_exists_reflects_presence_without_parsing() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        assert!(!request_exists(dir.path(), "a"));
        save_request(dir.path(), "a", &req()).unwrap();
        assert!(request_exists(dir.path(), "a"));
    }

    #[cfg(unix)]
    #[test]
    fn list_requests_surfaces_permission_denied_subdir_as_walk_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        // "aaa" sorts before the "sub" directory, so the walk (which visits
        // entries in sorted order) reaches it before hitting the
        // permission-denied error.
        save_request(dir.path(), "aaa", &req()).unwrap();
        let sub = dir.path().join("requests/sub");
        std::fs::create_dir_all(&sub).unwrap();
        save_request(dir.path(), "sub/inner", &req()).unwrap();

        let original_perms = std::fs::metadata(&sub).unwrap().permissions();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000)).unwrap();

        let (listing, walk_err) = list_requests(dir.path());

        // Restore perms before any assertion so tempdir cleanup can't fail.
        std::fs::set_permissions(&sub, original_perms).unwrap();

        assert!(
            walk_err.is_some(),
            "permission-denied subdir should surface an error"
        );
        assert!(
            listing.iter().any(|l| l.slug == "aaa"),
            "listing should still include everything walked before the error: {listing:?}"
        );
    }
}
