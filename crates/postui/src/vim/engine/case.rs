//! Vim's case mapping for `~`, `g~`, `gu`, `gU` and Visual `~ u U` (plan
//! 3b): `utf_toupper()`/`utf_tolower()` under `casemap=internal,keepascii`,
//! as a table generated from Vim itself (`case_table.rs`), with
//! `swapchar()`'s rules on top. Rust's `char` case functions disagree with
//! Vim at the edges (`ǅ`, `İ`, `ß`), so the engine never uses them.

use super::case_table::{LOWER, UPPER};
use super::keys::CaseOp;

fn map(table: &[(u32, u32, u32, i32)], c: char) -> char {
    let n = c as u32;
    let i = table.partition_point(|&(first, _, _, _)| first <= n);
    if i == 0 {
        return c;
    }
    let (first, last, step, delta) = table[i - 1];
    if n > last || !(n - first).is_multiple_of(step) {
        return c;
    }
    char::from_u32((i64::from(n) + i64::from(delta)) as u32).unwrap_or(c)
}

/// Vim's `utf_toupper()`.
fn upper(c: char) -> char {
    map(UPPER, c)
}

/// Vim's `utf_tolower()`.
fn lower(c: char) -> char {
    map(LOWER, c)
}

/// Vim's `utf_islower()`: `ß` is lower case with no upper-case form.
fn is_lower(c: char) -> bool {
    upper(c) != c || c == 'ß'
}

/// Vim's `utf_isupper()`.
fn is_upper(c: char) -> bool {
    lower(c) != c
}

/// Vim's `swapchar()`: what `how` makes of `c`, pushed onto `out`. Under
/// `gU` (and Visual `U`) `ß` becomes "SS", as Vim does for a Latin-1-like
/// encoding, UTF-8 included; every other result is one char. A char that
/// is both lower and upper case (`ǅ`) counts as lower case, so `gu` leaves
/// it.
pub(crate) fn swap(how: CaseOp, c: char, out: &mut String) {
    if how == CaseOp::Upper && c == 'ß' {
        out.push_str("SS");
        return;
    }
    let nc = if is_lower(c) {
        if how == CaseOp::Lower { c } else { upper(c) }
    } else if is_upper(c) {
        if how == CaseOp::Upper { c } else { lower(c) }
    } else {
        c
    };
    out.push(nc);
}

/// Vim's `oap->end` as `op_tilde()` sees it: a row, a BYTE column and
/// whether the motion was inclusive (an exclusive end is stepped back first).
#[derive(Debug, Clone, Copy)]
pub(crate) struct End {
    pub row: usize,
    pub col: usize,
    pub inclusive: bool,
}

/// UTF-8 bytes in `chars`.
pub(crate) fn bytes(chars: &[char]) -> usize {
    chars.iter().map(|c| c.len_utf8()).sum()
}

/// The char of `line` that holds byte `off` (Vim's `mb_adjust_cursor()`:
/// a byte inside a char belongs to it); `line.len()` past the end.
pub(crate) fn char_at_byte(line: &[char], off: usize) -> usize {
    let mut at = 0;
    for (i, c) in line.iter().enumerate() {
        at += c.len_utf8();
        if at > off {
            return i;
        }
    }
    line.len()
}

/// Vim's `swapchars()` loop over the rows `first..` of a buffer of `total`
/// rows, positions as (row, char index). Vim counts BYTES: the lengths it
/// walks, `oap->end.col` and the cursor are byte offsets, and the text
/// changes under the walk (`İ` and `ı` shrink to one byte), so a walk can
/// stop short of, or run past, the chars the range holds. The corpus pins
/// both (`g~e` over `İİ ǈ x` toggles all of it from the line above).
struct Walk<'a> {
    lines: &'a mut [Vec<char>],
    first: usize,
    total: usize,
    how: CaseOp,
}

type At = (usize, usize);

impl Walk<'_> {
    fn line(&self, row: usize) -> Option<&Vec<char>> {
        row.checked_sub(self.first).and_then(|i| self.lines.get(i))
    }

    fn byte_col(&self, p: At) -> usize {
        self.line(p.0).map_or(0, |l| bytes(&l[..p.1.min(l.len())]))
    }

    /// Vim's `inc()`: 0 to the next char, 2 onto the line's end, 1 to the
    /// next line, -1 at the end of the buffer.
    fn inc(&self, p: &mut At) -> i32 {
        let len = self.line(p.0).map_or(0, |l| l.len());
        if p.1 < len {
            p.1 += 1;
            return if p.1 < len { 0 } else { 2 };
        }
        if p.0 + 1 < self.total {
            *p = (p.0 + 1, 0);
            return 1;
        }
        -1
    }

    /// Vim's `dec()`: `end` one char back, or onto the previous line's end;
    /// -1 at the start of the buffer.
    fn dec(&self, end: &mut End) -> i32 {
        if end.col > 0 {
            if let Some(l) = self.line(end.row) {
                end.col = bytes(&l[..char_at_byte(l, end.col - 1)]);
            } else {
                end.col -= 1;
            }
            return 0;
        }
        if end.row > 0 {
            end.row -= 1;
            end.col = self.line(end.row).map_or(0, |l| bytes(l));
            return 1;
        }
        -1
    }

    /// Vim's `swapchar()`; `ß` under `gU` becomes two chars and the walk
    /// steps over the second.
    fn swapchar(&mut self, p: &mut At) {
        let (first, how) = (self.first, self.how);
        let Some(line) = p.0.checked_sub(first).and_then(|i| self.lines.get_mut(i)) else { return };
        let Some(&c) = line.get(p.1) else { return };
        let mut out = String::new();
        swap(how, c, &mut out);
        let mut chars = out.chars();
        line[p.1] = chars.next().unwrap_or(c);
        if let Some(second) = chars.next() {
            line.insert(p.1 + 1, second);
            p.1 += 1;
        }
    }

    /// Vim's `swapchars()`: `length` BYTES (counted by the chars' lengths
    /// before they change) from `p`, which it leaves after the last char.
    fn swapchars(&mut self, p: &mut At, length: i64) {
        let mut todo = length;
        while todo > 0 {
            if let Some(&c) = self.line(p.0).and_then(|l| l.get(p.1)) {
                todo -= c.len_utf8() as i64 - 1;
            }
            self.swapchar(p);
            if self.inc(p) == -1 {
                break;
            }
            todo -= 1;
        }
    }
}

/// Vim's `op_tilde()` for `g~ gu gU` over a charwise or linewise range, on
/// `lines` (the buffer's rows from `first`, one past the range's end at
/// most, `total` rows in all). `start` is the range start in chars; the
/// result is where the caret lands: the start's BYTE offset in the new text
/// (Vim never moves the cursor), which is a different char when the chars
/// before it changed length in chars (`ß` to "SS").
pub(crate) fn op_tilde(lines: &mut [Vec<char>], first: usize, total: usize, how: CaseOp, start: At, mut end: End, linewise: bool) -> At {
    let caret_row = start.0;
    let caret_off = lines.get(caret_row - first).map_or(0, |l| bytes(&l[..start.1.min(l.len())]));
    let mut w = Walk { lines, first, total, how };
    let mut pos = if linewise { (start.0, 0) } else { start };
    if linewise {
        end.col = w.line(end.row).map_or(0, |l| bytes(l)).saturating_sub(1);
    } else if !end.inclusive {
        w.dec(&mut end);
    }
    if pos.0 == end.row {
        let from = w.byte_col(pos) as i64;
        w.swapchars(&mut pos, end.col as i64 - from + 1);
    } else {
        loop {
            let length = if pos.0 == end.row {
                end.col as i64 + 1
            } else {
                w.line(pos.0).map_or(0, |l| bytes(&l[pos.1.min(l.len())..])) as i64
            };
            if w.line(pos.0).is_none() {
                break;
            }
            w.swapchars(&mut pos, length);
            let reached = end.row < pos.0 || (end.row == pos.0 && end.col <= w.byte_col(pos));
            if reached || w.inc(&mut pos) == -1 {
                break;
            }
        }
    }
    (caret_row, w.line(caret_row).map_or(0, |l| char_at_byte(l, caret_off)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<Vec<char>> {
        lines.iter().map(|l| l.chars().collect()).collect()
    }

    fn text(lines: &[Vec<char>]) -> Vec<String> {
        lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Probed on Vim 9.1: `g~e` from the line above `İİ ǈ x`. The two `İ`
    /// shrink to one byte each, so the walk stops with the cursor before
    /// `end.col` and goes on from the next char.
    #[test]
    fn a_shrinking_walk_runs_past_the_end_of_a_multi_line_range() {
        let mut l = rows(&["a", "İİ ǈ x"]);
        let end = End { row: 1, col: 3, inclusive: true };
        op_tilde(&mut l, 0, 2, CaseOp::Toggle, (0, 0), end, false);
        assert_eq!(text(&l), ["A", "ii Ǉ X"]);
        // One shrinking char stops exactly at the end.
        let mut l = rows(&["a", "İ ǈ x"]);
        let end = End { row: 1, col: 1, inclusive: true };
        op_tilde(&mut l, 0, 2, CaseOp::Toggle, (0, 0), end, false);
        assert_eq!(text(&l), ["A", "i ǈ x"]);
    }

    /// Probed on Vim 9.1: `gu0` in column 0 of a later line is an empty
    /// exclusive range, which `dec()` turns into the previous line's end,
    /// so the whole line changes; on the first line it changes one char.
    #[test]
    fn an_empty_exclusive_range_in_column_zero() {
        let mut l = rows(&["", "İı ǈ"]);
        let end = End { row: 1, col: 0, inclusive: false };
        op_tilde(&mut l, 0, 2, CaseOp::Lower, (1, 0), end, false);
        assert_eq!(text(&l), ["", "iı ǈ"]);
        let mut l = rows(&["ABC"]);
        let end = End { row: 0, col: 0, inclusive: false };
        op_tilde(&mut l, 0, 1, CaseOp::Lower, (0, 0), end, false);
        assert_eq!(text(&l), ["aBC"]);
        let mut l = rows(&["ABC"]);
        let end = End { row: 0, col: 2, inclusive: false };
        op_tilde(&mut l, 0, 1, CaseOp::Lower, (0, 2), end, false);
        assert_eq!(text(&l), ["ABC"]);
    }

    /// The caret stays on the same byte, not the same char.
    #[test]
    fn the_caret_keeps_its_byte_offset() {
        let mut l = rows(&["ß ǅ", "  straße"]);
        let end = End { row: 1, col: 0, inclusive: false };
        let at = op_tilde(&mut l, 0, 2, CaseOp::Upper, (0, 2), end, true);
        assert_eq!(text(&l), ["SS Ǆ", "  STRASSE"]);
        assert_eq!(at, (0, 3), "byte 3 is the Ǆ now");
    }

    fn swapped(how: CaseOp, s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            swap(how, c, &mut out);
        }
        out
    }

    /// Probed on Vim 9.1 (plan 3b, batches p1 and p6): `g~~`, `guu` and
    /// `gUU` on this line.
    #[test]
    fn vims_case_rules_at_the_edges() {
        let line = "ß ǅ İ ſ ǆ Ǆ ﬁ Σς ǈ ÿ Ÿ";
        assert_eq!(swapped(CaseOp::Toggle, line), "ß Ǆ i S Ǆ ǆ ﬁ σΣ Ǉ Ÿ ÿ");
        assert_eq!(swapped(CaseOp::Lower, line), "ß ǅ i ſ ǆ ǆ ﬁ σς ǈ ÿ ÿ");
        assert_eq!(swapped(CaseOp::Upper, line), "SS Ǆ İ S Ǆ Ǆ ﬁ ΣΣ Ǉ Ÿ Ÿ");
        assert_eq!(swapped(CaseOp::Toggle, "aZ-1"), "Az-1");
    }

    #[test]
    fn the_tables_are_sorted_disjoint_runs() {
        for table in [UPPER, LOWER] {
            for &(first, last, step, _) in table {
                assert!(first <= last && (step == 1 || step == 2), "({first:#x}, {last:#x}, {step})");
            }
            for w in table.windows(2) {
                assert!(w[0].1 < w[1].0, "{:?} overlaps {:?}", w[0], w[1]);
            }
        }
    }
}
