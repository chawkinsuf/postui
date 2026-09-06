//! The open project: one object that owns every read and write of the
//! project's files, holds the parsed documents, and records its own undo
//! journal. See docs/superpowers/specs/2026-09-05-project-file-access-design.md.
//!
//! `legacy` holds the stateless free functions the app still calls; they
//! are deleted once the app has migrated (stage 3). New code never calls
//! their disk paths, only their pure text helpers.

mod legacy;
mod local;
pub use legacy::*;

use crate::disk::{Disk, DiskError, RelPath, Ticket};
use crate::journal::{Entry, EntryMeta, Journal, Op};
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
    #[error("no active environment")]
    NoActiveEnvironment,
    #[error("no migration is pending")]
    NothingPending,
}

impl From<legacy::ProjectError> for Error {
    fn from(e: legacy::ProjectError) -> Self {
        match e {
            legacy::ProjectError::BadName(n) => Error::BadName(n),
            legacy::ProjectError::LastSpace => Error::LastSpace,
            legacy::ProjectError::AlreadyExists(n) => Error::AlreadyExists(n),
            legacy::ProjectError::NotFound(n) => Error::NotFound(n),
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
    /// Relative to `root`.
    pub file: String,
    pub error: String,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.error)
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
    listing: Vec<RequestListing>,
    listing_warning: Option<String>,
    /// Requests loaded on demand, held while open (Task 7).
    open_requests: IndexMap<String, crate::model::HttpRequest>,
    local: Local,
    journal: Journal,
    /// The ops of the transaction in progress; `None` outside one.
    recording: Option<Vec<Op>>,
    pending_migration: Option<MigrationOutcome>,
    migration_declined: bool,
}

pub(crate) const PROJECT_TOML: &str = "project.toml";
pub(crate) const VARIABLES_TOML: &str = "variables.toml";
pub(crate) const ENVIRONMENTS_DIR: &str = "environments";
pub(crate) const REQUESTS_DIR: &str = "requests";
pub(crate) const LOCAL_DIR: &str = ".local";
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
    if !legacy::valid_space_name(space) {
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

    pub fn root(&self) -> &Path {
        self.disk.root()
    }

    pub fn display_name(&self) -> String {
        legacy::display_name(self.root(), &self.meta)
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

    pub fn space_name(&self, slug: &str) -> String {
        legacy::space_display(&self.meta, slug)
    }

    pub fn env_name(&self, slug: &str) -> String {
        legacy::env_display(&self.meta, slug)
    }

    pub fn env_tls(&self) -> Option<TlsPolicy> {
        self.active_env
            .as_deref()
            .and_then(|slug| legacy::env_tls(&self.meta, slug))
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

    /// Runs `f` as one undo entry. Every primitive `f` calls records its
    /// inverse; on `Ok` the ops become one journal entry labelled `label`;
    /// on `Err` the ops done so far are reversed (best effort) and nothing
    /// is recorded. Nested calls join the outer transaction.
    pub(crate) fn transaction<T>(
        &mut self,
        label: &str,
        meta: EntryMeta,
        f: impl FnOnce(&mut Project) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.recording.is_some() {
            return f(self);
        }
        self.recording = Some(Vec::new());
        let result = f(self);
        let ops = self.recording.take().unwrap_or_default();
        match result {
            Ok(v) => {
                if !ops.is_empty() {
                    self.journal.push(Entry {
                        label: label.to_string(),
                        ops,
                        meta,
                        merge: None,
                    });
                }
                Ok(v)
            }
            Err(e) => {
                for op in ops.iter().rev() {
                    let _ = self.apply_inverse_unrecorded(op);
                }
                Err(e)
            }
        }
    }

    /// `transaction` for a keyboard reorder: the entry carries a merge key
    /// so a burst folds into one step.
    pub(crate) fn transaction_merging<T>(
        &mut self,
        label: &str,
        key: crate::journal::MergeKey,
        f: impl FnOnce(&mut Project) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.recording.is_some() {
            return f(self);
        }
        self.recording = Some(Vec::new());
        let result = f(self);
        let ops = self.recording.take().unwrap_or_default();
        match result {
            Ok(v) => {
                if !ops.is_empty() {
                    self.journal.push(Entry {
                        label: label.to_string(),
                        ops,
                        meta: EntryMeta::default(),
                        merge: Some((key, std::time::Instant::now())),
                    });
                }
                Ok(v)
            }
            Err(e) => {
                for op in ops.iter().rev() {
                    let _ = self.apply_inverse_unrecorded(op);
                }
                Err(e)
            }
        }
    }

    fn record(&mut self, op: Op) {
        if let Some(r) = &mut self.recording {
            r.push(op);
        }
    }

    /// Reverses one op without recording anything (rollback of a failed
    /// transaction). Task 11's `apply_inverse` is the recording twin.
    fn apply_inverse_unrecorded(&mut self, op: &Op) -> Result<(), Error> {
        match op {
            Op::Renamed { from, to } => self.disk.rename(to, from)?,
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
        self.fs_write_text(&path, Some(&new_text))?;
        self.meta = toml::from_str(&new_text).map_err(|e| parse_err(PROJECT_TOML)(&e))?;
        Ok(())
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

    fn list_environments(disk: &mut Disk) -> Vec<String> {
        let Ok(dir) = RelPath::new(ENVIRONMENTS_DIR) else { return Vec::new() };
        let mut out: Vec<String> = disk
            .list(&dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| e.name.strip_suffix(".toml").map(str::to_string))
            .filter(|stem| !stem.contains('/') && crate::storage::validate_slug(stem).is_ok())
            .collect();
        out.sort();
        out
    }

    /// `meta.spaces` first (invalid names skipped, duplicates dropped),
    /// then unlisted directories under `requests/`, alphabetically.
    fn list_spaces(disk: &mut Disk, meta: &ProjectMeta) -> (Vec<String>, Vec<Warning>) {
        let mut out: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        for name in &meta.spaces {
            if !legacy::valid_space_name(name) {
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
        if let Ok(dir) = RelPath::new(REQUESTS_DIR) {
            for e in disk.list(&dir).unwrap_or_default() {
                if e.is_dir && legacy::valid_space_name(&e.name) && !out.contains(&e.name) {
                    unlisted.push(e.name);
                }
            }
        }
        unlisted.sort();
        out.extend(unlisted);
        let warnings = skipped
            .into_iter()
            .map(|n| format!("project.toml lists {n:?}, which is not a valid space name (space names are a-z 0-9 - _)"))
            .collect();
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
        let mut warnings = Vec::new();

        let meta: ProjectMeta = Self::read_doc(&mut disk, PROJECT_TOML, |t| {
            toml::from_str(t).map_err(|e| e.to_string())
        })?;
        if let Err(e) = disk.empty_trash() {
            warnings.push(format!("could not empty .local/trash: {e}"));
        }
        let (spaces, space_warnings) = Self::list_spaces(&mut disk, &meta);
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

        let mut environments = Self::list_environments(&mut disk);
        if environments.is_empty() && !legacy_vars && Self::is_project(disk.root()) {
            let path = RelPath::new(&format!("{ENVIRONMENTS_DIR}/{DEFAULT_ENVIRONMENT}.toml"))
                .expect("constant path");
            match disk.write_new(
                &path,
                "# environments/default.toml: values for this project's variables\n",
            ) {
                Ok(()) => {
                    warnings.push(format!(
                        "no environments — created environments/{DEFAULT_ENVIRONMENT}.toml"
                    ));
                    environments = Self::list_environments(&mut disk);
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
        };
        warnings.extend(project.prune_stale_selections(legacy_vars));
        project.refresh_resolved();
        Ok((project, warnings))
    }

    /// Makes `root` a project (directories, `default` environment, a
    /// `project.toml` with `name`, empty `variables.toml`, `.gitignore`),
    /// never overwriting anything present, then opens it.
    pub fn init(root: &Path, name: Option<&str>) -> Result<(Project, Vec<Warning>), OpenError> {
        legacy::init_project(root, name).map_err(|e| OpenError {
            root: root.to_path_buf(),
            file: String::new(),
            error: e.to_string(),
        })?;
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
                    doc["spaces"] = toml_edit::value(legacy::spaces_array(&spaces));
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

mod migration {
    use super::*;
    pub(super) fn probe(disk: &mut Disk) -> (bool, Option<MigrationOutcome>, Vec<Warning>) {
        let vars = disk
            .read(&RelPath::new(VARIABLES_TOML).expect("constant"))
            .ok()
            .flatten()
            .unwrap_or_default();
        let envs: Vec<(String, String)> = Project::list_environments(disk)
            .into_iter()
            .map(|env| {
                let text = RelPath::new(&format!("{ENVIRONMENTS_DIR}/{env}.toml"))
                    .ok()
                    .and_then(|p| disk.read(&p).ok().flatten())
                    .unwrap_or_default();
                (env, text)
            })
            .collect();
        if !crate::migrate::needs_migration(&vars, &envs) {
            return (false, None, Vec::new());
        }
        match crate::migrate::migrate(&vars, &envs) {
            Ok(outcome) => (true, Some(outcome), Vec::new()),
            Err(e) => (
                true,
                None,
                vec![format!("variables use the old format and can't be converted automatically: {e}")],
            ),
        }
    }
}

mod requests {
    use super::*;
    pub(super) fn list(disk: &mut Disk) -> (Vec<RequestListing>, Option<String>) {
        let (listing, warning) = crate::storage::list_requests(disk.root());
        (listing, warning)
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
        std::fs::write(dir.path().join("variables.toml"), "[keep]\n").unwrap();
        let (p2, _w) = Project::init(dir.path(), Some("Other")).unwrap();
        assert_eq!(p2.display_name(), "New", "init never overwrites");
        assert!(p2.variables().vars.contains_key("keep"));
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
}
