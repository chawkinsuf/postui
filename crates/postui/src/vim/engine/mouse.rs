//! The mouse on a buffer's text, as Vim 9.1's `do_mouse()` runs it with
//! `'mouse'` `a` and `'mousemodel'` `extend` (probed): a press puts the
//! caret on the char it points at and ends Visual, a second press selects
//! a word (or a bracket's block), a third the line, and a drag extends by
//! chars, words or lines as the press began. A right press extends Visual.
//! Under Insert a selection runs inside `ctrl+o`, as Vim stuffs one, so
//! Insert comes back when it ends.

use super::buf::{Pos, TextBuf};
use super::class::{class, is_word, white};
use super::motion::{MatchFrom, find_match};
use super::{BufState, Engine, Mode, Shape};

/// What the mouse did at a char of the text ([`super::Settled::Mouse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    /// A left press, with Vim's click count: a press on the cell of the
    /// last one, within `'mousetime'` of it, counts on.
    Press(u8),
    /// The left button moved with it down.
    Drag,
    /// A right press.
    Extend,
}

/// The press a drag extends from (Vim's `orig_cursor`, `orig_num_clicks`).
#[derive(Debug, Clone, Copy)]
pub(super) struct Pressed {
    at: Pos,
    clicks: u8,
}

impl Engine {
    /// The mouse at `at`. The field's own mouse left the caret alone, so
    /// it is where the engine last left it.
    pub(super) fn mouse<B: TextBuf>(
        &mut self,
        at: Pos,
        how: Mouse,
        buf: &mut B,
        st: &mut BufState,
    ) {
        let from = buf.cursor();
        let at = Pos::new(at.row.min(buf.line_count() - 1), at.col);
        match how {
            Mouse::Press(clicks) => self.mouse_press(at, clicks, from, buf, st),
            Mouse::Drag => self.mouse_drag(at, from, buf, st),
            Mouse::Extend => self.mouse_extend(at, from, buf, st),
        }
    }

    fn mouse_press<B: TextBuf>(
        &mut self,
        at: Pos,
        clicks: u8,
        from: Pos,
        buf: &mut B,
        st: &mut BufState,
    ) {
        // A left press ends Visual (`MOUSE_MAY_STOP_VIS`); one opened inside
        // `ctrl+o` gives Insert back.
        if matches!(self.mode, Mode::Visual(_)) {
            self.end_visual(from, &*buf, st);
            if matches!(self.mode, Mode::InsertNormal { .. }) {
                self.resume_after_ctrl_o(buf, st);
            }
        }
        // Under Insert `ins_mouse()` passes no operator, so no block is
        // matched there.
        let insert = self.in_insert();
        let at = self.move_to(at, from, buf, st);
        // Vim's fourth press is blockwise, which the engine does not have: it
        // is a plain press, as Vim's fifth is.
        let clicks = if clicks > 3 { 1 } else { clicks };
        self.pressed = Some(Pressed { at, clicks });
        match clicks {
            2 => {
                let block = if insert { None } else { block_end(buf, at) };
                self.start_mouse_visual(Shape::Char, at, buf, st);
                let caret = match block {
                    Some(end) => end,
                    None => {
                        self.visual = Some(word_start(buf, at));
                        word_end(buf, at)
                    }
                };
                buf.set_cursor(caret);
            }
            3 => self.start_mouse_visual(Shape::Line, at, buf, st),
            _ => {}
        }
    }

    fn mouse_drag<B: TextBuf>(&mut self, at: Pos, from: Pos, buf: &mut B, st: &mut BufState) {
        let pressed = self.pressed.unwrap_or(Pressed { at: from, clicks: 1 });
        match (self.mode, self.visual) {
            // After a double click the ends go to whole words.
            (Mode::Visual(Shape::Char), Some(anchor)) if pressed.clicks == 2 => {
                let at = self.clamped(buf, at);
                let caret = if at < pressed.at {
                    self.visual = Some(word_end(buf, anchor));
                    word_start(buf, at)
                } else {
                    self.visual = Some(word_start(buf, anchor));
                    word_end(buf, at)
                };
                buf.set_cursor(caret);
            }
            (Mode::Visual(_), _) => buf.set_cursor(self.clamped(buf, at)),
            // Visual starts where the press left the caret (`MOUSE_MAY_VIS`).
            _ => self.mouse_visual_to(at, from, buf, st),
        }
    }

    fn mouse_extend<B: TextBuf>(&mut self, at: Pos, from: Pos, buf: &mut B, st: &mut BufState) {
        self.pressed = None;
        let (Mode::Visual(_), Some(anchor)) = (self.mode, self.visual) else {
            return self.mouse_visual_to(at, from, buf, st);
        };
        // The end nearer the pointer moves: by columns on one line, else by
        // lines, and on the middle line by the columns' midpoint.
        let (start, end) = if from < anchor { (from, anchor) } else { (anchor, from) };
        let at = self.clamped(buf, at);
        let nearer_end = if at < start {
            false
        } else if end < at {
            true
        } else if start.row == end.row {
            at.col - start.col > end.col - at.col
        } else {
            let (above, below) = (at.row - start.row, end.row - at.row);
            match above.cmp(&below) {
                std::cmp::Ordering::Equal => at.col >= (start.col + end.col) / 2,
                order => order == std::cmp::Ordering::Greater,
            }
        };
        self.visual = Some(if nearer_end { start } else { end });
        buf.set_cursor(at);
    }

    /// Charwise Visual from `from` to the pointer, which may be on a line's
    /// end once Visual is on.
    fn mouse_visual_to<B: TextBuf>(&mut self, at: Pos, from: Pos, buf: &mut B, st: &mut BufState) {
        self.move_to(at, from, buf, st);
        self.start_mouse_visual(Shape::Char, from, buf, st);
        buf.set_cursor(self.clamped(buf, at));
    }

    /// Puts the caret on `at` as the mode allows (`jump_to_mouse()`). In
    /// Insert the move splits the session (`ins_mouse()`'s `start_arrow()`).
    fn move_to<B: TextBuf>(&mut self, at: Pos, from: Pos, buf: &mut B, st: &mut BufState) -> Pos {
        let at = self.clamped(buf, at);
        buf.set_cursor(at);
        if self.in_insert() && at != from {
            self.split_insert(from, at, buf, st);
        }
        at
    }

    /// Visual from `anchor`, the caret kept. Under Insert it opens inside
    /// `ctrl+o`; the caret stays on a line's end there, since Visual may.
    fn start_mouse_visual<B: TextBuf>(
        &mut self,
        shape: Shape,
        anchor: Pos,
        buf: &mut B,
        st: &mut BufState,
    ) {
        if self.in_insert() {
            let caret = buf.cursor();
            self.ctrl_o(buf, st);
            buf.set_cursor(caret);
        }
        self.visual = Some(anchor);
        self.mode = Mode::Visual(shape);
    }
}

/// Vim's `get_mouse_class()`: what a double click takes as one word. Past
/// ASCII it is [`class`]. In ASCII a blank (or the line's end) is 0, a
/// word char 2, the operator chars `-+*/%<>&|^!=` 1, so `->` and `!=` go
/// together, and any other char is a class of its own.
fn mouse_class(c: Option<char>) -> u32 {
    match c {
        None => 0,
        Some(c) if !c.is_ascii() => class(c),
        Some(c) if white(c) => 0,
        Some(c) if is_word(c) => 2,
        Some(c) if "-+*/%<>&|^!=".contains(c) => 1,
        Some(c) => u32::from(c),
    }
}

/// Vim's `find_start_of_word()`.
fn word_start<B: TextBuf>(buf: &B, at: Pos) -> Pos {
    let line = buf.line(at.row);
    let k = mouse_class(line.get(at.col).copied());
    let mut col = at.col.min(line.len());
    while col > 0 && mouse_class(line.get(col - 1).copied()) == k {
        col -= 1;
    }
    Pos::new(at.row, col)
}

/// Vim's `find_end_of_word()` (`'selection'` inclusive): the word's last
/// char, or the line's end after a blank run that reaches it.
fn word_end<B: TextBuf>(buf: &B, at: Pos) -> Pos {
    let line = buf.line(at.row);
    let k = mouse_class(line.get(at.col).copied());
    let mut col = at.col;
    while col < line.len() && mouse_class(line.get(col + 1).copied()) == k {
        col += 1;
    }
    Pos::new(at.row, col)
}

/// Where a double click's block ends: on a char that is not a word char
/// (blanks skipped), the `%` match from the pointer, if any.
fn block_end<B: TextBuf>(buf: &B, at: Pos) -> Option<Pos> {
    let line = buf.line(at.row);
    let first = line.iter().skip(at.col).find(|&&c| !white(c));
    if first.is_some_and(|&c| is_word(c)) {
        return None;
    }
    find_match(buf, at, MatchFrom::Percent, true)
}
