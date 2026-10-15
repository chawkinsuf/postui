//! Vim's bracket matching (`%`, the block text objects, a double click on a
//! bracket), moved here from the vim engine so both profiles share it.

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
