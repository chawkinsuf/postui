//! The body's window (plan 3c): Vim's `move.c` under the pinned `nowrap`,
//! `scrolloff=0` and `scrolljump=1`, with every line one row and no folds.
//! Rows are 0-based; `top` is `TextBuf::top()`, Vim's `w_topline - 1`. The
//! engine runs `update_topline` before and after every key, as Vim's main
//! loop does before each command. The golden file compares `top` on texts
//! of 21 lines and more.

use super::ViewCtx;
use super::buf::TextBuf;

/// `scrolljump`.
const SCROLLJUMP: usize = 1;

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
    pub(crate) fn botline<B: TextBuf>(self, buf: &B) -> usize {
        (buf.top() + self.height).min(buf.line_count())
    }

    /// Vim's `w_empty_rows`: rows below the last line.
    pub(crate) fn empty_rows<B: TextBuf>(self, buf: &B) -> usize {
        self.height - (self.botline(buf) - buf.top())
    }

    fn clamp_top<B: TextBuf>(self, buf: &mut B) {
        let last = buf.line_count() - 1;
        if buf.top() > last {
            buf.set_top(last);
        }
    }

    /// Vim's `update_topline()`: scroll so the caret's line is shown. Above
    /// the window: `scroll_cursor_top` when close, halfway when far; below
    /// it: `scroll_cursor_bot` when within a screen, halfway when further.
    pub(crate) fn update_topline<B: TextBuf>(self, buf: &mut B) {
        self.clamp_top(buf);
        let lines = buf.line_count();
        let cur = buf.cursor().row.min(lines - 1);
        if lines == 1 && buf.line_len(0) == 0 {
            buf.set_top(0);
            return;
        }
        let top = buf.top();
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
    pub(crate) fn scroll_cursor_top<B: TextBuf>(self, buf: &mut B, min_scroll: usize, always: bool) {
        let cur = buf.cursor().row;
        let top = buf.top();
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
                buf.set_top(new_top);
            }
            if buf.top() > cur {
                buf.set_top(cur);
            }
        }
    }

    /// Vim's `scroll_cursor_bot()`: put the caret's line at the bottom,
    /// scrolling at least `min_scroll`; `set_topbot` (`zb`) first sets the
    /// window so the caret's line is its last row.
    pub(crate) fn scroll_cursor_bot<B: TextBuf>(self, buf: &mut B, min_scroll: usize, set_topbot: bool) {
        let lines = buf.line_count();
        let cur = buf.cursor().row;
        if set_topbot {
            buf.set_top((cur + 1).saturating_sub(self.height));
        }
        let top = buf.top();
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
            buf.set_top((top + line_count).min(lines - 1));
            if buf.cursor().row < buf.top() {
                let t = buf.top();
                buf.set_cursor(super::buf::Pos::new(t, 0));
            }
        }
    }

    /// Vim's `scroll_cursor_halfway()`: the caret's line in the middle.
    /// `atend` counts `~` rows past the end as used (`zz`); `prefer_above`
    /// adds a line above before one below.
    pub(crate) fn scroll_cursor_halfway<B: TextBuf>(self, buf: &mut B, atend: bool, prefer_above: bool) {
        let lines = buf.line_count();
        let cur = buf.cursor().row;
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
        buf.set_top(topline);
    }
}
