//! Test-only fixtures: the former free functions that write and read
//! project files directly. Tests use them to stand in for the outside
//! world (an editor, another process) and to read files back; nothing
//! in the library or the app calls them. Compiled only for tests and the
//! `test-util` feature. Exempt from the `std::fs` lint on that basis.
#![cfg(any(test, feature = "test-util"))]

use crate::model::HttpRequest;
use crate::order::{OrderEdit, level_of, merge_level, space_order, write_order};
use crate::project::{
    DEFAULT_ENVIRONMENT, DEFAULT_SPACE, Kind, ListChange, LocalState, Project, ProjectError, ProjectMeta,
    display_name_of, display_taken, env_display, environment_path, set_item_name, space_dir,
    slug_array, space_display, unique_slug_among, valid_space_name,
};
use crate::storage::{
    RequestListing, StorageError, request_path, requests_dir, space_of, validate_slug,
};
use crate::varmodel;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------
// `.local/trash/`: deletes were renames into a per-project trash so undo
// was a rename back. `Disk` owns the trash now; this is what the tests of
// the former free functions still need to see.
// ---------------------------------------------------------------------

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

// ---------------------------------------------------------------------
// Project-level files: `project.toml`, `variables.toml`,
// `environments/*.toml`, `.local/state.toml`, `.local/secrets.toml`.
// ---------------------------------------------------------------------

/// Reads `path`; missing file yields `Ok(None)`, any other IO error is
/// propagated as `ProjectError::Io`.
fn read_optional(path: &Path) -> Result<Option<String>, ProjectError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ProjectError::Io(e)),
    }
}

/// Writes `contents` to `path` through a sibling temp file and a rename,
/// so a crash mid-write leaves the old file intact rather than a
/// truncated one (an empty `project.toml` parses as a valid, empty meta —
/// order, display names and tls policy silently gone).
fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, contents)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Writes `path` with `contents` only if it does not already exist.
fn write_if_absent(path: &Path, contents: &str) -> std::io::Result<()> {
    if !path.is_file() {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// Rewrites `project.toml` through `f` (created if missing), preserving
/// everything `f` doesn't touch, comments included.
fn edit_project_toml(
    root: &Path,
    f: impl FnOnce(&mut toml_edit::DocumentMut),
) -> Result<(), ProjectError> {
    let path = root.join("project.toml");
    let text = read_optional(&path)?.unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ProjectError::Parse(e.to_string()))?;
    f(&mut doc);
    write_atomic(&path, doc.to_string().as_bytes())?;
    Ok(())
}

pub fn load_meta(root: &Path) -> Result<ProjectMeta, ProjectError> {
    match read_optional(&root.join("project.toml"))? {
        None => Ok(ProjectMeta::default()),
        Some(contents) => toml::from_str(&contents).map_err(|e| ProjectError::Parse(e.to_string())),
    }
}

pub fn load_variables(root: &Path) -> Result<varmodel::VarModel, ProjectError> {
    match read_optional(&root.join("variables.toml"))? {
        None => Ok(varmodel::VarModel::default()),
        Some(contents) => {
            varmodel::parse_variables(&contents).map_err(|e| ProjectError::Parse(e.to_string()))
        }
    }
}

pub fn list_environments(root: &Path) -> Vec<String> {
    let dir = root.join("environments");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        if !stem.contains('/') && crate::storage::validate_slug(&stem).is_ok() {
            out.push(stem);
        }
    }
    out.sort();
    out
}

/// Every space, in display order: `meta.spaces` first (invalid names
/// skipped, duplicates dropped), then any directory under `requests/`
/// that isn't listed, alphabetically. A listed name with no directory
/// still counts (an empty space survives git that way).
pub fn list_spaces(root: &Path, meta: &ProjectMeta) -> Vec<String> {
    list_spaces_with_warnings(root, meta).0
}

/// [`list_spaces`] plus one warning line per `meta.spaces` entry that was
/// skipped for being an invalid space name. Those entries are never
/// rewritten away (see `write_list`) — the user is told instead,
/// so the fix stays theirs to make (spec §Error handling).
pub fn list_spaces_with_warnings(root: &Path, meta: &ProjectMeta) -> (Vec<String>, Vec<String>) {
    let mut out: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for name in &meta.spaces {
        if !valid_space_name(name) {
            if !skipped.contains(name) {
                skipped.push(name.clone());
            }
            continue;
        }
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    let mut unlisted = Vec::new();
    if let Ok(entries) = std::fs::read_dir(crate::storage::requests_dir(root)) {
        for e in entries.filter_map(|e| e.ok()) {
            if !e.path().is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if valid_space_name(&name) && !out.contains(&name) {
                unlisted.push(name);
            }
        }
    }
    unlisted.sort();
    out.extend(unlisted);
    let warnings = skipped
        .into_iter()
        .map(|n| {
            format!("project.toml lists {n:?}, which is not a valid space name (space names are a-z 0-9 - _)")
        })
        .collect();
    (out, warnings)
}

/// Rewrites only the `spaces` key of `project.toml` (created if missing),
/// preserving everything else in the file, comments included.
pub fn write_spaces(root: &Path, spaces: &[String]) -> Result<(), ProjectError> {
    let path = root.join("project.toml");
    let text = read_optional(&path)?.unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ProjectError::Parse(e.to_string()))?;
    doc["spaces"] = toml_edit::value(slug_array(spaces));
    write_atomic(&path, doc.to_string().as_bytes())?;
    Ok(())
}

/// The list every space op edits and hands back to [`write_spaces`]:
/// `meta.spaces` exactly as written (duplicates dropped, **invalid names
/// kept in their original positions** — a hand-written entry the UI can't
/// show is still the user's, and must survive the next space op), then any
/// unlisted directory under `requests/`, alphabetically. Filtering the
/// display list back onto disk would silently erase those entries, which
/// the spec forbids.
///
/// A `project.toml` that doesn't parse is an error, never an empty list:
/// rebuilding the list from directories alone and writing it back would
/// destroy the user's order and every list-only space.
fn write_list(root: &Path) -> Result<Vec<String>, ProjectError> {
    let meta = load_meta(root)?;
    let mut out: Vec<String> = Vec::new();
    for name in &meta.spaces {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    for name in list_spaces(root, &meta) {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

/// The slug [`create_space`] / would give `display`
/// (`exclude` = the slug being renamed, which is not a collision with
/// itself).
pub fn space_slug_for(root: &Path, display: &str, exclude: Option<&str>) -> String {
    // Only a collision probe: with an unreadable meta the directories on
    // disk are the best available answer (the op itself refuses earlier).
    let listed = write_list(root).unwrap_or_default();
    unique_slug_among(
        Kind::Space,
        display,
        |slug| listed.iter().any(|s| s == slug) || space_dir(root, slug).exists(),
        exclude,
    )
}

/// The slug [`create_environment`] would give `display`.
pub fn environment_slug_for(root: &Path, display: &str, exclude: Option<&str>) -> String {
    unique_slug_among(
        Kind::Environment,
        display,
        |slug| environment_path(root, slug).exists(),
        exclude,
    )
}

/// Creates a space from a free-form display name: the directory is the
/// slugified name (`-2`, `-3`, … on a slug collision, request-style), the
/// name itself is recorded under `[space.<slug>]`. Returns the slug. A
/// display name another space already answers to is refused.
pub fn create_space(root: &Path, display: &str) -> Result<String, ProjectError> {
    let display = display_name_of(display)?;
    let meta = load_meta(root)?;
    let mut spaces = write_list(root)?;
    if display_taken(&display, &spaces, |s| space_display(&meta, s), None) {
        return Err(ProjectError::AlreadyExists(display));
    }
    let slug = space_slug_for(root, &display, None);
    std::fs::create_dir_all(space_dir(root, &slug))?;
    spaces.push(slug.clone());
    edit_project_toml(root, |doc| {
        doc["spaces"] = toml_edit::value(slug_array(&spaces));
        set_item_name(doc, Kind::Space, &slug, &display);
    })?;
    Ok(slug)
}

/// Creates an empty `root/environments/<slug>.toml` for a free-form
/// display name (slug rules as [`create_space`]), making the directory if
/// needed, and records the name under `[environment.<slug>]`. Returns the
/// slug. The file is opened with `create_new` — the check and the create
/// are one atomic step, so a concurrent writer can't be clobbered.
pub fn create_environment(root: &Path, display: &str) -> Result<String, ProjectError> {
    let display = display_name_of(display)?;
    let meta = load_meta(root)?;
    let existing = list_environments(root);
    if display_taken(&display, &existing, |s| env_display(&meta, s), None) {
        return Err(ProjectError::AlreadyExists(display));
    }
    let slug = environment_slug_for(root, &display, None);
    std::fs::create_dir_all(root.join("environments"))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(environment_path(root, &slug))?;
    edit_project_toml(root, |doc| {
        set_item_name(doc, Kind::Environment, &slug, &display);
    })?;
    Ok(slug)
}

pub fn load_environment(root: &Path, name: &str) -> Result<varmodel::EnvData, ProjectError> {
    if name.contains('/') || crate::storage::validate_slug(name).is_err() {
        return Err(ProjectError::BadName(name.to_string()));
    }
    let path = environment_path(root, name);
    let contents = std::fs::read_to_string(&path)?;
    varmodel::parse_environment(&contents).map_err(|e| ProjectError::Parse(e.to_string()))
}

pub fn load_local_state(root: &Path) -> Result<LocalState, ProjectError> {
    match read_optional(&root.join(".local").join("state.toml"))? {
        None => Ok(LocalState::default()),
        Some(contents) => toml::from_str(&contents).map_err(|e| ProjectError::Parse(e.to_string())),
    }
}

pub fn save_local_state(root: &Path, state: &LocalState) -> std::io::Result<()> {
    let dir = root.join(".local");
    std::fs::create_dir_all(&dir)?;
    let contents = toml::to_string(state).expect("LocalState always serializes");
    write_atomic(&dir.join("state.toml"), contents.as_bytes())
}

/// Loads `.local/secrets.toml`: env → name → value. Missing file yields an
/// empty map (secrets are never required to exist).
pub fn load_secrets(
    root: &Path,
) -> Result<IndexMap<String, IndexMap<String, String>>, ProjectError> {
    match read_optional(&root.join(".local").join("secrets.toml"))? {
        None => Ok(IndexMap::new()),
        Some(contents) => toml::from_str(&contents).map_err(|e| ProjectError::Parse(e.to_string())),
    }
}

/// Writes `.local/secrets.toml` atomically (temp file + rename), creating
/// `.local/` if needed.
pub fn save_secrets(
    root: &Path,
    secrets: &IndexMap<String, IndexMap<String, String>>,
) -> std::io::Result<()> {
    let dir = root.join(".local");
    std::fs::create_dir_all(&dir)?;
    let contents = toml::to_string(secrets).expect("secrets always serialize");
    write_atomic(&dir.join("secrets.toml"), contents.as_bytes())
}

/// Writes `root/environments/default.toml` when the project has no
/// environment files at all. Returns whether it wrote one.
pub fn ensure_default_environment(root: &Path) -> std::io::Result<bool> {
    if !list_environments(root).is_empty() {
        return Ok(false);
    }
    std::fs::create_dir_all(root.join("environments"))?;
    std::fs::write(
        environment_path(root, DEFAULT_ENVIRONMENT),
        "# environments/default.toml: values for this project's variables\n",
    )?;
    Ok(true)
}

pub fn init_project(root: &Path, name: Option<&str>) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("requests"))?;
    std::fs::create_dir_all(root.join("environments"))?;
    ensure_default_environment(root)?;

    let project_toml = match name {
        Some(n) => {
            let mut doc = toml_edit::DocumentMut::new();
            doc["name"] = toml_edit::value(n);
            doc.to_string()
        }
        None => "# project.toml: optional `name`, optional [default_headers]\n".to_string(),
    };
    write_if_absent(&root.join("project.toml"), &project_toml)?;

    write_if_absent(
        &root.join("variables.toml"),
        "# Declare variables: [name] with optional description/default\n",
    )?;

    write_if_absent(&root.join(".gitignore"), "/.local/\n")?;

    Ok(())
}

// ---------------------------------------------------------------------
// Slug-addressed request files under `root/requests/**/*.toml`.
// ---------------------------------------------------------------------

/// Builds a closure that wraps an `io::Error` with the path it concerns,
/// for use as `.map_err(io_err(&path))`.
fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> StorageError + '_ {
    move |source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    }
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
/// `Project::open`'s "never fail to open outright" policy of
/// degrading a broken `project.toml` to a warning rather than a hard
/// error. `list_spaces` still reports `main` as an unlisted directory
/// either way.
pub fn ensure_project(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(requests_dir(root))?;
    let Ok(meta) = load_meta(root) else {
        if list_spaces(root, &crate::project::ProjectMeta::default()).is_empty() {
            std::fs::create_dir_all(crate::project::space_dir(
                root,
                DEFAULT_SPACE,
            ))?;
        }
        return Ok(());
    };
    if Project::is_project(root) {
        if meta.spaces.is_empty() {
            let spaces = list_spaces(root, &meta);
            if spaces.is_empty() {
                create_space(root, DEFAULT_SPACE)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
            } else {
                write_spaces(root, &spaces).map_err(|e| std::io::Error::other(e.to_string()))?;
            }
        }
    } else if list_spaces(root, &meta).is_empty() {
        std::fs::create_dir_all(crate::project::space_dir(
            root,
            DEFAULT_SPACE,
        ))?;
    }
    Ok(())
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

/// Moves `root/requests/<slug>.toml` into the trash (see `trash` above)
/// and returns the record undo needs to bring it back.
pub fn delete_request(root: &Path, slug: &str) -> Result<Trashed, StorageError> {
    validate_slug(slug)?;
    let path = request_path(root, slug);
    if !path.is_file() {
        return Err(StorageError::NotFound(slug.to_string()));
    }
    trash(root, &path).map_err(io_err(&path))
}

// ---------------------------------------------------------------------
// The written request order of one level of a space.
// ---------------------------------------------------------------------

/// Reads the list, hands it to `f`, and writes back only if `f` changed
/// it — so every cascade is a no-op write when there is nothing to do.
/// `f` returns the edits it made; they come back only when something was
/// actually written.
fn edit_order(
    root: &Path,
    space: &str,
    f: impl FnOnce(&mut Vec<String>) -> Vec<OrderEdit>,
) -> Result<Vec<OrderEdit>, ProjectError> {
    // A list belongs to a space that exists. Writing one for a space
    // that does not (an undo step recorded before the space was renamed
    // or deleted, say) would plant an orphan `[space.<slug>]` table that
    // nothing displays and a future space of that slug would inherit.
    if !crate::project::space_dir(root, space).is_dir() {
        return Err(ProjectError::NotFound(space.to_string()));
    }
    let meta = load_meta(root)?;
    let before = space_order(&meta, space).to_vec();
    let mut after = before.clone();
    let edits = f(&mut after);
    if after == before {
        return Ok(Vec::new());
    }
    edit_project_toml(root, |doc| write_order(doc, space, &after))?;
    Ok(edits)
}

/// Rewrites one level of `space`'s order to exactly `slugs` (relative
/// slugs, all of the same `level`). See [`merge_level`] for what happens
/// to everything else in the list. Reports the whole list before and
/// after (`None` when it already read that way).
pub fn set_level_order(
    root: &Path,
    space: &str,
    level: &str,
    slugs: &[String],
) -> Result<Option<ListChange>, ProjectError> {
    debug_assert!(
        slugs.iter().all(|s| level_of(s) == level),
        "set_level_order got slugs from another level: {slugs:?} is not all under {level:?}"
    );
    let mut change = None;
    edit_order(root, space, |order| {
        let before = order.clone();
        *order = merge_level(order, level, slugs);
        change = Some(ListChange {
            before,
            after: order.clone(),
        });
        Vec::new()
    })?;
    Ok(change.filter(|c| c.before != c.after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::project::{TlsPolicy, display_name, env_tls};
    use tempfile::tempdir;

    #[test]
    fn init_project_is_idempotent_and_never_overwrites() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), Some("My API")).unwrap();
        assert!(dir.path().join("project.toml").is_file());
        assert!(dir.path().join("requests").is_dir());
        assert!(dir.path().join("environments").is_dir());
        assert_eq!(
            list_environments(dir.path()),
            vec![DEFAULT_ENVIRONMENT.to_string()],
            "a new project starts with its default environment"
        );
        assert!(dir.path().join("variables.toml").is_file());
        let gi = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gi.contains("/.local/"));

        // user edits survive a second init
        std::fs::write(dir.path().join("project.toml"), "name = \"edited\"\n").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "custom\n").unwrap();
        init_project(dir.path(), Some("My API")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("project.toml")).unwrap(),
            "name = \"edited\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            "custom\n"
        );
        assert!(Project::is_project(dir.path()));
    }

    #[test]
    fn init_project_escapes_names_with_quotes_and_backslashes() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), Some(r#"Bob's "Cool" API"#)).unwrap();
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(meta.name.as_deref(), Some(r#"Bob's "Cool" API"#));
    }

    #[test]
    fn load_environment_rejects_path_traversal_names() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            load_environment(dir.path(), "../x"),
            Err(ProjectError::BadName(_))
        ));
        assert!(matches!(
            load_environment(dir.path(), "a/b"),
            Err(ProjectError::BadName(_))
        ));
        assert!(matches!(
            load_environment(dir.path(), "Bad Name"),
            Err(ProjectError::BadName(_))
        ));
    }

    #[test]
    fn meta_defaults_and_display_name_fall_back_to_dir_basename() {
        let dir = tempdir().unwrap();
        let meta = load_meta(dir.path()).unwrap(); // no project.toml at all
        assert!(meta.name.is_none() && meta.default_headers.is_empty());
        let base = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(display_name(dir.path(), &meta), base);
        std::fs::write(
            dir.path().join("project.toml"),
            "name = \"svc\"\n[default_headers]\naccept = \"application/json\"\nx = { value = \"1\", enabled = false }\n",
        )
        .unwrap();
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(display_name(dir.path(), &meta), "svc");
        assert_eq!(meta.default_headers["accept"].value, "application/json");
        assert!(!meta.default_headers["x"].enabled);
    }

    #[test]
    fn variables_parse_validate_names_and_reject_unknown_fields() {
        let dir = tempdir().unwrap();
        assert!(
            load_variables(dir.path()).unwrap().vars.is_empty(),
            "missing file is empty"
        );
        std::fs::write(
            dir.path().join("variables.toml"),
            "[base_url]\ndescription = \"root\"\ndefault = \"http://l\"\n\n[token]\n",
        )
        .unwrap();
        let vars = load_variables(dir.path()).unwrap();
        assert_eq!(vars.vars["base_url"].default.as_deref(), Some("http://l"));
        assert!(vars.vars["token"].default.is_none());

        std::fs::write(dir.path().join("variables.toml"), "[\"bad name\"]\n").unwrap();
        assert!(matches!(
            load_variables(dir.path()),
            Err(ProjectError::Parse(_))
        ));
        std::fs::write(dir.path().join("variables.toml"), "[a]\nbogus = 1\n").unwrap();
        assert!(matches!(
            load_variables(dir.path()),
            Err(ProjectError::Parse(_))
        ));
    }

    #[test]
    fn meta_parses_per_space_and_per_environment_settings_tables() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            "spaces = [\"main\", \"auth-v2\"]\n\n[space.auth-v2]\nname = \"Auth v2\"\n\n[environment.staging]\nname = \"Staging (EU)\"\nfuture_key = 1\n",
        )
        .unwrap();
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(space_display(&meta, "auth-v2"), "Auth v2");
        assert_eq!(space_display(&meta, "main"), "main", "no table: the slug");
        assert_eq!(env_display(&meta, "staging"), "Staging (EU)");
        assert_eq!(env_display(&meta, "qa"), "qa");
    }

    #[test]
    fn meta_parses_the_environment_tls_policy() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            "[environment.prod]\nname = \"Prod\"\ntls = \"verify\"\n\n[environment.local]\ntls = \"insecure\"\n\n[environment.qa]\nname = \"QA\"\n",
        )
        .unwrap();
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(env_tls(&meta, "prod"), Some(TlsPolicy::Verify));
        assert_eq!(env_tls(&meta, "local"), Some(TlsPolicy::Insecure));
        assert_eq!(env_tls(&meta, "qa"), None, "no key: per request");
        assert_eq!(env_tls(&meta, "missing"), None, "no table: per request");
    }

    #[test]
    fn meta_rejects_an_unknown_tls_policy_value() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            "[environment.prod]\ntls = \"sometimes\"\n",
        )
        .unwrap();
        assert!(matches!(load_meta(dir.path()), Err(ProjectError::Parse(_))));
    }

    #[test]
    fn create_space_slugifies_the_display_name_and_records_it() {
        let dir = tempdir().unwrap();
        create_space(dir.path(), "main").unwrap();
        assert_eq!(create_space(dir.path(), "Auth v2!").unwrap(), "auth-v2");
        assert!(space_dir(dir.path(), "auth-v2").is_dir());
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(meta.spaces, ["main", "auth-v2"]);
        assert_eq!(space_display(&meta, "auth-v2"), "Auth v2!");
        // A slug collision gets the request-style `-2`; a display-name
        // collision (case-insensitive) is refused.
        assert_eq!(create_space(dir.path(), "auth V2").unwrap(), "auth-v2-2");
        assert!(matches!(
            create_space(dir.path(), "AUTH V2!"),
            Err(ProjectError::AlreadyExists(_))
        ));
        assert!(matches!(
            create_space(dir.path(), "  "),
            Err(ProjectError::BadName(_))
        ));
        assert_eq!(
            create_space(dir.path(), "???").unwrap(),
            "space",
            "all-unsafe falls back"
        );
    }

    #[test]
    fn create_environment_slugifies_the_display_name_and_records_it() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        assert_eq!(
            create_environment(dir.path(), "Staging (EU)").unwrap(),
            "staging-eu"
        );
        assert!(environment_path(dir.path(), "staging-eu").is_file());
        assert_eq!(
            env_display(&load_meta(dir.path()).unwrap(), "staging-eu"),
            "Staging (EU)"
        );
        assert_eq!(
            create_environment(dir.path(), "staging eu").unwrap(),
            "staging-eu-2"
        );
        assert!(matches!(
            create_environment(dir.path(), "staging (eu)"),
            Err(ProjectError::AlreadyExists(_))
        ));
        assert!(matches!(
            create_environment(dir.path(), ""),
            Err(ProjectError::BadName(_))
        ));
        assert_eq!(create_environment(dir.path(), "!!").unwrap(), "environment");
    }

    #[test]
    fn slug_for_display_predicts_what_create_and_rename_will_use() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        create_environment(dir.path(), "qa").unwrap();
        assert_eq!(environment_slug_for(dir.path(), "QA", None), "qa-2");
        assert_eq!(environment_slug_for(dir.path(), "QA", Some("qa")), "qa");
        create_space(dir.path(), "main").unwrap();
        assert_eq!(space_slug_for(dir.path(), "Main", None), "main-2");
        assert_eq!(space_slug_for(dir.path(), "Main", Some("main")), "main");
    }

    #[test]
    fn create_environment_writes_empty_file_and_creates_dir() {
        let dir = tempdir().unwrap();
        // no environments/ dir yet — create_environment must make it
        create_environment(dir.path(), "dev").unwrap();
        let path = dir.path().join("environments/dev.toml");
        assert!(path.is_file());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert_eq!(list_environments(dir.path()), vec!["dev".to_string()]);
        // no stray temp files left behind
        let leftovers: Vec<String> = std::fs::read_dir(dir.path().join("environments"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "dev.toml")
            .collect();
        assert!(leftovers.is_empty(), "leftover files: {leftovers:?}");
    }

    #[test]
    fn create_environment_rejects_bad_names_and_duplicates() {
        let dir = tempdir().unwrap();
        // Names are free-form now (display name + slug); only an empty
        // one is bad.
        for bad in ["", "   "] {
            assert!(
                matches!(
                    create_environment(dir.path(), bad),
                    Err(ProjectError::BadName(_))
                ),
                "expected BadName for {bad:?}"
            );
        }
        // nothing written by the rejections
        assert!(list_environments(dir.path()).is_empty());

        create_environment(dir.path(), "qa").unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "token = \"t\"\n").unwrap();
        // duplicate is an error and must not clobber the existing contents
        assert!(create_environment(dir.path(), "qa").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap(),
            "token = \"t\"\n"
        );
    }

    #[test]
    fn environments_list_load_and_resolve_with_env_over_default() {
        let dir = tempdir().unwrap();
        assert!(list_environments(dir.path()).is_empty());
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "token = \"qa-tok\"\nextra = \"e\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/prod.toml"),
            "token = \"prod-tok\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/Bad Name.toml"), "").unwrap();
        assert_eq!(
            list_environments(dir.path()),
            vec!["prod".to_string(), "qa".to_string()]
        );

        let mut model = varmodel::VarModel::default();
        model.vars.insert(
            "base".into(),
            varmodel::VarDecl {
                description: None,
                default: Some("http://l".into()),
                secret: false,
            },
        );
        model.vars.insert(
            "token".into(),
            varmodel::VarDecl {
                description: None,
                default: None,
                secret: false,
            },
        );
        let env = load_environment(dir.path(), "qa").unwrap();
        let r = varmodel::resolve_env(
            &model,
            &env,
            &varmodel::Selections::new(),
            &varmodel::SecretValues::new(),
        );
        assert_eq!(
            r.values["base"], "http://l",
            "default used when env has no value"
        );
        assert_eq!(r.values["token"], "qa-tok", "env value wins");
        assert_eq!(
            r.values["extra"], "e",
            "undeclared env value still resolves (lenient)"
        );
        let empty_env = varmodel::EnvData::default();
        let r = varmodel::resolve_env(
            &model,
            &empty_env,
            &varmodel::Selections::new(),
            &varmodel::SecretValues::new(),
        );
        assert_eq!(r.values.get("token"), None, "no env: only defaults resolve");
    }

    #[test]
    fn local_state_round_trips_and_missing_is_default() {
        let dir = tempdir().unwrap();
        let s = load_local_state(dir.path()).unwrap();
        assert!(
            s.environment.is_none()
                && s.open_request.is_none()
                && s.expanded.is_empty()
                && s.selections.is_empty()
        );
        let mut selections = IndexMap::new();
        let mut qa_selections = IndexMap::new();
        qa_selections.insert("user".into(), "alice".into());
        selections.insert("qa".to_string(), qa_selections);
        let state = LocalState {
            environment: Some("qa".into()),
            open_request: Some("users/list".into()),
            main_split: Some("editor-big".into()),
            expanded: vec!["users".into()],
            selections,
            ..Default::default()
        };
        save_local_state(dir.path(), &state).unwrap();
        assert_eq!(load_local_state(dir.path()).unwrap(), state);
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(dir.path().join(".local/state.toml"), "environment = 3\n").unwrap();
        assert!(
            load_local_state(dir.path()).is_err(),
            "corrupt state is an Err the app degrades from"
        );
    }

    #[test]
    fn local_state_round_trips_shared_selections() {
        let dir = tempdir().unwrap();
        let mut shared_selections = IndexMap::new();
        shared_selections.insert("locale".to_string(), "fr".to_string());
        let state = LocalState {
            environment: Some("qa".into()),
            shared_selections,
            ..Default::default()
        };
        save_local_state(dir.path(), &state).unwrap();
        assert_eq!(load_local_state(dir.path()).unwrap(), state);
        // An old state.toml without the table loads with an empty map.
        std::fs::write(
            dir.path().join(".local/state.toml"),
            "environment = \"qa\"\n",
        )
        .unwrap();
        assert!(
            load_local_state(dir.path())
                .unwrap()
                .shared_selections
                .is_empty()
        );
    }

    #[test]
    fn local_state_without_selections_field_still_parses() {
        // Old state.toml files written before selections existed have no
        // [selections] table at all; they must still load with an empty map.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(
            dir.path().join(".local/state.toml"),
            "environment = \"qa\"\nopen_request = \"ping\"\nexpanded = [\"users\"]\n",
        )
        .unwrap();
        let s = load_local_state(dir.path()).unwrap();
        assert_eq!(s.environment.as_deref(), Some("qa"));
        assert!(s.selections.is_empty());
    }

    #[test]
    fn local_state_with_unknown_field_is_still_permissive() {
        // LocalState does not `deny_unknown_fields`; unknown top-level keys
        // are ignored rather than erroring (today's behavior, preserved).
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(
            dir.path().join(".local/state.toml"),
            "environment = \"qa\"\nfuture_field = \"whatever\"\n",
        )
        .unwrap();
        let s = load_local_state(dir.path()).unwrap();
        assert_eq!(s.environment.as_deref(), Some("qa"));
    }

    #[test]
    fn secrets_round_trip_and_missing_file_is_empty() {
        let dir = tempdir().unwrap();
        assert!(
            load_secrets(dir.path()).unwrap().is_empty(),
            "missing secrets.toml is empty"
        );

        let mut secrets = IndexMap::new();
        let mut qa = IndexMap::new();
        qa.insert("api_key".to_string(), "sk-qa-123".to_string());
        secrets.insert("qa".to_string(), qa);
        let mut prod = IndexMap::new();
        prod.insert("api_key".to_string(), "sk-prod-456".to_string());
        secrets.insert("prod".to_string(), prod);

        save_secrets(dir.path(), &secrets).unwrap();
        assert!(dir.path().join(".local/secrets.toml").is_file());
        let loaded = load_secrets(dir.path()).unwrap();
        assert_eq!(loaded, secrets);
    }

    #[test]
    fn save_secrets_writes_atomically_via_temp_and_rename() {
        let dir = tempdir().unwrap();
        let mut secrets = IndexMap::new();
        let mut qa = IndexMap::new();
        qa.insert("api_key".to_string(), "sk-qa-123".to_string());
        secrets.insert("qa".to_string(), qa);
        save_secrets(dir.path(), &secrets).unwrap();

        // no stray temp files left behind in .local/
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join(".local"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "secrets.toml")
            .collect();
        assert!(leftovers.is_empty(), "leftover files: {leftovers:?}");
    }

    fn meta_with(spaces: &[&str]) -> ProjectMeta {
        ProjectMeta {
            spaces: spaces.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn list_spaces_listed_order_first_then_unlisted_dirs_alphabetically() {
        let dir = tempdir().unwrap();
        for d in ["zeta", "auth", "main", "Bad Name", "billing"] {
            std::fs::create_dir_all(dir.path().join("requests").join(d)).unwrap();
        }
        let meta = meta_with(&["main", "auth", "ghost"]);
        assert_eq!(
            list_spaces(dir.path(), &meta),
            ["main", "auth", "ghost", "billing", "zeta"]
        );
    }

    #[test]
    fn list_spaces_skips_invalid_listed_names_and_dedupes() {
        let dir = tempdir().unwrap();
        let meta = meta_with(&["main", "Not Valid", "main"]);
        assert_eq!(list_spaces(dir.path(), &meta), ["main"]);
    }

    #[test]
    fn list_spaces_with_warnings_names_each_skipped_invalid_entry_once() {
        let dir = tempdir().unwrap();
        let meta = meta_with(&["main", "Not Valid", "Not Valid", "a/b"]);
        let (spaces, warnings) = list_spaces_with_warnings(dir.path(), &meta);
        assert_eq!(spaces, ["main"]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("\"Not Valid\""), "{warnings:?}");
        assert!(
            warnings[0].contains("not a valid space name (space names are a-z 0-9 - _)"),
            "{warnings:?}"
        );
        assert!(warnings[1].contains("\"a/b\""), "{warnings:?}");
    }

    #[test]
    fn write_spaces_touches_only_the_spaces_key() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            "# keep me\nname = \"svc\"\n\n[default_headers]\nx = \"1\"\n",
        )
        .unwrap();
        write_spaces(dir.path(), &["main".into(), "auth".into()]).unwrap();
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("name = \"svc\""));
        assert!(text.contains("spaces = [\"main\", \"auth\"]"));
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(meta.spaces, ["main", "auth"]);
        assert_eq!(meta.default_headers.len(), 1);
    }

    #[test]
    fn write_spaces_creates_a_missing_project_toml() {
        let dir = tempdir().unwrap();
        write_spaces(dir.path(), &["main".into()]).unwrap();
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["main"]);
    }

    #[test]
    fn create_space_makes_the_dir_and_appends_to_the_list() {
        let dir = tempdir().unwrap();
        create_space(dir.path(), "main").unwrap();
        create_space(dir.path(), "auth").unwrap();
        assert!(space_dir(dir.path(), "auth").is_dir());
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["main", "auth"]);
        assert!(matches!(
            create_space(dir.path(), "auth"),
            Err(ProjectError::AlreadyExists(_))
        ));
        assert!(matches!(
            create_space(dir.path(), "   "),
            Err(ProjectError::BadName(_))
        ));
    }

    #[test]
    fn create_space_rejects_an_unlisted_dir_that_already_exists() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(space_dir(dir.path(), "auth")).unwrap();
        assert!(matches!(
            create_space(dir.path(), "auth"),
            Err(ProjectError::AlreadyExists(_))
        ));
    }

    #[test]
    fn local_state_round_trips_space_and_space_open() {
        let dir = tempdir().unwrap();
        let mut st = LocalState {
            space: Some("auth".into()),
            ..Default::default()
        };
        st.space_open.insert("auth".into(), "auth/login".into());
        st.space_open.insert("main".into(), "main/health".into());
        save_local_state(dir.path(), &st).unwrap();
        let back = load_local_state(dir.path()).unwrap();
        assert_eq!(back, st);
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
    fn load_of_missing_root_reports_not_found_not_io() {
        // Sanity check: a missing file surfaces as NotFound (checked up front),
        // not as a bare Io error lacking path context.
        let dir = tempfile::tempdir().unwrap();
        let err = load_request(dir.path(), "missing").unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
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
    fn ensure_project_seeds_the_main_space_once() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        ensure_project(dir.path()).unwrap();
        assert!(dir.path().join("requests/main").is_dir());
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(meta.spaces, ["main"]);
        // A project that already has a space is left alone.
        create_space(dir.path(), "auth").unwrap();
        std::fs::remove_dir(dir.path().join("requests/main")).unwrap();
        write_spaces(dir.path(), &["auth".into()]).unwrap();
        ensure_project(dir.path()).unwrap();
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["auth"]);
        assert!(!dir.path().join("requests/main").exists());
    }

    #[test]
    fn ensure_project_on_a_bare_dir_makes_main_but_no_project_toml() {
        let dir = tempfile::tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        assert!(dir.path().join("requests/main").is_dir());
        assert!(!dir.path().join("project.toml").exists());
        assert_eq!(
            list_spaces(dir.path(), &crate::project::ProjectMeta::default()),
            ["main"]
        );
    }

    #[test]
    fn ensure_project_materialises_an_existing_unlisted_space_into_the_list() {
        let dir = tempfile::tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/auth")).unwrap();
        ensure_project(dir.path()).unwrap();
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["auth"]);
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

    fn req() -> HttpRequest {
        HttpRequest::from_toml_str("url = \"https://x\"").unwrap()
    }

    fn project_with(slugs: &[&str]) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        ensure_project(dir.path()).unwrap();
        for s in slugs {
            save_request(dir.path(), s, &req()).unwrap();
        }
        dir
    }

    fn order_of(root: &std::path::Path, space: &str) -> Vec<String> {
        space_order(&load_meta(root).unwrap(), space).to_vec()
    }

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn set_level_order_writes_only_that_level_and_keeps_comments() {
        let dir = project_with(&["main/a", "main/b", "main/auth/x"]);
        std::fs::write(
            dir.path().join("project.toml"),
            "# keep me\nspaces = [\"main\"]\n\n[space.main]\nname = \"Main\"\norder = [\"auth/x\"]\n",
        )
        .unwrap();
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["auth/x", "b", "a"]));
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(text.starts_with("# keep me\n"), "{text}");
        assert!(text.contains("name = \"Main\""), "{text}");
    }

    #[test]
    fn set_level_order_creates_the_space_table_when_missing() {
        let dir = project_with(&["main/a", "main/b"]);
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["b", "a"]));
    }

    #[test]
    fn set_level_order_writes_nothing_when_the_level_is_already_that_order() {
        let dir = project_with(&["main/a", "main/b"]);
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        let path = dir.path().join("project.toml");
        let before = std::fs::read(&path).unwrap();
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before, "byte identical");
    }
}
