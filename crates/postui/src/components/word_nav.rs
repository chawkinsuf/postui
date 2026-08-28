//! Word-boundary motion shared by every text input: the body editor's
//! ctrl/alt+arrow nav and `LineInput`'s. One rule everywhere, desktop-editor
//! style: a "word" is a run of alphanumerics/`_`; a run of other
//! non-whitespace punctuation is its own hop; whitespace is skipped, never
//! landed in.

/// Whether `c` belongs to a word run (alphanumeric or `_`) rather than a
/// punctuation run.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The boundary a word-left motion from char index `col` lands on: skip any
/// whitespace immediately left of the caret, then walk to the start of the
/// run (word chars or punctuation) that precedes it. `col` clamps to the
/// line length; already at 0 stays at 0.
pub fn prev_word_boundary(chars: &[char], col: usize) -> usize {
    let mut i = col.min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    let word = is_word(chars[i - 1]);
    while i > 0 && !chars[i - 1].is_whitespace() && is_word(chars[i - 1]) == word {
        i -= 1;
    }
    i
}

/// The half-open span of the character run containing char index `col` —
/// what a double click selects: a word run, a punctuation run, or a
/// whitespace run, per the same classes the motions above hop by. `None`
/// when `col` is past the last char (a click beyond the line selects
/// nothing).
pub fn word_span_at(chars: &[char], col: usize) -> Option<(usize, usize)> {
    let class = |c: char| (c.is_whitespace(), is_word(c));
    let k = class(*chars.get(col)?);
    let mut s = col;
    while s > 0 && class(chars[s - 1]) == k {
        s -= 1;
    }
    let mut e = col + 1;
    while e < chars.len() && class(chars[e]) == k {
        e += 1;
    }
    Some((s, e))
}

/// The boundary a word-right motion from char index `col` lands on: skip any
/// whitespace under the caret, then walk to the end of the run (word chars
/// or punctuation) that follows. Already at the end stays at the end.
pub fn next_word_boundary(chars: &[char], col: usize) -> usize {
    let mut i = col.min(chars.len());
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if i == chars.len() {
        return i;
    }
    let word = is_word(chars[i]);
    while i < chars.len() && !chars[i].is_whitespace() && is_word(chars[i]) == word {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn next_stops_at_end_of_each_run() {
        let line = chars("foo bar");
        assert_eq!(next_word_boundary(&line, 0), 3);
        assert_eq!(next_word_boundary(&line, 3), 7);
        assert_eq!(next_word_boundary(&line, 7), 7);
    }

    #[test]
    fn prev_stops_at_start_of_each_run() {
        let line = chars("foo bar");
        assert_eq!(prev_word_boundary(&line, 7), 4);
        assert_eq!(prev_word_boundary(&line, 4), 0);
        assert_eq!(prev_word_boundary(&line, 0), 0);
    }

    #[test]
    fn punctuation_runs_are_their_own_hops() {
        // A JSON-ish line: `{"name": "test"` — the `{"` and `":` runs hop
        // separately from the identifiers they wrap.
        let line = chars("{\"name\": \"test\"");
        assert_eq!(next_word_boundary(&line, 0), 2); // past {"
        assert_eq!(next_word_boundary(&line, 2), 6); // past name
        assert_eq!(next_word_boundary(&line, 6), 8); // past ":
        assert_eq!(prev_word_boundary(&line, 8), 6);
        assert_eq!(prev_word_boundary(&line, 6), 2);
    }

    #[test]
    fn whitespace_is_skipped_never_landed_in() {
        let line = chars("foo   bar");
        assert_eq!(next_word_boundary(&line, 3), 9);
        assert_eq!(prev_word_boundary(&line, 6), 0);
    }

    #[test]
    fn underscores_and_digits_are_word_chars() {
        let line = chars("snake_case2 x");
        assert_eq!(next_word_boundary(&line, 0), 11);
        assert_eq!(prev_word_boundary(&line, 11), 0);
    }

    #[test]
    fn clamps_out_of_range_col() {
        let line = chars("hi");
        assert_eq!(next_word_boundary(&line, 99), 2);
        assert_eq!(prev_word_boundary(&line, 99), 0);
    }

    #[test]
    fn empty_line_is_a_no_op() {
        let line = chars("");
        assert_eq!(next_word_boundary(&line, 0), 0);
        assert_eq!(prev_word_boundary(&line, 0), 0);
    }

    #[test]
    fn span_covers_the_word_run_around_any_of_its_chars() {
        let line = chars("foo bar_2 baz");
        assert_eq!(word_span_at(&line, 4), Some((4, 9)));
        assert_eq!(word_span_at(&line, 8), Some((4, 9)));
        assert_eq!(word_span_at(&line, 0), Some((0, 3)));
    }

    #[test]
    fn span_on_punctuation_covers_the_punctuation_run() {
        let line = chars("{\"name\": 1}");
        assert_eq!(word_span_at(&line, 0), Some((0, 2))); // {"
        assert_eq!(word_span_at(&line, 7), Some((6, 8))); // ":
    }

    #[test]
    fn span_on_whitespace_covers_the_whitespace_run() {
        let line = chars("a   b");
        assert_eq!(word_span_at(&line, 2), Some((1, 4)));
    }

    #[test]
    fn span_past_the_end_is_none() {
        let line = chars("ab");
        assert_eq!(word_span_at(&line, 2), None);
        assert_eq!(word_span_at(&chars(""), 0), None);
    }
}
