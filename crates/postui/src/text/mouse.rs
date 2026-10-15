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

/// The word a double click at `col` of `line` selects, as an inclusive
/// `(start, end)`: [`word_start`] to [`word_end`], with a press past the
/// end counting as on the last char, as Vim's Normal caret clamps there.
/// The end stays on a char (a blank run that reaches the line's end stops
/// on its last blank). `None` on an empty line.
pub(crate) fn word_at(line: &[char], col: usize) -> Option<(usize, usize)> {
    let last = line.len().checked_sub(1)?;
    let at = Pos::new(0, col.min(last));
    Some((word_start(line, at).col, word_end(line, at).col.min(last)))
}

/// Where Vim's double click's block ends: on a char that is not a word
/// char (blanks skipped), the `%` match from the pointer, if any. That is
/// any punctuation with a bracket later on its line (probed: `:` in
/// `https://x.com/{{id}}` selects to the last `}`).
pub(crate) fn block_end<B: Lines + ?Sized>(buf: &B, at: Pos) -> Option<Pos> {
    let line = buf.line(at.row);
    let first = line.iter().skip(at.col).find(|&&c| !white(c));
    if first.is_some_and(|&c| is_word(c)) {
        return None;
    }
    find_match(buf, at, MatchFrom::Percent, true)
}

/// Where arrows' double click's block ends: only on a bracket, or on
/// blanks before one, its match (the user, 2026-10-14: Vim's reach to a
/// bracket later on the line stays vim's).
pub(crate) fn bracket_block_end<B: Lines + ?Sized>(buf: &B, at: Pos) -> Option<Pos> {
    let line = buf.line(at.row);
    let first = line.iter().skip(at.col).find(|&&c| !white(c));
    if !first.is_some_and(|c| "()[]{}".contains(*c)) {
        return None;
    }
    find_match(buf, at, MatchFrom::Percent, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(s: &str, col: usize) -> Option<(usize, usize)> {
        word_at(&s.chars().collect::<Vec<_>>(), col)
    }

    /// The probed Vim words (spec 2026-10-14, the vim mouse, R4), as both
    /// profiles' double click takes them.
    #[test]
    fn a_double_clicks_word_is_vims_mouse_word() {
        let line = "foo bar_2.baz qux->x  end";
        assert_eq!(word(line, 5), Some((4, 8)), "word chars, `_` and digits included");
        assert_eq!(word(line, 9), Some((9, 9)), "`.` stands alone");
        assert_eq!(word(line, 18), Some((17, 18)), "`->` goes together");
        assert_eq!(word(line, 21), Some((20, 21)), "a blank run");
        assert_eq!(word("https://x", 5), Some((5, 5)), "`:` alone");
        assert_eq!(word("https://x", 6), Some((6, 7)), "`//` together");
    }

    #[test]
    fn past_the_end_is_the_last_word_and_an_empty_line_none() {
        assert_eq!(word("hello world", 40), Some((6, 10)));
        assert_eq!(word("ab   ", 3), Some((2, 4)), "a blank run to the end stops on its last");
        assert_eq!(word("", 0), None);
    }

    #[test]
    fn a_bracket_ends_at_its_match_and_a_word_has_no_block() {
        let line: Vec<char> = "a {b c} d".chars().collect();
        assert_eq!(block_end(&line[..], Pos::new(0, 2)), Some(Pos::new(0, 6)));
        assert_eq!(block_end(&line[..], Pos::new(0, 1)), Some(Pos::new(0, 6)), "blanks first");
        assert_eq!(block_end(&line[..], Pos::new(0, 3)), None, "a word char");
        let url: Vec<char> = "https://x.com/{{id}}".chars().collect();
        assert_eq!(block_end(&url[..], Pos::new(0, 5)), Some(Pos::new(0, 19)), "Vim's reach");
        assert_eq!(bracket_block_end(&url[..], Pos::new(0, 5)), None, "arrows': brackets only");
        assert_eq!(bracket_block_end(&line[..], Pos::new(0, 1)), Some(Pos::new(0, 6)));
    }
}
