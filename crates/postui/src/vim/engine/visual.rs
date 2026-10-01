//! Visual mode (spec §3.9): `v` `V` `o`, objects that replace the
//! selection, and the Visual operators. The engine keeps the anchor; the
//! buffer only paints it.

use super::buf::{Pos, TextBuf};
use super::history::{Ed, MarkMove};
use super::keys::{Cmd, Object, VisualOp};
use super::motion::{Want, WantUpdate, char_width, updated_want, vcol_of};
use super::object;
use super::op::{RKind, Range, delete, join_rows, put, put_lines, recase_range, shift_rows, yank_of};
use super::register::RegKind;
use super::{BufState, Engine, Mode, Outcome, Shape, VisualSize, first_non_blank};

impl Engine {
    pub(super) fn exec_visual<B: TextBuf>(&mut self, cmd: Cmd, buf: &mut B, st: &mut BufState) -> Outcome {
        let caret = buf.cursor();
        match cmd {
            Cmd::VisualStart(shape) => match self.mode {
                Mode::Visual(s) if s == shape => {
                    // Vim's `end_visual_mode()`: `adjust_cursor_eol()` moves
                    // the caret off a line's end but keeps `w_curswant`; one
                    // still unset was taken before this command, in Visual.
                    let want = self.want_at(buf, st, caret);
                    self.leave_to_normal(buf);
                    st.set_want(want, buf.cursor());
                }
                Mode::Visual(_) => self.mode = Mode::Visual(shape),
                _ => {
                    // Vim updates `w_curswant` before each command
                    // (`update_topline_cursor()`), so a wanted column still
                    // unset is taken here, before the mode changes: in
                    // Normal, on a tab, its last cell. `v` leaves it alone.
                    let want = self.want_at(buf, st, caret);
                    st.set_want(want, caret);
                    self.visual = Some(caret);
                    self.mode = Mode::Visual(shape);
                }
            },
            Cmd::VisualSwap => {
                if let Some(anchor) = self.visual {
                    self.visual = Some(caret);
                    self.land_caret(anchor, buf, st);
                }
            }
            Cmd::VisualExit => {
                // Vim's `nv_esc()` sets `w_set_curswant` after leaving, so
                // the next command reads `w_virtcol`: the cached one while
                // the caret stays put (taken where it last moved, in
                // Visual), else recomputed in Normal.
                let here = updated_want(buf, caret, WantUpdate::Here, Want::default(), self.tab_rule(st, caret));
                self.leave_to_normal(buf);
                if buf.cursor() == caret {
                    st.set_want(here, caret);
                } else {
                    st.forget_want();
                }
            }
            Cmd::VisualObject { obj, inner, count } => self.visual_object(obj, inner, count, buf, st),
            Cmd::VisualOp { op, count, reg } => return self.visual_op(op, count, reg, buf, st),
            _ => {}
        }
        Outcome::consumed()
    }

    fn leave_visual(&mut self) {
        self.mode = Mode::Normal;
        self.visual = None;
    }

    /// Leaves Visual with no operator: the caret steps off a line's end.
    fn leave_to_normal<B: TextBuf>(&mut self, buf: &mut B) {
        self.leave_visual();
        let at = self.clamped(buf, buf.cursor());
        buf.set_cursor(at);
    }

    /// Vim's `nv_object()` in Visual: the object's start becomes the anchor
    /// and its end the caret. A word, quote or block makes the selection
    /// charwise; a paragraph makes it linewise, or keeps its shape when it
    /// grows a selection over more than one line. When there is no such
    /// object, the shape stays, but the caret goes where the object's walk
    /// stopped (and the anchor where Vim had moved it).
    fn visual_object<B: TextBuf>(&mut self, obj: Object, inner: bool, count: usize, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let anchor = self.visual.unwrap_or(caret);
        match object::pick(buf, caret, Some(anchor), self.mode == Mode::Visual(Shape::Line), obj, inner, count) {
            Ok(p) => {
                self.visual = Some(p.start);
                if let Some(shape) = p.shape {
                    self.mode = Mode::Visual(shape);
                }
                self.land_caret(p.end, buf, st);
            }
            Err(missed) => {
                if let Some(anchor) = missed.anchor {
                    self.visual = Some(anchor);
                }
                self.land_caret(missed.at, buf, st);
            }
        }
    }

    /// The selection as an operator range (Vim's `do_pending_operator()`).
    /// Charwise is inclusive, and takes the line break when its end sits on
    /// a line's end below which there is a line. Linewise starts at the
    /// anchor's column 0 unless the caret comes first, where Vim leaves the
    /// caret (so `Vy` goes to column 0 and `Vky` keeps the caret's column).
    /// A `.` replay starts at the anchor itself (the caret `.` was typed
    /// at): Vim's `redo_VIsual_busy` branch takes `oap->start` from the
    /// cursor and skips the column-0 rule.
    pub(super) fn visual_range<B: TextBuf>(&self, buf: &B, lines: bool) -> Range {
        let caret = buf.cursor();
        let anchor = self.visual.unwrap_or(caret);
        if lines || self.mode == Mode::Visual(Shape::Line) {
            let from = if self.replaying_visual { anchor } else { Pos::new(anchor.row, 0) };
            let start = from.min(caret);
            let end = anchor.max(caret);
            return Range { start, end: Pos::new(end.row, 0), kind: RKind::Line };
        }
        let (lo, hi) = if anchor <= caret { (anchor, caret) } else { (caret, anchor) };
        let len = buf.line_len(hi.row);
        let end = if hi.col < len {
            Pos::new(hi.row, hi.col + 1)
        } else if hi.row + 1 < buf.line_count() {
            Pos::new(hi.row + 1, 0)
        } else {
            Pos::new(hi.row, len)
        };
        Range { start: Pos::new(lo.row, lo.col.min(buf.line_len(lo.row))), end, kind: RKind::Char }
    }

    /// The rows the selection covers, for the operators that work on lines
    /// (`J` `>` `<`: Vim's `op_on_lines()`, which never take the break).
    fn visual_rows<B: TextBuf>(&self, buf: &B) -> (usize, usize) {
        let caret = buf.cursor();
        let anchor = self.visual.unwrap_or(caret);
        (anchor.row.min(caret.row), anchor.row.max(caret.row))
    }

    /// The selection's size for `.` (Vim's `resel_VIsual_*`, taken in
    /// `do_pending_operator()` from the ordered ends before a linewise
    /// selection is widened). `D` `X` `Y` `C` `S` `R` make it linewise
    /// (`v_visop()` sets `VIsual_mode = 'V'`).
    fn visual_size<B: TextBuf>(&self, lines: bool, buf: &B, st: &BufState) -> VisualSize {
        let caret = buf.cursor();
        let anchor = self.visual.unwrap_or(caret);
        let (lo, hi) = if anchor <= caret { (anchor, caret) } else { (caret, anchor) };
        // `getvvcol()`: the end's last cell, the start's first; a position
        // on the line's end is one cell there.
        let hi_line = buf.line(hi.row);
        let hi_start = vcol_of(&hi_line, hi.col);
        let end_vcol = hi_start + hi_line.get(hi.col).map_or(1, |&c| char_width(c, hi_start)) - 1;
        let cols = if hi.row == lo.row { end_vcol + 1 - vcol_of(&buf.line(lo.row), lo.col) } else { end_vcol };
        VisualSize {
            line: lines || self.mode == Mode::Visual(Shape::Line),
            rows: hi.row - lo.row + 1,
            cols,
            to_end: st.want(caret) == Some(Want::End),
        }
    }

    fn visual_op<B: TextBuf>(&mut self, op: VisualOp, count: usize, reg: Option<char>, buf: &mut B, st: &mut BufState) -> Outcome {
        let lines = matches!(op, VisualOp::DeleteLines | VisualOp::YankLines | VisualOp::ChangeLines);
        self.last_visual_size = Some(self.visual_size(lines, buf, st));
        let r = self.visual_range(buf, lines);
        let (first, last) = self.visual_rows(buf);
        self.leave_visual();
        // Vim's `oap->empty`: the charwise selection is only the end of the
        // last line (an empty last line).
        let empty = r.kind == RKind::Char && r.start == r.end;
        let caret = match op {
            VisualOp::Change | VisualOp::ChangeLines => {
                let origin = Cmd::VisualOp { op, count, reg };
                if empty || st.history.emptied(&*buf) {
                    self.change_text(r, origin, buf, st);
                } else {
                    self.change(r, reg, origin, buf, st);
                }
                return Outcome::consumed();
            }
            VisualOp::Put { before } => return self.visual_put(r, before, count, reg, buf, st),
            VisualOp::Delete | VisualOp::DeleteLines => {
                if !self.visual_delete(r, reg, buf, st) {
                    return Outcome::consumed();
                }
                buf.cursor()
            }
            VisualOp::Yank | VisualOp::YankLines => {
                self.regs.yank(reg, yank_of(buf, r));
                r.start
            }
            VisualOp::Replace(ch) => {
                if empty || st.history.emptied(&*buf) {
                    // Vim's `op_replace()` returns before saving anything.
                    r.start
                } else {
                    replace_range(&mut Ed { buf: &mut *buf, hist: &mut st.history }, r, ch)
                }
            }
            VisualOp::Join => {
                let n = (last - first + 1).max(2);
                if !B::MULTILINE || first + n > buf.line_count() {
                    // Vim beeps: the caret still goes to the range start.
                    r.start
                } else {
                    let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
                    ed.hist.begin(r.start);
                    join_rows(&mut ed, first, n)
                }
            }
            VisualOp::Case(how) => {
                let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
                // Vim's `op_tilde()` saves the lines first, so a case change
                // that changes nothing is still an undo step.
                ed.save_rows(r.start, r.start.row, r.end.row);
                recase_range(&mut ed, how, r);
                // `op_tilde()` leaves the caret on `oap->start`, which keeps
                // its column when linewise (`Vk~`, a `.` replay).
                r.start
            }
            VisualOp::Shift { right } => {
                let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
                // Vim's `op_shift()` saves the lines first, as above.
                ed.save_rows(r.start, first, last);
                shift_rows(&mut ed, first, last, right, count.max(1))
            }
        };
        self.land_caret(caret, buf, st);
        Outcome::consumed()
    }

    /// Visual `d` (Vim's `op_delete()` on a Visual area); the caret goes
    /// where the delete leaves it. `false`: nothing to delete in a buffer
    /// with no lines. An empty area writes no register but is still an undo
    /// step (`u_save_cursor()`).
    fn visual_delete<B: TextBuf>(&mut self, r: Range, reg: Option<char>, buf: &mut B, st: &mut BufState) -> bool {
        if st.history.emptied(&*buf) {
            st.forget_want();
            return false;
        }
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        let at = if r.kind == RKind::Char && r.start == r.end {
            ed.save_cursor_line(r.start);
            r.start
        } else {
            // Vim's `u_save` runs with the caret on the range start.
            ed.hist.begin(r.start);
            self.regs.delete(reg, yank_of(ed.buf, r));
            delete(&mut ed, r)
        };
        buf.set_cursor(at);
        true
    }

    /// Visual `p`/`P` (Vim's `nv_put()` in Visual): the selection is deleted,
    /// then the register is put there. `p` deletes into the unnamed
    /// register, so it holds the replaced text afterwards; `P` deletes into
    /// the black hole and changes no register (Vim 9.1). `"0` is never
    /// written, so `yiw` then `viwp` … `viw"0p` keeps putting the yank.
    fn visual_put<B: TextBuf>(&mut self, r: Range, before: bool, count: usize, reg: Option<char>, buf: &mut B, st: &mut BufState) -> Outcome {
        // What to put, read before the delete writes the unnamed register
        // (Vim's `reg1`).
        let text = self.regs.read(reg).clone();
        let del = if before { Some('_') } else { None };
        // In a buffer with no lines (Vim's `ML_EMPTY`) there is nothing to
        // delete, but the put still runs.
        if !self.visual_delete(r, del, buf, st) {
            buf.set_cursor(r.start);
        }
        // Vim's `empty`: every line is gone, so the put leaves a stray empty
        // last line that `nv_put()` deletes.
        let empty = st.history.emptied(&*buf);
        let n = count.max(1);
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        let mut caret = if text.text.is_empty() && r.kind == RKind::Char {
            // An empty register (Vim's `setreg('"', '')`: one empty
            // charwise line) puts nothing between chars; the delete stays.
            ed.buf.cursor()
        } else if r.kind == RKind::Line && !B::MULTILINE {
            // A one-line field: the field's put rules give the text (a
            // linewise register loses its line break), and the caret goes to
            // the first non-blank as Vim's put of lines leaves it.
            ed.buf.set_cursor(Pos::new(0, 0));
            put(&mut ed, &text, true, n);
            ed.put_a_line();
            Pos::new(0, first_non_blank(&ed.buf.line(0)))
        } else if r.kind == RKind::Line {
            // Replacing lines: the register goes in as lines of its own.
            let body = match text.kind {
                RegKind::Line => text.text.repeat(n),
                RegKind::Char => format!("{}\n", text.text).repeat(n),
            };
            put_lines(&mut ed, r.start.row, &body)
        } else {
            let at = r.start;
            let len = ed.buf.line_len(at.row);
            if text.kind == RegKind::Line && B::MULTILINE {
                // Linewise text replacing chars splits the line around it.
                ed.splice(at, at, &format!("\n{}", text.text.repeat(n)));
                Pos::new(at.row + 1, first_non_blank(&ed.buf.line(at.row + 1)))
            } else {
                // The delete reached the line's end: put after its last char.
                let forward = len > 0 && at.col >= len;
                ed.buf.set_cursor(Pos::new(at.row, if forward { len - 1 } else { at.col.min(len) }));
                put(&mut ed, &text, !forward, n)
            }
        };
        let last = ed.buf.line_count() - 1;
        if empty && last > 0 && ed.buf.line_len(last) == 0 {
            let len = ed.buf.line_len(last - 1);
            // `nv_put()` deletes it with `deleted_lines()`: no mark moves.
            ed.splice_moving(Pos::new(last - 1, len), Pos::new(last, 0), "", MarkMove::Keep);
            if caret.row == last {
                // The caret was on that line: the end of the new last line.
                caret = Pos::new(last - 1, len);
            }
        }
        self.land_caret(caret, buf, st);
        Outcome::consumed()
    }
}

/// Visual `r{c}` (Vim's `op_replace()`): every char of the range except
/// line breaks. The lines are saved first, so replacing a char with itself
/// is still an undo step. The caret goes to the range start (column 0 for
/// linewise).
fn replace_range<B: TextBuf>(ed: &mut Ed<'_, B>, r: Range, ch: char) -> Pos {
    let (start, end) = match r.kind {
        RKind::Char => (r.start, r.end),
        RKind::Line => (Pos::new(r.start.row, 0), Pos::new(r.end.row, ed.buf.line_len(r.end.row))),
    };
    ed.save_rows(r.start, start.row, end.row);
    let text: String = ed.buf.slice(start, end).chars().map(|c| if c == '\n' { c } else { ch }).collect();
    ed.splice(start, end, &text);
    start
}
