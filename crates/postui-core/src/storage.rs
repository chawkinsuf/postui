//! File-based storage for HTTP requests: project layout, atomic saves, and
//! slug-addressed CRUD over `root/requests/**/*.toml`.

use crate::model::{HttpRequest, Method};
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
    /// The request's HTTP method, parsed for free alongside `broken`
    /// detection. `None` exactly when `broken` is `Some` — a file that
    /// failed to parse has no method to show.
    pub method: Option<Method>,
    /// The request's display name, parsed in the same pass. `None` for
    /// legacy files without one (and for broken files) — display falls
    /// back to the slug leaf.
    pub name: Option<String>,
}

/// The default project directory: `<config dir>/default`.
pub fn default_project_dir() -> Option<PathBuf> {
    crate::config_dir().map(|dir| dir.join("default"))
}

/// Ensures `root/requests/` exists and that the project has at least one
/// space. When `root` is already a project (`project.toml` present) and
/// parses cleanly, a fresh one gets `main` seeded both on disk and in
/// `project.toml`'s `spaces` list — including the case where `main`
/// already exists on disk (e.g. seeded earlier while this was still a bare
/// directory) but was never recorded, which is simply written into the
/// list rather than re-created. When `root` is a bare directory (not yet a
/// project) — or an existing `project.toml` fails to parse — only the
/// `requests/main/` directory is materialised, never touching
/// `project.toml`: a bare directory must never gain one behind the user's
/// back, ahead of the "create a project here?" consent modal, and an
/// unreadable one must never be overwritten by seeding, matching
/// `ProjectContext::open`'s "never fail to open outright" policy of
/// degrading a broken `project.toml` to a warning rather than a hard
/// error. `list_spaces` still reports `main` as an unlisted directory
/// either way.
pub fn ensure_project(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(requests_dir(root))?;
    let Ok(meta) = crate::project::load_meta(root) else {
        if crate::project::list_spaces(root, &crate::project::ProjectMeta::default()).is_empty() {
            std::fs::create_dir_all(crate::project::space_dir(
                root,
                crate::project::DEFAULT_SPACE,
            ))?;
        }
        return Ok(());
    };
    if crate::project::is_project(root) {
        if meta.spaces.is_empty() {
            let spaces = crate::project::list_spaces(root, &meta);
            if spaces.is_empty() {
                crate::project::create_space(root, crate::project::DEFAULT_SPACE)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
            } else {
                crate::project::write_spaces(root, &spaces)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
            }
        }
    } else if crate::project::list_spaces(root, &meta).is_empty() {
        std::fs::create_dir_all(crate::project::space_dir(
            root,
            crate::project::DEFAULT_SPACE,
        ))?;
    }
    Ok(())
}

/// `root/requests/`.
pub fn requests_dir(root: &Path) -> PathBuf {
    root.join("requests")
}

/// The space a slug lives in: its first segment. `None` for a bare
/// single-segment slug (a loose top-level file).
pub fn space_of(slug: &str) -> Option<&str> {
    slug.split_once('/').map(|(space, _)| space)
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

/// `root/requests/<slug>.toml` — the on-disk path for a request slug.
/// Exposed so callers outside this module (postui's undo history, which
/// records raw file states) can name a request's file without duplicating
/// the layout rule.
pub fn request_path(root: &Path, slug: &str) -> PathBuf {
    requests_dir(root).join(format!("{slug}.toml"))
}

/// Inverse of [`request_path`]: the slug `path` names, when it sits under
/// `root/requests/` with a `.toml` extension. `None` for anything else
/// (a path from a different kind of step, e.g. an environment file) —
/// exposed for the same undo-history caller as `request_path`, to follow
/// a request's file to its new slug after a move.
pub fn slug_for_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(requests_dir(root)).ok()?;
    let rel = rel.to_str()?.strip_suffix(".toml")?;
    Some(rel.replace(std::path::MAIN_SEPARATOR, "/"))
}

/// Derives a safe filename segment from a free-form display name:
/// lowercase, `[a-z0-9_-]` kept, every other char collapsed to a single
/// `-`, trimmed at both ends, `"request"` when nothing survives. The
/// result always passes [`validate_slug`] as a single segment — the user
/// never sees or types it.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() || validate_slug(&out).is_err() {
        "request".to_string()
    } else {
        out
    }
}

/// Splits a typed display path into `(folder_slug_prefix, leaf_display)`:
/// `/` still means folders, each folder segment is slugified, and the
/// last segment — trimmed — is the free-form display name. `None` when
/// the leaf is empty.
pub fn split_display_path(input: &str) -> Option<(String, String)> {
    let (folders, leaf) = match input.rsplit_once('/') {
        Some((f, l)) => (f, l),
        None => ("", input),
    };
    let leaf = leaf.trim();
    if leaf.is_empty() {
        return None;
    }
    let folder = folders
        .split('/')
        .filter(|s| !s.trim().is_empty())
        .map(slugify)
        .collect::<Vec<_>>()
        .join("/");
    Some((folder, leaf.to_string()))
}

/// Whether a request in `folder` already answers to `leaf_display`
/// (case-insensitive string equality; a legacy file's display name is its
/// slug leaf). `exclude_slug` is the renaming request itself.
pub fn sibling_name_taken(
    root: &Path,
    folder: &str,
    leaf_display: &str,
    exclude_slug: Option<&str>,
) -> bool {
    let wanted = leaf_display.to_lowercase();
    let (listing, _) = list_requests(root);
    listing.iter().any(|l| {
        if exclude_slug == Some(l.slug.as_str()) {
            return false;
        }
        let (dir, leaf) = match l.slug.rsplit_once('/') {
            Some((d, f)) => (d, f),
            None => ("", l.slug.as_str()),
        };
        if dir != folder {
            return false;
        }
        let display = l.name.as_deref().unwrap_or(leaf);
        display.to_lowercase() == wanted
    })
}

/// Creates a request from a typed display path ("Folder/My Request!"):
/// validates the leaf, rejects a sibling with the same display name,
/// derives + dedupes the slug, and saves `req` with `name` set. Returns
/// `(slug, leaf_display)`.
pub fn create_request_named(
    root: &Path,
    display_path: &str,
    mut req: HttpRequest,
) -> Result<(String, String), StorageError> {
    let Some((folder, leaf)) = split_display_path(display_path) else {
        return Err(StorageError::InvalidSlug(display_path.to_string()));
    };
    if sibling_name_taken(root, &folder, &leaf, None) {
        return Err(StorageError::AlreadyExists(leaf));
    }
    let slug = unique_slug(root, &folder, &leaf, None);
    req.name = Some(leaf.clone());
    save_request(root, &slug, &req)?;
    Ok((slug, leaf))
}

/// Renames a request to a new typed display path: validates the leaf,
/// rejects a sibling with the same display name, derives + dedupes the
/// new slug (the request's own file never counts as a collision), moves
/// the file when the slug changed, and rewrites `name` when the file
/// parses (a broken file just moves — its display falls back to the new
/// slug leaf). Returns the new `(slug, leaf_display)`.
pub fn rename_request_named(
    root: &Path,
    from_slug: &str,
    display_path: &str,
) -> Result<(String, String), StorageError> {
    validate_slug(from_slug)?;
    if !request_path(root, from_slug).is_file() {
        return Err(StorageError::NotFound(from_slug.to_string()));
    }
    let Some((folder, leaf)) = split_display_path(display_path) else {
        return Err(StorageError::InvalidSlug(display_path.to_string()));
    };
    if sibling_name_taken(root, &folder, &leaf, Some(from_slug)) {
        return Err(StorageError::AlreadyExists(leaf));
    }
    let slug = unique_slug(root, &folder, &leaf, Some(from_slug));
    rename_request(root, from_slug, &slug)?;
    if let Ok(mut req) = load_request(root, &slug)
        && req.name.as_deref() != Some(leaf.as_str())
    {
        req.name = Some(leaf.clone());
        save_request(root, &slug, &req)?;
    }
    Ok((slug, leaf))
}

/// The slug a request named `leaf_display` in `folder` should live at:
/// `folder/slugify(leaf)`, with `-2`, `-3`, … appended while the file
/// already exists. `exclude` is the renaming request's own slug — its
/// file doesn't count as a collision.
pub fn unique_slug(root: &Path, folder: &str, leaf_display: &str, exclude: Option<&str>) -> String {
    let base = if folder.is_empty() {
        slugify(leaf_display)
    } else {
        format!("{folder}/{}", slugify(leaf_display))
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while exclude != Some(candidate.as_str()) && request_exists(root, &candidate) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
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
/// return is a `; `-joined warning, if any: the first directory-walk IO
/// error encountered (e.g. a permission-denied subdirectory) followed by a
/// line per file found directly under `requests/` — those belong to no
/// space, so they are left where they are and left out of the listing. The
/// listing itself still contains everything that *was* successfully walked.
pub fn list_requests(root: &Path) -> (Vec<RequestListing>, Option<String>) {
    let base = requests_dir(root);
    let mut out = Vec::new();
    // Files sitting directly under `requests/` belong to no space. They are
    // never moved for the user (no migration); they're skipped and named in
    // the warning so the fix is theirs to make.
    let mut loose: Vec<String> = Vec::new();
    let walk_err = walk_toml_files(&base, &mut |path| {
        let rel = path.strip_prefix(&base).unwrap_or(&path);
        let slug = rel.with_extension("");
        let slug = slug
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        match space_of(&slug) {
            None => {
                loose.push(format!(
                    "requests/{} is not in a space (move it into a space directory)",
                    rel.display()
                ));
                return;
            }
            // A top-level directory whose name can't be a space name is no
            // space at all: nothing roots a sidebar there, so the requests
            // under it would be invisible without a word. Same treatment as
            // a loose file — named, skipped, never migrated.
            Some(space) if validate_slug(space).is_err() => {
                loose.push(format!(
                    "requests/{} is not in a valid space (space names are a-z 0-9 - _)",
                    rel.display()
                ));
                return;
            }
            Some(_) => {}
        }
        let (method, name, broken) = match std::fs::read_to_string(&path) {
            Ok(contents) => match HttpRequest::from_toml_str(&contents) {
                Ok(req) => (Some(req.method), req.name, None),
                Err(e) => (None, None, Some(e.to_string())),
            },
            Err(e) => (None, None, Some(e.to_string())),
        };
        out.push(RequestListing {
            slug,
            broken,
            method,
            name,
        });
    })
    .err()
    .map(|e| e.to_string());
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    let mut warnings: Vec<String> = walk_err.into_iter().collect();
    warnings.extend(loose);
    let warning = if warnings.is_empty() {
        None
    } else {
        Some(warnings.join("; "))
    };
    (out, warning)
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

/// Moves a request into `space`, keeping its sub-path (`main/x/y` →
/// `auth/x/y`), suffixing `-2`, `-3`, … while the target slug exists.
/// Returns the new slug.
pub fn move_request_to_space(root: &Path, slug: &str, space: &str) -> Result<String, StorageError> {
    validate_slug(slug)?;
    validate_slug(space)?;
    let Some((_, rest)) = slug.split_once('/') else {
        return Err(StorageError::InvalidSlug(slug.to_string()));
    };
    if !request_path(root, slug).is_file() {
        return Err(StorageError::NotFound(slug.to_string()));
    }
    let base = format!("{space}/{rest}");
    let mut candidate = base.clone();
    let mut n = 2;
    while request_exists(root, &candidate) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    rename_request(root, slug, &candidate)?;
    Ok(candidate)
}

/// `move_request_to_space` over every request in `from`. Stops at the
/// first failure; the pairs already moved stay moved and are returned
/// alongside the error.
pub fn move_all_requests(
    root: &Path,
    from: &str,
    to: &str,
) -> (Vec<(String, String)>, Option<StorageError>) {
    let (listing, _) = list_requests(root);
    let mut moved = Vec::new();
    for l in listing.iter().filter(|l| space_of(&l.slug) == Some(from)) {
        match move_request_to_space(root, &l.slug, to) {
            Ok(new) => moved.push((l.slug.clone(), new)),
            Err(e) => return (moved, Some(e)),
        }
    }
    (moved, None)
}

/// Moves `root/requests/<slug>.toml` into the trash (see `crate::trash`)
/// and returns the record undo needs to bring it back.
pub fn delete_request(root: &Path, slug: &str) -> Result<crate::trash::Trashed, StorageError> {
    validate_slug(slug)?;
    let path = request_path(root, slug);
    if !path.is_file() {
        return Err(StorageError::NotFound(slug.to_string()));
    }
    crate::trash::trash(root, &path).map_err(io_err(&path))
}

/// Copies `root/requests/<slug>.toml` to `<slug>-copy.toml` (then `-copy-2`, `-copy-3`, …
/// on collision), byte-identical content (raw file copy — round-tripping through parse
/// would reorder). Returns the new slug.
pub fn duplicate_request(root: &Path, slug: &str) -> Result<String, StorageError> {
    validate_slug(slug)?;
    let source_path = request_path(root, slug);
    if !source_path.is_file() {
        return Err(StorageError::NotFound(slug.to_string()));
    }

    // Read the original file as bytes
    let contents = std::fs::read(&source_path).map_err(io_err(&source_path))?;

    // A parsable source gets a proper display name — "<name> copy",
    // "<name> copy 2", … — with the copy's slug derived from it. A broken
    // file falls through to the byte-copy path below (nothing to name).
    if let Ok(req) = std::str::from_utf8(&contents)
        .map_err(|e| e.to_string())
        .and_then(|s| HttpRequest::from_toml_str(s).map_err(|e| e.to_string()))
    {
        let (folder, leaf) = match slug.rsplit_once('/') {
            Some((d, f)) => (d, f),
            None => ("", slug),
        };
        let base = req.name.clone().unwrap_or_else(|| leaf.to_string());
        let mut copy_name = format!("{base} copy");
        let mut n = 2;
        while sibling_name_taken(root, folder, &copy_name, None) {
            copy_name = format!("{base} copy {n}");
            n += 1;
        }
        let mut copy = req;
        copy.name = Some(copy_name);
        let new_slug = unique_slug(root, folder, copy.name.as_deref().unwrap(), None);
        save_request(root, &new_slug, &copy)?;
        return Ok(new_slug);
    }

    // Find the next available copy slug
    let mut new_slug = format!("{slug}-copy");
    let mut counter = 2;
    while request_exists(root, &new_slug) {
        new_slug = format!("{slug}-copy-{counter}");
        counter += 1;
    }

    // Validate the new slug before proceeding
    validate_slug(&new_slug)?;

    // Write the bytes to the new location using the atomic NamedTempFile+persist pattern
    let new_path = request_path(root, &new_slug);
    let parent = new_path.parent().expect("request path always has a parent");
    std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(io_err(parent))?;
    use std::io::Write;
    tmp.write_all(&contents).map_err(io_err(&new_path))?;
    tmp.persist(&new_path).map_err(|e| StorageError::Io {
        path: new_path.clone(),
        source: e.error,
    })?;

    Ok(new_slug)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn req() -> HttpRequest {
        HttpRequest {
            name: None,
            method: Method::Get,
            url: "https://x.test".into(),
            substitute_body: false,
            insecure: false,
            params: Default::default(),
            headers: Default::default(),
            variables: Default::default(),
            body: None,
        }
    }

    #[test]
    fn slugify_maps_free_form_names_to_safe_segments() {
        assert_eq!(slugify("Get user by ID!"), "get-user-by-id");
        assert_eq!(slugify("  spaced   out  "), "spaced-out");
        assert_eq!(slugify("keep_under-scores"), "keep_under-scores");
        assert_eq!(slugify("???"), "request", "all-unsafe falls back");
        assert_eq!(slugify(""), "request");
        // Whatever comes out must be a valid single path segment.
        for name in ["Ünïcode Näme", "a.b.c", "..", "-x-", "MiXeD Case"] {
            let s = slugify(name);
            assert!(
                validate_slug(&s).is_ok() && !s.contains('/'),
                "{name:?} -> {s:?} must validate"
            );
        }
    }

    #[test]
    fn split_display_path_slugifies_folders_and_keeps_the_leaf_verbatim() {
        assert_eq!(
            split_display_path("API Auth/Get User"),
            Some(("api-auth".into(), "Get User".into()))
        );
        assert_eq!(
            split_display_path("Get User"),
            Some(("".into(), "Get User".into()))
        );
        assert_eq!(
            split_display_path("a/b/  Leaf Name  "),
            Some(("a/b".into(), "Leaf Name".into()))
        );
        assert_eq!(split_display_path("folder/   "), None, "empty leaf");
        assert_eq!(split_display_path(""), None);
    }

    #[test]
    fn create_request_named_derives_dedupes_and_stores_the_display_name() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        let (slug, leaf) = create_request_named(dir.path(), "main/My Request!", req()).unwrap();
        assert_eq!(
            (slug.as_str(), leaf.as_str()),
            ("main/my-request", "My Request!")
        );
        let loaded = load_request(dir.path(), "main/my-request").unwrap();
        assert_eq!(loaded.name.as_deref(), Some("My Request!"));

        // A *different* display name that slugifies identically dedupes.
        let (slug2, _) = create_request_named(dir.path(), "main/My Request?", req()).unwrap();
        assert_eq!(slug2, "main/my-request-2");

        // The *same* display name (case-insensitive) is rejected.
        let err = create_request_named(dir.path(), "main/my request!", req()).unwrap_err();
        assert!(matches!(err, StorageError::AlreadyExists(_)), "{err:?}");

        // Empty leaf is invalid.
        assert!(create_request_named(dir.path(), "folder/  ", req()).is_err());
    }

    #[test]
    fn listing_carries_display_names_and_legacy_leafs_count_as_taken() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        create_request_named(dir.path(), "main/Fancy Name", req()).unwrap();
        save_request(dir.path(), "main/legacy-file", &req()).unwrap(); // no name field

        let (listing, err) = list_requests(dir.path());
        assert!(err.is_none());
        let by_slug = |s: &str| listing.iter().find(|l| l.slug == s).unwrap();
        assert_eq!(
            by_slug("main/fancy-name").name.as_deref(),
            Some("Fancy Name")
        );
        assert_eq!(by_slug("main/legacy-file").name, None);

        // A legacy file's slug leaf is its display name for uniqueness
        // (case-insensitive string equality — "Legacy File" is a
        // different name and stays allowed).
        assert!(sibling_name_taken(dir.path(), "main", "LEGACY-FILE", None));
        assert!(!sibling_name_taken(dir.path(), "main", "Legacy File", None));
        assert!(sibling_name_taken(dir.path(), "main", "fancy name", None));
        assert!(!sibling_name_taken(dir.path(), "main", "Unrelated", None));
        // Excluding a slug frees its own name (rename-onto-itself).
        assert!(!sibling_name_taken(
            dir.path(),
            "main",
            "Fancy Name",
            Some("main/fancy-name")
        ));
    }

    #[test]
    fn rename_request_named_regenerates_slug_and_rewrites_name() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        create_request_named(dir.path(), "main/Get User", req()).unwrap();

        let (slug, leaf) =
            rename_request_named(dir.path(), "main/get-user", "main/Get User v2").unwrap();
        assert_eq!(
            (slug.as_str(), leaf.as_str()),
            ("main/get-user-v2", "Get User v2")
        );
        assert!(!request_exists(dir.path(), "main/get-user"));
        let loaded = load_request(dir.path(), "main/get-user-v2").unwrap();
        assert_eq!(loaded.name.as_deref(), Some("Get User v2"));

        // Renaming onto its own current name is a no-op Ok.
        let (slug, _) =
            rename_request_named(dir.path(), "main/get-user-v2", "main/Get User v2").unwrap();
        assert_eq!(slug, "main/get-user-v2");

        // A slug collision with a *different* request dedupes.
        create_request_named(dir.path(), "main/Other", req()).unwrap();
        let (slug, _) =
            rename_request_named(dir.path(), "main/other", "main/Get User v2!").unwrap();
        assert_eq!(slug, "main/get-user-v2-2");

        // But the same display name as a sibling errors... note the names
        // above differ ("Get User v2" vs "Get User v2!").
        let err = rename_request_named(dir.path(), "main/get-user-v2-2", "main/get user V2");
        assert!(
            matches!(err, Err(StorageError::AlreadyExists(_))),
            "{err:?}"
        );
    }

    #[test]
    fn rename_request_named_moves_broken_files_without_parsing() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        let path = requests_dir(dir.path()).join("broken.toml");
        std::fs::write(&path, "not = valid = toml").unwrap();
        let (slug, _) = rename_request_named(dir.path(), "broken", "Still Broken").unwrap();
        assert_eq!(slug, "still-broken");
        assert!(requests_dir(dir.path()).join("still-broken.toml").is_file());
    }

    #[test]
    fn duplicate_names_the_copy_and_derives_its_slug() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        create_request_named(dir.path(), "main/Get User", req()).unwrap();

        let copy = duplicate_request(dir.path(), "main/get-user").unwrap();
        assert_eq!(copy, "main/get-user-copy");
        let loaded = load_request(dir.path(), &copy).unwrap();
        assert_eq!(loaded.name.as_deref(), Some("Get User copy"));

        let copy2 = duplicate_request(dir.path(), "main/get-user").unwrap();
        let loaded2 = load_request(dir.path(), &copy2).unwrap();
        assert_eq!(loaded2.name.as_deref(), Some("Get User copy 2"));
    }

    #[test]
    fn unique_slug_dedupes_against_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        assert_eq!(
            unique_slug(dir.path(), "", "My Request", None),
            "my-request"
        );
        save_request(dir.path(), "my-request", &req()).unwrap();
        assert_eq!(
            unique_slug(dir.path(), "", "My Request", None),
            "my-request-2"
        );
        // The renaming request's own slug is not a collision.
        assert_eq!(
            unique_slug(dir.path(), "", "My Request", Some("my-request")),
            "my-request"
        );
        // Folder prefix carries through.
        assert_eq!(
            unique_slug(dir.path(), "auth", "My Request", None),
            "auth/my-request"
        );
    }

    #[test]
    fn save_load_list_roundtrip_with_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "auth/login", &req()).unwrap();
        save_request(dir.path(), "main/get-user", &req()).unwrap();
        let (listing, walk_err) = list_requests(dir.path());
        assert!(walk_err.is_none());
        let slugs: Vec<&str> = listing.iter().map(|l| l.slug.as_str()).collect();
        assert_eq!(
            slugs,
            ["auth/login", "main/get-user"],
            "sorted, subdir path as slug"
        );
        assert!(listing.iter().all(|l| l.broken.is_none()));
        assert!(
            listing.iter().all(|l| l.method == Some(Method::Get)),
            "method parsed alongside broken detection for valid files"
        );
        assert_eq!(load_request(dir.path(), "auth/login").unwrap(), req());
    }

    #[test]
    fn broken_file_is_listed_with_error_and_load_reports_line() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("requests/main/bad.toml"),
            "url = \"x\"\nurl = \"dup\"\n",
        )
        .unwrap();
        let (listing, walk_err) = list_requests(dir.path());
        assert!(walk_err.is_none());
        assert_eq!(listing[0].slug, "main/bad");
        assert!(listing[0].broken.is_some());
        assert_eq!(
            listing[0].method, None,
            "a broken file has no parsed method"
        );
        let err = load_request(dir.path(), "main/bad")
            .unwrap_err()
            .to_string();
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
            .filter(|e| !e.path().is_dir() && e.path().extension().is_none_or(|x| x != "toml"))
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
        save_request(dir.path(), "main/aaa", &req()).unwrap();
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
            listing.iter().any(|l| l.slug == "main/aaa"),
            "listing should still include everything walked before the error: {listing:?}"
        );
    }

    #[test]
    fn list_requests_skips_loose_top_level_files_and_reports_them() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "main/ok", &req()).unwrap();
        std::fs::write(dir.path().join("requests/loose.toml"), "url = \"x\"\n").unwrap();
        let (listing, warn) = list_requests(dir.path());
        assert_eq!(
            listing.iter().map(|l| l.slug.as_str()).collect::<Vec<_>>(),
            ["main/ok"]
        );
        let warn = warn.expect("loose file reported");
        assert!(warn.contains("loose.toml"), "{warn}");
        assert!(warn.contains("not in a space"), "{warn}");
    }

    #[test]
    fn list_requests_skips_requests_under_an_invalid_space_dir_and_reports_them() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "main/ok", &req()).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/Auth")).unwrap();
        std::fs::write(dir.path().join("requests/Auth/login.toml"), "url = \"x\"\n").unwrap();
        let (listing, warn) = list_requests(dir.path());
        assert_eq!(
            listing.iter().map(|l| l.slug.as_str()).collect::<Vec<_>>(),
            ["main/ok"],
            "a request under a non-space directory is never listed"
        );
        let warn = warn.expect("invalid space dir reported");
        assert!(warn.contains("Auth"), "{warn}");
        assert!(
            warn.contains("is not in a valid space (space names are a-z 0-9 - _)"),
            "{warn}"
        );
    }

    #[test]
    fn duplicate_request_preserves_content_and_names_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "users/list", &req()).unwrap();

        let new_slug = duplicate_request(dir.path(), "users/list").unwrap();

        assert_eq!(new_slug, "users/list-copy");
        let copy = load_request(dir.path(), &new_slug).unwrap();
        // Everything but the display name is identical to the source.
        assert_eq!(copy.name.as_deref(), Some("list copy"));
        let mut nameless = copy;
        nameless.name = None;
        assert_eq!(nameless, req());
    }

    #[test]
    fn duplicate_request_yields_copy_2_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "users/list", &req()).unwrap();

        let first_copy = duplicate_request(dir.path(), "users/list").unwrap();
        assert_eq!(first_copy, "users/list-copy");

        let second_copy = duplicate_request(dir.path(), "users/list").unwrap();
        assert_eq!(second_copy, "users/list-copy-2");
        assert_eq!(
            load_request(dir.path(), &second_copy)
                .unwrap()
                .name
                .as_deref(),
            Some("list copy 2")
        );
    }

    #[test]
    fn duplicate_of_a_broken_file_falls_back_to_a_byte_copy() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        let path = requests_dir(dir.path()).join("broken.toml");
        std::fs::write(&path, "not = valid = toml").unwrap();
        let new_slug = duplicate_request(dir.path(), "broken").unwrap();
        assert_eq!(new_slug, "broken-copy");
        let copy = std::fs::read(requests_dir(dir.path()).join("broken-copy.toml")).unwrap();
        assert_eq!(copy, b"not = valid = toml");
    }

    #[test]
    fn duplicate_request_copies_broken_toml_without_parsing() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        let broken_content = "url = \"x\"\nurl = \"dup\"\n";
        std::fs::write(dir.path().join("requests/broken.toml"), broken_content).unwrap();

        let new_slug = duplicate_request(dir.path(), "broken").unwrap();

        assert_eq!(new_slug, "broken-copy");
        let copy_bytes = std::fs::read(dir.path().join("requests/broken-copy.toml")).unwrap();
        assert_eq!(
            copy_bytes,
            broken_content.as_bytes(),
            "copy should preserve broken content without parsing"
        );
    }

    #[test]
    fn duplicate_request_returns_not_found_for_missing_slug() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();

        let err = duplicate_request(dir.path(), "missing").unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[test]
    fn duplicate_request_generated_slug_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "auth/login", &req()).unwrap();

        let new_slug = duplicate_request(dir.path(), "auth/login").unwrap();

        assert!(
            validate_slug(&new_slug).is_ok(),
            "generated slug should be valid"
        );
    }

    #[test]
    fn ensure_project_seeds_the_main_space_once() {
        let dir = tempfile::tempdir().unwrap();
        crate::project::init_project(dir.path(), None).unwrap();
        ensure_project(dir.path()).unwrap();
        assert!(dir.path().join("requests/main").is_dir());
        let meta = crate::project::load_meta(dir.path()).unwrap();
        assert_eq!(meta.spaces, ["main"]);
        // A project that already has a space is left alone.
        crate::project::create_space(dir.path(), "auth").unwrap();
        std::fs::remove_dir(dir.path().join("requests/main")).unwrap();
        crate::project::write_spaces(dir.path(), &["auth".into()]).unwrap();
        ensure_project(dir.path()).unwrap();
        assert_eq!(
            crate::project::load_meta(dir.path()).unwrap().spaces,
            ["auth"]
        );
        assert!(!dir.path().join("requests/main").exists());
    }

    #[test]
    fn ensure_project_on_a_bare_dir_makes_main_but_no_project_toml() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        assert!(dir.path().join("requests/main").is_dir());
        assert!(!dir.path().join("project.toml").exists());
        assert_eq!(
            crate::project::list_spaces(dir.path(), &crate::project::ProjectMeta::default()),
            ["main"]
        );
    }

    #[test]
    fn ensure_project_materialises_an_existing_unlisted_space_into_the_list() {
        let dir = tempfile::tempdir().unwrap();
        crate::project::init_project(dir.path(), None).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/auth")).unwrap();
        ensure_project(dir.path()).unwrap();
        assert_eq!(
            crate::project::load_meta(dir.path()).unwrap().spaces,
            ["auth"]
        );
    }

    #[test]
    fn space_of_is_the_first_segment_of_a_nested_slug() {
        assert_eq!(space_of("auth/login"), Some("auth"));
        assert_eq!(space_of("auth/tokens/refresh"), Some("auth"));
        assert_eq!(space_of("loose"), None);
    }

    #[test]
    fn move_request_to_space_keeps_the_sub_path_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        crate::project::create_space(dir.path(), "auth").unwrap();
        save_request(dir.path(), "main/tokens/refresh", &req()).unwrap();
        let new = move_request_to_space(dir.path(), "main/tokens/refresh", "auth").unwrap();
        assert_eq!(new, "auth/tokens/refresh");
        assert!(request_exists(dir.path(), "auth/tokens/refresh"));
        assert!(!request_exists(dir.path(), "main/tokens/refresh"));

        save_request(dir.path(), "main/tokens/refresh", &req()).unwrap();
        let new = move_request_to_space(dir.path(), "main/tokens/refresh", "auth").unwrap();
        assert_eq!(new, "auth/tokens/refresh-2");

        assert!(matches!(
            move_request_to_space(dir.path(), "loose", "auth"),
            Err(StorageError::InvalidSlug(_))
        ));
        assert!(matches!(
            move_request_to_space(dir.path(), "main/nope", "auth"),
            Err(StorageError::NotFound(_))
        ));
    }

    #[test]
    fn move_all_requests_moves_every_request_of_a_space() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        crate::project::create_space(dir.path(), "auth").unwrap();
        save_request(dir.path(), "main/a", &req()).unwrap();
        save_request(dir.path(), "main/deep/b", &req()).unwrap();
        save_request(dir.path(), "auth/keep", &req()).unwrap();
        let (moved, err) = move_all_requests(dir.path(), "main", "auth");
        assert!(err.is_none());
        let mut moved = moved;
        moved.sort();
        assert_eq!(
            moved,
            vec![
                ("main/a".to_string(), "auth/a".to_string()),
                ("main/deep/b".to_string(), "auth/deep/b".to_string()),
            ]
        );
        let (listing, _) = list_requests(dir.path());
        let slugs: Vec<&str> = listing.iter().map(|l| l.slug.as_str()).collect();
        assert_eq!(slugs, ["auth/a", "auth/deep/b", "auth/keep"]);
    }

    #[test]
    fn delete_request_moves_the_file_to_the_trash() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        save_request(dir.path(), "main/a", &req()).unwrap();
        let t = delete_request(dir.path(), "main/a").unwrap();
        assert!(!request_exists(dir.path(), "main/a"));
        assert_eq!(t.original, request_path(dir.path(), "main/a"));
        assert!(t.trashed.is_file());
        assert!(matches!(
            delete_request(dir.path(), "main/a"),
            Err(StorageError::NotFound(_))
        ));
    }

    #[test]
    fn ensure_project_leaves_an_unparseable_project_toml_alone() {
        let dir = tempfile::tempdir().unwrap();
        let bad = "this = = not toml
";
        std::fs::write(dir.path().join("project.toml"), bad).unwrap();
        ensure_project(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("project.toml")).unwrap(),
            bad
        );
        assert!(dir.path().join("requests/main").is_dir());
    }
}
