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
use super::history::{Ed, MarkMove, end_of};
use super::keys::{Cmd, InsertHow, Key};
use super::motion::{Want, col_for, vcol_of};
use super::op::{RKind, Range, yank_of};
use super::settings::TABSTOP;
use super::{BufState, Engine, Mode, Outcome, Target, first_non_blank};
use crate::components::line_input::flatten_paste;
use ratatui::crossterm::event::KeyEvent;

/// The `.` record of typing that did not start with an Insert command: a
/// session `enter` or `carry` opened, or typing after a cursor key (Vim's
/// pretend `1i`).
const ONE_I: Cmd = Cmd::Insert { how: InsertHow::Before, count: 1 };

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

impl InsertKey {
    /// The key as a buffer of kind `B` takes it, typed now or replayed by
    /// `.` (a replay acts as if its keys were typed in the target buffer,
    /// spec §4.2). A one-line field has no
    /// Enter or Tab (`None`: typed, they are declined; replayed, dropped)
    /// and flattens a paste (`flatten_paste`); the body takes CRLF and CR
    /// as line breaks.
    fn for_buffer<B: TextBuf>(self) -> Option<Self> {
        match self {
            InsertKey::Enter | InsertKey::Tab if !B::MULTILINE => None,
            InsertKey::Paste(text) if B::MULTILINE => Some(InsertKey::Paste(text.replace("\r\n", "\n").replace('\r', "\n"))),
            InsertKey::Paste(text) => Some(InsertKey::Paste(flatten_paste(&text))),
            key => Some(key),
        }
    }
}

/// An open Insert session (spec §3.8).
#[derive(Debug, Clone)]
pub(crate) struct Session {
    /// Where the current stretch of typing began (Vim's `Insstart`): the
    /// session's start, reset when typing resumes after a cursor key, and
    /// moved up when `BS` joins its line to the one above.
    pub start: Pos,
    /// Where `ctrl+w` and `ctrl+u` stop once (Vim's `Insstart_orig`). It
    /// follows `start` after every key until `orig_fixed`.
    orig: Pos,
    /// Vim's `update_Insstart_orig = FALSE`: typing resumed right of
    /// `orig`'s column after something was already changed, so `orig`
    /// stays where it is for the rest of the session.
    orig_fixed: bool,
    /// Something was changed in this session (Vim's `!ins_need_undo`).
    changed: bool,
    /// A cursor key was used and no change followed yet (Vim's
    /// `arrow_used`).
    arrow_used: bool,
    /// The command that opened the session, for `.`.
    pub origin: Cmd,
    pub typed: Vec<InsertKey>,
    /// The row whose indent `autoindent` added with nothing typed after it
    /// yet (Vim's `did_ai`, which `autoindent` sets even for a zero
    /// indent): Esc, Enter or Up/Down remove that indent.
    pub ai_row: Option<usize>,
    /// Typing resumed after a cursor key: `.` only takes this record if
    /// something is typed (Vim's pretend `1i`).
    pub resumed: bool,
}

impl Session {
    pub(crate) fn new(start: Pos, origin: Cmd) -> Self {
        Self {
            start,
            orig: start,
            orig_fixed: false,
            changed: false,
            arrow_used: false,
            origin,
            typed: Vec::new(),
            ai_row: None,
            resumed: false,
        }
    }

    /// Vim's `stop_arrow()`, run before each change: after a cursor key the
    /// new stretch of typing starts at `caret`, and `orig` freezes when that
    /// is right of it and the session already changed something.
    fn stop_arrow(&mut self, caret: Pos) {
        if self.arrow_used {
            self.start = caret;
            if caret.col > self.orig.col && self.changed {
                self.orig_fixed = true;
            }
            self.arrow_used = false;
        }
        self.changed = true;
    }

    /// The top of Vim's Insert loop, after every key:
    /// `if (update_Insstart_orig) Insstart_orig = Insstart`.
    fn key_done(&mut self) {
        if !self.orig_fixed {
            self.orig = self.start;
        }
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
    /// The open session. Insert mode always has one: `enter`, `carry` and
    /// the Insert commands open it, and only leaving Insert drops it. A
    /// missing one is a bug in those edges; release builds open a stand-in
    /// rather than panic.
    fn session(&mut self) -> &mut Session {
        debug_assert!(self.insert.is_some(), "Insert mode with no open session");
        self.insert.get_or_insert_with(|| Session::new(Pos::default(), ONE_I))
    }

    pub(super) fn open_session(&mut self, at: Pos, origin: Cmd, ai_row: Option<usize>) {
        self.mode = Mode::Insert;
        self.insert = Some(Session { ai_row, ..Session::new(at, origin) });
    }

    /// Typing from the caret on starts a fresh record for `.`, as an `i`
    /// (Vim's pretend `1i`). With no session open (`enter`, `carry`) one
    /// opens at `at`. An open one is split as Vim's `start_arrow()` splits
    /// it (a cursor key, an external edit; its undo step is already
    /// closed): it keeps its `ctrl+w` stop, and the new stretch's start is
    /// set when typing resumes (`stop_arrow()`). `resumed`: `.` takes the
    /// record only once something is typed, and keeps its old value until
    /// then.
    pub(super) fn open_resumed(&mut self, at: Pos, resumed: bool) {
        self.mode = Mode::Insert;
        let s = self.insert.get_or_insert_with(|| Session::new(at, ONE_I));
        s.arrow_used = true;
        s.origin = ONE_I;
        s.typed.clear();
        s.ai_row = None;
        s.resumed = resumed;
        s.key_done();
    }

    /// Typing after a break in the open session (a cursor key, an external
    /// edit) starts a fresh `1i` record.
    pub(super) fn resume_insert(&mut self, at: Pos) {
        debug_assert!(self.insert.is_some(), "only an open session is split");
        self.open_resumed(at, true);
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
                    let opened = MarkMove::InsertLines { at: caret.row + 1, count: 1 };
                    ed.splice_moving(Pos::new(caret.row, len), Pos::new(caret.row, len), &format!("\n{indent}"), opened);
                    Pos::new(caret.row + 1, indent.len())
                } else {
                    let opened = MarkMove::InsertLines { at: caret.row, count: 1 };
                    ed.splice_moving(Pos::new(caret.row, 0), Pos::new(caret.row, 0), &format!("{indent}\n"), opened);
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

    /// `gi` (Vim's `nv_gi_cmd()`): Insert where Insert last ended in this
    /// buffer (`'^`), clamped into the text; with no such place, at the
    /// caret. `.` repeats it as the `i` it becomes.
    pub(super) fn exec_gi<B: TextBuf>(&mut self, count: usize, buf: &mut B, st: &mut BufState) {
        if let Some(at) = st.history.marks.insert {
            let row = at.row.min(buf.line_count() - 1);
            buf.set_cursor(Pos::new(row, at.col.min(buf.line_len(row))));
        }
        self.exec_insert(InsertHow::Before, count, buf, st);
    }

    /// One key in Insert (the Insert key table, spec §4.3). A key the
    /// engine does not take is declined as typed.
    pub(super) fn insert_key<B: TextBuf>(&mut self, ev: KeyEvent, buf: &mut B, st: &mut BufState) -> Outcome {
        let decline = Outcome::Declined { count: None, keys: vec![ev] };
        let input = match Key::of(&ev) {
            Key::Esc if self.insert_only => return decline,
            Key::Esc => {
                self.end_insert(buf, st, true);
                return Outcome::consumed();
            }
            Key::Char(c) => InsertKey::Char(c),
            Key::Enter => InsertKey::Enter,
            Key::Tab => InsertKey::Tab,
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
        let Some(input) = input.for_buffer::<B>() else { return decline };
        self.insert_input(input, buf, st);
        Outcome::consumed()
    }

    /// Replays an Insert session's recorded keys for `.`, as if typed in
    /// this buffer (see [`InsertKey::for_buffer`]): a key the buffer would
    /// decline is dropped and the replay goes on.
    pub(super) fn replay_insert<B: TextBuf>(&mut self, keys: Vec<InsertKey>, buf: &mut B, st: &mut BufState) {
        for key in keys.into_iter().filter_map(InsertKey::for_buffer::<B>) {
            self.insert_input(key, buf, st);
        }
    }

    /// One recorded Insert key: typed now, or replayed by `.`. Every one of
    /// them resets the wanted column (Vim's Insert loop sets
    /// `w_set_curswant` until a cursor key is used).
    pub(super) fn insert_input<B: TextBuf>(&mut self, key: InsertKey, buf: &mut B, st: &mut BufState) {
        st.forget_want();
        // `ins_bs()` runs `stop_arrow()` only once it knows it can delete.
        if !matches!(key, InsertKey::Backspace | InsertKey::CtrlW | InsertKey::CtrlU) {
            let caret = buf.cursor();
            self.session().stop_arrow(caret);
        }
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
        self.session().key_done();
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
        let caret = buf.cursor();
        if self.replace_gui_selection(buf, st) {
            let s = self.session();
            s.stop_arrow(caret);
            s.typed.push(record);
            return;
        }
        // Vim beeps at the start of the buffer, before `stop_arrow()`.
        if caret == Pos::new(0, 0) {
            return;
        }
        self.session().stop_arrow(caret);
        let Session { start, orig, .. } = *self.session();
        let joined = caret.col == 0;
        let to = if joined {
            // `backspace=eol`: join with the line above (no space).
            let prev = buf.line_len(caret.row - 1);
            let to = Pos::new(caret.row - 1, prev);
            // `ins_bs()` and `do_join()` save both lines (`u_save`), so
            // `u` finds the saved caret in a block of two, not one.
            Ed { buf: &mut *buf, hist: &mut st.history }.splice_lines_joined(to, caret);
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
                if mode == Erase::Char || col <= mincol || Pos::new(caret.row, col) == orig {
                    break;
                }
            }
            let to = Pos::new(caret.row, col);
            Ed { buf: &mut *buf, hist: &mut st.history }.splice(to, caret, "");
            to
        };
        buf.set_cursor(to);
        let s = self.session();
        // "If deleted before the insertion point, adjust it" (`ins_bs()`);
        // unless frozen, `key_done` then puts it back on `start`.
        if to.row == s.orig.row && to.col < s.orig.col {
            s.orig.col = to.col;
        }
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
        self.split_insert(c, to, buf, st);
        buf.set_cursor(to);
        match key {
            // `j`/`k` style: the wanted column survives.
            Key::Up | Key::Down => st.set_want(want, to),
            // `ins_end()` wants the end, but a `w_set_curswant` still set
            // from typing overrides it.
            Key::End if st.want(c).is_some() => st.set_want(Want::End, to),
            _ => st.forget_want(),
        }
    }

    /// Vim's `start_arrow()`: the caret went from `from` to `to` in Insert
    /// without typing (a cursor key, a click, an external edit). An unused
    /// autoindent on `from`'s line is stripped when the caret left that
    /// line (`stop_insert()`; `cpoptions` has no `I`), the undo step and the
    /// `.` record end there, and typing at `to` starts a fresh `1i` record.
    /// The caller moves the caret.
    pub(super) fn split_insert<B: TextBuf>(&mut self, from: Pos, to: Pos, buf: &mut B, st: &mut BufState) {
        if to.row != from.row {
            self.strip_autoindent(from, buf, st);
        }
        st.history.commit();
        self.finish_record();
        self.resume_insert(to);
    }

    /// Ends the open session with no `.` record (`carry`): an unused
    /// autoindent is stripped and the session dropped. The caller closes
    /// the undo step and leaves Insert.
    pub(super) fn drop_session<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        let mut at = buf.cursor();
        if let Some(stripped) = self.strip_autoindent(at, buf, st)
            && stripped != at
        {
            buf.set_cursor(stripped);
            at = stripped;
        }
        // `gi` in the buffer left behind resumes here, as after `leave`.
        st.history.marks.insert = Some(at);
        self.insert = None;
    }

    /// Vim's `ins_esc()` for a counted insert (`3iX<Esc>`, `3o…`): what was
    /// typed goes in `count - 1` more times, and each `o`/`O` repeat starts
    /// on a new line (`start_redo_ins()` stuffs a line break first). A
    /// session a cursor key split no longer has its count: its record
    /// restarted as `1i` (Vim's `arrow_used`). `.` keeps the keys typed
    /// once, with the count (Vim's `block_redo`).
    fn repeat_insert<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        let Some(s) = &self.insert else { return };
        let Cmd::Insert { how, count } = s.origin else { return };
        if count < 2 {
            return;
        }
        let typed = s.typed.clone();
        let open = B::MULTILINE && matches!(how, InsertHow::OpenBelow | InsertHow::OpenAbove);
        for _ in 1..count {
            if open {
                self.insert_input(InsertKey::Enter, buf, st);
            }
            self.replay_insert(typed.clone(), buf, st);
        }
        self.session().typed = typed;
    }

    /// Leaves Insert: `Esc` (`step_back`), or `leave`. An indent
    /// `autoindent` added that nothing followed is removed, the session's
    /// undo step closes, and the caret steps back unless at column 0.
    ///
    /// The wanted column follows `ins_esc()`: Insert's loop last set it to
    /// the caret's Insert-mode virtual column (`update_curswant()` before
    /// each key), and `w_set_curswant` is set again only when
    /// `stop_insert()` left the caret's column where it was. When removing
    /// the autoindent moved it, the wanted column stays after the indent
    /// (`o<Esc>k` aims for the indent's width). Esc repeats a counted insert
    /// first (`repeat_insert`).
    pub(super) fn end_insert<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState, step_back: bool) {
        // Esc first repeats a counted insert (Vim's `ins_esc()`); `leave`
        // never does (Vim's `:stopinsert` drops the count).
        if step_back {
            self.repeat_insert(buf, st);
        }
        let mut caret = buf.cursor();
        let temp = caret.col;
        let insert_want = Want::Col(vcol_of(&buf.line(caret.row), caret.col));
        if let Some(at) = self.strip_autoindent(caret, buf, st) {
            caret = at;
        }
        // Vim's `'^` mark, for `gi`: where the caret was when Insert ended,
        // before it steps back.
        st.history.marks.insert = Some(caret);
        let keep_want = caret.col != temp;
        st.history.commit();
        self.finish_record();
        self.insert = None;
        self.mode = Mode::Normal;
        if step_back && caret.col > 0 {
            caret.col -= 1;
        }
        // Only when it moves: a one-line field drops its GUI selection on
        // any `set_cursor`, and a mouse sweep that ends Insert keeps it.
        if caret != buf.cursor() {
            buf.set_cursor(caret);
        }
        if keep_want {
            st.set_want(insert_want, caret);
        } else {
            st.forget_want();
        }
    }

    /// Records the session for `.` when it ends or a cursor key splits it:
    /// its opening command plus the keys typed. After a cursor key
    /// (`resumed`) only once something was typed: Vim's `stop_arrow()`
    /// starts a fresh `1i` record only then, and until then `.` keeps what
    /// the key before it closed.
    ///
    /// A query box (`Start::InsertOnly`: the `:` line, the palette, the jq
    /// bar, pickers, filters) never records: typing on Vim's command line
    /// never touches the redo buffer (spec §4.2's `i` record is for
    /// `Start::Insert` only).
    pub(super) fn finish_record(&mut self) {
        if self.insert_only {
            return;
        }
        let Some(s) = &self.insert else { return };
        if s.resumed && s.typed.is_empty() {
            return;
        }
        let (cmd, typed) = (s.origin, s.typed.clone());
        self.remember(cmd, Some(typed));
    }

    /// The change operator (spec §3.6, Vim's `op_change()`): the text goes
    /// to the register and a session opens at the range start.
    pub(super) fn change<B: TextBuf>(&mut self, r: Range, reg: Option<char>, origin: Cmd, buf: &mut B, st: &mut BufState) {
        self.regs.delete(reg, yank_of(buf, r));
        self.change_text(r, origin, buf, st);
    }

    /// `op_change()` after the yank: deletes `r` and opens the session. A
    /// linewise change keeps the first line's indent (`autoindent`), which
    /// Esc removes if nothing follows. When nothing is deleted (an empty
    /// region, or a buffer with no lines), Vim still saves the line, so `u`
    /// undoes the step.
    pub(super) fn change_text<B: TextBuf>(&mut self, r: Range, origin: Cmd, buf: &mut B, st: &mut BufState) {
        let emptied = st.history.emptied(&*buf);
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
        } else if r.kind == RKind::Line && r.end.row > r.start.row {
            // Vim's `op_change()` deletes the lines after the first
            // (`del_lines()`), then empties the first after its indent.
            ed.splice_moving(from, to, "", MarkMove::DeleteLines { first: r.start.row + 1, last: r.end.row });
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
        let before = buf.cursor();
        if let Some(key) = InsertKey::Paste(text.to_string()).for_buffer::<B>() {
            self.insert_input(key, buf, state);
        }
        let changed = self.finish_key(before, true, buf, state);
        Outcome::Consumed { changed, note: None, request: None }
    }
}
