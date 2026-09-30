//! Undo and redo (spec §3.11). Each step stores only the changed text, as
//! the edits the one splice wrapper ([`Ed::splice`]) recorded. The engine
//! owns the body's undo in the vim profile (open question 1, decided A),
//! and the same code serves every buffer.

use super::buf::{Pos, TextBuf};
use super::first_non_blank_fix;
use super::settings::UNDOLEVELS;

/// One splice: `removed` was replaced by `inserted` at `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edit {
    pub at: Pos,
    pub removed: String,
    pub inserted: String,
}

/// One undo step: the edits of one Normal command, or of an Insert session
/// together with the command that opened it, and the caret Vim's `u_save`
/// saw at the first edit (`uh_cursor`): an operator's range start.
#[derive(Debug, Clone)]
struct Step {
    edits: Vec<Edit>,
    caret_before: Pos,
    /// Vim's `ML_EMPTY` before and after the step (`UH_EMPTYBUF`).
    empty_before: bool,
    empty_after: bool,
}

#[derive(Debug, Default)]
pub(crate) struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    open: Option<Step>,
    changed: bool,
    /// Vim's `ML_EMPTY`: the last line was deleted, so the buffer holds no
    /// line at all (shown as one empty line). A delete then does nothing
    /// (`op_delete`), not even write the register. Any other edit clears it.
    emptied: bool,
}

impl History {
    /// Opens a step unless one is open. `caret` is Vim's `uh_cursor`: where
    /// `u` returns when the change is around it.
    pub(crate) fn begin(&mut self, caret: Pos) {
        if self.open.is_none() {
            let empty_before = self.emptied;
            self.open = Some(Step { edits: Vec::new(), caret_before: caret, empty_before, empty_after: false });
        }
    }

    pub(crate) fn record(&mut self, edit: Edit) {
        self.changed |= edit.removed != edit.inserted;
        let caret = edit.at;
        self.begin(caret);
        self.open.as_mut().expect("begin opened a step").edits.push(edit);
    }

    /// Closes the open step. A step with no edits is dropped; a new step
    /// clears the redo branch; the oldest step goes past `UNDOLEVELS`.
    pub(crate) fn commit(&mut self) {
        if let Some(mut step) = self.open.take()
            && !step.edits.is_empty()
        {
            step.empty_after = self.emptied;
            self.redo.clear();
            self.undo.push(step);
            if self.undo.len() > UNDOLEVELS {
                self.undo.remove(0);
            }
        }
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Steps that can be undone.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.undo.len()
    }

    /// Whether any text changed since the last call.
    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Vim's `ML_EMPTY` (see the field).
    pub(crate) fn emptied(&self) -> bool {
        self.emptied
    }

    /// Undoes the newest step; the caret it lands on.
    pub(crate) fn undo<B: TextBuf>(&mut self, buf: &mut B) -> Option<Pos> {
        let step = self.undo.pop()?;
        let at = undo_redo(buf, &step, true);
        self.emptied = step.empty_before && is_blank_buffer(buf);
        self.changed |= step.changes();
        self.redo.push(step);
        Some(at)
    }

    /// Redoes the newest undone step; the caret it lands on.
    pub(crate) fn redo<B: TextBuf>(&mut self, buf: &mut B) -> Option<Pos> {
        let step = self.redo.pop()?;
        let at = undo_redo(buf, &step, false);
        self.emptied = step.empty_after && is_blank_buffer(buf);
        self.changed |= step.changes();
        self.undo.push(step);
        Some(at)
    }
}

impl Step {
    /// False for a step that only saved a line (`dh` in column 0).
    fn changes(&self) -> bool {
        self.edits.iter().any(|e| e.removed != e.inserted)
    }
}

fn is_blank_buffer<B: TextBuf>(buf: &B) -> bool {
    buf.line_count() == 1 && buf.line_len(0) == 0
}

/// Where `text` ends when it starts at `at`.
pub(crate) fn end_of(at: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => Pos::new(at.row, at.col + text.chars().count()),
        Some(i) => Pos::new(at.row + text.matches('\n').count(), text[i + 1..].chars().count()),
    }
}

/// One edit as the block of whole lines Vim's `u_save` would have saved:
/// the block's first row, its size now and after the edit is reverted or
/// replayed, and those lines.
struct Block {
    first: usize,
    now: usize,
    lines: Vec<String>,
}

/// `now` is the text the buffer holds at `at`, `then` what replaces it.
fn block<B: TextBuf>(buf: &B, at: Pos, now: &str, then: &str) -> Block {
    let nl = |s: &str| s.matches('\n').count();
    let whole = |s: &str| s.is_empty() || s.ends_with('\n');
    let joined = |s: &str| s.is_empty() || s.starts_with('\n');
    if at.col == 0 && whole(now) && whole(then) {
        // Whole lines taken or given (`dd` in the middle, `p` linewise).
        let lines = then.split_terminator('\n').map(str::to_string).collect();
        return Block { first: at.row, now: nl(now), lines };
    }
    if at.col == buf.line_len(at.row) && joined(now) && joined(then) {
        // Whole lines after `at`'s row, the break before them included
        // (`dd` on the last line, `o`).
        let lines = then.split('\n').skip(1).map(str::to_string).collect();
        return Block { first: at.row + 1, now: nl(now), lines };
    }
    // Charwise: every row the edit touches, whole.
    let head: String = buf.line(at.row).iter().take(at.col).collect();
    let end = end_of(at, now);
    let tail: String = buf.line(end.row).iter().skip(end.col).collect();
    let lines = format!("{head}{then}{tail}").split('\n').map(str::to_string).collect();
    Block { first: at.row, now: nl(now) + 1, lines }
}

/// Reverts (`undo`) or replays a step, and returns the caret: Vim's
/// `u_undoredo()` (undo.c), with each edit read as the block of lines its
/// `u_save` saved. For the block nearest the top: if the saved caret is in
/// it (or on a line next to it), the caret goes back there; otherwise to
/// the first line in it that really changed. That row is first clamped to
/// the buffer (Vim's `check_cursor_lnum()`: redoing a delete of the last
/// lines leaves it past the end), then goes one line up when it is just
/// below the saved caret (the `o` case), and takes the saved column only on
/// the saved caret's row, else `beginline(BL_SOL | BL_FIX)`. The golden
/// file's `u` and `u<C-r>` cases pin all of this.
fn undo_redo<B: TextBuf>(buf: &mut B, step: &Step, undo: bool) -> Pos {
    let saved = step.caret_before;
    let order: Vec<&Edit> = if undo { step.edits.iter().rev().collect() } else { step.edits.iter().collect() };
    // Vim's `newlnum`, as a 0-based row of the first line in a block.
    let mut newlnum = usize::MAX;
    let mut row = None;
    for (n, e) in order.iter().enumerate() {
        let (now, then) = if undo { (&e.inserted, &e.removed) } else { (&e.removed, &e.inserted) };
        let b = block(buf, e.at, now, then);
        if b.first < newlnum {
            if saved.row + 1 >= b.first && saved.row <= b.first + b.lines.len() {
                row = Some(saved.row);
                newlnum = saved.row;
            } else {
                let size = b.lines.len();
                let same = (0..size.min(b.now))
                    .take_while(|&i| buf.line(b.first + i).iter().copied().eq(b.lines[i].chars()))
                    .count();
                if same == size && newlnum == usize::MAX && n + 1 == order.len() {
                    newlnum = b.first;
                    row = Some(b.first);
                } else if same < size {
                    newlnum = b.first + same;
                    row = Some(b.first + same);
                }
            }
        }
        buf.splice(e.at, end_of(e.at, now), then);
    }
    let mut row = row.unwrap_or_else(|| buf.cursor().row).min(buf.line_count() - 1);
    if saved.row + 1 == row && row > 0 {
        row -= 1;
    }
    let line = buf.line(row);
    let col = if saved.row == row { saved.col.min(line.len().saturating_sub(1)) } else { first_non_blank_fix(&line) };
    Pos::new(row, col)
}

/// The one way the engine changes text: splices through the buffer and
/// records the edit in the open step, opening one at the caret (Vim's
/// lazy `u_save`).
pub(crate) struct Ed<'x, B: TextBuf> {
    pub buf: &'x mut B,
    pub hist: &'x mut History,
}

impl<B: TextBuf> Ed<'_, B> {
    pub(crate) fn splice(&mut self, start: Pos, end: Pos, text: &str) {
        let removed = self.buf.slice(start, end);
        if removed == text {
            return;
        }
        self.hist.begin(self.buf.cursor());
        self.buf.splice(start, end, text);
        self.hist.emptied = false;
        self.hist.record(Edit { at: start, removed, inserted: text.to_string() });
    }

    /// Vim's `u_save_cursor()` with no change after it: `u` then undoes a
    /// step that changed nothing (`dh` in column 0).
    /// A step already open keeps its caret; a new one opens at column 0.
    pub(crate) fn save_line(&mut self, row: usize) {
        self.save_rows(Pos::new(row, 0), row, row);
    }

    /// Vim's `u_save(first - 1, last + 1)` before a command that saves its
    /// lines whatever it then changes: opens the step at `caret` (Vim's
    /// `uh_cursor`) and saves rows `first..=last`, so the command is an undo
    /// step even when the text ends up the same (Visual `~` on a space, `<`
    /// with no indent).
    pub(crate) fn save_rows(&mut self, caret: Pos, first: usize, last: usize) {
        self.hist.begin(caret);
        let text = self.buf.slice(Pos::new(first, 0), Pos::new(last, self.buf.line_len(last)));
        self.hist.record(Edit { at: Pos::new(first, 0), removed: text.clone(), inserted: text });
    }

    /// Vim's `u_save_cursor()`: [`Ed::save_rows`] for the caret's line (`rX`
    /// on an X, `p` of `""`, `J` joining one line).
    pub(crate) fn save_cursor_line(&mut self, caret: Pos) {
        self.save_rows(caret, caret.row, caret.row);
    }

    /// Deletes every line: Vim's buffer is then `ML_EMPTY`. Vim's
    /// `u_savedel` saves even when the buffer was already one blank line,
    /// so `dd` there is still a step `u` undoes.
    pub(crate) fn empty_buffer(&mut self) {
        let last = self.buf.line_count() - 1;
        if last == 0 && self.buf.line_len(0) == 0 {
            self.save_line(0);
        } else {
            self.splice(Pos::new(0, 0), Pos::new(last, self.buf.line_len(last)), "");
        }
        self.hist.emptied = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::buf::BodyBuf;
    use super::super::op::delete_lines;
    use super::*;
    use edtui::{EditorState, Lines};
    use ratatui::style::Style;

    /// Review probe P4: a delete of the last line made outside an operator
    /// (the step saved at a caret above it), undone and redone. Vim clamps
    /// the landing row before the `o` rule and lands on the saved caret.
    #[test]
    fn a_redo_past_the_end_clamps_the_row_before_the_o_rule() {
        let mut state = EditorState::new(Lines::from("a\n  b\n  c\n  d\n  e"));
        let mut buf = BodyBuf::new(&mut state, Style::default());
        let mut hist = History::default();
        buf.set_cursor(Pos::new(2, 2));
        hist.begin(Pos::new(2, 2));
        delete_lines(&mut Ed { buf: &mut buf, hist: &mut hist }, 4, 4);
        hist.commit();
        assert_eq!(buf.text(), "a\n  b\n  c\n  d");
        assert!(hist.undo(&mut buf).is_some());
        assert_eq!(buf.text(), "a\n  b\n  c\n  d\n  e");
        assert_eq!(hist.redo(&mut buf), Some(Pos::new(2, 2)), "Vim lands on [3,3]");
    }

    /// Review probe P1: `dd` in a buffer that is already one blank line is
    /// still a step, so `u` is not handed to the app history.
    #[test]
    fn dd_on_a_blank_buffer_is_still_an_undo_step() {
        let mut state = EditorState::new(Lines::from(""));
        let mut buf = BodyBuf::new(&mut state, Style::default());
        let mut hist = History::default();
        hist.begin(Pos::new(0, 0));
        delete_lines(&mut Ed { buf: &mut buf, hist: &mut hist }, 0, 0);
        hist.commit();
        assert!(hist.can_undo() && hist.emptied());
        assert!(!hist.take_changed(), "no text changed");
        assert_eq!(hist.undo(&mut buf), Some(Pos::new(0, 0)));
        assert!(!hist.can_undo());
    }
}
