//! The binding table (piece 4 spec §3.5): one static table for both
//! profiles and every surface. A key's meaning is a lookup here; what a
//! prefix is follows from the rows, so no separate prefix list can drift.
//! M1's rows are the vim profile's motions and its `u`, `ctrl+r`, `:`; M2
//! adds the list verbs in both profiles (spec §5.1–§5.4).

use super::ctx::Surface;
use super::engine_glue::OpenSeat;
use super::notation::{self, Tok};
use crate::action::{Action, ColMove, PaneDir, Place, ResponseMotion, Verb};
use crate::config::KeyProfile;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::OnceLock;

/// A named key a row replays, with or without ctrl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Named {
    pub code: KeyCode,
    pub ctrl: bool,
}

impl Named {
    pub const fn key(code: KeyCode) -> Self {
        Self { code, ctrl: false }
    }

    pub const fn ctrl(code: KeyCode) -> Self {
        Self { code, ctrl: true }
    }

    pub fn event(self) -> KeyEvent {
        let mods = if self.ctrl { KeyModifiers::CONTROL } else { KeyModifiers::NONE };
        KeyEvent::new(self.code, mods)
    }
}

/// What a complete row does.
#[derive(Debug, Clone, Copy)]
pub enum Intent {
    /// Replay a named key through today's routing: `j` is Down.
    Key(Named),
    /// `gg` and `G`: with no count, `bare`; with a count N, `home` then
    /// N−1 Downs (row N, clamped by the surface).
    Edge { bare: Named, home: Named },
    /// A fixed action.
    Act(fn() -> Action),
    /// A list verb on the row under the cursor (spec §6.1): runs
    /// `Action::OnSelection`, which the app resolves against the surface.
    Verb(Verb),
    /// A short note (an Info toast, plan M1 Deviation 9); nothing else.
    Note(&'static str),
    /// Leave the open one-line field, then this key on the closed surface:
    /// `j` in the URL is `CloseField`, then Down (spec §3.7, §5.6; plan M3b
    /// Deviation 7, which replaces M3a Deviation 11's Esc). The count
    /// repeats the key.
    Leave(Named),
    /// A known key that does nothing here: `Z` and the key after it in a
    /// field, until M4's `ZZ` and `ZQ` (plan M3a Deviation 9).
    Swallow,
    /// Open the field under the cursor in Insert: `key` opens it as it does
    /// today, and the end-of-event sync seats the engine (spec §5.6; plan
    /// M3a Deviation 12).
    Open { key: Named, seat: OpenSeat },
    /// Open the field under the cursor in Normal (`key` opens it as it does
    /// today), then type these keys into its engine with the count before
    /// them: `cc`, `S` and `C` change the closed URL line as Vim's do, the
    /// old text in the unnamed register.
    Edit { key: Named, keys: &'static str },
    /// Reopen a dialog's selected text field (plan M3b Deviation 3): in
    /// Insert at this seat, or in Normal (`None`).
    Reopen(Option<OpenSeat>),
    /// `gt` (`delta` 1) and `gT` (-1) (spec §5.8), as `GoTab`: no count
    /// steps the strip; `{N}gt` goes to tab N; `{N}gT` steps back N enabled
    /// tabs, wrapping. A tab it selects lands the caret
    /// (`App::land_editor_caret`). Typed in an open screen field, the field
    /// closes first, only when there is a tab to go to (`App::go_tab`).
    Tab { delta: i8 },
    /// A window key (spec §5.8): `PaneMove`. Typed in an open screen field,
    /// the field closes first, only when there is a pane to move to.
    Pane(PaneDir),
    /// `q{reg}` (spec §5.11): the macro note, and the recording flag armed
    /// with the register, which the next bare `q` in Normal clears
    /// (`Router::recording`).
    Recording,
    /// Close the open field (committing it), then this action: `ZZ` in a
    /// screen field saves what the field holds (plan M4b Deviation 2).
    CloseThen(fn() -> Action),
    /// A response motion built from the count, `None` when none was typed
    /// (plan M5a Deviation 15): `{N}G`, `{N}zt`, `{N}<C-d>`.
    Motion(fn(Option<usize>) -> crate::action::ResponseMotion),
    /// A response motion in V, built from the count: it extends the line
    /// selection (plan M5a Deviation 11).
    Extend(fn(Option<usize>) -> crate::action::ResponseMotion),
    /// Any action built from the count, `None` when none was typed (plan
    /// M5a Deviation 15): `{N}zo`.
    Counted(fn(Option<usize>) -> Action),
}

/// How a count applies (spec §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountRule {
    /// The intent runs N times.
    Repeat,
    /// It runs once; the count is dropped.
    Ignore,
    /// The intent reads N itself (`Edge`).
    Pass,
    /// A count runs nothing and shows this note; with no count the
    /// intent runs once (spec §3.5).
    Refuse(&'static str),
}

/// Which profiles a row binds in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profiles(u8);

impl Profiles {
    pub const ARROWS: Self = Self(1);
    pub const VIM: Self = Self(2);
    pub const BOTH: Self = Self(3);

    pub fn has(self, p: KeyProfile) -> bool {
        let bit = match p {
            KeyProfile::Arrows => 1,
            KeyProfile::Vim => 2,
        };
        self.0 & bit != 0
    }
}

/// A set of surface kinds (the data a `Surface` carries is ignored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surfaces(u32);

impl Surfaces {
    pub const SIDEBAR: Self = Self(1 << 0);
    pub const URL_LINE: Self = Self(1 << 1);
    pub const METHOD: Self = Self(1 << 2);
    pub const TABLE: Self = Self(1 << 3);
    pub const BODY: Self = Self(1 << 4);
    pub const RESPONSE: Self = Self(1 << 5);
    pub const VAR_LIST: Self = Self(1 << 6);
    pub const VAR_GRID: Self = Self(1 << 7);
    pub const VAR_FORM: Self = Self(1 << 8);
    pub const MANAGE_LIST: Self = Self(1 << 9);
    pub const MANAGE_DETAIL: Self = Self(1 << 10);
    pub const SETTINGS: Self = Self(1 << 11);
    pub const PROMPT: Self = Self(1 << 12);
    pub const FORM: Self = Self(1 << 13);
    pub const DIALOG: Self = Self(1 << 14);
    pub const PICKER: Self = Self(1 << 15);
    pub const OTHER: Self = Self(1 << 16);
    /// An open one-line field in idle Normal (spec §3.7): the rows the
    /// router checks before the engine sees a key. Never a `Surface`.
    pub const FIELD_LINE: Self = Self(1 << 17);
    /// The request body in idle Normal (spec §3.7).
    pub const FIELD_BODY: Self = Self(1 << 18);
    pub const FIELDS: Self = Self::FIELD_LINE.or(Self::FIELD_BODY);
    /// A one-field prompt's own field in idle Normal (plan M3b): Enter
    /// confirms, Esc cancels. Always with `FIELD_LINE`.
    pub const FIELD_PROMPT: Self = Self(1 << 19);
    /// A query box (spec §3.7 item 2): always Insert, so no field row ever
    /// applies; the class only keeps `FIELD_LINE`'s rows away.
    pub const FIELD_QUERY: Self = Self(1 << 20);
    /// A screen field (plan M4a Deviation 2): the URL line, a table cell,
    /// the body, the Variables form and grid, the Settings field. The pane
    /// and tab rows apply in their idle Normal; never in a dialog's field,
    /// which keeps `g` and `ctrl+w` for the engine.
    pub const FIELD_SCREEN: Self = Self(1 << 21);
    /// The response with a line selection up (plan M5a Deviation 11).
    pub const RESPONSE_LINES: Self = Self(1 << 22);

    /// Every surface whose rows or controls the keyboard walks: vim's
    /// motions bind here, and in vim a bare char with no row is
    /// swallowed here (spec §3.4 step 4).
    pub const NAV: Self = Self::SIDEBAR
        .or(Self::URL_LINE)
        .or(Self::METHOD)
        .or(Self::TABLE)
        .or(Self::RESPONSE)
        .or(Self::MANAGE);
    /// The Manage screen's surfaces: its tabs' lists, panes and fields.
    pub const MANAGE: Self = Self::VAR_LIST
        .or(Self::VAR_GRID)
        .or(Self::VAR_FORM)
        .or(Self::MANAGE_LIST)
        .or(Self::MANAGE_DETAIL)
        .or(Self::SETTINGS);
    /// Where vim builds counts and swallows a bare char no row names (spec
    /// §3.4): the navigation surfaces and the response's V.
    pub const KEYED: Self = Self::NAV.or(Self::RESPONSE_LINES);
    /// The navigation surfaces but the response, whose motions are its own
    /// `Motion` rows.
    pub const LISTS: Self = Self::NAV.minus(Self::RESPONSE);
    /// Where a row opens on Enter (`o`).
    pub const OPENS_ROW: Self =
        Self::SIDEBAR.or(Self::TABLE).or(Self::VAR_LIST).or(Self::VAR_GRID).or(Self::MANAGE_LIST);
    /// Where a field opens on Enter (`i`). Manage detail panes take it
    /// too (spec §5.4; plan M1 Deviation 24).
    pub const OPENS_FIELD: Self = Self::TABLE
        .or(Self::VAR_GRID)
        .or(Self::VAR_FORM)
        .or(Self::SETTINGS)
        .or(Self::URL_LINE)
        .or(Self::MANAGE_DETAIL);
    /// The lists vim's `/` has no search on yet (key list §6, parked).
    pub const SEARCHLESS: Self = Self::OPENS_ROW;
    /// The lists and tables the list verbs act on (key list §6).
    pub const ROWS: Self =
        Self::SIDEBAR.or(Self::TABLE).or(Self::MANAGE_LIST).or(Self::VAR_LIST).or(Self::VAR_GRID);
    /// Where `u`, `ctrl+r` and `:` are the app's (spec §5.11): everywhere
    /// but pickers (they type) and dialogs (their letters answer).
    pub const APP: Self = Self::NAV.or(Self::BODY).or(Self::PROMPT).or(Self::FORM).or(Self::OTHER);
    /// The dialogs with a field: a one-field prompt and a form.
    pub const FORMS: Self = Self::PROMPT.or(Self::FORM);
    /// Where the window and tab keys apply (spec §5.8; plan M4a Deviation 2,
    /// plan M5a Deviation 16): the navigation surfaces, a screen field's
    /// idle Normal and the response's V. One row serves them all: a key
    /// typed in a field closes it first, on a closed surface no field is
    /// open, and a V selection ends as the key moves (`App::pane_move`).
    pub const WINDOWS: Self = Self::NAV.or(Self::FIELD_SCREEN).or(Self::RESPONSE_LINES);

    pub const fn or(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }

    pub const fn minus(self, o: Self) -> Self {
        Self(self.0 & !o.0)
    }

    pub const fn of(s: Surface) -> Self {
        match s {
            Surface::Sidebar => Self::SIDEBAR,
            Surface::UrlLine => Self::URL_LINE,
            Surface::Method => Self::METHOD,
            Surface::Table(_) => Self::TABLE,
            Surface::Body => Self::BODY,
            Surface::Response(_) => Self::RESPONSE,
            Surface::ResponseLines(_) => Self::RESPONSE_LINES,
            Surface::VarList => Self::VAR_LIST,
            Surface::VarGrid => Self::VAR_GRID,
            Surface::VarForm => Self::VAR_FORM,
            Surface::ManageList(_) => Self::MANAGE_LIST,
            Surface::ManageDetail(_) => Self::MANAGE_DETAIL,
            Surface::Settings => Self::SETTINGS,
            Surface::Prompt => Self::PROMPT,
            Surface::Form => Self::FORM,
            Surface::Dialog => Self::DIALOG,
            Surface::Picker => Self::PICKER,
            Surface::Other => Self::OTHER,
        }
    }

    pub fn has(self, s: Surface) -> bool {
        self.0 & Self::of(s).0 != 0
    }

    /// Whether the two sets share a kind: how a row's `on` meets a field
    /// class, which no `Surface` names.
    pub const fn overlaps(self, o: Self) -> bool {
        self.0 & o.0 != 0
    }
}

/// The field class the router checks in idle Normal and after a key the
/// engine hands back (spec §3.7; plans M3b and M4a).
pub fn field_class(id: super::ctx::FieldId, surface: Surface) -> Surfaces {
    use super::ctx::FieldId;
    match id {
        _ if id.insert_only() => Surfaces::FIELD_QUERY,
        FieldId::Body => Surfaces::FIELD_BODY.or(Surfaces::FIELD_SCREEN),
        FieldId::ModalInput(_) if surface == Surface::Prompt => {
            Surfaces::FIELD_LINE.or(Surfaces::FIELD_PROMPT)
        }
        FieldId::ModalInput(_) => Surfaces::FIELD_LINE,
        _ => Surfaces::FIELD_LINE.or(Surfaces::FIELD_SCREEN),
    }
}

/// One row: these keys, in these profiles, on these surfaces, mean this.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    /// Vim notation: `j`, `gg`, `<C-r>`.
    pub keys: &'static str,
    pub profile: Profiles,
    pub on: Surfaces,
    pub intent: Intent,
    pub count: CountRule,
}

const fn row(keys: &'static str, on: Surfaces, intent: Intent, count: CountRule) -> Binding {
    Binding { keys, profile: Profiles::VIM, on, intent, count }
}

/// An arrows row: one key, no count (arrows never builds one).
const fn arrows(keys: &'static str, on: Surfaces, intent: Intent) -> Binding {
    Binding { keys, profile: Profiles::ARROWS, on, intent, count: CountRule::Ignore }
}

/// A shared letter (the user's rule: a single-letter verb is the same
/// key in both profiles); a vim count is ignored.
const fn both(keys: &'static str, on: Surfaces, intent: Intent) -> Binding {
    Binding { keys, profile: Profiles::BOTH, on, intent, count: CountRule::Ignore }
}

/// A vim window row: `<C-w>` and a target (spec §5.8), on a closed surface
/// or from a screen field's idle Normal; a count is dropped.
const fn pane(keys: &'static str, dir: PaneDir) -> Binding {
    row(keys, Surfaces::WINDOWS, Intent::Pane(dir), CountRule::Ignore)
}

pub const NO_COUNTED_DELETE: &str = "Counted delete works on requests and table rows";
pub const NO_COUNTED_YANK: &str = "Counted yank is not supported";
pub const NO_MACROS: &str = "Macros are not supported";
pub const NO_MARKS: &str = "Marks are not supported";
pub const NO_REGISTERS: &str = "Registers are not supported here";

/// `ZZ`: `:x`, which quits on the main screen and closes Manage.
fn write_quit() -> Action {
    Action::CloseWindow { write: true, force: false }
}

/// `ZQ`: `:q!`, likewise.
fn force_quit() -> Action {
    Action::CloseWindow { write: false, force: true }
}

fn new_option() -> Action {
    Action::StartNewOptionEdit
}

const DOWN: Named = Named::key(KeyCode::Down);
const ENTER: Named = Named::key(KeyCode::Enter);
const HOME: Named = Named::key(KeyCode::Home);
const UP: Named = Named::key(KeyCode::Up);
const LEFT: Named = Named::key(KeyCode::Left);
const RIGHT: Named = Named::key(KeyCode::Right);
const END: Named = Named::key(KeyCode::End);

fn undo() -> Action {
    Action::Undo
}

fn repeat_change(n: Option<usize>) -> Action {
    Action::RepeatChange { count: n }
}

fn redo() -> Action {
    Action::Redo
}

/// Key list §8: `:` opens the command line, the palette in ex mode (spec
/// §5.9).
fn ex_palette() -> Action {
    Action::OpenExPalette
}

fn confirm_top() -> Action {
    Action::ConfirmTopModal
}

fn cancel_top() -> Action {
    Action::CancelTopModal
}

fn fold_of(op: crate::action::FoldOp, n: Option<usize>) -> Action {
    Action::TreeFold { op, count: count1(n) }
}

fn fold_open(n: Option<usize>) -> Action {
    fold_of(crate::action::FoldOp::Open, n)
}

fn fold_close(n: Option<usize>) -> Action {
    fold_of(crate::action::FoldOp::Close, n)
}

fn fold_toggle(n: Option<usize>) -> Action {
    fold_of(crate::action::FoldOp::Toggle, n)
}

fn fold_open_all() -> Action {
    fold_of(crate::action::FoldOp::OpenAll, None)
}

fn fold_close_all() -> Action {
    fold_of(crate::action::FoldOp::CloseAll, None)
}

/// Vim's E490, without its number (plan M5b Deviation 11).
pub const NO_FOLD: &str = "No fold found";

fn folder_fold(op: crate::action::FoldOp, n: Option<usize>) -> Action {
    Action::SidebarFold { op, count: count1(n) }
}

fn folder_open(n: Option<usize>) -> Action {
    folder_fold(crate::action::FoldOp::Open, n)
}

fn folder_close(n: Option<usize>) -> Action {
    folder_fold(crate::action::FoldOp::Close, n)
}

fn folder_toggle(n: Option<usize>) -> Action {
    folder_fold(crate::action::FoldOp::Toggle, n)
}

fn folders_open_all() -> Action {
    folder_fold(crate::action::FoldOp::OpenAll, None)
}

fn folders_close_all() -> Action {
    folder_fold(crate::action::FoldOp::CloseAll, None)
}

fn response_search() -> Action {
    Action::OpenResponseSearch { backward: false }
}

fn next_match() -> Action {
    Action::StepResponseSearch(1)
}

fn prev_match() -> Action {
    Action::StepResponseSearch(-1)
}

fn copy_header() -> Action {
    Action::CopyResponseHeader
}

fn count1(n: Option<usize>) -> usize {
    n.unwrap_or(1).max(1)
}

fn find_n(n: Option<usize>) -> Action {
    Action::ResponseFind(crate::action::ResponseFind::Next { reverse: false, count: count1(n) })
}

fn find_shift_n(n: Option<usize>) -> Action {
    Action::ResponseFind(crate::action::ResponseFind::Next { reverse: true, count: count1(n) })
}

fn find_star(n: Option<usize>) -> Action {
    Action::ResponseFind(crate::action::ResponseFind::Word { backward: false, count: count1(n) })
}

fn find_hash(n: Option<usize>) -> Action {
    Action::ResponseFind(crate::action::ResponseFind::Word { backward: true, count: count1(n) })
}

fn response_search_back() -> Action {
    Action::OpenResponseSearch { backward: true }
}

pub const ONLY_LINES: &str = "Only line selection (V) here";

fn lines_start() -> Action {
    Action::ResponseLines(crate::action::LinesOp::Start)
}

fn lines_end() -> Action {
    Action::ResponseLines(crate::action::LinesOp::End)
}

fn lines_swap() -> Action {
    Action::ResponseLines(crate::action::LinesOp::Swap)
}

fn lines_fold_open() -> Action {
    Action::ResponseLines(crate::action::LinesOp::Fold { open: true })
}

fn lines_fold_close() -> Action {
    Action::ResponseLines(crate::action::LinesOp::Fold { open: false })
}

fn lines_yank() -> Action {
    Action::ResponseLines(crate::action::LinesOp::Yank { count: None })
}

fn yank_rows(n: Option<usize>) -> Action {
    Action::ResponseLines(crate::action::LinesOp::Yank { count: n })
}

/// A count as a line distance: no count is one.
fn lines_of(n: Option<usize>) -> i64 {
    i64::try_from(n.unwrap_or(1)).unwrap_or(i64::MAX)
}

fn m_down(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Lines(lines_of(n))
}

fn m_up(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Lines(-lines_of(n))
}

fn m_first(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Goto { line: n, last: false }
}

fn m_last(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Goto { line: n, last: true }
}

fn m_half_down(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::HalfPage { down: true, count: n }
}

fn m_half_up(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::HalfPage { down: false, count: n }
}

fn m_end_goto(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::GotoEnd { line: n }
}

fn m_next_line(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::NextLine(count1(n))
}

fn m_left(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Col(ColMove::Left(count1(n)))
}

fn m_right(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Col(ColMove::Right(count1(n)))
}

fn m_space(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Col(ColMove::Space(count1(n)))
}

fn m_line_start(_: Option<usize>) -> ResponseMotion {
    ResponseMotion::Col(ColMove::Start)
}

fn m_line_end(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Col(ColMove::End(count1(n)))
}

fn m_page_down(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Page { down: true, count: n.unwrap_or(1) }
}

fn m_page_up(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Page { down: false, count: n.unwrap_or(1) }
}

fn m_top(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Place { at: Place::Top, line: n }
}

fn m_middle(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Place { at: Place::Middle, line: n }
}

fn m_bottom(n: Option<usize>) -> ResponseMotion {
    ResponseMotion::Place { at: Place::Bottom, line: n }
}

/// Every row. Order does not matter: no complete row is a prefix of
/// another on a shared surface (`no_two_rows_collide`).
#[rustfmt::skip]
pub static TABLE: &[Binding] = &[
    // Key list §6 and §7: moving.
    row("j", Surfaces::LISTS, Intent::Key(DOWN), CountRule::Repeat),
    row("k", Surfaces::LISTS, Intent::Key(Named::key(KeyCode::Up)), CountRule::Repeat),
    row("h", Surfaces::LISTS, Intent::Key(Named::key(KeyCode::Left)), CountRule::Repeat),
    row("l", Surfaces::LISTS, Intent::Key(Named::key(KeyCode::Right)), CountRule::Repeat),
    row("gg", Surfaces::LISTS, Intent::Edge { bare: HOME, home: HOME }, CountRule::Pass),
    row("G", Surfaces::LISTS, Intent::Edge { bare: Named::key(KeyCode::End), home: HOME }, CountRule::Pass),
    // The response's moves along a line (the user's decision, 2026-10-10:
    // Vim's hidden column): they move the column and still scroll the view
    // sideways; Space is Vim's `l` that wraps (`'whichwrap'` `s`), Enter
    // Vim's `+`, ctrl+Home `gg` and ctrl+End Vim's `G$`.
    row("h", Surfaces::RESPONSE, Intent::Motion(m_left), CountRule::Pass),
    row("l", Surfaces::RESPONSE, Intent::Motion(m_right), CountRule::Pass),
    row("<Left>", Surfaces::RESPONSE, Intent::Motion(m_left), CountRule::Pass),
    row("<Right>", Surfaces::RESPONSE, Intent::Motion(m_right), CountRule::Pass),
    row("0", Surfaces::RESPONSE, Intent::Motion(m_line_start), CountRule::Ignore),
    row("<Home>", Surfaces::RESPONSE, Intent::Motion(m_line_start), CountRule::Ignore),
    row("$", Surfaces::RESPONSE, Intent::Motion(m_line_end), CountRule::Pass),
    row("<End>", Surfaces::RESPONSE, Intent::Motion(m_line_end), CountRule::Pass),
    row("<Space>", Surfaces::RESPONSE, Intent::Motion(m_space), CountRule::Pass),
    row("<CR>", Surfaces::RESPONSE, Intent::Motion(m_next_line), CountRule::Pass),
    row("<C-Home>", Surfaces::RESPONSE, Intent::Motion(m_first), CountRule::Pass),
    row("<C-End>", Surfaces::RESPONSE, Intent::Motion(m_end_goto), CountRule::Pass),
    row("h", Surfaces::RESPONSE_LINES, Intent::Extend(m_left), CountRule::Pass),
    row("l", Surfaces::RESPONSE_LINES, Intent::Extend(m_right), CountRule::Pass),
    row("<Left>", Surfaces::RESPONSE_LINES, Intent::Extend(m_left), CountRule::Pass),
    row("<Right>", Surfaces::RESPONSE_LINES, Intent::Extend(m_right), CountRule::Pass),
    row("0", Surfaces::RESPONSE_LINES, Intent::Extend(m_line_start), CountRule::Ignore),
    row("<Home>", Surfaces::RESPONSE_LINES, Intent::Extend(m_line_start), CountRule::Ignore),
    row("$", Surfaces::RESPONSE_LINES, Intent::Extend(m_line_end), CountRule::Pass),
    row("<End>", Surfaces::RESPONSE_LINES, Intent::Extend(m_line_end), CountRule::Pass),
    row("<C-Home>", Surfaces::RESPONSE_LINES, Intent::Extend(m_first), CountRule::Pass),
    row("<C-End>", Surfaces::RESPONSE_LINES, Intent::Extend(m_end_goto), CountRule::Pass),
    // Key list §7 (`R.move`, `R.ctrl-d`, `R.page`), plan M5a Task 3: the
    // response's motions are Vim's on a window of its visible lines, each
    // reading its own count (Deviation 4). Arrows' named keys stay the
    // component's arms, which run the same window code.
    row("j", Surfaces::RESPONSE, Intent::Motion(m_down), CountRule::Pass),
    row("k", Surfaces::RESPONSE, Intent::Motion(m_up), CountRule::Pass),
    row("<Down>", Surfaces::RESPONSE, Intent::Motion(m_down), CountRule::Pass),
    row("<Up>", Surfaces::RESPONSE, Intent::Motion(m_up), CountRule::Pass),
    row("gg", Surfaces::RESPONSE, Intent::Motion(m_first), CountRule::Pass),
    row("G", Surfaces::RESPONSE, Intent::Motion(m_last), CountRule::Pass),
    row("<C-d>", Surfaces::RESPONSE, Intent::Motion(m_half_down), CountRule::Pass),
    row("<C-u>", Surfaces::RESPONSE, Intent::Motion(m_half_up), CountRule::Pass),
    row("<C-f>", Surfaces::RESPONSE, Intent::Motion(m_page_down), CountRule::Pass),
    row("<C-b>", Surfaces::RESPONSE, Intent::Motion(m_page_up), CountRule::Pass),
    // Vim's PageDown and PageUp are ctrl+f and ctrl+b, counted as they are
    // (`3<PageDown>` pages three times). Arrows keeps the component's arms.
    row("<PageDown>", Surfaces::RESPONSE, Intent::Motion(m_page_down), CountRule::Pass),
    row("<PageUp>", Surfaces::RESPONSE, Intent::Motion(m_page_up), CountRule::Pass),
    row("zt", Surfaces::RESPONSE, Intent::Motion(m_top), CountRule::Pass),
    row("zz", Surfaces::RESPONSE, Intent::Motion(m_middle), CountRule::Pass),
    row("zb", Surfaces::RESPONSE, Intent::Motion(m_bottom), CountRule::Pass),
    // Key list §7 (`R.yy`, `R.visual`), plan M5a Deviations 5 and 11: Vim's
    // yank into the unnamed register, and its line Visual.
    row("yy", Surfaces::RESPONSE, Intent::Counted(yank_rows), CountRule::Pass),
    row("V", Surfaces::RESPONSE, Intent::Act(lines_start), CountRule::Ignore),
    row("v", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Note(ONLY_LINES), CountRule::Ignore),
    row("V", Surfaces::RESPONSE_LINES, Intent::Act(lines_end), CountRule::Ignore),
    row("<Esc>", Surfaces::RESPONSE_LINES, Intent::Act(lines_end), CountRule::Ignore),
    row("o", Surfaces::RESPONSE_LINES, Intent::Act(lines_swap), CountRule::Ignore),
    row("y", Surfaces::RESPONSE_LINES, Intent::Act(lines_yank), CountRule::Ignore),
    row("j", Surfaces::RESPONSE_LINES, Intent::Extend(m_down), CountRule::Pass),
    row("k", Surfaces::RESPONSE_LINES, Intent::Extend(m_up), CountRule::Pass),
    row("<Down>", Surfaces::RESPONSE_LINES, Intent::Extend(m_down), CountRule::Pass),
    row("<Up>", Surfaces::RESPONSE_LINES, Intent::Extend(m_up), CountRule::Pass),
    row("gg", Surfaces::RESPONSE_LINES, Intent::Extend(m_first), CountRule::Pass),
    row("G", Surfaces::RESPONSE_LINES, Intent::Extend(m_last), CountRule::Pass),
    row("<C-d>", Surfaces::RESPONSE_LINES, Intent::Extend(m_half_down), CountRule::Pass),
    row("<C-u>", Surfaces::RESPONSE_LINES, Intent::Extend(m_half_up), CountRule::Pass),
    row("<C-f>", Surfaces::RESPONSE_LINES, Intent::Extend(m_page_down), CountRule::Pass),
    row("<C-b>", Surfaces::RESPONSE_LINES, Intent::Extend(m_page_up), CountRule::Pass),
    // Amendment A6.3: Vim's PageDown and PageUp extend Visual as ctrl+f and
    // ctrl+b do (probe V14).
    row("<PageDown>", Surfaces::RESPONSE_LINES, Intent::Extend(m_page_down), CountRule::Pass),
    row("<PageUp>", Surfaces::RESPONSE_LINES, Intent::Extend(m_page_up), CountRule::Pass),
    // Vim's Enter (`+`) and Space in V move as they do outside it,
    // extending. Arrows keeps main's Space and Enter: these are vim rows.
    row("<CR>", Surfaces::RESPONSE_LINES, Intent::Extend(m_next_line), CountRule::Pass),
    row("<Space>", Surfaces::RESPONSE_LINES, Intent::Extend(m_space), CountRule::Pass),
    row("zt", Surfaces::RESPONSE_LINES, Intent::Extend(m_top), CountRule::Pass),
    row("zz", Surfaces::RESPONSE_LINES, Intent::Extend(m_middle), CountRule::Pass),
    row("zb", Surfaces::RESPONSE_LINES, Intent::Extend(m_bottom), CountRule::Pass),
    // Key list §7 (`R.fold`, `R.foldall`): the tree's folds, counted (plan
    // M5a Deviation 9). Nothing on Raw or Headers.
    row("zo", Surfaces::RESPONSE, Intent::Counted(fold_open), CountRule::Pass),
    row("zc", Surfaces::RESPONSE, Intent::Counted(fold_close), CountRule::Pass),
    row("za", Surfaces::RESPONSE, Intent::Counted(fold_toggle), CountRule::Pass),
    row("zR", Surfaces::RESPONSE, Intent::Act(fold_open_all), CountRule::Ignore),
    row("zM", Surfaces::RESPONSE, Intent::Act(fold_close_all), CountRule::Ignore),
    // In V, as Vim's Visual has them: `zo` `zc` act on every selected line
    // and end the selection; `za` `zR` `zM` act as outside it and keep it.
    row("zo", Surfaces::RESPONSE_LINES, Intent::Act(lines_fold_open), CountRule::Ignore),
    row("zc", Surfaces::RESPONSE_LINES, Intent::Act(lines_fold_close), CountRule::Ignore),
    row("za", Surfaces::RESPONSE_LINES, Intent::Counted(fold_toggle), CountRule::Pass),
    row("zR", Surfaces::RESPONSE_LINES, Intent::Act(fold_open_all), CountRule::Ignore),
    row("zM", Surfaces::RESPONSE_LINES, Intent::Act(fold_close_all), CountRule::Ignore),
    // Key list §6 (`L.fold`), plan M5b Deviations 9–11: the sidebar's
    // folders are Vim's folds, counted. The other lists have none and say
    // so, as Vim's E490 does, so `za` never adds a row there.
    row("zo", Surfaces::SIDEBAR, Intent::Counted(folder_open), CountRule::Pass),
    row("zc", Surfaces::SIDEBAR, Intent::Counted(folder_close), CountRule::Pass),
    row("za", Surfaces::SIDEBAR, Intent::Counted(folder_toggle), CountRule::Pass),
    row("zR", Surfaces::SIDEBAR, Intent::Act(folders_open_all), CountRule::Ignore),
    row("zM", Surfaces::SIDEBAR, Intent::Act(folders_close_all), CountRule::Ignore),
    row("zo", Surfaces::LISTS.minus(Surfaces::SIDEBAR), Intent::Note(NO_FOLD), CountRule::Ignore),
    row("zc", Surfaces::LISTS.minus(Surfaces::SIDEBAR), Intent::Note(NO_FOLD), CountRule::Ignore),
    row("za", Surfaces::LISTS.minus(Surfaces::SIDEBAR), Intent::Note(NO_FOLD), CountRule::Ignore),
    row("zR", Surfaces::LISTS.minus(Surfaces::SIDEBAR), Intent::Swallow, CountRule::Ignore),
    row("zM", Surfaces::LISTS.minus(Surfaces::SIDEBAR), Intent::Swallow, CountRule::Ignore),
    // Key list §8 (`P.gt`), spec §5.8: every tab strip, on a closed
    // surface or from a screen field's idle Normal (§3.7).
    row("gt", Surfaces::WINDOWS, Intent::Tab { delta: 1 }, CountRule::Pass),
    row("gT", Surfaces::WINDOWS, Intent::Tab { delta: -1 }, CountRule::Pass),
    // Key list §8 (`P.ctrlw`), spec §5.8: the window keys, with Vim's ctrl
    // spellings of the targets (plan M4a Deviation 3). In a screen field a
    // non-target is swallowed, never replayed as an edit (Deviation 4).
    pane("<C-w>h", PaneDir::Left),
    pane("<C-w>j", PaneDir::Down),
    pane("<C-w>k", PaneDir::Up),
    pane("<C-w>l", PaneDir::Right),
    pane("<C-w>w", PaneDir::Next),
    pane("<C-w>W", PaneDir::Prev),
    pane("<C-w>p", PaneDir::Last),
    pane("<C-w><C-h>", PaneDir::Left),
    pane("<C-w><C-j>", PaneDir::Down),
    pane("<C-w><C-k>", PaneDir::Up),
    pane("<C-w><C-l>", PaneDir::Right),
    pane("<C-w><C-w>", PaneDir::Next),
    pane("<C-w><C-p>", PaneDir::Last),
    // Key list §8 (`P.ctrlw-arrows`): the arrow keys, as Vim's `<C-w><Left>`.
    pane("<C-w><Left>", PaneDir::Left),
    pane("<C-w><Down>", PaneDir::Down),
    pane("<C-w><Up>", PaneDir::Up),
    pane("<C-w><Right>", PaneDir::Right),
    row("<C-w><any>", Surfaces::FIELD_SCREEN, Intent::Swallow, CountRule::Ignore),
    // Key list §6: opening.
    row("o", Surfaces::OPENS_ROW, Intent::Key(Named::key(KeyCode::Enter)), CountRule::Ignore),
    row("i", Surfaces::OPENS_FIELD, Intent::Open { key: ENTER, seat: OpenSeat::Keep }, CountRule::Ignore),
    // Spec §5.6: the closed URL line's own openings.
    row("a", Surfaces::URL_LINE, Intent::Open { key: ENTER, seat: OpenSeat::After }, CountRule::Ignore),
    row("I", Surfaces::URL_LINE, Intent::Open { key: ENTER, seat: OpenSeat::FirstNonBlank }, CountRule::Ignore),
    row("A", Surfaces::URL_LINE, Intent::Open { key: ENTER, seat: OpenSeat::End }, CountRule::Ignore),
    row("cc", Surfaces::URL_LINE, Intent::Edit { key: ENTER, keys: "cc" }, CountRule::Pass),
    row("S", Surfaces::URL_LINE, Intent::Edit { key: ENTER, keys: "S" }, CountRule::Pass),
    row("C", Surfaces::URL_LINE, Intent::Edit { key: ENTER, keys: "C" }, CountRule::Pass),
    // Vim's Esc in Normal moves nothing: on the method badge, the closed
    // URL line and a table it keeps the selection where it is, where
    // arrows' Esc steps out of the badge or line and deselects the row.
    row("<Esc>", Surfaces::METHOD.or(Surfaces::URL_LINE).or(Surfaces::TABLE), Intent::Swallow, CountRule::Ignore),
    // Key list §7 and spec §4.5 (plan M4a Deviation 11): `/` is the same in
    // both profiles. Vim's `n` `N` are counted and follow `?`/`#`'s
    // direction; arrows' step down and up, as the ▼/▲ buttons do (plan M5a
    // Deviation 10). `?`, `*` and `#` are vim's. Arrows' `c` copies a header.
    both("/", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Act(response_search)),
    arrows("n", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Act(next_match)),
    arrows("N", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Act(prev_match)),
    // In vim's V they are motions that extend the selection, as Vim's
    // Visual takes them (Vim 9.1, probed).
    row("n", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Counted(find_n), CountRule::Pass),
    row("N", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Counted(find_shift_n), CountRule::Pass),
    row("*", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Counted(find_star), CountRule::Pass),
    row("#", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Counted(find_hash), CountRule::Pass),
    row("?", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Act(response_search_back), CountRule::Ignore),
    arrows("c", Surfaces::RESPONSE.or(Surfaces::RESPONSE_LINES), Intent::Act(copy_header)),
    // Key list §6, Out (parked by the user): a list has no search yet.
    row("/", Surfaces::SEARCHLESS, Intent::Note("No search in this list yet"), CountRule::Ignore),
    // Key list §6 and §9: the app's undo, redo and command palette.
    row("u", Surfaces::APP, Intent::Act(undo), CountRule::Repeat),
    row("<C-r>", Surfaces::APP, Intent::Act(redo), CountRule::Repeat),
    row(":", Surfaces::APP, Intent::Act(ex_palette), CountRule::Ignore),
    // Key list §6 (`L.dot`), spec §5.12 (plan M5b Deviations 2 and 5): `.`
    // repeats the last list change where it runs, its count replaced by one
    // typed here.
    row(".", Surfaces::NAV, Intent::Counted(repeat_change), CountRule::Pass),
    // Vim's `:` in Visual opens the line over the selection (its `'<,'>`):
    // V stays up for the palette's commands and ends once it closes
    // (`App::end_visual_off_response`). Visual's `u` is not undo.
    row(":", Surfaces::RESPONSE_LINES, Intent::Act(ex_palette), CountRule::Ignore),
    // Key list §6, spec §5.1–§5.4 and §6.1: the list verbs. The shared
    // letters are one row for both profiles; vim doubles delete and yank
    // and takes counts, arrows uses one letter. Vim's other verbs on the
    // Manage and Variables screens are M4's ex rows (plan M2 Deviation 1).
    both("a", Surfaces::ROWS, Intent::Verb(Verb::Add)),
    arrows("a", Surfaces::MANAGE_DETAIL, Intent::Verb(Verb::Add)),
    both("A", Surfaces::VAR_LIST, Intent::Verb(Verb::AddAlt)),
    both("r", Surfaces::SIDEBAR.or(Surfaces::MANAGE_LIST).or(Surfaces::VAR_LIST), Intent::Verb(Verb::Rename)),
    arrows("r", Surfaces::MANAGE_DETAIL.or(Surfaces::VAR_GRID), Intent::Verb(Verb::Rename)),
    arrows("r", Surfaces::VAR_FORM, Intent::Verb(Verb::Reveal)),
    both("m", Surfaces::SIDEBAR.or(Surfaces::MANAGE_LIST), Intent::Verb(Verb::Move)),
    arrows("m", Surfaces::MANAGE_DETAIL, Intent::Verb(Verb::Move)),
    arrows("m", Surfaces::VAR_LIST.or(Surfaces::VAR_GRID), Intent::Verb(Verb::Fields)),
    row("dd", Surfaces::SIDEBAR.or(Surfaces::TABLE), Intent::Verb(Verb::Delete { count: 1 }), CountRule::Pass),
    row(
        "dd",
        Surfaces::MANAGE_LIST.or(Surfaces::VAR_LIST).or(Surfaces::VAR_GRID),
        Intent::Verb(Verb::Delete { count: 1 }),
        CountRule::Refuse(NO_COUNTED_DELETE),
    ),
    arrows("d", Surfaces::ROWS.or(Surfaces::MANAGE_DETAIL), Intent::Verb(Verb::Delete { count: 1 })),
    row(
        "yy",
        Surfaces::SIDEBAR.or(Surfaces::TABLE).or(Surfaces::VAR_GRID),
        Intent::Verb(Verb::Yank),
        CountRule::Refuse(NO_COUNTED_YANK),
    ),
    arrows("y", Surfaces::SIDEBAR.or(Surfaces::TABLE), Intent::Verb(Verb::Yank)),
    arrows("c", Surfaces::VAR_GRID, Intent::Verb(Verb::Yank)),
    row("p", Surfaces::SIDEBAR.or(Surfaces::TABLE).or(Surfaces::VAR_GRID), Intent::Verb(Verb::Put { above: false, count: 1 }), CountRule::Pass),
    row("P", Surfaces::SIDEBAR.or(Surfaces::TABLE).or(Surfaces::VAR_GRID), Intent::Verb(Verb::Put { above: true, count: 1 }), CountRule::Pass),
    arrows("p", Surfaces::SIDEBAR.or(Surfaces::TABLE), Intent::Verb(Verb::Put { above: false, count: 1 })),
    arrows("P", Surfaces::SIDEBAR.or(Surfaces::TABLE), Intent::Verb(Verb::Put { above: true, count: 1 })),
    arrows("v", Surfaces::VAR_GRID, Intent::Verb(Verb::Put { above: false, count: 1 })),
    arrows("p", Surfaces::VAR_FORM, Intent::Verb(Verb::Promote)),
    arrows("s", Surfaces::VAR_LIST.or(Surfaces::VAR_FORM), Intent::Verb(Verb::Secret)),
    arrows("x", Surfaces::VAR_FORM, Intent::Verb(Verb::ClearValue)),
    arrows("e", Surfaces::VAR_GRID, Intent::Verb(Verb::EditValue)),
    arrows("t", Surfaces::MANAGE_LIST.or(Surfaces::MANAGE_DETAIL), Intent::Verb(Verb::Tls)),
    // `main`'s arrows `o` (new option) stays; vim's `o` there opens the row
    // (plan M2 Deviation 13).
    arrows("o", Surfaces::VAR_LIST.or(Surfaces::VAR_GRID), Intent::Act(new_option)),
    // Spec §3.7: an open field in idle Normal. The router checks these
    // before the engine; a key that matches none of them is replayed into
    // the engine as typed. `j` `k` and the paging chords leave a one-line
    // field (the user's decision 2), the rest are notes until M4 gives
    // them their own meaning (plan M3a Deviation 9), so no prefix key ever
    // reaches the engine as a wrong edit.
    row(":", Surfaces::FIELDS, Intent::Act(ex_palette), CountRule::Ignore),
    row("j", Surfaces::FIELD_LINE, Intent::Leave(DOWN), CountRule::Repeat),
    row("k", Surfaces::FIELD_LINE, Intent::Leave(UP), CountRule::Repeat),
    row("<Down>", Surfaces::FIELD_LINE, Intent::Leave(DOWN), CountRule::Repeat),
    row("<Up>", Surfaces::FIELD_LINE, Intent::Leave(UP), CountRule::Repeat),
    row("<C-d>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('d'))), CountRule::Ignore),
    row("<C-u>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('u'))), CountRule::Ignore),
    row("<C-f>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('f'))), CountRule::Ignore),
    row("<C-b>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('b'))), CountRule::Ignore),
    row("q<any>", Surfaces::FIELDS, Intent::Recording, CountRule::Ignore),
    row("@<any>", Surfaces::FIELDS, Intent::Note(NO_MACROS), CountRule::Ignore),
    row("m<any>", Surfaces::FIELDS, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("'<any>", Surfaces::FIELDS, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("`<any>", Surfaces::FIELDS, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("Z<any>", Surfaces::FIELDS, Intent::Swallow, CountRule::Ignore),
    // Key list §9 (G.ZZ, G.qa, G.marks, G.regs), spec §5.11: the globals
    // on the navigation surfaces and in a screen field's idle Normal, never
    // on a dialog, a prompt, a picker or a dialog's field (plan M4b
    // Deviation 2). `ZZ` from a field closes it first; `Z<any>` above
    // still swallows anything else after `Z` in a field.
    row("ZZ", Surfaces::NAV, Intent::Act(write_quit), CountRule::Ignore),
    row("ZQ", Surfaces::NAV, Intent::Act(force_quit), CountRule::Ignore),
    row("ZZ", Surfaces::FIELD_SCREEN, Intent::CloseThen(write_quit), CountRule::Ignore),
    row("ZQ", Surfaces::FIELD_SCREEN, Intent::Act(force_quit), CountRule::Ignore),
    // The notes on a closed surface (Deviation 4): `m` stays move there.
    row("q<any>", Surfaces::NAV, Intent::Recording, CountRule::Ignore),
    row("@<any>", Surfaces::NAV, Intent::Note(NO_MACROS), CountRule::Ignore),
    row("'<any>", Surfaces::NAV, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("`<any>", Surfaces::NAV, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("\"<any>", Surfaces::NAV, Intent::Note(NO_REGISTERS), CountRule::Ignore),
    // Spec §5.10, key list D.prompt (plan M3b Deviations 7 and 8): a
    // one-field prompt confirms on Enter from anywhere, field or not; its
    // field's Esc in idle Normal cancels (Insert's Esc is the engine's).
    // Enter typed in Insert reaches the first row too: the engine hands a
    // one-line field's Enter back.
    row("<CR>", Surfaces::PROMPT.or(Surfaces::FIELD_PROMPT), Intent::Act(confirm_top), CountRule::Ignore),
    row("<Esc>", Surfaces::FIELD_PROMPT, Intent::Act(cancel_top), CountRule::Ignore),
    // Spec §5.10 and §4.5: a dialog's stops, its buttons and a field-less
    // picker (the dropdown, the theme picker) move with vim's keys;
    // modal.rs no longer aliases them. A dialog's own answer letters come
    // first (`answers`). `h`/`l` on a picker flip the theme picker's
    // dark/light set (task #6).
    row("j", Surfaces::FORMS.or(Surfaces::DIALOG).or(Surfaces::PICKER), Intent::Key(DOWN), CountRule::Ignore),
    row("k", Surfaces::FORMS.or(Surfaces::DIALOG).or(Surfaces::PICKER), Intent::Key(UP), CountRule::Ignore),
    row("h", Surfaces::FORMS.or(Surfaces::DIALOG).or(Surfaces::PICKER), Intent::Key(LEFT), CountRule::Ignore),
    row("l", Surfaces::FORMS.or(Surfaces::DIALOG).or(Surfaces::PICKER), Intent::Key(RIGHT), CountRule::Ignore),
    row("G", Surfaces::DIALOG.or(Surfaces::PICKER), Intent::Key(END), CountRule::Ignore),
    row("gg", Surfaces::PICKER, Intent::Key(HOME), CountRule::Ignore),
    // Spec §5.6 (plan M3b Deviation 3): a selected dialog field reopens,
    // `o` in Normal, `i a I A` in Insert seated as on the URL line.
    row("o", Surfaces::FORMS, Intent::Reopen(None), CountRule::Ignore),
    row("i", Surfaces::FORMS, Intent::Reopen(Some(OpenSeat::Keep)), CountRule::Ignore),
    row("a", Surfaces::FORMS, Intent::Reopen(Some(OpenSeat::After)), CountRule::Ignore),
    row("I", Surfaces::FORMS, Intent::Reopen(Some(OpenSeat::FirstNonBlank)), CountRule::Ignore),
    row("A", Surfaces::FORMS, Intent::Reopen(Some(OpenSeat::End)), CountRule::Ignore),
];

/// Each row's keys, parsed once.
fn parsed() -> &'static [Vec<Tok>] {
    static PARSED: OnceLock<Vec<Vec<Tok>>> = OnceLock::new();
    PARSED.get_or_init(|| TABLE.iter().map(|b| notation::parse(b.keys)).collect())
}

/// What the buffered keys are on a surface.
#[derive(Debug, Clone, Copy)]
pub enum Match {
    Complete(&'static Binding),
    /// Some row on this surface extends the buffer.
    Prefix,
    NoMatch,
}

pub fn lookup(buf: &[Tok], profile: KeyProfile, surface: Surface) -> Match {
    lookup_class(buf, profile, Surfaces::of(surface))
}

/// [`lookup`] on a set of surface kinds: a `Surface`'s, or a field class
/// (`FIELD_LINE`, `FIELD_BODY`, `FIELD_PROMPT`, `FIELD_QUERY`,
/// `FIELD_SCREEN`) that no
/// `Surface` names.
pub fn lookup_class(buf: &[Tok], profile: KeyProfile, class: Surfaces) -> Match {
    let rows = TABLE
        .iter()
        .zip(parsed())
        .filter(|(b, _)| b.profile.has(profile))
        .map(|(b, keys)| (b, keys.as_slice()));
    best(rows, buf, class)
}

/// Whether a row's keys `row` fit the typed keys `typed`, position by
/// position, `<any>` fitting anything. Same length is the caller's check.
fn fits(row: &[Tok], typed: &[Tok]) -> bool {
    row.iter().zip(typed).all(|(r, t)| *r == Tok::Any || r == t)
}

/// The best row for `buf` among `rows` on `class`: a complete row with no
/// `<any>` first, then one with `<any>` (plan M3a Deviation 10), then
/// whether some row extends `buf`.
fn best<'a>(
    rows: impl IntoIterator<Item = (&'static Binding, &'a [Tok])>,
    buf: &[Tok],
    class: Surfaces,
) -> Match {
    let mut prefix = false;
    let mut wild = None;
    for (b, keys) in rows {
        if !b.on.overlaps(class) {
            continue;
        }
        if keys.len() == buf.len() && fits(keys, buf) {
            if !keys.contains(&Tok::Any) {
                return Match::Complete(b);
            }
            wild.get_or_insert(b);
        } else if keys.len() > buf.len() && fits(&keys[..buf.len()], buf) {
            prefix = true;
        }
    }
    match wild {
        Some(b) => Match::Complete(b),
        None if prefix => Match::Prefix,
        None => Match::NoMatch,
    }
}

/// The keys of the row that runs `action` here, for keycaps (spec §4.6).
pub fn keys_for(profile: KeyProfile, surface: Surface, action: &Action) -> Option<&'static str> {
    keys_for_class(profile, Surfaces::of(surface), action)
}

/// [`keys_for`] on a surface class: a field class in idle Normal (plan M4b
/// Deviation 21). A tab row spells `CycleTabs` whether or not it leaves a
/// field; the `o` reopen row spells `ReopenField`.
pub fn keys_for_class(
    profile: KeyProfile,
    class: Surfaces,
    action: &Action,
) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|b| {
            b.profile.has(profile)
                && b.on.overlaps(class)
                && match b.intent {
                    Intent::Act(f) | Intent::CloseThen(f) => f() == *action,
                    Intent::Verb(v) => {
                        matches!(action, Action::OnSelection(w) if w.kind() == v.kind())
                    }
                    Intent::Tab { delta } => *action == Action::CycleTabs(delta),
                    Intent::Reopen(None) => *action == Action::ReopenField,
                    _ => false,
                }
        })
        .map(|b| b.keys)
}

/// The chars some vim row starts with: a `keys.toml` binding of one of
/// them shadows vim's key (spec §4.4, plan M1 Deviation 19).
pub fn vim_first_chars() -> Vec<char> {
    let mut out: Vec<char> = TABLE
        .iter()
        .zip(parsed())
        // A keys.toml letter only ever reaches a navigation surface (spec
        // §4.4), so a field row cannot be shadowed (plan M3a Deviation 18).
        .filter(|(b, _)| b.profile.has(KeyProfile::Vim) && b.on.overlaps(Surfaces::NAV))
        .filter_map(|(_, keys)| match keys.first() {
            Some(Tok::Char(c)) => Some(*c),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether a digit builds a count on `surface` in the vim profile: a
/// navigation surface (lists and panes) or the response's V, where some
/// row repeats or reads one (plan M1 Deviation 22). The app-wide `u` and
/// `ctrl+r` rows repeat on every surface, so the navigation filter keeps
/// prompts, forms, the body and the testbed out. Which navigation
/// surfaces qualify is derived from [`TABLE`], so it cannot drift from
/// the rows.
pub fn takes_count(surface: Surface) -> bool {
    Surfaces::KEYED.has(surface)
        && TABLE.iter().any(|b| {
            b.profile.has(KeyProfile::Vim)
                && b.on.has(surface)
                && matches!(b.count, CountRule::Repeat | CountRule::Pass)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_match(keys: &str, class: Surfaces) -> Match {
        lookup_class(&notation::parse(keys), KeyProfile::Vim, class)
    }

    /// Spec §3.7 and plan M3a Deviations 9 and 11: the rows an open field
    /// checks in idle Normal.
    #[test]
    fn field_rows_leave_note_and_swallow() {
        let line = Surfaces::FIELD_LINE;
        let body = Surfaces::FIELD_BODY;
        assert!(
            matches!(class_match("j", line), Match::Complete(b) if matches!(b.intent, Intent::Leave(n) if n == DOWN))
        );
        assert!(
            matches!(class_match("<Up>", line), Match::Complete(b) if matches!(b.intent, Intent::Leave(_)))
        );
        assert!(
            matches!(class_match("<C-d>", line), Match::Complete(b) if b.count == CountRule::Ignore)
        );
        assert!(matches!(class_match("j", body), Match::NoMatch), "j is a motion in the body");
        assert!(matches!(class_match("q", line), Match::Prefix));
        for keys in ["qa", "q<CR>", "q<C-c>"] {
            assert!(
                matches!(class_match(keys, line), Match::Complete(b) if matches!(b.intent, Intent::Recording)),
                "{keys}"
            );
        }
        for keys in ["@a", "@<Esc>"] {
            assert!(
                matches!(class_match(keys, line), Match::Complete(b) if matches!(b.intent, Intent::Note(NO_MACROS))),
                "{keys}"
            );
        }
        for keys in ["ma", "'a", "`a"] {
            assert!(
                matches!(class_match(keys, body), Match::Complete(b) if matches!(b.intent, Intent::Note(NO_MARKS))),
                "{keys}"
            );
        }
        assert!(
            matches!(class_match("ZZ", line), Match::Complete(b) if matches!(b.intent, Intent::Swallow)),
            "a dialog field"
        );
        let screen_line = line.or(Surfaces::FIELD_SCREEN);
        assert!(
            matches!(class_match("ZZ", screen_line), Match::Complete(b) if matches!(b.intent, Intent::CloseThen(_)))
        );
        assert!(
            matches!(class_match("ZQ", screen_line), Match::Complete(b) if matches!(b.intent, Intent::Act(_)))
        );
        assert!(
            matches!(class_match("Zx", screen_line), Match::Complete(b) if matches!(b.intent, Intent::Swallow))
        );
        assert!(
            matches!(class_match(":", body), Match::Complete(b) if matches!(b.intent, Intent::Act(_)))
        );
        assert!(matches!(class_match("x", line), Match::NoMatch), "the engine's");
        assert!(
            matches!(class_match("g", line), Match::NoMatch),
            "a dialog field's g is the engine's"
        );
    }

    /// Plan M3a Deviation 10: a specific row beats an `<any>` row of the
    /// same length (M4's `ZZ` over `Z<any>`).
    #[test]
    fn a_specific_row_beats_an_any_row() {
        static ROWS: [Binding; 2] = [
            row("Z<any>", Surfaces::FIELDS, Intent::Swallow, CountRule::Ignore),
            row("ZZ", Surfaces::FIELDS, Intent::Note("zz"), CountRule::Ignore),
        ];
        let keys: Vec<&'static [Tok]> =
            ROWS.iter().map(|b| &*Box::leak(notation::parse(b.keys).into_boxed_slice())).collect();
        let rows = || ROWS.iter().zip(keys.iter().copied());
        let typed = |s: &str| notation::parse(s);
        assert!(
            matches!(best(rows(), &typed("ZZ"), Surfaces::FIELD_LINE), Match::Complete(b) if b.keys == "ZZ")
        );
        assert!(
            matches!(best(rows(), &typed("Zx"), Surfaces::FIELD_LINE), Match::Complete(b) if b.keys == "Z<any>")
        );
        assert!(matches!(best(rows(), &typed("Z"), Surfaces::FIELD_LINE), Match::Prefix));
    }

    /// Plan M5b Deviations 9 and 11: the fold keys are the sidebar's; every
    /// other list says it has no fold, so `za` never becomes `a`.
    #[test]
    fn the_fold_keys_are_the_sidebars_and_a_note_elsewhere() {
        use crate::components::manage::ManageTab;
        let complete =
            |keys: &str, s: Surface| match lookup(&notation::parse(keys), KeyProfile::Vim, s) {
                Match::Complete(b) => Some(b.intent),
                _ => None,
            };
        assert!(matches!(complete("zc", Surface::Sidebar), Some(Intent::Counted(_))));
        assert!(matches!(complete("zM", Surface::Sidebar), Some(Intent::Act(_))));
        for s in [
            Surface::VarList,
            Surface::Settings,
            Surface::ManageList(ManageTab::Environments),
            Surface::UrlLine,
        ] {
            assert!(matches!(complete("za", s), Some(Intent::Note(t)) if t == NO_FOLD), "{s:?}");
            assert!(matches!(complete("zR", s), Some(Intent::Swallow)), "{s:?}");
        }
        assert!(
            matches!(
                lookup(&notation::parse("zc"), KeyProfile::Arrows, Surface::Sidebar),
                Match::NoMatch
            ),
            "vim's"
        );
    }

    /// Plan M3a Deviation 18: a keys.toml letter never reaches a field, so
    /// only navigation rows can be shadowed.
    #[test]
    fn shadow_chars_come_from_navigation_rows_only() {
        let chars = vim_first_chars();
        for c in ['q', '@', '\'', '`', '"', 'Z', 'j', 'd'] {
            assert!(chars.contains(&c), "{c} starts a navigation row now (plan M4b)");
        }
        assert!(!chars.contains(&'x'), "x starts no row");
    }

    /// Spec §3.4: in the arrows profile the machine never leaves Idle.
    #[test]
    fn arrows_rows_are_one_key_long() {
        for (b, keys) in TABLE.iter().zip(parsed()) {
            if b.profile.has(KeyProfile::Arrows) {
                assert_eq!(keys.len(), 1, "{}", b.keys);
            }
        }
    }

    #[test]
    fn field_classes() {
        use crate::keyroute::FieldId;
        let screen_line = Surfaces::FIELD_LINE.or(Surfaces::FIELD_SCREEN);
        assert_eq!(field_class(FieldId::Url, Surface::UrlLine), screen_line);
        assert_eq!(
            field_class(
                FieldId::TableCell { row: 0, col: 1 },
                Surface::Table(crate::components::editor::EditorTab::Headers)
            ),
            screen_line
        );
        assert_eq!(
            field_class(FieldId::Body, Surface::Body),
            Surfaces::FIELD_BODY.or(Surfaces::FIELD_SCREEN)
        );
        assert_eq!(
            field_class(FieldId::ModalInput(0), Surface::Prompt),
            Surfaces::FIELD_LINE.or(Surfaces::FIELD_PROMPT)
        );
        assert_eq!(field_class(FieldId::ModalInput(1), Surface::Form), Surfaces::FIELD_LINE);
        for id in [FieldId::Palette, FieldId::Jq, FieldId::Search, FieldId::FilePicker] {
            assert_eq!(field_class(id, Surface::Picker), Surfaces::FIELD_QUERY, "{id:?}");
        }
        assert!(
            matches!(
                lookup_class(&notation::parse("j"), KeyProfile::Vim, Surfaces::FIELD_QUERY),
                Match::NoMatch
            ),
            "no row in a query box"
        );
    }

    #[test]
    fn no_two_rows_collide() {
        // Equal keys collide, and so does a complete row that is a strict
        // prefix of another (`<any>` matching anything). Two rows of the same
        // length that overlap only through `<any>` are allowed: the specific
        // one wins (plan M3a Deviation 10).
        let prefix_of = |a: &[Tok], b: &[Tok]| a.len() < b.len() && fits(&b[..a.len()], a);
        let classes =
            Surface::SAMPLES.iter().map(|s| (format!("{s:?}"), Surfaces::of(*s))).chain([
                ("FIELD_LINE".to_string(), Surfaces::FIELD_LINE),
                ("FIELD_BODY".to_string(), Surfaces::FIELD_BODY),
                ("FIELD_PROMPT".to_string(), Surfaces::FIELD_LINE.or(Surfaces::FIELD_PROMPT)),
                ("FIELD_QUERY".to_string(), Surfaces::FIELD_QUERY),
                ("FIELD_SCREEN_LINE".to_string(), Surfaces::FIELD_LINE.or(Surfaces::FIELD_SCREEN)),
                ("FIELD_SCREEN_BODY".to_string(), Surfaces::FIELD_BODY.or(Surfaces::FIELD_SCREEN)),
            ]);
        for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
            for (name, class) in classes.clone() {
                let rows: Vec<_> = TABLE
                    .iter()
                    .zip(parsed())
                    .filter(|(b, _)| b.profile.has(profile) && b.on.overlaps(class))
                    .collect();
                for (i, (a, ka)) in rows.iter().enumerate() {
                    for (b, kb) in &rows[i + 1..] {
                        assert_ne!(ka, kb, "{} twice on {name}", a.keys);
                        assert!(
                            !prefix_of(ka, kb) && !prefix_of(kb, ka),
                            "{} and {} on {name}: a complete row is a prefix of another",
                            a.keys,
                            b.keys
                        );
                    }
                }
            }
        }
    }

    /// Plan M5a Task 1 (Deviation 16): each window and tab key is one row,
    /// for the navigation surfaces and a screen field alike.
    #[test]
    fn window_and_tab_keys_are_one_row_each() {
        for keys in ["gt", "gT", "<C-w>h", "<C-w><C-w>", "<C-w>p", "<C-w><Left>", "<C-w><Up>"] {
            assert_eq!(TABLE.iter().filter(|b| b.keys == keys).count(), 1, "{keys}");
        }
    }

    #[test]
    fn counts_fit_their_intents() {
        use crate::action::Verb;
        for b in TABLE {
            let edge = matches!(b.intent, Intent::Edge { .. });
            let counted_verb =
                matches!(b.intent, Intent::Verb(Verb::Delete { .. } | Verb::Put { .. }));
            if edge {
                assert_eq!(b.count, CountRule::Pass, "{}: an Edge row reads its count", b.keys);
            }
            let tab = matches!(b.intent, Intent::Tab { .. });
            if b.count == CountRule::Pass {
                assert!(
                    edge || counted_verb
                        || tab
                        || matches!(
                            b.intent,
                            Intent::Motion(_)
                                | Intent::Extend(_)
                                | Intent::Counted(_)
                                | Intent::Edit { .. }
                        ),
                    "{}: Pass is for Edge rows, dd, p, the tab keys, the response's motions and counted actions, \
                     and the engine's edits",
                    b.keys
                );
            }
            if matches!(
                b.intent,
                Intent::Note(_)
                    | Intent::Swallow
                    | Intent::Open { .. }
                    | Intent::Reopen(_)
                    | Intent::Pane(_)
                    | Intent::Recording
                    | Intent::CloseThen(_)
            ) {
                assert_eq!(b.count, CountRule::Ignore, "{}", b.keys);
            }
            if b.profile.has(KeyProfile::Arrows) {
                assert_eq!(b.count, CountRule::Ignore, "{}: arrows never builds a count", b.keys);
            }
        }
    }

    /// Spec §4.6: a verb's keycap is its row's keys, whatever the count.
    #[test]
    fn verb_rows_spell_their_keys() {
        use crate::action::Verb;
        use KeyProfile::{Arrows, Vim};
        let del = Action::OnSelection(Verb::Delete { count: 1 });
        assert_eq!(keys_for(Vim, Surface::Sidebar, &del), Some("dd"));
        assert_eq!(keys_for(Arrows, Surface::Sidebar, &del), Some("d"));
        assert_eq!(
            keys_for(Vim, Surface::Sidebar, &Action::OnSelection(Verb::Delete { count: 3 })),
            Some("dd")
        );
        assert_eq!(keys_for(Vim, Surface::VarGrid, &Action::OnSelection(Verb::Yank)), Some("yy"));
        assert_eq!(keys_for(Arrows, Surface::VarGrid, &Action::OnSelection(Verb::Yank)), Some("c"));
        assert_eq!(
            keys_for(Vim, Surface::VarForm, &Action::OnSelection(Verb::Reveal)),
            None,
            "M4's :reveal"
        );
        assert_eq!(keys_for(Vim, Surface::Sidebar, &Action::CycleTabs(1)), Some("gt"));
        assert_eq!(keys_for(Vim, Surface::Sidebar, &Action::CycleTabs(-1)), Some("gT"));
        assert_eq!(
            keys_for(Arrows, Surface::Sidebar, &Action::CycleTabs(1)),
            None,
            "arrows' tab keys are the keymap's"
        );
        assert!(vim_first_chars().contains(&'d') && vim_first_chars().contains(&'y'));
    }

    #[test]
    fn lookups_find_rows_prefixes_and_misses() {
        use KeyProfile::Vim;
        assert!(
            matches!(lookup(&[Tok::Char('j')], Vim, Surface::Sidebar), Match::Complete(b) if b.keys == "j")
        );
        assert!(matches!(lookup(&[Tok::Char('g')], Vim, Surface::Sidebar), Match::Prefix));
        assert!(matches!(lookup(&[Tok::Char('x')], Vim, Surface::Sidebar), Match::NoMatch));
        assert!(matches!(
            lookup(&[Tok::Char('j')], KeyProfile::Arrows, Surface::Sidebar),
            Match::NoMatch
        ));
        assert!(
            matches!(lookup(&[Tok::Char('j')], Vim, Surface::Picker), Match::Complete(b) if b.keys == "j"),
            "a field-less picker (the dropdown) moves; a query box never reaches the table"
        );
        assert_eq!(keys_for(Vim, Surface::Sidebar, &Action::Undo), Some("u"));
        assert_eq!(keys_for(Vim, Surface::Picker, &Action::Undo), None);
        assert!(vim_first_chars().contains(&'g'));
    }

    #[test]
    fn i_opens_the_field_on_a_manage_detail_pane() {
        use crate::components::manage::ManageTab;
        let detail = Surface::ManageDetail(ManageTab::Environments);
        assert!(
            matches!(lookup(&[Tok::Char('i')], KeyProfile::Vim, detail), Match::Complete(b) if b.keys == "i")
        );
    }

    #[test]
    fn counts_are_taken_where_a_row_takes_one() {
        assert!(takes_count(Surface::Sidebar), "j repeats on a list");
        assert!(takes_count(Surface::Response(crate::components::response::ViewMode::Pretty)));
        assert!(!takes_count(Surface::Dialog), "a dialog's letters answer; digits are not counts");
        assert!(!takes_count(Surface::Picker), "a picker types");
        for s in [Surface::Prompt, Surface::Form, Surface::Other, Surface::Body] {
            assert!(
                !takes_count(s),
                "{s:?} is not a navigation surface; `u` repeating there is not a count"
            );
        }
    }
}
