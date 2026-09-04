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
    /// Per-space settings, keyed by slug: `[space.<slug>]`.
    #[serde(default)]
    pub space: IndexMap<String, ItemSettings>,
    /// Per-environment settings, keyed by slug: `[environment.<slug>]`.
    #[serde(default)]
    pub environment: IndexMap<String, ItemSettings>,
}

/// The settings a space or an environment carries in `project.toml`
/// (`[space.<slug>]` / `[environment.<slug>]`). The slug is the directory
/// or file name; the display name is free-form, the way a request's
/// `name` is. Unknown keys are tolerated so a newer file still loads.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ItemSettings {
    #[serde(default)]
    pub name: Option<String>,
    /// Environments only: force certificate verification on or off for
    /// every request sent under this environment, overriding each
    /// request's own `insecure` flag. Absent = per request.
    #[serde(default)]
    pub tls: Option<TlsPolicy>,
    /// Spaces only: request order, slugs relative to the space
    /// (`"login"`, `"auth/refresh"`). Only the relative order among
    /// siblings of one level carries meaning — see `order::order_level`.
    #[serde(default)]
    pub order: Vec<String>,
}

/// An environment's certificate-verification force
/// (`[environment.<slug>] tls = "verify" | "insecure"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsPolicy {
    Verify,
    Insecure,
}

impl TlsPolicy {
    /// The next policy in the cycle per request → verify → insecure →
    /// per request (the Manage screen's `t` key).
    pub fn cycle(current: Option<TlsPolicy>) -> Option<TlsPolicy> {
        match current {
            None => Some(TlsPolicy::Verify),
            Some(TlsPolicy::Verify) => Some(TlsPolicy::Insecure),
            Some(TlsPolicy::Insecure) => None,
        }
    }

    /// The value as written in `project.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            TlsPolicy::Verify => "verify",
            TlsPolicy::Insecure => "insecure",
        }
    }
}

/// The environment's TLS force, if any.
pub fn env_tls(meta: &ProjectMeta, slug: &str) -> Option<TlsPolicy> {
    meta.environment.get(slug).and_then(|s| s.tls)
}

/// Sets or clears `[environment.<slug>] tls`. Clearing removes the key
/// but leaves the table (and its name) in place.
pub fn set_env_tls(root: &Path, slug: &str, policy: Option<TlsPolicy>) -> Result<(), ProjectError> {
    edit_project_toml(root, |doc| match policy {
        Some(p) => set_item_key(doc, Kind::Environment, slug, "tls", p.as_str()),
        None => {
            if let Some(it) = doc
                .get_mut(Kind::Environment.table())
                .and_then(|i| i.as_table_mut())
                .and_then(|t| t.get_mut(slug))
                .and_then(|i| i.as_table_mut())
            {
                it.remove("tls");
            }
        }
    })
}

/// The name a space shows as: its `[space.<slug>] name`, else the slug.
pub fn space_display(meta: &ProjectMeta, slug: &str) -> String {
    meta.space
        .get(slug)
        .and_then(|s| s.name.clone())
        .unwrap_or_else(|| slug.to_string())
}

/// The name an environment shows as: its `[environment.<slug>] name`,
/// else the slug.
pub fn env_display(meta: &ProjectMeta, slug: &str) -> String {
    meta.environment
        .get(slug)
        .and_then(|s| s.name.clone())
        .unwrap_or_else(|| slug.to_string())
}

/// Which of the two settings tables an op edits.
#[derive(Clone, Copy)]
enum Kind {
    Space,
    Environment,
}

impl Kind {
    fn table(self) -> &'static str {
        match self {
            Kind::Space => "space",
            Kind::Environment => "environment",
        }
    }
    fn fallback_slug(self) -> &'static str {
        match self {
            Kind::Space => "space",
            Kind::Environment => "environment",
        }
    }
}

/// Rewrites `project.toml` through `f` (created if missing), preserving
/// everything `f` doesn't touch, comments included.
pub(crate) fn edit_project_toml(
    root: &Path,
    f: impl FnOnce(&mut toml_edit::DocumentMut),
) -> Result<(), ProjectError> {
    let path = root.join("project.toml");
    let text = read_optional(&path)?.unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ProjectError::Parse(e.to_string()))?;
    f(&mut doc);
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// `[<kind>.<slug>] name = <name>`, keeping the table's other keys.
fn set_item_name(doc: &mut toml_edit::DocumentMut, kind: Kind, slug: &str, name: &str) {
    set_item_key(doc, kind, slug, "name", name)
}

/// `[<kind>.<slug>] <key> = <value>`, keeping the table's other keys.
fn set_item_key(doc: &mut toml_edit::DocumentMut, kind: Kind, slug: &str, key: &str, value: &str) {
    let table = doc
        .entry(kind.table())
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(t) = table.as_table_mut() {
        // A bare `[space]` line with nothing but sub-tables under it would
        // be noise, so the parent stays implicit.
        t.set_implicit(true);
        let item = t
            .entry(slug)
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        if let Some(it) = item.as_table_mut() {
            it[key] = toml_edit::value(value);
        }
    }
}

/// Moves `[<kind>.<from>]` to `[<kind>.<to>]` whole, so any setting a
/// future build (or the user's hand) put there survives a rename.
fn move_item_table(doc: &mut toml_edit::DocumentMut, kind: Kind, from: &str, to: &str) {
    if from == to {
        return;
    }
    if let Some(t) = doc.get_mut(kind.table()).and_then(|i| i.as_table_mut())
        && let Some(item) = t.remove(from)
    {
        t.insert(to, item);
    }
}

fn remove_item_table(doc: &mut toml_edit::DocumentMut, kind: Kind, slug: &str) {
    if let Some(t) = doc.get_mut(kind.table()).and_then(|i| i.as_table_mut()) {
        t.remove(slug);
        if t.is_empty() {
            doc.remove(kind.table());
        }
    }
}

/// A trimmed, non-empty display name, or `BadName`.
fn display_name_of(input: &str) -> Result<String, ProjectError> {
    let name = input.trim();
    if name.is_empty() {
        return Err(ProjectError::BadName(input.to_string()));
    }
    Ok(name.to_string())
}

/// The slug `display` gets among `taken` (slugs already in use, `exclude`
/// not counting): `slugify(display)`, then `-2`, `-3`, … until free.
fn unique_slug_among(
    kind: Kind,
    display: &str,
    taken: impl Fn(&str) -> bool,
    exclude: Option<&str>,
) -> String {
    let base = crate::storage::slugify_or(display, kind.fallback_slug());
    let mut candidate = base.clone();
    let mut n = 2;
    while exclude != Some(candidate.as_str()) && taken(&candidate) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

/// Whether `display` (case-insensitively) already names one of `slugs`,
/// other than `exclude`.
fn display_taken(
    display: &str,
    slugs: &[String],
    display_of: impl Fn(&str) -> String,
    exclude: Option<&str>,
) -> bool {
    let wanted = display.to_lowercase();
    slugs
        .iter()
        .filter(|s| exclude != Some(s.as_str()))
        .any(|s| display_of(s).to_lowercase() == wanted)
}

/// The slug [`create_space`] / [`rename_space`] would give `display`
/// (`exclude` = the slug being renamed, which is not a collision with
/// itself).
pub fn space_slug_for(root: &Path, display: &str, exclude: Option<&str>) -> String {
    let listed = write_list(root);
    unique_slug_among(
        Kind::Space,
        display,
        |slug| listed.iter().any(|s| s == slug) || space_dir(root, slug).exists(),
        exclude,
    )
}

/// The slug [`create_environment`] / [`rename_environment`] would give
/// `display`.
pub fn environment_slug_for(root: &Path, display: &str, exclude: Option<&str>) -> String {
    unique_slug_among(
        Kind::Environment,
        display,
        |slug| environment_path(root, slug).exists(),
        exclude,
    )
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
    doc["spaces"] = toml_edit::value(spaces_array(spaces));
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

/// Creates a space from a free-form display name: the directory is the
/// slugified name (`-2`, `-3`, … on a slug collision, request-style), the
/// name itself is recorded under `[space.<slug>]`. Returns the slug. A
/// display name another space already answers to is refused.
pub fn create_space(root: &Path, display: &str) -> Result<String, ProjectError> {
    let display = display_name_of(display)?;
    let meta = load_meta(root).unwrap_or_default();
    let mut spaces = write_list(root);
    if display_taken(&display, &spaces, |s| space_display(&meta, s), None) {
        return Err(ProjectError::AlreadyExists(display));
    }
    let slug = space_slug_for(root, &display, None);
    std::fs::create_dir_all(space_dir(root, &slug))?;
    spaces.push(slug.clone());
    edit_project_toml(root, |doc| {
        doc["spaces"] = toml_edit::value(spaces_array(&spaces));
        set_item_name(doc, Kind::Space, &slug, &display);
    })?;
    Ok(slug)
}

/// Renames space `from` (a slug) to the display name `display`: the
/// directory is re-slugged (created when `from` was list-only), the list
/// entry rewritten in place, and the `[space.<slug>]` table moved whole.
/// Returns the new slug — the same as `from` when only the display name
/// changed. Local-state cascades (open request, expanded folders) are the
/// caller's job.
pub fn rename_space(root: &Path, from: &str, display: &str) -> Result<String, ProjectError> {
    let display = display_name_of(display)?;
    let meta = load_meta(root).unwrap_or_default();
    let mut spaces = write_list(root);
    let Some(idx) = spaces.iter().position(|s| s == from) else {
        return Err(ProjectError::NotFound(from.to_string()));
    };
    if display_taken(&display, &spaces, |s| space_display(&meta, s), Some(from)) {
        return Err(ProjectError::AlreadyExists(display));
    }
    let to = space_slug_for(root, &display, Some(from));
    let from_dir = space_dir(root, from);
    let to_dir = space_dir(root, &to);
    if to != from && from_dir.is_dir() {
        std::fs::rename(&from_dir, &to_dir)?;
    } else if !to_dir.is_dir() {
        std::fs::create_dir_all(&to_dir)?;
    }
    spaces[idx] = to.clone();
    edit_project_toml(root, |doc| {
        doc["spaces"] = toml_edit::value(spaces_array(&spaces));
        move_item_table(doc, Kind::Space, from, &to);
        set_item_name(doc, Kind::Space, &to, &display);
    })?;
    Ok(to)
}

fn spaces_array(spaces: &[String]) -> toml_edit::Array {
    let mut arr = toml_edit::Array::new();
    for s in spaces {
        arr.push(s.as_str());
    }
    arr
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
    edit_project_toml(root, |doc| {
        doc["spaces"] = toml_edit::value(spaces_array(&spaces));
        remove_item_table(doc, Kind::Space, name);
    })?;
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

/// Writes `displayed` — the whole displayed space order, as a drag left
/// it — over the list on disk. Unlike [`move_space`], which swaps two
/// slots, a drop *shifts* every row between source and target by one
/// (remove/insert), so the caller hands the finished order over rather
/// than a delta.
///
/// The valid-name slots of the written list are collected exactly as
/// [`move_space`] does and `displayed` is dealt back out over them in
/// order; an invalid listed entry keeps the slot it was written in. The
/// order must name exactly those slots, once each — anything else would
/// drop or duplicate a space on disk, so it is refused with
/// [`ProjectError::NotFound`] for the offending name. Nothing is written
/// when the result equals the list already on disk.
pub fn set_space_order(root: &Path, displayed: &[String]) -> Result<(), ProjectError> {
    let mut spaces = write_list(root);
    let slots: Vec<usize> = (0..spaces.len())
        .filter(|i| valid_space_name(&spaces[*i]))
        .collect();
    let valid: Vec<String> = slots.iter().map(|i| spaces[*i].clone()).collect();
    if let Some(extra) = displayed.iter().find(|n| !valid.contains(n)) {
        return Err(ProjectError::NotFound(extra.clone()));
    }
    if let Some(missing) = valid.iter().find(|n| !displayed.contains(n)) {
        return Err(ProjectError::NotFound(missing.clone()));
    }
    if displayed.len() != valid.len() {
        // Same names, wrong count: a duplicate. Name the first one.
        return Err(ProjectError::NotFound(displayed[0].clone()));
    }
    for (slot, name) in slots.iter().zip(displayed) {
        spaces[*slot] = name.clone();
    }
    if valid == displayed {
        return Ok(());
    }
    write_spaces(root, &spaces)
}

/// Creates an empty `root/environments/<slug>.toml` for a free-form
/// display name (slug rules as [`create_space`]), making the directory if
/// needed, and records the name under `[environment.<slug>]`. Returns the
/// slug. The file is opened with `create_new` — the check and the create
/// are one atomic step, so a concurrent writer can't be clobbered.
pub fn create_environment(root: &Path, display: &str) -> Result<String, ProjectError> {
    let display = display_name_of(display)?;
    let meta = load_meta(root).unwrap_or_default();
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

/// `root/environments/<name>.toml`.
pub fn environment_path(root: &Path, name: &str) -> PathBuf {
    root.join("environments").join(format!("{name}.toml"))
}

/// Renames environment `from` (a slug) to the display name `display`:
/// the file is re-slugged and the `[environment.<slug>]` table moved
/// whole. Returns the new slug — the same as `from` when only the display
/// name changed. Secrets and local selections keyed by the old slug are
/// the caller's cascade.
pub fn rename_environment(root: &Path, from: &str, display: &str) -> Result<String, ProjectError> {
    let display = display_name_of(display)?;
    let from_path = environment_path(root, from);
    if !from_path.is_file() {
        return Err(ProjectError::NotFound(from.to_string()));
    }
    let meta = load_meta(root).unwrap_or_default();
    let existing = list_environments(root);
    if display_taken(&display, &existing, |s| env_display(&meta, s), Some(from)) {
        return Err(ProjectError::AlreadyExists(display));
    }
    let to = environment_slug_for(root, &display, Some(from));
    if to != from {
        std::fs::rename(&from_path, environment_path(root, &to))?;
    }
    edit_project_toml(root, |doc| {
        move_item_table(doc, Kind::Environment, from, &to);
        set_item_name(doc, Kind::Environment, &to, &display);
    })?;
    Ok(to)
}

/// Moves the environment file into the trash and drops its
/// `[environment.<slug>]` table.
pub fn delete_environment(root: &Path, name: &str) -> Result<Trashed, ProjectError> {
    let path = environment_path(root, name);
    if !path.is_file() {
        return Err(ProjectError::NotFound(name.to_string()));
    }
    let trashed = crate::trash::trash(root, &path)?;
    edit_project_toml(root, |doc| {
        remove_item_table(doc, Kind::Environment, name);
    })?;
    Ok(trashed)
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

/// The environment every project starts with. A project always has at
/// least one environment — there is no "no environment" state to fall
/// back to — so this one is written at init and recreated on open when a
/// project has none left.
pub const DEFAULT_ENVIRONMENT: &str = "default";

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

    // --- display names: `[space.<slug>]` / `[environment.<slug>]` -------

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
    fn set_env_tls_writes_and_clears_the_key_keeping_the_name() {
        let dir = tempdir().unwrap();
        create_environment(dir.path(), "Prod").unwrap();
        set_env_tls(dir.path(), "prod", Some(TlsPolicy::Verify)).unwrap();
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(env_tls(&meta, "prod"), Some(TlsPolicy::Verify));
        assert_eq!(env_display(&meta, "prod"), "Prod", "name kept");

        set_env_tls(dir.path(), "prod", Some(TlsPolicy::Insecure)).unwrap();
        assert_eq!(
            env_tls(&load_meta(dir.path()).unwrap(), "prod"),
            Some(TlsPolicy::Insecure)
        );

        set_env_tls(dir.path(), "prod", None).unwrap();
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(!text.contains("tls"), "key removed:\n{text}");
        assert!(text.contains("name = \"Prod\""), "name kept:\n{text}");
    }

    #[test]
    fn set_env_tls_on_an_env_without_a_table_creates_one() {
        let dir = tempdir().unwrap();
        set_env_tls(dir.path(), "local", Some(TlsPolicy::Insecure)).unwrap();
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(text.contains("[environment.local]"), "{text}");
        assert!(text.contains("tls = \"insecure\""), "{text}");
        assert!(
            !text.contains("[environment]\n"),
            "parent stays implicit:\n{text}"
        );
    }

    #[test]
    fn rename_environment_carries_the_tls_policy_along() {
        let dir = tempdir().unwrap();
        create_environment(dir.path(), "Prod").unwrap();
        set_env_tls(dir.path(), "prod", Some(TlsPolicy::Verify)).unwrap();
        rename_environment(dir.path(), "prod", "Production").unwrap();
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(env_tls(&meta, "production"), Some(TlsPolicy::Verify));
        assert_eq!(env_tls(&meta, "prod"), None);
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
    fn rename_space_reslugs_the_dir_and_moves_its_settings_table() {
        let dir = tempdir().unwrap();
        create_space(dir.path(), "main").unwrap();
        create_space(dir.path(), "auth").unwrap();
        std::fs::write(
            space_dir(dir.path(), "auth").join("login.toml"),
            "url = \"x\"\n",
        )
        .unwrap();
        // A hand-written extra key rides along with the rename.
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(text.contains("[space.auth]\n"), "{text}");
        let text = text.replace("[space.auth]\n", "[space.auth]\ncolor = \"red\"\n");
        std::fs::write(dir.path().join("project.toml"), text).unwrap();

        assert_eq!(
            rename_space(dir.path(), "auth", "Identity & SSO").unwrap(),
            "identity-sso"
        );
        assert!(!space_dir(dir.path(), "auth").exists());
        assert!(
            space_dir(dir.path(), "identity-sso")
                .join("login.toml")
                .is_file()
        );
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(meta.spaces, ["main", "identity-sso"]);
        assert_eq!(space_display(&meta, "identity-sso"), "Identity & SSO");
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(text.contains("[space.identity-sso]"), "{text}");
        assert!(text.contains("color = \"red\""), "{text}");
        assert!(!text.contains("[space.auth]"), "{text}");

        // Same slug, new casing: the dir stays, only the name changes.
        assert_eq!(rename_space(dir.path(), "main", "Main").unwrap(), "main");
        assert_eq!(
            space_display(&load_meta(dir.path()).unwrap(), "main"),
            "Main"
        );
        // Taken display names are refused, whichever slug they'd get.
        assert!(matches!(
            rename_space(dir.path(), "identity-sso", "main"),
            Err(ProjectError::AlreadyExists(_))
        ));
    }

    #[test]
    fn delete_space_drops_its_settings_table() {
        let dir = tempdir().unwrap();
        create_space(dir.path(), "main").unwrap();
        create_space(dir.path(), "Auth v2").unwrap();
        delete_space(dir.path(), "auth-v2").unwrap();
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(!text.contains("auth-v2"), "{text}");
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
    fn rename_environment_reslugs_the_file_and_moves_its_settings_table() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        create_environment(dir.path(), "qa").unwrap();
        create_environment(dir.path(), "prod").unwrap();
        std::fs::write(environment_path(dir.path(), "qa"), "tok = \"q\"\n").unwrap();
        assert_eq!(
            rename_environment(dir.path(), "qa", "QA / Staging").unwrap(),
            "qa-staging"
        );
        assert!(!environment_path(dir.path(), "qa").exists());
        assert_eq!(
            std::fs::read_to_string(environment_path(dir.path(), "qa-staging")).unwrap(),
            "tok = \"q\"\n"
        );
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(env_display(&meta, "qa-staging"), "QA / Staging");
        assert!(meta.environment.get("qa").is_none());
        assert!(matches!(
            rename_environment(dir.path(), "qa-staging", "Prod"),
            Err(ProjectError::AlreadyExists(_))
        ));
        assert!(matches!(
            rename_environment(dir.path(), "nope", "x"),
            Err(ProjectError::NotFound(_))
        ));
    }

    #[test]
    fn delete_environment_drops_its_settings_table() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), None).unwrap();
        create_environment(dir.path(), "Staging (EU)").unwrap();
        delete_environment(dir.path(), "staging-eu").unwrap();
        assert!(
            load_meta(dir.path())
                .unwrap()
                .environment
                .get("staging-eu")
                .is_none()
        );
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
    fn set_space_order_hands_the_dragged_order_out_over_the_valid_slots() {
        let dir = tempdir().unwrap();
        // "Bad Name" is not a valid space name: it keeps its slot while
        // the three real spaces are dealt back out around it.
        write_spaces(
            dir.path(),
            &[
                "main".into(),
                "Bad Name".into(),
                "auth".into(),
                "billing".into(),
            ],
        )
        .unwrap();
        set_space_order(
            dir.path(),
            &["billing".into(), "main".into(), "auth".into()],
        )
        .unwrap();
        assert_eq!(
            load_meta(dir.path()).unwrap().spaces,
            ["billing", "Bad Name", "main", "auth"]
        );
    }

    #[test]
    fn set_space_order_writes_nothing_when_the_order_is_unchanged() {
        let dir = tempdir().unwrap();
        write_spaces(dir.path(), &["main".into(), "auth".into()]).unwrap();
        let path = dir.path().join("project.toml");
        let before = std::fs::read(&path).unwrap();
        set_space_order(dir.path(), &["main".into(), "auth".into()]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before, "byte identical");
    }

    #[test]
    fn set_space_order_refuses_an_order_that_is_not_the_space_list() {
        let dir = tempdir().unwrap();
        write_spaces(dir.path(), &["main".into(), "auth".into()]).unwrap();
        assert!(matches!(
            set_space_order(dir.path(), &["main".into(), "nope".into()]),
            Err(ProjectError::NotFound(n)) if n == "nope"
        ));
        assert!(matches!(
            set_space_order(dir.path(), &["main".into()]),
            Err(ProjectError::NotFound(n)) if n == "auth"
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
            rename_environment(dir.path(), "staging", ""),
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
