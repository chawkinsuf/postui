//! Insert mode (spec §3.8). The engine types into the buffer itself
//! through `Ed::splice`, so every change is recorded; `LineInput::handle_key`
//! is never called in the vim profile. Also the change operator, which
//! opens a session, and `Engine::paste`.
//!
//! The keys follow Vim's `edit.c` under the pinned settings (`autoindent`,
//! `expandtab`, `tabstop=2`, `softtabstop=0`, `backspace=indent,eol,start`,
//! `whichwrap=b,s`): `ins_bs()`, `ins_del()`, `ins_eol()` with
//! `open_line()`, `ins_tab()`, the cursor keys with `start_arrow()`, and
//! `ins_esc()` with `stop_insert()`. The golden file is the judge.

use super::buf::{Pos, TextBuf};
use super::class::{class, is_space, is_word, white};
use super::history::{Ed, end_of};
use super::keys::{Cmd, InsertHow, Key};
use super::motion::{Want, col_for, vcol_of};
use super::op::{RKind, Range, yank_of};
use super::settings::TABSTOP;
use super::{BufState, Engine, Mode, Outcome, Target, first_non_blank};
use crate::components::line_input::flatten_paste;
use ratatui::crossterm::event::KeyEvent;

/// A key typed in Insert, kept so `.` can replay the session (spec §3.12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InsertKey {
    Char(char),
    Enter,
    Backspace,
    Delete,
    CtrlW,
    CtrlU,
    Tab,
    Paste(String),
}

/// An open Insert session (spec §3.8).
#[derive(Debug, Clone)]
pub(crate) struct Session {
    /// Where typing began (Vim's `Insstart_orig`): `ctrl+w` and `ctrl+u`
    /// stop here once.
    pub start: Pos,
    /// The command that opened the session, for `.`.
    #[allow(dead_code)] // read by `.` (Task 12)
    pub origin: Cmd,
    pub typed: Vec<InsertKey>,
    /// The row whose indent `autoindent` added with nothing typed after it
    /// yet (Vim's `did_ai`, which `autoindent` sets even for a zero
    /// indent): Esc, Enter or Up/Down remove that indent.
    pub ai_row: Option<usize>,
    /// Typing resumed after a cursor key: `.` only takes this record if
    /// something is typed (Vim's pretend `1i`).
    #[allow(dead_code)] // read by `.` (Task 12)
    pub resumed: bool,
}

impl Session {
    pub(crate) fn new(start: Pos, origin: Cmd) -> Self {
        Self { start, origin, typed: Vec::new(), ai_row: None, resumed: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Erase {
    Char,
    Word,
    Line,
}

/// The indent `autoindent` copies from `line` when the line breaks at
/// `col` (Vim's `open_line()` measures the text before the caret only): its
/// width, as spaces (`expandtab`).
fn indent_before(line: &[char], col: usize) -> String {
    " ".repeat(vcol_of(line, first_non_blank(line).min(col)))
}

impl Engine {
    fn session(&mut self) -> &mut Session {
        self.insert.get_or_insert_with(|| Session::new(Pos::default(), Cmd::Insert { how: InsertHow::Before, count: 0 }))
    }

    pub(super) fn open_session(&mut self, at: Pos, origin: Cmd, ai_row: Option<usize>) {
        self.mode = Mode::Insert;
        self.insert = Some(Session { ai_row, ..Session::new(at, origin) });
    }

    /// `i a I A o O` (spec §3.8, Vim's `nv_edit()` and `n_opencmd()`).
    /// `o`/`O` do nothing in a one-line field.
    pub(super) fn exec_insert<B: TextBuf>(&mut self, how: InsertHow, count: usize, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let len = buf.line_len(caret.row);
        let mut ai_row = None;
        let at = match how {
            InsertHow::Before => caret,
            InsertHow::After => Pos::new(caret.row, (caret.col + 1).min(len)),
            InsertHow::LineStart => Pos::new(caret.row, first_non_blank(&buf.line(caret.row))),
            InsertHow::LineEnd => Pos::new(caret.row, len),
            InsertHow::OpenBelow | InsertHow::OpenAbove => {
                if !B::MULTILINE {
                    return;
                }
                let indent = indent_before(&buf.line(caret.row), len);
                // The step's caret is where `o` was typed (`n_opencmd`'s `u_save`).
                let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
                let at = if how == InsertHow::OpenBelow {
                    ed.splice(Pos::new(caret.row, len), Pos::new(caret.row, len), &format!("\n{indent}"));
                    Pos::new(caret.row + 1, indent.len())
                } else {
                    ed.splice(Pos::new(caret.row, 0), Pos::new(caret.row, 0), &format!("{indent}\n"));
                    Pos::new(caret.row, indent.len())
                };
                ai_row = Some(at.row);
                at
            }
        };
        buf.set_cursor(at);
        st.forget_want();
        self.open_session(at, Cmd::Insert { how, count }, ai_row);
    }

    /// One key in Insert (the table in the plan, Task 9).
    pub(super) fn insert_key<B: TextBuf>(&mut self, ev: KeyEvent, buf: &mut B, st: &mut BufState) -> Outcome {
        let decline = Outcome::Declined { count: None, keys: vec![ev] };
        let input = match Key::of(&ev) {
            Key::Esc if self.insert_only => return decline,
            Key::Esc => {
                self.end_insert(buf, st, true);
                return Outcome::consumed();
            }
            Key::Char(c) => InsertKey::Char(c),
            Key::Enter if B::MULTILINE => InsertKey::Enter,
            Key::Tab if B::MULTILINE => InsertKey::Tab,
            Key::Backspace | Key::Ctrl('h') => InsertKey::Backspace,
            Key::Ctrl('w') => InsertKey::CtrlW,
            Key::Ctrl('u') => InsertKey::CtrlU,
            Key::Delete => InsertKey::Delete,
            key @ (Key::Left | Key::Right | Key::Home | Key::End) => {
                self.insert_arrow(key, buf, st);
                return Outcome::consumed();
            }
            key @ (Key::Up | Key::Down) if B::MULTILINE => {
                self.insert_arrow(key, buf, st);
                return Outcome::consumed();
            }
            _ => return decline,
        };
        self.insert_input(input, buf, st);
        Outcome::consumed()
    }

    /// One recorded Insert key: typed now, or replayed by `.`. Every one of
    /// them resets the wanted column (Vim's Insert loop sets
    /// `w_set_curswant` until a cursor key is used).
    pub(super) fn insert_input<B: TextBuf>(&mut self, key: InsertKey, buf: &mut B, st: &mut BufState) {
        st.forget_want();
        match key {
            InsertKey::Char(c) => self.type_text(&c.to_string(), InsertKey::Char(c), buf, st),
            InsertKey::Paste(text) => {
                let record = InsertKey::Paste(text.clone());
                self.type_text(&text, record, buf, st);
            }
            InsertKey::Enter => self.newline(buf, st),
            InsertKey::Tab => {
                // `ins_tab()` with `expandtab`: spaces to the next stop,
                // counted from the caret's virtual column (a tab's start).
                let caret = buf.cursor();
                let v = vcol_of(&buf.line(caret.row), caret.col);
                self.type_text(&" ".repeat(TABSTOP - v % TABSTOP), InsertKey::Tab, buf, st);
            }
            InsertKey::Backspace => self.backspace(Erase::Char, buf, st),
            InsertKey::CtrlW => self.backspace(Erase::Word, buf, st),
            InsertKey::CtrlU => self.backspace(Erase::Line, buf, st),
            InsertKey::Delete => self.delete_forward(buf, st),
        }
    }

    /// A query box has no Visual layer, so typing over a mouse or GUI-key
    /// selection replaces it, as in any GUI field. `true` when one went.
    fn replace_gui_selection<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) -> bool {
        if !self.insert_only {
            return false;
        }
        let Some(sel) = buf.gui_selection() else { return false };
        let (lo, hi) = if sel.anchor <= sel.head { (sel.anchor, sel.head) } else { (sel.head, sel.anchor) };
        let end = Pos::new(hi.row, (hi.col + 1).min(buf.line_len(hi.row)));
        buf.clear_gui_selection();
        Ed { buf: &mut *buf, hist: &mut st.history }.splice(lo, end, "");
        buf.set_cursor(lo);
        true
    }

    fn type_text<B: TextBuf>(&mut self, text: &str, record: InsertKey, buf: &mut B, st: &mut BufState) {
        self.replace_gui_selection(buf, st);
        let caret = buf.cursor();
        Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, caret, text);
        buf.set_cursor(end_of(caret, text));
        let s = self.session();
        s.typed.push(record);
        s.ai_row = None;
    }

    /// Insert `Enter` in the body: Vim's `ins_eol()` and `open_line()` with
    /// `autoindent`. The new line gets the indent of the text before the
    /// caret; the moved text loses its leading blanks; a line that holds
    /// only an indent `autoindent` added loses its trailing blanks. Vim
    /// sets `did_ai` for the new line even when text moved onto it (the
    /// golden file's `i<CR><CR>` cases).
    fn newline<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let line = buf.line(caret.row).into_owned();
        let indent = indent_before(&line, caret.col);
        let mut cut = caret.col;
        if self.session().ai_row == Some(caret.row) {
            while cut > 0 && white(line[cut - 1]) {
                cut -= 1;
            }
        }
        let rest = caret.col + line[caret.col..].iter().take_while(|&&c| white(c)).count();
        Ed { buf: &mut *buf, hist: &mut st.history }.splice(Pos::new(caret.row, cut), Pos::new(caret.row, rest), &format!("\n{indent}"));
        let at = Pos::new(caret.row + 1, indent.len());
        buf.set_cursor(at);
        let s = self.session();
        s.typed.push(InsertKey::Enter);
        s.ai_row = Some(at.row);
    }

    /// `BS` `ctrl+h` `ctrl+w` `ctrl+u` (Vim's `ins_bs()`).
    fn backspace<B: TextBuf>(&mut self, mode: Erase, buf: &mut B, st: &mut BufState) {
        let record = match mode {
            Erase::Char => InsertKey::Backspace,
            Erase::Word => InsertKey::CtrlW,
            Erase::Line => InsertKey::CtrlU,
        };
        if self.replace_gui_selection(buf, st) {
            self.session().typed.push(record);
            return;
        }
        let caret = buf.cursor();
        if caret == Pos::new(0, 0) {
            return;
        }
        let start = self.session().start;
        let joined = caret.col == 0;
        let to = if joined {
            // `backspace=eol`: join with the line above (no space).
            let prev = buf.line_len(caret.row - 1);
            let to = Pos::new(caret.row - 1, prev);
            Ed { buf: &mut *buf, hist: &mut st.history }.splice(to, caret, "");
            if caret.row == start.row {
                self.session().start = to;
            }
            to
        } else {
            let line = buf.line(caret.row).into_owned();
            // `ctrl+u` with `autoindent` keeps an indent the caret is past.
            let fnb = first_non_blank(&line);
            let mincol = if mode == Erase::Line && fnb < caret.col { fnb } else { 0 };
            let mut col = caret.col;
            let mut in_word = false;
            let mut word_kind = false;
            let mut cclass = line.get(col).map_or(0, |&c| class(c));
            loop {
                col -= 1;
                let cc = line[col];
                let prev_cclass = cclass;
                cclass = class(cc);
                if mode == Erase::Word && !in_word && !is_space(cc) {
                    in_word = true;
                    word_kind = is_word(cc);
                } else if in_word && (is_space(cc) || is_word(cc) != word_kind || prev_cclass != cclass) {
                    col += 1;
                    break;
                }
                // `backspace=start` crosses the insert start, but `ctrl+w`
                // and `ctrl+u` stop there once.
                if mode == Erase::Char || col <= mincol || Pos::new(caret.row, col) == start {
                    break;
                }
            }
            let to = Pos::new(caret.row, col);
            Ed { buf: &mut *buf, hist: &mut st.history }.splice(to, caret, "");
            let s = self.session();
            if s.start.row == caret.row && col < s.start.col {
                s.start.col = col;
            }
            to
        };
        buf.set_cursor(to);
        let s = self.session();
        s.typed.push(record);
        // Vim keeps `did_ai` over a BS unless it joined lines or left the
        // caret in column 0 or 1.
        if joined || to.col <= 1 {
            s.ai_row = None;
        }
    }

    /// `Del` (Vim's `ins_del()`): forward; at the line's end, join the next
    /// line (no space).
    fn delete_forward<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        if !self.replace_gui_selection(buf, st) {
            let caret = buf.cursor();
            let end = if caret.col < buf.line_len(caret.row) {
                Some(Pos::new(caret.row, caret.col + 1))
            } else if caret.row + 1 < buf.line_count() {
                Some(Pos::new(caret.row + 1, 0))
            } else {
                None
            };
            if let Some(end) = end {
                Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, end, "");
                buf.set_cursor(caret);
            }
        }
        let s = self.session();
        s.typed.push(InsertKey::Delete);
        s.ai_row = None;
    }

    /// Vim's `stop_insert()` when `did_ai` is set (an `autoindent` nothing
    /// was typed after): from `at`, blanks are deleted backwards, stepping
    /// off the line's end first, until a non-blank or column 0. Returns
    /// where the caret goes when it stays on `at`'s line: one further right
    /// when it rests on a last char ("put cursor back on the NUL"). `None`
    /// when there was no autoindent.
    fn strip_autoindent<B: TextBuf>(&mut self, at: Pos, buf: &mut B, st: &mut BufState) -> Option<Pos> {
        if self.insert.as_ref().is_none_or(|s| s.ai_row != Some(at.row)) {
            return None;
        }
        let row = at.row;
        let mut col = at.col.min(buf.line_len(row));
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        let cc = loop {
            let len = ed.buf.line_len(row);
            if col >= len && col > 0 {
                col -= 1;
            }
            match ed.buf.line(row).get(col).copied() {
                Some(c) if white(c) => ed.splice(Pos::new(row, col), Pos::new(row, col + 1), ""),
                cc => break cc,
            }
        };
        if cc.is_some() && col + 1 == buf.line_len(row) {
            col += 1;
        }
        Some(Pos::new(row, col))
    }

    /// A cursor key in Insert. One that moves calls Vim's `start_arrow()`:
    /// the undo step and the `.` record end, and typing after it starts a
    /// fresh `i` record. One that cannot move (`Left` in column 0, `Up` on
    /// the first line) only beeps in Vim, so nothing ends.
    fn insert_arrow<B: TextBuf>(&mut self, key: Key, buf: &mut B, st: &mut BufState) {
        let c = buf.cursor();
        let len = buf.line_len(c.row);
        let last = buf.line_count() - 1;
        // `cursor_up()`/`cursor_down()` aim for the wanted column, taken
        // from the caret before anything is stripped.
        let want = self.want_at(buf, st, c);
        let to = match key {
            Key::Left if c.col > 0 => Pos::new(c.row, c.col - 1),
            Key::Right if c.col < len => Pos::new(c.row, c.col + 1),
            Key::Home => Pos::new(c.row, 0),
            Key::End => Pos::new(c.row, len),
            Key::Up if c.row > 0 => Pos::new(c.row - 1, col_for(&buf.line(c.row - 1), want, true)),
            Key::Down if c.row < last => Pos::new(c.row + 1, col_for(&buf.line(c.row + 1), want, true)),
            _ => return,
        };
        // `stop_insert()` strips an unused autoindent only when the caret
        // leaves its line (`cpoptions` has no `I`), and the caret then goes
        // to the new line regardless.
        if to.row != c.row {
            self.strip_autoindent(c, buf, st);
        }
        st.history.commit();
        self.finish_record();
        buf.set_cursor(to);
        match key {
            // `j`/`k` style: the wanted column survives.
            Key::Up | Key::Down => st.set_want(want, to),
            // `ins_end()` wants the end, but a `w_set_curswant` still set
            // from typing overrides it.
            Key::End if st.want(c).is_some() => st.set_want(Want::End, to),
            _ => st.forget_want(),
        }
        let s = self.session();
        s.start = to;
        s.origin = Cmd::Insert { how: InsertHow::Before, count: 1 };
        s.typed.clear();
        s.ai_row = None;
        s.resumed = true;
    }

    /// Leaves Insert: `Esc` (`step_back`), or `leave`. An indent
    /// `autoindent` added that nothing followed is removed, the session's
    /// undo step closes, and the caret steps back unless at column 0.
    pub(super) fn end_insert<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState, step_back: bool) {
        let mut caret = buf.cursor();
        if let Some(at) = self.strip_autoindent(caret, buf, st) {
            caret = at;
        }
        st.history.commit();
        self.finish_record();
        self.insert = None;
        self.mode = Mode::Normal;
        if step_back && caret.col > 0 {
            caret.col -= 1;
        }
        buf.set_cursor(caret);
        st.forget_want();
    }

    /// `.` bookkeeping for a session that ends or is split by a cursor key.
    /// Task 12 fills this in.
    fn finish_record(&mut self) {}

    /// The change operator (spec §3.6, Vim's `op_change()`): the text goes
    /// to the register and a session opens at the range start.
    pub(super) fn change<B: TextBuf>(&mut self, r: Range, reg: Option<char>, origin: Cmd, buf: &mut B, st: &mut BufState) {
        self.regs.write(reg, yank_of(buf, r));
        self.change_text(r, origin, buf, st);
    }

    /// `op_change()` after the yank: deletes `r` and opens the session. A
    /// linewise change keeps the first line's indent (`autoindent`), which
    /// Esc removes if nothing follows. When nothing is deleted (an empty
    /// region, or a buffer with no lines), Vim still saves the line, so `u`
    /// undoes the step.
    pub(super) fn change_text<B: TextBuf>(&mut self, r: Range, origin: Cmd, buf: &mut B, st: &mut BufState) {
        let emptied = st.history.emptied();
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        // Vim's first `u_save` runs with the caret on the range start,
        // except that a linewise change of several lines deletes all but
        // the first with the caret moved one line down (`op_delete()`).
        let saved = match r.kind {
            RKind::Line if r.end.row > r.start.row => Pos::new(r.start.row + 1, r.start.col),
            _ => r.start,
        };
        ed.hist.begin(saved);
        let (from, to, at, ai_row) = match r.kind {
            _ if emptied => (Pos::new(0, 0), Pos::new(0, 0), Pos::new(0, 0), None),
            RKind::Char => (r.start, r.end, r.start, None),
            RKind::Line => {
                let indent = first_non_blank(&ed.buf.line(r.start.row));
                let last_len = ed.buf.line_len(r.end.row);
                let from = Pos::new(r.start.row, indent);
                (from, Pos::new(r.end.row, last_len), from, Some(r.start.row))
            }
        };
        if ed.buf.slice(from, to).is_empty() {
            ed.save_line(from.row);
        } else {
            ed.splice(from, to, "");
        }
        buf.set_cursor(at);
        st.forget_want();
        self.open_session(at, origin, ai_row);
    }

    /// Bracketed paste (spec §4.2). In Insert it is typed as part of the
    /// session: a one-line field flattens it (`flatten_paste`), and the
    /// body takes CRLF and CR as line breaks and keeps tabs (Vim pastes
    /// with `paste` set: no autoindent, no expandtab). In Normal or Visual
    /// it is declined; piece 4 routes it through `external_edit`
    /// (deviation 13).
    pub fn paste<B: TextBuf>(&mut self, text: &str, t: Target<'_, B>) -> Outcome {
        let Target { buf, state } = t;
        if self.mode != Mode::Insert {
            return Outcome::Declined { count: None, keys: Vec::new() };
        }
        self.clamp(buf);
        let text = if B::MULTILINE { text.replace("\r\n", "\n").replace('\r', "\n") } else { flatten_paste(text) };
        self.insert_input(InsertKey::Paste(text), buf, state);
        let changed = state.history.take_changed();
        state.edited |= changed;
        self.paint(buf);
        Outcome::Consumed { changed, note: None, request: None }
    }
}
