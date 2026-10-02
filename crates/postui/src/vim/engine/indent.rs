//! Insert `ctrl+t` and `ctrl+d` (spec §3.8, plan 3c): Vim's `ins_shift()`
//! and `change_indent()` (edit.c, indent.c) with `shift_line()` rounding
//! (ops.c) under `expandtab`. The golden file is the judge.

use super::buf::{Pos, TextBuf};
use super::class::white;
use super::history::Ed;
use super::insert::InsertKey;
use super::motion::vcol_of;
use super::settings::{SHIFTWIDTH, TABSTOP};
use super::{BufState, Engine};

/// A line's leading blanks, measured in one pass (a deep indent is then
/// cheap to shift again and again: `9999i<C-t><Esc>`).
struct Lead {
    /// The first non-blank column (the line's length when it is all blank).
    fnb: usize,
    /// Vim's `get_indent_str()`: the blanks in virtual columns (a tab runs
    /// to its stop).
    width: usize,
    /// The spaces the indent starts with, before any tab.
    spaces: usize,
}

fn lead_of(line: &[char]) -> Lead {
    let (mut width, mut tab, mut fnb) = (0, None, line.len());
    for (i, &c) in line.iter().enumerate() {
        match c {
            ' ' => width += 1,
            '\t' => {
                tab.get_or_insert(i);
                width += TABSTOP - width % TABSTOP;
            }
            _ => {
                fnb = i;
                break;
            }
        }
    }
    Lead { fnb, width, spaces: tab.unwrap_or(fnb) }
}

/// Vim's `set_indent(size)` with `expandtab`: the leading blanks (`lead`)
/// become `size` spaces. Returns the new first non-blank column. Only the
/// part that differs is spliced (the spaces both indents start with stay),
/// so a deep indent costs its change, not its width; a splice inside one
/// line moves no mark either way.
fn set_indent<B: TextBuf>(ed: &mut Ed<'_, B>, row: usize, lead: &Lead, size: usize) -> usize {
    let same = lead.spaces.min(size);
    ed.splice(Pos::new(row, same), Pos::new(row, lead.fnb), &" ".repeat(size - same));
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
        Ed { buf: &mut *buf, hist: &mut st.history }.save_cursor_line_once(caret);
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
                let width = lead_of(&buf.line(row)).width;
                self.session().old_indent = Some(width);
            }
        }
        // change_indent(type, 0, round = TRUE)
        let (lead, vcol) = {
            let line = buf.line(row);
            let lead = lead_of(&line);
            // The caret's virtual column matters only inside the indent.
            let vcol = if caret.col < lead.fnb { vcol_of(&line, caret.col) } else { 0 };
            (lead, vcol)
        };
        let fnb = lead.fnb;
        let rel = caret.col as isize - fnb as isize;
        let mut insstart_less = fnb as isize;
        let in_indent_vcol = if rel < 0 { lead.width as isize - vcol as isize } else { 0 };
        // The replace stack can be fixed only when the caret is in the indent.
        let start_col: isize = if rel > 0 { -1 } else { caret.col as isize };
        let new_indent = if zero_all {
            0
        } else {
            // shift_line(left, round = TRUE, amount = 1)
            let count = lead.width;
            let (i, j) = (count / SHIFTWIDTH, count % SHIFTWIDTH);
            let i = if dec {
                // With spaces left over, removing them is the whole step.
                if j > 0 { i } else { i.saturating_sub(1) }
            } else {
                i + 1
            };
            i * SHIFTWIDTH
        };
        let new_fnb = set_indent(&mut Ed { buf: &mut *buf, hist: &mut st.history }, row, &lead, new_indent);
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
