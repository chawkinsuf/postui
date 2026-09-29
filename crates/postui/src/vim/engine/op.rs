//! Operators (spec §3.6): a motion's or object's reach becomes a range
//! under Vim's operator rules, and `d` `y` (Task 9: `c`) apply to it; Visual
//! `~ u U > <` (Task 11) reuse the ranges. Also undo/redo, and (Task 10)
//! put, replace and join.

use super::buf::{Pos, TextBuf};
use super::history::Ed;
use super::keys::{CaseOp, Op, Reach};
use super::motion::{self, MKind, MotionCx};
use super::register::{RegKind, Register};
use super::settings::SHIFTWIDTH;
use super::{BufState, Engine, Outcome, first_non_blank};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A motion's or object's reach, `start <= end` in buffer order.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Span {
    pub start: Pos,
    pub end: Pos,
    pub kind: MKind,
    /// Skip rule 1 (`<BS>` across a line break).
    pub no_adjust: bool,
}

impl Span {
    pub(crate) fn between(a: Pos, b: Pos, kind: MKind) -> Self {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        Self { start, end, kind, no_adjust: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RKind {
    Char,
    Line,
}

/// What an operator acts on. `Char`: `[start, end)`, where `end` may be
/// `(row + 1, 0)` to take a line break. `Line`: rows `start.row..=end.row`;
/// `start` keeps its column, which is where a yank leaves the caret.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Range {
    pub start: Pos,
    pub end: Pos,
    pub kind: RKind,
}

/// Vim's `beginline(BL_WHITE | BL_FIX)`: the first non-blank, but never
/// past the last char of an all-blank line.
fn first_non_blank_fix(line: &[char]) -> usize {
    first_non_blank(line).min(line.len().saturating_sub(1))
}

/// Vim's `inindent(0)` at `at`: nothing but blanks before it on its line.
fn in_indent<B: TextBuf>(buf: &B, at: Pos) -> bool {
    buf.line(at.row).iter().take(at.col).all(|&c| c == ' ' || c == '\t')
}

fn step_over<B: TextBuf>(buf: &B, p: Pos) -> Pos {
    if p.col < buf.line_len(p.row) { Pos::new(p.row, p.col + 1) } else { p }
}

/// Vim's operator rules (spec §3.6).
pub(crate) fn range<B: TextBuf>(buf: &B, span: Span, op: Op) -> Range {
    let Span { start, mut end, kind, no_adjust } = span;
    let mut range = match kind {
        MKind::Linewise => return Range { start, end, kind: RKind::Line },
        MKind::Inclusive => Range { start, end: step_over(buf, end), kind: RKind::Char },
        MKind::Exclusive => {
            // Rule 1 (`:help exclusive`): an exclusive end in column 0 moves
            // to the end of the previous line; from within the indent the
            // operator becomes linewise.
            if end.col == 0 && end.row > start.row && !no_adjust {
                if in_indent(buf, start) {
                    return Range { start, end: Pos::new(end.row - 1, 0), kind: RKind::Line };
                }
                end = Pos::new(end.row - 1, buf.line_len(end.row - 1));
            }
            Range { start, end, kind: RKind::Char }
        }
    };
    // Rule 1b (op_delete): a multi-line charwise delete that leaves only
    // blanks after its end and starts within the indent is linewise.
    if op == Op::Delete && range.end.row > range.start.row {
        let rest_blank = buf.line(range.end.row).iter().skip(range.end.col).all(|&c| c == ' ' || c == '\t');
        if rest_blank && in_indent(buf, start) {
            range.kind = RKind::Line;
        }
    }
    range
}

/// The text an operator takes, as a register (a linewise one ends in `'\n'`).
pub(crate) fn yank_of<B: TextBuf>(buf: &B, r: Range) -> Register {
    match r.kind {
        RKind::Char => Register { text: buf.slice(r.start, r.end), kind: RegKind::Char },
        RKind::Line => {
            let last = r.end.row;
            let text = buf.slice(Pos::new(r.start.row, 0), Pos::new(last, buf.line_len(last)));
            Register { text: text + "\n", kind: RegKind::Line }
        }
    }
}

/// Removes whole lines `first..=last`; a buffer keeps at least one line.
pub(crate) fn delete_lines<B: TextBuf>(ed: &mut Ed<'_, B>, first: usize, last: usize) {
    let lines = ed.buf.line_count();
    let last_len = ed.buf.line_len(last);
    if last + 1 < lines {
        ed.splice(Pos::new(first, 0), Pos::new(last + 1, 0), "");
    } else if first > 0 {
        let prev = ed.buf.line_len(first - 1);
        ed.splice(Pos::new(first - 1, prev), Pos::new(last, last_len), "");
    } else {
        ed.empty_buffer();
    }
}

/// Deletes `r`; the caret after (spec §3.6 rule 5).
pub(crate) fn delete<B: TextBuf>(ed: &mut Ed<'_, B>, r: Range) -> Pos {
    match r.kind {
        RKind::Char => {
            ed.splice(r.start, r.end, "");
            r.start
        }
        RKind::Line => {
            delete_lines(ed, r.start.row, r.end.row);
            let row = r.start.row.min(ed.buf.line_count() - 1);
            Pos::new(row, first_non_blank(&ed.buf.line(row)))
        }
    }
}

/// One char when the case mapping gives one, else the char unchanged
/// (Vim keeps `ß` under `gU`).
fn one(mut mapped: impl Iterator<Item = char>, c: char) -> char {
    match (mapped.next(), mapped.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

fn recase(how: CaseOp, c: char) -> char {
    match how {
        CaseOp::Upper => one(c.to_uppercase(), c),
        CaseOp::Lower => one(c.to_lowercase(), c),
        CaseOp::Toggle if c.is_lowercase() => one(c.to_uppercase(), c),
        CaseOp::Toggle if c.is_uppercase() => one(c.to_lowercase(), c),
        CaseOp::Toggle => c,
    }
}

/// Visual `~ u U` (and plan 3b's `g~` `gu` `gU`) over `r`; the caret goes
/// to its start (column 0 for linewise).
#[allow(dead_code)] // used from Task 11
pub(crate) fn recase_range<B: TextBuf>(ed: &mut Ed<'_, B>, how: CaseOp, r: Range) -> Pos {
    let (start, end) = match r.kind {
        RKind::Char => (r.start, r.end),
        RKind::Line => (Pos::new(r.start.row, 0), Pos::new(r.end.row, ed.buf.line_len(r.end.row))),
    };
    let text = ed.buf.slice(start, end);
    let new: String = text.chars().map(|c| if c == '\n' { c } else { recase(how, c) }).collect();
    ed.splice(start, end, &new);
    start
}

/// Visual `>` `<` over rows `first..=last`, `amount` shiftwidths each;
/// empty rows are skipped and the new indent is spaces (`expandtab`). The
/// caret goes to the first row's first non-blank.
#[allow(dead_code)] // used from Task 11
pub(crate) fn shift_rows<B: TextBuf>(ed: &mut Ed<'_, B>, first: usize, last: usize, right: bool, amount: usize) -> Pos {
    let step = SHIFTWIDTH * amount.max(1);
    for row in first..=last {
        let line = ed.buf.line(row).into_owned();
        if line.is_empty() {
            continue;
        }
        let lead = first_non_blank(&line);
        let width = motion::vcol_of(&line, lead);
        let new = if right { width + step } else { width.saturating_sub(step) };
        ed.splice(Pos::new(row, 0), Pos::new(row, lead), &" ".repeat(new));
    }
    Pos::new(first, first_non_blank(&ed.buf.line(first)))
}

impl Engine {
    /// An operator with its motion, object or doubled letter (spec §3.6).
    pub(super) fn exec_operate<B: TextBuf>(
        &mut self,
        op: Op,
        reach: Reach,
        count: usize,
        reg: Option<char>,
        buf: &mut B,
        st: &mut BufState,
    ) -> Outcome {
        // Task 9 replaces this with the change operator.
        if op == Op::Change {
            return Outcome::consumed();
        }
        let Some(span) = self.op_span(op, reach, count, buf, st) else {
            return Outcome::consumed();
        };
        let r = range(buf, span, op);
        let caret = if op == Op::Yank {
            self.regs.write(reg, yank_of(buf, r));
            r.start
        } else if st.history.emptied() {
            // Vim's `op_delete`: nothing to do in a buffer with no lines.
            return Outcome::consumed();
        } else {
            let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
            // Vim's `u_save` runs with the caret on the range start.
            ed.hist.begin(r.start);
            if r.kind == RKind::Char && r.start == r.end {
                // An empty region writes no register. An exclusive one
                // (`dh` in column 0, `x` on an empty line) still saves its
                // line for undo, so `u` undoes it; `D` on an empty line
                // does nothing at all.
                if span.kind == MKind::Exclusive {
                    ed.save_line(r.start.row);
                }
                r.start
            } else {
                self.regs.write(reg, yank_of(ed.buf, r));
                delete(&mut ed, r)
            }
        };
        let caret = self.clamped(buf, caret);
        buf.set_cursor(caret);
        st.forget_want();
        Outcome::consumed()
    }

    /// What an operator acts on, or `None` when its motion or object fails
    /// (the operator is cancelled).
    pub(super) fn op_span<B: TextBuf>(
        &mut self,
        op: Op,
        reach: Reach,
        count: usize,
        buf: &mut B,
        st: &mut BufState,
    ) -> Option<Span> {
        let from = buf.cursor();
        match reach {
            Reach::Line => {
                let last = buf.line_count() - 1;
                let n = count.max(1);
                // Vim's `cursor_down()` fails on the last line: `2dd` there
                // does nothing (spec §3.6 rule 4 clamps only from above it).
                if n > 1 && from.row == last {
                    return None;
                }
                let row = (from.row + n - 1).min(last);
                // Vim's `nv_lineop`: except for `yy`, the caret first goes to
                // the last line's first non-blank, so `dd` right of the
                // indent starts its range (and its undo caret) there.
                let col = if op == Op::Yank { from.col } else { first_non_blank_fix(&buf.line(row)) };
                Some(Span::between(from, Pos::new(row, col), MKind::Linewise))
            }
            Reach::Motion(m) => {
                let want = self.want_at(buf, st, from);
                let cx = MotionCx { op: Some(op), visual: false, want };
                let moved = motion::run(buf, from, m, count, &cx, &mut self.last_find);
                if moved.failed {
                    // The caret still goes where Vim's walk ended.
                    buf.set_cursor(self.clamped(buf, moved.to));
                    if moved.want != motion::WantUpdate::Keep {
                        st.forget_want();
                    }
                    return None;
                }
                let mut span = Span::between(from, moved.to, moved.kind);
                span.no_adjust = moved.no_adjust;
                Some(span)
            }
            // Task 8 replaces this arm with `object::select`.
            Reach::Object { .. } => None,
        }
    }

    /// `u` / `ctrl+r` with a count. Nothing left in this buffer: declined,
    /// and piece 4 hands the key to the app history (spec §3.11, §4.3).
    pub(super) fn exec_undo<B: TextBuf>(&mut self, count: usize, redo: bool, buf: &mut B, st: &mut BufState) -> Outcome {
        let available = if redo { st.history.can_redo() } else { st.history.can_undo() };
        if !available {
            let key = if redo {
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
            } else {
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
            };
            return Outcome::Declined { count: (count > 0).then_some(count), keys: vec![key] };
        }
        let mut landed = None;
        for _ in 0..count.max(1) {
            let at = if redo { st.history.redo(buf) } else { st.history.undo(buf) };
            match at {
                Some(at) => landed = Some(at),
                None => break,
            }
        }
        if let Some(at) = landed {
            let at = self.clamped(buf, at);
            buf.set_cursor(at);
        }
        st.forget_want();
        Outcome::consumed()
    }
}
