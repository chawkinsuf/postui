use super::json_tree::{JsonTree, TokenKind};
use super::line_input::LineInput;
use super::{Component, DrawCtx, pane_surface};
use crate::action::{Action, CopyTarget};
use crate::hit::ScrollbarSpec;
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::time::{Duration, Instant};

/// Bodies up to this size are parsed on the UI thread, where the parse is
/// too quick to be noticed. Anything larger is parsed on a blocking worker
/// and delivered later via [`Response::attach_tree`], so no response is ever
/// too big to pretty-print and none of them stall the UI.
pub const SYNC_PRETTY_BYTES: usize = 256 * 1024;

/// Braille spinner frames, cycled while a request is in flight.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    /// When the background parse started, so the wait can animate.
    parse_started: Instant,
    /// Verbatim body lines — never reformatted, never re-wrapped.
    raw_lines: Vec<String>,
    header_lines: Vec<String>,
    pub cursor: usize,
    pub scroll: usize,
    pub search: Option<SearchState>,
    /// Height of the body viewport as of the last draw, so key handling can
    /// keep the cursor on screen. A sane guess until the first frame.
    height: usize,
}

impl ReadyView {
    fn new(data: &crate::http::ResponseData, generation: u64) -> Self {
        // A big body is parsed off-thread; until that lands there is no tree
        // to show, so the raw view leads and the Tree tab spins.
        let parsing = data.body.len() > SYNC_PRETTY_BYTES;
        let tree = if parsing {
            None
        } else {
            JsonTree::parse(&data.body)
        };
        let mode = if tree.is_some() {
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
            parse_started: Instant::now(),
            raw_lines: data.body.split('\n').map(|l| l.to_string()).collect(),
            header_lines: data
                .headers
                .iter()
                .map(|(k, v)| format!("{:<width$} {v}", format!("{k}:"), width = width + 1))
                .collect(),
            cursor: 0,
            scroll: 0,
            search: None,
            height: 10,
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
            ViewMode::Pretty => self.tree.as_ref().map_or(0, |t| t.visible_indices().len()),
            ViewMode::Raw => self.raw_lines.len(),
            ViewMode::Headers => self.header_lines.len().max(1),
        }
    }

    /// The current view's text with nothing hidden — the corpus search runs
    /// over, and the coordinate space its match positions live in.
    fn search_corpus(&self) -> Vec<String> {
        match self.mode {
            ViewMode::Pretty => self
                .tree
                .as_ref()
                .map(|t| t.full_text_lines())
                .unwrap_or_default(),
            ViewMode::Raw => self.raw_lines.clone(),
            ViewMode::Headers => self.header_lines.clone(),
        }
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
        if mode != ViewMode::Headers {
            self.body_mode = mode;
        }
        self.cursor = 0;
        self.scroll = 0;
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
        match (self.mode, self.tree.as_mut()) {
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
}

/// The search hits that fall on one rendered line, as char ranges.
struct LineMatches {
    ranges: Vec<(usize, usize)>,
    current: Option<(usize, usize)>,
}

#[derive(Default)]
pub struct Response {
    state: ResponseState,
    view: Option<ReadyView>,
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
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        if !view.awaits_tree(generation) {
            return false;
        }
        view.parsing = false;
        match tree {
            Some(tree) => {
                view.tree = Some(tree);
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

    /// Opens the in-pane search, exactly as `/` does (the `⌕` button).
    pub fn open_search(&mut self) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        view.open_search();
        true
    }

    /// Steps to the next (`1`) or previous (`-1`) match, exactly as `n`/`N`
    /// do (the `▼`/`▲` buttons).
    pub fn step_search(&mut self, delta: i32) -> bool {
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        if view.search.is_none() {
            return false;
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
        if toggle
            && view.mode == ViewMode::Pretty
            && let Some(tree) = view.tree.as_mut()
        {
            tree.toggle(view.cursor);
            view.clamp_cursor();
            view.follow_cursor();
        }
    }

    /// Ready-state key handling. Split out so [`Component::handle_key`] stays
    /// a readable state dispatch.
    fn ready_key(&mut self, ev: KeyEvent) -> Option<Action> {
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
                    view.set_mode(next);
                }
                Some(Action::Render)
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
                view.set_mode(next);
                Some(Action::Render)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                view.move_cursor(1);
                Some(Action::Render)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                view.move_cursor(-1);
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
                if view.mode == ViewMode::Pretty
                    && let Some(tree) = view.tree.as_mut()
                {
                    tree.toggle(view.cursor);
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
            KeyCode::Esc if view.search.is_some() => {
                view.search = None;
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
            // Modified combos belong to the global keymap, not the pane.
            ResponseState::Ready(_)
                if !ev
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
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
                let muted = Style::default().fg(t.text_muted);
                let lines = match other {
                    ResponseState::Empty => vec![
                        Line::raw(""),
                        Line::styled("Send a request — the response will appear here.", muted),
                    ],
                    ResponseState::InFlight { started } => {
                        let e = started.elapsed();
                        let frame_i = (e.subsec_millis() / 100) as usize % SPINNER.len();
                        vec![
                            Line::raw(""),
                            Line::styled(
                                format!("{} sending… {}", SPINNER[frame_i], human_elapsed(e)),
                                muted,
                            ),
                            Line::styled("esc to cancel", muted),
                        ]
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
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(HEADER_STRIP_HEIGHT), // status chip / chips+tabs / underline
                Constraint::Min(0),                      // body
                Constraint::Length(if footer { 1 } else { 0 }), // search footer
            ])
            .split(inner);

        draw_header_strip(frame, hits, rows[0], data, view, ctx);

        view.height = rows[1].height as usize;
        let mut body_area = rows[1];
        crate::paint::fill(frame.buffer_mut(), body_area, t.page);
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
            crate::hit::draw_scrollbar(frame, hits, column, &spec, ctx.hovered, ctx.dragging, t);
        }
        // `body_lines` already starts at `view.scroll`, so the paragraph
        // itself is drawn unscrolled.
        let body = body_lines(view, t, ctx.focused, ctx.hovered, hits, body_area);
        frame.render_widget(Paragraph::new(body), body_area);

        if footer {
            draw_search_footer(frame, hits, rows[2], view, ctx);
        }
    }
}

/// Total on-screen height (in rows) of [`draw_header_strip`]'s painted
/// surface: status chip / chips + right-aligned tabs / tabs underline.
const HEADER_STRIP_HEIGHT: u16 = 3;

/// Paints the 3-row header strip on `theme.panel`: the status chip and,
/// right-aligned on the same row, the Copy body / Save to file buttons, on
/// row 0; the timing + size chips (plain muted text — they are not
/// clickable) plus content type on the left and the response tabs
/// right-aligned on row 1; the tabs' accent underline on row 2.
fn draw_header_strip(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    data: &crate::http::ResponseData,
    view: &ReadyView,
    ctx: &DrawCtx,
) {
    let t = ctx.theme;
    let buf = frame.buffer_mut();
    crate::paint::fill(buf, area, t.panel);

    // Row 0 (left): the status chip, e.g. " 200 ".
    crate::paint::Chip {
        label: &data.status.to_string(),
        color: t.status_color(data.status),
    }
    .paint(buf, area.x, area.y, t.panel, t);

    // Row 0 (right): ⌕ / Copy body / Save to file, right-aligned — the
    // row's only other content is the short status chip, so there's no risk
    // of the buttons colliding with it at any pane width worth supporting.
    draw_header_actions(frame, hits, area, ctx);

    let buf = frame.buffer_mut();

    // Row 1 (left): timing + size, plain muted text (not clickable, so no
    // control fill — chip fill means clickability), then content type.
    let row1_y = area.y + 1;
    let mut x = area.x;
    for label in [human_elapsed(data.elapsed), human_size(data.size)] {
        let s = format!(" {label} ");
        let w = s.chars().count() as u16;
        crate::paint::text(buf, x, row1_y, &s, t.text_muted, t.panel, false);
        x += w + 1;
    }
    if let Some(ct) = &data.content_type {
        let s = format!(" {ct}");
        crate::paint::text(buf, x, row1_y, &s, t.text_muted, t.panel, false);
    }

    // Row 1 (right) + row 2 (its underline): the response tabs,
    // right-aligned.
    let mut tabs: Vec<(String, Option<(char, ratatui::style::Color)>)> = Vec::new();
    let mut modes: Vec<ViewMode> = Vec::new();
    if view.has_tree_view() {
        tabs.push(("Tree".to_string(), None));
        modes.push(ViewMode::Pretty);
    }
    tabs.push(("Raw".to_string(), None));
    modes.push(ViewMode::Raw);
    tabs.push(("Headers".to_string(), None));
    modes.push(ViewMode::Headers);

    let tabs_width = tabstrip_width(&tabs);
    let tabs_x = area.right().saturating_sub(tabs_width).max(area.x);
    let active = modes.iter().position(|m| *m == view.mode).unwrap_or(0);
    let hovered = match ctx.hovered {
        Some(crate::hit::Hit::ResponseTab(m)) => modes.iter().position(|mode| mode == m),
        _ => None,
    };
    let tabstrip_area = Rect::new(tabs_x, row1_y, tabs_width, 2);
    let rects = crate::paint::TabStrip {
        tabs: &tabs,
        active,
        hovered,
        // Response tabs are switched by plain keys (r/h), not by focusing
        // the strip, so it never claims keyboard focus of its own.
        focused: false,
    }
    .paint(buf, tabstrip_area, t.panel, t);
    for (rect, mode) in rects.into_iter().zip(modes) {
        hits.register(rect, crate::hit::Hit::ResponseTab(mode));
    }
}

/// The horizontal span [`crate::paint::TabStrip::paint`] occupies for
/// `tabs`, mirroring its own padded-block-width + 1-column-gap layout so
/// callers can right-align the strip without painting it first.
fn tabstrip_width(tabs: &[(String, Option<(char, ratatui::style::Color)>)]) -> u16 {
    let widths: Vec<u16> = tabs
        .iter()
        .map(|(label, badge)| label.chars().count() as u16 + 2 + badge.map_or(0, |_| 2))
        .collect();
    let sum: u16 = widths.iter().sum();
    sum + widths.len().saturating_sub(1) as u16
}

/// The header strip's plain painted actions — `⌕` (open search), `Copy
/// body`, `Save to file` — right-aligned in `area` on its `theme.panel`
/// fill. Overflows leftward when `area` is too narrow rather than off its
/// right edge.
fn draw_header_actions(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    area: Rect,
    ctx: &DrawCtx,
) {
    let actions = [
        (" ⌕ ".to_string(), crate::hit::Hit::ResponseSearchButton),
        (" Copy body ".to_string(), crate::hit::Hit::CopyBodyButton),
        (
            " Save to file ".to_string(),
            crate::hit::Hit::SaveBodyButton,
        ),
    ];
    let widths: Vec<u16> = actions
        .iter()
        .map(|(label, _)| label.chars().count() as u16)
        .collect();
    // One blank column between neighbours, the group flush to the right.
    let total: u16 = widths.iter().sum::<u16>() + widths.len().saturating_sub(1) as u16;
    let mut x = area.right().saturating_sub(total).max(area.x);

    let mut rects = Vec::new();
    let buf = frame.buffer_mut();
    for ((label, hit), w) in actions.iter().zip(widths) {
        let rect = Rect::new(x, area.y, w, 1);
        draw_pane_action(
            buf,
            rect,
            label,
            hit.clone(),
            ctx.hovered,
            ctx.theme.panel,
            ctx.theme,
        );
        rects.push((rect, hit.clone()));
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
/// `HeaderCopy` hit over the trailing ` ⧉ ` pill appended to each row.
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

    let mut push = |i: usize, full: usize, pieces: Vec<(String, Style)>, highlightable: bool| {
        let hits = if highlightable {
            view.match_ranges(full)
        } else {
            LineMatches {
                ranges: Vec::new(),
                current: None,
            }
        };
        let mut line = highlighted(pieces, &hits);
        if focused && i == view.cursor {
            line = line.style(cursor_bg);
        }
        out.push(line);
    };

    match view.mode {
        ViewMode::Pretty => {
            let Some(tree) = &view.tree else {
                if view.parsing {
                    let e = view.parse_started.elapsed();
                    let frame_i = (e.subsec_millis() / 100) as usize % SPINNER.len();
                    out.push(Line::styled(
                        format!(" {} parsing…", SPINNER[frame_i]),
                        Style::default().fg(t.text_muted),
                    ));
                }
                return out;
            };
            let lines = tree.visible_lines();
            let indices = tree.visible_indices();
            for i in start..end.min(lines.len()) {
                let line = lines[i];
                let mut pieces = vec![(" ".repeat(line.indent), text)];
                for tok in line.render_tokens() {
                    pieces.push((
                        tok.text.clone(),
                        Style::default().fg(token_color(tok.kind, t)),
                    ));
                }
                // A collapsed line renders its summary, not its real text, so
                // the match columns computed over the expanded text no longer
                // apply to it.
                push(i, indices[i], pieces, !line.collapsed);

                let y = area.y.saturating_add((i - start) as u16);
                if y < area.y.saturating_add(area.height) {
                    hits.register(
                        Rect::new(area.x, y, area.width, 1),
                        crate::hit::Hit::JsonRow(i),
                    );
                    if tree.is_container_at_visible(i) {
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
            for i in start..end.min(view.raw_lines.len()) {
                push(i, i, vec![(view.raw_lines[i].clone(), text)], true);
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
                    (" ⧉ ".to_string(), glyph_style),
                ];
                push(i, i, pieces, true);

                let y = area.y.saturating_add((i - start) as u16);
                if y < area.y.saturating_add(area.height) {
                    let glyph_x = area.x.saturating_add(text_len as u16);
                    let glyph_w = area.width.saturating_sub(text_len as u16).min(3);
                    if glyph_w > 0 {
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
    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn data(body: &str) -> crate::http::ResponseData {
        crate::http::ResponseData {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
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
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
        };
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| resp.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
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
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        // Row 1, just past the pane's 1-col left padding, lands inside the
        // elapsed chip's leading space.
        let cell = buf.cell((1, 1)).expect("elapsed chip cell");
        assert_eq!(
            cell.bg, theme.panel,
            "timing chip must not be control-filled: {cell:?}"
        );
        // Find the "ms" text and confirm it's muted, not on control fill.
        let mut found = false;
        for x in 0..60u16 {
            let cell = buf.cell((x, 1)).unwrap();
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
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| r.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        // Rows 0..3 (panes carry no border of their own) are the strip.
        // Column 15 is blank on row 0 (past the status chip, short of the
        // right-aligned Copy/Save buttons); column 36 is blank on rows 1-2
        // (past the chips and content type, short of the right-aligned
        // block tabs starting at 37). Both should still read as panel fill.
        for (y, x) in [(0u16, 15u16), (1, 36), (2, 36)] {
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
            vec![" ", "⧉", " "],
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
        assert_eq!(r.handle_key(ch('r')), Some(Action::Render));
        let out = render(&mut r);
        assert!(out.contains("{\"a\": 1,"), "raw is verbatim: {out}");
        assert!(
            out.contains("     \"b\": 2}"),
            "raw keeps original spacing: {out}"
        );
        assert_eq!(r.handle_key(ch('r')), Some(Action::Render));
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
    fn a_big_body_defers_its_parse_and_leads_with_raw() {
        let body = big_json();
        let mut r = ready(&body);
        let v = r.view().unwrap();
        assert!(v.parsing, "the parse was handed off, not run inline");
        assert!(v.tree.is_none(), "no tree until it lands");
        assert_eq!(v.mode, ViewMode::Raw, "the raw body is readable at once");
        assert!(
            render(&mut r).contains("Tree"),
            "the Tree tab is offered while the parse runs"
        );
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
        assert_eq!(r.handle_key(ch('h')), Some(Action::Render));
        let out = render(&mut r);
        assert!(out.contains("content-type: application/json"), "{out}");
        assert_eq!(r.handle_key(ch('h')), Some(Action::Render));
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
        r.handle_key(ch('r'));
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
}
