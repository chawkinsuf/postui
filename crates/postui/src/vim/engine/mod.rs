//! The vim profile's text engine (piece 3; design:
//! `docs/superpowers/specs/2026-09-27-vim-text-engine-design.md`).
//!
//! One engine runs every text surface in the vim profile: one-line fields
//! through [`OneLineBuf`] and the request body through [`BodyBuf`]. It owns
//! the vim semantics (grammar, motions, operators, Insert, Visual, undo,
//! registers, `.`) and makes no app decisions: a key it does not own comes
//! back as [`Outcome::Declined`] and piece 4 routes it. Only `buf.rs` names
//! app types (`tests.rs` enforces it).
//!
//! Vim 9.1 is the specification: `tests/vim_conformance.rs` replays every
//! case of the generated golden file against this engine.

pub mod buf;
mod case;
mod case_table;
mod class;
mod class_table;
mod history;
mod indent;
mod insert;
mod keys;
mod motion;
mod number;
mod object;
mod op;
mod register;
mod search;
pub mod settings;
#[cfg(test)]
mod tests;
mod view;
mod visual;

pub use buf::{BodyBuf, BodyVisual, GuiSel, OneLineBuf, Paint, Pos, TextBuf};
pub use register::{RegKind, Register, Registers};
pub use search::Dir;

use insert::{InsertKey, RegPending};
use keys::{Cmd, Motion, Op, ParseCx, Pending, Reach, Step, VisualOp};
use ratatui::crossterm::event::KeyEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Char,
    Line,
}

/// The engine's mode (spec §4.1). Global: only one buffer has the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    /// An `R` session: typing overwrites (plan 3c).
    Replace,
    /// Insert `ctrl+o`: one Normal command, then the session resumes
    /// (Vim's `niI`; `replace` is `niR`).
    InsertNormal { replace: bool },
    Visual(Shape),
}

/// How a buffer that just got the caret starts (spec §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Entered by keyboard.
    Normal,
    /// Made by an add verb, or a prompt: Esc goes to Normal, and a second
    /// Esc is declined.
    Insert,
    /// A query box (`:` line, palette, jq bar, search bar, pickers): Insert
    /// keys work, there is no Normal layer, and Esc is declined.
    InsertOnly,
}

/// Where [`Engine::enter`] seats the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seat {
    Keep,
    ColZero,
    End,
    FirstNonBlank,
}

/// What just happened, for [`Engine::settle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// A key piece 4 handled itself.
    Key,
    /// A click, or a release with no sweep live.
    Click,
    /// A text sweep is in progress: the mouse owns the selection.
    Sweep,
    /// The sweep just ended.
    Release,
}

/// Footer text: a key a vim user expects but the engine does not do
/// (`Unsupported`), or a status message such as a search's (`Message`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Unsupported(String),
    /// A status message (the search messages): shown like a note, nothing is unsupported.
    Message(String),
}

/// Something only the app can do (tier 2: `"+y`, plan 3c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRequest {
    CopyToClipboard(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The engine used the key: an edit, a move, a mode change, or one
    /// more key of a half-typed command.
    Consumed { changed: bool, note: Option<Note>, request: Option<AppRequest> },
    /// Not the engine's. `keys` holds every key the engine swallowed for
    /// it, the declined one included (`[g, t]`), and `count` the count
    /// typed before them. Engine state is as if they were never typed.
    Declined { count: Option<usize>, keys: Vec<KeyEvent> },
}

impl Outcome {
    pub(crate) fn consumed() -> Self {
        Outcome::Consumed { changed: false, note: None, request: None }
    }
}

/// What the engine needs to know about the view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewCtx {
    /// Rows the body shows (tier 2 scroll keys, plan 3c). `None` for a
    /// one-line field.
    pub viewport_rows: Option<usize>,
}

/// The buffer the caret is in, with the state Vim keeps for it.
pub struct Target<'a, B: TextBuf> {
    pub buf: &'a mut B,
    pub state: &'a mut BufState,
}

/// What Vim keeps per buffer (spec §4.1): the app owns one per live buffer.
#[derive(Debug, Default)]
pub struct BufState {
    text_at_start: Option<String>,
    edited: bool,
    /// The text when the buffer last lost the caret ([`Engine::leave`]).
    /// The next [`Engine::enter`] drops the history if the text differs
    /// (reload, format, app undo, `$EDITOR`; spec §3.11). `None` while the
    /// buffer has the caret. [`Engine::carry`] leaves its source buffer
    /// the same way, so it records the text too.
    text_at_leave: Option<String>,
    /// Vim's `w_curswant` (the column `j` and `k` aim for) with the caret it
    /// was recorded at; `None` is Vim's `w_set_curswant = TRUE`. It is
    /// trusted only while the caret is still there.
    ///
    /// Every command except `j`, `k`, `$` and refused motions must call
    /// [`BufState::forget_want`]: Vim resets `curswant` on them, including
    /// edits that leave the caret in place (`rX`), which the position check
    /// cannot see. Motions set it through [`BufState::set_want`].
    curswant: Option<(motion::Want, Pos)>,
    /// Vim's cached `w_virtcol`, as the tab rule (does a caret on a tab
    /// count from its last cell?) in force when it was last computed. Vim
    /// recomputes it only when the caret moves or the text changes
    /// (`check_cursor_moved()`), when Insert starts (`edit()` calls
    /// `curs_columns(TRUE)`) or ends (`ins_esc()` drops it on a tab), and
    /// after any undo or redo (`u_undoredo()` calls `changed_lines()`). So
    /// a Visual mode change alone (`v`, `Esc`) keeps the old value, and the
    /// next `w_curswant` is read from it. `None`: computed afresh, in the
    /// mode of the moment, when next needed.
    virtcol: Option<(Pos, bool)>,
    pub(crate) history: history::History,
    /// Vim's window-local 'scroll': the half page a counted ctrl+d/ctrl+u set; None is half the height.
    pub(crate) scroll: Option<usize>,
}

impl BufState {
    /// The wanted column when it is still valid for a caret at `at`.
    pub(crate) fn want(&self, at: Pos) -> Option<motion::Want> {
        self.curswant.filter(|&(_, p)| p == at).map(|(w, _)| w)
    }

    pub(crate) fn set_want(&mut self, want: motion::Want, at: Pos) {
        self.curswant = Some((want, at));
    }

    /// Vim's `w_set_curswant = TRUE`: recompute from the caret next time.
    pub(crate) fn forget_want(&mut self) {
        self.curswant = None;
    }

    /// The cached tab rule when the cache is for a caret at `at`.
    pub(crate) fn cached_tab_rule(&self, at: Pos) -> Option<bool> {
        self.virtcol.filter(|&(p, _)| p == at).map(|(_, t)| t)
    }

    /// Drops the cached `w_virtcol` (see the field).
    pub(crate) fn forget_virtcol(&mut self) {
        self.virtcol = None;
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// The buffer gets the caret with `text` in it: a history that no
    /// longer fits the text is dropped (§3.11), and the start text noted.
    fn begin_session(&mut self, text: String) {
        if self.text_at_leave.take().is_some_and(|left| left != text) {
            self.history.clear();
        }
        self.text_at_start = Some(text);
        self.edited = false;
    }

    /// Whether `u` has anything left in this buffer.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Whether the text changed since the last [`Engine::enter`].
    pub fn edited(&self) -> bool {
        self.edited
    }

    /// The text when this buffer last got the caret; `None` before that.
    pub fn text_at_start(&self) -> Option<&str> {
        self.text_at_start.as_deref()
    }

    /// Forgets everything: history, the session start, the edited flag.
    /// A one-line field calls this when it closes (spec §3.11).
    pub fn end_session(&mut self) {
        *self = Self::default();
    }
}

/// The last change, for `.` (spec §3.12): Vim's redo buffer.
#[derive(Debug, Clone)]
struct Dot {
    cmd: Cmd,
    /// The keys an Insert session typed after `cmd` opened it.
    insert: Option<Vec<InsertKey>>,
    /// A Visual command's selection size, replayed from the caret.
    visual: Option<VisualSize>,
}

/// The last search (Vim's `spats[0]`): global, so `n` in a field repeats a
/// search made in the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LastSearch {
    pub text: String,
    pub dir: Dir,
}

/// A pending Insert restart (Vim's `restart_edit`, `ins_at_eol`, `o_lnum`):
/// `ctrl+o` left Insert for one Normal command.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Restart {
    pub(crate) replace: bool,
    /// The Insert caret was past the line's end, on `row`.
    pub(crate) at_eol: bool,
    pub(crate) row: usize,
    /// `.` repeats the change before the insert (Vim's old redo buffer),
    /// not the insert itself.
    pub(crate) old_redo: bool,
}

/// How big a Visual selection was, for `.` (Vim's `resel_VIsual_*`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct VisualSize {
    pub line: bool,
    pub rows: usize,
    /// Charwise, in virtual columns (Vim's `resel_VIsual_vcol`): the width
    /// on one row, else the last cell of the end on the last row. The end
    /// on a line's end counts as one cell there.
    pub cols: usize,
    /// The wanted column was the line's end (`$`, Vim's `MAXCOL`).
    pub to_end: bool,
}

/// The state Vim keeps globally (spec §4.1): the mode, the half-typed
/// command, registers, `.`, the last find. The app owns one.
#[derive(Debug, Default)]
pub struct Engine {
    mode: Mode,
    pending: Pending,
    /// The fixed end of the Visual selection (the caret is the moving end).
    visual: Option<Pos>,
    regs: Registers,
    /// The last `f` `t` `F` `T` (global, as in Vim), for `;` and `,`.
    last_find: Option<(keys::FindKind, char)>,
    /// The open Insert session; `None` outside Insert.
    insert: Option<insert::Session>,
    /// `Start::InsertOnly` (spec §4.2): Esc is declined.
    insert_only: bool,
    /// The last change, global as in Vim: `.` in another buffer repeats it.
    dot: Option<Dot>,
    /// `.` is replaying a Visual command (Vim's `redo_VIsual_busy`), which
    /// leaves the record as it was.
    replaying_visual: bool,
    /// The size of the selection the last Visual operator acted on.
    last_visual_size: Option<VisualSize>,
    /// Where the engine last left the caret (after a key, a paste or a
    /// session edge). [`Engine::settle`] compares it with the caret to see
    /// that a click or a key piece 4 handled moved the caret in Insert,
    /// which splits the session as Vim's `ins_mouse()` does.
    rested: Option<Pos>,
    /// Insert `ctrl+r` waiting for its register name.
    reg_pending: Option<RegPending>,
    /// Insert `ctrl+o` in progress: Insert resumes after one command.
    restart: Option<Restart>,
    /// The change before `dot` (Vim's `old_redobuff`), for `.` inside `ctrl+o`.
    dot_prev: Option<Dot>,
    /// What the last command asks the app to do; `handle` takes it.
    request: Option<AppRequest>,
    /// A message the last command left; `handle` takes it (a note from a
    /// motion, a search).
    note: Option<Note>,
    /// The last pattern and its direction, for `n` and `N`.
    last_search: Option<LastSearch>,
    /// Vim's `hlsearch` state: on after any search command until `no_hlsearch`.
    hl: bool,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// A session is open: Insert or Replace share every Insert key.
    fn in_insert(&self) -> bool {
        matches!(self.mode, Mode::Insert | Mode::Replace)
    }

    /// The half-typed command for the footer (`"02d3`, or `^R` while
    /// Insert `ctrl+r` waits for a register); `""` when none.
    pub fn echo(&self) -> String {
        match self.reg_pending {
            Some(reg) => reg.echo(),
            None => self.pending.echo(),
        }
    }

    /// Whether a count, register, operator or prefix is in flight, or an
    /// Insert `ctrl+r`.
    pub fn pending(&self) -> bool {
        !self.pending.is_empty() || self.reg_pending.is_some()
    }

    /// Drops a half-typed command: Normal's, or an Insert `ctrl+r`.
    fn clear_pending(&mut self) {
        self.pending.clear();
        self.reg_pending = None;
    }

    /// The last pattern as typed, or made by `*`/`#`.
    pub fn last_search(&self) -> Option<&str> {
        self.last_search.as_ref().map(|s| s.text.as_str())
    }

    /// Whether the body should paint the last pattern's matches.
    pub fn hlsearch(&self) -> bool {
        self.hl && self.last_search.is_some()
    }

    /// `:noh` (piece 4): the highlight goes until the next search.
    pub fn no_hlsearch(&mut self) {
        self.hl = false;
    }

    pub fn registers(&self) -> &Registers {
        &self.regs
    }

    pub fn registers_mut(&mut self) -> &mut Registers {
        &mut self.regs
    }

    /// A yank into `reg` (`Registers::yank`); `"+` also asks the app to
    /// copy the text out (the engine cannot reach the clipboard).
    pub(super) fn reg_yank(&mut self, reg: Option<char>, r: Register) {
        if reg == Some('+') {
            self.request = Some(AppRequest::CopyToClipboard(r.text.clone()));
        }
        self.regs.yank(reg, r);
    }

    /// A delete or change into `reg`; `"+` copies out too (a cut).
    pub(super) fn reg_delete(&mut self, reg: Option<char>, r: Register) {
        if reg == Some('+') {
            self.request = Some(AppRequest::CopyToClipboard(r.text.clone()));
        }
        self.regs.delete(reg, r);
    }

    /// The fixed end of the Visual selection; `None` outside Visual.
    pub fn visual_anchor(&self) -> Option<Pos> {
        match self.mode {
            Mode::Visual(_) => self.visual,
            _ => None,
        }
    }

    /// A buffer gets the caret (spec §4.2). Pending keys and Visual are
    /// dropped, and the caret is seated, clamped for the mode. The history
    /// is dropped if the text changed since the last [`Engine::leave`]
    /// (§3.11), and the session's start text is noted.
    pub fn enter<B: TextBuf>(&mut self, start: Start, seat: Seat, t: Target<'_, B>) {
        let Target { buf, state } = t;
        state.begin_session(buf.text());
        self.clear_pending();
        self.note = None;
        self.request = None;
        self.visual = None;
        self.insert = None;
        self.restart = None;
        self.insert_only = start == Start::InsertOnly;
        self.mode = if start == Start::Normal { Mode::Normal } else { Mode::Insert };
        let caret = buf.cursor();
        let row = caret.row.min(buf.line_count() - 1);
        let len = buf.line_len(row);
        let col = match seat {
            Seat::Keep => caret.col,
            Seat::ColZero => 0,
            Seat::End => len,
            Seat::FirstNonBlank => first_non_blank(&buf.line(row)),
        };
        buf.set_cursor(Pos::new(row, col));
        self.clamp(buf);
        if self.in_insert() {
            // Vim's `:startinsert`: a session opened in Insert records as an
            // `i` for `.`, but only once something is typed; Esc alone keeps
            // the old `.` (user ruling 2026-09-30, overriding spec §4.2's
            // "even when nothing is typed").
            self.open_resumed(buf.cursor(), true);
        }
        self.rest(buf, state);
    }

    /// The caret moves straight from one buffer to another in the same
    /// event: Tab to the next cell, a click into the next field, or out of
    /// the body (spec §4.2, profile spec §3.3). `from` is left as
    /// [`Engine::leave`] leaves it: its undo step closes, an unused
    /// autoindent is stripped, its Visual paint and Insert caret go, and its
    /// text is remembered for the next [`Engine::enter`]'s history check.
    /// The one difference is that the unfinished insert record is dropped,
    /// so `.` keeps its old value. Insert and Normal carry over: Insert
    /// opens a fresh session in `to` that `.` takes only once something is
    /// typed, and keys typed there are taken as `to`'s kind takes them.
    /// Visual and pending keys are dropped.
    pub fn carry<A: TextBuf, B: TextBuf>(&mut self, from: Target<'_, A>, to: Target<'_, B>) {
        self.restart = None;
        self.note = None;
        self.request = None;
        let inserting = self.in_insert().then_some(self.mode);
        let insert_only = self.insert_only;
        let Target { buf, state } = from;
        if inserting.is_some() {
            self.drop_session(buf, state);
        }
        self.leave(Target { buf, state });
        self.insert_only = insert_only;
        let Target { buf, state } = to;
        state.begin_session(buf.text());
        self.mode = inserting.unwrap_or(Mode::Normal);
        self.clamp(buf);
        if inserting.is_some() {
            self.open_resumed(buf.cursor(), true);
            if self.mode == Mode::Replace {
                self.session().replace = Some(Vec::new());
            }
        }
        self.rest(buf, state);
    }

    /// Rests the caret and the mode after an event the engine did not
    /// handle: a click, a mouse sweep, a key piece 4 handled (spec §4.2).
    /// A GUI selection (mouse, double-click, shift+arrow, select-all) is
    /// adopted as charwise Visual on `Click`, `Release`, or a `Key` from
    /// Normal or Insert; during a `Sweep` only the mode flips, and the
    /// geometry and paint stay the mouse's. A `Click` with no selection
    /// ends Visual. A query box has no Visual layer, so its GUI selection
    /// stays (typing replaces it). A click drops a half-typed command. In
    /// Insert, a caret the event moved splits the session as a cursor key
    /// does (Vim's `ins_mouse()`). Then the caret is clamped and becomes
    /// the wanted column.
    pub fn settle<B: TextBuf>(&mut self, t: Target<'_, B>, how: Settled) {
        let Target { buf, state } = t;
        self.note = None;
        self.request = None;
        if matches!(how, Settled::Click | Settled::Release) {
            self.clear_pending();
        }
        if matches!(self.mode, Mode::InsertNormal { .. }) && !self.pending() {
            self.resume_after_ctrl_o(buf, state);
        }
        let caret = self.clamped(buf, buf.cursor());
        if self.in_insert()
            && let Some(from) = self.rested
            && from != caret
        {
            // Vim's `ins_mouse()` → `start_arrow(&tpos)`: before and after
            // are separate undo steps, only what is typed next is redone
            // (as an `i`), and an unused autoindent is stripped where the
            // caret was, not where it went.
            self.split_insert(from, caret, buf, state);
        }
        if !self.insert_only {
            match buf.gui_selection() {
                Some(sel) if how == Settled::Sweep => {
                    self.enter_visual_from(sel.anchor, buf, state);
                    // No `rest`: the caret, the paint and the wanted column
                    // stay the mouse's until the `Release` settle rests them.
                    return;
                }
                Some(sel) if how != Settled::Key || !matches!(self.mode, Mode::Visual(_)) => {
                    self.enter_visual_from(sel.anchor, buf, state);
                    buf.clear_gui_selection();
                    buf.set_cursor(sel.head);
                }
                // A GUI key in Visual: the engine's selection stands, and the
                // GUI one goes, so a later settle does not adopt it stale.
                Some(_) => buf.clear_gui_selection(),
                None if how == Settled::Click && matches!(self.mode, Mode::Visual(_)) => {
                    // The area as it was before the click moved the caret.
                    let at = self.rested.unwrap_or(caret);
                    self.end_visual(at, &*buf, state);
                    // Visual opened inside `ctrl+o`: the click is the command.
                    if matches!(self.mode, Mode::InsertNormal { .. }) {
                        self.resume_after_ctrl_o(buf, state);
                    }
                }
                None => {}
            }
        }
        self.rest(buf, state);
    }

    /// Adopts a GUI selection's anchor as charwise Visual. An open Insert
    /// session ends first, as `leave` ends it (no step back). `settle` has
    /// already split a session the event moved, so an unused autoindent
    /// went where the caret was, not where the mouse put it.
    fn enter_visual_from<B: TextBuf>(&mut self, anchor: Pos, buf: &mut B, state: &mut BufState) {
        if self.in_insert() {
            self.end_insert(buf, state, false);
        }
        self.clear_pending();
        self.visual = Some(anchor);
        self.mode = Mode::Visual(Shape::Char);
    }

    /// The buffer loses the caret (spec §4.2). An open Insert session ends
    /// as one undo step, recorded for `.`, without the caret stepping back.
    /// Visual ends (its area remembered for `gv`), pending keys are dropped,
    /// and the mode becomes Normal. The text is remembered for the next
    /// [`Engine::enter`]'s history check.
    pub fn leave<B: TextBuf>(&mut self, t: Target<'_, B>) {
        let Target { buf, state } = t;
        if self.in_insert() {
            self.end_insert(buf, state, false);
        }
        state.history.commit();
        self.clear_pending();
        self.restart = None;
        self.note = None;
        self.request = None;
        if matches!(self.mode, Mode::Visual(_)) {
            self.end_visual(buf.cursor(), &*buf, state);
        }
        self.insert_only = false;
        self.mode = Mode::Normal;
        self.rest(buf, state);
        state.text_at_leave = Some(buf.text());
    }

    /// A change made outside the key path: a picked `{{token}}`, format or
    /// minify, a Normal-mode paste (spec §4.2). `f` edits through the
    /// [`Splicer`], and the whole call is one undo step, not repeatable
    /// with `.`. In Insert it splits the session as a cursor key does: the
    /// typing before it is its own step and record, and typing after it
    /// starts a fresh record. Visual and a half-typed command end first:
    /// they no longer fit the changed text. `t` must be the live buffer
    /// (the one with the caret), since the engine's mode is that buffer's.
    /// Returns whether the text changed.
    pub fn external_edit<B: TextBuf>(&mut self, t: Target<'_, B>, f: impl FnOnce(&mut Splicer<'_, B>)) -> bool {
        let Target { buf, state } = t;
        self.clear_pending();
        self.note = None;
        self.request = None;
        if matches!(self.mode, Mode::Visual(_)) {
            self.end_visual(buf.cursor(), &*buf, state);
        }
        // After Visual: one opened inside `ctrl+o` ends back in it.
        if matches!(self.mode, Mode::InsertNormal { .. }) {
            self.resume_after_ctrl_o(buf, state);
        }
        self.clamp(buf);
        if self.in_insert() {
            let caret = buf.cursor();
            self.split_insert(caret, caret, buf, state);
        }
        state.history.commit();
        f(&mut Splicer { ed: history::Ed { buf: &mut *buf, hist: &mut state.history } });
        state.history.commit();
        self.rest(buf, state)
    }

    /// Where every session edge ends. The caret is clamped for the mode and
    /// a text change is noted. The wanted column and the cached `w_virtcol`
    /// are dropped, to be recomputed from the caret when next needed: the
    /// edge may have changed the text, the caret or the mode where `handle`
    /// did not see it (Vim's `changed_lines()` and `w_set_curswant`). Returns
    /// whether the text changed.
    fn rest<B: TextBuf>(&mut self, buf: &mut B, state: &mut BufState) -> bool {
        self.clamp(buf);
        let changed = state.history.take_changed();
        state.edited |= changed;
        state.forget_want();
        state.forget_virtcol();
        self.rested = Some(buf.cursor());
        self.paint(buf);
        changed
    }

    /// One key while a buffer has the caret (spec §4.1).
    pub fn handle<B: TextBuf>(&mut self, ev: KeyEvent, t: Target<'_, B>, ctx: &ViewCtx) -> Outcome {
        let Target { buf, state } = t;
        self.clamp(buf);
        // Vim's main loop validates `w_topline` before every command.
        if let Some(view) = view::View::of::<B>(ctx) {
            view.update_topline(buf);
        }
        let before = buf.cursor();
        let was_insert = self.in_insert();
        if state.cached_tab_rule(before).is_none() {
            // Vim validates `w_virtcol` before a command, in its mode.
            state.virtcol = Some((before, self.tab_end(before)));
        }
        let out = match self.mode {
            Mode::Insert | Mode::Replace => self.insert_key(ev, buf, state),
            Mode::Normal | Mode::Visual(_) | Mode::InsertNormal { .. } => {
                let cx = ParseCx {
                    visual: matches!(self.mode, Mode::Visual(_)),
                    multiline: B::MULTILINE,
                    restart: matches!(self.mode, Mode::InsertNormal { .. }),
                };
                match self.pending.feed(ev, cx) {
                    Step::More => Outcome::consumed(),
                    Step::Inert(note) => Outcome::Consumed { changed: false, note, request: None },
                    Step::Decline { count, keys } => Outcome::Declined { count, keys },
                    Step::Undo { redo, count, declined } => self.exec_undo(count, redo, declined, buf, state),
                    Step::Cmd(cmd) => self.run(cmd, buf, state, ctx),
                }
            }
        };
        // A Normal or Visual command is one undo step; an Insert session
        // keeps its step open until it ends (spec §3.11).
        if !self.in_insert() {
            state.history.commit();
        }
        // Vim's `normal_cmd()` tail: Insert restarts after one complete
        // command, and before a key the engine hands back (the app sees it
        // from Insert). Visual entered inside `ctrl+o` keeps the restart.
        // Only after a key the Normal parser took: `ctrl+o` itself is an
        // Insert key.
        if !was_insert
            && matches!(self.mode, Mode::InsertNormal { .. })
            && (!self.pending() || matches!(out, Outcome::Declined { .. }))
        {
            self.resume_after_ctrl_o(buf, state);
        }
        let changed = self.finish_key(before, was_insert, buf, state, ctx);
        let (left_note, left_request) = (self.note.take(), self.request.take());
        match out {
            Outcome::Consumed { note, request, .. } => {
                Outcome::Consumed { changed, note: note.or(left_note), request: request.or(left_request) }
            }
            declined => declined,
        }
    }

    /// The end of every key ([`Engine::handle`], [`Engine::paste`]): the
    /// caret is clamped for the mode, a text change is noted, and the
    /// buffer is painted. `w_virtcol` is recomputed where the caret moved,
    /// the text changed, or Insert started or ended (see
    /// `BufState::virtcol`), and `top` follows the caret. Returns whether
    /// the text changed.
    fn finish_key<B: TextBuf>(
        &mut self,
        before: Pos,
        was_insert: bool,
        buf: &mut B,
        state: &mut BufState,
        ctx: &ViewCtx,
    ) -> bool {
        let changed = state.history.take_changed();
        state.edited |= changed;
        self.clamp(buf);
        // And before the screen is redrawn: the caret after the key is shown.
        if let Some(view) = view::View::of::<B>(ctx) {
            view.update_topline(buf);
        }
        let after = buf.cursor();
        if changed || after != before || was_insert != self.in_insert() {
            state.virtcol = Some((after, self.tab_end(after)));
        }
        self.rested = Some(after);
        self.paint(buf);
        changed
    }

    /// Runs one complete command.
    fn run<B: TextBuf>(&mut self, cmd: Cmd, buf: &mut B, st: &mut BufState, ctx: &ViewCtx) -> Outcome {
        // What `.` repeats (spec §3.12): a change that ran, even when the
        // text ends up the same (`x` on an empty line, `rX` on an X), as
        // Vim's `prep_redo()` runs once the command is under way. A change
        // that fails (its motion, `r` past the end, `J` on the last line)
        // sets nothing. A change that opens Insert records when the session
        // ends (`finish_record`).
        let mut record = None;
        // Vim's `nv_beginline()` and `nv_home()`: "Don't move cursor past
        // eol (only necessary in a one-character line)" when Insert resumes.
        if let Cmd::Move { motion: m, .. } | Cmd::Operate { reach: Reach::Motion(m), .. } = cmd
            && matches!(m, Motion::LineStart | Motion::FirstNonBlank)
            && let Some(r) = &mut self.restart
        {
            r.at_eol = false;
        }
        let out = match cmd {
            Cmd::Move { motion, count } => {
                self.exec_move(motion, count, buf, st, ctx);
                Outcome::consumed()
            }
            Cmd::Operate { op, reach, count, reg } => {
                if self.exec_operate(op, reach, count, reg, buf, st, ctx) && op != Op::Yank {
                    record = Some(cmd);
                }
                Outcome::consumed()
            }
            Cmd::Put { before, count, reg } => {
                self.exec_put(before, count, reg, buf, st);
                // A `"+` put is refused with a note: nothing for `.`.
                if reg != Some('+') {
                    record = Some(cmd);
                }
                Outcome::consumed()
            }
            Cmd::Replace { ch, count } => {
                if self.exec_replace(ch, count, buf, st) {
                    record = Some(cmd);
                }
                Outcome::consumed()
            }
            Cmd::Join { count } => {
                record = self.exec_join(count, buf, st).map(|count| Cmd::Join { count });
                Outcome::consumed()
            }
            Cmd::Tilde { count } => {
                if self.exec_tilde(count, buf, st) {
                    record = Some(cmd);
                }
                Outcome::consumed()
            }
            Cmd::AddSub { add, count } => {
                // Vim's `nv_addsub()` prepares `.` before it looks for a
                // number, so a `ctrl+a` that finds none still repeats.
                self.exec_addsub(add, count, buf, st);
                record = Some(cmd);
                Outcome::consumed()
            }
            Cmd::Scroll { how, count } => {
                self.exec_scroll(how, count, buf, st, ctx);
                Outcome::consumed()
            }
            Cmd::ScrollCursor { place, count } => {
                self.exec_scroll_cursor(place, count, buf, st, ctx);
                Outcome::consumed()
            }
            Cmd::Insert { how, count } => {
                self.exec_insert(how, count, buf, st);
                Outcome::consumed()
            }
            Cmd::Gi { count } => {
                self.exec_gi(count, buf, st);
                Outcome::consumed()
            }
            Cmd::Gv => {
                self.exec_gv(buf, st);
                Outcome::consumed()
            }
            Cmd::Repeat(count) => self.exec_repeat(count, buf, st, ctx),
            Cmd::VisualStart(_) | Cmd::VisualSwap | Cmd::VisualExit | Cmd::VisualObject { .. } | Cmd::VisualOp { .. } => {
                if let Cmd::VisualOp { op, reg, .. } = cmd
                    && !matches!(op, VisualOp::Yank | VisualOp::YankLines)
                    && !(matches!(op, VisualOp::Put { .. }) && reg == Some('+'))
                {
                    record = Some(cmd);
                }
                self.exec_visual(cmd, buf, st)
            }
        };
        if let Some(cmd) = record
            && !self.in_insert()
        {
            self.remember(cmd, None);
        }
        out
    }

    /// Remembers a finished change for `.`. A Visual replay leaves the
    /// record alone (Vim's `redo_VIsual_busy`); any other replay records
    /// itself again, so the count `{N}.` gave it stays for the next `.`.
    fn remember(&mut self, cmd: Cmd, insert: Option<Vec<InsertKey>>) {
        if self.replaying_visual {
            return;
        }
        let visual = if matches!(cmd, Cmd::VisualOp { .. }) { self.last_visual_size } else { None };
        self.dot_prev = self.dot.take();
        self.dot = Some(Dot { cmd, insert, visual });
    }

    /// `.` with an optional new count (spec §3.12, Vim's `start_redo()`):
    /// one undo step, since `handle` commits once after it.
    fn exec_repeat<B: TextBuf>(&mut self, count: usize, buf: &mut B, st: &mut BufState, ctx: &ViewCtx) -> Outcome {
        // Inside `ctrl+o` the insert itself is the newest record; Vim's
        // `start_redo(old_redo)` runs the one before it, unless a cursor key
        // split the insert and nothing was typed since (then the newest).
        let old = matches!(self.mode, Mode::InsertNormal { .. }) && self.restart.is_some_and(|r| r.old_redo);
        let Some(dot) = (if old { self.dot_prev.clone() } else { self.dot.clone() }) else { return Outcome::consumed() };
        // A replayed insert replaces the restart while it runs (`invoke_edit`
        // with something stuffed keeps it); put it back after.
        let restart = self.restart.take();
        let out = match (dot.visual, dot.cmd) {
            (Some(size), Cmd::VisualOp { op, count: own, .. }) => {
                // Vim's `redo_VIsual`: the same size from the caret, and the
                // command's own count (`{N}.` is ignored).
                self.replaying_visual = true;
                self.reselect(size, buf, st);
                match op {
                    // Visual `p`/`P` repeat as the delete they begin with
                    // (`nv_put()`): `p`'s into the unnamed register, `P`'s
                    // into the black hole.
                    VisualOp::Put { before } => {
                        let reg = if before { Some('_') } else { None };
                        self.run(Cmd::VisualOp { op: VisualOp::Delete, count: own, reg }, buf, st, ctx)
                    }
                    _ => self.run(dot.cmd, buf, st, ctx),
                }
            }
            (_, Cmd::Replace { ch: '\t', count: own }) => {
                // `{N}r<Tab>` was stored as Vim's `{N}R<Tab><Esc>`: an `R`
                // that never fails on a short line (`replace_tabs`).
                let count = if count > 0 { count } else { own };
                self.replace_tabs(count.max(1), buf, st);
                self.remember(Cmd::Replace { ch: '\t', count }, None);
                Outcome::consumed()
            }
            (_, cmd) => self.run(if count > 0 { with_count(cmd, count) } else { cmd }, buf, st, ctx),
        };
        if let Some(keys) = dot.insert
            && self.in_insert()
        {
            // As if typed here: `.` is global, so a session recorded in the
            // body can replay in a one-line field (`replay_insert`).
            self.replay_insert(keys, buf, st);
            self.end_insert(buf, st, true);
        }
        self.replaying_visual = false;
        if let Some(r) = restart
            && !self.in_insert()
        {
            self.restart = Some(r);
            self.mode = Mode::InsertNormal { replace: r.replace };
        }
        out
    }

    /// Selects the size of a recorded Visual command from the caret (Vim's
    /// `do_pending_operator()` with `redo_VIsual_busy`): the caret goes down
    /// `rows - 1` lines. Charwise over one line it goes to virtual column
    /// `w_virtcol + cols - 1`, over more to `cols`, then `coladvance()`.
    /// After `$` it goes to the line's end, except that over one line Vim
    /// computes `w_virtcol + MAXCOL - 1`, which overflows to a negative
    /// column (so column 0) once `w_virtcol` is past 1.
    fn reselect<B: TextBuf>(&mut self, size: VisualSize, buf: &mut B, st: &BufState) {
        let caret = buf.cursor();
        // Vim's `w_virtcol`, with the tab rule of the mode the `.` was
        // typed in.
        let virtcol = match motion::updated_want(buf, caret, motion::WantUpdate::Here, motion::Want::default(), self.tab_rule(st, caret)) {
            motion::Want::Col(v) => v,
            motion::Want::End => unreachable!("WantUpdate::Here always gives a column"),
        };
        let row = (caret.row + size.rows - 1).min(buf.line_count() - 1);
        let want = match (size.line, size.to_end) {
            (true, false) => None,
            (_, true) if !size.line && size.rows == 1 && virtcol > 1 => Some(motion::Want::Col(0)),
            (_, true) => Some(motion::Want::End),
            (false, false) if size.rows == 1 => Some(motion::Want::Col(virtcol + size.cols - 1)),
            (false, false) => Some(motion::Want::Col(size.cols)),
        };
        let line = buf.line(row);
        let col = match want {
            Some(want) => motion::col_for(&line, want, true),
            None => caret.col.min(line.len()),
        };
        self.visual = Some(caret);
        self.mode = Mode::Visual(if size.line { Shape::Line } else { Shape::Char });
        buf.set_cursor(Pos::new(row, col));
    }

    /// Review focus 2: a change outside the engine (reload, format, app
    /// undo) can leave the caret or the Visual anchor past the text.
    fn clamp<B: TextBuf>(&mut self, buf: &mut B) {
        let caret = buf.cursor();
        let at = self.clamped(buf, caret);
        if at != caret {
            buf.set_cursor(at);
        }
        if let Some(anchor) = self.visual {
            let row = anchor.row.min(buf.line_count() - 1);
            self.visual = Some(Pos::new(row, anchor.col.min(buf.line_len(row))));
        }
    }

    /// Where the caret may rest in the current mode: on a char in Normal,
    /// also on the line's end in Insert and Visual (`selection=inclusive`),
    /// and after a command inside Insert `ctrl+o` (Vim's `check_cursor_col()`
    /// with `restart_edit` set: `x` on a line's last char leaves the caret on
    /// the end, and Insert resumes there). A bare motion still stops on a
    /// char there (`exec_move`).
    fn clamped<B: TextBuf>(&self, buf: &B, at: Pos) -> Pos {
        let row = at.row.min(buf.line_count() - 1);
        let len = buf.line_len(row);
        let max = if self.mode == Mode::Normal { len.saturating_sub(1) } else { len };
        Pos::new(row, at.col.min(max))
    }

    fn paint<B: TextBuf>(&self, buf: &mut B) {
        buf.show(match self.mode {
            Mode::Normal | Mode::InsertNormal { .. } => Paint::Normal,
            Mode::Insert | Mode::Replace => Paint::Insert,
            Mode::Visual(shape) => Paint::Visual {
                anchor: self.visual.unwrap_or_else(|| buf.cursor()),
                line: shape == Shape::Line,
            },
        });
    }
}

/// What [`Engine::external_edit`] hands its closure: every splice is
/// recorded in the buffer's history.
pub struct Splicer<'x, B: TextBuf> {
    ed: history::Ed<'x, B>,
}

impl<B: TextBuf> Splicer<'_, B> {
    /// Replaces `[start, end)` with `text` (see [`TextBuf::splice`]).
    /// The marks move by the charwise rule (`MarkMove::Chars`),
    /// which has no Vim counterpart here; harmless, since `gi` clamps.
    pub fn splice(&mut self, start: Pos, end: Pos, text: &str) {
        self.ed.splice(start, end, text);
    }

    /// Raw; the engine clamps the caret for the mode afterwards.
    pub fn set_cursor(&mut self, at: Pos) {
        self.ed.buf.set_cursor(at);
    }

    pub fn buf(&self) -> &B {
        &*self.ed.buf
    }
}

/// A recorded command with the count `{N}.` gave it. A Visual command
/// keeps its own (see `exec_repeat`).
fn with_count(cmd: Cmd, count: usize) -> Cmd {
    match cmd {
        Cmd::Operate { op, reach, reg, .. } => Cmd::Operate { op, reach, count, reg },
        Cmd::Put { before, reg, .. } => Cmd::Put { before, count, reg },
        Cmd::Replace { ch, .. } => Cmd::Replace { ch, count },
        Cmd::Join { .. } => Cmd::Join { count },
        Cmd::Tilde { .. } => Cmd::Tilde { count },
        Cmd::AddSub { add, .. } => Cmd::AddSub { add, count },
        Cmd::Insert { how, .. } => Cmd::Insert { how, count },
        other => other,
    }
}

/// Vim's `beginline(BL_WHITE)`: the first char that is not a space or a
/// tab; the line's length when there is none.
pub(crate) fn first_non_blank(line: &[char]) -> usize {
    line.iter().position(|&c| c != ' ' && c != '\t').unwrap_or(line.len())
}

/// Vim's `beginline(BL_WHITE | BL_FIX)`: the first non-blank, but never
/// past the last char of an all-blank line.
pub(crate) fn first_non_blank_fix(line: &[char]) -> usize {
    first_non_blank(line).min(line.len().saturating_sub(1))
}
