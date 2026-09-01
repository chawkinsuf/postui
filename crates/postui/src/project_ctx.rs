//! `ProjectContext`: the open project's metadata, variables, environments,
//! and resolved environment values, plus the UI-owned bits of local state
//! (active environment, expanded sidebar dirs, per-env option selections)
//! that persist across runs.

use indexmap::IndexMap;
use postui_core::migrate::{self, MigrationOutcome};
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
    /// Every space in display order (see `postui_core::project::list_spaces`).
    pub spaces: Vec<String>,
    /// The space the sidebar shows. Always one of `spaces` (repaired on
    /// open/reload when the stored one vanished).
    pub active_space: String,
    /// space → the request last open in it, from `.local/state.toml`.
    space_open: IndexMap<String, String>,
    /// env → name → selected option key, loaded from `.local/state.toml`
    /// and kept in sync in-memory (no more disk round-trip needed to avoid
    /// clobbering it on persist — see `persist_local_state`).
    selections: IndexMap<String, IndexMap<String, String>>,
    /// Shared selectors' `name → selected option key` — one global pick,
    /// independent of the active environment, same load/persist story as
    /// `selections`.
    shared_selections: IndexMap<String, String>,
    /// mtimes of the files this context was built from, used by
    /// `reload_if_changed` to detect on-disk edits between UI polls.
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
    /// `open_request` from the local state loaded at `open()`, so the
    /// caller can restore the previously-open request. Not kept in sync
    /// afterward — it's a one-shot startup value.
    local_open_request: Option<String>,
    /// The editor/response split token from `.local/state.toml` (see
    /// `crate::split::SplitState::to_token`). Unlike `local_open_request`
    /// this *is* kept in sync: `App` writes it here before persisting
    /// whenever the split changes.
    pub main_split: Option<String>,
    /// Set when the on-disk files still use stage-6 syntax (spec §3.3):
    /// the conversion the confirm modal offers to apply. While it is
    /// `Some`, `model`/`env_data` are left at their defaults — nothing
    /// parses the legacy shapes, so the variables are simply inert until
    /// the user applies or declines.
    pending_migration: Option<MigrationOutcome>,
    /// Set once the user declines: the files stay legacy (so they keep
    /// failing `needs_migration`), but we stop re-offering the migration
    /// on every reload for the rest of this session.
    migration_declined: bool,
}

/// Loads an environment's data, validating it against `model` (spec §3.2:
/// a flat value for a secret, a selector, or a selector field, or an
/// `[options.*]` table for an undeclared selector, is an error).
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
/// `pub(crate)`: also used by `App::apply_undo_step` to replay a raw
/// `FileStates` step's text onto disk.
pub(crate) fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().expect("path always has a parent");
    std::fs::create_dir_all(parent)?;
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Drops any option in `selections` naming an option that no longer exists
/// among `env`'s options for that selector (spec §1.3: "a selection naming a
/// missing option key degrades to unselected ... with a toast on load").
/// `resolve_env` already degrades a stale selection to `NeedsSelection` on
/// its own, so this isn't required for correct resolution — it's purely so
/// the user gets told once, and so the stale option doesn't linger forever
/// in `.local/state.toml`. Returns one warning per cleared option, worded to
/// match the finding's example exactly.
fn prune_stale_selections(
    model: &VarModel,
    env_data: &varmodel::EnvData,
    env_label: &str,
    selections: &mut IndexMap<String, String>,
) -> Vec<String> {
    let stale: Vec<String> = selections
        .iter()
        .filter(|(selector, option)| {
            let entry_exists = model.selectors.contains_key(selector.as_str())
                && varmodel::selector_options(env_data, selector)
                    .is_some_and(|options| options.contains_key(option.as_str()));
            !entry_exists
        })
        .map(|(name, _)| name.clone())
        .collect();

    stale
        .into_iter()
        .map(|name| {
            selections.shift_remove(&name);
            format!("selection for `{name}` no longer exists in env `{env_label}` \u{2014} cleared")
        })
        .collect()
}

/// [`prune_stale_selections`]'s twin for the global shared table: drops any
/// pick naming a shared selector (or option) the model no longer has.
fn prune_stale_shared_selections(
    model: &VarModel,
    shared_selections: &mut IndexMap<String, String>,
) -> Vec<String> {
    let stale: Vec<String> = shared_selections
        .iter()
        .filter(|(selector, option)| {
            let entry_exists = model
                .selectors
                .get(selector.as_str())
                .is_some_and(|d| d.shared)
                && model
                    .options
                    .get(selector.as_str())
                    .is_some_and(|options| options.contains_key(option.as_str()));
            !entry_exists
        })
        .map(|(name, _)| name.clone())
        .collect();

    stale
        .into_iter()
        .map(|name| {
            shared_selections.shift_remove(&name);
            format!("selection for `{name}` no longer exists \u{2014} cleared")
        })
        .collect()
}

/// `environments/<env>.toml` under `root`.
fn env_path(root: &Path, env: &str) -> PathBuf {
    root.join("environments").join(format!("{env}.toml"))
}

/// The raw text of `variables.toml` plus every environment document, as
/// [`migrate`] wants them: unparsed, so stage-6 shapes survive the trip.
/// Unreadable files read as empty — a file we can't read has nothing to
/// migrate, and the ordinary load path reports the read failure.
fn raw_docs(root: &Path) -> (String, Vec<(String, String)>) {
    let vars = read_or_empty(&root.join("variables.toml")).unwrap_or_default();
    let envs = postui_core::project::list_environments(root)
        .into_iter()
        .map(|env| {
            let doc = read_or_empty(&env_path(root, &env)).unwrap_or_default();
            (env, doc)
        })
        .collect();
    (vars, envs)
}

/// Probes `root` for stage-6 syntax (spec §3.3). Returns `(legacy,
/// outcome, warnings)`: `legacy` is true whenever the files need
/// migrating — even when the conversion itself can't be computed, since
/// the model can't read those files either way — and `outcome` is the
/// conversion to offer, absent when `migrate` refused to convert (its
/// reason is the warning).
fn probe_migration(root: &Path) -> (bool, Option<MigrationOutcome>, Vec<String>) {
    let (vars, envs) = raw_docs(root);
    if !migrate::needs_migration(&vars, &envs) {
        return (false, None, Vec::new());
    }
    match migrate::migrate(&vars, &envs) {
        Ok(outcome) => (true, Some(outcome), Vec::new()),
        Err(e) => (
            true,
            None,
            vec![format!(
                "variables use the old format and can't be converted automatically: {e}"
            )],
        ),
    }
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Builds the stamp vector `reload_if_changed` compares against: mtimes of
/// `project.toml`, `variables.toml`, the `environments/` dir, the active
/// env file (a sentinel path with `None` when there is no active env), and
/// the `requests/` dir (so an external `mkdir` of a space is noticed).
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
        (root.join("requests"), mtime(&root.join("requests"))),
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
        // The trash only backs this session's undo; a fresh open starts clean.
        if !root.as_os_str().is_empty()
            && let Err(e) = postui_core::trash::empty(&root)
        {
            warnings.push(format!("could not empty .local/trash: {e}"));
        }
        let spaces = postui_core::project::list_spaces(&root, &meta);
        let (legacy, pending_migration, migration_warnings) = probe_migration(&root);
        warnings.extend(migration_warnings);
        let model = if legacy {
            // Legacy files parse as nothing useful (and would only produce
            // confusing errors): leave the variables inert until the user
            // applies or declines the migration.
            VarModel::default()
        } else {
            postui_core::project::load_variables(&root).unwrap_or_else(|e| {
                warnings.push(format!("could not read variables.toml: {e}"));
                VarModel::default()
            })
        };
        let environments = postui_core::project::list_environments(&root);

        let local_state = postui_core::project::load_local_state(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read local state: {e}"));
            postui_core::project::LocalState::default()
        });

        let first_space = spaces
            .first()
            .cloned()
            .unwrap_or_else(|| postui_core::project::DEFAULT_SPACE.to_string());
        let open_request_space = local_state
            .open_request
            .as_deref()
            .and_then(postui_core::storage::space_of)
            .map(str::to_string)
            .filter(|s| spaces.contains(s));
        let active_space = match local_state.space.clone() {
            Some(s) if spaces.contains(&s) => s,
            Some(s) => {
                warnings.push(format!("saved space {s:?} no longer exists"));
                open_request_space
                    .clone()
                    .unwrap_or_else(|| first_space.clone())
            }
            None => open_request_space
                .clone()
                .unwrap_or_else(|| first_space.clone()),
        };
        // The request to restore must live in the active space; a stale
        // `open_request` from another space yields to that space's own entry.
        let local_open_request = match local_state.open_request.clone() {
            Some(r) if postui_core::storage::space_of(&r) == Some(active_space.as_str()) => Some(r),
            _ => local_state.space_open.get(&active_space).cloned(),
        };

        let secrets = postui_core::project::load_secrets(&root).unwrap_or_else(|e| {
            warnings.push(format!("could not read secrets: {e}"));
            IndexMap::new()
        });

        let mut active_env = None;
        let mut env_data = varmodel::EnvData::default();
        if let Some(env) = local_state.environment {
            if !environments.contains(&env) {
                warnings.push(format!("saved environment {env:?} no longer exists"));
            } else if legacy {
                // Same reasoning as `model` above: the env file is legacy
                // too, so it stays unread — but the environment is still
                // the active one, so applying the migration loads it.
                active_env = Some(env);
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
            spaces,
            active_space,
            space_open: local_state.space_open,
            selections: local_state.selections,
            shared_selections: local_state.shared_selections,
            stamps,
            local_open_request,
            main_split: local_state.main_split,
            pending_migration,
            migration_declined: false,
        };
        // `legacy` leaves `model`/`env_data` empty, which would make
        // every recorded selection look stale: pruning there would wipe
        // (and persist the loss of) selections the migration is about to
        // carry over, and toast a bogus warning for each one.
        if let Some(env) = ctx.active_env.clone().filter(|_| !legacy) {
            let stale = prune_stale_selections(
                &ctx.model,
                &ctx.env_data,
                &env,
                ctx.selections.entry(env.clone()).or_default(),
            );
            if !stale.is_empty() {
                let open_request = ctx.local_open_request.clone();
                ctx.persist_local_state(open_request.as_deref());
            }
            warnings.extend(stale);
        }
        if !legacy {
            let stale = prune_stale_shared_selections(&ctx.model, &mut ctx.shared_selections);
            if !stale.is_empty() {
                let open_request = ctx.local_open_request.clone();
                ctx.persist_local_state(open_request.as_deref());
            }
            warnings.extend(stale);
        }

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
        let empty_secrets = IndexMap::new();
        // The env's own selections plus the global picks for shared
        // selectors — shared wins, since a shared selector's pick never
        // belongs to one environment.
        let mut selections = self.selections.get(&key).cloned().unwrap_or_default();
        for (name, option) in &self.shared_selections {
            selections.insert(name.clone(), option.clone());
        }
        let secrets = self.secrets.get(&key).unwrap_or(&empty_secrets);
        self.resolved = varmodel::resolve_env(&self.model, &self.env_data, &selections, secrets);
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

    /// The global `selector → selected option key` map for shared
    /// selectors (read-only; write through [`Self::set_selection_for`]).
    pub fn shared_selections(&self) -> &IndexMap<String, String> {
        &self.shared_selections
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
        // A shared selector has one global pick: whichever env column the
        // gesture came from, the selection lands in the shared table and
        // applies everywhere.
        if self.model.selectors.get(name).is_some_and(|d| d.shared) {
            self.shared_selections
                .insert(name.to_string(), key.to_string());
            self.persist_local_state_keep_open_request();
            self.refresh_resolved();
            return;
        }
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

    /// Drops `name`'s recorded selection for `env`, if any — used when the
    /// option key it names is deleted out from under it (finding 3: a
    /// deleted-while-selected option would otherwise leave a stale
    /// selection option; `resolve_env` already degrades that to
    /// `NeedsSelection` harmlessly, but clearing it here keeps local state
    /// tidy rather than accumulating dead options). A no-op if `name` has
    /// no selection recorded for `env`.
    pub fn clear_selection_for(&mut self, env: &str, name: &str) {
        // Both homes: `name`'s global pick when it is (or was — the caller
        // may have just deleted the declaration) a shared selector, and
        // `env`'s own entry otherwise. The two never coexist.
        let removed_shared = self.shared_selections.shift_remove(name).is_some();
        let removed_env = self
            .selections
            .get_mut(env)
            .and_then(|sel| sel.shift_remove(name))
            .is_some();
        if !removed_shared && !removed_env {
            return;
        }
        self.persist_local_state_keep_open_request();
        if removed_shared || self.env_key() == env {
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

    /// Removes `name`'s stored secret value for `env` (memory and
    /// `.local/secrets.toml` both), a quiet no-op when nothing is stored —
    /// the write-through twin of [`Self::set_secret_for`], with its same
    /// build-then-commit failure contract.
    pub fn remove_secret_for(&mut self, env: &str, name: &str) -> Result<(), String> {
        let mut secrets = self.secrets.clone();
        if secrets
            .get_mut(env)
            .and_then(|m| m.shift_remove(name))
            .is_none()
        {
            return Ok(());
        }
        if self.can_persist() {
            postui_core::project::save_secrets(&self.root, &secrets).map_err(|e| e.to_string())?;
        }
        self.secrets = secrets;
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

    /// Re-lists spaces from `meta` + disk. A vanished active space falls
    /// back to the first one; returns a warning when that happened.
    pub fn reload_spaces(&mut self) -> Option<String> {
        self.spaces = postui_core::project::list_spaces(&self.root, &self.meta);
        if self.spaces.contains(&self.active_space) {
            return None;
        }
        let gone = std::mem::take(&mut self.active_space);
        self.active_space = self
            .spaces
            .first()
            .cloned()
            .unwrap_or_else(|| postui_core::project::DEFAULT_SPACE.to_string());
        Some(format!("space {gone:?} no longer exists"))
    }

    pub fn set_active_space(&mut self, name: &str) -> bool {
        if !self.spaces.iter().any(|s| s == name) {
            return false;
        }
        self.active_space = name.to_string();
        true
    }

    /// Remembers `slug` as the active space's open request (`None` clears
    /// the entry). Only slugs actually inside the active space are kept.
    pub fn record_space_open(&mut self, slug: Option<&str>) {
        match slug {
            Some(s) if postui_core::storage::space_of(s) == Some(self.active_space.as_str()) => {
                self.space_open
                    .insert(self.active_space.clone(), s.to_string());
            }
            _ => {
                self.space_open.shift_remove(&self.active_space);
            }
        }
    }

    pub fn space_open_for(&self, space: &str) -> Option<String> {
        self.space_open.get(space).cloned()
    }

    /// Drops everything local state remembers about `name`.
    pub fn forget_space(&mut self, name: &str) {
        self.space_open.shift_remove(name);
        let prefix = format!("{name}/");
        self.expanded
            .retain(|p| !p.starts_with(&prefix) && p != name);
    }

    /// Re-keys local state after a space rename (the caller has already
    /// renamed on disk). Run this *before* any re-list (`reload_spaces`,
    /// or `reload_if_changed`, which calls it): a re-list drops an active
    /// space it no longer finds, and the old name is gone from disk by
    /// then.
    pub fn rename_space_state(&mut self, from: &str, to: &str) {
        let from_prefix = format!("{from}/");
        let to_prefix = format!("{to}/");
        if let Some(open) = self.space_open.shift_remove(from)
            && let Some(rest) = open.strip_prefix(&from_prefix)
        {
            self.space_open
                .insert(to.to_string(), format!("{to_prefix}{rest}"));
        }
        if self.active_space == from {
            self.active_space = to.to_string();
        }
        self.expanded = self
            .expanded
            .iter()
            .map(|p| match p.strip_prefix(&from_prefix) {
                Some(rest) => format!("{to_prefix}{rest}"),
                None if p == from => to.to_string(),
                None => p.clone(),
            })
            .collect();
    }

    /// In-memory cascade of an environment delete: its selections and
    /// secrets go; the active env is cleared if it was the one deleted.
    /// Persisting (state, secrets file) is the caller's job.
    pub fn remove_env_state(&mut self, name: &str) {
        self.selections.shift_remove(name);
        self.secrets.shift_remove(name);
        if self.active_env.as_deref() == Some(name) {
            self.active_env = None;
            self.env_data = varmodel::EnvData::default();
        }
    }

    /// In-memory cascade of an environment rename.
    pub fn rename_env_state(&mut self, from: &str, to: &str) {
        if let Some(sel) = self.selections.shift_remove(from) {
            self.selections.insert(to.to_string(), sel);
        }
        if let Some(sec) = self.secrets.shift_remove(from) {
            self.secrets.insert(to.to_string(), sec);
        }
        if self.active_env.as_deref() == Some(from) {
            self.active_env = Some(to.to_string());
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

    /// Forces the next [`Self::reload_if_changed`] to do a full reload,
    /// whatever the mtime stamps say. The file-level undo/redo arms need
    /// it: they rewrite files and then re-stamp through `set_env`, so the
    /// stamps can already match by the time the reload runs — and pieces
    /// that only the reload path re-reads (secrets, above all) would stay
    /// stale in memory.
    pub fn invalidate_stamps(&mut self) {
        self.stamps.clear();
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
        let (legacy, pending, migration_warnings) = probe_migration(&self.root);
        warnings.extend(migration_warnings);
        self.pending_migration = if self.migration_declined {
            None
        } else {
            pending
        };
        if legacy {
            self.model = VarModel::default();
            self.env_data = varmodel::EnvData::default();
        } else {
            match postui_core::project::load_variables(&self.root) {
                Ok(model) => self.model = model,
                Err(e) => warnings.push(format!("could not read variables.toml: {e}")),
            }
        }
        self.environments = postui_core::project::list_environments(&self.root);
        if let Some(w) = self.reload_spaces() {
            warnings.push(w);
        }

        match postui_core::project::load_secrets(&self.root) {
            Ok(secrets) => self.secrets = secrets,
            Err(e) => warnings.push(format!("could not read secrets: {e}")),
        }

        if let Some(env) = self.active_env.clone() {
            if !self.environments.contains(&env) {
                warnings.push(format!("active environment {env:?} no longer exists"));
                self.active_env = None;
                self.env_data = varmodel::EnvData::default();
            } else if !legacy {
                match load_and_validate_env(&self.root, &env, &self.model) {
                    Ok(data) => self.env_data = data,
                    Err(e) => warnings.push(format!("could not load environment {env:?}: {e}")),
                }
            }
        }

        // See `open`: an unparsed legacy model must never prune.
        if let Some(env) = self.active_env.clone().filter(|_| !legacy) {
            let stale = prune_stale_selections(
                &self.model,
                &self.env_data,
                &env,
                self.selections.entry(env.clone()).or_default(),
            );
            if !stale.is_empty() {
                self.persist_local_state_keep_open_request();
            }
            warnings.extend(stale);
        }
        if !legacy {
            let stale = prune_stale_shared_selections(&self.model, &mut self.shared_selections);
            if !stale.is_empty() {
                self.persist_local_state_keep_open_request();
            }
            warnings.extend(stale);
        }

        self.stamps = stamp(&self.root, &self.active_env);
        self.refresh_resolved();
        (true, warnings)
    }

    /// The stage-6 → stage-7 conversion waiting to be confirmed (spec
    /// §3.3), computed at `open`/`reload_if_changed` from the raw file
    /// texts. `None` once applied or declined, and for any project already
    /// in the new format.
    pub fn pending_migration(&self) -> Option<&MigrationOutcome> {
        self.pending_migration.as_ref()
    }

    /// Applies the pending migration: each rewritten file is copied to
    /// `<file>.bak` first (a plain write — it's the safety copy, not the
    /// live file), then the new text is written atomically, and
    /// `environments/default.toml` is created when the conversion needs
    /// somewhere to put the migrated options. Reloads everything
    /// afterwards, so the model comes up on the new format. `Ok(notes)`
    /// are the conversion's human-readable notes, for a toast.
    pub fn apply_migration(&mut self) -> Result<Vec<String>, String> {
        let Some(outcome) = self.pending_migration.take() else {
            return Err("no migration is pending".to_string());
        };
        let write = |path: &Path, text: &str| -> Result<(), String> {
            let backup = path.with_extension("toml.bak");
            // Only ever back up once: retrying after a partly-applied
            // attempt would otherwise copy the already-migrated text over
            // the only surviving copy of the original.
            if path.exists() && !backup.exists() {
                let existing = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                std::fs::write(&backup, existing).map_err(|e| e.to_string())?;
            }
            atomic_write(path, text).map_err(|e| e.to_string())
        };

        let result = (|| -> Result<(), String> {
            if let Some(text) = &outcome.variables {
                write(&self.root.join("variables.toml"), text)?;
            }
            for (env, text) in &outcome.envs {
                write(&env_path(&self.root, env), text)?;
            }
            if let Some(text) = &outcome.new_default_env {
                write(&env_path(&self.root, "default"), text)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Keep it pending so the user can retry (and so a partial
            // write is still recognized as legacy on the next probe).
            self.pending_migration = Some(outcome);
            return Err(e);
        }

        // Force a full reload: the stamps recorded before the rewrite say
        // nothing changed for files we just replaced in the same instant.
        self.stamps.clear();
        let (_, warnings) = self.reload_if_changed();
        let mut notes = outcome.notes;
        notes.extend(warnings);
        Ok(notes)
    }

    /// The confirm modal's "Not now": leaves every file untouched and the
    /// variables inert (the model stays `Default`), and stops re-offering
    /// the migration for the rest of this session.
    pub fn decline_migration(&mut self) {
        self.pending_migration = None;
        self.migration_declined = true;
    }

    /// Best-effort save of the UI-owned local state: active environment,
    /// expanded sidebar dirs, per-env option selections, and (when given)
    /// the currently-open request. A failed save never breaks interaction,
    /// so errors are dropped.
    ///
    /// The active space and the per-space open requests come straight from
    /// this context — it owns them from `open()` onward.
    pub fn persist_local_state(&self, open_request: Option<&str>) {
        if !self.can_persist() {
            return;
        }
        let state = postui_core::project::LocalState {
            environment: self.active_env.clone(),
            open_request: open_request.map(|s| s.to_string()),
            main_split: self.main_split.clone(),
            expanded: self.expanded.iter().cloned().collect(),
            selections: self.selections.clone(),
            shared_selections: self.shared_selections.clone(),
            space: Some(self.active_space.clone()),
            space_open: self.space_open.clone(),
        };
        let _ = postui_core::project::save_local_state(&self.root, &state);
    }

    /// `persist_local_state`, but for callers (`set_selection`) that don't
    /// track the currently-open request themselves: reads just that one
    /// field back from disk first so it isn't clobbered, then writes
    /// everything else (environment/expanded/selections) straight from
    /// `self` — the authoritative in-memory copy, no need to round-trip it
    /// through disk first.
    pub(crate) fn persist_local_state_keep_open_request(&self) {
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

    /// Every on-disk file the variable manager can rewrite: `variables.toml`,
    /// every environment override, and the (git-ignored) secrets file.
    /// Environments are re-listed from disk on every call — not read from
    /// `self.environments` — so an op that creates or deletes an
    /// environment file is picked up the moment it's on disk, including
    /// when called on the "after" side of an undo capture, before
    /// `self.environments` itself has been refreshed.
    pub fn var_file_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.root.join("variables.toml")];
        for env in postui_core::project::list_environments(&self.root) {
            paths.push(env_path(&self.root, &env));
        }
        paths.push(self.root.join(".local").join("secrets.toml"));
        paths
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

    /// One edit that has to change `variables.toml` *and* every environment
    /// file together, because neither half validates without the other:
    /// renaming a selector (an environment's `[options.<old>]` names a selector
    /// the new model no longer declares — and the new name's options don't
    /// exist yet) and reshaping a selector's field list (every option must
    /// supply exactly the declared fields, so a rename/add/remove is
    /// invalid the moment one side lands alone). Doing it as two
    /// [`Self::edit_variables`]/[`Self::edit_env`] calls fails whichever
    /// goes first, in either order.
    ///
    /// So both halves are built in memory, and the *new* model is validated
    /// against every *new* environment before anything is written. A
    /// refusal leaves every file untouched, exactly like the single-file
    /// verbs. (The writes themselves are per-file atomic and sequential —
    /// an I/O failure part-way can still leave the project half-written,
    /// the same exposure the existing per-env cascades in
    /// `App::apply_var_struct` already carry.)
    pub fn edit_variables_and_envs(
        &mut self,
        vf: impl FnOnce(&str) -> Result<String, EditError>,
        ef: impl Fn(&str) -> Result<String, EditError>,
    ) -> Result<(), String> {
        let vars_path = self.root.join("variables.toml");
        let new_vars_text = vf(&read_or_empty(&vars_path)?).map_err(|e| e.to_string())?;
        let new_model = varmodel::parse_variables(&new_vars_text).map_err(|e| e.to_string())?;

        let mut envs: Vec<(String, PathBuf, String, varmodel::EnvData)> = Vec::new();
        for env in &self.environments {
            let path = self.root.join("environments").join(format!("{env}.toml"));
            let new_text = ef(&read_or_empty(&path)?).map_err(|e| e.to_string())?;
            let new_data = varmodel::parse_environment(&new_text).map_err(|e| e.to_string())?;
            varmodel::validate_env(&new_model, &new_data).map_err(|e| e.to_string())?;
            envs.push((env.clone(), path, new_text, new_data));
        }

        atomic_write(&vars_path, &new_vars_text).map_err(|e| e.to_string())?;
        for (env, path, text, data) in envs {
            atomic_write(&path, &text).map_err(|e| e.to_string())?;
            if self.active_env.as_deref() == Some(env.as_str()) {
                self.env_data = data;
            }
        }

        self.model = new_model;
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
        postui_core::storage::ensure_project(dir.path()).unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"t\"\n").unwrap();
        postui_core::project::save_local_state(
            dir.path(),
            &postui_core::project::LocalState {
                environment: Some("qa".into()),
                open_request: Some("main/ping".into()),
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
        assert_eq!(ctx.local_open_request().as_deref(), Some("main/ping"));
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
        // A directory can't be opened for append; read-only is enough for
        // `set_modified` (futimens) on the owner's own directory.
        let f = if p.is_dir() {
            std::fs::File::open(p).unwrap()
        } else {
            std::fs::File::options().append(true).open(p).unwrap()
        };
        f.set_modified(t).unwrap();
    }

    #[test]
    fn reload_picks_up_changed_variables_and_keeps_active_env() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        postui_core::storage::ensure_project(dir.path()).unwrap();
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
            "[selectors.user]\nfields = [\"user\"]\n\n[api_key]\nsecret = true\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.alice]\nuser = \"1001\"\n[options.user.bob]\nuser = \"2002\"\n",
        )
        .unwrap();

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

    /// MINOR 4 (spec §1.3): a selection naming a missing option key must
    /// warn once on load and be cleared from local state, not linger
    /// silently forever.
    #[test]
    fn open_warns_and_clears_a_stale_selection() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.user]\nfields = [\"user\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.alice]\nuser = \"1001\"\n",
        )
        .unwrap();
        let mut selections = IndexMap::new();
        let mut qa_sel = IndexMap::new();
        qa_sel.insert("user".to_string(), "ghost".to_string());
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

        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(
            warns
                .iter()
                .any(|w| w.contains("user") && w.contains("qa") && w.contains("no longer exists")),
            "{warns:?}"
        );
        assert!(
            !ctx.selections_for("qa").contains_key("user"),
            "the stale selection must be cleared in memory"
        );
        assert!(!ctx.resolved.values.contains_key("user"));

        // ...and persisted, so the warning doesn't repeat forever.
        let state = postui_core::project::load_local_state(dir.path()).unwrap();
        assert!(
            !state
                .selections
                .get("qa")
                .is_some_and(|s| s.contains_key("user"))
        );
    }

    #[test]
    fn reload_warns_and_clears_a_stale_selection_when_the_option_disappears() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.user]\nfields = [\"user\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.alice]\nuser = \"1001\"\n",
        )
        .unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        ctx.set_selection("user", "alice");
        assert_eq!(ctx.resolved.values["user"], "1001");

        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.bob]\nuser = \"2002\"\n",
        )
        .unwrap();
        bump_mtime(&dir.path().join("environments/qa.toml"));

        let (changed, warns) = ctx.reload_if_changed();
        assert!(changed);
        assert!(
            warns
                .iter()
                .any(|w| w.contains("user") && w.contains("no longer exists")),
            "{warns:?}"
        );
        assert!(!ctx.selections_for("qa").contains_key("user"));
    }

    #[test]
    fn clear_selection_for_removes_the_entry_and_re_resolves_the_active_env() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.user]\nfields = [\"user\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.alice]\nuser = \"1001\"\n",
        )
        .unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        ctx.set_selection("user", "alice");
        assert_eq!(ctx.resolved.values["user"], "1001");

        ctx.clear_selection_for("qa", "user");

        assert!(!ctx.selections_for("qa").contains_key("user"));
        assert!(!ctx.resolved.values.contains_key("user"));
        let state = postui_core::project::load_local_state(dir.path()).unwrap();
        assert!(!state.selections["qa"].contains_key("user"));
    }

    #[test]
    fn clear_selection_for_a_non_active_env_does_not_touch_resolved() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.user]\nfields = [\"user\"]\n",
        )
        .unwrap();
        let options = "[options.user.alice]\nuser = \"1001\"\n";
        std::fs::write(dir.path().join("environments/qa.toml"), options).unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), options).unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        ctx.set_selection("user", "alice");
        ctx.set_selection_for("dev", "user", "alice");
        assert_eq!(ctx.resolved.values["user"], "1001");

        ctx.clear_selection_for("dev", "user");

        assert!(!ctx.selections_for("dev").contains_key("user"));
        assert_eq!(
            ctx.resolved.values["user"], "1001",
            "clearing a non-active env's selection must not disturb the active env's resolution"
        );
    }

    #[test]
    fn set_selection_persists_and_re_resolves() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.user]\nfields = [\"user\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.alice]\nuser = \"1001\"\n[options.user.bob]\nuser = \"2002\"\n",
        )
        .unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        assert!(!ctx.resolved.values.contains_key("user"));

        ctx.set_selection("user", "alice");
        assert_eq!(ctx.resolved.values["user"], "1001");

        let state = postui_core::project::load_local_state(dir.path()).unwrap();
        assert_eq!(state.selections["qa"]["user"], "alice");
    }

    fn write_shared_locale_project(dir: &std::path::Path) {
        postui_core::project::init_project(dir, None).unwrap();
        std::fs::write(
            dir.join("variables.toml"),
            "[selectors.locale]\nshared = true\nfields = [\"lang\"]\n\n[options.locale.en]\nlang = \"en\"\n\n[options.locale.fr]\nlang = \"fr\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("environments/qa.toml"), "").unwrap();
        std::fs::write(dir.join("environments/dev.toml"), "").unwrap();
    }

    #[test]
    fn shared_selection_persists_globally_and_survives_env_switch() {
        let dir = tempfile::tempdir().unwrap();
        write_shared_locale_project(dir.path());
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));

        ctx.set_selection("locale", "fr");
        assert_eq!(ctx.resolved.values["lang"], "fr");

        // The pick is global: switching environments keeps it.
        ctx.set_env(Some("dev".into()));
        assert_eq!(ctx.resolved.values["lang"], "fr");

        // ...and it lands in state.toml's global table, not under an env.
        let state = postui_core::project::load_local_state(dir.path()).unwrap();
        assert_eq!(state.shared_selections["locale"], "fr");
        assert!(
            !state
                .selections
                .get("qa")
                .is_some_and(|s| s.contains_key("locale"))
        );
    }

    #[test]
    fn shared_selection_restores_on_open() {
        let dir = tempfile::tempdir().unwrap();
        write_shared_locale_project(dir.path());
        let mut shared_selections = IndexMap::new();
        shared_selections.insert("locale".to_string(), "en".to_string());
        postui_core::project::save_local_state(
            dir.path(),
            &postui_core::project::LocalState {
                environment: Some("qa".into()),
                shared_selections,
                ..Default::default()
            },
        )
        .unwrap();

        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(ctx.resolved.values["lang"], "en");
    }

    #[test]
    fn stale_shared_selection_warns_and_clears_on_open() {
        let dir = tempfile::tempdir().unwrap();
        write_shared_locale_project(dir.path());
        let mut shared_selections = IndexMap::new();
        shared_selections.insert("locale".to_string(), "ghost".to_string());
        postui_core::project::save_local_state(
            dir.path(),
            &postui_core::project::LocalState {
                environment: Some("qa".into()),
                shared_selections,
                ..Default::default()
            },
        )
        .unwrap();

        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(
            warns
                .iter()
                .any(|w| w.contains("locale") && w.contains("no longer exists")),
            "{warns:?}"
        );
        assert!(!ctx.shared_selections().contains_key("locale"));
        let state = postui_core::project::load_local_state(dir.path()).unwrap();
        assert!(!state.shared_selections.contains_key("locale"));
    }

    #[test]
    fn clear_selection_for_a_shared_selector_clears_the_global_pick() {
        let dir = tempfile::tempdir().unwrap();
        write_shared_locale_project(dir.path());
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        ctx.set_selection("locale", "fr");
        assert_eq!(ctx.resolved.values["lang"], "fr");

        ctx.clear_selection_for("qa", "locale");
        assert!(!ctx.shared_selections().contains_key("locale"));
        assert!(!ctx.resolved.values.contains_key("lang"));
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

    /// Review finding: a retry after a partly-applied migration used to
    /// re-back-up whatever was on disk — by then the already-migrated
    /// text — overwriting the only copy of the original.
    #[test]
    fn retrying_a_partly_applied_migration_keeps_the_original_bak() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        let legacy_vars = "[tier]\n[tier.options.gold]\nvalue = \"g-1\"\n";
        std::fs::write(dir.path().join("variables.toml"), legacy_vars).unwrap();
        // A directory where `environments/qa.toml` should be: the env
        // write fails after `variables.toml` has already been rewritten.
        std::fs::create_dir(dir.path().join("environments/qa.toml")).unwrap();

        let (mut ctx, _warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(ctx.pending_migration().is_some());
        assert!(ctx.apply_migration().is_err(), "the env write must fail");

        let bak = dir.path().join("variables.toml.bak");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            legacy_vars,
            "the first attempt saved the original"
        );
        assert_ne!(
            std::fs::read_to_string(dir.path().join("variables.toml")).unwrap(),
            legacy_vars,
            "...and rewrote the live file before failing"
        );

        // Clear the obstruction and retry.
        std::fs::remove_dir(dir.path().join("environments/qa.toml")).unwrap();
        assert!(ctx.pending_migration().is_some(), "still retryable");
        ctx.apply_migration().unwrap();

        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            legacy_vars,
            "the retry must not overwrite the original with migrated text"
        );
    }

    #[test]
    fn reload_with_broken_env_entries_table_warns_and_keeps_previous() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.user]\nfields = [\"user\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.user.alice]\nuser = \"9001\"\n",
        )
        .unwrap();
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_env(Some("qa".into()));
        assert_eq!(ctx.env_data.options["user"]["alice"].values["user"], "9001");

        // break it: an [options.*] table for an undeclared selector
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.nope.x]\nuser = \"1\"\n",
        )
        .unwrap();
        bump_mtime(&dir.path().join("environments/qa.toml"));
        let (changed, warns) = ctx.reload_if_changed();
        assert!(changed && !warns.is_empty());
        assert_eq!(
            ctx.env_data.options["user"]["alice"].values["user"], "9001",
            "previous good env data kept"
        );
    }

    // -----------------------------------------------------------------
    // spaces
    // -----------------------------------------------------------------

    fn spaced_project(dir: &Path) {
        postui_core::project::init_project(dir, None).unwrap();
        postui_core::storage::ensure_project(dir).unwrap(); // seeds main
        postui_core::project::create_space(dir, "auth").unwrap();
        for slug in ["main/health", "auth/login"] {
            postui_core::storage::save_request(
                dir,
                slug,
                &postui_core::model::HttpRequest {
                    name: None,
                    method: postui_core::model::Method::Get,
                    url: "https://x".into(),
                    substitute_body: false,
                    insecure: false,
                    params: Default::default(),
                    headers: Default::default(),
                    variables: Default::default(),
                    body: None,
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn open_lists_spaces_and_defaults_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        spaced_project(dir.path());
        let (ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        assert_eq!(ctx.spaces, ["main", "auth"]);
        assert_eq!(ctx.active_space, "main");
    }

    #[test]
    fn open_restores_the_stored_space_and_its_open_request() {
        let dir = tempfile::tempdir().unwrap();
        spaced_project(dir.path());
        let mut st = postui_core::project::LocalState {
            space: Some("auth".into()),
            // stale: not in the stored space
            open_request: Some("main/health".into()),
            ..Default::default()
        };
        st.space_open.insert("auth".into(), "auth/login".into());
        postui_core::project::save_local_state(dir.path(), &st).unwrap();
        let (ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        assert_eq!(ctx.active_space, "auth");
        assert_eq!(ctx.local_open_request().as_deref(), Some("auth/login"));
    }

    #[test]
    fn open_falls_back_to_the_open_requests_space_then_the_first() {
        let dir = tempfile::tempdir().unwrap();
        spaced_project(dir.path());
        let st = postui_core::project::LocalState {
            space: Some("gone".into()),
            open_request: Some("auth/login".into()),
            ..Default::default()
        };
        postui_core::project::save_local_state(dir.path(), &st).unwrap();
        let (ctx, warnings) = ProjectContext::open(dir.path().to_path_buf());
        assert_eq!(ctx.active_space, "auth");
        assert_eq!(ctx.local_open_request().as_deref(), Some("auth/login"));
        assert!(warnings.iter().any(|w| w.contains("gone")), "{warnings:?}");

        let st = postui_core::project::LocalState {
            space: Some("gone".into()),
            open_request: None,
            ..Default::default()
        };
        postui_core::project::save_local_state(dir.path(), &st).unwrap();
        let (ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        assert_eq!(ctx.active_space, "main");
    }

    #[test]
    fn open_empties_the_trash() {
        let dir = tempfile::tempdir().unwrap();
        spaced_project(dir.path());
        let t = postui_core::storage::delete_request(dir.path(), "auth/login").unwrap();
        assert!(t.trashed.is_file());
        let _ = ProjectContext::open(dir.path().to_path_buf());
        assert!(!postui_core::trash::trash_dir(dir.path()).exists());
    }

    #[test]
    fn persist_writes_space_and_space_open() {
        let dir = tempfile::tempdir().unwrap();
        spaced_project(dir.path());
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.record_space_open(Some("main/health"));
        assert!(ctx.set_active_space("auth"));
        ctx.record_space_open(Some("auth/login"));
        ctx.persist_local_state(Some("auth/login"));
        let st = postui_core::project::load_local_state(dir.path()).unwrap();
        assert_eq!(st.space.as_deref(), Some("auth"));
        assert_eq!(st.space_open["main"], "main/health");
        assert_eq!(st.space_open["auth"], "auth/login");
        assert!(!ctx.set_active_space("nope"));
        assert_eq!(ctx.active_space, "auth");
    }

    #[test]
    fn reload_picks_up_a_new_space_dir_and_repairs_a_vanished_active_space() {
        let dir = tempfile::tempdir().unwrap();
        spaced_project(dir.path());
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_active_space("auth");
        std::fs::create_dir_all(dir.path().join("requests/billing")).unwrap();
        bump_mtime(&dir.path().join("requests"));
        let (changed, _) = ctx.reload_if_changed();
        assert!(changed);
        assert_eq!(ctx.spaces, ["main", "auth", "billing"]);
        std::fs::remove_dir_all(dir.path().join("requests/auth")).unwrap();
        postui_core::project::write_spaces(dir.path(), &["main".into(), "billing".into()]).unwrap();
        bump_mtime(&dir.path().join("project.toml"));
        let (_, warnings) = ctx.reload_if_changed();
        assert_eq!(ctx.active_space, "main");
        assert!(warnings.iter().any(|w| w.contains("auth")), "{warnings:?}");
    }

    #[test]
    fn rename_and_forget_cascade_local_state() {
        let dir = tempfile::tempdir().unwrap();
        spaced_project(dir.path());
        let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
        ctx.set_active_space("auth");
        ctx.record_space_open(Some("auth/login"));
        ctx.expanded.insert("auth/tokens".into());
        ctx.expanded.insert("main/x".into());
        ctx.spaces = vec!["main".into(), "identity".into()]; // as reload_spaces would after a disk rename
        ctx.rename_space_state("auth", "identity");
        assert_eq!(ctx.active_space, "identity");
        assert_eq!(
            ctx.space_open_for("identity").as_deref(),
            Some("identity/login")
        );
        assert_eq!(ctx.space_open_for("auth"), None);
        assert!(ctx.expanded.contains("identity/tokens"));
        assert!(!ctx.expanded.contains("auth/tokens"));
        assert!(ctx.expanded.contains("main/x"));
        ctx.forget_space("identity");
        assert_eq!(ctx.space_open_for("identity"), None);
        assert!(!ctx.expanded.contains("identity/tokens"));
    }
}
