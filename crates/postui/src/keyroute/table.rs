//! The binding table (piece 4 spec §3.5): one static table for both
//! profiles and every surface. A key's meaning is a lookup here; what a
//! prefix is follows from the rows, so no separate prefix list can drift.
//! M1's rows are the vim profile's motions and its `u`, `ctrl+r`, `:`; M2
//! adds the list verbs in both profiles (spec §5.1–§5.4).

use super::ctx::Surface;
use super::engine_glue::OpenSeat;
use super::notation::{self, Tok};
use crate::action::{Action, PaneDir, Verb};
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
        let mods = if self.ctrl {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::NONE
        };
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
    /// Reopen a dialog's selected text field (plan M3b Deviation 3): in
    /// Insert at this seat, or in Normal (`None`).
    Reopen(Option<OpenSeat>),
    /// `gt` (`delta` 1) and `gT` (-1) (spec §5.8): no count steps the strip
    /// and lands the caret (`FocusEditorContent`); `{N}gt` is `SelectTab`;
    /// `{N}gT` steps back N, as one step of N mod [`TAB_CYCLE`]. `leave`:
    /// typed in an open field, which closes first (for `{N}gt`, only when
    /// tab N is there to select).
    Tab { delta: i8, leave: bool },
    /// A window key (spec §5.8): `PaneMove`. `leave`: typed in an open
    /// field, which closes first, but only when there is a pane to move to.
    Pane { dir: PaneDir, leave: bool },
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

    /// Every surface whose rows or controls the keyboard walks: vim's
    /// motions bind here, and in vim a bare char with no row is
    /// swallowed here (spec §3.4 step 4).
    pub const NAV: Self = Self::SIDEBAR
        .or(Self::URL_LINE)
        .or(Self::METHOD)
        .or(Self::TABLE)
        .or(Self::RESPONSE)
        .or(Self::VAR_LIST)
        .or(Self::VAR_GRID)
        .or(Self::VAR_FORM)
        .or(Self::MANAGE_LIST)
        .or(Self::MANAGE_DETAIL)
        .or(Self::SETTINGS);
    /// The navigation surfaces but the response, whose `gg`/`G` are
    /// ctrl+Home/ctrl+End (its bare Home/End scroll sideways).
    pub const LISTS: Self = Self::NAV.minus(Self::RESPONSE);
    /// Where a row opens on Enter (`o`).
    pub const OPENS_ROW: Self = Self::SIDEBAR
        .or(Self::TABLE)
        .or(Self::VAR_LIST)
        .or(Self::VAR_GRID)
        .or(Self::MANAGE_LIST);
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
    pub const ROWS: Self = Self::SIDEBAR
        .or(Self::TABLE)
        .or(Self::MANAGE_LIST)
        .or(Self::VAR_LIST)
        .or(Self::VAR_GRID);
    /// Where `u`, `ctrl+r` and `:` are the app's (spec §5.11): everywhere
    /// but pickers (they type) and dialogs (their letters answer).
    pub const APP: Self = Self::NAV
        .or(Self::BODY)
        .or(Self::PROMPT)
        .or(Self::FORM)
        .or(Self::OTHER);
    /// The dialogs with a field: a one-field prompt and a form.
    pub const FORMS: Self = Self::PROMPT.or(Self::FORM);

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
        FieldId::ModalInput(_) if surface == Surface::Prompt => Surfaces::FIELD_LINE.or(Surfaces::FIELD_PROMPT),
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
    Binding {
        keys,
        profile: Profiles::VIM,
        on,
        intent,
        count,
    }
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
const fn pane(keys: &'static str, on: Surfaces, dir: PaneDir, leave: bool) -> Binding {
    row(keys, on, Intent::Pane { dir, leave }, CountRule::Ignore)
}

pub const NO_COUNTED_DELETE: &str = "Counted delete works on requests and table rows";
pub const NO_COUNTED_YANK: &str = "Counted yank is not supported";
pub const NO_MACROS: &str = "Macros are not supported";
pub const NO_MARKS: &str = "Marks are not supported";

/// A multiple of every tab strip's length (2–4 tabs): `{N}gT` steps back
/// N mod this, exactly (plan M4a Deviation 8).
pub const TAB_CYCLE: usize = 12;

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

fn redo() -> Action {
    Action::Redo
}

fn palette() -> Action {
    Action::OpenPalette
}

fn confirm_top() -> Action {
    Action::ConfirmTopModal
}

fn cancel_top() -> Action {
    Action::CancelTopModal
}

fn fold_open() -> Action {
    Action::TreeFold(crate::action::FoldOp::Open)
}

fn fold_close() -> Action {
    Action::TreeFold(crate::action::FoldOp::Close)
}

fn fold_toggle() -> Action {
    Action::TreeFold(crate::action::FoldOp::Toggle)
}

fn response_search() -> Action {
    Action::OpenResponseSearch
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

/// Every row. Order does not matter: no complete row is a prefix of
/// another on a shared surface (`no_two_rows_collide`).
pub static TABLE: &[Binding] = &[
    // Key list §6 and §7: moving.
    row("j", Surfaces::NAV, Intent::Key(DOWN), CountRule::Repeat),
    row("k", Surfaces::NAV, Intent::Key(Named::key(KeyCode::Up)), CountRule::Repeat),
    row("h", Surfaces::NAV, Intent::Key(Named::key(KeyCode::Left)), CountRule::Repeat),
    row("l", Surfaces::NAV, Intent::Key(Named::key(KeyCode::Right)), CountRule::Repeat),
    row("gg", Surfaces::LISTS, Intent::Edge { bare: HOME, home: HOME }, CountRule::Pass),
    row("G", Surfaces::LISTS, Intent::Edge { bare: Named::key(KeyCode::End), home: HOME }, CountRule::Pass),
    row(
        "gg",
        Surfaces::RESPONSE,
        Intent::Edge { bare: Named::ctrl(KeyCode::Home), home: Named::ctrl(KeyCode::Home) },
        CountRule::Pass,
    ),
    row(
        "G",
        Surfaces::RESPONSE,
        Intent::Edge { bare: Named::ctrl(KeyCode::End), home: Named::ctrl(KeyCode::Home) },
        CountRule::Pass,
    ),
    row("0", Surfaces::RESPONSE, Intent::Key(HOME), CountRule::Ignore),
    row("$", Surfaces::RESPONSE, Intent::Key(Named::key(KeyCode::End)), CountRule::Ignore),
    // Key list §7 (`R.fold`): the tree's folds. Nothing on Raw or Headers.
    row("zo", Surfaces::RESPONSE, Intent::Act(fold_open), CountRule::Ignore),
    row("zc", Surfaces::RESPONSE, Intent::Act(fold_close), CountRule::Ignore),
    row("za", Surfaces::RESPONSE, Intent::Act(fold_toggle), CountRule::Ignore),
    // Key list §8 (`P.gt`), spec §5.8: every tab strip, on a closed
    // surface or from a screen field's idle Normal (§3.7).
    row("gt", Surfaces::NAV, Intent::Tab { delta: 1, leave: false }, CountRule::Pass),
    row("gT", Surfaces::NAV, Intent::Tab { delta: -1, leave: false }, CountRule::Pass),
    row("gt", Surfaces::FIELD_SCREEN, Intent::Tab { delta: 1, leave: true }, CountRule::Pass),
    row("gT", Surfaces::FIELD_SCREEN, Intent::Tab { delta: -1, leave: true }, CountRule::Pass),
    // Key list §8 (`P.ctrlw`), spec §5.8: the window keys, with Vim's ctrl
    // spellings of the targets (plan M4a Deviation 3). In a screen field a
    // non-target is swallowed, never replayed as an edit (Deviation 4).
    pane("<C-w>h", Surfaces::NAV, PaneDir::Left, false),
    pane("<C-w>j", Surfaces::NAV, PaneDir::Down, false),
    pane("<C-w>k", Surfaces::NAV, PaneDir::Up, false),
    pane("<C-w>l", Surfaces::NAV, PaneDir::Right, false),
    pane("<C-w>w", Surfaces::NAV, PaneDir::Next, false),
    pane("<C-w>W", Surfaces::NAV, PaneDir::Prev, false),
    pane("<C-w>p", Surfaces::NAV, PaneDir::Last, false),
    pane("<C-w><C-h>", Surfaces::NAV, PaneDir::Left, false),
    pane("<C-w><C-j>", Surfaces::NAV, PaneDir::Down, false),
    pane("<C-w><C-k>", Surfaces::NAV, PaneDir::Up, false),
    pane("<C-w><C-l>", Surfaces::NAV, PaneDir::Right, false),
    pane("<C-w><C-w>", Surfaces::NAV, PaneDir::Next, false),
    pane("<C-w><C-p>", Surfaces::NAV, PaneDir::Last, false),
    pane("<C-w>h", Surfaces::FIELD_SCREEN, PaneDir::Left, true),
    pane("<C-w>j", Surfaces::FIELD_SCREEN, PaneDir::Down, true),
    pane("<C-w>k", Surfaces::FIELD_SCREEN, PaneDir::Up, true),
    pane("<C-w>l", Surfaces::FIELD_SCREEN, PaneDir::Right, true),
    pane("<C-w>w", Surfaces::FIELD_SCREEN, PaneDir::Next, true),
    pane("<C-w>W", Surfaces::FIELD_SCREEN, PaneDir::Prev, true),
    pane("<C-w>p", Surfaces::FIELD_SCREEN, PaneDir::Last, true),
    pane("<C-w><C-h>", Surfaces::FIELD_SCREEN, PaneDir::Left, true),
    pane("<C-w><C-j>", Surfaces::FIELD_SCREEN, PaneDir::Down, true),
    pane("<C-w><C-k>", Surfaces::FIELD_SCREEN, PaneDir::Up, true),
    pane("<C-w><C-l>", Surfaces::FIELD_SCREEN, PaneDir::Right, true),
    pane("<C-w><C-w>", Surfaces::FIELD_SCREEN, PaneDir::Next, true),
    pane("<C-w><C-p>", Surfaces::FIELD_SCREEN, PaneDir::Last, true),
    row("<C-w><any>", Surfaces::FIELD_SCREEN, Intent::Swallow, CountRule::Ignore),
    // Key list §6: opening.
    row("o", Surfaces::OPENS_ROW, Intent::Key(Named::key(KeyCode::Enter)), CountRule::Ignore),
    row("i", Surfaces::OPENS_FIELD, Intent::Open { key: ENTER, seat: OpenSeat::Keep }, CountRule::Ignore),
    // Spec §5.6: the closed URL line's own openings.
    row("a", Surfaces::URL_LINE, Intent::Open { key: ENTER, seat: OpenSeat::After }, CountRule::Ignore),
    row("I", Surfaces::URL_LINE, Intent::Open { key: ENTER, seat: OpenSeat::FirstNonBlank }, CountRule::Ignore),
    row("A", Surfaces::URL_LINE, Intent::Open { key: ENTER, seat: OpenSeat::End }, CountRule::Ignore),
    // Key list §7 and spec §4.5 (plan M4a Deviation 11): the response's
    // letters, the same in both profiles; arrows' `c` copies a header.
    both("/", Surfaces::RESPONSE, Intent::Act(response_search)),
    both("n", Surfaces::RESPONSE, Intent::Act(next_match)),
    both("N", Surfaces::RESPONSE, Intent::Act(prev_match)),
    arrows("c", Surfaces::RESPONSE, Intent::Act(copy_header)),
    // Key list §6, Out (parked by the user): a list has no search yet.
    row("/", Surfaces::SEARCHLESS, Intent::Note("No search in this list yet"), CountRule::Ignore),
    // Key list §6 and §9: the app's undo, redo and command palette.
    row("u", Surfaces::APP, Intent::Act(undo), CountRule::Repeat),
    row("<C-r>", Surfaces::APP, Intent::Act(redo), CountRule::Repeat),
    row(":", Surfaces::APP, Intent::Act(palette), CountRule::Ignore),
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
    row(":", Surfaces::FIELDS, Intent::Act(palette), CountRule::Ignore),
    row("j", Surfaces::FIELD_LINE, Intent::Leave(DOWN), CountRule::Repeat),
    row("k", Surfaces::FIELD_LINE, Intent::Leave(UP), CountRule::Repeat),
    row("<Down>", Surfaces::FIELD_LINE, Intent::Leave(DOWN), CountRule::Repeat),
    row("<Up>", Surfaces::FIELD_LINE, Intent::Leave(UP), CountRule::Repeat),
    row("<C-d>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('d'))), CountRule::Ignore),
    row("<C-u>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('u'))), CountRule::Ignore),
    row("<C-f>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('f'))), CountRule::Ignore),
    row("<C-b>", Surfaces::FIELD_LINE, Intent::Leave(Named::ctrl(KeyCode::Char('b'))), CountRule::Ignore),
    row("q<any>", Surfaces::FIELDS, Intent::Note(NO_MACROS), CountRule::Ignore),
    row("@<any>", Surfaces::FIELDS, Intent::Note(NO_MACROS), CountRule::Ignore),
    row("m<any>", Surfaces::FIELDS, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("'<any>", Surfaces::FIELDS, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("`<any>", Surfaces::FIELDS, Intent::Note(NO_MARKS), CountRule::Ignore),
    row("Z<any>", Surfaces::FIELDS, Intent::Swallow, CountRule::Ignore),
    // Spec §5.10, key list D.prompt (plan M3b Deviations 7 and 8): a
    // one-field prompt confirms on Enter from anywhere, field or not; its
    // field's Esc in idle Normal cancels (Insert's Esc is the engine's).
    // Enter typed in Insert reaches the first row too: the engine hands a
    // one-line field's Enter back.
    row("<CR>", Surfaces::PROMPT.or(Surfaces::FIELD_PROMPT), Intent::Act(confirm_top), CountRule::Ignore),
    row("<Esc>", Surfaces::FIELD_PROMPT, Intent::Act(cancel_top), CountRule::Ignore),
    // Spec §5.10 and §4.5: a dialog's stops, its buttons and a field-less
    // picker (the dropdown) move with vim's keys; modal.rs no longer
    // aliases them. A dialog's own answer letters come first (`answers`).
    row("j", Surfaces::FORMS.or(Surfaces::DIALOG).or(Surfaces::PICKER), Intent::Key(DOWN), CountRule::Ignore),
    row("k", Surfaces::FORMS.or(Surfaces::DIALOG).or(Surfaces::PICKER), Intent::Key(UP), CountRule::Ignore),
    row("h", Surfaces::FORMS.or(Surfaces::DIALOG), Intent::Key(LEFT), CountRule::Ignore),
    row("l", Surfaces::FORMS.or(Surfaces::DIALOG), Intent::Key(RIGHT), CountRule::Ignore),
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
fn best<'a>(rows: impl IntoIterator<Item = (&'static Binding, &'a [Tok])>, buf: &[Tok], class: Surfaces) -> Match {
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
    TABLE
        .iter()
        .find(|b| {
            b.profile.has(profile)
                && b.on.has(surface)
                && match b.intent {
                    Intent::Act(f) => f() == *action,
                    Intent::Verb(v) => matches!(action, Action::OnSelection(w) if w.kind() == v.kind()),
                    Intent::Tab { delta, leave: false } => *action == Action::CycleTabs(delta),
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
/// navigation surface (lists and panes) where some row repeats or reads
/// one (plan M1 Deviation 22). The app-wide `u` and `ctrl+r` rows repeat
/// on every surface, so the navigation filter keeps prompts, forms, the
/// body and the testbed out. Which navigation surfaces qualify is
/// derived from [`TABLE`], so it cannot drift from the rows.
pub fn takes_count(surface: Surface) -> bool {
    Surfaces::NAV.has(surface)
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
        assert!(matches!(class_match("j", line), Match::Complete(b) if matches!(b.intent, Intent::Leave(n) if n == DOWN)));
        assert!(matches!(class_match("<Up>", line), Match::Complete(b) if matches!(b.intent, Intent::Leave(_))));
        assert!(matches!(class_match("<C-d>", line), Match::Complete(b) if b.count == CountRule::Ignore));
        assert!(matches!(class_match("j", body), Match::NoMatch), "j is a motion in the body");
        assert!(matches!(class_match("q", line), Match::Prefix));
        for keys in ["qa", "q<CR>", "q<C-c>", "@a", "@<Esc>"] {
            assert!(
                matches!(class_match(keys, line), Match::Complete(b) if matches!(b.intent, Intent::Note(NO_MACROS))),
                "{keys}"
            );
        }
        for keys in ["ma", "'a", "`a"] {
            assert!(matches!(class_match(keys, body), Match::Complete(b) if matches!(b.intent, Intent::Note(NO_MARKS))), "{keys}");
        }
        assert!(matches!(class_match("ZZ", line), Match::Complete(b) if matches!(b.intent, Intent::Swallow)));
        assert!(matches!(class_match(":", body), Match::Complete(b) if matches!(b.intent, Intent::Act(_))));
        assert!(matches!(class_match("x", line), Match::NoMatch), "the engine's");
        assert!(matches!(class_match("g", line), Match::NoMatch), "a dialog field's g is the engine's");
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
        assert!(matches!(best(rows(), &typed("ZZ"), Surfaces::FIELD_LINE), Match::Complete(b) if b.keys == "ZZ"));
        assert!(matches!(best(rows(), &typed("Zx"), Surfaces::FIELD_LINE), Match::Complete(b) if b.keys == "Z<any>"));
        assert!(matches!(best(rows(), &typed("Z"), Surfaces::FIELD_LINE), Match::Prefix));
    }

    /// Plan M3a Deviation 18: a keys.toml letter never reaches a field, so
    /// only navigation rows can be shadowed.
    #[test]
    fn shadow_chars_come_from_navigation_rows_only() {
        let chars = vim_first_chars();
        for c in ['q', '@', '\'', '`', 'Z'] {
            assert!(!chars.contains(&c), "{c} starts only a field row");
        }
        assert!(chars.contains(&'j'), "j is a list motion too");
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
        assert_eq!(field_class(FieldId::TableCell { row: 0, col: 1 }, Surface::Table(crate::components::editor::EditorTab::Headers)), screen_line);
        assert_eq!(field_class(FieldId::Body, Surface::Body), Surfaces::FIELD_BODY.or(Surfaces::FIELD_SCREEN));
        assert_eq!(field_class(FieldId::ModalInput(0), Surface::Prompt), Surfaces::FIELD_LINE.or(Surfaces::FIELD_PROMPT));
        assert_eq!(field_class(FieldId::ModalInput(1), Surface::Form), Surfaces::FIELD_LINE);
        for id in [FieldId::Palette, FieldId::Jq, FieldId::Search, FieldId::FilePicker] {
            assert_eq!(field_class(id, Surface::Picker), Surfaces::FIELD_QUERY, "{id:?}");
        }
        assert!(matches!(lookup_class(&notation::parse("j"), KeyProfile::Vim, Surfaces::FIELD_QUERY), Match::NoMatch), "no row in a query box");
    }

    /// Plan M4a Deviation 8: `{N}gT` runs as one step of N mod `TAB_CYCLE`,
    /// exact only while every strip's length divides it. The lengths are
    /// read from the strips themselves (amendment R7).
    #[test]
    fn every_tab_strip_divides_the_tab_cycle() {
        use crate::components::editor::EditorTab;
        use crate::components::manage::ManageTab;
        use crate::components::response::{Response, ResponseState};
        let response = |body: &str| {
            let mut r = Response::default();
            r.set_state(
                ResponseState::Ready(Box::new(crate::http::ResponseData {
                    status: 200,
                    url: "https://x.test/a".into(),
                    headers: Vec::new(),
                    sent_headers: Vec::new(),
                    body: body.into(),
                    ttfb: std::time::Duration::ZERO,
                    elapsed: std::time::Duration::ZERO,
                    size: body.len(),
                    content_type: Some("application/json".into()),
                })),
                0,
            );
            r.tab_modes().expect("a ready view").len()
        };
        let (tree, no_tree) = (response(r#"{"a": 1}"#), response("not json"));
        assert_eq!((tree, no_tree), (3, 2), "a JSON body has a Tree tab");
        for len in [ManageTab::ALL.len(), EditorTab::COUNT, tree, no_tree] {
            assert_eq!(TAB_CYCLE % len, 0, "a strip of {len}");
        }
    }

    #[test]
    fn no_two_rows_collide() {
        // Equal keys collide, and so does a complete row that is a strict
        // prefix of another (`<any>` matching anything). Two rows of the same
        // length that overlap only through `<any>` are allowed: the specific
        // one wins (plan M3a Deviation 10).
        let prefix_of = |a: &[Tok], b: &[Tok]| a.len() < b.len() && fits(&b[..a.len()], a);
        let classes = Surface::SAMPLES
            .iter()
            .map(|s| (format!("{s:?}"), Surfaces::of(*s)))
            .chain([
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

    #[test]
    fn counts_fit_their_intents() {
        use crate::action::Verb;
        for b in TABLE {
            let edge = matches!(b.intent, Intent::Edge { .. });
            let counted_verb = matches!(b.intent, Intent::Verb(Verb::Delete { .. } | Verb::Put { .. }));
            if edge {
                assert_eq!(b.count, CountRule::Pass, "{}: an Edge row reads its count", b.keys);
            }
            let tab = matches!(b.intent, Intent::Tab { .. });
            if b.count == CountRule::Pass {
                assert!(edge || counted_verb || tab, "{}: Pass is for Edge rows, dd, p and the tab keys", b.keys);
            }
            if matches!(
                b.intent,
                Intent::Note(_) | Intent::Swallow | Intent::Open { .. } | Intent::Reopen(_) | Intent::Pane { .. }
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
        assert_eq!(keys_for(Vim, Surface::Sidebar, &Action::OnSelection(Verb::Delete { count: 3 })), Some("dd"));
        assert_eq!(keys_for(Vim, Surface::VarGrid, &Action::OnSelection(Verb::Yank)), Some("yy"));
        assert_eq!(keys_for(Arrows, Surface::VarGrid, &Action::OnSelection(Verb::Yank)), Some("c"));
        assert_eq!(keys_for(Vim, Surface::VarForm, &Action::OnSelection(Verb::Reveal)), None, "M4's :reveal");
        assert_eq!(keys_for(Vim, Surface::Sidebar, &Action::CycleTabs(1)), Some("gt"));
        assert_eq!(keys_for(Vim, Surface::Sidebar, &Action::CycleTabs(-1)), Some("gT"));
        assert_eq!(keys_for(Arrows, Surface::Sidebar, &Action::CycleTabs(1)), None, "arrows' tab keys are the keymap's");
        assert!(vim_first_chars().contains(&'d') && vim_first_chars().contains(&'y'));
    }

    #[test]
    fn lookups_find_rows_prefixes_and_misses() {
        use KeyProfile::Vim;
        assert!(matches!(lookup(&[Tok::Char('j')], Vim, Surface::Sidebar), Match::Complete(b) if b.keys == "j"));
        assert!(matches!(lookup(&[Tok::Char('g')], Vim, Surface::Sidebar), Match::Prefix));
        assert!(matches!(lookup(&[Tok::Char('x')], Vim, Surface::Sidebar), Match::NoMatch));
        assert!(matches!(lookup(&[Tok::Char('j')], KeyProfile::Arrows, Surface::Sidebar), Match::NoMatch));
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
            assert!(!takes_count(s), "{s:?} is not a navigation surface; `u` repeating there is not a count");
        }
    }
}
