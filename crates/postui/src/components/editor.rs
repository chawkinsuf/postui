use super::line_input::LineInput;
use super::table_editor::{Col, TableEditorState, TableOutcome, table_height};
use super::toast::ToastKind;
use super::{Component, DrawCtx, pane_surface};
use crate::action::Action;
use crate::hit::ScrollbarSpec;
use crate::layout::PaneId;
use crate::theme::Theme;
use edtui::{
    EditorEventHandler, EditorMode, EditorState, EditorTheme, EditorView, LineNumbers, Lines,
};
use indexmap::IndexMap;
use postui_core::model::{Body, Entry, HttpRequest, Method};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::time::Instant;

/// Which editor tab is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTab {
    Params,
    Headers,
    Body,
    Vars,
}

/// Left-to-right tab-strip order, and the order `EditorTabCycle` walks:
/// Headers → Params → Vars → Body. The `alt+1/2/3/4` shortcuts
/// ([`EditorTab::index`]) follow this same order, so the number you press
/// always matches the tab's on-screen position.
const DRAW_ORDER: [EditorTab; 4] = [
    EditorTab::Headers,
    EditorTab::Params,
    EditorTab::Vars,
    EditorTab::Body,
];

impl EditorTab {
    /// Slot number for the `alt+1/2/3/4` shortcuts
    /// (`Action::EditorTabSelect`) — identical to [`EditorTab::draw_position`]
    /// so alt-numbers match the tab strip left to right.
    pub fn index(self) -> usize {
        self.draw_position()
    }

    pub fn from_index(i: usize) -> Self {
        Self::from_draw_position(i)
    }

    /// This tab's position in [`DRAW_ORDER`] — what the tab strip's left-to-
    /// right layout, mouse clicks, and `EditorTabCycle` all key off.
    pub fn draw_position(self) -> usize {
        DRAW_ORDER
            .iter()
            .position(|t| *t == self)
            .expect("DRAW_ORDER lists every tab")
    }

    pub fn from_draw_position(i: usize) -> Self {
        DRAW_ORDER[i % DRAW_ORDER.len()]
    }

    fn label(self) -> &'static str {
        match self {
            EditorTab::Params => "Params",
            EditorTab::Headers => "Headers",
            EditorTab::Body => "Body",
            EditorTab::Vars => "Vars",
        }
    }

    /// What the alt+a footer chip says it will add on this tab — `None` on
    /// Body, where `Action::TableAddRow` is inert and the chip is hidden.
    pub fn add_row_label(self) -> Option<&'static str> {
        match self {
            EditorTab::Params => Some("add param"),
            EditorTab::Headers => Some("add header"),
            EditorTab::Vars => Some("add variable"),
            EditorTab::Body => None,
        }
    }
}

/// Which sub-region of the editor pane has keyboard focus: the method
/// badge, the URL line, the Params/Headers/Body tab strip, the active
/// tab's content (params table / headers table / body editor), or nothing
/// — the blurred state Enter/Esc/click-away leave behind, in which no
/// editor input captures keys until one is re-entered. Arrow keys walk
/// the chain the way it sits on screen: Method ↔ URL horizontally,
/// URL/Method → Tabs → Content vertically (and back with Up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubFocus {
    Method,
    Url,
    Tabs,
    Content,
    None,
}

pub struct Editor {
    pub slug: Option<String>,
    /// The open request's display name (free-form; the slug is derived
    /// from it). `None` for scratch requests and legacy files, which
    /// display as the slug leaf.
    pub name: Option<String>,
    pub saved: Option<HttpRequest>,
    pub method: Method,
    pub url: LineInput,
    pub substitute_body: bool,
    pub params: IndexMap<String, Entry>,
    pub headers: IndexMap<String, Entry>,
    /// Request-scoped `[variables]`, edited by the Vars tab (shares the
    /// Params/Headers table editor) and carried through load/save.
    pub variables: IndexMap<String, Entry>,
    /// `name → "overrides <env>: <value>"`, already formatted (masked for
    /// secrets) by `App` out of `ProjectContext::resolved` — one entry per
    /// project variable that a request-scope entry with the same name
    /// shadows. Synced by `App` on every `update()` alongside
    /// `inherited_headers`. Draw-only: consumed by the Vars tab's table draw
    /// to show the dim "overrides" hint under a shadowing row's expanded
    /// form; never itself edited here.
    pub shadowed: IndexMap<String, String>,
    /// Enabled default headers inherited from the project, synced by `App`
    /// on every `update()` alongside `open_slug`. Draw-only: rendered above
    /// the request headers table but never edited directly here.
    pub inherited_headers: IndexMap<String, Entry>,
    /// The request body buffer. edtui owns the text, cursor and undo stack;
    /// it is only ever rewritten wholesale by an explicit user action
    /// (load / format / minify / external editor), so half-typed JSON
    /// survives a save verbatim.
    pub body: EditorState,
    /// Emacs-mode (modeless) key handling for `body`.
    body_handler: EditorEventHandler,
    /// The body text `body.highlights` was last computed from, plus the
    /// palette it was tinted with (`None` = never computed) — the cheap
    /// change check that keeps the whole-buffer JSON re-lex off draws
    /// where nothing changed. Reset by [`Self::set_body_text`], which
    /// rebuilds the edtui state (and with it the highlight list).
    body_hl_text: String,
    body_hl_marker: Option<ratatui::style::Color>,
    /// The fixed end of an in-progress body selection: planted by a left
    /// click (mouse) or the first shifted motion (keyboard), consumed by
    /// `body_drag_to`/shifted motions to rebuild `body.selection` as the
    /// moving end travels. `None` when no selection gesture is live.
    body_sel_anchor: Option<edtui::Index2>,
    pub active_tab: EditorTab,
    /// The last tab the user explicitly chose (click, alt+number, or
    /// arrow-cycling) — where `active_tab` returns whenever it can.
    /// `active_tab` only ever diverges from this through the forced hop
    /// off a disabled Body tab (GET/HEAD); `App::sync_active_tab` restores
    /// the preference as soon as the open request enables it again, so
    /// switching requests never loses the user's place.
    pub preferred_tab: EditorTab,
    pub sub_focus: SubFocus,
    /// Shared cursor/edit state for the key/value table, reused by both the
    /// Params and Headers tabs (never holds the entry data itself).
    pub table: TableEditorState,
    /// The screen area the body editor was rendered into on the last frame,
    /// recorded by `draw_tab_content`'s Body arm; `None` on any other tab
    /// (including the very first frame before anything has drawn). Mouse
    /// events are hit-tested against this before being forwarded to edtui.
    pub last_body_area: Option<Rect>,
    /// Mirrors whether the open request has a send in flight, synced by `App::update` on every
    /// action alongside `open_slug`. Draw-only: swaps the address bar's Send
    /// cap to its spinner + "Sending" face (or "Cancel" on hover); its
    /// `Hit` stays registered while sending -- clicking it still cancels,
    /// routed by `App`'s `Hit::SendButton` handler checking `in_flight`.
    pub sending: bool,
    /// The method-badge cell's screen area, recorded on every draw; consumed
    /// by the click handling that opens the method dropdown (a later task).
    pub last_method_area: Option<Rect>,

    /// The 1-row screen area the URL text was last drawn into (after its
    /// left padding); consumed by the click handler that focuses the URL
    /// line and places the caret at the clicked column.
    pub last_url_text_area: Option<Rect>,
    /// When the in-flight send belonging to this editor started, mirrored
    /// from `Session::InFlight::started` by `App::update` alongside
    /// `sending`. Draw-only: `elapsed()` off this wall-clock instant drives
    /// the Send cap's spinner glyph while a request is in flight, the same
    /// way `ResponseState::InFlight`'s own spinner derives its frame from
    /// elapsed time (see `components::response`) rather than a tick
    /// counter. `None` whenever `sending` is false. The
    /// accent/accent_edge_dark breathe is a separate, eased effect (Task
    /// 14) driven by `AnimKey::SendBreathe` via `App::tick_send_breathe`,
    /// not this.
    pub send_started: Option<Instant>,
    /// Mirrors `App::table_collapsed`, synced by `App::update` on every
    /// action alongside `sending`. Draw-only: when set, the active tab's
    /// params/headers table body (header/rows/ghost/edge) is skipped and the
    /// tab strip's `⌄ hide`/`› show` toggle flips to `› show`.
    pub table_collapsed: bool,
    /// The Headers tab's read-only computed-headers section (spec §6):
    /// everything that will actually be sent beyond the editable request
    /// rows above — default headers (struck through when overridden), the
    /// auto Content-Type, and the client-generated Host/Content-Length
    /// rows. Recomputed every draw by `recompute_computed_headers` (cheap,
    /// small N) so an env switch or a body edit shows up live; never
    /// itself edited here.
    pub computed: ComputedHeadersView,
    /// The variable snapshot inline `{{token}}` highlighting and the hover
    /// tooltip resolve against (spec §7), synced by `App::update` on every
    /// action alongside `shadowed`. Draw-only: every surface that can show a
    /// token (URL bar, table cells, computed-header rows, the body editor)
    /// tints and registers its spans from this.
    pub vars: crate::components::var_tokens::VarView,
}

/// State for [`Editor::draw_computed_headers`]. `rows` is the full result
/// of `postui_core::prepare::computed_headers` (request-origin rows
/// included, so `Hit::AutoHeaderCopy`'s index can be mapped back to the
/// same non-`Request` subsequence the draw filtered); `revealed` is
/// whether secrets currently show in the clear; `has_secret` is whether
/// any row *would* be masked at the current values, independent of
/// `revealed` — this is what keeps the reveal/hide toggle visible after
/// revealing (a masked probe recomputed alongside `rows`, since `rows`
/// itself no longer carries [`postui_core::prepare::SECRET_MASK`] once
/// revealed).
#[derive(Debug, Default)]
pub struct ComputedHeadersView {
    pub rows: Vec<postui_core::prepare::ComputedHeader>,
    pub revealed: bool,
    pub has_secret: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            slug: None,
            name: None,
            saved: None,
            method: Method::Get,
            url: LineInput::new(""),
            substitute_body: false,
            params: IndexMap::new(),
            headers: IndexMap::new(),
            variables: IndexMap::new(),
            shadowed: IndexMap::new(),
            vars: Default::default(),
            inherited_headers: IndexMap::new(),
            body: new_body_state(""),
            body_hl_text: String::new(),
            body_hl_marker: None,
            body_handler: EditorEventHandler::emacs_mode(),
            body_sel_anchor: None,
            active_tab: EditorTab::Headers,
            preferred_tab: EditorTab::Headers,
            sub_focus: SubFocus::Url,
            table: TableEditorState::default(),
            last_body_area: None,
            sending: false,
            last_method_area: None,
            last_url_text_area: None,
            send_started: None,
            table_collapsed: false,
            computed: ComputedHeadersView::default(),
        }
    }
}

impl Editor {
    /// Loads `req` into the editor for editing, and records it as the
    /// last-saved state so `is_dirty` starts out `false`. Also re-masks the
    /// computed-headers section (`computed.revealed = false`): reveal is a
    /// per-request gesture (spec §3: secrets masked by default), so opening
    /// a different request must not carry an earlier reveal along with it
    /// -- the same re-masking-on-context-switch the Variable Manager's own
    /// scoped reveal already does.
    pub fn load(&mut self, slug: Option<String>, req: HttpRequest) {
        self.slug = slug;
        self.name = req.name.clone();
        self.method = req.method;
        self.url = LineInput::new(&req.url);
        self.substitute_body = req.substitute_body;
        self.params = req.params.clone();
        self.headers = req.headers.clone();
        self.variables = req.variables.clone();
        self.set_body_text(match &req.body {
            Some(Body::Json { text }) => text,
            None => "",
        });
        self.saved = Some(req);
        self.computed.revealed = false;
    }

    /// Swaps in `req`'s fields the way `load` does, for undo/redo stepping
    /// through history rather than opening a different request: leaves
    /// `slug`, `saved`, and `computed.revealed` untouched (the request
    /// identity and dirty baseline don't change, and reveal state is a
    /// per-request gesture unrelated to which snapshot is showing). Also
    /// drops any live table cell edit and selection — mirroring
    /// `Action::DiscardChanges` — so a snapshot swap can't leave a stale
    /// in-progress edit pointed at fields that just got replaced.
    pub fn apply_snapshot(&mut self, req: &HttpRequest) {
        self.name = req.name.clone();
        self.method = req.method;
        self.url = LineInput::new(&req.url);
        self.substitute_body = req.substitute_body;
        self.params = req.params.clone();
        self.headers = req.headers.clone();
        self.variables = req.variables.clone();
        self.set_body_text(match &req.body {
            Some(Body::Json { text }) => text,
            None => "",
        });
        self.table.editing = None;
        self.table.selected = None;
    }

    /// Where the caret sits right now, for `undo::Context::cursor_before`/
    /// `cursor_after`. `None` covers every focus state a step doesn't know
    /// how to restore (Method/Tabs/None sub-focus, or a table tab with
    /// nothing selected).
    pub fn cursor_pos(&self) -> crate::undo::CursorPos {
        use crate::undo::CursorPos;
        match self.sub_focus {
            SubFocus::Url => CursorPos::Url(self.url.cursor()),
            SubFocus::Content if self.active_tab == EditorTab::Body => CursorPos::Body {
                row: self.body.cursor.row,
                col: self.body.cursor.col,
            },
            SubFocus::Content => match self.table.selected.and_then(|i| self.table_key_at(i)) {
                Some(key) => CursorPos::Cell {
                    tab: self.active_tab,
                    key,
                },
                None => CursorPos::None,
            },
            _ => CursorPos::None,
        }
    }

    /// Restores a caret position captured by [`Self::cursor_pos`], clamping
    /// against whatever the fields now hold (an undo/redo step may have
    /// shortened the URL, dropped a body line, or removed a table key).
    /// `CursorPos::None` leaves focus exactly as it is.
    pub fn restore_cursor(&mut self, pos: &crate::undo::CursorPos) {
        use crate::undo::CursorPos;
        match pos {
            CursorPos::Url(i) => {
                self.sub_focus = SubFocus::Url;
                self.url.set_cursor(*i);
            }
            CursorPos::Body { row, col } => {
                self.sub_focus = SubFocus::Content;
                self.active_tab = EditorTab::Body;
                let rows = self.body.lines.len();
                let row = (*row).min(rows.saturating_sub(1));
                let col = (*col).min(self.body.lines.len_col(row).unwrap_or(0));
                self.body.cursor = edtui::Index2::new(row, col);
                // An undo/redo snapshot swap rebuilt the edtui state, whose
                // fresh view (num_rows = 0) makes edtui's own
                // scroll-to-cursor a no-op on the very next render — the
                // view would show the buffer top instead of the edit. Seed
                // the viewport here: the restored row roughly centered in
                // the last-drawn body height (top-aligned when no draw has
                // recorded one yet). Wrapped lines make this approximate;
                // edtui trues it up as soon as the cursor moves.
                let half = self
                    .last_body_area
                    .map(|a| usize::from(a.height) / 2)
                    .unwrap_or(0);
                self.body.set_viewport_offset(0, row.saturating_sub(half));
            }
            CursorPos::Cell { tab, key } => {
                self.active_tab = *tab;
                self.sub_focus = SubFocus::Content;
                self.table.selected = self.table_index_of(key);
            }
            CursorPos::None => {}
        }
    }

    /// Builds an `HttpRequest` from the editor's current field values.
    pub fn current_request(&self) -> HttpRequest {
        HttpRequest {
            name: self.name.clone(),
            method: self.method,
            url: self.url.text().to_string(),
            substitute_body: self.substitute_body,
            params: self.params.clone(),
            headers: self.headers.clone(),
            variables: self.variables.clone(),
            body: {
                let text = self.body_text();
                if text.is_empty() {
                    None
                } else {
                    Some(Body::Json { text })
                }
            },
        }
    }

    /// Recomputes `self.computed` from the editor's current fields against
    /// `ctx` — cheap (small N), so callers run it on every draw rather than
    /// trying to track exactly which edits invalidate it. `rows` uses the
    /// live `revealed` flag (masked unless the user has toggled reveal on);
    /// `has_secret` is a second, always-masked pass used only to decide
    /// whether the reveal/hide toggle should draw at all — it must stay
    /// independent of `revealed`, or revealing would make the toggle that
    /// un-reveals it disappear.
    pub fn recompute_computed_headers(&mut self, ctx: &postui_core::prepare::PrepareContext) {
        let req = self.current_request();
        self.computed.rows =
            postui_core::prepare::computed_headers(&req, ctx, !self.computed.revealed);
        self.computed.has_secret = postui_core::prepare::computed_headers(&req, ctx, true)
            .iter()
            .any(|r| r.value.contains(postui_core::prepare::SECRET_MASK));
    }

    /// A never-loaded (fresh scratch) editor is not dirty even though it has
    /// no saved snapshot to compare against; only a request that has been
    /// loaded and then changed counts as dirty.
    pub fn is_dirty(&self) -> bool {
        match &self.saved {
            Some(s) => *s != self.current_request(),
            None => false,
        }
    }

    pub fn mark_saved(&mut self) {
        self.saved = Some(self.current_request());
    }

    /// A request that was typed but never saved at all: no slug, no saved
    /// snapshot, and content that isn't the blank editor's. `is_dirty` can
    /// never be true here (there is nothing to diff against), yet quitting
    /// or opening another request over it loses real work — the gates
    /// check both.
    pub fn is_scratch_dirty(&self) -> bool {
        self.slug.is_none()
            && self.saved.is_none()
            && !(self.method == Method::Get
                && self.url.text().is_empty()
                && self.params.is_empty()
                && self.headers.is_empty()
                && self.variables.is_empty()
                && self.body_text().is_empty())
    }

    /// The name of the `{{token}}` the keyboard caret is currently sitting
    /// in, for the caret-resting tooltip (spec §7 — a keyboard user gets the
    /// same value readout a hover gives). Covers the two fields a caret can
    /// rest in: the URL line, and the body editor's current line. `None`
    /// anywhere else, and whenever the caret is outside every token.
    pub fn caret_token(&self) -> Option<String> {
        let (text, byte_off) = match self.sub_focus {
            SubFocus::Url => {
                let text = self.url.text().to_string();
                let off = char_byte_offset(&text, self.url.cursor());
                (text, off)
            }
            SubFocus::Content if self.active_tab == EditorTab::Body => {
                let cursor = self.body.cursor;
                let line: String = self.body.lines.iter_row().nth(cursor.row)?.iter().collect();
                let off = char_byte_offset(&line, cursor.col);
                (line, off)
            }
            _ => return None,
        };
        // Strictly inside the span: a caret parked immediately *after* a
        // token (the natural resting place right after typing one) would
        // otherwise hold a tooltip open over the pane indefinitely.
        postui_core::vars::find_tokens(&text)
            .into_iter()
            .find(|t| byte_off >= t.start && byte_off < t.end)
            .map(|t| t.name)
    }

    /// The body buffer's text, with lines joined by `\n`.
    pub fn body_text(&self) -> String {
        self.body.lines.to_string()
    }

    /// Feeds each char of `s` through the body's key handler as a
    /// synthesized plain `KeyCode::Char` event, as if the user had typed it.
    /// Used to splice a picked variable token into the body buffer, which
    /// (unlike `LineInput`) has no direct string-insertion API of its own.
    pub fn body_insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.body_handler.on_key_event(
                KeyEvent::new(
                    KeyCode::Char(c),
                    ratatui::crossterm::event::KeyModifiers::NONE,
                ),
                &mut self.body,
            );
        }
    }

    /// Replaces the whole body buffer. Only explicit user actions (loading a
    /// request, format, minify, the `$EDITOR` round-trip) may call this;
    /// typing goes through `body_handler`, so the text is never rewritten
    /// behind the user's back.
    pub fn set_body_text(&mut self, s: &str) {
        self.body = new_body_state(s);
        // The fresh state has no highlights; force the next draw to relex.
        self.body_hl_text.clear();
        self.body_hl_marker = None;
    }

    /// Whether the body parses as JSON. An empty body is vacuously valid:
    /// there is nothing to be wrong with yet.
    fn body_is_valid(&self) -> bool {
        let text = self.body_text();
        text.is_empty() || postui_core::json::validate(&text).is_ok()
    }

    /// `t`'s tab-strip label text: its name, plus a live entry count for
    /// Params/Headers/Vars once non-empty (Body never carries a count).
    /// Whether the Body tab is disabled: GET and HEAD requests send no
    /// body, so the tab can't be selected while one of them is the method.
    /// The body *text* is untouched — switching back to a body-sending
    /// method finds it exactly as it was.
    pub fn body_tab_disabled(&self) -> bool {
        !self.method.sends_body()
    }

    /// Whether the Body tab carries its validity badge: only once there's
    /// body text to validate, and never while the tab is disabled — a
    /// bright ✓ on a disabled tab made it read as lit as its neighbours.
    /// Shared by [`Self::draw_tab_bar`] and [`Self::tab_strip_spans`] since
    /// badge presence affects the tab's on-screen width.
    fn body_badge_present(&self) -> bool {
        !self.body_tab_disabled() && !self.body_text().is_empty()
    }

    /// Shared by [`Self::draw_tab_bar`] and [`Self::tab_strip_spans`] so
    /// the two can never drift apart on the counts that drive each tab's
    /// on-screen width.
    fn tab_label_text(&self, t: EditorTab) -> String {
        let count = match t {
            EditorTab::Params => self.params.len(),
            EditorTab::Headers => self.headers.len(),
            EditorTab::Vars => self.variables.len(),
            EditorTab::Body => 0,
        };
        if count > 0 {
            format!("{} · {count}", t.label())
        } else {
            t.label().to_string()
        }
    }

    /// The tab strip's current per-tab spans (see [`crate::paint::TabStrip::spans`]),
    /// in [`DRAW_ORDER`]. Used by `app.rs` to compute where the underline
    /// animation (Task 10) should retarget to on a tab switch, without
    /// needing a `Theme` — badge *presence* (Body only) affects a span's
    /// width, but badge color never does, so this stands in a fixed color
    /// where [`Self::draw_tab_bar`] uses the real validity color.
    pub fn tab_strip_spans(&self) -> Vec<(u16, u16)> {
        let labels: Vec<(String, Option<(char, ratatui::style::Color)>)> = DRAW_ORDER
            .iter()
            .map(|t| {
                let label = self.tab_label_text(*t);
                let badge = (matches!(t, EditorTab::Body) && self.body_badge_present())
                    .then_some(('_', ratatui::style::Color::Reset));
                (label, badge)
            })
            .collect();
        crate::paint::TabStrip::spans(&labels)
    }

    /// The body buffer's scroll state, as of the last draw. `None` unless the
    /// Body tab is showing (the other tabs have nothing scrollable wired up).
    ///
    /// `offset` is edtui's own viewport offset (its public
    /// `EditorState::viewport_offset`); `content` and `viewport` are counted
    /// in *logical* lines against rendered rows, which line up exactly until
    /// a line is long enough to wrap — the bar then reads slightly
    /// pessimistically (it shows a little more content than a page holds)
    /// rather than wrongly.
    pub fn scrollbar_spec(&self) -> Option<ScrollbarSpec> {
        if self.active_tab != EditorTab::Body {
            return None;
        }
        let area = self.last_body_area?;
        if area.height == 0 {
            return None;
        }
        Some(ScrollbarSpec {
            pane: PaneId::Editor,
            offset: self.body.viewport_offset().1,
            content: self.body.lines.len(),
            viewport: area.height as usize,
        })
    }

    /// Cursor-movement niceties edtui 0.11 doesn't provide, applied before
    /// its event handler sees the key: ←/→ wrapping across line boundaries,
    /// ctrl/alt+←/→ word hops (alt is the spelling macOS terminals deliver
    /// for option+arrow), ctrl/alt+Backspace word deletion, ctrl+Home/
    /// ctrl+End buffer jumps, and smart Home (first non-whitespace first,
    /// column 0 on the next press). Only unmodified (or exactly-ctrl/alt)
    /// combos are touched, so shifted selection keys and edtui's own emacs
    /// bindings pass through untouched. Returns true when handled here.
    fn body_nav_key(&mut self, ev: &KeyEvent) -> bool {
        let ctrl = ev.modifiers == KeyModifiers::CONTROL;
        let word = ctrl || ev.modifiers == KeyModifiers::ALT;
        let plain = ev.modifiers.is_empty();
        let cursor = self.body.cursor;
        let rows = self.body.lines.len();
        let len_of = |row: usize| self.body.lines.len_col(row).unwrap_or(0);
        let row_chars = |body: &EditorState, row: usize| -> Vec<char> {
            body.lines
                .iter_row()
                .nth(row)
                .map(|l| l.to_vec())
                .unwrap_or_default()
        };
        match ev.code {
            KeyCode::Left if word => {
                if cursor.col == 0 && cursor.row > 0 {
                    // At a line's start the hop wraps like plain Left.
                    self.body.cursor = edtui::Index2::new(cursor.row - 1, len_of(cursor.row - 1));
                } else {
                    let line = row_chars(&self.body, cursor.row);
                    self.body.cursor.col = super::word_nav::prev_word_boundary(&line, cursor.col);
                }
                true
            }
            KeyCode::Right if word => {
                if cursor.col >= len_of(cursor.row) && cursor.row + 1 < rows {
                    // At a line's end the hop wraps like plain Right.
                    self.body.cursor = edtui::Index2::new(cursor.row + 1, 0);
                } else {
                    let line = row_chars(&self.body, cursor.row);
                    self.body.cursor.col = super::word_nav::next_word_boundary(&line, cursor.col);
                }
                true
            }
            // Word deletion: select the hop word-left would make, then
            // reuse the selection-delete path. At a line's start there is
            // no same-line word behind; a plain Backspace (the line join)
            // is forwarded instead.
            // The ctrl+h spelling is how a physical ctrl+backspace reaches
            // a legacy terminal (crossterm parses the 0x08 byte as ctrl+h);
            // alt+h stays untouched — only alt+Backspace is a word delete.
            KeyCode::Backspace | KeyCode::Char('h')
                if (ev.code == KeyCode::Backspace && word)
                    || (ev.code == KeyCode::Char('h') && ctrl) =>
            {
                if cursor.col == 0 {
                    self.body_handler.on_key_event(
                        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                        &mut self.body,
                    );
                    return true;
                }
                let line = row_chars(&self.body, cursor.row);
                let target = super::word_nav::prev_word_boundary(&line, cursor.col);
                if target < cursor.col {
                    self.set_body_selection_cells(
                        edtui::Index2::new(cursor.row, target),
                        edtui::Index2::new(cursor.row, cursor.col - 1),
                    );
                    self.delete_body_selection();
                }
                true
            }
            KeyCode::Home if ctrl => {
                self.body.cursor = edtui::Index2::new(0, 0);
                true
            }
            KeyCode::End if ctrl => {
                let row = rows.saturating_sub(1);
                self.body.cursor = edtui::Index2::new(row, len_of(row));
                true
            }
            KeyCode::Left if plain && cursor.col == 0 && cursor.row > 0 => {
                self.body.cursor = edtui::Index2::new(cursor.row - 1, len_of(cursor.row - 1));
                true
            }
            KeyCode::Right
                if plain && cursor.col >= len_of(cursor.row) && cursor.row + 1 < rows =>
            {
                self.body.cursor = edtui::Index2::new(cursor.row + 1, 0);
                true
            }
            KeyCode::Home if plain => {
                let first_nw = self
                    .body
                    .lines
                    .iter_row()
                    .nth(cursor.row)
                    .and_then(|line| line.iter().position(|c| !c.is_whitespace()))
                    .unwrap_or(0);
                self.body.cursor.col = if cursor.col == first_nw { 0 } else { first_nw };
                true
            }
            _ => false,
        }
    }

    /// Forwards a raw mouse event to the body editor when the Body tab is
    /// active and the event landed inside the area it was last drawn into.
    /// Returns `true` when the event was consumed (edtui itself does its own
    /// narrower bounds check against the area it recorded at render time,
    /// which excludes the line-number gutter — this outer check only rules
    /// out events elsewhere in the app).
    pub fn handle_mouse(&mut self, m: ratatui::crossterm::event::MouseEvent) -> bool {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        use ratatui::layout::Position;

        if self.active_tab != EditorTab::Body {
            return false;
        }
        let Some(area) = self.last_body_area else {
            return false;
        };
        if !area.contains(Position {
            x: m.column,
            y: m.row,
        }) {
            return false;
        }
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            self.sub_focus = SubFocus::Content;
        }
        self.body_handler.on_mouse_event(m, &mut self.body);
        // edtui 0.11.6 gets the caret wrong on a plain click: its own
        // click→cursor mapping clamps the column to `len - 1` (so a click
        // past the end of a line lands *on* its last character instead of
        // after it) and, when the click is below the last line, its wrapped
        // walk falls through without ever setting a column, leaving the
        // clamp to snap the caret to the last character of the whole
        // buffer. Both are wrong for a desktop-style editor, so the click's
        // caret is recomputed here. Only `Down` is corrected: a drag that
        // reaches this far still rides on edtui's own mapping, where a
        // selection endpoint wants the clamped, on-a-character semantics.
        // (`App::handle_mouse` does not currently route drags here at all.)
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(cursor) = self.body_cursor_for_click(m.column, m.row) {
                self.body.cursor = cursor;
            }
            // A plain click collapses any selection and plants the anchor a
            // following drag will extend from.
            self.body.selection = None;
            self.body_sel_anchor = Some(self.body.cursor);
        }
        // Modeless invariant: whatever edtui's own handling did (its mouse
        // paths switch to Normal/Visual), the body editor never leaves
        // Insert.
        self.body.mode = EditorMode::Insert;
        true
    }

    /// Extends the body selection to the drag point `(x, y)`: the moving
    /// end of a selection anchored by the preceding left click. The head
    /// cell is included in the selection (edtui's inclusive-`end`
    /// semantics); dragging back onto the anchor cell collapses it.
    /// Returns `false` when there is no live anchor or the point doesn't
    /// resolve to a body position (e.g. the gutter).
    pub fn body_drag_to(&mut self, x: u16, y: u16) -> bool {
        if self.active_tab != EditorTab::Body {
            return false;
        }
        let Some(anchor) = self.body_sel_anchor else {
            return false;
        };
        let Some(cursor) = self.body_cursor_for_click(x, y) else {
            return false;
        };
        self.body.cursor = cursor;
        self.set_body_selection(anchor, cursor);
        self.body.mode = EditorMode::Insert;
        true
    }

    /// Sets `body.selection` to the inclusive cell range between two caret
    /// positions (each clamped onto its line's last character — carets sit
    /// on boundaries, selections cover cells), or clears it when both
    /// clamp to the same cell (a mouse drag that never left its cell is
    /// not a selection).
    fn set_body_selection(&mut self, a: edtui::Index2, b: edtui::Index2) {
        let (a, b) = (self.clamp_to_cell(a), self.clamp_to_cell(b));
        if a == b {
            self.body.selection = None;
            return;
        }
        self.set_body_selection_cells(a, b);
    }

    /// Sets `body.selection` to the inclusive cell range `a..=b` (either
    /// order), allowing a single-cell selection. edtui doesn't export its
    /// `Selection` type, so the range is built by letting
    /// `SwitchMode(Visual)` construct one and then rewriting its public
    /// endpoints; the mode goes straight back to Insert (modeless
    /// invariant).
    fn set_body_selection_cells(&mut self, a: edtui::Index2, b: edtui::Index2) {
        use edtui::actions::{Execute, SwitchMode};
        let (a, b) = (self.clamp_to_cell(a), self.clamp_to_cell(b));
        let cursor = self.body.cursor;
        self.body.cursor = a;
        SwitchMode(EditorMode::Visual).execute(&mut self.body);
        if let Some(sel) = self.body.selection.as_mut() {
            sel.end = b;
        }
        self.body.cursor = cursor;
        self.body.mode = EditorMode::Insert;
    }

    /// Sets `body.selection` from two caret *boundaries* (keyboard
    /// semantics: the selection covers the chars strictly between them, so
    /// one shift+Right selects exactly one char). Clears the selection
    /// when the boundaries coincide.
    fn set_body_selection_boundaries(&mut self, a: edtui::Index2, b: edtui::Index2) {
        let (lo, hi) = if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        if lo == hi {
            self.body.selection = None;
            return;
        }
        // The exclusive upper boundary becomes an inclusive end cell: the
        // char just before it (last char of the previous row when the
        // boundary sits at a line start).
        let end = if hi.col > 0 {
            edtui::Index2::new(hi.row, hi.col - 1)
        } else {
            let row = hi.row - 1; // hi > lo, so hi.row >= 1 here
            edtui::Index2::new(row, self.body.lines.len_col(row).unwrap_or(0))
        };
        self.set_body_selection_cells(lo, end);
    }

    /// Clamps a caret position onto a character cell of its line (col
    /// `len` -> `len - 1`; col 0 on an empty line).
    fn clamp_to_cell(&self, i: edtui::Index2) -> edtui::Index2 {
        let len = self.body.lines.len_col(i.row).unwrap_or(0);
        edtui::Index2::new(i.row, i.col.min(len.saturating_sub(1)))
    }

    /// Deletes the selected body text (cursor collapses to the selection
    /// start), staying in Insert mode. Selections cover characters, never
    /// the line break after them — but edtui's `DeleteSelection` (jagged
    /// `extract`) consumes that break whenever the selection ends on a
    /// row's last character, joining the next row up; the break is
    /// restored so deleting a selected word at a line end can't silently
    /// merge lines.
    fn delete_body_selection(&mut self) {
        use edtui::actions::{DeleteSelection, Execute, LineBreak};
        let Some(sel) = self.body.selection.as_ref() else {
            self.body_sel_anchor = None;
            return;
        };
        let (start, end) = (sel.start(), sel.end());
        let end_len = self.body.lines.len_col(end.row).unwrap_or(0);
        let eats_break = end.col + 1 >= end_len && end.row + 1 < self.body.lines.len();
        DeleteSelection.execute(&mut self.body);
        if eats_break {
            // The cursor sits exactly at the join point after the delete.
            LineBreak(1).execute(&mut self.body);
            self.body.cursor = start;
        }
        self.body_sel_anchor = None;
        self.body.mode = EditorMode::Insert;
    }

    /// The selected body text, rows joined with `\n` (the head cell
    /// included). `None` when nothing is selected.
    pub fn body_selected_text(&self) -> Option<String> {
        let sel = self.body.selection.as_ref()?;
        let (start, end) = (sel.start(), sel.end());
        let mut out = String::new();
        for (row, line) in self
            .body
            .lines
            .iter_row()
            .enumerate()
            .skip(start.row)
            .take(end.row - start.row + 1)
        {
            if row > start.row {
                out.push('\n');
            }
            let from = if row == start.row { start.col } else { 0 };
            let to = if row == end.row {
                (end.col + 1).min(line.len())
            } else {
                line.len()
            };
            out.extend(line.iter().skip(from).take(to.saturating_sub(from)));
        }
        Some(out)
    }

    /// Selects the entire body buffer (ctrl+a, the toolbar chip, and the
    /// palette command all land here).
    pub fn body_select_all(&mut self) {
        let last = self.body.lines.len().saturating_sub(1);
        let last_col = self.body.lines.len_col(last).unwrap_or(0);
        self.body_sel_anchor = Some(edtui::Index2::new(0, 0));
        self.set_body_selection_cells(edtui::Index2::new(0, 0), edtui::Index2::new(last, last_col));
    }

    /// Drops any body selection and its anchor.
    pub fn clear_body_selection(&mut self) {
        self.body.selection = None;
        self.body_sel_anchor = None;
    }

    /// Shifts the body lines the selection spans (the caret's line when
    /// nothing is selected) one tab stop right (`indent`) or left: Tab /
    /// shift+Tab on selected JSON. Indent prepends [`BODY_TAB_WIDTH`]
    /// spaces (empty lines are skipped — indenting them would only plant
    /// trailing whitespace); dedent strips up to one tab stop of leading
    /// whitespace, where a single `\t` counts as a full stop. The
    /// selection, its anchor, and the caret ride along with their lines'
    /// shifts, so repeated presses keep working on the same text.
    fn indent_body_lines(&mut self, indent: bool) {
        let (first, last) = match self.body.selection.as_ref() {
            Some(sel) => (sel.start().row, sel.end().row),
            None => (self.body.cursor.row, self.body.cursor.row),
        };
        let mut shifts = vec![0isize; last - first + 1];
        for row in first..=last {
            let Some(line) = self.body.lines.get_mut(edtui::RowIndex::new(row)) else {
                continue;
            };
            shifts[row - first] = if indent {
                if line.is_empty() {
                    continue;
                }
                line.splice(0..0, [' '; BODY_TAB_WIDTH]);
                BODY_TAB_WIDTH as isize
            } else {
                let n = if line.first() == Some(&'\t') {
                    1
                } else {
                    line.iter()
                        .take(BODY_TAB_WIDTH)
                        .take_while(|c| **c == ' ')
                        .count()
                };
                line.drain(0..n);
                -(n as isize)
            };
        }
        let shift_of = |i: edtui::Index2| -> edtui::Index2 {
            let s = *shifts.get(i.row.wrapping_sub(first)).unwrap_or(&0);
            edtui::Index2::new(i.row, i.col.saturating_add_signed(s))
        };
        self.body.cursor = shift_of(self.body.cursor);
        self.body_sel_anchor = self.body_sel_anchor.map(shift_of);
        if let Some(sel) = self.body.selection.as_mut() {
            // Shift raw endpoints in place (they may sit in either order),
            // then re-clamp onto character cells: a dedent that emptied an
            // endpoint's line can leave its col past the line's last char.
            sel.start = shift_of(sel.start);
            sel.end = shift_of(sel.end);
        }
        if let Some(sel) = self.body.selection.take() {
            self.set_body_selection_cells(sel.start, sel.end);
        }
    }

    /// Maps a screen click inside `last_body_area` to a body-buffer cursor,
    /// honouring `wrap(true)`, the line-number gutter and the vertical
    /// viewport offset — a port of edtui 0.11.6's
    /// `mouse_position_to_cursor_position` with the two corrections postui
    /// needs: the column clamps to the line's length (the caret sits AFTER
    /// the last character, which insert mode allows), and a click below the
    /// last rendered line resolves to the end of the LAST line rather than
    /// falling through.
    ///
    /// Returns `None` when the click cannot be resolved to a position —
    /// notably a click in the line-number gutter, which edtui ignores too
    /// (its recorded screen area starts after the gutter), so the caret
    /// stays put.
    fn body_cursor_for_click(&self, x: u16, y: u16) -> Option<edtui::Index2> {
        let area = self.last_body_area?;
        if area.height == 0 || y < area.y {
            return None;
        }
        // edtui records the post-gutter content rect in its `view.screen_area`,
        // but that field is crate-private, so the gutter width is recomputed
        // here the way edtui's `EditorView::line_number_width` does.
        let gutter = line_number_gutter_width(self.body.lines.len());
        let content_x = area.x.saturating_add(gutter);
        let width = usize::from(area.width.saturating_sub(gutter));
        if width == 0 || x < content_x {
            return None;
        }
        let mouse_row = usize::from(y - area.y);
        let mouse_col = usize::from(x - content_x);

        // Walk the visible logical lines, each occupying as many screen rows
        // as it wraps into, until the one covering `mouse_row` is found.
        let top = self.body.viewport_offset().1;
        let mut screen_row = 0usize;
        for (row, line) in (top..).zip(self.body.lines.iter_row().skip(top)) {
            let segments = wrap_segments(line, width);
            let rows = segments.len().max(1);
            if screen_row + rows > mouse_row {
                let col =
                    column_in_wrapped_line(line, &segments, mouse_row - screen_row, mouse_col);
                return Some(edtui::Index2::new(row, col));
            }
            screen_row += rows;
        }

        // Below the last line: the end of the last line, like every desktop
        // editor (edtui instead snapped to the last char of the buffer).
        let last = self.body.lines.len().checked_sub(1)?;
        Some(edtui::Index2::new(
            last,
            self.body.lines.len_col(last).unwrap_or(0),
        ))
    }
}

/// Byte offset of char index `idx` in `text` (the end offset when `idx` is
/// past the last char) — `find_tokens`'s spans are byte ranges, while both
/// caret positions postui tracks are char indices.
fn char_byte_offset(text: &str, idx: usize) -> usize {
    text.char_indices()
        .nth(idx)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// The tab width edtui renders the body with: its `ViewState` default, which
/// postui never overrides (there is no public getter to read it back).
const BODY_TAB_WIDTH: usize = 2;

/// The rendered width of `ch` in the body editor, matching edtui's own
/// `helper::char_width`.
fn body_char_width(ch: char) -> usize {
    use unicode_width::UnicodeWidthChar;
    if ch == '\t' {
        return BODY_TAB_WIDTH;
    }
    ch.width().unwrap_or(0)
}

/// The width of the line-number gutter edtui splits off the left of the body
/// area: one column per digit of the line count, plus a separating space.
/// Duplicates edtui 0.11.6's private `EditorView::line_number_width` (postui
/// always renders the body with `LineNumbers::Absolute`).
fn line_number_gutter_width(line_count: usize) -> u16 {
    let digits = line_count.max(1).to_string().len();
    u16::try_from(digits + 1).unwrap_or(u16::MAX)
}

/// Splits `line` into the character ranges edtui's `LineWrapper::wrap_line`
/// would render as successive screen rows in a `max_width`-wide content area.
fn wrap_segments(line: &[char], max_width: usize) -> Vec<std::ops::Range<usize>> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut line_width = 0usize;
    for (i, &ch) in line.iter().enumerate() {
        let char_width = body_char_width(ch);
        if line_width + char_width > max_width {
            segments.push(start..i);
            start = i;
            line_width = 0;
        }
        line_width += char_width;
    }
    if start < line.len() {
        segments.push(start..line.len());
    }
    segments
}

/// The buffer column a click at `mouse_col` on wrapped row `sub_row` of
/// `line` addresses. A click past the end of the row's text yields the
/// column *after* its last character — for the final wrapped row that is the
/// line's length, which is where a desktop editor puts the caret.
fn column_in_wrapped_line(
    line: &[char],
    segments: &[std::ops::Range<usize>],
    sub_row: usize,
    mouse_col: usize,
) -> usize {
    let Some(segment) = segments.get(sub_row) else {
        // Only reachable for an empty line, which renders as one blank row.
        return line.len();
    };
    let mut width = 0usize;
    let mut col = segment.start;
    for &ch in &line[segment.clone()] {
        let char_width = body_char_width(ch);
        if width + char_width > mouse_col {
            break;
        }
        width += char_width;
        col += 1;
    }
    col
}

/// edtui's emacs keybindings are all registered against `EditorMode::Insert`,
/// so a state left in the default `Normal` mode silently swallows every
/// keystroke. Building body buffers through here keeps that invariant in one
/// place.
fn new_body_state(text: &str) -> EditorState {
    let mut state = EditorState::new(Lines::from(text));
    state.mode = EditorMode::Insert;
    state
}

impl Component for Editor {
    fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match self.sub_focus {
            // The method badge: a plain control, not a text input, so
            // Enter/Space activate it (opening the chooser) and arrows only
            // navigate. alt+m still cycles the method without focusing it.
            SubFocus::Method => match ev.code {
                KeyCode::Enter | KeyCode::Char(' ') => Some(Action::OpenMethodDropdown),
                KeyCode::Right => {
                    self.sub_focus = SubFocus::Url;
                    Some(Action::Render)
                }
                KeyCode::Down => {
                    self.sub_focus = SubFocus::Tabs;
                    Some(Action::Render)
                }
                KeyCode::Esc => {
                    self.sub_focus = SubFocus::None;
                    Some(Action::Render)
                }
                _ => None,
            },
            SubFocus::Url => {
                // Left with the caret already at the start of the line has
                // no text to move over; it steps out onto the method badge
                // instead (the only keyboard route to it).
                if ev.code == KeyCode::Left && self.url.cursor() == 0 {
                    self.sub_focus = SubFocus::Method;
                    return Some(Action::Render);
                }
                if self.url.handle_key(ev) {
                    if ev.code == KeyCode::Char('{') && self.url.ends_with_at_cursor("{{") {
                        return Some(Action::OpenVarPicker { completing: true });
                    }
                    return Some(Action::Render);
                }
                if ev.code == KeyCode::Down {
                    self.sub_focus = SubFocus::Tabs;
                    return Some(Action::Render);
                }
                // Enter commits and Esc abandons; both blur the input, so a
                // caret on screen always means keys land in the URL line.
                if matches!(ev.code, KeyCode::Enter | KeyCode::Esc) {
                    self.sub_focus = SubFocus::None;
                    return Some(Action::Render);
                }
                None
            }
            // The tab strip: Left/Right switch tabs (the tab-change action
            // resets table state, so it goes through App like a click),
            // Down/Enter descend into the active tab's content, Up climbs
            // back to the URL line.
            SubFocus::Tabs => match ev.code {
                KeyCode::Left => Some(Action::EditorTabCycle(-1)),
                KeyCode::Right => Some(Action::EditorTabCycle(1)),
                KeyCode::Down | KeyCode::Enter => {
                    self.sub_focus = SubFocus::Content;
                    // Entering a table tab must land somewhere visible:
                    // select its first row — or, on an empty table, its
                    // ghost "+ Add" row (index 0 either way) — instead of
                    // a focused-but-nothing-selected limbo.
                    if matches!(
                        self.active_tab,
                        EditorTab::Params | EditorTab::Headers | EditorTab::Vars
                    ) && self.table.selected.is_none()
                    {
                        self.table.selected = Some(0);
                    }
                    Some(Action::Render)
                }
                KeyCode::Up => {
                    self.sub_focus = SubFocus::Url;
                    Some(Action::Render)
                }
                KeyCode::Esc => {
                    self.sub_focus = SubFocus::None;
                    Some(Action::Render)
                }
                _ => None,
            },
            // On the Params/Headers tabs the table editor gets first crack at
            // every key (including Up/Down navigation within the table); on
            // the Body tab edtui does, except for the two keys that are the
            // only keyboard route back out of the buffer.
            SubFocus::Content => {
                if matches!(
                    self.active_tab,
                    EditorTab::Params | EditorTab::Headers | EditorTab::Vars
                ) {
                    let map = match self.active_tab {
                        EditorTab::Params => &mut self.params,
                        EditorTab::Headers => &mut self.headers,
                        EditorTab::Vars => &mut self.variables,
                        EditorTab::Body => unreachable!(),
                    };
                    let outcome = self.table.handle_key(ev, map);
                    if outcome.consumed {
                        if let Some(i) = outcome.request_delete {
                            return Some(Action::ConfirmDeleteTableRow(i));
                        }
                        if ev.code == KeyCode::Char('{')
                            && self
                                .table
                                .editing
                                .as_ref()
                                .map(|e| &e.input)
                                .is_some_and(|i| i.ends_with_at_cursor("{{"))
                        {
                            return Some(Action::OpenVarPicker { completing: true });
                        }
                        return Some(match outcome.warning {
                            Some(w) => Action::ShowToast(w, ToastKind::Warning),
                            None => Action::Render,
                        });
                    }
                    // An unconsumed Up (empty table, or already at row 0)
                    // climbs out to the tab strip instead of being a dead
                    // end with no keyboard path back up the chain.
                    // Leaving the table also drops its selection — the mouse
                    // click-away path clears it, and a row that stays lit
                    // while keys land in the URL line misstates focus.
                    if ev.code == KeyCode::Up {
                        self.table.selected = None;
                        self.sub_focus = SubFocus::Tabs;
                        return Some(Action::Render);
                    }
                    // An Esc the table didn't consume (nothing selected, no
                    // edit in progress) blurs the pane's inputs entirely.
                    if ev.code == KeyCode::Esc {
                        self.sub_focus = SubFocus::None;
                        return Some(Action::Render);
                    }
                    return None;
                }
                // GUI-style selection first (plan 2026-08-23-text-selection):
                // ctrl+a selects the whole body (Home/ctrl+Home cover the
                // start-of-line jump edtui's emacs ctrl+a used to give),
                // shifted motions extend a selection through the same
                // wrap-aware nav path the unshifted keys use, and while a
                // selection is live the editing keys take their desktop
                // meanings (type to replace, Backspace/Delete to remove,
                // Esc to deselect).
                if ev.code == KeyCode::Char('a') && ev.modifiers == KeyModifiers::CONTROL {
                    self.body_select_all();
                    return Some(Action::Render);
                }
                let is_motion = matches!(
                    ev.code,
                    KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::Home
                        | KeyCode::End
                );
                if is_motion && ev.modifiers.contains(KeyModifiers::SHIFT) {
                    let anchor = *self.body_sel_anchor.get_or_insert(self.body.cursor);
                    let stripped =
                        KeyEvent::new(ev.code, ev.modifiers.difference(KeyModifiers::SHIFT));
                    if !self.body_nav_key(&stripped) {
                        self.body_handler.on_key_event(stripped, &mut self.body);
                    }
                    self.set_body_selection_boundaries(anchor, self.body.cursor);
                    self.body.mode = EditorMode::Insert;
                    return Some(Action::Render);
                }
                // Tab / shift+tab adjust indentation: with a selection Tab
                // shifts the selected lines a tab stop right instead of
                // replacing them, and shift+tab always shifts left.
                // shift+tab is consumed unconditionally — crossterm's
                // BackTab has no edtui KeyCode conversion (it panics
                // `unimplemented!()`), so it must never reach the handler.
                if ev.code == KeyCode::BackTab {
                    self.indent_body_lines(false);
                    return Some(Action::Render);
                }
                if ev.code == KeyCode::Tab
                    && ev.modifiers.is_empty()
                    && self.body.selection.is_some()
                {
                    self.indent_body_lines(true);
                    return Some(Action::Render);
                }
                if self.body.selection.is_some() {
                    match ev.code {
                        KeyCode::Esc => {
                            self.clear_body_selection();
                            return Some(Action::Render);
                        }
                        KeyCode::Backspace | KeyCode::Delete => {
                            self.delete_body_selection();
                            return Some(Action::Render);
                        }
                        // ctrl+h: a legacy terminal's ctrl+backspace.
                        KeyCode::Char('h') if ev.modifiers == KeyModifiers::CONTROL => {
                            self.delete_body_selection();
                            return Some(Action::Render);
                        }
                        // Typing replaces: delete the selection, then fall
                        // through so the key inserts at the collapsed
                        // cursor.
                        KeyCode::Char(_)
                            if ev.modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
                        {
                            self.delete_body_selection();
                        }
                        KeyCode::Enter if ev.modifiers.is_empty() => {
                            self.delete_body_selection();
                        }
                        // Any other key drops the selection and acts
                        // normally.
                        _ => self.clear_body_selection(),
                    }
                }
                // Esc blurs the buffer (Enter must stay a newline in a
                // multi-line editor); Up climbs out to the tab strip only
                // from the top row, so it can still navigate the body. CTRL/ALT
                // combos the keymap binds to an app action are shadowed here
                // (the router hands those to the global keymap first); any
                // unbound modified combo falls through to this component and
                // reaches edtui's own emacs-style bindings (ctrl+e/k etc.)
                // deliberately, so those keep working for body editing.
                if ev.code == KeyCode::Esc {
                    self.sub_focus = SubFocus::None;
                    return Some(Action::Render);
                }
                if ev.code == KeyCode::Up && self.body.cursor.row == 0 {
                    self.sub_focus = SubFocus::Tabs;
                    return Some(Action::Render);
                }
                if self.body_nav_key(&ev) {
                    return Some(Action::Render);
                }
                self.body_handler.on_key_event(ev, &mut self.body);
                Some(Action::Render)
            }
            // Blurred: no input captures keys. Down re-enters at the URL
            // line, mirroring the Url -> Down -> Content chain, so keyboard
            // users aren't stranded (alt+u and clicking work too).
            SubFocus::None => {
                if ev.code == KeyCode::Down {
                    self.sub_focus = SubFocus::Url;
                    return Some(Action::Render);
                }
                None
            }
        }
    }

    /// Wheel-over-an-unfocused-pane path (`Action::ScrollPane`). Only the
    /// Body tab has anything under edtui's control to scroll; synthesizes
    /// `|delta|` scroll events and forwards them through `handle_mouse` so
    /// both paths share edtui's own bounds check. Other tabs are a no-op
    /// here (params/headers scrolling isn't wired up).
    ///
    /// The synthesized column must land inside the area edtui itself
    /// recorded (`EditorState::view::screen_area`, set on every render),
    /// which — with a line-numbers gutter always on — is `last_body_area`
    /// *minus* the gutter's width on the left. That width is an edtui-
    /// internal calculation we have no public access to and would have to
    /// duplicate (and could silently drift out of sync with) if we aimed
    /// for the area's left edge, so instead this targets the area's last
    /// column: the gutter is only ever split off the left side, so the
    /// rightmost column of `last_body_area` is always inside edtui's
    /// content area, regardless of how wide the gutter is.
    fn handle_scroll(&mut self, delta: i16) {
        if self.active_tab != EditorTab::Body {
            return;
        }
        let Some(area) = self.last_body_area else {
            return;
        };
        let kind = if delta < 0 {
            ratatui::crossterm::event::MouseEventKind::ScrollUp
        } else {
            ratatui::crossterm::event::MouseEventKind::ScrollDown
        };
        let column = area.x + area.width.saturating_sub(1);
        for _ in 0..delta.unsigned_abs() {
            self.handle_mouse(ratatui::crossterm::event::MouseEvent {
                kind,
                column,
                row: area.y,
                modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
            });
        }
    }

    fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        let inner = pane_surface(frame.buffer_mut(), area, ctx.theme);

        // Params/Headers get exactly the height their compact table needs
        // (so unused rows read as empty pane rather than a stretched table);
        // Body still fills whatever is left. Collapsed, the table body is
        // skipped entirely (`Length(0)`) — this pane's own rect is already
        // shrunk to `CHROME_HEIGHT` by `layout::compute_layout` in that case
        // (it reads `App::table_collapsed` + the active tab the same way
        // this draw does), so `Length(0)` here just means "nothing left to
        // give it" rather than leaving dead space; the rows it would have
        // used are already the Response pane's.
        let content_constraint = match self.active_tab {
            _ if self.table_collapsed => Constraint::Length(0),
            EditorTab::Body => Constraint::Min(0),
            EditorTab::Params | EditorTab::Headers | EditorTab::Vars => {
                let (rows, active, active_hint) = self.table_geometry();
                let (inherited, computed_extra) = match self.active_tab {
                    EditorTab::Headers => {
                        let auto_rows = self.computed_row_count();
                        let divider = if auto_rows > 0 { 1 } else { 0 };
                        (
                            self.inherited_header_lines(ctx.theme).len() as u16,
                            auto_rows + divider,
                        )
                    }
                    // The referenced-vars section: one row per token the
                    // request references, plus its divider.
                    EditorTab::Vars => {
                        let refs = self.referenced_var_names().len() as u16;
                        (0, if refs > 0 { refs + 1 } else { 0 })
                    }
                    _ => (0, 0),
                };
                // Capped to what's left after the fixed address bar and tab
                // bar rows, never the other way around: those two must never
                // be squeezed to make room for a table that wants more
                // height than the pane has.
                let available = inner.height.saturating_sub(CHROME_HEIGHT);
                Constraint::Length(
                    (inherited + table_height(rows, active, active_hint) + computed_extra)
                        .min(available),
                )
            }
        };

        // Hidden, the tab bar is a single row (the labels' underline row
        // went with the labels). In every state the row heights are
        // clamped to what's actually left below the address bar:
        // over-constrained, ratatui shortchanges the *first* `Length`,
        // shearing the address bar's caps into its text row — both at the
        // settled collapsed height and at every mid-animation height while
        // the pane eases between the two.
        let below_bar = inner.height.saturating_sub(ADDRESS_BAR_HEIGHT);
        let tab_bar_height = if self.table_collapsed {
            1.min(below_bar)
        } else {
            TAB_BAR_HEIGHT.min(below_bar)
        };
        // The toolbar row holds the Body tab's body-only tools; every other
        // tab starts its content directly under the tab bar — and a hidden
        // editor drops the row along with the content it acts on.
        let toolbar_height = if self.active_tab == EditorTab::Body && !self.table_collapsed {
            TOOLBAR_HEIGHT.min(below_bar.saturating_sub(tab_bar_height))
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(ADDRESS_BAR_HEIGHT), // fused address bar + its ring margins
                Constraint::Length(tab_bar_height),     // tab bar (+ right-aligned save/vars)
                Constraint::Length(toolbar_height),     // Body-only tools chip row
                content_constraint,                     // active tab content
            ])
            .split(inner);

        self.draw_address_bar(frame, rows[0], ctx, hits);
        self.draw_tab_bar(frame, rows[1], ctx, hits);
        self.draw_toolbar(frame, rows[2], ctx, hits);
        self.draw_tab_content(frame, rows[3], ctx, hits);
    }
}

/// Fixed width, in cells, of the address bar's method segment.
const METHOD_SEGMENT_WIDTH: u16 = 10;
/// Fixed width, in cells, of the address bar's Send cap.
const SEND_SEGMENT_WIDTH: u16 = 24;

/// Height of the fused address bar + its ring margins — the first row of
/// `Editor::draw`'s vertical split.
pub const ADDRESS_BAR_HEIGHT: u16 = 5;

/// Columns of padding between the method segment and the URL text, so the
/// text isn't flush against the method button.
const URL_PAD: u16 = 2;
/// Fixed width, in cells, of the `❐` copy-URL chip at the URL well's right
/// edge (`" ❐ "` — one glyph column, one padding column each side).
const COPY_CHIP_WIDTH: u16 = 3;
/// Height of the tab bar row — the second row of that split.
pub const TAB_BAR_HEIGHT: u16 = 2;
/// Height of the toolbar chip row holding the Body tab's
/// format/minify/substitute/`$EDITOR` chips — the third row of that split
/// on the Body tab only. The other tabs have no body tools, and the
/// request-level save/vars chips live on the tab-label row, so they skip
/// the row entirely.
pub const TOOLBAR_HEIGHT: u16 = 1;
/// The fixed chrome above the tab content: address bar + full tab bar.
/// What the expanded pane's content height is measured against. Panes no
/// longer draw a border, so this is exactly the two rows' combined height
/// — no border-row inset to add. The *hidden* pane is smaller still — see
/// [`COLLAPSED_HEIGHT`].
pub const CHROME_HEIGHT: u16 = ADDRESS_BAR_HEIGHT + TAB_BAR_HEIGHT;

/// The hidden Editor pane's total height — what `layout::compute_layout`
/// shrinks the pane to while it's collapsed: the full address bar (the
/// request's controls stay usable while its tab content is put away) plus
/// a single tab-strip row holding only the `› show` toggle, the labels'
/// underline row having gone with the labels.
pub const COLLAPSED_HEIGHT: u16 = ADDRESS_BAR_HEIGHT + 1;

/// Cycled through (one glyph per `Action::Tick`) at the start of the Send
/// cap's label while a request is in flight.
const SPINNER_GLYPHS: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl Editor {
    /// Paints the fused method/URL/Send address bar: one 3-cell-row control
    /// (a shaded half-block cap row, the text row, a shaded half-block cap
    /// row — reading as 2 text lines) with no gap columns between its
    /// segments, plus a 1-row/1-column breathing margin around it. Focus is
    /// shown by lifting the focused segment's own fill/caps (no ring): the
    /// URL well brightens two hover-steps, the method badge takes hover's
    /// lift color. `area` is the whole envelope (5 rows tall, the pane's
    /// full inner width).
    fn draw_address_bar(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        use crate::paint::{bevel_bottom, bevel_top, fill, text};
        use crate::theme::{lift_color, mix};

        let theme = ctx.theme;
        // Focus styling must track where keys actually go: the URL only
        // counts as focused while its pane is, or the lifted fill/caret
        // would keep painting after Tab moves the keyboard elsewhere.
        let url_focused = ctx.focused && self.sub_focus == SubFocus::Url;
        let method_focused = ctx.focused && self.sub_focus == SubFocus::Method;

        let margins = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // breathing margin above
                Constraint::Length(3), // the bar itself
                Constraint::Length(1), // breathing margin below
            ])
            .split(area);
        let bar_outer = margins[1];
        let bar = Rect {
            x: bar_outer.x + 1,
            width: bar_outer.width.saturating_sub(2),
            ..bar_outer
        };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(METHOD_SEGMENT_WIDTH),
                Constraint::Min(0),
                Constraint::Length(SEND_SEGMENT_WIDTH),
            ])
            .split(bar);
        let method_area = cols[0];
        let url_area = cols[1];
        let send_area = cols[2];
        // Thin bevel rows above and below the centered text row, all three
        // on the control's own fill — the same 3-row solid anatomy `Button`
        // and `TextField` use.
        let text_y = bar.y + 1;
        let top_row = |r: Rect| Rect::new(r.x, r.y, r.width, 1);
        let bottom_row = |r: Rect| Rect::new(r.x, r.y + r.height - 1, r.width, 1);

        let buf = frame.buffer_mut();

        // --- method segment --------------------------------------------
        // Focus reuses hover's lift color: the badge has no pressed state,
        // so the lift is unambiguous, and hover/focus rarely coexist (a
        // deliberate choice over a ring — see the focus-outline sweep).
        // Both hover and focus ease in from the resting face rather than
        // snapping, via the shared `Hover`/`FocusFade` fade timers (hover:
        // `App::begin_hover_fade`, on every hover-hit change; focus:
        // `App::begin_focus_fade`, on `Action::FocusUrl`) — at rest (no fade
        // in flight) `hover_t`/`focus_t` default to 1.0, so the steady-state
        // colors below are unchanged from before.
        let method_face = theme.method_color(self.method);
        let method_lifted = lift_color(method_face, 0.12);
        let method_hovered = ctx.hovered == Some(&crate::hit::Hit::MethodSelector);
        let method_fill = if method_hovered {
            mix(method_face, method_lifted, ctx.hover_t())
        } else if method_focused {
            mix(method_face, method_lifted, ctx.focus_t())
        } else {
            method_face
        };
        // The bevel follows the currently shown fill so the whole segment
        // lifts on hover/focus, matching `Button`'s convention.
        let m_light = lift_color(method_fill, 0.12);
        let m_dark = lift_color(method_fill, -0.12);
        fill(buf, method_area, method_fill);
        bevel_top(buf, top_row(method_area), m_light, method_fill);
        bevel_bottom(buf, bottom_row(method_area), m_dark, method_fill);
        let method_label = format!("{} ▾", self.method.as_str());
        let label_w = method_label.chars().count() as u16;
        let start_x = method_area.x + method_area.width.saturating_sub(label_w) / 2;
        text(
            buf,
            start_x,
            text_y,
            &method_label,
            theme.on_accent,
            method_fill,
            true,
        );
        hits.register(method_area, crate::hit::Hit::MethodSelector);
        // The dropdown opens just below `anchor.y` (see `modal::draw_popup`),
        // so the anchor is the bar's bottom (cap) row -- not the whole hit
        // target -- or the popup would overlap the badge itself.
        self.last_method_area = Some(Rect {
            y: method_area.y + method_area.height - 1,
            height: 1,
            ..method_area
        });

        // --- URL segment -------------------------------------------------
        // Focus lifts the fill two hover-steps up — a stronger step than
        // `control_hover`, because the URL well is large and dark and a
        // single step is nearly invisible (same lift `TextField`'s Focused
        // state uses). The bevel follows the lifted fill, but at the
        // softer ±0.08 delta `TextField` uses around its own fill —
        // ±0.12 (the method badge's colored-face delta) reads as a hard
        // black line on the already-dark neutral control fill.
        let url_lifted = lift_color(theme.control, 0.12);
        let url_fill = if url_focused {
            mix(theme.control, url_lifted, ctx.focus_t())
        } else {
            theme.control
        };
        let (u_light, u_dark) = if url_focused {
            (lift_color(url_fill, 0.08), lift_color(url_fill, -0.08))
        } else {
            (theme.edge_light, theme.edge_dark)
        };
        fill(buf, url_area, url_fill);
        bevel_top(buf, top_row(url_area), u_light, url_fill);
        bevel_bottom(buf, bottom_row(url_area), u_dark, url_fill);
        hits.register(url_area, crate::hit::Hit::UrlBar);
        // The copy-URL chip claims a fixed slice at the well's right edge;
        // the text window is narrowed to leave room for it so the two never
        // overlap.
        let chip_w = COPY_CHIP_WIDTH.min(url_area.width);
        // The text is inset URL_PAD columns from the method segment so it
        // isn't flush against the badge.
        let url_text_area = Rect {
            x: url_area.x + URL_PAD.min(url_area.width),
            width: url_area
                .width
                .saturating_sub(URL_PAD)
                .saturating_sub(chip_w),
            ..url_area
        };
        let mut url_line = self
            .url
            .draw_line_windowed(url_focused, theme, url_text_area.width);
        url_line.style = Style::default().bg(url_fill).patch(url_line.style);
        buf.set_line(url_text_area.x, text_y, &url_line, url_text_area.width);
        // Token tinting paints over the text just drawn, and registers its
        // spans on top of `UrlBar` so a click lands on the token.
        crate::components::var_tokens::paint_var_tokens(
            buf,
            Rect::new(url_text_area.x, text_y, url_text_area.width, 1),
            &self.url.visible_window(url_focused, url_text_area.width),
            url_text_area.x,
            &self.vars,
            theme,
            hits,
        );
        self.last_url_text_area = Some(Rect {
            y: text_y,
            height: 1,
            ..url_text_area
        });

        // --- copy-URL chip ---------------------------------------------
        // A `Chip`-style tinted pill at the well's right edge; hover tints
        // it toward the well's accent-tinted color, blending in over the
        // shared hover fade like every other hovered control here. Drawn
        // (and registered) after the URL text/token painting above, so it
        // sits on top and wins the hit test over `UrlBar` beneath it.
        let chip_area = Rect {
            x: url_area.x + url_area.width.saturating_sub(chip_w),
            y: text_y,
            width: chip_w,
            height: 1,
        };
        if chip_area.width > 0 {
            let chip_hovered = ctx.hovered == Some(&crate::hit::Hit::CopyUrl);
            let chip_tinted = theme.tint(theme.accent, url_fill);
            let chip_bg = if chip_hovered {
                mix(url_fill, chip_tinted, ctx.hover_t())
            } else {
                url_fill
            };
            let chip_fg = if chip_hovered {
                theme.text
            } else {
                theme.text_muted
            };
            fill(buf, chip_area, chip_bg);
            text(
                buf,
                chip_area.x + chip_area.width / 2,
                chip_area.y,
                "❐",
                chip_fg,
                chip_bg,
                false,
            );
            hits.register(chip_area, crate::hit::Hit::CopyUrl);
        }

        // --- Send cap ------------------------------------------------------
        // In flight is a distinct state from disabled: the mouse-first
        // principle outranks visual tidiness here, so a send in progress
        // keeps `Hit::SendButton` registered (clicking it cancels, routed by
        // `App`'s existing `Hit::SendButton` handler checking `in_flight`).
        // Only a genuinely inert control -- not sending, and nothing to send
        // -- unregisters its hit.
        let url_empty = self.url.text().trim().is_empty();
        let disabled = !self.sending && url_empty;
        let send_hovered = ctx.hovered == Some(&crate::hit::Hit::SendButton);
        let (label, send_fill, label_fg, bold) = if self.sending {
            // Wall-clock frame index, matching `ResponseState::InFlight`'s
            // own spinner (`components::response`) rather than counting
            // ticks -- the tick period is adaptive (see `main.rs`), so a
            // tick-counted frame would race ahead while anything animates.
            let elapsed = self.send_started.map(|s| s.elapsed()).unwrap_or_default();
            let glyph =
                SPINNER_GLYPHS[(elapsed.subsec_millis() / 100) as usize % SPINNER_GLYPHS.len()];
            // The breathe pulse: `AnimKey::SendBreathe` ping-pongs 0<->1 over
            // `ui_settings.anim_ms.send_breathe` (700ms by default) for as
            // long as `App::tick_send_breathe` sees a real send in flight
            // (a separate driver from the testbed's own pingpong of the
            // same key -- they never run on the same screen). Defaults to
            // 0.0 (pure `accent`) the one frame a send starts before its
            // first `Action::Tick` has run.
            let breathe_t = ctx
                .anims
                .value_or(crate::anim::AnimKey::SendBreathe, ctx.now, 0.0);
            let face = mix(theme.accent, theme.accent_edge_dark, breathe_t);
            // Hovering a send in flight swaps the label to "Cancel" so the
            // click-to-cancel affordance is discoverable, without touching
            // the pulse/spinner face logic.
            let label = if send_hovered {
                "Cancel".to_string()
            } else {
                format!("{glyph} Sending")
            };
            (label, face, theme.on_accent, true)
        } else if disabled {
            (
                "Send".to_string(),
                theme.control,
                theme.text_disabled,
                false,
            )
        } else if send_hovered {
            (
                "Send".to_string(),
                mix(theme.accent, theme.accent_edge_light, ctx.hover_t()),
                theme.on_accent,
                true,
            )
        } else {
            ("Send".to_string(), theme.accent, theme.on_accent, true)
        };
        // The bevel follows the currently shown fill (matching `Button`'s
        // convention: the whole control reacts to hover/pulse), at the
        // accent delta `Button`'s Primary kind uses; disabled drops the
        // bevel entirely (flat fill, no edges), same as a disabled `Button`.
        fill(buf, send_area, send_fill);
        if !disabled {
            bevel_top(
                buf,
                top_row(send_area),
                lift_color(send_fill, 0.12),
                send_fill,
            );
            bevel_bottom(
                buf,
                bottom_row(send_area),
                lift_color(send_fill, -0.12),
                send_fill,
            );
        }
        let send_label_w = label.chars().count() as u16;
        let send_start_x = send_area.x + send_area.width.saturating_sub(send_label_w) / 2;
        text(buf, send_start_x, text_y, &label, label_fg, send_fill, bold);
        if !disabled {
            hits.register(send_area, crate::hit::Hit::SendButton);
        }
    }

    /// The key of the active tab's table row `i`, if it has one. Paired
    /// with [`Self::table_index_of`] to re-resolve a row across a commit
    /// that may have collapsed rows (and shifted every later index down).
    pub fn table_key_at(&self, i: usize) -> Option<String> {
        let map = match self.active_tab {
            EditorTab::Params => &self.params,
            EditorTab::Headers => &self.headers,
            EditorTab::Vars => &self.variables,
            EditorTab::Body => return None,
        };
        map.get_index(i).map(|(k, _)| k.clone())
    }

    /// How many rows the active tab's table has (the ghost row's index).
    pub fn table_len(&self) -> usize {
        match self.active_tab {
            EditorTab::Params => self.params.len(),
            EditorTab::Headers => self.headers.len(),
            EditorTab::Vars => self.variables.len(),
            EditorTab::Body => 0,
        }
    }

    /// Where `key` sits in the active tab's table now. `None` once the row
    /// is gone (a duplicate-key commit collapsed it away).
    pub fn table_index_of(&self, key: &str) -> Option<usize> {
        let map = match self.active_tab {
            EditorTab::Params => &self.params,
            EditorTab::Headers => &self.headers,
            EditorTab::Vars => &self.variables,
            EditorTab::Body => return None,
        };
        map.get_index_of(key)
    }

    /// Starts a new row on the active tab's table: focuses the table and
    /// begins editing the ghost row's key cell, exactly like clicking
    /// "+ Add …". A no-op on the Body tab. Any prior edit must already be
    /// committed (see `App`'s `Action::TableAddRow` arm).
    pub fn begin_add_row(&mut self) {
        let map = match self.active_tab {
            EditorTab::Params => &self.params,
            EditorTab::Headers => &self.headers,
            EditorTab::Vars => &self.variables,
            EditorTab::Body => return,
        };
        self.sub_focus = SubFocus::Content;
        self.table.begin_add(map);
    }

    /// Commits any in-progress table cell edit into the active tab's map —
    /// the click-away / focus-loss path. Its warning is the caller's to
    /// surface.
    pub fn commit_table(&mut self) -> TableOutcome {
        match self.active_tab {
            EditorTab::Params => self.table.commit(&mut self.params),
            EditorTab::Headers => self.table.commit(&mut self.headers),
            EditorTab::Vars => self.table.commit(&mut self.variables),
            EditorTab::Body => TableOutcome::default(),
        }
    }

    /// Begins editing one cell of the active tab's table in place,
    /// committing whatever was being edited before it.
    pub fn click_table_cell(&mut self, row: usize, col: Col) -> TableOutcome {
        match self.active_tab {
            EditorTab::Params => self.table.click_cell(row, col, &mut self.params),
            EditorTab::Headers => self.table.click_cell(row, col, &mut self.headers),
            EditorTab::Vars => self.table.click_cell(row, col, &mut self.variables),
            EditorTab::Body => TableOutcome::default(),
        }
    }

    /// `(total row-lines, active-row presence, active row carries a shadow
    /// hint)` for the active tab's table (Params/Headers/Vars), fed to
    /// [`table_height`] both by `draw`'s layout pass and
    /// `draw_tab_content`'s actual paint.
    fn table_geometry(&self) -> (usize, Option<usize>, bool) {
        let map = match self.active_tab {
            EditorTab::Params => &self.params,
            EditorTab::Headers => &self.headers,
            EditorTab::Vars => &self.variables,
            EditorTab::Body => return (0, None, false),
        };
        let map_len = map.len();
        // The ghost row is always drawn (it is `table_height`'s constant
        // `+ 1`); it only affects the geometry when it is the expanded row,
        // i.e. while it is being typed into.
        let rows = map_len;
        let active = self
            .table
            .active_index(map_len)
            .or_else(|| self.table.editing_ghost(map_len).then_some(map_len));
        let active_hint = active.is_some_and(|_| {
            self.active_tab == EditorTab::Vars
                && self
                    .table
                    .active_index(map_len)
                    .and_then(|i| map.get_index(i))
                    .is_some_and(|(k, _)| self.shadowed.contains_key(k))
        });
        (rows, active, active_hint)
    }

    fn draw_tab_bar(
        &self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        let theme = ctx.theme;
        let tabs = DRAW_ORDER;
        // Params/Headers carry their entry count inside the tab label; Body
        // carries the live JSON-validity badge, colored from the semantic
        // tokens so it also reads without the glyph.
        let tab_strip: Vec<(String, Option<(char, ratatui::style::Color)>)> = tabs
            .iter()
            .map(|t| {
                let label = self.tab_label_text(*t);
                let badge = match t {
                    EditorTab::Body if self.body_badge_present() => Some(if self.body_is_valid() {
                        ('✓', theme.success)
                    } else {
                        ('✗', theme.error)
                    }),
                    _ => None,
                };
                (label, badge)
            })
            .collect();
        let active = self.active_tab.draw_position();
        let hovered = tabs
            .iter()
            .enumerate()
            .find(|(i, _)| ctx.hovered == Some(&crate::hit::Hit::EditorTab(*i)))
            .map(|(i, _)| i);

        let strip_area = Rect { height: 2, ..area };
        let spans = crate::paint::TabStrip::spans(&tab_strip);
        let (static_left, static_width) = spans
            .get(active)
            .map(|(x, w)| (*x as f32, *w as f32))
            .unwrap_or((0.0, 0.0));
        // Independently animated left/right edges (Task 10): each key falls
        // back to this tab's own static edge when untracked, so the very
        // first draw of the strip snaps straight there with no slide-in
        // from zero — `app.rs`'s tab-switch handling is what actually sets
        // these keys in motion on a later switch.
        let left = ctx.anims.value_or(
            crate::anim::AnimKey::TabUnderline(crate::anim::StripId::EditorTabs),
            ctx.now,
            static_left,
        );
        let right = ctx.anims.value_or(
            crate::anim::AnimKey::TabUnderlineWidth(crate::anim::StripId::EditorTabs),
            ctx.now,
            static_left + static_width,
        );
        let underline = (left, right - left);
        // The pane-collapse progress doubles as the tabs' fade: hiding the
        // editor fades the tab labels (underline, badge, and vars indicator
        // with them) out entirely — settled hidden, the strip isn't painted
        // at all, and the `› show` toggle is the row's only surviving
        // control. Hidden tabs also take no clicks, so registration is
        // gated on the settled flag, not the fade.
        let fade_t = ctx
            .anims
            .value_or(
                crate::anim::AnimKey::PaneCollapse,
                ctx.now,
                if self.table_collapsed { 1.0 } else { 0.0 },
            )
            .clamp(0.0, 1.0);
        if fade_t < 1.0 {
            let rects = {
                let buf = frame.buffer_mut();
                crate::paint::TabStrip {
                    tabs: &tab_strip,
                    active,
                    hovered,
                    focused: ctx.focused && self.sub_focus == SubFocus::Tabs,
                    underline,
                    disabled: self
                        .body_tab_disabled()
                        .then(|| EditorTab::Body.draw_position()),
                }
                .paint(buf, strip_area, theme.page, theme)
            };
            if !self.table_collapsed {
                for (i, rect) in rects.iter().enumerate() {
                    hits.register(*rect, crate::hit::Hit::EditorTab(i));
                }
            }

            // The "vars" indicator sits right after the tab blocks, on the
            // labels row.
            if self.substitute_body {
                let last_rect = rects[tabs.len() - 1];
                let x = last_rect.x + last_rect.width + 2;
                frame.render_widget(
                    Paragraph::new(Line::styled("vars ", Style::default().fg(theme.accent))),
                    Rect::new(x, area.y, 5, 1),
                );
            }
            if fade_t > 0.0 {
                // Mid-slide: blend the whole strip toward the page it sits
                // on. The toggle is painted after this, so it never fades.
                crate::paint::fade_to(frame.buffer_mut(), strip_area, theme.page, fade_t);
            }
        }

        // --- collapse toggle (right-aligned) ---
        // Drawn on top of the same row's plain (unfilled) background, which
        // is why `on` here is `theme.page` — the app's own background,
        // never explicitly painted over this row.
        let buf = frame.buffer_mut();
        let toggle_label = if self.table_collapsed {
            "\u{203a} show"
        } else {
            "\u{2304} hide"
        };
        let toggle_hovered = ctx.hovered == Some(&crate::hit::Hit::TableCollapse);
        let toggle_w = toggle_label.chars().count() as u16;
        // One-column right inset, matching the Response pane's toggle so
        // the two line up on screen.
        let toggle_x = area.right().saturating_sub(toggle_w + 1);
        let toggle_fg = if toggle_hovered {
            theme.text
        } else {
            theme.text_muted
        };
        crate::paint::text(
            buf,
            toggle_x,
            area.y,
            toggle_label,
            toggle_fg,
            theme.page,
            false,
        );
        hits.register(
            Rect::new(toggle_x, area.y, toggle_w, 1),
            crate::hit::Hit::TableCollapse,
        );
    }

    /// Rows the computed-headers section will draw: every `self.computed`
    /// row that isn't `Request`-origin (those are already the editable
    /// table above). Shared by the height math in `draw` and the draw
    /// itself so they can never disagree.
    fn computed_row_count(&self) -> u16 {
        self.computed
            .rows
            .iter()
            .filter(|r| r.origin != postui_core::prepare::HeaderOrigin::Request)
            .count() as u16
    }

    /// Every `{{variable}}` the request references, deduped in first-seen
    /// order, from exactly the fields a send substitutes: the URL, enabled
    /// param and header keys/values, enabled request-var values, and the
    /// body only while `substitute_body` is on. Shared by the Vars tab's
    /// height math and its referenced-section draw so they can never
    /// disagree.
    fn referenced_var_names(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        let mut scan = |text: &str| {
            for t in postui_core::vars::find_tokens(text) {
                if seen.insert(t.name.clone()) {
                    names.push(t.name);
                }
            }
        };
        scan(self.url.text());
        for map in [&self.params, &self.headers] {
            for (k, e) in map {
                if e.enabled {
                    scan(k);
                    scan(&e.value);
                }
            }
        }
        for (_, e) in &self.variables {
            if e.enabled {
                scan(&e.value);
            }
        }
        if self.substitute_body {
            scan(&self.body_text());
        }
        names
    }

    /// Builds the muted status lines for enabled inherited (project-default)
    /// headers, shown above the request headers table. Each line notes
    /// whether the name is untouched by the request (`project`), overridden
    /// by an enabled request header (`overridden`), or shadowed by a
    /// disabled request header (`disabled`); the match against `self.headers`
    /// is case-insensitive.
    fn inherited_header_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        self.inherited_headers
            .iter()
            .filter(|(_, entry)| entry.enabled)
            .map(|(name, entry)| {
                let status = match self
                    .headers
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                {
                    Some((_, req_entry)) if req_entry.enabled => "overridden",
                    Some(_) => "disabled",
                    None => "project",
                };
                Line::styled(
                    format!("  ✓ {name}  {}  ({status})", entry.value),
                    Style::default().fg(theme.text_muted),
                )
            })
            .collect()
    }

    /// Paints the Body tab's toolbar chip row: `format`/`minify`/
    /// `substitute`/`$EDITOR` chips for the body-only actions that
    /// alt+f/alt+g/alt+b/ctrl+e already bind, but had no mouse-reachable
    /// equivalent before. Body-scoped only — the request-level save/vars
    /// chips live on the tab-label row (`draw_tab_bar`), so this row only
    /// exists while the Body tab is active. Chips reuse
    /// `Hit::FooterChip(Action)` and `footer::paint_chip_row`'s painting so
    /// hover/click behave exactly like the footer's own chips; `on_hit`
    /// already dispatches `FooterChip`'s action with no new `Hit` variant.
    fn draw_toolbar(
        &self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        if area.height == 0 {
            return;
        }
        let theme = ctx.theme;
        let buf = frame.buffer_mut();
        crate::paint::fill(buf, area, theme.panel);

        let sub_label = if self.substitute_body {
            "substitute {{on}}"
        } else {
            "substitute {{off}}"
        };
        let chips: Vec<(&str, &str, Option<Action>)> = vec![
            ("alt+f", "format", Some(Action::FormatBody)),
            ("alt+g", "minify", Some(Action::MinifyBody)),
            ("alt+b", sub_label, Some(Action::ToggleBodyVars)),
            ("alt+e", "$EDITOR", Some(Action::OpenBodyInEditor)),
            ("alt+x", "clear", Some(Action::BodyClear)),
        ];

        let right_limit = area.x + area.width;
        crate::components::footer::paint_chip_row(
            buf,
            area.y,
            area.x + 1,
            right_limit,
            &chips,
            theme,
            hits,
            ctx.hovered,
        );
    }

    fn draw_tab_content(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        let theme = ctx.theme;
        let focused = ctx.focused && self.sub_focus == SubFocus::Content;
        self.last_body_area = None;
        match self.active_tab {
            EditorTab::Params => {
                if self.table_collapsed {
                    return;
                }
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                    dragging: ctx.dragging,
                    anims: ctx.anims,
                    now: ctx.now,
                };
                self.table.draw(
                    frame,
                    area,
                    &self.params,
                    &table_ctx,
                    "+ Add param",
                    hits,
                    None,
                    &self.vars,
                );
            }
            EditorTab::Vars => {
                if self.table_collapsed {
                    return;
                }
                // Same split the Headers tab uses for its computed section:
                // the editable table keeps its full height first, and the
                // referenced list only gets what's left, clamped to what it
                // asked for.
                let refs = self.referenced_var_names();
                let (rows, active, active_hint) = self.table_geometry();
                let table_h = table_height(rows, active, active_hint).min(area.height);
                let refs_h = if refs.is_empty() {
                    0
                } else {
                    (refs.len() as u16 + 1).min(area.height.saturating_sub(table_h))
                };
                let refs_area = (refs_h > 0).then(|| Rect {
                    y: area.y + table_h,
                    height: refs_h,
                    ..area
                });
                let table_area = Rect {
                    height: table_h,
                    ..area
                };
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                    dragging: ctx.dragging,
                    anims: ctx.anims,
                    now: ctx.now,
                };
                self.table.draw(
                    frame,
                    table_area,
                    &self.variables,
                    &table_ctx,
                    "+ Add variable",
                    hits,
                    Some(&self.shadowed),
                    &self.vars,
                );
                if let Some(refs_area) = refs_area {
                    self.draw_referenced_vars(frame, refs_area, &refs, ctx, hits);
                }
            }
            EditorTab::Headers => {
                if self.table_collapsed {
                    return;
                }
                let inherited_lines = self.inherited_header_lines(theme);
                let table_area = if inherited_lines.is_empty() {
                    area
                } else {
                    let split = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(inherited_lines.len() as u16),
                            Constraint::Min(0),
                        ])
                        .split(area);
                    frame.render_widget(Paragraph::new(inherited_lines), split[0]);
                    split[1]
                };
                // The table keeps its own full height first — it's the
                // editable data — and the computed section only gets
                // whatever's left after that, clamped to what it asked
                // for; under real space pressure the auto section shrinks
                // (or disappears) rather than eating into the table.
                let auto_rows = self.computed_row_count();
                let (rows, active, active_hint) = self.table_geometry();
                let table_h = table_height(rows, active, active_hint).min(table_area.height);
                let computed_h = if auto_rows == 0 {
                    0
                } else {
                    (auto_rows + 1).min(table_area.height.saturating_sub(table_h))
                };
                let computed_area = (computed_h > 0).then(|| Rect {
                    y: table_area.y + table_h,
                    height: computed_h,
                    ..table_area
                });
                let table_area = Rect {
                    height: table_h,
                    ..table_area
                };
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                    dragging: ctx.dragging,
                    anims: ctx.anims,
                    now: ctx.now,
                };
                self.table.draw(
                    frame,
                    table_area,
                    &self.headers,
                    &table_ctx,
                    "+ Add header",
                    hits,
                    None,
                    &self.vars,
                );
                if let Some(computed_area) = computed_area {
                    self.draw_computed_headers(frame, computed_area, ctx, hits);
                }
            }
            EditorTab::Body => {
                // A 1-cell gutter all around hosts the focus ring (painted
                // below, only while `SubFocus::Content`) — reserved
                // unconditionally so the body's own geometry never shifts
                // as focus moves in and out; `body_cursor_for_click` and
                // friends read the click math straight back out of
                // `last_body_area`, so shrinking this once here is the only
                // place that needs to change for the ring to have room.
                let ring_area = area;
                let mut area = Rect {
                    x: area.x + 1,
                    y: area.y + 1,
                    width: area.width.saturating_sub(2),
                    height: area.height.saturating_sub(2),
                };
                // The bar takes the last column before edtui is told about
                // the area, so its own screen_area (which mouse routing is
                // resolved against) never overlaps the bar.
                let spec = ScrollbarSpec {
                    pane: PaneId::Editor,
                    offset: self.body.viewport_offset().1,
                    content: self.body.lines.len(),
                    viewport: area.height as usize,
                };
                if spec.overflows() && area.width > 1 {
                    let column = Rect {
                        x: area.x + area.width - 1,
                        width: 1,
                        ..area
                    };
                    area.width -= 1;
                    crate::hit::draw_scrollbar(
                        frame,
                        hits,
                        column,
                        &spec,
                        ctx.hovered,
                        ctx.dragging,
                        theme,
                    );
                }
                self.last_body_area = Some(area);
                hits.register(area, crate::hit::Hit::BodyEditor);
                if focused {
                    // Fades in via the shared `AnimKey::FocusFade` (the URL
                    // well's own focus lift retargets the same key — see
                    // `App::begin_focus_fade`'s doc comment) — `on` matches
                    // the gutter's own resting fill (`theme.page`, painted
                    // by `pane_surface`) so at t=0 the ring color equals its
                    // background and reads as not there yet.
                    let ring_color = crate::theme::mix(theme.page, theme.focus_ring, ctx.focus_t());
                    crate::paint::ring(frame.buffer_mut(), ring_area, ring_color, theme.page);
                }
                // Refresh the syntax highlights only when the text (or the
                // palette) actually changed — they're whole-buffer ranges
                // fed through edtui's Highlight mechanism, not a per-line
                // parse (see `json_body_highlights`).
                let text_now = self.body_text();
                if self.body_hl_text != text_now || self.body_hl_marker != Some(theme.accent) {
                    self.body
                        .set_highlights(json_body_highlights(&self.body.lines, theme));
                    self.body_hl_text = text_now;
                    self.body_hl_marker = Some(theme.accent);
                }
                let mut edtui_theme = EditorTheme::default()
                    .base(Style::default().bg(theme.page).fg(theme.text))
                    .cursor_style(Style::default().add_modifier(Modifier::REVERSED))
                    .line_numbers_style(Style::default().bg(theme.page).fg(theme.text_muted))
                    .selection_style(Style::default().bg(theme.selection).fg(theme.text))
                    .hide_status_line();
                // A cursor block on an unfocused pane reads as "you are typing
                // here", so only the focused editor shows one — and while a
                // selection is live the highlighted range is the visual
                // focus, so the block caret hides rather than dangling at
                // the selection's edge.
                if !focused || self.body.selection.is_some() {
                    edtui_theme = edtui_theme.hide_cursor();
                }
                let view = EditorView::new(&mut self.body)
                    .theme(edtui_theme)
                    .wrap(true)
                    .line_numbers(LineNumbers::Absolute);
                frame.render_widget(view, area);
                // Body coverage (spec §7): edtui paints the text itself, so
                // its tokens are found by reading the rendered rows back out
                // of the buffer. A token wrapped across two visual rows is
                // not `{{name}}` on either of them and is therefore skipped
                // — the documented limitation of this approach.
                let buf = frame.buffer_mut();
                for y in area.y..area.bottom() {
                    let row = Rect::new(area.x, y, area.width, 1);
                    let text = crate::components::var_tokens::row_text(buf, row);
                    if !text.contains("{{") {
                        continue;
                    }
                    crate::components::var_tokens::paint_var_tokens(
                        buf, row, &text, area.x, &self.vars, theme, hits,
                    );
                }
            }
        }
    }

    /// Paints the Headers tab's computed-headers section (spec §6, the
    /// user's #4 complaint: see everything that will actually be sent): a
    /// dim divider, then one dim row per `self.computed.rows` entry that
    /// isn't `Request`-origin — those are already the editable table
    /// above. A `DefaultHeader { suppressed: true }` row (overridden, or a
    /// duplicate default) renders struck through; a row with unresolved
    /// `{{tokens}}` tints its whole value `theme.error` (span-level tinting
    /// is Task 12). Each row gets a trailing `❐` copy icon
    /// (`Hit::AutoHeaderCopy`, indexed by its position in this filtered
    /// list); the divider carries a `👁 reveal`/`hide` toggle
    /// (`Hit::AutoHeaderReveal`) whenever `self.computed.has_secret`.
    /// The Vars tab's read-only "referenced" section: one row per
    /// `{{variable}}` the request references (see
    /// [`Self::referenced_var_names`]), laid out on the editable table's
    /// own name/value columns so the two read as one table — the token in
    /// the name column, its resolved value (secrets masked) in the value
    /// column, and the value's source right-aligned. Tokens are tinted
    /// and hover-tooltipped by `paint_var_tokens`, exactly like tokens
    /// drawn anywhere else.
    fn draw_referenced_vars(
        &self,
        frame: &mut Frame,
        area: Rect,
        names: &[String],
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let theme = ctx.theme;
        let dim = Style::default().fg(theme.text_muted);
        let mut y = area.y;
        let max_y = area.y.saturating_add(area.height);

        let prefix = "\u{2500}\u{2500} referenced ";
        let dash_w = area.width.saturating_sub(prefix.chars().count() as u16);
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("{prefix}{}", "\u{2500}".repeat(dash_w as usize)),
                dim,
            )),
            Rect::new(area.x, y, area.width, 1),
        );
        y += 1;

        let cols = super::table_editor::columns(area.x, area.width);
        let clip = |s: &str, avail: u16| -> String { s.chars().take(avail as usize).collect() };
        let buf = frame.buffer_mut();
        for name in names {
            if y >= max_y {
                break;
            }
            let info = self.vars.describe(name);
            let token_piece = format!("{{{{{name}}}}}");
            let name_w = cols.divider_x.saturating_sub(cols.name_x);
            crate::paint::text(
                buf,
                cols.name_x,
                y,
                &clip(&token_piece, name_w),
                theme.text_muted,
                theme.page,
                false,
            );
            // Retint the token itself (and register its tooltip hit).
            crate::components::var_tokens::paint_var_tokens(
                buf,
                Rect::new(cols.name_x, y, name_w, 1),
                &token_piece,
                cols.name_x,
                &self.vars,
                theme,
                hits,
            );
            // Source label right-aligned; the value gets what's between
            // the value column and the source, with a 2-cell gap.
            let source = format!("({})", info.source.label());
            let source_w = (source.chars().count() as u16).min(area.width);
            let source_x = area.right().saturating_sub(source_w + 1);
            crate::paint::text(
                buf,
                source_x,
                y,
                &source,
                theme.text_muted,
                theme.page,
                false,
            );
            let value_avail = source_x.saturating_sub(2).saturating_sub(cols.value_x);
            crate::paint::text(
                buf,
                cols.value_x,
                y,
                &clip(&info.display_value(), value_avail),
                theme.text_muted,
                theme.page,
                false,
            );
            y += 1;
        }
    }

    fn draw_computed_headers(
        &self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        use postui_core::prepare::HeaderOrigin;
        if area.height == 0 || area.width == 0 {
            return;
        }
        let theme = ctx.theme;
        let dim = Style::default().fg(theme.text_muted);
        let mut y = area.y;
        let max_y = area.y.saturating_add(area.height);

        // Divider, with the reveal/hide toggle right-aligned onto it when
        // there's a secret to reveal.
        if y < max_y {
            let toggle_label = if self.computed.revealed {
                "\u{1F441} hide"
            } else {
                "\u{1F441} reveal"
            };
            let show_toggle = self.computed.has_secret;
            let toggle_w = if show_toggle {
                toggle_label.chars().count() as u16 + 1
            } else {
                0
            };
            let prefix = "\u{2500}\u{2500} auto ";
            let dash_w = area
                .width
                .saturating_sub(prefix.chars().count() as u16 + toggle_w);
            let mut spans = vec![Span::styled(
                format!("{prefix}{}", "\u{2500}".repeat(dash_w as usize)),
                dim,
            )];
            if show_toggle {
                let hovered = ctx.hovered == Some(&crate::hit::Hit::AutoHeaderReveal);
                let toggle_style = if hovered {
                    Style::default().bg(theme.accent).fg(theme.on_accent)
                } else {
                    Style::default().fg(theme.accent)
                };
                spans.push(Span::styled(format!(" {toggle_label}"), toggle_style));
                let toggle_w = toggle_label.chars().count() as u16;
                let toggle_x = area.x.saturating_add(area.width.saturating_sub(toggle_w));
                hits.register(
                    Rect::new(toggle_x, y, toggle_w, 1),
                    crate::hit::Hit::AutoHeaderReveal,
                );
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, y, area.width, 1),
            );
            y += 1;
        }

        let auto = self
            .computed
            .rows
            .iter()
            .filter(|r| r.origin != HeaderOrigin::Request);
        for (i, row) in auto.enumerate() {
            if y >= max_y {
                break;
            }
            let suppressed = matches!(row.origin, HeaderOrigin::DefaultHeader { suppressed: true });
            let mut name_style = dim;
            // The value draws dim throughout; an unresolved `{{token}}` in
            // it is tinted span-level by `paint_var_tokens` below (Task 12
            // replaces Task 10's whole-value error tint, which coloured the
            // resolved parts of the value red too).
            let mut value_style = dim;
            if suppressed {
                name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
                value_style = value_style.add_modifier(Modifier::CROSSED_OUT);
            }
            let name_piece = format!("  {}: ", row.name);
            let value_x = area.x.saturating_add(name_piece.chars().count() as u16);
            let value_piece = row.value.clone();
            let text_len = name_piece.chars().count() + value_piece.chars().count();
            let glyph_hovered = ctx.hovered == Some(&crate::hit::Hit::AutoHeaderCopy(i));
            let glyph_style = if glyph_hovered {
                Style::default().bg(theme.accent).fg(theme.on_accent)
            } else {
                Style::default().fg(theme.accent)
            };
            let line = Line::from(vec![
                Span::styled(name_piece, name_style),
                Span::styled(value_piece, value_style),
                Span::styled(" ❐ ", glyph_style),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
            crate::components::var_tokens::paint_var_tokens(
                frame.buffer_mut(),
                Rect::new(value_x, y, area.right().saturating_sub(value_x), 1),
                &row.value,
                value_x,
                &self.vars,
                theme,
                hits,
            );
            let glyph_x = area.x.saturating_add(text_len as u16);
            let glyph_w = area.width.saturating_sub(text_len as u16).min(3);
            if glyph_w > 0 {
                hits.register(
                    Rect::new(glyph_x, y, glyph_w, 1),
                    crate::hit::Hit::AutoHeaderCopy(i),
                );
            }
            y += 1;
        }
    }
}

/// Context-aware JSON token highlights for the body buffer, matching the
/// response pane's palette exactly (see `token_color` in
/// `components::response`): keys `accent`, value strings `success`,
/// numbers `warning`, `true`/`false`/`null` `text_muted`; punctuation
/// keeps the base text color, so it needs no highlight. A string is a key
/// iff the next non-whitespace character after its closing quote is `:` —
/// the same classification the response tree gets from parsed JSON, but
/// derived lexically so it works on half-typed bodies too (an
/// unterminated string colors to its line's end). This replaces edtui's
/// syntect path, whose per-line parses forgot the enclosing `{` and
/// colored every key on a later line as a plain string.
fn json_body_highlights(lines: &Lines, theme: &Theme) -> Vec<edtui::Highlight> {
    use edtui::{Highlight, Index2};
    let rows: Vec<Vec<char>> = lines.iter_row().map(|l| l.to_vec()).collect();
    let mut out = Vec::new();
    let hl = |r: usize, a: usize, b: usize, color| {
        Highlight::new(
            Index2::new(r, a),
            Index2::new(r, b),
            Style::default().fg(color),
        )
    };
    let (mut r, mut c) = (0usize, 0usize);
    while r < rows.len() {
        let row = &rows[r];
        if c >= row.len() {
            r += 1;
            c = 0;
            continue;
        }
        let ch = row[c];
        if ch == '"' {
            // The whole string token, quotes included (the response pane's
            // Key/Str tokens carry their quotes too). JSON strings never
            // hold a raw newline, so an unterminated one stops at the line.
            let start = c;
            let mut j = c + 1;
            let mut closed = false;
            while j < row.len() {
                match row[j] {
                    '\\' => j += 2,
                    '"' => {
                        closed = true;
                        break;
                    }
                    _ => j += 1,
                }
            }
            let is_key = closed && {
                let (mut rr, mut cc) = (r, j + 1);
                loop {
                    match rows.get(rr).and_then(|rw| rw.get(cc)) {
                        Some(c2) if c2.is_whitespace() => cc += 1,
                        Some(c2) => break *c2 == ':',
                        None => {
                            if rr + 1 >= rows.len() {
                                break false;
                            }
                            rr += 1;
                            cc = 0;
                        }
                    }
                }
            };
            let color = if is_key { theme.accent } else { theme.success };
            let end = if closed { j } else { row.len() - 1 };
            out.push(hl(r, start, end, color));
            c = end + 1;
            continue;
        }
        if ch.is_ascii_digit() || (ch == '-' && row.get(c + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = c;
            let mut j = c + 1;
            while j < row.len() && matches!(row[j], '0'..='9' | '.' | 'e' | 'E' | '+' | '-') {
                j += 1;
            }
            out.push(hl(r, start, j - 1, theme.warning));
            c = j;
            continue;
        }
        if ch.is_ascii_alphabetic() {
            let start = c;
            let mut j = c;
            while j < row.len() && row[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word: String = row[start..j].iter().collect();
            if matches!(word.as_str(), "true" | "false" | "null") {
                out.push(hl(r, start, j - 1, theme.text_muted));
            }
            c = j;
            continue;
        }
        c += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::hit::Hit;
    use postui_core::model::{HttpRequest, Method};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    /// A disabled (instantly-jumping) `Anims` shared by every test's
    /// `DrawCtx`, so tests stay deterministic without threading an owned
    /// `Anims` through each call site.
    fn test_anims() -> &'static crate::anim::Anims {
        static ANIMS: std::sync::OnceLock<crate::anim::Anims> = std::sync::OnceLock::new();
        ANIMS.get_or_init(|| crate::anim::Anims::new(false))
    }

    #[test]
    fn typing_into_url_marks_dirty_and_updates_request() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
        assert!(!e.is_dirty());
        assert_eq!(e.sub_focus, SubFocus::Url, "load must not change sub_focus");
        e.handle_key(key(KeyCode::Char('/')));
        assert_eq!(e.current_request().url, "https://x/");
        assert!(e.is_dirty());
    }

    #[test]
    fn fresh_scratch_editor_is_not_dirty() {
        let e = Editor::default();
        assert!(!e.is_dirty(), "never-loaded editor must not be dirty");
    }

    #[test]
    fn mark_saved_clears_dirty_flag() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
        e.handle_key(key(KeyCode::Char('/')));
        assert!(e.is_dirty());
        e.mark_saved();
        assert!(!e.is_dirty());
    }

    #[test]
    fn method_cycles_via_action_and_tabs_select() {
        let mut app = App::new_for_test();
        app.update(Action::CycleMethod);
        assert_eq!(app.editor.method, Method::Post);
        app.update(Action::EditorTabSelect(3));
        assert_eq!(app.editor.active_tab, EditorTab::Body);
        app.update(Action::EditorTabCycle(1));
        assert_eq!(app.editor.active_tab, EditorTab::Headers, "cycle wraps");
    }

    #[test]
    fn tab_cycle_backward_wraps() {
        let mut app = App::new_for_test();
        app.update(Action::SetMethod(postui_core::model::Method::Post));
        assert_eq!(app.editor.active_tab, EditorTab::Headers);
        app.update(Action::EditorTabCycle(-1));
        assert_eq!(
            app.editor.active_tab,
            EditorTab::Body,
            "backward wraps to last tab"
        );
    }

    #[test]
    fn focus_url_action_focuses_editor_and_url() {
        use crate::layout::PaneId;
        let mut app = App::new_for_test();
        app.editor.sub_focus = SubFocus::Content;
        app.update(Action::FocusUrl);
        assert_eq!(app.focus, PaneId::Editor);
        assert_eq!(app.editor.sub_focus, SubFocus::Url);
    }

    #[test]
    fn up_down_walks_url_tabs_content_and_back() {
        // The vertical chain is URL -> tab strip -> content; Up walks it in
        // reverse (an empty Params table doesn't consume Up, so Editor's
        // fallback applies).
        let mut e = Editor::default();
        assert_eq!(
            e.sub_focus,
            SubFocus::Url,
            "default sub_focus starts on the URL line"
        );
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Tabs);
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Tabs);
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn body_tab_up_returns_to_tab_strip() {
        // Body tab has no table editor to intercept Up at all; from the
        // buffer's top row, Up climbs back out to the tab strip.
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Tabs);
    }

    /// A Body-tab editor with the buffer focused and `text` loaded.
    fn body_editor(text: &str) -> Editor {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.set_body_text(text);
        e
    }

    #[test]
    fn body_arrows_wrap_across_line_boundaries() {
        let mut e = body_editor("ab\ncd");
        e.body.cursor = edtui::Index2::new(0, 2); // after "ab"
        e.handle_key(key(KeyCode::Right));
        assert_eq!(
            e.body.cursor,
            edtui::Index2::new(1, 0),
            "Right at a line's end wraps to the next line's start"
        );
        e.handle_key(key(KeyCode::Left));
        assert_eq!(
            e.body.cursor,
            edtui::Index2::new(0, 2),
            "Left at a line's start wraps to the previous line's end"
        );
    }

    #[test]
    fn body_ctrl_home_and_ctrl_end_jump_to_the_buffer_ends() {
        let mut e = body_editor("ab\ncd\nef");
        e.body.cursor = edtui::Index2::new(1, 1);
        e.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(
            e.body.cursor,
            edtui::Index2::new(2, 2),
            "ctrl+End lands after the last char of the last line"
        );
        e.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
        assert_eq!(
            e.body.cursor,
            edtui::Index2::new(0, 0),
            "ctrl+Home lands at the buffer's start"
        );
    }

    #[test]
    fn body_ctrl_arrows_jump_by_word() {
        let mut e = body_editor("foo bar\nbaz");
        e.body.cursor = edtui::Index2::new(0, 0);
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 3), "end of foo");
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 7), "end of bar");
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(
            e.body.cursor,
            edtui::Index2::new(1, 0),
            "word-right at a line's end wraps like plain Right"
        );
        e.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(
            e.body.cursor,
            edtui::Index2::new(0, 7),
            "word-left at a line's start wraps to the previous line's end"
        );
        e.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 4), "start of bar");
    }

    #[test]
    fn body_alt_arrows_jump_by_word_for_macos() {
        let mut e = body_editor("foo bar");
        e.body.cursor = edtui::Index2::new(0, 0);
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 3));
        e.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 0));
    }

    #[test]
    fn body_ctrl_shift_right_selects_the_next_word() {
        let mut e = body_editor("foo bar");
        e.body.cursor = edtui::Index2::new(0, 0);
        e.handle_key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 3));
        assert_eq!(e.body_selected_text().as_deref(), Some("foo"));
    }

    #[test]
    fn body_ctrl_backspace_deletes_the_previous_word() {
        let mut e = body_editor("foo bar");
        e.body.cursor = edtui::Index2::new(0, 7);
        e.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(e.body_text(), "foo ");
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 4));
        // The macOS spelling of the same gesture: skips the whitespace and
        // takes the word behind it too.
        e.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(e.body_text(), "");
        assert_eq!(e.body.cursor, edtui::Index2::new(0, 0));
    }

    #[test]
    fn restoring_a_body_cursor_seeds_the_viewport_near_the_edit() {
        let text: String = (0..100).map(|i| format!("\"l{i}\": {i},\n")).collect();
        let mut e = body_editor(&text);
        e.last_body_area = Some(Rect::new(0, 0, 60, 10));
        e.restore_cursor(&crate::undo::CursorPos::Body { row: 50, col: 0 });
        let offset = e.body.viewport_offset().1;
        assert!(
            (41..=50).contains(&offset),
            "the restored row (50) sits inside a 10-row viewport at offset {offset}"
        );
    }

    #[test]
    fn restoring_a_body_cursor_with_no_known_area_still_scrolls_to_the_row() {
        let text: String = (0..100).map(|i| format!("\"l{i}\": {i},\n")).collect();
        let mut e = body_editor(&text);
        e.last_body_area = None;
        e.restore_cursor(&crate::undo::CursorPos::Body { row: 50, col: 0 });
        assert_eq!(
            e.body.viewport_offset().1,
            50,
            "unknown height: top-align the restored row"
        );
    }

    /// Draws `e` and returns the fg color of the buffer cell holding the
    /// first occurrence of `needle` inside the body area.
    fn body_cell_fg(e: &mut Editor, needle: char) -> ratatui::style::Color {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let area = e.last_body_area.unwrap();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if buf[(x, y)].symbol() == needle.to_string() {
                    return buf[(x, y)].fg;
                }
            }
        }
        panic!("{needle:?} not found in the body area");
    }

    #[test]
    fn multiline_body_keys_highlight_like_the_response_pane() {
        // A formatted body: the keys sit on lines BELOW their `{`. A
        // per-line parse loses that context and colors them as plain
        // strings; the response-matched highlighter must not.
        let theme = Theme::dark();
        let mut e = body_editor("{\n  \"kzz\": [17, \"vzz\", true],\n  \"qzz\": null\n}");
        e.method = Method::Post;
        assert_eq!(body_cell_fg(&mut e, 'k'), theme.accent, "key on line 2");
        assert_eq!(body_cell_fg(&mut e, 'q'), theme.accent, "key on line 3");
        assert_eq!(body_cell_fg(&mut e, 'v'), theme.success, "value string");
        assert_eq!(body_cell_fg(&mut e, '7'), theme.warning, "number");
        assert_eq!(body_cell_fg(&mut e, 't'), theme.text_muted, "literal");
        assert_eq!(body_cell_fg(&mut e, '['), theme.text, "punctuation");
    }

    #[test]
    fn json_body_highlights_classifies_tokens_with_full_context() {
        let theme = Theme::dark();
        let lines = Lines::from("{\n  \"key\": \"a:b\",\n  \"n\":\n    12.5e3\n}");
        let hls = json_body_highlights(&lines, &theme);
        let find = |row: usize, col: usize| {
            hls.iter()
                .find(|h| h.start.row == row && h.start.col == col)
                .unwrap_or_else(|| panic!("no highlight starting at ({row},{col}): {hls:?}"))
        };
        // `"key"` on row 1 (cols 2..=6, quotes included) is a key.
        let key = find(1, 2);
        assert_eq!(key.end, edtui::Index2::new(1, 6));
        assert_eq!(key.style.fg, Some(theme.accent));
        // `"a:b"` is a value string — the colon INSIDE the quotes must not
        // make it a key.
        assert_eq!(find(1, 9).style.fg, Some(theme.success));
        // `"n"` is a key even with its colon... on the same row here, but
        // the number VALUE sits on the next row.
        assert_eq!(find(2, 2).style.fg, Some(theme.accent));
        // `12.5e3` on row 3 is one number token.
        let num = find(3, 4);
        assert_eq!(num.end, edtui::Index2::new(3, 9));
        assert_eq!(num.style.fg, Some(theme.warning));
    }

    #[test]
    fn json_body_highlights_survives_half_typed_json() {
        let theme = Theme::dark();
        // An unterminated string colors to its line's end and no further.
        let lines = Lines::from("{\n  \"unfinished: 1\n}");
        let hls = json_body_highlights(&lines, &theme);
        let h = hls
            .iter()
            .find(|h| h.start == edtui::Index2::new(1, 2))
            .expect("unterminated string still highlights");
        assert_eq!(h.end.row, 1, "never crosses the line break");
        // A key whose colon sits on the NEXT line still reads as a key.
        let lines = Lines::from("{\n  \"k\"\n  : 1\n}");
        let hls = json_body_highlights(&lines, &theme);
        let h = hls
            .iter()
            .find(|h| h.start == edtui::Index2::new(1, 2))
            .expect("key highlight");
        assert_eq!(h.style.fg, Some(theme.accent));
    }

    #[test]
    fn body_caret_hides_while_a_selection_is_live() {
        let reversed_cells = |e: &mut Editor| {
            let theme = Theme::dark();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(120, 20);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let area = e.last_body_area.unwrap();
            let mut n = 0;
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if buf[(x, y)].modifier.contains(Modifier::REVERSED) {
                        n += 1;
                    }
                }
            }
            n
        };
        let mut e = body_editor("foo bar");
        e.method = Method::Post;
        assert!(
            reversed_cells(&mut e) > 0,
            "no selection: the caret cell renders reversed"
        );
        e.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        // The selection itself renders via the theme's selection *color*,
        // not REVERSED — so with the caret hidden no reversed cell remains.
        assert_eq!(
            reversed_cells(&mut e),
            0,
            "selection live: the block caret hides"
        );
    }

    #[test]
    fn body_ctrl_h_is_word_backspace_for_legacy_terminals() {
        // Terminals without the enhanced-keys protocol deliver a physical
        // ctrl+backspace as the 0x08 byte, which crossterm parses as
        // ctrl+h — so ctrl+h must mean the same word deletion.
        let mut e = body_editor("foo bar");
        e.body.cursor = edtui::Index2::new(0, 7);
        e.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(e.body_text(), "foo ");
    }

    #[test]
    fn body_home_goes_to_first_non_whitespace_then_column_zero() {
        let mut e = body_editor("  \"a\": 1");
        e.body.cursor = edtui::Index2::new(0, 6);
        e.handle_key(key(KeyCode::Home));
        assert_eq!(e.body.cursor.col, 2, "first press: first non-whitespace");
        e.handle_key(key(KeyCode::Home));
        assert_eq!(e.body.cursor.col, 0, "second press: column 0");
        e.handle_key(key(KeyCode::Home));
        assert_eq!(e.body.cursor.col, 2, "and it toggles back");
    }

    #[test]
    fn body_home_on_an_all_whitespace_line_goes_to_column_zero() {
        let mut e = body_editor("    ");
        e.body.cursor = edtui::Index2::new(0, 3);
        e.handle_key(key(KeyCode::Home));
        assert_eq!(e.body.cursor.col, 0);
    }

    #[test]
    fn left_at_url_start_focuses_method_and_right_returns() {
        let mut e = Editor {
            url: LineInput::new("x"), // caret starts at the end, after "x"
            sub_focus: SubFocus::Url,
            ..Editor::default()
        };
        // With the caret mid-text, Left is caret movement, not navigation.
        e.handle_key(key(KeyCode::Left));
        assert_eq!(e.sub_focus, SubFocus::Url);
        // At the start of the line there is nothing left to move over; the
        // next Left steps out onto the method badge.
        e.handle_key(key(KeyCode::Left));
        assert_eq!(e.sub_focus, SubFocus::Method);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn method_focus_enter_or_space_opens_the_dropdown() {
        let mut e = Editor {
            sub_focus: SubFocus::Method,
            ..Editor::default()
        };
        assert_eq!(
            e.handle_key(key(KeyCode::Enter)),
            Some(Action::OpenMethodDropdown)
        );
        assert_eq!(
            e.handle_key(key(KeyCode::Char(' '))),
            Some(Action::OpenMethodDropdown)
        );
    }

    #[test]
    fn method_focus_down_lands_on_tab_strip_and_esc_blurs() {
        let mut e = Editor {
            sub_focus: SubFocus::Method,
            ..Editor::default()
        };
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Tabs);
        e.sub_focus = SubFocus::Method;
        e.handle_key(key(KeyCode::Esc));
        assert_eq!(e.sub_focus, SubFocus::None);
    }

    #[test]
    fn tab_strip_left_right_switch_tabs_and_enter_descends() {
        let mut e = Editor {
            sub_focus: SubFocus::Tabs,
            ..Editor::default()
        };
        assert_eq!(
            e.handle_key(key(KeyCode::Right)),
            Some(Action::EditorTabCycle(1))
        );
        assert_eq!(
            e.handle_key(key(KeyCode::Left)),
            Some(Action::EditorTabCycle(-1))
        );
        e.handle_key(key(KeyCode::Enter));
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.sub_focus = SubFocus::Tabs;
        e.handle_key(key(KeyCode::Esc));
        assert_eq!(e.sub_focus, SubFocus::None);
    }

    #[test]
    fn descending_from_the_tab_strip_lands_on_a_visible_selection() {
        // Entering the content must never be an invisible state: on
        // Params/Headers the first row (or the ghost + Add row of an empty
        // table) is selected immediately, so every keyboard stop shows.
        let mut e = Editor::default();
        e.params.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        e.sub_focus = SubFocus::Tabs;
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        assert_eq!(e.table.selected, Some(0), "first row selected on entry");

        let mut e = Editor {
            sub_focus: SubFocus::Tabs,
            ..Editor::default() // empty params table
        };
        e.handle_key(key(KeyCode::Enter));
        assert_eq!(e.sub_focus, SubFocus::Content);
        assert_eq!(
            e.table.selected,
            Some(0),
            "an empty table's entry point is its ghost + Add row"
        );
    }

    #[test]
    fn params_tab_up_at_row_zero_returns_to_tab_strip() {
        // A non-empty table still doesn't consume Up at row 0 (the top),
        // so Editor's fallback kicks in even though the table has rows.
        let mut e = Editor::default();
        e.params.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        e.params.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        e.sub_focus = SubFocus::Content;
        e.table.selected = Some(0);
        let action = e.handle_key(key(KeyCode::Up));
        assert_eq!(action, Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::Tabs);
        assert_eq!(
            e.table.selected, None,
            "leaving the table clears its selection, matching the mouse \
             click-away path — keys go to the URL line, so a still-lit row \
             would be lying about where input lands"
        );
    }

    #[test]
    fn params_tab_up_below_row_zero_navigates_within_table() {
        // Once selected has moved off row 0, Up navigates the table instead
        // of returning focus to the URL line.
        let mut e = Editor::default();
        e.params.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        e.params.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        e.sub_focus = SubFocus::Content;
        e.table.selected = Some(1);
        let action = e.handle_key(key(KeyCode::Up));
        assert_eq!(action, Some(Action::Render));
        assert_eq!(
            e.sub_focus,
            SubFocus::Content,
            "table navigation must not move focus"
        );
        assert_eq!(e.table.selected, Some(0));
    }

    #[test]
    fn duplicate_key_commit_in_params_tab_shows_warning_toast() {
        let mut e = Editor {
            active_tab: EditorTab::Params,
            ..Editor::default()
        };
        for (k, v) in [("a", "1"), ("b", "2")] {
            e.params.insert(
                k.into(),
                Entry {
                    value: v.into(),
                    enabled: true,
                },
            );
        }
        e.sub_focus = SubFocus::Content;
        // Rename "a" onto "b": the two rows collapse, with a warning.
        e.table.selected = Some(0);
        e.handle_key(key(KeyCode::Enter));
        e.handle_key(key(KeyCode::Backspace));
        e.handle_key(key(KeyCode::Char('b')));
        let action = e.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, Some(Action::ShowToast(_, ToastKind::Warning))),
            "expected a warning toast, got {action:?}"
        );
        assert_eq!(e.params.len(), 1);
        assert_eq!(e.params["b"].value, "1");

        // Typing an existing key into the ghost row warns too, rather than
        // silently overwriting the row that already owns the key.
        e.handle_key(key(KeyCode::Char('a'))); // opens the ghost row
        e.handle_key(key(KeyCode::Char('b')));
        let action = e.handle_key(key(KeyCode::Tab));
        assert!(
            matches!(action, Some(Action::ShowToast(_, ToastKind::Warning))),
            "expected a warning toast, got {action:?}"
        );
        assert_eq!(e.params.len(), 1, "no second 'b' row");
        assert_eq!(e.params["b"].value, "1", "the existing value is untouched");
    }

    #[test]
    fn body_text_roundtrip_and_empty_means_no_body() {
        let mut e = Editor::default();
        e.set_body_text("{\n  \"a\": 1\n}");
        assert_eq!(e.body_text(), "{\n  \"a\": 1\n}");
        assert!(matches!(e.current_request().body, Some(Body::Json { .. })));
        e.set_body_text("");
        assert!(e.current_request().body.is_none());
    }

    #[test]
    fn typing_in_body_tab_inserts_text_modelessly() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.handle_key(key(KeyCode::Char('{')));
        assert_eq!(
            e.body_text(),
            "{",
            "emacs mode: chars insert without entering a vim insert mode"
        );
    }

    #[test]
    fn esc_in_body_blurs_the_editor_without_editing() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.set_body_text("{}");
        assert_eq!(e.handle_key(key(KeyCode::Esc)), Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::None);
        assert_eq!(e.body_text(), "{}");
    }

    #[test]
    fn body_up_leaves_editor_only_from_the_first_row() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.set_body_text("one\ntwo");
        e.handle_key(key(KeyCode::Down)); // move to row 1 inside the body
        assert_eq!(
            e.sub_focus,
            SubFocus::Content,
            "Up/Down navigate inside a multi-line body"
        );
        e.handle_key(key(KeyCode::Up)); // back to row 0, still inside
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.handle_key(key(KeyCode::Up)); // at row 0 → climb out to the tab strip
        assert_eq!(e.sub_focus, SubFocus::Tabs);
    }

    #[test]
    fn save_preserves_invalid_body_verbatim() {
        let mut e = Editor::default();
        e.set_body_text("{ \"in-progress\": ");
        let req = e.current_request();
        let back = HttpRequest::from_toml_str(&req.to_toml_string()).unwrap();
        assert_eq!(
            back.body,
            Some(Body::Json {
                text: "{ \"in-progress\": ".into()
            })
        );
    }

    #[test]
    fn format_body_pretty_prints_only_valid_json() {
        let mut app = App::new_for_test();
        app.editor.set_body_text("{\"a\":1}");
        app.update(Action::FormatBody);
        assert!(app.editor.body_text().contains('\n'));
        app.editor.set_body_text("{oops");
        app.update(Action::FormatBody);
        assert_eq!(app.editor.body_text(), "{oops", "invalid body untouched");
        app.editor.set_body_text("{ \"a\": 1 }");
        app.update(Action::MinifyBody);
        assert_eq!(app.editor.body_text(), "{\"a\":1}");
    }

    #[test]
    fn format_body_on_invalid_json_toasts_position_and_empty_body_is_a_noop() {
        // Animations off: the toast's slide-in would otherwise still be
        // mid-flight (offset off-screen) at the instant this test draws it,
        // one line after the push that starts it.
        let mut app = App::new_for_test_with_anims(false);
        app.editor.set_body_text("{\n  \"a\": oops\n}");
        assert!(app.update(Action::FormatBody));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                app.toasts
                    .draw(f, f.area(), &theme, &app.anims, std::time::Instant::now())
            })
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("line 2"),
            "toast must carry the error position: {content}"
        );

        let mut app = App::new_for_test();
        assert!(
            app.update(Action::MinifyBody),
            "empty body is a no-op, not an error"
        );
        assert_eq!(app.editor.body_text(), "");
        assert!(app.toasts.is_empty(), "no toast for an empty body");
    }

    #[test]
    fn open_body_in_editor_defers_to_the_main_loop() {
        let mut app = App::new_for_test();
        assert!(app.update(Action::OpenBodyInEditor));
        assert_eq!(
            app.pending_terminal_action.take(),
            Some(Action::OpenBodyInEditor),
            "App::update must not touch the terminal itself"
        );
    }

    /// Draws `e` at 120x14 (wide enough for every toolbar chip, Body tab
    /// included) and returns (buffer content, hits) — shared by the
    /// toolbar tests below.
    fn draw_editor(e: &mut Editor) -> (String, crate::hit::HitMap) {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    #[test]
    fn body_tab_label_paints_disabled_for_get_and_normal_for_post() {
        let draw = |e: &mut Editor| {
            let theme = Theme::dark();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(120, 14);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            let body_rect = hits
                .rect_of(&crate::hit::Hit::EditorTab(EditorTab::Body.draw_position()))
                .expect("body tab hit");
            terminal.backend().buffer()[(body_rect.x + 1, body_rect.y)].fg
        };
        let theme = Theme::dark();
        let mut e = Editor::default();
        assert_eq!(e.method, postui_core::model::Method::Get);
        assert_eq!(
            draw(&mut e),
            theme.text_disabled,
            "GET: Body label in the disabled color"
        );
        e.method = postui_core::model::Method::Post;
        assert_eq!(
            draw(&mut e),
            theme.text_muted,
            "POST: Body label back to the normal inactive color"
        );
    }

    #[test]
    fn save_vars_and_discard_no_longer_sit_on_the_tab_label_row() {
        // They moved to the footer: save/discard to its global right-side
        // group, vars to the editor's context chips. Only the collapse
        // toggle keeps the tab-label row's right edge.
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap(),
        );
        e.url = LineInput::new("https://x/changed");
        assert!(e.is_dirty());
        let (_, hits) = draw_editor(&mut e);
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::SaveRequest))
                .is_none()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::ConfirmDiscardChanges))
                .is_none()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenVarPicker {
                completing: false,
            }))
            .is_none()
        );
        assert!(
            hits.rect_of(&Hit::TableCollapse).is_some(),
            "the collapse toggle stays"
        );
    }

    /// Hiding hides the controls too: with the table collapsed the tab
    /// labels fade out entirely (settled: not painted at all) and take no
    /// clicks — only the `› show` toggle keeps the row.
    #[test]
    fn hidden_tab_labels_fade_out_and_take_no_clicks() {
        let mut e = Editor {
            table_collapsed: true,
            ..Editor::default()
        };
        let (out, hits) = draw_editor(&mut e);
        assert!(!out.contains("Params"), "tab labels are invisible: {out}");
        assert!(
            hits.rect_of(&Hit::EditorTab(0)).is_none(),
            "hidden tabs take no clicks"
        );
        assert!(
            hits.rect_of(&Hit::TableCollapse).is_some(),
            "the show toggle stays"
        );
        assert!(out.contains("show"), "{out}");
    }

    /// Hiding puts away the tab content, not the request's controls: the
    /// address bar (method / URL / Send) stays fully usable while hidden.
    #[test]
    fn hidden_editor_keeps_its_address_bar() {
        let mut e = Editor {
            table_collapsed: true,
            ..Editor::default()
        };
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str("url = \"https://x/path\"\n").unwrap(),
        );
        e.table_collapsed = true;
        let (out, hits) = draw_editor(&mut e);
        assert!(out.contains("https://x/path"), "{out}");
        assert!(hits.rect_of(&Hit::UrlBar).is_some(), "URL stays editable");
        assert!(hits.rect_of(&Hit::SendButton).is_some());
        assert!(hits.rect_of(&Hit::MethodSelector).is_some());
        assert!(hits.rect_of(&Hit::TableCollapse).is_some());
    }

    fn draw_editor_sized(e: &mut Editor, w: u16, h: u16) -> crate::hit::HitMap {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        hits
    }

    /// At exactly its collapsed height the address bar still gets its full
    /// rows — an over-constrained split shortchanges the *first*
    /// constraint, shearing the bar's caps into its text row. The `› show`
    /// row sits directly below the bar (its bottom breathing margin
    /// included — kept by user choice).
    #[test]
    fn hidden_editor_at_collapsed_height_keeps_the_address_bar_intact() {
        let expanded_url = {
            let mut e = Editor::default();
            let (_, hits) = draw_editor(&mut e);
            hits.rect_of(&Hit::UrlBar).unwrap()
        };

        let mut e = Editor {
            table_collapsed: true,
            ..Editor::default()
        };
        let hits = draw_editor_sized(&mut e, 120, COLLAPSED_HEIGHT);
        let url = hits.rect_of(&Hit::UrlBar).expect("URL well drawn");
        assert_eq!(
            (url.y, url.height),
            (expanded_url.y, expanded_url.height),
            "address bar keeps its exact expanded geometry while hidden"
        );
        let toggle = hits
            .rect_of(&Hit::TableCollapse)
            .expect("the show toggle fits");
        assert_eq!(toggle.y, ADDRESS_BAR_HEIGHT, "toggle on the strip row");
    }

    /// The editor's hide/show toggle right-aligns with the same inset the
    /// Response pane's toggle uses (2 cols in from the pane edge: 1 for
    /// `pane_surface`, 1 for the toggle's own margin) — expanded and
    /// hidden alike, so the two panes' toggles line up on screen.
    #[test]
    fn collapse_toggle_aligns_with_the_response_panes_inset() {
        let mut e = Editor::default();
        let (_, hits) = draw_editor(&mut e);
        let expanded = hits.rect_of(&Hit::TableCollapse).unwrap();
        assert_eq!(expanded.x + expanded.width, 120 - 2, "{expanded:?}");

        let mut e = Editor {
            table_collapsed: true,
            ..Editor::default()
        };
        let hits = draw_editor_sized(&mut e, 120, COLLAPSED_HEIGHT);
        let hidden = hits.rect_of(&Hit::TableCollapse).unwrap();
        assert_eq!(hidden.x + hidden.width, 120 - 2, "{hidden:?}");
    }

    /// Un-hiding eases the pane taller while the expanded constraints are
    /// already in force: at every intermediate height the address bar must
    /// keep its exact geometry — the tab bar and toolbar give way instead
    /// (otherwise the whole bar shears up for the animation, then jumps
    /// back).
    #[test]
    fn address_bar_geometry_is_stable_at_every_mid_animation_height() {
        let expanded_url = {
            let mut e = Editor::default();
            let (_, hits) = draw_editor(&mut e);
            hits.rect_of(&Hit::UrlBar).unwrap()
        };
        for h in COLLAPSED_HEIGHT..=CHROME_HEIGHT + 2 {
            let mut e = Editor {
                active_tab: EditorTab::Body,
                ..Editor::default()
            };
            e.method = postui_core::model::Method::Post;
            let hits = draw_editor_sized(&mut e, 120, h);
            let url = hits.rect_of(&Hit::UrlBar).expect("URL well drawn");
            assert_eq!(
                (url.y, url.height),
                (expanded_url.y, expanded_url.height),
                "address bar sheared at height {h}"
            );
        }
    }

    #[test]
    fn the_toolbar_row_exists_only_on_the_body_tab() {
        let mut e = Editor {
            active_tab: EditorTab::Params,
            ..Editor::default()
        };
        let (_, hits) = draw_editor(&mut e);
        // With no toolbar row outside Body, the params table starts directly
        // under the tab bar: its NAME/VALUE header row first, then row 0.
        let row = hits
            .rect_of(&Hit::TableRow(0))
            .expect("the params table's first row must be a registered hit");
        assert_eq!(row.y, ADDRESS_BAR_HEIGHT + TAB_BAR_HEIGHT + 1);
    }

    #[test]
    fn scratch_dirty_is_typed_content_with_no_saved_snapshot() {
        let mut e = Editor::default();
        assert!(
            !e.is_scratch_dirty(),
            "a blank editor holds nothing to lose"
        );
        e.url = LineInput::new("https://x");
        assert!(e.is_scratch_dirty(), "typed content with no file behind it");
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap(),
        );
        assert!(
            !e.is_scratch_dirty(),
            "a loaded request is `is_dirty`'s territory, never this"
        );
    }

    #[test]
    fn body_only_toolbar_chips_are_absent_on_the_params_tab() {
        let mut e = Editor {
            active_tab: EditorTab::Params,
            ..Editor::default()
        };
        let (_, hits) = draw_editor(&mut e);
        assert!(hits.rect_of(&Hit::FooterChip(Action::FormatBody)).is_none());
        assert!(hits.rect_of(&Hit::FooterChip(Action::MinifyBody)).is_none());
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::ToggleBodyVars))
                .is_none()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenBodyInEditor))
                .is_none()
        );
    }

    #[test]
    fn body_only_toolbar_chips_appear_on_the_body_tab() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            ..Editor::default()
        };
        let (content, hits) = draw_editor(&mut e);
        assert!(content.contains("format"), "{content}");
        assert!(content.contains("minify"), "{content}");
        assert!(content.contains("$EDITOR"), "{content}");
        assert!(hits.rect_of(&Hit::FooterChip(Action::FormatBody)).is_some());
        assert!(hits.rect_of(&Hit::FooterChip(Action::MinifyBody)).is_some());
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::ToggleBodyVars))
                .is_some()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenBodyInEditor))
                .is_some()
        );
        assert!(
            !content.contains("select all"),
            "the select-all chip is gone (ctrl+a covers it): {content}"
        );
        assert!(content.contains("alt+e"), "{content}");
        assert!(content.contains("clear"), "{content}");
        assert!(hits.rect_of(&Hit::FooterChip(Action::BodyClear)).is_some());
    }

    #[test]
    fn substitute_chip_label_reflects_substitute_body_state() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            substitute_body: false,
            ..Editor::default()
        };
        let (off, _) = draw_editor(&mut e);
        assert!(off.contains("{{off}}"), "{off}");

        e.substitute_body = true;
        let (on, _) = draw_editor(&mut e);
        assert!(on.contains("{{on}}"), "{on}");
    }

    #[test]
    fn chrome_height_excludes_the_body_only_toolbar_row() {
        // The toolbar row only exists on the Body tab, and a Body-tab editor
        // never collapses to chrome — so collapsed chrome is address bar +
        // tab bar alone.
        assert_eq!(CHROME_HEIGHT, ADDRESS_BAR_HEIGHT + TAB_BAR_HEIGHT);
    }

    #[test]
    fn body_tab_label_shows_json_validity() {
        let render = |method: postui_core::model::Method, text: &str| {
            let mut e = Editor {
                active_tab: EditorTab::Body,
                method,
                ..Editor::default()
            };
            e.set_body_text(text);
            let theme = Theme::dark();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(60, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            format!("{:?}", terminal.backend().buffer())
        };
        use postui_core::model::Method;
        assert!(render(Method::Post, "{\"a\": 1}").contains("Body ✓"));
        assert!(render(Method::Post, "{oops").contains("Body ✗"));
        // No badge at all on an empty body — there's nothing to validate.
        let empty = render(Method::Post, "");
        assert!(
            !empty.contains('✓') && !empty.contains('✗'),
            "empty body shows no validity badge: {empty}"
        );
        // No badge while the tab is disabled (GET/HEAD send no body): the
        // bright validity glyph is what made a disabled tab read as lit.
        let disabled = render(Method::Get, "{\"a\": 1}");
        assert!(
            !disabled.contains('✓') && !disabled.contains('✗'),
            "disabled Body tab shows no validity badge: {disabled}"
        );
    }

    /// `tab_strip_spans` must agree with `draw_tab_bar` on badge presence
    /// (it affects the Body tab's width): no badge on an empty body, one
    /// once the body has text and the method sends it.
    #[test]
    fn tab_strip_spans_track_badge_presence() {
        let mut e = Editor {
            method: postui_core::model::Method::Post,
            ..Editor::default()
        };
        let without = e.tab_strip_spans()[EditorTab::Body.draw_position()].1;
        e.set_body_text("{}");
        let with = e.tab_strip_spans()[EditorTab::Body.draw_position()].1;
        assert_eq!(with, without + 2, "badge adds its 2-cell span");
        e.method = postui_core::model::Method::Get;
        let disabled = e.tab_strip_spans()[EditorTab::Body.draw_position()].1;
        assert_eq!(disabled, without, "disabled tab drops the badge");
    }

    #[test]
    fn param_and_header_counts_render_inside_their_tabs_not_at_the_far_right() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(
                r#"url = "https://x"
[params]
page = "2"
q = "cats"

[headers]
x-a = "1"
"#,
            )
            .unwrap(),
        );
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("Params · 2"),
            "param count lives inside its tab: {content}"
        );
        assert!(
            content.contains("Headers · 1"),
            "header count lives inside its tab: {content}"
        );

        // No standalone count chip left of the collapse toggle any more:
        // the cell two columns left of the `⌄ hide` toggle (where the chip
        // used to end) must be plain page background.
        let toggle = hits.rect_of(&crate::hit::Hit::TableCollapse).unwrap();
        let buf = terminal.backend().buffer();
        let cell = buf.cell((toggle.x - 2, toggle.y)).unwrap();
        assert_eq!(
            cell.bg, theme.page,
            "no count chip at the strip's right edge: {cell:?}"
        );
    }

    #[test]
    fn body_tab_renders_its_text() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            ..Editor::default()
        };
        e.set_body_text("{\"marker\": 1}");
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        // A couple of rows taller than the other 60x10 body-tab tests: the
        // body content area now reserves a 1-cell gutter on every side for
        // the focus ring (Task 12), so the old 10-row terminal left the
        // body exactly zero rows once the chrome above it (address bar, tab
        // bar, toolbar) was accounted for.
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("marker"),
            "body text must render: {content}"
        );
    }

    /// The body content's accent ring (Task 12) shows exactly when
    /// `SubFocus::Content` — no ring on any other sub-focus (or with the
    /// pane itself unfocused), and once `focus_t` has settled (`test_anims`
    /// is a disabled/instant `Anims`), a full-strength `theme.focus_ring`
    /// ring stroke. Checks the exact gutter cell just left of the recorded
    /// `last_body_area` (its left edge stroke, `│`) rather than scanning
    /// the whole buffer for a ring glyph.
    #[test]
    fn body_ring_shows_exactly_when_content_focused() {
        let theme = Theme::dark();
        let render = |sub_focus: SubFocus, pane_focused: bool| -> (Terminal<TestBackend>, Rect) {
            let mut e = Editor {
                active_tab: EditorTab::Body,
                sub_focus,
                ..Editor::default()
            };
            e.set_body_text("{}");
            let ctx = DrawCtx {
                theme: &theme,
                focused: pane_focused,
                hovered: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(60, 12);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            let body_area = e.last_body_area.expect("body area recorded");
            (terminal, body_area)
        };
        let left_edge_cell = |terminal: &Terminal<TestBackend>, body_area: Rect| {
            terminal
                .backend()
                .buffer()
                .cell((body_area.x - 1, body_area.y + 1))
                .unwrap()
                .clone()
        };

        let (terminal, body_area) = render(SubFocus::Content, true);
        let cell = left_edge_cell(&terminal, body_area);
        assert_eq!(cell.symbol(), "│", "content focused: ring shows");
        assert_eq!(cell.fg, theme.focus_ring);

        let (terminal, body_area) = render(SubFocus::Content, false);
        assert_ne!(
            left_edge_cell(&terminal, body_area).symbol(),
            "│",
            "pane not focused: no ring even on Content sub-focus"
        );

        let (terminal, body_area) = render(SubFocus::Url, true);
        assert_ne!(
            left_edge_cell(&terminal, body_area).symbol(),
            "│",
            "url sub-focus: no ring"
        );

        let (terminal, body_area) = render(SubFocus::None, true);
        assert_ne!(
            left_edge_cell(&terminal, body_area).symbol(),
            "│",
            "no sub-focus: no ring"
        );
    }

    #[test]
    fn draw_shows_method_badge_url_and_tab_bar() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(
                r#"method = "POST"
url = "https://api.example.com/users""#,
            )
            .unwrap(),
        );
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        // Wide enough that the fused bar's URL segment (bar width minus the
        // fixed 10-wide method segment and 24-wide Send cap) still fits the
        // whole URL without windowed scrolling truncating it.
        let backend = TestBackend::new(90, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("POST"), "method badge: {content}");
        assert!(
            content.contains("https://api.example.com/users"),
            "url text: {content}"
        );
        assert!(content.contains("Params"), "params tab label: {content}");
        assert!(content.contains("Headers"), "headers tab label: {content}");
        assert!(content.contains("Body"), "body tab label: {content}");
    }

    /// Renders the editor into a fresh 60x10 buffer and returns the bar's
    /// outer rect (`Hit::MethodSelector`'s rect) plus the terminal for
    /// per-cell assertions.
    fn draw_for_bar_test(e: &mut Editor) -> (Terminal<TestBackend>, crate::hit::HitMap) {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        (terminal, hits)
    }

    #[test]
    fn method_focus_lifts_the_badge_to_hovers_color() {
        let mut e = Editor {
            sub_focus: SubFocus::Method,
            ..Editor::default()
        };
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let buf = terminal.backend().buffer();
        let method_face = theme.method_color(postui_core::model::Method::Get);
        let cell = buf.cell((method_area.x, method_area.y + 1)).unwrap();
        assert_eq!(
            cell.bg,
            crate::paint::face_edges(method_face, &theme).0,
            "the focused method badge lifts to hover's color, so a keyboard \
             user can see where Enter will land"
        );
        // No ring: the margin cell left of the badge stays plain page.
        let margin = buf
            .cell((method_area.x.saturating_sub(1), method_area.y + 1))
            .unwrap();
        assert_eq!(margin.symbol(), " ", "no ring glyph beside the badge");
    }

    #[test]
    fn tab_strip_focus_recolors_the_underline_to_the_focus_ring() {
        let mut e = Editor {
            sub_focus: SubFocus::Tabs,
            ..Editor::default()
        };
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let tab0 = hits.rect_of(&crate::hit::Hit::EditorTab(0)).unwrap();
        let buf = terminal.backend().buffer();
        let underline = buf.cell((tab0.x + 1, tab0.y + 1)).unwrap();
        assert_eq!(underline.symbol(), "━");
        assert_eq!(
            underline.fg, theme.focus_ring,
            "focused strip: the active tab's underline recolors to focus_ring"
        );
    }

    #[test]
    fn fused_bar_centers_text_between_shaded_bevel_edges() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x/y""#).unwrap(),
        );
        // This test asserts the resting bevel; URL focus (which `load`
        // grants) would lift the caps.
        e.sub_focus = SubFocus::None;
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        assert_eq!(method_area.height, 3, "method segment occupies 3 rows");

        let buf = terminal.backend().buffer();
        let text_y = method_area.y + 1;
        let method_face = theme.method_color(postui_core::model::Method::Get);
        let cell = buf.cell((method_area.x, text_y)).unwrap();
        assert_eq!(
            cell.bg, method_face,
            "method cell bg must be the GET method color"
        );
        // Thin bevel rows above and below, on the segment's own fill: light
        // "▔" on top, dark "▁" below, so the bar reads as a 3-row solid
        // with a raised edge (the anatomy `Button`/`TextField` use).
        let (m_light, m_dark) = crate::paint::face_edges(method_face, &theme);
        let top_cap = buf.cell((method_area.x, method_area.y)).unwrap();
        assert_eq!(top_cap.symbol(), "▔", "method top cap: {top_cap:?}");
        assert_eq!(top_cap.fg, m_light);
        assert_eq!(
            top_cap.bg, method_face,
            "cap sits on the segment's own fill"
        );
        let bottom_cap = buf.cell((method_area.x, text_y + 1)).unwrap();
        assert_eq!(
            bottom_cap.symbol(),
            "▁",
            "method bottom cap: {bottom_cap:?}"
        );
        assert_eq!(bottom_cap.fg, m_dark);
        let url_cap = buf
            .cell((method_area.x + method_area.width + 2, text_y + 1))
            .unwrap();
        assert_eq!(url_cap.symbol(), "▁", "url cap row: {url_cap:?}");
        assert_eq!(url_cap.fg, theme.edge_dark);

        // The URL text is drawn on the same row as the method label -- the
        // bar is one fused control, not stacked rows.
        let content = format!("{buf:?}");
        assert!(content.contains("https://x/y"), "url text: {content}");
        let url_row: String = (0..60)
            .filter_map(|x| buf.cell((x, text_y)).map(|c| c.symbol()))
            .collect();
        assert!(
            url_row.contains("https://x/y"),
            "url text must share the method label's row: {url_row}"
        );
        // The URL text is inset from the method segment, not flush against it.
        let gap: String = (0..2)
            .filter_map(|dx| {
                buf.cell((method_area.x + method_area.width + dx, text_y))
                    .map(|c| c.symbol())
            })
            .collect();
        assert_eq!(gap, "  ", "2 columns of left padding before the URL text");
    }

    /// Draws the address bar with an explicit `anims`/`now` (rather than
    /// `test_anims()`'s permanently-settled clock), so a fade can be
    /// sampled mid-flight.
    fn draw_for_bar_test_at(
        e: &mut Editor,
        hovered: Option<&crate::hit::Hit>,
        anims: &crate::anim::Anims,
        now: std::time::Instant,
    ) -> (Terminal<TestBackend>, crate::hit::HitMap) {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered,
            dragging: false,
            anims,
            now,
        };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        (terminal, hits)
    }

    #[test]
    fn hovering_the_method_badge_blends_in_the_hover_fill_over_the_fade() {
        use crate::anim::AnimKey;
        use std::time::Duration;

        let mut e = Editor::default();
        let theme = Theme::dark();
        let method_face = theme.method_color(postui_core::model::Method::Get);
        let method_lifted = crate::theme::lift_color(method_face, 0.12);

        // Mid-flight: half-eased toward the hover fill, not there yet.
        let mut anims = crate::anim::Anims::new(true);
        let t0 = std::time::Instant::now();
        anims.snap(AnimKey::Hover, 0.0);
        anims.retarget(AnimKey::Hover, 1.0, Duration::from_millis(70), t0);
        let mid = t0 + Duration::from_millis(35);
        let (terminal, hits) =
            draw_for_bar_test_at(&mut e, Some(&crate::hit::Hit::MethodSelector), &anims, mid);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let cell = terminal
            .backend()
            .buffer()
            .cell((method_area.x, method_area.y + 1))
            .unwrap();
        assert_ne!(
            cell.bg, method_face,
            "must have eased away from the resting face"
        );
        assert_ne!(
            cell.bg, method_lifted,
            "must not have reached the fully hovered fill yet"
        );

        // Settled: the fade has finished, so the fill reaches the target
        // exactly — the same value hover always painted before the fade
        // existed.
        let done = t0 + Duration::from_millis(70);
        let (terminal, hits) =
            draw_for_bar_test_at(&mut e, Some(&crate::hit::Hit::MethodSelector), &anims, done);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let cell = terminal
            .backend()
            .buffer()
            .cell((method_area.x, method_area.y + 1))
            .unwrap();
        assert_eq!(
            cell.bg, method_lifted,
            "settles on the same hover fill as before"
        );
    }

    #[test]
    fn focusing_the_url_well_blends_in_the_focus_lift_over_the_fade() {
        use crate::anim::AnimKey;
        use std::time::Duration;

        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
        e.sub_focus = SubFocus::Url;
        let theme = Theme::dark();
        let focused_fill = crate::theme::lift_color(theme.control, 0.12);

        let mut anims = crate::anim::Anims::new(true);
        let t0 = std::time::Instant::now();
        anims.snap(AnimKey::FocusFade, 0.0);
        anims.retarget(AnimKey::FocusFade, 1.0, Duration::from_millis(90), t0);
        let mid = t0 + Duration::from_millis(45);
        let (terminal, hits) = draw_for_bar_test_at(&mut e, None, &anims, mid);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let url_x = method_area.x + method_area.width;
        let text_y = method_area.y + 1;
        let well = terminal.backend().buffer().cell((url_x, text_y)).unwrap();
        assert_ne!(
            well.bg, theme.control,
            "must have eased away from the resting fill"
        );
        assert_ne!(
            well.bg, focused_fill,
            "must not have reached the fully focused fill yet"
        );

        let done = t0 + Duration::from_millis(90);
        let (terminal, hits) = draw_for_bar_test_at(&mut e, None, &anims, done);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let url_x = method_area.x + method_area.width;
        let text_y = method_area.y + 1;
        let well = terminal.backend().buffer().cell((url_x, text_y)).unwrap();
        assert_eq!(
            well.bg, focused_fill,
            "settles on the same focus lift as before"
        );
    }

    #[test]
    fn send_cap_is_bold_on_accent_when_enabled() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let send_area = hits.rect_of(&crate::hit::Hit::SendButton).unwrap();
        let buf = terminal.backend().buffer();
        let mid_y = send_area.y + 1;
        // Find the "Send" label's cell by scanning the send cap's row.
        let found = (send_area.x..send_area.x + send_area.width).any(|x| {
            let cell = buf.cell((x, mid_y)).unwrap();
            cell.symbol() == "S" && cell.fg == theme.on_accent && cell.bg == theme.accent
        });
        assert!(found, "Send label must be bold on_accent over accent");
        let bold_found = (send_area.x..send_area.x + send_area.width).any(|x| {
            let cell = buf.cell((x, mid_y)).unwrap();
            cell.symbol() == "S" && cell.modifier.contains(Modifier::BOLD)
        });
        assert!(bold_found, "Send label must be bold");
    }

    /// In flight is a distinct state from disabled (mouse-first ruling):
    /// the Send hit must stay registered while sending so a click can still
    /// cancel it (`App`'s `Hit::SendButton` handler routes to
    /// `Action::CancelSend` when the open request is in flight).
    #[test]
    fn sending_shows_spinner_glyph_and_keeps_send_hit_registered() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
        e.sending = true;
        let (terminal, hits) = draw_for_bar_test(&mut e);
        assert!(
            hits.rect_of(&crate::hit::Hit::SendButton).is_some(),
            "Send hit must stay registered while sending, so a click can cancel"
        );
        let content = format!("{:?}", terminal.backend().buffer());
        let spinner_glyphs = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        assert!(
            spinner_glyphs.iter().any(|g| content.contains(*g)),
            "sending must show a spinner glyph: {content}"
        );
        assert!(
            content.contains("Sending"),
            "sending label must read Sending: {content}"
        );
    }

    /// Hovering the Send cap while a request is in flight swaps its label to
    /// "Cancel" so the click-to-cancel affordance is discoverable, without
    /// disturbing the pulse/spinner face logic.
    #[test]
    fn hovering_send_while_sending_shows_cancel_label() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
        e.sending = true;
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: Some(&crate::hit::Hit::SendButton),
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("Cancel"),
            "hovering a send in flight must read Cancel: {content}"
        );
        assert!(
            hits.rect_of(&crate::hit::Hit::SendButton).is_some(),
            "hit stays registered while hovered+sending"
        );
    }

    #[test]
    fn send_cap_disabled_when_url_empty() {
        let mut e = Editor::default();
        // Fresh scratch editor: url is empty by default.
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        assert!(
            hits.rect_of(&crate::hit::Hit::SendButton).is_none(),
            "Send hit must be unregistered when the URL is empty"
        );
        let buf = terminal.backend().buffer();
        // Find the "S" of "Send" and assert it is not bold when disabled.
        let found_non_bold = (0..60).any(|x| {
            (0..10).any(|y| {
                let cell = buf.cell((x, y)).unwrap();
                cell.symbol() == "S"
                    && cell.fg == theme.text_disabled
                    && !cell.modifier.contains(Modifier::BOLD)
            })
        });
        assert!(found_non_bold, "disabled Send label must not be bold");
    }

    #[test]
    fn focused_url_lifts_the_url_fill_and_caps() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
        e.sub_focus = SubFocus::Url;
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let buf = terminal.backend().buffer();
        let text_y = method_area.y + 1;
        let url_x = method_area.x + method_area.width;
        let lifted = crate::theme::lift_color(theme.control, 0.12);
        let well = buf.cell((url_x, text_y)).unwrap();
        assert_eq!(
            well.bg, lifted,
            "the focused URL well brightens past control_hover: {well:?}"
        );
        assert_ne!(lifted, theme.control_hover, "focus must outshine hover");
        // The bevel follows the lifted fill, at the softer ±0.08 delta
        // `TextField`'s Focused state uses around its own fill (not the
        // method badge's ±0.12) — the stronger delta reads as a hard line
        // on the neutral control fill.
        let cap = buf.cell((url_x, method_area.y)).unwrap();
        assert_eq!(cap.symbol(), "▔", "url top cap: {cap:?}");
        assert_eq!(cap.fg, crate::theme::lift_color(lifted, 0.08));
        // No ring: the bar's old top-left ring corner cell stays plain.
        let corner = buf
            .cell((
                method_area.x.saturating_sub(1),
                method_area.y.saturating_sub(1),
            ))
            .unwrap();
        assert_eq!(corner.symbol(), " ", "no ring glyph around the bar");
    }

    #[test]
    fn headers_tab_shows_inherited_rows_with_status() {
        let mut e = Editor {
            active_tab: EditorTab::Headers,
            ..Editor::default()
        };
        e.inherited_headers.insert(
            "accept".into(),
            Entry {
                value: "application/json".into(),
                enabled: true,
            },
        );
        e.inherited_headers.insert(
            "x-a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        e.inherited_headers.insert(
            "x-b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        e.headers.insert(
            "X-A".into(),
            Entry {
                value: "9".into(),
                enabled: true,
            },
        );
        e.headers.insert(
            "X-B".into(),
            Entry {
                value: "n".into(),
                enabled: false,
            },
        );
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(70, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("(project)"), "{content}");
        assert!(content.contains("(overridden)"), "{content}");
        assert!(content.contains("(disabled)"), "{content}");
        assert!(content.contains("application/json"));
    }

    /// Regression test for a bug where the synthesized scroll events landed
    /// on the line-numbers gutter (always on, `LineNumbers::Absolute`)
    /// rather than inside edtui's own recorded content area, so
    /// `MouseEventHandler`'s bounds check silently dropped every event and
    /// the wheel over an unfocused Body tab did nothing at all.
    #[test]
    fn wheel_scroll_over_unfocused_body_tab_actually_scrolls() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Body;
        let many_lines: String = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.editor.set_body_text(&many_lines);
        assert_eq!(app.editor.body.cursor.row, 0, "starts at the top");

        // Render once for real so the view records its (gutter-inclusive)
        // area, exactly as `EditorView::render` does on every draw.
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert!(app.editor.last_body_area.is_some(), "body area recorded");

        // A large downward scroll, well past the visible page, should move
        // the viewport (and, via edtui's own clamp, the cursor) down from
        // row 0 -- not a "no panic" no-op.
        assert!(app.update(Action::ScrollPane(crate::layout::PaneId::Editor, 100)));
        assert!(
            app.editor.body.cursor.row > 0,
            "scrolling down a long body must move the cursor off row 0, got {}",
            app.editor.body.cursor.row
        );
    }

    /// Regression test for the controller sweep's Paint Gap B report: with
    /// an empty params table (the default scratch editor), the fixed-height
    /// address bar + tab bar + tiny table content rows don't sum to the
    /// pane's full height, and the leftover space below them must still
    /// read as `theme.page` — painted once, up front, by `pane_surface`
    /// filling the whole pane rect before the layout split — not the
    /// terminal's default background.
    #[test]
    fn empty_params_table_still_paints_page_below_its_short_content() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Params;
        assert!(
            app.editor.params.is_empty(),
            "fresh scratch editor: no rows"
        );

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        let layout =
            crate::layout::compute_layout(ratatui::layout::Rect::new(0, 0, 120, 40), 0.0, 0.0);
        let buf = terminal.backend().buffer();
        // Deep into the pane's lower region, well past the address bar, tab
        // bar, and the empty table's few header/ghost/edge rows.
        let probe_y = layout.editor.y + layout.editor.height - 1;
        let cell = buf.cell((layout.editor.x + 2, probe_y)).unwrap();
        assert_eq!(
            cell.bg, app.theme.page,
            "editor pane's lower region must be page-filled, not left at the terminal default: {cell:?}"
        );
    }

    #[test]
    fn vars_tab_reuses_the_table_editor_with_a_count_in_its_label() {
        let mut e = Editor {
            active_tab: EditorTab::Vars,
            ..Editor::default()
        };
        e.variables.insert(
            "token".into(),
            Entry {
                value: "abc".into(),
                enabled: true,
            },
        );
        e.variables.insert(
            "region".into(),
            Entry {
                value: "us-east".into(),
                enabled: true,
            },
        );
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("Vars · 2"),
            "count in tab label: {content}"
        );
        assert!(content.contains("token"), "key text: {content}");
        assert!(content.contains("us-east"), "value text: {content}");
        assert!(
            hits.rect_of(&crate::hit::Hit::TableCell {
                row: e.variables.len(),
                col: 0
            })
            .is_some(),
            "the ghost row's cells must be registered on the Vars tab"
        );
    }

    #[test]
    fn vars_tab_lists_the_variables_the_request_references() {
        let mut e = Editor {
            active_tab: EditorTab::Vars,
            ..Editor::default()
        };
        e.url = LineInput::new("https://{{base}}/v1");
        e.headers.insert(
            "auth".into(),
            Entry {
                value: "Bearer {{token}}".into(),
                enabled: true,
            },
        );
        e.headers.insert(
            "x-off".into(),
            Entry {
                value: "{{ignored}}".into(),
                enabled: false,
            },
        );
        e.set_body_text("{\"k\": \"{{body_var}}\"}");
        // substitute_body stays false: body tokens are sent verbatim, so
        // the body's token is not "referenced" by the send.
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("referenced"), "section divider: {content}");
        assert!(content.contains("{{base}}"), "url token listed: {content}");
        assert!(
            content.contains("{{token}}"),
            "header token listed: {content}"
        );
        assert!(
            content.contains("not defined"),
            "an unresolvable token says so: {content}"
        );
        assert!(
            !content.contains("ignored"),
            "disabled rows are not scanned: {content}"
        );
        assert!(
            !content.contains("body_var"),
            "body tokens only count with substitute on: {content}"
        );

        // Turn substitution on: the body's token joins the list.
        e.substitute_body = true;
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("body_var"), "{content}");
    }

    #[test]
    fn referenced_vars_render_as_aligned_table_columns() {
        // Tokens of different lengths land in the table's own name column,
        // and their values all start at the table's value column.
        let mut e = Editor {
            active_tab: EditorTab::Vars,
            ..Editor::default()
        };
        e.url = LineInput::new("https://{{b}}/{{longer_name}}");
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        let row_text = |y: u16| -> String {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect()
        };
        // `str::find` returns a byte offset; rows can hold multi-byte
        // glyphs (the header's `│` divider), so convert to a column.
        let col_of = |line: &str, needle: &str| -> Option<usize> {
            line.find(needle).map(|b| line[..b].chars().count())
        };
        let divider_y = (0..buf.area.height)
            .find(|y| row_text(*y).contains("referenced"))
            .expect("section divider");
        let mut token_cols = Vec::new();
        let mut value_cols = Vec::new();
        for y in divider_y + 1..buf.area.height {
            let line = row_text(y);
            if line.contains("{{b}}") || line.contains("{{longer_name}}") {
                token_cols.push(col_of(&line, "{{").unwrap());
                value_cols.push(col_of(&line, "\u{2014}").expect("undefined value dash"));
            }
        }
        assert_eq!(token_cols.len(), 2, "both referenced rows drawn");
        assert_eq!(token_cols[0], token_cols[1], "names share a column");
        assert_eq!(value_cols[0], value_cols[1], "values share a column");
        // And the value column is the table's own: the header row's VALUE
        // label sits at the same x.
        let header = (0..buf.area.height)
            .map(row_text)
            .find(|l| l.contains("VALUE"))
            .expect("table header row");
        assert_eq!(
            col_of(&header, "VALUE").unwrap(),
            value_cols[0],
            "referenced values align with the table's VALUE column"
        );
    }

    #[test]
    fn vars_tab_shows_no_referenced_section_without_tokens() {
        let mut e = Editor {
            active_tab: EditorTab::Vars,
            ..Editor::default()
        };
        e.url = LineInput::new("https://plain.example/v1");
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            !content.contains("referenced"),
            "no tokens, no section: {content}"
        );
    }

    #[test]
    fn vars_tab_add_expand_and_delete_confirm_all_function() {
        // Reuses the Params tab's own table-editor patterns
        // (add_edit_commit_creates_entry / descending_from_the_tab_strip_…)
        // pointed at the Vars tab, proving the shared component works
        // unmodified there.
        let mut e = Editor {
            active_tab: EditorTab::Vars,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        // Ghost "+ Add" -> type a new row.
        e.handle_key(key(KeyCode::Char('a')));
        for c in "token".chars() {
            e.handle_key(key(KeyCode::Char(c)));
        }
        e.handle_key(key(KeyCode::Tab));
        for c in "abc".chars() {
            e.handle_key(key(KeyCode::Char(c)));
        }
        e.handle_key(key(KeyCode::Enter));
        assert_eq!(e.variables.len(), 1);
        assert_eq!(e.variables["token"].value, "abc");

        // Selecting the row expands it (feeds table_geometry / draws 3
        // lines) exactly like Params/Headers.
        e.table.selected = Some(0);
        let (rows, active, hint) = e.table_geometry();
        assert_eq!(rows, 1);
        assert_eq!(active, Some(0));
        assert!(!hint, "no shadow hint without a shadowed project var");

        // `d` requests a delete-confirm, same as Params/Headers.
        let action = e.handle_key(key(KeyCode::Char('d')));
        assert_eq!(action, Some(Action::ConfirmDeleteTableRow(0)));
    }

    #[test]
    fn vars_tab_shows_dim_overrides_hint_on_a_shadowing_row() {
        let mut e = Editor {
            active_tab: EditorTab::Vars,
            ..Editor::default()
        };
        e.variables.insert(
            "token".into(),
            Entry {
                value: "override".into(),
                enabled: true,
            },
        );
        e.shadowed.insert("token".into(), "qa: 1001".into());
        e.table.selected = Some(0);
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("overrides qa: 1001"),
            "expected the dim shadow hint: {content}"
        );
        let buf = terminal.backend().buffer();
        let row = hits.rect_of(&crate::hit::Hit::TableRow(0)).unwrap();
        assert_eq!(row.height, 4, "hint adds one extra row to the expansion");
        let hint_cell = buf.cell((row.x + 2, row.y + 2)).unwrap();
        assert_eq!(
            hint_cell.fg, theme.text_muted,
            "the overrides hint is dim, not full text color"
        );
    }

    #[test]
    fn params_and_headers_tabs_never_show_a_shadow_hint() {
        // `shadow` is only ever passed for Vars; Params/Headers keep their
        // existing 3-line expansion regardless of `Editor::shadowed`.
        let mut e = Editor {
            active_tab: EditorTab::Params,
            ..Editor::default()
        };
        e.params.insert(
            "page".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        e.shadowed.insert("page".into(), "qa: 1001".into());
        e.table.selected = Some(0);
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let row = hits.rect_of(&crate::hit::Hit::TableRow(0)).unwrap();
        assert_eq!(row.height, 3, "Params tab never grows a hint row");
    }

    #[test]
    fn tab_order_headers_first_and_alt_matches_screen() {
        // Draw order / EditorTabCycle: Headers -> Params -> Vars -> Body.
        assert_eq!(EditorTab::from_draw_position(0), EditorTab::Headers);
        assert_eq!(EditorTab::from_draw_position(1), EditorTab::Params);
        assert_eq!(EditorTab::from_draw_position(2), EditorTab::Vars);
        assert_eq!(EditorTab::from_draw_position(3), EditorTab::Body);
        // alt+1/2/3/4 (`EditorTabSelect(0..3)`) follow the screen order:
        // what you see is what the number selects.
        for i in 0..4 {
            assert_eq!(EditorTab::from_index(i), EditorTab::from_draw_position(i));
            assert_eq!(
                EditorTab::from_draw_position(i).index(),
                EditorTab::from_draw_position(i).draw_position()
            );
        }
    }

    // --- computed request-headers section (Task 10, spec §6) ---------

    fn buffer_has_crossed_out(buf: &ratatui::buffer::Buffer, needle: &str) -> bool {
        for y in 0..buf.area.height {
            let line: String = (0..buf.area.width)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect();
            if line.contains(needle) {
                let start = line.find(needle).unwrap();
                // `find` is a byte offset into a `String` built one grapheme
                // cell at a time; every cell here is ASCII, so it doubles as
                // a column index.
                let x = buf.area.x + start as u16;
                if buf
                    .cell((x, y))
                    .unwrap()
                    .modifier
                    .contains(Modifier::CROSSED_OUT)
                {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn headers_tab_shows_overridden_default_as_struck_through_auto_row() {
        let mut app = App::new_for_test();
        app.project.meta.default_headers.insert(
            "Accept".into(),
            Entry {
                value: "application/json".into(),
                enabled: true,
            },
        );
        app.editor.active_tab = EditorTab::Headers;
        app.editor.headers.insert(
            "Accept".into(),
            Entry {
                value: "text/plain".into(),
                enabled: true,
            },
        );
        app.update(Action::Render);

        let backend = TestBackend::new(100, 70);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("text/plain"),
            "the editable override row is still the table above: {content}"
        );
        assert!(
            content.contains("auto"),
            "the dim divider introduces the computed section: {content}"
        );
        assert!(
            buffer_has_crossed_out(terminal.backend().buffer(), "application/json"),
            "the suppressed default's own value renders struck through"
        );
    }

    #[test]
    fn headers_tab_shows_auto_content_type_with_a_body() {
        let mut app = App::new_for_test();
        app.update(Action::SetMethod(postui_core::model::Method::Post));
        app.editor.active_tab = EditorTab::Headers;
        app.editor.set_body_text(r#"{"a":1}"#);
        app.update(Action::Render);

        let backend = TestBackend::new(100, 70);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Content-Type"), "{content}");
        assert!(content.contains("application/json"), "{content}");
    }

    #[test]
    fn headers_tab_shows_the_host_row() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/foo");
        app.update(Action::Render);

        let backend = TestBackend::new(100, 70);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Host"), "{content}");
        assert!(content.contains("example.com"), "{content}");
    }

    #[test]
    fn computed_headers_height_math_reserves_the_divider_and_auto_rows() {
        // table_height's own header/ghost/edge rows plus 1 divider + however
        // many auto rows must fit within what the pane hands the content
        // constraint, never overflowing it (draw would panic on an
        // out-of-bounds rect otherwise — this is a "does not panic and
        // draws the whole thing" check, not just a row count).
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/foo");
        app.update(Action::Render);

        let backend = TestBackend::new(100, 70);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        assert!(
            app.editor.computed_row_count() > 0,
            "a scratch request with a resolvable URL still gets a Host row"
        );
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Host"), "{content}");
    }
}

#[cfg(test)]
mod body_click_tests {
    use super::*;
    use edtui::Index2;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::widgets::Widget;

    /// The synthetic area the body editor is "rendered" into by the helper.
    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 10,
    };

    /// Builds a Body-tab editor with `text` in the buffer, then renders the
    /// body view once into a synthetic 40x10 area with exactly the same
    /// builder options the real `draw` uses, so edtui records its own
    /// (post-gutter) screen area and viewport just as it would on screen.
    fn editor_with_body(text: &str) -> Editor {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.set_body_text(text);
        render_body(&mut e);
        e
    }

    fn render_body(e: &mut Editor) {
        e.last_body_area = Some(AREA);
        let mut buf = Buffer::empty(AREA);
        EditorView::new(&mut e.body)
            .theme(EditorTheme::default().hide_status_line())
            .wrap(true)
            .line_numbers(LineNumbers::Absolute)
            .render(AREA, &mut buf);
    }

    fn left_down(x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn click_past_line_end_places_caret_at_line_end() {
        let mut e = editor_with_body("{\n  \"a\": 1,\n  \"bb\": 2\n}\n");
        e.handle_mouse(left_down(35, 1));
        // Line 1 is `  "a": 1,` - 9 chars. The caret belongs AFTER the
        // trailing comma (insert mode allows col == len), not on it.
        assert_eq!(e.body.cursor, Index2::new(1, 9));
    }

    #[test]
    fn click_below_last_line_goes_to_end_of_last_line() {
        // No trailing newline: the last row has text, so the "end of the
        // last line" is a column edtui's own clamp could not produce.
        let mut e = editor_with_body("{\n  \"a\": 1\n}");
        e.handle_mouse(left_down(5, 8));
        assert_eq!(e.body.cursor, Index2::new(2, 1));

        // With a trailing newline the last row is empty, and that is where a
        // click in the void below belongs.
        let mut e = editor_with_body("{\n  \"a\": 1\n}\n");
        e.handle_mouse(left_down(5, 8));
        assert_eq!(e.body.cursor, Index2::new(3, 0));
    }

    #[test]
    fn click_on_a_character_lands_on_that_character() {
        let mut e = editor_with_body("{\n  \"a\": 1,\n}\n");
        // The gutter is 2 cells wide (3 rows -> 1 digit + 1 space), so
        // content column 3 (the `a`) sits at screen x = 5.
        e.handle_mouse(left_down(5, 1));
        assert_eq!(e.body.cursor, Index2::new(1, 3));
    }

    #[test]
    fn click_at_content_column_zero_is_the_start_of_the_line() {
        let mut e = editor_with_body("{\n  \"a\": 1,\n}\n");
        e.handle_mouse(left_down(2, 1));
        assert_eq!(e.body.cursor, Index2::new(1, 0));
    }

    #[test]
    fn click_in_the_gutter_leaves_the_cursor_alone() {
        let mut e = editor_with_body("{\n  \"a\": 1,\n}\n");
        e.body.cursor = Index2::new(0, 1);
        e.handle_mouse(left_down(0, 1));
        assert_eq!(e.body.cursor, Index2::new(0, 1));
    }

    #[test]
    fn click_on_the_second_visual_row_of_a_wrapped_line() {
        // Content width is 40 - 2 (gutter) = 38, so a 50-char line wraps
        // after 38 chars and its second visual row is screen row 1.
        let long: String = std::iter::repeat_n('x', 50).collect();
        let mut e = editor_with_body(&format!("{long}\nend\n"));
        e.handle_mouse(left_down(2 + 5, 1));
        assert_eq!(e.body.cursor, Index2::new(0, 38 + 5));
        // Past the end of the wrapped line's tail: caret after the last char.
        e.handle_mouse(left_down(35, 1));
        assert_eq!(e.body.cursor, Index2::new(0, 50));
        // The next logical line starts on the third visual row.
        e.handle_mouse(left_down(2 + 1, 2));
        assert_eq!(e.body.cursor, Index2::new(1, 1));
    }

    #[test]
    fn drag_selects_from_the_click_anchor_and_stays_modeless() {
        let mut e = editor_with_body("hello\nworld\n");
        e.handle_mouse(left_down(2, 0)); // caret at (0,0), anchor planted
        assert!(e.body_drag_to(4, 0), "drag inside the body area consumed");
        assert_eq!(e.body.cursor, Index2::new(0, 2));
        assert_eq!(e.body_selected_text().as_deref(), Some("hel"));
        assert_eq!(
            e.body.mode,
            EditorMode::Insert,
            "selection never surfaces a vim mode"
        );
    }

    #[test]
    fn drag_across_lines_joins_with_newlines() {
        let mut e = editor_with_body("hello\nworld\n");
        e.handle_mouse(left_down(2, 0));
        e.body_drag_to(4, 1);
        assert_eq!(e.body_selected_text().as_deref(), Some("hello\nwor"));
    }

    #[test]
    fn a_plain_click_clears_the_selection() {
        let mut e = editor_with_body("hello\nworld\n");
        e.handle_mouse(left_down(2, 0));
        e.body_drag_to(4, 0);
        assert!(e.body.selection.is_some());
        e.handle_mouse(left_down(3, 1));
        assert!(e.body.selection.is_none());
        assert_eq!(e.body_selected_text(), None);
    }

    #[test]
    fn drag_back_to_the_anchor_cell_is_no_selection() {
        let mut e = editor_with_body("hello\n");
        e.handle_mouse(left_down(2, 0));
        e.body_drag_to(4, 0);
        assert!(e.body.selection.is_some());
        e.body_drag_to(2, 0);
        assert!(e.body.selection.is_none());
    }

    fn skey(code: ratatui::crossterm::event::KeyCode) -> ratatui::crossterm::event::KeyEvent {
        ratatui::crossterm::event::KeyEvent::new(code, KeyModifiers::SHIFT)
    }
    fn pkey(code: ratatui::crossterm::event::KeyCode) -> ratatui::crossterm::event::KeyEvent {
        ratatui::crossterm::event::KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn shift_right_selects_one_char_and_stays_modeless() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("abc\n");
        e.body.cursor = Index2::new(0, 0);
        e.handle_key(skey(KeyCode::Right));
        assert_eq!(e.body.cursor, Index2::new(0, 1));
        assert_eq!(e.body_selected_text().as_deref(), Some("a"));
        assert_eq!(e.body.mode, EditorMode::Insert);
    }

    #[test]
    fn shift_down_selects_the_first_line() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("hello\nworld\n");
        e.body.cursor = Index2::new(0, 0);
        e.handle_key(skey(KeyCode::Down));
        assert_eq!(e.body.cursor, Index2::new(1, 0));
        assert_eq!(e.body_selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn shift_end_then_typing_replaces_the_selection() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("abc\n");
        e.body.cursor = Index2::new(0, 0);
        e.handle_key(skey(KeyCode::End));
        assert_eq!(e.body_selected_text().as_deref(), Some("abc"));
        e.handle_key(pkey(KeyCode::Char('x')));
        assert_eq!(e.body_text(), "x\n");
        assert!(e.body.selection.is_none());
        assert_eq!(e.body.mode, EditorMode::Insert);
    }

    #[test]
    fn backspace_deletes_only_the_selection() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("abcd\n");
        e.body.cursor = Index2::new(0, 1);
        e.handle_key(skey(KeyCode::Right));
        e.handle_key(skey(KeyCode::Right));
        assert_eq!(e.body_selected_text().as_deref(), Some("bc"));
        e.handle_key(pkey(KeyCode::Backspace));
        assert_eq!(e.body_text(), "ad\n");
        assert_eq!(e.body.cursor, Index2::new(0, 1));
    }

    #[test]
    fn unshifted_motion_clears_the_selection() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("abc\n");
        e.body.cursor = Index2::new(0, 0);
        e.handle_key(skey(KeyCode::Right));
        assert!(e.body.selection.is_some());
        e.handle_key(pkey(KeyCode::Right));
        assert!(e.body.selection.is_none());
    }

    #[test]
    fn esc_clears_the_selection_before_it_blurs() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("abc\n");
        e.body.cursor = Index2::new(0, 0);
        e.handle_key(skey(KeyCode::Right));
        e.handle_key(pkey(KeyCode::Esc));
        assert!(e.body.selection.is_none());
        assert_eq!(e.sub_focus, SubFocus::Content, "first Esc only clears");
        e.handle_key(pkey(KeyCode::Esc));
        assert_eq!(e.sub_focus, SubFocus::None, "second Esc blurs");
    }

    #[test]
    fn ctrl_a_selects_the_whole_body() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("hello\nworld");
        e.body.cursor = Index2::new(1, 2);
        e.handle_key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(e.body_selected_text().as_deref(), Some("hello\nworld"));
    }

    #[test]
    fn click_maps_through_a_scrolled_viewport() {
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let mut e = editor_with_body(&text);
        // Put the cursor deep in the buffer and re-render so edtui scrolls
        // the viewport down to follow it.
        e.body.cursor = Index2::new(25, 0);
        render_body(&mut e);
        let top = e.body.viewport_offset().1;
        assert!(top > 0, "viewport should have scrolled, got {top}");
        // 31 rows -> a 3-cell gutter, so content column 1 sits at x = 4.
        e.handle_mouse(left_down(4, 0));
        assert_eq!(e.body.cursor, Index2::new(top, 1));
        // Past the end of the top visible line: caret after its last char.
        e.handle_mouse(left_down(35, 0));
        let len = e.body.lines.len_col(top).unwrap();
        assert_eq!(e.body.cursor, Index2::new(top, len));
    }

    #[test]
    fn click_in_an_empty_body_stays_at_the_origin() {
        let mut e = editor_with_body("");
        e.handle_mouse(left_down(20, 5));
        assert_eq!(e.body.cursor, Index2::new(0, 0));
    }

    /// shift+tab arrives from the terminal as `BackTab` with SHIFT set — a
    /// code edtui's crossterm conversion panics on (`unimplemented!()`), so
    /// it must be consumed here whether or not a selection is live.
    fn backtab() -> ratatui::crossterm::event::KeyEvent {
        ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::BackTab,
            KeyModifiers::SHIFT,
        )
    }

    #[test]
    fn tab_indents_the_selected_lines_and_keeps_the_selection() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("{\n\"a\": 1\n}");
        e.set_body_selection_cells(Index2::new(0, 0), Index2::new(2, 0));
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "  {\n  \"a\": 1\n  }");
        assert!(e.body.selection.is_some(), "selection survives an indent");
        // A second press keeps working on the same (shifted) selection.
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "    {\n    \"a\": 1\n    }");
        assert_eq!(e.body.mode, EditorMode::Insert);
    }

    #[test]
    fn tab_skips_empty_lines_when_indenting() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("a\n\nb");
        e.set_body_selection_cells(Index2::new(0, 0), Index2::new(2, 0));
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "  a\n\n  b");
    }

    #[test]
    fn shift_tab_dedents_the_selected_lines_without_panicking() {
        let mut e = editor_with_body("  a\n\tb\n    c\nd");
        e.set_body_selection_cells(Index2::new(0, 2), Index2::new(3, 0));
        e.handle_key(backtab());
        assert_eq!(
            e.body_text(),
            "a\nb\n  c\nd",
            "one tab stop stripped per line: two spaces, one tab, \
             already-flush lines untouched"
        );
        assert!(e.body.selection.is_some(), "selection survives a dedent");
    }

    #[test]
    fn shift_tab_without_a_selection_dedents_the_cursor_line() {
        let mut e = editor_with_body("  a\n  b");
        e.body.cursor = Index2::new(1, 2);
        e.handle_key(backtab());
        assert_eq!(e.body_text(), "  a\nb");
        assert_eq!(e.body.cursor, Index2::new(1, 0), "caret rides the shift");
        // On an already-flush line it's a no-op, not a panic.
        e.handle_key(backtab());
        assert_eq!(e.body_text(), "  a\nb");
    }

    #[test]
    fn tab_without_a_selection_still_inserts() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("ab");
        e.body.cursor = Index2::new(0, 1);
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "a\tb", "plain Tab keeps edtui's insert");
    }
}
