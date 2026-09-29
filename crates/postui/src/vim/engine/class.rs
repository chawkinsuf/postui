//! Vim's character classes for word motions and objects (spec §3.13).
//! The table is Vim's own `charclass()` output under the pinned
//! `iskeyword` (`class_table.rs`, generated), so NBSP is blank, all of
//! 192–255 are word chars (× and ÷ included), CJK ideographs are their
//! own class apart from Latin, and emoji are class 3. `word_nav::is_word`
//! disagrees at those edges, which is why the engine does not use it.

use super::class_table::TABLE;

/// Vim's `utf_class()` for `c`: 0 blank, 1 punctuation, 2 word, 3 emoji,
/// larger: a script's own word class.
#[allow(dead_code)] // used from Task 6
pub(crate) fn class(c: char) -> u32 {
    let c = c as u32;
    let i = TABLE.partition_point(|&(first, _, _)| first <= c);
    // The table starts at 0 and covers every scalar value, so `i >= 1`.
    let (_, last, k) = TABLE[i - 1];
    if c <= last { k } else { 2 }
}

/// Vim's `vim_iswordc()`: part of a keyword.
#[allow(dead_code)] // used from Task 9
pub(crate) fn is_word(c: char) -> bool {
    class(c) >= 2
}

/// Vim's `vim_isspace()`: ASCII 9–13 and space. Insert `ctrl+w` uses it.
#[allow(dead_code)] // used from Task 9
pub(crate) fn is_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_match_vim_at_the_spec_edges() {
        for (c, k) in [
            (' ', 0), ('\t', 0), ('\u{a0}', 0), ('\u{3000}', 0),
            ('a', 2), ('Z', 2), ('0', 2), ('_', 2), ('µ', 2),
            ('.', 1), ('"', 1), ('{', 1), ('—', 1),
            ('é', 2), ('ö', 2), ('×', 2), ('÷', 2),
            ('日', 0x4e00), ('ひ', 0x3040), ('カ', 0x30a0), ('한', 0xac00),
            ('😀', 3),
        ] {
            assert_eq!(class(c), k, "class of {c:?}");
        }
    }

    #[test]
    fn helpers_follow_vim() {
        assert!(!is_space('\u{a0}'), "NBSP: blank for words, not vim_isspace");
        assert!(is_space('\n') && is_space(' ') && is_space('\t'));
        assert!(is_word('日') && is_word('😀') && is_word('_') && !is_word('-'));
    }

    #[test]
    fn the_table_is_sorted_and_covers_every_scalar() {
        assert_eq!(TABLE[0].0, 0);
        for pair in TABLE.windows(2) {
            let ((_, last, _), (first, _, _)) = (pair[0], pair[1]);
            assert!(first == last + 1 || (last == 0xd7ff && first == 0xe000), "gap after {last:#x}");
        }
        assert_eq!(TABLE.last().unwrap().1, 0x10ffff);
    }
}
