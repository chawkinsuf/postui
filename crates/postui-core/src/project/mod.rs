//! The open project: one object that owns every read and write of the
//! project's files, holds the parsed documents, and records its own undo
//! journal. See docs/superpowers/specs/2026-09-05-project-file-access-design.md.
//!
//! `meta` holds the pure helpers over `ProjectMeta`, slugs and display
//! names that the rest of the module builds on; it touches no files.

mod environments;
mod local;
mod meta;
mod migration;
mod spaces;
mod undo;
mod varedit_ops;
mod variables;
pub use meta::*;
pub use undo::Undone;
pub use varedit_ops::VarEdit;

use crate::disk::{Disk, DiskError, RelPath, Stamp, Ticket};
use crate::journal::{Entry, EntryId, EntryMeta, Journal, Op};
use crate::migrate::MigrationOutcome;
use crate::storage::RequestListing;
use crate::varmodel::{self, EnvData, Resolved, VarModel};
use indexmap::IndexMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub type Warning = String;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Disk(#[from] DiskError),
    #[error("{file}: {error}")]
    Parse { file: String, error: String },
    #[error("invalid name: {0}")]
    BadName(String),
    #[error("cannot delete the last space")]
    LastSpace,
    #[error("cannot delete the last environment")]
    LastEnvironment,
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// An undo or redo whose target no longer looks as expected.
    #[error("{0}")]
    Conflict(String),
    /// A variable edit refused by `varedit` or validation (never carries
    /// a secret value).
    #[error("{0}")]
    Edit(String),
    #[error("no migration is pending")]
    NothingPending,
}

impl From<meta::ProjectError> for Error {
    fn from(e: meta::ProjectError) -> Self {
        match e {
            meta::ProjectError::BadName(n) => Error::BadName(n),
            meta::ProjectError::LastSpace => Error::LastSpace,
            meta::ProjectError::AlreadyExists(n) => Error::AlreadyExists(n),
            meta::ProjectError::NotFound(n) => Error::NotFound(n),
            other => Error::Edit(other.to_string()),
        }
    }
}

/// Why a project refused to open: a file the app would later write back
/// exists but could not be read or parsed. Opening anyway would mean
/// running on defaults and writing them over the user's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenError {
    pub root: PathBuf,
    /// Relative to `root`. Empty when the failure names no single file
    /// (`Project::init`'s seeding, which creates several) — the `Display`
    /// then omits it rather than emitting a bare `": "`.
    pub file: String,
    pub error: String,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.file.is_empty() {
            write!(f, "{}", self.error)
        } else {
            write!(f, "{}: {}", self.file, self.error)
        }
    }
}

/// The per-project, machine-owned state from `.local/state.toml`.
/// Data only; `Project` persists it (Task 6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Local {
    pub active_space: String,
    pub space_open: IndexMap<String, String>,
    pub expanded: BTreeSet<String>,
    pub selections: IndexMap<String, IndexMap<String, String>>,
    pub shared_selections: IndexMap<String, String>,
    pub main_split: Option<String>,
    /// The request the editor has open, as last told to the project.
    pub open_request: Option<String>,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project").field("root", &self.root()).finish_non_exhaustive()
    }
}

pub struct Project {
    disk: Disk,
    meta: ProjectMeta,
    model: VarModel,
    environments: Vec<String>,
    active_env: Option<String>,
    env_data: EnvData,
    secrets: IndexMap<String, IndexMap<String, String>>,
    resolved: Resolved,
    spaces: Vec<String>,
    /// `project.toml` entries that are not valid space names, joined;
    /// re-derived by every `refresh_spaces`.
    spaces_warning: Option<String>,
    listing: Vec<RequestListing>,
    listing_warning: Option<String>,
    /// Requests loaded on demand, held while open (Task 7).
    open_requests: IndexMap<String, Held>,
    local: Local,
    journal: Journal,
    /// The ops of the transaction in progress; `None` outside one.
    recording: Option<Vec<Op>>,
    pending_migration: Option<MigrationOutcome>,
    migration_declined: bool,
    /// Set by `invalidate_stamps`; forces the next `poll` to reload even
    /// though no watched stamp actually differs.
    force_reload: bool,
}

/// A request held open in the editor, plus the stamp its file had when the
/// editor's buffer was last seeded from disk (by `open_request` or
/// `save_request`). `held_request_drift` compares that seed stamp against a
/// fresh one to tell whether the file moved outside the app since.
pub(crate) struct Held {
    req: crate::model::HttpRequest,
    stamp: Stamp,
}

/// A snapshot of every in-memory document `Project` holds, for restoring
/// memory when a transaction's disk ops must be rolled back. See
/// [`Project::snapshot`] / [`Project::restore`].
struct Memory {
    meta: ProjectMeta,
    model: VarModel,
    environments: Vec<String>,
    active_env: Option<String>,
    env_data: EnvData,
    secrets: IndexMap<String, IndexMap<String, String>>,
    resolved: Resolved,
    spaces: Vec<String>,
    spaces_warning: Option<String>,
    local: Local,
    /// Slugs held in `open_requests` (keys only; bodies are re-read).
    open_request_keys: Vec<String>,
}

pub(crate) const PROJECT_TOML: &str = "project.toml";
pub(crate) const VARIABLES_TOML: &str = "variables.toml";
pub(crate) const ENVIRONMENTS_DIR: &str = "environments";
pub(crate) const REQUESTS_DIR: &str = "requests";
pub(crate) const STATE_TOML: &str = ".local/state.toml";
pub(crate) const SECRETS_TOML: &str = ".local/secrets.toml";

pub(crate) fn rel(s: &str) -> Result<RelPath, Error> {
    Ok(RelPath::new(s)?)
}

pub(crate) fn request_rel(slug: &str) -> Result<RelPath, Error> {
    crate::storage::validate_slug(slug).map_err(|e| Error::BadName(e.to_string()))?;
    rel(&format!("{REQUESTS_DIR}/{slug}.toml"))
}

pub(crate) fn space_rel(space: &str) -> Result<RelPath, Error> {
    if !meta::valid_space_name(space) {
        return Err(Error::BadName(space.to_string()));
    }
    rel(&format!("{REQUESTS_DIR}/{space}"))
}

pub(crate) fn env_rel(env: &str) -> Result<RelPath, Error> {
    if env.contains('/') || crate::storage::validate_slug(env).is_err() {
        return Err(Error::BadName(env.to_string()));
    }
    rel(&format!("{ENVIRONMENTS_DIR}/{env}.toml"))
}

/// The `list_spaces` warnings as one line, or `None` when there are
/// none: what `spaces_warning` reports.
fn join_warnings(warnings: &[Warning]) -> Option<String> {
    (!warnings.is_empty()).then(|| warnings.join("; "))
}

fn parse_err(file: &str) -> impl Fn(&dyn std::fmt::Display) -> Error + '_ {
    move |e| Error::Parse {
        file: file.to_string(),
        error: e.to_string(),
    }
}

impl Project {
    pub fn is_project(root: &Path) -> bool {
        root.join(PROJECT_TOML).is_file()
    }

    /// The name to show for a project that is *not* open — the Projects
    /// chooser's row labels. Reads `<root>/project.toml` through a
    /// throwaway [`Disk`] (`peek`: no stamp recorded, nothing written)
    /// and parses it with the same deserialiser [`Self::open`] uses. A
    /// missing, unreadable or unparsable file falls back to the default
    /// meta, so a broken project still lists under its directory name
    /// rather than dropping out of the chooser — and, unlike `open`, this
    /// never refuses: it only ever labels a row.
    pub fn peek_display_name(root: &Path) -> String {
        let disk = Disk::new(root.to_path_buf());
        let meta = RelPath::new(PROJECT_TOML)
            .ok()
            .and_then(|rel| disk.peek(&rel).ok())
            .flatten()
            .and_then(|text| toml::from_str::<ProjectMeta>(&text).ok())
            .unwrap_or_default();
        meta::display_name(root, &meta)
    }

    pub fn root(&self) -> &Path {
        self.disk.root()
    }

    pub fn display_name(&self) -> String {
        meta::display_name(self.root(), &self.meta)
    }

    pub fn meta(&self) -> &ProjectMeta {
        &self.meta
    }

    pub fn variables(&self) -> &VarModel {
        &self.model
    }

    pub fn environments(&self) -> &[String] {
        &self.environments
    }

    pub fn active_env(&self) -> Option<&str> {
        self.active_env.as_deref()
    }

    pub fn env_data(&self) -> &EnvData {
        &self.env_data
    }

    pub fn secrets(&self) -> &IndexMap<String, IndexMap<String, String>> {
        &self.secrets
    }

    pub fn resolved(&self) -> &Resolved {
        &self.resolved
    }

    pub fn spaces(&self) -> &[String] {
        &self.spaces
    }

    /// `project.toml` entries that are not valid space names, joined; the
    /// app toasts it once per change.
    pub fn spaces_warning(&self) -> Option<&str> {
        self.spaces_warning.as_deref()
    }

    pub fn space_name(&self, slug: &str) -> String {
        meta::space_display(&self.meta, slug)
    }

    pub fn env_name(&self, slug: &str) -> String {
        meta::env_display(&self.meta, slug)
    }

    pub fn env_tls(&self) -> Option<TlsPolicy> {
        self.active_env
            .as_deref()
            .and_then(|slug| meta::env_tls(&self.meta, slug))
    }

    pub fn prepare_context(&self) -> crate::prepare::PrepareContext {
        crate::prepare::PrepareContext {
            vars: self.resolved.values.clone(),
            default_headers: self.meta.default_headers.clone(),
            meta: self.resolved.meta.clone(),
            tls_override: self.env_tls(),
        }
    }

    pub fn local(&self) -> &Local {
        &self.local
    }

    pub fn journal_len(&self) -> usize {
        self.journal.len()
    }

    pub fn can_undo(&self) -> bool {
        self.journal.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.journal.can_redo()
    }

    /// The entry `undo` would replay next: its id and label.
    pub fn last_entry(&self) -> Option<(EntryId, &str)> {
        self.journal.peek_undo().map(|e| (e.id, e.label.as_str()))
    }

    /// The entry `redo` would replay next.
    pub fn next_redo(&self) -> Option<EntryId> {
        self.journal.peek_redo().map(|e| e.id)
    }

    /// The key selections and secrets are stored under for the active
    /// environment: the empty string when none is active.
    pub(crate) fn env_key(&self) -> String {
        self.active_env.clone().unwrap_or_default()
    }

    /// Recomputes `resolved` from the model, the active env's data, its
    /// selections (shared picks win) and its secrets.
    pub(crate) fn refresh_resolved(&mut self) {
        let key = self.env_key();
        let empty = IndexMap::new();
        let mut selections = self.local.selections.get(&key).cloned().unwrap_or_default();
        for (name, option) in &self.local.shared_selections {
            selections.insert(name.clone(), option.clone());
        }
        let secrets = self.secrets.get(&key).unwrap_or(&empty);
        self.resolved = varmodel::resolve_env(&self.model, &self.env_data, &selections, secrets);
    }

    // ----- transaction and the journaled primitives -----

    /// A snapshot of every in-memory document, taken before the outermost
    /// transaction runs so a failed transaction can restore memory to
    /// match the disk ops its rollback already reversed.
    fn snapshot(&self) -> Memory {
        Memory {
            meta: self.meta.clone(),
            model: self.model.clone(),
            environments: self.environments.clone(),
            active_env: self.active_env.clone(),
            env_data: self.env_data.clone(),
            secrets: self.secrets.clone(),
            resolved: self.resolved.clone(),
            spaces: self.spaces.clone(),
            spaces_warning: self.spaces_warning.clone(),
            local: self.local.clone(),
            open_request_keys: self.open_requests.keys().cloned().collect(),
        }
    }

    fn restore(&mut self, m: Memory) {
        self.meta = m.meta;
        self.model = m.model;
        self.environments = m.environments;
        self.active_env = m.active_env;
        self.env_data = m.env_data;
        self.secrets = m.secrets;
        self.resolved = m.resolved;
        self.spaces = m.spaces;
        self.spaces_warning = m.spaces_warning;
        self.local = m.local;
        self.open_requests = m
            .open_request_keys
            .into_iter()
            .map(|k| {
                (
                    k,
                    Held {
                        req: crate::model::HttpRequest::default(),
                        stamp: Stamp::Absent,
                    },
                )
            })
            .collect();
    }

    /// Re-reads every key of `open_requests` from disk (re-parse, or drop
    /// when it no longer parses or exists). Extracted so both `reload_all`
    /// and a failed transaction's rollback can use it: the listing and
    /// every held request are not part of [`Memory`] (they can be large),
    /// so both are re-derived from disk instead of snapshotted.
    pub(crate) fn reload_held_requests(&mut self) {
        let held: Vec<String> = self.open_requests.keys().cloned().collect();
        for slug in held {
            // The editor's buffer was not re-seeded by this reload, so the
            // seed stamp taken by `open_request`/`save_request` is kept as
            // is — the file is still "moved since the editor last saw it".
            let stamp = self.open_requests[&slug].stamp;
            match request_rel(&slug).and_then(|p| Ok(self.disk.read(&p)?)) {
                Ok(Some(text)) => match crate::model::HttpRequest::from_toml_str(&text) {
                    Ok(req) => {
                        self.open_requests.insert(slug, Held { req, stamp });
                    }
                    Err(_) => {
                        self.open_requests.shift_remove(&slug);
                    }
                },
                _ => {
                    self.open_requests.shift_remove(&slug);
                }
            }
        }
    }

    /// Runs `f` as one undo entry. Every primitive `f` calls records its
    /// inverse; on `Ok` the ops become one journal entry labelled `label`
    /// (carrying `merge` when given, for a keyboard-reorder burst); on
    /// `Err` the ops done so far are reversed (best effort), the
    /// in-memory documents are restored to their pre-transaction
    /// snapshot, and nothing is recorded. Nested calls join the outer
    /// transaction (no snapshot, no separate entry).
    fn run_transaction<T>(
        &mut self,
        label: &str,
        meta: EntryMeta,
        merge: Option<crate::journal::MergeKey>,
        f: impl FnOnce(&mut Project) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.recording.is_some() {
            return f(self);
        }
        let snapshot = self.snapshot();
        self.recording = Some(Vec::new());
        let result = f(self);
        let ops = self.recording.take().unwrap_or_default();
        match result {
            Ok(v) => {
                if !ops.is_empty() {
                    self.journal.push(Entry {
                        id: EntryId(0),
                        label: label.to_string(),
                        ops,
                        meta,
                        merge: merge.map(|k| (k, std::time::Instant::now())),
                    });
                }
                Ok(v)
            }
            Err(e) => {
                // Best-effort rollback: if any reversal itself fails, disk
                // and the about-to-be-restored memory snapshot may now
                // disagree with each other. Force the next `poll` to
                // re-sync memory from whatever disk actually holds rather
                // than let it silently drift.
                let mut reversal_failed = false;
                for op in ops.iter().rev() {
                    if self.apply_inverse_unrecorded(op).is_err() {
                        reversal_failed = true;
                    }
                }
                self.restore(snapshot);
                // The listing and every held request are not part of the
                // snapshot (they can be large); re-derive them from disk,
                // which the rollback above has already put back to its
                // pre-transaction state.
                self.relist();
                self.reload_held_requests();
                if reversal_failed {
                    self.force_reload = true;
                }
                Err(e)
            }
        }
    }

    /// Runs `f` as one undo entry. See [`Self::run_transaction`].
    pub(crate) fn transaction<T>(
        &mut self,
        label: &str,
        meta: EntryMeta,
        f: impl FnOnce(&mut Project) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.run_transaction(label, meta, None, f)
    }

    /// `transaction` for a keyboard reorder: the entry carries a merge key
    /// so a burst folds into one step.
    pub(crate) fn transaction_merging<T>(
        &mut self,
        label: &str,
        key: crate::journal::MergeKey,
        f: impl FnOnce(&mut Project) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.run_transaction(label, EntryMeta::default(), Some(key), f)
    }

    pub(super) fn record(&mut self, op: Op) {
        if let Some(r) = &mut self.recording {
            r.push(op);
        }
    }

    /// Reverses one op without recording anything (rollback of a failed
    /// transaction). Task 11's `apply_inverse` is the recording twin.
    fn apply_inverse_unrecorded(&mut self, op: &Op) -> Result<(), Error> {
        match op {
            Op::Renamed { from, to } => {
                self.disk.rename(to, from)?;
            }
            Op::Created { path } => {
                if self.disk.is_dir(path) {
                    self.disk.remove_dir_all(path)?
                } else {
                    self.disk.remove(path)?
                }
            }
            Op::Trashed { ticket } => self.disk.restore(ticket)?,
            Op::Restored { ticket } => self.disk.retrash(ticket)?,
            Op::Text { path, before, .. } => match before {
                Some(t) => self.disk.write(path, t)?,
                None => self.disk.remove(path)?,
            },
        }
        Ok(())
    }

    /// Reads the current text (for the op's `before`) and writes `after`
    /// (`None` removes). The only way a document's text changes.
    pub(crate) fn fs_write_text(&mut self, path: &RelPath, after: Option<&str>) -> Result<(), Error> {
        let before = self.disk.read(path)?;
        if before.as_deref() == after {
            return Ok(());
        }
        match after {
            Some(t) => self.disk.write(path, t)?,
            None => self.disk.remove(path)?,
        }
        self.record(Op::Text {
            path: path.clone(),
            before,
            after: after.map(str::to_string),
        });
        Ok(())
    }

    pub(crate) fn fs_rename(&mut self, from: &RelPath, to: &RelPath) -> Result<(), Error> {
        self.disk.rename(from, to)?;
        self.record(Op::Renamed {
            from: from.clone(),
            to: to.clone(),
        });
        Ok(())
    }

    /// Creates a directory that did not exist. Records nothing when it
    /// already did (nothing to undo).
    pub(crate) fn fs_create_dir(&mut self, path: &RelPath) -> Result<(), Error> {
        if self.disk.is_dir(path) {
            return Ok(());
        }
        self.disk.create_dir(path)?;
        self.record(Op::Created { path: path.clone() });
        Ok(())
    }

    /// Creates a file that must not exist (`AlreadyExists` otherwise).
    pub(crate) fn fs_create_file(&mut self, path: &RelPath, text: &str) -> Result<(), Error> {
        self.disk.write_new(path, text)?;
        self.record(Op::Created { path: path.clone() });
        Ok(())
    }

    pub(crate) fn fs_trash(&mut self, path: &RelPath) -> Result<Ticket, Error> {
        let t = self.disk.trash(path)?;
        self.record(Op::Trashed { ticket: t.clone() });
        Ok(t)
    }

    pub(crate) fn fs_restore(&mut self, t: &Ticket) -> Result<(), Error> {
        self.disk.restore(t)?;
        self.record(Op::Restored { ticket: t.clone() });
        Ok(())
    }

    pub(crate) fn fs_retrash(&mut self, t: &Ticket) -> Result<(), Error> {
        self.disk.retrash(t)?;
        self.record(Op::Trashed { ticket: t.clone() });
        Ok(())
    }

    /// Rewrites `project.toml` through `f`, preserving comments and
    /// everything `f` does not touch, journaled as a text op, and reloads
    /// `meta`. A `project.toml` that will not parse is an error, never an
    /// empty document.
    pub(crate) fn edit_project_toml(
        &mut self,
        f: impl FnOnce(&mut toml_edit::DocumentMut),
    ) -> Result<(), Error> {
        let path = rel(PROJECT_TOML)?;
        let text = self.disk.read(&path)?.unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .map_err(|e: toml_edit::TomlError| parse_err(PROJECT_TOML)(&e))?;
        f(&mut doc);
        let new_text = doc.to_string();
        // Validate before writing: a rejected edit must leave the file and
        // `self.meta` untouched, not a document `ProjectMeta` can't parse.
        let parsed: ProjectMeta = toml::from_str(&new_text).map_err(|e| parse_err(PROJECT_TOML)(&e))?;
        self.fs_write_text(&path, Some(&new_text))?;
        self.meta = parsed;
        Ok(())
    }

    /// The files today's `reload_if_changed` stamps: `project.toml`,
    /// `variables.toml`, `environments/`, the active env file, and
    /// `requests/`.
    fn watched(&self) -> Vec<RelPath> {
        let mut out = vec![
            RelPath::new(PROJECT_TOML).expect("constant"),
            RelPath::new(VARIABLES_TOML).expect("constant"),
            RelPath::new(ENVIRONMENTS_DIR).expect("constant"),
            RelPath::new(REQUESTS_DIR).expect("constant"),
        ];
        if let Some(env) = &self.active_env
            && let Ok(p) = env_rel(env)
        {
            out.push(p);
        }
        out
    }

    /// Records the watched files' stamps (what `open`'s reads did for
    /// the ones it read; directories need an explicit list).
    fn stamp_watched(&mut self) {
        for path in self.watched() {
            if self.disk.is_dir(&path) {
                let _ = self.disk.list(&path);
            } else {
                let _ = self.disk.read(&path);
            }
        }
    }

    /// Forces the next `poll` to reload.
    pub fn invalidate_stamps(&mut self) {
        self.disk.forget_stamps();
        // A cleared table compares as "unchanged"; mark one watched path
        // as never seen by recording a stamp that cannot match.
        self.force_reload = true;
    }

    /// Today's timer reload: silent, mtime-gated, keeps what fails to
    /// parse. Returns whether anything was re-read.
    pub fn poll(&mut self) -> (bool, Vec<Warning>) {
        let changed = self.force_reload || self.watched().iter().any(|p| self.disk.changed(p));
        if !changed {
            return (false, Vec::new());
        }
        self.force_reload = false;
        let warnings = self.reload_documents();
        self.relist();
        // An outside edit to a request the editor holds open must reach
        // the editor too, not just the listing.
        self.reload_held_requests();
        (true, warnings)
    }

    /// Re-reads meta, variables, environments, spaces, secrets and the
    /// active env. A file that fails to parse keeps its previous value
    /// with a warning. Selections are pruned; stamps re-recorded.
    fn reload_documents(&mut self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        match self.disk.read(&RelPath::new(PROJECT_TOML).expect("constant")) {
            Ok(text) => match toml::from_str::<ProjectMeta>(&text.unwrap_or_default()) {
                Ok(meta) => self.meta = meta,
                Err(e) => warnings.push(format!("could not read project.toml: {e}")),
            },
            Err(e) => warnings.push(format!("could not read project.toml: {e}")),
        }
        let (legacy_vars, pending, w) = migration::probe(&mut self.disk);
        warnings.extend(w);
        self.pending_migration = if self.migration_declined { None } else { pending };
        if legacy_vars {
            self.model = VarModel::default();
            self.env_data = EnvData::default();
        } else {
            match self.disk.read(&RelPath::new(VARIABLES_TOML).expect("constant")) {
                Ok(text) => match varmodel::parse_variables(&text.unwrap_or_default()) {
                    Ok(model) => self.model = model,
                    Err(e) => warnings.push(format!("could not read variables.toml: {e}")),
                },
                Err(e) => warnings.push(format!("could not read variables.toml: {e}")),
            }
        }
        self.refresh_environments();
        if let Some(w) = self.refresh_spaces() {
            warnings.push(w);
        }
        match self.disk.read(&RelPath::new(SECRETS_TOML).expect("constant")) {
            Ok(text) => match toml::from_str(&text.unwrap_or_default()) {
                Ok(secrets) => self.secrets = secrets,
                Err(e) => warnings.push(format!("could not read secrets: {e}")),
            },
            Err(e) => warnings.push(format!("could not read secrets: {e}")),
        }
        if let Some(env) = self.active_env.clone() {
            if !self.environments.contains(&env) {
                warnings.push(format!("active environment {env:?} no longer exists"));
                self.active_env = None;
                self.env_data = EnvData::default();
            } else if !legacy_vars {
                match self.load_environment(&env) {
                    Ok(data) => self.env_data = data,
                    Err(e) => warnings.push(format!("could not load environment {env:?}: {e}")),
                }
            }
        }
        // `reload_documents` runs inside `replay`'s recording window, so
        // this is the one remaining *unrecorded* `state.toml` write in
        // that window: `prune_stale_selections` persists through
        // `persist_local`, not `persist_local_journaled`. Tolerated
        // because it writes only when a reload actually pruned a stale
        // selection — a selector or option that vanished from the model
        // or the environment — which no undo/redo of a journaled op
        // produces on its own. The redo-preflight defect that made
        // `apply_meta_active_env` journal its write (an unrecorded write
        // leaves `state.toml` present where the opposite entry's
        // preflight expects it absent) needs the file to be *created*
        // here; pruning only ever rewrites a `state.toml` that the
        // selection it is pruning already put on disk. Journal it if that
        // ever stops holding.
        warnings.extend(self.prune_stale_selections(legacy_vars));
        self.stamp_watched();
        self.refresh_resolved();
        warnings
    }

    /// After an undo or redo: everything, including the listing, the
    /// local state and every held request. `.local/state.toml` is read
    /// into `self.local` first — before `reload_documents` — so that its
    /// `refresh_spaces` (which repairs a now-stale `active_space`) and
    /// `prune_stale_selections` (which may persist) run against the
    /// entry's own restored local state rather than overwriting it. The
    /// restored `environment` is applied last, once `reload_documents`
    /// has refreshed the environment list it must be checked against.
    pub(crate) fn reload_all(&mut self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        // `None` = the read/parse failed, leave `active_env` untouched;
        // `Some(env)` = what `.local/state.toml` said it should be.
        let mut restored_environment: Option<Option<String>> = None;
        match self.disk.read(&RelPath::new(STATE_TOML).expect("constant")) {
            Ok(text) => match toml::from_str::<LocalState>(&text.unwrap_or_default()) {
                Ok(state) => {
                    self.local.open_request = state.open_request;
                    self.local.main_split = state.main_split;
                    self.local.expanded = state.expanded.into_iter().collect();
                    self.local.selections = state.selections;
                    self.local.shared_selections = state.shared_selections;
                    self.local.space_open = state.space_open;
                    if let Some(space) = state.space {
                        self.local.active_space = space;
                    }
                    restored_environment = Some(state.environment);
                }
                Err(e) => warnings.push(format!("could not read .local/state.toml: {e}")),
            },
            Err(e) => warnings.push(format!("could not read .local/state.toml: {e}")),
        }
        warnings.extend(self.reload_documents());
        if let Some(env) = restored_environment {
            match env {
                Some(name) if self.environments.contains(&name) => {
                    if self.active_env.as_deref() != Some(name.as_str())
                        && let Err(e) = self.load_active_env(&name)
                    {
                        warnings.push(format!("could not load environment {name:?}: {e}"));
                    }
                }
                Some(name) => warnings.push(format!("restored environment {name:?} no longer exists")),
                // `.local/state.toml` never distinguishes "explicitly no
                // environment" from "this field was never written" (its
                // default): a project that has never persisted local
                // state at all reads back `None` here even though an
                // environment is legitimately active (picked by `open`'s
                // own first-environment fallback). Clearing on `None`
                // would destroy that. Leave `active_env` as whatever
                // `reload_documents` already validated; a transition this
                // entry actually made is instead restored precisely by
                // `apply_meta_active_env`, from the entry's own record.
                None => {}
            }
        }
        self.relist();
        self.reload_held_requests();
        self.refresh_resolved();
        warnings
    }
}

impl Project {
    fn read_doc<T: Default>(
        disk: &mut Disk,
        file: &str,
        parse: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<T, OpenError> {
        let root = disk.root().to_path_buf();
        let fatal = |error: String| OpenError {
            root: root.clone(),
            file: file.to_string(),
            error,
        };
        let path = RelPath::new(file).map_err(|e| fatal(e.to_string()))?;
        match disk.read(&path).map_err(|e| fatal(e.to_string()))? {
            None => Ok(T::default()),
            Some(text) => parse(&text).map_err(fatal),
        }
    }

    /// The environment slugs under `environments/`. A missing directory
    /// is empty; one that fails to list is an error, never an empty list
    /// — `open` would otherwise take it for "no environments" and create
    /// a `default` the user never asked for.
    fn list_environments(disk: &mut Disk) -> Result<Vec<String>, DiskError> {
        let dir = RelPath::new(ENVIRONMENTS_DIR)?;
        let mut out: Vec<String> = disk
            .list(&dir)?
            .into_iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| e.name.strip_suffix(".toml").map(str::to_string))
            .filter(|stem| !stem.contains('/') && crate::storage::validate_slug(stem).is_ok())
            .collect();
        out.sort();
        Ok(out)
    }

    /// `meta.spaces` first (invalid names skipped, duplicates dropped),
    /// then unlisted directories under `requests/`, alphabetically.
    fn list_spaces(disk: &mut Disk, meta: &ProjectMeta) -> (Vec<String>, Vec<Warning>) {
        let mut out: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        for name in &meta.spaces {
            if !meta::valid_space_name(name) {
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
        let mut warnings: Vec<Warning> = skipped
            .into_iter()
            .map(|n| format!("project.toml lists {n:?}, which is not a valid space name (space names are a-z 0-9 - _)"))
            .collect();
        if let Ok(dir) = RelPath::new(REQUESTS_DIR) {
            match disk.list(&dir) {
                Ok(entries) => {
                    for e in entries {
                        if e.is_dir && meta::valid_space_name(&e.name) && !out.contains(&e.name) {
                            unlisted.push(e.name);
                        }
                    }
                }
                Err(e) => warnings.push(format!("could not list {REQUESTS_DIR}/: {e}")),
            }
        }
        unlisted.sort();
        out.extend(unlisted);
        (out, warnings)
    }

    /// Opens `root`. A missing file is its default; a file that exists but
    /// can't be read or parsed refuses the open naming it. Anything else
    /// off (a stale saved space, a selection that no longer resolves)
    /// degrades into a warning.
    pub fn open(root: PathBuf) -> Result<(Project, Vec<Warning>), OpenError> {
        if root.as_os_str().is_empty() {
            // An empty root is "no project", never the process's cwd:
            // `"".join("project.toml")` is `./project.toml`, which the
            // loaders would happily read (and write back to).
            return Err(OpenError {
                root,
                file: PROJECT_TOML.to_string(),
                error: "no project root given (an empty root would read the current directory)"
                    .to_string(),
            });
        }
        let mut disk = Disk::new(root);
        // The one file whose mode is not the umask's business: secrets
        // land owner-only however the project directory is shared.
        disk.mark_private(RelPath::new(SECRETS_TOML).expect("constant path"));
        let mut warnings = Vec::new();

        let meta: ProjectMeta = Self::read_doc(&mut disk, PROJECT_TOML, |t| {
            toml::from_str(t).map_err(|e| e.to_string())
        })?;
        if let Err(e) = disk.empty_trash() {
            warnings.push(format!("could not empty .local/trash: {e}"));
        }
        let (spaces, space_warnings) = Self::list_spaces(&mut disk, &meta);
        let spaces_warning = join_warnings(&space_warnings);
        warnings.extend(space_warnings);

        let (legacy_vars, pending_migration, migration_warnings) =
            migration::probe(&mut disk);
        warnings.extend(migration_warnings);
        let model = if legacy_vars {
            VarModel::default()
        } else {
            Self::read_doc(&mut disk, VARIABLES_TOML, |t| {
                varmodel::parse_variables(t).map_err(|e| e.to_string())
            })?
        };

        let list_failed = {
            let root = disk.root().to_path_buf();
            move |e: DiskError| OpenError {
                root: root.clone(),
                file: ENVIRONMENTS_DIR.to_string(),
                error: e.to_string(),
            }
        };
        let mut environments = Self::list_environments(&mut disk).map_err(&list_failed)?;
        if environments.is_empty() && !legacy_vars && Self::is_project(disk.root()) {
            let path = RelPath::new(format!("{ENVIRONMENTS_DIR}/{DEFAULT_ENVIRONMENT}.toml"))
                .expect("constant path");
            match disk.write_new(
                &path,
                "# environments/default.toml: values for this project's variables\n",
            ) {
                Ok(()) => {
                    warnings.push(format!(
                        "no environments — created environments/{DEFAULT_ENVIRONMENT}.toml"
                    ));
                    environments = Self::list_environments(&mut disk).map_err(&list_failed)?;
                }
                Err(e) => warnings.push(format!("could not create the default environment: {e}")),
            }
        }

        let state: LocalState = Self::read_doc(&mut disk, STATE_TOML, |t| {
            toml::from_str(t).map_err(|e| e.to_string())
        })?;
        let secrets: IndexMap<String, IndexMap<String, String>> =
            Self::read_doc(&mut disk, SECRETS_TOML, |t| {
                toml::from_str(t).map_err(|e| e.to_string())
            })?;

        let first_space = spaces
            .first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_SPACE.to_string());
        let open_request_space = state
            .open_request
            .as_deref()
            .and_then(crate::storage::space_of)
            .map(str::to_string)
            .filter(|s| spaces.contains(s));
        let active_space = match state.space.clone() {
            Some(s) if spaces.contains(&s) => s,
            Some(s) => {
                warnings.push(format!("saved space {s:?} no longer exists"));
                open_request_space.clone().unwrap_or_else(|| first_space.clone())
            }
            None => open_request_space.clone().unwrap_or_else(|| first_space.clone()),
        };
        let open_request = match state.open_request.clone() {
            Some(r) if crate::storage::space_of(&r) == Some(active_space.as_str()) => Some(r),
            _ => state.space_open.get(&active_space).cloned(),
        };

        let mut active_env = None;
        let mut env_data = EnvData::default();
        let wanted = match state.environment.clone() {
            Some(env) if !environments.contains(&env) => {
                warnings.push(format!("saved environment {env:?} no longer exists"));
                environments.first().cloned()
            }
            Some(env) => Some(env),
            None => environments.first().cloned(),
        };
        if let Some(env) = wanted {
            if legacy_vars {
                active_env = Some(env);
            } else {
                let file = format!("{ENVIRONMENTS_DIR}/{env}.toml");
                let data: EnvData = Self::read_doc(&mut disk, &file, |t| {
                    varmodel::parse_environment(t).map_err(|e| e.to_string())
                })?;
                match varmodel::validate_env(&model, &data) {
                    Ok(()) => {
                        env_data = data;
                        active_env = Some(env);
                    }
                    Err(e) => warnings.push(format!("could not load environment {env:?}: {e}")),
                }
            }
        }

        let (listing, listing_warning) = requests::list(&mut disk);

        let mut project = Project {
            disk,
            meta,
            model,
            environments,
            active_env,
            env_data,
            secrets,
            resolved: Resolved::default(),
            spaces,
            spaces_warning,
            listing,
            listing_warning,
            open_requests: IndexMap::new(),
            local: Local {
                active_space,
                space_open: state.space_open,
                expanded: state.expanded.into_iter().collect(),
                selections: state.selections,
                shared_selections: state.shared_selections,
                main_split: state.main_split,
                open_request,
            },
            journal: Journal::new(),
            recording: None,
            pending_migration,
            migration_declined: false,
            force_reload: false,
        };
        warnings.extend(project.prune_stale_selections(legacy_vars));
        project.stamp_watched();
        project.refresh_resolved();
        Ok((project, warnings))
    }

    /// Makes `root` a project (directories, `default` environment, a
    /// `project.toml` with `name`, empty `variables.toml`, `.gitignore`),
    /// never overwriting anything present, then opens it.
    pub fn init(root: &Path, name: Option<&str>) -> Result<(Project, Vec<Warning>), OpenError> {
        let mut disk = Disk::new(root.to_path_buf());
        // What `meta::init_project` wrote, through `Disk`: the two
        // directories, a `default` environment when the project has none
        // that `list_environments` recognises, and the three seed files —
        // each created only if absent, never rewritten.
        let seed = |disk: &mut Disk| -> Result<(), DiskError> {
            disk.create_dir(&RelPath::new(REQUESTS_DIR)?)?;
            disk.create_dir(&RelPath::new(ENVIRONMENTS_DIR)?)?;
            if Self::list_environments(disk)?.is_empty() {
                match disk.write_new(
                    &RelPath::new(format!("{ENVIRONMENTS_DIR}/{DEFAULT_ENVIRONMENT}.toml"))?,
                    "# environments/default.toml: values for this project's variables\n",
                ) {
                    Ok(()) | Err(DiskError::AlreadyExists(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            let project_toml = match name {
                Some(n) => {
                    let mut doc = toml_edit::DocumentMut::new();
                    doc["name"] = toml_edit::value(n);
                    doc.to_string()
                }
                None => "# project.toml: optional `name`, optional [default_headers]\n".to_string(),
            };
            for (file, text) in [
                (PROJECT_TOML, project_toml.as_str()),
                (
                    VARIABLES_TOML,
                    "# Declare variables: [name] with optional description/default\n",
                ),
                (".gitignore", "/.local/\n"),
            ] {
                match disk.write_new(&RelPath::new(file)?, text) {
                    Ok(()) | Err(DiskError::AlreadyExists(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        };
        seed(&mut disk).map_err(|e| OpenError {
            root: root.to_path_buf(),
            file: String::new(),
            error: e.to_string(),
        })?;
        drop(disk);
        // A fresh project has `main` on disk but not in the list yet.
        let (mut project, mut warnings) = Project::open(root.to_path_buf())?;
        if project.meta.spaces.is_empty() {
            let spaces = project.spaces.clone();
            let spaces = if spaces.is_empty() {
                vec![DEFAULT_SPACE.to_string()]
            } else {
                spaces
            };
            let seeded = (|| -> Result<(), Error> {
                project.fs_create_dir(&space_rel(&spaces[0])?)?;
                project.edit_project_toml(|doc| {
                    doc["spaces"] = toml_edit::value(meta::spaces_array(&spaces));
                })
            })();
            match seeded {
                Ok(()) => {
                    let (spaces, w) = Self::list_spaces(&mut project.disk, &project.meta);
                    project.spaces = spaces;
                    warnings.extend(w);
                    project.journal.clear();
                }
                Err(e) => warnings.push(format!("could not record the first space: {e}")),
            }
        }
        Ok((project, warnings))
    }

    /// What `storage::ensure_project` did on every open: `requests/`
    /// exists; a project with no spaces gets `requests/main` and, when it
    /// has a `project.toml`, a `spaces` list. A bare directory never
    /// gains a `project.toml` behind the user's back. Not journaled.
    pub fn ensure_spaces(&mut self) -> Result<(), Error> {
        self.disk.create_dir(&rel(REQUESTS_DIR)?)?;
        if self.spaces.is_empty() {
            self.disk.create_dir(&space_rel(DEFAULT_SPACE)?)?;
        }
        if Self::is_project(self.disk.root()) && self.meta.spaces.is_empty() {
            let spaces = if self.spaces.is_empty() {
                vec![DEFAULT_SPACE.to_string()]
            } else {
                self.spaces.clone()
            };
            let path = rel(PROJECT_TOML)?;
            let text = self.disk.read(&path)?.unwrap_or_default();
            let mut doc: toml_edit::DocumentMut = text
                .parse()
                .map_err(|e: toml_edit::TomlError| parse_err(PROJECT_TOML)(&e))?;
            doc["spaces"] = toml_edit::value(meta::spaces_array(&spaces));
            let new_text = doc.to_string();
            // Validate before writing, as `edit_project_toml` does.
            let parsed: ProjectMeta =
                toml::from_str(&new_text).map_err(|e| parse_err(PROJECT_TOML)(&e))?;
            self.disk.write(&path, &new_text)?;
            self.meta = parsed;
        }
        // A vanished active space is repaired here as it is in `poll`;
        // the warning is `spaces_warning`'s job, not this one's.
        let _ = self.refresh_spaces();
        Ok(())
    }

    /// Drops selections naming options that no longer exist (told once,
    /// as today), persisting the pruned state. Never prunes while the
    /// variables are legacy and unparsed.
    fn prune_stale_selections(&mut self, legacy_vars: bool) -> Vec<Warning> {
        if legacy_vars {
            return Vec::new();
        }
        let mut warnings = Vec::new();
        if let Some(env) = self.active_env.clone() {
            let sel = self.local.selections.entry(env.clone()).or_default();
            let stale: Vec<String> = sel
                .iter()
                .filter(|(selector, option)| {
                    !(self.model.selectors.contains_key(selector.as_str())
                        && varmodel::selector_options(&self.env_data, selector)
                            .is_some_and(|o| o.contains_key(option.as_str())))
                })
                .map(|(n, _)| n.clone())
                .collect();
            for name in stale {
                sel.shift_remove(&name);
                warnings.push(format!(
                    "selection for `{name}` no longer exists in env `{env}` \u{2014} cleared"
                ));
            }
        }
        let stale: Vec<String> = self
            .local
            .shared_selections
            .iter()
            .filter(|(selector, option)| {
                !(self.model.selectors.get(selector.as_str()).is_some_and(|d| d.shared)
                    && self
                        .model
                        .options
                        .get(selector.as_str())
                        .is_some_and(|o| o.contains_key(option.as_str())))
            })
            .map(|(n, _)| n.clone())
            .collect();
        for name in stale {
            self.local.shared_selections.shift_remove(&name);
            warnings.push(format!("selection for `{name}` no longer exists \u{2014} cleared"));
        }
        if !warnings.is_empty() {
            let _ = self.persist_local();
        }
        warnings
    }
}

mod requests;

#[cfg(test)]
mod tests {
    use super::*;

    /// A project with two spaces, one request each, two environments, a
    /// variable, and nothing local. Every later task's tests start here.
    pub(crate) fn fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("requests/main")).unwrap();
        std::fs::create_dir_all(root.join("requests/auth")).unwrap();
        std::fs::create_dir_all(root.join("environments")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "name = \"Demo\"\nspaces = [\"main\", \"auth\"]\n\n[space.auth]\nname = \"Auth\"\n\n[environment.dev]\nname = \"Dev\"\n",
        )
        .unwrap();
        std::fs::write(root.join("variables.toml"), "[host]\ndefault = \"localhost\"\n").unwrap();
        std::fs::write(root.join("environments/dev.toml"), "host = \"dev.local\"\n").unwrap();
        std::fs::write(root.join("environments/qa.toml"), "").unwrap();
        std::fs::write(
            root.join("requests/main/ping.toml"),
            "name = \"Ping\"\nmethod = \"GET\"\nurl = \"https://{{host}}/ping\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("requests/auth/login.toml"),
            "name = \"Login\"\nmethod = \"POST\"\nurl = \"https://{{host}}/login\"\n",
        )
        .unwrap();
        let (project, warnings) = Project::open(root.to_path_buf()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        (dir, project)
    }

    pub(crate) fn read(dir: &tempfile::TempDir, rel: &str) -> Option<String> {
        std::fs::read_to_string(dir.path().join(rel)).ok()
    }

    #[test]
    fn open_loads_every_document_and_lands_in_the_first_environment() {
        let (_d, p) = fixture();
        assert_eq!(p.display_name(), "Demo");
        assert_eq!(p.spaces(), ["main", "auth"]);
        assert_eq!(p.space_name("auth"), "Auth");
        assert_eq!(p.environments(), ["dev", "qa"]);
        assert_eq!(p.active_env(), Some("dev"));
        assert_eq!(p.env_name("dev"), "Dev");
        assert_eq!(p.resolved().values.get("host").map(String::as_str), Some("dev.local"));
        assert_eq!(p.local().active_space, "main");
    }

    #[test]
    fn an_empty_root_is_refused_not_the_cwd() {
        let err = Project::open(PathBuf::new()).unwrap_err();
        assert_eq!(err.file, "project.toml");
    }

    #[test]
    fn a_file_that_will_not_parse_refuses_the_open_naming_it() {
        let (dir, _p) = fixture();
        std::fs::write(dir.path().join("variables.toml"), "[host\n").unwrap();
        let err = Project::open(dir.path().to_path_buf()).unwrap_err();
        assert_eq!(err.file, "variables.toml");
        assert_eq!(err.root, dir.path());
        std::fs::write(dir.path().join("variables.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(dir.path().join(".local/state.toml"), "space = 3\n").unwrap();
        let err = Project::open(dir.path().to_path_buf()).unwrap_err();
        assert_eq!(err.file, ".local/state.toml");
    }

    #[test]
    fn missing_files_are_their_defaults_and_the_trash_is_emptied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".local/trash/1/requests/x")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "").unwrap();
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(p.meta().name.is_none());
        assert!(p.variables().vars.is_empty());
        assert!(!dir.path().join(".local/trash").exists());
    }

    /// An `environments/` that cannot be listed must not read as "no
    /// environments": that path creates `default.toml` and switches the
    /// active environment, which a transient listing failure must never do.
    #[cfg(unix)]
    #[test]
    fn an_unlistable_environments_dir_refuses_the_open_instead_of_recreating_default() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, _p) = fixture();
        let envs = dir.path().join("environments");
        std::fs::set_permissions(&envs, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = Project::open(dir.path().to_path_buf());
        std::fs::set_permissions(&envs, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = result.unwrap_err();
        assert_eq!(err.file, "environments");
        assert!(!envs.join("default.toml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_unlistable_requests_dir_is_a_warning_not_an_empty_space_list() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, _p) = fixture();
        let requests = dir.path().join("requests");
        std::fs::set_permissions(&requests, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = Project::open(dir.path().to_path_buf());
        std::fs::set_permissions(&requests, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (_p, warnings) = result.unwrap();
        assert!(warnings.iter().any(|w| w.contains("could not list requests/")), "{warnings:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_secrets_file_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = fixture();
        p.set_secret_for("dev", "token", "s3cret".to_string()).unwrap();
        let mode = std::fs::metadata(dir.path().join(".local/secrets.toml")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let public = std::fs::metadata(dir.path().join("project.toml")).unwrap().permissions().mode() & 0o777;
        assert_ne!(public, 0o600, "project.toml keeps the umask mode");
    }

    #[test]
    fn open_recreates_the_default_environment_when_none_are_left() {
        let (dir, _p) = fixture();
        std::fs::remove_file(dir.path().join("environments/dev.toml")).unwrap();
        std::fs::remove_file(dir.path().join("environments/qa.toml")).unwrap();
        let (p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(p.environments(), ["default"]);
        assert_eq!(p.active_env(), Some("default"));
        assert!(warnings.iter().any(|w| w.contains("created environments/default.toml")));
    }

    /// Ported from the app's `ProjectContext` tests
    /// (`open_loads_secrets_and_resolved_reflects_a_selection_from_state`,
    /// `shared_selection_restores_on_open`): a selection and a shared pick
    /// recorded in `.local/state.toml` are in `resolved` the moment the
    /// project opens, and so are the secrets.
    #[test]
    fn open_restores_selections_shared_picks_and_secrets_into_resolved() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[token]\nsecret = true\n\n[selectors.region]\nfields = [\"host\"]\n\n[selectors.locale]\nfields = [\"lang\"]\nshared = true\n\n[options.locale.en]\nlang = \"en\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/dev.toml"),
            "[options.region.east]\nhost = \"east.local\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(
            dir.path().join(".local/secrets.toml"),
            "[dev]\ntoken = \"s3cret\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".local/state.toml"),
            "environment = \"dev\"\n\n[selections.dev]\nregion = \"east\"\n\n[shared_selections]\nlocale = \"en\"\n",
        )
        .unwrap();

        let (p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(p.resolved().values.get("host").map(String::as_str), Some("east.local"));
        assert_eq!(p.resolved().values.get("lang").map(String::as_str), Some("en"));
        assert_eq!(p.resolved().values.get("token").map(String::as_str), Some("s3cret"));
    }

    /// Ported from `open_warns_and_clears_a_stale_selection`,
    /// `stale_shared_selection_warns_and_clears_on_open` and
    /// `reload_warns_and_clears_a_stale_selection_when_the_option_disappears`:
    /// a selection (env-scoped or shared) naming an option that is gone is
    /// dropped, once, with a warning — at open and again at poll.
    #[test]
    fn a_stale_selection_is_cleared_with_a_warning_at_open_and_at_poll() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.region]\nfields = [\"host\"]\n\n[selectors.locale]\nfields = [\"lang\"]\nshared = true\n\n[options.locale.en]\nlang = \"en\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/dev.toml"),
            "[options.region.east]\nhost = \"east.local\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(
            dir.path().join(".local/state.toml"),
            "environment = \"dev\"\n\n[selections.dev]\nregion = \"gone\"\n\n[shared_selections]\nlocale = \"nope\"\n",
        )
        .unwrap();

        let (mut p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("`region`") && w.contains("cleared")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("`locale`") && w.contains("cleared")),
            "{warnings:?}"
        );
        assert!(p.selections_for("dev").get("region").is_none());
        assert!(p.local().shared_selections.get("locale").is_none());
        // The pruned table is written back, so the stale pick does not
        // linger in `.local/state.toml`.
        let state = read(&dir, ".local/state.toml").unwrap();
        assert!(!state.contains("gone"), "{state}");

        // The same happens on a reload when the option disappears from a
        // file edited outside the app.
        p.set_selection_for("dev", "region", "east");
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        p.invalidate_stamps();
        let (changed, warnings) = p.poll();
        assert!(changed);
        assert!(
            warnings.iter().any(|w| w.contains("`region`") && w.contains("cleared")),
            "{warnings:?}"
        );
        assert!(p.selections_for("dev").get("region").is_none());
    }

    #[test]
    fn a_stale_saved_space_and_environment_degrade_with_warnings() {
        let (dir, _p) = fixture();
        std::fs::create_dir_all(dir.path().join(".local")).unwrap();
        std::fs::write(
            dir.path().join(".local/state.toml"),
            "environment = \"gone\"\nspace = \"nope\"\nopen_request = \"auth/login\"\n",
        )
        .unwrap();
        let (p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(p.active_env(), Some("dev"));
        assert_eq!(p.local().active_space, "auth", "the open request's space wins over the first");
        assert_eq!(p.local().open_request.as_deref(), Some("auth/login"));
        assert!(warnings.iter().any(|w| w.contains("saved environment")));
        assert!(warnings.iter().any(|w| w.contains("saved space")));
    }

    #[test]
    fn init_makes_a_project_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (p, _w) = Project::init(dir.path(), Some("New")).unwrap();
        assert_eq!(p.display_name(), "New");
        assert_eq!(p.spaces(), ["main"]);
        assert_eq!(p.environments(), ["default"]);
        assert_eq!(read(&dir, ".gitignore").as_deref(), Some("/.local/\n"));
        // `init`'s own seeding appends the `spaces` list to the stub.
        let project_toml = read(&dir, "project.toml").unwrap();
        assert!(project_toml.contains("name = \"New\""), "{project_toml}");
        assert!(project_toml.contains("spaces = [\"main\"]"), "{project_toml}");
        assert_eq!(
            read(&dir, "variables.toml").as_deref(),
            Some("# Declare variables: [name] with optional description/default\n")
        );
        assert_eq!(
            read(&dir, "environments/default.toml").as_deref(),
            Some("# environments/default.toml: values for this project's variables\n")
        );
        std::fs::write(dir.path().join("variables.toml"), "[keep]\n").unwrap();
        let (p2, _w) = Project::init(dir.path(), Some("Other")).unwrap();
        assert_eq!(p2.display_name(), "New", "init never overwrites");
        assert!(p2.variables().vars.contains_key("keep"));
    }

    /// The three branches of the seed that the happy path does not show:
    /// no `name` writes the comment stub, an existing environment stops
    /// `default` being made, and nothing already on disk is rewritten.
    #[test]
    fn init_without_a_name_stubs_project_toml_and_keeps_an_existing_environment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "mine\n").unwrap();
        let (p, _w) = Project::init(dir.path(), None).unwrap();
        assert!(p.meta().name.is_none());
        let project_toml = read(&dir, "project.toml").unwrap();
        assert!(
            project_toml.contains("# project.toml: optional `name`, optional [default_headers]"),
            "{project_toml}"
        );
        assert!(!project_toml.contains("name = "), "no name was given: {project_toml}");
        assert_eq!(p.environments(), ["dev"], "an existing environment is enough");
        assert!(!dir.path().join("environments/default.toml").exists());
        assert_eq!(read(&dir, ".gitignore").as_deref(), Some("mine\n"));
        assert!(dir.path().join("requests").is_dir());
    }

    /// `list_environments` skips files that are not valid slugs, so a
    /// directory holding only those still gets the `default` environment.
    #[test]
    fn init_ignores_environment_files_that_are_not_valid_slugs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("environments/Bad Name.toml"), "").unwrap();
        std::fs::write(dir.path().join("environments/notes.md"), "").unwrap();
        let (p, _w) = Project::init(dir.path(), None).unwrap();
        assert_eq!(p.environments(), ["default"]);
    }

    /// The four branches of what `storage::ensure_project` did on every
    /// open, now on `Disk`.
    #[test]
    fn ensure_spaces_seeds_main_in_a_bare_dir_and_lists_dirs_in_a_project() {
        // 1. A bare directory with no spaces: `main` on disk, no project.toml.
        let dir = tempfile::tempdir().unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.ensure_spaces().unwrap();
        assert!(dir.path().join("requests/main").is_dir());
        assert!(!dir.path().join("project.toml").exists(), "a bare dir stays bare");
        assert_eq!(p.spaces(), ["main"]);

        // 2. A bare directory that already has a space: left alone.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/auth")).unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.ensure_spaces().unwrap();
        assert!(!dir.path().join("requests/main").exists());
        assert!(!dir.path().join("project.toml").exists());
        assert_eq!(p.spaces(), ["auth"]);

        // 3. A project with an empty `spaces` and dirs on disk: listed.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("requests/auth")).unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.ensure_spaces().unwrap();
        assert_eq!(p.meta().spaces, ["auth"]);
        assert!(!dir.path().join("requests/main").exists());
        assert!(read(&dir, "project.toml").unwrap().contains("spaces = [\"auth\"]"));

        // 4. A project with an empty `spaces` and none on disk: `main`,
        // both as a directory and in the list.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.toml"), "# keep me\n").unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.ensure_spaces().unwrap();
        assert!(dir.path().join("requests/main").is_dir());
        assert_eq!(p.meta().spaces, ["main"]);
        assert_eq!(p.spaces(), ["main"]);
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("spaces = [\"main\"]") && text.contains("# keep me"), "{text}");
    }

    #[test]
    fn ensure_spaces_leaves_a_listed_project_untouched_and_is_not_journaled() {
        let (dir, mut p) = fixture();
        let before = read(&dir, "project.toml").unwrap();
        p.ensure_spaces().unwrap();
        assert_eq!(read(&dir, "project.toml").as_deref(), Some(before.as_str()));
        assert_eq!(p.spaces(), ["main", "auth"]);
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn spaces_warning_names_an_invalid_entry_and_clears_when_it_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\", \"Bad Name\"]\n").unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main")).unwrap();
        let (mut p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        let w = p.spaces_warning().expect("an invalid entry warns").to_string();
        assert!(w.contains("Bad Name"), "{w}");
        assert!(warnings.contains(&w), "open reports it too: {warnings:?}");
        assert_eq!(p.spaces(), ["main"]);

        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\"]\n").unwrap();
        p.invalidate_stamps();
        assert!(p.poll().0);
        assert_eq!(p.spaces_warning(), None);
    }

    #[test]
    fn a_transaction_that_fails_rolls_its_ops_back_and_records_nothing() {
        let (dir, mut p) = fixture();
        let path = RelPath::new("variables.toml").unwrap();
        let r: Result<(), Error> = p.transaction("t", EntryMeta::default(), |p| {
            p.fs_write_text(&path, Some("[changed]\n"))?;
            Err(Error::Conflict("boom".into()))
        });
        assert!(r.is_err());
        assert_eq!(read(&dir, "variables.toml").as_deref(), Some("[host]\ndefault = \"localhost\"\n"));
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn a_transaction_that_succeeds_records_one_entry_with_every_op() {
        let (dir, mut p) = fixture();
        p.transaction("t", EntryMeta::default(), |p| {
            p.fs_write_text(&RelPath::new("variables.toml").unwrap(), Some("[a]\n"))?;
            p.fs_rename(
                &RelPath::new("requests/main/ping.toml").unwrap(),
                &RelPath::new("requests/main/pong.toml").unwrap(),
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(p.journal_len(), 1);
        assert!(dir.path().join("requests/main/pong.toml").is_file());
        let entry = p.journal.pop_undo().unwrap();
        assert_eq!(entry.ops.len(), 2);
        assert!(matches!(entry.ops[0], Op::Text { .. }));
        assert!(matches!(entry.ops[1], Op::Renamed { .. }));
    }

    fn bump_mtime(path: &std::path::Path) {
        // Coarse-mtime filesystems need the clock to move.
        let t = std::fs::metadata(path).unwrap().modified().unwrap() + std::time::Duration::from_secs(2);
        std::fs::File::open(path).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn poll_is_quiet_until_a_watched_file_changes_then_re_reads() {
        let (dir, mut p) = fixture();
        assert_eq!(p.poll(), (false, Vec::new()));
        std::fs::write(dir.path().join("variables.toml"), "[host]\ndefault = \"changed\"\n[extra]\n").unwrap();
        bump_mtime(&dir.path().join("variables.toml"));
        let (changed, warnings) = p.poll();
        assert!(changed && warnings.is_empty(), "{warnings:?}");
        assert!(p.variables().vars.contains_key("extra"));
        assert_eq!(p.active_env(), Some("dev"), "the active env is kept");
        assert!(!p.poll().0);
    }

    #[test]
    fn poll_relists_requests_when_a_watched_file_changes() {
        let (dir, mut p) = fixture();
        std::fs::write(
            dir.path().join("requests/main/extra.toml"),
            "name = \"Extra\"\nmethod = \"GET\"\nurl = \"u\"\n",
        )
        .unwrap();
        bump_mtime(&dir.path().join("requests"));
        let (changed, warnings) = p.poll();
        assert!(changed && warnings.is_empty(), "{warnings:?}");
        assert!(p.requests().iter().any(|l| l.slug == "main/extra"));
    }

    #[test]
    fn poll_re_reads_the_requests_the_editor_holds_open() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        p.open_request("auth/login").unwrap();
        std::fs::write(
            dir.path().join("requests/main/ping.toml"),
            "method = \"GET\"\nurl = \"outside\"\n",
        )
        .unwrap();
        std::fs::remove_file(dir.path().join("requests/auth/login.toml")).unwrap();
        p.invalidate_stamps();
        assert!(p.poll().0);
        assert_eq!(p.held_request("main/ping").unwrap().url, "outside");
        assert!(p.held_request("auth/login").is_none(), "a vanished request is dropped");
    }

    #[test]
    fn poll_with_a_broken_file_warns_and_keeps_the_previous_value() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("variables.toml"), "[host\n").unwrap();
        bump_mtime(&dir.path().join("variables.toml"));
        let (changed, warnings) = p.poll();
        assert!(changed);
        assert!(warnings.iter().any(|w| w.contains("variables.toml")));
        assert!(p.variables().vars.contains_key("host"));
    }

    #[test]
    fn poll_notices_a_new_space_dir_and_a_deleted_active_env() {
        let (dir, mut p) = fixture();
        std::fs::create_dir_all(dir.path().join("requests/new")).unwrap();
        bump_mtime(&dir.path().join("requests"));
        assert!(p.poll().0);
        assert_eq!(p.spaces(), ["main", "auth", "new"]);
        std::fs::remove_file(dir.path().join("environments/dev.toml")).unwrap();
        bump_mtime(&dir.path().join("environments"));
        let (_, warnings) = p.poll();
        assert!(warnings.iter().any(|w| w.contains("no longer exists")));
        assert_eq!(p.active_env(), None);
    }

    #[test]
    fn invalidate_stamps_forces_the_next_poll() {
        let (_d, mut p) = fixture();
        assert!(!p.poll().0);
        p.invalidate_stamps();
        assert!(p.poll().0);
    }

    #[test]
    fn reload_all_re_reads_held_requests_and_drops_vanished_ones() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        p.open_request("auth/login").unwrap();
        std::fs::write(dir.path().join("requests/main/ping.toml"), "method = \"GET\"\nurl = \"outside\"\n").unwrap();
        std::fs::remove_file(dir.path().join("requests/auth/login.toml")).unwrap();
        p.reload_all();
        assert_eq!(p.held_request("main/ping").unwrap().url, "outside");
        assert!(p.held_request("auth/login").is_none());
    }

    #[test]
    fn a_migration_refuses_to_run_over_a_backup_the_user_already_has() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "").unwrap();
        let legacy = "[groups.region]\nmembers = [\"host\"]\n";
        std::fs::write(dir.path().join("variables.toml"), legacy).unwrap();
        std::fs::write(dir.path().join("variables.toml.bak"), "theirs").unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let r = p.apply_migration();
        assert!(matches!(r, Err(Error::AlreadyExists(_))), "{r:?}");
        assert_eq!(read(&dir, "variables.toml").unwrap(), legacy, "the original is untouched");
        assert_eq!(read(&dir, "variables.toml.bak").unwrap(), "theirs", "so is their backup");
        assert!(p.pending_migration().is_some(), "still offered once the .bak is moved aside");
    }

    #[test]
    fn a_legacy_project_offers_a_migration_and_applying_it_backs_up_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "").unwrap();
        // Stage-6 shape: a `[groups]` table `migrate::needs_migration` recognises.
        std::fs::write(dir.path().join("variables.toml"), "[groups.region]\nmembers = [\"host\"]\n").unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(p.pending_migration().is_some());
        assert!(p.variables().vars.is_empty(), "legacy files stay inert");
        let notes = p.apply_migration().unwrap();
        assert!(p.pending_migration().is_none());
        assert!(dir.path().join("variables.toml.bak").is_file());
        let bak = read(&dir, "variables.toml.bak").unwrap();
        assert!(bak.contains("[groups.region]"));
        let _ = notes;
        // A second apply is refused; the .bak is untouched.
        assert!(matches!(p.apply_migration(), Err(Error::NothingPending)));
        assert_eq!(read(&dir, "variables.toml.bak").unwrap(), bak);
    }

    /// The migration is one journal entry: undo puts every rewritten file
    /// back byte-for-byte and takes the `.bak` copies it made with it.
    #[test]
    fn undo_of_a_migration_restores_the_originals_and_removes_the_baks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "").unwrap();
        let legacy_vars = "[tier]\n[tier.options.gold]\nvalue = \"g-1\"\n";
        std::fs::write(dir.path().join("variables.toml"), legacy_vars).unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();

        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(p.pending_migration().is_some());
        p.apply_migration().unwrap();
        let migrated_vars = read(&dir, "variables.toml").unwrap();
        assert_ne!(migrated_vars, legacy_vars, "the apply rewrote variables.toml");
        assert!(dir.path().join("variables.toml.bak").is_file());
        assert_eq!(p.journal_len(), 1, "the whole migration is one entry");

        assert!(p.undo().unwrap().is_some());
        assert_eq!(
            read(&dir, "variables.toml").as_deref(),
            Some(legacy_vars),
            "undo restores the original text"
        );
        assert!(
            !dir.path().join("variables.toml.bak").exists(),
            "undo takes the backup it made with it"
        );

        assert!(p.redo().unwrap().is_some());
        assert_eq!(read(&dir, "variables.toml").unwrap(), migrated_vars);
        assert!(dir.path().join("variables.toml.bak").is_file());
    }

    /// Ported from the app's `ProjectContext` test
    /// `retrying_a_partly_applied_migration_keeps_the_original_bak`, and
    /// re-scoped now that the apply is one transaction: a failure part-way
    /// can no longer leave a *partly* applied migration behind — the
    /// rollback puts the original text back and removes the `.bak` it had
    /// already made — so the retry sees a pristine project and saves the
    /// original as its backup.
    #[test]
    fn a_failed_migration_rolls_back_and_the_retry_still_backs_up_the_original() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "").unwrap();
        // A stage-6 enumerated variable: the conversion rewrites
        // `variables.toml` first and then every environment file, so an
        // environment that cannot be written fails the apply part-way.
        let legacy_vars = "[tier]\n[tier.options.gold]\nvalue = \"g-1\"\n";
        std::fs::write(dir.path().join("variables.toml"), legacy_vars).unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        // A directory where the env file's backup must go: the env write
        // fails after `variables.toml` has already been backed up and
        // rewritten.
        std::fs::create_dir(dir.path().join("environments/dev.toml.bak")).unwrap();

        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(p.pending_migration().is_some());
        assert!(p.apply_migration().is_err(), "the env write must fail");

        assert_eq!(
            read(&dir, "variables.toml").as_deref(),
            Some(legacy_vars),
            "the rollback put the live file back"
        );
        assert!(
            !dir.path().join("variables.toml.bak").exists(),
            "...and removed the backup the failed attempt had made"
        );
        assert_eq!(p.journal_len(), 0, "a rolled-back apply journals nothing");

        // Clear the obstruction and retry.
        std::fs::remove_dir(dir.path().join("environments/dev.toml.bak")).unwrap();
        assert!(p.pending_migration().is_some(), "still retryable");
        p.apply_migration().unwrap();

        assert_eq!(
            read(&dir, "variables.toml.bak").as_deref(),
            Some(legacy_vars),
            "the retry must not overwrite the original with migrated text"
        );
    }

    #[test]
    fn declining_stops_the_offer_for_the_session() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "").unwrap();
        std::fs::write(dir.path().join("variables.toml"), "[groups.region]\nmembers = [\"host\"]\n").unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.decline_migration();
        assert!(p.pending_migration().is_none());
        p.invalidate_stamps();
        p.poll();
        assert!(p.pending_migration().is_none());
    }

    #[test]
    fn edit_project_toml_never_writes_a_document_meta_rejects() {
        let (dir, mut p) = fixture();
        let before = read(&dir, "project.toml").unwrap();
        let r = p.edit_project_toml(|doc| doc["bogus_key"] = toml_edit::value(1));
        assert!(matches!(r, Err(Error::Parse { .. })), "{r:?}");
        assert_eq!(read(&dir, "project.toml").as_deref(), Some(before.as_str()));
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn a_failed_transaction_restores_the_in_memory_documents() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        let path = RelPath::new("variables.toml").unwrap();
        let from = RelPath::new("requests/main/ping.toml").unwrap();
        let to = RelPath::new("requests/main/renamed.toml").unwrap();
        let r: Result<(), Error> = p.transaction("t", EntryMeta::default(), |p| {
            p.spaces.push("bogus".to_string());
            p.active_env = None;
            p.secrets.entry("dev".to_string()).or_default().insert("k".to_string(), "v".to_string());
            p.fs_write_text(&path, Some("[changed]\n"))?;
            p.fs_rename(&from, &to)?;
            if let Some(req) = p.open_requests.shift_remove("main/ping") {
                p.open_requests.insert("main/renamed".to_string(), req);
            }
            Err(Error::Conflict("boom".into()))
        });
        assert!(r.is_err());
        assert_eq!(p.spaces(), ["main", "auth"]);
        assert_eq!(p.active_env(), Some("dev"));
        assert!(p.secrets().get("dev").is_none());
        assert_eq!(read(&dir, "variables.toml").as_deref(), Some("[host]\ndefault = \"localhost\"\n"));
        assert_eq!(p.journal_len(), 0);
        assert!(p.held_request("main/ping").is_some(), "the rename was rolled back");
        assert!(p.held_request("main/renamed").is_none());
    }

    #[test]
    fn peek_display_name_reads_an_unopened_project_and_never_writes() {
        // A named project: the declared name wins.
        let named = tempfile::tempdir().unwrap();
        std::fs::write(named.path().join("project.toml"), "name = \"Alpha\"\n").unwrap();
        assert_eq!(Project::peek_display_name(named.path()), "Alpha");

        // A bare directory (no project.toml at all): today's fallback,
        // `display_name` of the default meta — the directory's own name.
        let bare = tempfile::tempdir().unwrap();
        let bare_root = bare.path().join("my-project");
        std::fs::create_dir(&bare_root).unwrap();
        assert_eq!(Project::peek_display_name(&bare_root), "my-project");
        assert!(!bare_root.join("project.toml").exists(), "peek must not write");

        // A project.toml that does not parse: same fallback, and the
        // broken file is left exactly as it was.
        let broken = tempfile::tempdir().unwrap();
        let broken_root = broken.path().join("busted");
        std::fs::create_dir(&broken_root).unwrap();
        let toml = broken_root.join("project.toml");
        std::fs::write(&toml, "name = [unclosed\n").unwrap();
        assert_eq!(Project::peek_display_name(&broken_root), "busted");
        assert_eq!(
            std::fs::read_to_string(&toml).unwrap(),
            "name = [unclosed\n",
            "peek must leave the unparsable file untouched"
        );
    }

    #[test]
    fn open_error_naming_a_file_shows_it_before_the_reason() {
        let e = OpenError {
            root: PathBuf::from("/p"),
            file: "project.toml".into(),
            error: "expected a value".into(),
        };
        assert_eq!(e.to_string(), "project.toml: expected a value");
    }

    #[test]
    fn open_error_with_no_file_shows_the_reason_alone() {
        let e = OpenError {
            root: PathBuf::from("/p"),
            file: String::new(),
            error: "create environments: Permission denied".into(),
        };
        assert_eq!(e.to_string(), "create environments: Permission denied");
    }
}
