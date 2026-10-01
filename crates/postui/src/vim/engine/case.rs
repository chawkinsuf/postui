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

#[cfg(test)]
mod tests {
    use super::*;

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
