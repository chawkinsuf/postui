//! Motions (spec §3.6): where every tier-1 motion lands and how an operator
//! waiting for it treats it (exclusive, inclusive, linewise). The word
//! motions and `%` are ports of Vim's `fwd_word`, `bck_word`, `end_word`,
//! `bckend_word` (textobject.c) and `findmatchlimit` (search.c), so the
//! engine meets Vim's edge cases exactly. The golden file is the judge.

use super::buf::{Pos, TextBuf};
use super::class::class;
use super::keys::{FindKind, Motion, Op, Screen};
use super::settings::{PARAGRAPHS, SECTIONS, TABSTOP};
use super::search::{self, Dir};
use super::view::View;
use super::{BufState, Engine, LastSearch, Mode, Note, ViewCtx, first_non_blank, first_non_blank_fix};
use unicode_width::UnicodeWidthChar;

/// How an operator treats a motion (`:help exclusive`, `:help linewise`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MKind {
    Exclusive,
    Inclusive,
    Linewise,
}

/// The column `j` and `k` aim for (Vim's `w_curswant`), in virtual
/// (display) columns. `End` after `$`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Want {
    Col(usize),
    End,
}

impl Default for Want {
    fn default() -> Self {
        Want::Col(0)
    }
}

/// What a motion does to the wanted column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WantUpdate {
    /// `j` `k`: keep it.
    Keep,
    /// Most motions: wherever the caret lands.
    Here,
    /// `$`.
    End,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Moved {
    pub to: Pos,
    pub kind: MKind,
    pub want: WantUpdate,
    /// The motion failed: a waiting operator is cancelled (spec §3.6). The
    /// caret still goes to `to`, where Vim's walk ended (`9b` near the start
    /// rests on 1:1); for most failures `to` is the start. `h` `l` `<BS>`
    /// `<Space>` `w` `e` never fail with an operator waiting (Vim's
    /// `nv_left`, `nv_right`, `nv_wordcmd`): `dh` in column 0 acts on an
    /// empty range.
    pub failed: bool,
    /// `<BS>` across a line break inside `d`/`c`: the operator's end is not
    /// adjusted by the exclusive rule (Vim's `CA_NO_ADJ_OP_END`).
    pub no_adjust: bool,
}

impl Moved {
    fn to(to: Pos, kind: MKind, want: WantUpdate) -> Self {
        Self { to, kind, want, failed: false, no_adjust: false }
    }

    /// A failure that changes nothing, not even the wanted column.
    fn refused(from: Pos) -> Self {
        Self { to: from, kind: MKind::Exclusive, want: WantUpdate::Keep, failed: true, no_adjust: false }
    }

    /// A search that failed (Vim's `normal_search()`): the caret stays and
    /// the wanted column is reset, since `w_set_curswant` is set first.
    fn search_failed(from: Pos) -> Self {
        Self { to: from, kind: MKind::Exclusive, want: WantUpdate::Here, failed: true, no_adjust: false }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MotionCx {
    /// The operator waiting for this motion, if any.
    pub op: Option<Op>,
    /// Visual: the caret may rest on the line's end (`selection=inclusive`).
    pub visual: bool,
    pub want: Want,
}

/// Display width of `c` at virtual column `vcol` (tabs run to the next stop).
pub(crate) fn char_width(c: char, vcol: usize) -> usize {
    match c {
        '\t' => TABSTOP - vcol % TABSTOP,
        // Every other ASCII char takes one cell; skipping the table keeps a
        // deep indent cheap to measure.
        c if c.is_ascii() => 1,
        c => c.width().unwrap_or(0).max(1),
    }
}

/// The virtual column where char `col` starts.
pub(crate) fn vcol_of(line: &[char], col: usize) -> usize {
    line.iter().take(col).fold(0, |v, &c| v + char_width(c, v))
}

/// Vim's `coladvance()`: the char covering virtual column `want`, or the
/// line's last char when the line is shorter (`one_more`: one past it, as
/// in Insert and Visual).
pub(crate) fn col_for(line: &[char], want: Want, one_more: bool) -> usize {
    let end = if one_more { line.len() } else { line.len().saturating_sub(1) };
    let Want::Col(target) = want else { return end };
    let mut v = 0;
    for (i, &c) in line.iter().enumerate() {
        let w = char_width(c, v);
        if v + w > target {
            return i.min(end);
        }
        v += w;
    }
    end
}

/// The new wanted column after a motion landed on `at`. `tab_end`: the caret
/// on a tab sits on its last cell (Vim's `getvcol` in Normal mode, and in
/// Visual when the caret is past the anchor).
pub(crate) fn updated_want<B: TextBuf>(buf: &B, at: Pos, update: WantUpdate, old: Want, tab_end: bool) -> Want {
    match update {
        WantUpdate::Keep => old,
        WantUpdate::End => Want::End,
        WantUpdate::Here => {
            let line = buf.line(at.row);
            let start = vcol_of(&line, at.col);
            match line.get(at.col) {
                Some('\t') if tab_end => Want::Col(start + char_width('\t', start) - 1),
                _ => Want::Col(start),
            }
        }
    }
}

/// A caret walking the buffer the way Vim's `inc()`/`dec()` do, with the
/// current row cached (a one-line buffer's `line()` allocates).
pub(crate) struct Walk<'b, B: TextBuf> {
    buf: &'b B,
    pub pos: Pos,
    row: usize,
    chars: Vec<char>,
}

impl<'b, B: TextBuf> Walk<'b, B> {
    pub(crate) fn new(buf: &'b B, pos: Pos) -> Self {
        Self { buf, pos, row: pos.row, chars: buf.line(pos.row).into_owned() }
    }

    pub(crate) fn last_row(&self) -> usize {
        self.buf.line_count() - 1
    }

    pub(crate) fn line(&mut self) -> &[char] {
        if self.row != self.pos.row {
            self.chars = self.buf.line(self.pos.row).into_owned();
            self.row = self.pos.row;
        }
        &self.chars
    }

    /// The char under the walker; `None` on the NUL past the line's end.
    pub(crate) fn at(&mut self) -> Option<char> {
        let col = self.pos.col;
        self.line().get(col).copied()
    }

    /// Vim's `cls()`: 0 blank (and the NUL), 1 punctuation, 2+ word
    /// classes; with `big` every non-blank is 1.
    pub(crate) fn cls(&mut self, big: bool) -> u32 {
        match self.at() {
            None => 0,
            Some(c) => {
                let k = class(c);
                if big && k != 0 { 1 } else { k }
            }
        }
    }

    /// Vim's `inc()`: 0 moved within the line, 2 moved onto the NUL at its
    /// end, 1 moved to the next line, -1 at the end of the buffer.
    pub(crate) fn inc(&mut self) -> i32 {
        let len = self.line().len();
        if self.pos.col < len {
            self.pos.col += 1;
            return if self.pos.col < len { 0 } else { 2 };
        }
        if self.pos.row < self.last_row() {
            self.pos = Pos::new(self.pos.row + 1, 0);
            return 1;
        }
        -1
    }

    /// Vim's `dec()`: 0 within the line, 1 moved onto the previous line's
    /// NUL, -1 at the start of the buffer.
    pub(crate) fn dec(&mut self) -> i32 {
        if self.pos.col > 0 {
            self.pos.col -= 1;
            return 0;
        }
        if self.pos.row > 0 {
            self.pos.row -= 1;
            self.pos.col = self.line().len();
            return 1;
        }
        -1
    }

    /// Vim's `incl()`: `inc()` that skips over a line's NUL.
    pub(crate) fn incl(&mut self) -> i32 {
        let r = self.inc();
        if r >= 1 && self.pos.col > 0 { self.inc() } else { r }
    }

    /// Vim's `decl()`: `dec()` that skips over a line's NUL.
    pub(crate) fn decl(&mut self) -> i32 {
        let r = self.dec();
        if r == 1 && self.pos.col > 0 { self.dec() } else { r }
    }

    fn line_is_empty(&mut self) -> bool {
        self.line().is_empty()
    }
}

/// Vim's `skip_chars()`: moves while the class stays `cclass`. `true` when
/// it ran off the buffer.
fn skip_chars<B: TextBuf>(w: &mut Walk<B>, cclass: u32, forward: bool, big: bool) -> bool {
    while w.cls(big) == cclass {
        if (if forward { w.inc() } else { w.dec() }) == -1 {
            return true;
        }
    }
    false
}

/// Vim's `fwd_word()`. `eol`: stop at the end of a line on the last count
/// (an operator is pending). `false` = FAIL (the caret may still have moved).
pub(crate) fn fwd_word<B: TextBuf>(w: &mut Walk<B>, count: usize, big: bool, eol: bool) -> bool {
    for left in (0..count).rev() {
        let sclass = w.cls(big);
        let last_line = w.pos.row == w.last_row();
        let i = w.inc();
        if i == -1 || (i >= 1 && last_line) {
            return false;
        }
        if i >= 1 && eol && left == 0 {
            return true;
        }
        if sclass != 0 {
            while w.cls(big) == sclass {
                let i = w.inc();
                if i == -1 || (i >= 1 && eol && left == 0) {
                    return true;
                }
            }
        }
        while w.cls(big) == 0 {
            if w.pos.col == 0 && w.line_is_empty() {
                break;
            }
            let i = w.inc();
            if i == -1 || (i >= 1 && eol && left == 0) {
                return true;
            }
        }
    }
    true
}

/// Vim's `bck_word()`. `false` = FAIL (started at the start of the buffer).
pub(crate) fn bck_word<B: TextBuf>(w: &mut Walk<B>, count: usize, big: bool, mut stop: bool) -> bool {
    'outer: for _ in 0..count {
        let sclass = w.cls(big);
        if w.dec() == -1 {
            return false;
        }
        if !stop || sclass == w.cls(big) || sclass == 0 {
            while w.cls(big) == 0 {
                if w.pos.col == 0 && w.line_is_empty() {
                    stop = false;
                    continue 'outer;
                }
                if w.dec() == -1 {
                    return true;
                }
            }
            let c = w.cls(big);
            if skip_chars(w, c, false, big) {
                return true;
            }
        }
        w.inc();
        stop = false;
    }
    true
}

/// Vim's `end_word()`. `stop`: the first count does not hop when already on
/// a word end (`cw`). `empty`: stop on an empty line.
pub(crate) fn end_word<B: TextBuf>(w: &mut Walk<B>, count: usize, big: bool, mut stop: bool, empty: bool) -> bool {
    'outer: for _ in 0..count {
        let sclass = w.cls(big);
        if w.inc() == -1 {
            return false;
        }
        if w.cls(big) == sclass && sclass != 0 {
            if skip_chars(w, sclass, true, big) {
                return false;
            }
        } else if !stop || sclass == 0 {
            while w.cls(big) == 0 {
                if w.pos.col == 0 && w.line_is_empty() && empty {
                    stop = false;
                    continue 'outer;
                }
                if w.inc() == -1 {
                    return false;
                }
            }
            let c = w.cls(big);
            if skip_chars(w, c, true, big) {
                return false;
            }
        }
        w.dec();
        stop = false;
    }
    true
}

/// Vim's `bckend_word()` (Visual `iw` extending backwards).
pub(crate) fn bckend_word<B: TextBuf>(w: &mut Walk<B>, count: usize, big: bool, eol: bool) -> bool {
    for _ in 0..count {
        let sclass = w.cls(big);
        let i = w.dec();
        if i == -1 {
            return false;
        }
        if eol && i == 1 {
            return true;
        }
        if sclass != 0 {
            while w.cls(big) == sclass {
                let i = w.dec();
                if i == -1 || (eol && i == 1) {
                    return true;
                }
            }
        }
        while w.cls(big) == 0 {
            if w.pos.col == 0 && w.line_is_empty() {
                break;
            }
            let i = w.dec();
            if i == -1 || (eol && i == 1) {
                return true;
            }
        }
    }
    true
}

/// Every tier-1 motion from `from` (spec §3.6). `count` 0 means none typed.
pub(crate) fn run<B: TextBuf>(
    buf: &B,
    from: Pos,
    motion: Motion,
    count: usize,
    cx: &MotionCx,
    last_find: &mut Option<(FindKind, char)>,
) -> Moved {
    let n = count.max(1);
    let last = buf.line_count() - 1;
    match motion {
        Motion::Left => {
            let to = Pos::new(from.row, from.col.saturating_sub(n));
            if to == from && cx.op.is_none() {
                Moved::refused(from)
            } else {
                Moved::to(to, MKind::Exclusive, WantUpdate::Here)
            }
        }
        Motion::BackWrap => back_wrap(buf, from, n, cx),
        Motion::Right => right(buf, from, n, cx, false),
        Motion::ForwardWrap => right(buf, from, n, cx, true),
        Motion::Up | Motion::Down => {
            let row = if motion == Motion::Down { (from.row + n).min(last) } else { from.row.saturating_sub(n) };
            if row == from.row {
                return Moved::refused(from);
            }
            let col = col_for(&buf.line(row), cx.want, cx.visual);
            Moved::to(Pos::new(row, col), MKind::Linewise, WantUpdate::Keep)
        }
        Motion::LineStart => Moved::to(Pos::new(from.row, 0), MKind::Exclusive, WantUpdate::Here),
        Motion::FirstNonBlank => {
            Moved::to(Pos::new(from.row, first_non_blank(&buf.line(from.row))), MKind::Exclusive, WantUpdate::Here)
        }
        Motion::LineEnd => {
            if n > 1 && from.row == last {
                // Vim's `nv_dollar` wants the end before `cursor_down` fails.
                return Moved { to: from, kind: MKind::Inclusive, want: WantUpdate::End, failed: true, no_adjust: false };
            }
            let row = (from.row + n - 1).min(last);
            let len = buf.line_len(row);
            let col = if cx.visual { len } else { len.saturating_sub(1) };
            Moved::to(Pos::new(row, col), MKind::Inclusive, WantUpdate::End)
        }
        Motion::FirstLine | Motion::LastLine => {
            let row = match count {
                0 if motion == Motion::FirstLine => 0,
                0 => last,
                c => (c - 1).min(last),
            };
            Moved::to(Pos::new(row, first_non_blank(&buf.line(row))), MKind::Linewise, WantUpdate::Here)
        }
        Motion::WordFwd { big } => word_fwd(buf, from, n, big, cx, false),
        Motion::WordEnd { big } => word_fwd(buf, from, n, big, cx, true),
        Motion::WordBack { big } => {
            let mut w = Walk::new(buf, from);
            let ok = bck_word(&mut w, n, big, false);
            let mut m = Moved::to(w.pos, MKind::Exclusive, WantUpdate::Here);
            m.failed = !ok;
            m
        }
        Motion::Find(kind, ch) => {
            *last_find = Some((kind, ch));
            found(find(buf, from, kind, ch, n, true), from, kind)
        }
        Motion::RepeatFind { reverse } => {
            let Some((kind, ch)) = *last_find else { return Moved::refused(from) };
            let kind = if reverse { kind.reversed() } else { kind };
            // `cpoptions` has no `;`: a repeated till with count 1 skips a
            // match right next to the caret.
            let stop = !(n == 1 && kind.till());
            found(find(buf, from, kind, ch, n, stop), from, kind)
        }
        Motion::WordEndBack { big } => {
            // Vim's `nv_g_cmd()` 'e': the wanted column resets first;
            // `bckend_word()` fails only when it starts on the first char.
            let mut w = Walk::new(buf, from);
            let ok = bckend_word(&mut w, n, big, false);
            Moved { to: w.pos, kind: MKind::Inclusive, want: WantUpdate::Here, failed: !ok, no_adjust: false }
        }
        Motion::Paragraph { forward } => match find_par(buf, from.row, forward, n) {
            Some((row, true)) => Moved::to(Pos::new(row, buf.line_len(row) - 1), MKind::Inclusive, WantUpdate::Here),
            Some((row, false)) => Moved::to(Pos::new(row, 0), MKind::Exclusive, WantUpdate::Here),
            // Vim's `nv_findpar()` resets the wanted column before it looks.
            None => Moved { to: from, kind: MKind::Exclusive, want: WantUpdate::Here, failed: true, no_adjust: false },
        },
        Motion::DownFirstNonBlank | Motion::UpFirstNonBlank => {
            let row = if motion == Motion::DownFirstNonBlank { (from.row + n).min(last) } else { from.row.saturating_sub(n) };
            if row == from.row {
                return Moved::refused(from);
            }
            Moved::to(Pos::new(row, first_non_blank_fix(&buf.line(row))), MKind::Linewise, WantUpdate::Here)
        }
        Motion::MatchPair if count > 0 => {
            if count > 100 {
                return Moved::refused(from);
            }
            let lines = buf.line_count();
            let row = ((count * lines).div_ceil(100)).clamp(1, lines) - 1;
            Moved::to(Pos::new(row, first_non_blank(&buf.line(row))), MKind::Linewise, WantUpdate::Here)
        }
        Motion::MatchPair => match find_match(buf, from, MatchFrom::Percent, true) {
            Some(to) => Moved::to(to, MKind::Inclusive, WantUpdate::Here),
            None => Moved::refused(from),
        },
        Motion::ScreenLine(_) | Motion::SearchNext { .. } | Motion::Ident { .. } | Motion::Search { .. } => {
            unreachable!("Engine::run_motion takes the window and search motions")
        }
    }
}

fn found(to: Option<Pos>, from: Pos, kind: FindKind) -> Moved {
    match to {
        Some(to) => Moved::to(to, if kind.forward() { MKind::Inclusive } else { MKind::Exclusive }, WantUpdate::Here),
        None => Moved::refused(from),
    }
}

/// Vim's `findpar(dir, count, NUL, FALSE)` for `}` (`forward`) and `{`: the
/// row it stops on, and whether it ran onto the last line going forward
/// (the motion then takes that line's last char and is inclusive). `None`
/// when a count is left over at the first or last line.
fn find_par<B: TextBuf>(buf: &B, start: usize, forward: bool, count: usize) -> Option<(usize, bool)> {
    let last = buf.line_count() - 1;
    let mut curr = start;
    for left in (0..count).rev() {
        let mut did_skip = false;
        let mut first = true;
        loop {
            if buf.line_len(curr) > 0 {
                did_skip = true;
            }
            if !first && did_skip && starts_paragraph(&buf.line(curr)) {
                break;
            }
            first = false;
            let next = if forward { (curr < last).then_some(curr + 1) } else { curr.checked_sub(1) };
            match next {
                Some(row) => curr = row,
                None if left > 0 => return None,
                None => break,
            }
        }
    }
    Some((curr, forward && curr == last && buf.line_len(curr) > 0))
}

/// Vim's `startPS(lnum, NUL, FALSE)`: the line starts a paragraph for `{ }`
/// and `ip ap`. It is empty, starts with a form feed, or is an nroff macro
/// line from 'paragraphs' or 'sections' (`.PP`, `.SH`).
pub(crate) fn starts_paragraph(line: &[char]) -> bool {
    match line.first() {
        None | Some('\u{c}') => true,
        Some('.') => in_macro(SECTIONS, &line[1..]) || in_macro(PARAGRAPHS, &line[1..]),
        _ => false,
    }
}

/// Vim's `inmacro()`: the two chars after the `.` match one of `opt`'s
/// two-char names; a space in a name matches a space or the line's end.
/// `opt` is ASCII ('paragraphs' and 'sections' are constants), so it is
/// walked as bytes: this runs on every line a `}` or `ip` crosses.
fn in_macro(opt: &str, s: &[char]) -> bool {
    debug_assert!(opt.is_ascii());
    let names = opt.as_bytes();
    let (s0, s1) = (s.first().copied(), s.get(1).copied());
    let mut i = 0;
    while let Some(&m0) = names.get(i) {
        let m0 = m0 as char;
        let m1 = names.get(i + 1).map(|&b| b as char);
        let first = Some(m0) == s0 || (m0 == ' ' && matches!(s0, None | Some(' ')));
        let second = m1 == s1 || (matches!(m1, None | Some(' ')) && (s0.is_none() || matches!(s1, None | Some(' '))));
        if first && second {
            return true;
        }
        if m1.is_none() {
            break;
        }
        i += 2;
    }
    false
}

/// Vim's `nv_right()` for `l`, `<Right>` and (`wrap`) `<Space>`.
fn right<B: TextBuf>(buf: &B, from: Pos, n: usize, cx: &MotionCx, wrap: bool) -> Moved {
    let last = buf.line_count() - 1;
    let mut p = from;
    let mut inclusive = false;
    for i in 0..n {
        let len = buf.line_len(p.row);
        // `oneright()` fails on the last char; Visual may step onto the NUL.
        let stuck = if cx.visual { p.col >= len } else { p.col + 1 >= len };
        if stuck {
            if wrap && p.row < last {
                // An operator counts the line break as a char: first take
                // the last char, then move on.
                if cx.op.is_some() && !inclusive && len > 0 {
                    inclusive = true;
                } else {
                    p = Pos::new(p.row + 1, 0);
                    inclusive = false;
                }
                continue;
            }
            if cx.op.is_none() {
                if i == 0 {
                    return Moved::refused(from);
                }
            } else if len > 0 {
                inclusive = true;
            }
            break;
        }
        p.col += 1;
    }
    Moved {
        to: p,
        kind: if inclusive { MKind::Inclusive } else { MKind::Exclusive },
        want: WantUpdate::Here,
        failed: p == from && !inclusive && cx.op.is_none(),
        no_adjust: false,
    }
}

/// Vim's `nv_left()` for `<BS>` with `whichwrap=b`.
fn back_wrap<B: TextBuf>(buf: &B, from: Pos, n: usize, cx: &MotionCx) -> Moved {
    let mut p = from;
    let mut no_adjust = false;
    for _ in 0..n {
        if p.col > 0 {
            p.col -= 1;
            continue;
        }
        if p.row == 0 {
            break;
        }
        p.row -= 1;
        let len = buf.line_len(p.row);
        p.col = if cx.visual { len } else { len.saturating_sub(1) };
        // Deleting the line break before the first char: rest on the NUL
        // after the previous line ("a very special case", normal.c).
        if matches!(cx.op, Some(Op::Delete | Op::Change)) && len > 0 {
            p.col = len;
            no_adjust = true;
        }
    }
    if p == from && cx.op.is_none() {
        return Moved::refused(from);
    }
    Moved { to: p, kind: MKind::Exclusive, want: WantUpdate::Here, failed: false, no_adjust }
}

/// Vim's `nv_wordcmd()`: `w` `W` (`end` false) and `e` `E`.
fn word_fwd<B: TextBuf>(buf: &B, from: Pos, n: usize, big: bool, cx: &MotionCx, end: bool) -> Moved {
    let mut w = Walk::new(buf, from);
    let mut kind = if end { MKind::Inclusive } else { MKind::Exclusive };
    let mut word_end = end;
    let mut stop = false;
    // `cw`/`cW` on a non-blank act as `ce`/`cE`, and the first count does
    // not hop when already on a word end (spec §3.6 rule 3).
    if !end && cx.op == Some(Op::Change) && w.at().is_some_and(|c| c != ' ' && c != '\t') {
        kind = MKind::Inclusive;
        word_end = true;
        stop = true;
    }
    let ok = if word_end { end_word(&mut w, n, big, stop, false) } else { fwd_word(&mut w, n, big, cx.op.is_some()) };
    // Don't leave the caret on the NUL past the end of a line (Vim's
    // `adjust_cursor()`): `dw` on a line's last word stops at its end
    // (spec §3.6 rule 2).
    if w.pos > from && w.pos.col > 0 && w.at().is_none() && !cx.visual {
        w.pos.col -= 1;
        kind = MKind::Inclusive;
    }
    Moved { to: w.pos, kind, want: WantUpdate::Here, failed: !ok && cx.op.is_none(), no_adjust: false }
}

/// Vim's `searchc()`: the `n`th `ch` on the line. `stop` false skips a
/// match right next to the caret (a repeated till).
fn find<B: TextBuf>(buf: &B, from: Pos, kind: FindKind, ch: char, n: usize, mut stop: bool) -> Option<Pos> {
    let line = buf.line(from.row);
    let len = line.len() as isize;
    let dir: isize = if kind.forward() { 1 } else { -1 };
    let mut col = from.col as isize;
    for _ in 0..n {
        loop {
            col += dir;
            if col < 0 || col >= len {
                return None;
            }
            if line[col as usize] == ch && stop {
                break;
            }
            stop = true;
        }
    }
    if kind.till() {
        col -= dir;
    }
    Some(Pos::new(from.row, col as usize))
}

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
pub(crate) fn find_match<B: TextBuf>(buf: &B, from: Pos, how: MatchFrom, quotes: bool) -> Option<Pos> {
    let last = buf.line_count() - 1;
    let mut row = from.row;
    let mut line: Vec<char> = buf.line(row).into_owned();
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
                line = buf.line(row).into_owned();
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
            line = buf.line(row).into_owned();
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
                if line[i] == '"' && (i == 0 || line[i - 1] != '\'' || line.get(i + 1) != Some(&'\'')) {
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

impl Engine {
    /// A bare motion in Normal or Visual.
    pub(super) fn exec_move<B: TextBuf>(&mut self, motion: Motion, count: usize, buf: &mut B, st: &mut BufState, ctx: &ViewCtx) {
        let visual = matches!(self.mode, Mode::Visual(_));
        let from = buf.cursor();
        let want = self.want_at(buf, st, from);
        let cx = MotionCx { op: None, visual, want };
        let search = matches!(motion, Motion::Search { .. } | Motion::SearchNext { .. } | Motion::Ident { .. });
        let m = self.run_motion(buf, from, motion, count, &cx, ctx);
        let mut to = self.clamped(buf, m.to);
        if matches!(self.mode, Mode::InsertNormal { .. }) && !search {
            // Inside `ctrl+o` a motion still stops on a char (`oneright()`,
            // `adjust_cursor()`); `j`, `k` and `$` reach the end through
            // the wanted column when Insert resumes. A search has no
            // `adjust_cursor()`: `normal_search()` leaves the caret where
            // `check_cursor()` puts it ([`Engine::clamped`]), so
            // `<C-o>/$<CR>` resumes Insert at the line's end.
            to.col = to.col.min(buf.line_len(to.row).saturating_sub(1));
        }
        buf.set_cursor(to);
        let to = buf.cursor();
        match m.want {
            // Vim's `w_set_curswant = TRUE`: computed when next needed, with
            // the tab rule of that moment (Deviation 9).
            WantUpdate::Here => st.forget_want(),
            update => st.set_want(updated_want(buf, to, update, want, self.tab_rule(st, to)), to),
        }
    }

    /// Every motion: the window-relative ones here, the rest in [`run`].
    pub(super) fn run_motion<B: TextBuf>(&mut self, buf: &B, from: Pos, motion: Motion, count: usize, cx: &MotionCx, ctx: &ViewCtx) -> Moved {
        match motion {
            Motion::ScreenLine(place) => {
                let view = View::of::<B>(ctx);
                let row = match view {
                    Some(view) => view.screen_line(buf, place, count),
                    // No window: the whole text is one (Deviation 12).
                    None => match place {
                        Screen::Top => (count.max(1) - 1).min(buf.line_count() - 1),
                        Screen::Middle => (buf.line_count() - 1) / 2,
                        Screen::Bottom => (buf.line_count() - 1).saturating_sub(count.max(1) - 1),
                    },
                };
                // Without an operator `cursor_correct()` pulls the row into
                // the window; `beginline(BL_SOL | BL_FIX)`.
                let row = match (cx.op, view) {
                    (None, Some(view)) => view.corrected_row(buf, row),
                    _ => row,
                };
                Moved::to(Pos::new(row, first_non_blank_fix(&buf.line(row))), MKind::Linewise, WantUpdate::Here)
            }
            Motion::SearchNext { reverse } => {
                let Some(last) = self.last_search.clone() else {
                    // `normal_search()` resets the wanted column before
                    // `do_search()` finds no pattern (probed: `jnj`).
                    self.note = Some(Note::Message("No previous regular expression".into()));
                    return Moved::search_failed(from);
                };
                let dir = match (last.dir, reverse) {
                    (Dir::Forward, false) | (Dir::Backward, true) => Dir::Forward,
                    _ => Dir::Backward,
                };
                self.search_motion(buf, from, &last.text, dir, count)
            }
            Motion::Ident { backward } => {
                let line = buf.line(from.row);
                let Some((start, end)) = search::find_ident(&line, from.col) else {
                    self.note = Some(Note::Message("No string under cursor".into()));
                    return Moved::refused(from);
                };
                let dir = if backward { Dir::Backward } else { Dir::Forward };
                // Vim's `do_search()` keeps the pattern as its delimiter
                // leaves it: `#`'s `\?` becomes `?`.
                let text = search::as_stored(&search::ident_pattern(&line, start, end, backward), dir);
                self.last_search = Some(LastSearch { text: text.clone(), dir });
                self.search_motion(buf, Pos::new(from.row, start), &text, dir, count)
            }
            // The prompt's Enter: `run_search` stored the typed pattern, or
            // the prompt was empty and the last one stands.
            Motion::Search { dir } => {
                let Some(last) = self.last_search.clone() else {
                    self.note = Some(Note::Message("No previous regular expression".into()));
                    return Moved::search_failed(from);
                };
                self.search_motion(buf, from, &last.text, dir, count)
            }
            _ => run(buf, from, motion, count, cx, &mut self.last_find),
        }
    }

    /// A search as a motion (Vim's `normal_search()`): exclusive, charwise,
    /// the wanted column reset even on failure; a wrap or a miss leaves its
    /// message, and the highlight goes on.
    pub(super) fn search_motion<B: TextBuf>(&mut self, buf: &B, from: Pos, text: &str, dir: Dir, count: usize) -> Moved {
        self.hl = true;
        let pat = match self.compiled(text) {
            Ok(p) => p,
            Err(atom) => {
                self.note = Some(Note::Message(format!("pattern not supported: {atom}")));
                return Moved::search_failed(from);
            }
        };
        match search::search(buf, from, dir, count, &pat) {
            Some((to, wrapped)) => {
                if wrapped {
                    self.note = Some(Note::Message(
                        match dir {
                            Dir::Forward => "search hit BOTTOM, continuing at TOP",
                            Dir::Backward => "search hit TOP, continuing at BOTTOM",
                        }
                        .into(),
                    ));
                }
                // `normal_search()` ends with `check_cursor()`: a match on
                // the line's end (`/$`) lands on the last char in Normal,
                // and stays on the end in Visual and inside Insert `ctrl+o`
                // (`clamped`). The motion stays exclusive, after an
                // operator too: `d/$` keeps the last char in Normal and
                // deletes it inside `ctrl+o`.
                Moved::to(self.clamped(buf, to), MKind::Exclusive, WantUpdate::Here)
            }
            None => {
                self.note = Some(Note::Message(format!("Pattern not found: {text}")));
                Moved::search_failed(from)
            }
        }
    }

    /// The column `j` and `k` aim for from `at`: the remembered one while
    /// the caret is still there, else the caret's own.
    pub(super) fn want_at<B: TextBuf>(&self, buf: &B, st: &BufState, at: Pos) -> Want {
        st.want(at).unwrap_or_else(|| updated_want(buf, at, WantUpdate::Here, Want::default(), self.tab_rule(st, at)))
    }

    /// The tab rule Vim's `w_virtcol` holds for a caret at `at`: the cached
    /// one while the caret has not moved (`v0j` on a tab keeps Normal's
    /// last cell), else the current mode's.
    pub(super) fn tab_rule(&self, st: &BufState, at: Pos) -> bool {
        st.cached_tab_rule(at).unwrap_or_else(|| self.tab_end(at))
    }

    /// Whether a caret at `at` on a tab sits on its last cell (Vim's
    /// `getvcol`: in Normal always, in Visual past the anchor, in Insert
    /// and Replace never).
    pub(super) fn tab_end(&self, at: Pos) -> bool {
        match (self.mode, self.visual) {
            (Mode::Visual(_), Some(anchor)) => at > anchor,
            (Mode::Insert | Mode::Replace, _) => false,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_columns_count_tabs_and_wide_chars() {
        let tabbed: Vec<char> = "\tab".chars().collect();
        assert_eq!(vcol_of(&tabbed, 1), TABSTOP);
        let cjk: Vec<char> = "日本x".chars().collect();
        assert_eq!(vcol_of(&cjk, 2), 4);
        assert_eq!(col_for(&cjk, Want::Col(3), false), 1, "the middle of a wide char is that char");
        assert_eq!(col_for(&cjk, Want::Col(40), false), 2);
        assert_eq!(col_for(&cjk, Want::Col(40), true), 3);
        assert_eq!(col_for(&cjk, Want::End, false), 2);
        assert_eq!(col_for(&[], Want::End, false), 0);
    }

    #[test]
    fn the_wanted_column_is_trusted_only_at_its_caret_until_forgotten() {
        let mut st = BufState::new();
        assert_eq!(st.want(Pos::new(0, 0)), None);
        st.set_want(Want::End, Pos::new(0, 3));
        assert_eq!(st.want(Pos::new(0, 3)), Some(Want::End));
        assert_eq!(st.want(Pos::new(0, 4)), None, "the caret moved");
        st.forget_want();
        assert_eq!(st.want(Pos::new(0, 3)), None, "forgotten");
    }

    /// Vim's `startPS(lnum, NUL, FALSE)` with the pinned 'paragraphs' and
    /// 'sections' (probed, batch p3).
    #[test]
    fn paragraph_starts_follow_vims_start_ps() {
        let starts = |s: &str| starts_paragraph(&s.chars().collect::<Vec<_>>());
        for (line, yes) in [
            ("", true), ("\u{c}", true), ("  ", false), ("x", false), (".PP", true), (".SH x", true),
            (".H", true), (".IP", true), (".x", false), (". ", false), (".", false), ("{", false),
        ] {
            assert_eq!(starts(line), yes, "{line:?}");
        }
    }

    #[test]
    fn the_caret_on_a_tab_wants_its_last_cell_in_normal() {
        use super::super::buf::OneLineBuf;
        use crate::components::line_input::LineInput;
        let mut input = LineInput::new("\tx");
        let buf = OneLineBuf::new(&mut input);
        let at = Pos::new(0, 0);
        let end = updated_want(&buf, at, WantUpdate::Here, Want::default(), true);
        let start = updated_want(&buf, at, WantUpdate::Here, Want::default(), false);
        assert_eq!((end, start), (Want::Col(TABSTOP - 1), Want::Col(0)));
    }
}
