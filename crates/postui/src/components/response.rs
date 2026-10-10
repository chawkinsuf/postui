use super::json_tree::{JsonTree, TokenKind};
use super::line_input::LineInput;
use super::{Component, DrawCtx, pane_surface};
use crate::action::{Action, Place, ResponseMotion};
use crate::config::JqTab;
use crate::hit::ScrollbarSpec;
use crate::layout::PaneId;
use crate::theme::Theme;
use postui_core::jq::complete::{self, Candidate, Context, Kind};
use postui_core::jq::{JqDocument, JqError};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
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

/// The inline limit the app hands `apply_jq` / `refresh_jq_completion`:
/// zero, so every jq run and every completion key fetch goes to the
/// blocking pool whatever the body's size. A filter's cost is a property
/// of the filter, not the body — `[range(1e8)]` or `last(range(1e12))`
/// over a 1 KB response would freeze a synchronous UI thread with no Esc
/// to reach for. (Component tests pass `SYNC_PRETTY_BYTES` to exercise
/// the inline path directly.)
pub const JQ_SYNC_BYTES: usize = 0;

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

/// How the search box's text reads (plan M5a Deviation 3): Vim's pattern
/// in the vim profile, plain text in arrows. Both match case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSyntax {
    Literal,
    Vim,
}

impl SearchSyntax {
    pub fn of(profile: crate::config::KeyProfile) -> Self {
        match profile {
            crate::config::KeyProfile::Vim => SearchSyntax::Vim,
            crate::config::KeyProfile::Arrows => SearchSyntax::Literal,
        }
    }
}

/// The in-pane search: a live input while `active`, then a committed query
/// with its match list. Matches are `(full line, start, end)`, chars, end
/// exclusive, in the current view's *fully expanded* text, so a hit inside a
/// collapsed container is still found (and jumped to).
pub struct SearchState {
    pub input: LineInput,
    pub active: bool,
    /// What is searched for. A Vim pattern is kept as Vim's `do_search()`
    /// stores it (`as_stored`: `\?` typed in a `?` search is `?`), so a
    /// later search in the other direction reuses it unchanged.
    pub query: String,
    pub syntax: SearchSyntax,
    /// `?` and `#` search backward: Vim's `n` follows it, `N` goes the
    /// other way.
    pub backward: bool,
    pub matches: Vec<(usize, usize, usize)>,
    pub current: usize,
    /// Why `query` found nothing to search with: a Vim pattern outside the
    /// engine's subset. `n` says it again, as Vim's `n` repeats E54.
    pub error: Option<String>,
    /// While the box is open, the search it replaced: vim's Esc puts it
    /// back, as Vim's Esc abandons the `/` line and leaves the last search
    /// as it was. Its matches follow the view as this one's do.
    before: Option<Box<SearchState>>,
}

impl SearchState {
    /// The box, open for typing, over the search it replaces.
    fn open(syntax: SearchSyntax, backward: bool, before: Option<SearchState>) -> Self {
        let mut search = Self::with(LineInput::new(""), true, String::new(), syntax, backward);
        search.before = before.map(Box::new);
        search
    }

    /// A search for `query` that needs no typing (`*`, `n` after Esc),
    /// its matches still to compute.
    fn committed(query: String, syntax: SearchSyntax, backward: bool) -> Self {
        Self::with(LineInput::new(&query), false, query, syntax, backward)
    }

    fn with(input: LineInput, active: bool, query: String, syntax: SearchSyntax, backward: bool) -> Self {
        Self { input, active, query, syntax, backward, matches: Vec::new(), current: 0, error: None, before: None }
    }

    /// Fills `matches` (and `error`) for `query` over `corpus`, from the
    /// first. `Some(note)` for a Vim pattern outside the engine's subset:
    /// no matches then.
    fn find_matches(&mut self, corpus: &[String]) -> Option<String> {
        use crate::vim::engine::{compile, not_supported_note};
        self.matches.clear();
        self.current = 0;
        self.error = None;
        if self.query.is_empty() {
            return None;
        }
        match self.syntax {
            SearchSyntax::Literal => {
                for (i, line) in corpus.iter().enumerate() {
                    // Columns are chars, so they line up with rendered spans.
                    let (mut byte, mut col) = (0, 0);
                    for (at, hit) in line.match_indices(self.query.as_str()) {
                        col += line[byte..at].chars().count();
                        let len = hit.chars().count();
                        self.matches.push((i, col, col + len));
                        (byte, col) = (at + hit.len(), col + len);
                    }
                }
            }
            SearchSyntax::Vim => {
                let pat = match compile(&self.query) {
                    Ok(p) => p,
                    Err(atom) => {
                        let note = not_supported_note(&atom);
                        self.error = Some(note.clone());
                        return Some(note);
                    }
                };
                for (i, line) in corpus.iter().enumerate() {
                    let chars: Vec<char> = line.chars().collect();
                    self.matches.extend(pat.chain(&chars).into_iter().map(|(s, e)| (i, s, e)));
                }
            }
        }
        None
    }
}

/// Menu mode's candidate row: the text the completion was applied to,
/// the candidates as they stood, and which one is in. Each step rebuilds
/// the bar text from `base` so stepping never compounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JqMenu {
    base: String,
    items: Vec<Candidate>,
    /// The chip text per item: the whole token the completion yields
    /// (partial + ghost, or the rewritten token), not just the ghost.
    labels: Vec<String>,
    index: usize,
}

impl JqMenu {
    /// The chips, with the selected one flagged.
    pub fn chips(&self) -> impl Iterator<Item = (&str, bool)> {
        self.labels
            .iter()
            .enumerate()
            .map(move |(i, l)| (l.as_str(), i == self.index))
    }

    pub fn index(&self) -> usize {
        self.index
    }
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
    /// The candidate list is the closing bracket offered by
    /// [`Self::fall_back_to_closer`] rather than anything the context
    /// yielded, so the chip is labelled by its ghost alone: a closer
    /// does not continue the partial the way a key or builtin does.
    closer: bool,
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

    /// With nothing else on offer at the caret, offers the closing
    /// bracket of the innermost unclosed opener (see
    /// [`complete::closer`]): a key or builtin that extends what is
    /// typed always wins, so this only fills the gap once the token is
    /// finished.
    fn fall_back_to_closer(&mut self, text: &str) {
        if !self.candidates.is_empty() {
            self.closer = false;
            return;
        }
        self.candidates = complete::closer(text).into_iter().collect();
        self.closer = !self.candidates.is_empty();
    }
}

/// The jq filter bar's own state: the input, focus, and the outcome of the
/// last filter applied. Lives on [`Response`], not [`ReadyView`] — the bar
/// (and whatever filter the user typed) survives a new response landing in
/// the same slot, exactly like the address bar survives a send.
pub struct JqBar {
    pub input: LineInput,
    pub focused: bool,
    /// Whether the filter is switched on. Closing the bar (the header
    /// button, `alt+shift+q`) switches it off and keeps the text; the
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
    /// Menu mode's candidate row once Tab has entered it (`jq_tab =
    /// "menu"`): the first Tab wrote a candidate into the bar and the row
    /// under it shows the selection, Tab and shift+Tab stepping through.
    /// Any other key closes it and keeps whatever is selected. Before
    /// that Tab the row is still on show (see [`JqBar::menu_row`]), built
    /// live from the completion with nothing selected — menu mode has no
    /// ghost, the row is its preview. `None` in cycle mode.
    menu: Option<JqMenu>,
    /// The candidate row's first visible column as last drawn, so the
    /// row scrolls like a list: it stays put while the selected chip is
    /// in view and slides — either way — only when the selection reaches
    /// an edge. Set by the draw (it needs the width), reset whenever the
    /// row is entered or left.
    menu_scroll: std::cell::Cell<u16>,
}

impl JqBar {
    /// Whether a background run has been outstanding long enough (as of
    /// `now`) for the bar's chip to spin: past `JQ_SPINNER_AFTER`, so a
    /// run that finishes sooner never flickers.
    fn running_long(&self, now: Instant) -> bool {
        self.pending.is_some()
            && now.saturating_duration_since(self.pending_since) >= JQ_SPINNER_AFTER
    }

    /// Gives the bar the caret. The line's own edit session starts with
    /// the first keystroke and ends on [`Self::blur`].
    fn focus(&mut self) {
        self.focused = true;
    }

    /// Takes the caret away. Whatever was typed stands (the filter is
    /// live already) and the edit session ends: the app history records
    /// the close as one step.
    fn blur(&mut self) {
        self.focused = false;
        self.menu = None;
        self.input.end_edit();
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
        self.input.selection().is_none() && self.input.cursor() == self.input.text().chars().count()
    }

    /// Whether the completion has something to offer at the caret, in
    /// either mode: focused, no AI request pending, caret at the end,
    /// candidates present.
    fn offering(&self) -> bool {
        self.focused
            && !self.ai_pending
            && self.caret_at_end()
            && !self.completion.candidates.is_empty()
    }

    /// The candidate the ghost shows, when one is showing. Menu mode has
    /// no ghost for a guess — its row is the preview — but a closer is
    /// not a guess: there is exactly one bracket that can close the
    /// innermost opener, so it ghosts in both modes.
    fn candidate(&self) -> Option<&Candidate> {
        if !self.offering() || (self.tab == JqTab::Menu && !self.completion.closer) {
            return None;
        }
        self.completion.candidates.get(self.completion.index)
    }

    /// Menu mode's candidate row once Tab has entered it.
    pub fn menu(&self) -> Option<&JqMenu> {
        self.menu.as_ref()
    }

    /// The chip text for the completion's candidates as they stand: the
    /// whole token each (partial + ghost, or the rewritten token).
    fn candidate_labels(&self) -> Vec<String> {
        let partial = self
            .completion
            .ctx
            .as_ref()
            .map(|c| c.partial.as_str())
            .unwrap_or("");
        self.completion
            .candidates
            .iter()
            .map(|c| {
                // A closer does not continue the partial — `sort_by(.id`
                // offers `)`, not `id)` — so it is its own whole chip.
                if c.replace_from.is_some() || self.completion.closer {
                    c.ghost.clone()
                } else {
                    format!("{partial}{}", c.ghost)
                }
            })
            .collect()
    }

    /// Menu mode's row as drawn: `(label, selected)` per chip. With Tab
    /// in the row, the entered row's chips and selection; before that,
    /// the live candidates for what is being typed with nothing
    /// selected. `None` in cycle mode, or with nothing to offer. A
    /// closer is a ghost in menu mode too, never a row.
    pub fn menu_row(&self) -> Option<Vec<(String, bool)>> {
        if self.tab != JqTab::Menu {
            return None;
        }
        if let Some(menu) = &self.menu {
            return Some(menu.chips().map(|(l, sel)| (l.to_string(), sel)).collect());
        }
        if !self.offering() || self.completion.closer {
            return None;
        }
        Some(
            self.candidate_labels()
                .into_iter()
                .map(|l| (l, false))
                .collect(),
        )
    }

    /// Writes `cand` into `input` as accepting it does: the token is
    /// rewritten from `replace_from` when set, else the insert is typed
    /// at the caret.
    fn apply_candidate(input: &mut LineInput, cand: &Candidate) {
        if let Some(from) = cand.replace_from {
            let from_chars = input.text()[..from].chars().count();
            input.set_cursor(from_chars);
            input.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT));
            input.paste(&cand.insert);
        } else {
            input.insert_str(&cand.insert);
        }
    }

    /// Menu mode's Tab on the offered row: writes the candidate at
    /// `index` into the bar and, with more than one on offer, enters the
    /// row so Tab/shift+Tab can step through the rest. A lone candidate
    /// is simply accepted.
    fn open_menu(&mut self, index: usize) {
        if !self.offering() {
            return;
        }
        let items = self.completion.candidates.clone();
        let labels = self.candidate_labels();
        let index = index.min(items.len() - 1);
        let base = self.input.text().to_string();
        Self::apply_candidate(&mut self.input, &items[index]);
        self.edited = true;
        if items.len() < 2 {
            return;
        }
        self.menu_scroll.set(0);
        self.menu = Some(JqMenu {
            base,
            items,
            labels,
            index,
        });
    }

    /// The jq-aware word-delete target for ctrl/alt+backspace: inside a
    /// path token (one holding a `.`), the char index just after the
    /// last `.` before the caret — `.data.items` → `.data.`, and a
    /// trailing dot goes with the segment before it, `.data.` → `.` →
    /// nothing — so repeated presses climb the path one segment at a
    /// time and each stop is a place the completion has keys for. A
    /// quoted key is one segment whatever it holds (`."a.b c"` → `.`),
    /// and a subscript belongs to its segment (`.items[0]` → `.`).
    /// `None` outside a path token (a builtin, a pipe): the input's own
    /// word rule applies. Tokens are cut at whitespace and at jq's
    /// own separators, none of which count inside quotes.
    fn segment_delete_target(&self) -> Option<usize> {
        let chars: Vec<char> = self.input.text().chars().collect();
        // Whitespace right behind the caret goes with the delete, as the
        // input's own word rule has it; the segment rule looks past it.
        let mut end = self.input.cursor().min(chars.len());
        while end > 0 && chars[end - 1].is_whitespace() {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        let is_boundary = |c: char| c.is_whitespace() || matches!(c, '|' | '(' | ')' | ',' | ';');
        // Nothing inside a quoted key (`."my key"`, `."a|b.c"`) is a cut
        // or a segment: track the quote state up to each char (a `\"`
        // inside the quotes does not close them).
        let mut quoted = Vec::with_capacity(end);
        let mut inside = false;
        let mut escaped = false;
        for &c in &chars[..end] {
            quoted.push(inside);
            if escaped {
                escaped = false;
            } else if inside && c == '\\' {
                escaped = true;
            } else if c == '"' {
                inside = !inside;
            }
        }
        let start = (0..end)
            .rev()
            .find(|&i| !quoted[i] && is_boundary(chars[i]))
            .map_or(0, |i| i + 1);
        let is_dot = |i: usize| !quoted[i] && chars[i] == '.';
        if !(start..end).any(is_dot) {
            return None;
        }
        let e = if is_dot(end - 1) { end - 1 } else { end };
        Some(
            (start..e)
                .rev()
                .find(|&i| is_dot(i))
                .map_or(start, |i| i + 1),
        )
    }

    /// Leaves the entered row with nothing picked: the bar text goes back
    /// to the row's base (what was typed before Tab), the row closing and
    /// then showing again, unentered, for that text.
    fn unpick_menu(&mut self) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        self.menu_scroll.set(0);
        self.input = LineInput::new(&menu.base);
        self.edited = true;
    }

    /// Steps the open row (wrapping) and rewrites the bar text from the
    /// row's base with the newly selected candidate.
    fn step_menu(&mut self, forward: bool) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let n = menu.items.len();
        menu.index = if forward {
            (menu.index + 1) % n
        } else {
            (menu.index + n - 1) % n
        };
        let mut input = LineInput::new(&menu.base);
        Self::apply_candidate(&mut input, &menu.items[menu.index]);
        self.input = input;
        self.edited = true;
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
            tab: JqTab::Menu,
            menu: None,
            menu_scroll: std::cell::Cell::new(0),
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

/// A completion key fetch for the blocking pool: run `input_expr` against
/// `doc` (`complete::keys_at`) and hand the keys back as
/// `Action::JqCompleteFinished`. As with [`JqRunRequest`], `body` is the
/// raw text to parse first when no document is cached yet; the parse
/// comes back with the keys so the view can keep it.
pub struct JqCompleteRequest {
    pub generation: u64,
    pub seq: u64,
    pub input_expr: String,
    pub doc: Option<JqDocument>,
    pub body: Option<String>,
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
    /// The Headers tab's lines, one per `header_rows` entry: the response
    /// headers, the `sent` divider, then the headers the request went out
    /// with (secrets masked — the real values live in `ResponseData`).
    header_lines: Vec<String>,
    header_rows: Vec<HeaderRow>,
    pub cursor: usize,
    pub scroll: usize,
    /// Column offset of the body viewport — verbatim lines are never
    /// wrapped, so lines wider than the pane scroll horizontally instead.
    pub h_scroll: usize,
    pub search: Option<SearchState>,
    /// Height of the body viewport as of the last draw, so key handling can
    /// keep the cursor on screen. A sane guess until the first frame.
    height: usize,
    /// Vim's window option `'scroll'` (plan M5a Task 3): the lines ctrl+d
    /// and ctrl+u move, set by a counted one, reset when the viewport's
    /// height changes, as Vim's `win_comp_scroll()` does (Deviation 19).
    scroll_opt: Option<usize>,
    /// The full tree line the cursor is really on while a closed fold hides
    /// it (plan M5a Deviation 8): the cursor shows on the fold's line, and
    /// opening the fold, a search or `*` go from this one. Any cursor move
    /// forgets it ([`ReadyView::cursor_moved`]), and so does a new tree or
    /// view.
    fold_line: Option<usize>,
    /// The column Vim's cursor would have (plan M5a Deviation 7): a search
    /// that lands sets `(line, column)` of its match, in search-corpus
    /// lines; any cursor move forgets it ([`ReadyView::cursor_moved`]).
    at_col: Option<(usize, usize)>,
    /// A note for the app to toast (Vim's message line): a wrap, a miss.
    note: Option<String>,
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
    /// While the selection is whole lines (vim's `V`, shift+↑/↓: the
    /// response's V surface, plan M5a Deviation 11), the buffer line it
    /// started on, so a fold a motion opens above it never moves it. A
    /// drag's character selection has none.
    line_anchor: Option<usize>,
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

/// What a line of the Headers tab is: a header the server answered with
/// (index into `ResponseData::headers`), one the request went out with
/// (index into `ResponseData::sent_headers`), the divider between the
/// two sections, or a note where a section has nothing to list. Only
/// the two header kinds carry a copy pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderRow {
    Received(usize),
    Sent(usize),
    Divider,
    Note,
}

/// The Headers tab's divider text; the paint extends its rule to the
/// pane's width.
const SENT_DIVIDER: &str = "\u{2500}\u{2500} sent ";

/// `name: value` rows with the values aligned past the widest name.
fn aligned_header_lines<'a>(
    rows: impl Iterator<Item = (&'a str, &'a str)> + Clone,
) -> Vec<String> {
    let width = rows
        .clone()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    rows.map(|(k, v)| format!("{:<width$} {v}", format!("{k}:"), width = width + 1))
        .collect()
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
        let mut header_lines = Vec::new();
        let mut header_rows = Vec::new();
        if data.headers.is_empty() {
            header_lines.push("(no headers)".to_string());
            header_rows.push(HeaderRow::Note);
        }
        header_lines.extend(aligned_header_lines(
            data.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        ));
        header_rows.extend((0..data.headers.len()).map(HeaderRow::Received));
        header_lines.push(SENT_DIVIDER.to_string());
        header_rows.push(HeaderRow::Divider);
        if data.sent_headers.is_empty() {
            header_lines.push("(none)".to_string());
            header_rows.push(HeaderRow::Note);
        }
        header_lines.extend(aligned_header_lines(
            data.sent_headers
                .iter()
                .map(|h| (h.name.as_str(), h.display.as_str())),
        ));
        header_rows.extend((0..data.sent_headers.len()).map(HeaderRow::Sent));
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
            header_lines,
            header_rows,
            cursor: 0,
            scroll: 0,
            h_scroll: 0,
            search: None,
            height: 10,
            scroll_opt: None,
            fold_line: None,
            at_col: None,
            note: None,
            width: 40,
            content_width: None,
            last_area: None,
            sel_anchor: None,
            sel: None,
            sel_word_anchor: None,
            line_anchor: None,
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

    /// The views on the tab strip, in on-screen order: Pretty (when there
    /// is a tree to show), Raw, Headers. `response_tab_defs` draws them.
    fn tab_modes(&self) -> Vec<ViewMode> {
        [ViewMode::Pretty, ViewMode::Raw, ViewMode::Headers]
            .into_iter()
            .filter(|m| *m != ViewMode::Pretty || self.has_tree_view())
            .collect()
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
        self.cursor_moved();
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

    /// Vim's box keeps the search it replaces for its Esc; a box opened
    /// over an open one keeps what that one replaced.
    fn open_search(&mut self, syntax: SearchSyntax, backward: bool) {
        let before = match self.search.take() {
            Some(open) if open.active => open.before.map(|b| *b),
            shown => shown,
        };
        let before = before.filter(|_| syntax == SearchSyntax::Vim);
        self.search = Some(SearchState::open(syntax, backward, before));
    }

    /// The text of search-corpus line `line` (the expanded tree line on
    /// Pretty), without building the whole corpus.
    fn buffer_line(&self, line: usize) -> Option<String> {
        match self.mode {
            ViewMode::Pretty => {
                let tree = self.active_tree()?;
                (line < tree.line_count()).then(|| tree.line(line).expanded_text())
            }
            ViewMode::Raw => self.raw_lines.get(line).cloned(),
            ViewMode::Headers => self.header_lines.get(line).cloned(),
        }
    }

    /// How many lines the current view shows right now (collapse included).
    pub fn visible_len(&self) -> usize {
        match self.mode {
            ViewMode::Pretty => self.active_tree().map_or(0, |t| t.visible_len()),
            ViewMode::Raw => self.raw_lines.len(),
            ViewMode::Headers => self.header_lines.len(),
        }
    }

    /// What line `i` of the Headers tab is — how a copy on it finds the
    /// header it names.
    pub fn header_row(&self, i: usize) -> Option<HeaderRow> {
        self.header_rows.get(i).copied()
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
        if let Some(w) = self.measured_width() {
            return w;
        }
        let key = (self.mode, self.visible_len(), self.tree_epoch);
        let w = match self.mode {
            ViewMode::Pretty => self.active_tree().map_or(0, JsonTree::visible_width),
            ViewMode::Raw => self.raw_lines.iter().map(|l| l.width()).max().unwrap_or(0),
            // + 3 for the ` 󰆏 ` copy pill appended to each rendered row.
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

    /// The body viewport's height as of the last draw, in rows.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The widest visible line as the last draw measured it, or `None`
    /// when the view changed since (or never drew): a read that never pays
    /// for a measure, for the key router's repeat cap.
    pub fn measured_width(&self) -> Option<usize> {
        let key = (self.mode, self.visible_len(), self.tree_epoch);
        self.content_width
            .and_then(|(mode, len, run, w)| ((mode, len, run) == key).then_some(w))
    }

    /// The cached widest-visible-line measure without recomputing — 0 until
    /// a draw has run, which is fine for the drag math that reads it: the
    /// horizontal bar it serves only exists on drawn frames.
    fn cached_content_width(&self) -> usize {
        self.content_width.map(|(_, _, _, w)| w).unwrap_or(0)
    }

    /// The current view's text with nothing hidden — the corpus search runs
    /// over, and the coordinate space its match positions live in.
    fn search_corpus(&self) -> std::borrow::Cow<'_, [String]> {
        use std::borrow::Cow;
        match self.mode {
            ViewMode::Pretty => Cow::Owned(self.active_tree().map(|t| t.full_text_lines()).unwrap_or_default()),
            ViewMode::Raw => Cow::Borrowed(&self.raw_lines),
            ViewMode::Headers => Cow::Borrowed(&self.header_lines),
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

    /// The window Vim's `move.c` port runs on (plan M5a Task 3).
    fn window(&self) -> crate::vim::engine::View {
        crate::vim::engine::View { height: self.height.max(1) }
    }

    /// Keeps the cursor inside the viewport after a move: Vim's
    /// `update_topline()`, so a near move scrolls just enough and a far
    /// jump (a search, `50G`) puts its line mid-window (plan M5a
    /// Deviation 4).
    fn follow_cursor(&mut self) {
        if self.visible_len() == 0 {
            self.scroll = 0;
            return;
        }
        self.window().update_topline(self);
    }

    /// The line the cursor is really on: on the tree view, the full line,
    /// which a closed fold may hide (Deviation 8); elsewhere, the cursor.
    pub fn real_line(&self) -> Option<usize> {
        match self.mode {
            ViewMode::Pretty => self.fold_line.or_else(|| self.active_tree()?.full_index_of_visible(self.cursor)),
            _ => (self.cursor < self.visible_len()).then_some(self.cursor),
        }
    }

    /// `zo` `zc` `za` `zR` `zM` (spec §5.7; plan M5a Deviations 8 and 9):
    /// on the tree view only. The cursor shows on the line that now holds
    /// its real line and remembers that line while it is hidden. Whether
    /// anything changed.
    fn fold(&mut self, op: crate::action::FoldOp, count: usize) -> bool {
        if self.mode != ViewMode::Pretty {
            return false;
        }
        let Some(real) = self.real_line() else {
            return false;
        };
        let Some(tree) = self.active_tree_mut() else {
            return false;
        };
        if !tree.fold_at(real, op, count) {
            return false;
        }
        let shown = tree.holder(real).unwrap_or(0);
        let hidden = tree.visible_index_of(real).is_none();
        self.clear_sel();
        self.cursor = shown;
        self.fold_line = hidden.then_some(real);
        self.follow_cursor();
        true
    }

    /// The cursor moved (a key, a click, a pull to the view's edge, a new
    /// tree or view): it is no longer on the hidden line a closed fold
    /// remembered (plan M5a Deviation 8), nor on the column a search landed
    /// on (Deviation 7).
    fn cursor_moved(&mut self) {
        self.fold_line = None;
        self.at_col = None;
    }

    /// After a fold opens or closes (a click, Space, Enter): the change
    /// renumbers the visible lines a selection is addressed in, and may
    /// leave the cursor past the end or out of view. They act on the visible
    /// opener, so its line is the cursor's.
    fn after_fold(&mut self) {
        self.cursor_moved();
        self.clear_sel();
        self.clamp_cursor();
        self.follow_cursor();
    }

    /// The one place a key puts the cursor on a line: clamped, a line
    /// selection dropped (every cursor move but an extend drops one, or the
    /// next shift+↓ would grow it from a stale anchor). The view does not
    /// follow here; [`Self::jump_cursor`] does.
    fn land(&mut self, at: usize) {
        self.clear_sel();
        self.cursor_moved();
        self.cursor = at;
        self.clamp_cursor();
    }

    /// Lands on buffer line `line` (clamped to the last), as Vim's `{N}G`
    /// counts lines (final review I1; probes G1–G8). On the tree view a
    /// closed fold that hides it shows the cursor on its row and remembers
    /// the line (Deviation 8); elsewhere a line is a row. The view does not
    /// follow here.
    fn land_line(&mut self, line: usize) {
        let tree = match self.mode {
            ViewMode::Pretty => self.active_tree().filter(|t| t.line_count() > 0),
            _ => None,
        };
        let Some((real, shown, hidden)) = tree.map(|t| {
            let real = line.min(t.line_count() - 1);
            (real, t.holder(real).unwrap_or(0), t.visible_index_of(real).is_none())
        }) else {
            self.land(line);
            return;
        };
        self.land(shown);
        self.fold_line = hidden.then_some(real);
    }

    /// `delta` lines, clamped, the view following (Vim's `j`/`k`).
    fn move_cursor(&mut self, delta: i32) {
        self.motion(ResponseMotion::Lines(i64::from(delta)));
    }

    /// Lands the cursor on a line (a document jump, a search match), the
    /// view following.
    fn jump_cursor(&mut self, at: usize) {
        self.land(at);
        self.follow_cursor();
    }

    /// The cursor, or the nearest line the view shows when the wheel or the
    /// scrollbar left it off screen: where a window command starts, as Vim's
    /// cursor never leaves the window (plan M4a Deviation 10, plan M5a
    /// Deviation 12).
    fn shown_cursor(&self) -> usize {
        let height = self.height.max(1);
        let last = self.visible_len().saturating_sub(1);
        self.cursor.clamp(self.scroll.min(last), (self.scroll + height - 1).min(last))
    }

    /// Puts the cursor on `row`, the view's edge a window command pulled an
    /// off-screen cursor to: a cursor that moves forgets its hidden line.
    fn pull_cursor(&mut self, row: usize) {
        if row != self.cursor {
            self.cursor = row;
            self.cursor_moved();
        }
    }

    /// What a motion can change: when none of it did, the motion beeped and
    /// the frame needs no redraw.
    #[allow(clippy::type_complexity)]
    fn motion_mark(
        &self,
    ) -> (usize, usize, usize, Option<usize>, Option<(usize, usize)>, Option<((usize, usize), (usize, usize))>, Option<usize>, usize)
    {
        (self.cursor, self.scroll, self.h_scroll, self.fold_line, self.at_col, self.sel, self.line_anchor, self.visible_len())
    }

    /// A vim motion or window command (plan M5a Task 3), as Vim 9.1 runs
    /// it on a window of the visible lines. A window key that beeps at an
    /// edge changes nothing, a selection included (amendment A3.2).
    fn motion(&mut self, m: ResponseMotion) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        let w = self.window();
        match m {
            ResponseMotion::Lines(n) => {
                let at = (self.cursor as i64).saturating_add(n).clamp(0, len as i64 - 1) as usize;
                if at == self.cursor && self.fold_line.is_none() {
                    // Vim's `j`/`k` beeps at the edge and the cursor keeps
                    // its column (final review T5 m4). A selection still
                    // drops, as on main; a cursor hidden in a fold still
                    // moves to the row (its real line changes).
                    self.clear_sel();
                    self.follow_cursor();
                } else {
                    self.jump_cursor(at);
                }
            }
            ResponseMotion::Goto { line, last } => {
                let at = match line {
                    Some(n) => n.saturating_sub(1),
                    None if last => usize::MAX,
                    None => 0,
                };
                self.land_line(at);
                self.follow_cursor();
            }
            ResponseMotion::HalfPage { down, count } => {
                let shown = self.shown_cursor();
                // `nv_halfpage()`: at the edge it beeps before `'scroll'` is set.
                if (down && shown + 1 >= len) || (!down && shown == 0) {
                    return;
                }
                self.clear_sel();
                self.pull_cursor(shown);
                let mut scroll = self.scroll_opt;
                w.halfpage(self, down, count.unwrap_or(0), &mut scroll);
                self.scroll_opt = scroll;
            }
            ResponseMotion::Page { down, count } => {
                let before = (self.cursor, self.fold_line, self.at_col);
                self.pull_cursor(self.shown_cursor());
                let shown = (self.scroll, self.cursor);
                let moved = w.onepage(self, down, count.max(1));
                if !moved && (self.scroll, self.cursor) == shown {
                    // Beeped at once: nothing changed, the selection and
                    // the search column included (final review m3).
                    (self.cursor, self.fold_line, self.at_col) = before;
                } else {
                    self.clear_sel();
                }
            }
            ResponseMotion::Place { at, line } => {
                match line {
                    Some(n) => self.land_line(n.saturating_sub(1)),
                    None => self.pull_cursor(self.shown_cursor()),
                }
                match at {
                    Place::Top => w.scroll_cursor_top(self, 0, true),
                    Place::Middle => w.scroll_cursor_halfway(self, true, false),
                    Place::Bottom => w.scroll_cursor_bot(self, 0, true),
                }
            }
        }
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
        self.cursor_moved();
        self.cursor = 0;
        self.scroll = 0;
        self.h_scroll = 0;
        // Match positions are per-view coordinates; the query survives, the
        // positions do not.
        self.recompute_matches();
    }

    /// Every match of the query, in corpus order, and of the search an open
    /// box replaced. `Some(note)` for a Vim pattern outside the engine's
    /// subset: no matches then.
    fn recompute_matches(&mut self) -> Option<String> {
        let mut search = self.search.take()?;
        let wanted = |s: &SearchState| !s.query.is_empty();
        let corpus = if wanted(&search) || search.before.as_deref().is_some_and(wanted) {
            self.search_corpus()
        } else {
            std::borrow::Cow::Borrowed(&[][..])
        };
        let note = search.find_matches(&corpus);
        if let Some(before) = search.before.as_deref_mut() {
            before.find_matches(&corpus);
        }
        drop(corpus);
        self.search = Some(search);
        note
    }

    /// Where Vim's cursor is (plan M5a Deviation 7): the cursor's real line
    /// in corpus lines, and the column of the match last landed on there,
    /// else 0. `*` takes its word here; a search starts from
    /// [`Self::search_from`].
    fn search_pos(&self) -> (usize, usize) {
        let line = self.real_line().unwrap_or(0);
        let col = self.at_col.filter(|(l, _)| *l == line).map_or(0, |(_, c)| c);
        (line, col)
    }

    /// Where a search in `backward`'s direction starts: [`Self::search_pos`],
    /// except when a closed fold holds the cursor's line (the cursor hidden
    /// inside it, or on its own collapsed row). Then Vim's `do_search()`
    /// starts outside the outermost closed one: a forward search after its
    /// closing line, a backward one at its opening line's column 0, so the
    /// matches it hides are skipped (probes R1–R10, the controller's ruling
    /// over the plan's Review Focus 1).
    fn search_from(&self, backward: bool) -> (usize, usize) {
        let (line, col) = self.search_pos();
        let fold = match self.mode {
            ViewMode::Pretty => self.active_tree().and_then(|t| t.closed_fold(line)),
            _ => None,
        };
        match fold {
            Some((open, _)) if backward => (open, 0),
            Some((_, close)) => (close, usize::MAX),
            None => (line, col),
        }
    }

    /// Lands on match `current`, expanding whatever containers hide it, and
    /// remembers its column. A jump like any other: a line selection drops,
    /// except vim's V, which a search extends as Vim 9.1's Visual does
    /// (probed). Its anchor is a buffer line, so a fold
    /// the landing opens above it does not move it.
    fn jump_to_match(&mut self) {
        let Some(search) = &self.search else { return };
        let Some(&(line, start, _)) = search.matches.get(search.current) else {
            return;
        };
        let anchor = self.line_anchor.filter(|_| search.syntax == SearchSyntax::Vim);
        let at = match (self.mode, self.active_tree_mut()) {
            (ViewMode::Pretty, Some(tree)) => {
                tree.expand_ancestors(line);
                tree.visible_index_of(line).unwrap_or(0)
            }
            _ => line,
        };
        self.jump_cursor(at);
        if let Some(anchor) = anchor {
            self.select_lines_from(anchor);
        }
        self.at_col = Some((line, start));
    }

    /// The buffer line shown on visible row `row`: on the tree, the line
    /// the row is (a collapsed container's opening line); elsewhere a row is
    /// a line.
    fn row_to_line(&self, row: usize) -> usize {
        match (self.mode, self.active_tree()) {
            (ViewMode::Pretty, Some(tree)) => tree.full_index_of_visible(row).unwrap_or(row),
            _ => row,
        }
    }

    /// [`Self::row_to_line`] back: the row showing `line`, or the closed
    /// fold's row that hides it.
    fn line_to_row(&self, line: usize) -> usize {
        match (self.mode, self.active_tree()) {
            (ViewMode::Pretty, Some(tree)) => tree.holder(line).unwrap_or(0),
            _ => line,
        }
    }

    /// Vim's `searchit()` from the cursor ([`Self::search_from`]), `count`
    /// hits, wrapping (plan M5a Deviation 4): forward, the first match
    /// starting after the cursor (one starting on its line's end counts one
    /// column back, as the engine's `search` has it: amendment A5.3);
    /// backward, the last one starting before it. The note says a wrap or a
    /// miss, as Vim's message line does.
    fn land_match(&mut self, backward: bool, count: usize) -> Option<String> {
        use crate::vim::engine::{Dir, not_found_note, wrap_note};
        let (line, col) = self.search_from(backward);
        let search = self.search.as_ref()?;
        if let Some(error) = &search.error {
            return Some(error.clone());
        }
        let matches = &search.matches;
        if matches.is_empty() {
            return Some(not_found_note(&search.query));
        }
        // The first hit, then every match in turn, cyclically: hit k is the
        // first one stepped k − 1 times, so a 9 999 count costs nothing.
        let n = matches.len();
        let steps = count.max(1) - 1;
        let (idx, wrapped) = if backward {
            let before = matches.partition_point(|m| (m.0, m.1) < (line, col));
            let start = before.checked_sub(1);
            let from = start.unwrap_or(n - 1);
            ((from + n - steps % n) % n, start.is_none() || steps > from)
        } else {
            let at_end = |m: &(usize, usize, usize)| {
                m.1 > 0 && m.1 == m.2 && self.buffer_line(m.0).is_some_and(|t| t.chars().count() == m.1)
            };
            let after = matches.partition_point(|m| (m.0, if at_end(m) { m.1 - 1 } else { m.1 }) <= (line, col));
            let start = (after < n).then_some(after);
            let from = start.unwrap_or(0);
            ((from + steps) % n, start.is_none() || from + steps >= n)
        };
        self.search.as_mut()?.current = idx;
        self.jump_to_match();
        wrapped.then(|| wrap_note(if backward { Dir::Backward } else { Dir::Forward }).to_string())
    }

    /// Commits the box (Enter, Esc, the ▼/▲ click), then lands from the
    /// cursor in the box's direction. Vim's Esc abandons the box instead:
    /// the search it replaced comes back, the cursor and the last pattern
    /// stay (probed: `/bar` Enter, then `/foo` Esc leaves `@/` `bar`, the
    /// cursor and the highlight). Arrows' Esc on an empty box only closes
    /// it, as main ran nothing; on a typed one it runs the search, main's
    /// field rule. Otherwise an empty Vim box takes the last query, as
    /// Vim's `/` Enter does, and says Vim's E35 when there is none (probe
    /// E1); an empty arrows box finds nothing, as on main. A Vim pattern
    /// becomes the last one before it compiles, as Vim's `do_search()`
    /// stores it, so one outside the engine's subset is kept too and `n`
    /// says why again (probed: `/\(` then `n` repeats E54).
    fn commit_search(&mut self, esc: bool, last: &mut LastPattern) {
        use crate::vim::engine::{Dir, as_stored};
        let Some(search) = self.search.as_mut() else { return };
        let before = search.before.take();
        if esc && search.syntax == SearchSyntax::Vim {
            self.search = before.map(|b| *b);
            return;
        }
        search.active = false;
        search.input.end_edit();
        let typed = search.input.text().to_string();
        if esc && typed.is_empty() {
            search.query.clear();
            search.matches.clear();
            search.current = 0;
            return;
        }
        let vim = search.syntax == SearchSyntax::Vim;
        let dir = if search.backward { Dir::Backward } else { Dir::Forward };
        search.query = match (vim, typed.is_empty()) {
            (true, true) => last.pattern.as_ref().map(|(q, _)| q.clone()).unwrap_or_default(),
            (true, false) => as_stored(&typed, dir),
            (false, _) => typed,
        };
        let (query, backward) = (search.query.clone(), search.backward);
        if vim && !query.is_empty() {
            last.set(query.clone(), backward);
        }
        self.note = match self.recompute_matches() {
            Some(note) => Some(note),
            None if query.is_empty() => vim.then(|| crate::vim::engine::NO_PREVIOUS_PATTERN.to_string()),
            None => self.land_match(backward, 1),
        };
    }

    /// Makes vim's last pattern the search, in its direction: after Esc
    /// cleared the search, or once a search elsewhere replaced the pattern.
    /// `n` and `N` then go on as Vim's do.
    fn restore_search(&mut self, query: String, backward: bool) {
        self.search = Some(SearchState::committed(query, SearchSyntax::Vim, backward));
        // A pattern that does not compile keeps its note for `n` to say.
        self.recompute_matches();
    }

    /// Vim's `*` and `#` (plan M5a Task 5): the keyword under or after the
    /// cursor's column, else the non-blank string there, searched as a Vim
    /// pattern whatever the profile. Whether a search ran.
    fn star(&mut self, backward: bool, count: usize, last: &mut LastPattern) -> bool {
        use crate::vim::engine::{Dir, NO_STRING_UNDER_CURSOR, as_stored, find_ident, ident_pattern};
        let (line, col) = self.search_pos();
        let Some(text) = self.buffer_line(line) else {
            return false;
        };
        let chars: Vec<char> = text.chars().collect();
        let Some((start, end)) = find_ident(&chars, col) else {
            self.note = Some(NO_STRING_UNDER_CURSOR.into());
            return false;
        };
        let dir = if backward { Dir::Backward } else { Dir::Forward };
        let pat = as_stored(&ident_pattern(&chars, start, end, backward), dir);
        self.search = Some(SearchState::committed(pat.clone(), SearchSyntax::Vim, backward));
        last.set(pat, backward);
        if let Some(note) = self.recompute_matches() {
            self.note = Some(note);
            return false;
        }
        // Vim searches from the word's start, so the word itself is skipped.
        self.at_col = Some((line, start));
        self.note = self.land_match(backward, count);
        true
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
        let ranges = search
            .matches
            .iter()
            .filter(|(l, _, _)| *l == line)
            .map(|(_, s, e)| (*s, *e))
            .collect();
        let current = search
            .matches
            .get(search.current)
            .filter(|(l, _, _)| *l == line)
            .map(|(_, s, e)| (*s, *e));
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
        self.line_anchor = None;
    }

    /// The row the V anchor's buffer line shows on now.
    fn line_anchor_row(&self) -> Option<usize> {
        self.line_anchor.map(|line| self.line_to_row(line))
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

    /// A line selection from visible row `anchor` to the cursor's line.
    fn set_line_sel(&mut self, anchor: usize) {
        self.select_lines_from(self.row_to_line(anchor));
    }

    /// A line selection from buffer line `anchor` (the row showing it now)
    /// to the cursor's line, every line between covered in full.
    fn select_lines_from(&mut self, anchor: usize) {
        self.line_anchor = Some(anchor);
        let row = self.line_to_row(anchor);
        self.sel_anchor = Some((row, 0));
        let cl = self.cursor;
        let len = |v: &Self, l: usize| v.display_line_text(l).map_or(0, |t| t.chars().count());
        self.sel = Some(if cl >= row {
            ((row, 0), (cl, len(self, cl).saturating_sub(1)))
        } else {
            ((row, len(self, row).saturating_sub(1)), (cl, 0))
        });
    }

    /// A motion that keeps the selection's anchor (vim's V, shift+↑/↓):
    /// the cursor moves as `motion` moves it, then the lines from the
    /// anchor to it are selected. The anchor is the selection's, a drag's
    /// included, so shift+↑/↓ turns a drag into lines from the line it
    /// started on, as on main (amendment A6.1, the user's decision); with
    /// no selection, the cursor's line. A window key that pulls an
    /// off-screen cursor to the view's edge takes the moving end with it
    /// (amendment A6.5). The anchor is held by buffer line across the
    /// motion, so a fold the motion opens or closes never moves it.
    fn extend(&mut self, m: ResponseMotion) {
        let len = self.visible_len();
        if len == 0 {
            self.motion(m);
            return;
        }
        let anchor = self
            .line_anchor
            .unwrap_or_else(|| self.row_to_line(self.sel_anchor.map_or(self.cursor, |(l, _)| l).min(len - 1)));
        self.motion(m);
        if self.visible_len() > 0 {
            self.select_lines_from(anchor);
        }
    }

    /// shift+↑/↓: extend a line selection by `delta` lines.
    fn select_line_extend(&mut self, delta: i32) {
        self.extend(ResponseMotion::Lines(i64::from(delta)));
    }

    /// The buffer lines visible row `row` stands for: itself, or on the
    /// tree a closed container's every line, as Vim's fold yanks (probes
    /// Y1–Y4).
    fn buffer_lines_of(&self, row: usize) -> Vec<String> {
        match self.mode {
            ViewMode::Pretty => {
                let Some(tree) = self.active_tree() else { return Vec::new() };
                let Some(full) = tree.full_index_of_visible(row) else { return Vec::new() };
                let line = tree.line(full);
                let end = match (&line.container, line.collapsed) {
                    (Some(c), true) => c.end_line,
                    _ => full,
                };
                (full..=end).map(|i| tree.line(i).expanded_text()).collect()
            }
            ViewMode::Raw => self.raw_lines.get(row).cloned().into_iter().collect(),
            ViewMode::Headers => self.header_lines.get(row).cloned().into_iter().collect(),
        }
    }

    /// `yy` (`count` rows from the cursor, clamped) or a V `y` (the
    /// selected rows): the lines, as Vim's buffer holds them. The selection
    /// ends and the cursor goes to the first line yanked, as Vim leaves
    /// Visual (probes V1–V13). Only a `yy` from the cursor's own line keeps
    /// its column (probe Y5, in the Task 6 report); a V `y` puts it in
    /// column 0 even on the cursor's line (R6a, R6d), and a yank from a
    /// closed fold the cursor is hidden in puts it on the fold's first line
    /// (Y8, Y10). A `{N}yy` with N ≥ 2 on the last row yanks nothing, as
    /// Vim's `cursor_down()` fails there and the yank beeps (probed: `3yy`
    /// on the last line leaves `@"` alone; one line up it yanks two).
    fn yank_rows(&mut self, count: Option<usize>) -> Option<Vec<String>> {
        let len = self.visible_len();
        if len == 0 {
            return None;
        }
        let anchor = self.line_anchor_row();
        if anchor.is_none() && count.is_some_and(|n| n >= 2) && self.cursor + 1 >= len {
            return None;
        }
        let (lo, hi) = match anchor {
            Some(a) => (a.min(self.cursor), a.max(self.cursor)),
            None => (self.cursor, self.cursor.saturating_add(count.unwrap_or(1).max(1) - 1).min(len - 1)),
        };
        let lines = (lo..=hi).flat_map(|row| self.buffer_lines_of(row)).collect();
        if anchor.is_none() && lo == self.cursor && self.fold_line.is_none() {
            self.clear_sel();
        } else {
            self.jump_cursor(lo);
        }
        Some(lines)
    }

    /// `o` in V: the cursor and the anchor trade ends. A cursor that moves
    /// forgets its hidden line and column (amendment A6.4).
    fn swap_ends(&mut self) {
        let Some(anchor) = self.line_anchor_row() else { return };
        let cursor = self.cursor;
        if anchor != cursor {
            self.cursor_moved();
        }
        self.cursor = anchor;
        self.follow_cursor();
        self.set_line_sel(cursor);
    }
}

/// The response's lines as Vim's window code sees them (plan M5a Task 3):
/// the visible lines, `scroll` the top, `cursor` the caret's row. There is
/// no column (Deviation 7).
impl crate::vim::engine::Rows for ReadyView {
    fn lines(&self) -> usize {
        self.visible_len().max(1)
    }
    fn blank(&self) -> bool {
        self.visible_len() == 0
    }
    fn topline(&self) -> usize {
        self.scroll
    }
    fn set_topline(&mut self, row: usize) {
        self.scroll = row;
    }
    fn cur_row(&self) -> usize {
        self.cursor
    }
    fn set_cur_row(&mut self, row: usize, _col0: bool) {
        if row != self.cursor {
            self.cursor_moved();
        }
        self.cursor = row;
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
    /// Where the last frame drew the focused jq bar's caret — the cell
    /// the terminal's own cursor belongs in — or `None` when the bar was
    /// unfocused, closed, or not drawn. An output of `draw`, kept on the
    /// pane like the hit map is kept on the app: the frame reads it after
    /// the panes have drawn and places the cursor only when no modal
    /// covers the bar.
    jq_caret: Option<Position>,
    /// Vim's last pattern, which outlives a new response.
    last_search: LastPattern,
}

/// Vim's last pattern as the response sees it: the pattern (as stored,
/// see [`SearchState::query`]) and whether it went backward. Vim has one
/// for every window, so the app keeps it equal to the text engine's
/// ([`Response::take_last_search`], [`Response::adopt_last_search`] at
/// each input event's end). An empty `/` Enter searches it again, and so
/// does `n` once Esc has cleared the search: Esc never forgets it, as in
/// Vim.
#[derive(Default)]
struct LastPattern {
    pattern: Option<(String, bool)>,
    /// A search here set `pattern`: the app gives it to the engine.
    set: bool,
}

impl LastPattern {
    /// A search here made `query` the last pattern.
    fn set(&mut self, query: String, backward: bool) {
        self.pattern = Some((query, backward));
        self.set = true;
    }
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
        // The bar's text survives a new response, but nothing derived from
        // the old document does: the cached keys are for a body that is
        // gone, and an in-flight pool fetch will be dropped on arrival, so
        // forget it here or `refresh_jq_completion` would never re-issue
        // it. The sequence counter is kept so that stale fetch can't
        // collide with a fresh one's `seq` (as in `shed_derived`).
        self.jq.completion = JqCompletion {
            seq: self.jq.completion.seq,
            ..JqCompletion::default()
        };
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
                view.cursor_moved();
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
            // never appears for a small non-JSON body — and with it the
            // jq bar a filter typed against the spinner had focused.
            None => {
                view.set_mode(ViewMode::Raw);
                self.jq.blur();
            }
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

    /// The response's tab strip, in on-screen order (the tab keys, through
    /// `App::tab_target`), or `None` with no ready view.
    pub fn tab_modes(&self) -> Option<Vec<ViewMode>> {
        self.view.as_ref().map(ReadyView::tab_modes)
    }

    /// The jq bar's current text.
    pub fn jq_text(&self) -> &str {
        self.jq.input.text()
    }

    /// Sets the bar's text programmatically (the editor ↔ bar sync), cursor
    /// at the end. Not an edit — [`Response::take_jq_edited`] is unaffected.
    pub fn set_jq_text(&mut self, text: &str) {
        self.jq.menu = None;
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

    /// Sets the bar's text and cursor together (a tee-up from elsewhere —
    /// e.g. the AI describe flow seeding a filter). Counts as an edit, and
    /// switches a closed bar back on: a verb or the AI landing a filter
    /// means "show me this". The text goes in through `set_text`, not a
    /// fresh line, so a tee-up into a focused bar keeps the session's
    /// history and is undoable in the bar. Landing in an unfocused bar
    /// ends the session again: a verb or an AI reply that arrives while
    /// the bar is closed belongs to the app history alone, and leaving a
    /// step behind would let a later ctrl+z — after a bare focus, nothing
    /// typed — walk back an action the app history already owns.
    pub fn set_jq_text_with_cursor(&mut self, text: &str, cursor: usize) {
        self.jq.menu = None;
        self.jq.input.set_text(text);
        self.jq.input.set_cursor(cursor);
        if !self.jq.focused {
            self.jq.input.end_edit();
        }
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
    ///
    /// Only ever on the `Pretty` tab. jq filters the tree, so `Raw` and
    /// `Headers` show neither the bar nor the toolbar's filter icon; the
    /// filter itself is kept, and comes back with the tab.
    pub fn jq_open(&self) -> bool {
        self.jq.is_open() && self.on_jq_tab()
    }

    /// Whether the open tab is the one jq filters (`Pretty`).
    fn on_jq_tab(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.mode == ViewMode::Pretty)
    }

    /// Opens the bar: switches the filter on (an edit, when that changes
    /// anything) and focuses the input. `false` when jq has nothing to run
    /// against, in which case nothing changes.
    pub fn open_jq(&mut self) -> bool {
        if !self.jq_available() {
            return false;
        }
        self.jq.focus();
        if !self.jq.enabled {
            self.jq.enabled = true;
            self.jq.edited = true;
        }
        true
    }

    /// Closes the bar: blurs it and, when there is text to keep, switches
    /// the filter off (an edit — the request remembers). An empty bar just
    /// goes away; there is nothing to switch off.
    pub fn close_jq(&mut self) {
        self.jq.blur();
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

    /// Whether the jq bar has keystrokes of its own in flight — the bar is
    /// open and its line has an edit session with history. The app history
    /// waits while it does (see `App::field_gate`).
    pub fn jq_field_edited(&self) -> bool {
        self.jq.focused && self.jq.input.edited()
    }

    /// The vim engine typed into the jq bar (plan M3b): as a typed key
    /// does, it leaves an entered menu row and marks the bar edited, so
    /// `App::sync_jq` carries the text into the request.
    pub(crate) fn mark_jq_typed(&mut self) {
        self.leave_jq_menu();
        self.jq.edited = true;
    }

    /// The vim engine took a key in the jq bar: as any key but Tab,
    /// shift+Tab, Enter and Esc does in `ready_key`, it leaves an entered
    /// menu row keeping the selection, whether or not it edited.
    pub(crate) fn leave_jq_menu(&mut self) {
        self.jq.menu = None;
    }

    /// Whether plain Right or End accepts the jq bar's offered completion:
    /// `ready_key`'s offer arm, and in vim the claim that puts those keys
    /// before the engine (plan M3b Deviation 5).
    pub(crate) fn jq_accepts_completion(&self) -> bool {
        self.jq.focused
            && (self.jq.ghost().is_some() || self.jq.menu_row().is_some())
            && (self.jq.tab == JqTab::Cycle || self.jq.completion.closer)
    }

    /// Ends the bar's edit session in place — the caret stays, the text
    /// stays, only the keystroke history closes — so the app history can
    /// take the edit as a step before something replaces the request
    /// under it (`App::flush_field_session`).
    pub fn end_jq_edit_session(&mut self) {
        self.jq.input.end_edit();
    }

    /// Routes an undo (or `redo`) into the pane's open text field. The bar
    /// cannot be stepped through [`Self::open_text_field_mut`] alone: a
    /// bare `LineInput::undo` leaves `jq.edited` clear, and the reconcile
    /// at the end of the same `update` would write the editor's filter
    /// straight back over it. So the jq branch marks the edit exactly as a
    /// keystroke into the bar does. `false` when there was nothing to step.
    pub fn field_undo(&mut self, redo: bool) -> bool {
        if self.jq.focused {
            let stepped = if redo {
                self.jq.input.redo()
            } else {
                self.jq.input.undo()
            };
            if stepped {
                self.jq.edited = true;
            }
            return stepped;
        }
        match self.open_text_field_mut() {
            Some(f) => {
                if redo {
                    f.redo()
                } else {
                    f.undo()
                }
            }
            None => false,
        }
    }

    /// The pane's open text field, if any: the jq bar while it has the
    /// caret, else the search box while its input is live. What the app's
    /// undo routing acts on.
    pub fn open_text_field_mut(&mut self) -> Option<&mut LineInput> {
        if self.jq.focused {
            return Some(&mut self.jq.input);
        }
        let view = self.view.as_mut()?;
        let search = view.search.as_mut()?;
        search.active.then_some(&mut search.input)
    }

    /// Whether [`Self::open_text_field_mut`] would hand one back — the
    /// immutable twin, for `App::field_open` and the footer.
    pub fn field_open(&self) -> bool {
        self.jq.focused
            || self
                .view
                .as_ref()
                .and_then(|v| v.search.as_ref())
                .is_some_and(|s| s.active)
    }

    /// Focuses (or blurs) the jq bar. Returns whether it took: focusing
    /// fails with no ready view or a body jq can't run against.
    pub fn set_jq_focus(&mut self, focused: bool) -> bool {
        if focused && !self.jq_available() {
            return false;
        }
        if focused {
            self.jq.focus();
        } else {
            self.jq.blur();
        }
        true
    }

    pub fn jq_bar(&self) -> &JqBar {
        &self.jq
    }

    pub fn jq_bar_mut(&mut self) -> &mut JqBar {
        &mut self.jq
    }

    /// The cell the last `draw` left the focused jq bar's caret in, for
    /// the frame to place the terminal cursor on. See `jq_caret`.
    pub fn jq_caret_cell(&self) -> Option<Position> {
        self.jq_caret
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

    /// Whether jq can run right now: a ready view with a parsed (or
    /// still-parsing) JSON body, showing the tab jq filters.
    pub fn jq_available(&self) -> bool {
        self.jq_blocked_reason().is_none()
    }

    /// Why jq can't run right now, in the words the keyboard routes toast
    /// — `None` while it can. The tab rule gets its own line: on `Raw` or
    /// `Headers` the response may well be JSON, it just isn't the view a
    /// filter applies to.
    pub fn jq_blocked_reason(&self) -> Option<&'static str> {
        match self.view.as_ref() {
            Some(v) if !v.has_tree_view() => Some("The response is not JSON"),
            Some(v) if v.mode != ViewMode::Pretty => Some("jq filters the Pretty tab"),
            Some(_) => None,
            None => Some("The response is not JSON"),
        }
    }

    /// Pastes into the jq bar (the bracketed-paste/ctrl+v path). `false`
    /// while the bar isn't focused, mirroring `paste_into_search`.
    pub fn paste_into_jq(&mut self, text: &str) -> bool {
        if !self.jq.focused {
            return false;
        }
        self.jq.menu = None;
        self.jq.input.paste(text);
        self.jq.edited = true;
        true
    }

    /// The focused bar's selection, for ctrl+c: the copy twin of
    /// [`Self::paste_into_jq`]. `None` when the bar does not own the
    /// keyboard, so an old highlight in a blurred bar never shadows the
    /// tree's.
    pub fn jq_selected_text(&self) -> Option<String> {
        if !self.jq.focused {
            return None;
        }
        self.jq.input.selected_text()
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

    /// Whether menu mode's candidate row has been entered with Tab (a
    /// chip is selected and Tab/shift+Tab step it).
    pub fn jq_menu_open(&self) -> bool {
        self.jq.menu.is_some()
    }

    /// Whether menu mode's candidate row is on show under the bar at all,
    /// entered or not.
    pub fn jq_menu_offered(&self) -> bool {
        self.jq.menu_row().is_some()
    }

    pub fn jq_ghost(&self) -> Option<&str> {
        self.jq.ghost()
    }

    /// Whether the showing ghost is the closing-bracket fallback rather
    /// than a key or builtin continuation — Tab, Right and End all accept
    /// it the same way menu mode's lone candidate does, so the footer
    /// needs to tell the two apart.
    pub fn jq_ghost_is_closer(&self) -> bool {
        self.jq.ghost().is_some() && self.jq.completion.closer
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
        if !focused || !at_end {
            let c = &mut bar.completion;
            c.ctx = None;
            c.candidates.clear();
            c.index = 0;
            c.closer = false;
            return None;
        }
        let text = bar.input.text().to_string();
        let c = &mut bar.completion;
        let ctx = complete::context(&text);
        if ctx != c.ctx {
            c.index = 0;
        }
        c.ctx = ctx;
        c.closer = false;
        let Some(ctx) = c.ctx.clone() else {
            c.candidates.clear();
            c.fall_back_to_closer(&text);
            return None;
        };
        let expr = match ctx.kind {
            Kind::Word => {
                c.candidates = complete::candidates(&ctx, &[]);
                c.fall_back_to_closer(&text);
                return None;
            }
            Kind::Key { .. } | Kind::Shorthand { .. } => {
                ctx.input_expr.clone().unwrap_or_else(|| ".".into())
            }
        };
        if c.cached_expr.as_deref() == Some(expr.as_str()) {
            c.candidates = complete::candidates(&ctx, &c.cached_keys);
            c.fall_back_to_closer(&text);
            return None;
        }
        c.candidates.clear();
        if c.pending.as_ref().is_some_and(|(_, e)| *e == expr) {
            return None;
        }
        let view = self.view.as_mut()?;
        // No document and no JSON body to make one from: nothing to
        // complete against (a body parse still running comes back through
        // `attach_tree`, after which this runs again).
        if view.jq_doc.is_none() && view.tree.is_none() {
            return None;
        }
        let body_len = view.raw_lines.iter().map(|l| l.len() + 1).sum::<usize>();
        if body_len <= sync_limit {
            if view.jq_doc.is_none() {
                view.jq_doc = JqDocument::parse(&view.raw_lines.join("\n")).ok();
            }
            let doc = view.jq_doc.clone()?;
            c.cached_keys = complete::keys_at(&expr, &doc);
            c.cached_expr = Some(expr);
            c.candidates = complete::candidates(&ctx, &c.cached_keys);
            c.fall_back_to_closer(&text);
            return None;
        }
        c.seq += 1;
        c.pending = Some((c.seq, expr.clone()));
        let doc = view.jq_doc.clone();
        let body = doc.is_none().then(|| view.raw_lines.join("\n"));
        Some(JqCompleteRequest {
            generation: view.generation,
            seq: c.seq,
            input_expr: expr,
            doc,
            body,
        })
    }

    /// Lands a pool key fetch. Keys for a view that is no longer on
    /// screen (a stale `generation`), or from a fetch a newer one has
    /// superseded (a stale `seq`), are dropped without being cached — but
    /// a stale generation whose `seq` is still the outstanding fetch
    /// clears it, so the expression can be fetched again. A valid attach
    /// caches the keys even when the caret's context has moved on; the
    /// ghost updates only if that context still asks for this expression.
    /// Returns whether the ghost changed.
    pub fn attach_jq_completion(
        &mut self,
        generation: u64,
        seq: u64,
        input_expr: String,
        keys: Vec<String>,
        doc: Option<JqDocument>,
    ) -> bool {
        let c = &mut self.jq.completion;
        if c.pending() != Some(seq) {
            return false;
        }
        match self.view.as_mut() {
            Some(view) if view.generation == generation => {
                // The fetch had to parse the body: keep the document so
                // the next run or fetch for this view needn't.
                if view.jq_doc.is_none() {
                    view.jq_doc = doc;
                }
            }
            _ => {
                self.jq.completion.pending = None;
                return false;
            }
        }
        let c = &mut self.jq.completion;
        c.pending = None;
        c.cached_keys = keys;
        c.cached_expr = Some(input_expr);
        if let Some(ctx) = &c.ctx
            && ctx.input_expr.as_deref() == c.cached_expr.as_deref()
        {
            c.candidates = complete::candidates(ctx, &c.cached_keys);
            let text = self.jq.input.text().to_string();
            let c = &mut self.jq.completion;
            c.fall_back_to_closer(&text);
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
        JqBar::apply_candidate(&mut self.jq.input, &cand);
        self.jq.edited = true;
    }

    /// The bar's ctrl/alt+backspace: a path segment when the caret is in
    /// a path token and nothing is selected, else the input's own rule.
    /// Both are calls, not keys: the vim profile forwards this key with
    /// the input quiet (plan M3b Deviation 6).
    fn jq_segment_backspace(&mut self) {
        self.jq.menu = None;
        match self.jq.segment_delete_target() {
            Some(target) if self.jq.input.selection().is_none() => {
                self.jq.input.delete_back_to(target);
            }
            _ => self.jq.input.delete_word_back(),
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

    /// The live search box's selection, for ctrl+c: the copy twin of
    /// [`Self::paste_into_search`].
    pub fn search_selected_text(&self) -> Option<String> {
        let search = self.view.as_ref()?.search.as_ref()?;
        if !search.active {
            return None;
        }
        search.input.selected_text()
    }

    /// The in-pane search's input, for the vim engine (plan M3b).
    pub fn search_input_mut(&mut self) -> Option<&mut LineInput> {
        let search = self.view.as_mut()?.search.as_mut()?;
        Some(&mut search.input)
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
        // The bar goes with the tab, so the caret can't stay in it — but
        // the filter stays switched on, ready for the way back.
        if mode != ViewMode::Pretty {
            self.jq.blur();
        }
    }

    /// Opens the in-pane search (`/`, `?`, the search button): Vim's
    /// pattern or plain text (plan M5a Deviation 3), forward or backward.
    pub fn open_search(&mut self, syntax: SearchSyntax, backward: bool) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        view.open_search(syntax, backward);
        true
    }

    /// Arrows' `n`/`N` and the ▼/▲ buttons: the next match down (`1`) or
    /// up (`-1`) from the cursor. A still-live input is committed first, as
    /// Enter does, and that commit *is* the step: the mouse route types a
    /// query and clicks ▼ (found by the stage-7 tmux sweep). With no search,
    /// or an empty one, nothing happens, as on main (amendment A5.4, the
    /// user's decision): vim's `n` is [`Response::find_next`].
    pub fn step_search(&mut self, delta: i32) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        match view.search.as_ref() {
            None => false,
            Some(s) if s.active => {
                view.commit_search(false, &mut self.last_search);
                true
            }
            Some(s) if s.query.is_empty() => false,
            Some(_) => {
                view.note = view.land_match(delta < 0, 1);
                true
            }
        }
    }

    /// Vim's `n` and `N` (plan M5a Deviation 10): `count` hits of vim's
    /// last pattern in its direction, `N` against it. Unless the search on
    /// screen is that pattern (Esc cleared it, or it is an arrows one),
    /// the pattern comes back first; with none, Vim's E35 (probe E6). A box
    /// still being typed in is committed instead, as [`Self::step_search`]
    /// does, so what was typed is never lost.
    pub fn find_next(&mut self, reverse: bool, count: usize) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        if view.search.as_ref().is_some_and(|s| s.active) {
            view.commit_search(false, &mut self.last_search);
            return true;
        }
        let Some((query, backward)) = self.last_search.pattern.clone() else {
            view.note = Some(crate::vim::engine::NO_PREVIOUS_PATTERN.into());
            return false;
        };
        let shown = view
            .search
            .as_ref()
            .is_some_and(|s| s.syntax == SearchSyntax::Vim && s.query == query && s.backward == backward);
        if !shown {
            view.restore_search(query, backward);
        }
        view.note = view.land_match(backward ^ reverse, count);
        true
    }

    /// Vim's `*` and `#` (plan M5a Task 5).
    pub fn find_word(&mut self, backward: bool, count: usize) -> bool {
        self.view.as_mut().is_some_and(|view| view.star(backward, count, &mut self.last_search))
    }

    /// The last pattern a search here set since the app last asked, for
    /// the text engine: Vim has one for every window.
    pub fn take_last_search(&mut self) -> Option<(String, bool)> {
        std::mem::take(&mut self.last_search.set).then(|| self.last_search.pattern.clone()).flatten()
    }

    /// The text engine's last pattern, at each input event's end: a search
    /// in a field may have replaced it. A committed vim search on screen
    /// becomes the new pattern, as Vim's highlight shows the last pattern
    /// in every window; a box being typed in, an arrows search, or none
    /// (after Esc) stay as they are, and `n` takes the pattern up.
    pub fn adopt_last_search(&mut self, pattern: Option<(&str, bool)>) {
        let Some((query, backward)) = pattern else { return };
        if self.last_search.pattern.as_ref().is_some_and(|(q, b)| q == query && *b == backward) {
            return;
        }
        self.last_search.pattern = Some((query.to_string(), backward));
        if let Some(view) = self.view.as_mut()
            && view
                .search
                .as_ref()
                .is_some_and(|s| !s.active && s.syntax == SearchSyntax::Vim && !s.query.is_empty())
        {
            view.restore_search(query.to_string(), backward);
        }
    }

    /// The note a search left for the app to toast, once.
    pub fn take_note(&mut self) -> Option<String> {
        self.view.as_mut()?.note.take()
    }

    /// The cursor's row on the Headers view when it is a header with a
    /// value to copy (a received or sent header, not the divider or a
    /// note): what arrows' `c` copies (`Action::CopyResponseHeader`).
    pub fn copyable_header_row(&self) -> Option<usize> {
        let view = self.view.as_ref()?;
        (view.mode == ViewMode::Headers
            && matches!(view.header_row(view.cursor), Some(HeaderRow::Received(_) | HeaderRow::Sent(_))))
        .then_some(view.cursor)
    }

    /// Clicking a body row: moves the cursor there, and — when `toggle` is
    /// set and the view is `Pretty` — collapses/expands the container it
    /// opens (a no-op on a scalar row).
    pub fn click_row(&mut self, row: usize, toggle: bool) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        view.cursor_moved();
        view.cursor = row.min(view.visible_len().saturating_sub(1));
        let cursor = view.cursor;
        if toggle
            && view.mode == ViewMode::Pretty
            && let Some(tree) = view.active_tree_mut()
        {
            tree.toggle(cursor);
            view.after_fold();
        }
    }

    /// `zo` `zc` `za` `zR` `zM` (piece 4 spec §5.7): the tree view's folds,
    /// `count` levels. Whether anything changed; Raw and Headers have none.
    pub fn fold(&mut self, op: crate::action::FoldOp, count: usize) -> bool {
        self.view.as_mut().is_some_and(|view| view.fold(op, count))
    }

    /// A vim motion or window command (plan M5a Task 3). Whether anything
    /// moved: a beep needs no redraw.
    pub fn motion(&mut self, m: ResponseMotion) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        let mark = view.motion_mark();
        view.motion(m);
        view.motion_mark() != mark
    }

    /// Whether a line selection is up: the response's V surface.
    pub fn line_selection(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.line_anchor.is_some())
    }

    /// A motion in V (plan M5a Deviation 11). Whether anything moved.
    pub fn extend(&mut self, m: ResponseMotion) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        let mark = view.motion_mark();
        view.extend(m);
        view.motion_mark() != mark
    }

    /// `V`, Esc, `o` and the yanks (plan M5a Task 6): the yanked lines for
    /// a yank, else `None`.
    pub fn lines(&mut self, op: crate::action::LinesOp) -> Option<Vec<String>> {
        use crate::action::LinesOp;
        let view = self.view.as_mut()?;
        match op {
            LinesOp::Start => {
                if view.visible_len() > 0 {
                    let at = view.cursor;
                    view.set_line_sel(at);
                }
                None
            }
            LinesOp::End => {
                view.clear_sel();
                None
            }
            LinesOp::Swap => {
                view.swap_ends();
                None
            }
            LinesOp::Yank { count } => view.yank_rows(count),
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
        // keys go to its LineInput, Enter/Down blur (committing is
        // implicit — every edit re-runs the filter — so the filter
        // stays on; Enter on an entered menu row first confirms it), Esc
        // does the same — it leaves the bar with the text kept (the field
        // rule; ctrl+z, not Esc, is what walks an edit back) — unless an
        // AI request is pending, in which case it cancels that instead.
        // Runs before the view is borrowed, so it works even with no ready
        // view (it never should, in practice: the bar can't focus without
        // one).
        if self.jq.focused {
            // Menu mode's candidate row is entered: Tab and shift+Tab step
            // through it, Enter confirms the selected chip and leaves the
            // row (staying in the bar — its text is already the chip's,
            // so there is nothing more to take), Esc un-picks it — the
            // text goes back to what was typed before Tab, the row closes,
            // the caret stays (Esc again leaves the bar as usual) — and
            // any other key leaves the row keeping the selection and is
            // then handled as usual.
            if self.jq.menu.is_some() {
                match ev.code {
                    KeyCode::Tab if ev.modifiers.is_empty() => {
                        self.jq.step_menu(true);
                        return Some(Action::Render);
                    }
                    KeyCode::BackTab => {
                        self.jq.step_menu(false);
                        return Some(Action::Render);
                    }
                    KeyCode::Enter => {
                        self.jq.menu = None;
                        return Some(Action::Render);
                    }
                    KeyCode::Esc => {
                        self.jq.unpick_menu();
                        return Some(Action::Render);
                    }
                    _ => self.jq.menu = None,
                }
            }
            // Something is on offer — a ghost (cycle mode) or the live row
            // (menu mode): Tab steps the ghost or enters the row, shift+Tab
            // the same backwards, Right/End accept a ghost, everything
            // else falls through to the input (and re-derives the offer
            // in the reconcile).
            if self.jq.ghost().is_some() || self.jq.menu_row().is_some() {
                let plain = ev.modifiers.is_empty();
                let cycle = self.jq.tab == JqTab::Cycle;
                match ev.code {
                    KeyCode::Tab if plain && cycle => {
                        self.jq.completion.step(true);
                        return Some(Action::Render);
                    }
                    KeyCode::Tab if plain => {
                        self.jq.open_menu(0);
                        return Some(Action::Render);
                    }
                    KeyCode::BackTab => {
                        if cycle {
                            self.jq.completion.step(false);
                        } else {
                            self.jq.open_menu(usize::MAX);
                        }
                        return Some(Action::Render);
                    }
                    // Right/End accept a ghost. Menu mode has one only
                    // for a closer, so there they accept exactly that.
                    // The vim profile asks the same before its engine.
                    KeyCode::Right | KeyCode::End if plain && self.jq_accepts_completion() => {
                        self.accept_jq_completion();
                        return Some(Action::Render);
                    }
                    _ => {}
                }
            }
            match ev.code {
                // Enter and Down both leave the bar with the edit kept:
                // Enter commits what was typed (a ghost is an offer, not
                // text — fish's rule, so `.id` can be the filter when
                // `.identifier` exists), Down is the URL field's leave key.
                KeyCode::Enter | KeyCode::Down => self.jq.blur(),
                // ctrl/alt+backspace (and the legacy ctrl+h byte) delete a
                // path segment rather than the input's "word": see
                // `segment_delete_target`. A live selection, or a plain
                // word, is the input's business as usual.
                KeyCode::Backspace
                    if ev
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.jq_segment_backspace();
                }
                KeyCode::Char('h') if ev.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.jq_segment_backspace();
                }
                KeyCode::Esc => {
                    if self.jq.ai_pending {
                        return Some(Action::CancelJqDescribe);
                    }
                    // The field rule: Esc closes the bar keeping its text,
                    // exactly like Enter.
                    self.jq.blur();
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
        // go to the LineInput; Enter runs the search and hands the caret
        // back to the tree, and so does arrows' Esc (the field rule) — Esc
        // in the tree is what clears it. Vim's Esc abandons the box
        // (`ReadyView::commit_search`).
        if view.search.as_ref().is_some_and(|s| s.active) {
            match ev.code {
                KeyCode::Enter => view.commit_search(false, &mut self.last_search),
                KeyCode::Esc => view.commit_search(true, &mut self.last_search),
                _ => {
                    let search = view.search.as_mut().expect("checked above");
                    search.input.handle_key(ev);
                }
            }
            return Some(Action::Render);
        }

        // Each navigation arm takes exactly the chord it names: a bare named
        // key, shift+↑/↓ (extend the selection), ctrl+Home/End (document
        // jumps) or ctrl+d/u/f/b. Any other chord does nothing rather than
        // act as the bare key (piece 4 spec §5.1).
        let plain = ev.modifiers.is_empty();
        match ev.code {
            // No pane-local view key: the strip is walked by the tab chord
            // every pane shares (alt+←/→ → `Action::CycleTabs`, resolved
            // against the focused pane in `app.rs`).
            // The pane's letters (`c`, `/`, `n`, `N`) are the router's table
            // rows (piece 4 spec §4.5): the component keeps named keys only.
            // ctrl+d/u/f/b and PageDown/PageUp are Vim's window commands in
            // both profiles (the user's decision, 2026-10-10).
            KeyCode::Char('d') if ev.modifiers == KeyModifiers::CONTROL => {
                view.motion(ResponseMotion::HalfPage { down: true, count: None });
                Some(Action::Render)
            }
            KeyCode::Char('u') if ev.modifiers == KeyModifiers::CONTROL => {
                view.motion(ResponseMotion::HalfPage { down: false, count: None });
                Some(Action::Render)
            }
            KeyCode::Char('f') if ev.modifiers == KeyModifiers::CONTROL => {
                view.motion(ResponseMotion::Page { down: true, count: 1 });
                Some(Action::Render)
            }
            KeyCode::Char('b') if ev.modifiers == KeyModifiers::CONTROL => {
                view.motion(ResponseMotion::Page { down: false, count: 1 });
                Some(Action::Render)
            }
            KeyCode::Down if ev.modifiers == KeyModifiers::SHIFT => {
                view.select_line_extend(1);
                Some(Action::Render)
            }
            KeyCode::Up if ev.modifiers == KeyModifiers::SHIFT => {
                view.select_line_extend(-1);
                Some(Action::Render)
            }
            KeyCode::Down if plain => {
                view.move_cursor(1);
                Some(Action::Render)
            }
            KeyCode::Up if plain => {
                view.move_cursor(-1);
                Some(Action::Render)
            }
            KeyCode::PageDown if plain => {
                view.motion(ResponseMotion::Page { down: true, count: 1 });
                Some(Action::Render)
            }
            KeyCode::PageUp if plain => {
                view.motion(ResponseMotion::Page { down: false, count: 1 });
                Some(Action::Render)
            }
            KeyCode::Right if plain => {
                view.scroll_h(H_SCROLL_STEP.into());
                Some(Action::Render)
            }
            KeyCode::Left if plain => {
                view.scroll_h((-H_SCROLL_STEP).into());
                Some(Action::Render)
            }
            // Document jumps: ctrl+Home/End.
            KeyCode::Home if ev.modifiers == KeyModifiers::CONTROL => {
                view.jump_cursor(0);
                Some(Action::Render)
            }
            KeyCode::End if ev.modifiers == KeyModifiers::CONTROL => {
                view.jump_cursor(usize::MAX);
                Some(Action::Render)
            }
            KeyCode::Home if plain => {
                view.h_scroll = 0;
                Some(Action::Render)
            }
            KeyCode::End if plain => {
                view.scroll_h(i32::MAX);
                Some(Action::Render)
            }
            KeyCode::Char(' ') | KeyCode::Enter if plain => {
                let cursor = view.cursor;
                if view.mode == ViewMode::Pretty
                    && let Some(tree) = view.active_tree_mut()
                {
                    tree.toggle(cursor);
                    view.after_fold();
                }
                Some(Action::Render)
            }
            KeyCode::Esc if view.sel.is_some() => {
                view.clear_sel();
                Some(Action::Render)
            }
            // The search and its highlight go; vim's last pattern stays
            // for `n` and an empty `/`, as Vim's Esc never forgets it
            // (replacing plan M5a Deviation 13).
            KeyCode::Esc if view.search.is_some() => {
                view.search = None;
                Some(Action::Render)
            }
            // Esc from the tree stops at the selection and the search —
            // clearing the search is its job, since the box's own Esc only
            // runs the query and hands the caret back. It never touches
            // the jq filter (that is saved with the request; the 󰈲
            // button/alt+shift+q are its switch, and ctrl+z in the bar is
            // what walks an edit back).
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
            // jumps, and ctrl+d/u/f/b, the vim-alias paging keys, which
            // nothing global binds — unless the jq bar has the caret: then,
            // like the URL field, it gets every combo the keymap left
            // unbound (ctrl+arrows, ctrl+a, ctrl/alt+backspace…), since
            // those are text editing, not navigation.
            ResponseState::Ready(_)
                if self.jq.focused
                    || !ev
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    || (ev.modifiers == KeyModifiers::CONTROL
                        && matches!(
                            ev.code,
                            KeyCode::Home
                                | KeyCode::End
                                | KeyCode::Char('d')
                                | KeyCode::Char('u')
                                | KeyCode::Char('f')
                                | KeyCode::Char('b')
                        )) =>
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
        // stays visible after the bar itself loses focus. On `Raw` and
        // `Headers` there is nothing for a filter to do, so the bar goes
        // with the tab (`Response::jq_open`, spelled out here because
        // `view` already holds the borrow it would need).
        let jq_open = self.jq.is_open() && view.mode == ViewMode::Pretty;
        let jq_rows = if jq_open {
            1 + u16::from(
                self.jq.error.is_some() || self.jq.note.is_some() || self.jq.menu_row().is_some(),
            )
        } else {
            0
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(HEADER_STRIP_HEIGHT), // status chip / chips+tabs / underline
                Constraint::Length(jq_rows),             // jq bar (+ error/menu row)
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
        self.jq_caret = None;
        if jq_open {
            self.jq_caret = draw_jq_bar(frame, hits, rows[1], &self.jq, ctx);
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

        if view.height != body_area.height as usize {
            view.scroll_opt = None;
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
    let jq_available = view.has_tree_view() && view.mode == ViewMode::Pretty;
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
        underline,
        disabled: None,
        right_anchored: 0,
    }
    .paint(buf, tabstrip_area, t.panel, t);
    for (rect, mode) in rects.into_iter().zip(modes) {
        hits.register(rect, crate::hit::Hit::ResponseTab(mode));
    }
}

/// A [`crate::paint::TabStrip::tabs`]-shaped label list: `(text, badge)`
/// per tab, where a badge is a trailing colored glyph.
type TabLabels = Vec<(String, Option<(&'static str, ratatui::style::Color)>)>;

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
    // The strip's order lives in `ReadyView::tab_modes` alone, so `{N}gt`
    // and the tab chord can never disagree with what is drawn.
    let modes = view.tab_modes();
    let tabs: TabLabels = modes
        .iter()
        .map(|mode| match mode {
            ViewMode::Pretty => {
                let badge = view.jq_tree.is_some().then_some((
                    crate::glyph::FILTER,
                    if bar.stale { theme.error } else { theme.accent },
                ));
                ("Tree".to_string(), badge)
            }
            ViewMode::Raw => ("Raw".to_string(), None),
            ViewMode::Headers => ("Headers".to_string(), None),
        })
        .collect();
    (tabs, modes)
}

/// The horizontal span [`crate::paint::TabStrip::paint`] occupies for
/// `tabs`, mirroring its own padded-block-width + 1-column-gap layout so
/// callers can right-align the strip without painting it first.
fn tabstrip_width(tabs: &[(String, Option<(&'static str, ratatui::style::Color)>)]) -> u16 {
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
    (
        crate::glyph::SEARCH_PILL,
        crate::hit::Hit::ResponseSearchButton,
    ),
    (
        crate::glyph::PENCIL_PILL,
        crate::hit::Hit::ResponseEditorButton,
    ),
    (crate::glyph::SAVE_PILL, crate::hit::Hit::SaveBodyButton),
    (crate::glyph::COPY_PILL, crate::hit::Hit::CopyBodyButton),
    (crate::glyph::FILTER_PILL, crate::hit::Hit::ResponseJqButton),
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
        crate::paint::action(
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
/// `HeaderCopy` hit over the trailing ` 󰆏 ` pill appended to each row.
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
            let dim = Style::default().fg(t.text_muted);
            for (i, line) in view.header_lines.iter().enumerate().take(end).skip(start) {
                let row = view.header_row(i).unwrap_or(HeaderRow::Note);
                match row {
                    HeaderRow::Divider => {
                        let rule = "\u{2500}".repeat(
                            (area.width as usize).saturating_sub(line.chars().count()),
                        );
                        push(i, i, vec![(format!("{line}{rule}"), dim)], true, (0, view.h_scroll));
                        continue;
                    }
                    HeaderRow::Note => {
                        push(i, i, vec![(line.clone(), dim)], true, (0, view.h_scroll));
                        continue;
                    }
                    HeaderRow::Received(_) | HeaderRow::Sent(_) => {}
                }
                // A sent row reads dim, like the editor's computed rows: the
                // record of the send, not the answer.
                let text = if matches!(row, HeaderRow::Sent(_)) {
                    dim
                } else {
                    text
                };
                let (name, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
                let name_piece = format!("{name}:");
                // One plain cell between the value and the pill, so the
                // pill's hover fill never touches the text.
                let value_piece = format!("{value} ");
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
                    (crate::glyph::COPY_PILL.to_string(), glyph_style),
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
/// spinner while an AI request is pending), and the `󰙴` AI button
/// right-aligned. A second row — when the bar reserved one — shows the
/// last error's message, with its span (when known) underlined in the bar
/// text above it. An incomplete filter with the caret at its end is drawn
/// muted, since it is still being typed.
fn draw_jq_bar(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    bar: &JqBar,
    ctx: &DrawCtx,
) -> Option<Position> {
    let t = ctx.theme;
    crate::paint::fill(frame.buffer_mut(), area, t.page);
    if area.height == 0 {
        return None;
    }
    let mut caret = None;
    const AI: &str = crate::glyph::CREATION_PILL;
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
        let w = text_w.saturating_sub(3);
        // The focused bar's caret is the terminal's own cursor (a bar, set
        // once at startup) rather than the painted reversed cell every
        // other input uses: a completion ghost trails the caret, and a
        // thin bar reads as "the text continues here" where a block
        // swallowing the ghost's first letter did not. The cell is
        // reported to the caller, which places the cursor once the frame
        // has decided nothing covers the bar.
        let line = bar.input.draw_line_windowed_no_caret(bar.focused, t, w);
        spans.extend(line.spans);
        if bar.focused && w > 0 {
            let col = bar.input.caret_column(w);
            caret = Some(Position::new(row.x + 3 + col, row.y));
        }
        if let Some(ghost) = bar.ghost() {
            // The ghost starts in the caret's own cell (the bar cursor sits
            // on that cell's left edge) and gets what is left of the row;
            // it is clipped, never the text.
            let used = bar.input.visible_window(true, w).chars().count() as u16;
            let room = w.saturating_sub(used) as usize;
            if room > 0 {
                let shown: String = ghost.chars().take(room).collect();
                spans.push(Span::styled(shown, Style::default().fg(t.text_muted)));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    hits.register(row, crate::hit::Hit::ResponseJqBar);
    if area.width > ai_w {
        let ai_area = Rect::new(area.right() - ai_w, area.y, ai_w, 1);
        crate::paint::action(
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
    // Menu mode's candidate row owns the second row while there is one:
    // the user is choosing, and a half-typed token's error under it
    // would only say what the row already shows.
    if let Some(chips) = bar.menu_row()
        && area.height >= 2
    {
        let menu_row = Rect {
            y: area.y + 1,
            height: 1,
            ..area
        };
        let scroll = draw_jq_menu(frame, menu_row, &chips, bar.menu_scroll.get(), t);
        bar.menu_scroll.set(scroll);
        return caret;
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
        // An unfinished filter (an unclosed bracket, a trailing pipe) is
        // what every filter looks like while it is being typed: with the
        // caret at its end it is a note in the muted colour, not an
        // error. Once the caret moves back into the text or the bar is
        // left, the filter as it stands is what runs, and it is broken.
        let typing = bar.focused && bar.caret_at_end();
        let color = if err.incomplete() && typing {
            t.text_muted
        } else {
            t.error
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("   {}", err.message()),
                Style::default().fg(color),
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
    caret
}

/// One chip per candidate, aligned under the bar text (past the `jq `
/// chip), the selected one (if any) filled. The row is a window over
/// the chips that scrolls like a list: from `prev` (its last offset) it
/// moves only when the selected chip is out of view, and only as far as
/// it must — right until the chip's end is in, left until its start is —
/// so Tab walks the selection to the right edge before the row slides,
/// and shift+Tab walks it back to the left edge before the row slides
/// the other way. Nothing selected, it starts at the left. Returns the
/// offset used, for the caller to remember.
fn draw_jq_menu(
    frame: &mut Frame,
    row: Rect,
    chips: &[(String, bool)],
    prev: u16,
    t: &Theme,
) -> u16 {
    let x0 = row.x + 3;
    let width = row.width.saturating_sub(3);
    if width == 0 {
        return 0;
    }
    // Lay the chips out on an unbounded line: (start col, label).
    let mut cells: Vec<(u16, String, bool)> = Vec::new();
    let mut col: u16 = 0;
    let mut selected = (0u16, 0u16);
    for (label, is_selected) in chips.iter().map(|(l, sel)| (l.as_str(), *sel)) {
        let chip = format!(" {label} ");
        let w = chip.chars().count() as u16;
        if is_selected {
            selected = (col, col + w);
        }
        cells.push((col, chip, is_selected));
        col = col.saturating_add(w).saturating_add(1);
    }
    let scroll = if chips.iter().any(|(_, sel)| *sel) {
        prev.max(selected.1.saturating_sub(width)).min(selected.0)
    } else {
        0
    };
    let buf = frame.buffer_mut();
    for (start, chip, is_selected) in cells {
        let style = if is_selected {
            Style::default().bg(t.selection).fg(t.text)
        } else {
            Style::default().fg(t.text_muted)
        };
        for (i, ch) in chip.chars().enumerate() {
            let c = start + i as u16;
            if c < scroll {
                continue;
            }
            let x = c - scroll;
            if x >= width {
                break;
            }
            let cell = &mut buf[(x0 + x, row.y)];
            cell.set_char(ch);
            cell.set_style(style);
        }
    }
    scroll
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
        crate::paint::action(buf, rect, label, hit.clone(), ctx.hovered, t.page, t);
        hits.register(rect, hit);
    }
}

/// `/query   3/17` — or the live input while the search is being typed.
fn search_footer(view: &ReadyView, t: &Theme, width: u16) -> Line<'static> {
    let Some(search) = &view.search else {
        return Line::raw("");
    };
    let accent = Style::default().fg(t.accent);
    let mut spans = vec![Span::styled(if search.backward { "?" } else { "/" }, accent)];
    if search.active {
        // The leading "/" (or "?") already claims one column of `width`.
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
    /// Steps the view one tab forward along the strip, wrapping, as alt+→
    /// with the pane focused does (`App::tab_target`).
    fn cycle(r: &mut Response) {
        let Some(modes) = r.tab_modes() else { return };
        let at = modes.iter().position(|m| Some(*m) == r.view().map(|v| v.mode)).unwrap_or(0);
        r.set_view_mode(modes[(at + 1) % modes.len()]);
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
            sent_headers: vec![crate::http::SentHeader {
                name: "authorization".into(),
                value: "Bearer s3cret".into(),
                display: format!("Bearer {}", postui_core::prepare::SECRET_MASK),
            }],
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
            pointer: None,
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
            pointer: None,
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

    /// Every cursor move but shift+↑/↓ drops a line selection, the half
    /// and whole page steps and the document jumps included, so the next
    /// shift+↓ grows from the cursor and not from a stale anchor the jump
    /// left behind.
    #[test]
    fn a_cursor_jump_drops_the_line_selection() {
        let body = (0..100).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let shift_down = KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT);
        let jumps = [
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            key(KeyCode::PageDown),
            key(KeyCode::PageUp),
            KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL),
        ];
        for jump in jumps {
            let mut r = ready(&body);
            render(&mut r);
            // Mid-document, so no page key beeps at an edge: a beep changes
            // nothing and keeps the selection (plan M5a amendment A3.2).
            r.view.as_mut().unwrap().scroll = 50;
            r.view.as_mut().unwrap().cursor = 50;
            r.handle_key(shift_down);
            r.handle_key(shift_down);
            assert_eq!(r.selected_text().as_deref(), Some("50\n51\n52"), "{jump:?}: the selection to drop");
            r.handle_key(jump);
            assert_eq!(r.selected_text(), None, "{jump:?}: the jump drops it");
            let cursor = r.view().unwrap().cursor;
            r.handle_key(shift_down);
            let sel = r.view().unwrap().sel.expect("shift+down selects again");
            assert_eq!(sel.0.0, cursor, "{jump:?}: the new selection starts at the cursor the jump left");
        }
    }

    /// The same for the search jumps (fourth M4a review): the commit of a
    /// typed query (Enter, or the `▼` button while the input is live) and
    /// each `n`/`N` step land the cursor on a match and drop the selection
    /// with it.
    #[test]
    fn a_search_jump_drops_the_line_selection() {
        let body = (0..100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let shift_down = KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT);
        let select = |r: &mut Response| {
            r.handle_key(shift_down);
            r.handle_key(shift_down);
            assert_eq!(r.selected_text().as_deref(), Some("line 0\nline 1\nline 2"), "the selection to drop");
        };
        let selects_from_cursor = |r: &mut Response, what: &str| {
            assert_eq!(r.selected_text(), None, "{what}: the jump drops it");
            let cursor = r.view().unwrap().cursor;
            r.handle_key(shift_down);
            let sel = r.view().unwrap().sel.expect("shift+down selects again");
            assert_eq!(sel.0.0, cursor, "{what}: the new selection starts at the cursor the jump left");
        };
        // Enter commits the query and jumps to the first match.
        let mut r = ready(&body);
        render(&mut r);
        select(&mut r);
        assert!(r.open_search(SearchSyntax::Literal, false));
        for c in "line 5".chars() {
            r.handle_key(ch(c));
        }
        r.handle_key(key(KeyCode::Enter));
        assert_eq!(r.view().unwrap().cursor, 5, "jumped to the first match");
        selects_from_cursor(&mut r, "Enter");
        // `n` and `N` step the matches (the router's rows, so the pane's
        // `step_search` as they call it).
        for (delta, what) in [(1, "n"), (-1, "N")] {
            r.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
            select(&mut r);
            assert!(r.step_search(delta));
            selects_from_cursor(&mut r, what);
        }
        // The `▼` button while the input is still live commits it, and
        // that commit is the jump.
        let mut r = ready(&body);
        render(&mut r);
        select(&mut r);
        assert!(r.open_search(SearchSyntax::Literal, false));
        for c in "line 7".chars() {
            r.handle_key(ch(c));
        }
        assert!(r.step_search(1));
        assert_eq!(r.view().unwrap().cursor, 7, "jumped to the first match");
        selects_from_cursor(&mut r, "button commit");
    }

    fn thirty() -> Response {
        let body = (1..=30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let mut r = ready(&body);
        r.view.as_mut().unwrap().height = 23;
        r
    }

    fn lines_of(range: std::ops::RangeInclusive<usize>) -> Vec<String> {
        range.map(|i| format!("line {i}")).collect()
    }

    /// The plan's probes V1–V13: the yanked lines and where the cursor ends.
    #[test]
    fn yank_and_v_match_vim() {
        use crate::action::{LinesOp, ResponseMotion as M};
        let yank = LinesOp::Yank { count: None };
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 4;
        r.lines(LinesOp::Start);
        r.extend(M::Lines(2));
        assert!(r.line_selection());
        assert_eq!(r.lines(yank), Some(lines_of(5..=7)), "V1");
        assert_eq!((r.view().unwrap().cursor, r.line_selection()), (4, false));
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 4;
        r.lines(LinesOp::Start);
        r.extend(M::Lines(-2));
        assert_eq!(r.lines(yank), Some(lines_of(3..=5)), "V2");
        assert_eq!(r.view().unwrap().cursor, 2, "the cursor on the first line yanked");
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 4;
        r.lines(LinesOp::Start);
        r.extend(M::Lines(1));
        r.lines(LinesOp::Swap);
        assert_eq!(r.view().unwrap().cursor, 4, "V12: o swaps the ends");
        assert!(r.line_selection());
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 24;
        r.lines(LinesOp::Start);
        r.extend(M::Goto { line: None, last: true });
        assert_eq!(r.lines(yank), Some(lines_of(25..=30)), "V4");
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 4;
        assert_eq!(r.lines(LinesOp::Yank { count: Some(3) }), Some(lines_of(5..=7)), "V6");
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 24;
        assert_eq!(r.lines(LinesOp::Yank { count: Some(99) }), Some(lines_of(25..=30)), "V13: clamped");
        // Vim's `{N}yy` on the last line beeps (probed: `3yy` there leaves
        // `@"` alone); `yy` and `1yy` yank it.
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 29;
        assert_eq!(r.lines(LinesOp::Yank { count: Some(3) }), None, "{{N}}yy on the last line");
        assert_eq!(r.view().unwrap().cursor, 29);
        assert_eq!(r.lines(LinesOp::Yank { count: Some(1) }), Some(lines_of(30..=30)));
        assert_eq!(r.lines(LinesOp::Yank { count: None }), Some(lines_of(30..=30)));
        r.view.as_mut().unwrap().cursor = 28;
        assert_eq!(r.lines(LinesOp::Yank { count: Some(3) }), Some(lines_of(29..=30)), "one line up: clamped");
        let mut r = thirty();
        r.view.as_mut().unwrap().cursor = 4;
        r.lines(LinesOp::Start);
        r.extend(M::Lines(1));
        r.lines(LinesOp::End);
        assert_eq!((r.view().unwrap().cursor, r.line_selection()), (5, false), "V10, V11");
    }

    /// Arrows' text search: char columns past wide chars, non-overlapping
    /// hits, case kept (plan M5a Deviation 6).
    #[test]
    fn a_literal_search_counts_columns_in_chars() {
        let corpus = ["aéé".to_string(), "x".into(), "ééb é".into(), "aaaa".into(), "A".into()];
        let mut s = SearchState::committed("é".into(), SearchSyntax::Literal, false);
        assert_eq!(s.find_matches(&corpus), None);
        assert_eq!(s.matches, [(0, 1, 2), (0, 2, 3), (2, 0, 1), (2, 1, 2), (2, 4, 5)]);
        let mut s = SearchState::committed("aa".into(), SearchSyntax::Literal, false);
        s.find_matches(&corpus);
        assert_eq!(s.matches, [(3, 0, 2), (3, 2, 4)], "non-overlapping, and `A` is not `a`");
        let mut s = SearchState::committed("éb é".into(), SearchSyntax::Literal, false);
        s.find_matches(&corpus);
        assert_eq!(s.matches, [(2, 1, 5)]);
    }

    /// A motion that beeps changes nothing, so it asks for no redraw; one
    /// that moves does.
    #[test]
    fn a_beeping_motion_asks_no_redraw() {
        use crate::action::{LinesOp, ResponseMotion as M};
        let mut r = thirty();
        assert!(r.motion(M::Goto { line: None, last: true }));
        assert!(!r.motion(M::Lines(1)), "j on the last line");
        assert!(!r.motion(M::HalfPage { down: true, count: None }), "ctrl+d at the end");
        assert!(r.motion(M::Lines(-1)));
        r.lines(LinesOp::Start);
        assert!(r.extend(M::Lines(1)), "V grows");
        assert!(!r.extend(M::Lines(1)), "V at the end");
        assert!(r.line_selection());
        let mut r = thirty();
        assert!(!r.motion(M::Lines(-1)), "k on the first line");
        assert!(r.motion(M::Page { down: true, count: 1 }));
    }

    /// A vim search in V extends it, as Vim's Visual does, and
    /// a fold the landing opens above the anchor leaves the anchor on its
    /// own line, as Vim's Visual start is a buffer position.
    #[test]
    fn a_vim_search_in_v_keeps_the_anchor_line_when_it_opens_a_fold() {
        use crate::action::{FoldOp, LinesOp};
        // Lines: 0 `{`, 1 `"a": {`, 2 `"b": "hit"`, 3 `},`, 4 `"c": 1,`,
        // 5 `"d": 2`, 6 `}`.
        let mut r = ready(r#"{"a": {"b": "hit"}, "c": 1, "d": 2}"#);
        render(&mut r);
        r.view.as_mut().unwrap().cursor = 1;
        r.fold(FoldOp::Close, 1);
        r.view.as_mut().unwrap().cursor = 3;
        r.lines(LinesOp::Start);
        r.open_search(SearchSyntax::Vim, true);
        typed(&mut r, "hit");
        assert_eq!(r.view().unwrap().cursor, 2, "on `b`, the fold opened");
        assert!(r.line_selection(), "V extended");
        let lines = r.lines(LinesOp::Yank { count: None }).unwrap();
        assert_eq!(lines.len(), 4, "from `b` to `d`: {lines:?}");
        assert_eq!(lines[3].trim(), r#""d": 2"#);
    }

    /// Probes Y1, Y2 and Y4: a closed fold yanks all its lines.
    #[test]
    fn a_closed_fold_yanks_all_its_lines() {
        use crate::action::{FoldOp, LinesOp};
        let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
        render(&mut r);
        r.view.as_mut().unwrap().cursor = 1;
        r.fold(FoldOp::Close, 1);
        let lines = r.lines(LinesOp::Yank { count: None }).unwrap();
        assert_eq!(lines.len(), 3, "Y1: {lines:?}");
        assert_eq!(lines[0].trim(), r#""a": {"#);
        let lines = r.lines(LinesOp::Yank { count: Some(2) }).unwrap();
        assert_eq!(lines.len(), 4, "Y2");
        r.fold(FoldOp::CloseAll, 1);
        assert_eq!(r.lines(LinesOp::Yank { count: None }).unwrap().len(), 6, "Y4");
        assert_eq!(r.view().unwrap().cursor, 0);
    }

    /// Where Vim leaves its cursor after a yank (probes Y5, Y8, Y9, Y10
    /// in the Task 6 report, R6a and R6d in its review): a `yy` on the
    /// cursor's own line keeps its column, so a following `n` starts
    /// there; a V `y` goes to column 0; a yank of a closed fold puts the
    /// cursor on the fold's first line, so `zR` shows it there.
    #[test]
    fn a_yank_leaves_the_cursor_where_vim_does() {
        use crate::action::{FoldOp, LinesOp};
        let mut r = thirty();
        let v = r.view.as_mut().unwrap();
        v.cursor = 4;
        v.at_col = Some((4, 3));
        r.lines(LinesOp::Yank { count: Some(3) });
        assert_eq!(r.view().unwrap().at_col, Some((4, 3)), "Y5: yy keeps the column");
        // R6a, R6d: a V `y` puts the cursor in column 0, even with the
        // cursor on the selection's first line, so `n` finds the same match.
        let body = (1..=30).map(|i| format!("line {i} line {i}")).collect::<Vec<_>>().join("\n");
        let mut r = ready(&body);
        r.view.as_mut().unwrap().height = 23;
        assert!(r.open_search(SearchSyntax::Vim, false));
        r.handle_key(ch('5'));
        r.handle_key(key(KeyCode::Enter));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (4, Some((4, 5))), "/5 lands");
        r.lines(LinesOp::Start);
        r.lines(LinesOp::Yank { count: None });
        assert_eq!(r.view().unwrap().at_col, None, "R6a: V y drops the column");
        assert!(r.find_next(false, 1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (4, Some((4, 5))), "R6d: n finds the same match");
        for v_line in [false, true] {
            let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
            render(&mut r);
            r.view.as_mut().unwrap().cursor = 2;
            r.fold(FoldOp::CloseAll, 1);
            assert_eq!(r.view().unwrap().real_line(), Some(2), "hidden in the closed root");
            if v_line {
                r.lines(LinesOp::Start);
            }
            assert_eq!(r.lines(LinesOp::Yank { count: None }).unwrap().len(), 6);
            r.fold(FoldOp::OpenAll, 1);
            assert_eq!(r.view().unwrap().cursor, 0, "Y8/Y10 (V {v_line}): the fold's first line");
        }
    }

    /// Review Focus 4: the selection ends when the view changes under it.
    #[test]
    fn v_line_ends_when_the_view_changes_under_it() {
        use crate::action::{LinesOp, ResponseMotion as M};
        let mut r = thirty();
        r.lines(LinesOp::Start);
        r.extend(M::Lines(1));
        r.set_view_mode(ViewMode::Headers);
        assert!(!r.line_selection(), "a view switch");
        let mut r = thirty();
        r.lines(LinesOp::Start);
        r.motion(M::Lines(1));
        assert!(!r.line_selection(), "a plain motion");
        let mut r = thirty();
        render(&mut r);
        r.lines(LinesOp::Start);
        let area = r.view().unwrap().last_area.unwrap();
        r.begin_selection_at(area.x + 1, area.y + 1);
        assert!(!r.line_selection(), "a click");
    }

    /// Amendment A6.1 (the user's decision): shift+↑/↓ after a drag turns
    /// the drag's character selection into a line selection from the
    /// drag's anchor line, as on main, wherever the cursor is.
    #[test]
    fn shift_arrows_after_a_drag_grow_lines_from_the_drags_anchor() {
        for (key, cursor, want) in [
            (KeyCode::Down, 0, "line 2\nline 3\nline 4"),
            (KeyCode::Up, 8, "line 4\nline 5\nline 6\nline 7\nline 8"),
        ] {
            let mut r = thirty();
            render(&mut r);
            r.view.as_mut().unwrap().cursor = cursor;
            let area = r.view().unwrap().last_area.unwrap();
            assert!(r.begin_selection_at(area.x + 2, area.y + 3));
            assert!(r.drag_selection_to(area.x + 1, area.y + 5));
            assert!(!r.line_selection(), "a drag is a character selection");
            r.handle_key(KeyEvent::new(key, KeyModifiers::SHIFT));
            assert!(r.line_selection(), "{key:?}: now a line selection");
            assert_eq!(r.selected_text().as_deref(), Some(want), "{key:?}: from the drag's line 4 to the cursor");
        }
    }

    /// Amendment A6.5: in V, a window key that pulls an off-screen cursor
    /// to the view's edge (Deviation 12) takes the selection's moving end
    /// with it.
    #[test]
    fn v_line_follows_a_cursor_a_window_key_pulls() {
        use crate::action::{Place, ResponseMotion as M};
        for m in [
            M::Place { at: Place::Top, line: None },
            M::Place { at: Place::Middle, line: None },
            M::Place { at: Place::Bottom, line: None },
            M::HalfPage { down: true, count: None },
            M::HalfPage { down: false, count: None },
            M::Page { down: true, count: 1 },
            M::Page { down: false, count: 1 },
        ] {
            let mut r = hundred_at(0, 5);
            r.lines(crate::action::LinesOp::Start);
            // The wheel leaves the cursor above the view.
            r.view.as_mut().unwrap().scroll = 40;
            r.extend(m);
            let v = r.view().unwrap();
            assert!(v.cursor >= v.scroll && v.cursor < v.scroll + 23, "{m:?}: the cursor is in the view");
            assert_ne!(v.cursor, 5, "{m:?}: the cursor moved");
            let sel = v.sel.expect("still a selection");
            assert!(r.line_selection(), "{m:?}");
            assert_eq!((sel.0.0, sel.1.0), (5, v.cursor), "{m:?}: from the anchor to the cursor");
        }
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

    /// The header strip carries a 󰏫 icon that opens the active tab's text
    /// in `$EDITOR`, registered like the other icon actions.
    #[test]
    fn header_actions_include_an_open_in_editor_icon() {
        let mut r = ready(r#"{"a": 1}"#);
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
            "the 󰏫 icon registers its hit"
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

    /// The Headers tab lists what the request went out with under the
    /// response headers, past a `sent` divider — so the send is still on
    /// record after the request's headers have been edited. A secret's
    /// value shows as its mask, never in the clear.
    #[test]
    fn headers_tab_lists_the_sent_headers_masked_below_a_divider() {
        let mut r = ready(r#"{"a": 1}"#);
        r.set_view_mode(ViewMode::Headers);
        let out = render(&mut r);
        let received = out.find("content-type:").expect("response header row");
        let divider = out.find("\u{2500} sent").expect("sent divider");
        let sent = out.find("authorization:").expect("sent header row");
        assert!(
            received < divider && divider < sent,
            "response headers, then the divider, then the sent headers: {out}"
        );
        assert!(
            out.contains(&format!("Bearer {}", postui_core::prepare::SECRET_MASK)),
            "the sent value is masked: {out}"
        );
        assert!(!out.contains("s3cret"), "the secret never renders: {out}");
    }

    /// `c` (the router's row) copies the header under the cursor; on the
    /// divider there is no header, so there is nothing to copy.
    #[test]
    fn c_on_the_sent_divider_copies_nothing() {
        let mut r = ready(r#"{"a": 1}"#);
        r.set_view_mode(ViewMode::Headers);
        assert_eq!(r.copyable_header_row(), Some(0), "row 0 is a response header");
        r.view.as_mut().unwrap().cursor = 1;
        assert_eq!(r.view().unwrap().header_row(1), Some(HeaderRow::Divider));
        assert_eq!(r.copyable_header_row(), None, "the divider has nothing to copy");
    }

    /// Plan M4a Deviation 11: `c` copies only a header row on the Headers
    /// view; the response no longer reads its letters itself.
    #[test]
    fn only_a_header_row_is_copyable_and_the_letters_are_not_the_panes() {
        let mut r = ready(r#"{"a": 1}"#);
        render(&mut r);
        assert_eq!(r.copyable_header_row(), None, "not the Headers view");
        assert_eq!(r.handle_key(ch('/')), None, "the router's row opens the search now");
        assert_eq!(r.handle_key(ch('n')), None);
        r.set_view_mode(ViewMode::Headers);
        render(&mut r);
        assert_eq!(r.handle_key(ch('c')), None);
        let row = (0..20).find(|&i| matches!(r.view().unwrap().header_row(i), Some(HeaderRow::Received(_))));
        let row = row.expect("the fixture has a received header");
        r.view.as_mut().unwrap().cursor = row;
        assert_eq!(r.copyable_header_row(), Some(row));
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
            pointer: None,
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
            pointer: None,
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
            pointer: None,
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
            pointer: None,
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
            vec![" ", "\u{F018F}", " "],
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
            pointer: None,
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
    fn t_cycles_to_raw_verbatim_and_back_to_pretty() {
        let body = "{\"a\": 1,\n     \"b\": 2}";
        let mut r = ready(body);
        assert!(render(&mut r).contains("  \"a\": 1,"), "pretty re-indents");
        cycle(&mut r);
        let out = render(&mut r);
        assert!(out.contains("{\"a\": 1,"), "raw is verbatim: {out}");
        assert!(
            out.contains("     \"b\": 2}"),
            "raw keeps original spacing: {out}"
        );
        cycle(&mut r); // Raw -> Headers
        cycle(&mut r); // Headers -> Pretty
        assert!(
            render(&mut r).contains("  \"a\": 1,"),
            "cycles back to pretty"
        );
    }

    #[test]
    fn non_json_defaults_to_raw() {
        let mut r = ready("<html>hi</html>");
        assert!(render(&mut r).contains("<html>hi</html>"));
        // `r` no longer binds anything in the response pane.
        assert_eq!(r.handle_key(ch('r')), None);
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
        cycle(&mut r); // Pretty -> Raw
        cycle(&mut r); // Raw -> Headers
        let out = render(&mut r);
        assert!(out.contains("content-type: application/json"), "{out}");
        cycle(&mut r); // Headers -> Pretty
        assert!(
            render(&mut r).contains("\"a\""),
            "t cycles back to the body view"
        );
    }

    #[test]
    fn cursor_moves_and_clamps_at_both_ends() {
        let mut r = ready(r#"{"a": 1, "b": 2}"#); // 4 lines
        for _ in 0..20 {
            r.handle_key(key(KeyCode::Down));
        }
        assert_eq!(r.view().unwrap().cursor, 3, "clamped to the last line");
        for _ in 0..20 {
            r.handle_key(key(KeyCode::Up));
        }
        assert_eq!(r.view().unwrap().cursor, 0, "clamped to the first line");
        r.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(r.view().unwrap().cursor, 3);
        r.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL));
        assert_eq!(r.view().unwrap().cursor, 0);
    }

    /// The vim letters are the router's (piece 4 spec §4.5): the pane
    /// answers none of them, and the cursor and scroll stay put.
    #[test]
    fn the_vim_letters_do_nothing_in_the_response_pane() {
        let body = (0..30).map(|i| format!("{:>200}", i)).collect::<Vec<_>>().join("\n");
        let mut r = ready(&body);
        render(&mut r);
        for letter in ['j', 'k', 'h', 'l', 'g', 'G'] {
            assert_eq!(r.handle_key(ch(letter)), None, "{letter}");
            let v = r.view().unwrap();
            assert_eq!((v.cursor, v.h_scroll), (0, 0), "{letter} moved the view");
        }
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
            pointer: None,
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

    /// "line 1" … "line 100" on Raw, a 23-row viewport as in the plan's Vim
    /// 9.1 probes, the view's top on `top` and the cursor on `cursor`
    /// (0-based).
    fn hundred_at(top: usize, cursor: usize) -> Response {
        let body = (1..=100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let mut r = ready(&body);
        let v = r.view.as_mut().unwrap();
        v.height = 23;
        v.scroll = top;
        v.cursor = cursor;
        r
    }

    /// Plan M5a Task 3: every window key gives Vim 9.1's top line and
    /// cursor (the plan's probe record, IDs as there).
    #[test]
    fn the_window_keys_match_vim() {
        use crate::action::{Place, ResponseMotion as M};
        let f = |count| M::Page { down: true, count };
        let b = M::Page { down: false, count: 1 };
        let d = |count| M::HalfPage { down: true, count };
        let u = M::HalfPage { down: false, count: None };
        let at = |at, line| M::Place { at, line };
        let goto = |line| M::Goto { line, last: true };
        type Probe = (&'static str, usize, usize, Vec<M>, usize, usize);
        let cases: Vec<Probe> = vec![
            ("P1", 0, 0, vec![f(1)], 21, 21),
            ("P2", 0, 0, vec![f(1), f(1)], 42, 42),
            ("P3", 49, 59, vec![b], 28, 50),
            ("P4", 77, 79, vec![f(1)], 99, 99),
            ("P5", 77, 79, vec![f(1), f(1)], 99, 99),
            ("P7", 0, 4, vec![b], 0, 4),
            ("P8", 0, 0, vec![f(2)], 42, 42),
            ("P9", 0, 9, vec![f(1)], 21, 21),
            ("P10", 29, 39, vec![b], 8, 30),
            ("X1", 0, 0, vec![f(1), b], 0, 22),
            ("X5", 0, 0, vec![d(None), u], 0, 0),
            ("Z1", 39, 49, vec![at(Place::Top, None)], 49, 49),
            ("Z2", 39, 49, vec![at(Place::Middle, None)], 38, 49),
            ("Z3", 39, 49, vec![at(Place::Bottom, None)], 27, 49),
            ("Z4", 77, 99, vec![at(Place::Top, None)], 99, 99),
            ("Z5", 77, 97, vec![at(Place::Middle, None)], 86, 97),
            ("Z6", 0, 2, vec![at(Place::Bottom, None)], 0, 2),
            ("Z7", 39, 49, vec![at(Place::Top, Some(10))], 9, 9),
            ("Z8", 0, 1, vec![at(Place::Middle, None)], 0, 1),
            ("D1", 0, 0, vec![d(None)], 11, 11),
            ("D2", 77, 89, vec![d(None)], 77, 99),
            ("D3", 77, 99, vec![d(None)], 77, 99),
            ("D4", 0, 0, vec![d(Some(3)), d(None)], 6, 6),
            ("J1", 0, 0, vec![goto(None)], 77, 99),
            ("J2", 0, 0, vec![goto(Some(50))], 38, 49),
            ("J3", 39, 39, vec![M::Lines(-1)], 38, 38),
            ("J4", 39, 44, vec![goto(Some(30))], 18, 29),
            ("J5", 39, 44, vec![goto(Some(10))], 0, 9),
            ("J6", 0, 4, vec![goto(Some(75))], 63, 74),
            ("J7", 0, 4, vec![goto(Some(40))], 28, 39),
            ("X4", 0, 0, vec![goto(None), at(Place::Top, None), M::Lines(-1)], 98, 98),
        ];
        for (id, top, cursor, motions, want_top, want_cursor) in cases {
            let mut r = hundred_at(top, cursor);
            for m in motions {
                r.motion(m);
            }
            let v = r.view().unwrap();
            assert_eq!((v.scroll, v.cursor), (want_top, want_cursor), "{id}");
        }
    }

    /// Review Focus 5: short, past-the-end and empty views.
    #[test]
    fn window_keys_on_short_and_empty_views() {
        use crate::action::{Place, ResponseMotion as M};
        let mut r = ready("line 1\nline 2\nline 3\nline 4\nline 5");
        r.view.as_mut().unwrap().height = 23;
        r.motion(M::Page { down: true, count: 1 });
        assert_eq!((r.view().unwrap().scroll, r.view().unwrap().cursor), (4, 4), "X2: the last line on top");
        let mut r = ready("line 1\nline 2\nline 3\nline 4\nline 5");
        r.view.as_mut().unwrap().height = 23;
        r.view.as_mut().unwrap().cursor = 2;
        r.motion(M::Page { down: false, count: 1 });
        assert_eq!((r.view().unwrap().scroll, r.view().unwrap().cursor), (0, 2), "X3: ctrl+b at the top beeps");
        // A big body still parsing has no tree yet: an empty Pretty view.
        let mut r = ready_gen(&big_json(), 1);
        assert_eq!(r.view().unwrap().visible_len(), 0);
        for m in [
            M::Lines(3),
            M::Goto { line: None, last: true },
            M::HalfPage { down: true, count: None },
            M::Page { down: false, count: 1 },
            M::Place { at: Place::Middle, line: Some(4) },
        ] {
            r.motion(m);
        }
        assert_eq!((r.view().unwrap().scroll, r.view().unwrap().cursor), (0, 0));
    }

    /// Amendment A3.2: a window key that beeps at an edge changes nothing,
    /// and a line selection stays up (Vim keeps Visual on a failed page key).
    #[test]
    fn a_window_key_that_beeps_keeps_the_selection() {
        use crate::action::ResponseMotion as M;
        let beeps = [
            M::Page { down: false, count: 1 },
            M::HalfPage { down: false, count: None },
        ];
        for m in beeps {
            let mut r = hundred_at(0, 0);
            r.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
            let before = {
                let v = r.view().unwrap();
                assert!(v.sel.is_some(), "a line selection is up");
                (v.scroll, v.cursor, v.sel)
            };
            r.motion(m);
            let v = r.view().unwrap();
            assert_eq!((v.scroll, v.cursor, v.sel), before, "{m:?} at the top changes nothing");
        }
        let mut r = hundred_at(99, 98);
        r.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        let before = {
            let v = r.view().unwrap();
            (v.scroll, v.cursor, v.sel)
        };
        r.motion(M::Page { down: true, count: 1 });
        r.motion(M::HalfPage { down: true, count: None });
        let v = r.view().unwrap();
        assert_eq!((v.scroll, v.cursor, v.sel), before, "ctrl+f and ctrl+d at the end change nothing");
        // A page key that does move ends the selection.
        let mut r = hundred_at(0, 0);
        r.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        r.motion(M::Page { down: true, count: 1 });
        assert!(r.view().unwrap().sel.is_none(), "a page that moves drops the selection");
    }

    /// PageDown and PageUp are Vim's ctrl+f and ctrl+b in both profiles
    /// (the user's decision, 2026-10-10).
    #[test]
    fn the_page_keys_are_vims_page() {
        let mut r = hundred_at(0, 0);
        r.handle_key(key(KeyCode::PageDown));
        assert_eq!((r.view().unwrap().scroll, r.view().unwrap().cursor), (21, 21));
        r.handle_key(key(KeyCode::PageUp));
        assert_eq!((r.view().unwrap().scroll, r.view().unwrap().cursor), (0, 22));
    }

    #[test]
    fn t_cycles_pretty_raw_headers_and_skips_pretty_without_a_tree() {
        let mut r = ready(r#"{"a":1}"#);
        render(&mut r);
        assert_eq!(r.view().unwrap().mode, ViewMode::Pretty);
        cycle(&mut r);
        assert_eq!(r.view().unwrap().mode, ViewMode::Raw);
        cycle(&mut r);
        assert_eq!(r.view().unwrap().mode, ViewMode::Headers);
        cycle(&mut r);
        assert_eq!(r.view().unwrap().mode, ViewMode::Pretty);
        let mut r = ready("not json");
        render(&mut r);
        assert_eq!(r.view().unwrap().mode, ViewMode::Raw);
        cycle(&mut r);
        assert_eq!(r.view().unwrap().mode, ViewMode::Headers);
        cycle(&mut r);
        assert_eq!(r.view().unwrap().mode, ViewMode::Raw, "no tree: Raw and Headers only");
        assert!(
            !matches!(r.handle_key(ch('r')), Some(Action::ResponseViewMode(_))),
            "r no longer switches views"
        );
        assert!(
            !matches!(r.handle_key(ch('h')), Some(Action::ResponseViewMode(_))),
            "h never switches views"
        );
    }

    /// Piece 4 spec §5.7 (`R.ctrl-d`) and plan M4a Deviation 10: half a page
    /// of view and cursor together; at the ends the cursor still moves.
    #[test]
    fn ctrl_d_and_ctrl_u_scroll_with_the_cursor_and_stop_at_the_ends() {
        let body = (0..100).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let mut r = ready(&body);
        render(&mut r);
        let height = r.view().unwrap().height;
        let half = (height / 2).max(1);
        let down = || KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        let up = || KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        r.handle_key(down());
        let v = r.view().unwrap();
        assert_eq!((v.cursor, v.scroll), (half, half), "the view scrolls with the cursor");
        r.handle_key(up());
        let v = r.view().unwrap();
        assert_eq!((v.cursor, v.scroll), (0, 0));
        for _ in 0..100 {
            r.handle_key(down());
        }
        let v = r.view().unwrap();
        assert_eq!(v.cursor, 99, "the cursor reaches the last line");
        assert_eq!(v.scroll, 100 - height, "the view never scrolls past the end");
        r.handle_key(up());
        let v = r.view().unwrap();
        assert_eq!(v.cursor, 99 - half);
        r.view.as_mut().unwrap().cursor = 3;
        r.view.as_mut().unwrap().scroll = 0;
        r.handle_key(up());
        let v = r.view().unwrap();
        assert_eq!((v.cursor, v.scroll), (0, 0), "at the top the cursor still moves");
        // A view left past the end (the pane grew, or a fold shrank what
        // shows) never scrolls back up on ctrl+d.
        let past = 100 - height + 3;
        r.view.as_mut().unwrap().scroll = past;
        r.view.as_mut().unwrap().cursor = past;
        r.handle_key(down());
        let v = r.view().unwrap();
        assert_eq!((v.cursor, v.scroll), (past + half, past), "ctrl+d never moves the view up");
    }

    /// The wheel and the scrollbar move the view and leave the cursor off
    /// screen; `ctrl+d` and `ctrl+u` then step from the view, never back
    /// toward the cursor it left behind.
    #[test]
    fn ctrl_d_and_ctrl_u_step_from_a_view_the_wheel_moved() {
        let body = (0..200).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let mut r = ready(&body);
        render(&mut r);
        let height = r.view().unwrap().height;
        let half = (height / 2).max(1);
        let down = || KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        let up = || KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        r.handle_scroll(50);
        assert_eq!(r.view().unwrap().cursor, 0, "the wheel leaves the cursor behind");
        r.handle_key(down());
        let v = r.view().unwrap();
        assert_eq!(v.scroll, 50 + half, "ctrl+d moves the view down, not back to the cursor");
        assert_eq!(v.cursor, 50 + half, "the cursor lands on the view's top row");
        let bottom = 120;
        r.view.as_mut().unwrap().cursor = bottom + height + 10;
        r.set_scroll(bottom);
        r.handle_key(up());
        let v = r.view().unwrap();
        assert_eq!(v.scroll, bottom - half, "ctrl+u moves the view up, not down to the cursor");
        assert_eq!(v.cursor, bottom + height - 1 - half, "the cursor keeps the view's bottom row");
    }

    /// Plan M5a Deviation 8 (probes F2, F3, F4, F7): the cursor shows on
    /// the fold's line but keeps the line it is really on until it moves.
    #[test]
    fn folds_keep_the_cursors_line() {
        use crate::action::FoldOp;
        let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
        render(&mut r);
        r.view.as_mut().unwrap().cursor = 2;
        assert!(r.fold(FoldOp::Close, 1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().real_line()), (1, Some(2)), "F2");
        assert!(r.fold(FoldOp::Open, 1));
        assert_eq!(r.view().unwrap().cursor, 2, "F3: back on its line");
        r.fold(FoldOp::Close, 1);
        r.motion(crate::action::ResponseMotion::Lines(1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().real_line()), (2, Some(4)), "F4: j leaves the fold");
        let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
        render(&mut r);
        r.view.as_mut().unwrap().cursor = 2;
        assert!(r.fold(FoldOp::CloseAll, 1));
        assert_eq!(r.view().unwrap().cursor, 0, "F0");
        assert!(r.fold(FoldOp::Open, 1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().real_line()), (1, Some(2)), "F7");
        assert!(r.fold(FoldOp::OpenAll, 1));
        assert_eq!(r.view().unwrap().cursor, 2, "F1");
        r.set_view_mode(ViewMode::Raw);
        assert!(!r.fold(FoldOp::Toggle, 1), "Raw has no folds");
    }

    /// Amendment A4.1 (Deviation 8, "any cursor move forgets it"): a click,
    /// which writes the cursor outside `set_cur_row`, forgets the hidden
    /// line too, so a later `zo` goes from the clicked row.
    #[test]
    fn a_click_forgets_the_line_a_fold_hides() {
        use crate::action::FoldOp;
        let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
        render(&mut r);
        r.view.as_mut().unwrap().cursor = 2;
        assert!(r.fold(FoldOp::Close, 1));
        assert_eq!(r.view().unwrap().real_line(), Some(2), "hidden inside \"a\"");
        // visible: 0 {, 1 "a": {..}, 2 "c": 2, 3 }
        r.click_row(2, false);
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().real_line()), (2, Some(4)));
        assert!(!r.fold(FoldOp::Open, 1), "nothing closed holds the clicked line");
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().real_line()), (2, Some(4)), "not back on the old hidden line");
    }

    /// Final review I1 (probes G1–G8, Vim 9.1.697): on a folded tree,
    /// `{N}G`, `{N}gg` and `{N}zt` count buffer lines, as Vim does, not
    /// the rows shown. A line a closed fold hides shows on the fold's row,
    /// which remembers it; bare `G` goes to the last line the same way.
    #[test]
    fn counted_jumps_count_buffer_lines_on_a_folded_tree() {
        use crate::action::{FoldOp, Place, ResponseMotion as M};
        let gg = |line| M::Goto { line, last: false };
        let big_g = |line| M::Goto { line, last: true };
        let zt = |line| M::Place { at: Place::Top, line };
        // Full lines: 0 {, 1 "a": {, 2 "b": 1, 3 }, 4 "c": 2, 5 }. After
        // `zc` on line 2 the rows are: 0 {, 1 "a": {..} (lines 1-3), 2
        // "c": 2, 3 }. Each case: (id, zM instead of zc, motions, cursor
        // row, real line, top row).
        let cases = [
            ("G1", false, vec![gg(None), big_g(Some(4))], 1, 3, None),
            ("G2", false, vec![gg(None), big_g(Some(5))], 2, 4, None),
            ("G3", false, vec![gg(None), zt(Some(5))], 2, 4, Some(2)),
            ("G4", false, vec![big_g(None), gg(Some(3))], 1, 2, None),
            ("G5", true, vec![big_g(None)], 0, 5, None),
            ("G6", false, vec![gg(None), zt(Some(3))], 1, 2, Some(1)),
            ("G7", false, vec![gg(None), big_g(Some(99))], 3, 5, None),
        ];
        for (id, all, motions, row, real, top) in cases {
            let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
            r.view.as_mut().unwrap().height = 23;
            r.view.as_mut().unwrap().cursor = 2;
            assert!(r.fold(if all { FoldOp::CloseAll } else { FoldOp::Close }, 1));
            for m in motions {
                r.motion(m);
            }
            let v = r.view().unwrap();
            assert_eq!((v.cursor, v.real_line()), (row, Some(real)), "{id}");
            if let Some(top) = top {
                assert_eq!(v.scroll, top, "{id}: the line's row on top");
            }
        }
        // G8: `zM G zR` shows the cursor on the last line it went to.
        let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
        r.view.as_mut().unwrap().height = 23;
        r.fold(FoldOp::CloseAll, 1);
        r.motion(big_g(None));
        assert!(r.fold(FoldOp::OpenAll, 1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().real_line()), (5, Some(5)), "G8");
        // Task 4's review probe: `k`, `3k`, `gg` and `1G` from a line hidden
        // under a fold on the top row land on the opener, forgetting it.
        for m in [M::Lines(-1), M::Lines(-3), gg(None), big_g(Some(1))] {
            let mut r = ready(r#"{"a": {"b": 1}, "c": 2}"#);
            r.view.as_mut().unwrap().height = 23;
            r.view.as_mut().unwrap().cursor = 2;
            r.fold(FoldOp::CloseAll, 1);
            r.motion(m);
            assert_eq!((r.view().unwrap().cursor, r.view().unwrap().real_line()), (0, Some(0)), "{m:?}");
        }
    }

    /// Final review m3: a page key that pulls an off-screen cursor to the
    /// view's edge and then beeps changes nothing, the search column
    /// included, so `n` still steps on from the match it landed on.
    #[test]
    fn a_page_key_that_beeps_keeps_the_search_column() {
        use crate::action::ResponseMotion as M;
        let body = (0..100).map(|i| if i == 50 { "a x x".to_string() } else { format!("line {i}") }).collect::<Vec<_>>().join("\n");
        let mut r = ready(&body);
        r.view.as_mut().unwrap().height = 23;
        assert!(r.open_search(SearchSyntax::Vim, false));
        r.handle_key(ch('x'));
        r.handle_key(key(KeyCode::Enter));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (50, Some((50, 2))), "/x lands");
        r.set_scroll(0);
        r.motion(M::Page { down: false, count: 1 });
        let v = r.view().unwrap();
        assert_eq!((v.scroll, v.cursor, v.at_col), (0, 50, Some((50, 2))), "ctrl+b at the top beeps");
        assert!(r.find_next(false, 1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (50, Some((50, 4))), "n finds the next match");
    }

    /// Final review T5 m4 (probes J1–J3, Vim 9.1.697): a `j` or `k` that
    /// beeps at the view's edge leaves the cursor where it is, its search
    /// column included, so `n` finds the next match, not the same one.
    #[test]
    fn a_line_motion_that_beeps_keeps_the_search_column() {
        use crate::action::ResponseMotion as M;
        let mut r = ready("x\nb\nc x x");
        r.view.as_mut().unwrap().height = 23;
        r.view.as_mut().unwrap().cursor = 1;
        assert!(r.open_search(SearchSyntax::Vim, false));
        r.handle_key(ch('x'));
        r.handle_key(key(KeyCode::Enter));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (2, Some((2, 2))), "J2: /x lands");
        r.motion(M::Lines(1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (2, Some((2, 2))), "J2: j beeps");
        assert!(r.find_next(false, 1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (2, Some((2, 4))), "J1: n finds the next match");
        let mut r = ready("c x x\nb\nx");
        r.view.as_mut().unwrap().height = 23;
        r.view.as_mut().unwrap().cursor = 1;
        assert!(r.open_search(SearchSyntax::Vim, true));
        r.handle_key(ch('x'));
        r.handle_key(key(KeyCode::Enter));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (0, Some((0, 4))), "?x lands");
        r.motion(M::Lines(-1));
        assert!(r.find_next(false, 1));
        assert_eq!((r.view().unwrap().cursor, r.view().unwrap().at_col), (0, Some((0, 2))), "J3: n goes on backward");
    }

    #[test]
    fn the_arrows_scroll_and_the_ctrl_keys_page() {
        let body = (0..100).map(|i| format!("{:>200}", i)).collect::<Vec<_>>().join("\n");
        let mut a = ready(&body);
        let mut b = ready(&body);
        render(&mut a);
        render(&mut b);
        a.handle_key(key(KeyCode::Right));
        assert_ne!(a.view().unwrap().h_scroll, 0, "Right scrolls sideways");
        a.handle_key(key(KeyCode::Left));
        assert_eq!(a.view().unwrap().h_scroll, 0, "Left scrolls back");
        a.view.as_mut().unwrap().height = 23;
        b.view.as_mut().unwrap().height = 23;
        a.handle_key(key(KeyCode::PageDown));
        b.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_eq!(a.view().unwrap().cursor, b.view().unwrap().cursor);
        assert_eq!((b.view().unwrap().scroll, b.view().unwrap().cursor), (21, 21), "P1");
        b.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!((b.view().unwrap().scroll, b.view().unwrap().cursor), (10, 10), "half a page back");
        b.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!((b.view().unwrap().scroll, b.view().unwrap().cursor), (21, 21));
        a.handle_key(key(KeyCode::PageUp));
        b.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!((a.view().unwrap().scroll, a.view().unwrap().cursor), (0, 22), "X1");
        assert_eq!(a.view().unwrap().cursor, b.view().unwrap().cursor);
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
        cycle(&mut r);
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
            pointer: None,
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
        r.handle_key(key(KeyCode::Down)); // onto the "a" container line
        assert_eq!(r.handle_key(key(KeyCode::Char(' '))), Some(Action::Render));
        assert!(r.view().unwrap().visible_len() < before, "collapsed");
        assert!(render(&mut r).contains("2 keys"));
        r.handle_key(key(KeyCode::Enter));
        assert_eq!(r.view().unwrap().visible_len(), before, "re-expanded");
    }

    #[test]
    fn search_commits_counts_matches_and_wraps() {
        let mut r = ready(r#"{"aa": "zz", "bb": "zz"}"#);
        assert!(r.open_search(SearchSyntax::Literal, false));
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
        r.step_search(1);
        assert!(render(&mut r).contains("2/2"));
        r.step_search(1);
        assert!(render(&mut r).contains("1/2"), "n wraps around");
        r.step_search(-1);
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
        r.open_search(SearchSyntax::Literal, false);
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
        r.open_search(SearchSyntax::Literal, false);
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
        r.open_search(SearchSyntax::Literal, false);
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
    fn search_matches_case_and_finds_nothing_gracefully() {
        let mut r = ready(r#"{"Needle": 1}"#);
        r.open_search(SearchSyntax::Literal, false);
        typed(&mut r, "NEEDLE");
        let out = render(&mut r);
        assert!(out.contains("no matches"), "{out}");
        assert_eq!(r.take_note().as_deref(), Some("Pattern not found: NEEDLE"));
        r.open_search(SearchSyntax::Literal, false);
        typed(&mut r, "Needle");
        assert!(render(&mut r).contains("1/1"));
        assert!(r.step_search(1), "n on the one match stays on it");
    }

    /// The probe record's search JSON (lines 0–11), pretty-printed by the
    /// tree exactly as Vim's buffer held it.
    const PROBE_JSON: &str = r#"{"user-id": 5, "user_id": 6, "Name": "alice", "name": "bob", "items": [1, 2], "id": "x1"}"#;

    fn typed(r: &mut Response, text: &str) {
        for c in text.chars() {
            r.handle_key(ch(c));
        }
        r.handle_key(key(KeyCode::Enter));
    }

    /// Where the cursor and the search are: (cursor line, current match's
    /// line and start, the stored query).
    fn landed(r: &Response) -> (usize, Option<(usize, usize)>, String) {
        let v = r.view().unwrap();
        let s = v.search.as_ref().unwrap();
        (v.cursor, s.matches.get(s.current).map(|m| (m.0, m.1)), s.query.clone())
    }

    /// Probes S1, S3–S12, S19 and the blank-line rule (Review Focus 2).
    /// The tree's lines: 0 `{`, 1 `"user-id": 5,`, 2 `"user_id": 6,`,
    /// 3 `"Name": "alice",`, 4 `"name": "bob",`, 5 `"items": [`, 6 `1,`,
    /// 7 `2`, 8 `],`, 9 `"id": "x1"`, 10 `}`, each indented as printed.
    #[test]
    fn star_takes_vims_word_or_string() {
        let at = |line: usize| {
            let mut r = ready(PROBE_JSON);
            r.view.as_mut().unwrap().height = 23;
            r.view.as_mut().unwrap().cursor = line;
            r
        };
        let lines = at(0).view().unwrap().search_corpus().into_owned();
        assert_eq!(
            lines,
            [
                "{", r#"  "user-id": 5,"#, r#"  "user_id": 6,"#, r#"  "Name": "alice","#, r#"  "name": "bob","#,
                r#"  "items": ["#, "    1,", "    2", "  ],", r#"  "id": "x1""#, "}",
            ],
            "the probe's lines, less its blank line 9"
        );
        let mut r = at(1);
        assert!(r.find_word(false, 1));
        assert_eq!(landed(&r), (1, Some((1, 3)), r"\<user\>".into()), "S1");
        assert_eq!(r.take_note().as_deref(), Some("search hit BOTTOM, continuing at TOP"));
        let mut r = at(2);
        r.find_word(false, 2);
        assert_eq!(landed(&r), (2, Some((2, 3)), r"\<user_id\>".into()), "S9");
        let mut r = at(0);
        r.find_word(false, 1);
        assert_eq!(landed(&r).2, "{", "S3: no keyword, the non-blank string, unescaped");
        let mut r = at(8);
        r.find_word(false, 1);
        assert_eq!(landed(&r), (8, Some((8, 2)), "],".into()), "S10");
        let mut r = at(4);
        r.find_word(true, 1);
        assert_eq!(landed(&r), (4, Some((4, 3)), r"\<name\>".into()), "S6: Name is another word");
        assert_eq!(r.take_note().as_deref(), Some("search hit TOP, continuing at BOTTOM"));
        let mut r = at(3);
        r.find_word(false, 1);
        assert_eq!(landed(&r), (3, Some((3, 3)), r"\<Name\>".into()), "S5: case matters, so it wraps to itself");
        let mut r = at(6);
        r.find_word(false, 1);
        assert_eq!(landed(&r), (6, Some((6, 4)), r"\<1\>".into()), "S11");
        // S7, S8, S19 with matches on both sides (`name` on raw lines 0, 2,
        // 4 and 6), so a wrong direction lands elsewhere.
        let names = || {
            let mut r = ready("name\nx\nname\nx\nname\nx\nname");
            r.view.as_mut().unwrap().cursor = 4;
            r.find_word(true, 1);
            assert_eq!(landed(&r).0, 2, "# goes up");
            r
        };
        let mut r = names();
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r).0, 0, "S7: n keeps #'s direction, up");
        let mut r = names();
        assert!(r.find_next(true, 1));
        assert_eq!(landed(&r).0, 4, "S8: N after # goes down");
        let mut r = names();
        r.find_word(false, 1);
        assert_eq!(landed(&r).0, 4, "* goes down");
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r).0, 6, "S19: * sets the direction forward again");
        let mut r = at(9);
        r.find_word(false, 1);
        assert_eq!(landed(&r), (1, Some((1, 8)), r"\<id\>".into()), "S12: user-id's id");
        let mut r = ready("a\n\nb");
        r.view.as_mut().unwrap().cursor = 1;
        assert!(!r.find_word(false, 1));
        assert_eq!(r.take_note().as_deref(), Some("No string under cursor"), "S4");
        assert_eq!(r.view().unwrap().cursor, 1);
        // No keyword on the line: the string `.*`, escaped, so it matches
        // only itself and never the whole line as a regex would.
        let mut r = ready(".* .*\nabc");
        r.find_word(false, 1);
        let s = r.view().unwrap().search.as_ref().unwrap();
        assert_eq!(s.query, r"\.\*");
        assert_eq!(s.matches.len(), 2, "{:?}", s.matches);
    }

    /// Probes S13–S17, Deviation 4 and Review Focus 3.
    #[test]
    fn the_search_syntax_follows_the_profile() {
        let open = |syntax| {
            let mut r = ready(PROBE_JSON);
            r.view.as_mut().unwrap().height = 23;
            r.view.as_mut().unwrap().cursor = 4;
            r.open_search(syntax, false);
            r
        };
        let mut r = open(SearchSyntax::Vim);
        typed(&mut r, "name");
        assert_eq!(landed(&r).1, Some((4, 3)), "S13: a match later on the cursor's line");
        let mut r = open(SearchSyntax::Vim);
        typed(&mut r, "zzz");
        assert_eq!(r.take_note().as_deref(), Some("Pattern not found: zzz"), "S15");
        assert_eq!(r.view().unwrap().cursor, 4);
        let mut r = ready(PROBE_JSON);
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "name");
        assert_eq!(landed(&r).1, Some((4, 3)), "S17: Name on line 3 is another case");
        let mut r = ready("a.c\nabc\na.c\nxa.c");
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "^a.c$");
        let s = r.view().unwrap().search.as_ref().unwrap();
        assert_eq!(s.matches.iter().map(|m| m.0).collect::<Vec<_>>(), [0, 1, 2], "vim: a pattern, anchored");
        let mut r = ready("a.c\nabc\na.c");
        r.open_search(SearchSyntax::Literal, false);
        typed(&mut r, "a.c");
        assert_eq!(r.view().unwrap().search.as_ref().unwrap().matches.len(), 2, "arrows: plain text");
        let mut r = ready("abc");
        r.open_search(SearchSyntax::Literal, false);
        typed(&mut r, "ABC");
        assert_eq!(r.take_note().as_deref(), Some("Pattern not found: ABC"), "arrows matches case too");
        let mut r = ready("abc\nxyz\nabc");
        r.view.as_mut().unwrap().cursor = 1;
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, r"a\{-}");
        let note = r.take_note().unwrap_or_default();
        assert!(note.starts_with("pattern not supported"), "an atom outside the subset: {note}");
        assert_eq!(r.view().unwrap().cursor, 1, "and nothing moves");
    }

    /// Probe S16: `n` from the cursor, wrapping with Vim's note.
    #[test]
    fn n_steps_from_the_cursor_and_wraps() {
        let mut r = ready(PROBE_JSON);
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "user");
        assert_eq!(landed(&r).1, Some((1, 3)));
        r.find_next(false, 1);
        assert_eq!(landed(&r).1, Some((2, 3)));
        assert_eq!(r.take_note(), None);
        r.find_next(false, 1);
        assert_eq!(landed(&r).1, Some((1, 3)));
        assert_eq!(r.take_note().as_deref(), Some("search hit BOTTOM, continuing at TOP"));
        r.find_next(true, 3);
        assert_eq!(landed(&r).1, Some((2, 3)), "3N: back three, wrapping");
        let mut r = ready(PROBE_JSON);
        assert!(!r.find_next(false, 1));
        assert_eq!(r.take_note().as_deref(), Some("No previous regular expression"), "no search yet");
    }

    /// Amendment A5.3 (probes E2–E5, E7): a match that starts on its line's
    /// end counts one column back, as the engine's `searchit()` has it, so
    /// `/$` on one-char lines steps a line at a time. And Vim's E35 for an
    /// empty `/` with no last pattern (probes E1, E6), the vim syntax only.
    #[test]
    fn a_match_on_the_lines_end_counts_one_column_back() {
        let mut r = ready("a\nb\nc");
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "$");
        assert_eq!(r.view().unwrap().cursor, 1, "E2: not the end of the cursor's own line");
        assert!(r.find_next(false, 1));
        assert_eq!(r.view().unwrap().cursor, 2);
        assert!(r.find_next(false, 1));
        assert_eq!(r.view().unwrap().cursor, 0, "E3");
        assert_eq!(r.take_note().as_deref(), Some("search hit BOTTOM, continuing at TOP"));
        let mut r = ready("x\nab");
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, r"b\|$");
        assert_eq!(landed(&r).1, Some((1, 1)));
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r).1, Some((0, 1)), "E5: ab's end is b's column, skipped");
        assert!(r.find_next(true, 1));
        assert_eq!(landed(&r).1, Some((1, 1)), "E4: back from the first line's end, wrapping to b");
        assert!(r.find_next(true, 1));
        assert_eq!(landed(&r).1, Some((0, 1)), "E7: N from b skips b");
        let mut r = ready("a\nb");
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "");
        assert_eq!(r.take_note().as_deref(), Some("No previous regular expression"), "E1");
        assert!(!r.find_next(false, 1));
        assert_eq!(r.take_note().as_deref(), Some("No previous regular expression"), "E6");
        let mut r = ready("a\nb");
        r.open_search(SearchSyntax::Literal, false);
        typed(&mut r, "");
        assert_eq!(r.take_note(), None, "arrows: an empty box finds nothing, as on main");
    }

    /// The controller's rulings on an empty box, and the view's Esc: an
    /// empty Enter re-runs the last query in vim only (Vim's `/<CR>`);
    /// arrows' finds nothing, as on main. Esc on an empty box only closes
    /// it, in both profiles, while Esc on a typed box runs it (plan Q2).
    /// Esc in the view clears the search but keeps vim's last pattern, so
    /// `n` and an empty `/` still find it, as in Vim.
    #[test]
    fn an_empty_box_reruns_only_in_vim_and_esc_cancels_it() {
        use SearchSyntax::{Literal, Vim};
        // Raw lines: x on 0, 2 and 4.
        const BODY: &str = "x\ny\nx\ny\nx";
        let cursor = |r: &Response| r.view().unwrap().cursor;
        let mut r = ready(BODY);
        r.open_search(Vim, false);
        typed(&mut r, "x");
        assert_eq!(cursor(&r), 2);
        r.open_search(Vim, false);
        typed(&mut r, "");
        assert_eq!((cursor(&r), landed(&r).2), (4, "x".into()), "vim: the last query again");
        let mut r = ready(BODY);
        r.open_search(Literal, false);
        typed(&mut r, "x");
        r.open_search(Literal, false);
        typed(&mut r, "");
        assert_eq!((cursor(&r), landed(&r)), (2, (2, None, String::new())), "arrows: nothing, as on main");
        assert_eq!(r.take_note(), None);
        // Arrows: Esc on an empty box runs nothing; on a typed one it runs
        // it (main's field rule).
        let mut r = ready(BODY);
        r.open_search(Literal, false);
        typed(&mut r, "x");
        r.take_note();
        r.open_search(Literal, false);
        r.handle_key(key(KeyCode::Esc));
        assert_eq!(landed(&r), (2, None, String::new()), "arrows: Esc on an empty box runs nothing");
        assert_eq!(r.take_note(), None);
        assert!(!r.view().unwrap().search.as_ref().unwrap().active, "arrows: the box closed");
        r.open_search(Literal, false);
        r.handle_key(ch('x'));
        r.handle_key(key(KeyCode::Esc));
        assert_eq!(cursor(&r), 4, "arrows: Esc on a typed box runs it");
        // Vim's Esc abandons the box, empty or typed: the search it replaced
        // comes back as it was, the cursor stays, nothing is said.
        let mut r = ready(BODY);
        r.open_search(Vim, false);
        typed(&mut r, "x");
        r.take_note();
        for text in ["", "y"] {
            r.open_search(Vim, false);
            for c in text.chars() {
                r.handle_key(ch(c));
            }
            r.handle_key(key(KeyCode::Esc));
            assert_eq!(landed(&r), (2, Some((2, 0)), "x".into()), "vim: Esc on {text:?} puts the search back");
            assert_eq!(r.take_note(), None, "{text:?}");
            assert!(!r.view().unwrap().search.as_ref().unwrap().active, "{text:?}: the box closed");
        }
        // With no search up, Esc leaves none; `n` still has the last one.
        r.handle_key(key(KeyCode::Esc));
        r.open_search(Vim, false);
        r.handle_key(ch('y'));
        r.handle_key(key(KeyCode::Esc));
        assert!(r.view().unwrap().search.is_none(), "vim: nothing to put back");
        assert_eq!(r.take_last_search(), Some(("x".into(), false)), "the box's y never became the pattern");
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r), (4, Some((4, 0)), "x".into()), "vim's n goes on with x");
        let mut r = ready(BODY);
        r.open_search(Vim, false);
        typed(&mut r, "x");
        r.handle_key(key(KeyCode::Esc));
        assert!(r.view().unwrap().search.is_none(), "Esc in the view clears the search");
        assert!(r.find_next(false, 1), "vim's n still has the last pattern");
        assert_eq!(landed(&r), (4, Some((4, 0)), "x".into()));
        r.handle_key(key(KeyCode::Esc));
        r.open_search(Vim, false);
        typed(&mut r, "");
        assert_eq!(landed(&r), (0, Some((0, 0)), "x".into()), "and so does an empty /");
        assert_eq!(r.take_note().as_deref(), Some("search hit BOTTOM, continuing at TOP"));
    }

    /// Vim's `do_search()` stores a pattern before it compiles it, so one
    /// outside the engine's subset is the last pattern too, and `n` says
    /// why again, before Esc and after (probed: `/\(` then `n` repeats
    /// E54): never "Pattern not found", and never an older pattern.
    #[test]
    fn an_unsupported_pattern_is_the_last_one_and_n_says_so_again() {
        let mut r = ready("abc\nxyz\nabc");
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "abc");
        r.take_note();
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, r"a\{-}");
        let note = r.take_note().unwrap_or_default();
        assert!(note.starts_with("pattern not supported"), "{note}");
        let cursor = r.view().unwrap().cursor;
        assert!(r.find_next(false, 1));
        assert_eq!(r.take_note().as_deref(), Some(note.as_str()), "n before Esc");
        r.handle_key(key(KeyCode::Esc));
        assert!(r.find_next(false, 1));
        assert_eq!(r.take_note().as_deref(), Some(note.as_str()), "n after Esc, not the older abc");
        assert_eq!(r.view().unwrap().cursor, cursor, "nothing moves");
        assert_eq!(r.take_last_search(), Some((r"a\{-}".to_string(), false)), "for the engine too");
    }

    /// Vim has one last pattern, not one per response: it outlives a new
    /// response landing (a re-send, another request), so `n` finds it there.
    #[test]
    fn the_last_pattern_outlives_a_new_response() {
        let mut r = ready("x\ny\nx");
        r.open_search(SearchSyntax::Vim, true);
        typed(&mut r, "y");
        r.set_state(ResponseState::Ready(Box::new(data("a\ny\nb\ny"))), 1);
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r), (3, Some((3, 0)), "y".into()), "`n` follows the `?`: backward, wrapping");
    }

    /// The app keeps the response's last pattern and the text engine's one
    /// (`App::sync_last_pattern`): a search here is handed over once, and
    /// a pattern searched in a field replaces a committed vim search on
    /// screen, as Vim's highlight shows the last pattern in every window.
    /// A box being typed in, or a search Esc cleared, stay as they are.
    #[test]
    fn the_last_pattern_is_handed_over_and_taken_up() {
        let mut r = ready("x\ny\nx");
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "x");
        assert_eq!(r.take_last_search(), Some(("x".to_string(), false)));
        assert_eq!(r.take_last_search(), None, "once");
        r.adopt_last_search(Some(("y", true)));
        assert_eq!(r.take_last_search(), None, "the engine's is not handed back");
        let s = r.view().unwrap().search.as_ref().unwrap();
        assert_eq!((s.query.as_str(), s.backward, s.matches.len()), ("y", true, 1), "the highlight shows y");
        r.open_search(SearchSyntax::Vim, false);
        r.handle_key(ch('x'));
        r.adopt_last_search(Some(("x", false)));
        let s = r.view().unwrap().search.as_ref().unwrap();
        assert!(s.active && s.input.text() == "x", "the box being typed in stays");
        r.handle_key(key(KeyCode::Esc));
        r.handle_key(key(KeyCode::Esc));
        r.adopt_last_search(Some(("y", false)));
        assert!(r.view().unwrap().search.is_none(), "a search Esc cleared stays cleared");
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r).2, "y", "and `n` takes the pattern up");
    }

    /// A route that runs vim's `n` while the box is still being typed in
    /// (a palette or footer action) commits what was typed, as the ▼ step
    /// does, rather than throwing it away for the old pattern.
    #[test]
    fn n_with_the_box_open_commits_what_was_typed() {
        let mut r = ready("x\ny\nx\ny");
        r.open_search(SearchSyntax::Vim, false);
        typed(&mut r, "x");
        r.open_search(SearchSyntax::Vim, false);
        r.handle_key(ch('y'));
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r), (3, Some((3, 0)), "y".into()));
        assert!(!r.view().unwrap().search.as_ref().unwrap().active);
    }

    /// The user's decision (2026-10-10, replacing plan M5a Deviation 13):
    /// Esc on the response clears its search and highlight, but vim keeps
    /// the last pattern and its direction, as Vim's Esc never forgets them.
    /// Arrows' `n`/▼ after Esc do nothing, as on main.
    #[test]
    fn esc_keeps_the_last_pattern_and_its_direction_in_vim_only() {
        const BODY: &str = "x\ny\nx\ny\nx";
        let mut r = ready(BODY);
        r.view.as_mut().unwrap().cursor = 4;
        r.open_search(SearchSyntax::Vim, true);
        typed(&mut r, "x");
        assert_eq!(r.view().unwrap().cursor, 2);
        r.handle_key(key(KeyCode::Esc));
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r), (0, Some((0, 0)), "x".into()), "n goes on backward, as `?` set it");
        let mut r = ready(BODY);
        r.view.as_mut().unwrap().cursor = 2;
        r.find_word(true, 1);
        assert_eq!(r.view().unwrap().cursor, 0);
        r.handle_key(key(KeyCode::Esc));
        assert!(r.find_next(true, 1));
        assert_eq!(r.view().unwrap().cursor, 2, "`#` set backward, so N goes forward");
        let mut r = ready(BODY);
        r.open_search(SearchSyntax::Literal, false);
        typed(&mut r, "x");
        r.handle_key(key(KeyCode::Esc));
        assert!(!r.step_search(1), "arrows: no search, nothing, as on main");
        assert_eq!(r.view().unwrap().cursor, 2);
    }

    /// The last query is the pattern as Vim's `do_search()` stored it, so an
    /// empty box in the other direction searches the same thing (2026-10-10
    /// probe, Vim 9.1): `?foo\?` stores `foo?`, a literal `?`, and `/fo\?`
    /// stores `fo\?`, Vim's 0-or-1, which an empty `?` keeps.
    #[test]
    fn an_empty_box_reuses_the_pattern_as_vim_stored_it() {
        use SearchSyntax::Vim;
        const BODY: &str = "xfoo\nfoo?\nfox\nzz";
        let mut r = ready(BODY);
        r.view.as_mut().unwrap().cursor = 3;
        r.open_search(Vim, true);
        typed(&mut r, r"foo\?");
        assert_eq!(r.view().unwrap().cursor, 1);
        r.view.as_mut().unwrap().cursor = 0;
        r.open_search(Vim, false);
        typed(&mut r, "");
        assert_eq!(landed(&r), (1, Some((1, 0)), "foo?".into()), "a literal ?, not xfoo's foo");
        r.open_search(Vim, false);
        typed(&mut r, r"fo\?");
        r.view.as_mut().unwrap().cursor = 3;
        r.open_search(Vim, true);
        typed(&mut r, "");
        assert_eq!(landed(&r), (2, Some((2, 0)), r"fo\?".into()), "still 0-or-1: fox's fo");
    }

    /// Amendment A5.4 (the user's decision): arrows' `n`/`N` and the ▼/▲
    /// buttons with no search do nothing, as on main; vim's `n` says E35.
    #[test]
    fn n_with_no_search_is_silent_in_arrows_and_e35_in_vim() {
        let mut r = ready(PROBE_JSON);
        assert!(!r.step_search(1));
        assert!(!r.step_search(-1));
        assert_eq!(r.take_note(), None, "arrows: nothing, no note");
        assert_eq!(r.view().unwrap().cursor, 0);
        assert!(!r.find_next(true, 2));
        assert_eq!(r.take_note().as_deref(), Some("No previous regular expression"), "vim");
    }

    /// Review Focus 1, as Vim 9.1 does it (probes X15, X16, R1–R4, R6–R8,
    /// R10; the controller's ruling over the plan's premise): when a closed
    /// fold holds the cursor's line, hidden in it or on its own collapsed
    /// row, a forward search starts after the outermost closed fold's
    /// closing line and a backward one at its opening line, so the matches
    /// it hides are skipped and it stays closed. `*` and `#` still take
    /// their word from the hidden line. Both profiles.
    #[test]
    fn a_search_from_a_closed_fold_starts_outside_it() {
        use crate::action::FoldOp;
        // 0 {, 1 "a": {, 2 "b": 1, 3 }, 4 "c": 2, 5 }
        const FOLD: &str = r#"{"a": {"b": 1}, "c": 2}"#;
        // 0 {, 1 "a": [, 2 "b", 3 "b", 4 ], 5 "b": 3, 6 }
        const ARR: &str = r#"{"a": ["b", "b"], "b": 3}"#;
        // 0 {, 1 "b": 0, 2 "a": [, 3 "b", 4 "b", 5 ], 6 "c": 3, 7 }
        const ARR2: &str = r#"{"b": 0, "a": ["b", "b"], "c": 3}"#;
        let folded = |body: &str, line: usize| {
            let mut r = ready(body);
            render(&mut r);
            r.view.as_mut().unwrap().cursor = line;
            assert!(r.fold(FoldOp::Close, 1));
            r
        };
        let closed = |r: &Response, line: usize| r.view().unwrap().active_tree().unwrap().closed_fold(line);
        let searched = |mut r: Response, syntax, backward, text: &str| {
            r.open_search(syntax, backward);
            typed(&mut r, text);
            r
        };
        let r = searched(folded(FOLD, 2), SearchSyntax::Vim, false, "}");
        assert_eq!(landed(&r).1, Some((5, 0)), "R1: not the fold's own closing line");
        assert_eq!(closed(&r, 2), Some((1, 3)), "R1: the fold stays closed");
        let r = searched(folded(FOLD, 2), SearchSyntax::Literal, false, "}");
        assert_eq!(landed(&r).1, Some((5, 0)), "R1 in arrows too");
        let r = searched(folded(FOLD, 2), SearchSyntax::Vim, true, "{");
        assert_eq!(landed(&r).1, Some((0, 0)), "R2: back from the fold's opening line's column 0");
        let r = searched(folded(FOLD, 2), SearchSyntax::Vim, false, "c");
        assert_eq!(landed(&r).1, Some((4, 3)), "R8 (X15)");
        let mut r = folded(ARR, 2);
        assert_eq!(r.view().unwrap().cursor, 1);
        assert!(r.find_word(false, 1));
        assert_eq!(landed(&r), (2, Some((5, 3)), r"\<b\>".into()), "R3: the word from the hidden line, found past the fold");
        assert_eq!(closed(&r, 2), Some((1, 4)));
        let mut r = folded(ARR2, 4);
        assert!(r.find_word(true, 1));
        assert_eq!(landed(&r), (1, Some((1, 3)), r"\<b\>".into()), "R4: # from the fold's opening line");
        let r = searched(folded(ARR, 1), SearchSyntax::Vim, false, "b");
        assert_eq!(landed(&r).1, Some((5, 3)), "R7: on the fold's own collapsed row");
        let mut r = searched(ready(ARR), SearchSyntax::Vim, false, "b");
        assert_eq!(landed(&r).1, Some((2, 5)));
        assert!(r.fold(FoldOp::Close, 1));
        assert!(r.find_next(false, 1));
        assert_eq!(landed(&r).1, Some((5, 3)), "R6: n skips the fold the match closed into");
        let mut r = searched(ready(ARR2), SearchSyntax::Vim, false, "b");
        r.find_next(false, 2);
        assert_eq!(landed(&r).1, Some((4, 5)));
        assert!(r.fold(FoldOp::Close, 1));
        assert!(r.find_next(true, 1));
        assert_eq!(landed(&r).1, Some((1, 3)), "R10: N skips the hidden b on line 3");
    }

    #[test]
    fn search_works_in_raw_view_too() {
        let mut r = ready("plain text with a needle inside");
        r.open_search(SearchSyntax::Literal, false);
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

    /// The view strip is switched by the one tab chord every pane shares
    /// (alt+←/→ → `Action::CycleTabs`, resolved by `app.rs`), not by a
    /// pane-local letter: `t` is unclaimed here.
    #[test]
    fn t_is_not_a_response_key() {
        let mut r = ready(r#"{"a": 1}"#);
        assert_eq!(r.handle_key(ch('t')), None);
        assert_eq!(r.view().unwrap().mode, ViewMode::Pretty);
    }

    /// The strip `App::tab_target` walks: Pretty, Raw, Headers, with no
    /// Pretty when there is no tree to show, and no strip at all with no
    /// ready view.
    #[test]
    fn tab_modes_skip_pretty_without_a_tree() {
        let r = ready(r#"{"a": 1}"#);
        assert_eq!(r.tab_modes(), Some(vec![ViewMode::Pretty, ViewMode::Raw, ViewMode::Headers]));
        let r = ready("not json");
        assert_eq!(r.tab_modes(), Some(vec![ViewMode::Raw, ViewMode::Headers]));
        assert_eq!(Response::default().tab_modes(), None);
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
            pointer: None,
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
            pointer: None,
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
            pointer: None,
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
            pointer: None,
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
                pointer: None,
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
            pointer: None,
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

    /// jq only ever filters the tree, so the toolbar's filter icon, the
    /// bar, and the caret in it all belong to the `Pretty` tab. Switching
    /// away takes the three of them; switching back brings them, and the
    /// filter they were holding, straight back.
    #[test]
    fn the_jq_button_and_bar_belong_to_the_pretty_tab() {
        let mut r = ready(r#"{"a": 1}"#);
        assert!(r.open_jq(), "the tree tab has jq");
        r.set_jq_text(".a");
        assert!(r.jq_focused() && r.jq_open());
        let hits = render_hits(&mut r);
        assert!(hits.rect_of(&crate::hit::Hit::ResponseJqButton).is_some());
        assert!(hits.rect_of(&crate::hit::Hit::ResponseJqBar).is_some());

        for mode in [ViewMode::Raw, ViewMode::Headers] {
            r.set_view_mode(mode);
            assert!(!r.jq_available(), "{mode:?} has nothing for jq to filter");
            assert!(!r.jq_open(), "{mode:?} hides the bar");
            assert!(!r.jq_focused(), "{mode:?} takes the caret out of it");
            assert!(r.jq_enabled(), "{mode:?} keeps the filter switched on");
            let hits = render_hits(&mut r);
            assert!(
                hits.rect_of(&crate::hit::Hit::ResponseJqButton).is_none(),
                "no filter icon on {mode:?}"
            );
            assert!(
                hits.rect_of(&crate::hit::Hit::ResponseJqBar).is_none(),
                "no filter bar on {mode:?}"
            );
            assert!(!r.open_jq(), "and no way to open one from {mode:?}");
        }

        r.set_view_mode(ViewMode::Pretty);
        assert!(r.jq_available() && r.jq_open(), "the bar comes back");
        assert_eq!(r.jq_text(), ".a", "still holding the filter it had");
        let hits = render_hits(&mut r);
        assert!(hits.rect_of(&crate::hit::Hit::ResponseJqButton).is_some());
    }

    /// The refusal has to say the right thing: on `Raw`/`Headers` the body
    /// may well be JSON, it just isn't the view a filter applies to.
    #[test]
    fn the_blocked_reason_tells_a_wrong_tab_from_a_non_json_body() {
        let mut r = ready(r#"{"a": 1}"#);
        assert_eq!(r.jq_blocked_reason(), None);
        r.set_view_mode(ViewMode::Raw);
        assert_eq!(r.jq_blocked_reason(), Some("jq filters the Pretty tab"));

        let r = ready("<html>hi</html>");
        assert_eq!(r.jq_blocked_reason(), Some("The response is not JSON"));
        assert_eq!(
            Response::default().jq_blocked_reason(),
            Some("The response is not JSON"),
            "no response at all reads as nothing to filter"
        );
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
        let last = || KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL);
        r.handle_key(last());
        assert_eq!(r.view().unwrap().cursor, 3, "last pretty line");
        cycle(&mut r);
        assert_eq!(
            r.view().unwrap().cursor,
            0,
            "switching views restarts at the top"
        );
        r.handle_key(last());
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
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".");
        assert_eq!(r.jq_ghost(), Some("data"));
        type_jq(&mut r, ".data.");
        assert_eq!(r.jq_ghost(), Some("items"));
        type_jq(&mut r, ".data.items[] | .s");
        assert_eq!(r.jq_ghost(), Some("tatus"));
        type_jq(&mut r, ".data.items[] | select(.st");
        assert_eq!(
            r.jq_ghost(),
            Some("atus"),
            "inside select the dot is the item"
        );
        type_jq(&mut r, ".data.items[] | .zz");
        assert_eq!(r.jq_ghost(), None);
    }

    #[test]
    fn a_word_ghosts_a_builtin_even_with_no_document() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
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
        r.set_jq_tab(JqTab::Cycle);
        type_jq_unfocusable(&mut r, "leng");
        assert_eq!(r.jq_ghost(), Some("th"), "builtins need no body");
        type_jq_unfocusable(&mut r, ".d");
        assert_eq!(r.jq_ghost(), None, "no document, no keys");
    }

    #[test]
    fn a_finished_token_inside_a_bracket_ghosts_its_closer() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items | sort_by(.i");
        assert_eq!(r.jq_ghost(), Some("d"), "a key still extends: the key wins");
        type_jq(&mut r, ".data.items | sort_by(.id");
        assert_eq!(r.jq_ghost(), Some(")"), "nothing extends `id`: the closer");
        type_jq(&mut r, ".data.items | map({s: .status");
        assert_eq!(r.jq_ghost(), Some("}"));
        type_jq(&mut r, ".data.items | map(select(.id == 1");
        assert_eq!(r.jq_ghost(), Some(")"), "no key context at all: still the closer");
        type_jq(&mut r, ".data.items | sort_by(");
        assert_eq!(r.jq_ghost(), None, "nothing typed inside: nothing offered");
        type_jq(&mut r, ".data.items | sort_by(.id)");
        assert_eq!(r.jq_ghost(), None, "closed: nothing left to offer");
        type_jq(&mut r, ".data.items | leng");
        assert_eq!(r.jq_ghost(), Some("th"), "a builtin still wins over a closer");
    }

    #[test]
    fn a_word_inside_braces_ghosts_the_shorthand_key_then_the_closer() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items | map({s");
        assert_eq!(r.jq_ghost(), Some("tatus"), "`{{status}}` is `{{status: .status}}`");
        bar_key(&mut r, key(KeyCode::Right));
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_text(), ".data.items | map({status");
        assert_eq!(r.jq_ghost(), Some("}"), "the key is complete: the closer");
        type_jq(&mut r, ".data.items | map({status, i");
        assert_eq!(r.jq_ghost(), Some("d"), "after a comma the next key");
        type_jq(&mut r, ".data.items | map({status: s");
        assert_eq!(
            r.jq_ghost(),
            Some("calars"),
            "after a colon a word is a builtin (`scalars`), not a key"
        );
    }

    #[test]
    fn a_shorthand_key_is_a_whole_word_chip_in_menu_mode() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items | map({s");
        assert_eq!(
            r.jq_bar().menu_row(),
            Some(vec![("status".to_string(), false)])
        );
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.items | map({status");
    }

    #[test]
    fn accepting_the_closer_types_it_and_the_offer_moves_on() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items | map({s: .status");
        assert_eq!(r.jq_ghost(), Some("}"));
        bar_key(&mut r, key(KeyCode::Right));
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_text(), ".data.items | map({s: .status}");
        assert_eq!(r.jq_ghost(), Some(")"), "the next unclosed opener is offered");
        bar_key(&mut r, key(KeyCode::Right));
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_text(), ".data.items | map({s: .status})");
        assert_eq!(r.jq_ghost(), None);
    }

    #[test]
    fn the_closer_is_a_ghost_in_menu_mode_too_and_tab_or_right_takes_it() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items | sort_by(.id");
        assert_eq!(r.jq_bar().menu_row(), None, "one right answer needs no row");
        assert_eq!(r.jq_ghost(), Some(")"), "it is a ghost, as in cycle mode");
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.items | sort_by(.id)");
        assert!(r.jq_bar().menu().is_none());
        type_jq(&mut r, ".data.items | map({s: .status");
        assert_eq!(r.jq_ghost(), Some("}"));
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".data.items | map({s: .status}", "Right accepts a closer ghost in menu mode");
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        assert_eq!(r.jq_ghost(), Some(")"));
        bar_key(&mut r, key(KeyCode::End));
        assert_eq!(r.jq_text(), ".data.items | map({s: .status})", "End too");
    }

    #[test]
    fn a_lone_key_candidate_is_still_a_chip_in_menu_mode() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .st");
        assert_eq!(r.jq_ghost(), None, "a guess is never pushed as a ghost");
        assert_eq!(
            r.jq_bar().menu_row(),
            Some(vec![("status".to_string(), false)])
        );
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".data.items[] | .st", "Right is a plain caret move on a chip row");
    }

    #[test]
    fn the_muted_note_shows_under_a_closer_ghost_in_menu_mode() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items | sort_by(.id");
        let theme = Theme::dark();
        let (line, color) = error_row(&mut r, "unclosed (");
        assert_eq!(line.trim(), "unclosed (");
        assert_eq!(color, theme.text_muted);
        let text = render(&mut r);
        assert!(text.contains("sort_by(.id)"), "the ghost `)` is on the bar row: {text}");
    }

    #[test]
    fn a_closer_is_offered_after_keys_land_from_the_pool_with_nothing_extending() {
        let body = big_json();
        let mut r = ready_gen(&body, 7);
        r.set_jq_tab(JqTab::Cycle);
        // A big body arrives unparsed, and the bar refuses focus with no
        // tree to filter; land the parse as the app would.
        r.attach_tree(7, crate::components::json_tree::JsonTree::parse(&body));
        assert!(r.set_jq_focus(true));
        r.jq_bar_mut().input = LineInput::new("map(.a");
        r.apply_jq("map(.a", 0);
        let req = r
            .refresh_jq_completion(0)
            .expect("big body: keys come from the pool");
        assert_eq!(r.jq_ghost(), None, "nothing offered until the keys land");
        let landed =
            r.attach_jq_completion(7, req.seq, req.input_expr.clone(), vec!["a".into()], None);
        assert!(landed, "the ghost changed");
        assert_eq!(r.jq_ghost(), Some(")"), "`a` extends nothing: the closer");
    }

    #[test]
    fn the_ghost_hides_off_the_end_of_the_text_and_when_unfocused() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
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
        r.set_jq_tab(JqTab::Cycle);
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
        let big = format!(r#"{{"pad": "{}", "n": 7}}"#, "x".repeat(SYNC_PRETTY_BYTES));
        let mut r = ready_gen(&big, 3);
        r.set_jq_tab(JqTab::Cycle);
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
        // (A stale generation is `an_attach_for_a_gone_generation_still_
        // clears_its_pending_fetch`: it drops the keys *and* the fetch.)
        assert!(
            !r.attach_jq_completion(3, creq.seq + 9, ".".into(), vec!["pad".into()], None),
            "stale sequence"
        );
        assert!(r.attach_jq_completion(
            3,
            creq.seq,
            ".".into(),
            vec!["pad".into(), "n".into()],
            None
        ));
        assert_eq!(r.jq_ghost(), Some("pad"));
    }

    /// The big-body setup `a_big_body_fetches_keys_on_the_pool` uses: a
    /// response over the sync limit with its tree and document attached,
    /// the bar focused with `text` typed and applied.
    fn big_body_bar(body: &str, generation: u64, text: &str) -> Response {
        let mut r = ready_gen(body, generation);
        // These tests read the ghost, which only cycle mode draws.
        r.set_jq_tab(JqTab::Cycle);
        r.attach_tree(
            generation,
            crate::components::json_tree::JsonTree::parse(body),
        );
        r.set_jq_focus(true);
        r.jq_bar_mut().input = LineInput::new(text);
        let req = r.apply_jq(text, SYNC_PRETTY_BYTES).expect("background run");
        let doc = postui_core::jq::JqDocument::parse(body).unwrap();
        let out = JqRunOutput::from_outputs(Some(doc), vec![body.to_string()]);
        r.attach_jq_result(generation, req.run, Ok(out));
        r
    }

    /// A big body over the sync limit whose root keys are `first` and `n`.
    fn big_body(first: &str) -> String {
        format!(
            r#"{{"{first}": "{}", "n": 7}}"#,
            "x".repeat(SYNC_PRETTY_BYTES)
        )
    }

    #[test]
    fn a_new_response_re_derives_the_completion_from_the_new_body() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".");
        assert_eq!(r.jq_ghost(), Some("data"));
        r.set_state(
            ResponseState::Ready(Box::new(data(r#"{"other":{"x":1}}"#))),
            1,
        );
        r.set_jq_focus(true);
        assert!(
            r.refresh_jq_completion(SYNC_PRETTY_BYTES).is_none(),
            "small bodies fetch inline"
        );
        assert_eq!(
            r.jq_ghost(),
            Some("other"),
            "the ghost is the new body's key, not the old one's"
        );
    }

    #[test]
    fn a_new_response_clears_a_pending_key_fetch_so_the_next_one_is_issued() {
        let big = big_body("pad");
        let mut r = big_body_bar(&big, 3, ".");
        let creq = r
            .refresh_jq_completion(SYNC_PRETTY_BYTES)
            .expect("a big body fetches keys on the pool");
        assert!(r.jq_bar().completion.pending().is_some());

        let next = big_body("other");
        r.set_state(ResponseState::Ready(Box::new(data(&next))), 4);
        assert!(
            r.jq_bar().completion.pending().is_none(),
            "the in-flight fetch goes with the body it was for"
        );
        r.attach_tree(4, crate::components::json_tree::JsonTree::parse(&next));
        r.set_jq_focus(true);
        let req = r.apply_jq(".", SYNC_PRETTY_BYTES).expect("background run");
        let doc = postui_core::jq::JqDocument::parse(&next).unwrap();
        let out = JqRunOutput::from_outputs(Some(doc), vec![next.clone()]);
        r.attach_jq_result(4, req.run, Ok(out));
        let creq2 = r
            .refresh_jq_completion(SYNC_PRETTY_BYTES)
            .expect("the new body's keys are fetched afresh");
        assert_eq!(creq2.generation, 4);
        assert!(creq2.seq > creq.seq, "a new sequence number");
        assert!(r.attach_jq_completion(4, creq2.seq, ".".into(), vec!["other".into()], None));
        assert_eq!(r.jq_ghost(), Some("other"));
    }

    #[test]
    fn an_attach_for_a_gone_generation_still_clears_its_pending_fetch() {
        let big = big_body("pad");
        let mut r = big_body_bar(&big, 3, ".");
        let creq = r
            .refresh_jq_completion(SYNC_PRETTY_BYTES)
            .expect("a big body fetches keys on the pool");
        // The view moved on under the in-flight fetch. (A new response
        // landing clears `pending` itself; this pins the attach side of
        // the rule on its own.)
        r.view.as_mut().unwrap().generation = 4;
        assert!(
            !r.attach_jq_completion(3, creq.seq, ".".into(), vec!["pad".into()], None),
            "keys for a view that is gone are dropped"
        );
        assert_eq!(r.jq_ghost(), None);
        assert!(
            r.jq_bar().completion.pending().is_none(),
            "and the fetch is not left pending forever"
        );
        assert!(
            r.refresh_jq_completion(SYNC_PRETTY_BYTES).is_some(),
            "so the expression can be fetched again"
        );
    }

    #[test]
    fn a_stale_sequence_attach_leaves_the_cache_alone() {
        let big = big_body("pad");
        let mut r = big_body_bar(&big, 3, ".");
        let creq = r
            .refresh_jq_completion(SYNC_PRETTY_BYTES)
            .expect("a big body fetches keys on the pool");
        assert!(!r.attach_jq_completion(
            3,
            creq.seq + 9,
            ".".into(),
            vec!["superseded".into()],
            None
        ));
        assert_eq!(r.jq_ghost(), None, "a superseded fetch shows nothing");
        assert!(
            r.attach_jq_completion(
                3,
                creq.seq,
                ".".into(),
                vec!["pad".into(), "n".into()],
                None
            ),
            "the fetch that is still wanted lands"
        );
        assert_eq!(r.jq_ghost(), Some("pad"), "not the superseded keys");
    }

    #[test]
    fn an_attach_whose_context_moved_on_caches_without_showing() {
        let big = big_body("pad");
        let mut r = big_body_bar(&big, 3, ".");
        let creq = r
            .refresh_jq_completion(SYNC_PRETTY_BYTES)
            .expect("a big body fetches keys on the pool");
        // The caret leaves the end of the text while the fetch is in flight.
        r.jq_bar_mut().input.set_cursor(0);
        assert!(r.refresh_jq_completion(SYNC_PRETTY_BYTES).is_none());
        assert!(
            !r.attach_jq_completion(
                3,
                creq.seq,
                ".".into(),
                vec!["pad".into(), "n".into()],
                None
            ),
            "nothing is offered at the caret, so the ghost does not change"
        );
        assert_eq!(r.jq_ghost(), None);
        // Back at the end: the keys were cached, so no second fetch.
        r.jq_bar_mut().input.set_cursor(1);
        assert!(
            r.refresh_jq_completion(SYNC_PRETTY_BYTES).is_none(),
            "the fetched keys are cached, not fetched again"
        );
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
        r.set_jq_tab(JqTab::Cycle);
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
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_ghost(), Some("status"));
        bar_key(&mut r, ch('i'));
        assert_eq!(
            r.jq_ghost(),
            Some("d"),
            "a new partial starts at the first candidate"
        );
        bar_key(&mut r, key(KeyCode::End));
        assert_eq!(r.jq_text(), ".data.items[] | .id");
    }

    #[test]
    fn tab_in_menu_mode_completes_and_opens_a_row_that_tab_steps_through() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .");
        assert!(!r.jq_menu_open());
        assert_eq!(r.jq_ghost(), None, "menu mode has no ghost");
        assert_eq!(
            r.jq_bar().menu_row(),
            Some(vec![
                ("id".to_string(), false),
                ("status".to_string(), false)
            ]),
            "the row is on show while typing, nothing selected"
        );
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(
            r.jq_text(),
            ".data.items[] | .id",
            "the first candidate lands"
        );
        assert!(r.jq_menu_open(), "…and the row is entered");
        let chips = |r: &Response| {
            r.jq_bar()
                .menu()
                .unwrap()
                .chips()
                .map(|(l, sel)| (l.to_string(), sel))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            chips(&r),
            vec![("id".to_string(), true), ("status".to_string(), false)],
            "chips carry the whole token, not the ghost"
        );
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(
            r.jq_text(),
            ".data.items[] | .status",
            "rebuilt from the base"
        );
        assert_eq!(r.view().unwrap().view_text(), "\"active\"\n\"off\"");
        assert!(chips(&r)[1].1);
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.items[] | .id", "wraps");
        bar_key(&mut r, shift(KeyCode::BackTab));
        assert_eq!(
            r.jq_text(),
            ".data.items[] | .status",
            "shift+tab goes back"
        );
        // Any other key keeps the selection, closes the row, and is typed.
        bar_key(&mut r, ch(' '));
        assert!(!r.jq_menu_open());
        assert_eq!(r.jq_text(), ".data.items[] | .status ");
    }

    #[test]
    fn menu_mode_with_a_lone_candidate_just_accepts() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.i");
        assert_eq!(
            r.jq_bar().menu_row(),
            Some(vec![("items".to_string(), false)]),
            "a lone candidate is still previewed"
        );
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.items");
        assert!(!r.jq_menu_open(), "nothing else to choose from");
    }

    #[test]
    fn shift_tab_on_a_ghost_in_menu_mode_opens_the_row_at_the_last_candidate() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, shift(KeyCode::BackTab));
        assert_eq!(r.jq_text(), ".data.items[] | .status");
        assert_eq!(r.jq_bar().menu().unwrap().index(), 1);
    }

    #[test]
    fn right_is_just_a_caret_move_in_menu_mode_there_is_no_ghost_to_accept() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".data.items[] | .");
        assert!(!r.jq_menu_open());
    }

    #[test]
    fn the_offered_row_narrows_as_the_user_types_and_goes_with_the_caret() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, ch('s'));
        assert_eq!(
            r.jq_bar().menu_row(),
            Some(vec![("status".to_string(), false)])
        );
        bar_key(&mut r, ch('x'));
        assert_eq!(r.jq_bar().menu_row(), None, "nothing matches");
        bar_key(&mut r, key(KeyCode::Backspace));
        assert!(r.jq_bar().menu_row().is_some());
        bar_key(&mut r, key(KeyCode::Left));
        assert_eq!(r.jq_bar().menu_row(), None, "only at the end of the text");
    }

    #[test]
    fn esc_on_an_entered_row_unpicks_it_and_stays_and_a_second_esc_leaves() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        r.set_jq_text(".data");
        assert!(r.set_jq_focus(true));
        for c in ".items[] | .".chars() {
            bar_key(&mut r, ch(c));
        }
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.items[] | .status");
        bar_key(&mut r, key(KeyCode::Esc));
        assert_eq!(
            r.jq_text(),
            ".data.items[] | .",
            "the pick is undone, the rest of the edit kept"
        );
        assert!(!r.jq_menu_open(), "the row is left…");
        assert!(r.jq_menu_offered(), "…and shows again, unentered");
        assert!(r.jq_focused(), "the caret stays");
        bar_key(&mut r, key(KeyCode::Esc));
        assert_eq!(
            r.jq_text(),
            ".data.items[] | .",
            "the second Esc leaves the bar, the text kept"
        );
        assert!(!r.jq_focused());
    }

    #[test]
    fn enter_on_an_entered_row_confirms_the_chip_and_a_second_enter_leaves() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Enter));
        assert_eq!(r.jq_text(), ".data.items[] | .status");
        assert!(!r.jq_menu_open(), "the row is left…");
        assert!(r.jq_focused(), "…but the caret stays in the bar");
        bar_key(&mut r, key(KeyCode::Enter));
        assert!(!r.jq_focused(), "nothing selected: Enter leaves");
        assert_eq!(r.jq_text(), ".data.items[] | .status");
    }

    #[test]
    fn enter_on_the_unentered_row_leaves_with_what_was_typed() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .");
        assert!(r.jq_menu_offered() && !r.jq_menu_open());
        bar_key(&mut r, key(KeyCode::Enter));
        assert_eq!(r.jq_text(), ".data.items[] | .", "nothing was picked");
        assert!(!r.jq_focused());
    }

    #[test]
    fn enter_on_a_ghost_commits_the_typed_text_not_the_ghost() {
        let mut r = ready(r#"{"id": 1, "identifier": 2}"#);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".id");
        assert_eq!(r.jq_ghost(), Some("entifier"));
        bar_key(&mut r, key(KeyCode::Enter));
        assert_eq!(r.jq_text(), ".id", "a ghost is an offer, not text");
        assert!(!r.jq_focused());
        assert_eq!(r.view().unwrap().view_text(), "1");
    }

    #[test]
    fn the_focused_bar_gets_unbound_editing_combos_ctrl_arrows_and_select_all() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.items | length");
        bar_key(&mut r, KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(
            r.jq_bar().input.cursor(),
            14,
            "word-left lands before `length`"
        );
        bar_key(&mut r, KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(r.jq_bar().input.cursor(), 12, "then before `|`");
        bar_key(&mut r, KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(r.jq_bar().input.cursor(), 13);
        bar_key(
            &mut r,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        );
        assert_eq!(r.jq_bar().input.selection(), Some((0, 20)), "select-all");
        // Unfocused, the pane keeps handing combos to the keymap.
        r.set_jq_focus(false);
        assert_eq!(
            r.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn ctrl_backspace_climbs_the_path_one_segment_at_a_time() {
        let mut r = ready(ITEMS);
        let ctrl_bs = KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL);
        type_jq(&mut r, ".data.items[] | .status");
        bar_key(&mut r, ctrl_bs);
        assert_eq!(r.jq_text(), ".data.items[] | .");
        bar_key(&mut r, ctrl_bs);
        assert_eq!(r.jq_text(), ".data.items[] | ", "a bare dot goes too");
        bar_key(&mut r, ctrl_bs);
        assert_eq!(
            r.jq_text(),
            ".data.items[] ",
            "outside a path: the input's own word rule, one run (`| `) at a time"
        );
        bar_key(&mut r, ctrl_bs);
        assert_eq!(
            r.jq_text(),
            ".data.",
            "the space is skipped, and `items[]` is one segment — back to the dot, which stays"
        );
        bar_key(&mut r, ctrl_bs);
        assert_eq!(r.jq_text(), ".", "the trailing dot goes with `data`");
        bar_key(&mut r, ctrl_bs);
        assert_eq!(r.jq_text(), "");
        bar_key(&mut r, ctrl_bs);
        assert_eq!(r.jq_text(), "", "nothing left: no-op");
        assert!(r.take_jq_edited());
    }

    #[test]
    fn alt_backspace_and_the_legacy_ctrl_h_byte_delete_the_same_segment() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, "map(select(.data.items))");
        r.jq_bar_mut().input.set_cursor(22); // after `.items`
        bar_key(&mut r, KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(r.jq_text(), "map(select(.data.))");
        bar_key(
            &mut r,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );
        assert_eq!(r.jq_text(), "map(select(.))", "`(` cuts the token");
        // A builtin is a plain word: the input's own rule, whole word.
        type_jq(&mut r, ".data.items | length");
        bar_key(&mut r, KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(r.jq_text(), ".data.items | ");
    }

    #[test]
    fn ctrl_backspace_treats_a_quoted_key_as_one_segment() {
        let mut r = ready(r#"{"my key.v2": 1}"#);
        let ctrl_bs = KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL);
        type_jq(&mut r, r#".data."my key.v2" | length"#);
        bar_key(&mut r, ctrl_bs);
        assert_eq!(r.jq_text(), r#".data."my key.v2" | "#, "a plain word first");
        bar_key(&mut r, ctrl_bs);
        assert_eq!(r.jq_text(), r#".data."my key.v2" "#, "then the `| ` run");
        bar_key(&mut r, ctrl_bs);
        assert_eq!(
            r.jq_text(),
            ".data.",
            "the quoted key is one segment: its space and its dot are not cuts"
        );
        // A matrix of quoted shapes, each from the caret at the end.
        let climb = |text: &str| -> Vec<String> {
            let mut r = ready(ITEMS);
            type_jq(&mut r, text);
            let mut steps = Vec::new();
            for _ in 0..3 {
                bar_key(&mut r, ctrl_bs);
                steps.push(r.jq_text().to_string());
            }
            steps
        };
        assert_eq!(climb(r#"."a.b""#), [".", "", ""]);
        assert_eq!(climb(r#".x."a.b""#), [".x.", ".", ""]);
        assert_eq!(climb(r#".x."a b""#), [".x.", ".", ""]);
        assert_eq!(climb(r#".x."a|b.c""#), [".x.", ".", ""]);
        assert_eq!(
            climb(r#".x[0]."a.b""#),
            [".x[0].", ".", ""],
            "a subscript stays with its segment"
        );
        assert_eq!(climb(r#".x."a.b"[0]"#), [".x.", ".", ""]);
        assert_eq!(climb(".a.b[0].c[]"), [".a.b[0].", ".a.", "."]);
    }

    #[test]
    fn ctrl_backspace_with_a_selection_deletes_the_selection() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data.items");
        r.jq_bar_mut().input.select_all();
        bar_key(
            &mut r,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
        );
        assert_eq!(r.jq_text(), "");
    }

    #[test]
    fn down_leaves_the_bar_like_the_url_field() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data");
        bar_key(&mut r, key(KeyCode::Down));
        assert!(!r.jq_focused());
        assert_eq!(r.jq_text(), ".data");
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".data.items[] | .");
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Down));
        assert!(!r.jq_focused() && !r.jq_menu_open());
        assert_eq!(r.jq_text(), ".data.items[] | .id", "the chip is kept");
    }

    #[test]
    fn the_row_rewrites_a_quoted_key_from_the_base_each_step() {
        let mut r = ready(r#"{"my key": 1, "myth": 2}"#);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".my");
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".\"my key\"");
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".myth", "the rewrite does not compound");
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".\"my key\"");
    }

    #[test]
    fn the_menu_row_draws_chips_under_the_bar_and_slides_to_show_the_selected_one() {
        let body = r#"{"alpha":1,"bravo":2,"charlie":3,"delta":4,"echo":5}"#;
        let mut r = ready(body);
        r.set_jq_tab(JqTab::Menu);
        type_jq(&mut r, ".");
        bar_key(&mut r, key(KeyCode::Tab));
        // 24 columns: the pane inset takes 2 and the row's `jq ` indent
        // 3, leaving 19 cells. Chips are ` alpha `, ` bravo `, ` charlie `…
        // one column apart (15 cells for the first two, 25 with the
        // third), so the first two fit and `charlie` does not.
        // Chips are padded (` alpha `), which tells them apart from the
        // bar's own `.alpha`.
        let text = render_sized(&mut r, 24, 20);
        let row = text
            .lines()
            .find(|l| l.contains(" alpha "))
            .unwrap_or_else(|| panic!("no menu row: {text}"));
        assert!(row.contains(" bravo "), "{row}");
        assert!(!row.contains("charlie"), "clipped at the edge: {row}");
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".charlie");
        let text = render_sized(&mut r, 24, 20);
        let row = text
            .lines()
            .find(|l| l.contains(" charlie "))
            .unwrap_or_else(|| panic!("selected chip not whole in view: {text}"));
        assert!(!row.contains("alpha"), "slid past the first chip: {row}");
        // Stepping back: the row holds still while the selection walks
        // left inside it, and slides left only once it reaches the edge.
        bar_key(&mut r, shift(KeyCode::BackTab));
        assert_eq!(r.jq_text(), ".bravo");
        let text = render_sized(&mut r, 24, 20);
        let row = text
            .lines()
            .find(|l| l.contains(" bravo "))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(
            row.contains(" charlie "),
            "the row did not move for a chip still in view: {row}"
        );
        assert!(!row.contains(" alpha "), "{row}");
        bar_key(&mut r, shift(KeyCode::BackTab));
        assert_eq!(r.jq_text(), ".alpha");
        let text = render_sized(&mut r, 24, 20);
        let row = text
            .lines()
            .find(|l| l.contains(" alpha "))
            .unwrap_or_else(|| panic!("slid left to show the chip: {text}"));
        assert!(row.contains(" bravo "), "{row}");
        assert!(!row.contains("charlie"), "just far enough: {row}");
        // Back to the first by wrapping forward: the row snaps home.
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Tab));
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".alpha");
        let text = render_sized(&mut r, 24, 20);
        assert!(
            text.lines()
                .any(|l| l.contains(" alpha ") && l.contains(" bravo ")),
            "{text}"
        );
        // The selected chip is filled with the selection color.
        let (_, buf) = render_buf(&mut r);
        let theme = Theme::dark();
        let filled = (0..buf.area.height).any(|y| {
            (0..buf.area.width)
                .any(|x| buf[(x, y)].symbol() == "a" && buf[(x, y)].bg == theme.selection)
        });
        assert!(filled, "selected chip uses the selection fill");
    }

    #[test]
    fn accepting_a_quoted_key_rewrites_the_token() {
        let mut r = ready(r#"{"my key": 1, "myth": 2}"#);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".my");
        assert_eq!(r.jq_ghost(), Some("\"my key\""));
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_ghost(), Some("th"));
        bar_key(&mut r, shift(KeyCode::BackTab));
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".\"my key\"");
        assert_eq!(r.view().unwrap().view_text(), "1");
        assert_eq!(
            r.jq_bar().input.cursor(),
            9,
            "caret after the closing quote"
        );
    }

    #[test]
    fn accepting_a_builtin_leaves_the_caret_inside_its_parens() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items[] | sel");
        assert_eq!(r.jq_ghost(), Some("ect("));
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(r.jq_text(), ".data.items[] | select(");
        assert!(
            r.jq_bar().error.is_some(),
            "an unfinished filter is an error, as when typed"
        );
    }

    #[test]
    fn without_a_ghost_tab_and_right_behave_as_before() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.zz");
        assert_eq!(r.jq_ghost(), None);
        bar_key(&mut r, key(KeyCode::Tab));
        assert_eq!(r.jq_text(), ".data.zz");
        r.jq_bar_mut().input.set_cursor(2);
        bar_key(&mut r, key(KeyCode::Right));
        assert_eq!(
            r.jq_bar().input.cursor(),
            3,
            "Right moves the caret mid-text"
        );
        bar_key(&mut r, shift(KeyCode::Right));
        assert!(
            r.jq_bar().input.selection().is_some(),
            "shift+Right still selects"
        );
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
        r.apply_jq(".data.items )", SYNC_PRETTY_BYTES);
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
        r.apply_jq(".a )", SYNC_PRETTY_BYTES);
        let err = r.jq_bar().error.clone().expect("syntax error recorded");
        assert!(err.span().is_some());
        // A re-send lands a non-JSON body: jq has nothing to run against.
        r.set_state(ResponseState::Ready(Box::new(data("plain text"))), 1);
        assert!(!r.jq_available());
        r.apply_jq(".a )", SYNC_PRETTY_BYTES);
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
        assert_eq!(r.jq_text(), ".ab", "Esc keeps the edit");
        assert!(!r.jq_focused(), "…and leaves the bar");
        assert!(!r.take_jq_edited(), "…changing nothing on the way out");
        assert!(r.set_jq_focus(true));
        r.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!r.jq_focused(), "Esc with nothing typed blurs");
        assert_eq!(r.jq_text(), ".ab");
        assert!(!r.take_jq_edited(), "…which is not an edit");
        r.set_jq_text_with_cursor("map(select(.x == ))", 17);
        assert!(r.take_jq_edited(), "a tee-up counts as an edit");
        assert_eq!(r.jq_bar().input.cursor(), 17);
    }

    #[test]
    fn esc_on_a_bar_opened_onto_no_filter_keeps_the_typed_filter() {
        let mut r = ready(ITEMS);
        assert!(r.open_jq());
        r.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));
        r.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(r.take_jq_edited());
        r.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(r.jq_text(), ".a", "Esc keeps what was typed");
        assert!(!r.jq_focused());
        assert!(r.jq_open(), "…so the bar stays, unfocused");
        assert!(r.jq_enabled());
        assert!(!r.take_jq_edited(), "leaving changes nothing");
    }

    #[test]
    fn esc_on_a_bar_opened_from_off_keeps_the_text_and_leaves_it_on() {
        let mut r = ready(ITEMS);
        r.set_jq_text(".a");
        assert!(r.open_jq());
        r.close_jq();
        assert!(!r.jq_enabled() && !r.jq_open());
        r.take_jq_edited();
        assert!(r.open_jq(), "on + focused");
        assert!(r.jq_enabled());
        r.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        r.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(r.jq_text(), ".ab", "the typed char stands");
        assert!(r.jq_enabled(), "the switch is never part of the edit");
        assert!(r.jq_open() && !r.jq_focused());
    }

    #[test]
    fn esc_with_nothing_typed_only_blurs_even_when_opened_from_off() {
        let mut r = ready(ITEMS);
        r.set_jq_text(".a");
        assert!(r.open_jq());
        r.close_jq();
        r.take_jq_edited();
        assert!(r.open_jq());
        // Moving around is not typing (it still flags a reconcile, for
        // the completion ghost that follows the caret).
        r.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        r.take_jq_edited();
        r.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(r.jq_text(), ".a");
        assert!(
            r.jq_enabled(),
            "opening it was the user's doing; it stays on"
        );
        assert!(r.jq_open() && !r.jq_focused());
        assert!(!r.take_jq_edited(), "nothing to revert");
    }

    #[test]
    fn enter_keeps_the_edit_and_ends_the_session() {
        let mut r = ready(ITEMS);
        r.set_jq_text(".a");
        assert!(r.set_jq_focus(true));
        r.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        r.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(r.jq_text(), ".ab");
        assert!(!r.jq_focused());
        assert!(
            !r.jq_bar().input.edited(),
            "Enter ended the session: ctrl+z is the app history again"
        );
        // A fresh edit session starts from the committed text.
        assert!(r.set_jq_focus(true));
        r.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(r.field_undo(false));
        assert_eq!(r.jq_text(), ".ab", "back to the last commit, not further");
        assert!(!r.field_undo(false), "and no further: the session is its own");
    }

    #[test]
    fn text_landing_in_a_focused_bar_stands_and_is_one_step_in_the_bar() {
        let mut r = ready(ITEMS);
        r.set_jq_text(".a");
        assert!(r.set_jq_focus(true));
        // The AI reply / a tee-up lands text into the focused bar.
        r.set_jq_text_with_cursor(".data.items", 11);
        r.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(r.jq_text(), ".data.items", "Esc keeps it");
        // …and while the bar still has the caret it is one undoable step.
        assert!(r.set_jq_focus(true));
        r.set_jq_text_with_cursor(".data.total", 11);
        assert!(r.field_undo(false));
        assert_eq!(r.jq_text(), ".data.items");
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
        assert_eq!(tabs[0].1.map(|(c, _)| c), Some(crate::glyph::FILTER));
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
        r.open_search(SearchSyntax::Literal, false);
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

    #[test]
    fn the_ghost_is_drawn_muted_in_the_caret_cell() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.");
        let text = render(&mut r);
        // The focused bar paints no caret cell of its own — the terminal
        // cursor (a bar) sits on the ghost's first cell — so the row reads
        // `.data.items` with nothing between the typed text and the ghost.
        assert!(text.contains(".data.items"), "{text}");
        let (_, buf) = render_buf(&mut r);
        let w = buf.area.width;
        let row = (0..buf.area.height)
            .find(|&y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains(".data.items")
            })
            .expect("bar row");
        let line: String = (0..w).map(|x| buf[(x, row)].symbol()).collect();
        let ghost_x = line.find("items").unwrap() as u16;
        assert_eq!(
            r.jq_caret_cell(),
            Some(Position::new(ghost_x, row)),
            "the terminal cursor is placed on the ghost's first cell"
        );
        assert!(
            (ghost_x - 6..=ghost_x + 4)
                .all(|x| !buf[(x, row)].modifier.contains(Modifier::REVERSED)),
            "no painted caret anywhere in the bar text"
        );
        let theme = Theme::dark();
        assert_eq!(buf[(ghost_x, row)].fg, theme.text_muted);
        assert_eq!(
            buf[(ghost_x - 3, row)].fg,
            theme.text,
            "typed text keeps its color"
        );
    }

    /// The bar's second row for a filter, and the colour it is drawn in:
    /// the first row under the bar text whose symbols hold `needle`.
    fn error_row(r: &mut Response, needle: &str) -> (String, ratatui::style::Color) {
        let (_, buf) = render_buf(r);
        let w = buf.area.width;
        let row = (0..buf.area.height)
            .find(|&y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap_or_else(|| panic!("no row holds {needle:?}"));
        let line: String = (0..w).map(|x| buf[(x, row)].symbol()).collect();
        let x = line.find(needle).unwrap() as u16;
        (line.trim_end().to_string(), buf[(x, row)].fg)
    }

    #[test]
    fn an_unfinished_filter_being_typed_is_drawn_muted_with_no_red_character() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items | sort_by(.id");
        let theme = Theme::dark();
        let (line, color) = error_row(&mut r, "unclosed (");
        assert_eq!(line.trim(), "unclosed (", "the message says what is missing");
        assert_eq!(color, theme.text_muted, "typing: not an error yet");
        let (_, buf) = render_buf(&mut r);
        let w = buf.area.width;
        let row = (0..buf.area.height)
            .find(|&y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains("sort_by(.id")
            })
            .expect("bar row");
        assert!(
            (0..w).all(|x| buf[(x, row)].fg != theme.error),
            "no character of the bar text is painted red"
        );
    }

    #[test]
    fn an_unfinished_filter_is_an_error_once_the_caret_leaves_the_end_or_the_bar() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items | sort_by(.id");
        let theme = Theme::dark();
        r.jq_bar_mut().input.set_cursor(3);
        r.refresh_jq_completion(SYNC_PRETTY_BYTES);
        let (_, color) = error_row(&mut r, "unclosed (");
        assert_eq!(color, theme.error, "caret mid-text: the filter as it stands is broken");
        r.jq_bar_mut().input.set_cursor(".data.items | sort_by(.id".len());
        r.set_jq_focus(false);
        let (_, color) = error_row(&mut r, "unclosed (");
        assert_eq!(color, theme.error, "blurred: the filter as it stands is broken");
    }

    #[test]
    fn a_real_syntax_error_stays_red_while_typing() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.items )");
        let theme = Theme::dark();
        let (_, color) = error_row(&mut r, "unexpected");
        assert_eq!(color, theme.error);
    }

    #[test]
    fn the_caret_cell_is_reported_only_while_the_bar_is_focused() {
        let mut r = ready(ITEMS);
        type_jq(&mut r, ".data");
        render_buf(&mut r);
        assert!(
            r.jq_caret_cell().is_some(),
            "focused: the cursor has a cell"
        );
        r.set_jq_focus(false);
        render_buf(&mut r);
        assert_eq!(r.jq_caret_cell(), None, "blurred: the cursor stays hidden");
    }

    #[test]
    fn the_ghost_is_clipped_to_the_bar_and_never_moves_the_caret() {
        let mut r = ready(ITEMS);
        r.set_jq_tab(JqTab::Cycle);
        type_jq(&mut r, ".data.");
        // 18 columns: the pane's 1-column-each-side inset (pane_surface)
        // takes 2, the ` 󰙴 ` chip plus its gap take 4, and `jq ` takes 3,
        // leaving the input 9 cells; `.data.` uses 6 (the caret is the
        // terminal cursor, on the ghost's first cell, not a cell of its
        // own), so the ghost is cut to three characters.
        let text = render_sized(&mut r, 18, 20);
        let bar_line = text
            .lines()
            .find(|l| l.contains(".data.ite"))
            .unwrap_or_else(|| panic!("bar row not found: {text}"));
        assert!(
            !bar_line.contains("items"),
            "clipped: {bar_line} (full buffer: {text})"
        );
    }
}
