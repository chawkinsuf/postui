//! The mouse's words, as Vim 9.1's `do_mouse()` takes them (probed with
//! `'mouse'` `a`): what a double click selects and a drag after it grows
//! by. Both profiles use them (spec 2026-10-14, align the text mouse).

use super::brackets::{MatchFrom, find_match};
use super::class::{class, is_word, white};
use super::{Lines, Pos};

/// Vim's `get_mouse_class()`: what a double click takes as one word. Past
/// ASCII it is [`class`]. In ASCII a blank (or the line's end) is 0, a
/// word char 2, the operator chars `-+*/%<>&|^!=` 1, so `->` and `!=` go
/// together, and any other char is a class of its own.
fn mouse_class(c: Option<char>) -> u32 {
    match c {
        None => 0,
        Some(c) if !c.is_ascii() => class(c),
        Some(c) if white(c) => 0,
        Some(c) if is_word(c) => 2,
        Some(c) if "-+*/%<>&|^!=".contains(c) => 1,
        Some(c) => u32::from(c),
    }
}

/// Vim's `find_start_of_word()`.
pub(crate) fn word_start<B: Lines + ?Sized>(buf: &B, at: Pos) -> Pos {
    let line = buf.line(at.row);
    let k = mouse_class(line.get(at.col).copied());
    let mut col = at.col.min(line.len());
    while col > 0 && mouse_class(line.get(col - 1).copied()) == k {
        col -= 1;
    }
    Pos::new(at.row, col)
}

/// Vim's `find_end_of_word()` (`'selection'` inclusive): the word's last
/// char, or the line's end after a blank run that reaches it.
pub(crate) fn word_end<B: Lines + ?Sized>(buf: &B, at: Pos) -> Pos {
    let line = buf.line(at.row);
    let k = mouse_class(line.get(at.col).copied());
    let mut col = at.col;
    while col < line.len() && mouse_class(line.get(col + 1).copied()) == k {
        col += 1;
    }
    Pos::new(at.row, col)
}

/// Where a double click's block ends: on a char that is not a word char
/// (blanks skipped), the `%` match from the pointer, if any.
pub(crate) fn block_end<B: Lines + ?Sized>(buf: &B, at: Pos) -> Option<Pos> {
    let line = buf.line(at.row);
    let first = line.iter().skip(at.col).find(|&&c| !white(c));
    if first.is_some_and(|&c| is_word(c)) {
        return None;
    }
    find_match(buf, at, MatchFrom::Percent, true)
}
