//! Text objects (spec §3.7): ports of Vim 9.1's `current_word`,
//! `current_quote` and `current_block` (textobject.c), for an operator
//! (`vis: None`) and for Visual (`vis: Some(anchor)`).

use super::buf::{Pos, TextBuf};
use super::class::white;
use super::keys::Object;
use super::motion::{MKind, MatchFrom, Walk, bck_word, bckend_word, end_word, find_match, fwd_word};
use super::op::Span;

/// What an object selected. For an operator, `start..end` with
/// `inclusive` saying whether `end` is taken. In Visual, `start` is the
/// new anchor and `end` the new caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Picked {
    pub start: Pos,
    pub end: Pos,
    pub inclusive: bool,
}

impl Picked {
    /// The operator's reach. `end` can come before `start` (`diw` on an
    /// empty last line walks back onto the line above); Vim's
    /// `do_pending_operator` swaps them, as [`Span::between`] does.
    pub(crate) fn span(self) -> Span {
        Span::between(self.start, self.end, if self.inclusive { MKind::Inclusive } else { MKind::Exclusive })
    }
}

/// `inner` is `i`, otherwise `a`. `count` 0 means none typed. `Err`: no
/// such object here; it holds where the caret goes, since Vim's word
/// walk moves the caret before it fails (`d3aW` running off the buffer).
pub(crate) fn pick<B: TextBuf>(
    buf: &B,
    caret: Pos,
    vis: Option<Pos>,
    obj: Object,
    inner: bool,
    count: usize,
) -> Result<Picked, Pos> {
    let count = count.max(1);
    match obj {
        Object::Word { big } => word(buf, caret, vis, count, !inner, big),
        Object::Quote(q) => quote(buf, caret, vis, count, !inner, q).ok_or(caret),
        Object::Block(open) => block(buf, caret, vis, count, !inner, open).ok_or(caret),
    }
}

fn char_at<B: TextBuf>(buf: &B, p: Pos) -> Option<char> {
    buf.line(p.row).get(p.col).copied()
}

/// Vim's `inindent(1)`: the char at `p` and everything before it are blanks.
fn in_indent_through<B: TextBuf>(buf: &B, p: Pos) -> bool {
    buf.line(p.row).iter().take_while(|&&c| white(c)).count() > p.col
}

/// Vim's `back_in_line()`: to the start of the word or blank run under the walker.
fn back_in_line<B: TextBuf>(w: &mut Walk<B>, big: bool) {
    let sclass = w.cls(big);
    while w.pos.col > 0 {
        w.dec();
        if w.cls(big) != sclass {
            w.inc();
            break;
        }
    }
}

/// Vim's `current_word()`. `Err` holds where the walk stopped.
fn word<B: TextBuf>(buf: &B, caret: Pos, vis: Option<Pos>, count: usize, include: bool, big: bool) -> Result<Picked, Pos> {
    let mut w = Walk::new(buf, caret);
    let mut count = count;
    let mut start = vis.unwrap_or(caret);
    let mut inclusive = true;
    let mut include_white = false;
    if vis.is_none_or(|v| v == caret) {
        back_in_line(&mut w, big);
        start = w.pos;
        if (w.cls(big) == 0) == include {
            if !end_word(&mut w, 1, big, true, true) {
                return Err(w.pos);
            }
        } else {
            fwd_word(&mut w, 1, big, true);
            // A one-char word at the start of the next line: back up to the
            // end of this one.
            if w.pos.col == 0 {
                w.decl();
            } else {
                w.pos.col -= 1;
            }
            include_white = include;
        }
        count -= 1;
    }
    while count > 0 {
        inclusive = true;
        // `start` is the Visual anchor here (Vim's `VIsual`).
        if vis.is_some() && w.pos < start {
            // In Visual with the caret at the start: move it back.
            if w.decl() == -1 {
                return Err(w.pos);
            }
            if include != (w.cls(big) != 0) {
                if !bck_word(&mut w, 1, big, true) {
                    return Err(w.pos);
                }
            } else {
                if !bckend_word(&mut w, 1, big, true) {
                    return Err(w.pos);
                }
                w.incl();
            }
        } else {
            if w.incl() == -1 {
                return Err(w.pos);
            }
            if include != (w.cls(big) == 0) {
                if !fwd_word(&mut w, 1, big, true) && count > 1 {
                    return Err(w.pos);
                }
                // If the end is just past a line break, don't take the next
                // line's first char: rest on the last blank.
                if w.pos.col == 0 {
                    inclusive = false;
                } else {
                    w.pos.col -= 1;
                }
            } else if !end_word(&mut w, 1, big, true, true) {
                return Err(w.pos);
            }
        }
        count -= 1;
    }
    if include_white && (w.cls(big) != 0 || (w.pos.col == 0 && !inclusive)) {
        // No blank to take after the word: take the blanks before it, but
        // never the indent at the start of a line (this makes `daw` work on
        // the last word of a line).
        let end = w.pos;
        w.pos = start;
        if w.pos.col > 0 {
            w.pos.col -= 1;
            back_in_line(&mut w, big);
            if w.cls(big) == 0 && w.pos.col > 0 {
                start = w.pos;
            }
        }
        w.pos = end;
    }
    Ok(Picked { start, end: w.pos, inclusive })
}

/// Vim's `find_next_quote()`: the next `q` at or after `col`; `escape`
/// skips the char after a backslash (`quoteescape`).
fn find_next_quote(line: &[char], mut col: usize, q: char, escape: bool) -> Option<usize> {
    loop {
        let c = *line.get(col)?;
        if escape && c == '\\' {
            col += 1;
            if col >= line.len() {
                return None;
            }
        } else if c == q {
            return Some(col);
        }
        col += 1;
    }
}

/// Vim's `find_prev_quote()`: the previous `q` before `col` (0 when none).
fn find_prev_quote(line: &[char], mut col: usize, q: char, escape: bool) -> usize {
    while col > 0 {
        col -= 1;
        let n = if escape { line[..col].iter().rev().take_while(|&&c| c == '\\').count() } else { 0 };
        if n % 2 == 1 {
            col -= n;
        } else if line[col] == q {
            break;
        }
    }
    col
}

/// Vim's `current_quote()` (within one line).
fn quote<B: TextBuf>(buf: &B, caret: Pos, vis: Option<Pos>, count: usize, include: bool, q: char) -> Option<Picked> {
    let line: Vec<char> = buf.line(caret.row).into_owned();
    let at = |i: usize| line.get(i).copied();
    let mut vis_empty = true;
    let mut vis_bef_curs = false;
    let mut inside_quotes = false;
    let mut selected_quote = false;
    if let Some(v) = vis {
        if v.row != caret.row {
            return None;
        }
        vis_bef_curs = v < caret;
        vis_empty = v == caret;
        if !vis_empty {
            let (lo, hi) = if vis_bef_curs { (v.col, caret.col) } else { (caret.col, v.col) };
            inside_quotes = lo > 0 && at(lo - 1) == Some(q) && at(hi).is_some() && at(hi + 1) == Some(q);
            selected_quote = (lo..=hi).take_while(|&i| i < line.len()).any(|i| line[i] == q);
        }
    }
    let mut col_start = caret.col;
    let mut col_end;
    if !vis_empty && at(col_start) == Some(q) {
        // Already selecting and on a quote: the next quoted string.
        if vis_bef_curs {
            col_start = find_next_quote(&line, col_start + 1, q, false)?;
            match find_next_quote(&line, col_start + 1, q, true) {
                Some(e) => col_end = e,
                None => {
                    col_end = col_start;
                    col_start = caret.col;
                }
            }
        } else {
            col_end = find_prev_quote(&line, col_start, q, false);
            if at(col_end) != Some(q) {
                return None;
            }
            col_start = find_prev_quote(&line, col_end, q, true);
            if at(col_start) != Some(q) {
                col_start = col_end;
                col_end = caret.col;
            }
        }
    } else if at(col_start) == Some(q) || !vis_empty {
        // On a quote (opening or closing?) or extending: pair quotes from
        // the start of the line to find the string around `first_col`.
        let first_col = if vis_empty {
            col_start
        } else if vis_bef_curs {
            find_next_quote(&line, col_start, q, false)?
        } else {
            find_prev_quote(&line, col_start, q, false)
        };
        col_start = 0;
        loop {
            col_start = find_next_quote(&line, col_start, q, false)?;
            if col_start > first_col {
                return None;
            }
            col_end = find_next_quote(&line, col_start + 1, q, true)?;
            if col_start <= first_col && first_col <= col_end {
                break;
            }
            col_start = col_end + 1;
        }
    } else {
        // Search backward for a starting quote, else forward for one.
        col_start = find_prev_quote(&line, col_start, q, true);
        if at(col_start) != Some(q) {
            col_start = find_next_quote(&line, col_start, q, false)?;
        }
        col_end = find_next_quote(&line, col_start + 1, q, true)?;
    }
    // `a"`: blanks after the closing quote, or else before the opening one.
    if include {
        if at(col_end + 1).is_some_and(white) {
            while at(col_end + 1).is_some_and(white) {
                col_end += 1;
            }
        } else {
            while col_start > 0 && white(line[col_start - 1]) {
                col_start -= 1;
            }
        }
    }
    // After `vi"` another `i"` takes the quotes; `v2i"` does too.
    if !include && count < 2 && (vis_empty || !inside_quotes) {
        col_start += 1;
    }
    let row = caret.row;
    let mut anchor = Pos::new(row, col_start);
    if let Some(v) = vis {
        let keep = !vis_empty
            && !(vis_bef_curs
                && !selected_quote
                && (inside_quotes || (at(v.col) != Some(q) && (v.col == 0 || at(v.col - 1) != Some(q)))));
        if keep {
            anchor = v;
        }
    }
    let mut end = col_end;
    let mut inclusive = false;
    if include || count > 1 || (!vis_empty && inside_quotes) {
        end += 1;
        // Vim's inc_cursor() == 2: stepped onto the line's end.
        if end >= line.len() {
            end = line.len();
            inclusive = true;
        }
    }
    let Some(v) = vis else {
        return Some(Picked { start: Pos::new(row, col_start), end: Pos::new(row, end), inclusive });
    };
    if vis_empty || vis_bef_curs {
        // `selection=inclusive`: the caret sits on the last char.
        Some(Picked { start: anchor, end: Pos::new(row, end.saturating_sub(1)), inclusive: true })
    } else {
        // The caret is at the Visual area's start: mostly restore the
        // selection an empty Visual area would have made.
        let mut head_anchor = v;
        if inside_quotes
            || (!selected_quote && at(v.col) != Some(q) && (at(v.col).is_none() || at(v.col + 1) != Some(q)))
        {
            head_anchor = Pos::new(row, end.saturating_sub(1));
        }
        Some(Picked { start: head_anchor, end: Pos::new(row, col_start), inclusive: true })
    }
}

/// Vim's `current_block()`: `open` is `(`, `[` or `{`.
fn block<B: TextBuf>(buf: &B, caret: Pos, vis: Option<Pos>, count: usize, include: bool, open: char) -> Option<Picked> {
    let close = match open {
        '(' => ')',
        '[' => ']',
        _ => '}',
    };
    let mut cursor = caret;
    let (mut old_start, mut old_end) = (caret, caret);
    match vis {
        Some(v) if v != caret => {
            if v < caret {
                old_start = v;
                cursor = v;
            } else {
                old_end = v;
            }
        }
        _ => {
            if open == '{' {
                // Ignore indent.
                let mut w = Walk::new(buf, cursor);
                while in_indent_through(buf, w.pos) {
                    if w.inc() != 0 {
                        break;
                    }
                }
                cursor = w.pos;
            }
            if char_at(buf, cursor) == Some(open) {
                cursor.col += 1;
            }
        }
    }
    // The unclosed opener before the caret, `count` levels out, with quotes
    // ignored (Vim's temporary `cpo=%`). Inside no block: the next one.
    let how = if find_match(buf, cursor, MatchFrom::Unclosed(open), false).is_some() {
        MatchFrom::Unclosed(open)
    } else {
        MatchFrom::NextOpen(open)
    };
    let mut at = cursor;
    for _ in 0..count {
        at = find_match(buf, at, how, false)?;
    }
    let mut start = at;
    let mut end = find_match(buf, start, MatchFrom::Unclosed(close), true)?;
    let mut sol = false;
    if !include {
        loop {
            // Exclude the brackets, and the indent before a closer that
            // starts its line.
            let mut s = Walk::new(buf, start);
            s.incl();
            start = s.pos;
            sol = end.col == 0;
            let mut e = Walk::new(buf, end);
            e.decl();
            while in_indent_through(buf, e.pos) {
                sol = true;
                if e.decl() != 0 {
                    break;
                }
            }
            end = e.pos;
            // In Visual, a result no bigger than the selection grows to the
            // next block out, then excludes again.
            if vis.is_some() && start >= old_start && old_end >= end && start != end {
                let mut w = Walk::new(buf, old_start);
                w.decl();
                start = find_match(buf, w.pos, MatchFrom::Unclosed(open), true)?;
                end = find_match(buf, start, MatchFrom::Unclosed(close), true)?;
            } else {
                break;
            }
        }
    }
    if vis.is_some() {
        let mut head = end;
        if sol && char_at(buf, head).is_some() {
            // Take the line break too.
            let mut w = Walk::new(buf, head);
            w.inc();
            head = w.pos;
        }
        return Some(Picked { start, end: head, inclusive: true });
    }
    if sol {
        let mut w = Walk::new(buf, end);
        w.incl();
        return Some(Picked { start, end: w.pos, inclusive: false });
    }
    if start <= end {
        Some(Picked { start, end, inclusive: true })
    } else {
        // `()`: nothing between the brackets; operate on nothing.
        Some(Picked { start, end: start, inclusive: false })
    }
}
