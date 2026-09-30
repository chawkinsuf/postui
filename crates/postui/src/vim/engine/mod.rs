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
mod class;
mod class_table;
mod history;
mod insert;
mod keys;
mod motion;
mod object;
mod op;
mod register;
pub mod settings;
#[cfg(test)]
mod tests;
mod visual;

pub use buf::{BodyBuf, GuiSel, OneLineBuf, Paint, Pos, TextBuf};
pub use register::{RegKind, Register, Registers};

use keys::{Cmd, InsertHow, ParseCx, Pending, Step};
use ratatui::crossterm::event::KeyEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Char,
    Line,
}

/// The engine's mode (spec §4.1). Global: only one buffer has the caret.
/// Plan 3c adds `InsertNormal`, `Replace` and `Search`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
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

/// What just happened, for [`Engine::settle`] (Task 13).
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

/// Footer text for a key a vim user expects but the engine does not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Unsupported(String),
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
    /// count from its last cell?) in force where the caret last moved or
    /// the text last changed. Vim recomputes it only then
    /// (`check_cursor_moved()`), so a mode change alone (`v`, `Esc`) keeps
    /// the old value, and the next `w_curswant` is read from it.
    virtcol: Option<(Pos, bool)>,
    pub(crate) history: history::History,
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

    pub fn new() -> Self {
        Self::default()
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
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The half-typed command for the footer (`"02d3`); `""` when none.
    pub fn echo(&self) -> String {
        self.pending.echo()
    }

    /// Whether a count, register, operator or prefix is in flight.
    pub fn pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn registers(&self) -> &Registers {
        &self.regs
    }

    pub fn registers_mut(&mut self) -> &mut Registers {
        &mut self.regs
    }

    /// The fixed end of the Visual selection; `None` outside Visual.
    pub fn visual_anchor(&self) -> Option<Pos> {
        match self.mode {
            Mode::Visual(_) => self.visual,
            _ => None,
        }
    }

    /// A buffer gets the caret (spec §4.2). Task 13 completes this.
    pub fn enter<B: TextBuf>(&mut self, start: Start, seat: Seat, t: Target<'_, B>) {
        let Target { buf, state } = t;
        state.text_at_start = Some(buf.text());
        state.edited = false;
        state.virtcol = None;
        self.pending.clear();
        self.visual = None;
        self.mode = if start == Start::Normal { Mode::Normal } else { Mode::Insert };
        self.insert_only = start == Start::InsertOnly;
        self.insert = None;
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
        if self.mode == Mode::Insert {
            // A session opened in Insert records as an `i` for `.` (§4.2).
            self.insert = Some(insert::Session::new(buf.cursor(), Cmd::Insert { how: InsertHow::Before, count: 0 }));
        }
        self.paint(buf);
    }

    /// One key while a buffer has the caret (spec §4.1).
    pub fn handle<B: TextBuf>(&mut self, ev: KeyEvent, t: Target<'_, B>, ctx: &ViewCtx) -> Outcome {
        let Target { buf, state } = t;
        self.clamp(buf);
        let before = buf.cursor();
        if state.cached_tab_rule(before).is_none() {
            // Vim validates `w_virtcol` before a command, in its mode.
            state.virtcol = Some((before, self.tab_end(before)));
        }
        let out = match self.mode {
            Mode::Insert => self.insert_key(ev, buf, state),
            Mode::Normal | Mode::Visual(_) => {
                let cx = ParseCx { visual: self.mode != Mode::Normal, multiline: B::MULTILINE };
                match self.pending.feed(ev, cx) {
                    Step::More => Outcome::consumed(),
                    Step::Inert(note) => Outcome::Consumed { changed: false, note, request: None },
                    Step::Decline { count, keys } => Outcome::Declined { count, keys },
                    Step::Cmd(cmd) => self.run(cmd, buf, state, ctx),
                }
            }
        };
        // A Normal or Visual command is one undo step; an Insert session
        // keeps its step open until it ends (spec §3.11).
        if self.mode != Mode::Insert {
            state.history.commit();
        }
        let changed = state.history.take_changed();
        state.edited |= changed;
        self.clamp(buf);
        let after = buf.cursor();
        if changed || after != before {
            state.virtcol = Some((after, self.tab_end(after)));
        }
        self.paint(buf);
        match out {
            Outcome::Consumed { note, request, .. } => Outcome::Consumed { changed, note, request },
            declined => declined,
        }
    }

    /// Runs one complete command.
    fn run<B: TextBuf>(&mut self, cmd: Cmd, buf: &mut B, st: &mut BufState, ctx: &ViewCtx) -> Outcome {
        match cmd {
            Cmd::Move { motion, count } => {
                self.exec_move(motion, count, buf, st);
                Outcome::consumed()
            }
            Cmd::Operate { op, reach, count, reg } => self.exec_operate(op, reach, count, reg, buf, st),
            Cmd::Put { before, count, reg } => self.exec_put(before, count, reg, buf, st),
            Cmd::Replace { ch, count } => self.exec_replace(ch, count, buf, st),
            Cmd::Join { count } => self.exec_join(count, buf, st),
            Cmd::Insert { how, count } => {
                self.exec_insert(how, count, buf, st);
                Outcome::consumed()
            }
            Cmd::Undo(count) => self.exec_undo(count, false, buf, st),
            Cmd::Redo(count) => self.exec_undo(count, true, buf, st),
            Cmd::Repeat(count) => self.exec_repeat(count, buf, st, ctx),
            Cmd::VisualStart(_) | Cmd::VisualSwap | Cmd::VisualExit | Cmd::VisualObject { .. } | Cmd::VisualOp { .. } => {
                self.exec_visual(cmd, buf, st)
            }
        }
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
    /// also on the line's end in Insert and Visual (`selection=inclusive`).
    fn clamped<B: TextBuf>(&self, buf: &B, at: Pos) -> Pos {
        let row = at.row.min(buf.line_count() - 1);
        let len = buf.line_len(row);
        let max = if self.mode == Mode::Normal { len.saturating_sub(1) } else { len };
        Pos::new(row, at.col.min(max))
    }

    fn paint<B: TextBuf>(&self, buf: &mut B) {
        buf.show(match self.mode {
            Mode::Normal => Paint::Normal,
            Mode::Insert => Paint::Insert,
            Mode::Visual(shape) => Paint::Visual {
                anchor: self.visual.unwrap_or_else(|| buf.cursor()),
                line: shape == Shape::Line,
            },
        });
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

// ---- Stubs ---------------------------------------------------------------
// Each later task moves one of these into its own module with the real
// behaviour and deletes it here. Until then its command has no effect.
impl Engine {
    /// Task 12 (mod.rs, `.`).
    fn exec_repeat<B: TextBuf>(&mut self, _count: usize, _buf: &mut B, _st: &mut BufState, _ctx: &ViewCtx) -> Outcome {
        Outcome::consumed()
    }
}
