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

/// Where a double click's block ends: the `%` match from the pointer, if
/// the first char from it that is not a blank starts one. With
/// `vim_reach`, as Vim: any char that is not a word char does, so any
/// punctuation with a bracket later on its line (probed: `:` in
/// `https://x.com/{{id}}` selects to the last `}`). Without it, for
/// arrows, only a bracket (the user, 2026-10-14: that reach stays vim's).
pub(crate) fn block_end<B: Lines + ?Sized>(buf: &B, at: Pos, vim_reach: bool) -> Option<Pos> {
    let line = buf.line(at.row);
    let first = line.iter().skip(at.col).find(|&&c| !white(c));
    let starts = if vim_reach {
        !first.is_some_and(|&c| is_word(c))
    } else {
        first.is_some_and(|c| "()[]{}".contains(*c))
    };
    if !starts {
        return None;
    }
    find_match(buf, at, MatchFrom::Percent, true)
}

/// A span a double or triple click selected: the chars `fixed` to `moving`
/// inclusive, in either order, the caret riding `moving`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) fixed: Pos,
    pub(crate) moving: Pos,
}

/// What an arrows double click at `at` selects (spec 2026-10-14 A1–A3): a
/// bracket's block, from the press (blanks before an opener included) or
/// from a closer to its match, else the shared word. A press past the
/// line's end counts as on its last char. `None` on an empty line.
pub(crate) fn double_click<B: Lines + ?Sized>(buf: &B, at: Pos) -> Option<Span> {
    let line = buf.line(at.row);
    let at = Pos::new(at.row, at.col.min(line.len().checked_sub(1)?));
    Some(match block_end(buf, at, false) {
        // From blanks before a closer, the closer is the fixed end, so the
        // block comes whole (Vim's stops at the press, the closer left out).
        Some(end) if end < at => {
            let closer = (at.col..line.len()).find(|&c| !white(line[c])).unwrap_or(at.col);
            Span { fixed: Pos::new(at.row, closer), moving: end }
        }
        Some(end) => Span { fixed: at, moving: end },
        None => {
            let (s, e) = word_at(&line, at.col)?;
            Span { fixed: Pos::new(at.row, s), moving: Pos::new(at.row, e) }
        }
    })
}

/// Where an arrows drag after a double or triple click takes its `span`
/// (A4, A6 in a one-line field): while the pointer is inside it, the span as
/// clicked, its direction kept; past either end, grown by the shared words
/// to the pointer's, the far end fixed (the user, 2026-10-14: arrows keeps
/// a bracket's block, where Vim's drag regrows from the bracket).
pub(crate) fn grow_by_words<B: Lines + ?Sized>(buf: &B, span: Span, at: Pos) -> Span {
    let (start, end) = (span.fixed.min(span.moving), span.fixed.max(span.moving));
    let word = word_at(&buf.line(at.row), at.col);
    if at < start {
        Span { fixed: end, moving: Pos::new(at.row, word.map_or(at.col, |w| w.0)) }
    } else if at > end {
        Span { fixed: start, moving: Pos::new(at.row, word.map_or(at.col, |w| w.1)) }
    } else {
        span
    }
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
        assert_eq!(block_end(&line[..], Pos::new(0, 2), true), Some(Pos::new(0, 6)));
        assert_eq!(block_end(&line[..], Pos::new(0, 1), true), Some(Pos::new(0, 6)), "blanks");
        assert_eq!(block_end(&line[..], Pos::new(0, 3), true), None, "a word char");
        let url: Vec<char> = "https://x.com/{{id}}".chars().collect();
        assert_eq!(block_end(&url[..], Pos::new(0, 5), true), Some(Pos::new(0, 19)), "Vim's reach");
        assert_eq!(block_end(&url[..], Pos::new(0, 5), false), None, "arrows': brackets only");
        assert_eq!(block_end(&line[..], Pos::new(0, 1), false), Some(Pos::new(0, 6)));
    }

    #[test]
    fn a_double_click_is_a_block_from_the_press_or_the_word() {
        let line: Vec<char> = "a {b c} d".chars().collect();
        let p = |c| Pos::new(0, c);
        assert_eq!(
            double_click(&line[..], p(6)),
            Some(Span { fixed: p(6), moving: p(2) }),
            "on the closing brace: from it back to its opener"
        );
        assert_eq!(double_click(&line[..], p(8)), Some(Span { fixed: p(8), moving: p(8) }));
        assert_eq!(double_click(&[][..], p(0)), None);
    }

    #[test]
    fn a_drag_keeps_the_span_inside_and_grows_by_words_outside() {
        let line: Vec<char> = "x{{id}} yz".chars().collect();
        let p = |c| Pos::new(0, c);
        let block = Span { fixed: p(5), moving: p(2) };
        assert_eq!(grow_by_words(&line[..], block, p(4)), block, "inside: as clicked");
        assert_eq!(grow_by_words(&line[..], block, p(5)), block, "on its closer");
        assert_eq!(
            grow_by_words(&line[..], block, p(9)),
            Span { fixed: p(2), moving: p(9) },
            "past it: to the end of the pointer's word"
        );
        assert_eq!(
            grow_by_words(&line[..], block, p(0)),
            Span { fixed: p(5), moving: p(0) },
            "before it: to the start of the pointer's word"
        );
    }
}
