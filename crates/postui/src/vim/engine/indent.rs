//! Insert `ctrl+t` and `ctrl+d` (spec §3.8, plan 3c): Vim's `ins_shift()`
//! and `change_indent()` (edit.c, indent.c) with `shift_line()` rounding
//! (ops.c) under `expandtab`. The golden file is the judge.

use super::buf::{Pos, TextBuf};
use super::class::white;
use super::history::Ed;
use super::insert::InsertKey;
use super::motion::vcol_of;
use super::settings::SHIFTWIDTH;
use super::{BufState, Engine, first_non_blank};

/// Vim's `get_indent_str()`: the leading blanks in virtual columns (a tab
/// runs to its stop).
pub(crate) fn indent_width(line: &[char]) -> usize {
    vcol_of(line, first_non_blank(line))
}

/// Vim's `set_indent(size)` with `expandtab`: the leading blanks become
/// `size` spaces. Returns the new first non-blank column.
fn set_indent<B: TextBuf>(ed: &mut Ed<'_, B>, row: usize, size: usize) -> usize {
    let lead = first_non_blank(&ed.buf.line(row));
    ed.splice(Pos::new(row, 0), Pos::new(row, lead), &" ".repeat(size));
    size
}

impl Engine {
    /// Vim's `ins_shift()`: `ctrl+t` (`dec` false) or `ctrl+d`. The line is
    /// saved first (`stop_arrow()` → `u_save_cursor()`), so an unchanged
    /// indent is still an undo step. `0<C-d>` and `^<C-d>` delete the char
    /// just typed and set the indent to 0; `^` remembers the indent for the
    /// next Enter (`old_indent`).
    pub(super) fn ins_shift<B: TextBuf>(&mut self, dec: bool, buf: &mut B, st: &mut BufState) {
        let mut caret = buf.cursor();
        let row = caret.row;
        let last = self.session().last_key.clone();
        let zero_all = dec && caret.col > 0 && matches!(last, Some(InsertKey::Char('0' | '^')));
        Ed { buf: &mut *buf, hist: &mut st.history }.save_cursor_line(caret);
        if zero_all {
            let at = Pos::new(row, caret.col - 1);
            Ed { buf: &mut *buf, hist: &mut st.history }.splice(at, caret, "");
            caret = at;
            buf.set_cursor(caret);
            if self.session().replace.is_some() {
                // The '0' or '^' gives back what it covered (`replace_pop_ins()`).
                self.replace_restore_at(buf, st);
            }
            if matches!(last, Some(InsertKey::Char('^'))) {
                let width = indent_width(&buf.line(row));
                self.session().old_indent = Some(width);
            }
        }
        // change_indent(type, 0, round = TRUE)
        let line = buf.line(row).into_owned();
        let vcol = vcol_of(&line, caret.col);
        let fnb = first_non_blank(&line);
        let rel = caret.col as isize - fnb as isize;
        let mut insstart_less = fnb as isize;
        let in_indent_vcol = if rel < 0 { indent_width(&line) as isize - vcol as isize } else { 0 };
        // The replace stack can be fixed only when the caret is in the indent.
        let start_col: isize = if rel > 0 { -1 } else { caret.col as isize };
        let new_indent = if zero_all {
            0
        } else {
            // shift_line(left, round = TRUE, amount = 1)
            let count = indent_width(&line);
            let (i, j) = (count / SHIFTWIDTH, count % SHIFTWIDTH);
            let i = if dec {
                // With spaces left over, removing them is the whole step.
                if j > 0 { i } else { i.saturating_sub(1) }
            } else {
                i + 1
            };
            i * SHIFTWIDTH
        };
        let new_fnb = set_indent(&mut Ed { buf: &mut *buf, hist: &mut st.history }, row, new_indent);
        insstart_less -= new_fnb as isize;
        let new_col: isize = if rel >= 0 {
            if rel == 0 {
                insstart_less = isize::MAX;
            }
            rel + new_fnb as isize
        } else {
            // In the indent: the same screen distance from the indent's end,
            // which is all spaces now.
            insstart_less = isize::MAX;
            (new_indent as isize - in_indent_vcol).max(0)
        };
        caret.col = new_col.max(0) as usize;
        buf.set_cursor(caret);
        st.forget_want();
        let s = self.session();
        if caret.row == s.start.row && s.start.col != 0 {
            s.start.col = if (s.start.col as isize) <= insstart_less { 0 } else { (s.start.col as isize - insstart_less) as usize };
        }
        if let Some(stack) = s.replace.as_mut()
            && start_col >= 0
        {
            let mut have = start_col;
            while have > caret.col as isize {
                // `replace_join(0)`: the topmost marker goes.
                if let Some(i) = stack.iter().rposition(Option::is_none) {
                    stack.remove(i);
                }
                have -= 1;
            }
            while have < caret.col as isize {
                stack.push(None);
                have += 1;
            }
        }
        // An autoindent stays one only while the line has nothing else.
        if s.ai_row == Some(row) && buf.line(row).iter().any(|&c| !white(c)) {
            s.ai_row = None;
        }
    }
}
