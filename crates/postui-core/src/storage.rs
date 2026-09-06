//! Pure helpers for the request layout under `root/requests/**/*.toml`:
//! slug validation and arithmetic, path derivation, and the listing type
//! `Project` fills in. Nothing here touches a file — `Project` owns every
//! read and write.

use crate::model::Method;
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
    slugify_or(name, "request")
}

/// [`slugify`] with a caller-chosen fallback for a name nothing safe
/// survives from (`"space"`, `"environment"`, …).
pub fn slugify_or(name: &str, fallback: &str) -> String {
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
        fallback.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn space_of_is_the_first_segment_of_a_nested_slug() {
        assert_eq!(space_of("auth/login"), Some("auth"));
        assert_eq!(space_of("auth/tokens/refresh"), Some("auth"));
        assert_eq!(space_of("loose"), None);
    }
}
