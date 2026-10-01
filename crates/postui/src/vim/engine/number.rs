//! `ctrl+a` / `ctrl+x` in Normal mode (plan 3b): Vim's `do_addsub()` under
//! the pinned `nrformats=bin,hex`, with `vim_str2nr()` reading the number.
//! The number is the one at or after the caret on its line. A decimal
//! number takes a `-` right before it as its sign (even after a letter:
//! `x-1` becomes `x0`); hex and binary ignore it, keep their case and wrap
//! without a sign.

use super::buf::{Pos, TextBuf};
use super::history::Ed;
use super::{BufState, Engine};

/// Vim's `vim_str2nr()` with `STR2NR_BIN | STR2NR_HEX` at the start of `s`:
/// the prefix (`x` `X` `b` `B`, or `None` for decimal), the length in chars
/// (a leading `-` and the prefix included), the value (saturated at
/// `u64::MAX`) and whether it overflowed.
fn str2nr(s: &[char]) -> (Option<char>, usize, u64, bool) {
    let mut i = usize::from(s.first() == Some(&'-'));
    let mut pre = None;
    if s.get(i) == Some(&'0') && !matches!(s.get(i + 1), Some('8' | '9')) {
        match s.get(i + 1).copied() {
            Some(p @ ('x' | 'X')) if s.get(i + 2).is_some_and(|c| c.is_ascii_hexdigit()) => pre = Some(p),
            Some(p @ ('b' | 'B')) if matches!(s.get(i + 2), Some('0' | '1')) => pre = Some(p),
            _ => {}
        }
    }
    if pre.is_some() {
        i += 2;
    }
    let radix = match pre {
        Some('x' | 'X') => 16,
        Some(_) => 2,
        None => 10,
    };
    let (mut n, mut overflow) = (0u64, false);
    while let Some(d) = s.get(i).and_then(|c| c.to_digit(radix)) {
        match n.checked_mul(u64::from(radix)).and_then(|v| v.checked_add(u64::from(d))) {
            Some(v) => n = v,
            None => {
                n = u64::MAX;
                overflow = true;
            }
        }
        i += 1;
    }
    (pre, i, n, overflow)
}

/// What `ctrl+a` (`subtract` false) or `ctrl+x` does to `line` with the
/// caret at `caret` (Vim's `do_addsub()` in Normal mode): the chars
/// `start..end` it replaces and the new number, or `None` when there is no
/// number at or after the caret.
pub(crate) fn add_sub(line: &[char], caret: usize, subtract: bool, amount: u64) -> Option<(usize, usize, String)> {
    if caret >= line.len() {
        return None;
    }
    let at = |i: usize| line.get(i).copied();
    let digit = |i: usize| at(i).is_some_and(|c| c.is_ascii_digit());
    let xdigit = |i: usize| at(i).is_some_and(|c| c.is_ascii_hexdigit());
    let bdigit = |i: usize| matches!(at(i), Some('0' | '1'));
    // `0x`/`0b` with `col` on the `x`/`b` and a digit after it.
    let hex = |col: usize| col > 0 && matches!(at(col), Some('x' | 'X')) && at(col - 1) == Some('0') && xdigit(col + 1);
    let bin = |col: usize| col > 0 && matches!(at(col), Some('b' | 'B')) && at(col - 1) == Some('0') && bdigit(col + 1);
    // On a hex or binary number, after its `0x`/`0b`: back over its digits.
    let mut col = caret;
    while col > 0 && bdigit(col) {
        col -= 1;
    }
    while col > 0 && xdigit(col) {
        col -= 1;
    }
    if !hex(col) {
        // The binary and hex scans overlap: rescan over decimal digits.
        col = caret;
        while col > 0 && digit(col) {
            col -= 1;
        }
    }
    if hex(col) || bin(col) {
        col -= 1;
    } else {
        // Forward to a decimal digit, then back to its number's start.
        col = caret;
        while col < line.len() && !digit(col) {
            col += 1;
        }
        while col > 0 && digit(col - 1) {
            col -= 1;
        }
    }
    let first_digit = at(col).filter(char::is_ascii_digit)?;
    let mut negative = false;
    if col > 0 && at(col - 1) == Some('-') {
        col -= 1;
        negative = true;
    }
    let (pre, mut len, mut n, overflow) = str2nr(&line[col..]);
    // A `-` before a hex or binary number is not its sign.
    if pre.is_some() && negative {
        col += 1;
        len -= 1;
        negative = false;
    }
    let subtract = subtract ^ negative;
    let old = n;
    if !overflow {
        n = if subtract { n.wrapping_sub(amount) } else { n.wrapping_add(amount) };
    }
    // A decimal number that wrapped past zero changes sign.
    if pre.is_none() {
        if subtract {
            if n > old {
                n = 1u64.wrapping_add(n ^ u64::MAX);
                negative = !negative;
            }
        } else if n < old {
            n ^= u64::MAX;
            negative = !negative;
        }
        if n == 0 {
            negative = false;
        }
    }
    let old_text = &line[col..col + len];
    // The width kept is the number's without its `-`; hex keeps the case of
    // the old number's last letter (Vim's `hexupper`).
    let mut width = len as isize - isize::from(old_text[0] == '-');
    let upper = old_text.iter().rev().find(|c| c.is_ascii_alphabetic()).is_some_and(char::is_ascii_uppercase);
    let mut new = String::new();
    if negative {
        new.push('-');
    }
    if let Some(p) = pre {
        new.push('0');
        new.push(p);
        width -= 2;
    }
    let digits = match pre {
        Some('b' | 'B') => format!("{n:b}"),
        Some(_) if upper => format!("{n:X}"),
        Some(_) => format!("{n:x}"),
        None => n.to_string(),
    };
    // A number that started with 0 keeps its width with leading zeros.
    if first_digit == '0' {
        for _ in 0..(width - digits.len() as isize).max(0) {
            new.push('0');
        }
    }
    new += &digits;
    Some((col, col + len, new))
}

impl Engine {
    /// `ctrl+a` / `ctrl+x` with a count (Vim's `op_addsub()`). The line is
    /// saved first, so a line with no number is still an undo step. On a
    /// number the caret lands on its last char and the wanted column
    /// resets; with no number nothing moves and the wanted column stays.
    pub(super) fn exec_addsub<B: TextBuf>(&mut self, add: bool, count: usize, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        ed.save_cursor_line(caret);
        let line = ed.buf.line(caret.row).into_owned();
        let Some((start, end, text)) = add_sub(&line, caret.col, !add, count.max(1) as u64) else {
            return;
        };
        ed.splice(Pos::new(caret.row, start), Pos::new(caret.row, end), &text);
        self.land_caret(Pos::new(caret.row, start + text.chars().count() - 1), buf, st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(line: &str, caret: usize, subtract: bool, amount: u64) -> Option<String> {
        let chars: Vec<char> = line.chars().collect();
        add_sub(&chars, caret, subtract, amount).map(|(start, end, text)| {
            let mut out: String = chars[..start].iter().collect();
            out += &text;
            out.extend(&chars[end..]);
            out
        })
    }

    /// Each row was probed on Vim 9.1 (plan 3b, batch p2).
    #[test]
    fn numbers_change_as_vim_changes_them() {
        for (line, caret, subtract, amount, want) in [
            ("id: 41, n: -7", 0, false, 1, "id: 42, n: -7"),
            ("n: -7", 3, false, 7, "n: 0"),
            ("pad: 007", 0, false, 1, "pad: 008"),
            ("pad: 007", 0, true, 8, "pad: -001"),
            ("pad: 007", 0, false, 993, "pad: 1000"),
            ("0x1f", 2, false, 1, "0x20"),
            ("0XfF", 0, false, 1, "0X100"),
            ("0x0F", 0, true, 1, "0x0E"),
            ("-0x10", 0, false, 1, "-0x11"),
            ("x-1", 0, false, 1, "x0"),
            ("--2", 1, false, 3, "-1"),
            ("-0", 0, false, 1, "1"),
            ("0", 0, true, 1, "-1"),
            ("0b11111111", 0, false, 1, "0b100000000"),
            ("0x0", 0, true, 1, "0xffffffffffffffff"),
            ("0xg", 0, false, 1, "1xg"),
            ("0b2", 0, false, 1, "1b2"),
            ("é5", 0, false, 1, "é6"),
            ("18446744073709551615", 0, false, 1, "-18446744073709551615"),
            ("18446744073709551616", 0, false, 1, "18446744073709551615"),
            ("-18446744073709551615", 0, true, 1, "18446744073709551615"),
        ] {
            assert_eq!(apply(line, caret, subtract, amount).as_deref(), Some(want), "{line:?} @{caret}");
        }
        assert_eq!(apply("abc", 0, false, 1), None);
        assert_eq!(apply("12 ab", 3, false, 1), None, "no number at or after the caret");
        assert_eq!(apply("", 0, false, 1), None);
    }
}
