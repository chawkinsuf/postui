use super::json_tree::{JsonTree, TokenKind};
use super::line_input::LineInput;
use super::{Component, DrawCtx, pane_surface};
use crate::action::{Action, CopyTarget};
use crate::hit::ScrollbarSpec;
use crate::layout::PaneId;
use crate::theme::Theme;
use crate::config::JqTab;
use postui_core::jq::complete::{self, Candidate, Context, Kind};
use postui_core::jq::{JqDocument, JqError};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Bodies up to this size are parsed on the UI thread, where the parse is
/// too quick to be noticed. Anything larger is parsed on a blocking worker
/// and delivered later via [`Response::attach_tree`], so no response is ever
/// too big to pretty-print and none of them stall the UI.
pub const SYNC_PRETTY_BYTES: usize = 256 * 1024;

/// Braille spinner frames, cycled while a request is in flight.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How long a request may sit in flight before the in-flight view adds its
/// "taking a while" warning line. There is deliberately no client timeout —
/// the user cancels with Esc when they've waited long enough.
const LONG_WAIT_WARNING_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// Columns moved per ←/→ key press or horizontal wheel notch.
pub(crate) const H_SCROLL_STEP: i16 = 4;

/// The response pane's lifecycle: nothing sent yet, a request in flight (and
/// since when — used to animate a spinner), a completed response, a failed
/// send, or a send the user cancelled.
#[derive(Default)]
pub enum ResponseState {
    #[default]
    Empty,
    InFlight {
        started: Instant,
    },
    Ready(Box<crate::http::ResponseData>),
    Failed(String),
    Cancelled,
}

/// Which of the three renderings of a ready response is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Pretty,
    Raw,
    Headers,
}

/// The in-pane search: a live input while `active`, then a committed query
/// with its match list. Matches are `(full line index, char column)` into the
/// current view's *fully expanded* text, so a hit inside a collapsed
/// container is still found (and jumped to).
pub struct SearchState {
    pub input: LineInput,
    pub active: bool,
    pub query: String,
    pub matches: Vec<(usize, usize)>,
    pub current: usize,
}

/// The bar's completion state: the candidates for the caret's position
/// and the keys they were built from. See the completion spec.
#[derive(Default)]
pub struct JqCompletion {
    /// The context expression `cached_keys` were fetched for.
    cached_expr: Option<String>,
    cached_keys: Vec<String>,
    /// What the caret is typing; `None` when nothing is offered there.
    ctx: Option<Context>,
    candidates: Vec<Candidate>,
    /// Which candidate the ghost shows; reset whenever the context
    /// changes.
    index: usize,
    /// A key fetch outstanding on the blocking pool: its sequence number
    /// and the expression it is for. Only the newest fetch's result is
    /// kept.
    pending: Option<(u64, String)>,
    seq: u64,
}

impl JqCompletion {
    pub fn pending(&self) -> Option<u64> {
        self.pending.as_ref().map(|(s, _)| *s)
    }

    fn step(&mut self, forward: bool) {
        let n = self.candidates.len();
        if n == 0 {
            return;
        }
        self.index = if forward {
            (self.index + 1) % n
        } else {
            (self.index + n - 1) % n
        };
    }
}

/// The jq filter bar's own state: the input, focus, and the outcome of the
/// last filter applied. Lives on [`Response`], not [`ReadyView`] — the bar
/// (and whatever filter the user typed) survives a new response landing in
/// the same slot, exactly like the address bar survives a send.
pub struct JqBar {
    pub input: LineInput,
    pub focused: bool,
    /// Whether the filter is switched on. Closing the bar (`Esc`, the
    /// header button, `alt+q`) switches it off and keeps the text; the
    /// tree shows the full body until it is opened again. Mirrors
    /// `Editor::jq_enabled`, which is what persists.
    pub enabled: bool,
    /// Set on every bar edit — text or the on/off switch;
    /// [`Response::take_jq_edited`] clears it.
    edited: bool,
    pub error: Option<JqError>,
    /// The last run produced nothing to show: it failed (`error` says
    /// why, and the previous good tree stays) or every output was `null`
    /// (`note` says which, and the full body shows). Either way the tree
    /// on screen is not this filter's output.
    pub stale: bool,
    /// Why a run that succeeded still shows the full body: every output
    /// was `null` (or one array of nothing but nulls) → `"null"`; no
    /// output at all → `"no output"`. A mid-typing `.mo` yields `null`,
    /// and blanking the tree under someone reading it is worse than
    /// leaving it up. Drawn as a red "invalid filter" under the bar (the
    /// distinction is kept for tests and a future, wordier message).
    pub note: Option<&'static str>,
    /// A background run is outstanding (its per-view counter).
    pub pending: Option<u64>,
    /// When `pending`'s run was started, so the bar can hold its spinner
    /// back for `JQ_SPINNER_AFTER` and then animate it.
    pending_since: Instant,
    pub ai_pending: bool,
    /// When the AI request started, for the spinner.
    pub ai_started: std::time::Instant,
    pub completion: JqCompletion,
    /// What Tab does on a ghost (`config.toml` `jq_tab`).
    pub tab: JqTab,
}

impl JqBar {
    /// Whether a background run has been outstanding long enough (as of
    /// `now`) for the bar's chip to spin: past `JQ_SPINNER_AFTER`, so a
    /// run that finishes sooner never flickers.
    fn running_long(&self, now: Instant) -> bool {
        self.pending.is_some()
            && now.saturating_duration_since(self.pending_since) >= JQ_SPINNER_AFTER
    }

    /// See [`Response::jq_open`].
    fn is_open(&self) -> bool {
        self.focused
            || (self.enabled && !self.input.text().is_empty())
            || self.ai_pending
            || self.pending.is_some()
    }

    /// Whether the caret is at the end of the text with nothing selected —
    /// the only place a ghost is drawn.
    fn caret_at_end(&self) -> bool {
        self.input.selection().is_none()
            && self.input.cursor() == self.input.text().chars().count()
    }

    /// The candidate the ghost shows, when one is showing.
    fn candidate(&self) -> Option<&Candidate> {
        if !self.focused || self.ai_pending || !self.caret_at_end() {
            return None;
        }
        self.completion.candidates.get(self.completion.index)
    }

    /// The ghost text after the caret, when there is one.
    pub fn ghost(&self) -> Option<&str> {
        self.candidate().map(|c| c.ghost.as_str())
    }
}

impl Default for JqBar {
    fn default() -> Self {
        Self {
            input: LineInput::new(""),
            focused: false,
            enabled: true,
            edited: false,
            error: None,
            stale: false,
            note: None,
            pending: None,
            pending_since: Instant::now(),
            ai_pending: false,
            ai_started: Instant::now(),
            completion: JqCompletion::default(),
            tab: JqTab::Cycle,
        }
    }
}

/// Whether a filter's outputs are all `null` — every document is `null`,
/// or the single document is an array holding nothing but nulls (what a
/// mistyped `map(.fied)` gives). An empty array is a real answer, not this.
fn all_null(outputs: &[String]) -> bool {
    if outputs.iter().all(|o| o == "null") {
        return true;
    }
    // Outputs are jq's own serialisation, so a flat array of nulls is
    // `[null,null,…]` — read off the text rather than parsed: the single
    // output of `.` on a multi-megabyte body is the whole body, and a full
    // parse of it here ran on the UI thread on every keystroke.
    let [only] = outputs else { return false };
    let Some(inner) = only
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
    else {
        return false;
    };
    !inner.trim().is_empty() && inner.split(',').all(|item| item.trim() == "null")
}

/// Work the app must hand to the blocking pool: a jq run too big to run
/// inline. `doc` is the cached parse when there is one; otherwise `body` is
/// the raw text for the worker to parse before running the filter.
pub struct JqRunRequest {
    pub generation: u64,
    pub run: u64,
    pub code: String,
    pub doc: Option<JqDocument>,
    pub body: Option<String>,
}

/// A completion key fetch too big for the UI thread: run `input_expr`
/// against `doc` on the blocking pool (`complete::keys_at`) and hand the
/// keys back as `Action::JqCompleteFinished`.
pub struct JqCompleteRequest {
    pub generation: u64,
    pub seq: u64,
    pub input_expr: String,
    pub doc: JqDocument,
}

/// What a jq run hands back: the document it parsed (when it had to), the
/// filter's outputs as jq would print them, and those outputs already
/// flattened into a tree. The tree is built where the run ran — on the
/// blocking pool for a big body — because flattening a multi-megabyte
/// output takes several times longer than the filter itself, and doing it
/// on the UI thread froze the app for exactly the stretch the bar's
/// spinner is meant to cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JqRunOutput {
    pub doc: Option<JqDocument>,
    pub outputs: Vec<String>,
    /// `None` when an output is not JSON (jq's `@text`-style strings) —
    /// reported as an error on attach.
    pub tree: Option<JsonTree>,
}

impl JqRunOutput {
    /// Flattens `outputs` into the tree on the calling thread.
    pub fn from_outputs(doc: Option<JqDocument>, outputs: Vec<String>) -> Self {
        let tree = JsonTree::parse_many(&outputs);
        Self { doc, outputs, tree }
    }
}

/// How long a background run may take before the bar shows a spinner:
/// a run that finishes inside this window never flickers one.
pub const JQ_SPINNER_AFTER: Duration = Duration::from_millis(100);

/// Everything about *how* a ready response is being looked at. Rebuilt from
/// scratch whenever a new response lands, so no state leaks between requests.
pub struct ReadyView {
    pub mode: ViewMode,
    /// The body view (`Pretty` or `Raw`) to come back to from `Headers`.
    body_mode: ViewMode,
    pub tree: Option<JsonTree>,
    /// The send generation this response belongs to, so a background parse
    /// can be matched to the view it was started for (and only that one).
    pub generation: u64,
    /// True while a background parse of this body is still running: no tree
    /// yet, but one may still arrive.
    pub parsing: bool,
    /// The tree (and jq state) were shed to save memory while this response
    /// sat in the session cache (`Response::shed_derived`); the next time it
    /// comes on screen `take_reparse` starts the parse again.
    shed: bool,
    /// The body tree landed while a filter was switched on: it stays
    /// hidden behind the spinner until that filter's output (`jq_tree`)
    /// lands too, so the user never sees the unfiltered tree flash up and
    /// dim. Cleared by the run's result, or by the filter being cleared
    /// or failing to compile.
    awaiting_filter: bool,
    /// When the background parse started, so the wait can animate.
    parse_started: Instant,
    /// Verbatim body lines — never reformatted, never re-wrapped.
    raw_lines: Vec<String>,
    header_lines: Vec<String>,
    pub cursor: usize,
    pub scroll: usize,
    /// Column offset of the body viewport — verbatim lines are never
    /// wrapped, so lines wider than the pane scroll horizontally instead.
    pub h_scroll: usize,
    pub search: Option<SearchState>,
    /// Height of the body viewport as of the last draw, so key handling can
    /// keep the cursor on screen. A sane guess until the first frame.
    height: usize,
    /// Width of the body viewport as of the last draw — the horizontal
    /// counterpart of `height`, used to clamp `h_scroll`.
    width: usize,
    /// Cached widest visible line in display columns, keyed by the (mode,
    /// visible line count, tree epoch) it was measured for — a
    /// collapse/expand or a view switch changes the visible set, so either
    /// invalidates it, and so does a tree swap: a filtered tree can have
    /// the same line count as the body tree (or an earlier filtered tree)
    /// while its content is entirely different, which the (mode, len) key
    /// alone can't tell apart. Measuring is O(all visible lines), too much
    /// to redo per wheel tick — or per keystroke — on a megabyte body.
    content_width: Option<(ViewMode, usize, u64, usize)>,
    /// The body content rect as of the last draw — the coordinate frame
    /// mouse selection maps through. `None` before the first frame.
    pub last_area: Option<Rect>,
    /// The fixed (visible line, char col) cell a selection sweep grows
    /// from; planted on `Down`, consumed by drags and shift+Up/Down.
    sel_anchor: Option<(usize, usize)>,
    /// A live selection: its anchor and head cells (either order, both
    /// inclusive), in (visible line, char col) coordinates of the current
    /// view mode.
    sel: Option<((usize, usize), (usize, usize))>,
    /// The inclusive cell span of a double-clicked word: while set, drag
    /// sweeps extend by whole words from this span instead of by cells.
    /// Cleared by any single click (`clear_sel`).
    sel_word_anchor: Option<((usize, usize), (usize, usize))>,
    /// The cached parse of the response body for jq to run filters
    /// against — built lazily (a sync run parses it the first time it's
    /// needed) or handed back from a background run.
    jq_doc: Option<JqDocument>,
    /// The tree the last successful filter produced; `active_tree` prefers
    /// this over `tree` while it's set.
    jq_tree: Option<JsonTree>,
    /// The code `jq_tree` (or `Response::jq`'s error) reflects — `None`
    /// before any filter has run, `Some("")` for an explicitly cleared one.
    jq_applied: Option<String>,
    /// The code `jq_tree` is the output of — what the structural verbs
    /// compose onto, since the tree on screen is what the user clicked.
    /// `None` while the body tree shows (no filter, or a filter with
    /// nothing to show), and differs from `jq_applied` while a bad filter
    /// keeps the previous good tree up.
    jq_tree_code: Option<String>,
    /// How many outputs the last successful filter produced (1 when no
    /// filter is applied, or the filter emits exactly one document).
    jq_outputs: usize,
    /// Counts every `apply_jq` run started for this view, so a background
    /// result can be matched to the run it was started for (and only that
    /// one — a superseded run's result is dropped).
    jq_runs: u64,
    /// Bumped whenever the tree on screen is swapped for another (a
    /// filter's output landing, or the body tree coming back) — what the
    /// content-width cache is keyed on, so a run that leaves the same tree
    /// up (a null result, a superseded run) keeps the measure.
    tree_epoch: u64,
    /// Column indexes (`col_marks`) of the raw lines that have been
    /// scrolled sideways, by line — built lazily by `index_raw_rows`, never
    /// invalidated (the raw lines are immutable for the view's life).
    raw_marks: HashMap<usize, Vec<ColMark>>,
}

impl ReadyView {
    fn new(data: &crate::http::ResponseData, generation: u64) -> Self {
        // A big body is parsed off-thread; until that lands there is no
        // tree to show. One that looks like JSON (starts with `{` or `[`)
        // still leads with the Tree tab — showing its spinner, the same
        // pane it will settle on — rather than flashing the raw body first;
        // `attach_tree` kicks it back to Raw should the parse prove it
        // isn't JSON after all. A big body that plainly isn't JSON leads
        // with Raw at once, no spinner to sit through.
        let parsing = data.body.len() > SYNC_PRETTY_BYTES;
        let tree = if parsing {
            None
        } else {
            JsonTree::parse(&data.body)
        };
        let looks_json = data.body.trim_start().starts_with(['{', '[']);
        let mode = if tree.is_some() || (parsing && looks_json) {
            ViewMode::Pretty
        } else {
            ViewMode::Raw
        };
        let width = data
            .headers
            .iter()
            .map(|(k, _)| k.chars().count())
            .max()
            .unwrap_or(0);
        Self {
            mode,
            body_mode: mode,
            tree,
            generation,
            parsing,
            shed: false,
            awaiting_filter: false,
            parse_started: Instant::now(),
            raw_lines: data.body.split('\n').map(|l| l.to_string()).collect(),
            header_lines: data
                .headers
                .iter()
                .map(|(k, v)| format!("{:<width$} {v}", format!("{k}:"), width = width + 1))
                .collect(),
            cursor: 0,
            scroll: 0,
            h_scroll: 0,
            search: None,
            height: 10,
            width: 40,
            content_width: None,
            last_area: None,
            sel_anchor: None,
            sel: None,
            sel_word_anchor: None,
            jq_doc: None,
            jq_tree: None,
            jq_applied: None,
            jq_tree_code: None,
            jq_outputs: 1,
            jq_runs: 0,
            tree_epoch: 0,
            raw_marks: HashMap::new(),
        }
    }

    /// Whether the `Pretty` view is offered at all: a parsed tree, or a
    /// parse still running that may yet produce one.
    fn has_tree_view(&self) -> bool {
        self.tree.is_some() || self.parsing
    }

    /// Whether a background parse tagged `generation` belongs to this view
    /// and is still expected.
    fn awaits_tree(&self, generation: u64) -> bool {
        self.parsing && self.generation == generation
    }

    /// The tree the `Pretty` view actually shows: the filtered tree while a
    /// jq filter is applied, otherwise the body tree.
    pub fn active_tree(&self) -> Option<&JsonTree> {
        self.jq_tree.as_ref().or(self.tree.as_ref())
    }

    /// Swaps the filtered tree in (or out), bumping the epoch only when
    /// the tree on screen actually changes — clearing an already-absent
    /// filter tree leaves the body tree, and its width measure, as it was.
    fn set_jq_tree(&mut self, tree: Option<JsonTree>) {
        if tree.is_some() || self.jq_tree.is_some() {
            self.tree_epoch += 1;
        }
        self.jq_tree = tree;
    }

    fn active_tree_mut(&mut self) -> Option<&mut JsonTree> {
        if self.jq_tree.is_some() {
            self.jq_tree.as_mut()
        } else {
            self.tree.as_mut()
        }
    }

    fn open_search(&mut self) {
        self.search = Some(SearchState {
            input: LineInput::new(""),
            active: true,
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        });
    }

    /// How many lines the current view shows right now (collapse included).
    pub fn visible_len(&self) -> usize {
        match self.mode {
            ViewMode::Pretty => self.active_tree().map_or(0, |t| t.visible_len()),
            ViewMode::Raw => self.raw_lines.len(),
            ViewMode::Headers => self.header_lines.len().max(1),
        }
    }

    /// Builds the column index of every raw line in `rows` that is long
    /// enough to need one, ahead of a sideways-scrolled frame — the one
    /// `&mut` step `body_lines` (which borrows the view shared) relies on.
    fn index_raw_rows(&mut self, rows: std::ops::Range<usize>) {
        for i in rows {
            let Some(line) = self.raw_lines.get(i) else {
                break;
            };
            if line.len() > COL_MARK_STEP && !self.raw_marks.contains_key(&i) {
                self.raw_marks.insert(i, col_marks(line));
            }
        }
    }

    /// Widest visible line of the current view, in display columns, from
    /// the cache when its (mode, visible count, tree epoch) key still
    /// matches.
    fn content_width(&mut self) -> usize {
        use unicode_width::UnicodeWidthStr;
        let key = (self.mode, self.visible_len(), self.tree_epoch);
        if let Some((mode, len, run, w)) = self.content_width
            && (mode, len, run) == key
        {
            return w;
        }
        let w = match self.mode {
            ViewMode::Pretty => self.active_tree().map_or(0, JsonTree::visible_width),
            ViewMode::Raw => self.raw_lines.iter().map(|l| l.width()).max().unwrap_or(0),
            // + 3 for the ` ❐ ` copy pill appended to each rendered row.
            ViewMode::Headers => self
                .header_lines
                .iter()
                .map(|l| l.width() + 3)
                .max()
                .unwrap_or(0),
        };
        self.content_width = Some((key.0, key.1, key.2, w));
        w
    }

    /// Moves the viewport `delta` columns right (negative: left), clamped so
    /// the widest visible line's end never scrolls past the right edge.
    fn scroll_h(&mut self, delta: i32) {
        let max = self.content_width().saturating_sub(self.width.max(1)) as i32;
        self.h_scroll = (self.h_scroll as i32)
            .saturating_add(delta)
            .clamp(0, max.max(0)) as usize;
    }

    /// The body viewport's width as of the last draw, in columns.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The cached widest-visible-line measure without recomputing — 0 until
    /// a draw has run, which is fine for the drag math that reads it: the
    /// horizontal bar it serves only exists on drawn frames.
    fn cached_content_width(&self) -> usize {
        self.content_width.map(|(_, _, _, w)| w).unwrap_or(0)
    }

    /// The current view's text with nothing hidden — the corpus search runs
    /// over, and the coordinate space its match positions live in.
    fn search_corpus(&self) -> Vec<String> {
        match self.mode {
            ViewMode::Pretty => self
                .active_tree()
                .map(|t| t.full_text_lines())
                .unwrap_or_default(),
            ViewMode::Raw => self.raw_lines.clone(),
            ViewMode::Headers => self.header_lines.clone(),
        }
    }

    /// The active tab's full text — what the toolbar's copy/save/editor
    /// actions operate on, so they follow the tab exactly as search does
    /// (same corpus, joined into one string).
    pub fn view_text(&self) -> String {
        self.search_corpus().join("\n")
    }

    /// The raw response body, regardless of the active tab — what the
    /// "describe a filter" prompt shapes.
    pub fn body_text(&self) -> String {
        self.raw_lines.join("\n")
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.min(self.visible_len().saturating_sub(1));
    }

    /// Keeps the cursor inside the viewport after a move.
    fn follow_cursor(&mut self) {
        let h = self.height.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + h {
            self.scroll = self.cursor + 1 - h;
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        let max = self.visible_len().saturating_sub(1) as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(0, max) as usize;
        self.follow_cursor();
    }

    fn set_mode(&mut self, mode: ViewMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        // Selection coordinates live in the old mode's line space.
        self.clear_sel();
        if mode != ViewMode::Headers {
            self.body_mode = mode;
        }
        self.cursor = 0;
        self.scroll = 0;
        self.h_scroll = 0;
        // Match positions are per-view coordinates; the query survives, the
        // positions do not.
        self.recompute_matches();
    }

    fn recompute_matches(&mut self) {
        let Some(search) = &self.search else { return };
        if search.query.is_empty() {
            return;
        }
        let needle = search.query.to_lowercase();
        let mut matches = Vec::new();
        for (i, line) in self.search_corpus().iter().enumerate() {
            let hay = line.to_lowercase();
            // Column is a char offset so it lines up with rendered spans.
            let mut from = 0;
            while let Some(at) = hay[from..].find(&needle) {
                let byte = from + at;
                matches.push((i, hay[..byte].chars().count()));
                from = byte + needle.len();
            }
        }
        let search = self.search.as_mut().expect("checked above");
        search.matches = matches;
        search.current = 0;
    }

    /// Moves the cursor onto match `current`, expanding whatever containers
    /// hide it.
    fn jump_to_match(&mut self) {
        let Some(search) = &self.search else { return };
        let Some(&(line, _)) = search.matches.get(search.current) else {
            return;
        };
        match (self.mode, self.active_tree_mut()) {
            (ViewMode::Pretty, Some(tree)) => {
                tree.expand_ancestors(line);
                self.cursor = tree.visible_index_of(line).unwrap_or(0);
            }
            _ => self.cursor = line,
        }
        self.clamp_cursor();
        self.follow_cursor();
    }

    fn step_match(&mut self, delta: i32) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let len = search.matches.len();
        if len == 0 {
            return;
        }
        search.current = (search.current as i32 + delta).rem_euclid(len as i32) as usize;
        self.jump_to_match();
    }

    /// Char ranges of every match on `line`, plus the current one.
    fn match_ranges(&self, line: usize) -> LineMatches {
        let none = LineMatches {
            ranges: Vec::new(),
            current: None,
        };
        let Some(search) = &self.search else {
            return none;
        };
        if search.active || search.query.is_empty() {
            return none;
        }
        let width = search.query.chars().count();
        let ranges = search
            .matches
            .iter()
            .filter(|(l, _)| *l == line)
            .map(|(_, c)| (*c, c + width))
            .collect();
        let current = search
            .matches
            .get(search.current)
            .filter(|(l, _)| *l == line)
            .map(|(_, c)| (*c, c + width));
        LineMatches { ranges, current }
    }

    /// The text visible line `i` shows in the current mode: the verbatim
    /// raw/header line, or (Pretty) the row's indent plus its rendered
    /// tokens — the summary for a collapsed row, exactly what's painted.
    fn display_line_text(&self, i: usize) -> Option<String> {
        match self.mode {
            ViewMode::Raw => self.raw_lines.get(i).cloned(),
            ViewMode::Headers => self.header_lines.get(i).cloned(),
            ViewMode::Pretty => {
                let tree = self.active_tree()?;
                let line = tree.visible_line(i)?;
                let mut out = " ".repeat(line.indent);
                for tok in line.render_tokens() {
                    out.push_str(&tok.text);
                }
                Some(out)
            }
        }
    }

    /// Maps a screen position to a (visible line, char col) cell of the
    /// current view, through the drawn area, `scroll` and `h_scroll`.
    /// `clamp` pulls positions outside the drawn area onto its nearest
    /// cell (drag sweeps keep selecting at the edges); without it such
    /// positions return `None` (a click must land inside).
    fn cell_at(&self, x: u16, y: u16, clamp: bool) -> Option<(usize, usize)> {
        use ratatui::layout::Position;
        let area = self.last_area?;
        if area.width == 0 || area.height == 0 || self.visible_len() == 0 {
            return None;
        }
        if !clamp && !area.contains(Position { x, y }) {
            return None;
        }
        let yy = y.clamp(area.y, area.y + area.height - 1);
        let xx = x.clamp(area.x, area.x + area.width - 1);
        let line = (self.scroll + usize::from(yy - area.y)).min(self.visible_len() - 1);
        let disp_col = self.h_scroll + usize::from(xx - area.x);
        let text = self.display_line_text(line).unwrap_or_default();
        Some((line, char_cell_at_display_col(&text, disp_col)))
    }

    /// The selection's char range on visible line `i`, as half-open
    /// `[from, to)` over that line's display text (`to` may exceed the
    /// line's length — callers clamp). `None` when `i` is outside the
    /// selection.
    fn sel_range_on_line(&self, i: usize) -> Option<(usize, usize)> {
        let (a, b) = self.sel?;
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        if i < s.0 || i > e.0 {
            return None;
        }
        let from = if i == s.0 { s.1 } else { 0 };
        let to = if i == e.0 { e.1 + 1 } else { usize::MAX };
        Some((from, to))
    }

    /// The selected text, lines joined with `\n`. `None` without a
    /// selection.
    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.sel?;
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for line in s.0..=e.0 {
            if line > s.0 {
                out.push('\n');
            }
            let text = self.display_line_text(line).unwrap_or_default();
            let len = text.chars().count();
            let from = if line == s.0 { s.1.min(len) } else { 0 };
            let to = if line == e.0 { (e.1 + 1).min(len) } else { len };
            out.extend(text.chars().skip(from).take(to.saturating_sub(from)));
        }
        Some(out)
    }

    fn clear_sel(&mut self) {
        self.sel = None;
        self.sel_anchor = None;
        self.sel_word_anchor = None;
    }

    /// The inclusive cell span a double click at cell `(line, col)`
    /// selects: the word/punctuation/whitespace run around `col` (see
    /// [`word_nav::word_span_at`]), on that one line. `None` on an empty
    /// line or a col past its last char.
    fn word_cells_at(&self, line: usize, col: usize) -> Option<((usize, usize), (usize, usize))> {
        let chars: Vec<char> = self.display_line_text(line)?.chars().collect();
        let (s, e) = super::word_nav::word_span_at(&chars, col)?;
        Some(((line, s), (line, e - 1)))
    }

    /// Extends a line-wise selection by `delta` lines (shift+Up/Down): the
    /// anchor line is fixed, the cursor line moves, and every line between
    /// them is covered in full.
    fn select_line_extend(&mut self, delta: i32) {
        let anchor_line = self.sel_anchor.map(|(l, _)| l).unwrap_or(self.cursor);
        self.sel_anchor = Some((anchor_line, 0));
        self.move_cursor(delta);
        let cl = self.cursor;
        let len = |v: &Self, l: usize| v.display_line_text(l).map_or(0, |t| t.chars().count());
        self.sel = Some(if cl >= anchor_line {
            ((anchor_line, 0), (cl, len(self, cl).saturating_sub(1)))
        } else {
            (
                (anchor_line, len(self, anchor_line).saturating_sub(1)),
                (cl, 0),
            )
        });
    }
}

/// The char index of the cell at display column `col` of `text` (wide
/// chars span several columns), clamped onto the line's last char (0 when
/// empty).
fn char_cell_at_display_col(text: &str, col: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut w = 0usize;
    let mut count = 0usize;
    for (i, c) in text.chars().enumerate() {
        w += c.width().unwrap_or(0);
        if w > col {
            return i;
        }
        count = i + 1;
    }
    count.saturating_sub(1)
}

/// The search hits that fall on one rendered line, as char ranges.
struct LineMatches {
    ranges: Vec<(usize, usize)>,
    current: Option<(usize, usize)>,
}

impl LineMatches {
    /// The same ranges in the coordinates of a row that starts `char_start`
    /// chars into the line: shifted left, clipped at the row's start, and
    /// dropped when they end before it.
    fn shifted(self, char_start: usize) -> Self {
        let shift = |(s, e): (usize, usize)| {
            (e > char_start).then(|| (s.saturating_sub(char_start), e - char_start))
        };
        LineMatches {
            ranges: self.ranges.into_iter().filter_map(shift).collect(),
            current: self.current.and_then(shift),
        }
    }
}

#[derive(Default)]
pub struct Response {
    state: ResponseState,
    view: Option<ReadyView>,
    /// Whether the pane is hidden — collapsed to just its header strip,
    /// with the editor taking the freed rows (`Action::ToggleResponseCollapse`).
    /// A sticky layout preference: `Session::sync_open` carries it across
    /// request switches instead of swapping it with the response.
    /// Session-only — never persisted.
    pub collapsed: bool,
    /// Display copy of the column's split, refreshed by the app before
    /// every draw: the header's ▲/▼ step pill greys the arrow that
    /// points past an endpoint from it. The app state stays the
    /// authority; `collapsed` above is still this pane's own flag.
    pub split: crate::split::SplitState,
    /// The jq filter bar. Lives here rather than on `ReadyView` so it
    /// survives a new response landing in the same slot — the filter the
    /// user typed keeps working (once reconciled) against the new body,
    /// exactly like the address bar survives a send.
    jq: JqBar,
}

impl Response {
    pub fn state(&self) -> &ResponseState {
        &self.state
    }

    /// The only way to change state, so the view (tree, cursor, search) is
    /// always rebuilt for — and only for — the response actually on screen.
    /// `generation` tags a ready view with the send it came from, so a
    /// background pretty-print can find it again (see
    /// [`Response::attach_tree`]).
    pub fn set_state(&mut self, state: ResponseState, generation: u64) {
        self.view = match &state {
            ResponseState::Ready(data) => Some(ReadyView::new(data, generation)),
            _ => None,
        };
        self.state = state;
    }

    /// Delivers the result of a background pretty-print started for
    /// `generation`: `Some` tree to show, `None` when the body turned out
    /// not to be JSON. Returns whether it was accepted — a tree for a view
    /// this response no longer holds, or one that is not waiting for a
    /// parse, is dropped.
    pub fn attach_tree(&mut self, generation: u64, tree: Option<JsonTree>) -> bool {
        let filter_on = self.jq.enabled && !self.jq.input.text().trim().is_empty();
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        if !view.awaits_tree(generation) {
            return false;
        }
        view.parsing = false;
        view.clear_sel();
        match tree {
            Some(tree) => {
                view.tree = Some(tree);
                view.tree_epoch += 1;
                // A switched-on filter is about to run against this tree
                // (see the `jq_applied` reset below): keep the spinner up
                // until its output lands rather than showing the full
                // body first.
                view.awaiting_filter = filter_on;
                // A filter typed while the body was still parsing has
                // nothing to run against yet (`apply_jq` bails out); now
                // that the body tree exists, forget what was "applied" so
                // the app's reconcile step re-runs the pending filter.
                view.jq_applied = None;
                // Already on the Tree tab (watching the spinner): the tree
                // it was waiting for is now the view's content.
                if view.mode == ViewMode::Pretty {
                    view.cursor = 0;
                    view.scroll = 0;
                    view.recompute_matches();
                }
            }
            // Not JSON after all: the Tree tab disappears, exactly as it
            // never appears for a small non-JSON body.
            None => view.set_mode(ViewMode::Raw),
        }
        true
    }

    /// Whether this response is still waiting on the background parse
    /// started for `generation` — how [`crate::session::Session`] finds the
    /// slot a finished parse belongs to.
    pub fn awaits_tree(&self, generation: u64) -> bool {
        self.view
            .as_ref()
            .is_some_and(|v| v.awaits_tree(generation))
    }

    pub fn view(&self) -> Option<&ReadyView> {
        self.view.as_ref()
    }

    /// The jq bar's current text.
    pub fn jq_text(&self) -> &str {
        self.jq.input.text()
    }

    /// Sets the bar's text programmatically (the editor ↔ bar sync), cursor
    /// at the end. Not an edit — [`Response::take_jq_edited`] is unaffected.
    pub fn set_jq_text(&mut self, text: &str) {
        self.jq.input = LineInput::new(text);
        // The text just changed underneath any previous error/stale span —
        // `apply_jq` re-derives both against the new text right after this
        // call, so a leftover from before must not linger if it bails
        // early (e.g. the new response isn't JSON).
        self.jq.error = None;
        self.jq.stale = false;
        self.jq.note = None;
        self.jq.pending = None;
    }

    /// Clears the filter: wipes the bar's text (an edit, when there was
    /// any) so the full body shows. Focus is untouched — from the bar the
    /// caret stays put for the next filter; from the tree the empty,
    /// unfocused bar simply disappears. The on/off switch is left on —
    /// there is nothing left to be off — so the request never persists
    /// `jq_enabled = false` without a filter.
    pub fn clear_jq(&mut self) {
        if !self.jq.input.text().is_empty() {
            self.jq.input = LineInput::new("");
            self.jq.edited = true;
        }
        self.jq.enabled = true;
    }

    /// Sets the bar's text and cursor together (a tee-up from elsewhere —
    /// e.g. the AI describe flow seeding a filter). Counts as an edit, and
    /// switches a closed bar back on: a verb or the AI landing a filter
    /// means "show me this".
    pub fn set_jq_text_with_cursor(&mut self, text: &str, cursor: usize) {
        self.jq.input = LineInput::new(text);
        self.jq.input.set_cursor(cursor);
        self.jq.enabled = true;
        self.jq.edited = true;
    }

    /// The bar's on/off switch as the editor holds it (the persisted
    /// state). Not an edit.
    pub fn set_jq_enabled(&mut self, enabled: bool) {
        self.jq.enabled = enabled;
    }

    pub fn jq_enabled(&self) -> bool {
        self.jq.enabled
    }

    /// Whether the bar is showing: focused, holding a switched-on filter,
    /// or waiting on a run or an AI reply. Closed means the filter is off.
    pub fn jq_open(&self) -> bool {
        self.jq.is_open()
    }

    /// Opens the bar: switches the filter on (an edit, when that changes
    /// anything) and focuses the input. `false` when jq has nothing to run
    /// against, in which case nothing changes.
    pub fn open_jq(&mut self) -> bool {
        if !self.jq_available() {
            return false;
        }
        if !self.jq.enabled {
            self.jq.enabled = true;
            self.jq.edited = true;
        }
        self.jq.focused = true;
        true
    }

    /// Closes the bar: blurs it and, when there is text to keep, switches
    /// the filter off (an edit — the request remembers). An empty bar just
    /// goes away; there is nothing to switch off.
    pub fn close_jq(&mut self) {
        self.jq.focused = false;
        if self.jq.enabled && !self.jq.input.text().is_empty() {
            self.jq.enabled = false;
            self.jq.edited = true;
        }
    }

    /// Whether the bar's text changed since the last call — set on every
    /// edit, cleared here so the app's reconcile step runs each change
    /// exactly once.
    pub fn take_jq_edited(&mut self) -> bool {
        std::mem::take(&mut self.jq.edited)
    }

    pub fn jq_focused(&self) -> bool {
        self.jq.focused
    }

    /// Focuses (or blurs) the jq bar. Returns whether it took: focusing
    /// fails with no ready view or a body jq can't run against.
    pub fn set_jq_focus(&mut self, focused: bool) -> bool {
        if focused && !self.jq_available() {
            return false;
        }
        self.jq.focused = focused;
        true
    }

    pub fn jq_bar(&self) -> &JqBar {
        &self.jq
    }

    pub fn jq_bar_mut(&mut self) -> &mut JqBar {
        &mut self.jq
    }

    /// Forgets an outstanding background jq run when this response is
    /// being stashed or restored across a request switch: the run's
    /// `JqRunFinished` is bound for a `Response` that has moved into (or
    /// out of) the session cache and won't be looked up again by the
    /// generation/run it was started for, so it would otherwise arrive to
    /// nothing and leave `jq_applied` claiming a filter that was never
    /// actually run. Resetting `jq_applied` to `None` makes `sync_jq`'s
    /// next reconcile re-apply the filter (a fresh run) instead of no-oping
    /// on a filter it only ever *started*.
    /// Whether this response holds the derived state a big body is worth
    /// shedding from a cache slot: a tree or a jq document over a body past
    /// the background-parse threshold. Small bodies are never shed — their
    /// tree is cheap and re-parsing it would be pure churn.
    pub fn holds_big_derived(&self) -> bool {
        let Some(view) = self.view.as_ref() else {
            return false;
        };
        let big =
            matches!(&self.state, ResponseState::Ready(d) if d.body.len() > SYNC_PRETTY_BYTES);
        big && (view.tree.is_some() || view.jq_doc.is_some())
    }

    /// Drops the tree, the filtered tree, the jq document and the raw
    /// column index — everything derived from the body — while keeping the
    /// body, headers and view settings (tab, cursor, scroll, filter). The
    /// response reads as "parsing" again, so the Tree tab stays and shows
    /// its spinner once the response is back on screen and
    /// `take_reparse` has restarted the parse; `attach_tree` then clears
    /// `jq_applied` so the filter re-runs against the fresh tree.
    pub fn shed_derived(&mut self) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        view.tree = None;
        view.set_jq_tree(None);
        view.jq_doc = None;
        view.jq_tree_code = None;
        view.jq_applied = None;
        view.jq_outputs = 1;
        view.raw_marks.clear();
        view.content_width = None;
        view.clear_sel();
        view.parsing = true;
        view.shed = true;
        self.jq.pending = None;
        // Keep the sequence counter so a stale in-flight pool fetch can't
        // collide with a fresh fetch's `seq` after a shed.
        self.jq.completion = JqCompletion {
            seq: self.jq.completion.seq,
            ..JqCompletion::default()
        };
    }

    /// The parse a shed response needs now that it is back on screen: its
    /// generation and body, once — the caller hands them to the same
    /// background parse a fresh arrival gets. `None` when nothing was shed.
    pub fn take_reparse(&mut self) -> Option<(u64, String)> {
        let view = self.view.as_mut()?;
        if !view.shed {
            return None;
        }
        view.shed = false;
        view.parse_started = Instant::now();
        let body = match &self.state {
            ResponseState::Ready(d) => d.body.clone(),
            _ => return None,
        };
        Some((view.generation, body))
    }

    pub fn drop_pending_jq(&mut self) {
        if self.jq.pending.take().is_some()
            && let Some(view) = self.view.as_mut()
        {
            view.jq_applied = None;
        }
        self.jq.completion.pending = None;
    }

    /// Whether jq has anything to run against: a ready view with a parsed
    /// (or still-parsing) JSON body.
    pub fn jq_available(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.has_tree_view())
    }

    /// Pastes into the jq bar (the bracketed-paste/ctrl+v path). `false`
    /// while the bar isn't focused, mirroring `paste_into_search`.
    pub fn paste_into_jq(&mut self, text: &str) -> bool {
        if !self.jq.focused {
            return false;
        }
        self.jq.input.paste(text);
        self.jq.edited = true;
        true
    }

    /// The tree the `Pretty` view is showing: the filtered tree while a jq
    /// filter is applied, otherwise the body tree.
    pub fn active_tree(&self) -> Option<&JsonTree> {
        self.view.as_ref().and_then(|v| v.active_tree())
    }

    /// How many outputs the applied filter produced (1 when unfiltered, or
    /// the filter emits exactly one document).
    /// The filter whose output the Pretty view is showing — what a
    /// structural verb composes onto. `None` when the body tree shows.
    pub fn jq_tree_code(&self) -> Option<&str> {
        self.view.as_ref().and_then(|v| v.jq_tree_code.as_deref())
    }

    pub fn jq_output_count(&self) -> usize {
        self.view.as_ref().map_or(1, |v| v.jq_outputs)
    }

    /// Ensures the view reflects filter `code`: runs it inline when the
    /// body is small enough (`sync_limit`, in bytes), or hands the run to
    /// the caller for the blocking pool when it's not. `None` means either
    /// nothing to do (already applied, or nothing to filter yet) or that
    /// the run finished inline; `Some` is work the app must complete with
    /// [`Response::attach_jq_result`].
    pub fn apply_jq(&mut self, code: &str, sync_limit: usize) -> Option<JqRunRequest> {
        let Some(view) = self.view.as_mut() else {
            // No ready view at all: jq is disabled, not merely stale —
            // nothing left over from a previous response should linger.
            self.jq.error = None;
            self.jq.stale = false;
            self.jq.note = None;
            self.jq.pending = None;
            return None;
        };
        if !view.has_tree_view() {
            // A non-JSON (or not-yet-parsed) body: same as above — jq has
            // nothing to run against, so any error/staleness left behind
            // by a filter typed against a previous, JSON response must not
            // survive as a stale, undrawable leftover.
            self.jq.error = None;
            self.jq.stale = false;
            self.jq.note = None;
            self.jq.pending = None;
            return None;
        }
        if view.jq_applied.as_deref() == Some(code) {
            return None;
        }
        let code_trim = code.trim();
        if code_trim.is_empty() {
            view.awaiting_filter = false;
            view.set_jq_tree(None);
            view.jq_tree_code = None;
            view.jq_outputs = 1;
            view.jq_applied = Some(code.to_string());
            self.jq.error = None;
            self.jq.stale = false;
            self.jq.note = None;
            self.jq.pending = None;
            view.clear_sel();
            view.clamp_cursor();
            view.follow_cursor();
            view.h_scroll = 0;
            view.recompute_matches();
            return None;
        }
        if let Err(e) = postui_core::jq::check(code_trim) {
            view.awaiting_filter = false;
            self.jq.error = Some(e);
            self.jq.stale = true;
            self.jq.note = None;
            self.jq.pending = None;
            view.jq_applied = Some(code.to_string());
            return None;
        }
        // Body parse still running: nothing to filter yet; reconcile
        // re-applies once `attach_tree` lands.
        view.tree.as_ref()?;
        let body_len = view.raw_lines.iter().map(|l| l.len() + 1).sum::<usize>();
        if view.jq_doc.is_none() && body_len <= sync_limit {
            match JqDocument::parse(&view.raw_lines.join("\n")) {
                Ok(doc) => view.jq_doc = Some(doc),
                Err(e) => {
                    self.jq.error = Some(e);
                    self.jq.stale = true;
                    self.jq.note = None;
                    view.jq_applied = Some(code.to_string());
                    return None;
                }
            }
        }
        view.jq_applied = Some(code.to_string());
        view.jq_runs += 1;
        let run = view.jq_runs;
        if body_len <= sync_limit {
            let doc = view.jq_doc.clone().expect("parsed above");
            // Already cached on `view.jq_doc` above: no document to hand
            // back, just the outputs.
            let result = postui_core::jq::run(code_trim, &doc)
                .map(|outputs| JqRunOutput::from_outputs(None, outputs));
            self.jq.pending = None;
            let generation = view.generation;
            self.attach_jq_result(generation, run, result);
            return None;
        }
        self.jq.pending = Some(run);
        self.jq.pending_since = Instant::now();
        let doc = view.jq_doc.clone();
        let body = doc.is_none().then(|| view.raw_lines.join("\n"));
        Some(JqRunRequest {
            generation: view.generation,
            run,
            code: code_trim.to_string(),
            doc,
            body,
        })
    }

    pub fn set_jq_tab(&mut self, tab: JqTab) {
        self.jq.tab = tab;
    }

    pub fn jq_tab(&self) -> JqTab {
        self.jq.tab
    }

    pub fn jq_ghost(&self) -> Option<&str> {
        self.jq.ghost()
    }

    /// Recomputes the bar's completion for the caret's position. Runs
    /// after every `apply_jq` in the app's reconcile. Keys for a new
    /// context are fetched inline for a body under `sync_limit`, else
    /// the returned request goes to the blocking pool and the ghost
    /// stays empty until `attach_jq_completion` lands it.
    pub fn refresh_jq_completion(&mut self, sync_limit: usize) -> Option<JqCompleteRequest> {
        let bar = &mut self.jq;
        let focused = bar.focused;
        let at_end = bar.caret_at_end();
        let text = bar.input.text().to_string();
        let c = &mut bar.completion;
        if !focused || !at_end {
            c.ctx = None;
            c.candidates.clear();
            c.index = 0;
            return None;
        }
        let ctx = complete::context(&text);
        if ctx != c.ctx {
            c.index = 0;
        }
        c.ctx = ctx;
        let Some(ctx) = c.ctx.clone() else {
            c.candidates.clear();
            return None;
        };
        let expr = match ctx.kind {
            Kind::Word => {
                c.candidates = complete::candidates(&ctx, &[]);
                return None;
            }
            Kind::Key { .. } => ctx.input_expr.clone().unwrap_or_else(|| ".".into()),
        };
        if c.cached_expr.as_deref() == Some(expr.as_str()) {
            c.candidates = complete::candidates(&ctx, &c.cached_keys);
            return None;
        }
        c.candidates.clear();
        if c.pending.as_ref().is_some_and(|(_, e)| *e == expr) {
            return None;
        }
        let Some(view) = self.view.as_mut() else {
            return None;
        };
        let body_len = view.raw_lines.iter().map(|l| l.len() + 1).sum::<usize>();
        if view.jq_doc.is_none() && view.has_tree_view() && body_len <= sync_limit {
            view.jq_doc = JqDocument::parse(&view.raw_lines.join("\n")).ok();
        }
        let Some(doc) = view.jq_doc.clone() else {
            return None;
        };
        if body_len <= sync_limit {
            c.cached_keys = complete::keys_at(&expr, &doc);
            c.cached_expr = Some(expr);
            c.candidates = complete::candidates(&ctx, &c.cached_keys);
            return None;
        }
        c.seq += 1;
        c.pending = Some((c.seq, expr.clone()));
        Some(JqCompleteRequest {
            generation: view.generation,
            seq: c.seq,
            input_expr: expr,
            doc,
        })
    }

    /// Lands a pool key fetch. Dropped unless `generation` is the view on
    /// screen and `seq` is the newest fetch. The keys are cached either
    /// way; the ghost updates only if the caret's context still asks for
    /// that expression. Returns whether the ghost changed.
    pub fn attach_jq_completion(
        &mut self,
        generation: u64,
        seq: u64,
        input_expr: String,
        keys: Vec<String>,
    ) -> bool {
        if self.view.as_ref().is_none_or(|v| v.generation != generation) {
            return false;
        }
        let c = &mut self.jq.completion;
        if c.pending() != Some(seq) {
            return false;
        }
        c.pending = None;
        c.cached_keys = keys;
        c.cached_expr = Some(input_expr);
        if let Some(ctx) = &c.ctx
            && ctx.input_expr.as_deref() == c.cached_expr.as_deref()
        {
            c.candidates = complete::candidates(ctx, &c.cached_keys);
            c.index = 0;
            return true;
        }
        false
    }

    /// Inserts the showing candidate as if typed: a plain insert at the
    /// caret, or — for a key that needs quoting — a replacement of the
    /// token from its `.` (selected, then pasted over).
    fn accept_jq_completion(&mut self) {
        let Some(cand) = self.jq.candidate().cloned() else {
            return;
        };
        let input = &mut self.jq.input;
        if let Some(from) = cand.replace_from {
            let from_chars = input.text()[..from].chars().count();
            input.set_cursor(from_chars);
            input.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT));
            input.paste(&cand.insert);
        } else {
            input.insert_str(&cand.insert);
        }
        self.jq.edited = true;
    }

    /// Accepts (or drops) the result of a background jq run. Dropped when
    /// the view it was started for is gone, or a later run for the same
    /// view has since started (superseded). Returns whether it was
    /// accepted.
    pub fn attach_jq_result(
        &mut self,
        generation: u64,
        run: u64,
        result: Result<JqRunOutput, JqError>,
    ) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        if view.generation != generation {
            return false;
        }
        // Adopt the parsed document even from a superseded run (a stale
        // `run` counter): the parse itself is still good for this view, and
        // caching it here means the next `apply_jq` doesn't have to ask the
        // worker to re-parse a body that may be huge, on every keystroke.
        if let Ok(JqRunOutput { doc: Some(doc), .. }) = &result
            && view.jq_doc.is_none()
        {
            view.jq_doc = Some(doc.clone());
        }
        if view.jq_runs != run {
            return false;
        }
        self.jq.pending = None;
        view.awaiting_filter = false;
        match result {
            Ok(JqRunOutput { doc, outputs, tree }) => {
                if let Some(doc) = doc {
                    view.jq_doc = Some(doc);
                }
                match tree {
                    Some(_) if outputs.is_empty() || all_null(&outputs) => {
                        // Nothing to show: keep the full body up with a
                        // note rather than replace it with `null`s.
                        view.jq_outputs = 1;
                        view.set_jq_tree(None);
                        view.jq_tree_code = None;
                        self.jq.error = None;
                        self.jq.stale = true;
                        self.jq.note = Some(if outputs.is_empty() {
                            "no output"
                        } else {
                            "null"
                        });
                        view.clear_sel();
                        view.clamp_cursor();
                        view.follow_cursor();
                        view.h_scroll = 0;
                        view.recompute_matches();
                    }
                    Some(tree) => {
                        view.jq_outputs = outputs.len();
                        view.set_jq_tree(Some(tree));
                        view.jq_tree_code = view.jq_applied.clone();
                        self.jq.error = None;
                        self.jq.stale = false;
                        self.jq.note = None;
                        view.clear_sel();
                        view.clamp_cursor();
                        view.follow_cursor();
                        view.h_scroll = 0;
                        view.recompute_matches();
                    }
                    None => {
                        self.jq.error = Some(JqError::Runtime {
                            message: "filter output is not JSON".into(),
                        });
                        self.jq.stale = true;
                        self.jq.note = None;
                    }
                }
            }
            Err(e) => {
                self.jq.error = Some(e);
                self.jq.stale = true;
                self.jq.note = None;
            }
        }
        true
    }

    /// Pastes into the in-pane search's live input (the bracketed-paste/
    /// ctrl+v path). `false` when no search input is active — a committed
    /// query or no search at all — mirroring `ready_key`'s "an active
    /// search input swallows everything" gate.
    pub fn paste_into_search(&mut self, text: &str) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        match view.search.as_mut() {
            Some(search) if search.active => {
                search.input.paste(text);
                true
            }
            _ => false,
        }
    }

    /// The body view's scroll state, as of the last draw (the viewport height
    /// is a render-time fact, so this is `None` before the first frame).
    pub fn scrollbar_spec(&self) -> Option<ScrollbarSpec> {
        let view = self.view.as_ref()?;
        if view.height == 0 {
            return None;
        }
        Some(ScrollbarSpec {
            pane: PaneId::Response,
            offset: view.scroll,
            content: view.visible_len(),
            viewport: view.height,
        })
    }

    /// Moves the body viewport `delta` columns right (negative: left) — the
    /// horizontal counterpart of `handle_scroll`, fed by shift+wheel, the
    /// ←/→ keys, and horizontal track clicks.
    pub fn handle_scroll_h(&mut self, delta: i16) {
        if let Some(view) = self.view.as_mut() {
            view.scroll_h(delta as i32);
        }
    }

    /// Jumps the body viewport to column `offset` (horizontal thumb drag),
    /// clamped the same way the wheel is. Returns true when it moved.
    pub fn set_scroll_h(&mut self, offset: usize) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        let max = view.content_width().saturating_sub(view.width.max(1));
        let next = offset.min(max);
        let changed = next != view.h_scroll;
        view.h_scroll = next;
        changed
    }

    /// The horizontal scroll state the last-drawn frame's bottom bar
    /// reflects — the same numbers `draw_h_indicator` painted from, so the
    /// drag math and the drawn thumb can never disagree. `None` when
    /// nothing is clipped (no bar on screen).
    pub fn h_scrollbar_spec(&self) -> Option<ScrollbarSpec> {
        let view = self.view.as_ref()?;
        let spec = ScrollbarSpec {
            pane: PaneId::Response,
            offset: view.h_scroll,
            content: view.cached_content_width(),
            viewport: view.width,
        };
        spec.overflows().then_some(spec)
    }

    /// Jumps the body view to `offset` (scrollbar drag). Clamped the same way
    /// `handle_scroll` clamps the wheel.
    pub fn set_scroll(&mut self, offset: usize) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        let max = view.visible_len().saturating_sub(view.height.max(1));
        let next = offset.min(max);
        let changed = next != view.scroll;
        view.scroll = next;
        changed
    }

    /// Switches to `mode` (the tabs row's click target). A no-op with no
    /// ready response, and a no-op switching to `Pretty` when the body has
    /// no tree and none is coming (not JSON). While a background parse is
    /// running `Pretty` is allowed: it shows the wait, then the tree.
    pub fn set_view_mode(&mut self, mode: ViewMode) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        if mode == ViewMode::Pretty && !view.has_tree_view() {
            return;
        }
        view.set_mode(mode);
    }

    /// Opens the in-pane search, exactly as `/` does (the search button).
    pub fn open_search(&mut self) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        view.open_search();
        true
    }

    /// Steps to the next (`1`) or previous (`-1`) match, exactly as `n`/`N`
    /// do (the `▼`/`▲` buttons).
    ///
    /// A still-live input is committed first, exactly as `Enter` does, and
    /// that commit *is* the step: matches only highlight once the query is
    /// committed, so a click on `▼` straight after typing must land on the
    /// first match rather than silently skipping it. Without this the whole
    /// mouse route into search dead-ended — nothing highlighted, and the
    /// buttons looked broken (found by the stage-7 tmux sweep).
    pub fn step_search(&mut self, delta: i32) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        let Some(search) = view.search.as_mut() else {
            return false;
        };
        if search.active {
            search.active = false;
            search.query = search.input.text().to_string();
            view.recompute_matches();
            view.jump_to_match();
            return true;
        }
        view.step_match(delta);
        true
    }

    /// Clicking a body row: moves the cursor there, and — when `toggle` is
    /// set and the view is `Pretty` — collapses/expands the container it
    /// opens (a no-op on a scalar row).
    pub fn click_row(&mut self, row: usize, toggle: bool) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        view.cursor = row.min(view.visible_len().saturating_sub(1));
        let cursor = view.cursor;
        if toggle
            && view.mode == ViewMode::Pretty
            && let Some(tree) = view.active_tree_mut()
        {
            tree.toggle(cursor);
            // Collapsing/expanding renumbers the visible lines a selection
            // is addressed in.
            view.clear_sel();
            view.clamp_cursor();
            view.follow_cursor();
        }
    }

    /// Plants a selection anchor at the screen position of a left click in
    /// the body content (clearing any previous selection). Returns whether
    /// an anchor was planted — the caller arms the drag sweep on `true`.
    pub fn begin_selection_at(&mut self, x: u16, y: u16) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        view.clear_sel();
        let Some(cell) = view.cell_at(x, y, false) else {
            return false;
        };
        view.sel_anchor = Some(cell);
        true
    }

    /// Double-click word select: selects the character run under the click
    /// (word, punctuation or whitespace — the same classes `word_nav` hops
    /// by) and plants the word anchor a following drag extends from, whole
    /// words at a time. A click past a line's last character selects
    /// nothing (and returns `false`: no sweep to arm).
    pub fn select_word_at(&mut self, x: u16, y: u16) -> bool {
        use unicode_width::UnicodeWidthChar;
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        view.clear_sel();
        let Some((line, col)) = view.cell_at(x, y, false) else {
            return false;
        };
        // `cell_at` clamps a click past the line end onto its last char;
        // a double click there should select nothing, so re-check the
        // clicked display column against the line's width.
        let area = view.last_area.expect("cell_at resolved through last_area");
        let disp_col = view.h_scroll + usize::from(x.saturating_sub(area.x));
        let text = view.display_line_text(line).unwrap_or_default();
        if disp_col >= text.chars().map(|c| c.width().unwrap_or(0)).sum() {
            return false;
        }
        let Some(span) = view.word_cells_at(line, col) else {
            return false;
        };
        view.sel = Some(span);
        view.sel_anchor = Some(span.0);
        view.sel_word_anchor = Some(span);
        true
    }

    /// Extends the selection sweep to the drag point (clamped onto the
    /// body area, so sweeping past an edge keeps selecting the nearest
    /// cells). At the ragged edges the pointer reads as a boundary: a
    /// downward sweep onto a row's first cell ends at the end of the
    /// line above, and an upward sweep past a row's end starts at the
    /// start of the row below. Dragging back onto the anchor cell
    /// collapses the selection.
    /// After a double click (word anchor set) the sweep extends by whole
    /// words instead: the selection is the union of the anchor word and
    /// the run under the pointer, so the anchor word always stays covered.
    pub fn drag_selection_to(&mut self, x: u16, y: u16) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        let Some(head) = view.cell_at(x, y, true) else {
            return false;
        };
        if let Some((ws, we)) = view.sel_word_anchor {
            let (hs, he) = view.word_cells_at(head.0, head.1).unwrap_or((head, head));
            view.sel = Some(if head > we {
                (ws, he)
            } else if head < ws {
                (hs, we)
            } else {
                (ws, we)
            });
            return true;
        }
        let Some(anchor) = view.sel_anchor else {
            return false;
        };
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
        if head.1 == 0 && head.0 > anchor.0 {
            let row = head.0 - 1;
            let last = view
                .display_line_text(row)
                .map_or(0, |t| t.chars().count().saturating_sub(1));
            view.sel = Some((anchor, (row, last)));
            return true;
        }
        if head.0 < anchor.0 {
            // `cell_at` clamped a pointer past the row's end onto its
            // last char; re-check the pointed display column against the
            // row's width to tell the two apart (an empty row has no
            // last char to spuriously grab — it stays in the sweep).
            use unicode_width::UnicodeWidthChar;
            let area = view.last_area.expect("cell_at resolved through last_area");
            let xx = x.clamp(area.x, area.x + area.width.max(1) - 1);
            let disp_col = view.h_scroll + usize::from(xx - area.x);
            let text = view.display_line_text(head.0).unwrap_or_default();
            let width: usize = text.chars().map(|c| c.width().unwrap_or(0)).sum();
            if width > 0 && disp_col >= width {
                view.sel = Some((anchor, (head.0 + 1, 0)));
                return true;
            }
        }
        view.sel = (head != anchor).then_some((anchor, head));
        true
    }

    /// The selected response text, if any.
    pub fn selected_text(&self) -> Option<String> {
        self.view.as_ref()?.selected_text()
    }

    /// Clears any selection; returns whether there was one.
    pub fn clear_selection(&mut self) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        let had = view.sel.is_some();
        view.clear_sel();
        had
    }

    /// Ready-state key handling. Split out so [`Component::handle_key`] stays
    /// a readable state dispatch.
    fn ready_key(&mut self, ev: KeyEvent) -> Option<Action> {
        // The jq bar swallows everything while focused: chars and editing
        // keys go to its LineInput, Enter blurs (committing is implicit —
        // every edit re-runs the filter — so the filter stays on), Esc
        // clears the filter and keeps the caret (a second Esc, on the
        // now-empty bar, leaves it) unless an AI request is pending, in
        // which case it cancels that instead. Runs before the
        // view is borrowed, so it works even with no ready view (it never
        // should, in practice: the bar can't focus without one).
        if self.jq.focused {
            // A completion ghost is showing: Tab steps or accepts per
            // `jq_tab`, Right/End accept, everything else falls through
            // to the input (and re-derives the ghost in the reconcile).
            if self.jq.ghost().is_some() {
                let plain = ev.modifiers.is_empty();
                let cycle = self.jq.tab == JqTab::Cycle;
                match ev.code {
                    KeyCode::Tab if plain && cycle => {
                        self.jq.completion.step(true);
                        return Some(Action::Render);
                    }
                    KeyCode::BackTab => {
                        if cycle {
                            self.jq.completion.step(false);
                        }
                        return Some(Action::Render);
                    }
                    KeyCode::Tab | KeyCode::Right | KeyCode::End if plain => {
                        self.accept_jq_completion();
                        return Some(Action::Render);
                    }
                    _ => {}
                }
            }
            match ev.code {
                KeyCode::Enter => self.jq.focused = false,
                KeyCode::Esc => {
                    if self.jq.ai_pending {
                        return Some(Action::CancelJqDescribe);
                    }
                    if self.jq.input.text().is_empty() {
                        self.jq.focused = false;
                    } else {
                        self.clear_jq();
                    }
                }
                _ => {
                    self.jq.input.handle_key(ev);
                    self.jq.edited = true;
                }
            }
            return Some(Action::Render);
        }

        let view = self.view.as_mut()?;

        // An active search input swallows everything: chars and editing keys
        // go to the LineInput, Enter commits, Esc closes.
        if view.search.as_ref().is_some_and(|s| s.active) {
            match ev.code {
                KeyCode::Enter => {
                    let search = view.search.as_mut().expect("checked above");
                    search.active = false;
                    search.query = search.input.text().to_string();
                    view.recompute_matches();
                    view.jump_to_match();
                }
                KeyCode::Esc => view.search = None,
                _ => {
                    let search = view.search.as_mut().expect("checked above");
                    search.input.handle_key(ev);
                }
            }
            return Some(Action::Render);
        }

        match ev.code {
            KeyCode::Char('r') => {
                if view.has_tree_view() {
                    let next = match view.body_mode {
                        ViewMode::Pretty => ViewMode::Raw,
                        _ => ViewMode::Pretty,
                    };
                    // Dispatched as an action rather than mutated here so it
                    // funnels through `app.rs`'s `Action::ResponseViewMode`
                    // arm — the one place the animated tab underline is
                    // retargeted — exactly like a tab click.
                    Some(Action::ResponseViewMode(next))
                } else {
                    Some(Action::Render)
                }
            }
            KeyCode::Char('c') if view.mode == ViewMode::Headers => Some(Action::CopyToClipboard(
                CopyTarget::ResponseHeader(view.cursor),
            )),
            KeyCode::Char('h') => {
                let next = if view.mode == ViewMode::Headers {
                    view.body_mode
                } else {
                    ViewMode::Headers
                };
                Some(Action::ResponseViewMode(next))
            }
            KeyCode::Down if ev.modifiers.contains(KeyModifiers::SHIFT) => {
                view.select_line_extend(1);
                Some(Action::Render)
            }
            KeyCode::Up if ev.modifiers.contains(KeyModifiers::SHIFT) => {
                view.select_line_extend(-1);
                Some(Action::Render)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                view.clear_sel();
                view.move_cursor(1);
                Some(Action::Render)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                view.clear_sel();
                view.move_cursor(-1);
                Some(Action::Render)
            }
            KeyCode::PageDown => {
                view.move_cursor(view.height.max(1) as i32);
                Some(Action::Render)
            }
            KeyCode::PageUp => {
                view.move_cursor(-(view.height.max(1) as i32));
                Some(Action::Render)
            }
            KeyCode::Right => {
                view.scroll_h(H_SCROLL_STEP.into());
                Some(Action::Render)
            }
            KeyCode::Left => {
                view.scroll_h((-H_SCROLL_STEP).into());
                Some(Action::Render)
            }
            // Document jumps come before the line jumps so the plain
            // Home/End arms (which don't check modifiers) can't shadow them.
            KeyCode::Home if ev.modifiers.contains(KeyModifiers::CONTROL) => {
                view.cursor = 0;
                view.follow_cursor();
                Some(Action::Render)
            }
            KeyCode::End if ev.modifiers.contains(KeyModifiers::CONTROL) => {
                view.cursor = view.visible_len().saturating_sub(1);
                view.follow_cursor();
                Some(Action::Render)
            }
            KeyCode::Home => {
                view.h_scroll = 0;
                Some(Action::Render)
            }
            KeyCode::End => {
                view.scroll_h(i32::MAX);
                Some(Action::Render)
            }
            KeyCode::Char('g') => {
                view.cursor = 0;
                view.follow_cursor();
                Some(Action::Render)
            }
            KeyCode::Char('G') => {
                view.cursor = view.visible_len().saturating_sub(1);
                view.follow_cursor();
                Some(Action::Render)
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                let cursor = view.cursor;
                if view.mode == ViewMode::Pretty
                    && let Some(tree) = view.active_tree_mut()
                {
                    tree.toggle(cursor);
                    view.clear_sel();
                    view.clamp_cursor();
                    view.follow_cursor();
                }
                Some(Action::Render)
            }
            KeyCode::Char('/') => {
                view.open_search();
                Some(Action::Render)
            }
            KeyCode::Char('n') => {
                view.step_match(1);
                Some(Action::Render)
            }
            KeyCode::Char('N') => {
                view.step_match(-1);
                Some(Action::Render)
            }
            KeyCode::Esc if view.sel.is_some() => {
                view.clear_sel();
                Some(Action::Render)
            }
            KeyCode::Esc if view.search.is_some() => {
                view.search = None;
                Some(Action::Render)
            }
            // Innermost thing first: selection, then search, then the jq
            // filter — so Enter-to-browse, Esc-to-drop-the-filter reads as
            // one gesture from the tree. Clears rather than switches off:
            // Esc is "get rid of it", the 󰈲 button/alt+q are the switch.
            KeyCode::Esc if self.jq.is_open() => {
                self.clear_jq();
                Some(Action::Render)
            }
            _ => None,
        }
    }
}

impl Component for Response {
    fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match self.state {
            ResponseState::InFlight { .. } if ev.code == KeyCode::Esc => Some(Action::CancelSend),
            // Modified combos belong to the global keymap, not the pane —
            // except ctrl+Home/ctrl+End, the standard document-top/bottom
            // jumps, which nothing global binds.
            ResponseState::Ready(_)
                if !ev
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    || (ev.modifiers == KeyModifiers::CONTROL
                        && matches!(ev.code, KeyCode::Home | KeyCode::End)) =>
            {
                self.ready_key(ev)
            }
            _ => None,
        }
    }

    fn handle_scroll(&mut self, delta: i16) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        // Stop once the last line reaches the bottom of the viewport, rather
        // than letting the document scroll off the top entirely.
        let max = view.visible_len().saturating_sub(view.height.max(1)) as i32;
        view.scroll = (view.scroll as i32 + delta as i32).clamp(0, max) as usize;
    }

    fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        let inner = pane_surface(frame.buffer_mut(), area, ctx.theme);
        let t = ctx.theme;

        let data = match &self.state {
            ResponseState::Ready(data) => data,
            other => {
                // No response yet: row 0 is the same panel-tone header
                // strip the ready pane has — a status-shaped chip naming
                // the state in the status chip's slot, the step pill at
                // the right — so the pane reads as a pane with a header
                // before the first send too. Hidden, that strip is the
                // whole pane.
                let strip = Rect {
                    height: inner.height.min(COLLAPSED_HEIGHT),
                    ..inner
                };
                draw_pending_strip(frame.buffer_mut(), hits, strip, other, self.split, ctx);
                if self.collapsed || inner.height <= COLLAPSED_HEIGHT {
                    return;
                }
                let inner = Rect {
                    y: inner.y + COLLAPSED_HEIGHT,
                    height: inner.height - COLLAPSED_HEIGHT,
                    ..inner
                };
                let muted = Style::default().fg(t.text_muted);
                let lines = match other {
                    ResponseState::Empty => vec![
                        Line::raw(""),
                        Line::styled("Send a request — the response will appear here.", muted),
                    ],
                    ResponseState::InFlight { started } => {
                        let e = started.elapsed();
                        let frame_i = (e.subsec_millis() / 100) as usize % SPINNER.len();
                        let mut lines = vec![
                            Line::raw(""),
                            Line::styled(
                                format!("{} sending… {}", SPINNER[frame_i], human_elapsed(e)),
                                muted,
                            ),
                            Line::styled("esc to cancel", muted),
                        ];
                        // No client timeout exists (the user decides when to
                        // give up), so a slow server warns instead of dying.
                        if e >= LONG_WAIT_WARNING_AFTER {
                            lines.push(Line::raw(""));
                            lines.push(Line::styled(
                                "this is taking a while — the server hasn't responded yet",
                                Style::default().fg(t.warning),
                            ));
                        }
                        lines
                    }
                    ResponseState::Failed(err) => vec![
                        Line::raw(""),
                        Line::styled(err.clone(), Style::default().fg(t.error)),
                    ],
                    ResponseState::Cancelled => {
                        vec![Line::raw(""), Line::styled("Request cancelled", muted)]
                    }
                    ResponseState::Ready(_) => unreachable!("handled above"),
                };
                let widget = Paragraph::new(lines).style(muted).centered();
                frame.render_widget(widget, inner);
                return;
            }
        };
        let Some(view) = self.view.as_mut() else {
            return;
        };

        let footer = view.search.is_some();
        // Open while focused, holding a switched-on filter, an AI request
        // is pending, or a background run is outstanding — so the bar
        // stays up through a run that outlives the keystroke that started
        // it, and a bad filter's error row (the second `jq_rows` line)
        // stays visible after the bar itself loses focus.
        let jq_open = self.jq.is_open();
        let jq_rows = if jq_open {
            1 + u16::from(self.jq.error.is_some() || self.jq.note.is_some())
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(HEADER_STRIP_HEIGHT), // status chip / chips+tabs / underline
                Constraint::Length(jq_rows),             // jq bar (+ error row)
                Constraint::Min(0),                      // body
                Constraint::Length(if footer { 1 } else { 0 }), // search footer
            ])
            .split(inner);

        draw_header_strip(
            frame,
            hits,
            rows[0],
            data,
            view,
            &self.jq,
            self.collapsed,
            self.split,
            ctx,
        );
        if jq_open {
            draw_jq_bar(frame, hits, rows[1], &self.jq, ctx);
        }

        let mut body_area = rows[2];
        crate::paint::fill(frame.buffer_mut(), body_area, t.page);

        // A line wider than the pane reserves the bottom row for a
        // horizontal position indicator, mirroring how vertical overflow
        // takes the right column.
        let content_w = view.content_width();
        let mut h_bar = None;
        if content_w > body_area.width as usize && body_area.height > 1 {
            h_bar = Some(Rect {
                y: body_area.y + body_area.height - 1,
                height: 1,
                ..body_area
            });
            body_area.height -= 1;
        }

        view.height = body_area.height as usize;
        let spec = ScrollbarSpec {
            pane: PaneId::Response,
            offset: view.scroll,
            content: view.visible_len(),
            viewport: view.height,
        };
        if spec.overflows() && body_area.width > 1 {
            let column = Rect {
                x: body_area.x + body_area.width - 1,
                width: 1,
                ..body_area
            };
            body_area.width -= 1;
            if let Some(bar) = h_bar.as_mut() {
                bar.width -= 1;
            }
            crate::hit::draw_scrollbar(frame, hits, column, &spec, ctx.hovered, ctx.dragging, t);
        }
        view.width = body_area.width as usize;
        view.last_area = Some(body_area);
        // The visible set may have shrunk (a collapse, a view switch) since
        // the offset was set; never leave the viewport past the content.
        view.h_scroll = view
            .h_scroll
            .min(content_w.saturating_sub(view.width.max(1)));

        if let Some(bar) = h_bar {
            draw_h_indicator(frame, hits, bar, view.h_scroll, content_w, ctx);
        }
        // `body_lines` already starts at `view.scroll` and each line is
        // cropped by `view.h_scroll` columns, so the paragraph itself is
        // drawn unscrolled.
        if view.mode == ViewMode::Raw && view.h_scroll > 0 {
            view.index_raw_rows(view.scroll..view.scroll + view.height);
        }
        let body = body_lines(view, t, ctx.focused, ctx.hovered, hits, body_area);
        frame.render_widget(Paragraph::new(body), body_area);

        if footer {
            draw_search_footer(frame, hits, rows[3], view, ctx);
        }
    }
}

/// Total on-screen height (in rows) of [`draw_header_strip`]'s painted
/// surface: status chip / chips + right-aligned tabs / tabs underline.
/// `pub` because it is also the collapsed Response pane's height — what
/// `layout::compute_layout` shrinks the pane to while it's hidden.
pub const HEADER_STRIP_HEIGHT: u16 = 3;

/// The hidden Response pane's total height — what `layout::compute_layout`
/// shrinks the pane to while it's collapsed: just the strip's first row
/// (status chip + `› show`), the tabs and icon actions having slid away
/// with the body.
pub const COLLAPSED_HEIGHT: u16 = 1;

/// The not-yet-ready pane's one-row header strip on `area`: panel fill,
/// a status-shaped chip at the left naming the state — `—` while empty,
/// the spinner and elapsed time in flight, `failed` in the error tone,
/// `cancelled` — and the ▲/▼ step pill at the right. The same strip
/// serves the hidden pane (where it is the whole pane) and the expanded
/// one (where the body message sits below it), so the header never
/// changes shape when the pane opens.
fn draw_pending_strip(
    buf: &mut ratatui::buffer::Buffer,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    state: &ResponseState,
    split: crate::split::SplitState,
    ctx: &DrawCtx,
) {
    let t = ctx.theme;
    crate::paint::fill(buf, area, t.panel);
    let pill_x = draw_step_pill(buf, hits, area, split, ctx);
    let (label, color) = match state {
        ResponseState::Empty => ("\u{2014}".to_string(), t.text_muted),
        ResponseState::InFlight { started } => {
            let e = started.elapsed();
            let frame_i = (e.subsec_millis() / 100) as usize % SPINNER.len();
            (
                format!("{} sending\u{2026} {}", SPINNER[frame_i], human_elapsed(e)),
                t.text_muted,
            )
        }
        ResponseState::Failed(_) => ("failed".to_string(), t.error),
        ResponseState::Cancelled => ("cancelled".to_string(), t.text_muted),
        ResponseState::Ready(_) => unreachable!("the ready pane paints its own header"),
    };
    let chip = crate::paint::Chip {
        label: &label,
        color,
    };
    // On a strip too tight for both, the pill wins: it is the control.
    if area.x + chip.width() < pill_x {
        chip.paint(buf, area.x, area.y, t.panel, t);
    }
}

/// Paints the header's ▲/▼ step pill right-aligned on `area`'s first
/// row (one cell in from the right edge, like the tab strip below it)
/// and registers its two `Hit::SplitStep` chips. Returns the x of the
/// pill's leading cap — the column the row's text must stop short of.
/// Shared by the ready header strip and the not-yet-ready pane so the
/// arrows sit in the same place in every state.
fn draw_step_pill(
    buf: &mut ratatui::buffer::Buffer,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    split: crate::split::SplitState,
    ctx: &DrawCtx,
) -> u16 {
    let x = area
        .right()
        .saturating_sub(crate::paint::STEP_CONTROL_WIDTH + 1);
    if x <= area.x || area.height == 0 {
        return area.right();
    }
    let hovered = match ctx.hovered {
        Some(crate::hit::Hit::SplitStep(d)) => Some(*d),
        _ => None,
    };
    let rects = crate::paint::StepControl {
        state: split,
        hovered,
    }
    .paint(buf, x, area.y, ctx.theme);
    for (rect, delta) in rects {
        hits.register(rect, crate::hit::Hit::SplitStep(delta));
    }
    x
}

/// Paints the 3-row header strip on `theme.panel`: the status chip plus
/// the timing + size chips (plain muted text — they are not clickable) and
/// content type, all on row 0, with the ▲/▼ step pill at its right end;
/// the response tabs right-aligned on row 1;
/// row 2 holds the tabs' accent underline on the right and the icon
/// actions (search / edit / save / copy) on the left, directly above the body they act on.
#[allow(clippy::too_many_arguments)] // one display-state flag per row feature, all from `Response`
fn draw_header_strip(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    data: &crate::http::ResponseData,
    view: &ReadyView,
    bar: &JqBar,
    collapsed: bool,
    split: crate::split::SplitState,
    ctx: &DrawCtx,
) {
    let t = ctx.theme;
    let buf = frame.buffer_mut();
    crate::paint::fill(buf, area, t.panel);

    // Row 0 (right): the ▲/▼ step pill. Painted first so the URL below
    // knows where to stop.
    let pill_x = draw_step_pill(buf, hits, area, split, ctx);

    // Row 0 (left): the status chip, e.g. " 200 ", then timing + size,
    // plain muted text (not clickable, so no control fill — chip fill
    // means clickability), then content type.
    let chip_w = crate::paint::Chip {
        label: &data.status.to_string(),
        color: t.status_color(data.status),
    }
    .paint(buf, area.x, area.y, t.panel, t);

    let mut x = area.x + chip_w + 1;
    // One combined timing figure — ttfb → total — rather than two chips.
    let timing = format!(
        "{} → {}",
        human_elapsed(data.ttfb),
        human_elapsed(data.elapsed)
    );
    for label in [timing, human_size(data.size)] {
        let s = format!(" {label} ");
        let w = s.chars().count() as u16;
        crate::paint::text(buf, x, area.y, &s, t.text_muted, t.panel, false);
        x += w + 1;
    }
    if let Some(ct) = &data.content_type {
        let s = format!(" {ct}");
        crate::paint::text(buf, x, area.y, &s, t.text_muted, t.panel, false);
        x += s.chars().count() as u16 + 1;
    }

    // The rendered URL this response actually came from — `{{vars}}`
    // substituted, params merged, secrets masked — so the send's real
    // target is visible without decoding the address bar's tokens by
    // hand. Muted like the other row-0 facts, truncated to what's left
    // of the row, and dropped entirely on a row too tight to say
    // anything useful.
    if !data.url.is_empty() {
        let right = pill_x.saturating_sub(1);
        if right > x + 2 {
            let avail = (right - x - 1) as usize;
            let s = if data.url.chars().count() > avail {
                let mut cut: String = data.url.chars().take(avail.saturating_sub(1)).collect();
                cut.push('\u{2026}');
                cut
            } else {
                data.url.clone()
            };
            crate::paint::text(buf, x + 1, area.y, &s, t.text_muted, t.panel, false);
        }
    }

    // Hidden (or mid-slide with only the one row left): row 0 is the whole
    // strip — the tabs and icon actions slid away with the body, leaving
    // the status chip and the `› show` toggle.
    if collapsed || area.height < HEADER_STRIP_HEIGHT {
        return;
    }

    // Row 2 (left): the icon actions, on the stretch of the underline row
    // the tabs' rule doesn't reach.
    let jq_available = view.has_tree_view();
    let jq_on = jq_available && bar.enabled && !bar.input.text().is_empty();
    draw_header_actions(frame, hits, area, jq_available, jq_on, ctx);

    let buf = frame.buffer_mut();
    let row1_y = area.y + 1;

    // Row 1 (right) + row 2 (its underline): the response tabs,
    // right-aligned.
    let (tabs, modes) = response_tab_defs(view, bar, t);

    let tabs_width = tabstrip_width(&tabs);
    let tabs_x = area.right().saturating_sub(tabs_width).max(area.x);
    let active = modes.iter().position(|m| *m == view.mode).unwrap_or(0);
    let hovered = match ctx.hovered {
        Some(crate::hit::Hit::ResponseTab(m)) => modes.iter().position(|mode| mode == m),
        _ => None,
    };
    let tabstrip_area = Rect::new(tabs_x, row1_y, tabs_width, 2);
    let spans = crate::paint::TabStrip::spans(&tabs);
    let (static_left, static_width) = spans
        .get(active)
        .map(|(x, w)| (*x as f32, *w as f32))
        .unwrap_or((0.0, 0.0));
    // Independently animated left/right edges (Task 10): each key falls
    // back to this tab's own static edge when untracked, so the very first
    // draw of the strip snaps straight there with no slide-in from zero —
    // `app.rs`'s `Action::ResponseViewMode` handling is what actually sets
    // these keys in motion on a later switch.
    let left = ctx.anims.value_or(
        crate::anim::AnimKey::TabUnderline(crate::anim::StripId::ResponseTabs),
        ctx.now,
        static_left,
    );
    let right = ctx.anims.value_or(
        crate::anim::AnimKey::TabUnderlineWidth(crate::anim::StripId::ResponseTabs),
        ctx.now,
        static_left + static_width,
    );
    let underline = (left, right - left);
    let rects = crate::paint::TabStrip {
        tabs: &tabs,
        active,
        hovered,
        // Response tabs are switched by plain keys (r/h), not by focusing
        // the strip, so it never claims keyboard focus of its own.
        focused: false,
        underline,
        disabled: None,
    }
    .paint(buf, tabstrip_area, t.panel, t);
    for (rect, mode) in rects.into_iter().zip(modes) {
        hits.register(rect, crate::hit::Hit::ResponseTab(mode));
    }
}

/// A [`crate::paint::TabStrip::tabs`]-shaped label list: `(text, badge)`
/// per tab, where a badge is a trailing colored glyph.
type TabLabels = Vec<(String, Option<(char, ratatui::style::Color)>)>;

/// The response tab strip's labels and the [`ViewMode`] each one selects,
/// in on-screen order: `Tree` (only while `view.has_tree_view()`), `Raw`,
/// `Headers`. Shared by [`draw_header_strip`] and `app.rs`'s tab-switch
/// handling (Task 10), so the underline animation's retarget geometry can
/// never drift from what's actually painted.
///
/// The `Tree` tab wears a filter badge (nf-md-filter) while a jq filter is
/// applied: `theme.accent` normally, `theme.error` while the last run
/// failed and the tree on screen is stale.
pub fn response_tab_defs(
    view: &ReadyView,
    bar: &JqBar,
    theme: &Theme,
) -> (TabLabels, Vec<ViewMode>) {
    let mut tabs: TabLabels = Vec::new();
    let mut modes: Vec<ViewMode> = Vec::new();
    if view.has_tree_view() {
        let badge = view.jq_tree.is_some().then_some((
            '\u{F0232}',
            if bar.stale { theme.error } else { theme.accent },
        ));
        tabs.push(("Tree".to_string(), badge));
        modes.push(ViewMode::Pretty);
    }
    tabs.push(("Raw".to_string(), None));
    modes.push(ViewMode::Raw);
    tabs.push(("Headers".to_string(), None));
    modes.push(ViewMode::Headers);
    (tabs, modes)
}

/// The horizontal span [`crate::paint::TabStrip::paint`] occupies for
/// `tabs`, mirroring its own padded-block-width + 1-column-gap layout so
/// callers can right-align the strip without painting it first.
fn tabstrip_width(tabs: &[(String, Option<(char, ratatui::style::Color)>)]) -> u16 {
    crate::paint::TabStrip::spans(tabs)
        .last()
        .map(|(x, w)| x + w)
        .unwrap_or(0)
}

/// The icon actions in the header strip: search, open the view in
/// `$EDITOR`, save to file, copy. Nerd Font Material icons (the family
/// the address bar's lock uses), not emoji: they are one cell in every
/// width table, so ratatui paints each cell and its cursor accounting
/// matches the terminal's (a wide emoji leaves its second cell unpainted,
/// which showed stale content in terminals that restyle only the first),
/// and as text glyphs they take the theme's foreground instead of the
/// emoji font's fixed colours — so all four sit level, at one size.
/// All of them act on the *active tab's* text, following it like search.
const HEADER_ACTIONS: [(&str, crate::hit::Hit); 5] = [
    (" \u{F0349} ", crate::hit::Hit::ResponseSearchButton), // 󰍉 nf-md-magnify
    (" \u{F03EB} ", crate::hit::Hit::ResponseEditorButton), // 󰏫 nf-md-pencil
    (" \u{F0193} ", crate::hit::Hit::SaveBodyButton),       // 󰆓 nf-md-content_save
    (" \u{F018F} ", crate::hit::Hit::CopyBodyButton),       // 󰆏 nf-md-content_copy
    (" \u{F0232} ", crate::hit::Hit::ResponseJqButton),     // 󰈲 nf-md-filter
];

/// The header strip's icon actions, left-aligned on the underline row
/// (`area`'s third row) — the stretch the tabs' rule doesn't reach — so
/// they sit directly above the response body they act on. The jq button
/// (the last entry) is skipped entirely while `jq_available` is false —
/// there is nothing for it to filter — and painted in its pressed
/// (inverted) state while `jq_on`, so a filtered tree is explained even
/// when the bar has scrolled out of mind.
fn draw_header_actions(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    jq_available: bool,
    jq_on: bool,
    ctx: &DrawCtx,
) {
    use unicode_width::UnicodeWidthStr;
    let y = area.y + 2;
    let mut x = area.x + 1;
    let buf = frame.buffer_mut();
    let mut rects = Vec::new();
    for (label, hit) in HEADER_ACTIONS {
        if !jq_available && hit == crate::hit::Hit::ResponseJqButton {
            continue;
        }
        // Display width, not char count, so the labels' padding is honoured.
        let w = label.width() as u16;
        let rect = Rect::new(x, y, w, 1);
        let pressed = jq_on && hit == crate::hit::Hit::ResponseJqButton;
        draw_pane_action(
            buf,
            rect,
            label,
            hit.clone(),
            if pressed { Some(&hit) } else { ctx.hovered },
            ctx.theme.panel,
            ctx.theme,
        );
        rects.push((rect, hit));
        x += w + 1;
    }
    for (rect, hit) in rects {
        hits.register(rect, hit);
    }
}

/// A plain (unbracketed) clickable text action painted on `surface`: accent
/// fg at rest; inverted (accent fill, `on_accent` fg, bold) while
/// `hovered == Some(&hit)`.
fn draw_pane_action(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    label: &str,
    hit: crate::hit::Hit,
    hovered: Option<&crate::hit::Hit>,
    surface: Color,
    theme: &Theme,
) {
    if hovered == Some(&hit) {
        crate::paint::fill(buf, area, theme.accent);
        crate::paint::text(
            buf,
            area.x,
            area.y,
            label,
            theme.on_accent,
            theme.accent,
            true,
        );
    } else {
        crate::paint::text(buf, area.x, area.y, label, theme.accent, surface, false);
    }
}

fn human_elapsed(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}

/// Builds the body viewport's lines for whichever view is active, applying
/// search highlighting and the cursor row's background.
///
/// Only the `view.scroll .. view.scroll + view.height` window is built — a
/// multi-megabyte body is a lot of lines, and none of the ones off screen
/// are worth styling. The caller therefore renders the result at scroll
/// offset 0.
///
/// In `Pretty` mode, also registers a `JsonRow` hit over each rendered row
/// (click selects) and a `JsonArrow` hit over its first two columns when the
/// row opens a container (click toggles). In `Headers` mode, registers a
/// `HeaderCopy` hit over the trailing ` ❐ ` pill appended to each row.
/// `Raw` registers nothing per-row.
fn body_lines(
    view: &ReadyView,
    t: &Theme,
    focused: bool,
    hovered: Option<&crate::hit::Hit>,
    hits: &mut crate::hit::HitMap,
    area: Rect,
) -> Vec<Line<'static>> {
    let text = Style::default().fg(t.text);
    let cursor_bg = Style::default().bg(t.panel);
    let mut out = Vec::new();
    let start = view.scroll;
    let end = start.saturating_add(view.height.max(1));

    // `window` is where `pieces` sit in the full line: the char offset
    // they start at, and the display columns `crop_cols` still has to drop
    // from their front — `(0, h_scroll)` for a row built from its whole
    // text, or whatever `viewport_window` cut for a raw row.
    let mut push = |i: usize,
                    full: usize,
                    pieces: Vec<(String, Style)>,
                    highlightable: bool,
                    window: (usize, usize)| {
        let (char_start, crop) = window;
        let hits = if highlightable {
            view.match_ranges(full).shifted(char_start)
        } else {
            LineMatches {
                ranges: Vec::new(),
                current: None,
            }
        };
        let mut line = highlighted(pieces, &hits);
        // Selection bg on top of search styling, before the h-crop (its
        // char columns are pre-crop coordinates, shifted into the window).
        if let Some((from, to)) = view.sel_range_on_line(i) {
            let (from, to) = (
                from.saturating_sub(char_start),
                to.saturating_sub(char_start),
            );
            if to > from {
                line = apply_col_bg(line, from, to, t.selection);
            }
        }
        let mut line = crop_cols(line, crop);
        if focused && i == view.cursor {
            line = line.style(cursor_bg);
        }
        out.push(line);
    };

    match view.mode {
        ViewMode::Pretty => {
            // The body tree stays behind the spinner while a switched-on
            // filter's output is still on its way.
            let held = view.awaiting_filter && view.jq_tree.is_none();
            let Some(tree) = view.active_tree().filter(|_| !held) else {
                if view.parsing || held {
                    let e = view.parse_started.elapsed();
                    let frame_i = (e.subsec_millis() / 100) as usize % SPINNER.len();
                    let verb = if view.parsing { "parsing" } else { "filtering" };
                    out.push(Line::styled(
                        format!(" {} {verb}…", SPINNER[frame_i]),
                        Style::default().fg(t.text_muted),
                    ));
                }
                return out;
            };
            let indices = tree.visible_indices();
            for (i, &full) in indices.iter().enumerate().take(end).skip(start) {
                let full = full as usize;
                let line = tree.line(full);
                let mut pieces = vec![(" ".repeat(line.indent), text)];
                for tok in line.render_tokens() {
                    pieces.push((
                        tok.text.into_owned(),
                        Style::default().fg(token_color(tok.kind, t)),
                    ));
                }
                // A collapsed line renders its summary, not its real text, so
                // the match columns computed over the expanded text no longer
                // apply to it.
                push(i, full, pieces, !line.collapsed, (0, view.h_scroll));

                let y = area.y.saturating_add((i - start) as u16);
                if y < area.y.saturating_add(area.height) {
                    hits.register(
                        Rect::new(area.x, y, area.width, 1),
                        crate::hit::Hit::JsonRow(i),
                    );
                    // The arrow occupies the row's first two unscrolled
                    // columns; once the view is scrolled right it is off
                    // screen, and a hit would land on unrelated content.
                    if view.h_scroll == 0 && tree.is_container_at_visible(i) {
                        let arrow_w = area.width.min(2);
                        hits.register(
                            Rect::new(area.x, y, arrow_w, 1),
                            crate::hit::Hit::JsonArrow(i),
                        );
                    }
                }
            }
        }
        ViewMode::Raw => {
            // A verbatim line can be megabytes long (a minified body is one
            // line), and this runs on every frame of a scrollbar drag: only
            // the chars that can reach the viewport are copied and styled.
            let width = area.width as usize;
            for i in start..end.min(view.raw_lines.len()) {
                let from = mark_for(view.raw_marks.get(&i).map(Vec::as_slice), view.h_scroll);
                let (char_start, crop, slice) =
                    viewport_window(&view.raw_lines[i], view.h_scroll, width, from);
                push(
                    i,
                    i,
                    vec![(slice.to_string(), text)],
                    true,
                    (char_start, crop),
                );
            }
        }
        ViewMode::Headers => {
            if view.header_lines.is_empty() {
                out.push(Line::styled(
                    "(no headers)",
                    Style::default().fg(t.text_muted),
                ));
                return out;
            }
            for (i, line) in view.header_lines.iter().enumerate().take(end).skip(start) {
                let (name, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
                let name_piece = format!("{name}:");
                let value_piece = value.to_string();
                let text_len = name_piece.chars().count() + value_piece.chars().count();
                let glyph_hovered = hovered == Some(&crate::hit::Hit::HeaderCopy(i));
                let glyph_style = if glyph_hovered {
                    Style::default().bg(t.accent).fg(t.on_accent)
                } else {
                    Style::default().fg(t.accent)
                };
                // The glyph sits centered in an odd-width pill so its hover
                // highlight surrounds it symmetrically instead of leaving a
                // blank highlighted cell on one side.
                let pieces = vec![
                    (name_piece, Style::default().fg(t.accent)),
                    (value_piece, text),
                    (" ❐ ".to_string(), glyph_style),
                ];
                push(i, i, pieces, true, (0, view.h_scroll));

                let y = area.y.saturating_add((i - start) as u16);
                if y < area.y.saturating_add(area.height) {
                    // The glyph's on-screen column shifts left with the
                    // horizontal scroll; once the pill itself is cropped
                    // away there is nothing left to click.
                    let glyph_col = text_len.saturating_sub(view.h_scroll) as u16;
                    let glyph_x = area.x.saturating_add(glyph_col);
                    let glyph_w = area.width.saturating_sub(glyph_col).min(3);
                    if glyph_w > 0 && text_len + 3 > view.h_scroll {
                        hits.register(
                            Rect::new(glyph_x, y, glyph_w, 1),
                            crate::hit::Hit::HeaderCopy(i),
                        );
                    }
                }
            }
        }
    }
    out
}

/// Paints the horizontal position indicator on the reserved bottom row: a
/// muted `─` track with an accent `█` thumb, the sideways twin of
/// [`crate::hit::draw_scrollbar`]. Read-only — the wheel and ←/→ move the
/// viewport, the bar just shows where it is.
fn draw_h_indicator(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    bar: Rect,
    offset: usize,
    content: usize,
    ctx: &DrawCtx,
) {
    let t = ctx.theme;
    if bar.width == 0 {
        return;
    }
    let spec = ScrollbarSpec {
        pane: PaneId::Response,
        offset,
        content,
        viewport: bar.width as usize,
    };
    let (left, width) = crate::hit::thumb_geometry(&spec, bar.width);
    let mut spans = Vec::new();
    let track = Style::default().fg(t.text_muted);
    let thumb_hit = crate::hit::Hit::HScrollThumb(PaneId::Response);
    // Same rationale as the vertical thumb: a full block hides its own
    // background, so the active state brightens over accent instead of the
    // usual inversion.
    let thumb = if ctx.dragging || ctx.hovered == Some(&thumb_hit) {
        Style::default().bg(t.accent).fg(t.text)
    } else {
        Style::default().fg(t.accent)
    };
    spans.push(Span::styled("─".repeat(left as usize), track));
    spans.push(Span::styled("█".repeat(width as usize), thumb));
    spans.push(Span::styled(
        "─".repeat(bar.width.saturating_sub(left + width) as usize),
        track,
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), bar);

    // Page segments first so the thumb wins where they would overlap,
    // mirroring `hit::draw_scrollbar`'s vertical layout.
    hits.register_h_track(PaneId::Response, bar);
    let page = spec.viewport.min(i16::MAX as usize) as i16;
    if left > 0 {
        hits.register(
            Rect { width: left, ..bar },
            crate::hit::Hit::HScrollTrack(PaneId::Response, -page),
        );
    }
    let after_x = bar.x + left + width;
    let after_w = bar.width.saturating_sub(left + width);
    if after_w > 0 {
        hits.register(
            Rect {
                x: after_x,
                width: after_w,
                ..bar
            },
            crate::hit::Hit::HScrollTrack(PaneId::Response, page),
        );
    }
    hits.register(
        Rect {
            x: bar.x + left,
            width,
            ..bar
        },
        thumb_hit,
    );
}

/// Repaints the chars in the half-open column range `[from, to)` of `line`
/// onto background `bg`, splitting spans at the boundaries and keeping
/// every other style bit — how a selection lies over already-styled
/// content (tokens, search matches).
fn apply_col_bg(line: Line<'static>, from: usize, to: usize, bg: Color) -> Line<'static> {
    let style = line.style;
    let mut spans = Vec::new();
    let mut at = 0usize;
    for span in line.spans {
        let chars: Vec<char> = span.content.chars().collect();
        let (s0, s1) = (at, at + chars.len());
        at = s1;
        if to <= s0 || from >= s1 {
            spans.push(span);
            continue;
        }
        let a = from.saturating_sub(s0).min(chars.len());
        let b = to.saturating_sub(s0).min(chars.len());
        if a > 0 {
            spans.push(Span::styled(
                chars[..a].iter().collect::<String>(),
                span.style,
            ));
        }
        spans.push(Span::styled(
            chars[a..b].iter().collect::<String>(),
            span.style.bg(bg),
        ));
        if b < chars.len() {
            spans.push(Span::styled(
                chars[b..].iter().collect::<String>(),
                span.style,
            ));
        }
    }
    Line::from(spans).style(style)
}

/// Columns between two entries of a raw line's column index (`col_marks`).
/// A window lookup walks at most this many chars from the nearest mark, so
/// a scrolled frame on a megabyte line stays flat instead of re-walking
/// its whole prefix.
const COL_MARK_STEP: usize = 1024;

/// A resume point for `viewport_window` — a char's byte offset, char index
/// and the display column it starts at. `(0, 0, 0)` is the line start.
type ColMark = (usize, usize, usize);

/// The column index of `text`: entry `k` is the char covering display
/// column `k * COL_MARK_STEP` (a wide char covering two marks appears
/// twice). Built once per long raw line, on the first frame that scrolls
/// it sideways; `O(chars)` then, `O(1)` to look up after.
fn col_marks(text: &str) -> Vec<ColMark> {
    use unicode_width::UnicodeWidthChar;
    let mut marks = Vec::new();
    let mut cols = 0usize;
    for (idx, (byte, c)) in text.char_indices().enumerate() {
        let w = c.width().unwrap_or(0);
        while marks.len() * COL_MARK_STEP < cols + w {
            marks.push((byte, idx, cols));
        }
        cols += w;
    }
    marks
}

/// The mark to resume a `skip`-column window from: the one at or before
/// column `skip`, or the last one when `skip` runs past the index. `None`
/// for a line with no index (short, or never scrolled).
fn mark_for(marks: Option<&[ColMark]>, skip: usize) -> ColMark {
    marks
        .and_then(|m| m.get(skip / COL_MARK_STEP).or(m.last()))
        .copied()
        .unwrap_or((0, 0, 0))
}

/// The part of `text` that can reach a viewport `width` columns wide once
/// its first `skip` display columns are cropped: the char offset the window
/// starts at, the columns `crop_cols` still has to drop (a wide char
/// straddling the crop is kept whole, so its already-covered column is
/// owed), and the window itself — a slice starting on that char and
/// running at least `width` columns, so cropping and truncating it paints
/// exactly what cropping the whole line would. Skipping past the end gives
/// an empty window at the line's end. The walk starts from `from`, a
/// `col_marks` entry at or before `skip` (or the line start), so it costs
/// `O(COL_MARK_STEP + width)` chars with an index and `O(skip + width)`
/// without — never the whole line.
fn viewport_window(text: &str, skip: usize, width: usize, from: ColMark) -> (usize, usize, &str) {
    use unicode_width::UnicodeWidthChar;
    let (from_byte, from_char, from_col) = from;
    debug_assert!(from_col <= skip, "a resume mark must not lie past the crop");
    let mut chars = text[from_byte..]
        .char_indices()
        .enumerate()
        .map(|(i, (b, c))| (i + from_char, (b + from_byte, c)));
    let mut cols = from_col;
    let mut total_chars = from_char;
    let mut start = None;
    for (idx, (byte, c)) in chars.by_ref() {
        total_chars = idx + 1;
        let w = c.width().unwrap_or(0);
        if cols + w > skip {
            start = Some((idx, byte, skip - cols, w));
            break;
        }
        cols += w;
    }
    let Some((char_start, byte_start, residual, first_w)) = start else {
        return (total_chars, 0, "");
    };
    let need = residual + width;
    let mut taken = first_w;
    let mut byte_end = text.len();
    for (_, (byte, c)) in chars {
        if taken >= need {
            byte_end = byte;
            break;
        }
        taken += c.width().unwrap_or(0);
    }
    (char_start, residual, &text[byte_start..byte_end])
}

/// Drops the first `skip` display columns of `line`, keeping every span
/// style. A double-width character straddling the cut is replaced by a
/// space per swallowed column, so the remaining cells stay aligned.
fn crop_cols(line: Line<'static>, skip: usize) -> Line<'static> {
    use unicode_width::UnicodeWidthChar;
    if skip == 0 {
        return line;
    }
    let style = line.style;
    let mut spans = Vec::new();
    let mut remaining = skip;
    for span in line.spans {
        if remaining == 0 {
            spans.push(span);
            continue;
        }
        let mut kept = String::new();
        for c in span.content.chars() {
            if remaining == 0 {
                kept.push(c);
                continue;
            }
            let w = c.width().unwrap_or(0);
            if w <= remaining {
                remaining -= w;
            } else {
                kept.extend(std::iter::repeat_n(' ', w - remaining));
                remaining = 0;
            }
        }
        if !kept.is_empty() {
            spans.push(Span::styled(kept, span.style));
        }
    }
    Line::from(spans).style(style)
}

fn token_color(kind: TokenKind, t: &Theme) -> Color {
    match kind {
        TokenKind::Key => t.accent,
        TokenKind::Str => t.success,
        TokenKind::Number => t.warning,
        TokenKind::Literal => t.text_muted,
        TokenKind::Punct => t.text,
    }
}

/// Re-slices `pieces` at match boundaries so highlighting can be added
/// without losing the syntax colors underneath it. Offsets are char indices
/// into the concatenated text.
fn highlighted(pieces: Vec<(String, Style)>, hits: &LineMatches) -> Line<'static> {
    if hits.ranges.is_empty() {
        return Line::from(
            pieces
                .into_iter()
                .map(|(s, st)| Span::styled(s, st))
                .collect::<Vec<_>>(),
        );
    }
    let hit = |at: usize| -> (bool, bool) {
        (
            hits.ranges.iter().any(|(s, e)| at >= *s && at < *e),
            hits.current.is_some_and(|(s, e)| at >= s && at < e),
        )
    };
    let mut spans = Vec::new();
    let mut offset = 0;
    for (piece_text, base) in pieces {
        let chars: Vec<char> = piece_text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let class = hit(offset + i);
            let mut j = i + 1;
            while j < chars.len() && hit(offset + j) == class {
                j += 1;
            }
            let mut style = base;
            if class.0 {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if class.1 {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(chars[i..j].iter().collect::<String>(), style));
            i = j;
        }
        offset += chars.len();
    }
    Line::from(spans)
}

/// The jq filter bar: a `jq ` chip, the live filter text (or the "asking…"
/// spinner while an AI request is pending), and the `✦` AI button
/// right-aligned. A second row — when the bar reserved one — shows the
/// last error's message, with its span (when known) underlined in the bar
/// text above it.
fn draw_jq_bar(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    bar: &JqBar,
    ctx: &DrawCtx,
) {
    let t = ctx.theme;
    crate::paint::fill(frame.buffer_mut(), area, t.page);
    if area.height == 0 {
        return;
    }
    const AI: &str = " ✦ ";
    let ai_w = AI.chars().count() as u16;
    let text_w = area.width.saturating_sub(ai_w + 1);
    let row = Rect {
        height: 1,
        width: text_w,
        ..area
    };
    let chip_color = if bar.stale { t.text_muted } else { t.accent };
    // A background run past its grace period takes over the `jq` chip —
    // the spinner sits right beside the text being typed, where the wait
    // is felt, and a run that finishes sooner never flickers it. Both
    // chips are three columns (` ⠋ ` / `jq `), so the text never shifts;
    // the leading space keeps the glyph off the pane edge.
    let spinning = bar.running_long(ctx.now);
    let chip = if spinning {
        let frame_i = (ctx
            .now
            .saturating_duration_since(bar.pending_since)
            .as_millis()
            / 80) as usize
            % SPINNER.len();
        format!(" {} ", SPINNER[frame_i])
    } else {
        "jq ".to_string()
    };
    let mut spans = vec![Span::styled(chip, Style::default().fg(chip_color))];
    if bar.ai_pending {
        let frame_i = (ctx
            .now
            .saturating_duration_since(bar.ai_started)
            .as_millis()
            / 80) as usize
            % SPINNER.len();
        spans.push(Span::styled(
            format!("{} asking…", SPINNER[frame_i]),
            Style::default().fg(t.text_muted),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    } else {
        let line = bar
            .input
            .draw_line_windowed(bar.focused, t, text_w.saturating_sub(3));
        spans.extend(line.spans);
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    hits.register(row, crate::hit::Hit::ResponseJqBar);
    if area.width > ai_w {
        let ai_area = Rect::new(area.right() - ai_w, area.y, ai_w, 1);
        draw_pane_action(
            frame.buffer_mut(),
            ai_area,
            AI,
            crate::hit::Hit::ResponseJqAiButton,
            ctx.hovered,
            t.page,
            t,
        );
        hits.register(ai_area, crate::hit::Hit::ResponseJqAiButton);
    }
    // A filter with nothing to show reads as an error to the user — the
    // tree they're looking at isn't its output — so it's drawn like one.
    if bar.note.is_some() && bar.error.is_none() && area.height >= 2 {
        let note_row = Rect {
            y: area.y + 1,
            height: 1,
            ..area
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "   invalid filter",
                Style::default().fg(t.error),
            ))),
            note_row,
        );
    }
    if let Some(err) = &bar.error
        && area.height >= 2
    {
        let err_row = Rect {
            y: area.y + 1,
            height: 1,
            ..area
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("   {}", err.message()),
                Style::default().fg(t.error),
            ))),
            err_row,
        );
        // Underline the span in the bar text when known. Ignores
        // horizontal windowing of a very long filter — acceptable.
        //
        // The span is a byte range into `code.trim()` (what was actually
        // compiled), not the untrimmed bar text — offset it by the leading
        // whitespace `trim()` stripped, and bail out defensively rather
        // than index: a stale span from a filter compiled before the bar
        // text last shrank can point past the end, or (once shifted) land
        // off a char boundary.
        if let Some(span) = err.span() {
            let text = bar.input.text();
            let leading_ws = text.len() - text.trim_start().len();
            let start_byte = span.start.saturating_add(leading_ws);
            let end_byte = span.end.saturating_add(leading_ws);
            if end_byte <= text.len()
                && start_byte <= end_byte
                && text.is_char_boundary(start_byte)
                && text.is_char_boundary(end_byte)
            {
                let start = text[..start_byte].chars().count() as u16;
                let len = text[start_byte..end_byte].chars().count().max(1) as u16;
                let x0 = row.x + 3 + start; // after "jq "
                for x in x0..(x0 + len).min(row.right()) {
                    frame.buffer_mut()[(x, row.y)].set_style(
                        Style::default()
                            .add_modifier(Modifier::UNDERLINED)
                            .fg(t.error),
                    );
                }
            }
        }
    }
}

/// The search row: the query/match counter (or the live input) on the left,
/// and the `▲`/`▼` step buttons right-aligned — the mouse's `N`/`n`. The
/// buttons are painted over the row's own fill, and the text is given the
/// remaining width so the two never overlap.
fn draw_search_footer(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    view: &ReadyView,
    ctx: &DrawCtx,
) {
    let t = ctx.theme;
    crate::paint::fill(frame.buffer_mut(), area, t.page);

    const PREV: &str = " ▲ ";
    const NEXT: &str = " ▼ ";
    let step_w = PREV.chars().count() as u16;
    let buttons_w = step_w * 2 + 1;
    let text_w = area.width.saturating_sub(buttons_w + 1);
    frame.render_widget(
        Paragraph::new(search_footer(view, t, text_w)),
        Rect {
            width: text_w,
            ..area
        },
    );

    if area.width <= buttons_w {
        return;
    }
    let prev_area = Rect::new(area.right() - buttons_w, area.y, step_w, 1);
    let next_area = Rect::new(area.right() - step_w, area.y, step_w, 1);
    let buf = frame.buffer_mut();
    for (rect, label, hit) in [
        (prev_area, PREV, crate::hit::Hit::ResponseSearchPrev),
        (next_area, NEXT, crate::hit::Hit::ResponseSearchNext),
    ] {
        draw_pane_action(buf, rect, label, hit.clone(), ctx.hovered, t.page, t);
        hits.register(rect, hit);
    }
}

/// `/query   3/17` — or the live input while the search is being typed.
fn search_footer(view: &ReadyView, t: &Theme, width: u16) -> Line<'static> {
    let Some(search) = &view.search else {
        return Line::raw("");
    };
    let accent = Style::default().fg(t.accent);
    let mut spans = vec![Span::styled("/", accent)];
    if search.active {
        // The leading "/" already claims one column of `width`.
        let input_width = width.saturating_sub(1);
        spans.extend(search.input.draw_line_windowed(true, t, input_width).spans);
        return Line::from(spans);
    }
    spans.push(Span::styled(
        search.query.clone(),
        Style::default().fg(t.text),
    ));
    let muted = Style::default().fg(t.text_muted);
    if search.matches.is_empty() {
        spans.push(Span::styled("  no matches", muted));
    } else {
        spans.push(Span::styled(
            format!("  {}/{}", search.current + 1, search.matches.len()),
            muted,
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use std::time::Duration;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    /// Presses a key and applies a resulting `ResponseViewMode` action the
    /// way `app.rs` would — the component itself no longer mutates the mode.
    fn press(r: &mut Response, ev: KeyEvent) {
        if let Some(Action::ResponseViewMode(mode)) = r.handle_key(ev) {
            r.set_view_mode(mode);
        }
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    /// A disabled (instantly-jumping) `Anims` shared by every test's
    /// `DrawCtx`, so tests stay deterministic without threading an owned
    /// `Anims` through each call site.
    fn test_anims() -> &'static crate::anim::Anims {
        static ANIMS: std::sync::OnceLock<crate::anim::Anims> = std::sync::OnceLock::new();
        ANIMS.get_or_init(|| crate::anim::Anims::new(false))
    }

    fn data(body: &str) -> crate::http::ResponseData {
        crate::http::ResponseData {
            status: 200,
            url: "https://api.example.com/things?page=2".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
            ttfb: Duration::from_millis(38),
            elapsed: Duration::from_millis(342),
            size: body.len(),
            content_type: Some("application/json".into()),
        }
    }

    fn ready(body: &str) -> Response {
        ready_gen(body, 0)
    }

    fn ready_gen(body: &str, generation: u64) -> Response {
        let mut r = Response::default();
        r.set_state(ResponseState::Ready(Box::new(data(body))), generation);
        r
    }

    /// A JSON body over [`SYNC_PRETTY_BYTES`], so its parse is deferred.
    fn big_json() -> String {
        format!("{{\"a\": \"{}\"}}", "x".repeat(3 * 1024 * 1024))
    }

    fn render(resp: &mut Response) -> String {
        render_sized(resp, 60, 20)
    }

    fn render_sized(resp: &mut Response, w: u16, h: u16) -> String {
        render_sized_at(resp, w, h, std::time::Instant::now())
    }

    /// `render_sized` with the frame drawn as of `now`, for time-gated
    /// affordances like the jq bar's spinner.
    fn render_sized_at(resp: &mut Response, w: u16, h: u16, now: std::time::Instant) -> String {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now,
        };
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| resp.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    /// Renders and returns the (body-area rect, buffer) so selection tests
    /// can map screen cells.
    fn render_buf(resp: &mut Response) -> (Rect, ratatui::buffer::Buffer) {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| resp.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let area = resp.view().unwrap().last_area.expect("area recorded");
        (area, terminal.backend().buffer().clone())
    }

    #[test]
    fn raw_drag_selects_across_lines_and_copies_with_newlines() {
        let mut r = ready("hello world\nsecond line\nthird");
        let (area, _) = render_buf(&mut r);
        assert!(r.begin_selection_at(area.x, area.y));
        assert!(r.drag_selection_to(area.x + 2, area.y + 1));
        assert_eq!(r.selected_text().as_deref(), Some("hello world\nsec"));
    }

    /// Sweeping up with the pointer past a line's end must not grab that
    /// line's last char: the pointer sits past the boundary after it, so
    /// the selection starts at the start of the row below.
    #[test]
    fn upward_drag_past_a_line_end_starts_at_the_row_below() {
        let mut r = ready("ab\nworld\nthird");
        let (area, _) = render_buf(&mut r);
        assert!(r.begin_selection_at(area.x + 4, area.y + 1)); // 'd'
        assert!(r.drag_selection_to(area.x + 8, area.y)); // past "ab"
        assert_eq!(r.selected_text().as_deref(), Some("world"));
    }

    /// Sweeping down onto a row's first cell must not grab that row's
    /// first char: the pointer sits on the boundary before it, so the
    /// selection ends at the end of the line above.
    #[test]
    fn downward_drag_onto_a_row_start_ends_at_the_line_above() {
        let mut r = ready("hello\nworld\nthird");
        let (area, _) = render_buf(&mut r);
        assert!(r.begin_selection_at(area.x, area.y));
        assert!(r.drag_selection_to(area.x, area.y + 2));
        assert_eq!(r.selected_text().as_deref(), Some("hello\nworld"));
    }

    #[test]
    fn drag_past_the_line_end_clamps_to_its_last_char() {
        let mut r = ready("ab\nlonger line");
        let (area, _) = render_buf(&mut r);
        r.begin_selection_at(area.x, area.y);
        r.drag_selection_to(area.x + 50, area.y);
        assert_eq!(r.selected_text().as_deref(), Some("ab"));
    }

    #[test]
    fn double_click_selects_the_word_under_it() {
        let mut r = ready("hello world");
        let (area, _) = render_buf(&mut r);
        assert!(r.select_word_at(area.x + 7, area.y));
        assert_eq!(r.selected_text().as_deref(), Some("world"));
    }

    #[test]
    fn word_drag_extends_the_selection_by_whole_words() {
        let mut r = ready("alpha beta gamma");
        let (area, _) = render_buf(&mut r);
        assert!(r.select_word_at(area.x + 1, area.y));
        assert_eq!(r.selected_text().as_deref(), Some("alpha"));
        // Dragging onto "beta" grows the selection a whole word at a time...
        assert!(r.drag_selection_to(area.x + 7, area.y));
        assert_eq!(r.selected_text().as_deref(), Some("alpha beta"));
        // ...and dragging back onto the anchor word shrinks it again.
        assert!(r.drag_selection_to(area.x + 1, area.y));
        assert_eq!(r.selected_text().as_deref(), Some("alpha"));
    }

    #[test]
    fn word_drag_backward_keeps_the_anchor_word_selected() {
        let mut r = ready("alpha beta gamma");
        let (area, _) = render_buf(&mut r);
        assert!(r.select_word_at(area.x + 12, area.y)); // "gamma"
        assert!(r.drag_selection_to(area.x + 7, area.y)); // back over "beta"
        assert_eq!(r.selected_text().as_deref(), Some("beta gamma"));
    }

    #[test]
    fn double_click_past_the_line_end_selects_nothing() {
        let mut r = ready("ab\nlonger line");
        let (area, _) = render_buf(&mut r);
        assert!(!r.select_word_at(area.x + 30, area.y));
        assert_eq!(r.selected_text(), None);
    }

    #[test]
    fn a_new_click_or_view_switch_clears_the_selection() {
        let mut r = ready("hello\nworld");
        let (area, _) = render_buf(&mut r);
        r.begin_selection_at(area.x, area.y);
        r.drag_selection_to(area.x + 3, area.y);
        assert!(r.selected_text().is_some());
        // A fresh click collapses...
        r.begin_selection_at(area.x + 1, area.y + 1);
        assert_eq!(r.selected_text(), None);
        // ...and so does a tab switch.
        r.begin_selection_at(area.x, area.y);
        r.drag_selection_to(area.x + 3, area.y);
        assert!(r.selected_text().is_some());
        r.set_view_mode(ViewMode::Headers);
        assert_eq!(r.selected_text(), None);
    }

    #[test]
    fn selection_paints_on_the_selection_background_even_h_scrolled() {
        let theme = Theme::dark();
        let mut r = ready("abcdefghij\nklmnopqrst");
        let (area, _) = render_buf(&mut r);
        r.begin_selection_at(area.x, area.y);
        r.drag_selection_to(area.x + 4, area.y);
        let (area, buf) = render_buf(&mut r);
        assert_eq!(
            buf.cell((area.x + 1, area.y)).unwrap().bg,
            theme.selection,
            "selected cell paints on the selection bg"
        );
        assert_ne!(
            buf.cell((area.x + 7, area.y)).unwrap().bg,
            theme.selection,
            "unselected cell stays plain"
        );
        // Scroll right: painting must crop, not panic, and the visible
        // remainder of the selection stays highlighted.
        r.view.as_mut().unwrap().h_scroll = 2;
        let (area, buf) = render_buf(&mut r);
        assert_eq!(
            buf.cell((area.x, area.y)).unwrap().bg,
            theme.selection,
            "col 2 of the selection is now at the left edge"
        );
    }

    #[test]
    fn shift_down_extends_a_line_wise_selection() {
        let mut r = ready("hello\nworld\nthird");
        render_buf(&mut r);
        r.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        assert_eq!(r.selected_text().as_deref(), Some("hello\nworld"));
        r.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        assert_eq!(r.selected_text().as_deref(), Some("hello\nworld\nthird"));
        // Esc clears the selection before anything else.
        r.handle_key(key(KeyCode::Esc));
        assert_eq!(r.selected_text(), None);
    }

    #[test]
    fn pretty_mode_selection_copies_the_on_screen_text() {
        let mut r = ready(r#"{"a": 1}"#);
        let (area, _) = render_buf(&mut r);
        // Row 1 renders `  "a": 1` (indent 2). Select its first 5 cells.
        r.begin_selection_at(area.x, area.y + 1);
        r.drag_selection_to(area.x + 4, area.y + 1);
        assert_eq!(r.selected_text().as_deref(), Some("  \"a\""));
    }

    #[test]
    fn ready_summary_shows_status_elapsed_size_and_a_json_key() {
        let mut r = ready(r#"{"hello": "world"}"#);
        let out = render(&mut r);
        assert!(out.contains("200"), "status pill: {out}");
        assert!(out.contains("342 ms"), "elapsed: {out}");
        assert!(out.contains(" B"), "human size: {out}");
        assert!(out.contains("application/json"), "content type: {out}");
        assert!(out.contains("\"hello\""), "pretty body key: {out}");
    }

    /// The header strip carries a ✎ icon that opens the active tab's text
    /// in `$EDITOR`, registered like the other icon actions.
    #[test]
    fn header_actions_include_an_open_in_editor_icon() {
        let mut r = ready(r#"{"a": 1}"#);
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let out = format!("{:?}", terminal.backend().buffer());
        assert!(out.contains("\u{F03EB}"), "editor icon: {out}");
        assert!(
            hits.rect_of(&crate::hit::Hit::ResponseEditorButton)
                .is_some(),
            "the ✎ icon registers its hit"
        );
    }

    /// `view_text` follows the active tab exactly as search does: the
    /// pretty rendering on Pretty, the verbatim body on Raw, the header
    /// list on Headers.
    #[test]
    fn view_text_follows_the_active_tab() {
        let mut r = ready(r#"{"a":1}"#);

        let pretty = r.view().unwrap().view_text();
        assert!(
            pretty.contains('\n') && pretty.contains("\"a\""),
            "Pretty tab yields the formatted rendering: {pretty:?}"
        );

        r.set_view_mode(ViewMode::Raw);
        assert_eq!(
            r.view().unwrap().view_text(),
            r#"{"a":1}"#,
            "Raw tab yields the verbatim body"
        );

        r.set_view_mode(ViewMode::Headers);
        let headers = r.view().unwrap().view_text();
        assert!(
            headers.contains("content-type:") && headers.contains("application/json"),
            "Headers tab yields the header list: {headers:?}"
        );
        assert!(!headers.contains("\"a\""), "no body on the Headers tab");
    }

    /// The timing chip pairs time-to-first-byte with the total as one
    /// `ttfb → total` figure rather than two separate chips.
    #[test]
    fn timing_chip_combines_ttfb_and_total() {
        let mut r = ready(r#"{"a": 1}"#);
        let out = render(&mut r);
        assert!(out.contains("38 ms → 342 ms"), "combined timing: {out}");
    }

    /// Row 0 carries the rendered URL the response actually came from
    /// (`{{vars}}` substituted, params merged, secrets masked upstream),
    /// after the timing/size/content-type facts — truncated with an
    /// ellipsis when the row can't hold it whole.
    #[test]
    fn header_strip_shows_the_sent_url_truncated_to_the_row() {
        let mut r = ready(r#"{"a": 1}"#);
        let out = render_sized(&mut r, 110, 20);
        assert!(
            out.contains("https://api.example.com/things?page=2"),
            "the full URL fits a wide pane: {out}"
        );

        // Too narrow for the whole URL after the chips: it truncates with
        // an ellipsis rather than vanishing or wrapping.
        let out = render_sized(&mut r, 80, 20);
        assert!(
            out.contains("https://") && out.contains('\u{2026}'),
            "a clipped URL ends in an ellipsis: {out}"
        );
    }

    #[test]
    fn human_size_and_elapsed_scale_up() {
        let mut d = data("x");
        d.size = 1434;
        d.elapsed = Duration::from_millis(1234);
        let mut r = Response::default();
        r.set_state(ResponseState::Ready(Box::new(d)), 0);
        let out = render(&mut r);
        assert!(out.contains("1.4 KB"), "{out}");
        assert!(out.contains("1.2 s"), "{out}");
    }

    #[test]
    fn status_chip_bg_is_tinted_with_the_semantic_status_color() {
        let theme = Theme::dark();
        let mut r = ready(r#"{"a": 1}"#);
        let out = render(&mut r);
        assert!(out.contains("200"), "status chip label: {out}");

        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        // The status chip is the first thing painted, at the header strip's
        // top-left corner (just inside the pane's 1-col padding; panes carry
        // no border of their own).
        let cell = buf.cell((2, 0)).expect("status digit cell");
        assert_eq!(cell.symbol(), "2", "expected the '2' of '200': {cell:?}");
        assert_eq!(
            cell.bg,
            theme.tint(theme.status_color(200), theme.panel),
            "chip bg is the status color tinted onto the strip's panel surface"
        );
        assert!(cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn timing_and_size_chips_are_plain_muted_text_not_control_filled() {
        // Ruling: chip fill means clickability. The timing/size figures on
        // row 1 aren't clickable, so they must not carry the `control` pill
        // fill — just muted text on the strip's `panel` surface.
        let theme = Theme::dark();
        let mut r = ready(r#"{"a": 1}"#);
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        // Row 0, just past the status chip (` 200 ` from x=1), lands in the
        // gap before the elapsed figure.
        let cell = buf.cell((6, 0)).expect("elapsed chip cell");
        assert_eq!(
            cell.bg, theme.panel,
            "timing chip must not be control-filled: {cell:?}"
        );
        // Find the "ms" text and confirm it's muted, not on control fill.
        let mut found = false;
        for x in 0..60u16 {
            let cell = buf.cell((x, 0)).unwrap();
            if cell.symbol() == "m" {
                assert_eq!(cell.fg, theme.text_muted, "elapsed text should be muted");
                assert_eq!(
                    cell.bg, theme.panel,
                    "elapsed text bg should be plain panel, not control"
                );
                found = true;
                break;
            }
        }
        assert!(found, "expected to find the elapsed chip's 'ms' text");
    }

    #[test]
    fn header_strip_is_three_rows_on_panel() {
        let theme = Theme::dark();
        let mut r = ready(r#"{"a": 1}"#);
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        // Rows 0..3 (panes carry no border of their own) are the strip.
        // Column 45 is blank on row 0 (past the status/timing/size/content
        // figures); column 10 is blank on row 1 (left of the right-aligned
        // tabs); column 20 is blank on row 2 (past the left-aligned icon
        // actions, short of the tabs' underline rule). All should still
        // read as panel fill.
        for (y, x) in [(0u16, 45u16), (1, 10), (2, 20)] {
            let cell = buf.cell((x, y)).unwrap();
            assert_eq!(
                cell.bg, theme.panel,
                "row {y} col {x} should be panel-filled outside the chips/tabs: {cell:?}"
            );
        }
        // Row 4 (the second body row — row 3 is the cursor row, which
        // paints its own highlight bg) is not part of the strip: it's on
        // the `page` surface instead.
        let body_row = buf.cell((2, 4)).unwrap();
        assert_eq!(
            body_row.bg, theme.page,
            "body area starts right below the 3-row strip: {body_row:?}"
        );
    }

    /// The hover highlight behind a header row's copy glyph must hold the
    /// glyph in its center — an even-width pill leaves the glyph shoved to
    /// one edge with a blank highlighted cell beside it.
    #[test]
    fn header_copy_glyph_is_centered_in_its_hover_pill() {
        let theme = Theme::dark();
        let mut r = ready("{}");
        r.set_view_mode(ViewMode::Headers);
        let hovered = crate::hit::Hit::HeaderCopy(0);
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: Some(&hovered),
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();

        // Locate the header row by its text, then collect the hover pill:
        // the contiguous run of accent-background cells on that row.
        let row_y = (0..buf.area.height)
            .find(|&y| {
                let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                line.contains("content-type:")
            })
            .expect("headers view shows the content-type row");
        let pill: Vec<String> = (0..buf.area.width)
            .filter(|&x| buf[(x, row_y)].bg == theme.accent)
            .map(|x| buf[(x, row_y)].symbol().to_string())
            .collect();
        assert_eq!(
            pill,
            vec![" ", "❐", " "],
            "the hovered pill is odd-width with the glyph in its center"
        );
    }

    #[test]
    fn response_tabs_register_hits_below_the_strip() {
        let mut r = ready(r#"{"a": 1}"#);
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let rect = hits
            .rect_of(&crate::hit::Hit::ResponseTab(ViewMode::Headers))
            .expect("Headers tab hit registered");
        assert_eq!(rect.y, 1, "tabs live on the strip's middle row");
        assert!(
            hits.rect_of(&crate::hit::Hit::ResponseTab(ViewMode::Raw))
                .is_some()
        );
        assert!(
            hits.rect_of(&crate::hit::Hit::ResponseTab(ViewMode::Pretty))
                .is_some()
        );
    }

    #[test]
    fn r_toggles_between_pretty_and_raw_verbatim() {
        let body = "{\"a\": 1,\n     \"b\": 2}";
        let mut r = ready(body);
        assert!(render(&mut r).contains("  \"a\": 1,"), "pretty re-indents");
        press(&mut r, ch('r'));
        let out = render(&mut r);
        assert!(out.contains("{\"a\": 1,"), "raw is verbatim: {out}");
        assert!(
            out.contains("     \"b\": 2}"),
            "raw keeps original spacing: {out}"
        );
        press(&mut r, ch('r'));
        assert!(
            render(&mut r).contains("  \"a\": 1,"),
            "toggles back to pretty"
        );
    }

    #[test]
    fn non_json_defaults_to_raw() {
        let mut r = ready("<html>hi</html>");
        assert!(render(&mut r).contains("<html>hi</html>"));
        // No tree, so `r` has nothing to toggle to.
        assert_eq!(r.handle_key(ch('r')), Some(Action::Render));
        assert!(render(&mut r).contains("<html>hi</html>"));
    }

    #[test]
    fn a_big_json_body_defers_its_parse_and_leads_with_the_spinning_tree_tab() {
        let body = big_json();
        let mut r = ready(&body);
        let v = r.view().unwrap();
        assert!(v.parsing, "the parse was handed off, not run inline");
        assert!(v.tree.is_none(), "no tree until it lands");
        assert_eq!(
            v.mode,
            ViewMode::Pretty,
            "the Tree tab leads, as it will once the parse lands"
        );
        let out = render(&mut r);
        assert!(out.contains("parsing"), "…showing the wait: {out}");
        assert!(out.contains("Raw"), "the raw body is a tab away: {out}");
    }

    #[test]
    fn a_big_body_that_does_not_look_like_json_leads_with_raw() {
        let body = format!("<html>{}</html>", "x".repeat(SYNC_PRETTY_BYTES));
        let r = ready(&body);
        let v = r.view().unwrap();
        assert!(
            v.parsing,
            "the parse still runs (the body might yet be JSON)"
        );
        assert_eq!(v.mode, ViewMode::Raw, "no spinner to sit through");
    }

    #[test]
    fn the_tree_tab_spins_while_a_big_body_is_parsed() {
        let mut r = ready(&big_json());
        r.set_view_mode(ViewMode::Pretty);
        assert_eq!(
            r.view().unwrap().mode,
            ViewMode::Pretty,
            "switching to Tree mid-parse is allowed"
        );
        let out = render(&mut r);
        assert!(out.contains("parsing"), "the wait is named: {out}");
        assert!(
            SPINNER.iter().any(|g| out.contains(*g)),
            "a spinner glyph: {out}"
        );
    }

    #[test]
    fn an_attached_tree_renders_in_the_pretty_view() {
        let body = big_json();
        let mut r = ready_gen(&body, 7);
        let tree = JsonTree::parse(&body).unwrap();
        assert!(
            r.attach_tree(7, Some(tree)),
            "delivered to the waiting view"
        );
        let v = r.view().unwrap();
        assert!(!v.parsing && v.tree.is_some());
        r.set_view_mode(ViewMode::Pretty);
        let out = render(&mut r);
        assert!(out.contains("\"a\""), "the parsed key is drawn: {out}");
        assert!(!out.contains("parsing"), "the spinner is gone: {out}");
    }

    #[test]
    fn a_tree_from_a_superseded_generation_is_dropped() {
        let body = big_json();
        let mut r = ready_gen(&body, 7);
        let tree = JsonTree::parse(&body).unwrap();
        assert!(!r.attach_tree(6, Some(tree)), "an older parse is not ours");
        assert!(r.view().unwrap().parsing, "still waiting for generation 7");

        let tree = JsonTree::parse(&body).unwrap();
        let mut r = ready_gen(&body, 7);
        assert!(r.attach_tree(7, Some(tree)));
        assert!(!r.attach_tree(7, None), "a second delivery is dropped");
        assert!(r.view().unwrap().tree.is_some(), "and changes nothing");
    }

    #[test]
    fn a_big_body_that_is_not_json_settles_on_raw() {
        let body = "x".repeat(SYNC_PRETTY_BYTES + 1);
        let mut r = ready_gen(&body, 3);
        r.set_view_mode(ViewMode::Pretty);
        assert!(r.attach_tree(3, None), "the parse said: not JSON");
        let v = r.view().unwrap();
        assert!(!v.parsing);
        assert_eq!(v.mode, ViewMode::Raw, "kicked back to the raw view");
        let out = render(&mut r);
        assert!(!out.contains("parsing"), "no spinner left behind: {out}");
        assert!(!out.contains("Tree"), "and no Tree tab to click: {out}");
    }

    #[test]
    fn headers_view_toggles_and_renders_a_header() {
        let mut r = ready(r#"{"a": 1}"#);
        press(&mut r, ch('h'));
        let out = render(&mut r);
        assert!(out.contains("content-type: application/json"), "{out}");
        press(&mut r, ch('h'));
        assert!(
            render(&mut r).contains("\"a\""),
            "h again returns to the body view"
        );
    }

    #[test]
    fn cursor_moves_and_clamps_at_both_ends() {
        let mut r = ready(r#"{"a": 1, "b": 2}"#); // 4 lines
        for _ in 0..20 {
            r.handle_key(ch('j'));
        }
        assert_eq!(r.view().unwrap().cursor, 3, "clamped to the last line");
        for _ in 0..20 {
            r.handle_key(key(KeyCode::Up));
        }
        assert_eq!(r.view().unwrap().cursor, 0, "clamped to the first line");
        r.handle_key(ch('G'));
        assert_eq!(r.view().unwrap().cursor, 3);
        r.handle_key(ch('g'));
        assert_eq!(r.view().unwrap().cursor, 0);
    }

    #[test]
    fn scroll_clamps_and_is_independent_of_the_cursor() {
        let body = format!(
            "[{}]",
            (0..50).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        );
        let mut r = ready(&body);
        render_sized(&mut r, 40, 10); // teaches the view its viewport height
        r.handle_scroll(-5);
        assert_eq!(r.view().unwrap().scroll, 0, "clamped at the top");
        assert_eq!(
            r.view().unwrap().cursor,
            0,
            "scrolling does not move the cursor"
        );
        r.handle_scroll(500);
        let v = r.view().unwrap();
        assert!(
            v.scroll > 0 && v.scroll < 52,
            "clamped inside the document: {}",
            v.scroll
        );
        assert_eq!(v.cursor, 0);
    }

    #[test]
    fn scrolling_moves_the_rendered_window() {
        let body = format!(
            "[{}]",
            (0..50)
                .map(|i| format!("\"e{i}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut r = ready(&body);
        let top = render_sized(&mut r, 40, 10);
        assert!(top.contains("\"e0\""));
        assert!(!top.contains("\"e40\""), "the tail is off screen: {top}");
        r.handle_scroll(40);
        let down = render_sized(&mut r, 40, 10);
        assert!(
            down.contains("\"e40\""),
            "scrolling reveals later lines: {down}"
        );
        assert!(!down.contains("\"e0\""), "and hides earlier ones: {down}");
    }

    /// A one-line body that is not JSON (so the view settles on `Raw`) and
    /// is far wider than the 60-col test viewport; "TAIL" is only visible
    /// once the view is scrolled right.
    fn wide_raw() -> Response {
        ready(&format!("{}TAIL", "x".repeat(100)))
    }

    /// The buffer rows of a rendered frame, in order — `TestBackend`'s
    /// `Debug` prints each row as its own quoted line.
    fn buffer_rows(rendered: &str) -> Vec<String> {
        rendered
            .lines()
            .filter(|l| l.trim_start().starts_with('"'))
            .map(|l| l.to_string())
            .collect()
    }

    /// Draws `resp` at 60x20 and returns the frame's hits.
    fn render_hits(resp: &mut Response) -> crate::hit::HitMap {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| resp.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        hits
    }

    #[test]
    fn page_keys_move_the_cursor_by_a_viewport_page() {
        let body = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut r = ready(&body);
        render(&mut r); // records the viewport height
        let height = {
            let v = r.view().unwrap();
            assert_eq!(v.cursor, 0);
            v.height
        };
        r.handle_key(key(KeyCode::PageDown));
        let v = r.view().unwrap();
        assert_eq!(v.cursor, height, "PageDown jumps a full viewport");
        assert!(v.scroll > 0, "the viewport follows the cursor down");
        r.handle_key(key(KeyCode::PageUp));
        let v = r.view().unwrap();
        assert_eq!(v.cursor, 0, "PageUp comes back and clamps at the top");
    }

    #[test]
    fn end_key_jumps_to_the_widest_line_end() {
        let mut r = wide_raw();
        render(&mut r);
        r.handle_key(key(KeyCode::End));
        let out = render(&mut r);
        assert!(out.contains("TAIL"), "End shows the line's end: {out}");
        r.handle_key(key(KeyCode::Home));
        assert_eq!(r.view().unwrap().h_scroll, 0);
    }

    #[test]
    fn ctrl_home_and_ctrl_end_jump_to_document_top_and_bottom() {
        let body = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut r = ready(&body);
        render(&mut r);
        r.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
        let v = r.view().unwrap();
        assert_eq!(v.cursor, 99, "ctrl+End lands on the last line");
        assert!(v.scroll > 0, "the viewport follows to the bottom");
        r.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
        let v = r.view().unwrap();
        assert_eq!(v.cursor, 0, "ctrl+Home lands on the first line");
        assert_eq!(v.scroll, 0, "the viewport follows back to the top");
    }

    #[test]
    fn the_horizontal_scrollbar_is_a_real_control_with_thumb_and_track_hits() {
        let mut r = wide_raw();
        render(&mut r);
        r.handle_scroll_h(20); // off both edges, so both track sides exist
        let hits = render_hits(&mut r);
        let thumb = hits
            .rect_of(&crate::hit::Hit::HScrollThumb(PaneId::Response))
            .expect("the horizontal thumb must be a registered hit");
        assert_eq!(thumb.height, 1, "the bar lives on a single row");
        assert!(
            hits.h_track_of(PaneId::Response).is_some(),
            "the full bar row is recorded as the horizontal track"
        );
        let page = |d: i16| crate::hit::Hit::HScrollTrack(PaneId::Response, d);
        let viewport = r.view().unwrap().width() as i16;
        assert!(
            hits.rect_of(&page(-viewport)).is_some(),
            "clicking left of the thumb pages left"
        );
        assert!(
            hits.rect_of(&page(viewport)).is_some(),
            "clicking right of the thumb pages right"
        );
    }

    #[test]
    fn set_scroll_h_jumps_and_clamps_like_the_wheel() {
        let mut r = wide_raw();
        render(&mut r);
        assert!(r.set_scroll_h(10), "moving to a new offset reports change");
        assert_eq!(r.view().unwrap().h_scroll, 10);
        r.set_scroll_h(10_000);
        let v = r.view().unwrap();
        assert!(
            v.h_scroll > 10 && v.h_scroll < 104,
            "clamped to the widest line minus the viewport: {}",
            v.h_scroll
        );
        assert!(
            !r.set_scroll_h(v.h_scroll),
            "a no-op move reports no change"
        );
    }

    #[test]
    fn horizontal_scroll_reveals_clipped_columns_and_clamps() {
        let mut r = wide_raw();
        let before = render(&mut r); // first draw records the viewport size
        assert!(!before.contains("TAIL"), "clipped at 60 cols: {before}");
        r.handle_scroll_h(500);
        let after = render(&mut r);
        assert!(
            after.contains("TAIL"),
            "scrolled to the line's end: {after}"
        );
        r.handle_scroll_h(-1000);
        assert_eq!(r.view().unwrap().h_scroll, 0, "clamped at the left edge");
    }

    #[test]
    fn left_right_and_home_keys_scroll_horizontally() {
        let mut r = wide_raw();
        render(&mut r);
        for _ in 0..30 {
            r.handle_key(key(KeyCode::Right));
        }
        let out = render(&mut r);
        assert!(out.contains("TAIL"), "right key scrolls and clamps: {out}");
        r.handle_key(key(KeyCode::Left));
        assert!(r.view().unwrap().h_scroll > 0, "left steps back");
        r.handle_key(key(KeyCode::Home));
        assert_eq!(r.view().unwrap().h_scroll, 0, "Home jumps to column 0");
    }

    #[test]
    fn horizontal_scroll_resets_when_the_view_mode_changes() {
        let mut r = wide_raw();
        render(&mut r);
        r.handle_scroll_h(20);
        assert!(r.view().unwrap().h_scroll > 0);
        press(&mut r, ch('h'));
        assert_eq!(
            r.view().unwrap().h_scroll,
            0,
            "column offset is per-view state, reset on a view switch"
        );
    }

    #[test]
    fn a_wide_body_draws_a_horizontal_scrollbar_and_a_narrow_one_does_not() {
        let mut r = wide_raw();
        let wide = render(&mut r);
        let rows = buffer_rows(&wide);
        let bottom = rows.last().expect("rendered rows");
        assert!(bottom.contains('█'), "thumb on the bottom row: {wide}");
        assert!(bottom.contains('─'), "track on the bottom row: {wide}");

        let mut narrow = ready("short");
        let out = render(&mut narrow);
        let rows = buffer_rows(&out);
        let bottom = rows.last().expect("rendered rows");
        assert!(
            !bottom.contains('█') && !bottom.contains('─'),
            "no indicator when nothing is clipped: {out}"
        );
    }

    #[test]
    fn json_arrow_hits_are_suppressed_while_scrolled_horizontally() {
        let body = format!("{{\"key\": \"{}\"}}", "x".repeat(100));
        let mut r = ready(&body);
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let draw = |r: &mut Response| {
            let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            hits
        };
        let hits = draw(&mut r);
        assert!(
            hits.rect_of(&crate::hit::Hit::JsonArrow(0)).is_some(),
            "the root container's arrow is clickable unscrolled"
        );
        r.handle_scroll_h(10);
        let hits = draw(&mut r);
        assert!(
            hits.rect_of(&crate::hit::Hit::JsonArrow(0)).is_none(),
            "the arrow has scrolled off screen, so its hit must go too"
        );
    }

    #[test]
    fn space_toggles_collapse_at_the_cursor() {
        let mut r = ready(r#"{"a": {"b": 1, "c": 2}}"#);
        let before = r.view().unwrap().visible_len();
        r.handle_key(ch('j')); // onto the "a" container line
        assert_eq!(r.handle_key(key(KeyCode::Char(' '))), Some(Action::Render));
        assert!(r.view().unwrap().visible_len() < before, "collapsed");
        assert!(render(&mut r).contains("2 keys"));
        r.handle_key(key(KeyCode::Enter));
        assert_eq!(r.view().unwrap().visible_len(), before, "re-expanded");
    }

    #[test]
    fn search_commits_counts_matches_and_wraps() {
        let mut r = ready(r#"{"aa": "zz", "bb": "zz"}"#);
        assert_eq!(r.handle_key(ch('/')), Some(Action::Render));
        for c in "zz".chars() {
            assert_eq!(r.handle_key(ch(c)), Some(Action::Render));
        }
        // While the input is active, other response keys are suspended.
        assert_eq!(r.view().unwrap().cursor, 0);
        assert!(
            render(&mut r).contains("/zz"),
            "the query echoes in the footer"
        );
        r.handle_key(key(KeyCode::Enter));
        let out = render(&mut r);
        assert!(out.contains("1/2"), "match counter: {out}");
        assert_eq!(r.view().unwrap().cursor, 1, "jumped to the first match");
        r.handle_key(ch('n'));
        assert!(render(&mut r).contains("2/2"));
        r.handle_key(ch('n'));
        assert!(render(&mut r).contains("1/2"), "n wraps around");
        r.handle_key(ch('N'));
        assert!(render(&mut r).contains("2/2"), "N wraps backwards");
        r.handle_key(key(KeyCode::Esc));
        assert!(r.view().unwrap().search.is_none(), "esc closes search");
    }

    /// The mouse route: `\u{2315}` opens the search, the query is typed, and
    /// the `\u{25bc}` button both commits it and lands on the first match —
    /// there is no click that means "Enter", so a button that only stepped
    /// a never-committed query left the whole flow inert.
    #[test]
    fn the_next_match_button_commits_a_live_query_and_lands_on_the_first_match() {
        let mut r = ready(r#"{"aa": "zz", "bb": "zz"}"#);
        r.open_search();
        for c in "zz".chars() {
            r.handle_key(ch(c));
        }
        assert!(r.view().unwrap().search.as_ref().unwrap().active);

        assert!(r.step_search(1));
        let search = r.view().unwrap().search.as_ref().unwrap();
        assert!(!search.active, "the click committed the query");
        assert_eq!(search.query, "zz");
        assert_eq!(search.matches.len(), 2);
        let out = render(&mut r);
        assert!(
            out.contains("1/2"),
            "on the first match, not the second: {out}"
        );

        // A second click steps, as `n` does.
        assert!(r.step_search(1));
        assert!(render(&mut r).contains("2/2"));
    }

    #[test]
    fn typing_while_search_is_active_does_not_move_the_cursor() {
        let mut r = ready(r#"{"a": 1, "b": 2}"#);
        r.handle_key(ch('/'));
        for c in "jjG".chars() {
            assert_eq!(r.handle_key(ch(c)), Some(Action::Render));
        }
        assert_eq!(r.view().unwrap().cursor, 0, "navigation keys are suspended");
        assert_eq!(
            r.view().unwrap().search.as_ref().unwrap().input.text(),
            "jjG"
        );
    }

    #[test]
    fn search_jumps_into_a_collapsed_container() {
        let mut r = ready(r#"{"a": {"b": {"c": "needle"}}}"#);
        r.handle_key(ch('j'));
        r.handle_key(key(KeyCode::Char(' '))); // collapse "a"
        assert!(!render(&mut r).contains("needle"), "hidden while collapsed");
        r.handle_key(ch('/'));
        for c in "needle".chars() {
            r.handle_key(ch(c));
        }
        r.handle_key(key(KeyCode::Enter));
        let out = render(&mut r);
        assert!(
            out.contains("needle"),
            "jumping expands the ancestors: {out}"
        );
        assert!(out.contains("1/1"));
    }

    #[test]
    fn search_is_case_insensitive_and_finds_nothing_gracefully() {
        let mut r = ready(r#"{"Needle": 1}"#);
        r.handle_key(ch('/'));
        for c in "NEEDLE".chars() {
            r.handle_key(ch(c));
        }
        r.handle_key(key(KeyCode::Enter));
        assert!(render(&mut r).contains("1/1"));
        r.handle_key(ch('/'));
        for c in "absent".chars() {
            r.handle_key(ch(c));
        }
        r.handle_key(key(KeyCode::Enter));
        let out = render(&mut r);
        assert!(out.contains("no matches"), "{out}");
        assert_eq!(
            r.handle_key(ch('n')),
            Some(Action::Render),
            "n with no matches is inert"
        );
    }

    #[test]
    fn search_works_in_raw_view_too() {
        let mut r = ready("plain text with a needle inside");
        r.handle_key(ch('/'));
        for c in "needle".chars() {
            r.handle_key(ch(c));
        }
        r.handle_key(key(KeyCode::Enter));
        assert!(render(&mut r).contains("1/1"));
    }

    #[test]
    fn in_flight_shows_a_spinner_and_the_cancel_hint() {
        let mut r = Response::default();
        r.set_state(
            ResponseState::InFlight {
                started: Instant::now(),
            },
            0,
        );
        let out = render(&mut r);
        assert!(
            SPINNER.iter().any(|g| out.contains(*g)),
            "a spinner glyph: {out}"
        );
        assert!(out.contains("esc to cancel"), "{out}");
    }

    #[test]
    fn a_long_in_flight_wait_shows_a_warning_line() {
        // There is no client timeout any more — the user decides when to
        // give up — so a slow request warns instead of dying at 30s.
        let mut r = Response::default();
        r.set_state(
            ResponseState::InFlight {
                started: Instant::now() - std::time::Duration::from_secs(11),
            },
            0,
        );
        let out = render(&mut r);
        assert!(out.contains("taking a while"), "{out}");

        r.set_state(
            ResponseState::InFlight {
                started: Instant::now(),
            },
            0,
        );
        let out = render(&mut r);
        assert!(
            !out.contains("taking a while"),
            "no warning early on: {out}"
        );
    }

    #[test]
    fn failed_and_cancelled_render_their_messages() {
        let mut r = Response::default();
        r.set_state(ResponseState::Failed("connection refused".into()), 0);
        assert!(render(&mut r).contains("connection refused"));
        r.set_state(ResponseState::Cancelled, 0);
        assert!(render(&mut r).contains("Request cancelled"));
        r.set_state(ResponseState::Empty, 0);
        assert!(render(&mut r).contains("response will appear here"));
    }

    #[test]
    fn keys_do_nothing_outside_the_ready_state() {
        let mut r = Response::default();
        assert_eq!(
            r.handle_key(ch('j')),
            None,
            "an empty pane ignores navigation"
        );
        assert_eq!(r.handle_key(ch('/')), None);
        r.set_state(
            ResponseState::InFlight {
                started: Instant::now(),
            },
            0,
        );
        assert_eq!(r.handle_key(key(KeyCode::Esc)), Some(Action::CancelSend));
        assert_eq!(r.handle_key(ch('j')), None);
    }

    #[test]
    fn r_and_h_dispatch_response_view_mode_actions() {
        // Through the action, not a direct mutation: `app.rs`'s
        // `Action::ResponseViewMode` arm is what retargets the animated
        // tab underline, so the keyboard path must funnel through it
        // exactly like a tab click does.
        let mut r = ready(r#"{"a": 1}"#);
        assert_eq!(
            r.handle_key(ch('r')),
            Some(Action::ResponseViewMode(ViewMode::Raw))
        );
        assert_eq!(
            r.handle_key(ch('h')),
            Some(Action::ResponseViewMode(ViewMode::Headers))
        );
    }

    #[test]
    fn search_button_is_the_magnifying_glass() {
        // The `⌕` glyph reads as a refresh arrow in many fonts; the emoji
        // magnifier can't be misread.
        let mut r = ready(r#"{"a": 1}"#);
        let out = render(&mut r);
        assert!(out.contains("\u{F0349}"), "search icon: {out}");
        assert!(!out.contains("⌕"), "{out}");
    }

    /// Renders with a hovered hit, returning (buffer content, hits).
    fn render_hovered(
        resp: &mut Response,
        hovered: Option<&crate::hit::Hit>,
    ) -> (String, crate::hit::HitMap) {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| resp.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    /// The status figures share the top row with the status chip: `200
    /// 342 ms  18 B  application/json` all on one line.
    #[test]
    fn status_timing_size_and_content_type_share_the_top_row() {
        let theme = Theme::dark();
        let mut r = ready(r#"{"a": 1}"#);
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        let row0: String = (0..60u16)
            .map(|x| buf.cell((x, 0)).unwrap().symbol())
            .collect();
        assert!(row0.contains("200"), "status chip on row 0: {row0}");
        assert!(row0.contains("342 ms"), "elapsed on row 0: {row0}");
        assert!(row0.contains("8 B"), "size on row 0: {row0}");
        assert!(
            row0.contains("application/json"),
            "content type on row 0: {row0}"
        );
    }

    /// The header actions are icons — search, copy body, save to file —
    /// Nerd Font glyphs at one cell each, sitting
    /// left-aligned on the underline row, directly above the body they act
    /// on. The old text labels are gone.
    #[test]
    fn header_actions_are_icons_on_the_underline_row() {
        let mut r = ready(r#"{"a": 1}"#);
        let (out, hits) = render_hovered(&mut r, None);
        assert!(out.contains("\u{F018F}"), "copy icon: {out}");
        assert!(out.contains("\u{F0193}"), "save icon: {out}");
        assert!(!out.contains("Copy body"), "no text label at rest: {out}");
        assert!(
            !out.contains("Save to file"),
            "no text label at rest: {out}"
        );
        let search = hits
            .rect_of(&crate::hit::Hit::ResponseSearchButton)
            .expect("search hit");
        let copy = hits
            .rect_of(&crate::hit::Hit::CopyBodyButton)
            .expect("copy hit");
        let save = hits
            .rect_of(&crate::hit::Hit::SaveBodyButton)
            .expect("save hit");
        for (name, rect) in [("search", search), ("copy", copy), ("save", save)] {
            assert_eq!(rect.y, 2, "{name} sits on the underline row: {rect:?}");
        }
        assert!(
            search.x < save.x && save.x < copy.x,
            "search / save / copy, left to right — copy keeps the right edge"
        );
        assert!(
            copy.x + copy.width < 22,
            "icons are left-aligned, not flushed right: {copy:?}"
        );
    }

    /// The pane no longer paints split buttons of its own — the column's
    /// five-stop control lives in the editor's fixed tab-bar row, so
    /// this header never registers a `SplitStop` hit that would move out
    /// from under a click.
    #[test]
    fn header_offers_no_split_hits_of_its_own() {
        let mut r = ready(r#"{"a": 1}"#);
        let (_, hits) = render_hovered(&mut r, None);
        for stop in crate::split::SplitStop::ALL {
            assert!(hits.rect_of(&crate::hit::Hit::SplitStop(stop)).is_none());
        }
    }

    /// Collapsed with no response: the pane is nothing but its one-row
    /// strip — the centered empty-state message is gone.
    #[test]
    fn collapsed_empty_state_is_just_the_strip() {
        let mut r = Response {
            collapsed: true,
            ..Default::default()
        };
        let out = render_sized(&mut r, 60, 1);
        assert!(!out.contains("Send a request"), "{out}");
    }

    /// Collapsed mid-send: the strip keeps a compact state hint so the
    /// in-flight send isn't invisible, without the full pane's hint text.
    #[test]
    fn collapsed_in_flight_shows_a_state_hint() {
        let mut r = Response::default();
        r.set_state(
            ResponseState::InFlight {
                started: Instant::now(),
            },
            0,
        );
        r.collapsed = true;
        let out = render_sized(&mut r, 60, 1);
        assert!(out.contains("sending"), "{out}");
        assert!(!out.contains("esc to cancel"), "{out}");
    }

    /// The header's own split affordance is the one-step ▲/▼ pill,
    /// right-aligned on row 0 — the row that survives every state, so the
    /// arrows are reachable expanded, collapsed, and before the first
    /// response has arrived.
    #[test]
    fn header_row_0_carries_the_step_pill_in_every_state() {
        use crate::hit::Hit;
        let mut r = ready(r#"{"a": 1}"#);
        let mut empty = Response::default();
        for (name, resp) in [("ready", &mut r), ("empty", &mut empty)] {
            for collapsed in [false, true] {
                resp.collapsed = collapsed;
                let hits = render_hits(resp);
                let up = hits
                    .rect_of(&Hit::SplitStep(1))
                    .unwrap_or_else(|| panic!("{name} collapsed={collapsed}: ▲ hit"));
                let down = hits
                    .rect_of(&Hit::SplitStep(-1))
                    .unwrap_or_else(|| panic!("{name} collapsed={collapsed}: ▼ hit"));
                assert_eq!(up.y, 0, "{name} collapsed={collapsed}");
                assert_eq!(down.y, 0, "{name} collapsed={collapsed}");
                assert_eq!(up.x + up.width, down.x, "▲ then ▼, flush");
                // Right-aligned: the trailing cap sits one cell in from the
                // pane's inner right edge (the 60-wide pane insets one
                // column each side).
                assert_eq!(
                    down.x + down.width + 1,
                    60 - 2,
                    "{name} collapsed={collapsed}"
                );
            }
        }
    }

    /// The pill lights from the display copy of the split the app hands
    /// the pane: with the response minimized, ▼ has nowhere to go.
    #[test]
    fn step_pill_greys_from_the_panes_split_copy() {
        let theme = Theme::dark();
        let mut r = ready(r#"{"a": 1}"#);
        r.collapsed = true;
        r.split = crate::split::SplitState {
            response_minimized: true,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let down = hits.rect_of(&crate::hit::Hit::SplitStep(-1)).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(down.x + 1, 0)].symbol(), "\u{25BC}");
        assert_eq!(buf[(down.x + 1, 0)].fg, theme.text_disabled);
        let up = hits.rect_of(&crate::hit::Hit::SplitStep(1)).unwrap();
        assert_ne!(buf[(up.x + 1, 0)].fg, theme.text_disabled, "▲ still live");
    }

    /// A long sent URL truncates short of the pill instead of running
    /// underneath it.
    #[test]
    fn sent_url_stops_clear_of_the_step_pill() {
        let mut r = ready(r#"{"a": 1}"#);
        if let ResponseState::Ready(d) = &mut r.state {
            d.url = format!("https://example.com/{}", "x".repeat(200));
        }
        let theme = Theme::dark();
        // Wide enough that the row-0 facts leave room for some URL.
        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        let up = hits.rect_of(&crate::hit::Hit::SplitStep(1)).unwrap();
        let cap_x = up.x - 1;
        assert_eq!(buf[(cap_x, 0)].bg, theme.control, "leading cap");
        // The cell before the cap is a gap, and the URL ends in an
        // ellipsis just before it — it never runs under the pill.
        assert_eq!(buf[(cap_x - 1, 0)].symbol(), " ", "gap before the pill");
        assert_eq!(
            buf[(cap_x - 2, 0)].symbol(),
            "\u{2026}",
            "URL ends in an ellipsis"
        );
        assert_eq!(
            buf[(cap_x - 3, 0)].symbol(),
            "x",
            "URL text runs right up to it"
        );
    }

    /// Before a response lands the pane still reads as a pane with a
    /// header: row 0 is the same panel-tone strip the ready header uses,
    /// with a status-shaped chip in the status chip's slot naming the
    /// state (`—` empty, spinner in flight, `failed`, `cancelled`) and the
    /// step pill at the right — and the centred body message below it.
    #[test]
    fn pending_pane_row_0_is_a_panel_strip_with_a_state_chip() {
        let theme = Theme::dark();
        let cases: Vec<(&str, ResponseState, &str, ratatui::style::Color)> = vec![
            ("empty", ResponseState::Empty, "\u{2014}", theme.text_muted),
            (
                "failed",
                ResponseState::Failed("boom".into()),
                "failed",
                theme.error,
            ),
            (
                "cancelled",
                ResponseState::Cancelled,
                "cancelled",
                theme.text_muted,
            ),
        ];
        for (name, state, word, color) in cases {
            let mut r = Response::default();
            r.set_state(state, 0);
            let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
                dragging: false,
                anims: test_anims(),
                now: std::time::Instant::now(),
            };
            terminal
                .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            let buf = terminal.backend().buffer();
            // The strip spans the inner width on the panel tone (a cell
            // clear of both the chip and the pill).
            assert_eq!(
                buf[(30, 0)].bg,
                theme.panel,
                "{name}: row 0 is a panel strip"
            );
            assert_eq!(
                buf[(30, 1)].bg,
                theme.page,
                "{name}: the body stays page-toned"
            );
            // The chip sits where the ready header's status chip sits:
            // inner x (1), label from x + 1.
            let label: String = (2..2 + word.chars().count() as u16)
                .map(|x| buf[(x, 0)].symbol().to_string())
                .collect();
            assert_eq!(label, word, "{name}: chip label");
            // The chip's fill is the state tone tinted onto the strip,
            // exactly as the ready status chip is (its text picks a
            // contrasting tone from that fill).
            assert_eq!(
                buf[(2, 0)].bg,
                theme.tint(color, theme.panel),
                "{name}: chip fill"
            );
            assert!(
                hits.rect_of(&crate::hit::Hit::SplitStep(1)).is_some(),
                "{name}: pill still on row 0"
            );
        }
        // The body message survives, below the strip.
        let mut r = Response::default();
        let out = render(&mut r);
        assert!(out.contains("Send a request"), "{out}");
    }

    /// In flight, the chip carries the spinner and elapsed time — the
    /// strip is where the wait shows, collapsed or not.
    #[test]
    fn pending_in_flight_chip_shows_the_spinner() {
        let mut r = Response::default();
        r.set_state(
            ResponseState::InFlight {
                started: std::time::Instant::now(),
            },
            0,
        );
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        let row0: String = (0..60).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(row0.contains("sending"), "{row0}");
        assert_ne!(buf[(2, 0)].bg, theme.panel, "chip has a fill");
    }

    /// Hiding hides the controls too: collapsed, the header strip drops its
    /// tab strip and icon actions — only row 0 (status chip + `› show`)
    /// survives.
    #[test]
    fn collapsed_ready_header_drops_tabs_and_icon_actions() {
        let mut r = ready(r#"{"a": 1}"#);
        r.collapsed = true;
        let hits = render_hits(&mut r);
        assert!(
            hits.rect_of(&crate::hit::Hit::ResponseTab(ViewMode::Raw))
                .is_none(),
            "tabs are gone while hidden"
        );
        assert!(
            hits.rect_of(&crate::hit::Hit::ResponseSearchButton)
                .is_none()
        );
        assert!(hits.rect_of(&crate::hit::Hit::SaveBodyButton).is_none());
        assert!(hits.rect_of(&crate::hit::Hit::CopyBodyButton).is_none());
    }

    /// The icon buttons carry no hover tooltip (a one-line floating label
    /// read as noise) — hovering one changes only the button's own styling.
    #[test]
    fn hovered_header_action_raises_no_tooltip() {
        for (hit, name) in [
            (crate::hit::Hit::ResponseSearchButton, "Search"),
            (crate::hit::Hit::CopyBodyButton, "Copy body"),
            (crate::hit::Hit::SaveBodyButton, "Save to file"),
        ] {
            let mut r = ready(r#"{"a": 1}"#);
            let (out, _) = render_hovered(&mut r, Some(&hit));
            assert!(!out.contains(name), "no tooltip for {hit:?}: {out}");
        }
    }

    #[test]
    fn set_view_mode_switches_the_view() {
        let mut r = ready(r#"{"a": 1}"#);
        r.set_view_mode(ViewMode::Headers);
        assert_eq!(r.view().unwrap().mode, ViewMode::Headers);
    }

    #[test]
    fn set_view_mode_is_a_no_op_when_the_body_has_no_tree() {
        let mut r = ready("<html>hi</html>");
        assert_eq!(r.view().unwrap().mode, ViewMode::Raw, "non-JSON is raw");
        r.set_view_mode(ViewMode::Pretty);
        assert_eq!(
            r.view().unwrap().mode,
            ViewMode::Raw,
            "there is no tree to switch to"
        );
    }

    #[test]
    fn click_row_toggle_collapses_a_container_row() {
        let mut r = ready(r#"{"a": {"b": 1, "c": 2}}"#);
        let before = r.view().unwrap().visible_len();
        r.click_row(1, true); // the "a" container line
        assert!(r.view().unwrap().visible_len() < before, "collapsed");
        assert_eq!(r.view().unwrap().cursor, 1);
    }

    #[test]
    fn click_row_without_toggle_only_moves_the_cursor() {
        let mut r = ready(r#"{"a": 1, "b": 2}"#);
        let before = r.view().unwrap().visible_len();
        r.click_row(2, false);
        assert_eq!(r.view().unwrap().cursor, 2);
        assert_eq!(r.view().unwrap().visible_len(), before, "no collapse");
    }

    #[test]
    fn a_new_response_resets_the_view() {
        let mut r = ready("{\"a\": 1,\n \"b\": 2}");
        r.handle_key(ch('G'));
        assert_eq!(r.view().unwrap().cursor, 3, "last pretty line");
        press(&mut r, ch('r'));
        assert_eq!(
            r.view().unwrap().cursor,
            0,
            "switching views restarts at the top"
        );
        r.handle_key(ch('G'));
        assert_eq!(r.view().unwrap().cursor, 1, "last raw line");
        r.set_state(ResponseState::Ready(Box::new(data(r#"{"z": 1}"#))), 0);
        let v = r.view().unwrap();
        assert_eq!(v.cursor, 0);
        assert_eq!(v.scroll, 0);
        assert!(v.search.is_none());
        assert!(
            render(&mut r).contains("\"z\""),
            "back to pretty for the new body"
        );
    }

    const ITEMS: &str =
        r#"{"data":{"items":[{"id":1,"status":"active"},{"id":2,"status":"off"}]}}"#;

    #[test]
    fn applying_a_filter_swaps_the_filtered_tree_into_the_pretty_view() {
        let mut r = ready(ITEMS);
        assert!(
            r.apply_jq(".data.items | length", SYNC_PRETTY_BYTES)
                .is_none(),
            "small bodies run inline"
        );
        let view = r.view().unwrap();
        assert_eq!(view.view_text(), "2");
        assert_eq!(view.visible_len(), 1);
        assert_eq!(r.jq_output_count(), 1);
        assert!(r.jq_bar().error.is_none());
        r.apply_jq("", SYNC_PRETTY_BYTES);
        assert!(
            r.view().unwrap().view_text().starts_with("{"),
            "an empty filter shows the body again"
        );
    }

    /// Types `text` into a focused bar the way `App::sync_jq` drives it:
    /// apply the filter, then refresh the completion.
    fn type_jq(r: &mut Response, text: &str) {
        r.set_jq_focus(true);
        r.jq_bar_mut().input = LineInput::new(text);
        r.apply_jq(text, SYNC_PRETTY_BYTES);
        assert!(
            r.refresh_jq_completion(SYNC_PRETTY_BYTES).is_none(),
            "small bodies fetch inline"
        );
    }

    #[test]
    fn a_dot_ghosts_the_first_key_of_what_the_caret_sees() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".");
        assert_eq!(r.jq_ghost(), Some("data"));
        type_jq(&mut r, ".data.");
        assert_eq!(r.jq_ghost(), Some("items"));
        type_jq(&mut r, ".data.items[] | .s");
        assert_eq!(r.jq_ghost(), Some("tatus"));
        type_jq(&mut r, ".data.items[] | select(.st");
        assert_eq!(r.jq_ghost(), Some("atus"), "inside select the dot is the item");
        type_jq(&mut r, ".data.items[] | .zz");
        assert_eq!(r.jq_ghost(), None);
    }

    #[test]
    fn a_word_ghosts_a_builtin_even_with_no_document() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.items | leng");
        assert_eq!(r.jq_ghost(), Some("th"));
        // `set_jq_focus` refuses a non-JSON body (no tree to filter), so
        // this seeds focus directly instead — the reachable real case is
        // a bar already focused when a non-JSON response lands.
        let type_jq_unfocusable = |r: &mut Response, text: &str| {
            r.jq_bar_mut().focused = true;
            r.jq_bar_mut().input = LineInput::new(text);
            r.apply_jq(text, SYNC_PRETTY_BYTES);
            assert!(
                r.refresh_jq_completion(SYNC_PRETTY_BYTES).is_none(),
                "small bodies fetch inline"
            );
        };
        let mut r = ready("<html>");
        type_jq_unfocusable(&mut r, "leng");
        assert_eq!(r.jq_ghost(), Some("th"), "builtins need no body");
        type_jq_unfocusable(&mut r, ".d");
        assert_eq!(r.jq_ghost(), None, "no document, no keys");
    }

    #[test]
    fn the_ghost_hides_off_the_end_of_the_text_and_when_unfocused() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.");
        assert!(r.jq_ghost().is_some());
        r.jq_bar_mut().input.set_cursor(3);
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_ghost(), None, "caret mid-text");
        r.jq_bar_mut().input.set_cursor(6);
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_ghost(), Some("items"));
        r.jq_bar_mut().input.select_all();
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_ghost(), None, "a selection hides it");
        r.jq_bar_mut().input.clear_selection();
        r.set_jq_focus(false);
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_ghost(), None, "unfocused");
    }

    #[test]
    fn typing_more_of_a_partial_reuses_the_fetched_keys() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.items[] | .");
        assert_eq!(r.jq_ghost(), Some("id"));
        // Same context, longer partial: no fetch would be needed even for
        // a big body — the request path is exercised in the app tests;
        // here the observable is the ghost narrowing.
        r.jq_bar_mut().input = LineInput::new(".data.items[] | .st");
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_ghost(), Some("atus"));
    }

    #[test]
    fn a_big_body_fetches_keys_on_the_pool_and_attaches_by_sequence() {
        let big = format!(
            r#"{{"pad": "{}", "n": 7}}"#,
            "x".repeat(SYNC_PRETTY_BYTES)
        );
        let mut r = ready_gen(&big, 3);
        r.attach_tree(3, crate::components::json_tree::JsonTree::parse(&big));
        r.set_jq_focus(true);
        r.jq_bar_mut().input = LineInput::new(".");
        // The first filter run parses the document on the pool; feed it
        // back as the app would.
        let req = r.apply_jq(".", SYNC_PRETTY_BYTES).expect("background run");
        let doc = postui_core::jq::JqDocument::parse(&big).unwrap();
        let out = JqRunOutput::from_outputs(Some(doc.clone()), vec![big.clone()]);
        r.attach_jq_result(3, req.run, Ok(out));
        let creq = r
            .refresh_jq_completion(SYNC_PRETTY_BYTES)
            .expect("a big body fetches keys on the pool");
        assert_eq!((creq.generation, creq.input_expr.as_str()), (3, "."));
        assert_eq!(r.jq_ghost(), None, "nothing until the fetch lands");
        assert!(
            r.refresh_jq_completion(SYNC_PRETTY_BYTES).is_none(),
            "the same context is not fetched twice while pending"
        );
        assert!(!r.attach_jq_completion(2, creq.seq, ".".into(), vec!["pad".into()]), "stale generation");
        assert!(!r.attach_jq_completion(3, creq.seq + 9, ".".into(), vec!["pad".into()]), "stale sequence");
        assert!(r.attach_jq_completion(3, creq.seq, ".".into(), vec!["pad".into(), "n".into()]));
        assert_eq!(r.jq_ghost(), Some("pad"));
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    /// One keystroke in the focused bar, followed by the app's reconcile
    /// (filter re-applied, completion refreshed).
    fn bar_key(r: &mut Response, ev: KeyEvent) {
        r.handle_key(ev);
        let text = r.jq_text().to_string();
        r.apply_jq(&text, SYNC_PRETTY_BYTES);
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
    }

    #[test]
    fn tab_cycles_and_right_accepts_in_cycle_mode() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.items[] | .");
        assert_eq!(r.jq_ghost(), Some("id"));
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_ghost(), Some("status"));
        assert_eq!(r.jq_text(), ".data.items[] | .", "Tab does not edit");
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_ghost(), Some("id"), "wraps");
        bar_key(&mut r, shift(KeyCode::BackTab));
        assert_eq!(r.jq_ghost(), Some("status"), "shift+Tab goes back");
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".data.items[] | .status");
        assert_eq!(r.jq_ghost(), None, "accepted: nothing more to add");
        assert_eq!(r.jq_output_count(), 2, "the filter ran");
    }

    #[test]
    fn end_accepts_too_and_typing_resets_the_cycle() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_ghost(), Some("status"));
        bar_key(&mut r, ch('i'));
        assert_eq!(r.jq_ghost(), Some("d"), "a new partial starts at the first candidate");
        bar_key(&mut r, key(KeyCode::End));
        assert_eq!(r.jq_text(), ".data.items[] | .id");
    }

    #[test]
    fn tab_accepts_in_accept_mode_and_shift_tab_does_nothing() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Accept);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, shift(KeyCode::BackTab));
        assert_eq!(r.jq_text(), ".data.items[] | .");
        assert_eq!(r.jq_ghost(), Some("id"));
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.items[] | .id");
    }

    #[test]
    fn accepting_a_quoted_key_rewrites_the_token() {
        let mut r = ready(r#"{"my key": 1, "myth": 2}"#);
        type_jq(&mut r, ".my");
        assert_eq!(r.jq_ghost(), Some("\"my key\""));
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_ghost(), Some("th"));
        bar_key(&mut r, shift(KeyCode::BackTab));
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".\"my key\"");
        assert_eq!(r.view().unwrap().view_text(), "1");
        assert_eq!(r.jq_bar().input.cursor(), 9, "caret after the closing quote");
    }

    #[test]
    fn accepting_a_builtin_leaves_the_caret_inside_its_parens() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.items[] | sel");
        assert_eq!(r.jq_ghost(), Some("ect("));
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".data.items[] | select(");
        assert!(r.jq_bar().error.is_some(), "an unfinished filter is an error, as when typed");
    }

    #[test]
    fn without_a_ghost_tab_and_right_behave_as_before() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.zz");
        assert_eq!(r.jq_ghost(), None);
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.zz");
        r.jq_bar_mut().input.set_cursor(2);
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_bar().input.cursor(), 3, "Right moves the caret mid-text");
        bar_key(&mut r, shift(KeyCode::Right));
        assert!(r.jq_bar().input.selection().is_some(), "shift+Right still selects");
    }

    /// Every keystroke in the bar is a new jq run, but most of them leave
    /// the tree on screen exactly as it was (a null result keeps the body
    /// tree up). The width cache must survive those: re-measuring every
    /// visible line of a big body costs a frame's worth of time each time.
    #[test]
    fn a_run_that_leaves_the_tree_unchanged_keeps_the_content_width_cache() {
        let mut r = ready(ITEMS);
        r.apply_jq(".data.nope", SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_bar().note, Some("null"));
        render(&mut r);
        let cached = r.view().unwrap().content_width;
        assert!(cached.is_some(), "the frame measured the width");
        r.apply_jq(".data.other", SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_bar().note, Some("null"));
        assert_eq!(
            r.view().unwrap().content_width,
            cached,
            "same tree on screen, so the measure is still good"
        );
        render(&mut r);
        assert_eq!(r.view().unwrap().content_width, cached);
    }

    #[test]
    fn all_null_reads_the_output_text_without_parsing_it() {
        assert!(all_null(&["null".into()]));
        assert!(all_null(&["null".into(), "null".into()]));
        assert!(all_null(&["[null,null]".into()]));
        assert!(all_null(&["[null, null]".into()]));
        assert!(!all_null(&["[]".into()]));
        assert!(!all_null(&["[null,1]".into()]));
        assert!(!all_null(&["[\"null\"]".into()]));
        assert!(!all_null(&["{\"a\":null}".into()]));
        assert!(!all_null(&["null".into(), "1".into()]));
        assert!(!all_null(&["[[null]]".into()]));
    }

    /// The horizontal scrollbar's content width is cached per (mode,
    /// visible line count) — but two different filtered trees can share
    /// both while their actual content widths differ wildly. The jq run
    /// counter in the cache key must tell them apart.
    #[test]
    fn a_new_jq_run_invalidates_the_content_width_cache_even_at_the_same_line_count() {
        let mut r = ready(ITEMS);
        r.apply_jq("\"short\"", SYNC_PRETTY_BYTES);
        assert_eq!(r.view().unwrap().visible_len(), 1);
        r.set_scroll_h(10_000);
        let short_scroll = r.view().unwrap().h_scroll;

        let long_literal = format!("\"{}\"", "x".repeat(200));
        r.apply_jq(&long_literal, SYNC_PRETTY_BYTES);
        assert_eq!(
            r.view().unwrap().visible_len(),
            1,
            "same line count as the short filter's output"
        );
        r.set_scroll_h(10_000);
        let long_scroll = r.view().unwrap().h_scroll;
        assert!(
            long_scroll > short_scroll,
            "the wider content must widen the scrollable range: {long_scroll} vs {short_scroll}"
        );
    }

    #[test]
    fn multiple_outputs_render_as_separate_documents_and_are_counted() {
        let mut r = ready(ITEMS);
        r.apply_jq(".data.items[] | .id", SYNC_PRETTY_BYTES);
        assert_eq!(
            r.view().unwrap().view_text(),
            "1\n2",
            "one after another, as jq prints"
        );
        assert_eq!(r.jq_output_count(), 2);
    }

    #[test]
    fn a_bad_filter_keeps_the_previous_tree_and_marks_the_bar_stale() {
        let mut r = ready(ITEMS);
        r.apply_jq(".data.items | length", SYNC_PRETTY_BYTES);
        r.apply_jq(".data.items | select(", SYNC_PRETTY_BYTES);
        assert_eq!(r.view().unwrap().view_text(), "2", "last good output stays");
        assert!(r.jq_bar().stale);
        let err = r.jq_bar().error.clone().expect("syntax error recorded");
        assert!(err.span().is_some());
        r.apply_jq(".data.items | length", SYNC_PRETTY_BYTES);
        assert!(!r.jq_bar().stale && r.jq_bar().error.is_none());
    }

    /// A superseded background run (an older run counter than the view's
    /// latest) still hands back a parsed document worth keeping: adopting
    /// it means the next `apply_jq` for this view finds a cached doc and
    /// doesn't ask the worker to re-parse the (possibly huge) body.
    #[test]
    fn a_superseded_runs_parsed_document_is_still_adopted() {
        let mut r = ready(ITEMS);
        let req1 = r
            .apply_jq(".data.items | length", 0)
            .expect("over the sync limit → background");
        assert!(req1.doc.is_none() && req1.body.is_some());
        let _req2 = r.apply_jq(".data.items[0].id", 0).unwrap();
        let doc = JqDocument::parse(ITEMS).unwrap();
        assert!(
            !r.attach_jq_result(
                req1.generation,
                req1.run,
                Ok(JqRunOutput::from_outputs(Some(doc), vec!["2".into()]))
            ),
            "superseded run is still dropped as a result"
        );
        let req3 = r
            .apply_jq(".data.items[1].id", 0)
            .expect("still over the sync limit");
        assert!(req3.doc.is_some(), "the superseded run's doc was adopted");
        assert!(req3.body.is_none(), "no need to re-parse the body");
    }

    #[test]
    fn a_runtime_error_is_reported_the_same_way_without_a_span() {
        let mut r = ready(ITEMS);
        r.apply_jq(".data.items[0].id | .x", SYNC_PRETTY_BYTES);
        let err = r.jq_bar().error.clone().expect("runtime error recorded");
        assert!(err.span().is_none());
        assert!(
            r.view().unwrap().view_text().starts_with("{"),
            "no good output yet: the body stays"
        );
    }

    /// The error span jaq reports is a byte range into `code.trim()`, not
    /// the untrimmed bar text — a leading space plus a multibyte char ahead
    /// of the span must not land the underline mid-character and panic.
    #[test]
    fn a_leading_space_and_multibyte_char_before_an_error_span_render_without_panicking() {
        let mut r = ready(ITEMS);
        let code = " é";
        r.set_jq_text(code);
        r.apply_jq(code, SYNC_PRETTY_BYTES);
        let err = r.jq_bar().error.clone().expect("syntax error recorded");
        assert!(err.span().is_some(), "trimmed code still has a span");
        // Must not panic despite the leading whitespace + multibyte char
        // ahead of the span in the untrimmed bar text.
        render(&mut r);
    }

    /// A stale error (and its span) from a filter typed against one
    /// response must not survive into a re-sent, non-JSON response: jq
    /// becomes unavailable, `apply_jq`'s early bail clears the error, and a
    /// shortened bar text renders with no error row and no panic.
    #[test]
    fn a_stale_error_is_cleared_when_the_body_becomes_non_json_and_the_bar_shrinks() {
        let mut r = ready(ITEMS);
        r.apply_jq(".a | select(", SYNC_PRETTY_BYTES);
        let err = r.jq_bar().error.clone().expect("syntax error recorded");
        assert!(err.span().is_some());
        // A re-send lands a non-JSON body: jq has nothing to run against.
        r.set_state(ResponseState::Ready(Box::new(data("plain text"))), 1);
        assert!(!r.jq_available());
        r.apply_jq(".a | select(", SYNC_PRETTY_BYTES);
        assert!(
            r.jq_bar().error.is_none(),
            "non-JSON response disables jq silently, no stale error"
        );
        assert!(!r.jq_bar().stale);
        assert!(r.jq_bar().pending.is_none());
        // Shorten the bar text directly (a backspace), bypassing focus and
        // `apply_jq`'s reconcile, and render — must not panic and no error
        // row should appear.
        r.jq_bar_mut().input = LineInput::new(".a | select");
        render(&mut r); // must not panic
        assert!(r.jq_bar().error.is_none(), "still no error row");
    }

    #[test]
    fn big_bodies_hand_the_run_to_the_caller_and_accept_only_the_latest_result() {
        let mut r = ready(ITEMS);
        let req = r
            .apply_jq(".data.items | length", 0)
            .expect("over the sync limit → background");
        assert_eq!(req.code, ".data.items | length");
        assert!(r.jq_bar().pending.is_some());
        let req2 = r.apply_jq(".data.items[0].id", 0).unwrap();
        assert!(
            req.doc.is_none() && req.body.is_some(),
            "no cached document yet: the worker parses the body"
        );
        assert!(
            !r.attach_jq_result(
                req.generation,
                req.run,
                Ok(JqRunOutput::from_outputs(None, vec!["2".into()]))
            ),
            "superseded run is dropped"
        );
        assert!(r.attach_jq_result(
            req2.generation,
            req2.run,
            Ok(JqRunOutput::from_outputs(None, vec!["1".into()]))
        ));
        assert_eq!(r.view().unwrap().view_text(), "1");
        assert!(r.jq_bar().pending.is_none());
        assert!(
            !r.attach_jq_result(
                req2.generation + 1,
                req2.run,
                Ok(JqRunOutput::from_outputs(None, vec!["9".into()]))
            ),
            "wrong generation is dropped"
        );
    }

    #[test]
    fn a_background_run_shows_a_spinner_only_past_the_grace_period() {
        let mut r = ready(ITEMS);
        r.open_jq();
        r.set_jq_text(".data");
        let req = r.apply_jq(".data", 0).expect("background run");
        let since = r.jq.pending_since;
        // The spinner takes over the `jq` chip at the bar's left edge;
        // both chips are three columns, so the text keeps its column.
        let spinner_at = |r: &mut Response, now| {
            let out = render_sized_at(r, 60, 20, now);
            let bar_row = buffer_rows(&out)
                .into_iter()
                .find(|row| row.contains(".data"))
                .expect("the jq bar row");
            let spins = SPINNER.iter().any(|g| bar_row.contains(*g));
            assert_ne!(
                spins,
                bar_row.contains("jq "),
                "spinner and chip swap: {bar_row}"
            );
            // A char column, not a byte offset: the braille glyph is 3 bytes.
            let col = bar_row.find(".data").map(|b| bar_row[..b].chars().count());
            (spins, col)
        };
        let (spins, col_before) = spinner_at(&mut r, since + Duration::from_millis(50));
        assert!(!spins, "inside the grace period the bar is unchanged");
        let (spins, col_during) = spinner_at(&mut r, since + JQ_SPINNER_AFTER);
        assert!(spins, "past it the bar spins");
        assert_eq!(col_before, col_during, "the filter text does not shift");
        assert!(
            r.attach_jq_result(
                req.generation,
                req.run,
                Ok(JqRunOutput::from_outputs(None, vec!["{}".into()]))
            ),
            "the run lands"
        );
        assert!(
            !spinner_at(&mut r, since + Duration::from_secs(5)).0,
            "and the spinner goes with it"
        );
    }

    #[test]
    fn a_parsed_body_stays_behind_the_spinner_until_a_switched_on_filter_lands() {
        let big = format!(
            r#"{{"pad": "{}", "data": {{"n": 1}}}}"#,
            "x".repeat(SYNC_PRETTY_BYTES)
        );
        let mut r = ready_gen(&big, 1);
        r.set_view_mode(ViewMode::Pretty);
        r.open_jq();
        r.set_jq_text(".data");
        assert!(r.attach_tree(1, JsonTree::parse(&big)));
        let out = render(&mut r);
        assert!(
            out.contains("filtering") && !out.contains("\"pad\""),
            "the full tree is held back, the wait is named: {out}"
        );
        let req = r.apply_jq(".data", 0).expect("background run");
        assert!(
            render(&mut r).contains("filtering"),
            "still held while the run is out"
        );
        assert!(r.attach_jq_result(
            req.generation,
            req.run,
            Ok(JqRunOutput::from_outputs(None, vec![r#"{"n": 1}"#.into()]))
        ));
        let out = render(&mut r);
        assert!(
            out.contains("\"n\"") && !out.contains("filtering"),
            "the filtered tree is what appears: {out}"
        );
        // A failing filter gives the full tree back with its error.
        let req = r.apply_jq(".data | error", 0).expect("background run");
        r.attach_jq_result(
            req.generation,
            req.run,
            Err(JqError::Runtime {
                message: "boom".into(),
            }),
        );
        assert!(!r.view().unwrap().awaiting_filter);
        // And a body parsed with the filter switched off shows at once.
        let mut r = ready_gen(&big, 2);
        r.set_view_mode(ViewMode::Pretty);
        assert!(r.attach_tree(2, JsonTree::parse(&big)));
        assert!(
            render(&mut r).contains("\"pad\""),
            "no filter: the tree shows"
        );
    }

    #[test]
    fn applying_the_same_code_twice_is_a_no_op() {
        let mut r = ready(ITEMS);
        r.apply_jq(".data", SYNC_PRETTY_BYTES);
        let before = r.view().unwrap().view_text();
        assert!(
            r.apply_jq(".data", 0).is_none(),
            "already applied: no background run either"
        );
        assert_eq!(r.view().unwrap().view_text(), before);
    }

    #[test]
    fn the_bar_text_is_settable_and_edits_are_flagged_until_taken() {
        let mut r = ready(ITEMS);
        r.set_jq_text(".a");
        assert_eq!(r.jq_text(), ".a");
        assert!(!r.take_jq_edited(), "programmatic set is not an edit");
        assert!(r.set_jq_focus(true));
        r.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(r.jq_text(), ".ab");
        assert!(r.take_jq_edited());
        assert!(!r.take_jq_edited());
        r.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(r.jq_text(), "", "Esc clears");
        assert!(r.jq_focused(), "…and keeps the caret in the bar");
        assert!(r.take_jq_edited(), "a clear is an edit");
        r.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!r.jq_focused(), "Esc on an empty bar blurs");
        assert!(!r.take_jq_edited(), "…which is not an edit");
        r.set_jq_text_with_cursor("map(select(.x == ))", 17);
        assert!(r.take_jq_edited(), "a tee-up counts as an edit");
        assert_eq!(r.jq_bar().input.cursor(), 17);
    }

    #[test]
    fn a_focused_bar_swallows_plain_keys_and_enter_blurs() {
        let mut r = ready(ITEMS);
        r.set_jq_focus(true);
        assert!(
            r.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
                .is_some()
        );
        assert_eq!(r.jq_text(), "j", "j typed, not cursor-down");
        r.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!r.jq_focused());
    }

    #[test]
    fn paste_lands_in_the_bar_only_while_it_is_focused() {
        let mut r = ready(ITEMS);
        assert!(!r.paste_into_jq(".x"));
        r.set_jq_focus(true);
        assert!(r.paste_into_jq(".x"));
        assert_eq!(r.jq_text(), ".x");
    }

    #[test]
    fn jq_is_unavailable_without_a_json_body() {
        let mut r = ready("plain text");
        assert!(!r.jq_available());
        assert!(!r.set_jq_focus(true));
        assert!(r.apply_jq(".a", SYNC_PRETTY_BYTES).is_none());
        assert!(
            r.jq_bar().error.is_none(),
            "nothing runs on a non-JSON body"
        );
    }

    #[test]
    fn the_tree_tab_wears_a_filter_badge_while_a_filter_is_applied() {
        let mut r = ready(ITEMS);
        let theme = Theme::dark();
        let (tabs, _) = response_tab_defs(r.view().unwrap(), r.jq_bar(), &theme);
        assert_eq!(tabs[0].1, None);
        r.apply_jq(".data", SYNC_PRETTY_BYTES);
        let (tabs, _) = response_tab_defs(r.view().unwrap(), r.jq_bar(), &theme);
        assert_eq!(tabs[0].1.map(|(c, _)| c), Some('\u{F0232}'));
    }

    #[test]
    fn applying_a_shrinking_filter_pulls_the_scroll_back_onto_the_new_content() {
        let items: Vec<String> = (0..50).map(|n| n.to_string()).collect();
        let body = format!(r#"{{"data":{{"items":[{}]}}}}"#, items.join(","));
        let mut r = ready(&body);
        // Scroll deep into the 50-item body and put the cursor even
        // deeper, mirroring the reviewer's repro (scroll=35, cursor=40)
        // against the default 10-row viewport height.
        r.click_row(40, false);
        assert_eq!(r.view().unwrap().cursor, 40);
        assert!(r.set_scroll(35));
        assert_eq!(r.view().unwrap().scroll, 35);
        // Same-module field access (private field, same crate module tree):
        // stand in for a horizontal scroll the wheel would otherwise need a
        // wide-enough drawn viewport to produce.
        r.view.as_mut().unwrap().h_scroll = 20;

        // A filter that collapses the whole body down to one short line —
        // far shorter than the old scroll position.
        r.apply_jq(".data.items | length", SYNC_PRETTY_BYTES);

        let view = r.view().unwrap();
        assert_eq!(view.visible_len(), 1, "filtered down to a single line");
        assert!(
            view.scroll <= view.cursor && view.cursor < view.scroll + view.height.max(1),
            "the cursor line is back on screen: scroll={} cursor={}",
            view.scroll,
            view.cursor
        );
        assert_eq!(view.h_scroll, 0, "horizontal scroll resets with the filter");
    }

    #[test]
    fn viewport_window_starts_at_the_first_char_touching_the_cropped_edge() {
        // Plain ASCII: skip 3 columns of a 10-char line, show 4.
        let (start, residual, slice) = viewport_window("abcdefghij", 3, 4, (0, 0, 0));
        assert_eq!((start, residual), (3, 0));
        assert!(
            slice.starts_with("defg"),
            "window begins at col 3: {slice:?}"
        );
        assert!(
            slice.len() < 10,
            "the window is a slice, not the whole line: {slice:?}"
        );
        // A wide char straddling the crop: the window starts on it and
        // owes `crop_cols` the column it already covered.
        let (start, residual, slice) = viewport_window("a日本b", 2, 4, (0, 0, 0));
        assert_eq!((start, residual), (1, 1));
        assert!(slice.starts_with('日'), "{slice:?}");
        // Nothing to skip: the whole width from the line start.
        let (start, residual, slice) = viewport_window("abc", 0, 10, (0, 0, 0));
        assert_eq!((start, residual, slice), (0, 0, "abc"));
        // Skipping past the end yields an empty window at the line's end.
        let (start, _, slice) = viewport_window("abc", 7, 4, (0, 0, 0));
        assert_eq!((start, slice), (3, ""));
        // A wide char after an ASCII run still owes its straddled column.
        let (start, residual, slice) = viewport_window("abcd日本e", 5, 3, (0, 0, 0));
        assert_eq!((start, residual), (4, 1));
        assert!(slice.starts_with('日'), "{slice:?}");
        // Skipping exactly the length: an empty window at the end.
        let (start, _, slice) = viewport_window("abc", 3, 4, (0, 0, 0));
        assert_eq!((start, slice), (3, ""));
    }

    #[test]
    fn a_window_resumed_from_a_column_mark_matches_the_walk_from_the_start() {
        // Mixed widths across several mark steps: narrow, wide, and
        // zero-width chars, so marks land inside and between wide chars.
        let text: String = "ab日\u{301}c".repeat(COL_MARK_STEP).chars().collect();
        let marks = col_marks(&text);
        assert!(marks.len() > 3, "several steps: {}", marks.len());
        assert_eq!(marks[0], (0, 0, 0), "mark 0 is the line start");
        for (k, &(_, _, col)) in marks.iter().enumerate() {
            assert!(
                col <= k * COL_MARK_STEP && col + 2 > k * COL_MARK_STEP,
                "mark {k} covers its column: {col}"
            );
        }
        for skip in [
            0,
            1,
            3,
            COL_MARK_STEP - 1,
            COL_MARK_STEP,
            2 * COL_MARK_STEP + 5,
            4000,
        ] {
            let from = mark_for(Some(&marks), skip);
            assert_eq!(
                viewport_window(&text, skip, 7, from),
                viewport_window(&text, skip, 7, (0, 0, 0)),
                "skip {skip}"
            );
        }
        // Past the end of the index: resume from the last mark, land on
        // the line's end.
        let far = text.chars().count() * 3;
        let from = mark_for(Some(&marks), far);
        assert_eq!(viewport_window(&text, far, 7, from).2, "");
        // A short line has no index and resumes from the start.
        assert_eq!(mark_for(None, 500), (0, 0, 0));
    }

    #[test]
    fn a_long_raw_line_gets_its_column_index_on_the_first_sideways_frame() {
        let mut r = ready(&format!("{}TAIL", "x".repeat(COL_MARK_STEP * 3)));
        render(&mut r);
        assert!(
            r.view().unwrap().raw_marks.is_empty(),
            "no index while unscrolled"
        );
        r.handle_scroll_h(COL_MARK_STEP as i16 * 2);
        let out = render(&mut r);
        let marks = &r.view().unwrap().raw_marks;
        assert_eq!(marks.len(), 1, "the one scrolled row is indexed");
        assert_eq!(marks[&0].len(), 4, "one mark per {COL_MARK_STEP} columns");
        assert!(out.contains("x"), "{out}");
        r.handle_scroll_h(10_000);
        assert!(render(&mut r).contains("TAIL"), "window reaches the end");
    }

    #[test]
    fn a_raw_search_match_far_into_a_windowed_line_still_highlights() {
        let body = format!("{}needle{}", "x".repeat(300), "y".repeat(300));
        let mut r = ready(&body);
        r.handle_key(ch('/'));
        for c in "needle".chars() {
            r.handle_key(ch(c));
        }
        r.handle_key(key(KeyCode::Enter));
        r.view.as_mut().unwrap().h_scroll = 295;
        let (area, buf) = render_buf(&mut r);
        let reversed = |x: u16| {
            buf.cell((x, area.y))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        };
        assert!(!reversed(area.x + 4), "col 299 is an x, not highlighted");
        assert!(reversed(area.x + 5), "col 300 starts the match");
        assert!(reversed(area.x + 10), "col 305 ends the match");
        assert!(!reversed(area.x + 11), "col 306 is a y");
    }

    #[test]
    fn a_selection_far_into_a_windowed_raw_line_paints_in_place() {
        let theme = Theme::dark();
        let mut r = ready(&"z".repeat(600));
        render_buf(&mut r);
        r.view.as_mut().unwrap().h_scroll = 298;
        let (area, _) = render_buf(&mut r);
        r.begin_selection_at(area.x + 2, area.y);
        r.drag_selection_to(area.x + 6, area.y);
        let (area, buf) = render_buf(&mut r);
        let bg = |x: u16| buf.cell((x, area.y)).unwrap().bg;
        assert_ne!(bg(area.x + 1), theme.selection, "col 299 is outside");
        assert_eq!(bg(area.x + 2), theme.selection, "col 300 is the anchor");
        assert_eq!(bg(area.x + 6), theme.selection, "col 304 is the head");
        assert_ne!(bg(area.x + 7), theme.selection, "col 305 is outside");
        assert_eq!(
            r.selected_text().as_deref(),
            Some("zzzzz"),
            "the copied text is the five selected cells"
        );
    }
}
