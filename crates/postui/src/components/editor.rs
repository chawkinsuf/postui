use super::line_input::LineInput;
use super::table_editor::{Col, RowNote, TableEditorState, TableOutcome, table_height};
use super::toast::ToastKind;
use super::{Component, DrawCtx, pane_surface};
use crate::action::{Action, Verb};
use crate::hit::ScrollbarSpec;
use crate::layout::PaneId;
use crate::text::indent::Indent;
use crate::theme::Theme;
use crate::vim::engine::BodyVisual;
use edtui::{
    EditorEventHandler, EditorMode, EditorState, EditorTheme, EditorView, LineNumbers, Lines,
};
use indexmap::IndexMap;
use postui_core::model::{Body, Entry, HttpRequest, JqView, Method, ViewNameRefusal};
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
/// Headers → Params → Vars → Body. The tab slot numbers
/// ([`EditorTab::index`], bindable as `editor_tab_N`) follow this same
/// order, so the number you bind always matches the tab's on-screen
/// position.
const DRAW_ORDER: [EditorTab; 4] =
    [EditorTab::Headers, EditorTab::Params, EditorTab::Vars, EditorTab::Body];

impl EditorTab {
    /// How many tabs the strip draws (`{N}gt`'s range; plan M4a).
    pub const COUNT: usize = DRAW_ORDER.len();

    /// Slot number for the `editor_tab_N` shortcuts
    /// (`Action::EditorTabSelect`) — identical to [`EditorTab::draw_position`]
    /// so the bound numbers match the tab strip left to right.
    pub fn index(self) -> usize {
        self.draw_position()
    }

    pub fn from_index(i: usize) -> Self {
        Self::from_draw_position(i)
    }

    /// This tab's position in [`DRAW_ORDER`] — what the tab strip's left-to-
    /// right layout, mouse clicks, and `EditorTabCycle` all key off.
    pub fn draw_position(self) -> usize {
        DRAW_ORDER.iter().position(|t| *t == self).expect("DRAW_ORDER lists every tab")
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

/// The editor's own caret, captured across a snapshot swap
/// ([`Editor::apply_snapshot`]) so the caret the user already has comes
/// back in the same place, clamped to the new contents. Table cells are
/// addressed by key (row indices shift under undo) with the index as the
/// fallback when the key is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caret {
    Url(usize),
    Body { row: usize, col: usize },
    Cell { row: usize, key: Option<String> },
    None,
}

/// An untouched auto-indent (spec 2026-10-16): body row `row` holds only
/// the `indent` arrows Enter copied onto it, in a buffer of `lines` rows.
/// Any edit that moves rows changes the count, so the mark never names a
/// row some other line has slid into (R4).
#[derive(Debug, Clone, PartialEq, Eq)]
struct AutoIndent {
    row: usize,
    indent: Vec<char>,
    lines: usize,
}

/// Which sub-region of the editor pane has keyboard focus: the method
/// badge, the URL line, the active tab's content (params table / headers
/// table / body editor), or nothing — the blurred state Enter/Esc/
/// click-away leave behind, in which no editor input captures keys until
/// one is re-entered. Arrow keys walk the chain the way it sits on
/// screen: Method ↔ URL horizontally, URL/Method → Content vertically
/// (and back with Up). The tab strip is not a stop: it is switched by
/// the shared alt+←/→ chord (`Action::CycleTabs`) or a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubFocus {
    Method,
    Url,
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
    /// Whether TLS certificate verification is skipped at send time
    /// (`insecure = true` in the request TOML).
    pub insecure: bool,
    /// The scratch: the request's unnamed view (jq views amendment A1),
    /// what the bar shows while no view is current. Empty is no filter.
    /// File content: editing it dirties the request.
    pub jq: String,
    /// Whether the bar's filter, the scratch or the current view, is
    /// switched on. Closing the bar switches it off and keeps the text.
    /// File content, as a plain edit (amendment A4).
    pub jq_enabled: bool,
    /// The request's saved jq views (spec 1), in saved order. File
    /// content: editing one dirties the request. Each carries an
    /// in-memory `id` (see `next_view_id`) so revert and the modified
    /// mark find its saved text through a rename.
    pub jq_views: Vec<JqView>,
    /// The view the bar shows and edits, by name; `None` (or a name no
    /// view has) is the scratch. File content, as a plain edit (A4).
    pub jq_current: Option<String>,
    /// The id the next view gets, a new one or one `load` numbers: past
    /// every id this editor (and any it replaced, [`Self::fresh`]) ever
    /// handed out or took in (a load, a snapshot), so an id never names two views
    /// while the app's undo history can still apply a snapshot.
    next_view_id: u64,
    /// The active environment's TLS force, mirrored in before each draw
    /// (`ui.rs`) so the padlock can show the *effective* state: the force
    /// when there is one, else `insecure`. Not part of the request.
    pub env_tls: Option<postui_core::project::TlsPolicy>,
    pub params: IndexMap<String, Entry>,
    pub headers: IndexMap<String, Entry>,
    /// Request-scoped `[variables]`, edited by the Vars tab (shares the
    /// Params/Headers table editor) and carried through load/save.
    pub variables: IndexMap<String, Entry>,
    /// `name → "<source>: <value>"`, already formatted (masked for secrets)
    /// by `App` out of `VarView::describe_project` — one entry per
    /// project variable that a request-scope entry with the same name
    /// shadows. Synced by `App` on every `update()`. Draw-only: consumed
    /// by the Vars tab's table draw to show the dim "overrides" hint
    /// trailing a shadowing row's value; never itself edited here.
    pub shadowed: IndexMap<String, String>,
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
    /// The vim engine's Visual span, written through
    /// the engine's body buffer. Draw puts it at the front of
    /// `body.highlights` with the search overlays behind it, where edtui
    /// gives them priority over the JSON colours; `body_hl_overlays` are
    /// the ones currently there. Draw is the only writer of
    /// `body.highlights`.
    pub(crate) body_visual: BodyVisual,
    /// The overlays now at the front of `body.highlights` (see `Overlay`).
    body_hl_overlays: Vec<Overlay>,
    /// The engine's search prompt and matches for the next draw, mirrored
    /// in by `App::vim_mirror_body_search` (spec §5.6). Empty in arrows.
    pub(crate) body_search: crate::keyroute::engine_glue::BodySearch,
    /// The fixed end of an in-progress body selection: planted by a left
    /// click (mouse) or the first shifted motion (keyboard), consumed by
    /// `body_drag_to`/shifted motions to rebuild `body.selection` as the
    /// moving end travels. `None` when no selection gesture is live.
    body_sel_anchor: Option<edtui::Index2>,
    /// The inclusive cell span a double click selected in the body (a word,
    /// or a bracket's block, which may span rows): while set, `body_drag_to`
    /// keeps it covered and extends by whole words past it instead of by
    /// cells. Cleared by any single click.
    body_word_anchor: Option<crate::text::mouse::Span>,
    /// The row a triple click selected: while set, `body_drag_to` extends
    /// by whole lines from it. Cleared by any single click.
    body_line_anchor: Option<usize>,
    /// The indent arrows Enter left alone on a new line, while it is
    /// untouched (spec 2026-10-16): the request's body leaves the line out
    /// ([`Self::body_text`]) and [`Self::settle_auto_indent`] drops it from
    /// the screen once the caret leaves.
    auto_indent: Option<AutoIndent>,
    /// What [`crate::text::indent::detect`] found in `saved`'s body: the
    /// body's own indent (spec 2026-10-15, round-4 amendment). Set only
    /// with `saved` ([`Editor::set_saved`]), so editing, Format and undo
    /// never move it.
    saved_indent: Option<Indent>,
    /// The configured indent (`UiSettings::default_indent`), mirrored in by
    /// `App` on every update so even a fresh scratch has it (R1, R6).
    pub indent_default: Indent,
    pub active_tab: EditorTab,
    /// The last tab the user explicitly chose (click, alt+number, or
    /// arrow-cycling) — where `active_tab` returns whenever it can.
    /// `active_tab` only ever diverges from this through the forced hop
    /// off a disabled Body tab (GET/HEAD); `App::sync_active_tab` restores
    /// the preference as soon as the open request enables it again, so
    /// switching requests never loses the user's place.
    pub preferred_tab: EditorTab,
    pub sub_focus: SubFocus,
    /// Whether the URL line has the caret (open) or is merely the
    /// keyboard's resting stop (selected) — spec 2026-09-16. Always
    /// `false` when `sub_focus != SubFocus::Url`; `select_url`/`open_url`/
    /// `close_url` and every `Url` transition in `handle_key` keep the two
    /// in sync.
    url_open: bool,
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
    /// The whole URL well's screen area (caps included), recorded on every
    /// draw. Read by `App::begin_hover_fade` to tell the pointer *entering*
    /// the well from the pointer merely crossing between the lock, the copy
    /// chip and the text inside it — a containment question the hit map
    /// can't answer, since those chips register on top of `Hit::UrlBar`.
    pub last_url_area: Option<Rect>,
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
    /// Display copy of the app-wide split (see [`crate::split`]), mirrored
    /// in by `App::update` before every draw — same pattern as
    /// `table_collapsed` above — so the tab strip's cluster can light the
    /// segment for the pane's current share.
    pub split: crate::split::SplitState,
    /// The Headers tab's read-only computed-headers section (spec §6):
    /// everything that will actually be sent beyond the editable request
    /// rows above — default headers, the auto Content-Type, and the
    /// client-generated rows (the URL's Authorization, Accept, Host,
    /// Content-Length). A row something overrides is struck through;
    /// Host and Content-Length never are (spec R3a). Recomputed
    /// every draw by `recompute_computed_headers` (cheap, small N) so an
    /// env switch or a body edit shows up live; never itself edited here.
    pub computed: ComputedHeadersView,
    /// The variable snapshot inline `{{token}}` highlighting and the hover
    /// tooltip resolve against (spec §7), synced by `App::update` on every
    /// action alongside `shadowed`. Draw-only: every surface that can show a
    /// token (URL bar, table cells, computed-header rows, the body editor)
    /// tints and registers its spans from this.
    pub vars: crate::components::var_tokens::VarView,
}

/// State for [`Editor::draw_computed_headers`], read off
/// `postui_core::prepare::header_rows` by
/// [`Editor::recompute_computed_headers`].
#[derive(Debug, Default)]
pub struct ComputedHeadersView {
    /// The auto section's rows, in `header_rows` order: every row other
    /// than the request's own (the editable table shows those), sent or
    /// not; one that isn't sent sits where it would have gone and draws
    /// struck through (task #57). `Hit::AutoHeaderCopy(i)` is `rows[i]`.
    pub rows: Vec<postui_core::prepare::HeaderRow>,
    /// The request table's notes, one per row: a row kept off the wire
    /// because the client owns its header (spec 2026-10-18 R3a) is
    /// [`RowNote::Unsent`], which the table draws struck in red and the
    /// footer explains ([`Editor::client_owned_message`]).
    pub notes: Vec<Option<RowNote>>,
    /// Whether secrets show in the clear (each row's `value`) rather than
    /// masked (its `display`).
    pub revealed: bool,
}

impl ComputedHeadersView {
    /// Whether any row has a secret to reveal — independent of
    /// `revealed`, so the toggle that hides it again stays on screen.
    pub fn has_secret(&self) -> bool {
        self.rows.iter().any(|r| r.header.is_masked())
    }
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
            insecure: false,
            jq: String::new(),
            // A never-saved scratch is on, as the bar always was.
            jq_enabled: true,
            jq_views: Vec::new(),
            jq_current: None,
            next_view_id: 1,
            env_tls: None,
            params: IndexMap::new(),
            headers: IndexMap::new(),
            variables: IndexMap::new(),
            shadowed: IndexMap::new(),
            vars: Default::default(),
            body: new_body_state(""),
            body_hl_text: String::new(),
            body_hl_marker: None,
            body_visual: None,
            body_hl_overlays: Vec::new(),
            body_search: Default::default(),
            body_handler: EditorEventHandler::emacs_mode(),
            body_sel_anchor: None,
            body_word_anchor: None,
            body_line_anchor: None,
            auto_indent: None,
            saved_indent: None,
            indent_default: Indent::DEFAULT,
            active_tab: EditorTab::Headers,
            preferred_tab: EditorTab::Headers,
            sub_focus: SubFocus::Url,
            url_open: false,
            table: TableEditorState::default(),
            last_body_area: None,
            sending: false,
            last_method_area: None,
            last_url_text_area: None,
            last_url_area: None,
            send_started: None,
            table_collapsed: false,
            split: crate::split::SplitState::default(),
            computed: ComputedHeadersView::default(),
        }
    }
}

impl Editor {
    /// The key/value table behind `tab`; `None` for Body, which has none.
    pub fn table_map(&self, tab: EditorTab) -> Option<&IndexMap<String, Entry>> {
        match tab {
            EditorTab::Params => Some(&self.params),
            EditorTab::Headers => Some(&self.headers),
            EditorTab::Vars => Some(&self.variables),
            EditorTab::Body => None,
        }
    }

    /// `table_map`, mutably. It borrows the whole editor, so a caller that
    /// also needs `self.table` at once keeps its own field-wise match.
    pub fn table_map_mut(&mut self, tab: EditorTab) -> Option<&mut IndexMap<String, Entry>> {
        match tab {
            EditorTab::Params => Some(&mut self.params),
            EditorTab::Headers => Some(&mut self.headers),
            EditorTab::Vars => Some(&mut self.variables),
            EditorTab::Body => None,
        }
    }

    /// Whether a plain printable key typed right now lands in one of the
    /// pane's text inputs rather than falling through to the global
    /// keymap: the URL line, the body editor, or a live table-cell edit.
    /// The method badge, the tab strip, a selected-but-not-editing table
    /// row, and the blurred state all pass plain keys on — so `q` quits
    /// from them, and the footer's quit chip can say so.
    pub fn plain_keys_type(&self) -> bool {
        match self.sub_focus {
            SubFocus::Url => self.url_open,
            SubFocus::Content => self.active_tab == EditorTab::Body || self.table.editing.is_some(),
            SubFocus::Method | SubFocus::None => false,
        }
    }

    /// Whether the URL line has the caret right now (spec 2026-09-16).
    pub fn url_open(&self) -> bool {
        self.sub_focus == SubFocus::Url && self.url_open
    }

    /// `Action::FocusUrl` and a click on the bar both open the line
    /// directly, rather than merely selecting it: a shortcut or a click
    /// names the field to type into.
    pub fn open_url_from_app(&mut self) {
        self.open_url();
    }

    /// `App::land_editor_caret`: the URL line closes if it was open, and
    /// the caret goes into the active tab's content.
    pub fn focus_content_from_app(&mut self) {
        self.close_url();
        self.enter_content();
    }

    fn select_url(&mut self) {
        self.sub_focus = SubFocus::Url;
        self.url_open = false;
    }

    /// Down from the URL line or the method badge: into the active tab's
    /// content. Entering a table tab must land somewhere visible, so its
    /// first row — or, on an empty table, its ghost "+ Add" row (index 0
    /// either way) — is selected instead of a focused-but-nothing-selected
    /// limbo. A selection already there is kept.
    fn enter_content(&mut self) {
        self.sub_focus = SubFocus::Content;
        if matches!(self.active_tab, EditorTab::Params | EditorTab::Headers | EditorTab::Vars)
            && self.table.selected.is_none()
        {
            self.table.selected = Some(0);
        }
    }

    fn open_url(&mut self) {
        self.sub_focus = SubFocus::Url;
        self.url_open = true;
    }

    /// Ends any open edit and leaves the URL line, wherever `sub_focus`
    /// goes next.
    fn close_url(&mut self) {
        self.url.end_edit();
        self.url_open = false;
    }

    /// Loads `req` into the editor for editing, and records it as the
    /// last-saved state so `is_dirty` starts out `false`. Also re-masks the
    /// computed-headers section (`computed.revealed = false`): reveal is a
    /// per-request gesture (spec §3: secrets masked by default), so opening
    /// a different request must not carry an earlier reveal along with it
    /// -- the same re-masking-on-context-switch the Variable Manager's own
    /// scoped reveal already does.
    pub fn load(&mut self, slug: Option<String>, mut req: HttpRequest) {
        // Ids before both the fields and `saved` take the views, so a view
        // finds its saved text by id. A request from memory (Discard
        // reloads `saved`) carries this editor's own ids: keep them, so
        // undo across the reload finds the same saved texts. One read
        // from disk has none (all 0): number from the counter, never from
        // 1 again, so an older snapshot's id never names another view.
        let mut ids: Vec<u64> = req.jq_views.iter().map(|v| v.id).collect();
        ids.sort_unstable();
        ids.dedup();
        let own = ids.len() == req.jq_views.len() && !ids.contains(&0);
        if own {
            let past = ids.last().map_or(0, |id| id + 1);
            self.next_view_id = self.next_view_id.max(past);
        } else {
            for view in &mut req.jq_views {
                view.id = self.next_view_id;
                self.next_view_id += 1;
            }
        }
        self.slug = slug;
        self.name = req.name.clone();
        self.method = req.method;
        self.url = LineInput::new(&req.url);
        self.substitute_body = req.substitute_body;
        self.insecure = req.insecure;
        self.jq = req.jq.clone().unwrap_or_default();
        self.jq_enabled = req.jq_enabled;
        self.jq_views = req.jq_views.clone();
        self.jq_current = req.jq_current.clone();
        self.params = req.params.clone();
        self.headers = req.headers.clone();
        self.variables = req.variables.clone();
        self.set_body_text(match &req.body {
            Some(Body::Json { text }) => text,
            None => "",
        });
        self.set_saved(Some(req));
        self.computed.revealed = false;
        // A cell still open belongs to the request being replaced: never
        // let it commit into this one. Every way to a load commits an open
        // cell first (the dirty gates, and the landing's
        // `App::flush_field_session`; spec 2026-10-19), so dropping one
        // here is only a backstop. The row selection stays, as it always
        // has: Save As lands on the content it was made in. So does a
        // value stashed on the "+ Add" row (task #68 owns where it should
        // go).
        self.table.editing = None;
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
        let caret = self.caret();
        let keys_before: Vec<String> =
            (0..self.table_len()).filter_map(|i| self.table_key_at(i)).collect();
        self.name = req.name.clone();
        self.method = req.method;
        self.url = LineInput::new(&req.url);
        self.substitute_body = req.substitute_body;
        self.insecure = req.insecure;
        self.jq = req.jq.clone().unwrap_or_default();
        self.jq_enabled = req.jq_enabled;
        // The snapshot's ids are this editor's own (it recorded them), so
        // they keep matching `saved`; new views go past every one of them.
        self.jq_views = req.jq_views.clone();
        self.jq_current = req.jq_current.clone();
        let past = self.jq_views.iter().map(|v| v.id + 1).max().unwrap_or(0);
        self.next_view_id = self.next_view_id.max(past);
        self.params = req.params.clone();
        self.headers = req.headers.clone();
        self.variables = req.variables.clone();
        self.set_body_text(match &req.body {
            Some(Body::Json { text }) => text,
            None => "",
        });
        self.table.editing = None;
        self.table.selected = None;
        self.restore_caret(&caret);
        // A row the swap brought back (undoing a delete, redoing an add) is
        // where the cursor goes, the way every list reselects its restored
        // row: the first one when several return. Only a row cursor moves —
        // focus and cell text carets stay put (ruling 6772cd9).
        if let Caret::Cell { .. } = caret
            && let Some(i) = (0..self.table_len())
                .find(|&i| self.table_key_at(i).is_some_and(|k| !keys_before.contains(&k)))
        {
            self.table.selected = Some(i);
        }
    }

    /// Where the caret sits right now, so [`Self::apply_snapshot`] can put
    /// it back after swapping the fields underneath it. `None` covers every
    /// focus state a swap can't disturb (Method/Tabs/None sub-focus, or a
    /// table tab with nothing selected).
    pub fn caret(&self) -> Caret {
        match self.sub_focus {
            SubFocus::Url => Caret::Url(self.url.cursor()),
            SubFocus::Content if self.active_tab == EditorTab::Body => {
                Caret::Body { row: self.body.cursor.row, col: self.body.cursor.col }
            }
            SubFocus::Content => match self.table.selected {
                Some(row) => Caret::Cell { row, key: self.table_key_at(row) },
                None => Caret::None,
            },
            _ => Caret::None,
        }
    }

    /// Re-places a caret captured by [`Self::caret`] against whatever the
    /// fields now hold (a snapshot swap may have shortened the URL, dropped
    /// a body line, or removed a table row). Focus never moves: the caret
    /// is the one the editor already had, only clamped. `Caret::None`
    /// leaves everything as it is.
    pub fn restore_caret(&mut self, pos: &Caret) {
        match pos {
            Caret::Url(i) => {
                self.url.set_cursor(*i);
            }
            Caret::Body { row, col } => {
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
                let half = self.last_body_area.map(|a| usize::from(a.height) / 2).unwrap_or(0);
                self.body.set_viewport_offset(0, row.saturating_sub(half));
            }
            Caret::Cell { row, key } => {
                // The same row by key (indices shift under undo); a row the
                // swap removed leaves the cursor at its old index, clamped
                // to the last real row. Never the ghost "+ Add" row at
                // index `len`. An empty table has only the ghost row to
                // offer. The cell column persists on its own.
                let len = match self.active_tab {
                    EditorTab::Params => self.params.len(),
                    EditorTab::Headers => self.headers.len(),
                    EditorTab::Vars => self.variables.len(),
                    EditorTab::Body => 0,
                };
                self.table.selected = Some(
                    key.as_deref()
                        .and_then(|k| self.table_index_of(k))
                        .unwrap_or_else(|| (*row).min(len.saturating_sub(1))),
                );
            }
            Caret::None => {}
        }
    }

    /// Builds an `HttpRequest` from the editor's current field values.
    pub fn current_request(&self) -> HttpRequest {
        HttpRequest {
            name: self.name.clone(),
            method: self.method,
            url: self.url.text().to_string(),
            substitute_body: self.substitute_body,
            insecure: self.insecure,
            // The switch as held: the file writes it as it is (A5).
            jq: (!self.jq.is_empty()).then(|| self.jq.clone()),
            jq_enabled: self.jq_enabled,
            jq_views: self.jq_views.clone(),
            jq_current: self.jq_current.clone(),
            params: self.params.clone(),
            headers: self.headers.clone(),
            variables: self.variables.clone(),
            body: {
                let text = self.body_text();
                if text.is_empty() { None } else { Some(Body::Json { text }) }
            },
        }
    }

    /// Recomputes `self.computed` from the editor's current fields against
    /// `ctx` — cheap (small N), so callers run it on every draw rather than
    /// trying to track exactly which edits invalidate it. One pass of
    /// `header_rows`, the list the send itself is built from, split into
    /// the auto section's rows and the request rows it keeps off the wire.
    pub fn recompute_computed_headers(&mut self, ctx: &postui_core::prepare::PrepareContext) {
        use postui_core::prepare::HeaderOrigin;
        let rows = postui_core::prepare::header_rows(&self.current_request(), ctx);
        let mut notes = vec![None; self.headers.len()];
        let mut auto = Vec::new();
        for r in rows {
            match r.origin {
                // Like the auto section, the verdict follows the committed
                // key: a name still being typed is judged once it lands.
                HeaderOrigin::Request { row } => {
                    if let Some(note) = notes.get_mut(row) {
                        *note = client_owned_header(r.unsent).map(RowNote::Unsent);
                    }
                }
                // Every other row is drawn, an unsent one struck through,
                // so the user sees what is overridden (task #57): a
                // default, the auto `Content-Type`, the client's `Accept`
                // or the URL's `Authorization`, whether a typed row or a
                // default replaced it. A struck `Authorization` shows its
                // credential no more than the URL bar shows the userinfo,
                // and masks the same way when a secret is in it.
                HeaderOrigin::Default | HeaderOrigin::ContentType | HeaderOrigin::Client => {
                    auto.push(r)
                }
            }
        }
        self.computed.rows = auto;
        self.computed.notes = notes;
    }

    /// What the footer says about a header row the client owns (spec
    /// 2026-10-18 R3b, task #59). Such a row draws struck in red and says
    /// nothing on its own line, so the footer explains it, the way it
    /// explains any hovered control: for the row `hovered` (a table row's
    /// surface or cell, or an auto row), else, for the keyboard, the table
    /// row under the cursor while the table holds it (`focused`: the
    /// editor pane has it). The fallback holds even with the pointer on
    /// another, ordinary row: the footer is the keyboard's context — its
    /// chips name the cursor row's keys wherever the pointer is — and a
    /// hover that has nothing to say leaves it so.
    ///
    /// Like the row's own red strike, the verdict follows the committed
    /// key: a name being typed is judged once it lands.
    ///
    /// The message comes longest form first, for the footer to show the
    /// longest that fits (`CentreLine::error`): it never takes room from
    /// the chips (the user: "just make it shorter when there is less
    /// space so we dont have to hide the chips"). Empty: nothing to say.
    pub fn client_owned_message(
        &self,
        hovered: Option<&crate::hit::Hit>,
        focused: bool,
    ) -> Vec<String> {
        self.client_owned_row(hovered, focused)
            .map(|header| {
                vec![
                    format!("{header} cannot be set manually"),
                    format!("{header} can't be set"),
                    "can't be set".to_string(),
                ]
            })
            .unwrap_or_default()
    }

    /// The client-owned header [`Editor::client_owned_message`] speaks
    /// for, by its rules.
    fn client_owned_row(
        &self,
        hovered: Option<&crate::hit::Hit>,
        focused: bool,
    ) -> Option<&'static str> {
        use crate::hit::Hit;
        if self.active_tab != EditorTab::Headers || self.table_collapsed {
            return None;
        }
        let table_row = |i: usize| match self.computed.notes.get(i)? {
            Some(RowNote::Unsent(header)) => Some(*header),
            _ => None,
        };
        let hovered = match hovered {
            Some(Hit::TableRow(i) | Hit::TableCell { row: i, .. }) => table_row(*i),
            Some(Hit::AutoHeaderRow(i)) => client_owned_header(self.computed.rows.get(*i)?.unsent),
            _ => None,
        };
        let cursor = (focused && self.sub_focus == SubFocus::Content)
            .then_some(self.table.selected)
            .flatten();
        hovered.or_else(|| table_row(cursor?))
    }

    /// A never-loaded (fresh scratch) editor is not dirty even though it has
    /// no saved snapshot to compare against; only a request that has been
    /// loaded and then changed counts as dirty.
    pub fn is_dirty(&self) -> bool {
        match &self.saved {
            // Every jq field is file content (jq views amendment): the
            // scratch, the switch, the current view and the views compare
            // like any field.
            Some(s) => *s != self.current_request(),
            None => false,
        }
    }

    pub fn mark_saved(&mut self) {
        self.set_saved(Some(self.current_request()));
    }

    /// Replaces the dirty baseline, and the indent detected from its body
    /// with it (spec 2026-10-15, round-4 amendment): the one writer of both.
    pub fn set_saved(&mut self, req: Option<HttpRequest>) {
        self.saved_indent = match req.as_ref().and_then(|r| r.body.as_ref()) {
            Some(Body::Json { text }) => {
                crate::text::indent::detect(&edtui::Lines::from(text.as_str()))
            }
            None => None,
        };
        self.saved = req;
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
                && self.body_text().is_empty()
                && self.jq.is_empty()
                && self.jq_views.is_empty())
    }

    /// A blank editor that keeps this one's view-id counter, for replacing
    /// the editor wholesale: the app's undo history outlives the editor,
    /// so ids must never restart while it can still apply a snapshot.
    pub fn fresh(&self) -> Self {
        Self { next_view_id: self.next_view_id, ..Self::default() }
    }

    /// The current view's index in `jq_views`; `None` with no view picked,
    /// when the bar holds the scratch.
    pub fn current_view(&self) -> Option<usize> {
        let name = self.jq_current.as_deref()?;
        self.jq_views.iter().position(|v| v.name == name)
    }

    /// The current view's name, `None` with no view picked: what the bar
    /// shows, so a `jq_current` naming no view reads as none picked here
    /// too.
    pub fn jq_current_name(&self) -> Option<&str> {
        self.current_view().map(|i| self.jq_views[i].name.as_str())
    }

    /// The filter the bar shows: the current view's, or the scratch.
    pub fn jq_filter(&self) -> &str {
        match self.current_view() {
            Some(i) => &self.jq_views[i].filter,
            None => &self.jq,
        }
    }

    /// Writes the bar's text into the current view, or the scratch.
    pub fn set_jq_filter(&mut self, text: &str) {
        match self.current_view() {
            Some(i) => self.jq_views[i].filter = text.to_string(),
            None => self.jq = text.to_string(),
        }
    }

    /// Makes view `name` current and switches the filter on (spec 16). A
    /// name no view has any more (a reload under the menu) changes nothing,
    /// as a stale delete does. Nothing picks no view: the clear goes there
    /// ([`Self::jq_clear_filter`], amendment C5), and so does a menu
    /// action's or the AI's filter (`App::land_jq_in_scratch`).
    pub fn jq_pick(&mut self, name: &str) -> bool {
        let known = self.jq_views.iter().any(|v| v.name == name);
        if known {
            self.make_view_current(name.to_string());
        }
        known
    }

    /// View `name` current and switched on, the scratch emptied (amendment
    /// C12): picking a view replaces what was typed with none picked, so no
    /// text waits in the file unseen behind the view. Undo brings it back.
    fn make_view_current(&mut self, name: String) {
        self.jq_current = Some(name);
        self.jq.clear();
        self.jq_enabled = true;
    }

    /// The view's filter in the saved snapshot, matched by id only
    /// (survives a rename). `None` for a view saved since (spec 24).
    ///
    /// By id only, never by name: a name can pass from a deleted view to
    /// another one, and then revert would write the wrong text. The cost:
    /// an unsaved buffer seated over a request freshly read from disk (an
    /// undo's landing) carries ids from before the read, so its views show
    /// no modified mark and no revert until the next save. Coarser, never
    /// wrong. The app asks by id ([`Self::jq_current_saved`]); tests ask
    /// by name.
    #[cfg(test)]
    pub fn jq_saved_filter(&self, name: &str) -> Option<&str> {
        self.jq_saved_filter_of(self.jq_views.iter().find(|v| v.name == name)?.id)
    }

    /// [`Self::jq_saved_filter`] for the view with in-memory `id`.
    fn jq_saved_filter_of(&self, id: u64) -> Option<&str> {
        let saved = self.saved.as_ref()?.jq_views.iter().find(|v| v.id == id)?;
        Some(&saved.filter)
    }

    /// The view has a saved text and its filter differs from it (spec 5,
    /// 25: the filter only, never the name). The app asks about the current
    /// filter only ([`Self::jq_current_modified`]); tests ask about any.
    #[cfg(test)]
    pub fn jq_modified(&self, name: &str) -> bool {
        let Some(saved) = self.jq_saved_filter(name) else { return false };
        self.jq_views.iter().any(|v| v.name == name && v.filter != saved)
    }

    /// The current view's saved text ([`Self::jq_saved_filter`]). `None`
    /// with no view current: what is typed with none picked has no ↺, only
    /// views do (amendment C11, superseding A8). `None` too for a view saved
    /// since, or a request never saved.
    pub fn jq_current_saved(&self) -> Option<&str> {
        self.jq_saved_filter_of(self.jq_views[self.current_view()?].id)
    }

    /// Whether the current view differs from its saved text: the bar's ↺
    /// (spec 5, amendments B3, C11).
    pub fn jq_current_modified(&self) -> bool {
        self.jq_current_saved().is_some_and(|saved| saved != self.jq_filter())
    }

    /// Why `name` may not name a view on this request beside every view but
    /// `except` (a rename's own), or `None`: [`JqView::name_refusal`], the
    /// rule the file reader applies too (spec 18).
    pub fn jq_name_refusal(&self, name: &str, except: Option<&str>) -> Option<ViewNameRefusal> {
        let others =
            self.jq_views.iter().map(|v| v.name.as_str()).filter(|other| Some(*other) != except);
        // The name as it will be stored: save-as and rename trim it, so a
        // pasted trailing newline is no control character in it.
        JqView::name_refusal(name.trim(), others)
    }

    /// Spec decision 6 and amendments A9, C12. Err with the refusal when
    /// the name may not be a view's. From a modified view, the edits move
    /// to the new view and the view goes back to its saved text; one with
    /// no saved text keeps it (a copy). From no view, the text moves to the
    /// new view and nothing is left behind it, as a pick leaves nothing.
    /// The new view is current and switched on, as a pick makes a view
    /// (spec 16): saving it from the palette with the bar closed shows what
    /// was saved.
    pub fn jq_save_as(&mut self, name: &str) -> Result<(), ViewNameRefusal> {
        if let Some(why) = self.jq_name_refusal(name, None) {
            return Err(why);
        }
        let filter = self.jq_filter().to_string();
        if let Some(saved) = self.jq_current_saved().map(str::to_string) {
            self.set_jq_filter(&saved);
        }
        let name = name.trim().to_string();
        self.jq_views.push(JqView { name: name.clone(), filter, id: self.next_view_id });
        self.next_view_id += 1;
        self.make_view_current(name);
        Ok(())
    }

    /// Renames view `from` to `to` (trimmed). Err with the refusal when
    /// `to` may not be its name. The view stays current if it was, and
    /// keeps its id, so revert still finds its saved text (spec 23).
    pub fn jq_rename(&mut self, from: &str, to: &str) -> Result<(), ViewNameRefusal> {
        if let Some(why) = self.jq_name_refusal(to, Some(from)) {
            return Err(why);
        }
        let to = to.trim().to_string();
        if let Some(view) = self.jq_views.iter_mut().find(|v| v.name == from) {
            view.name = to.clone();
            if self.jq_current.as_deref() == Some(from) {
                self.jq_current = Some(to);
            }
        }
        Ok(())
    }

    /// Removes the view; if it was current, none is picked after and the
    /// scratch shows as it is (spec 22). Since amendment C12 that is empty;
    /// only a file written before it can still hold text there. `false`
    /// when no view has `name`.
    pub fn jq_delete(&mut self, name: &str) -> bool {
        let before = self.jq_views.len();
        self.jq_views.retain(|v| v.name != name);
        if self.jq_current.as_deref() == Some(name) {
            self.jq_current = None;
        }
        self.jq_views.len() != before
    }

    /// Whether [`Self::jq_clear_filter`] has anything to do: a view is
    /// current, or the scratch holds text. By the raw `jq_current`, as
    /// `App::land_jq_in_scratch` decides "coming off a view": a name no view
    /// has (which a load and a delete never leave) is then cleared too,
    /// rather than kept because the bar reads it as none picked.
    pub fn jq_clearable(&self) -> bool {
        self.jq_current.is_some() || !self.jq.is_empty()
    }

    /// Amendment C4: no view current, with an empty filter. The view that
    /// was current keeps its filter, edits included; the switch is left
    /// as it is.
    pub fn jq_clear_filter(&mut self) {
        self.jq_current = None;
        self.jq.clear();
    }

    /// The current view back to its saved text; no-op with no view picked
    /// or a view never saved (amendment C11).
    pub fn jq_revert(&mut self) {
        if let Some(saved) = self.jq_current_saved().map(str::to_string) {
            self.set_jq_filter(&saved);
        }
    }

    /// The name of the `{{token}}` the keyboard caret is currently sitting
    /// in, for the caret-resting tooltip (spec §7 — a keyboard user gets the
    /// same value readout a hover gives). Covers the two fields a caret can
    /// rest in: the URL line, and the body editor's current line. `None`
    /// anywhere else, and whenever the caret is outside every token.
    pub fn caret_token(&self) -> Option<String> {
        let (text, byte_off) = match self.sub_focus {
            SubFocus::Url if self.url_open => {
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

    /// The body as the request holds it, lines joined by `\n`: the buffer's
    /// text, except that a line holding an untouched auto-indent is empty
    /// (spec 2026-10-16 R2). Save, send, dirty, undo, Format and `$EDITOR`
    /// all read this; only drawing reads the buffer itself.
    pub fn body_text(&self) -> String {
        let text = self.body.lines.to_string();
        let Some(end) = self.untouched_indent_end() else {
            return text;
        };
        // No row holds a `\n`, so splitting on it gives back the rows.
        let rows: Vec<&str> =
            text.split('\n').enumerate().map(|(i, r)| if i == end.row { "" } else { r }).collect();
        rows.join("\n")
    }

    /// Where the untouched auto-indent ends (its row, after its last
    /// blank), while the buffer still has the rows Enter left and the line
    /// still holds exactly that indent: an edit is never hidden by a mark
    /// the next settle has yet to forget.
    fn untouched_indent_end(&self) -> Option<edtui::Index2> {
        let mark = self.auto_indent.as_ref()?;
        (self.body.lines.len() == mark.lines
            && self.body.lines.get(edtui::RowIndex::new(mark.row)) == Some(&mark.indent))
        .then(|| edtui::Index2::new(mark.row, mark.indent.len()))
    }

    /// Settles the untouched auto-indent after an event (spec 2026-10-16):
    /// an edit on its line makes it ordinary text (R4); anything but the
    /// caret resting right after it with nothing selected drops it from
    /// the screen (R3: the caret moving into the blanks or off the line,
    /// leaving the body, or a selection, so nothing that acts on a
    /// selection sees blanks the request lacks), moving selection ends and
    /// anchors on it to column 0 (R8). The request never held the blanks,
    /// so neither changes `body_text`. Answers whether the screen changed.
    pub fn settle_auto_indent(&mut self, caret_in_body: bool) -> bool {
        let Some(end) = self.untouched_indent_end() else {
            self.auto_indent = None;
            return false;
        };
        if caret_in_body && self.body.cursor == end && self.body.selection.is_none() {
            return false;
        }
        let row = end.row;
        self.auto_indent = None;
        if let Some(line) = self.body.lines.get_mut(edtui::RowIndex::new(row)) {
            line.clear();
        }
        // Every position on the emptied row moves to its start.
        let to_start = |r: usize, col: &mut usize| {
            if r == row {
                *col = 0;
            }
        };
        let cursor = &mut self.body.cursor;
        to_start(cursor.row, &mut cursor.col);
        if let Some(sel) = self.body.selection.as_mut() {
            for p in [&mut sel.start, &mut sel.end].into_iter().chain(sel.anchor.as_mut()) {
                to_start(p.row, &mut p.col);
            }
        }
        if let Some(p) = self.body_sel_anchor.as_mut() {
            to_start(p.row, &mut p.col);
        }
        if let Some(span) = self.body_word_anchor.as_mut() {
            for p in [&mut span.fixed, &mut span.moving] {
                to_start(p.row, &mut p.col);
            }
        }
        // A selection wholly inside the blanks has nothing left to select.
        // Its anchor stays, as a collapsed selection's does, so a drag
        // through the blanks goes on selecting.
        if self.body.selection.as_ref().is_some_and(|s| s.start.row == row && s.end.row == row) {
            self.body.selection = None;
        }
        true
    }

    /// Feeds each char of `s` through the body's key handler as a
    /// synthesized plain `KeyCode::Char` event, as if the user had typed it.
    /// Used to splice a picked variable token into the body buffer, which
    /// (unlike `LineInput`) has no direct string-insertion API of its own.
    pub fn body_insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.body_handler.on_key_event(
                KeyEvent::new(KeyCode::Char(c), ratatui::crossterm::event::KeyModifiers::NONE),
                &mut self.body,
            );
        }
    }

    /// The overlays this draw wants, in precedence order (see `Overlay`).
    /// A search match is end-exclusive on one row; an empty one paints
    /// nothing.
    fn body_overlays(&self) -> Vec<Overlay> {
        use crate::keyroute::engine_glue::Pos;
        let span = |(s, e): (Pos, Pos)| {
            (e.col > s.col)
                .then(|| (edtui::Index2::new(s.row, s.col), edtui::Index2::new(e.row, e.col - 1)))
        };
        let mut out = Vec::new();
        if let Some((from, to)) = self.body_visual {
            out.push(Overlay { from, to, kind: OverlayKind::Visual });
        }
        if let Some((from, to)) = self.body_search.current.and_then(span) {
            out.push(Overlay { from, to, kind: OverlayKind::Landing });
        }
        for &m in &self.body_search.matches {
            if let Some((from, to)) = span(m) {
                out.push(Overlay { from, to, kind: OverlayKind::Match });
            }
        }
        out
    }

    /// Lights the bracket by the caret and its partner on the drawn frame
    /// (spec 2026-10-22, matching brackets, R1–R8). It runs after edtui
    /// renders, so the viewport is the one on screen, not a guess at
    /// edtui's scroll. `area` is the rect edtui drew into. `bottom` is the
    /// first screen row the body doesn't show (the vim search prompt
    /// covers the last one).
    ///
    /// A caret between chars (edtui's `Insert`: arrows always, Vim's Insert
    /// and Replace) also takes the bracket before it; Vim's Normal and
    /// Visual carets sit on a char. Both ends must show (R5). A cell the
    /// Visual span, a search hit or the selection paints keeps that paint,
    /// and the caret's cell keeps its reverse.
    fn paint_bracket_pair(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        bottom: u16,
        theme: &Theme,
    ) {
        use crate::text::brackets::{bracket_at, partner_of};
        use crate::text::{Lines as _, Pos};
        let lines = &self.body.lines;
        let gutter = line_number_gutter_width(lines.len());
        let width = usize::from(area.width.saturating_sub(gutter));
        // Most frames have no bracket by the caret: look before measuring
        // the view.
        let caret = self.body.cursor;
        let before = self.body.mode == EditorMode::Insert;
        let Some(col) = bracket_at(&lines.line(caret.row), caret.col, before) else { return };
        if width == 0 {
            return;
        }
        let a = Pos::new(caret.row, col);
        // Each row in view: the screen row it starts on and its wrapped
        // segments.
        let top = self.body.viewport_offset().1;
        let mut rows = Vec::new();
        let mut y = area.y;
        for row in top..lines.len() {
            if y >= bottom {
                break;
            }
            let segments = wrap_segments(&lines.line(row), width);
            let tall = u16::try_from(segments.len().max(1)).unwrap_or(u16::MAX);
            rows.push((y, segments));
            y = y.saturating_add(tall);
        }
        let Some(last) = (top + rows.len()).checked_sub(1) else { return };
        let Some(b) = partner_of(lines, a, top..=last) else { return };
        let cell = |at: Pos| {
            let (y0, segments) = &rows[at.row - top];
            let (k, segment) = segments.iter().enumerate().find(|(_, s)| s.contains(&at.col))?;
            let y = y0.saturating_add(u16::try_from(k).ok()?);
            let line = lines.line(at.row);
            let x: usize = line[segment.start..at.col].iter().map(|&c| body_char_width(c)).sum();
            let x = area.x + gutter + u16::try_from(x).ok()?;
            (y < bottom).then_some((x, y))
        };
        let (Some(cell_a), Some(cell_b)) = (cell(a), cell(b)) else { return };
        // The overlays lead `body.highlights` (the draw splices them in
        // first), so edtui's own test says what they cover.
        let overlays = &self.body.highlights[..self.body_hl_overlays.len()];
        let covered = |at: Pos| {
            let at = edtui::Index2::new(at.row, at.col);
            overlays.iter().any(|h| h.contains(&at))
                || self.body.selection.as_ref().is_some_and(|s| s.contains(&at))
        };
        let lit = Style::default().fg(theme.on_accent).bg(theme.accent);
        for (at, (x, y)) in [(a, cell_a), (b, cell_b)] {
            if !covered(at) {
                buf[(x, y)].set_style(lit);
            }
        }
    }

    /// Replaces the whole body buffer. Only explicit user actions (loading a
    /// request, format, minify, the `$EDITOR` round-trip) may call this;
    /// typing goes through `body_handler`, so the text is never rewritten
    /// behind the user's back.
    pub fn set_body_text(&mut self, s: &str) {
        self.body = new_body_state(s);
        // The new text is exactly the request's (spec 2026-10-16 R7).
        self.auto_indent = None;
        // The engine's Visual span named the old text (final review M-3);
        // the next key's rebase would clear it, a frame late.
        self.body_visual = None;
        // The fresh state has no highlights; force the next draw to relex.
        self.body_hl_text.clear();
        self.body_hl_marker = None;
        self.body_hl_overlays.clear();
    }

    /// The body as the `$EDITOR` temp file holds it: [`Self::body_text`]
    /// with one `\n` ending its last row, which
    /// [`Self::set_body_from_editor_file`] takes off again.
    ///
    /// The file is a text file whose lines are the body's rows. Editors
    /// read a final `\n` as the end of the last line, not as a line of its
    /// own (Vim's `eol`/`fixeol`, nano), and write it back the same way.
    /// Written as-is, a body ending in `\n` (an empty last row) shows no
    /// empty row in Vim, so the way back can't tell the editor's `\n` from
    /// the body's own (#29: stripping one always took the body's own;
    /// stripping only when the body had none left a Vim user no way to
    /// remove one they can't see). Ending the body here makes the way back
    /// exact: the ending taken off is the one put on.
    ///
    /// What it gives up: an editor that shows a final `\n` as an empty row
    /// of its own (VS Code without `files.insertFinalNewline`, Emacs) shows
    /// one empty row more than postui does. Leaving it, or deleting it,
    /// round-trips to the same body. An editor set to trim final newlines
    /// on save trims the body's own empty last rows too, as it would in
    /// any file.
    pub fn body_editor_file(&self) -> String {
        format!("{}\n", self.body_text())
    }

    /// Replaces the body with the text `$EDITOR` wrote back to the file
    /// [`Self::body_editor_file`] made, less the line ending put on there
    /// when the editor kept it.
    ///
    /// The ending taken off is `\r\n` or `\n`, the two [`Self::set_body_text`]
    /// reads as a row's end (edtui splits with `str::lines`), so a CRLF
    /// editor's last row loses its `\r` as every other row does. What that
    /// gives up: a body whose last row ends in a lone `\r` comes back
    /// without it, because in the file that `\r` and the `\n` put on read
    /// as one CRLF ending, to the editor as much as to postui.
    pub fn set_body_from_editor_file(&mut self, text: &str) {
        let body = text.strip_suffix("\r\n").or_else(|| text.strip_suffix('\n')).unwrap_or(text);
        self.set_body_text(body);
    }

    /// The indent the body's keys and Format use: what its text as last
    /// opened or saved uses, else the configured default (spec 2026-10-15
    /// R3, round-4 amendment). Undo never changes the saved text, so it
    /// gives back exactly the indent it had; and an indent never feeds the
    /// width of the next.
    pub fn body_indent(&self) -> Indent {
        self.saved_indent.unwrap_or(self.indent_default)
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

    /// Whether `t` can be selected: the one rule every tab switch asks
    /// (a click, a cycle, `{N}gt`, a method change). Only the Body tab is
    /// ever disabled.
    pub fn tab_enabled(&self, t: EditorTab) -> bool {
        t != EditorTab::Body || !self.body_tab_disabled()
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
        if count > 0 { format!("{} · {count}", t.label()) } else { t.label().to_string() }
    }

    /// The tab strip's current per-tab spans (see [`crate::paint::TabStrip::spans`]),
    /// in [`DRAW_ORDER`]. Used by `app.rs` to compute where the underline
    /// animation (Task 10) should retarget to on a tab switch, without
    /// needing a `Theme` — badge *presence* (Body only) affects a span's
    /// width, but badge color never does, so this stands in a fixed color
    /// where [`Self::draw_tab_bar`] uses the real validity color.
    pub fn tab_strip_spans(&self) -> Vec<(u16, u16)> {
        let labels: Vec<(String, Option<(&'static str, ratatui::style::Color)>)> = DRAW_ORDER
            .iter()
            .map(|t| {
                let label = self.tab_label_text(*t);
                let badge = (matches!(t, EditorTab::Body) && self.body_badge_present())
                    .then_some(("_", ratatui::style::Color::Reset));
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
            body.lines.iter_row().nth(row).map(|l| l.to_vec()).unwrap_or_default()
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
        if !area.contains(Position { x: m.column, y: m.row }) {
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
            // following drag will extend from. (On a double click the
            // caller re-selects the word right after, via
            // `body_select_word_at`.)
            self.body.selection = None;
            self.body_sel_anchor = Some(self.body.cursor);
            self.body_word_anchor = None;
            self.body_line_anchor = None;
        }
        // Modeless invariant: whatever edtui's own handling did (its mouse
        // paths switch to Normal/Visual), the body editor never leaves
        // Insert.
        self.body.mode = EditorMode::Insert;
        true
    }

    /// Double-click word select (spec 2026-10-14, align the text mouse):
    /// the shared mouse word under the click, a click past a line's end
    /// taking its last word; on a bracket (or blanks before one), the block
    /// to its match, across lines. Plants the clicked word as the anchor a
    /// following drag extends from, whole words at a time. An empty line
    /// only places the caret, which the preceding [`Self::handle_mouse`]
    /// already did; a click below the buffer counts as on the last line's
    /// end, as a single click does.
    pub fn body_select_word_at(&mut self, x: u16, y: u16) -> bool {
        if self.active_tab != EditorTab::Body {
            return false;
        }
        let Some(cursor) = self.body_cursor_for_click(x, y) else {
            return false;
        };
        let at = crate::text::Pos::new(cursor.row, cursor.col);
        let Some(click) = crate::text::mouse::double_click(&self.body.lines, at) else {
            return false;
        };
        self.select_body_span(click);
        self.body_word_anchor = Some(click);
        self.body_line_anchor = None;
        true
    }

    /// A mouse gesture's selection, the cells of `span` set whole, shown as
    /// a GUI selection (`text::mouse::Span::gui_ends`): the caret on the
    /// moving end, and the anchor a shift motion extends from on the fixed
    /// end's outer edge.
    fn select_body_span(&mut self, span: crate::text::mouse::Span) {
        let cell = |p: crate::text::Pos| edtui::Index2::new(p.row, p.col);
        self.set_body_selection_cells(cell(span.fixed), cell(span.moving));
        // The edge after a cell, which on an empty row is its only edge.
        let lines = &self.body.lines;
        let (anchor, caret) = span.gui_ends(|p| {
            crate::text::Pos::new(p.row, (p.col + 1).min(lines.len_col(p.row).unwrap_or(0)))
        });
        self.body.cursor = cell(caret);
        self.body_sel_anchor = Some(cell(anchor));
    }

    /// Triple-click line select: the clicked line's text without its break
    /// (the GUI convention), planting the row a following drag extends
    /// from, whole lines at a time.
    pub fn body_select_line_at(&mut self, x: u16, y: u16) -> bool {
        if self.active_tab != EditorTab::Body {
            return false;
        }
        let Some(cursor) = self.body_cursor_for_click(x, y) else {
            return false;
        };
        self.select_body_lines(cursor.row, cursor.row);
        self.body_line_anchor = Some(cursor.row);
        self.body_word_anchor = None;
        true
    }

    /// Selects rows `from` through `to` whole (either order; `to` is the
    /// moving end, where the caret goes), without the last row's break. An
    /// empty single row selects nothing.
    fn select_body_lines(&mut self, from: usize, to: usize) {
        use crate::text::{Pos, mouse::Span};
        let (top, bottom) = (from.min(to), from.max(to));
        let end = self.body.lines.len_col(bottom).unwrap_or(0);
        let (first, last) = (Pos::new(top, 0), Pos::new(bottom, end.saturating_sub(1)));
        if top == bottom && end == 0 {
            self.body.selection = None;
            self.body.cursor = edtui::Index2::new(top, 0);
            self.body_sel_anchor = Some(self.body.cursor);
        } else if to >= from {
            self.select_body_span(Span { fixed: first, moving: last });
        } else {
            self.select_body_span(Span { fixed: last, moving: first });
        }
    }

    /// The char at screen cell `(x, y)` in the body, as `(row, col)`: a
    /// press's (inside the drawn body, with a click's own mapping) or a
    /// sweep's (anywhere, with a sweep's). `None` off the Body tab.
    pub(crate) fn body_char_at(&self, x: u16, y: u16, drag: bool) -> Option<(usize, usize)> {
        if self.active_tab != EditorTab::Body {
            return None;
        }
        let at = if drag {
            self.body_cursor_for_drag(x, y)?
        } else {
            let area = self.last_body_area?;
            if !area.contains(ratatui::layout::Position { x, y }) {
                return None;
            }
            self.body_cursor_for_click(x, y)?
        };
        Some((at.row, at.col))
    }

    /// Extends the body selection to the drag point `(x, y)`: the moving
    /// end of a selection anchored by the preceding left click. The head
    /// cell is included in the selection (edtui's inclusive-`end`
    /// semantics) — except at the ragged edges, where the pointer sits on
    /// a boundary: a downward sweep onto a row's first cell ends at the
    /// end of the line above, and an upward sweep past a row's end starts
    /// at the start of the row below. Dragging back onto the anchor cell
    /// collapses it.
    /// Returns `false` only when there is no live anchor (or no rendered
    /// body area) — unlike a click, a sweep that leaves the box keeps
    /// selecting (see [`Self::body_cursor_for_drag`]).
    /// After a double click (word anchor set) the sweep keeps what it
    /// selected and grows by whole words past it
    /// (`text::mouse::grow_by_words`); after a triple click, by whole lines.
    pub fn body_drag_to(&mut self, x: u16, y: u16) -> bool {
        if self.active_tab != EditorTab::Body {
            return false;
        }
        let Some(cursor) = self.body_cursor_for_drag(x, y) else {
            return false;
        };
        if let Some(row) = self.body_line_anchor {
            let last = self.body.lines.len().saturating_sub(1);
            self.select_body_lines(row, cursor.row.min(last));
            return true;
        }
        if let Some(span) = self.body_word_anchor {
            let head = self.clamp_to_cell(cursor);
            let at = crate::text::Pos::new(head.row, head.col);
            let grown = crate::text::mouse::grow_by_words(&self.body.lines, span, at);
            self.select_body_span(grown);
            return true;
        }
        let Some(anchor) = self.body_sel_anchor else {
            return false;
        };
        self.body.cursor = cursor;
        // A drag's head is a boundary at the ragged edges: a downward
        // sweep whose pointer sits on a row's first cell stops at the
        // boundary *before* that char — the selection ends at the end of
        // the line above instead of grabbing the row's first char — and
        // an upward sweep whose pointer sits past a row's end starts at
        // the boundary *after* its last char — the selection starts at
        // the start of the row below instead of grabbing that last char.
        // In between, the head cell is the sweep's leading edge and is
        // included: down-drags take the char under the pointer, up-drags
        // landing on a row start take its first char.
        let up_past_end = cursor.row < anchor.row && {
            let len = self.body.lines.len_col(cursor.row).unwrap_or(0);
            // An empty row has no last char to spuriously grab; keep it
            // in the sweep so its line break stays selected.
            len > 0 && cursor.col >= len
        };
        if up_past_end {
            let row = cursor.row + 1;
            if anchor.row == row && anchor.col == 0 {
                // The anchor caret already sits at that start — only the
                // line break lies inside the sweep: nothing to select.
                self.body.selection = None;
            } else {
                self.set_body_selection_cells(edtui::Index2::new(row, 0), anchor);
            }
        } else if cursor.col == 0 && cursor.row > anchor.row {
            let row = cursor.row - 1;
            let len = self.body.lines.len_col(row).unwrap_or(0);
            if anchor.row == row && anchor.col >= len {
                // The anchor caret already sits at (or past) that end —
                // only the line break lies inside the sweep: nothing to
                // select.
                self.body.selection = None;
            } else {
                // Straight to the cell setter: the head may legitimately
                // land on the anchor's own cell (press on a line's last
                // char, drag just past its break) — a one-char selection,
                // not the never-left-its-cell case `set_body_selection`
                // clears.
                self.set_body_selection_cells(anchor, edtui::Index2::new(row, len));
            }
        } else {
            self.set_body_selection(anchor, cursor);
        }
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
        let (lo, hi) = if (a.row, a.col) <= (b.row, b.col) { (a, b) } else { (b, a) };
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

    /// Pastes `text` into the body at the caret (GUI semantics: a live
    /// selection is replaced first), staying in Insert mode. Line endings
    /// are normalized — `\r\n`/`\r` become `\n` — and inserted as real
    /// line breaks; everything else lands verbatim. Returns `false` when
    /// the Body tab isn't active (nothing to paste into).
    pub fn paste_body(&mut self, text: &str) -> bool {
        use edtui::actions::{Execute, InsertChar, LineBreak};
        if self.active_tab != EditorTab::Body {
            return false;
        }
        self.delete_body_selection();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    LineBreak(1).execute(&mut self.body);
                }
                '\n' => LineBreak(1).execute(&mut self.body),
                _ => InsertChar(c).execute(&mut self.body),
            }
        }
        self.body.mode = EditorMode::Insert;
        true
    }

    /// The selected body text, rows joined with `\n` (the head cell
    /// included). `None` when nothing is selected.
    pub fn body_selected_text(&self) -> Option<String> {
        let sel = self.body.selection.as_ref()?;
        let (start, end) = (sel.start(), sel.end());
        let mut out = String::new();
        for (row, line) in
            self.body.lines.iter_row().enumerate().skip(start.row).take(end.row - start.row + 1)
        {
            if row > start.row {
                out.push('\n');
            }
            let from = if row == start.row { start.col } else { 0 };
            let to = if row == end.row { (end.col + 1).min(line.len()) } else { line.len() };
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
    /// nothing is selected) one level of the body's indent ([`Self::body_indent`])
    /// right or left: Tab / shift+Tab on selected JSON (spec 2026-10-15 R9).
    /// Indent prepends one level, `width` spaces or a `\t` (empty lines are
    /// skipped — indenting them would only plant trailing whitespace);
    /// dedent strips one leading `\t`, or else up to one level of leading
    /// spaces. The selection, its anchor, and the caret ride along with
    /// their lines' shifts, so repeated presses keep working on the same
    /// text.
    fn indent_body_lines(&mut self, right: bool) {
        let (first, last) = match self.body.selection.as_ref() {
            Some(sel) => (sel.start().row, sel.end().row),
            None => (self.body.cursor.row, self.body.cursor.row),
        };
        let indent = self.body_indent();
        let unit: Vec<char> = indent.unit().chars().collect();
        // A dedent strips one leading tab, or up to one level of spaces: a
        // tab body's space-indented line gives back a tab's display width.
        let spaces = match indent {
            Indent::Spaces(n) => n,
            Indent::Tabs => BODY_TAB_WIDTH,
        };
        let mut shifts = vec![0isize; last - first + 1];
        for row in first..=last {
            let Some(line) = self.body.lines.get_mut(edtui::RowIndex::new(row)) else {
                continue;
            };
            shifts[row - first] = if right {
                if line.is_empty() {
                    continue;
                }
                line.splice(0..0, unit.iter().copied());
                unit.len() as isize
            } else {
                let n = if line.first() == Some(&'\t') {
                    1
                } else {
                    line.iter().take(spaces).take_while(|c| **c == ' ').count()
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

    /// Plain Tab in the arrows body (spec 2026-10-15 R8): under spaces,
    /// spaces up to the next multiple of the width by display column; under
    /// tabs, one tab.
    fn insert_body_tab(&mut self) {
        use edtui::actions::{Execute, InsertChar};
        let text = match self.body_indent() {
            Indent::Tabs => "\t".to_string(),
            Indent::Spaces(n) => {
                let at = self.body.cursor;
                let line = self.body.lines.get(edtui::RowIndex::new(at.row));
                let col: usize = line
                    .map(|l| l.iter().take(at.col).map(|&c| body_char_width(c)).sum())
                    .unwrap_or(0);
                " ".repeat(n - col % n)
            }
        };
        for c in text.chars() {
            InsertChar(c).execute(&mut self.body);
        }
        self.body.mode = EditorMode::Insert;
    }

    /// Enter in the arrows body (spec 2026-10-15 R14). After the line's
    /// first non-blank: the new line starts with the line's indent, and the
    /// text that moves down loses its own leading blanks (Vim's
    /// `autoindent`). At or before it (in the indent, or on a blank line):
    /// an empty line opens above, and the line moves down whole with the
    /// caret's column, so repeated Enters leave no trailing blanks.
    fn body_line_break(&mut self) {
        use edtui::actions::{Execute, LineBreak};
        let blank = |c: &char| *c == ' ' || *c == '\t';
        let at = self.body.cursor;
        let line: Vec<char> =
            self.body.lines.get(edtui::RowIndex::new(at.row)).cloned().unwrap_or_default();
        let lead = line.iter().take_while(|c| blank(c)).count();
        if at.col <= lead {
            // An untouched indent moves down with its line (spec 2026-10-16 R6).
            let carried = self.untouched_indent_end().is_some_and(|end| end.row == at.row);
            // A break at column 0 inserts the empty row above.
            self.body.cursor = edtui::Index2::new(at.row, 0);
            LineBreak(1).execute(&mut self.body);
            self.body.cursor = edtui::Index2::new(at.row + 1, at.col);
            if let Some(mark) = self.auto_indent.as_mut().filter(|_| carried) {
                mark.row += 1;
                mark.lines += 1;
            }
        } else {
            LineBreak(1).execute(&mut self.body);
            let row = self.body.cursor.row;
            let lines = self.body.lines.len();
            if let Some(moved) = self.body.lines.get_mut(edtui::RowIndex::new(row)) {
                let strip = moved.iter().take_while(|c| blank(c)).count();
                moved.drain(..strip);
                moved.splice(0..0, line[..lead].iter().copied());
                // Nothing followed the indent down: it is untouched (R1).
                if lead > 0 && moved.len() == lead {
                    self.auto_indent =
                        Some(AutoIndent { row, indent: line[..lead].to_vec(), lines });
                }
            }
            self.body.cursor = edtui::Index2::new(row, lead);
        }
        self.body.mode = EditorMode::Insert;
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
        Some(edtui::Index2::new(last, self.body.lines.len_col(last).unwrap_or(0)))
    }

    /// Like [`Self::body_cursor_for_click`], but drag-tolerant: a sweep
    /// that leaves the box keeps selecting instead of freezing. The
    /// pointer is clamped into the content rect horizontally (the gutter
    /// and anything left of it read as column 0), and a pointer above the
    /// box maps one buffer row above the viewport top per screen row of
    /// overshoot — mirroring the below-the-box case, where the click walk
    /// already continues past the viewport bottom. edtui's render scrolls
    /// the viewport to keep the cursor visible, so each drag event out
    /// the top/bottom also scrolls the view toward the pointer.
    fn body_cursor_for_drag(&self, x: u16, y: u16) -> Option<edtui::Index2> {
        let area = self.last_body_area?;
        let gutter = line_number_gutter_width(self.body.lines.len());
        let content_x = area.x.saturating_add(gutter);
        let width = usize::from(area.width.saturating_sub(gutter));
        if width == 0 {
            return None;
        }
        let x = x.max(content_x);
        if y >= area.y {
            return self.body_cursor_for_click(x, y);
        }
        let overshoot = usize::from(area.y - y);
        let top = self.body.viewport_offset().1;
        let row = top.saturating_sub(overshoot);
        let line = self.body.lines.iter_row().nth(row)?;
        let segments = wrap_segments(line, width);
        let col = column_in_wrapped_line(line, &segments, 0, usize::from(x - content_x));
        Some(edtui::Index2::new(row, col))
    }
}

/// Byte offset of char index `idx` in `text` (the end offset when `idx` is
/// past the last char) — `find_tokens`'s spans are byte ranges, while both
/// caret positions postui tracks are char indices.
fn char_byte_offset(text: &str, idx: usize) -> usize {
    text.char_indices().nth(idx).map(|(b, _)| b).unwrap_or(text.len())
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

/// One highlight the body paints over its JSON colours in the vim profile
/// (plan M3a Deviation 16): the Visual span, the search match Enter would
/// land on, then the other matches. edtui styles a cell with the first
/// highlight that contains it, so this order is the precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Overlay {
    from: edtui::Index2,
    to: edtui::Index2,
    kind: OverlayKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayKind {
    Visual,
    Landing,
    Match,
}

impl OverlayKind {
    fn style(self, theme: &Theme) -> Style {
        // The response viewer paints its search matches the same way
        // (`highlighted`).
        let found = Style::default().fg(theme.text).bg(theme.page).add_modifier(Modifier::REVERSED);
        match self {
            OverlayKind::Visual => Style::default().bg(theme.selection).fg(theme.text),
            OverlayKind::Landing => found.add_modifier(Modifier::BOLD),
            OverlayKind::Match => found,
        }
    }
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
        // On the resting stops (the method badge, the selected URL line,
        // the table, the blurred pane) a named key acts bare only: any
        // other chord does nothing rather than act as the bare key (piece
        // 4 spec §5.1). Open text fields and the body keep their chords.
        let plain = ev.modifiers.is_empty();
        match self.sub_focus {
            // The method badge: a plain control, not a text input, so
            // Enter/Space activate it (opening the chooser) and arrows only
            // navigate. alt+m still cycles the method without focusing it.
            // Vim's `l`/`j` reach here as Right/Down: the router replays
            // them (piece 4 spec §4.5).
            SubFocus::Method => match ev.code {
                KeyCode::Enter | KeyCode::Char(' ') if plain => Some(Action::OpenMethodDropdown),
                KeyCode::Right if plain => {
                    self.select_url();
                    Some(Action::Render)
                }
                KeyCode::Down if plain => {
                    self.enter_content();
                    Some(Action::Render)
                }
                KeyCode::Esc => {
                    self.sub_focus = SubFocus::None;
                    Some(Action::Render)
                }
                _ => None,
            },
            // Selected, not open: a resting stop like the method badge —
            // arrows navigate, and Enter/Space open the line so the
            // next key types into it (spec 2026-09-16).
            SubFocus::Url if !self.url_open => match ev.code {
                _ if plain && crate::keys::opens_field(&ev) => {
                    self.open_url();
                    Some(Action::Render)
                }
                KeyCode::Left if plain => {
                    self.sub_focus = SubFocus::Method;
                    Some(Action::Render)
                }
                KeyCode::Down if plain => {
                    self.close_url();
                    self.enter_content();
                    Some(Action::Render)
                }
                KeyCode::Esc => {
                    self.close_url();
                    self.sub_focus = SubFocus::None;
                    Some(Action::Render)
                }
                _ => None,
            },
            SubFocus::Url => {
                // Left with the caret already at the start of the line has
                // no text to move over; it steps out onto the method badge
                // instead (the only keyboard route to it), closed.
                if ev.code == KeyCode::Left && self.url.cursor() == 0 {
                    self.close_url();
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
                    self.close_url();
                    self.enter_content();
                    return Some(Action::Render);
                }
                // Enter and Esc both close the field with the text kept
                // (the field rule): the caret leaves, the edit session
                // ends, and the URL line stays selected — Esc only blurs
                // to `SubFocus::None` from the already-selected state
                // above.
                if matches!(ev.code, KeyCode::Enter | KeyCode::Esc) {
                    self.close_url();
                    return Some(Action::Render);
                }
                None
            }
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
                            return Some(Action::DeleteTableRow(i));
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
                    // climbs out to the URL line (selected, not open)
                    // instead of being a dead end with no keyboard path
                    // back up the chain. Leaving the table also drops its
                    // selection — the mouse click-away path clears it, and
                    // a row that stays lit while keys land in the URL line
                    // misstates focus.
                    if plain && ev.code == KeyCode::Up {
                        self.table.selected = None;
                        self.select_url();
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
                // The emacs caret bytes (ctrl+e → End, ESC b/f → word
                // hops with shift riding along — also what mac terminals
                // send for cmd/option+arrows) fold into the GUI keys the
                // paths below already implement, on every platform; only
                // ^A splits (mac: smart Home; Linux: falls through to
                // select-all).
                let ev = crate::keys::fold_text_nav_bytes(ev, cfg!(target_os = "macos"));
                // GUI-style selection (plan 2026-08-23-text-selection):
                // select-all (ctrl+a on Linux; ctrl+shift+a — either
                // reported spelling — everywhere, cmd+a via the SUPER
                // fold on macOS), shifted motions extend a selection
                // through the same wrap-aware nav path the unshifted keys
                // use, and while a selection is live the editing keys
                // take their desktop meanings (type to replace,
                // Backspace/Delete to remove, Esc to deselect). Plain
                // ctrl+a never survives the fold on macOS — it became
                // Home above (it's the byte cmd+left sends).
                let select_all = match ev.code {
                    KeyCode::Char('a') => {
                        ev.modifiers == KeyModifiers::CONTROL
                            || ev.modifiers == KeyModifiers::CONTROL | KeyModifiers::SHIFT
                    }
                    KeyCode::Char('A') => ev.modifiers == KeyModifiers::CONTROL,
                    _ => false,
                };
                if select_all {
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
                // Tab / shift+tab adjust indentation by the body's indent:
                // with a selection Tab shifts the selected lines a level
                // right instead of replacing them, and shift+tab always
                // shifts left. shift+tab is consumed unconditionally —
                // crossterm's BackTab has no edtui KeyCode conversion (it
                // panics `unimplemented!()`), so it must never reach the
                // handler.
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
                // Plain Tab (spec 2026-10-15 R8): spaces to the next stop of
                // the body's indent, or one tab. edtui's own Tab would insert
                // a literal tab whatever the body uses.
                if ev.code == KeyCode::Tab && ev.modifiers.is_empty() {
                    self.insert_body_tab();
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
                // Enter (spec 2026-10-15 R14): auto-indent, not edtui's bare
                // line break. A selection was already deleted above.
                if ev.code == KeyCode::Enter && ev.modifiers.is_empty() {
                    self.body_line_break();
                    return Some(Action::Render);
                }
                // Esc blurs the buffer (Enter must stay a newline in a
                // multi-line editor); Up climbs out to the URL line only
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
                    self.select_url();
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
                if plain && ev.code == KeyCode::Down {
                    self.select_url();
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
                let rows = self.table_len();
                let computed_extra = match self.active_tab {
                    EditorTab::Headers => {
                        let auto_rows = self.computed_row_count();
                        let divider = if auto_rows > 0 { 1 } else { 0 };
                        auto_rows + divider
                    }
                    // The referenced-vars section: one row per token the
                    // request references, plus its divider.
                    EditorTab::Vars => {
                        let refs = self.referenced_var_names().len() as u16;
                        if refs > 0 { refs + 1 } else { 0 }
                    }
                    _ => 0,
                };
                // Capped to what's left after the fixed address bar and tab
                // bar rows, never the other way around: those two must never
                // be squeezed to make room for a table that wants more
                // height than the pane has.
                let available = inner.height.saturating_sub(CHROME_HEIGHT);
                Constraint::Length((table_height(rows) + computed_extra).min(available))
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
        let tab_bar_height =
            if self.table_collapsed { 1.min(below_bar) } else { TAB_BAR_HEIGHT.min(below_bar) };
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
                Constraint::Length(ADDRESS_BAR_HEIGHT), // the fused address bar's block
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

/// Height of the fused address bar — the first row of `Editor::draw`'s
/// vertical split: the bar's own capped block, plus one row under it.
///
/// The bar used to reserve a blank row above it as well. That existed
/// for a raised control that needed clear air around its bevel; a capped
/// control carries its own margin, since three quarters of each cap row
/// is already the page, so keeping it double-spaced the bar.
///
/// The row *below* is not margin and cannot go the same way: the split
/// control on the tab strip paints its upper eave one row above itself
/// (`SplitControl::paint`), which is this row. Without it the eave lands
/// on the bar's bottom cap and erases twelve columns of it.
pub const ADDRESS_BAR_HEIGHT: u16 = crate::paint::BUTTON_HEIGHT + 1;

/// Columns of padding between the method segment and the URL text, so the
/// text isn't flush against the method button.
const URL_PAD: u16 = 2;
/// Fixed width, in cells, of the `󰆏` copy-URL chip at the URL well's right
/// edge (`" \u{F018F} "` — one glyph column, one padding column each side).
const COPY_CHIP_WIDTH: u16 = 3;
/// Fixed width, in cells, of the `󰌾`/`󰍀` TLS-verification lock at the URL
/// well's left edge (`" 󰌾 "` — a single-cell Nerd Font glyph plus one
/// padding column each side, the copy chip's anatomy).
const LOCK_CHIP_WIDTH: u16 = 3;
/// The scratch-request marker in the URL well's right-edge cluster: the
/// editor holds a request with no file behind it, so its edits are not
/// being kept anywhere. Drawn only while `Editor::slug` is `None`, in
/// `theme.text_muted` — the lock/copy chips' resting tone. Drawn whole or
/// not at all: a well with no room for it keeps its columns for the URL.
const UNSAVED_MARKER: &str = " unsaved ";
/// Lightness lift of the URL well's fill (and so its caps) while the
/// pointer is over it: a slight step — smaller than the focus lift below,
/// and on the same ramp — so pointing at the bar answers before the click
/// without ever being mistaken for "typing lands here".
pub const URL_HOVER_LIFT: f32 = 0.05;
/// Lightness lift of the URL well's fill while it holds keyboard focus.
/// Still clears `control_hover` (focus outshines hover on every control),
/// but a softer step than the method badge's ±0.12 colored-face delta,
/// which reads as a hard jump on the large neutral well.
pub const URL_FOCUS_LIFT: f32 = 0.09;
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
/// underline row having gone with the labels — and one empty pane row
/// under the strip, so the split control's lower half-cell eave (painted
/// on the row below its own) lands on pane surface instead of being cut
/// off at the pane's bottom edge.
pub const COLLAPSED_HEIGHT: u16 = ADDRESS_BAR_HEIGHT + 2;

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
        use crate::paint::{cap, fill, text};
        use crate::theme::{lift_color, mix};

        let theme = ctx.theme;
        // Focus styling must track where keys actually go: the URL only
        // counts as focused while its pane is, or the lifted fill/caret
        // would keep painting after Tab moves the keyboard elsewhere.
        // The fill lift is the resting-stop highlight -- it applies while
        // the line is merely *selected* too, same as the method badge and
        // tab strip get a lifted fill without a caret. The caret itself
        // (and the window scroll that follows it) is a stronger claim: it
        // must track whether the line actually has the caret, not just
        // whether it's the keyboard's current stop (spec 2026-09-16).
        let url_focused = ctx.focused && self.sub_focus == SubFocus::Url;
        let url_caret_live = ctx.focused && self.url_open();
        let method_focused = ctx.focused && self.sub_focus == SubFocus::Method;

        // No margin above: the bar's block starts on `area`'s first row.
        // The caps leave three quarters of the page showing in their own
        // rows, so the bar reads as floating without a blank row over it.
        // One column of inset each side stays -- the pane's focus bar
        // lives in the leftmost column.
        let bar = Rect {
            x: area.x + 1,
            width: area.width.saturating_sub(2),
            // Clamped to the pane: the bar is a fixed three rows, but a
            // pane shorter than that must not hand the guard below a
            // height it can never refuse.
            height: crate::paint::BUTTON_HEIGHT.min(area.height),
            ..area
        };
        // The bar's anatomy is three rows (bevel, text, bevel); a pane too
        // short to hold them (a tiny terminal) draws no bar at all rather
        // than writing rows past the buffer, which ratatui panics on, and
        // rather than registering hits over the panes below it.
        if bar.height < crate::paint::BUTTON_HEIGHT || bar.width == 0 {
            self.last_method_area = None;
            return;
        }

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
        // The bar is three fused segments in one capped block: a quarter
        // row of each segment's own fill above and below its text row,
        // over the page. Fused, so the cap run reads as one bar rather
        // than three buttons -- but it is the same `paint::cap` anatomy
        // every other full-size control in the app is painted with.
        let text_y = bar.y + 1;

        let buf = frame.buffer_mut();
        // Sampled once, from the bar's own top-left, so all three
        // segments cap against the same surface and the run has no seam
        // where the segments meet.
        let bar_surface = cap::backdrop(buf, bar, theme);

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
        // The caps follow the currently shown fill, so the whole segment
        // lifts on hover/focus rather than just its text row.
        cap::capped(buf, method_area, method_fill, bar_surface);
        let method_label = format!("{} ▾", self.method.as_str());
        let label_w = crate::ui::cells(&method_label) as u16;
        let start_x = method_area.x + method_area.width.saturating_sub(label_w) / 2;
        text(buf, start_x, text_y, &method_label, theme.on_accent, method_fill, true);
        hits.register(method_area, crate::hit::Hit::MethodSelector);
        // The dropdown opens just below `anchor.y` (see `modal::draw_popup`),
        // so the anchor is the bar's bottom (cap) row -- not the whole hit
        // target -- or the popup would overlap the badge itself.
        self.last_method_area =
            Some(Rect { y: method_area.y + method_area.height - 1, height: 1, ..method_area });

        // --- URL segment -------------------------------------------------
        // Hover and focus are two steps on one ramp off the resting fill
        // (`URL_HOVER_LIFT` < `URL_FOCUS_LIFT`), both ending clear of
        // `control_hover` at the top: the well is large and dark, so a
        // single step is nearly invisible, while the method badge's full
        // ±0.12 reads as a hard jump across that much area. Focus wins
        // when the pointer is over a focused well — the stronger state
        // must never be dimmed by hovering it. Both ease in through the
        // shared fade timers, same as the badge above.
        let url_hover_fill = lift_color(theme.control, URL_HOVER_LIFT);
        let url_focus_fill = lift_color(theme.control, URL_FOCUS_LIFT);
        // Hovered by geometry, not by hit: the lock, the copy chip and any
        // `{{token}}` in the URL all register on top of `UrlBar`, so
        // `ctx.hovered` stops naming the well the moment the pointer
        // crosses onto one of them. The well *contains* them — it stays
        // lit while the pointer is anywhere inside it, and each chip draws
        // its own hover on top of the lit fill.
        let url_hovered =
            ctx.pointer.is_some_and(|(x, y)| url_area.contains(ratatui::layout::Position { x, y }));
        // The surface the focus lift climbs from — the hover fill when the
        // pointer is on the well. Easing up from `theme.control` instead
        // would drop a lit well back to rest for the first frame of the
        // fade and climb from there: a visible dip under every click,
        // because a click always lands on a well the pointer is already
        // hovering.
        let url_base = if url_hovered {
            mix(theme.control, url_hover_fill, ctx.url_well_hover_t())
        } else {
            theme.control
        };
        let url_fill =
            if url_focused { mix(url_base, url_focus_fill, ctx.focus_t()) } else { url_base };
        cap::capped(buf, url_area, url_fill, bar_surface);
        hits.register(url_area, crate::hit::Hit::UrlBar);
        self.last_url_area = Some(url_area);
        // The copy-URL chip claims a fixed slice at the well's right edge;
        // the text window is narrowed to leave room for it so the two never
        // overlap.
        // The lock + copy chips cluster at the well's right edge; the text
        // window is narrowed to leave room for both so they never overlap.
        let chip_w = COPY_CHIP_WIDTH.min(url_area.width);
        let lock_w = LOCK_CHIP_WIDTH.min(url_area.width.saturating_sub(chip_w));
        // A request with no file behind it looks exactly like a saved one
        // otherwise. All or nothing: a well too narrow to hold the whole
        // word shows no marker rather than a clipped " unsav".
        let marker_w = crate::ui::cells(UNSAVED_MARKER) as u16;
        let unsaved_w = if self.slug.is_none()
            && url_area.width.saturating_sub(chip_w + lock_w) >= marker_w + URL_PAD
        {
            marker_w
        } else {
            0
        };
        // The text is inset URL_PAD columns from the method segment so it
        // isn't flush against the badge.
        let url_text_area = Rect {
            x: url_area.x + URL_PAD.min(url_area.width),
            width: url_area
                .width
                .saturating_sub(URL_PAD)
                .saturating_sub(chip_w)
                .saturating_sub(lock_w)
                .saturating_sub(unsaved_w),
            ..url_area
        };
        let mut url_line = self.url.draw_line_windowed(url_caret_live, theme, url_text_area.width);
        url_line.style = Style::default().bg(url_fill).patch(url_line.style);
        buf.set_line(url_text_area.x, text_y, &url_line, url_text_area.width);
        // Token tinting paints over the text just drawn, and registers its
        // spans on top of `UrlBar` so a click lands on the token.
        crate::components::var_tokens::paint_field_tokens(
            buf,
            Rect::new(url_text_area.x, text_y, url_text_area.width, 1),
            &self.url,
            url_caret_live,
            &self.vars,
            theme,
            hits,
        );
        self.last_url_text_area = Some(Rect { y: text_y, height: 1, ..url_text_area });

        // --- scratch marker ----------------------------------------------
        // Left of the copy chip, which sits left of the lock: the cluster
        // reads lock, copy, `unsaved` from the right edge. No hit: it says
        // something rather than doing something.
        if unsaved_w > 0 {
            let marker_area = Rect {
                x: url_area.x + url_area.width.saturating_sub(lock_w + chip_w + unsaved_w),
                y: text_y,
                width: unsaved_w,
                height: 1,
            };
            fill(buf, marker_area, url_fill);
            text(buf, marker_area.x, text_y, UNSAVED_MARKER, theme.text_muted, url_fill, false);
        }

        // --- TLS lock ----------------------------------------------------
        // The certificate-verification toggle, holding the well's rightmost
        // slot with the copy chip just inside it, styled exactly like it
        // (drawn and
        // registered after the URL text so it wins the hit test over
        // `UrlBar`). Filled lock = verifying, outline lock = `insecure` —
        // the glyphs differ in fill (and, in some fonts, an open shackle on
        // the outline), deliberately without a warning
        // treatment: skipping verification is routine for local
        // development, not an alarm. Click dispatches
        // `Action::ToggleInsecure` through the existing `Hit::FooterChip`
        // routing.
        if lock_w == LOCK_CHIP_WIDTH {
            let lock_area = Rect {
                x: url_area.x + url_area.width.saturating_sub(lock_w),
                y: text_y,
                width: lock_w,
                height: 1,
            };
            let lock_hit = crate::hit::Hit::FooterChip(Action::ToggleInsecure);
            let lock_hovered = ctx.hovered == Some(&lock_hit);
            let lock_bg = if lock_hovered {
                mix(url_fill, theme.tint(theme.accent, url_fill), ctx.hover_t())
            } else {
                url_fill
            };
            // Under an environment force the request's flag isn't what
            // goes out, so the lock shows the forced state and steps back
            // toward the well's fill to read as "not yours to flip here".
            let forced = self.env_tls.is_some();
            let effective_insecure = match self.env_tls {
                Some(postui_core::project::TlsPolicy::Verify) => false,
                Some(postui_core::project::TlsPolicy::Insecure) => true,
                None => self.insecure,
            };
            let lock_fg = if lock_hovered {
                theme.text
            } else if forced {
                mix(theme.text_muted, url_fill, 0.4)
            } else {
                theme.text_muted
            };
            fill(buf, lock_area, lock_bg);
            let label = if effective_insecure {
                crate::glyph::LOCK_OPEN_PILL
            } else {
                crate::glyph::LOCK_PILL
            };
            text(buf, lock_area.x, text_y, label, lock_fg, lock_bg, false);
            hits.register(lock_area, lock_hit);
        }

        // --- copy-URL chip ---------------------------------------------
        // A `Chip`-style tinted pill just inside the lock; hover tints
        // it toward the well's accent-tinted color, blending in over the
        // shared hover fade like every other hovered control here. Drawn
        // (and registered) after the URL text/token painting above, so it
        // sits on top and wins the hit test over `UrlBar` beneath it.
        let chip_area = Rect {
            x: url_area.x + url_area.width.saturating_sub(lock_w + chip_w),
            y: text_y,
            width: chip_w,
            height: 1,
        };
        if chip_area.width > 0 {
            let chip_hovered = ctx.hovered == Some(&crate::hit::Hit::CopyUrl);
            let chip_tinted = theme.tint(theme.accent, url_fill);
            let chip_bg =
                if chip_hovered { mix(url_fill, chip_tinted, ctx.hover_t()) } else { url_fill };
            let chip_fg = if chip_hovered { theme.text } else { theme.text_muted };
            fill(buf, chip_area, chip_bg);
            text(
                buf,
                chip_area.x + chip_area.width / 2,
                chip_area.y,
                crate::glyph::COPY,
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
            let breathe_t = ctx.anims.value_or(crate::anim::AnimKey::SendBreathe, ctx.now, 0.0);
            let face = mix(theme.accent, theme.accent_edge_dark, breathe_t);
            // Hovering a send in flight swaps the label to "Cancel" so the
            // click-to-cancel affordance is discoverable, without touching
            // the pulse/spinner face logic.
            let label =
                if send_hovered { "Cancel".to_string() } else { format!("{glyph} Sending") };
            (label, face, theme.on_accent, true)
        } else if disabled {
            ("Send".to_string(), theme.control, theme.text_disabled, false)
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
        // Capped in every state, disabled included -- a disabled `Button`
        // keeps its caps too, and dropping them here would make Send
        // change shape rather than just colour when it greys out.
        cap::capped(buf, send_area, send_fill, bar_surface);
        let send_label_w = crate::ui::cells(&label) as u16;
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

    /// The key and value of the active tab's table row `i` — what the
    /// row's copy button puts on the clipboard, verbatim as typed
    /// (`{{tokens}}` included). `None` out of range, or on Body, which has
    /// no table.
    pub fn table_row_at(&self, i: usize) -> Option<(&str, &str)> {
        let map = match self.active_tab {
            EditorTab::Params => &self.params,
            EditorTab::Headers => &self.headers,
            EditorTab::Vars => &self.variables,
            EditorTab::Body => return None,
        };
        map.get_index(i).map(|(k, e)| (k.as_str(), e.value.as_str()))
    }

    /// Row `i`'s enabled flag on the active table tab. `true` out of range
    /// or on Body — callers only ask about real rows.
    pub fn table_row_enabled(&self, i: usize) -> bool {
        let map = match self.active_tab {
            EditorTab::Params => &self.params,
            EditorTab::Headers => &self.headers,
            EditorTab::Vars => &self.variables,
            EditorTab::Body => return true,
        };
        map.get_index(i).is_none_or(|(_, e)| e.enabled)
    }

    /// How many rows the active tab's table has (the ghost row's index) —
    /// which is also the whole of the table's geometry, fed to
    /// [`table_height`] by `draw`'s layout pass and `draw_tab_content`'s
    /// actual paint. Every row is one line, cursor or no cursor, so
    /// nothing about the selection or a live edit can change the table's
    /// height; the ghost row is [`table_height`]'s own constant, not a row
    /// counted here.
    pub fn table_len(&self) -> usize {
        match self.active_tab {
            EditorTab::Params => self.params.len(),
            EditorTab::Headers => self.headers.len(),
            EditorTab::Vars => self.variables.len(),
            EditorTab::Body => 0,
        }
    }

    /// What a list verb means on the active table (piece 4 spec §5.3), on
    /// the row under the cursor. The add row takes add and put only; the
    /// Body tab takes nothing.
    pub fn table_verb_action(&self, verb: Verb) -> Option<Action> {
        if self.active_tab == EditorTab::Body {
            return None;
        }
        let len = self.table_len();
        let row = self.table.selected.filter(|&i| i < len);
        match verb {
            Verb::Add => Some(Action::TableAddRow),
            Verb::Delete { count: 0 | 1 } => row.map(Action::DeleteTableRow),
            Verb::Delete { count } => row.map(|start| Action::DeleteTableRows { start, count }),
            Verb::Yank => row.map(Action::YankTableRow),
            Verb::Put { above, count } => {
                let at = match self.table.selected {
                    Some(i) if above => i,
                    Some(i) => i + 1,
                    None => len,
                };
                Some(Action::PutTableRow { at: at.min(len), count })
            }
            _ => None,
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

    /// Whether the live table edit sits on the ghost row — a new row is
    /// mid-add, so "add row" is already underway (the footer hides its
    /// add chip). Editing an existing row doesn't count.
    pub fn adding_row(&self) -> bool {
        let map = match self.active_tab {
            EditorTab::Params => &self.params,
            EditorTab::Headers => &self.headers,
            EditorTab::Vars => &self.variables,
            EditorTab::Body => return false,
        };
        self.table.editing.as_ref().is_some_and(|e| e.row == map.len())
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
        let tab_strip: Vec<(String, Option<(&'static str, ratatui::style::Color)>)> = tabs
            .iter()
            .map(|t| {
                let label = self.tab_label_text(*t);
                let badge = match t {
                    EditorTab::Body if self.body_badge_present() => Some(if self.body_is_valid() {
                        ("✓", theme.success)
                    } else {
                        ("✗", theme.error)
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
        let (static_left, static_width) =
            spans.get(active).map(|(x, w)| (*x as f32, *w as f32)).unwrap_or((0.0, 0.0));
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
                    underline,
                    disabled: self.body_tab_disabled().then(|| EditorTab::Body.draw_position()),
                    right_anchored: 0,
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
            let last_rect = rects[tabs.len() - 1];
            if self.substitute_body {
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

        // --- split control (right-aligned) ---
        // The whole column's two-chip control: "editor only" and
        // "response only" (the lit one restores the split) — and it lives
        // on this fixed row, never repositioning itself mid-click the way
        // buttons on the response header would. No key is painted here:
        // each chip's hover hint names its hide/show toggle's key (spec
        // E12a), and `alt+w`, which steps the split, is in the palette.
        let buf = frame.buffer_mut();
        // `table_collapsed` stays the authority on this pane's own
        // minimized flag, so a standalone render still lights the right
        // chip.
        let split = crate::split::SplitState {
            editor_minimized: self.table_collapsed,
            response_minimized: self.split.response_minimized && !self.table_collapsed,
            ratio: self.split.ratio,
        };
        let hovered = match ctx.hovered {
            Some(crate::hit::Hit::SplitStop(s)) => Some(*s),
            _ => None,
        };
        let control_x = area.right().saturating_sub(crate::paint::SPLIT_CONTROL_WIDTH + 1);
        let rects = crate::paint::SplitControl { state: split, hovered }
            .paint(buf, control_x, area.y, theme);
        for (rect, stop) in rects {
            hits.register(rect, crate::hit::Hit::SplitStop(stop));
        }
    }

    /// Rows the computed-headers section will draw. Shared by the height
    /// math in `draw` and the draw itself so they can never disagree.
    fn computed_row_count(&self) -> u16 {
        self.computed.rows.len() as u16
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

    /// Paints the Body tab's toolbar chip row: `format`/`minify`/
    /// `substitute`/`$EDITOR` chips for the body-only actions that
    /// alt+j/alt+g/alt+s/alt+e already bind, but had no mouse-reachable
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

        let sub_label =
            if self.substitute_body { "substitute {{on}}" } else { "substitute {{off}}" };
        let chips: Vec<(&str, &str, Option<Action>)> = vec![
            ("alt+j", "format", Some(Action::FormatBody)),
            ("alt+g", "minify", Some(Action::MinifyBody)),
            ("alt+s", sub_label, Some(Action::ToggleBodyVars)),
            ("alt+e", "$EDITOR", Some(Action::OpenBodyInEditor)),
            ("alt+k", "clear", Some(Action::BodyClear)),
        ];

        let right_limit = area.x + area.width;
        crate::components::footer::paint_chip_row(
            buf,
            area,
            area.y,
            area.x + 1,
            right_limit,
            &chips,
            theme,
            hits,
            ctx.hovered,
            ctx.hover_t(),
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
                    pointer: ctx.pointer,
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
                    &[],
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
                let rows = self.table_len();
                let table_h = table_height(rows).min(area.height);
                let refs_h = if refs.is_empty() {
                    0
                } else {
                    (refs.len() as u16 + 1).min(area.height.saturating_sub(table_h))
                };
                let refs_area =
                    (refs_h > 0).then(|| Rect { y: area.y + table_h, height: refs_h, ..area });
                let table_area = Rect { height: table_h, ..area };
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                    pointer: ctx.pointer,
                    dragging: ctx.dragging,
                    anims: ctx.anims,
                    now: ctx.now,
                };
                let notes: Vec<Option<RowNote>> = self
                    .variables
                    .keys()
                    .map(|k| self.shadowed.get(k).map(|s| RowNote::Dim(format!("overrides {s}"))))
                    .collect();
                self.table.draw(
                    frame,
                    table_area,
                    &self.variables,
                    &table_ctx,
                    "+ Add variable",
                    hits,
                    &notes,
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
                // The table keeps its own full height first — it's the
                // editable data — and the computed section only gets
                // whatever's left after that, clamped to what it asked
                // for; under real space pressure the auto section shrinks
                // (or disappears) rather than eating into the table. Since
                // the status lines above the table went (spec 2026-10-18
                // R6), the project defaults live only in that section, so
                // they give way too; the pane split brings them back, and
                // #56 is the task that makes the tab scroll to them.
                let auto_rows = self.computed_row_count();
                let rows = self.table_len();
                let table_h = table_height(rows).min(area.height);
                let computed_h = if auto_rows == 0 {
                    0
                } else {
                    (auto_rows + 1).min(area.height.saturating_sub(table_h))
                };
                let computed_area = (computed_h > 0).then(|| Rect {
                    y: area.y + table_h,
                    height: computed_h,
                    ..area
                });
                let table_area = Rect { height: table_h, ..area };
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                    pointer: ctx.pointer,
                    dragging: ctx.dragging,
                    anims: ctx.anims,
                    now: ctx.now,
                };
                // A row naming a header the client sets itself is kept but
                // never sent (spec 2026-10-18 R3a), so it is struck in red.
                self.table.draw(
                    frame,
                    table_area,
                    &self.headers,
                    &table_ctx,
                    "+ Add header",
                    hits,
                    &self.computed.notes,
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
                    let column = Rect { x: area.x + area.width - 1, width: 1, ..area };
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
                // The buffer as drawn, not the request's `body_text`.
                let text_now = self.body.lines.to_string();
                if self.body_hl_text != text_now || self.body_hl_marker != Some(theme.accent) {
                    self.body.set_highlights(json_body_highlights(&self.body.lines, theme));
                    self.body_hl_text = text_now;
                    self.body_hl_marker = Some(theme.accent);
                    self.body_hl_overlays.clear();
                }
                // The vim overlays go first: edtui styles a cell with the
                // first highlight that contains it.
                let overlays = self.body_overlays();
                if self.body_hl_overlays != overlays {
                    // One splice: the JSON colours behind them move once.
                    let stale = self.body_hl_overlays.len().min(self.body.highlights.len());
                    let fresh = overlays
                        .iter()
                        .map(|o| edtui::Highlight::new(o.from, o.to, o.kind.style(theme)));
                    self.body.highlights.splice(..stale, fresh);
                    self.body_hl_overlays = overlays;
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
                // the selection's edge. Hiding must respect the selection
                // fill though: edtui repaints the caret cell with
                // `cursor_style` on top of everything, and the caret rides
                // a drag sweep's moving end — its head cell. A plain
                // `hide_cursor` (= base style) there erases the head
                // cell's highlight (the first char of the first line, on
                // an upward whole-lines sweep).
                if let Some(sel) = self.body.selection.as_ref() {
                    edtui_theme = if sel.contains(&self.body.cursor) {
                        edtui_theme
                            .cursor_style(Style::default().bg(theme.selection).fg(theme.text))
                    } else {
                        edtui_theme.hide_cursor()
                    };
                } else if !focused {
                    edtui_theme = edtui_theme.hide_cursor();
                }
                let view = EditorView::new(&mut self.body)
                    .theme(edtui_theme)
                    .wrap(true)
                    .line_numbers(LineNumbers::Absolute);
                frame.render_widget(view, area);
                // The engine's search prompt covers the bottom row (below).
                let prompt_row =
                    self.body_search.prompt.is_some().then(|| area.bottom().saturating_sub(1));
                if focused {
                    let bottom = prompt_row.unwrap_or(area.bottom());
                    self.paint_bracket_pair(frame.buffer_mut(), area, bottom, theme);
                }
                // Body coverage (spec §7): edtui paints the text itself, so
                // its tokens are found by reading the rendered rows back out
                // of the buffer. A token wrapped across two visual rows is
                // not `{{name}}` on either of them and is therefore skipped
                // — the documented limitation of this approach.
                let buf = frame.buffer_mut();
                // A token under the prompt row gets no hit a click could
                // find (final review M-4).
                for y in area.y..area.bottom() {
                    if Some(y) == prompt_row {
                        continue;
                    }
                    let row = Rect::new(area.x, y, area.width, 1);
                    let text = crate::components::var_tokens::row_text(buf, row);
                    if !text.contains("{{") {
                        continue;
                    }
                    crate::components::var_tokens::paint_var_tokens(
                        buf,
                        row,
                        &text,
                        area.x.into(),
                        &self.vars,
                        theme,
                        hits,
                    );
                }
                // The engine's search prompt on the pane's bottom row (spec
                // §5.6; plan M3a Deviation 16).
                if let Some(line) = &self.body_search.prompt {
                    let y = area.bottom().saturating_sub(1);
                    let row = Rect::new(area.x, y, area.width, 1);
                    let style = Style::default().bg(theme.panel).fg(theme.text);
                    let buf = frame.buffer_mut();
                    buf.set_style(row, style);
                    let text = format!("{}{}", line.dir.delim(), line.text);
                    buf.set_stringn(row.x, y, &text, usize::from(row.width), style);
                    // The cells painted before the caret's grapheme, the
                    // delimiter's included, measured on the painted string
                    // as `set_stringn` segments it.
                    let at = text.len() - line.text.len()
                        + line
                            .text
                            .char_indices()
                            .nth(line.cursor)
                            .map_or(line.text.len(), |c| c.0);
                    let before: usize = crate::ui::graphemes(&text)
                        .take_while(|(b, g, _)| b + g.len() <= at)
                        .map(|(_, _, w)| w)
                        .sum();
                    let x = row.x.saturating_add(u16::try_from(before).unwrap_or(u16::MAX));
                    if x < row.right() {
                        buf[(x, y)].set_style(style.add_modifier(Modifier::REVERSED));
                    }
                }
            }
        }
    }

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
        use crate::ui::cells;
        if area.height == 0 || area.width == 0 {
            return;
        }
        let theme = ctx.theme;
        let dim = Style::default().fg(theme.text_muted);
        let mut y = area.y;
        let max_y = area.y.saturating_add(area.height);

        let prefix = "\u{2500}\u{2500} referenced ";
        let dash_w = area.width.saturating_sub(cells(prefix) as u16);
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("{prefix}{}", "\u{2500}".repeat(dash_w as usize)),
                dim,
            )),
            Rect::new(area.x, y, area.width, 1),
        );
        y += 1;

        let cols = super::table_editor::columns(area.x, area.width);
        let name_w = cols.divider_x.saturating_sub(cols.name_x);
        // From the value column to the pane's last cell.
        let room = area.right().saturating_sub(1).saturating_sub(cols.value_x);
        let buf = frame.buffer_mut();
        for name in names {
            if y >= max_y {
                break;
            }
            let info = self.vars.describe(name);
            let token_piece = format!("{{{{{name}}}}}");
            crate::paint::text(
                buf,
                cols.name_x,
                y,
                crate::ui::take_cells(&token_piece, name_w as usize),
                theme.text_muted,
                theme.page,
                false,
            );
            // Retint the token itself (and register its tooltip hit).
            crate::components::var_tokens::paint_var_tokens(
                buf,
                Rect::new(cols.name_x, y, name_w, 1),
                &token_piece,
                cols.name_x.into(),
                &self.vars,
                theme,
                hits,
            );
            // The value at the value column, its source right-aligned in
            // parentheses; they share the room ([`split_room`]), measured
            // in cells — an environment's display name is free-form.
            let value = info.display_value();
            let label = info.source.label();
            let (value_room, source_room) =
                split_room(room as usize, cells(&value), cells(&label) + 2);
            if source_room > 0 {
                let source = format!("({})", crate::ui::ellipsize(&label, source_room - 2));
                let source_x = area.right().saturating_sub(cells(&source) as u16 + 1);
                crate::paint::text(buf, source_x, y, &source, theme.text_muted, theme.page, false);
            }
            crate::paint::text(
                buf,
                cols.value_x,
                y,
                crate::ui::take_cells(&value, value_room),
                theme.text_muted,
                theme.page,
                false,
            );
            y += 1;
        }
    }

    /// Paints the Headers tab's computed-headers section (spec §6, the
    /// user's #4 complaint: see everything that will actually be sent): a
    /// dim divider, then one dim row per `self.computed.rows` entry. A row
    /// that isn't sent (a default, auto or client row something overrides,
    /// a duplicate default, or a default naming a client-owned header,
    /// which also reads as an error) renders struck through; an unresolved
    /// `{{token}}` is tinted span-level. Each row gets a trailing `󰆏` copy
    /// icon (`Hit::AutoHeaderCopy`, indexed into `rows`), and a row a
    /// request row may take over a `󰏫` after it (`Hit::AutoHeaderOverride`,
    /// task #20); the divider
    /// carries a `👁 reveal`/`hide` toggle (`Hit::AutoHeaderReveal`)
    /// whenever a row has a secret to reveal. Under them, a client-owned
    /// row is a surface (`Hit::AutoHeaderRow`) whose hover lets the footer
    /// say why it isn't sent.
    fn draw_computed_headers(
        &self,
        frame: &mut Frame,
        area: Rect,
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

        // Divider, with the reveal/hide toggle right-aligned onto it when
        // there's a secret to reveal.
        if y < max_y {
            let toggle_label = if self.computed.revealed {
                format!("{} hide", crate::glyph::EYE_OFF)
            } else {
                format!("{} reveal", crate::glyph::EYE)
            };
            let show_toggle = self.computed.has_secret();
            let toggle_w = if show_toggle { crate::ui::cells(&toggle_label) as u16 + 1 } else { 0 };
            let prefix = "\u{2500}\u{2500} auto ";
            let dash_w = area.width.saturating_sub(crate::ui::cells(prefix) as u16 + toggle_w);
            let mut spans =
                vec![Span::styled(format!("{prefix}{}", "\u{2500}".repeat(dash_w as usize)), dim)];
            if show_toggle {
                let hovered = ctx.hovered == Some(&crate::hit::Hit::AutoHeaderReveal);
                let toggle_style = if hovered {
                    Style::default().bg(theme.accent).fg(theme.on_accent)
                } else {
                    Style::default().fg(theme.accent)
                };
                spans.push(Span::styled(format!(" {toggle_label}"), toggle_style));
                let toggle_w = crate::ui::cells(&toggle_label) as u16;
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

        for (i, row) in self.computed.rows.iter().enumerate() {
            if y >= max_y {
                break;
            }
            let suppressed = !row.sent();
            let (name, value) = if self.computed.revealed {
                (&row.header.name, &row.header.value)
            } else {
                (&row.header.display_name, &row.header.display)
            };
            // A default the client-owned rule drops reads as an error, as a
            // typed row naming the same header does, and hovering it puts
            // the reason in the footer (spec 2026-10-18 R3b, task #59):
            // struck and dim alone, it would look overridden by a request
            // row that isn't there.
            let client_owned = client_owned_header(row.unsent).is_some();
            let base = if client_owned { Style::default().fg(theme.error) } else { dim };
            let mut name_style = base;
            // The value draws dim throughout; an unresolved `{{token}}` in
            // it is tinted span-level by `paint_var_tokens` below (Task 12
            // replaces Task 10's whole-value error tint, which coloured the
            // resolved parts of the value red too).
            let mut value_style = base;
            if suppressed {
                name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
                value_style = value_style.add_modifier(Modifier::CROSSED_OUT);
            }
            // The indent is its own span, so a strike starts on the name's
            // first char, not on the blanks in front of it.
            let indent = "  ";
            let name_piece = format!("{name}: ");
            // One plain cell between the value and the pill, so the pill's
            // hover fill never touches the text.
            let value_piece = format!("{value} ");
            // Columns stay usize (the token painter and `register_cols`
            // clip them to the row): a long name or value would wrap a bare
            // `as u16` back over the text.
            let value_col =
                usize::from(area.x) + crate::ui::cells(indent) + crate::ui::cells(&name_piece);
            let pill_col = value_col + crate::ui::cells(&value_piece);
            let pill_style = |hit: crate::hit::Hit| {
                if ctx.hovered == Some(&hit) {
                    Style::default().bg(theme.accent).fg(theme.on_accent)
                } else {
                    Style::default().fg(theme.accent)
                }
            };
            // The pencil, on a row a request row may take over (task #20),
            // goes after the copy pill: it comes and goes, so the copy
            // pill keeps its place on every row. The pills trail the value
            // rather than sit at the pane's edge, so a value too long for
            // the pane pushes both off it (the clip rule of #51); the
            // section is a read-out, and giving its pills a reserved edge
            // would mean cutting values short.
            let pencil_col = pill_col + crate::ui::cells(crate::glyph::COPY_PILL);
            let overridable = row.overriding_row().is_some();
            let mut spans = vec![
                Span::styled(indent, base),
                Span::styled(name_piece, name_style),
                Span::styled(value_piece, value_style),
                Span::styled(
                    crate::glyph::COPY_PILL,
                    pill_style(crate::hit::Hit::AutoHeaderCopy(i)),
                ),
            ];
            if overridable {
                spans.push(Span::styled(
                    crate::glyph::PENCIL_PILL,
                    pill_style(crate::hit::Hit::AutoHeaderOverride(i)),
                ));
            }
            let line = Line::from(spans);
            let row_rect = Rect::new(area.x, y, area.width, 1);
            frame.render_widget(Paragraph::new(line), row_rect);
            // Only a client-owned row has something for the footer to say;
            // every other row stays the bare pane, hover and all.
            if client_owned {
                hits.register(row_rect, crate::hit::Hit::AutoHeaderRow(i));
            }
            // The tokens all lie in the value, so clipping them to the
            // whole row keeps them off the name. A struck (unsent) row's
            // tokens are tinted too, as a disabled table row's are: the
            // tint says whether the token resolves, while the strike is
            // what says the row won't block or reach the send.
            crate::components::var_tokens::paint_var_tokens(
                frame.buffer_mut(),
                row_rect,
                value,
                value_col,
                &self.vars,
                theme,
                hits,
            );
            // Cut off at the right edge, the pill keeps what shows. Its
            // spacer cells are part of the button (the hover fill covers
            // them), so even a one-cell sliver is a live edge of it, as on
            // the response Headers tab (#50).
            hits.register_cols(pill_col..pencil_col, row_rect, crate::hit::Hit::AutoHeaderCopy(i));
            if overridable {
                hits.register_cols(
                    pencil_col..pencil_col + crate::ui::cells(crate::glyph::PENCIL_PILL),
                    row_rect,
                    crate::hit::Hit::AutoHeaderOverride(i),
                );
            }
            y += 1;
        }
    }
}

/// The header a row names when it is kept off the wire because the client
/// owns it (`Host`, `Content-Length`, `Transfer-Encoding`; spec 2026-10-18
/// R3a/R3d), for a typed row and a project default alike: the reason the
/// user has to act on, so the row reads as an error and the footer
/// explains it. A replaced or duplicate row gets none: its strike is the
/// whole story.
fn client_owned_header(unsent: Option<postui_core::prepare::Unsent>) -> Option<&'static str> {
    use postui_core::prepare::Unsent;
    match unsent? {
        Unsent::ClientOwned { header } => Some(header),
        Unsent::Replaced | Unsent::Duplicate => None,
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
        Highlight::new(Index2::new(r, a), Index2::new(r, b), Style::default().fg(color))
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

/// How a Vars-tab referenced row's `room` (in cells) splits between a
/// value `value_w` wide and its `(source)` `source_w` wide, with a 2-cell
/// gap between them: both whole when they fit; otherwise a side no wider
/// than half keeps its width and the other takes the rest, else half each
/// (the value the odd cell). A source left fewer than 3 cells can't show
/// `(x)`, so it is dropped, gap and all, and the value takes the whole
/// room. Returns `(value, source)` budgets.
fn split_room(room: usize, value_w: usize, source_w: usize) -> (usize, usize) {
    let shared = room.saturating_sub(2);
    if value_w + source_w <= shared {
        return (value_w, source_w);
    }
    let half = shared / 2;
    let source = if source_w <= half {
        source_w
    } else if value_w <= shared - half {
        shared - value_w
    } else {
        half
    };
    if source < 3 { (room, 0) } else { (shared - source, source) }
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
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
        assert!(!e.is_dirty());
        assert_eq!(e.sub_focus, SubFocus::Url, "load must not change sub_focus");
        e.handle_key(key(KeyCode::Enter)); // open the selected URL line
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
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
        e.handle_key(key(KeyCode::Enter)); // open the selected URL line
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
        assert_eq!(app.editor.active_tab, EditorTab::Body, "backward wraps to last tab");
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
    fn up_down_walks_url_content_and_back() {
        // The vertical chain is URL -> content; the tab strip is not a
        // stop (alt+←/→ switch tabs from anywhere). Up walks it in reverse
        // (an empty Params table doesn't consume Up, so Editor's fallback
        // applies) and lands on the URL line selected, not open.
        let mut e = Editor::default();
        assert_eq!(e.sub_focus, SubFocus::Url, "default sub_focus starts on the URL line");
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        assert_eq!(e.table.selected, Some(0), "lands on a visible row");
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Url);
        assert!(!e.url_open, "selected, not open");
    }

    #[test]
    fn body_tab_up_returns_to_the_url_line() {
        // Body tab has no table editor to intercept Up at all; from the
        // buffer's top row, Up climbs back out to the URL line.
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Url);
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
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT));
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
        e.restore_caret(&Caret::Body { row: 50, col: 0 });
        let offset = e.body.viewport_offset().1;
        assert!(
            (41..=50).contains(&offset),
            "the restored row (50) sits inside a 10-row viewport at offset {offset}"
        );
    }

    /// Undoing an add removes the row the cursor sits on, so the key is
    /// gone and the index fallback fires. It must land on the last *real*
    /// row, never on the ghost "+ Add" row.
    #[test]
    fn a_restored_cell_cursor_never_parks_on_the_ghost_row() {
        let mut e = Editor {
            active_tab: EditorTab::Headers,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        for key in ["Accept", "X-Debug"] {
            e.headers.insert(key.into(), Entry { value: "1".into(), enabled: true });
        }
        e.restore_caret(&Caret::Cell { row: 2, key: Some("Accept-copy".into()) });
        assert_eq!(
            e.table.selected,
            Some(1),
            "a vanished key falls back to the last real row, not the ghost row at index {}",
            e.headers.len()
        );
    }

    /// The same fallback on an empty table has only the ghost row to sit on.
    #[test]
    fn a_restored_cell_cursor_on_an_emptied_table_sits_at_zero() {
        let mut e = Editor {
            active_tab: EditorTab::Headers,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.restore_caret(&Caret::Cell { row: 3, key: Some("Accept".into()) });
        assert_eq!(e.table.selected, Some(0));
    }

    #[test]
    fn restoring_a_body_cursor_with_no_known_area_still_scrolls_to_the_row() {
        let text: String = (0..100).map(|i| format!("\"l{i}\": {i},\n")).collect();
        let mut e = body_editor(&text);
        e.last_body_area = None;
        e.restore_caret(&Caret::Body { row: 50, col: 0 });
        assert_eq!(e.body.viewport_offset().1, 50, "unknown height: top-align the restored row");
    }

    /// Draws `e` and returns the fg color of the buffer cell holding the
    /// first occurrence of `needle` inside the body area.
    fn body_cell_fg(e: &mut Editor, needle: char) -> ratatui::style::Color {
        body_cell(e, needle).fg
    }

    /// Draws `e` at 120×24, the pane focused or not, and returns the frame.
    fn draw_body(e: &mut Editor, focused: bool) -> ratatui::buffer::Buffer {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Draws `e` and returns the buffer cell holding the first occurrence
    /// of `needle` inside the body area.
    fn body_cell(e: &mut Editor, needle: char) -> ratatui::buffer::Cell {
        let buf = draw_body(e, true);
        let area = e.last_body_area.unwrap();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if buf[(x, y)].symbol() == needle.to_string() {
                    return buf[(x, y)].clone();
                }
            }
        }
        panic!("{needle:?} not found in the body area");
    }

    /// Draws `e`, the pane focused or not, and returns the cell showing
    /// body position (`row`, `col`) of a line too short to wrap.
    fn body_cell_at(
        e: &mut Editor,
        focused: bool,
        row: usize,
        col: usize,
    ) -> ratatui::buffer::Cell {
        let buf = draw_body(e, focused);
        let area = e.last_body_area.unwrap();
        let gutter = line_number_gutter_width(e.body.lines.len());
        let top = e.body.viewport_offset().1;
        let x = area.x + gutter + u16::try_from(col).unwrap();
        let y = area.y + u16::try_from(row - top).unwrap();
        buf[(x, y)].clone()
    }

    fn bracket_lit(cell: &ratatui::buffer::Cell) -> bool {
        let theme = Theme::dark();
        (cell.fg, cell.bg) == (theme.on_accent, theme.accent)
    }

    /// Spec 2026-10-22 (matching brackets) R1, R2: the caret on a bracket,
    /// or just after one, lights it and its partner.
    #[test]
    fn the_caret_on_or_after_a_bracket_lights_the_pair() {
        let mut e = body_editor("{\"a\": [1]}");
        e.body.cursor = edtui::Index2::new(0, 0);
        assert!(bracket_lit(&body_cell_at(&mut e, true, 0, 9)), "the partner `}}`");
        assert!(bracket_lit(&body_cell_at(&mut e, true, 0, 0)), "the caret's `{{`");
        assert!(!bracket_lit(&body_cell_at(&mut e, true, 0, 6)), "an unrelated `[`");
        e.body.cursor = edtui::Index2::new(0, 10);
        assert!(bracket_lit(&body_cell_at(&mut e, true, 0, 0)), "after `}}`, the `{{` lights");
        e.body.cursor = edtui::Index2::new(0, 2);
        assert!(!bracket_lit(&body_cell_at(&mut e, true, 0, 0)), "on `a` nothing lights");
    }

    /// R7: only the focused body lights.
    #[test]
    fn an_unfocused_body_lights_no_bracket() {
        let mut e = body_editor("{\"a\": [1]}");
        e.body.cursor = edtui::Index2::new(0, 0);
        assert!(!bracket_lit(&body_cell_at(&mut e, false, 0, 9)));
    }

    /// R5: a partner below the view lights nothing, the caret's either.
    #[test]
    fn a_partner_out_of_view_lights_nothing() {
        let mut text = String::from("{\n");
        for _ in 0..40 {
            text.push_str("  1,\n");
        }
        text.push('}');
        let mut e = body_editor(&text);
        e.body.cursor = edtui::Index2::new(0, 0);
        assert!(!bracket_lit(&body_cell_at(&mut e, true, 0, 0)));
        // From the `}`, edtui scrolls to it; the `{` is far above.
        e.body.cursor = edtui::Index2::new(41, 0);
        assert!(!bracket_lit(&body_cell_at(&mut e, true, 41, 0)));
        // A short block lights across its rows.
        let mut e = body_editor("{\n  1\n}");
        e.body.cursor = edtui::Index2::new(2, 0);
        assert!(bracket_lit(&body_cell_at(&mut e, true, 0, 0)));
    }

    /// The lit cells of the body area after a focused draw.
    fn lit_body_cells(e: &mut Editor) -> usize {
        let buf = draw_body(e, true);
        let area = e.last_body_area.unwrap();
        (area.y..area.bottom())
            .flat_map(|y| (area.x..area.right()).map(move |x| (x, y)))
            .filter(|&(x, y)| bracket_lit(&buf[(x, y)]))
            .count()
    }

    /// R5 holds on a fresh body's first frame too, whatever edtui scrolls
    /// to: both ends light, or neither.
    #[test]
    fn a_first_frame_never_lights_one_end() {
        let mut text = String::new();
        for row in 0..42 {
            text.push_str(match row {
                36 => "{\n",
                41 => "}\n",
                _ => "1,\n",
            });
        }
        let mut e = body_editor(&text);
        e.body.cursor = edtui::Index2::new(41, 0);
        let mut lit = 0;
        for frame in 0..2 {
            lit = lit_body_cells(&mut e);
            let top = e.body.viewport_offset().1;
            let height = usize::from(e.last_body_area.unwrap().height);
            let shows = |row| (top..top + height).contains(&row);
            let expected = if shows(36) && shows(41) { 2 } else { 0 };
            assert_eq!(lit, expected, "frame {frame}: top {top}");
        }
        assert_eq!(lit, 2, "once edtui has scrolled to the caret, both ends light");
    }

    /// R5: the vim search prompt covers the body's bottom row, so a
    /// partner under it doesn't show and nothing lights.
    #[test]
    fn a_partner_under_the_search_prompt_lights_nothing() {
        // A first draw measures the body's height.
        let mut e = body_editor("{}");
        lit_body_cells(&mut e);
        let height = usize::from(e.last_body_area.unwrap().height);
        // The `}` on the body's last screen row.
        let mut text = String::from("{\n");
        for _ in 0..height - 2 {
            text.push_str("1,\n");
        }
        text.push('}');
        e.set_body_text(&text);
        e.body.cursor = edtui::Index2::new(0, 0);
        assert_eq!(lit_body_cells(&mut e), 2, "both show");
        e.body_search.prompt = Some(crate::vim::engine::SearchLine {
            dir: crate::vim::engine::Dir::Forward,
            text: String::new(),
            cursor: 0,
        });
        assert_eq!(lit_body_cells(&mut e), 0, "the prompt covers the `}}`");
    }

    /// R8: Visual paints over the pair.
    #[test]
    fn visual_paints_over_a_lit_bracket() {
        let theme = Theme::dark();
        let mut e = body_editor("{\"a\": [1]}");
        e.body.cursor = edtui::Index2::new(0, 0);
        e.body_visual = Some((edtui::Index2::new(0, 8), edtui::Index2::new(0, 9)));
        assert_eq!(body_cell_at(&mut e, true, 0, 9).bg, theme.selection);
    }

    /// R2: a vim Normal caret looks only under itself; Insert (edtui's
    /// `Insert`, which arrows always is) also looks before.
    #[test]
    fn a_normal_caret_after_a_bracket_lights_nothing() {
        let mut e = body_editor("[1], 2");
        e.body.mode = EditorMode::Normal;
        e.body.cursor = edtui::Index2::new(0, 3);
        assert!(!bracket_lit(&body_cell_at(&mut e, true, 0, 0)));
        e.body.mode = EditorMode::Insert;
        assert!(bracket_lit(&body_cell_at(&mut e, true, 0, 0)));
    }

    /// The engine's Visual span draws over the JSON colours, survives an
    /// edit that relexes them, and gives them back when it ends: the editor
    /// is the only writer of `body.highlights`.
    #[test]
    fn the_vim_visual_span_draws_over_the_json_colours() {
        let theme = Theme::dark();
        let mut e = body_editor("{\"kzz\": \"vzz\"}");
        e.method = Method::Post;
        assert_eq!(body_cell_fg(&mut e, 'k'), theme.accent, "key colour before Visual");
        // `"kzz"` is columns 1..=5; the span covers `kz` only.
        e.body_visual = Some((edtui::Index2::new(0, 2), edtui::Index2::new(0, 3)));
        let cell = body_cell(&mut e, 'k');
        assert_eq!((cell.fg, cell.bg), (theme.text, theme.selection), "Visual paints over the key");
        assert_eq!(body_cell_fg(&mut e, 'v'), theme.success, "outside the span keeps its colour");
        // An edit relexes the colours; the span stays drawn.
        e.body.lines = Lines::from("{\"kzz\": \"vzzz\"}");
        assert_eq!(body_cell(&mut e, 'k').bg, theme.selection, "the span survives a relex");
        assert_eq!(body_cell_fg(&mut e, 'v'), theme.success);
        e.body_visual = None;
        let cell = body_cell(&mut e, 'k');
        assert_eq!(cell.fg, theme.accent, "the key colour comes back");
        assert_ne!(cell.bg, theme.selection);
    }

    /// edtui repaints the caret cell on top of everything with
    /// `cursor_style` — and the caret rides the moving end of a drag
    /// sweep, i.e. the selection's head cell. A `hide_cursor` (= base
    /// style) caret erased that cell's selection highlight: on an upward
    /// whole-lines drag, the first char of the first line rendered
    /// unselected. The head cell must paint as selected.
    #[test]
    fn drag_head_cell_keeps_its_selection_highlight() {
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let theme = Theme::dark();
        let mut e = body_editor("alpha\nbravo\ncharlie");
        e.method = Method::Post;

        let draw = |e: &mut Editor| {
            let theme = Theme::dark();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
                pointer: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(120, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
            terminal.backend().buffer().clone()
        };

        // First draw records the body area the mouse math maps through.
        draw(&mut e);
        let area = e.last_body_area.unwrap();
        let content_x = area.x + line_number_gutter_width(e.body.lines.len());

        // Press past the end of "charlie", sweep up onto the first char.
        e.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: content_x + 20,
            row: area.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert!(e.body_drag_to(content_x, area.y));
        assert_eq!(
            e.body_selected_text().as_deref(),
            Some("alpha\nbravo\ncharlie"),
            "the selection state itself covers everything"
        );

        let buf = draw(&mut e);
        let first = &buf[(content_x, area.y)];
        assert_eq!(first.symbol(), "a", "expected the first char of line 1");
        assert_eq!(first.bg, theme.selection, "the drag head cell paints with the selection fill");
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
        let h = hls.iter().find(|h| h.start == edtui::Index2::new(1, 2)).expect("key highlight");
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
                pointer: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(120, 20);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
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
        assert!(reversed_cells(&mut e) > 0, "no selection: the caret cell renders reversed");
        e.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::CONTROL));
        // The selection itself renders via the theme's selection *color*,
        // not REVERSED — so with the caret hidden no reversed cell remains.
        assert_eq!(reversed_cells(&mut e), 0, "selection live: the block caret hides");
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
            url_open: true,
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
        let mut e = Editor { sub_focus: SubFocus::Method, ..Editor::default() };
        assert_eq!(e.handle_key(key(KeyCode::Enter)), Some(Action::OpenMethodDropdown));
        assert_eq!(e.handle_key(key(KeyCode::Char(' '))), Some(Action::OpenMethodDropdown));
    }

    #[test]
    fn method_focus_down_lands_in_the_content_and_esc_blurs() {
        let mut e = Editor { sub_focus: SubFocus::Method, ..Editor::default() };
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        assert_eq!(e.table.selected, Some(0), "lands on a visible row");
        e.sub_focus = SubFocus::Method;
        e.handle_key(key(KeyCode::Esc));
        assert_eq!(e.sub_focus, SubFocus::None);
    }

    /// The tab strip is no keyboard stop: plain ←/→ (and h/l, which are
    /// nothing here now) never cycle it from any sub-focus — the strip is
    /// switched by the shared alt+←/→ chord (`Action::CycleTabs`) or a
    /// click.
    #[test]
    fn plain_arrows_never_cycle_the_tab_strip() {
        for stop in [SubFocus::Method, SubFocus::Url, SubFocus::Content, SubFocus::None] {
            for code in [KeyCode::Left, KeyCode::Right, KeyCode::Char('h'), KeyCode::Char('l')] {
                let mut e = Editor { sub_focus: stop, ..Editor::default() };
                let out = e.handle_key(key(code));
                assert!(
                    !matches!(out, Some(Action::EditorTabCycle(_))),
                    "{stop:?} {code:?} -> {out:?}"
                );
            }
        }
    }

    /// The method badge is a resting stop (no text field is live), but the
    /// vim letters are not its keys: the router replays them as arrows
    /// (piece 4 spec §4.5).
    #[test]
    fn the_vim_letters_do_nothing_on_the_method_badge() {
        for letter in ['h', 'l', 'j', 'k'] {
            let mut e = Editor { sub_focus: SubFocus::Method, ..Editor::default() };
            assert_eq!(e.handle_key(key(KeyCode::Char(letter))), None, "{letter}");
            assert_eq!(e.sub_focus, SubFocus::Method, "{letter}");
        }
    }

    #[test]
    fn up_from_the_content_selects_the_url_without_opening_it() {
        // The URL line starts selected by default; Down walks that into
        // the content and Up from the top row walks back.
        let mut ed = Editor::default();
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.sub_focus, SubFocus::Content, "Down moved focus");
        ed.handle_key(key(KeyCode::Up));
        assert_eq!(ed.sub_focus, SubFocus::Url);
        assert!(!ed.plain_keys_type(), "selected, not open");
    }

    /// An `Editor` with the URL line selected but closed: the default
    /// editor walked down into the content and back up.
    fn editor_url_selected() -> Editor {
        let mut e = Editor::default();
        e.handle_key(key(KeyCode::Down));
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Url, "fixture lands on the URL line");
        assert!(!e.plain_keys_type(), "fixture: selected, not open");
        e
    }

    #[test]
    fn enter_or_space_opens_the_selected_url_line_and_i_does_not() {
        let mut ed = editor_url_selected();
        ed.handle_key(key(KeyCode::Char('i')));
        assert!(!ed.plain_keys_type(), "i is the router's, not the line's");
        for opener in [KeyCode::Enter, KeyCode::Char(' ')] {
            let mut ed = editor_url_selected();
            ed.handle_key(key(opener));
            assert!(ed.plain_keys_type(), "{opener:?} should have opened the field");
            ed.handle_key(key(KeyCode::Char('x')));
            assert!(ed.url.text().contains('x'), "{opener:?}: typed text landed");
        }
    }

    #[test]
    fn esc_and_enter_close_the_url_line_back_to_selected_not_blurred() {
        for closer in [KeyCode::Esc, KeyCode::Enter] {
            let mut ed = editor_url_selected();
            ed.handle_key(key(KeyCode::Enter));
            ed.handle_key(key(KeyCode::Char('x')));
            let text_before = ed.url.text().to_string();
            ed.handle_key(key(closer));
            assert_eq!(ed.sub_focus, SubFocus::Url, "{closer:?}: stays on the URL line, selected");
            assert!(!ed.plain_keys_type(), "{closer:?}: field is closed");
            assert_eq!(ed.url.text(), text_before, "{closer:?}: text kept");
        }
    }

    #[test]
    fn esc_from_a_selected_url_line_blurs_and_down_reselects_it() {
        let mut ed = editor_url_selected();
        ed.handle_key(key(KeyCode::Esc)); // blur
        assert_eq!(ed.sub_focus, SubFocus::None);
        ed.handle_key(key(KeyCode::Char('j')));
        assert_eq!(ed.sub_focus, SubFocus::None, "j is the router's, not the pane's");
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.sub_focus, SubFocus::Url);
        assert!(!ed.plain_keys_type(), "lands selected, not open");
    }

    #[test]
    fn descending_from_the_url_line_lands_on_a_visible_selection() {
        // Entering the content must never be an invisible state: on
        // Params/Headers the first row (or the ghost + Add row of an empty
        // table) is selected immediately, so every keyboard stop shows.
        let mut e = Editor::default();
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.sub_focus = SubFocus::Url;
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        assert_eq!(e.table.selected, Some(0), "first row selected on entry");

        let mut e = Editor {
            sub_focus: SubFocus::Url,
            ..Editor::default() // empty params table
        };
        e.open_url();
        e.handle_key(key(KeyCode::Down)); // from the open line too
        assert_eq!(e.sub_focus, SubFocus::Content);
        assert_eq!(
            e.table.selected,
            Some(0),
            "an empty table's entry point is its ghost + Add row"
        );
    }

    /// On a table tab the table owns ←/→ while a row is
    /// selected: they move the cell cursor and are consumed, never the tab
    /// strip's cycle. Opening the row then edits the cell the cursor is on.
    #[test]
    fn left_right_in_the_table_move_the_cell_cursor_not_the_tab_strip() {
        let mut e = Editor::default();
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.sub_focus = SubFocus::Url;
        e.handle_key(key(KeyCode::Down));
        assert_eq!((e.sub_focus, e.table.selected), (SubFocus::Content, Some(0)));
        let tab = e.active_tab;
        e.table.col = super::super::table_editor::Col::Key;
        assert_eq!(e.handle_key(key(KeyCode::Right)), Some(Action::Render));
        assert_eq!(e.table.col, super::super::table_editor::Col::Value);
        assert_eq!(e.active_tab, tab, "the tab strip did not cycle");
        e.handle_key(key(KeyCode::Enter));
        let edit = e.table.editing.as_ref().expect("the value cell opened");
        assert_eq!(edit.col, super::super::table_editor::Col::Value);
    }

    /// Up at the table's top clamp climbs out to the URL line rather than
    /// dying. `k` (the router's) and ctrl+k do nothing here.
    #[test]
    fn up_at_row_zero_climbs_out_and_k_and_ctrl_k_do_not() {
        let mut e = Editor::default();
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.sub_focus = SubFocus::Content;
        e.table.selected = Some(0);
        assert_eq!(e.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)), None);
        assert_eq!(e.sub_focus, SubFocus::Content, "ctrl+k is not a motion");
        assert_eq!(e.handle_key(key(KeyCode::Char('k'))), None);
        assert_eq!(e.sub_focus, SubFocus::Content, "k is not a motion");
        assert_eq!(e.handle_key(key(KeyCode::Up)), Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::Url);
        assert_eq!(e.table.selected, None);
    }

    #[test]
    fn params_tab_up_at_row_zero_returns_to_the_url_line() {
        // A non-empty table still doesn't consume Up at row 0 (the top),
        // so Editor's fallback kicks in even though the table has rows.
        let mut e = Editor::default();
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.params.insert("b".into(), Entry { value: "2".into(), enabled: true });
        e.sub_focus = SubFocus::Content;
        e.table.selected = Some(0);
        let action = e.handle_key(key(KeyCode::Up));
        assert_eq!(action, Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::Url);
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
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.params.insert("b".into(), Entry { value: "2".into(), enabled: true });
        e.sub_focus = SubFocus::Content;
        e.table.selected = Some(1);
        let action = e.handle_key(key(KeyCode::Up));
        assert_eq!(action, Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::Content, "table navigation must not move focus");
        assert_eq!(e.table.selected, Some(0));
    }

    #[test]
    fn duplicate_key_commit_in_params_tab_shows_warning_toast() {
        let mut e = Editor { active_tab: EditorTab::Params, ..Editor::default() };
        for (k, v) in [("a", "1"), ("b", "2")] {
            e.params.insert(k.into(), Entry { value: v.into(), enabled: true });
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
        e.begin_add_row(); // opens the ghost row
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
        assert_eq!(e.sub_focus, SubFocus::Content, "Up/Down navigate inside a multi-line body");
        e.handle_key(key(KeyCode::Up)); // back to row 0, still inside
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.handle_key(key(KeyCode::Up)); // at row 0 → climb out to the URL line
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn save_preserves_invalid_body_verbatim() {
        let mut e = Editor::default();
        e.set_body_text("{ \"in-progress\": ");
        let req = e.current_request();
        let back = HttpRequest::from_toml_str(&req.to_toml_string()).unwrap();
        assert_eq!(back.body, Some(Body::Json { text: "{ \"in-progress\": ".into() }));
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
            .draw(|f| app.toasts.draw(f, f.area(), &theme, &app.anims, std::time::Instant::now()))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("line 2"), "toast must carry the error position: {content}");

        let mut app = App::new_for_test();
        assert!(app.update(Action::MinifyBody), "empty body is a no-op, not an error");
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

    /// Both chip rects of the split control, `[EditorFull, ResponseFull]`,
    /// or a panic when either is missing.
    fn control_rects(hits: &crate::hit::HitMap) -> [Rect; 2] {
        [crate::split::SplitStop::EditorFull, crate::split::SplitStop::ResponseFull].map(|s| {
            hits.rect_of(&Hit::SplitStop(s)).unwrap_or_else(|| panic!("{s:?} chip registered"))
        })
    }

    /// The control is two end chips and nothing else: no middle stop, and
    /// no `alt+w` keycap pill (its key is named by the chips' hover hint).
    #[test]
    fn split_control_registers_two_chips_and_no_keycap_pill() {
        let mut e = Editor::default();
        let (content, hits) = draw_editor(&mut e);
        let [first, second] = control_rects(&hits);
        assert_eq!(first.x + first.width, second.x, "the chips sit flush");
        for stop in [
            crate::split::SplitStop::EditorBig,
            crate::split::SplitStop::Even,
            crate::split::SplitStop::ResponseBig,
        ] {
            assert!(hits.rect_of(&Hit::SplitStop(stop)).is_none(), "{stop:?} has no chip");
        }
        assert!(hits.rect_of(&Hit::FooterChip(Action::CycleSplit)).is_none(), "no keycap pill");
        assert!(!content.contains("+w"), "the tab-bar row names no alt+w: {content}");
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
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
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
                pointer: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(120, 14);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
            let body_rect = hits
                .rect_of(&crate::hit::Hit::EditorTab(EditorTab::Body.draw_position()))
                .expect("body tab hit");
            terminal.backend().buffer()[(body_rect.x + 1, body_rect.y)].fg
        };
        let theme = Theme::dark();
        let mut e = Editor::default();
        assert_eq!(e.method, postui_core::model::Method::Get);
        assert_eq!(draw(&mut e), theme.text_disabled, "GET: Body label in the disabled color");
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
        e.load(Some("a".into()), HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap());
        e.url = LineInput::new("https://x/changed");
        assert!(e.is_dirty());
        let (_, hits) = draw_editor(&mut e);
        assert!(hits.rect_of(&Hit::FooterChip(Action::SaveRequest)).is_none());
        assert!(hits.rect_of(&Hit::FooterChip(Action::DiscardChanges)).is_none());
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenVarPicker { completing: false })).is_none()
        );
        control_rects(&hits); // the split control stays
    }

    /// Hiding hides the controls too: with the table collapsed the tab
    /// labels fade out entirely (settled: not painted at all) and take no
    /// clicks — only the split cluster keeps the row.
    #[test]
    fn hidden_tab_labels_fade_out_and_take_no_clicks() {
        let mut e = Editor { table_collapsed: true, ..Editor::default() };
        let (out, hits) = draw_editor(&mut e);
        assert!(!out.contains("Params"), "tab labels are invisible: {out}");
        assert!(hits.rect_of(&Hit::EditorTab(0)).is_none(), "hidden tabs take no clicks");
        control_rects(&hits); // the split control stays
        assert!(out.contains('\u{2587}'), "the control's chips stay: {out}");
    }

    /// Hiding puts away the tab content, not the request's controls: the
    /// address bar (method / URL / Send) stays fully usable while hidden.
    #[test]
    fn hidden_editor_keeps_its_address_bar() {
        let mut e = Editor { table_collapsed: true, ..Editor::default() };
        e.load(Some("a".into()), HttpRequest::from_toml_str("url = \"https://x/path\"\n").unwrap());
        e.table_collapsed = true;
        let (out, hits) = draw_editor(&mut e);
        assert!(out.contains("https://x/path"), "{out}");
        assert!(hits.rect_of(&Hit::UrlBar).is_some(), "URL stays editable");
        assert!(hits.rect_of(&Hit::SendButton).is_some());
        assert!(hits.rect_of(&Hit::MethodSelector).is_some());
        control_rects(&hits); // the split control stays too
    }

    fn draw_editor_sized(e: &mut Editor, w: u16, h: u16) -> crate::hit::HitMap {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        hits
    }

    /// At exactly its collapsed height the pane keeps one empty row under
    /// the `› show` strip: the split control's lower half-cell eave paints
    /// there, so the control isn't cut off at the pane's bottom edge.
    #[test]
    fn hidden_editor_keeps_a_row_for_the_split_controls_lower_eave() {
        let theme = Theme::dark();
        let mut e = Editor { table_collapsed: true, ..Editor::default() };
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(120, COLLAPSED_HEIGHT)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let rects = control_rects(&hits);
        let strip_y = rects[0].y;
        assert_eq!(strip_y + 1, COLLAPSED_HEIGHT - 1, "one row remains under the strip");
        let cell = terminal.backend().buffer().cell((rects[0].x, strip_y + 1)).unwrap();
        assert_eq!(cell.symbol(), "\u{2580}", "the lower eave lands on the last row");
        assert_eq!(cell.fg, theme.control);
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

        let mut e = Editor { table_collapsed: true, ..Editor::default() };
        let hits = draw_editor_sized(&mut e, 120, COLLAPSED_HEIGHT);
        let url = hits.rect_of(&Hit::UrlBar).expect("URL well drawn");
        assert_eq!(
            (url.y, url.height),
            (expanded_url.y, expanded_url.height),
            "address bar keeps its exact expanded geometry while hidden"
        );
        let [first, ..] = control_rects(&hits);
        assert_eq!(first.y, ADDRESS_BAR_HEIGHT, "control on the strip row");
    }

    /// The split control right-aligns 2 cols in from the pane edge (1
    /// for `pane_surface`, 1 for its own margin) — expanded and hidden
    /// alike, so it never moves as the split changes. Its two chips run
    /// in stop order, the boundary at the top then at the bottom.
    #[test]
    fn the_tab_row_fits_beside_the_split_control_at_the_main_minimum() {
        // The widest everyday tab row: single-digit counts on the three
        // counted tabs, the Body badge, and the "vars" indicator (S5's
        // main-column minimum is sized for it).
        let mut e = Editor { method: postui_core::model::Method::Post, ..Editor::default() };
        for i in 0..9 {
            let entry = || Entry { value: "1".into(), enabled: true };
            e.params.insert(format!("p{i}"), entry());
            e.headers.insert(format!("h{i}"), entry());
            e.variables.insert(format!("v{i}"), entry());
        }
        e.set_body_text("{}");
        e.substitute_body = true;
        let hits = draw_editor_sized(&mut e, crate::layout::MAIN_MIN_COLS, 20);
        let tabs_end = (0..DRAW_ORDER.len())
            .filter_map(|i| hits.rect_of(&Hit::EditorTab(i)))
            .map(|r| r.x + r.width)
            .max()
            .expect("tabs drawn");
        // "vars " sits two cells after the last tab.
        let vars_end = tabs_end + 2 + 5;
        let [first, ..] = control_rects(&hits);
        assert!(
            vars_end < first.x,
            "tabs and vars end at {vars_end}, the control starts at {}",
            first.x
        );
    }

    #[test]
    fn split_control_keeps_its_right_inset_in_every_state() {
        let mut e = Editor::default();
        let (_, hits) = draw_editor(&mut e);
        let [.., last] = control_rects(&hits);
        // The last chip ends one cell shy of the 2-col inset: the
        // control's right end cap takes that cell.
        assert_eq!(last.x + last.width, 120 - 3, "{last:?}");

        let mut e = Editor { table_collapsed: true, ..Editor::default() };
        let hits = draw_editor_sized(&mut e, 120, COLLAPSED_HEIGHT);
        let rects = control_rects(&hits);
        assert_eq!(rects[1].x + rects[1].width, 120 - 3, "{:?}", rects[1]);
        for pair in rects.windows(2) {
            assert!(pair[0].x < pair[1].x, "chips in on-screen order");
        }
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
            let mut e = Editor { active_tab: EditorTab::Body, ..Editor::default() };
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
        let mut e = Editor { active_tab: EditorTab::Params, ..Editor::default() };
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
        assert!(!e.is_scratch_dirty(), "a blank editor holds nothing to lose");
        e.url = LineInput::new("https://x");
        assert!(e.is_scratch_dirty(), "typed content with no file behind it");
        e.load(Some("a".into()), HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap());
        assert!(!e.is_scratch_dirty(), "a loaded request is `is_dirty`'s territory, never this");
    }

    #[test]
    fn body_only_toolbar_chips_are_absent_on_the_params_tab() {
        let mut e = Editor { active_tab: EditorTab::Params, ..Editor::default() };
        let (_, hits) = draw_editor(&mut e);
        assert!(hits.rect_of(&Hit::FooterChip(Action::FormatBody)).is_none());
        assert!(hits.rect_of(&Hit::FooterChip(Action::MinifyBody)).is_none());
        assert!(hits.rect_of(&Hit::FooterChip(Action::ToggleBodyVars)).is_none());
        assert!(hits.rect_of(&Hit::FooterChip(Action::OpenBodyInEditor)).is_none());
    }

    #[test]
    fn body_only_toolbar_chips_appear_on_the_body_tab() {
        let mut e = Editor { active_tab: EditorTab::Body, ..Editor::default() };
        let (content, hits) = draw_editor(&mut e);
        assert!(content.contains("format"), "{content}");
        assert!(content.contains("minify"), "{content}");
        assert!(content.contains("$EDITOR"), "{content}");
        assert!(hits.rect_of(&Hit::FooterChip(Action::FormatBody)).is_some());
        assert!(hits.rect_of(&Hit::FooterChip(Action::MinifyBody)).is_some());
        assert!(hits.rect_of(&Hit::FooterChip(Action::ToggleBodyVars)).is_some());
        assert!(hits.rect_of(&Hit::FooterChip(Action::OpenBodyInEditor)).is_some());
        assert!(
            !content.contains("select all"),
            "the select-all chip is gone (ctrl+a covers it): {content}"
        );
        assert!(content.contains(&format!("{}+e", crate::keys::alt_label())), "{content}");
        assert!(content.contains("clear"), "{content}");
        assert!(hits.rect_of(&Hit::FooterChip(Action::BodyClear)).is_some());
    }

    #[test]
    fn substitute_chip_label_reflects_substitute_body_state() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            substitute_body: false,
            insecure: false,
            ..Editor::default()
        };
        let (off, _) = draw_editor(&mut e);
        assert!(off.contains("{{off}}"), "{off}");

        e.substitute_body = true;
        let (on, _) = draw_editor(&mut e);
        assert!(on.contains("{{on}}"), "{on}");
    }

    #[test]
    fn tls_lock_in_the_address_bar_reflects_insecure_state() {
        let mut e = Editor::default();
        let (locked, hits) = draw_editor(&mut e);
        assert!(locked.contains('\u{F033E}'), "verifying shows a filled lock: {locked}");
        assert!(!locked.contains('\u{F0340}'), "{locked}");
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::ToggleInsecure)).is_some(),
            "the lock is clickable"
        );

        e.insecure = true;
        let (unlocked, hits) = draw_editor(&mut e);
        assert!(unlocked.contains('\u{F0340}'), "insecure shows an outline lock: {unlocked}");
        assert!(!unlocked.contains('\u{F033E}'), "{unlocked}");
        assert!(!unlocked.contains("tls {{"), "the old tab-row chip is gone: {unlocked}");

        // The lock holds the well's rightmost slot, with the copy chip
        // flush against its inner edge.
        let lock = hits.rect_of(&Hit::FooterChip(Action::ToggleInsecure)).unwrap();
        let copy = hits.rect_of(&Hit::CopyUrl).unwrap();
        assert_eq!(
            copy.x + copy.width,
            lock.x,
            "copy chip sits flush against the lock: lock={lock:?} copy={copy:?}"
        );
    }

    #[test]
    fn insecure_round_trips_through_load_and_current_request() {
        let mut e = Editor::default();
        assert!(!e.insecure, "defaults to verifying TLS");
        let req = HttpRequest {
            insecure: true,
            url: "https://self-signed.test".into(),
            ..e.current_request()
        };
        e.load(None, req);
        assert!(e.insecure);
        assert!(e.current_request().insecure);
    }

    #[test]
    fn jq_round_trips_through_load_and_current_request_with_empty_meaning_none() {
        let mut ed = Editor::default();
        let mut req = HttpRequest {
            jq: Some(".a".into()),
            url: "https://self-signed.test".into(),
            ..ed.current_request()
        };
        ed.load(Some("s".into()), req.clone());
        assert_eq!(ed.jq, ".a");
        assert_eq!(ed.current_request().jq, Some(".a".into()));
        ed.jq.clear();
        assert_eq!(ed.current_request().jq, None, "an empty scratch is no filter");
        assert!(ed.is_dirty(), "the scratch is the request's (amendment A2)");
        req.jq = None;
        ed.apply_snapshot(&req);
        assert_eq!(ed.jq, "");
    }

    /// Amendment A5: the file holds the switch as the editor does, so a
    /// file switched off with nothing to filter, or with a hand-written
    /// `jq = ""`, opens clean and saves back as it was read.
    #[test]
    fn a_switched_off_file_with_nothing_to_filter_opens_clean_and_round_trips() {
        let text = "method = \"GET\"\nurl = \"u\"\njq = \"\"\njq_enabled = false\n";
        let read = HttpRequest::from_toml_str(text).unwrap();
        let mut ed = Editor::default();
        ed.load(Some("s".into()), read);
        assert!(!ed.is_dirty(), "opens clean");
        assert!(!ed.jq_enabled);
        let saved = ed.current_request().to_toml_string();
        assert_eq!(HttpRequest::from_toml_str(&saved).unwrap(), ed.current_request());
        assert!(saved.contains("jq_enabled = false") && !saved.contains("jq ="), "{saved}");
    }

    /// Amendment A4: the switch is file content, a plain edit.
    #[test]
    fn jq_enabled_round_trips_and_a_flip_dirties_the_request() {
        let mut ed = Editor::default();
        let req = HttpRequest {
            jq: Some(".a".into()),
            jq_enabled: false,
            url: "https://x".into(),
            ..ed.current_request()
        };
        ed.load(Some("s".into()), req.clone());
        assert!(!ed.jq_enabled);
        assert!(!ed.current_request().jq_enabled);
        assert!(!ed.is_dirty());
        ed.jq_enabled = true;
        assert!(ed.is_dirty(), "switching the filter on is an edit");
        ed.jq_enabled = false;
        assert!(!ed.is_dirty(), "and back is clean again");
        ed.apply_snapshot(&req);
        assert!(!ed.jq_enabled);
    }

    #[test]
    fn changing_the_jq_filter_alone_dirties_the_request() {
        let mut ed = Editor::default();
        ed.load(None, HttpRequest { url: "http://x".into(), ..Default::default() });
        ed.mark_saved();
        ed.jq = ".meta".into();
        assert!(ed.is_dirty(), "amendment A2: the scratch is the request's");
        assert_eq!(ed.current_request().jq.as_deref(), Some(".meta"), "the filter saves");
    }

    #[test]
    fn a_scratch_request_with_only_a_jq_filter_counts_as_dirty() {
        let mut ed = Editor::default();
        assert!(!ed.is_scratch_dirty());
        ed.jq = ".a".into();
        assert!(ed.is_scratch_dirty());
    }

    /// A saved request with two views, `names` and `ids`, the scratch
    /// current.
    fn editor_with_views() -> Editor {
        let mut ed = Editor::default();
        let view = |name: &str, filter: &str| postui_core::model::JqView {
            name: name.into(),
            filter: filter.into(),
            id: 0,
        };
        ed.load(
            Some("r".into()),
            HttpRequest {
                url: "http://x".into(),
                jq_views: vec![view("names", ".items[].name"), view("ids", ".items[].id")],
                ..Default::default()
            },
        );
        ed.mark_saved();
        ed
    }

    #[test]
    fn load_assigns_ids_and_every_jq_change_dirties() {
        let mut ed = editor_with_views();
        let ids: Vec<u64> = ed.jq_views.iter().map(|v| v.id).collect();
        assert_eq!(ids, [1, 2], "load numbers the views");
        ed.jq_pick("names");
        assert!(ed.jq_enabled, "picking switches the filter on");
        assert!(ed.is_dirty(), "amendment A4: picking is an edit");
        ed.mark_saved();
        ed.jq = ".x".into();
        assert!(ed.is_dirty(), "the scratch is the request's even while a view is current");
        ed.mark_saved();
        assert_eq!(ed.jq_filter(), ".items[].name");
        ed.set_jq_filter(".items[].name | length");
        assert!(ed.is_dirty());
        assert!(ed.jq_modified("names"));
        assert!(!ed.jq_modified("ids"));
        assert_eq!(ed.jq, ".x", "the scratch is untouched");
        ed.jq_current = None;
        assert_eq!(ed.jq_filter(), ".x", "the scratch is current again");
        ed.set_jq_filter(".y");
        assert_eq!(ed.jq, ".y", "the bar writes the scratch");
    }

    /// `saved` with the ids a read from disk has (none).
    fn as_from_disk(req: &HttpRequest) -> HttpRequest {
        let mut req = req.clone();
        for view in &mut req.jq_views {
            view.id = 0;
        }
        req
    }

    #[test]
    fn undoing_a_discard_keeps_the_modified_mark_and_revert() {
        // names=1, ids=2; add c (3), delete names, save: saved is [ids, c].
        let mut ed = editor_with_views();
        ed.jq_save_as("c").unwrap();
        ed.jq_delete("names");
        ed.mark_saved();
        // Edit ids, then discard: the reload takes `saved` from memory.
        ed.jq_pick("ids");
        ed.set_jq_filter(".edited");
        assert!(ed.jq_modified("ids"));
        let before_discard = ed.current_request();
        let ids_before: Vec<u64> = before_discard.jq_views.iter().map(|v| v.id).collect();
        let saved = ed.saved.clone().unwrap();
        ed.load(Some("r".into()), saved);
        let ids_after: Vec<u64> = ed.jq_views.iter().map(|v| v.id).collect();
        assert_eq!(ids_after, ids_before, "a request from memory keeps its own ids");
        assert!(!ed.jq_modified("ids"), "discarded");
        // Undoing the discard puts the edited snapshot back, exactly.
        ed.apply_snapshot(&before_discard);
        assert_eq!(ed.current_request(), before_discard);
        assert_eq!(ed.jq_saved_filter("ids"), Some(".items[].id"), "its own saved text");
        assert_eq!(ed.jq_saved_filter("c"), Some(""), "and c's its own");
        assert!(ed.jq_modified("ids"), "the mark is back");
        ed.jq_revert();
        assert_eq!(ed.jq_filter(), ".items[].id", "revert still works");
        ed.jq_save_as("d").unwrap();
        let ids: Vec<u64> = ed.jq_views.iter().map(|v| v.id).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "a new view takes a fresh id: {ids:?}");
    }

    #[test]
    fn a_view_made_since_the_save_under_a_deleted_views_name_has_no_saved_text() {
        let mut ed = editor_with_views();
        ed.jq = ".s".into();
        ed.jq_delete("names");
        ed.jq_save_as("names").unwrap();
        assert_eq!(ed.jq_saved_filter("names"), None, "spec 24: saved since, no saved text");
        assert!(!ed.jq_modified("names"), "no mark");
        ed.jq_revert();
        assert_eq!(ed.jq_filter(), ".s", "revert does nothing");
    }

    #[test]
    fn a_buffer_over_a_request_read_from_disk_has_no_saved_text() {
        // An undo's landing: the editor is replaced, the request read from
        // disk (fresh ids), and an unsaved buffer from before seated on it.
        // Its ids match nothing saved, and matching is by id only (a name
        // can pass to another view): no mark, no revert until the next
        // save. Coarser, never wrong.
        let mut ed = editor_with_views();
        ed.jq_pick("names");
        ed.set_jq_filter(".edited");
        let buffer = ed.current_request();
        let old_ids: Vec<u64> = buffer.jq_views.iter().map(|v| v.id).collect();
        let disk = as_from_disk(ed.saved.as_ref().unwrap());
        let mut ed = ed.fresh();
        ed.load(Some("r".into()), disk);
        let new_ids: Vec<u64> = ed.jq_views.iter().map(|v| v.id).collect();
        assert!(
            new_ids.iter().all(|n| old_ids.iter().all(|o| n > o)),
            "a read from disk numbers past every earlier id: {old_ids:?} then {new_ids:?}"
        );
        ed.apply_snapshot(&buffer);
        assert_eq!(ed.jq_saved_filter("names"), None, "never matched by name");
        assert!(!ed.jq_modified("names"), "no mark");
        ed.jq_revert();
        assert_eq!(ed.jq_filter(), ".edited", "revert does nothing");
        assert!(ed.is_dirty(), "the request's own dirty mark still shows the edit");
    }

    #[test]
    fn a_name_passed_from_a_deleted_view_to_another_never_carries_its_saved_text() {
        // The case a name fallback got wrong: undo back past a save that
        // followed delete-names-then-rename-ids-to-names.
        let mut ed = editor_with_views();
        let before = ed.current_request();
        ed.jq_delete("names");
        ed.jq_rename("ids", "names").unwrap();
        ed.mark_saved();
        ed.apply_snapshot(&before);
        assert_eq!(ed.jq_saved_filter("names"), None, "view 1 was deleted before the save");
        assert_eq!(ed.jq_saved_filter("ids"), Some(".items[].id"), "view 2, by id");
    }

    #[test]
    fn load_renumbers_from_the_counter_when_any_id_is_zero_or_repeated() {
        let mut ed = editor_with_views();
        let mut req = ed.saved.clone().unwrap();
        req.jq_views[1].id = req.jq_views[0].id;
        ed.load(Some("r".into()), req.clone());
        let ids: Vec<u64> = ed.jq_views.iter().map(|v| v.id).collect();
        assert_eq!(ids, [3, 4], "a repeated id: numbered past every earlier one");
        req.jq_views[0].id = 0;
        req.jq_views[1].id = 9;
        ed.load(Some("r".into()), req);
        let ids: Vec<u64> = ed.jq_views.iter().map(|v| v.id).collect();
        assert_eq!(ids, [5, 6], "an id of 0: numbered too");
        assert_eq!(ed.saved.as_ref().unwrap().jq_views[1].id, 6, "saved gets the same ids");
    }

    #[test]
    fn a_fresh_editor_keeps_numbering_past_the_one_it_replaces() {
        let ed = editor_with_views();
        let first: Vec<u64> = ed.jq_views.iter().map(|v| v.id).collect();
        let disk = as_from_disk(ed.saved.as_ref().unwrap());
        let mut next = ed.fresh();
        assert!(next.jq_views.is_empty() && next.saved.is_none(), "blank");
        next.load(Some("r".into()), disk);
        let again: Vec<u64> = next.jq_views.iter().map(|v| v.id).collect();
        assert!(
            again.iter().all(|n| first.iter().all(|o| n > o)),
            "ids never restart across a replacement: {first:?} then {again:?}"
        );
    }

    #[test]
    fn the_filter_falls_back_to_the_scratch_when_the_current_view_is_gone() {
        let mut ed = editor_with_views();
        ed.jq = ".s".into();
        ed.jq_current = Some("gone".into());
        assert_eq!(ed.jq_filter(), ".s");
        assert_eq!(ed.jq_current_name(), None, "named as the scratch too");
        ed.set_jq_filter(".t");
        assert_eq!(ed.jq, ".t");
        assert_eq!(ed.jq_views[0].filter, ".items[].name", "no view is touched");
    }

    #[test]
    fn current_request_carries_the_views_the_scratch_and_the_switch_as_held() {
        let mut ed = editor_with_views();
        ed.jq_pick("ids");
        ed.jq_enabled = false;
        let req = ed.current_request();
        assert_eq!(req.jq_views, ed.jq_views);
        assert_eq!(req.jq_views[1].id, 2, "the ids ride along for undo");
        assert_eq!(req.jq_current.as_deref(), Some("ids"));
        assert_eq!(req.jq, None, "an empty scratch is none");
        assert!(!req.jq_enabled, "off stays off: the switch also governs views");
    }

    #[test]
    fn apply_snapshot_takes_the_views_with_their_ids_and_new_views_never_reuse_one() {
        let mut ed = editor_with_views();
        let mut snap = ed.current_request();
        snap.jq_views[1].id = 9;
        snap.jq_current = Some("ids".into());
        snap.jq = Some(".s".into());
        ed.apply_snapshot(&snap);
        assert_eq!(ed.jq_views[1].id, 9);
        assert_eq!(ed.jq_current.as_deref(), Some("ids"));
        assert_eq!(ed.jq, ".s");
        ed.jq_save_as("more").unwrap();
        assert_eq!(ed.jq_views.last().unwrap().id, 10, "past every id the snapshot held");
    }

    #[test]
    fn revert_restores_only_the_current_views_filter_even_after_a_rename() {
        let mut ed = editor_with_views();
        ed.jq_pick("names");
        ed.set_jq_filter(".changed");
        ed.jq_rename("names", "Names2").unwrap();
        assert_eq!(ed.jq_saved_filter("Names2"), Some(".items[].name"), "matched by id");
        assert!(ed.jq_modified("Names2"));
        ed.jq_revert();
        assert_eq!(ed.jq_filter(), ".items[].name");
        assert_eq!(ed.jq_current.as_deref(), Some("Names2"), "revert keeps the name");
        assert!(!ed.jq_modified("Names2"));
    }

    /// Amendment C11 (superseding A8): with no view current there is no
    /// saved text to go back to, however the typed filter differs from the
    /// file's; the request is still dirty.
    #[test]
    fn the_typed_filter_has_no_saved_text_or_revert() {
        let mut ed = editor_with_views();
        ed.jq = ".s".into();
        ed.mark_saved();
        ed.set_jq_filter(".t");
        assert_eq!(ed.jq_current_saved(), None);
        assert!(!ed.jq_current_modified(), "no ↺");
        assert!(ed.is_dirty(), "still an edit to the request");
        ed.jq_revert();
        assert_eq!(ed.jq, ".t", "revert does nothing");
    }

    #[test]
    fn save_as_from_a_modified_view_moves_the_edits() {
        let mut ed = editor_with_views();
        ed.jq_pick("names");
        ed.set_jq_filter(".items[].name | length");
        ed.jq_save_as("name count").unwrap();
        assert_eq!(ed.jq_current.as_deref(), Some("name count"));
        assert_eq!(ed.jq_filter(), ".items[].name | length");
        assert_eq!(ed.jq_saved_filter("names"), Some(".items[].name"));
        assert!(!ed.jq_modified("names"), "the source went back to its saved text");
        assert!(!ed.jq_modified("name count"), "a never-saved view has no saved text");
        assert_eq!(ed.jq_saved_filter("name count"), None);
    }

    #[test]
    fn save_as_from_an_unedited_view_is_a_copy() {
        let mut ed = editor_with_views();
        ed.jq_pick("ids");
        ed.jq_save_as("ids again").unwrap();
        assert_eq!(ed.jq_views.len(), 3);
        assert_eq!(ed.jq_views[1].filter, ".items[].id");
        assert_eq!(ed.jq_views[2].filter, ".items[].id");
        assert_eq!(ed.jq_current.as_deref(), Some("ids again"));
    }

    /// Amendment C12: the typed filter moves to the new view, and nothing is
    /// left behind it, whether or not the file had text there.
    #[test]
    fn save_as_from_no_view_moves_the_text_and_refuses_a_taken_name() {
        let mut ed = editor_with_views();
        ed.jq = ".s".into();
        assert_eq!(
            ed.jq_save_as(" NAMES "),
            Err(ViewNameRefusal::Taken),
            "taken ignoring case and spaces"
        );
        assert_eq!(ed.jq_views.len(), 2, "a refused save adds nothing");
        ed.jq_save_as(" s ").unwrap();
        assert_eq!(ed.jq, "", "nothing left behind the view");
        assert_eq!(ed.jq_views.last().unwrap().filter, ".s");
        assert_eq!(ed.jq_views.last().unwrap().name, "s", "the name is trimmed");
        assert_eq!(ed.jq_current.as_deref(), Some("s"));
        assert!(ed.is_dirty(), "a new view is a request edit");

        let mut never = Editor { jq: ".a".into(), ..Editor::default() };
        never.jq_save_as("v").unwrap();
        assert_eq!(never.jq, "", "a never-saved request leaves nothing behind either");
        assert_eq!(never.jq_views[0].filter, ".a");

        let mut off = editor_with_views();
        off.jq = ".s".into();
        off.jq_enabled = false;
        off.jq_save_as("shown").unwrap();
        assert!(off.jq_enabled, "the new view is switched on, as a pick is");
    }

    #[test]
    fn rename_refuses_another_views_name_but_not_its_own() {
        use ViewNameRefusal::{Blank, Taken};
        let mut ed = editor_with_views();
        assert_eq!(ed.jq_name_refusal("IDS", None), Some(Taken));
        assert_eq!(
            ed.jq_name_refusal("ids", Some("ids")),
            None,
            "a view never collides with itself"
        );
        assert_eq!(ed.jq_name_refusal("other", None), None);
        assert_eq!(ed.jq_name_refusal("no view", None), None, "amendment C9: none reserved");
        assert_eq!(ed.jq_save_as("  "), Err(Blank), "the editor refuses a blank name itself");
        assert_eq!(ed.jq_rename("names", ""), Err(Blank));
        assert_eq!(ed.jq_name_refusal("x\ty", None), Some(ViewNameRefusal::Control));
        assert_eq!(ed.jq_name_refusal("pasted\n", None), None, "trimmed as it is stored");
        ed.jq_save_as("ärger").unwrap();
        assert_eq!(ed.jq_name_refusal("Ärger", None), Some(Taken), "case is compared past ASCII");
        assert!(ed.jq_delete("ärger"));
        assert!(!ed.jq_delete("ärger"), "a name no view has deletes nothing");
        assert_eq!(ed.jq_rename("names", " Ids "), Err(Taken));
        ed.jq_rename("ids", "IDs").unwrap();
        assert_eq!(ed.jq_views[1].name, "IDs", "a case change of its own name is fine");
        assert_eq!(ed.jq_current, None, "renaming a view that is not current picks nothing");
        assert!(ed.is_dirty(), "a rename is a request edit");
    }

    /// A pick of a name no view has any more changes nothing, as a stale
    /// delete does.
    #[test]
    fn a_stale_pick_changes_nothing() {
        let mut ed = editor_with_views();
        ed.jq_pick("ids");
        ed.jq_enabled = false;
        ed.jq_pick("gone");
        assert_eq!(ed.jq_current.as_deref(), Some("ids"));
        assert!(!ed.jq_enabled);
    }

    #[test]
    fn deleting_the_current_view_leaves_no_view_picked() {
        let mut ed = editor_with_views();
        ed.jq = ".s".into();
        ed.jq_pick("ids");
        assert_eq!(ed.jq, "", "amendment C12: the pick replaced the typed filter");
        ed.jq_delete("ids");
        assert_eq!(ed.jq_current, None);
        assert_eq!(ed.jq_views.len(), 1);
        assert_eq!(ed.jq_filter(), "", "no view, empty");
        assert!(ed.is_dirty(), "a delete is a request edit");
    }

    #[test]
    fn a_scratch_editor_with_only_views_or_a_scratch_counts_as_unsaved_work() {
        let mut ed = Editor::default();
        assert!(!ed.is_scratch_dirty());
        ed.jq_save_as("v").unwrap();
        assert!(ed.is_scratch_dirty(), "a view is content");
        ed.jq_delete("v");
        assert!(!ed.is_scratch_dirty());
        ed.jq = ".a".into();
        assert!(ed.is_scratch_dirty(), "a scratch is content");
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
            let mut e = Editor { active_tab: EditorTab::Body, method, ..Editor::default() };
            e.set_body_text(text);
            let theme = Theme::dark();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
                pointer: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(60, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
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
        let mut e = Editor { method: postui_core::model::Method::Post, ..Editor::default() };
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
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Params · 2"), "param count lives inside its tab: {content}");
        assert!(content.contains("Headers · 1"), "header count lives inside its tab: {content}");

        // No standalone count chip left of the split control any more:
        // the cell two columns left of the control (where the chip used
        // to end) must be plain page background.
        let [min, ..] = control_rects(&hits);
        let buf = terminal.backend().buffer();
        let cell = buf.cell((min.x - 2, min.y)).unwrap();
        assert_eq!(cell.bg, theme.page, "no count chip at the strip's right edge: {cell:?}");
    }

    #[test]
    fn body_tab_renders_its_text() {
        let mut e = Editor { active_tab: EditorTab::Body, ..Editor::default() };
        e.set_body_text("{\"marker\": 1}");
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
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
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("marker"), "body text must render: {content}");
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
            let mut e = Editor { active_tab: EditorTab::Body, sub_focus, ..Editor::default() };
            e.set_body_text("{}");
            let ctx = DrawCtx {
                theme: &theme,
                focused: pane_focused,
                hovered: None,
                pointer: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            let backend = TestBackend::new(60, 12);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
            let body_area = e.last_body_area.expect("body area recorded");
            (terminal, body_area)
        };
        let left_edge_cell = |terminal: &Terminal<TestBackend>, body_area: Rect| {
            terminal.backend().buffer().cell((body_area.x - 1, body_area.y + 1)).unwrap().clone()
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
        assert_ne!(left_edge_cell(&terminal, body_area).symbol(), "│", "url sub-focus: no ring");

        let (terminal, body_area) = render(SubFocus::None, true);
        assert_ne!(left_edge_cell(&terminal, body_area).symbol(), "│", "no sub-focus: no ring");
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
            pointer: None,
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
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("POST"), "method badge: {content}");
        assert!(content.contains("https://api.example.com/users"), "url text: {content}");
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
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        (terminal, hits)
    }

    #[test]
    fn method_focus_lifts_the_badge_to_hovers_color() {
        let mut e = Editor { sub_focus: SubFocus::Method, ..Editor::default() };
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let buf = terminal.backend().buffer();
        let method_face = theme.method_color(postui_core::model::Method::Get);
        let cell = buf.cell((method_area.x, method_area.y + 1)).unwrap();
        assert_eq!(
            cell.bg,
            crate::theme::lift_color(method_face, 0.12),
            "the focused method badge lifts to hover's color, so a keyboard \
             user can see where Enter will land"
        );
        // No ring: the margin cell left of the badge stays plain page.
        let margin = buf.cell((method_area.x.saturating_sub(1), method_area.y + 1)).unwrap();
        assert_eq!(margin.symbol(), " ", "no ring glyph beside the badge");
    }

    #[test]
    fn fused_bar_centers_text_between_quarter_row_caps() {
        let mut e = Editor::default();
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x/y""#).unwrap());
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
        assert_eq!(cell.bg, method_face, "method cell bg must be the GET method color");
        // Quarter-row caps above and below, in the segment's own fill
        // over the page -- the flat `paint::cap` anatomy every full-size
        // control in the app shares, and nothing bevelled.
        use crate::paint::cap::{CAP_BOTTOM, CAP_TOP};
        let top_cap = buf.cell((method_area.x, method_area.y)).unwrap();
        assert_eq!(top_cap.symbol(), CAP_TOP, "method top cap: {top_cap:?}");
        assert_eq!(top_cap.fg, method_face, "the cap quarter is the segment");
        assert_eq!(top_cap.bg, theme.page, "the rest of the row is the page");
        let bottom_cap = buf.cell((method_area.x, text_y + 1)).unwrap();
        assert_eq!(bottom_cap.symbol(), CAP_BOTTOM, "method bottom cap: {bottom_cap:?}");
        assert_eq!(bottom_cap.fg, theme.page, "inverted: page over the face");
        assert_eq!(bottom_cap.bg, method_face);
        // The run continues across the URL segment without a seam: same
        // glyph, same surface, only the segment's own fill changes.
        let url_cap = buf.cell((method_area.x + method_area.width + 2, text_y + 1)).unwrap();
        assert_eq!(url_cap.symbol(), CAP_BOTTOM, "url cap row: {url_cap:?}");
        assert_eq!(url_cap.fg, theme.page);
        assert_eq!(url_cap.bg, theme.control);

        // The URL text is drawn on the same row as the method label -- the
        // bar is one fused control, not stacked rows.
        let content = format!("{buf:?}");
        assert!(content.contains("https://x/y"), "url text: {content}");
        let url_row: String =
            (0..60).filter_map(|x| buf.cell((x, text_y)).map(|c| c.symbol())).collect();
        assert!(
            url_row.contains("https://x/y"),
            "url text must share the method label's row: {url_row}"
        );
        // The URL text is inset from the method segment, not flush against it.
        let gap: String = (0..2)
            .filter_map(|dx| {
                buf.cell((method_area.x + method_area.width + dx, text_y)).map(|c| c.symbol())
            })
            .collect();
        assert_eq!(gap, "  ", "2 columns of left padding before the URL text");
    }

    /// A scratch request (no file behind it) was indistinguishable from a
    /// saved one, so unsaved work looked safe. The marker says so, muted,
    /// and vanishes the moment the request has a slug.
    #[test]
    fn a_scratch_editor_shows_the_unsaved_marker_and_a_saved_one_does_not() {
        let theme = Theme::dark();
        let bar_row = |e: &mut Editor| -> (String, Option<ratatui::style::Color>) {
            let (terminal, hits) = draw_for_bar_test(e);
            let bar = hits.rect_of(&crate::hit::Hit::UrlBar).unwrap();
            let text_y = bar.y + 1;
            let buf = terminal.backend().buffer();
            let cells: Vec<String> = (0..buf.area.width)
                .filter_map(|x| buf.cell((x, text_y)).map(|c| c.symbol().to_string()))
                .collect();
            let at = (0..cells.len()).find(|&i| {
                cells[i..].iter().take(7).map(|s| s.as_str()).collect::<String>() == "unsaved"
            });
            let fg = at.and_then(|i| buf.cell((i as u16, text_y)).map(|c| c.fg));
            (cells.concat(), fg)
        };

        let mut scratch = Editor::default();
        scratch.load(None, HttpRequest::from_toml_str(r#"url = "https://x/y""#).unwrap());
        let (row, fg) = bar_row(&mut scratch);
        assert!(row.contains("unsaved"), "a scratch request must be marked: {row}");
        assert_eq!(fg, Some(theme.text_muted), "muted, like the chips beside it");

        let mut saved = Editor::default();
        saved.load(
            Some("main/a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x/y""#).unwrap(),
        );
        let (row, _) = bar_row(&mut saved);
        assert!(
            !row.contains("unsaved"),
            "a request with a file behind it carries no marker: {row}"
        );
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
        draw_for_bar_test_pointing(e, hovered, None, anims, now)
    }

    /// `draw_for_bar_test_at` plus an explicit pointer position, for the
    /// controls that read containment rather than the hit under it.
    fn draw_for_bar_test_pointing(
        e: &mut Editor,
        hovered: Option<&crate::hit::Hit>,
        pointer: Option<(u16, u16)>,
        anims: &crate::anim::Anims,
        now: std::time::Instant,
    ) -> (Terminal<TestBackend>, crate::hit::HitMap) {
        let theme = Theme::dark();
        let ctx =
            DrawCtx { theme: &theme, focused: true, hovered, pointer, dragging: false, anims, now };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
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
        let cell = terminal.backend().buffer().cell((method_area.x, method_area.y + 1)).unwrap();
        assert_ne!(cell.bg, method_face, "must have eased away from the resting face");
        assert_ne!(cell.bg, method_lifted, "must not have reached the fully hovered fill yet");

        // Settled: the fade has finished, so the fill reaches the target
        // exactly — the same value hover always painted before the fade
        // existed.
        let done = t0 + Duration::from_millis(70);
        let (terminal, hits) =
            draw_for_bar_test_at(&mut e, Some(&crate::hit::Hit::MethodSelector), &anims, done);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let cell = terminal.backend().buffer().cell((method_area.x, method_area.y + 1)).unwrap();
        assert_eq!(cell.bg, method_lifted, "settles on the same hover fill as before");
    }

    #[test]
    fn focusing_the_url_well_blends_in_the_focus_lift_over_the_fade() {
        use crate::anim::AnimKey;
        use std::time::Duration;

        let mut e = Editor::default();
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
        e.sub_focus = SubFocus::Url;
        let theme = Theme::dark();
        let focused_fill = crate::theme::lift_color(theme.control, URL_FOCUS_LIFT);

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
        assert_ne!(well.bg, theme.control, "must have eased away from the resting fill");
        assert_ne!(well.bg, focused_fill, "must not have reached the fully focused fill yet");

        let done = t0 + Duration::from_millis(90);
        let (terminal, hits) = draw_for_bar_test_at(&mut e, None, &anims, done);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let url_x = method_area.x + method_area.width;
        let text_y = method_area.y + 1;
        let well = terminal.backend().buffer().cell((url_x, text_y)).unwrap();
        assert_eq!(well.bg, focused_fill, "settles on the same focus lift as before");
    }

    #[test]
    fn send_cap_is_bold_on_accent_when_enabled() {
        let mut e = Editor::default();
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
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
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
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
        assert!(content.contains("Sending"), "sending label must read Sending: {content}");
    }

    /// Hovering the Send cap while a request is in flight swaps its label to
    /// "Cancel" so the click-to-cancel affordance is discoverable, without
    /// disturbing the pulse/spinner face logic.
    #[test]
    fn hovering_send_while_sending_shows_cancel_label() {
        let mut e = Editor::default();
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
        e.sending = true;
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: Some(&crate::hit::Hit::SendButton),
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
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

    /// Loads a request, draws the bar once to find the URL well, and hands
    /// back the editor plus the well's rect — every pointer-driven bar test
    /// needs the geometry before it can point at anything.
    fn editor_with_url_well() -> (Editor, Rect) {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://example.com/a""#).unwrap(),
        );
        let (_, hits) = draw_for_bar_test(&mut e);
        let url_area = hits.rect_of(&crate::hit::Hit::UrlBar).unwrap();
        (e, url_area)
    }

    #[test]
    fn hovering_the_url_well_lifts_it_a_step_short_of_focus() {
        let (mut e, url_area) = editor_with_url_well();
        // Not focused: the hover lift is the well's whole story here.
        e.sub_focus = SubFocus::Method;
        let theme = Theme::dark();
        let anims = crate::anim::Anims::new(false);
        let text_y = url_area.y + 1;
        let (terminal, _) = draw_for_bar_test_pointing(
            &mut e,
            Some(&crate::hit::Hit::UrlBar),
            Some((url_area.x, text_y)),
            &anims,
            std::time::Instant::now(),
        );
        let buf = terminal.backend().buffer();
        let hovered = crate::theme::lift_color(theme.control, URL_HOVER_LIFT);
        let focused = crate::theme::lift_color(theme.control, URL_FOCUS_LIFT);
        let well = buf.cell((url_area.x, text_y)).unwrap();
        assert_eq!(well.bg, hovered, "the hovered URL well lifts: {well:?}");
        assert_ne!(hovered, theme.control, "hover must be visible at all");
        assert_ne!(hovered, focused, "hover must stay short of focus");
        // The caps follow the fill, so the whole segment lifts as one.
        let cap = buf.cell((url_area.x, url_area.y)).unwrap();
        assert_eq!(cap.fg, hovered, "the cap carries the hover fill: {cap:?}");
    }

    #[test]
    fn the_url_well_stays_lit_while_the_pointer_is_on_a_chip_inside_it() {
        let (mut e, url_area) = editor_with_url_well();
        e.sub_focus = SubFocus::Method;
        let theme = Theme::dark();
        let anims = crate::anim::Anims::new(false);
        let text_y = url_area.y + 1;
        // The copy chip sits just inside the lock at the well's right edge,
        // and registers its own hit on top of `UrlBar`.
        let chip_x = url_area.x + url_area.width - LOCK_CHIP_WIDTH - COPY_CHIP_WIDTH + 1;
        let (terminal, hits) = draw_for_bar_test_pointing(
            &mut e,
            Some(&crate::hit::Hit::CopyUrl),
            Some((chip_x, text_y)),
            &anims,
            std::time::Instant::now(),
        );
        assert!(
            hits.rect_of(&crate::hit::Hit::CopyUrl)
                .is_some_and(|r| r.contains(ratatui::layout::Position { x: chip_x, y: text_y })),
            "the test must be pointing at the copy chip itself"
        );
        // Away from the chip, the well's own fill is still the hover lift.
        let well = terminal.backend().buffer().cell((url_area.x, text_y)).unwrap();
        assert_eq!(
            well.bg,
            crate::theme::lift_color(theme.control, URL_HOVER_LIFT),
            "a chip inside the well must not unlight the well: {well:?}"
        );
    }

    #[test]
    fn the_pointer_outside_the_well_leaves_it_at_rest() {
        let (mut e, url_area) = editor_with_url_well();
        e.sub_focus = SubFocus::Method;
        let theme = Theme::dark();
        let anims = crate::anim::Anims::new(false);
        let text_y = url_area.y + 1;
        let (terminal, _) = draw_for_bar_test_pointing(
            &mut e,
            Some(&crate::hit::Hit::MethodSelector),
            Some((url_area.x - 1, text_y)),
            &anims,
            std::time::Instant::now(),
        );
        let well = terminal.backend().buffer().cell((url_area.x, text_y)).unwrap();
        assert_eq!(well.bg, theme.control, "unhovered well rests: {well:?}");
    }

    /// A selected-but-closed URL line (spec 2026-09-16) keeps the fill
    /// lift — it's still the keyboard's resting stop — but must not paint
    /// a live caret: `k` from the tab strip lands here, and a reversed
    /// cell wherever the cursor last sat would be exactly the caret the
    /// spec says a merely-selected field does not have.
    #[test]
    fn a_selected_url_line_keeps_the_fill_lift_but_paints_no_caret() {
        let (mut e, url_area) = editor_with_url_well();
        // `editor_with_url_well` loads a request without touching
        // `sub_focus`/`url_open`, so the line starts selected, matching
        // `k` from the tab strip.
        assert_eq!(e.sub_focus, SubFocus::Url);
        assert!(!e.url_open());
        let theme = Theme::dark();
        let text_y = url_area.y + 1;
        let (terminal, _) = draw_for_bar_test(&mut e);
        let buf = terminal.backend().buffer();
        let text_area = e.last_url_text_area.unwrap();
        let reversed_cells = |buf: &ratatui::buffer::Buffer| {
            (text_area.x..text_area.right())
                .filter(|&x| buf[(x, text_y)].modifier.contains(Modifier::REVERSED))
                .count()
        };
        assert_eq!(reversed_cells(buf), 0, "selected, not open: no caret cell should paint");
        let well = buf.cell((url_area.x, text_y)).unwrap();
        assert_eq!(
            well.bg,
            crate::theme::lift_color(theme.control, URL_FOCUS_LIFT),
            "the resting-stop fill lift still applies while merely selected: {well:?}"
        );

        // Opening the line (Enter/Space, vim's `i`, or `Action::FocusUrl`)
        // puts the caret live, and the reversed cell shows up.
        e.open_url_from_app();
        let (terminal2, _) = draw_for_bar_test(&mut e);
        let buf2 = terminal2.backend().buffer();
        assert!(reversed_cells(buf2) > 0, "open: the caret cell renders reversed");
    }

    #[test]
    fn clicking_a_hovered_url_well_climbs_from_the_hover_fill_never_dipping() {
        use crate::anim::AnimKey;
        use std::time::Duration;

        let (mut e, url_area) = editor_with_url_well();
        // What a click leaves behind: focus taken, focus fade just started,
        // pointer still on the well.
        e.sub_focus = SubFocus::Url;
        let theme = Theme::dark();
        let text_y = url_area.y + 1;
        let hover_fill = crate::theme::lift_color(theme.control, URL_HOVER_LIFT);
        let focus_fill = crate::theme::lift_color(theme.control, URL_FOCUS_LIFT);

        let mut anims = crate::anim::Anims::new(true);
        let t0 = std::time::Instant::now();
        anims.snap(AnimKey::FocusFade, 0.0);
        anims.retarget(AnimKey::FocusFade, 1.0, Duration::from_millis(90), t0);
        let pointer = Some((url_area.x, text_y));

        // First frame of the fade: still exactly the hover fill it was at,
        // not a drop back to the resting control fill.
        let (terminal, _) =
            draw_for_bar_test_pointing(&mut e, Some(&crate::hit::Hit::UrlBar), pointer, &anims, t0);
        let well = terminal.backend().buffer().cell((url_area.x, text_y)).unwrap();
        assert_eq!(
            well.bg, hover_fill,
            "the click must not dip below the fill it was already at: {well:?}"
        );

        // And it lands on the focus fill when the fade completes.
        let (terminal, _) = draw_for_bar_test_pointing(
            &mut e,
            Some(&crate::hit::Hit::UrlBar),
            pointer,
            &anims,
            t0 + Duration::from_millis(90),
        );
        let well = terminal.backend().buffer().cell((url_area.x, text_y)).unwrap();
        assert_eq!(well.bg, focus_fill, "settles on the focus lift: {well:?}");
    }

    #[test]
    fn a_focused_url_well_is_not_dimmed_by_hovering_it() {
        let (mut e, url_area) = editor_with_url_well();
        e.sub_focus = SubFocus::Url;
        let theme = Theme::dark();
        let anims = crate::anim::Anims::new(false);
        let text_y = url_area.y + 1;
        let (terminal, _) = draw_for_bar_test_pointing(
            &mut e,
            Some(&crate::hit::Hit::UrlBar),
            Some((url_area.x, text_y)),
            &anims,
            std::time::Instant::now(),
        );
        let well = terminal.backend().buffer().cell((url_area.x, text_y)).unwrap();
        assert_eq!(
            well.bg,
            crate::theme::lift_color(theme.control, URL_FOCUS_LIFT),
            "focus wins over hover: {well:?}"
        );
    }

    #[test]
    fn focused_url_lifts_the_url_fill_and_caps() {
        let mut e = Editor::default();
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
        e.sub_focus = SubFocus::Url;
        let theme = Theme::dark();
        let (terminal, hits) = draw_for_bar_test(&mut e);
        let method_area = hits.rect_of(&crate::hit::Hit::MethodSelector).unwrap();
        let buf = terminal.backend().buffer();
        let text_y = method_area.y + 1;
        let url_x = method_area.x + method_area.width;
        let lifted = crate::theme::lift_color(theme.control, URL_FOCUS_LIFT);
        let well = buf.cell((url_x, text_y)).unwrap();
        assert_eq!(well.bg, lifted, "the focused URL well brightens past control_hover: {well:?}");
        assert_ne!(lifted, theme.control_hover, "focus must outshine hover");
        // The cap *is* the lifted fill -- a quarter row of the well
        // itself over the page, so the whole segment lifts as one. There
        // is no separate edge colour to keep in step any more.
        let cap = buf.cell((url_x, method_area.y)).unwrap();
        assert_eq!(cap.symbol(), crate::paint::cap::CAP_TOP, "url top cap: {cap:?}");
        assert_eq!(cap.fg, lifted, "the cap carries the lifted fill");
        assert_eq!(cap.bg, theme.page, "over the page, not over an edge");
        // No ring: the bar's old top-left ring corner cell stays plain.
        let corner =
            buf.cell((method_area.x.saturating_sub(1), method_area.y.saturating_sub(1))).unwrap();
        assert_eq!(corner.symbol(), " ", "no ring glyph around the bar");
    }

    /// Project defaults show once, in the auto section — struck through
    /// when a request row (enabled or disabled) replaces them — with no
    /// status lines above the table (spec 2026-10-18 R6).
    #[test]
    fn headers_tab_shows_defaults_only_in_the_auto_section() {
        let mut app = App::new_for_test();
        let root = app.proj().root().to_path_buf();
        std::fs::write(
            root.join("project.toml"),
            "[default_headers]\naccept = \"application/json\"\nx-a = \"dflt-a\"\nx-b = \"dflt-b\"\n",
        )
        .unwrap();
        app.resync_project();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.headers.insert("X-A".into(), Entry { value: "9".into(), enabled: true });
        app.editor.headers.insert("X-B".into(), Entry { value: "n".into(), enabled: false });
        app.update(Action::Render);

        let mut terminal = Terminal::new(TestBackend::new(100, 70)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let content = format!("{buf:?}");
        for gone in ["(project)", "(overridden)", "(disabled)", "\u{2713} "] {
            assert!(!content.contains(gone), "no status lines ({gone}): {content}");
        }
        assert_eq!(content.matches("application/json").count(), 1, "{content}");
        assert!(!buffer_has_crossed_out(buf, "application/json"), "a sent default");
        assert!(buffer_has_crossed_out(buf, "dflt-a"), "replaced by an enabled row");
        assert!(buffer_has_crossed_out(buf, "dflt-b"), "replaced by a disabled row");
    }

    /// Draws the whole app into a terminal wide enough for the footer's
    /// centre line, and returns the buffer's text.
    fn draw_wide(app: &mut App) -> (Terminal<TestBackend>, String) {
        let mut terminal = Terminal::new(TestBackend::new(180, 70)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        (terminal, content)
    }

    /// A project default naming `Host` is dropped by the same rule as a
    /// typed row, so its auto row reads as an error, struck, and says
    /// nothing inline: hovering the row puts the reason in the footer
    /// (spec 2026-10-18 R3b, amended by task #59).
    #[test]
    fn a_default_host_is_struck_in_red_and_its_hover_says_why() {
        let mut app = App::new_for_test();
        let root = app.proj().root().to_path_buf();
        std::fs::write(root.join("project.toml"), "[default_headers]\nHost = \"internal.test\"\n")
            .unwrap();
        app.resync_project();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/foo");
        app.update(Action::Render);

        let (terminal, content) = draw_wide(&mut app);
        let buf = terminal.backend().buffer();
        assert!(buffer_has_crossed_out(buf, "internal.test"), "the default is not sent");
        let (vx, vy) = find_text(buf, "internal.test").unwrap();
        assert_eq!(buf[(vx, vy)].fg, app.theme.error, "the row reads as an error");
        assert!(!content.contains("cannot be set manually"), "no inline note: {content}");

        let i = app.editor.computed.rows.iter().position(|r| r.header.value == "internal.test");
        let hit = Hit::AutoHeaderRow(i.expect("the default's auto row"));
        let row = app.hits.rect_of(&hit).expect("the auto row is a hover surface");
        assert_eq!(row.y, vy, "on the default's own row");
        let accept = app.editor.computed.rows.iter().position(|r| r.header.name == "Accept");
        let accept = Hit::AutoHeaderRow(accept.expect("the client's Accept row"));
        assert_eq!(app.hits.rect_of(&accept), None, "an ordinary row stays the bare pane");
        app.hovered = Some(hit);
        let (terminal, content) = draw_wide(&mut app);
        let (mx, my) = find_text(terminal.backend().buffer(), "Host cannot be set manually")
            .unwrap_or_else(|| panic!("the footer says why: {content}"));
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(mx, my)].fg, app.theme.error, "an error message");

        // A click on the row does what one on the bare pane under it did.
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        app.focus = crate::layout::PaneId::Sidebar;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: row.x,
            row: row.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.focus, crate::layout::PaneId::Editor);
    }

    /// Every client-owned name gets the message (`Transfer-Encoding` too),
    /// and a disabled row naming one is just off: it says nothing.
    #[test]
    fn transfer_encoding_speaks_and_a_disabled_host_row_stays_silent() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/foo");
        app.editor
            .headers
            .insert("transfer-encoding".into(), Entry { value: "chunked".into(), enabled: true });
        app.editor.headers.insert("Host".into(), Entry { value: "x.test".into(), enabled: false });
        app.update(Action::Render);
        draw_wide(&mut app);

        app.hovered = Some(Hit::TableCell { row: 0, col: 1 });
        let (_, content) = draw_wide(&mut app);
        assert!(content.contains("Transfer-Encoding cannot be set manually"), "{content}");

        app.hovered = Some(Hit::TableCell { row: 1, col: 1 });
        let (terminal, content) = draw_wide(&mut app);
        assert!(!content.contains("cannot be set manually"), "{content}");
        let key = app.hits.rect_of(&Hit::TableCell { row: 1, col: 0 }).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(key.x, key.y)].fg, app.theme.text_muted, "drawn as any disabled row");
    }

    /// The error never hides a chip (task #59, the user: "just make it
    /// shorter when there is less space so we dont have to hide the
    /// chips"): at every width the footer's chips are the same with the
    /// cursor on a client-owned row as on an ordinary one, and where the
    /// full message doesn't fit a shorter form shows.
    #[test]
    fn the_client_owned_error_shortens_and_never_hides_a_chip() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/foo");
        app.editor
            .headers
            .insert("Content-Length".into(), Entry { value: "5".into(), enabled: true });
        app.editor.headers.insert("X-A".into(), Entry { value: "1".into(), enabled: true });
        app.focus = crate::layout::PaneId::Editor;
        app.editor.sub_focus = SubFocus::Content;
        app.update(Action::Render);
        let chips = ["send", "add header", "disable", "delete", "vars", "tabs", "commands"];
        for width in [90, 110, 140, 180] {
            let mut footer = |row: usize| {
                app.editor.table.selected = Some(row);
                let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
                terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
                let buf = terminal.backend().buffer();
                let y = buf.area.height - 2;
                (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>()
            };
            let (owned, plain) = (footer(0), footer(1));
            for chip in chips {
                assert_eq!(
                    owned.contains(chip),
                    plain.contains(chip),
                    "{width}: {chip}\n{owned}\n{plain}"
                );
            }
            assert!(!owned.contains('…'), "a form shows whole: {owned}");
            if width >= 140 {
                assert!(owned.contains(" be set"), "some form shows, {width}: {owned}");
            }
            if width >= 180 {
                assert!(owned.contains("Content-Length cannot be set manually"), "{owned}");
            }
        }
    }

    /// The footer message follows the keyboard cursor too: on a typed
    /// `Content-Length` row of the focused table it shows, with nothing
    /// hovered, and it goes when the cursor leaves the row.
    #[test]
    fn the_cursor_on_a_client_owned_row_puts_the_reason_in_the_footer() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/foo");
        app.editor
            .headers
            .insert("Content-Length".into(), Entry { value: "5".into(), enabled: true });
        app.editor.headers.insert("X-A".into(), Entry { value: "1".into(), enabled: true });
        app.focus = crate::layout::PaneId::Editor;
        app.editor.sub_focus = SubFocus::Content;
        app.editor.table.selected = Some(0);
        app.update(Action::Render);

        let (terminal, content) = draw_wide(&mut app);
        assert!(content.contains("Content-Length cannot be set manually"), "{content}");
        let buf = terminal.backend().buffer();
        let key = app.hits.rect_of(&Hit::TableCell { row: 0, col: 0 }).unwrap();
        assert!(buf[(key.x, key.y)].modifier.contains(Modifier::CROSSED_OUT), "struck");

        app.editor.table.selected = Some(1);
        let (_, content) = draw_wide(&mut app);
        assert!(!content.contains("cannot be set manually"), "{content}");

        // With hover hints off, a pointer resting on a footer chip has
        // nothing to say, so the cursor row's error still shows.
        app.editor.table.selected = Some(0);
        app.ui_settings.hover_hints = false;
        app.hovered = Some(Hit::FooterChip(Action::OpenPalette));
        let (_, content) = draw_wide(&mut app);
        assert!(content.contains("Content-Length cannot be set manually"), "{content}");
        app.ui_settings.hover_hints = true;
        app.hovered = None;

        // Under a dialog the keyboard is the dialog's, so the cursor row
        // says nothing.
        app.modals
            .push(crate::components::modal::Modal::Message { title: "t".into(), body: "b".into() });
        let (_, content) = draw_wide(&mut app);
        assert!(!content.contains("cannot be set manually"), "{content}");
    }

    /// A request row naming `Host` is kept but never sent (spec 2026-10-18
    /// R3a/R3b): it draws error-red and struck, with no inline note, and
    /// hovering it puts the reason in the footer, even with hover hints
    /// off, since it stands in for a note that always showed (task #59).
    /// A button on the row keeps its own hint. The client's own `Host`
    /// row in the auto section stays, not struck.
    #[test]
    fn a_typed_host_row_is_struck_in_red_and_the_clients_host_stays() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/foo");
        app.editor
            .headers
            .insert("host".into(), Entry { value: "other.test".into(), enabled: true });
        app.update(Action::Render);

        let (terminal, content) = draw_wide(&mut app);
        let buf = terminal.backend().buffer();
        assert!(!content.contains("cannot be set manually"), "no inline note: {content}");
        let key = app.hits.rect_of(&Hit::TableCell { row: 0, col: 0 }).unwrap();
        assert_eq!(buf[(key.x, key.y)].fg, app.theme.error, "the row reads as an error");
        assert!(buffer_has_crossed_out(buf, "other.test"), "and it is struck");
        assert!(content.contains("Host: example.com"), "the client's Host row: {content}");
        assert!(!buffer_has_crossed_out(buf, "example.com"), "and it is not struck");

        app.ui_settings.hover_hints = false;
        app.hovered = Some(Hit::TableCell { row: 0, col: 1 });
        let (_, content) = draw_wide(&mut app);
        assert!(content.contains("Host cannot be set manually"), "{content}");

        app.ui_settings.hover_hints = true;
        app.hovered = Some(Hit::TableDelete(0));
        let (_, content) = draw_wide(&mut app);
        assert!(content.contains("Delete this row"), "{content}");
        assert!(!content.contains("cannot be set manually"), "{content}");
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
        let many_lines: String =
            (0..200).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
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
        assert!(app.editor.params.is_empty(), "fresh scratch editor: no rows");

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        let layout = crate::layout::compute_layout(
            ratatui::layout::Rect::new(0, 0, 120, 40),
            0.0,
            0.0,
            0.5,
            None,
        );
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

    /// The address bar is a fixed three rows, but a terminal too short to
    /// give the editor pane those rows must get no bar at all. Painting it
    /// anyway put its hits over the response pane and the footer, so a
    /// click on the footer copied the URL.
    #[test]
    fn a_terminal_too_short_for_the_address_bar_registers_none_of_its_hits() {
        let mut app = App::new_for_test();
        let backend = TestBackend::new(80, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

        for hit in [Hit::UrlBar, Hit::MethodSelector, Hit::CopyUrl] {
            assert!(
                app.hits.rect_of(&hit).is_none(),
                "{hit:?} registered on a terminal with no room for the bar: {:?}",
                app.hits.rect_of(&hit)
            );
        }
    }

    #[test]
    fn vars_tab_reuses_the_table_editor_with_a_count_in_its_label() {
        let mut e = Editor { active_tab: EditorTab::Vars, ..Editor::default() };
        e.variables.insert("token".into(), Entry { value: "abc".into(), enabled: true });
        e.variables.insert("region".into(), Entry { value: "us-east".into(), enabled: true });
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Vars · 2"), "count in tab label: {content}");
        assert!(content.contains("token"), "key text: {content}");
        assert!(content.contains("us-east"), "value text: {content}");
        assert!(
            hits.rect_of(&crate::hit::Hit::TableCell { row: e.variables.len(), col: 0 }).is_some(),
            "the ghost row's cells must be registered on the Vars tab"
        );
    }

    #[test]
    fn vars_tab_lists_the_variables_the_request_references() {
        let mut e = Editor { active_tab: EditorTab::Vars, ..Editor::default() };
        e.url = LineInput::new("https://{{base}}/v1");
        e.headers.insert("auth".into(), Entry { value: "Bearer {{token}}".into(), enabled: true });
        e.headers.insert("x-off".into(), Entry { value: "{{ignored}}".into(), enabled: false });
        e.set_body_text("{\"k\": \"{{body_var}}\"}");
        // substitute_body stays false: body tokens are sent verbatim, so
        // the body's token is not "referenced" by the send.
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("referenced"), "section divider: {content}");
        assert!(content.contains("{{base}}"), "url token listed: {content}");
        assert!(content.contains("{{token}}"), "header token listed: {content}");
        assert!(content.contains("not defined"), "an unresolvable token says so: {content}");
        assert!(!content.contains("ignored"), "disabled rows are not scanned: {content}");
        assert!(
            !content.contains("body_var"),
            "body tokens only count with substitute on: {content}"
        );

        // Turn substitution on: the body's token joins the list.
        e.substitute_body = true;
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("body_var"), "{content}");
    }

    /// A referenced row's room splits between the value and its source:
    /// both whole when they fit; else a side shorter than half keeps its
    /// width and the other takes the rest; else half each. A source with
    /// no room for `(x)` is dropped, and the value takes it all.
    #[test]
    fn a_referenced_rows_room_is_shared_by_value_and_source() {
        // `room` includes the 2-cell gap kept between the two.
        assert_eq!(split_room(32, 10, 12), (10, 12), "both fit: both whole");
        assert_eq!(split_room(32, 1, 40), (1, 29), "a short value: the source takes the rest");
        assert_eq!(split_room(32, 40, 6), (24, 6), "a short source keeps its width");
        assert_eq!(split_room(32, 40, 40), (15, 15), "both long: half each");
        assert_eq!(split_room(33, 40, 40), (16, 15), "the value takes the odd cell");
        assert_eq!(split_room(6, 40, 40), (6, 0), "no room for `(x)`: no source, nor its gap");
        assert_eq!(split_room(0, 5, 5), (0, 0));
    }

    #[test]
    fn referenced_vars_render_as_aligned_table_columns() {
        // Tokens of different lengths land in the table's own name column,
        // and their values all start at the table's value column.
        let mut e = Editor { active_tab: EditorTab::Vars, ..Editor::default() };
        e.url = LineInput::new("https://{{b}}/{{longer_name}}");
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let buf = terminal.backend().buffer();
        let row_text = |y: u16| -> String {
            (0..buf.area.width).map(|x| buf.cell((x, y)).unwrap().symbol().to_string()).collect()
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
        let mut e = Editor { active_tab: EditorTab::Vars, ..Editor::default() };
        e.url = LineInput::new("https://plain.example/v1");
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("referenced"), "no tokens, no section: {content}");
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
        e.begin_add_row();
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

        // Selecting the row costs the table no height, exactly like
        // Params/Headers: rows edit in place.
        e.table.selected = Some(0);
        assert_eq!(e.table_len(), 1);
        assert_eq!(table_height(e.table_len()), 4);

        // Delete requests a delete-confirm, same as Params/Headers.
        let action = e.handle_key(key(KeyCode::Delete));
        assert_eq!(action, Some(Action::DeleteTableRow(0)));
    }

    #[test]
    fn vars_tab_shows_dim_overrides_hint_on_a_shadowing_row() {
        let mut e = Editor { active_tab: EditorTab::Vars, ..Editor::default() };
        e.variables.insert("token".into(), Entry { value: "override".into(), enabled: true });
        e.shadowed.insert("token".into(), "qa: 1001".into());
        e.table.selected = Some(0);
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("overrides qa: 1001"), "expected the dim shadow hint: {content}");
        let buf = terminal.backend().buffer();
        let row = hits.rect_of(&crate::hit::Hit::TableRow(0)).unwrap();
        assert_eq!(row.height, 1, "the hint costs the row no height");
        // The hint trails the value on the row's own line, three columns
        // past where "override" ends.
        let value = hits.rect_of(&crate::hit::Hit::TableCell { row: 0, col: 1 }).unwrap();
        let hint_x = value.x + "override".len() as u16 + 3;
        let hint_cell = buf.cell((hint_x, row.y)).unwrap();
        assert_eq!(hint_cell.symbol(), "o", "the hint starts where it was measured to start");
        assert_eq!(
            hint_cell.fg, theme.text_muted,
            "the overrides hint is dim, not full text color"
        );
    }

    #[test]
    fn params_and_headers_tabs_never_show_a_shadow_hint() {
        // `shadow` is only ever passed for Vars, so a shadowed key on
        // Params/Headers draws no hint at all.
        let mut e = Editor { active_tab: EditorTab::Params, ..Editor::default() };
        e.params.insert("page".into(), Entry { value: "2".into(), enabled: true });
        e.shadowed.insert("page".into(), "qa: 1001".into());
        e.table.selected = Some(0);
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw(f, f.area(), &ctx, &mut hits)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("overrides"), "the Params tab shows no shadow hint: {content}");
    }

    #[test]
    fn tab_order_headers_first_and_slot_numbers_match_screen() {
        // Draw order / EditorTabCycle: Headers -> Params -> Vars -> Body.
        assert_eq!(EditorTab::from_draw_position(0), EditorTab::Headers);
        assert_eq!(EditorTab::from_draw_position(1), EditorTab::Params);
        assert_eq!(EditorTab::from_draw_position(2), EditorTab::Vars);
        assert_eq!(EditorTab::from_draw_position(3), EditorTab::Body);
        // The tab slot numbers (`EditorTabSelect(0..3)`, bindable as
        // `editor_tab_N`) follow the screen order: what you see is what
        // the number selects.
        for i in 0..4 {
            assert_eq!(EditorTab::from_index(i), EditorTab::from_draw_position(i));
            assert_eq!(
                EditorTab::from_draw_position(i).index(),
                EditorTab::from_draw_position(i).draw_position()
            );
        }
    }

    // --- computed request-headers section (Task 10, spec §6) ---------

    /// The cell where `needle` starts, counted in cells (a row's earlier
    /// glyphs may be wider than a byte).
    fn find_text(buf: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, u16)> {
        for y in 0..buf.area.height {
            let cells: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            for x in 0..cells.len() {
                if cells[x..].concat().starts_with(needle) {
                    return Some((x as u16, y));
                }
            }
        }
        None
    }

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
                if buf.cell((x, y)).unwrap().modifier.contains(Modifier::CROSSED_OUT) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn headers_tab_shows_overridden_default_as_struck_through_auto_row() {
        let mut app = App::new_for_test();
        let root = app.proj().root().to_path_buf();
        std::fs::write(
            root.join("project.toml"),
            "[default_headers]\nAccept = \"application/json\"\n",
        )
        .unwrap();
        app.resync_project();
        app.editor.active_tab = EditorTab::Headers;
        app.editor
            .headers
            .insert("Accept".into(), Entry { value: "text/plain".into(), enabled: true });
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

    /// A row that overrides an auto or client row leaves that row in the
    /// auto section, struck through, so the user sees what is overridden
    /// (task #57): the client's `Accept`, the auto `Content-Type`, the
    /// URL's `Authorization` — by a typed row, or by a project default.
    #[test]
    fn headers_tab_strikes_the_auto_rows_something_overrides() {
        let mut app = App::new_for_test();
        app.update(Action::SetMethod(postui_core::model::Method::Post));
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://u:pw@example.com/p");
        app.editor.set_body_text(r#"{"a":1}"#);
        for (name, value) in
            [("Accept", "text/plain"), ("Content-Type", "text/csv"), ("Authorization", "Bearer t")]
        {
            app.editor.headers.insert(name.into(), Entry { value: value.into(), enabled: true });
        }
        app.update(Action::Render);

        let mut terminal = Terminal::new(TestBackend::new(140, 70)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        assert!(buffer_has_crossed_out(buf, "*/*"), "the client's Accept");
        assert!(buffer_has_crossed_out(buf, "application/json"), "the auto Content-Type");
        assert!(buffer_has_crossed_out(buf, "Basic dTpwdw=="), "the URL's Authorization");
        let content = format!("{buf:?}");
        assert!(content.contains("Host: example.com"), "the client's Host is drawn");
        assert!(!buffer_has_crossed_out(buf, "example.com"), "and sent, so not struck");
        for typed in ["text/plain", "text/csv", "Bearer t"] {
            assert!(content.contains(typed), "{typed} is drawn in the table");
            assert!(!buffer_has_crossed_out(buf, typed), "{typed} is sent");
        }

        // A project default replaces the client's Accept just the same.
        let root = app.proj().root().to_path_buf();
        std::fs::write(root.join("project.toml"), "[default_headers]\nAccept = \"text/html\"\n")
            .unwrap();
        app.resync_project();
        app.editor.headers.clear();
        app.update(Action::Render);
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        assert!(format!("{buf:?}").contains("Accept: text/html"), "the default is drawn");
        assert!(!buffer_has_crossed_out(buf, "text/html"), "and sent");
        assert!(buffer_has_crossed_out(buf, "*/*"), "the client's Accept it replaces");
    }

    /// A struck auto row's strike starts on the name's first char: the
    /// indent in front of it is not struck.
    #[test]
    fn a_struck_auto_row_strikes_from_the_names_first_char() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://example.com/p");
        app.editor
            .headers
            .insert("Accept".into(), Entry { value: "text/plain".into(), enabled: true });
        app.update(Action::Render);

        let mut terminal = Terminal::new(TestBackend::new(140, 70)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        let struck =
            |x: u16, y: u16| buf.cell((x, y)).unwrap().modifier.contains(Modifier::CROSSED_OUT);
        let width = buf.area.width;
        let (x, y) = (0..buf.area.height)
            .find_map(|y| {
                let line: String = (0..width).map(|x| buf.cell((x, y)).unwrap().symbol()).collect();
                line.find("Accept: */*").map(|i| (line[..i].chars().count() as u16, y))
            })
            .expect("the client's Accept row is drawn");
        assert!(struck(x, y), "the name's first char is struck");
        assert!(!struck(x - 1, y) && !struck(x - 2, y), "the indent is not");
    }

    /// A struck URL `Authorization` masks a secret in the userinfo like a
    /// sent one does: never sent, still never shown in the clear.
    #[test]
    fn a_struck_url_authorization_keeps_its_secret_masked() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Headers;
        app.editor.url = LineInput::new("https://u:{{pw}}@example.com/p");
        app.editor
            .headers
            .insert("Authorization".into(), Entry { value: "Bearer t".into(), enabled: true });
        let mut ctx = postui_core::prepare::PrepareContext::default();
        ctx.vars.insert("pw".into(), "hunter2".into());
        ctx.meta.insert("pw".into(), postui_core::varmodel::VarMeta::Secret);
        app.editor.recompute_computed_headers(&ctx);

        let auth = app.editor.computed.rows.iter().find(|r| r.header.name == "Authorization");
        let auth = auth.expect("the URL's Authorization is listed");
        assert!(!auth.sent());
        let mask = postui_core::prepare::SECRET_MASK;
        assert_eq!(auth.header.display, format!("Basic {mask}"));
        assert!(app.editor.computed.has_secret(), "so the reveal toggle shows");
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

    /// Draws `e`'s auto-header section alone, 60 cells wide, at column 0.
    fn draw_auto_headers(e: &Editor) -> (crate::hit::HitMap, ratatui::buffer::Buffer) {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 5)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| e.draw_computed_headers(f, f.area(), &ctx, &mut hits)).unwrap();
        (hits, terminal.backend().buffer().clone())
    }

    fn auto_header(name: &str, value: &str) -> postui_core::prepare::HeaderRow {
        postui_core::prepare::HeaderRow {
            header: postui_core::prepare::SentHeader {
                name: name.into(),
                display_name: name.into(),
                value: value.into(),
                display: value.into(),
            },
            origin: postui_core::prepare::HeaderOrigin::Client,
            unsent: None,
            unresolved: Vec::new(),
            override_as: None,
        }
    }

    /// A row a request row may take over carries a pencil pill right after
    /// its copy pill (task #20); a row with no overriding row, or one
    /// already struck, carries the copy pill alone.
    #[test]
    fn an_overridable_auto_row_has_a_pencil_pill_after_its_copy_pill() {
        let mut e = Editor::default();
        let overridable = |name: &str, value: &str| postui_core::prepare::HeaderRow {
            override_as: Some((name.into(), value.into())),
            ..auto_header(name, value)
        };
        e.computed.rows = vec![
            overridable("Accept", "*/*"),
            auto_header("Host", "x.test"),
            postui_core::prepare::HeaderRow {
                unsent: Some(postui_core::prepare::Unsent::Replaced),
                ..overridable("X-Old", "1")
            },
        ];
        let (hits, buf) = draw_auto_headers(&e);
        let copy = hits.rect_of(&Hit::AutoHeaderCopy(0)).expect("the Accept row's copy pill");
        let pencil = hits.rect_of(&Hit::AutoHeaderOverride(0)).expect("the Accept row's pencil");
        assert_eq!(pencil, Rect { x: copy.x + copy.width, ..copy }, "right after the copy pill");
        assert_eq!(buf[(pencil.x + 1, pencil.y)].symbol(), crate::glyph::PENCIL);
        assert_eq!(hits.rect_of(&Hit::AutoHeaderOverride(1)), None, "nothing to write");
        assert_eq!(hits.rect_of(&Hit::AutoHeaderOverride(2)), None, "already overridden");
        assert!(hits.rect_of(&Hit::AutoHeaderCopy(2)).is_some(), "a struck row still copies");
    }

    /// A pill past column 65535 is far off the pane's right edge; its
    /// column must not wrap into the pane as a rect over the value text.
    #[test]
    fn an_auto_header_pill_past_u16_columns_registers_no_rect() {
        let mut e = Editor::default();
        // `  Host: ` + value + the spacer puts the pill at column 65546,
        // which a bare `as u16` wraps to 10.
        e.computed.rows = vec![auto_header("Host", &"v".repeat(65_537)), auto_header("A", "b")];
        let (hits, buf) = draw_auto_headers(&e);
        assert_eq!(hits.rect_of(&Hit::AutoHeaderCopy(0)), None);
        // A row that fits still gets its pill, on the painted glyph.
        let rect = hits.rect_of(&Hit::AutoHeaderCopy(1)).expect("the short row's pill");
        assert_eq!(rect.width, 3, "{rect:?}");
        assert_eq!(buf[(rect.x + 1, rect.y)].symbol(), crate::glyph::COPY_PILL.trim());
    }

    /// A value that starts past column 65535 shows nothing, so its tokens
    /// are not tinted or registered where the wrapped column would land.
    #[test]
    fn an_auto_header_value_past_u16_columns_paints_no_token() {
        let mut e = Editor::default();
        // `  ` + name + `: ` puts the value at column 65540, which a bare
        // `as u16` wraps to 4.
        e.computed.rows = vec![auto_header(&"n".repeat(65_536), "{{tok}}")];
        let (hits, buf) = draw_auto_headers(&e);
        assert_eq!(hits.rect_of(&Hit::VarToken("tok".into())), None);
        let muted = Theme::dark().text_muted;
        assert!((4..11).all(|x| buf[(x, 1)].fg == muted), "the name keeps its dim colour");

        // On screen, the token is found where the value painted it.
        e.computed.rows = vec![auto_header("A", "x {{tok}}")];
        let (hits, _) = draw_auto_headers(&e);
        assert_eq!(hits.rect_of(&Hit::VarToken("tok".into())), Some(Rect::new(7, 1, 7, 1)));
    }

    /// A pill cut by the pane's right edge registers the cells that show.
    #[test]
    fn an_auto_header_pill_cut_by_the_edge_keeps_what_shows() {
        let mut e = Editor::default();
        // `  A: ` + 52 cells + the spacer puts the pill at column 58 of 60.
        e.computed.rows = vec![auto_header("A", &"v".repeat(52))];
        let (hits, _) = draw_auto_headers(&e);
        assert_eq!(hits.rect_of(&Hit::AutoHeaderCopy(0)), Some(Rect::new(58, 1, 2, 1)));
    }
}

/// The `$EDITOR` round-trip's file (#29): `body_editor_file` and
/// `set_body_from_editor_file`, through edtui's own `Lines::from`.
#[cfg(test)]
mod editor_file_tests {
    use super::*;

    /// The lines a Vim-style editor (`fixeol`, nano) shows for `file`: a
    /// final `\n` ends the last line rather than starting one.
    fn eol_editor_lines(file: &str) -> Vec<&str> {
        file.strip_suffix('\n').unwrap_or(file).split('\n').collect()
    }

    /// What a Vim-style editor writes for `lines`, each ended by `eol`.
    fn eol_editor_write(lines: &[&str], eol: &str) -> String {
        lines.iter().map(|l| format!("{l}{eol}")).collect()
    }

    /// The lines an editor that adds no final newline (VS Code's default)
    /// shows for `file`; it writes them back joined by `\n`.
    fn raw_editor_lines(file: &str) -> Vec<&str> {
        file.split('\n').collect()
    }

    /// `body` through the `$EDITOR` round-trip, with `edit` as the editor:
    /// the file it was handed in, the file it wrote back out.
    fn editor_round_trip(body: &str, edit: impl Fn(&str) -> String) -> String {
        let mut e = Editor::default();
        e.set_body_text(body);
        let written = edit(&e.body_editor_file());
        e.set_body_from_editor_file(&written);
        e.body_text()
    }

    const ROUND_TRIP_BODIES: [&str; 5] = ["", "{}", "{}\n", "a\n\n", "{\n  \"a\": 1\n}"];

    /// #29: a body ending in `\n` came back without it from an editor that
    /// changed nothing.
    #[test]
    fn an_editor_that_changes_nothing_gives_back_the_body() {
        for body in ROUND_TRIP_BODIES {
            assert_eq!(editor_round_trip(body, str::to_string), body, "untouched, {body:?}");
            assert_eq!(
                editor_round_trip(body, |f| eol_editor_write(&eol_editor_lines(f), "\n")),
                body,
                "a Vim-style editor's write, {body:?}"
            );
            assert_eq!(
                editor_round_trip(body, |f| eol_editor_write(&eol_editor_lines(f), "\r\n")),
                body,
                "a CRLF editor's write, {body:?}"
            );
            assert_eq!(
                editor_round_trip(body, |f| raw_editor_lines(f).join("\n")),
                body,
                "a no-final-newline editor's write, {body:?}"
            );
        }
    }

    /// The file's lines, as a Vim-style editor shows them, are the body's
    /// rows: an empty last row shows as one, so it can be seen and edited.
    #[test]
    fn a_vim_style_editor_shows_the_body_s_rows() {
        for body in ROUND_TRIP_BODIES {
            let mut e = Editor::default();
            e.set_body_text(body);
            assert_eq!(
                eol_editor_lines(&e.body_editor_file()),
                body.split('\n').collect::<Vec<_>>(),
                "{body:?}"
            );
        }
    }

    #[test]
    fn an_edit_in_the_editor_is_what_comes_back() {
        let vim = |lines: &[&str]| eol_editor_write(lines, "\n");
        assert_eq!(editor_round_trip("{\n", |_| vim(&["{}"])), "{}", "Vim: replace a body");
        assert_eq!(
            editor_round_trip("{}\n", |f| vim(&eol_editor_lines(f)[..1])),
            "{}",
            "Vim: delete the empty last row"
        );
        assert_eq!(editor_round_trip("{}", |_| vim(&["{}", ""])), "{}\n", "Vim: add one");
        assert_eq!(
            editor_round_trip("{}", |f| {
                let mut lines = raw_editor_lines(f);
                lines.push("");
                lines.join("\n")
            }),
            "{}\n",
            "VS Code: an empty row added after the one the file shows"
        );
        assert_eq!(
            editor_round_trip("{}", |_| "{}".to_string()),
            "{}",
            "VS Code: the empty row the file shows, deleted"
        );
    }

    /// A CRLF editor's last row loses its `\r` as every other row does. The
    /// cost: a last row's own lone `\r` reads as half of a CRLF ending.
    #[test]
    fn a_crlf_ending_comes_off_whole() {
        assert_eq!(
            editor_round_trip("x\ny", |f| eol_editor_write(&eol_editor_lines(f), "\r\n")),
            "x\ny"
        );
        assert_eq!(editor_round_trip("a\r", str::to_string), "a");
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
    const AREA: Rect = Rect { x: 0, y: 0, width: 40, height: 10 };

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
        assert_eq!(e.body.mode, EditorMode::Insert, "selection never surfaces a vim mode");
    }

    #[test]
    fn drag_across_lines_joins_with_newlines() {
        let mut e = editor_with_body("hello\nworld\n");
        e.handle_mouse(left_down(2, 0));
        e.body_drag_to(4, 1);
        assert_eq!(e.body_selected_text().as_deref(), Some("hello\nwor"));
    }

    /// Sweeping up with the pointer past a line's end must not grab that
    /// line's last char: the pointer sits past the boundary after it, so
    /// the selection starts at the start of the row below.
    #[test]
    fn upward_drag_past_a_line_end_starts_at_the_row_below() {
        let mut e = editor_with_body("ab\nworld\n");
        e.handle_mouse(left_down(6, 1)); // caret (1,4), on 'd'
        assert!(e.body_drag_to(8, 0)); // pointer past the end of "ab"
        assert_eq!(e.body_selected_text().as_deref(), Some("world"));
    }

    /// Sweeping down onto a row's first cell must not grab that row's
    /// first char: the pointer sits on the boundary before it, so the
    /// selection ends at the end of the line above.
    #[test]
    fn downward_drag_onto_a_row_start_ends_at_the_line_above() {
        let mut e = editor_with_body("hello\nworld\n");
        e.handle_mouse(left_down(2, 0)); // caret (0,0), anchor planted
        assert!(e.body_drag_to(2, 1)); // pointer on row 1's first cell
        assert_eq!(e.body_selected_text().as_deref(), Some("hello"));
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

    /// A sweep that leaves the box out the LEFT (into the gutter or past
    /// it) keeps selecting, snapping to column 0 of the row under the
    /// pointer instead of freezing the selection at its last in-box state.
    #[test]
    fn drag_left_of_the_gutter_keeps_selecting_to_column_zero() {
        let mut e = editor_with_body("hello\nworld\n");
        e.handle_mouse(left_down(4, 0)); // caret (0,2), anchor planted
        assert!(e.body_drag_to(0, 1), "off-left drag still consumed");
        assert_eq!(e.body.cursor, Index2::new(1, 0));
        // Column 0 on a downward sweep is the boundary before the row's
        // first char: the selection ends at the end of the line above.
        assert_eq!(e.body_selected_text().as_deref(), Some("llo"));
    }

    /// A sweep that leaves the box out the TOP keeps selecting: each row
    /// of overshoot targets one buffer row above the viewport top, and the
    /// next render scrolls the viewport up to the new cursor (edtui keeps
    /// the cursor visible), so holding the drag above the box walks the
    /// selection — and the view — upward.
    #[test]
    fn drag_above_the_box_selects_upward_and_scrolls_on_render() {
        let text: String = (0..30).map(|i| format!("line{i}\n")).collect();
        let mut e = editor_with_body(&text);
        // Park the box lower on the screen and scroll it mid-buffer.
        let area = Rect { y: 5, ..AREA };
        e.last_body_area = Some(area);
        e.body.set_viewport_offset(0, 15);

        e.handle_mouse(left_down(4, 6)); // row 16 (top 15 + screen row 1)
        assert_eq!(e.body.cursor.row, 16);
        // Two rows above the box top: two buffer rows above the viewport.
        assert!(e.body_drag_to(4, 3), "off-top drag still consumed");
        assert_eq!(e.body.cursor.row, 13);
        assert!(e.body.selection.is_some());

        // The next render pulls the viewport up to the cursor.
        let mut buf = Buffer::empty(area);
        EditorView::new(&mut e.body)
            .theme(EditorTheme::default().hide_status_line())
            .wrap(true)
            .line_numbers(LineNumbers::Absolute)
            .render(area, &mut buf);
        assert!(
            e.body.viewport_offset().1 <= 13,
            "render scrolls the viewport up to the dragged-to row (offset {})",
            e.body.viewport_offset().1
        );
    }

    /// A sweep below the box bottom maps one buffer row per screen row of
    /// overshoot (the walk continues past the viewport), so dragging out
    /// the bottom selects downward and the render scrolls after it.
    #[test]
    fn drag_below_the_box_selects_downward() {
        let text: String = (0..30).map(|i| format!("line{i}\n")).collect();
        let mut e = editor_with_body(&text);
        e.handle_mouse(left_down(4, 2)); // row 2
        // AREA is 10 rows tall (y 0..10); y=12 is two rows past the bottom.
        assert!(e.body_drag_to(4, 12));
        assert_eq!(e.body.cursor.row, 12);
        assert!(e.body.selection.is_some());
    }

    /// The user's whole-lines gesture: press past the end of the last
    /// line, sweep up-left onto the first char of the first line. The
    /// head cell is included, so the first char is selected too.
    #[test]
    fn upward_drag_onto_the_first_char_includes_it() {
        let mut e = editor_with_body("alpha\nbravo\ncharlie\n");
        e.handle_mouse(left_down(20, 2)); // past "charlie": caret (2,7)
        assert!(e.body_drag_to(2, 0)); // first char of line 0
        assert_eq!(e.body_selected_text().as_deref(), Some("alpha\nbravo\ncharlie"));
    }

    #[test]
    fn paste_body_inserts_multiline_text_at_the_caret() {
        let mut e = editor_with_body("ab\ncd\n");
        e.body.cursor = Index2::new(0, 1);
        assert!(e.paste_body("X\nY"));
        assert_eq!(e.body_text(), "aX\nYb\ncd\n");
        assert_eq!(e.body.cursor, Index2::new(1, 1), "caret after the paste");
        assert_eq!(e.body.mode, EditorMode::Insert);
    }

    #[test]
    fn paste_body_replaces_a_live_selection_and_normalizes_crlf() {
        let mut e = editor_with_body("hello\nworld\n");
        e.handle_mouse(left_down(2, 0)); // anchor at (0,0)
        e.body_drag_to(4, 0); // select "hel"
        assert!(e.body.selection.is_some());
        assert!(e.paste_body("a\r\nb"));
        assert_eq!(e.body_text(), "a\nblo\nworld\n");
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
    fn ctrl_shift_a_selects_the_whole_body_and_ctrl_a_is_line_start() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("hello\nworld");
        e.body.cursor = Index2::new(1, 2);
        // Both reported spellings of ctrl+shift+a select all.
        e.handle_key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(e.body_selected_text().as_deref(), Some("hello\nworld"));
        let mut e = editor_with_body("hello\nworld");
        e.body.cursor = Index2::new(1, 2);
        e.handle_key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('A'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(e.body_selected_text().as_deref(), Some("hello\nworld"));
        // Plain ctrl+a: select-all on Linux builds (the fold is the
        // identity there — exercised explicitly so this asserts the
        // Linux meaning regardless of the build host).
        let mut e = editor_with_body("hello\nworld");
        e.body.cursor = Index2::new(1, 2);
        e.handle_key(crate::keys::fold_text_nav_bytes(
            ratatui::crossterm::event::KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            false,
        ));
        assert_eq!(e.body_selected_text().as_deref(), Some("hello\nworld"));
        // On macOS the fold turns the same byte into the smart-Home jump.
        let mut e = editor_with_body("  hello");
        e.body.cursor = Index2::new(0, 6);
        e.handle_key(crate::keys::fold_text_nav_bytes(
            ratatui::crossterm::event::KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            true,
        ));
        assert_eq!(e.body.cursor, Index2::new(0, 2), "first non-whitespace");
        assert_eq!(e.body_selected_text(), None);
    }

    #[test]
    fn alt_b_and_alt_f_word_hop_in_the_body() {
        use ratatui::crossterm::event::KeyCode;
        // ESC b / ESC f: option+arrows as mac terminals deliver them —
        // folded (macOS builds) to the same hops as the ctrl+arrow
        // spelling.
        let fold = |c, mods: KeyModifiers| {
            crate::keys::fold_text_nav_bytes(
                ratatui::crossterm::event::KeyEvent::new(KeyCode::Char(c), mods),
                true,
            )
        };
        let mut e = editor_with_body("one two three");
        e.body.cursor = Index2::new(0, 13);
        e.handle_key(fold('b', KeyModifiers::ALT));
        assert_eq!(e.body.cursor, Index2::new(0, 8), "alt+b hops a word left");
        e.handle_key(fold('f', KeyModifiers::ALT));
        assert_eq!(e.body.cursor, Index2::new(0, 13), "alt+f hops a word right");
        // The shifted spelling selects the hop.
        e.handle_key(fold('B', KeyModifiers::ALT));
        assert_eq!(e.body_selected_text().as_deref(), Some("three"));
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

    /// A Body-tab editor holding a POST request opened with `text` as its
    /// saved body, which the body's indent is detected from (spec round-4
    /// amendment).
    fn loaded_body(text: &str) -> Editor {
        let mut e = editor_with_body("");
        e.load(None, json_request(text));
        render_body(&mut e);
        e
    }

    fn json_request(text: &str) -> HttpRequest {
        HttpRequest {
            method: Method::Post,
            body: Some(Body::Json { text: text.into() }),
            ..Default::default()
        }
    }

    /// Spec R9: indent and dedent use the body's own detected indent.
    #[test]
    fn indent_and_dedent_follow_a_four_space_body() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = loaded_body("{\n    \"a\": 1\n}");
        e.set_body_selection_cells(Index2::new(0, 0), Index2::new(2, 0));
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "    {\n        \"a\": 1\n    }");
        e.handle_key(backtab());
        assert_eq!(e.body_text(), "{\n    \"a\": 1\n}");
    }

    #[test]
    fn indent_and_dedent_follow_a_tab_body() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = loaded_body("{\n\t\"a\": 1\n}");
        e.set_body_selection_cells(Index2::new(0, 0), Index2::new(2, 0));
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "\t{\n\t\t\"a\": 1\n\t}");
        e.handle_key(backtab());
        assert_eq!(e.body_text(), "{\n\t\"a\": 1\n}");
    }

    /// Spec round-4 amendment: the indent is detected from the text as last
    /// opened or saved, else the default. Editing (a replacement, a paste)
    /// never moves it; a save re-reads it.
    #[test]
    fn the_body_indent_follows_the_saved_text() {
        use crate::text::indent::Indent;
        let mut e = editor_with_body("");
        assert_eq!(e.body_indent(), Indent::Spaces(2), "nothing saved: the default");
        e.load(None, json_request("{\n    \"a\": 1\n}"));
        assert_eq!(e.body_indent(), Indent::Spaces(4));
        e.set_body_text("{\"a\":1}");
        assert_eq!(e.body_indent(), Indent::Spaces(4), "Minify keeps the saved indent");
        e.paste_body("\n\t\tx\n\t\ty");
        assert_eq!(e.body_indent(), Indent::Spaces(4), "a paste never moves it");
        e.mark_saved();
        assert_eq!(e.body_indent(), Indent::Tabs, "a save reads the text again");
        e.load(None, json_request("{}"));
        e.indent_default = Indent::Spaces(4);
        assert_eq!(e.body_indent(), Indent::Spaces(4), "a flat saved body: the default");
    }

    /// Review round 4: an indent never feeds the next one's width, so
    /// repeated presses on a body's only indented line step 2, 4, 6.
    #[test]
    fn repeated_indents_step_by_a_fixed_width() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = loaded_body("{\n  \"a\": 1\n}");
        e.set_body_selection_cells(Index2::new(1, 0), Index2::new(1, 0));
        e.handle_key(pkey(KeyCode::Tab));
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "{\n      \"a\": 1\n}");
    }

    /// Spec 2026-10-15 R8: plain Tab steps to the next indent stop with
    /// spaces, or inserts one tab under a tab indent.
    #[test]
    fn tab_without_a_selection_steps_to_the_next_stop() {
        use crate::text::indent::Indent;
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body("ab");
        e.body.cursor = Index2::new(0, 1);
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "a b", "column 1 to the stop at 2");
        e.body.cursor = Index2::new(0, 0);
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "  a b", "column 0 takes a whole level");
        assert_eq!(e.body.cursor, Index2::new(0, 2));
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "    a b", "column 2 to the stop at 4");
        let mut e = editor_with_body("ab");
        e.indent_default = Indent::Tabs;
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "\tab", "nothing saved: the default's tab");
    }

    fn enter_at(text: &str, row: usize, col: usize) -> Editor {
        use ratatui::crossterm::event::KeyCode;
        let mut e = editor_with_body(text);
        e.body.cursor = Index2::new(row, col);
        e.handle_key(pkey(KeyCode::Enter));
        e
    }

    /// Spec 2026-10-15 R14: after the first non-blank, Enter copies the
    /// line's indent and the moved text loses its own leading blanks
    /// (Vim's autoindent, probed).
    #[test]
    fn enter_after_the_text_starts_copies_the_indent() {
        let e = enter_at("    a", 0, 5);
        assert_eq!((screen(&e), e.body.cursor), ("    a\n    ".into(), Index2::new(1, 4)));
        let e = enter_at("    foo  bar", 0, 9);
        assert_eq!(e.body_text(), "    foo  \n    bar");
        assert_eq!(e.body.cursor, Index2::new(1, 4));
        let e = enter_at("\t\tk", 0, 3);
        assert_eq!(screen(&e), "\t\tk\n\t\t", "a tab indent is copied as tabs");
        let e = enter_at("ab", 0, 1);
        assert_eq!(e.body_text(), "a\nb", "no indent, nothing copied");
    }

    /// The buffer as drawn, untouched auto-indent included.
    fn screen(e: &Editor) -> String {
        e.body.lines.to_string()
    }

    /// Spec 2026-10-16 R1/R2: an indent Enter leaves alone on its line is
    /// untouched: the screen shows it, the request's body leaves it out.
    #[test]
    fn an_untouched_auto_indent_is_not_in_the_body() {
        let e = enter_at("    a", 0, 5);
        assert_eq!((screen(&e), e.body_text()), ("    a\n    ".into(), "    a\n".into()));
        let e = enter_at("    a  ", 0, 5);
        assert_eq!(e.body_text(), "    a\n", "only blanks after the caret");
        let e = enter_at("\t\tk", 0, 3);
        assert_eq!(e.body_text(), "\t\tk\n", "a tab indent too");
        let e = enter_at("    ab", 0, 5);
        assert_eq!(e.body_text(), "    a\n    b", "text moved down uses the indent");
        let e = enter_at("a", 0, 1);
        assert!(e.auto_indent.is_none(), "no indent, nothing marked");
    }

    /// R3: the screen drops the blanks once the caret leaves the line or
    /// the body.
    #[test]
    fn leaving_the_line_drops_the_untouched_indent() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = enter_at("    a", 0, 5);
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n    ", "the caret is still on it");
        e.handle_key(pkey(KeyCode::Up));
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n");
        assert!(e.auto_indent.is_none());
        let mut e = enter_at("    a", 0, 5);
        e.settle_auto_indent(false);
        assert_eq!((screen(&e), e.body.cursor), ("    a\n".into(), Index2::new(1, 0)));
        assert!(e.auto_indent.is_none());
    }

    /// R4: any edit on the line makes its indent the user's text, even
    /// one deleted back to the bare indent.
    #[test]
    fn an_edit_on_the_line_keeps_its_indent() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = enter_at("    a", 0, 5);
        e.handle_key(pkey(KeyCode::Char('x')));
        e.settle_auto_indent(true);
        assert_eq!(e.body_text(), "    a\n    x");
        e.handle_key(pkey(KeyCode::Backspace));
        e.settle_auto_indent(true);
        assert_eq!(e.body_text(), "    a\n    ", "back to the bare indent: still the user's");
        e.handle_key(pkey(KeyCode::Up));
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n    ");
    }

    /// R4, review round 1: an edit that moves rows (Backspace joining the
    /// untouched line up) forgets the mark, so a line of the user's own
    /// that slides into its row is never taken for it.
    #[test]
    fn an_edit_that_moves_rows_forgets_the_mark() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = enter_at("    a\n    ", 0, 5);
        e.handle_key(pkey(KeyCode::Home));
        e.handle_key(pkey(KeyCode::Backspace));
        assert_eq!(e.body_text(), "    a    \n    ", "the user's line is in the body");
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a    \n    ", "and stays on screen");
        assert!(e.auto_indent.is_none());
    }

    /// R6: Enter on the untouched line moves it down, still untouched.
    #[test]
    fn enter_carries_the_untouched_indent_down() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = enter_at("    a", 0, 5);
        e.settle_auto_indent(true);
        e.handle_key(pkey(KeyCode::Enter));
        e.settle_auto_indent(true);
        assert_eq!((screen(&e), e.body_text()), ("    a\n\n    ".into(), "    a\n\n".into()));
        e.handle_key(pkey(KeyCode::Up));
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n\n");
    }

    /// R7: replacing the body forgets the mark.
    #[test]
    fn replacing_the_body_forgets_the_untouched_indent() {
        let mut e = enter_at("    a", 0, 5);
        e.set_body_text("    a\n    ");
        assert!(e.auto_indent.is_none());
        assert_eq!(e.body_text(), "    a\n    ");
    }

    /// R8: a selection end or anchor on the dropped line moves to column 0.
    #[test]
    fn a_selection_on_the_dropped_line_clamps_to_its_start() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = enter_at("    a\nb", 0, 5);
        e.handle_key(skey(KeyCode::Down));
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n\nb");
        assert_eq!(e.body_sel_anchor, Some(Index2::new(1, 0)));
        let sel = e.body.selection.as_ref().expect("the selection survives");
        assert_eq!(sel.start().row, 1);
        assert_eq!(sel.start().col, 0);
        assert_eq!(e.body_selected_text().as_deref(), Some("\nb"), "the emptied row through b");
    }

    /// R3, review round 3: a selection is leaving too, so what acts on a
    /// selection (copy, Tab, cut) never sees blanks the request lacks.
    #[test]
    fn a_selection_drops_the_untouched_indent() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = enter_at("    a", 0, 5);
        e.body_select_all();
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n", "the caret stayed on the row, the blanks went");
        assert_eq!(e.body_selected_text().as_deref(), Some("    a\n"), "copy matches the body");
        e.handle_key(pkey(KeyCode::Tab));
        assert_eq!(e.body_text(), "      a\n", "Tab skips the empty line");
        // A selection inside the blanks leaves nothing to select; its
        // anchor stays, at the row's start, as a collapsed one does.
        let mut e = enter_at("    a", 0, 5);
        e.handle_key(skey(KeyCode::Left));
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n");
        assert_eq!(e.body.cursor, Index2::new(1, 0));
        assert!(e.body.selection.is_none());
        assert_eq!(e.body_sel_anchor, Some(Index2::new(1, 0)));
    }

    /// R3, review round 4: the indent rests only with the caret right after
    /// it. A click inside the blanks drops them, so the next key types on
    /// the empty line the request holds, never between leftover blanks.
    #[test]
    fn a_caret_inside_the_blanks_drops_them() {
        let mut e = enter_at("    a", 0, 5);
        e.handle_mouse(left_down(4, 1));
        e.settle_auto_indent(true);
        assert_eq!((screen(&e), e.body.cursor), ("    a\n".into(), Index2::new(1, 0)));
        e.handle_key(pkey(ratatui::crossterm::event::KeyCode::Char('x')));
        assert_eq!(e.body_text(), "    a\nx");
    }

    /// Review round 4: a drag that starts on the untouched indent and
    /// moves into it drops the blanks and goes on selecting.
    #[test]
    fn a_drag_through_the_blanks_keeps_selecting() {
        let mut e = enter_at("    a", 0, 5);
        e.handle_mouse(left_down(6, 1));
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n    ", "pressed at the indent's end: kept");
        assert!(e.body_drag_to(4, 1));
        e.settle_auto_indent(true);
        assert_eq!(screen(&e), "    a\n");
        assert!(e.body_drag_to(3, 0), "the drag goes on");
        assert_eq!(e.body_selected_text().as_deref(), Some("   a\n"));
    }

    /// R14 (user ruling): at or before the first non-blank, Enter opens an
    /// empty line above and the line keeps its indent and caret column.
    #[test]
    fn enter_inside_the_indent_opens_a_line_above() {
        for col in [0, 2, 4] {
            let e = enter_at("    }", 0, col);
            assert_eq!(e.body_text(), "\n    }", "caret at {col}");
            assert_eq!(e.body.cursor, Index2::new(1, col), "caret at {col}");
        }
        let e = enter_at("{\n    \n}", 1, 4);
        assert_eq!(e.body_text(), "{\n\n    \n}", "a whitespace-only line moves down whole");
    }

    #[test]
    fn enter_twice_leaves_no_trailing_blanks() {
        use ratatui::crossterm::event::KeyCode;
        let mut e = enter_at("    a", 0, 5);
        e.handle_key(pkey(KeyCode::Enter));
        e.handle_key(pkey(KeyCode::Char('x')));
        assert_eq!(e.body_text(), "    a\n\n    x");
    }

    #[test]
    fn enter_on_an_empty_body_and_over_a_selection() {
        use ratatui::crossterm::event::KeyCode;
        let e = enter_at("", 0, 0);
        assert_eq!((e.body_text(), e.body.cursor), ("\n".into(), Index2::new(1, 0)));
        let mut e = editor_with_body("  ab cd");
        e.set_body_selection_cells(Index2::new(0, 4), Index2::new(0, 4));
        e.body.cursor = Index2::new(0, 4);
        e.handle_key(pkey(KeyCode::Enter));
        assert_eq!(e.body_text(), "  ab\n  cd", "the selection goes, then an indented break");
    }

    #[test]
    fn a_paste_is_never_re_indented() {
        let mut e = editor_with_body("    a");
        e.body.cursor = Index2::new(0, 5);
        e.paste_body("\nb\n  c");
        assert_eq!(e.body_text(), "    a\nb\n  c");
    }
}
