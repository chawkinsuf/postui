//! The body's window (plan 3c): Vim's `move.c` under the pinned `nowrap`,
//! `scrolloff=0` and `scrolljump=1`, with every line one row and no folds.
//! Rows are 0-based; `top` is `TextBuf::top()`, Vim's `w_topline - 1`. The
//! engine runs `update_topline` before and after every key, as Vim's main
//! loop does before each command. The golden file compares `top` on texts
//! of 21 lines and more.

use super::buf::{Pos, TextBuf};
use super::keys::{Screen, Scroll};
use super::{BufState, Engine, ViewCtx, first_non_blank_fix};

/// `scrolljump`.
const SCROLLJUMP: usize = 1;

/// What Vim's window code reads and moves (plan M5a Deviation 14): a text
/// of one-row lines, the first row shown, and the caret's row. Every engine
/// buffer is one (the blanket impl below); the response viewer is another,
/// so the body and the response run the same `move.c` port. The names
/// differ from `TextBuf`'s, so the two traits never clash where both are in
/// scope.
pub(crate) trait Rows {
    /// At least 1.
    fn lines(&self) -> usize;
    /// The text is one empty line: Vim's `update_topline()` puts row 0 on top.
    fn blank(&self) -> bool;
    fn topline(&self) -> usize;
    fn set_topline(&mut self, row: usize);
    fn cur_row(&self) -> usize;
    /// Moves the caret to `row`: its column kept, or column 0 with `col0`.
    fn set_cur_row(&mut self, row: usize, col0: bool);
}

impl<B: TextBuf> Rows for B {
    fn lines(&self) -> usize {
        self.line_count()
    }
    fn blank(&self) -> bool {
        self.line_count() == 1 && self.line_len(0) == 0
    }
    fn topline(&self) -> usize {
        self.top()
    }
    fn set_topline(&mut self, row: usize) {
        self.set_top(row);
    }
    fn cur_row(&self) -> usize {
        self.cursor().row
    }
    fn set_cur_row(&mut self, row: usize, col0: bool) {
        let col = if col0 { 0 } else { self.cursor().col };
        self.set_cursor(Pos::new(row, col));
    }
}

/// The body's window: `height` rows, `top` the first row shown (`buf.top()`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct View {
    pub height: usize,
}

impl View {
    /// The body's window; `None` for a one-line field or a body drawn
    /// nowhere yet (Deviation 12: then the whole text is the window).
    pub(crate) fn of<B: TextBuf>(ctx: &ViewCtx) -> Option<View> {
        if !B::MULTILINE {
            return None;
        }
        ctx.viewport_rows.map(|height| View { height: height.max(1) })
    }

    /// Vim's `w_botline` as a 0-based row: the first row below the window,
    /// at most the line count (`comp_botline()`).
    pub(crate) fn botline<R: Rows>(self, buf: &R) -> usize {
        (buf.topline() + self.height).min(buf.lines())
    }

    /// Vim's `w_empty_rows`: rows below the last line.
    pub(crate) fn empty_rows<R: Rows>(self, buf: &R) -> usize {
        self.height - (self.botline(buf) - buf.topline())
    }

    fn clamp_top<R: Rows>(self, buf: &mut R) {
        let last = buf.lines() - 1;
        if buf.topline() > last {
            buf.set_topline(last);
        }
    }

    /// Vim's `update_topline()`: scroll so the caret's line is shown. Above
    /// the window: `scroll_cursor_top` when close, halfway when far; below
    /// it: `scroll_cursor_bot` when within a screen, halfway when further.
    /// A `top` past the text (a delete or an outside change shrank it) is
    /// not clamped first: Vim keeps the stale `w_topline`, so the caret is
    /// above the window and the distance picks the rule (the golden file's
    /// `search/long` `d/line 50<CR>` from line 100: line 28 on top, halfway).
    pub(crate) fn update_topline<R: Rows>(self, buf: &mut R) {
        let lines = buf.lines();
        let cur = buf.cur_row().min(lines - 1);
        if buf.blank() {
            buf.set_topline(0);
            return;
        }
        let top = buf.topline();
        let mut check_botline = false;
        if top > 0 && cur < top {
            let halfheight = (self.height / 2).saturating_sub(1).max(2);
            let n = top - cur;
            if n >= halfheight {
                self.scroll_cursor_halfway(buf, false, false);
            } else {
                self.scroll_cursor_top(buf, SCROLLJUMP, false);
                check_botline = true;
            }
        } else {
            check_botline = true;
        }
        if check_botline {
            let bot = self.botline(buf);
            if bot < lines && cur >= bot {
                let line_count = cur - bot + 1;
                if line_count <= self.height + 1 {
                    self.scroll_cursor_bot(buf, SCROLLJUMP, false);
                } else {
                    self.scroll_cursor_halfway(buf, false, false);
                }
            }
        }
    }

    /// Vim's `scroll_cursor_top()`: put the caret's line at the top,
    /// scrolling at least `min_scroll`; `always` sets `top` even upward.
    pub(crate) fn scroll_cursor_top<R: Rows>(self, buf: &mut R, min_scroll: usize, always: bool) {
        let cur = buf.cur_row();
        let top = buf.topline();
        let mut scrolled = usize::from(cur < top);
        let mut used = 1usize;
        let mut above = cur as isize - 1;
        let mut new_top = cur;
        while above >= 0 {
            if (above as usize) < top {
                scrolled += 1;
            }
            if new_top >= top || scrolled > min_scroll {
                break;
            }
            used += 1;
            if used > self.height {
                break;
            }
            new_top = above as usize;
            above -= 1;
        }
        if used > self.height {
            self.scroll_cursor_halfway(buf, false, false);
        } else {
            if new_top < top || always {
                buf.set_topline(new_top);
            }
            if buf.topline() > cur {
                buf.set_topline(cur);
            }
        }
    }

    /// Vim's `scroll_cursor_bot()`: put the caret's line at the bottom,
    /// scrolling at least `min_scroll`; `set_topbot` (`zb`) first sets the
    /// window so the caret's line is its last row.
    pub(crate) fn scroll_cursor_bot<R: Rows>(self, buf: &mut R, min_scroll: usize, set_topbot: bool) {
        let lines = buf.lines();
        let cur = buf.cur_row();
        if set_topbot {
            buf.set_topline((cur + 1).saturating_sub(self.height));
        }
        let top = buf.topline();
        let bot = self.botline(buf);
        let empty = self.empty_rows(buf) as isize;
        let mut used = 1usize;
        let mut scrolled: isize = 0;
        if cur >= bot {
            scrolled = 1;
            if cur == bot {
                scrolled -= empty;
            }
        }
        let (mut loff, mut boff) = (cur, cur);
        while loff > 0 {
            // Vim: `(((scrolled <= 0 || scrolled >= min_scroll) && extra >= so)
            // || boff + 1 > line_count) && loff <= botline`, with `so = 0`.
            if ((scrolled <= 0 || scrolled >= min_scroll as isize) || boff + 1 >= lines) && loff <= bot {
                break;
            }
            loff -= 1;
            used += 1;
            if used > self.height {
                break;
            }
            if loff >= bot {
                scrolled += 1;
                if loff == bot {
                    scrolled -= empty;
                }
            }
            if boff + 1 < lines {
                boff += 1;
                used += 1;
                if used > self.height {
                    break;
                }
                if scrolled < min_scroll as isize && boff >= bot {
                    scrolled += 1;
                    if boff == bot {
                        scrolled -= empty;
                    }
                }
            }
        }
        let line_count = if scrolled <= 0 {
            0
        } else if used > self.height {
            used
        } else {
            let mut lc = 0usize;
            let mut b = top as isize - 1;
            let mut i = 0isize;
            while i < scrolled && b < bot as isize {
                b += 1;
                i += 1;
                lc += 1;
            }
            if i < scrolled { 9999 } else { lc }
        };
        if line_count >= self.height && line_count > min_scroll {
            self.scroll_cursor_halfway(buf, false, true);
        } else if line_count > 0 {
            // `scrollup(line_count)`
            buf.set_topline((top + line_count).min(lines - 1));
            if buf.cur_row() < buf.topline() {
                let t = buf.topline();
                buf.set_cur_row(t, true);
            }
        }
    }

    /// Vim's `scroll_cursor_halfway()`: the caret's line in the middle.
    /// `atend` counts `~` rows past the end as used (`zz`); `prefer_above`
    /// adds a line above before one below.
    pub(crate) fn scroll_cursor_halfway<R: Rows>(self, buf: &mut R, atend: bool, prefer_above: bool) {
        let lines = buf.lines();
        let cur = buf.cur_row();
        let (mut above, mut below, mut used) = (0usize, 0usize, 1usize);
        let mut topline = cur;
        let (mut loff, mut boff) = (cur as isize, cur);
        while topline > 0 {
            let mut done = false;
            for round in 1..=2 {
                let add_below = if prefer_above { round == 2 && below < above } else { round == 1 && below <= above };
                if add_below {
                    if boff + 1 < lines {
                        boff += 1;
                        used += 1;
                        if used > self.height {
                            done = true;
                            break;
                        }
                        below += 1;
                    } else {
                        below += 1;
                        if atend {
                            used += 1;
                        }
                    }
                }
                let add_above = if prefer_above { round == 1 && below >= above } else { round == 1 && below > above };
                if add_above {
                    loff -= 1;
                    if loff < 0 {
                        used = usize::MAX;
                    } else {
                        used += 1;
                    }
                    if used > self.height {
                        done = true;
                        break;
                    }
                    above += 1;
                    topline = loff as usize;
                }
            }
            if done {
                break;
            }
        }
        buf.set_topline(topline);
    }

    /// Vim's `cursor_correct()` with `scrolloff=0`: the caret is pulled
    /// into the window when it sits outside it.
    pub(crate) fn cursor_correct<R: Rows>(self, buf: &mut R) {
        let cur = buf.cur_row();
        let row = self.corrected_row(buf, cur);
        if row != cur {
            buf.set_cur_row(row, false);
        }
    }

    /// Vim's `nv_scroll()` target row for `H` (`count` from the top), `M`
    /// (the middle of the lines shown) and `L` (`count` from the bottom).
    pub(crate) fn screen_line<R: Rows>(self, buf: &R, place: Screen, count: usize) -> usize {
        let lines = buf.lines();
        let top = buf.topline();
        let bot = self.botline(buf);
        let count1 = count.max(1);
        match place {
            // A count past the window's top lands on the text's first line.
            Screen::Bottom => (bot - 1).saturating_sub(count1 - 1),
            Screen::Middle => {
                let half = (self.height - self.empty_rows(buf)).div_ceil(2);
                let mut used = 0;
                let mut n = 0;
                while top + n < lines - 1 {
                    used += 1;
                    if used >= half {
                        break;
                    }
                    n += 1;
                }
                (top + n).min(lines - 1)
            }
            Screen::Top => (top + count1 - 1).min(lines - 1),
        }
    }

    /// `cursor_correct()` for a row the caret is about to take.
    pub(crate) fn corrected_row<R: Rows>(self, buf: &R, row: usize) -> usize {
        let top = buf.topline();
        let bot = self.botline(buf);
        if row < top && top > 0 {
            return top;
        }
        if row >= bot && bot < buf.lines() {
            return bot - 1;
        }
        row
    }

    /// Vim's `halfpage()`: scroll `'scroll'` lines (a count sets it, capped
    /// at the height) and move the caret as many; when the text's end (or
    /// start) is reached the caret moves the rest. Ends with
    /// `cursor_correct()`; the caller lands on the first non-blank.
    pub(crate) fn halfpage<R: Rows>(self, buf: &mut R, down: bool, count: usize, scroll: &mut Option<usize>) {
        if count > 0 {
            *scroll = Some(count.min(self.height));
        }
        // `win_comp_scroll()`: half the height, at least one line.
        let mut n = scroll.unwrap_or((self.height / 2).max(1)).min(self.height);
        // Clamps `top` too, before the window helpers read it.
        self.update_topline(buf);
        let lines = buf.lines();
        let mut top = buf.topline();
        let mut cur = buf.cur_row();
        if down {
            while n > 0 && top + self.height < lines {
                n -= 1;
                top += 1;
                if cur + 1 < lines {
                    cur += 1;
                }
            }
            if n > 0 {
                cur = (cur + n).min(lines - 1);
            }
        } else {
            while n > 0 && top > 0 {
                n -= 1;
                top -= 1;
                cur = cur.saturating_sub(1);
            }
            if n > 0 {
                cur = cur.saturating_sub(n);
            }
        }
        buf.set_topline(top);
        buf.set_cur_row(cur, false);
        self.cursor_correct(buf);
    }

    /// Vim's `onepage()` for `count` pages: forward keeps two lines of
    /// overlap and, once the last line shows, puts it at the top; backward
    /// makes the line above the window (plus the overlap) the bottom line,
    /// scrolling at least one. `false` when it beeped: a one-line text, or
    /// no further page (the caller then skips the first-non-blank landing).
    pub(crate) fn onepage<R: Rows>(self, buf: &mut R, down: bool, count: usize) -> bool {
        let lines = buf.lines();
        if lines == 1 {
            return false;
        }
        // The window helpers need `top` inside the text.
        self.clamp_top(buf);
        let mut ok = true;
        for _ in 0..count {
            let top = buf.topline();
            let bot = self.botline(buf);
            if down {
                if top >= lines - 1 && bot >= lines {
                    ok = false;
                    break;
                }
                if bot >= lines {
                    buf.set_topline(lines - 1);
                } else {
                    // `get_scroll_overlap(loff = botline, -1)`.
                    let new_top = self.overlap_up(bot);
                    buf.set_topline(new_top);
                    buf.set_cur_row(new_top, false);
                }
            } else {
                if top == 0 {
                    ok = false;
                    break;
                }
                // `get_scroll_overlap(loff = topline - 1, +1)`.
                let lp = self.overlap_down(top - 1, lines).min(lines - 1);
                buf.set_cur_row(lp, false);
                // The line just above the new topline: `height + 1` rows up,
                // then two forward again; past the start, the top is row 0.
                match lp.checked_sub(self.height + 1) {
                    None => buf.set_topline(0),
                    Some(above) if above + 2 >= top => {
                        // Always scroll at least one line.
                        buf.set_topline(top - 1);
                        let bot = self.botline(buf);
                        buf.set_cur_row(bot - 1, false);
                    }
                    Some(above) => buf.set_topline(above + 2),
                }
            }
        }
        self.cursor_correct(buf);
        ok
    }

    /// `get_scroll_overlap(lp, -1)` with one-row lines: from the row `below`
    /// the window, the new top is two rows up when three rows above it
    /// exist and fit, one row up when only two do, else `below` itself.
    fn overlap_up(self, below: usize) -> usize {
        let min_height = self.height.saturating_sub(2);
        // h2 or h3 missing (`MAXCOL`) or too tall: no overlap.
        if 2 > min_height || below < 2 {
            return below;
        }
        // h4 missing or too tall: one line of overlap.
        if 3 > min_height || below < 3 {
            return below - 1;
        }
        below - 2
    }

    /// `get_scroll_overlap(lp, +1)`: from the row `above` the window, the
    /// new bottom is two rows down when three rows below it exist and fit,
    /// one row down when only two do, else `above` itself.
    fn overlap_down(self, above: usize, lines: usize) -> usize {
        let min_height = self.height.saturating_sub(2);
        // h2 or h3 missing (`MAXCOL`) or too tall: no overlap.
        if 2 > min_height || above + 2 >= lines {
            return above;
        }
        // h4 missing or too tall: one line of overlap.
        if 3 > min_height || above + 3 >= lines {
            return above + 1;
        }
        above + 2
    }
}

impl Engine {
    /// `ctrl+d ctrl+u ctrl+f ctrl+b` (Vim's `nv_halfpage()` and
    /// `nv_page()`). A one-line field never gets here in Normal (declined)
    /// and treats them as failed motions in Visual: nothing happens.
    pub(super) fn exec_scroll<B: TextBuf>(&mut self, how: Scroll, count: usize, buf: &mut B, st: &mut BufState, ctx: &ViewCtx) {
        let Some(view) = View::of::<B>(ctx) else { return };
        let before = buf.cursor();
        let ok = match how {
            Scroll::HalfDown | Scroll::HalfUp => {
                let down = how == Scroll::HalfDown;
                // `nv_halfpage()`: at the edge it beeps before `'scroll'` is
                // set, and the wanted column stays.
                if (down && before.row == buf.line_count() - 1) || (!down && before.row == 0) {
                    return;
                }
                view.halfpage(buf, down, count, &mut st.scroll);
                true
            }
            Scroll::PageDown => view.onepage(buf, true, count.max(1)),
            Scroll::PageUp => view.onepage(buf, false, count.max(1)),
        };
        if ok {
            let row = buf.cursor().row;
            let col = first_non_blank_fix(&buf.line(row));
            buf.set_cursor(Pos::new(row, col));
        }
        // Visual keeps its anchor; the caret may rest on the line's end there.
        let at = self.clamped(buf, buf.cursor());
        buf.set_cursor(at);
        // `beginline()` resets `w_curswant`; a page that beeped skips it, so
        // the wanted column survives, re-keyed to where the caret now is.
        match st.want(before) {
            Some(want) if !ok => st.set_want(want, at),
            _ => st.forget_want(),
        }
    }

    /// `zt` `zz` `zb` (Vim's `nv_zet()`): a count is the line to go to (the
    /// column clamped), then the window is placed; the caret's column stays,
    /// and so does the wanted column (`nv_zet()` leaves `w_curswant` alone:
    /// `$3zzj` still reaches the end of the line below).
    /// Without a window the line still changes and only the placement has
    /// nothing to do (Deviation 12).
    pub(super) fn exec_scroll_cursor<B: TextBuf>(&mut self, place: Screen, count: usize, buf: &mut B, st: &mut BufState, ctx: &ViewCtx) {
        if count > 0 {
            let row = (count - 1).min(buf.line_count() - 1);
            let before = buf.cursor();
            if row != before.row {
                let want = self.want_at(buf, st, before);
                let at = self.clamped(buf, Pos::new(row, before.col));
                buf.set_cursor(at);
                st.set_want(want, at);
            }
        }
        let Some(view) = View::of::<B>(ctx) else { return };
        match place {
            Screen::Top => view.scroll_cursor_top(buf, 0, true),
            Screen::Middle => view.scroll_cursor_halfway(buf, true, false),
            Screen::Bottom => view.scroll_cursor_bot(buf, 0, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window with no text behind it: what the response viewer is (plan
    /// M5a Task 2). Rows and lines are 0-based.
    struct Win {
        lines: usize,
        top: usize,
        row: usize,
    }

    impl Rows for Win {
        fn lines(&self) -> usize {
            self.lines
        }
        fn blank(&self) -> bool {
            false
        }
        fn topline(&self) -> usize {
            self.top
        }
        fn set_topline(&mut self, row: usize) {
            self.top = row;
        }
        fn cur_row(&self) -> usize {
            self.row
        }
        fn set_cur_row(&mut self, row: usize, _col0: bool) {
            self.row = row;
        }
    }

    /// The plan's probe record (Vim 9.1, a 23-row window, 100 lines).
    #[test]
    fn the_window_code_runs_on_bare_rows() {
        let v = View { height: 23 };
        let mut w = Win { lines: 100, top: 0, row: 0 };
        assert!(v.onepage(&mut w, true, 1));
        assert_eq!((w.top, w.row), (21, 21), "P1: ctrl+f from the top");
        let mut w = Win { lines: 100, top: 39, row: 49 };
        v.scroll_cursor_halfway(&mut w, true, false);
        assert_eq!((w.top, w.row), (38, 49), "Z2: zz");
        let mut w = Win { lines: 100, top: 0, row: 0 };
        let mut scroll = None;
        v.halfpage(&mut w, true, 0, &mut scroll);
        assert_eq!((w.top, w.row), (11, 11), "D1: ctrl+d");
    }
}
