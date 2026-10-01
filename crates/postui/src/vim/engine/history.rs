//! Undo and redo (spec §3.11). Each step stores only the changed text, as
//! the edits the one splice wrapper ([`Ed::splice`]) recorded. The engine
//! owns the body's undo in the vim profile (open question 1, decided A),
//! and the same code serves every buffer.

use super::Shape;
use super::buf::{Pos, TextBuf};
use super::first_non_blank_fix;
use super::motion::Want;
use super::settings::UNDOLEVELS;

/// One splice: `removed` was replaced by `inserted` at `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edit {
    pub at: Pos,
    pub removed: String,
    pub inserted: String,
    /// A line join that saved both lines (`Ed::splice_lines_joined`): its
    /// undo block is always the charwise one.
    pub joined: bool,
}

/// A buffer's last Visual area, for `gv` (Vim's `b_visual`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LastVisual {
    pub anchor: Pos,
    pub caret: Pos,
    pub shape: Shape,
    /// The wanted column when Visual ended (Vim's `vi_curswant`); `gv`
    /// restores it.
    pub want: Want,
}

/// The positions Vim keeps per buffer and moves with its edits (its
/// marks): where Insert last ended, for `gi` (Vim's `'^`), and the last
/// Visual area, for `gv` (Vim's `b_visual`, its `'<` and `'>`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Marks {
    pub insert: Option<Pos>,
    pub visual: Option<LastVisual>,
}

impl Marks {
    /// Moves every mark: `f(mark, nodel)` says where it goes, `None` when
    /// it is gone. `nodel` is Vim's `one_adjust_nodel()`, for marks that
    /// are never deleted (the Visual area); `'^` can be.
    fn each(&mut self, f: impl Fn(Pos, bool) -> Option<Pos>) {
        self.insert = self.insert.and_then(|p| f(p, false));
        if let Some(v) = &mut self.visual {
            // `nodel`: the Visual area's marks only ever move.
            v.anchor = f(v.anchor, true).unwrap_or(v.anchor);
            v.caret = f(v.caret, true).unwrap_or(v.caret);
        }
    }

    fn adjust(&mut self, at: Pos, removed: &str, inserted: &str, how: MarkMove<'_>) {
        self.each(|p, nodel| how.apply(p, at, removed, inserted, nodel));
    }

    fn lines_deleted(&mut self, first: usize, last: usize) {
        self.each(|p, nodel| moved_by_delete(p, first, last, nodel));
    }

    fn lines_replaced(&mut self, first: usize, now: usize, new: usize) {
        self.each(|p, nodel| moved_by_replace(p, first, now, new, nodel));
    }
}

/// How a splice moves the marks on the lines it touches: the way Vim's
/// `mark_adjust()` and `mark_col_adjust()` move them for the command that
/// made the splice.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MarkMove<'m> {
    /// A charwise edit, the default: Vim's `op_delete()`, `ins_bs()` and
    /// `ins_del()` join lines, `open_line()` and a charwise put split them.
    Chars,
    /// Lines `first..=last` go (Vim's `del_lines()`).
    DeleteLines { first: usize, last: usize },
    /// `count` lines appear from row `at` on (Vim's `appended_lines_mark()`).
    InsertLines { at: usize, count: usize },
    /// `J` (Vim's `do_join()`): line `row + t` joins onto `row`, and
    /// `lines[t - 1]` is its (column amount, spaces removed) for
    /// `mark_col_adjust()`.
    Join { row: usize, lines: &'m [(isize, isize)] },
    /// No mark moves.
    Keep,
}

impl MarkMove<'_> {
    /// Where a mark at `p` goes when the splice at `at` replaced `removed`
    /// with `inserted`; `None` when it is gone. `nodel`: see `Marks::each`.
    pub(crate) fn apply(self, p: Pos, at: Pos, removed: &str, inserted: &str, nodel: bool) -> Option<Pos> {
        match self {
            MarkMove::Keep => Some(p),
            MarkMove::DeleteLines { first, last } => moved_by_delete(p, first, last, nodel),
            MarkMove::InsertLines { at: row, count } => Some(if p.row >= row { Pos::new(p.row + count, p.col) } else { p }),
            MarkMove::Join { row, lines } => {
                let n = lines.len();
                Some(if p.row > row && p.row <= row + n {
                    let (amount, spaces_removed) = lines[p.row - row - 1];
                    Pos::new(row, moved_by_join(p.col, amount, spaces_removed))
                } else if p.row > row + n {
                    Pos::new(p.row - n, p.col)
                } else {
                    p
                })
            }
            MarkMove::Chars => {
                let k = removed.matches('\n').count();
                let m = inserted.matches('\n').count();
                if k == m {
                    return Some(p);
                }
                let mut p = p;
                if k > 0 {
                    let last = at.row + k;
                    if p.row > at.row && p.row <= last {
                        // Vim's `op_delete()` deletes the middle lines (a
                        // `'^` there is gone) and joins what is left of the
                        // last one after the first line's kept `at.col`
                        // chars; `del_bytes()` moved no mark before that.
                        if !nodel && p.row < last {
                            return None;
                        }
                        p = Pos::new(at.row, p.col + at.col);
                    } else if p.row > last {
                        p.row -= k;
                    }
                }
                if m > 0 && p.row > at.row {
                    p.row += m;
                }
                Some(p)
            }
        }
    }
}

/// Vim's `del_lines()` (`mark_adjust(first, last, MAXLNUM, -n)`).
fn moved_by_delete(p: Pos, first: usize, last: usize, nodel: bool) -> Option<Pos> {
    if p.row < first {
        Some(p)
    } else if p.row <= last {
        nodel.then_some(Pos::new(first, p.col))
    } else {
        Some(Pos::new(p.row - (last - first + 1), p.col))
    }
}

/// Vim's `u_undoredo()`: rows `first..first + now` became `new` rows
/// (`mark_adjust(first, first + now - 1, MAXLNUM, new - now)` when the
/// sizes differ).
fn moved_by_replace(p: Pos, first: usize, now: usize, new: usize, nodel: bool) -> Option<Pos> {
    if now == new || p.row < first {
        Some(p)
    } else if p.row < first + now {
        nodel.then_some(Pos::new(first, p.col))
    } else {
        Some(Pos::new(p.row + new - now, p.col))
    }
}

/// Vim's `col_adjust()` for a mark on a line `J` joined.
fn moved_by_join(col: usize, amount: isize, spaces_removed: isize) -> usize {
    let c = col as isize;
    let new = if amount < 0 && c <= -amount {
        0
    } else if c < spaces_removed {
        amount + spaces_removed
    } else {
        c + amount
    };
    new.max(0) as usize
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
    /// The Visual area when the change began (Vim's `uh_visual`); undo puts it back.
    visual: Option<LastVisual>,
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
    /// The marks this buffer's edits move (`gi`; `gv`). Dropped with the
    /// history when the text changed outside the engine.
    pub(crate) marks: Marks,
}

impl History {
    /// Opens a step unless one is open. `caret` is Vim's `uh_cursor`: where
    /// `u` returns when the change is around it.
    pub(crate) fn begin(&mut self, caret: Pos) {
        if self.open.is_none() {
            let empty_before = self.emptied;
            let visual = self.marks.visual;
            self.open = Some(Step { edits: Vec::new(), caret_before: caret, empty_before, empty_after: false, visual });
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

    /// Vim's `ML_EMPTY` (see the field), which only a blank buffer can be:
    /// text put in outside the engine (piece 4, a reload) ends it.
    pub(crate) fn emptied<B: TextBuf>(&self, buf: &B) -> bool {
        self.emptied && is_blank_buffer(buf)
    }

    /// Forgets every step: the text changed outside the engine, so none of
    /// them fits it any more (spec §3.11).
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Undoes the newest step; the caret it lands on.
    pub(crate) fn undo<B: TextBuf>(&mut self, buf: &mut B) -> Option<Pos> {
        let mut step = self.undo.pop()?;
        let at = undo_redo(buf, &mut step, true, &mut self.marks);
        self.emptied = step.empty_before && is_blank_buffer(buf);
        self.changed |= step.changes();
        self.redo.push(step);
        Some(at)
    }

    /// Redoes the newest undone step; the caret it lands on.
    pub(crate) fn redo<B: TextBuf>(&mut self, buf: &mut B) -> Option<Pos> {
        let mut step = self.redo.pop()?;
        let at = undo_redo(buf, &mut step, false, &mut self.marks);
        self.emptied = step.empty_after && is_blank_buffer(buf);
        self.changed |= step.changes();
        self.undo.push(step);
        Some(at)
    }

    /// What the open step's last splice put in, as Vim's `'[` and `']` marks
    /// after a charwise put: where the text starts, and the last char of its
    /// last line (column 0 when that line is empty, as after a text ending
    /// in a line break). A put of whole lines does not use this: its splice
    /// can carry a line break that is not part of the text.
    pub(crate) fn last_put(&self) -> Option<(Pos, Pos)> {
        let e = self.open.as_ref()?.edits.last()?;
        let end = end_of(e.at, &e.inserted);
        Some((e.at, Pos::new(end.row, end.col.saturating_sub(1))))
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
fn block<B: TextBuf>(buf: &B, at: Pos, now: &str, then: &str, join: bool) -> Block {
    let nl = |s: &str| s.matches('\n').count();
    let whole = |s: &str| s.is_empty() || s.ends_with('\n');
    let joined = |s: &str| s.is_empty() || s.starts_with('\n');
    // A join that saved both lines skips the whole-line rules.
    if !join && at.col == 0 && whole(now) && whole(then) {
        // Whole lines taken or given (`dd` in the middle, `p` linewise).
        let lines = then.split_terminator('\n').map(str::to_string).collect();
        return Block { first: at.row, now: nl(now), lines };
    }
    if !join && at.col == buf.line_len(at.row) && joined(now) && joined(then) {
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
/// file's `u` and `u<C-r>` cases pin all of this. The marks move once, as
/// `u_undoredo()`'s `mark_adjust()` moves them for an undo entry whose size
/// changed, by the step's net block: from the first row any edit touched to
/// the last, sized before and after.
fn undo_redo<B: TextBuf>(buf: &mut B, step: &mut Step, undo: bool, marks: &mut Marks) -> Pos {
    let before = marks.visual;
    let saved = step.caret_before;
    let order: Vec<&Edit> = if undo { step.edits.iter().rev().collect() } else { step.edits.iter().collect() };
    // Vim's `newlnum`, as a 0-based row of the first line in a block.
    let mut newlnum = usize::MAX;
    let mut row = None;
    // The step's net block, for the marks: the rows before its first block
    // and after its last one, which no edit touches.
    let lines_before = buf.line_count();
    let (mut head, mut tail) = (usize::MAX, usize::MAX);
    for (n, e) in order.iter().enumerate() {
        let (now, then) = if undo { (&e.inserted, &e.removed) } else { (&e.removed, &e.inserted) };
        let b = block(buf, e.at, now, then, e.joined);
        head = head.min(b.first);
        tail = tail.min(buf.line_count().saturating_sub(b.first + b.now));
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
    // Vim's `u_undoredo()` moves the marks by each undo entry's net size,
    // and one change (a whole Insert session) is one entry: a session whose
    // line count changed and changed back moves no mark. The step's net
    // block stands in for that entry.
    if head != usize::MAX {
        let now = lines_before.saturating_sub(head + tail);
        let new = buf.line_count().saturating_sub(head + tail);
        marks.lines_replaced(head, now, new);
    }
    // Vim's `u_undoredo()` puts back the Visual area the change was made
    // with and keeps the one it replaces, for the way back.
    if let Some(v) = step.visual {
        marks.visual = Some(v);
        step.visual = before;
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
        self.splice_moving(start, end, text, MarkMove::Chars);
    }

    /// [`Ed::splice`] for a command that moves the marks its own way (see
    /// [`MarkMove`]).
    pub(crate) fn splice_moving(&mut self, start: Pos, end: Pos, text: &str, how: MarkMove<'_>) {
        self.splice_as(start, end, text, false, how);
    }

    /// Joins the line `end` is on to the one `start` is on, deleting the
    /// break between them (`start` is the end of the upper line, `end` the
    /// start of the lower). Vim's `do_join()` and `ins_bs()` save both
    /// lines, so the undo block is the two lines, even when they are empty
    /// and the edit looks like deleting a whole line. The marks move as
    /// for any charwise join (`ins_bs()`).
    pub(crate) fn splice_lines_joined(&mut self, start: Pos, end: Pos) {
        self.splice_as(start, end, "", true, MarkMove::Chars);
    }

    fn splice_as(&mut self, start: Pos, end: Pos, text: &str, joined: bool, how: MarkMove<'_>) {
        let removed = self.buf.slice(start, end);
        if removed == text {
            return;
        }
        self.hist.begin(self.buf.cursor());
        self.buf.splice(start, end, text);
        self.hist.emptied = false;
        self.hist.marks.adjust(start, &removed, text, how);
        self.hist.record(Edit { at: start, removed, inserted: text.to_string(), joined });
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
        self.hist.record(Edit { at: Pos::new(first, 0), removed: text.clone(), inserted: text, joined: false });
    }

    /// Vim's `u_save_cursor()`: [`Ed::save_rows`] for the caret's line (`rX`
    /// on an X, `p` of `""`, `J` joining one line).
    pub(crate) fn save_cursor_line(&mut self, caret: Pos) {
        self.save_rows(caret, caret.row, caret.row);
    }

    /// A line was put in, so the buffer has one again (Vim's `ML_EMPTY`
    /// ends) even when the put spliced nothing: a one-line field puts an
    /// empty linewise register as no text.
    pub(crate) fn put_a_line(&mut self) {
        self.hist.emptied = false;
    }

    /// Deletes every line: Vim's buffer is then `ML_EMPTY`. Vim's
    /// `u_savedel` saves even when the buffer was already one blank line,
    /// so `dd` there is still a step `u` undoes.
    pub(crate) fn empty_buffer(&mut self) {
        let last = self.buf.line_count() - 1;
        if last == 0 && self.buf.line_len(0) == 0 {
            self.save_line(0);
            self.hist.marks.lines_deleted(0, 0);
        } else {
            let len = self.buf.line_len(last);
            self.splice_moving(Pos::new(0, 0), Pos::new(last, len), "", MarkMove::DeleteLines { first: 0, last });
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

    /// Review probe P4: a delete of the last line made outside an operator
    /// (the step saved at a caret above it), undone and redone. Vim clamps
    /// the landing row before the `o` rule and lands on the saved caret.
    #[test]
    fn a_redo_past_the_end_clamps_the_row_before_the_o_rule() {
        let mut state = EditorState::new(Lines::from("a\n  b\n  c\n  d\n  e"));
        let mut visual = None;
        let mut buf = BodyBuf::new(&mut state, &mut visual);
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
        let mut visual = None;
        let mut buf = BodyBuf::new(&mut state, &mut visual);
        let mut hist = History::default();
        hist.begin(Pos::new(0, 0));
        delete_lines(&mut Ed { buf: &mut buf, hist: &mut hist }, 0, 0);
        hist.commit();
        assert!(hist.can_undo() && hist.emptied(&buf));
        assert!(!hist.take_changed(), "no text changed");
        assert_eq!(hist.undo(&mut buf), Some(Pos::new(0, 0)));
        assert!(!hist.can_undo());
    }

    /// The mark rules, each read from Vim's source and pinned by a probe
    /// (plan 3b, batches p4 to p6).
    #[test]
    fn marks_move_as_vims_do() {
        let p = Pos::new;
        // Whole lines deleted: a mark on them is gone ('^) or goes to the
        // first deleted line (the Visual area); marks below move up.
        let del = MarkMove::DeleteLines { first: 1, last: 2 };
        assert_eq!(del.apply(p(2, 3), p(0, 0), "", "", false), None);
        assert_eq!(del.apply(p(2, 3), p(0, 0), "", "", true), Some(p(1, 3)));
        assert_eq!(del.apply(p(4, 1), p(0, 0), "", "", false), Some(p(2, 1)));
        assert_eq!(del.apply(p(0, 1), p(0, 0), "", "", false), Some(p(0, 1)));
        // Lines put in move the marks at and below them.
        let ins = MarkMove::InsertLines { at: 1, count: 2 };
        assert_eq!(ins.apply(p(1, 0), p(0, 0), "", "", false), Some(p(3, 0)));
        assert_eq!(ins.apply(p(0, 5), p(0, 0), "", "", false), Some(p(0, 5)));
        // A charwise delete over lines (op_delete: truncate, delete the
        // middle lines, join what is left): a mark on the last line moves by
        // the first line's kept length; a '^ on a middle line is gone.
        let at = p(0, 1);
        assert_eq!(MarkMove::Chars.apply(p(1, 2), at, "bc def\ngh", "", false), Some(p(0, 3)));
        assert_eq!(MarkMove::Chars.apply(p(1, 2), at, "x\nmid\ngh", "", false), None);
        assert_eq!(MarkMove::Chars.apply(p(1, 2), at, "x\nmid\ngh", "", true), Some(p(0, 3)));
        assert_eq!(MarkMove::Chars.apply(p(3, 0), at, "x\ngh", "", false), Some(p(2, 0)));
        // A split moves only the lines below it; an edit that keeps the
        // line count moves nothing.
        assert_eq!(MarkMove::Chars.apply(p(0, 5), p(0, 2), "", "\n  ", false), Some(p(0, 5)));
        assert_eq!(MarkMove::Chars.apply(p(1, 5), p(0, 2), "", "\n  ", false), Some(p(2, 5)));
        assert_eq!(MarkMove::Chars.apply(p(1, 1), p(0, 0), "ab\ncd", "AB\nCD", false), Some(p(1, 1)));
        // `J`: a joined line's marks move to where its text lands; one in the
        // blanks it lost goes to the joining space; lines below move up.
        let join = MarkMove::Join { row: 0, lines: &[(1, 1)] };
        assert_eq!(join.apply(p(1, 3), p(0, 2), "", "", false), Some(p(0, 4)));
        assert_eq!(join.apply(p(1, 0), p(0, 2), "", "", false), Some(p(0, 2)));
        assert_eq!(join.apply(p(2, 0), p(0, 2), "", "", false), Some(p(1, 0)));
        assert_eq!(MarkMove::Keep.apply(p(5, 5), p(0, 0), "a\nb", "", false), Some(p(5, 5)));
    }

    /// Insert `BS` in column 0 joins through `splice_lines_joined`, which
    /// moves the marks as any charwise join does (Vim's `ins_bs()` joins
    /// with `do_join()`, no space): a mark below moves up, and one on the
    /// joined line lands after the text of the line above.
    #[test]
    fn a_join_that_saves_both_lines_still_moves_the_marks() {
        let mut state = EditorState::new(Lines::from("ab\ncd\nef"));
        let mut visual = None;
        let mut buf = BodyBuf::new(&mut state, &mut visual);
        let mut hist = History::default();
        hist.marks.insert = Some(Pos::new(2, 1));
        Ed { buf: &mut buf, hist: &mut hist }.splice_lines_joined(Pos::new(0, 2), Pos::new(1, 0));
        assert_eq!(hist.marks.insert, Some(Pos::new(1, 1)));
        Ed { buf: &mut buf, hist: &mut hist }.splice_lines_joined(Pos::new(0, 4), Pos::new(1, 0));
        assert_eq!(hist.marks.insert, Some(Pos::new(0, 5)));
        assert_eq!(buf.text(), "abcdef");
    }
}
