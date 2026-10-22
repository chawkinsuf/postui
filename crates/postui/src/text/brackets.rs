//! Vim's bracket matching (`%`, the block text objects, a double click on a
//! bracket), moved here from the vim engine so both profiles share it.

use std::ops::RangeInclusive;

use super::{Lines, Pos};

/// Where [`find_match`] starts from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MatchFrom {
    /// `%`: the bracket under or after the caret on its line.
    Percent,
    /// Vim's `findmatch(NULL, c)`: an opening `c` searches backward for the
    /// unclosed one; a closing `c` searches forward.
    Unclosed(char),
    /// Vim's `findmatchlimit(NULL, c, FM_FORWARD)` with an opening `c`: the
    /// next block that opens after the caret (text objects, spec §3.7).
    NextOpen(char),
}

fn pair(c: char) -> Option<(char, char)> {
    match c {
        '(' | ')' => Some(('(', ')')),
        '[' | ']' => Some(('[', ']')),
        '{' | '}' => Some(('{', '}')),
        _ => None,
    }
}

/// Whether `c` is one of the brackets this module matches.
pub(crate) fn is_bracket(c: char) -> bool {
    pair(c).is_some()
}

/// The bracket at `at` and its partner, both on rows `rows`, for the
/// highlight that lights them: Vim's matchparen with JSON syntax (spec
/// 2026-10-22, matching brackets, R2–R6). With `before` (a caret between
/// chars: arrows, or Vim's Insert), the char before `at` stands in when the
/// one at `at` is no bracket.
///
/// Not [`find_match`]: that is `%`, whose quote parity reads a string's
/// bracket differently from matchparen's syntax test (probe N3). A string
/// here is a run between unescaped `"` on one line, which is exact for
/// JSON. Outside a string, brackets inside strings are skipped; from inside
/// one, nothing is.
pub(crate) fn matching_pair<B: Lines + ?Sized>(
    buf: &B,
    at: Pos,
    before: bool,
    rows: RangeInclusive<usize>,
) -> Option<(Pos, Pos)> {
    let first = *rows.start();
    let last = (*rows.end()).min(buf.line_count() - 1);
    if at.row < first || at.row > last {
        return None;
    }
    let mut line = buf.line(at.row);
    let start = match line.get(at.col).copied().and_then(pair) {
        Some(_) => at.col,
        None if before && at.col > 0 => {
            pair(*line.get(at.col - 1)?)?;
            at.col - 1
        }
        None => return None,
    };
    let c = line[start];
    let (open, close) = pair(c)?;
    let (find, forward) = if c == open { (close, true) } else { (open, false) };
    let mut strings = in_strings(&line);
    let skip = !strings[start];
    let mut depth = 0usize;
    let (mut row, mut col) = (at.row, start);
    loop {
        // One char on, crossing to the next row in view at a line's end.
        if forward {
            col += 1;
            while col >= line.len() {
                if row == last {
                    return None;
                }
                row += 1;
                line = buf.line(row);
                strings = in_strings(&line);
                col = 0;
            }
        } else {
            while col == 0 {
                if row == first {
                    return None;
                }
                row -= 1;
                line = buf.line(row);
                strings = in_strings(&line);
                col = line.len();
            }
            col -= 1;
        }
        if skip && strings[col] {
            continue;
        }
        if line[col] == c {
            depth += 1;
        } else if line[col] == find {
            if depth == 0 {
                return Some((Pos::new(at.row, start), Pos::new(row, col)));
            }
            depth -= 1;
        }
    }
}

/// Which chars of `line` sit inside a JSON string, its quotes included.
pub(crate) fn in_strings(line: &[char]) -> Vec<bool> {
    let mut inside = false;
    let mut escaped = false;
    line.iter()
        .map(|&c| {
            let was = inside;
            if inside && escaped {
                escaped = false;
            } else if inside && c == '\\' {
                escaped = true;
            } else if c == '"' {
                inside = !inside;
            }
            was || inside
        })
        .collect()
}

fn backslashes_before(line: &[char], col: usize) -> usize {
    line[..col].iter().rev().take_while(|&&c| c == '\\').count()
}

/// Vim's `findmatchlimit()` for the three bracket pairs, including its
/// "smart" quote handling (`cpoptions` has no `%`): a bracket inside a
/// double-quoted string is skipped on a line with an even number of
/// quotes, `'x'` literals are skipped, and a bracket's escaping must match
/// the start's. `quotes` false is Vim's temporary `cpo=%` (the opening
/// search of a block object ignores quotes).
pub(crate) fn find_match<B: Lines + ?Sized>(
    buf: &B,
    from: Pos,
    how: MatchFrom,
    quotes: bool,
) -> Option<Pos> {
    let last = buf.line_count() - 1;
    let mut row = from.row;
    // Borrowed where the text allows: an unmatched bracket walks every line.
    let mut line = buf.line(row);
    let mut col = from.col.min(line.len());
    let (initc, findc, backwards, match_escaped) = match how {
        MatchFrom::Percent => {
            if col >= line.len() && col > 0 {
                col -= 1;
            }
            col = (col..line.len()).find(|&i| pair(line[i]).is_some())?;
            let c = line[col];
            let (open, close) = pair(c)?;
            let escaped = backslashes_before(&line, col) % 2 == 1;
            if c == open { (open, close, false, escaped) } else { (close, open, true, escaped) }
        }
        MatchFrom::Unclosed(c) => {
            let (open, close) = pair(c)?;
            if c == open { (close, open, true, false) } else { (open, close, false, false) }
        }
        MatchFrom::NextOpen(c) => {
            let (open, close) = pair(c)?;
            (close, open, false, false)
        }
    };
    let mut count = 0usize;
    let mut do_quotes: i32 = -1;
    let mut inquote = false;
    let mut start_in_quotes: Option<bool> = None;
    loop {
        if backwards {
            if col == 0 {
                if row == 0 {
                    return None;
                }
                row -= 1;
                line = buf.line(row);
                col = line.len();
                do_quotes = -1;
            } else {
                col -= 1;
            }
        } else if col >= line.len() {
            if row == last {
                return None;
            }
            row += 1;
            line = buf.line(row);
            col = 0;
            do_quotes = -1;
        } else {
            col += 1;
        }
        if !quotes {
            do_quotes = 0;
        } else if do_quotes == -1 {
            // Count the quotes on the line, skipping `\x` and `'"'`.
            let mut n: i32 = -1;
            let mut at_start = 0;
            let mut i = 0;
            while i < line.len() {
                if i == col + usize::from(backwards) {
                    at_start = n & 1;
                }
                if line[i] == '"'
                    && (i == 0 || line[i - 1] != '\'' || line.get(i + 1) != Some(&'\''))
                {
                    n += 1;
                }
                if line[i] == '\\' && i + 1 < line.len() {
                    i += 1;
                }
                i += 1;
            }
            do_quotes = n & 1;
            if do_quotes == 0 {
                inquote = false;
                if line.last() == Some(&'\\') {
                    do_quotes = 1;
                    if start_in_quotes.is_none() {
                        inquote = true;
                        start_in_quotes = Some(true);
                    } else if backwards {
                        inquote = true;
                    }
                }
                if row > 0 && buf.line(row - 1).last() == Some(&'\\') {
                    do_quotes = 1;
                    if start_in_quotes.is_none() {
                        inquote = at_start == 1;
                        if inquote {
                            start_in_quotes = Some(true);
                        }
                    } else if !backwards {
                        inquote = true;
                    }
                }
            }
        }
        if start_in_quotes.is_none() {
            start_in_quotes = Some(false);
        }
        match line.get(col).copied() {
            None => {
                if col == 0 || line[col - 1] != '\\' {
                    inquote = false;
                    start_in_quotes = Some(false);
                }
            }
            Some('"') => {
                if do_quotes == 1 && backslashes_before(&line, col).is_multiple_of(2) {
                    inquote = !inquote;
                    start_in_quotes = Some(false);
                }
            }
            Some(c) => {
                if c == '\'' && quotes {
                    if backwards {
                        if col > 1 && line[col - 2] == '\'' {
                            col -= 2;
                            continue;
                        } else if col > 2 && line[col - 2] == '\\' && line[col - 3] == '\'' {
                            col -= 3;
                            continue;
                        }
                    } else if col + 1 < line.len() {
                        if line[col + 1] == '\\' && col + 3 < line.len() && line[col + 3] == '\'' {
                            col += 3;
                            continue;
                        } else if col + 2 < line.len() && line[col + 2] == '\'' {
                            col += 2;
                            continue;
                        }
                    }
                }
                if (!inquote || start_in_quotes == Some(true))
                    && (c == initc || c == findc)
                    && (backslashes_before(&line, col) % 2 == 1) == match_escaped
                {
                    if c == initc {
                        count += 1;
                    } else if count == 0 {
                        return Some(Pos::new(row, col));
                    } else {
                        count -= 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe file (spec 2026-10-22, matching brackets, §1): line 0 has
    /// brackets inside strings, line 2's `{` closes on line 60, line 61 is
    /// `(x)(y)`.
    fn probe() -> edtui::Lines {
        let mut text = String::from("{\"a\": [1, 2], \"b\": \"x{y\", \"c\": \")\"}\n][\n{\n");
        for _ in 0..57 {
            text.push_str("  1,\n");
        }
        text.push_str("}\n(x)(y)");
        edtui::Lines::from(text.as_str())
    }

    fn p(row: usize, col: usize) -> Pos {
        Pos::new(row, col)
    }

    /// Each row of the Vim 9.1 probe table, matchparen with `ft=json`.
    #[test]
    fn the_pair_is_vims_matchparen_with_json_syntax() {
        let buf = probe();
        let all = 0..=61;
        let at = |row, col, before| matching_pair(&buf, p(row, col), before, all.clone());
        assert_eq!(at(0, 0, false), Some((p(0, 0), p(0, 34))), "N1: the string's `{{` is skipped");
        assert_eq!(at(0, 12, false), None, "N2: Normal looks only under the caret");
        assert_eq!(
            at(0, 21, false),
            Some((p(0, 21), p(0, 34))),
            "N3: inside a string, nothing is skipped"
        );
        assert_eq!(at(0, 32, false), None, "N3b: no `(` before the string's `)`");
        assert_eq!(at(61, 0, false), Some((p(61, 0), p(61, 2))), "N6");
        assert_eq!(at(0, 6, false), Some((p(0, 6), p(0, 11))), "N7");
        assert_eq!(at(0, 12, true), Some((p(0, 11), p(0, 6))), "I1: the char before the caret");
        assert_eq!(at(61, 3, true), Some((p(61, 3), p(61, 5))), "I2: the char under it wins");
        assert_eq!(at(61, 6, true), Some((p(61, 5), p(61, 3))), "I3: past the end, the one before");
    }

    #[test]
    fn a_partner_outside_the_rows_lights_nothing() {
        let buf = probe();
        assert_eq!(matching_pair(&buf, p(2, 0), false, 0..=28), None, "N4: the `}}` is below");
        assert_eq!(matching_pair(&buf, p(60, 0), false, 33..=61), None, "N5: the `{{` is above");
        assert_eq!(matching_pair(&buf, p(2, 0), false, 0..=60), Some((p(2, 0), p(60, 0))));
    }

    #[test]
    fn an_escaped_quote_does_not_end_a_string() {
        let line: Vec<char> = r#"{"a\"{": 1}"#.chars().collect();
        assert_eq!(matching_pair(&line[..], p(0, 0), false, 0..=0), Some((p(0, 0), p(0, 10))));
    }

    #[test]
    fn a_char_that_is_no_bracket_lights_nothing() {
        let line: Vec<char> = "ab".chars().collect();
        assert_eq!(matching_pair(&line[..], p(0, 1), true, 0..=0), None);
        assert_eq!(
            matching_pair(&line[..], p(0, 9), true, 0..=0),
            None,
            "past the end of a short line"
        );
        assert_eq!(matching_pair(&line[..], p(0, 0), true, 0..=0), None, "nothing before column 0");
    }
}
