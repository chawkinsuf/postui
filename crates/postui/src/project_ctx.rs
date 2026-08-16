//! `ProjectContext`: the open project's metadata, variables, environments,
//! and resolved environment values, plus the UI-owned bits of local state
//! (active environment, expanded sidebar dirs) that persist across runs.

use indexmap::IndexMap;
use postui_core::project::{ProjectMeta, Variables};
use std::path::PathBuf;
use std::time::SystemTime;

pub struct ProjectContext {
    pub root: PathBuf,
    pub meta: ProjectMeta,
    pub variables: Variables,
    pub environments: Vec<String>,
    pub active_env: Option<String>,
    pub env_values: IndexMap<String, String>,
    pub expanded: std::collections::BTreeSet<String>,
    /// mtimes of the files this context was built from, recorded here for
    /// Task 12's stale-reload detection; unused until then.
    #[allow(dead_code)]
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
    /// `open_request` from the local state loaded at `open()`, so the
    /// caller can restore the previously-open request. Not kept in sync
    /// afterward — it's a one-shot startup value.
    local_open_request: Option<String>,
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

impl ProjectContext {
    /// Opens `root` as a project: loads meta/variables/environments/local
    /// state and the active environment's values. Never fails outright —
    /// any individual piece that can't be read degrades to a sane default
    /// and its problem is appended to the returned warnings.
    pub fn open(root: PathBuf) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();

        let meta = postui_core::project::load_meta(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read project.toml: {e}"));
            ProjectMeta::default()
        });
        let variables = postui_core::project::load_variables(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read variables.toml: {e}"));
            Variables::default()
        });
        let environments = postui_core::project::list_environments(&root);

        let local_state = postui_core::project::load_local_state(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read local state: {e}"));
            postui_core::project::LocalState::default()
        });

        let mut active_env = None;
        let mut env_values = IndexMap::new();
        if let Some(env) = local_state.environment {
            if !environments.contains(&env) {
                warnings.push(format!("saved environment {env:?} no longer exists"));
            } else {
                match postui_core::project::load_environment(&root, &env) {
                    Ok(values) => {
                        env_values = values;
                        active_env = Some(env);
                    }
                    Err(e) => {
                        warnings.push(format!("could not load environment {env:?}: {e}"));
                    }
                }
            }
        }

        let expanded: std::collections::BTreeSet<String> =
            local_state.expanded.into_iter().collect();

        let stamps = vec![
            (root.join("project.toml"), mtime(&root.join("project.toml"))),
            (
                root.join("variables.toml"),
                mtime(&root.join("variables.toml")),
            ),
            (root.join("environments"), mtime(&root.join("environments"))),
            active_env
                .as_ref()
                .map(|env| {
                    let p = root.join("environments").join(format!("{env}.toml"));
                    let m = mtime(&p);
                    (p, m)
                })
                .unwrap_or_else(|| (root.join("environments").join("__none__.toml"), None)),
        ];

        let ctx = ProjectContext {
            root,
            meta,
            variables,
            environments,
            active_env,
            env_values,
            expanded,
            stamps,
            local_open_request: local_state.open_request,
        };
        (ctx, warnings)
    }

    /// The project's display name: `meta.name`, falling back to the root
    /// directory's basename.
    pub fn display_name(&self) -> String {
        postui_core::project::display_name(&self.root, &self.meta)
    }

    /// The active environment's name, or `"no env"` when none is active.
    pub fn env_label(&self) -> String {
        self.active_env.clone().unwrap_or_else(|| "no env".into())
    }

    /// Builds the `PrepareContext` for sending: variables resolved (env
    /// over declared defaults) plus the project's default headers.
    pub fn prepare_context(&self) -> postui_core::prepare::PrepareContext {
        postui_core::prepare::PrepareContext {
            vars: postui_core::project::resolve(
                &self.variables,
                self.active_env.is_some().then_some(&self.env_values),
            ),
            default_headers: self.meta.default_headers.clone(),
        }
    }

    /// Switches the active environment, loading its values (or clearing
    /// them for `None`), and persists the choice to local state. Returns
    /// any warning from a failed environment load.
    pub fn set_env(&mut self, env: Option<String>) -> Vec<String> {
        let mut warnings = Vec::new();
        match env {
            None => {
                self.active_env = None;
                self.env_values = IndexMap::new();
            }
            Some(name) => match postui_core::project::load_environment(&self.root, &name) {
                Ok(values) => {
                    self.env_values = values;
                    self.active_env = Some(name);
                }
                Err(e) => {
                    warnings.push(format!("could not load environment {name:?}: {e}"));
                    self.active_env = None;
                    self.env_values = IndexMap::new();
                }
            },
        }
        self.persist_local_state(None);
        warnings
    }

    /// Best-effort save of the UI-owned local state: active environment,
    /// expanded sidebar dirs, and (when given) the currently-open request.
    /// A failed save never breaks interaction, so errors are dropped.
    pub fn persist_local_state(&self, open_request: Option<&str>) {
        let state = postui_core::project::LocalState {
            environment: self.active_env.clone(),
            open_request: open_request.map(|s| s.to_string()),
            expanded: self.expanded.iter().cloned().collect(),
        };
        let _ = postui_core::project::save_local_state(&self.root, &state);
    }

    /// The `open_request` recorded in the local state read at `open()`.
    pub fn local_open_request(&self) -> Option<String> {
        self.local_open_request.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_bare_dir_defaults_and_open_project_restores_state() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(warns.is_empty());
        assert_eq!(ctx.env_label(), "no env");
        assert!(ctx.environments.is_empty());

        postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"t\"\n").unwrap();
        postui_core::project::save_local_state(
            dir.path(),
            &postui_core::project::LocalState {
                environment: Some("qa".into()),
                open_request: Some("ping".into()),
                expanded: vec!["users".into()],
            },
        )
        .unwrap();
        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(warns.is_empty());
        assert_eq!(ctx.display_name(), "svc");
        assert_eq!(ctx.env_label(), "qa");
        assert_eq!(ctx.env_values["tok"], "t");
        assert!(ctx.expanded.contains("users"));
        assert_eq!(ctx.local_open_request().as_deref(), Some("ping"));
    }

    #[test]
    fn stale_env_in_local_state_degrades_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        postui_core::project::save_local_state(
            dir.path(),
            &postui_core::project::LocalState {
                environment: Some("gone".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert_eq!(ctx.env_label(), "no env");
        assert!(!warns.is_empty(), "stale env must be surfaced");
    }
}
