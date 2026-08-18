//! Project-level files: `project.toml` (metadata + default headers),
//! `variables.toml` (declared variables), `environments/*.toml` (env
//! overlays), and `.local/state.toml` (machine-owned UI state).

use crate::model::Entry;
use crate::varmodel;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMeta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub default_headers: IndexMap<String, Entry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalState {
    pub environment: Option<String>,
    pub open_request: Option<String>,
    pub expanded: Vec<String>,
    /// Per-environment `name → selected option key`, shared by variables
    /// and groups (spec §1.3).
    pub selections: IndexMap<String, IndexMap<String, String>>,
}

#[derive(thiserror::Error, Debug)]
pub enum ProjectError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Parse(String),
    #[error("invalid name: {0}")]
    BadName(String),
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

pub fn load_environment(root: &Path, name: &str) -> Result<varmodel::EnvData, ProjectError> {
    if name.contains('/') || crate::storage::validate_slug(name).is_err() {
        return Err(ProjectError::BadName(name.to_string()));
    }
    let path = root.join("environments").join(format!("{name}.toml"));
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
                options: IndexMap::new(),
            },
        );
        model.vars.insert(
            "token".into(),
            varmodel::VarDecl {
                description: None,
                default: None,
                secret: false,
                options: IndexMap::new(),
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
            expanded: vec!["users".into()],
            selections,
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
}
