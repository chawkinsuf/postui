//! The mouse's words, as Vim 9.1's `do_mouse()` takes them (probed with
//! `'mouse'` `a`): what a double click selects and a drag after it grows
//! by. Both profiles use them (spec 2026-10-14, align the text mouse). The
//! block Vim's double click takes from any punctuation stays vim's, in
//! `vim/engine/mouse.rs`; arrows' block is [`double_click`]'s.

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

/// A span a double or triple click selected: the chars `fixed` to `moving`
/// inclusive, in either order, the caret riding `moving`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) fixed: Pos,
    pub(crate) moving: Pos,
}

impl Span {
    /// The GUI selection the span shows as, half-open: `(anchor, caret)`,
    /// the caret on the moving end (after its char going forward, on it
    /// going back) and the anchor on the fixed end's outer edge. `after` is
    /// the edge after a char, which a text may clamp (an empty row's only
    /// edge is its start).
    pub(crate) fn gui_ends(self, after: impl Fn(Pos) -> Pos) -> (Pos, Pos) {
        if self.moving < self.fixed {
            (after(self.fixed), self.moving)
        } else {
            (self.fixed, after(self.moving))
        }
    }
}

/// What an arrows double click at `at` selects (spec 2026-10-14 A1–A3): a
/// bracket's block, from the press (blanks before an opener included) or
/// from a closer to its match, else the shared word. A press past the
/// line's end takes the last word, never a block. `None` on an empty line.
pub(crate) fn double_click<B: Lines + ?Sized>(buf: &B, at: Pos) -> Option<Span> {
    let line = buf.line(at.row);
    let past_end = at.col >= line.len();
    let at = Pos::new(at.row, at.col.min(line.len().checked_sub(1)?));
    // Only a bracket starts a block, and never from past the end (the user,
    // 2026-10-14: Vim's reach from any punctuation stays vim's; past the
    // end is the last word).
    let block = (at.col..line.len())
        .find(|&c| !white(line[c]))
        .filter(|&c| !past_end && "()[]{}".contains(line[c]))
        .and_then(|bracket| Some((bracket, find_match(buf, at, MatchFrom::Percent, true)?)));
    // A block's match is its fixed end and the caret stays at the press, so
    // a far match never scrolls the view away from the pointer (the user,
    // 2026-10-14; Vim's cursor goes to the match).
    Some(match block {
        // From blanks before a closer, the block takes the closer too (Vim's
        // stops at the press, the closer left out).
        Some((bracket, end)) if end < at => Span { fixed: end, moving: Pos::new(at.row, bracket) },
        Some((_, end)) => Span { fixed: end, moving: at },
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
    fn only_a_bracket_starts_an_arrows_block() {
        let line: Vec<char> = "a {b c} d".chars().collect();
        let url: Vec<char> = "https://x.com/{{id}}".chars().collect();
        let p = |c| Pos::new(0, c);
        assert_eq!(
            double_click(&url[..], p(5)),
            Some(Span { fixed: p(5), moving: p(5) }),
            "Vim's reach from `:` is vim's"
        );
        assert_eq!(
            double_click(&line[..], p(1)),
            Some(Span { fixed: p(6), moving: p(1) }),
            "blanks: the caret stays at the press"
        );
    }

    #[test]
    fn a_double_click_is_a_block_from_the_press_or_the_word() {
        let line: Vec<char> = "a {b c} d".chars().collect();
        let p = |c| Pos::new(0, c);
        assert_eq!(
            double_click(&line[..], p(6)),
            Some(Span { fixed: p(2), moving: p(6) }),
            "on the closing brace: its opener fixed, the caret at the press"
        );
        assert_eq!(double_click(&line[..], p(8)), Some(Span { fixed: p(8), moving: p(8) }));
        let closes: Vec<char> = "a {b c}".chars().collect();
        assert_eq!(
            double_click(&closes[..], p(9)),
            Some(Span { fixed: p(6), moving: p(6) }),
            "past the end: the last word, never a block"
        );
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
