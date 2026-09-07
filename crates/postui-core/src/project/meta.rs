//! Pure helpers over a project's metadata: the `project.toml` model
//! (`ProjectMeta`, `ItemSettings`, `TlsPolicy`), the `.local/state.toml`
//! model (`LocalState`), slug and display-name arithmetic, and the
//! `toml_edit` table editors the write paths hand their documents to.
//! Nothing here touches a file — `Project` owns every read and write.

use crate::model::Entry;
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
pub(crate) enum Kind {
    Space,
    Environment,
}

impl Kind {
    pub(crate) fn table(self) -> &'static str {
        match self {
            Kind::Space => "space",
            Kind::Environment => "environment",
        }
    }
    pub(crate) fn fallback_slug(self) -> &'static str {
        match self {
            Kind::Space => "space",
            Kind::Environment => "environment",
        }
    }
}

/// `[<kind>.<slug>] name = <name>`, keeping the table's other keys.
pub(crate) fn set_item_name(doc: &mut toml_edit::DocumentMut, kind: Kind, slug: &str, name: &str) {
    set_item_key(doc, kind, slug, "name", name)
}

/// `[<kind>.<slug>] <key> = <value>`, keeping the table's other keys.
pub(crate) fn set_item_key(doc: &mut toml_edit::DocumentMut, kind: Kind, slug: &str, key: &str, value: &str) {
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
pub(crate) fn move_item_table(doc: &mut toml_edit::DocumentMut, kind: Kind, from: &str, to: &str) {
    if from == to {
        return;
    }
    if let Some(t) = doc.get_mut(kind.table()).and_then(|i| i.as_table_mut())
        && let Some(item) = t.remove(from)
    {
        t.insert(to, item);
    }
}

pub(crate) fn remove_item_table(doc: &mut toml_edit::DocumentMut, kind: Kind, slug: &str) {
    if let Some(t) = doc.get_mut(kind.table()).and_then(|i| i.as_table_mut()) {
        t.remove(slug);
        if t.is_empty() {
            doc.remove(kind.table());
        }
    }
}

/// A trimmed, non-empty display name, or `BadName`.
pub(crate) fn display_name_of(input: &str) -> Result<String, ProjectError> {
    let name = input.trim();
    if name.is_empty() {
        return Err(ProjectError::BadName(input.to_string()));
    }
    Ok(name.to_string())
}

/// The slug `display` gets among `taken` (slugs already in use, `exclude`
/// not counting): `slugify(display)`, then `-2`, `-3`, … until free.
pub(crate) fn unique_slug_among(
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
pub(crate) fn display_taken(
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

impl LocalState {
    /// Re-keys every field that names space `from` after it was renamed
    /// `to`: the active space, the open request, the remembered
    /// per-space requests (key and value) and the expanded folders.
    pub fn rename_space(&mut self, from: &str, to: &str) {
        let rekey = |s: &mut String| {
            if s == from {
                *s = to.to_string();
            } else if let Some(rest) = s.strip_prefix(from).and_then(|r| r.strip_prefix('/')) {
                *s = format!("{to}/{rest}");
            }
        };
        if let Some(space) = self.space.as_mut() {
            rekey(space);
        }
        if let Some(open) = self.open_request.as_mut() {
            rekey(open);
        }
        for folder in self.expanded.iter_mut() {
            rekey(folder);
        }
        self.space_open = std::mem::take(&mut self.space_open)
            .into_iter()
            .map(|(mut space, mut slug)| {
                rekey(&mut space);
                rekey(&mut slug);
                (space, slug)
            })
            .collect();
    }
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

pub fn display_name(root: &Path, meta: &ProjectMeta) -> String {
    meta.name.clone().unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string())
    })
}

pub(crate) fn valid_space_name(name: &str) -> bool {
    !name.contains('/') && crate::storage::validate_slug(name).is_ok()
}

/// `root/requests/<name>`.
pub fn space_dir(root: &Path, name: &str) -> PathBuf {
    crate::storage::requests_dir(root).join(name)
}

pub(crate) fn spaces_array(spaces: &[String]) -> toml_edit::Array {
    let mut arr = toml_edit::Array::new();
    for s in spaces {
        arr.push(s.as_str());
    }
    arr
}

/// Moves `name` by `delta` positions (clamped to the ends). Unlisted
/// directories are materialised into the written list so the order on
/// disk is exactly the order displayed.
/// What a reorder did to a list in `project.toml` — the displayed space
/// order, or a space's `[space.<slug>] order` — before and after. Undo
/// writes `before` back, redo `after`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListChange {
    pub before: Vec<String>,
    pub after: Vec<String>,
}

/// The displayed (valid-name) entries of a written list, in order.
pub(crate) fn displayed_spaces(spaces: &[String]) -> Vec<String> {
    spaces
        .iter()
        .filter(|n| valid_space_name(n))
        .cloned()
        .collect()
}

/// `root/environments/<name>.toml`.
pub fn environment_path(root: &Path, name: &str) -> PathBuf {
    root.join("environments").join(format!("{name}.toml"))
}

/// The environment every project starts with. A project always has at
/// least one environment — there is no "no environment" state to fall
/// back to — so this one is written at init and recreated on open when a
/// project has none left.
pub const DEFAULT_ENVIRONMENT: &str = "default";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_state_rename_space_rekeys_every_field_that_names_it() {
        let mut st = LocalState {
            space: Some("auth".into()),
            open_request: Some("auth/login".into()),
            expanded: vec![
                "auth".into(),
                "auth/deep".into(),
                "authx/no".into(),
                "main/a".into(),
            ],
            space_open: [("auth", "auth/login"), ("main", "main/a")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..LocalState::default()
        };
        st.rename_space("auth", "accounts");
        assert_eq!(st.space.as_deref(), Some("accounts"));
        assert_eq!(st.open_request.as_deref(), Some("accounts/login"));
        assert_eq!(
            st.expanded,
            ["accounts", "accounts/deep", "authx/no", "main/a"]
        );
        assert_eq!(
            st.space_open.get("accounts").map(String::as_str),
            Some("accounts/login")
        );
        assert!(!st.space_open.contains_key("auth"));
        assert_eq!(
            st.space_open.get("main").map(String::as_str),
            Some("main/a")
        );
    }
}
