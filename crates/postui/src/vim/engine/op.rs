//! Operators (spec §3.6): a motion's or object's reach becomes a range
//! under Vim's operator rules, and `d` `y` `c` apply to it; Visual
//! `~ u U > <` reuse the ranges. Also undo/redo, put, replace and join.

use super::buf::{Pos, TextBuf};
use super::case;
use super::class::white;
use super::history::Ed;
use super::keys::{CaseOp, Cmd, Op, Reach};
use super::motion::{self, MKind, MotionCx};
use super::register::{RegKind, Register};
use super::settings::{SHIFTWIDTH, TABSTOP};
use super::{BufState, Engine, Outcome, first_non_blank, first_non_blank_fix};
use crate::components::line_input::flatten_paste;
use ratatui::crossterm::event::KeyEvent;

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

/// Vim's `inindent(0)` at `at`: nothing but blanks before it on its line.
fn in_indent<B: TextBuf>(buf: &B, at: Pos) -> bool {
    buf.line(at.row).iter().take(at.col).all(|&c| white(c))
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
        let rest_blank = buf.line(range.end.row).iter().skip(range.end.col).all(|&c| white(c));
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

/// `g~` `gu` `gU`, `~` and Visual `~ u U` over `r` (whole rows when
/// linewise), re-casing only the chars inside it, one by one, with Vim's case
/// rules (`case::swap`; `ß` may become "SS", so the text can grow). Vim's own
/// `op_tilde()` counts bytes and so can overrun the range when a char's UTF-8
/// length changes; the engine does not (divergences.toml).
pub(crate) fn recase_range<B: TextBuf>(ed: &mut Ed<'_, B>, how: CaseOp, r: Range) {
    let (start, end) = match r.kind {
        RKind::Char => (r.start, r.end),
        RKind::Line => (Pos::new(r.start.row, 0), Pos::new(r.end.row, ed.buf.line_len(r.end.row))),
    };
    let text = ed.buf.slice(start, end);
    let mut new = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\n' {
            new.push(c);
        } else {
            case::swap(how, c, &mut new);
        }
    }
    ed.splice(start, end, &new);
}

/// Visual `>` `<` over rows `first..=last`, `amount` shiftwidths each;
/// empty rows are skipped and the new indent is spaces (`expandtab`). The
/// caret goes to the first row's first non-blank.
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

/// Puts `reg` `n` times (spec §3.6 rule 6, §3.10; Vim's `do_put()`); the
/// caret after.
pub(crate) fn put<B: TextBuf>(ed: &mut Ed<'_, B>, reg: &Register, before: bool, n: usize) -> Pos {
    let caret = ed.buf.cursor();
    if reg.text.is_empty() {
        return put_chars(ed, caret, "", before);
    }
    if !B::MULTILINE {
        // A one-line field: a linewise register goes in charwise without
        // its line break, and line breaks become spaces (key list §5).
        let text = match reg.kind {
            RegKind::Line => reg.text.strip_suffix('\n').unwrap_or(&reg.text),
            RegKind::Char => &reg.text,
        };
        return put_chars(ed, caret, &flatten_paste(text).repeat(n), before);
    }
    match reg.kind {
        RegKind::Char => put_chars(ed, caret, &reg.text.repeat(n), before),
        RegKind::Line => {
            let body = reg.text.repeat(n);
            put_lines(ed, if before { caret.row } else { caret.row + 1 }, &body)
        }
    }
}

/// Inserts linewise `body` (each line ended by `'\n'`) as whole rows
/// starting at `row`; the caret goes to the first new row's first
/// non-blank. After the last line there is no row to go before, so the text
/// takes a break before it and none after.
pub(crate) fn put_lines<B: TextBuf>(ed: &mut Ed<'_, B>, row: usize, body: &str) -> Pos {
    let lines = ed.buf.line_count();
    let row = if row < lines {
        ed.splice(Pos::new(row, 0), Pos::new(row, 0), body);
        row
    } else {
        let last = lines - 1;
        let len = ed.buf.line_len(last);
        let text = body.strip_suffix('\n').unwrap_or(body);
        ed.splice(Pos::new(last, len), Pos::new(last, len), &format!("\n{text}"));
        last + 1
    };
    Pos::new(row, first_non_blank(&ed.buf.line(row)))
}

/// A charwise put: `p` goes after the caret's char (at column 0 on an empty
/// line), `P` before it. One line leaves the caret on the last char put,
/// more than one on the first. Putting nothing (the register holds `""`)
/// is still an undo step: `do_put()` saves the caret's line before it
/// looks at the register.
fn put_chars<B: TextBuf>(ed: &mut Ed<'_, B>, caret: Pos, text: &str, before: bool) -> Pos {
    if text.is_empty() {
        ed.save_cursor_line(caret);
        return caret;
    }
    let len = ed.buf.line_len(caret.row);
    let at = if before || len == 0 { caret } else { Pos::new(caret.row, caret.col + 1) };
    ed.splice(at, at, text);
    if text.contains('\n') { at } else { Pos::new(at.row, at.col + text.chars().count() - 1) }
}

/// Joins `n` lines from `row` the way `J` does: Vim's `do_join()` with
/// `insert_space` under `nojoinspaces`. Each later line loses its leading
/// blanks and gets one space before it, except when it is then empty,
/// starts with `)`, the text joined so far is empty, or the line before it
/// ended in a blank (that line's text after its own leading blanks: an
/// all-blank line ends in nothing). The caret goes where the last line was
/// joined.
/// `n == 1` (a count run past the end on the last line) changes nothing
/// but is still an undo step, and the caret goes to column 0.
pub(crate) fn join_rows<B: TextBuf>(ed: &mut Ed<'_, B>, row: usize, n: usize) -> Pos {
    if n < 2 {
        ed.save_cursor_line(Pos::new(row, ed.buf.cursor().col));
        return Pos::new(row, 0);
    }
    let first = ed.buf.line(row).into_owned();
    let mut joined: Vec<char> = Vec::new();
    let mut sum = first.len();
    let mut end = first.last().copied();
    let mut col = 0;
    for t in 1..n {
        let line = ed.buf.line(row + t);
        let lead = first_non_blank(&line);
        let rest = &line[lead..];
        let space = !rest.is_empty() && rest[0] != ')' && sum != 0 && !matches!(end, Some(' ' | '\t'));
        if space {
            joined.push(' ');
        }
        col = sum;
        sum += usize::from(space) + rest.len();
        joined.extend_from_slice(rest);
        end = rest.last().copied();
    }
    let last = row + n - 1;
    let text: String = joined.iter().collect();
    ed.splice(Pos::new(row, first.len()), Pos::new(last, ed.buf.line_len(last)), &text);
    Pos::new(row, col)
}

impl Engine {
    /// An operator with its motion, object or doubled letter (spec §3.6).
    /// `false` when its motion or object failed and it did not run (Vim
    /// then sets no `.`).
    pub(super) fn exec_operate<B: TextBuf>(
        &mut self,
        op: Op,
        reach: Reach,
        count: usize,
        reg: Option<char>,
        buf: &mut B,
        st: &mut BufState,
    ) -> bool {
        let Some(span) = self.op_span(op, reach, count, buf, st) else {
            return false;
        };
        let r = range(buf, span, op);
        if op == Op::Change {
            let origin = Cmd::Operate { op, reach, count, reg };
            // Vim's `op_change()`: an empty region (`oap->empty`: exclusive,
            // start == end) or a buffer with no lines writes no register.
            let empty = span.kind == MKind::Exclusive && r.kind == RKind::Char && r.start == r.end;
            if empty || st.history.emptied(&*buf) {
                self.change_text(r, origin, buf, st);
            } else {
                self.change(r, reg, origin, buf, st);
            }
            return true;
        }
        if let Op::Case(how) = op {
            let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
            // Vim's `op_tilde()` saves the lines first, so a case change that
            // changes nothing is still an undo step; the caret goes to the
            // range start (spec §3.6 rule 5).
            ed.save_rows(r.start, r.start.row, r.end.row);
            recase_range(&mut ed, how, r);
            self.land_caret(r.start, buf, st);
            return true;
        }
        if let Op::Shift { right } = op {
            let (first, last) = (r.start.row, r.end.row);
            let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
            // Vim's `op_shift()`: the lines are saved first (so `<<` with no
            // indent is still an undo step), each non-empty line moves one
            // shiftwidth (an operator's count counts lines, not shifts), and
            // the caret goes to the first line's first non-blank.
            ed.save_rows(r.start, first, last);
            let caret = shift_rows(&mut ed, first, last, right, 1);
            self.land_caret(caret, buf, st);
            return true;
        }
        let caret = if op == Op::Yank {
            self.regs.yank(reg, yank_of(buf, r));
            r.start
        } else if st.history.emptied(&*buf) {
            // Vim's `op_delete`: nothing to do in a buffer with no lines.
            st.forget_want();
            return true;
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
                self.regs.delete(reg, yank_of(ed.buf, r));
                delete(&mut ed, r)
            }
        };
        self.land_caret(caret, buf, st);
        true
    }

    /// `p` `P` with a count.
    pub(super) fn exec_put<B: TextBuf>(&mut self, before: bool, count: usize, reg: Option<char>, buf: &mut B, st: &mut BufState) {
        let reg = self.regs.read(reg).clone();
        let caret = put(&mut Ed { buf: &mut *buf, hist: &mut st.history }, &reg, before, count.max(1));
        self.land_caret(caret, buf, st);
    }

    /// `r{c}` with a count (Vim's `nv_replace()`): fails whole, keeping the
    /// wanted column, when fewer than N chars remain (`false`: no `.`). The
    /// caret ends on the last char replaced.
    pub(super) fn exec_replace<B: TextBuf>(&mut self, ch: char, count: usize, buf: &mut B, st: &mut BufState) -> bool {
        let caret = buf.cursor();
        let n = count.max(1);
        if caret.col + n > buf.line_len(caret.row) {
            return false;
        }
        if ch == '\t' {
            self.replace_tabs(n, buf, st);
            return true;
        }
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        // `nv_replace()` saves the line first, so `r` that puts back the
        // same chars (`rX` on an X, `r<Space>` on a space) is still a step.
        ed.save_cursor_line(caret);
        let text: String = std::iter::repeat_n(ch, n).collect();
        ed.splice(caret, Pos::new(caret.row, caret.col + n), &text);
        self.land_caret(Pos::new(caret.row, caret.col + n - 1), buf, st);
        true
    }

    /// `{N}R<Tab><Esc>`, which Vim's `nv_replace()` runs for `{N}r<Tab>`
    /// under 'expandtab' once the length check passed: each Tab replaces
    /// one char with spaces to the next tab stop, and past the line's end
    /// only inserts them. Its `.` repeats this `R`, so the replay has no
    /// length check (`3r<Tab>j0.` on a shorter line). The line is saved
    /// first, so spaces put back over spaces are still a step.
    pub(super) fn replace_tabs<B: TextBuf>(&mut self, n: usize, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        ed.save_cursor_line(caret);
        let mut at = caret;
        for _ in 0..n {
            let width = TABSTOP - motion::vcol_of(&ed.buf.line(at.row), at.col) % TABSTOP;
            let end = (at.col + 1).min(ed.buf.line_len(at.row));
            ed.splice(at, Pos::new(at.row, end), &" ".repeat(width));
            at.col += width;
        }
        self.land_caret(Pos::new(at.row, at.col - 1), buf, st);
    }

    /// `~` with a count (`notildeop`, Vim's `n_swapchar()`): toggles the case
    /// of `count` chars from the caret on its line; the caret lands after
    /// the last one, clamped to the last char. On an empty line it fails
    /// (`false`: no undo step, no `.`, the wanted column kept); otherwise the
    /// line is saved first, so `~` on `-` is still an undo step.
    pub(super) fn exec_tilde<B: TextBuf>(&mut self, count: usize, buf: &mut B, st: &mut BufState) -> bool {
        let caret = buf.cursor();
        let len = buf.line_len(caret.row);
        if len == 0 {
            return false;
        }
        let n = count.max(1).min(len - caret.col);
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        ed.save_cursor_line(caret);
        let end = Pos::new(caret.row, caret.col + n);
        recase_range(&mut ed, CaseOp::Toggle, Range { start: caret, end, kind: RKind::Char });
        self.land_caret(Pos::new(caret.row, (caret.col + n).min(len - 1)), buf, st);
        true
    }

    /// `J` with a count (Vim's `nv_join()`); a no-op in a one-line field.
    /// On the last line `J` fails; a bigger count joins what there is.
    /// Returns the count it joined with, which is what Vim stores for `.`
    /// (`9J` two lines above the end repeats as `3J`); `None` when it
    /// failed.
    pub(super) fn exec_join<B: TextBuf>(&mut self, count: usize, buf: &mut B, st: &mut BufState) -> Option<usize> {
        if !B::MULTILINE {
            return None;
        }
        let row = buf.cursor().row;
        let lines = buf.line_count();
        let mut n = count.max(2);
        if row + n > lines {
            if n <= 2 {
                return None;
            }
            n = lines - row;
        }
        let caret = join_rows(&mut Ed { buf: &mut *buf, hist: &mut st.history }, row, n);
        self.land_caret(caret, buf, st);
        Some(n)
    }

    /// Puts the caret where a command left it (clamped for the mode) and
    /// resets the wanted column (Vim's `w_set_curswant = TRUE`).
    pub(super) fn land_caret<B: TextBuf>(&mut self, at: Pos, buf: &mut B, st: &mut BufState) {
        let at = self.clamped(buf, at);
        buf.set_cursor(at);
        st.forget_want();
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
            Reach::Object { obj, inner } => match super::object::pick(buf, from, None, false, obj, inner, count) {
                Ok(picked) => Some(picked.span()),
                Err(missed) => {
                    // Vim's `nv_object`: the operator is cancelled, the caret
                    // stays where the object's walk ended.
                    buf.set_cursor(self.clamped(buf, missed.at));
                    st.forget_want();
                    None
                }
            },
        }
    }

    /// `u` / `ctrl+r` with a count. Nothing left in this buffer: declined,
    /// and piece 4 hands the key to the app history (spec §3.11, §4.3).
    /// Either way the wanted column resets: Vim's `nv_kundo()` and
    /// `nv_redo_or_register()` set `w_set_curswant` even when
    /// `u_doit()` finds nothing to do (`j<C-r>k` aims for the caret's own
    /// column, not the one `j` kept).
    /// `declined` is what a declined command hands back (`Step::Undo`).
    pub(super) fn exec_undo<B: TextBuf>(
        &mut self,
        count: usize,
        redo: bool,
        declined: (Option<usize>, Vec<KeyEvent>),
        buf: &mut B,
        st: &mut BufState,
    ) -> Outcome {
        let available = if redo { st.history.can_redo() } else { st.history.can_undo() };
        if !available {
            st.forget_want();
            let (count, keys) = declined;
            return Outcome::Declined { count, keys };
        }
        let mut landed = None;
        for _ in 0..count.max(1) {
            let at = if redo { st.history.redo(buf) } else { st.history.undo(buf) };
            match at {
                Some(at) => landed = Some(at),
                None => break,
            }
        }
        match landed {
            Some(at) => self.land_caret(at, buf, st),
            None => st.forget_want(),
        }
        // Vim's `u_undoredo()` calls `changed_lines()` even for a step that
        // changed nothing, which drops the cached `w_virtcol`.
        st.forget_virtcol();
        Outcome::consumed()
    }
}
