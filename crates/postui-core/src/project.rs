//! Project-level files: `project.toml` (metadata + default headers),
//! `variables.toml` (declared variables), `environments/*.toml` (env
//! overlays), and `.local/state.toml` (machine-owned UI state).

use crate::model::Entry;
use crate::trash::Trashed;
use crate::varmodel;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The space a fresh project starts with.
pub const DEFAULT_SPACE: &str = "main";

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMeta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub default_headers: IndexMap<String, Entry>,
    /// Space order (spec: "Order lives in project.toml"). Directories not
    /// listed still count — see `list_spaces`.
    #[serde(default)]
    pub spaces: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalState {
    pub environment: Option<String>,
    pub open_request: Option<String>,
    /// The editor/response split the UI last settled on, as an opaque
    /// token owned by the UI layer (postui's `split` module); `None` (or
    /// a token the UI doesn't recognize) means the default split.
    pub main_split: Option<String>,
    pub expanded: Vec<String>,
    /// Per-environment `name → selected option key`, shared by variables
    /// and groups (spec §1.3).
    pub selections: IndexMap<String, IndexMap<String, String>>,
    /// `selector name → selected option key` for shared selectors — one
    /// global pick, not per-environment (a shared selector's options are
    /// identical everywhere, and so is its selection).
    pub shared_selections: IndexMap<String, String>,
    /// The active space.
    pub space: Option<String>,
    /// space → the request last open in it.
    pub space_open: IndexMap<String, String>,
}

#[derive(thiserror::Error, Debug)]
pub enum ProjectError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Parse(String),
    #[error("invalid name: {0}")]
    BadName(String),
    #[error("cannot delete the last space")]
    LastSpace,
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not found: {0}")]
    NotFound(String),
}

/// Reads `path`; missing file yields `Ok(None)`, any other IO error is
/// propagated as `ProjectError::Io`.
fn read_optional(path: &Path) -> Result<Option<String>, ProjectError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ProjectError::Io(e)),
    }
}

pub fn is_project(root: &Path) -> bool {
    root.join("project.toml").is_file()
}

pub fn display_name(root: &Path, meta: &ProjectMeta) -> String {
    meta.name.clone().unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string())
    })
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

fn valid_space_name(name: &str) -> bool {
    !name.contains('/') && crate::storage::validate_slug(name).is_ok()
}

/// `root/requests/<name>`.
pub fn space_dir(root: &Path, name: &str) -> PathBuf {
    crate::storage::requests_dir(root).join(name)
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
    let mut arr = toml_edit::Array::new();
    for s in spaces {
        arr.push(s.as_str());
    }
    doc["spaces"] = toml_edit::value(arr);
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// The list every space op edits and hands back to [`write_spaces`]:
/// `meta.spaces` exactly as written (duplicates dropped, **invalid names
/// kept in their original positions** — a hand-written entry the UI can't
/// show is still the user's, and must survive the next space op), then any
/// unlisted directory under `requests/`, alphabetically. Filtering the
/// display list back onto disk would silently erase those entries, which
/// the spec forbids.
fn write_list(root: &Path) -> Vec<String> {
    let meta = load_meta(root).unwrap_or_default();
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
    out
}

/// How many entries of a [`write_list`] are real, displayable spaces.
fn valid_count(spaces: &[String]) -> usize {
    spaces.iter().filter(|s| valid_space_name(s)).count()
}

pub fn create_space(root: &Path, name: &str) -> Result<(), ProjectError> {
    if !valid_space_name(name) {
        return Err(ProjectError::BadName(name.to_string()));
    }
    let mut spaces = write_list(root);
    if spaces.iter().any(|s| s == name) || space_dir(root, name).exists() {
        return Err(ProjectError::AlreadyExists(name.to_string()));
    }
    std::fs::create_dir_all(space_dir(root, name))?;
    spaces.push(name.to_string());
    write_spaces(root, &spaces)
}

/// Renames the directory (creating the target when `from` was list-only)
/// and rewrites the list entry in place. Local-state cascades (open
/// request, expanded folders) are the caller's job.
pub fn rename_space(root: &Path, from: &str, to: &str) -> Result<(), ProjectError> {
    if !valid_space_name(to) {
        return Err(ProjectError::BadName(to.to_string()));
    }
    let mut spaces = write_list(root);
    let Some(idx) = spaces.iter().position(|s| s == from) else {
        return Err(ProjectError::NotFound(from.to_string()));
    };
    if spaces.iter().any(|s| s == to) || space_dir(root, to).exists() {
        return Err(ProjectError::AlreadyExists(to.to_string()));
    }
    let from_dir = space_dir(root, from);
    let to_dir = space_dir(root, to);
    if from_dir.is_dir() {
        std::fs::rename(&from_dir, &to_dir)?;
    } else {
        std::fs::create_dir_all(&to_dir)?;
    }
    spaces[idx] = to.to_string();
    write_spaces(root, &spaces)
}

/// Trashes the space's directory (if it exists on disk) and drops the
/// list entry. Refuses the only remaining space. Confirmation is the
/// caller's job.
pub fn delete_space(root: &Path, name: &str) -> Result<Option<Trashed>, ProjectError> {
    let mut spaces = write_list(root);
    let Some(idx) = spaces.iter().position(|s| s == name) else {
        return Err(ProjectError::NotFound(name.to_string()));
    };
    if valid_count(&spaces) == 1 {
        return Err(ProjectError::LastSpace);
    }
    let dir = space_dir(root, name);
    let trashed = if dir.is_dir() {
        Some(crate::trash::trash(root, &dir)?)
    } else {
        None
    };
    spaces.remove(idx);
    write_spaces(root, &spaces)?;
    Ok(trashed)
}

/// Moves `name` by `delta` positions (clamped to the ends). Unlisted
/// directories are materialised into the written list so the order on
/// disk is exactly the order displayed.
pub fn move_space(root: &Path, name: &str, delta: i32) -> Result<(), ProjectError> {
    let mut spaces = write_list(root);
    // Reorder among the *displayed* spaces only: an invalid listed entry
    // isn't a row the user can see, so it must not absorb a step — and it
    // keeps the slot it was written in.
    let slots: Vec<usize> = (0..spaces.len())
        .filter(|i| valid_space_name(&spaces[*i]))
        .collect();
    let Some(pos) = slots.iter().position(|i| spaces[*i] == *name) else {
        return Err(ProjectError::NotFound(name.to_string()));
    };
    let target = (pos as i32 + delta).clamp(0, slots.len() as i32 - 1) as usize;
    if target != pos {
        spaces.swap(slots[pos], slots[target]);
    }
    write_spaces(root, &spaces)
}

/// Creates an empty `root/environments/<name>.toml`, making the directory
/// if needed. Rejects names `load_environment` would reject, and an already
/// existing file (`create_new` — the check and the create are one atomic
/// step, so a concurrent writer can't be clobbered).
pub fn create_environment(root: &Path, name: &str) -> Result<(), ProjectError> {
    if name.contains('/') || crate::storage::validate_slug(name).is_err() {
        return Err(ProjectError::BadName(name.to_string()));
    }
    std::fs::create_dir_all(root.join("environments"))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(environment_path(root, name))?;
    Ok(())
}

/// `root/environments/<name>.toml`.
pub fn environment_path(root: &Path, name: &str) -> PathBuf {
    root.join("environments").join(format!("{name}.toml"))
}

/// Renames the environment file. Secrets and local selections keyed by
/// the old name are the caller's cascade.
pub fn rename_environment(root: &Path, from: &str, to: &str) -> Result<(), ProjectError> {
    if !valid_space_name(to) {
        return Err(ProjectError::BadName(to.to_string()));
    }
    let from_path = environment_path(root, from);
    let to_path = environment_path(root, to);
    if !from_path.is_file() {
        return Err(ProjectError::NotFound(from.to_string()));
    }
    if to_path.exists() {
        return Err(ProjectError::AlreadyExists(to.to_string()));
    }
    std::fs::rename(&from_path, &to_path)?;
    Ok(())
}

/// Moves the environment file into the trash.
pub fn delete_environment(root: &Path, name: &str) -> Result<Trashed, ProjectError> {
    let path = environment_path(root, name);
    if !path.is_file() {
        return Err(ProjectError::NotFound(name.to_string()));
    }
    Ok(crate::trash::trash(root, &path)?)
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
    std::fs::write(dir.join("state.toml"), contents)
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
    let tmp_path = dir.join(".secrets.toml.tmp");
    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(&tmp_path, dir.join("secrets.toml"))
}

/// Writes `path` with `contents` only if it does not already exist.
fn write_if_absent(path: &Path, contents: &str) -> std::io::Result<()> {
    if !path.is_file() {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

pub fn init_project(root: &Path, name: Option<&str>) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("requests"))?;
    std::fs::create_dir_all(root.join("environments"))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn init_project_is_idempotent_and_never_overwrites() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), Some("My API")).unwrap();
        assert!(dir.path().join("project.toml").is_file());
        assert!(dir.path().join("requests").is_dir());
        assert!(dir.path().join("environments").is_dir());
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
        assert!(is_project(dir.path()));
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
        for bad in ["", "Bad Name", "UPPER", "a/b", "..", "dev.toml"] {
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
    fn space_ops_preserve_an_invalid_listed_entry_in_place() {
        let dir = tempdir().unwrap();
        write_spaces(dir.path(), &["main".into(), "Not Valid".into()]).unwrap();
        std::fs::create_dir_all(space_dir(dir.path(), "main")).unwrap();

        create_space(dir.path(), "auth").unwrap();
        assert_eq!(
            load_meta(dir.path()).unwrap().spaces,
            ["main", "Not Valid", "auth"]
        );

        // The reorder steps over the invalid entry rather than swapping
        // with it: it keeps slot 1, the two real spaces trade places.
        move_space(dir.path(), "auth", -1).unwrap();
        assert_eq!(
            load_meta(dir.path()).unwrap().spaces,
            ["auth", "Not Valid", "main"]
        );

        rename_space(dir.path(), "main", "identity").unwrap();
        assert_eq!(
            load_meta(dir.path()).unwrap().spaces,
            ["auth", "Not Valid", "identity"]
        );

        delete_space(dir.path(), "identity").unwrap();
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["auth", "Not Valid"]);

        // Only real spaces count towards "the last space".
        assert!(matches!(
            delete_space(dir.path(), "auth"),
            Err(ProjectError::LastSpace)
        ));
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
            create_space(dir.path(), "Bad/Name"),
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
    fn rename_space_moves_the_dir_and_rewrites_the_list_entry_in_place() {
        let dir = tempdir().unwrap();
        create_space(dir.path(), "main").unwrap();
        create_space(dir.path(), "auth").unwrap();
        std::fs::write(
            space_dir(dir.path(), "auth").join("login.toml"),
            "url = \"x\"\n",
        )
        .unwrap();
        rename_space(dir.path(), "auth", "identity").unwrap();
        assert!(!space_dir(dir.path(), "auth").exists());
        assert!(
            space_dir(dir.path(), "identity")
                .join("login.toml")
                .is_file()
        );
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["main", "identity"]);
        assert!(matches!(
            rename_space(dir.path(), "identity", "main"),
            Err(ProjectError::AlreadyExists(_))
        ));
        assert!(matches!(
            rename_space(dir.path(), "nope", "x"),
            Err(ProjectError::NotFound(_))
        ));
    }

    #[test]
    fn rename_space_of_a_list_only_space_creates_the_new_dir() {
        let dir = tempdir().unwrap();
        write_spaces(dir.path(), &["main".into(), "empty".into()]).unwrap();
        rename_space(dir.path(), "empty", "later").unwrap();
        assert!(space_dir(dir.path(), "later").is_dir());
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["main", "later"]);
    }

    #[test]
    fn delete_space_trashes_the_dir_and_refuses_the_last_space() {
        let dir = tempdir().unwrap();
        create_space(dir.path(), "main").unwrap();
        create_space(dir.path(), "auth").unwrap();
        std::fs::write(
            space_dir(dir.path(), "auth").join("login.toml"),
            "url = \"x\"\n",
        )
        .unwrap();
        let t = delete_space(dir.path(), "auth")
            .unwrap()
            .expect("dir existed");
        assert!(!space_dir(dir.path(), "auth").exists());
        assert!(t.trashed.join("login.toml").is_file());
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["main"]);
        assert!(matches!(
            delete_space(dir.path(), "main"),
            Err(ProjectError::LastSpace)
        ));
        assert!(matches!(
            delete_space(dir.path(), "auth"),
            Err(ProjectError::NotFound(_))
        ));
    }

    #[test]
    fn delete_space_of_a_list_only_space_returns_none() {
        let dir = tempdir().unwrap();
        write_spaces(dir.path(), &["main".into(), "empty".into()]).unwrap();
        assert_eq!(delete_space(dir.path(), "empty").unwrap(), None);
        assert_eq!(load_meta(dir.path()).unwrap().spaces, ["main"]);
    }

    #[test]
    fn move_space_swaps_positions_clamps_at_the_ends_and_materialises_unlisted_dirs() {
        let dir = tempdir().unwrap();
        write_spaces(dir.path(), &["main".into(), "auth".into()]).unwrap();
        std::fs::create_dir_all(space_dir(dir.path(), "billing")).unwrap();
        move_space(dir.path(), "billing", -1).unwrap();
        assert_eq!(
            load_meta(dir.path()).unwrap().spaces,
            ["main", "billing", "auth"]
        );
        move_space(dir.path(), "main", -1).unwrap(); // already first: no-op
        assert_eq!(
            load_meta(dir.path()).unwrap().spaces,
            ["main", "billing", "auth"]
        );
        move_space(dir.path(), "main", 1).unwrap();
        assert_eq!(
            load_meta(dir.path()).unwrap().spaces,
            ["billing", "main", "auth"]
        );
        assert!(matches!(
            move_space(dir.path(), "nope", 1),
            Err(ProjectError::NotFound(_))
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
    fn rename_environment_moves_the_file_and_refuses_collisions() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        create_environment(dir.path(), "qa").unwrap();
        create_environment(dir.path(), "prod").unwrap();
        std::fs::write(environment_path(dir.path(), "qa"), "tok = \"q\"\n").unwrap();
        rename_environment(dir.path(), "qa", "staging").unwrap();
        assert!(!environment_path(dir.path(), "qa").exists());
        assert_eq!(
            std::fs::read_to_string(environment_path(dir.path(), "staging")).unwrap(),
            "tok = \"q\"\n"
        );
        assert!(matches!(
            rename_environment(dir.path(), "staging", "prod"),
            Err(ProjectError::AlreadyExists(_))
        ));
        assert!(matches!(
            rename_environment(dir.path(), "nope", "x"),
            Err(ProjectError::NotFound(_))
        ));
        assert!(matches!(
            rename_environment(dir.path(), "staging", "Bad Name"),
            Err(ProjectError::BadName(_))
        ));
    }

    #[test]
    fn delete_environment_trashes_the_file() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        create_environment(dir.path(), "qa").unwrap();
        let t = delete_environment(dir.path(), "qa").unwrap();
        assert!(!environment_path(dir.path(), "qa").exists());
        assert!(t.trashed.is_file());
        assert!(matches!(
            delete_environment(dir.path(), "qa"),
            Err(ProjectError::NotFound(_))
        ));
    }
}
