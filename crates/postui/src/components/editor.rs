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
    SyntaxHighlighter,
};
use indexmap::IndexMap;
use postui_core::model::{Body, Entry, HttpRequest, Method};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Which editor tab is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTab {
    Params,
    Headers,
    Body,
    Vars,
}

/// Left-to-right tab-strip order, and the order `EditorTabCycle` walks:
/// Params → Headers → Vars → Body. Deliberately *not* the same order as
/// [`EditorTab::index`] (which stays Params/Headers/Body = 0/1/2 so the
/// existing alt+1/2/3 keybindings keep landing on the same tabs they always
/// did, even though Vars now sits between Headers and Body on screen).
const DRAW_ORDER: [EditorTab; 4] = [
    EditorTab::Params,
    EditorTab::Headers,
    EditorTab::Vars,
    EditorTab::Body,
];

impl EditorTab {
    /// Stable slot number for the `alt+1/2/3/4` shortcuts
    /// (`Action::EditorTabSelect`), unaffected by where Vars was inserted on
    /// screen: Params=0, Headers=1, Body=2, Vars=3.
    pub fn index(self) -> usize {
        match self {
            EditorTab::Params => 0,
            EditorTab::Headers => 1,
            EditorTab::Body => 2,
            EditorTab::Vars => 3,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i % 4 {
            0 => EditorTab::Params,
            1 => EditorTab::Headers,
            2 => EditorTab::Body,
            _ => EditorTab::Vars,
        }
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
    pub active_tab: EditorTab,
    pub sub_focus: SubFocus,
    /// Shared cursor/edit state for the key/value table, reused by both the
    /// Params and Headers tabs (never holds the entry data itself).
    pub table: TableEditorState,
    /// The screen area the body editor was rendered into on the last frame,
    /// recorded by `draw_tab_content`'s Body arm; `None` on any other tab
    /// (including the very first frame before anything has drawn). Mouse
    /// events are hit-tested against this before being forwarded to edtui.
    pub last_body_area: Option<Rect>,
    /// Mirrors `App::in_flight.is_some()`, synced by `App::update` on every
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
    /// Bumped on every `Action::Tick`, regardless of `sending`; drives the
    /// Send cap's spinner glyph and accent/accent_edge_dark pulse while a
    /// request is in flight. Wrapping is harmless -- only taken mod the
    /// spinner's frame count / a small pulse period.
    pub spinner_frame: u32,
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
            body_handler: EditorEventHandler::emacs_mode(),
            active_tab: EditorTab::Params,
            sub_focus: SubFocus::Url,
            table: TableEditorState::default(),
            last_body_area: None,
            sending: false,
            last_method_area: None,
            last_url_text_area: None,
            spinner_frame: 0,
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

    /// Builds an `HttpRequest` from the editor's current field values.
    pub fn current_request(&self) -> HttpRequest {
        HttpRequest {
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
    }

    /// Whether the body parses as JSON. An empty body is vacuously valid:
    /// there is nothing to be wrong with yet.
    fn body_is_valid(&self) -> bool {
        let text = self.body_text();
        text.is_empty() || postui_core::json::validate(&text).is_ok()
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

    /// Advances the Send cap's spinner/pulse frame counter. Called on every
    /// `Action::Tick` unconditionally (cheap wrapping add) so the counter is
    /// already warm the instant a send starts, rather than waiting for the
    /// first tick after `sending` flips.
    pub fn on_tick(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
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
        if m.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(cursor) = self.body_cursor_for_click(m.column, m.row)
        {
            self.body.cursor = cursor;
        }
        true
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
                // Esc blurs the buffer (Enter must stay a newline in a
                // multi-line editor); Up climbs out to the tab strip only
                // from the top row, so it can still navigate the body. CTRL/ALT
                // combos the keymap binds to an app action are shadowed here
                // (the router hands those to the global keymap first); any
                // unbound modified combo falls through to this component and
                // reaches edtui's own emacs-style bindings (ctrl+a/e/k etc.)
                // deliberately, so those keep working for body editing.
                if ev.code == KeyCode::Esc {
                    self.sub_focus = SubFocus::None;
                    return Some(Action::Render);
                }
                if ev.code == KeyCode::Up && self.body.cursor.row == 0 {
                    self.sub_focus = SubFocus::Tabs;
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
            EditorTab::Body => Constraint::Min(0),
            EditorTab::Params | EditorTab::Headers | EditorTab::Vars if self.table_collapsed => {
                Constraint::Length(0)
            }
            EditorTab::Params | EditorTab::Headers | EditorTab::Vars => {
                let (rows, active, active_hint) = self.table_geometry();
                let (inherited, computed_extra) = if self.active_tab == EditorTab::Headers {
                    let auto_rows = self.computed_row_count();
                    let divider = if auto_rows > 0 { 1 } else { 0 };
                    (
                        self.inherited_header_lines(ctx.theme).len() as u16,
                        auto_rows + divider,
                    )
                } else {
                    (0, 0)
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

        // The toolbar row holds the Body tab's body-only tools; every other
        // tab starts its content directly under the tab bar.
        let toolbar_height = if self.active_tab == EditorTab::Body {
            TOOLBAR_HEIGHT
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(ADDRESS_BAR_HEIGHT), // fused address bar + its ring margins
                Constraint::Length(TAB_BAR_HEIGHT),     // tab bar (+ right-aligned save/vars)
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
/// Height of the tab bar row — the second row of that split.
pub const TAB_BAR_HEIGHT: u16 = 2;
/// Height of the toolbar chip row holding the Body tab's
/// format/minify/substitute/`$EDITOR` chips — the third row of that split
/// on the Body tab only. The other tabs have no body tools, and the
/// request-level save/vars chips live on the tab-label row, so they skip
/// the row entirely.
pub const TOOLBAR_HEIGHT: u16 = 1;
/// The Editor pane's total on-screen height when its params/headers table is
/// collapsed: just the two fixed content rows above (address bar, tab bar),
/// with nothing left for a table — the toolbar row is Body-only and a
/// Body-tab editor never collapses to chrome. Panes no longer draw a
/// border, so this is exactly their combined height — no border-row inset
/// to add. `layout::compute_layout` sizes the Editor pane down to exactly
/// this so the Response pane can reclaim every row the table would
/// otherwise have used.
pub const CHROME_HEIGHT: u16 = ADDRESS_BAR_HEIGHT + TAB_BAR_HEIGHT;

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
        use crate::paint::{face_edges, fill, half_cap_bottom, half_cap_top, text};

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
        // Shaded cap rows above and below the centered text row.
        let text_y = bar.y + 1;
        let cap_top_row = |r: Rect| Rect::new(r.x, r.y, r.width, 1);
        let text_row = |r: Rect| Rect::new(r.x, r.y + 1, r.width, 1);
        let cap_bottom_row = |r: Rect| Rect::new(r.x, r.y + r.height - 1, r.width, 1);

        let buf = frame.buffer_mut();

        // --- method segment --------------------------------------------
        // Focus reuses hover's lift color: the badge has no pressed state,
        // so the lift is unambiguous, and hover/focus rarely coexist (a
        // deliberate choice over a ring — see the focus-outline sweep).
        let method_face = theme.method_color(self.method);
        let method_hovered = ctx.hovered == Some(&crate::hit::Hit::MethodSelector);
        let method_fill = if method_hovered || method_focused {
            face_edges(method_face, theme).0
        } else {
            method_face
        };
        // Caps follow the currently shown fill so the whole segment lifts
        // on hover, matching `Button`'s convention.
        let (m_light, m_dark) = face_edges(method_fill, theme);
        half_cap_top(buf, cap_top_row(method_area), m_light, theme.page);
        fill(buf, text_row(method_area), method_fill);
        half_cap_bottom(buf, cap_bottom_row(method_area), m_dark, theme.page);
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
        // single step is nearly invisible. Caps follow the lifted fill.
        let url_fill = if url_focused {
            crate::theme::lift_color(theme.control, 0.12)
        } else {
            theme.control
        };
        let (u_light, u_dark) = if url_focused {
            face_edges(url_fill, theme)
        } else {
            (theme.edge_light, theme.edge_dark)
        };
        half_cap_top(buf, cap_top_row(url_area), u_light, theme.page);
        fill(buf, text_row(url_area), url_fill);
        half_cap_bottom(buf, cap_bottom_row(url_area), u_dark, theme.page);
        // The text is inset URL_PAD columns from the method segment so it
        // isn't flush against the badge.
        let url_text_area = Rect {
            x: url_area.x + URL_PAD.min(url_area.width),
            width: url_area.width.saturating_sub(URL_PAD),
            ..url_area
        };
        let mut url_line = self
            .url
            .draw_line_windowed(url_focused, theme, url_text_area.width);
        url_line.style = Style::default().bg(url_fill).patch(url_line.style);
        buf.set_line(url_text_area.x, text_y, &url_line, url_text_area.width);
        hits.register(url_area, crate::hit::Hit::UrlBar);
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
            let glyph = SPINNER_GLYPHS[(self.spinner_frame as usize) % SPINNER_GLYPHS.len()];
            let pulse_dark = (self.spinner_frame / 3) % 2 == 1;
            let face = if pulse_dark {
                theme.accent_edge_dark
            } else {
                theme.accent
            };
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
                theme.accent_edge_light,
                theme.on_accent,
                true,
            )
        } else {
            ("Send".to_string(), theme.accent, theme.on_accent, true)
        };
        // Caps follow the currently shown fill (matching `Button`'s
        // convention: the whole control reacts to hover/pulse), except
        // disabled, which goes flat in the control fill.
        let (s_light, s_dark) = if disabled {
            (theme.control, theme.control)
        } else {
            face_edges(send_fill, theme)
        };
        half_cap_top(buf, cap_top_row(send_area), s_light, theme.page);
        fill(buf, text_row(send_area), send_fill);
        half_cap_bottom(buf, cap_bottom_row(send_area), s_dark, theme.page);
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
                let count = match t {
                    EditorTab::Params => self.params.len(),
                    EditorTab::Headers => self.headers.len(),
                    EditorTab::Vars => self.variables.len(),
                    EditorTab::Body => 0,
                };
                let label = if count > 0 {
                    format!("{} · {count}", t.label())
                } else {
                    t.label().to_string()
                };
                let badge = match t {
                    EditorTab::Body => Some(if self.body_is_valid() {
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
        let rects = {
            let buf = frame.buffer_mut();
            crate::paint::TabStrip {
                tabs: &tab_strip,
                active,
                hovered,
                focused: ctx.focused && self.sub_focus == SubFocus::Tabs,
            }
            .paint(buf, strip_area, theme.page, theme)
        };
        for (i, rect) in rects.iter().enumerate() {
            hits.register(*rect, crate::hit::Hit::EditorTab(i));
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
        let toggle_x = area.x + area.width.saturating_sub(toggle_w);
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

        // --- save / vars chips (right-aligned, left of the toggle) ---
        // Request-level actions: they save/parameterize the whole request,
        // not the active tab, so they sit apart from the tabs on the same
        // row — right-aligned, the same "about the whole pane" position the
        // Response pane's Copy/Save buttons use.
        let save_label = if self.is_dirty() { "save •" } else { "save" };
        let chips: Vec<(&str, &str, Option<Action>)> = vec![
            ("⭳", save_label, Some(Action::SaveRequest)),
            (
                "{{ }}",
                "vars",
                Some(Action::OpenVarPicker { completing: false }),
            ),
        ];
        // Each chip is ` {key}` + ` {label} ` wide, with `paint_chip_row`'s
        // 2-col gap between consecutive chips.
        let chips_w: u16 = chips
            .iter()
            .map(|(key, label, _)| (key.chars().count() + label.chars().count() + 3) as u16)
            .sum::<u16>()
            + 2 * (chips.len().saturating_sub(1)) as u16;
        // Right-aligned against the toggle, but never over the tab labels
        // (or the substitute indicator after them): in a pane too narrow for
        // everything, the chips start after the tabs instead and
        // `paint_chip_row` drops whatever runs past the limit.
        let tabs_end = rects
            .last()
            .map(|r| r.x + r.width + if self.substitute_body { 7 } else { 0 })
            .unwrap_or(area.x);
        let right_limit = toggle_x.saturating_sub(2);
        crate::components::footer::paint_chip_row(
            buf,
            area.y,
            right_limit.saturating_sub(chips_w).max(tabs_end + 2),
            right_limit,
            &chips,
            theme,
            hits,
            ctx.hovered,
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
            "{{on}}"
        } else {
            "{{off}}"
        };
        let chips: Vec<(&str, &str, Option<Action>)> = vec![
            ("align", "format", Some(Action::FormatBody)),
            ("min", "minify", Some(Action::MinifyBody)),
            ("sub", sub_label, Some(Action::ToggleBodyVars)),
            ("ed", "$EDITOR", Some(Action::OpenBodyInEditor)),
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
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                    dragging: ctx.dragging,
                };
                self.table.draw(
                    frame,
                    area,
                    &self.variables,
                    &table_ctx,
                    "+ Add variable",
                    hits,
                    Some(&self.shadowed),
                    &self.vars,
                );
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
                let mut area = area;
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
                let highlighter = json_highlighter(theme);
                let mut edtui_theme = EditorTheme::default()
                    .base(Style::default().bg(theme.page).fg(theme.text))
                    .cursor_style(Style::default().add_modifier(Modifier::REVERSED))
                    .line_numbers_style(Style::default().bg(theme.page).fg(theme.text_muted))
                    .hide_status_line();
                // A cursor block on an unfocused pane reads as "you are typing
                // here", so only the focused editor shows one.
                if !focused {
                    edtui_theme = edtui_theme.hide_cursor();
                }
                let view = EditorView::new(&mut self.body)
                    .theme(edtui_theme)
                    .wrap(true)
                    .line_numbers(LineNumbers::Absolute)
                    .syntax_highlighter(highlighter);
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
    /// is Task 12). Each row gets a trailing `⧉` copy icon
    /// (`Hit::AutoHeaderCopy`, indexed by its position in this filtered
    /// list); the divider carries a `👁 reveal`/`hide` toggle
    /// (`Hit::AutoHeaderReveal`) whenever `self.computed.has_secret`.
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
                Span::styled(" \u{29c9} ", glyph_style),
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

/// The bundled syntect theme closest to the app palette. edtui's theme names
/// use dashes rather than syntect's dotted defaults. Matching the app's own
/// colors exactly is stage-6 polish; until then a missing theme or JSON
/// syntax definition degrades to unhighlighted text rather than failing the
/// draw.
///
/// Rebuilt per draw because `SyntaxHighlighter` is not `Clone` and the view
/// takes it by value. That is cheap: the theme and syntax *sets* behind it are
/// `Arc`-shared process-wide statics, so only one theme and one syntax
/// reference are cloned, and draws are event-driven rather than continuous.
fn json_highlighter(theme: &Theme) -> Option<SyntaxHighlighter> {
    let name = if theme.is_dark() {
        "base16-ocean-dark"
    } else {
        "base16-ocean-light"
    };
    SyntaxHighlighter::new(name, "json").ok()
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
        app.update(Action::EditorTabSelect(2));
        assert_eq!(app.editor.active_tab, EditorTab::Body);
        app.update(Action::EditorTabCycle(1));
        assert_eq!(app.editor.active_tab, EditorTab::Params, "cycle wraps");
    }

    #[test]
    fn tab_cycle_backward_wraps() {
        let mut app = App::new_for_test();
        assert_eq!(app.editor.active_tab, EditorTab::Params);
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
        let mut e = Editor::default();
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
        let mut app = App::new_for_test();
        app.editor.set_body_text("{\n  \"a\": oops\n}");
        assert!(app.update(Action::FormatBody));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| app.toasts.draw(f, f.area(), &theme))
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
    fn save_and_vars_sit_right_aligned_on_the_tab_label_row() {
        let mut e = Editor::default();
        let (content, hits) = draw_editor(&mut e);
        assert!(content.contains("save"), "save chip label: {content}");
        assert!(content.contains("vars"), "vars chip label: {content}");

        let save_rect = hits
            .rect_of(&Hit::FooterChip(Action::SaveRequest))
            .expect("save chip must be a registered hit");
        let vars_rect = hits
            .rect_of(&Hit::FooterChip(Action::OpenVarPicker {
                completing: false,
            }))
            .expect("vars chip must be a registered hit");
        // Request-level actions live on the tab-label row (the first row of
        // the tab bar), not on a row of their own below the tabs.
        assert_eq!(save_rect.y, ADDRESS_BAR_HEIGHT);
        assert_eq!(vars_rect.y, ADDRESS_BAR_HEIGHT);
        assert!(
            vars_rect.x > save_rect.x,
            "vars chip sits right of save, left to right"
        );
        // Right-aligned, but left of the collapse toggle that keeps the far
        // right edge of the same row.
        let toggle = hits
            .rect_of(&Hit::TableCollapse)
            .expect("collapse toggle hit");
        assert!(
            vars_rect.x + vars_rect.width <= toggle.x,
            "chips must not overlap the collapse toggle: vars {vars_rect:?} toggle {toggle:?}"
        );
        assert!(
            save_rect.x > 60,
            "chips are right-aligned in a 120-wide pane: {save_rect:?}"
        );
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
    fn save_chip_label_gains_a_dirty_dot_only_while_the_editor_is_dirty() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap(),
        );
        let (clean, _) = draw_editor(&mut e);
        assert!(clean.contains("save "), "clean editor: {clean}");
        assert!(!clean.contains("save •"), "clean editor: {clean}");

        e.url = LineInput::new("https://x/changed");
        assert!(e.is_dirty());
        let (dirty, _) = draw_editor(&mut e);
        assert!(
            dirty.contains("save •"),
            "dirty editor shows the dot: {dirty}"
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
        let render = |text: &str| {
            let mut e = Editor {
                active_tab: EditorTab::Body,
                ..Editor::default()
            };
            e.set_body_text(text);
            let theme = Theme::dark();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
                dragging: false,
            };
            let backend = TestBackend::new(60, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            format!("{:?}", terminal.backend().buffer())
        };
        assert!(render("").contains("Body ✓"), "empty body counts as valid");
        assert!(render("{\"a\": 1}").contains("Body ✓"));
        assert!(render("{oops").contains("Body ✗"));
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
        };
        let backend = TestBackend::new(60, 10);
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
    fn tab_strip_focus_solidifies_the_active_tabs_cap() {
        let mut e = Editor {
            sub_focus: SubFocus::Tabs,
            ..Editor::default()
        };
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let tab0 = hits.rect_of(&crate::hit::Hit::EditorTab(0)).unwrap();
        let buf = terminal.backend().buffer();
        let cap = buf.cell((tab0.x, tab0.y + 1)).unwrap();
        assert_eq!(
            cap.bg, theme.accent,
            "focused strip: the active tab's cap is a solid accent row"
        );
        assert_eq!(cap.symbol(), " ");
    }

    #[test]
    fn fused_bar_centers_text_between_shaded_half_caps() {
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
        // Shaded caps above and below: light "▄" on top, dark "▀" below,
        // so the bar reads as 2 text lines with a raised bevel.
        let (m_light, m_dark) = crate::paint::face_edges(method_face, &theme);
        let top_cap = buf.cell((method_area.x, method_area.y)).unwrap();
        assert_eq!(top_cap.symbol(), "▄", "method top cap: {top_cap:?}");
        assert_eq!(top_cap.fg, m_light);
        let bottom_cap = buf.cell((method_area.x, text_y + 1)).unwrap();
        assert_eq!(
            bottom_cap.symbol(),
            "▀",
            "method bottom cap: {bottom_cap:?}"
        );
        assert_eq!(bottom_cap.fg, m_dark);
        let url_cap = buf
            .cell((method_area.x + method_area.width + 2, text_y + 1))
            .unwrap();
        assert_eq!(url_cap.symbol(), "▀", "url cap row: {url_cap:?}");
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
    /// `Action::CancelSend` when `in_flight.is_some()`).
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
        // Caps follow the lifted fill.
        let cap = buf.cell((url_x, method_area.y)).unwrap();
        assert_eq!(cap.symbol(), "▄", "url top cap: {cap:?}");
        assert_eq!(cap.fg, crate::paint::face_edges(lifted, &theme).0);
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
        assert_eq!(app.editor.active_tab, EditorTab::Params);
        assert!(
            app.editor.params.is_empty(),
            "fresh scratch editor: no rows"
        );

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        let layout =
            crate::layout::compute_layout(ratatui::layout::Rect::new(0, 0, 120, 40), false);
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
        let mut e = Editor::default();
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
    fn tab_order_and_alt_shortcuts_survive_vars_insertion() {
        // Draw order / EditorTabCycle: Params -> Headers -> Vars -> Body.
        assert_eq!(EditorTab::from_draw_position(0), EditorTab::Params);
        assert_eq!(EditorTab::from_draw_position(1), EditorTab::Headers);
        assert_eq!(EditorTab::from_draw_position(2), EditorTab::Vars);
        assert_eq!(EditorTab::from_draw_position(3), EditorTab::Body);
        // alt+1/2/3 (`EditorTabSelect(0/1/2)`) are unaffected: still
        // Params/Headers/Body. Vars has no alt shortcut (index 3, unbound).
        assert_eq!(EditorTab::from_index(0), EditorTab::Params);
        assert_eq!(EditorTab::from_index(1), EditorTab::Headers);
        assert_eq!(EditorTab::from_index(2), EditorTab::Body);
        assert_eq!(EditorTab::from_index(3), EditorTab::Vars);
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
}
