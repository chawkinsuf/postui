//! Project-level files: `project.toml` (metadata + default headers),
//! `variables.toml` (declared variables), `environments/*.toml` (env
//! overlays), and `.local/state.toml` (machine-owned UI state).

use crate::model::Entry;
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarDecl {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

pub type Variables = IndexMap<String, VarDecl>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalState {
    pub environment: Option<String>,
    pub open_request: Option<String>,
    pub expanded: Vec<String>,
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

pub fn load_variables(root: &Path) -> Result<Variables, ProjectError> {
    let vars: Variables = match read_optional(&root.join("variables.toml"))? {
        None => return Ok(Variables::new()),
        Some(contents) => {
            toml::from_str(&contents).map_err(|e| ProjectError::Parse(e.to_string()))?
        }
    };
    for key in vars.keys() {
        if !crate::vars::is_valid_var_name(key) {
            return Err(ProjectError::BadName(key.clone()));
        }
    }
    Ok(vars)
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

pub fn load_environment(root: &Path, name: &str) -> Result<IndexMap<String, String>, ProjectError> {
    if name.contains('/') || crate::storage::validate_slug(name).is_err() {
        return Err(ProjectError::BadName(name.to_string()));
    }
    let path = root.join("environments").join(format!("{name}.toml"));
    let contents = std::fs::read_to_string(&path)?;
    toml::from_str(&contents).map_err(|e| ProjectError::Parse(e.to_string()))
}

pub fn resolve(
    vars: &Variables,
    env: Option<&IndexMap<String, String>>,
) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for (name, decl) in vars {
        if let Some(default) = &decl.default {
            out.insert(name.clone(), default.clone());
        }
    }
    if let Some(env) = env {
        for (k, v) in env {
            out.insert(k.clone(), v.clone());
        }
    }
    out
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
            load_variables(dir.path()).unwrap().is_empty(),
            "missing file is empty"
        );
        std::fs::write(
            dir.path().join("variables.toml"),
            "[base_url]\ndescription = \"root\"\ndefault = \"http://l\"\n\n[token]\n",
        )
        .unwrap();
        let vars = load_variables(dir.path()).unwrap();
        assert_eq!(vars["base_url"].default.as_deref(), Some("http://l"));
        assert!(vars["token"].default.is_none());

        std::fs::write(dir.path().join("variables.toml"), "[\"bad name\"]\n").unwrap();
        assert!(matches!(
            load_variables(dir.path()),
            Err(ProjectError::BadName(_))
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

        let mut vars: Variables = Variables::new();
        vars.insert(
            "base".into(),
            VarDecl {
                description: None,
                default: Some("http://l".into()),
            },
        );
        vars.insert(
            "token".into(),
            VarDecl {
                description: None,
                default: None,
            },
        );
        let env = load_environment(dir.path(), "qa").unwrap();
        let r = resolve(&vars, Some(&env));
        assert_eq!(r["base"], "http://l", "default used when env has no value");
        assert_eq!(r["token"], "qa-tok", "env value wins");
        assert_eq!(
            r["extra"], "e",
            "undeclared env value still resolves (lenient)"
        );
        let r = resolve(&vars, None);
        assert_eq!(r.get("token"), None, "no env: only defaults resolve");
    }

    #[test]
    fn local_state_round_trips_and_missing_is_default() {
        let dir = tempdir().unwrap();
        let s = load_local_state(dir.path()).unwrap();
        assert!(s.environment.is_none() && s.open_request.is_none() && s.expanded.is_empty());
        let state = LocalState {
            environment: Some("qa".into()),
            open_request: Some("users/list".into()),
            expanded: vec!["users".into()],
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
}
