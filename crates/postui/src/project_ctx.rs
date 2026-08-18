//! `ProjectContext`: the open project's metadata, variables, environments,
//! and resolved environment values, plus the UI-owned bits of local state
//! (active environment, expanded sidebar dirs) that persist across runs.

use indexmap::IndexMap;
use postui_core::project::ProjectMeta;
use postui_core::varmodel::{self, VarModel};
use std::path::PathBuf;
use std::time::SystemTime;

pub struct ProjectContext {
    pub root: PathBuf,
    pub meta: ProjectMeta,
    pub variables: VarModel,
    pub environments: Vec<String>,
    pub active_env: Option<String>,
    pub env_values: IndexMap<String, String>,
    pub expanded: std::collections::BTreeSet<String>,
    /// mtimes of the files this context was built from, used by
    /// `reload_if_changed` to detect on-disk edits between UI polls.
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
    /// `open_request` from the local state loaded at `open()`, so the
    /// caller can restore the previously-open request. Not kept in sync
    /// afterward — it's a one-shot startup value.
    local_open_request: Option<String>,
}

/// Loads an environment's flat values, validating it against `model` (spec
/// §1.2: a flat value for an enumerated/secret name, or an `[options.*]`
/// table for an undeclared/secret name, is an error). Returns just the flat
/// `values` map — option tables aren't consumed by `ProjectContext` yet
/// (selections/secrets wiring lands with the full stage-6 integration).
fn load_and_validate_env(
    root: &std::path::Path,
    name: &str,
    model: &VarModel,
) -> Result<IndexMap<String, String>, String> {
    let env = postui_core::project::load_environment(root, name).map_err(|e| e.to_string())?;
    varmodel::validate_env(model, &env).map_err(|e| e.to_string())?;
    Ok(env.values)
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Builds the stamp vector `reload_if_changed` compares against: mtimes of
/// `project.toml`, `variables.toml`, the `environments/` dir, and the active
/// env file (a sentinel path with `None` when there is no active env).
fn stamp(
    root: &std::path::Path,
    active_env: &Option<String>,
) -> Vec<(PathBuf, Option<SystemTime>)> {
    vec![
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
    ]
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
            VarModel::default()
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
                match load_and_validate_env(&root, &env, &variables) {
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

        let stamps = stamp(&root, &active_env);

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
    ///
    /// Temporary: resolves with empty selections/secrets until the full
    /// stage-6 integration (task 8) wires `.local/` selections and secrets
    /// through `ProjectContext`.
    pub fn prepare_context(&self) -> postui_core::prepare::PrepareContext {
        let env = varmodel::EnvData {
            values: self.env_values.clone(),
            options: IndexMap::new(),
        };
        let resolved = varmodel::resolve_env(
            &self.variables,
            &env,
            &varmodel::Selections::new(),
            &varmodel::SecretValues::new(),
        );
        postui_core::prepare::PrepareContext {
            vars: resolved.values,
            default_headers: self.meta.default_headers.clone(),
            meta: resolved.meta,
        }
    }

    /// Switches the active environment, re-reading its file and loading
    /// its values (or clearing them for `None`). A missing or corrupt env
    /// file keeps the previous environment active and returns a warning
    /// instead of dropping to "no env". Does not persist — callers persist
    /// separately (see `Action::SwitchEnv`).
    pub fn set_env(&mut self, env: Option<String>) -> Vec<String> {
        let mut warnings = Vec::new();
        match env {
            None => {
                self.active_env = None;
                self.env_values = IndexMap::new();
                self.stamps = stamp(&self.root, &self.active_env);
            }
            Some(name) => match load_and_validate_env(&self.root, &name, &self.variables) {
                Ok(values) => {
                    self.env_values = values;
                    self.active_env = Some(name);
                    self.stamps = stamp(&self.root, &self.active_env);
                }
                Err(e) => {
                    warnings.push(format!("could not load environment {name:?}: {e}"));
                }
            },
        }
        warnings
    }

    /// Compares fresh mtime stamps against those recorded at the last
    /// `open`/`set_env`/`reload_if_changed`; if anything differs, re-runs
    /// the load path for the pieces that changed, keeping the current
    /// `active_env` if it still exists (otherwise degrading to no env with
    /// a warning). Parse failures keep the previous good value for that
    /// file and are surfaced as warnings. Returns `(changed, warnings)`.
    pub fn reload_if_changed(&mut self) -> (bool, Vec<String>) {
        let fresh = stamp(&self.root, &self.active_env);
        if fresh == self.stamps {
            return (false, Vec::new());
        }

        let mut warnings = Vec::new();

        match postui_core::project::load_meta(&self.root) {
            Ok(meta) => self.meta = meta,
            Err(e) => warnings.push(format!("could not read project.toml: {e}")),
        }
        match postui_core::project::load_variables(&self.root) {
            Ok(vars) => self.variables = vars,
            Err(e) => warnings.push(format!("could not read variables.toml: {e}")),
        }
        self.environments = postui_core::project::list_environments(&self.root);

        if let Some(env) = self.active_env.clone() {
            if !self.environments.contains(&env) {
                warnings.push(format!("active environment {env:?} no longer exists"));
                self.active_env = None;
                self.env_values = IndexMap::new();
            } else {
                match load_and_validate_env(&self.root, &env, &self.variables) {
                    Ok(values) => self.env_values = values,
                    Err(e) => warnings.push(format!("could not load environment {env:?}: {e}")),
                }
            }
        }

        self.stamps = stamp(&self.root, &self.active_env);
        (true, warnings)
    }

    /// Best-effort save of the UI-owned local state: active environment,
    /// expanded sidebar dirs, and (when given) the currently-open request.
    /// A failed save never breaks interaction, so errors are dropped.
    pub fn persist_local_state(&self, open_request: Option<&str>) {
        if !self.can_persist() {
            return;
        }
        // `selections` isn't tracked in-memory by `ProjectContext` yet (that
        // lands with the full stage-6 integration in task 8), so start from
        // whatever is on disk to avoid clobbering it here.
        let mut state = postui_core::project::load_local_state(&self.root).unwrap_or_default();
        state.environment = self.active_env.clone();
        state.open_request = open_request.map(|s| s.to_string());
        state.expanded = self.expanded.iter().cloned().collect();
        let _ = postui_core::project::save_local_state(&self.root, &state);
    }

    /// Whether this context's root is persistable: a bare (empty) root —
    /// as constructed when the app starts outside any project — must never
    /// write a stray `./.local/state.toml` relative to the process's cwd.
    pub fn can_persist(&self) -> bool {
        !self.root.as_os_str().is_empty()
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
                ..Default::default()
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

    #[test]
    fn bare_root_context_cannot_persist_local_state() {
        let (ctx, _warns) = ProjectContext::open(PathBuf::new());
        assert!(
            !ctx.can_persist(),
            "a bare (empty) root must not be persistable"
        );
    }

    fn bump_mtime(p: &std::path::Path) {
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        let f = std::fs::File::options().append(true).open(p).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn reload_picks_up_changed_variables_and_keeps_active_env() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"1\"\n").unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        let (changed, _) = ctx.reload_if_changed();
        assert!(!changed, "nothing changed yet");
        std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"2\"\n").unwrap();
        bump_mtime(&dir.path().join("environments/qa.toml"));
        let (changed, warns) = ctx.reload_if_changed();
        assert!(changed && warns.is_empty());
        assert_eq!(ctx.env_values["tok"], "2");
        assert_eq!(ctx.env_label(), "qa");
    }

    #[test]
    fn reload_with_broken_file_warns_and_keeps_previous_values() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(dir.path().join("variables.toml"), "[a]\ndefault = \"1\"\n").unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        assert_eq!(ctx.variables.vars["a"].default.as_deref(), Some("1"));
        std::fs::write(dir.path().join("variables.toml"), "not toml [").unwrap();
        bump_mtime(&dir.path().join("variables.toml"));
        let (_, warns) = ctx.reload_if_changed();
        assert!(!warns.is_empty(), "parse failure surfaced");
        assert_eq!(
            ctx.variables.vars["a"].default.as_deref(),
            Some("1"),
            "previous good state kept"
        );
    }

    #[test]
    fn deleted_active_env_degrades_to_no_env_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"1\"\n").unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        std::fs::remove_file(dir.path().join("environments/qa.toml")).unwrap();
        // no mtime bump needed: the active env file's stamp goes Some -> None,
        // which is itself a difference (bump_mtime can't open a directory anyway)
        let (changed, warns) = ctx.reload_if_changed();
        assert!(changed && !warns.is_empty());
        assert_eq!(ctx.env_label(), "no env");
    }
}
