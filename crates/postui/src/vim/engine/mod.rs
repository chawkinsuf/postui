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
mod register;
pub mod settings;

pub use buf::{BodyBuf, GuiSel, OneLineBuf, Paint, Pos, TextBuf};
pub use register::{RegKind, Register, Registers};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
}

impl BufState {
    pub fn new() -> Self {
        Self::default()
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
    regs: Registers,
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
        String::new()
    }

    /// Whether a count, register, operator or prefix is in flight.
    pub fn pending(&self) -> bool {
        false
    }

    pub fn registers(&self) -> &Registers {
        &self.regs
    }

    pub fn registers_mut(&mut self) -> &mut Registers {
        &mut self.regs
    }

    /// The fixed end of the Visual selection; `None` outside Visual.
    pub fn visual_anchor(&self) -> Option<Pos> {
        None
    }

    /// A buffer gets the caret (spec §4.2). Task 13 completes this.
    pub fn enter<B: TextBuf>(&mut self, start: Start, seat: Seat, t: Target<'_, B>) {
        let Target { buf, state } = t;
        state.text_at_start = Some(buf.text());
        state.edited = false;
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
        let max = if self.mode == Mode::Normal { len.saturating_sub(1) } else { len };
        buf.set_cursor(Pos::new(row, col.min(max)));
        self.paint(buf);
    }

    /// One key while a buffer has the caret. Task 5 replaces this body
    /// with the parser.
    pub fn handle<B: TextBuf>(&mut self, ev: KeyEvent, t: Target<'_, B>, _ctx: &ViewCtx) -> Outcome {
        let _ = t;
        if ev.code == KeyCode::Esc && ev.modifiers == KeyModifiers::NONE {
            return Outcome::Declined { count: None, keys: vec![ev] };
        }
        Outcome::consumed()
    }

    fn paint<B: TextBuf>(&self, buf: &mut B) {
        buf.show(match self.mode {
            Mode::Normal => Paint::Normal,
            Mode::Insert => Paint::Insert,
            Mode::Visual(shape) => Paint::Visual { anchor: buf.cursor(), line: shape == Shape::Line },
        });
    }
}

/// Vim's `beginline(BL_WHITE)`: the first char that is not a space or a
/// tab; the line's length when there is none.
pub(crate) fn first_non_blank(line: &[char]) -> usize {
    line.iter().position(|&c| c != ' ' && c != '\t').unwrap_or(line.len())
}
