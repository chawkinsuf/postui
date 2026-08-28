//! Inline `{{variable}}` highlighting and the hover tooltip's data model
//! (spec §7, the user's #5 complaint: "I can't see what the current value of
//! a variable is when I'm in the request").
//!
//! One [`VarView`] snapshot answers both halves: [`paint_var_tokens`] tints
//! every token drawn on a row (and registers a [`Hit::VarToken`] over it so
//! it can be hovered and clicked), and [`VarView::describe`] says what a
//! name resolves to and *where* the value came from, which is what the
//! tooltip renders.
//!
//! Precedence matches `prepare::overlaid_vars` exactly — the open request's
//! `[variables]` overlay wins over everything the environment resolved —
//! so what the tooltip shows is what a send would substitute.

use crate::hit::{Hit, HitMap};
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::Entry;
use postui_core::prepare::SECRET_MASK;
use postui_core::varmodel::{Resolved, VarMeta};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::collections::HashSet;
use unicode_width::UnicodeWidthChar;

/// Where a token's value comes from — the tooltip's second line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// The open request's own `[variables]` overlay (highest precedence).
    Request,
    /// A flat value in the active environment.
    Env(String),
    /// The variable declaration's `default`.
    Default,
    /// A selector field, filled by the selector's selected option.
    Selector { selector: String, selected: String },
    /// A selector field whose selector has no (or a stale) selection.
    NeedsSelection,
    /// A secret with no value recorded for this environment.
    MissingSecret,
    /// Not declared anywhere, and no value to be had.
    Undefined,
}

impl TokenSource {
    pub fn label(&self) -> String {
        match self {
            TokenSource::Request => "request var".to_string(),
            TokenSource::Env(env) => format!("env {env}"),
            TokenSource::Default => "default".to_string(),
            TokenSource::Selector { selector, selected } => {
                format!("selector {selector} \u{2192} \"{selected}\"")
            }
            TokenSource::NeedsSelection => "needs selection".to_string(),
            TokenSource::MissingSecret => "missing secret".to_string(),
            TokenSource::Undefined => "not defined".to_string(),
        }
    }
}

/// What one `{{name}}` resolves to right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInfo {
    /// `None` when nothing would substitute — the token would be sent
    /// verbatim (or refused).
    pub value: Option<String>,
    /// A secret's value is never shown: [`TokenInfo::display_value`] masks
    /// it, and there is no reveal anywhere in the tooltip.
    pub secret: bool,
    pub source: TokenSource,
    /// The declaration's description (a selector field carries its
    /// selector's), when one is written — the tooltip's optional third
    /// line.
    pub description: Option<String>,
}

impl TokenInfo {
    pub fn resolved(&self) -> bool {
        self.value.is_some()
    }

    /// The value as it may appear on screen: always [`SECRET_MASK`] for a
    /// secret, a dash when there is nothing to show.
    pub fn display_value(&self) -> String {
        match (&self.value, self.secret) {
            (Some(_), true) => SECRET_MASK.to_string(),
            (Some(v), false) => v.clone(),
            (None, _) => "\u{2014}".to_string(),
        }
    }
}

/// Everything token painting and the tooltip need, snapshotted from the
/// project context plus the open request's `[variables]`. Rebuilt by
/// `App::update` alongside the other derived editor state, so it is never
/// staler than the frame it paints.
#[derive(Debug, Clone, Default)]
pub struct VarView {
    resolved: Resolved,
    /// Enabled request-level `[variables]`: name → value.
    request: IndexMap<String, String>,
    /// Names with a flat value in the active environment's file.
    env_names: HashSet<String>,
    /// Names whose declaration carries a `default`.
    default_names: HashSet<String>,
    /// The active environment's name, when one is active.
    env_label: Option<String>,
    /// Declared descriptions: a variable's own, or — for a selector field —
    /// the owning selector's.
    descriptions: IndexMap<String, String>,
}

impl VarView {
    /// Snapshots `project` plus the open request's `[variables]` overlay.
    pub fn from_context(
        project: &crate::project_ctx::ProjectContext,
        request: &IndexMap<String, Entry>,
    ) -> Self {
        Self {
            resolved: project.resolved.clone(),
            request: request
                .iter()
                .filter(|(_, e)| e.enabled)
                .map(|(k, e)| (k.clone(), e.value.clone()))
                .collect(),
            env_names: project.env_data.values.keys().cloned().collect(),
            default_names: project
                .model
                .vars
                .iter()
                .filter(|(_, d)| d.default.is_some())
                .map(|(n, _)| n.clone())
                .collect(),
            env_label: project.active_env.clone(),
            descriptions: {
                let mut d: IndexMap<String, String> = project
                    .model
                    .vars
                    .iter()
                    .filter_map(|(n, decl)| decl.description.clone().map(|desc| (n.clone(), desc)))
                    .collect();
                for decl in project.model.selectors.values() {
                    if let Some(desc) = &decl.description {
                        for field in &decl.fields {
                            d.insert(field.clone(), desc.clone());
                        }
                    }
                }
                d
            },
        }
    }

    /// Resolves `name` the way a send would, and says where the value came
    /// from. Never `None`: an undeclared, valueless name is still described
    /// (as [`TokenSource::Undefined`]) so the tooltip can say so.
    pub fn describe(&self, name: &str) -> TokenInfo {
        // The request overlay outranks everything (`prepare::overlaid_vars`).
        if let Some(value) = self.request.get(name) {
            return TokenInfo {
                value: Some(value.clone()),
                secret: false,
                source: TokenSource::Request,
                description: self.descriptions.get(name).cloned(),
            };
        }
        let value = self.resolved.values.get(name).cloned();
        let source = match self.resolved.meta.get(name) {
            Some(VarMeta::MissingSecret) => TokenSource::MissingSecret,
            Some(VarMeta::NeedsSelection) => TokenSource::NeedsSelection,
            Some(VarMeta::SelectorMember { selector, selected }) => TokenSource::Selector {
                selector: selector.clone(),
                selected: selected.clone(),
            },
            // A secret's value only ever comes from `.local/secrets.toml`,
            // which is per-environment.
            Some(VarMeta::Secret) => self.env_source(),
            Some(VarMeta::Simple) | None => {
                if value.is_none() {
                    TokenSource::Undefined
                } else if self.env_names.contains(name) {
                    self.env_source()
                } else if self.default_names.contains(name) {
                    TokenSource::Default
                } else {
                    TokenSource::Undefined
                }
            }
        };
        TokenInfo {
            value,
            secret: matches!(
                self.resolved.meta.get(name),
                Some(VarMeta::Secret) | Some(VarMeta::MissingSecret)
            ),
            source,
            description: self.descriptions.get(name).cloned(),
        }
    }

    /// `env <name>` for the active environment, or [`TokenSource::Undefined`]
    /// when there is none (nothing to name as the source).
    fn env_source(&self) -> TokenSource {
        match &self.env_label {
            Some(env) => TokenSource::Env(env.clone()),
            None => TokenSource::Undefined,
        }
    }
}

/// The color a token's braces and name are tinted: a dim accent when it
/// resolves, `theme.error` when sending it would leave it verbatim.
pub fn token_color(theme: &Theme, info: &TokenInfo) -> Color {
    if info.resolved() {
        theme.accent_edge_dark
    } else {
        theme.error
    }
}

/// Tints every well-formed `{{token}}` in `text` and registers a
/// [`Hit::VarToken`] over each one, on top of whatever hit the control
/// underneath already registered (last-wins, so a click lands on the token).
///
/// `text` must be exactly the string drawn on `row.y` starting at
/// `text_origin_col` — the painter walks it by display width to find each
/// span's columns, and clips anything outside `row`. Only the foreground is
/// touched: the control keeps its own fill, caret and modifiers.
pub fn paint_var_tokens(
    buf: &mut Buffer,
    row: Rect,
    text: &str,
    text_origin_col: u16,
    vars: &VarView,
    theme: &Theme,
    hits: &mut HitMap,
) {
    if row.width == 0 || row.height == 0 {
        return;
    }
    for token in postui_core::vars::find_tokens(text) {
        let before: usize = text[..token.start]
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .sum();
        let width: usize = text[token.start..token.end]
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .sum();
        let start = text_origin_col as usize + before;
        let end = (start + width).min(row.right() as usize);
        let start = start.max(row.x as usize);
        if end <= start {
            continue;
        }
        let info = vars.describe(&token.name);
        let color = token_color(theme, &info);
        for x in start..end {
            if let Some(cell) = buf.cell_mut((x as u16, row.y)) {
                cell.set_fg(color);
            }
        }
        hits.register(
            Rect::new(start as u16, row.y, (end - start) as u16, 1),
            Hit::VarToken(token.name),
        );
    }
}

/// Reconstructs the text drawn on one buffer row, so already-rendered
/// content (the body editor, which paints through edtui) can be scanned for
/// tokens after the fact. Wide characters leave their trailing cells empty,
/// which concatenate to nothing — keeping the string's display widths in
/// step with the columns [`paint_var_tokens`] computes.
pub fn row_text(buf: &Buffer, row: Rect) -> String {
    let mut out = String::new();
    for x in row.x..row.right() {
        if let Some(cell) = buf.cell((x, row.y)) {
            out.push_str(cell.symbol());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::varmodel::VarMeta;

    fn view() -> VarView {
        let mut resolved = Resolved::default();
        resolved
            .values
            .insert("base_url".into(), "http://x".to_string());
        resolved.meta.insert("base_url".into(), VarMeta::Simple);
        resolved.values.insert("api_key".into(), "sk-live".into());
        resolved.meta.insert("api_key".into(), VarMeta::Secret);
        resolved.meta.insert("user".into(), VarMeta::NeedsSelection);
        VarView {
            resolved,
            request: IndexMap::new(),
            env_names: ["base_url".to_string()].into_iter().collect(),
            default_names: HashSet::new(),
            env_label: Some("qa".into()),
            descriptions: IndexMap::new(),
        }
    }

    #[test]
    fn describe_carries_the_declared_description() {
        let mut v = view();
        v.descriptions.insert("base_url".into(), "API root".into());
        assert_eq!(
            v.describe("base_url").description.as_deref(),
            Some("API root")
        );
        assert_eq!(v.describe("user").description, None);
    }

    #[test]
    fn describe_reports_value_and_scope_per_precedence() {
        let mut v = view();
        assert_eq!(
            v.describe("base_url").source,
            TokenSource::Env("qa".to_string())
        );
        assert_eq!(v.describe("base_url").value.as_deref(), Some("http://x"));
        assert_eq!(v.describe("user").source, TokenSource::NeedsSelection);
        assert!(!v.describe("user").resolved());
        assert_eq!(v.describe("nope").source, TokenSource::Undefined);

        // The request overlay outranks the environment.
        v.request.insert("base_url".into(), "http://local".into());
        let info = v.describe("base_url");
        assert_eq!(info.source, TokenSource::Request);
        assert_eq!(info.value.as_deref(), Some("http://local"));
    }

    #[test]
    fn a_secrets_value_is_never_displayed() {
        let v = view();
        let info = v.describe("api_key");
        assert!(info.secret && info.resolved());
        assert_eq!(info.display_value(), SECRET_MASK);
        assert!(!info.display_value().contains("sk-live"));
    }

    #[test]
    fn default_and_selector_scopes_are_labelled() {
        let mut v = view();
        v.resolved.values.insert("page".into(), "1".into());
        v.resolved.meta.insert("page".into(), VarMeta::Simple);
        v.default_names.insert("page".into());
        assert_eq!(v.describe("page").source, TokenSource::Default);

        v.resolved.values.insert("uid".into(), "1001".into());
        v.resolved.meta.insert(
            "uid".into(),
            VarMeta::SelectorMember {
                selector: "user".into(),
                selected: "user 2".into(),
            },
        );
        assert_eq!(
            v.describe("uid").source.label(),
            "selector user \u{2192} \"user 2\""
        );
    }

    #[test]
    fn painting_tints_spans_and_registers_hits_clipped_to_the_row() {
        let theme = Theme::dark();
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let mut hits = HitMap::default();
        let text = "{{base_url}}/{{user}}";
        crate::paint::text(&mut buf, 0, 0, text, theme.text, theme.page, false);
        paint_var_tokens(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            text,
            0,
            &view(),
            &theme,
            &mut hits,
        );

        assert_eq!(buf[(0, 0)].fg, theme.accent_edge_dark, "resolved token");
        assert_eq!(buf[(12, 0)].fg, theme.text, "the literal `/` is untouched");
        assert_eq!(buf[(13, 0)].fg, theme.error, "needs-selection token");

        let base = hits.rect_of(&Hit::VarToken("base_url".into())).unwrap();
        assert_eq!(base, Rect::new(0, 0, 12, 1));
        let user = hits.rect_of(&Hit::VarToken("user".into())).unwrap();
        assert_eq!(user.x, 13);
        assert_eq!(user.right(), 20, "clipped to the row's right edge");
    }

    #[test]
    fn row_text_round_trips_through_the_painter() {
        let theme = Theme::dark();
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 1));
        crate::paint::text(
            &mut buf,
            2,
            0,
            "h\u{e9}llo {{base_url}}",
            theme.text,
            theme.page,
            false,
        );
        let mut hits = HitMap::default();
        let row = Rect::new(2, 0, 22, 1);
        let text = row_text(&buf, row);
        paint_var_tokens(&mut buf, row, &text, 2, &view(), &theme, &mut hits);
        let hit = hits.rect_of(&Hit::VarToken("base_url".into())).unwrap();
        assert_eq!(hit.x, 8, "6 display columns of text after the origin");
        assert_eq!(hit.width, 12);
    }
}
