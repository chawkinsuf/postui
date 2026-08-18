//! `ProjectContext`: the open project's metadata, variables, environments,
//! and resolved environment values, plus the UI-owned bits of local state
//! (active environment, expanded sidebar dirs, per-env option selections)
//! that persist across runs.

use indexmap::IndexMap;
use postui_core::project::ProjectMeta;
use postui_core::varedit::EditError;
use postui_core::varmodel::{self, VarModel};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct ProjectContext {
    pub root: PathBuf,
    pub meta: ProjectMeta,
    pub model: VarModel,
    pub environments: Vec<String>,
    pub active_env: Option<String>,
    /// The active environment's data (empty when no env is active).
    pub env_data: varmodel::EnvData,
    /// env → name → secret value, loaded from `.local/secrets.toml`.
    pub secrets: IndexMap<String, IndexMap<String, String>>,
    /// `model` resolved against `env_data`, the active env's selections,
    /// and the active env's secrets. Recomputed by `refresh_resolved`
    /// whenever any of those inputs changes.
    pub resolved: varmodel::Resolved,
    pub expanded: std::collections::BTreeSet<String>,
    /// env → name → selected option key, loaded from `.local/state.toml`
    /// and kept in sync in-memory (no more disk round-trip needed to avoid
    /// clobbering it on persist — see `persist_local_state`).
    selections: IndexMap<String, IndexMap<String, String>>,
    /// mtimes of the files this context was built from, used by
    /// `reload_if_changed` to detect on-disk edits between UI polls.
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
    /// `open_request` from the local state loaded at `open()`, so the
    /// caller can restore the previously-open request. Not kept in sync
    /// afterward — it's a one-shot startup value.
    local_open_request: Option<String>,
}

/// Loads an environment's data, validating it against `model` (spec §1.2: a
/// flat value for an enumerated/secret name, or an `[options.*]` table for
/// an undeclared/secret name, is an error).
fn load_and_validate_env(
    root: &Path,
    name: &str,
    model: &VarModel,
) -> Result<varmodel::EnvData, String> {
    let env = postui_core::project::load_environment(root, name).map_err(|e| e.to_string())?;
    varmodel::validate_env(model, &env).map_err(|e| e.to_string())?;
    Ok(env)
}

/// Reads `path`'s contents; a missing file is treated as empty (the file
/// hasn't been created yet — e.g. a brand-new environment).
fn read_or_empty(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// Atomic write via temp file + rename in `path`'s own directory, matching
/// `storage::save_request`'s pattern (spec §5: writes atomic + immediate).
fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().expect("path always has a parent");
    std::fs::create_dir_all(parent)?;
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Builds the stamp vector `reload_if_changed` compares against: mtimes of
/// `project.toml`, `variables.toml`, the `environments/` dir, and the active
/// env file (a sentinel path with `None` when there is no active env).
fn stamp(root: &Path, active_env: &Option<String>) -> Vec<(PathBuf, Option<SystemTime>)> {
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
    /// state/secrets and the active environment's data. Never fails
    /// outright — any individual piece that can't be read degrades to a
    /// sane default and its problem is appended to the returned warnings.
    pub fn open(root: PathBuf) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();

        let meta = postui_core::project::load_meta(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read project.toml: {e}"));
            ProjectMeta::default()
        });
        let model = postui_core::project::load_variables(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read variables.toml: {e}"));
            VarModel::default()
        });
        let environments = postui_core::project::list_environments(&root);

        let local_state = postui_core::project::load_local_state(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read local state: {e}"));
            postui_core::project::LocalState::default()
        });

        let secrets = postui_core::project::load_secrets(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read secrets: {e}"));
            IndexMap::new()
        });

        let mut active_env = None;
        let mut env_data = varmodel::EnvData::default();
        if let Some(env) = local_state.environment {
            if !environments.contains(&env) {
                warnings.push(format!("saved environment {env:?} no longer exists"));
            } else {
                match load_and_validate_env(&root, &env, &model) {
                    Ok(data) => {
                        env_data = data;
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

        let mut ctx = ProjectContext {
            root,
            meta,
            model,
            environments,
            active_env,
            env_data,
            secrets,
            resolved: varmodel::Resolved::default(),
            expanded,
            selections: local_state.selections,
            stamps,
            local_open_request: local_state.open_request,
        };
        ctx.refresh_resolved();
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

    /// The key `selections`/`secrets` are keyed under for the active
    /// environment — the empty string when no env is active (spec: no-env
    /// selections/secrets live under that shared key).
    fn env_key(&self) -> String {
        self.active_env.clone().unwrap_or_default()
    }

    /// Recomputes `resolved` from `model`, `env_data`, and the active env's
    /// selections/secrets. Call after anything that changes any of those.
    pub fn refresh_resolved(&mut self) {
        let key = self.env_key();
        let empty_sel = IndexMap::new();
        let empty_secrets = IndexMap::new();
        let selections = self.selections.get(&key).unwrap_or(&empty_sel);
        let secrets = self.secrets.get(&key).unwrap_or(&empty_secrets);
        self.resolved = varmodel::resolve_env(&self.model, &self.env_data, selections, secrets);
    }

    /// `env`'s `name → selected option key` map (read-only, for any
    /// environment — not just the active one). Empty when nothing is
    /// recorded for `env`. Used by the Variable Manager grid to show each
    /// environment column's own selections side by side.
    pub fn selections_for(&self, env: &str) -> &IndexMap<String, String> {
        static EMPTY: std::sync::OnceLock<IndexMap<String, String>> = std::sync::OnceLock::new();
        self.selections
            .get(env)
            .unwrap_or_else(|| EMPTY.get_or_init(IndexMap::new))
    }

    /// Records `name`'s selection as `key` for the active env, persists
    /// local state, and re-resolves.
    pub fn set_selection(&mut self, name: &str, key: &str) {
        let env = self.env_key();
        self.set_selection_for(&env, name, key);
    }

    /// [`Self::set_selection`], for any environment (not just the active
    /// one) — the Variable Manager grid shows every environment's column
    /// side by side and lets the ✓ action target whichever column the
    /// cursor is on. Re-resolves only when `env` is the active one (the
    /// only environment `resolved` reflects); a non-active env's column
    /// recomputes fresh from `self.selections` on its next draw either way.
    pub fn set_selection_for(&mut self, env: &str, name: &str, key: &str) {
        self.selections
            .entry(env.to_string())
            .or_default()
            .insert(name.to_string(), key.to_string());
        self.persist_local_state_keep_open_request();
        // `env_key()` (not `active_env.as_deref()` directly) so the no-env
        // case matches too: `active_env` is `None` there, but every caller
        // (including `Self::set_selection`) addresses it by its storage key
        // `""` — comparing against `active_env` directly would never equal
        // `Some("")` and silently skip the resolve.
        if self.env_key() == env {
            self.refresh_resolved();
        }
    }

    /// Records `name`'s secret value for the active env, writes
    /// `.local/secrets.toml`, and re-resolves. `Err(msg)` — safe to toast,
    /// never the secret value itself — leaves everything (including the
    /// on-disk file and in-memory `secrets`) unchanged, matching
    /// `edit_variables`/`edit_env`'s build-then-commit failure contract
    /// (spec §5: a failed write must never persist a partial edit).
    pub fn set_secret(&mut self, name: &str, value: String) -> Result<(), String> {
        let env = self.env_key();
        self.set_secret_for(&env, name, value)
    }

    /// [`Self::set_secret`], for any environment (not just the active
    /// one) — the Variable Manager grid shows every environment's secret
    /// column side by side, so an edit in a non-active column must still
    /// land in that column's own env slot.
    pub fn set_secret_for(&mut self, env: &str, name: &str, value: String) -> Result<(), String> {
        let mut secrets = self.secrets.clone();
        secrets
            .entry(env.to_string())
            .or_default()
            .insert(name.to_string(), value);
        if self.can_persist() {
            postui_core::project::save_secrets(&self.root, &secrets).map_err(|e| e.to_string())?;
        }
        self.secrets = secrets;
        // See the matching comment in `set_selection_for`: compare against
        // `env_key()`, not `active_env` directly, so the no-env case (where
        // `active_env` is `None` but everything is keyed under `""`) still
        // re-resolves — otherwise the send-time secret prompt (spec §3)
        // would loop forever with no active environment, since the just-
        // confirmed secret would never actually resolve.
        if self.env_key() == env {
            self.refresh_resolved();
        }
        Ok(())
    }

    /// Builds the `PrepareContext` for sending: fully resolved variables
    /// (secret → selected option → env value → default, per spec §2) plus
    /// resolution metadata and the project's default headers.
    pub fn prepare_context(&self) -> postui_core::prepare::PrepareContext {
        postui_core::prepare::PrepareContext {
            vars: self.resolved.values.clone(),
            default_headers: self.meta.default_headers.clone(),
            meta: self.resolved.meta.clone(),
        }
    }

    /// Switches the active environment, re-reading its data (or clearing
    /// it for `None`). A missing or corrupt env file keeps the previous
    /// environment active and returns a warning instead of dropping to "no
    /// env". Does not persist — callers persist separately (see
    /// `Action::SwitchEnv`).
    pub fn set_env(&mut self, env: Option<String>) -> Vec<String> {
        let mut warnings = Vec::new();
        match env {
            None => {
                self.active_env = None;
                self.env_data = varmodel::EnvData::default();
                self.stamps = stamp(&self.root, &self.active_env);
            }
            Some(name) => match load_and_validate_env(&self.root, &name, &self.model) {
                Ok(data) => {
                    self.env_data = data;
                    self.active_env = Some(name);
                    self.stamps = stamp(&self.root, &self.active_env);
                }
                Err(e) => {
                    warnings.push(format!("could not load environment {name:?}: {e}"));
                }
            },
        }
        self.refresh_resolved();
        warnings
    }

    /// Compares fresh mtime stamps against those recorded at the last
    /// `open`/`set_env`/`reload_if_changed`; if anything differs, re-runs
    /// the load path for the pieces that changed (including secrets),
    /// keeping the current `active_env` if it still exists (otherwise
    /// degrading to no env with a warning). Parse/validation failures keep
    /// the previous good value for that file and are surfaced as warnings.
    /// Returns `(changed, warnings)`.
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
            Ok(model) => self.model = model,
            Err(e) => warnings.push(format!("could not read variables.toml: {e}")),
        }
        self.environments = postui_core::project::list_environments(&self.root);

        match postui_core::project::load_secrets(&self.root) {
            Ok(secrets) => self.secrets = secrets,
            Err(e) => warnings.push(format!("could not read secrets: {e}")),
        }

        if let Some(env) = self.active_env.clone() {
            if !self.environments.contains(&env) {
                warnings.push(format!("active environment {env:?} no longer exists"));
                self.active_env = None;
                self.env_data = varmodel::EnvData::default();
            } else {
                match load_and_validate_env(&self.root, &env, &self.model) {
                    Ok(data) => self.env_data = data,
                    Err(e) => warnings.push(format!("could not load environment {env:?}: {e}")),
                }
            }
        }

        self.stamps = stamp(&self.root, &self.active_env);
        self.refresh_resolved();
        (true, warnings)
    }

    /// Best-effort save of the UI-owned local state: active environment,
    /// expanded sidebar dirs, per-env option selections, and (when given)
    /// the currently-open request. A failed save never breaks interaction,
    /// so errors are dropped.
    pub fn persist_local_state(&self, open_request: Option<&str>) {
        if !self.can_persist() {
            return;
        }
        let state = postui_core::project::LocalState {
            environment: self.active_env.clone(),
            open_request: open_request.map(|s| s.to_string()),
            expanded: self.expanded.iter().cloned().collect(),
            selections: self.selections.clone(),
        };
        let _ = postui_core::project::save_local_state(&self.root, &state);
    }

    /// `persist_local_state`, but for callers (`set_selection`) that don't
    /// track the currently-open request themselves: reads just that one
    /// field back from disk first so it isn't clobbered, then writes
    /// everything else (environment/expanded/selections) straight from
    /// `self` — the authoritative in-memory copy, no need to round-trip it
    /// through disk first.
    fn persist_local_state_keep_open_request(&self) {
        if !self.can_persist() {
            return;
        }
        let open_request = postui_core::project::load_local_state(&self.root)
            .ok()
            .and_then(|s| s.open_request);
        self.persist_local_state(open_request.as_deref());
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

    /// Applies `f` to `variables.toml`'s current text (missing file =
    /// empty), validates the result against the active env (if any) so a
    /// bad edit is rejected instead of persisted, writes atomically, and
    /// reloads `model` + re-resolves. `Err(msg)` — safe to toast, never a
    /// secret value — leaves everything (including the on-disk file)
    /// unchanged.
    pub fn edit_variables(
        &mut self,
        f: impl FnOnce(&str) -> Result<String, EditError>,
    ) -> Result<(), String> {
        let path = self.root.join("variables.toml");
        let contents = read_or_empty(&path)?;
        let new_contents = f(&contents).map_err(|e| e.to_string())?;
        let new_model = varmodel::parse_variables(&new_contents).map_err(|e| e.to_string())?;
        if self.active_env.is_some() {
            varmodel::validate_env(&new_model, &self.env_data).map_err(|e| e.to_string())?;
        }
        atomic_write(&path, &new_contents).map_err(|e| e.to_string())?;

        self.model = new_model;
        self.stamps = stamp(&self.root, &self.active_env);
        self.refresh_resolved();
        Ok(())
    }

    /// Applies `f` to `environments/<env>.toml`'s current text (missing
    /// file = empty, e.g. a brand-new environment), validates the result
    /// against `model`, writes atomically, and — when `env` is the active
    /// environment — reloads `env_data` + re-resolves. `Err(msg)` leaves
    /// everything (including the on-disk file) unchanged.
    pub fn edit_env(
        &mut self,
        env: &str,
        f: impl FnOnce(&str) -> Result<String, EditError>,
    ) -> Result<(), String> {
        if env.contains('/') || postui_core::storage::validate_slug(env).is_err() {
            return Err(format!("invalid environment name: {env:?}"));
        }
        let path = self.root.join("environments").join(format!("{env}.toml"));
        let contents = read_or_empty(&path)?;
        let new_contents = f(&contents).map_err(|e| e.to_string())?;
        let new_env = varmodel::parse_environment(&new_contents).map_err(|e| e.to_string())?;
        varmodel::validate_env(&self.model, &new_env).map_err(|e| e.to_string())?;
        atomic_write(&path, &new_contents).map_err(|e| e.to_string())?;

        self.environments = postui_core::project::list_environments(&self.root);
        if self.active_env.as_deref() == Some(env) {
            self.env_data = new_env;
        }
        self.stamps = stamp(&self.root, &self.active_env);
        self.refresh_resolved();
        Ok(())
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
        assert_eq!(ctx.env_data.values["tok"], "t");
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
        assert_eq!(ctx.env_data.values["tok"], "2");
        assert_eq!(ctx.env_label(), "qa");
    }

    #[test]
    fn reload_with_broken_file_warns_and_keeps_previous_values() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(dir.path().join("variables.toml"), "[a]\ndefault = \"1\"\n").unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        assert_eq!(ctx.model.vars["a"].default.as_deref(), Some("1"));
        std::fs::write(dir.path().join("variables.toml"), "not toml [").unwrap();
        bump_mtime(&dir.path().join("variables.toml"));
        let (_, warns) = ctx.reload_if_changed();
        assert!(!warns.is_empty(), "parse failure surfaced");
        assert_eq!(
            ctx.model.vars["a"].default.as_deref(),
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

    // -----------------------------------------------------------------
    // stage-6 integration: model/env_data/secrets/resolved
    // -----------------------------------------------------------------

    #[test]
    fn open_loads_secrets_and_resolved_reflects_a_selection_from_state() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[user]\n[user.options.alice]\nvalue = \"1001\"\n[user.options.bob]\nvalue = \"2002\"\n\n[api_key]\nsecret = true\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "").unwrap();

        let mut selections = IndexMap::new();
        let mut qa_sel = IndexMap::new();
        qa_sel.insert("user".to_string(), "bob".to_string());
        selections.insert("qa".to_string(), qa_sel);
        postui_core::project::save_local_state(
            dir.path(),
            &postui_core::project::LocalState {
                environment: Some("qa".into()),
                selections,
                ..Default::default()
            },
        )
        .unwrap();

        let mut secrets = IndexMap::new();
        let mut qa_secrets = IndexMap::new();
        qa_secrets.insert("api_key".to_string(), "sk-test".to_string());
        secrets.insert("qa".to_string(), qa_secrets);
        postui_core::project::save_secrets(dir.path(), &secrets).unwrap();

        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(ctx.resolved.values["user"], "2002");
        assert_eq!(ctx.resolved.values["api_key"], "sk-test");
    }

    #[test]
    fn set_selection_persists_and_re_resolves() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[user]\n[user.options.alice]\nvalue = \"1001\"\n[user.options.bob]\nvalue = \"2002\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "").unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        assert!(!ctx.resolved.values.contains_key("user"));

        ctx.set_selection("user", "alice");
        assert_eq!(ctx.resolved.values["user"], "1001");

        let state = postui_core::project::load_local_state(dir.path()).unwrap();
        assert_eq!(state.selections["qa"]["user"], "alice");
    }

    #[test]
    fn set_secret_writes_secrets_file_and_resolves() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[api_key]\nsecret = true\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "").unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        assert!(!ctx.resolved.values.contains_key("api_key"));

        ctx.set_secret("api_key", "sk-live".into()).unwrap();
        assert_eq!(ctx.resolved.values["api_key"], "sk-live");

        let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
        assert_eq!(secrets["qa"]["api_key"], "sk-live");
    }

    /// Regression: `active_env.as_deref() == Some(env)` never matched with
    /// no active environment (`active_env` is `None`, but everything is
    /// keyed under `""`) — `resolved` silently kept the pre-secret
    /// `MissingSecret` state, so the send-time secret prompt (Task 16,
    /// spec §3) would loop forever on a fresh clone with no env selected.
    #[test]
    fn set_secret_resolves_immediately_with_no_active_environment() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[api_key]\nsecret = true\n",
        )
        .unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        assert!(ctx.active_env.is_none());
        assert!(!ctx.resolved.values.contains_key("api_key"));

        ctx.set_secret("api_key", "sk-live".into()).unwrap();
        assert_eq!(ctx.resolved.values["api_key"], "sk-live");

        let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
        assert_eq!(secrets[""]["api_key"], "sk-live");
    }

    #[test]
    fn edit_variables_applies_upsert_var_and_preserves_comment() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "# variables.toml\n\n[base_url]\ndescription = \"API root\"\ndefault = \"http://localhost:8080\"\n",
        )
        .unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());

        ctx.edit_variables(|doc| {
            postui_core::varedit::upsert_var(doc, "base_url", None, Some("http://localhost:9090"))
        })
        .unwrap();

        assert_eq!(
            ctx.model.vars["base_url"].default.as_deref(),
            Some("http://localhost:9090")
        );
        let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
        assert!(on_disk.contains("# variables.toml"), "{on_disk}");
        assert!(on_disk.contains("http://localhost:9090"), "{on_disk}");
    }

    #[test]
    fn reload_with_broken_env_option_table_warns_and_keeps_previous() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[user]\n[user.options.alice]\nvalue = \"1001\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.alice]\nvalue = \"9001\"\n",
        )
        .unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        assert_eq!(ctx.env_data.options["user"]["alice"]["value"], "9001");

        // break it: an [options.*] table for an undeclared name
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.nope.x]\nvalue = \"1\"\n",
        )
        .unwrap();
        bump_mtime(&dir.path().join("environments/qa.toml"));
        let (changed, warns) = ctx.reload_if_changed();
        assert!(changed && !warns.is_empty());
        assert_eq!(
            ctx.env_data.options["user"]["alice"]["value"], "9001",
            "previous good env data kept"
        );
    }
}
