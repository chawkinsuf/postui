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
use super::register::RegKind;
use super::settings::TABSTOP;
use super::{BufState, Engine, Mode, Note, Outcome, Restart, Target, first_non_blank};
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
    CtrlT,
    CtrlD,
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

/// Insert `ctrl+r` waiting for its register name (Vim's `ins_reg()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegPending {
    /// `ctrl+r`: the text goes in as typed.
    Typed,
    /// `ctrl+r ctrl+r`: control chars go in literally.
    Literal,
    /// `ctrl+r ctrl+o` or `ctrl+r ctrl+p`: Vim puts the register instead.
    /// Not supported; the register name is taken with a note.
    Unsupported(char),
}

impl RegPending {
    /// The footer echo, as Vim's showcmd shows it.
    pub(crate) fn echo(self) -> String {
        match self {
            RegPending::Typed => "^R".to_string(),
            RegPending::Literal => "^R^R".to_string(),
            RegPending::Unsupported(c) => format!("^R^{}", c.to_ascii_uppercase()),
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
    /// `Some` in a Replace session: Vim's replace stack. `None` is a NUL
    /// marker (a char typed past the line's end, a line break, an indent
    /// `autoindent` added: nothing to put back); `Some(c)` is the char a
    /// typed char overwrote, pushed after its marker. `BS` pops it.
    pub replace: Option<Vec<Option<char>>>,
    /// The key typed before this one in the session (Vim's `lastc`): `0`
    /// or `^` before `ctrl+d` removes the whole indent.
    pub last_key: Option<InsertKey>,
    /// The indent `^<C-d>` removed, given to the next Enter's line (Vim's
    /// `old_indent`).
    pub old_indent: Option<usize>,
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
            replace: None,
            last_key: None,
            old_indent: None,
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
    pub(super) fn session(&mut self) -> &mut Session {
        debug_assert!(self.insert.is_some(), "Insert mode with no open session");
        self.insert.get_or_insert_with(|| Session::new(Pos::default(), ONE_I))
    }

    pub(super) fn open_session(&mut self, at: Pos, origin: Cmd, ai_row: Option<usize>) {
        // A nested insert replaces a `ctrl+o` restart (Vim's `edit()` clears it).
        self.restart = None;
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
        // A split Replace session stays Replace.
        if !self.in_insert() {
            self.mode = Mode::Insert;
        }
        let s = self.insert.get_or_insert_with(|| Session::new(at, ONE_I));
        s.arrow_used = true;
        s.origin = ONE_I;
        s.typed.clear();
        s.ai_row = None;
        s.resumed = resumed;
        s.last_key = None;
        // A cursor key ends the stretch (`stop_insert()` → `replace_flush()`):
        // the stack empties, the session stays Replace.
        if let Some(stack) = &mut s.replace {
            stack.clear();
        }
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
            InsertHow::Replace => caret,
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
        if how == InsertHow::Replace {
            self.session().replace = Some(Vec::new());
            self.mode = Mode::Replace;
        }
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
        // Vim's `lastc` is every key of the Insert loop: one taken here
        // without `insert_input` (`ctrl+r`, its register name, a cursor
        // key) leaves no `0` or `^` before the next `ctrl+d`. A register's
        // text sets it again as it is typed.
        if let Some(how) = self.reg_pending.take() {
            self.session().last_key = None;
            return self.insert_register(how, ev, buf, st);
        }
        let decline = Outcome::Declined { count: None, keys: vec![ev] };
        let input = match Key::of(&ev) {
            Key::Esc if self.insert_only => return decline,
            Key::Esc => {
                self.end_insert(buf, st, true);
                return Outcome::consumed();
            }
            Key::Ctrl('o') if !self.insert_only => {
                self.ctrl_o(buf, st);
                return Outcome::consumed();
            }
            Key::Ctrl('r') => {
                self.session().last_key = None;
                self.reg_pending = Some(RegPending::Typed);
                return Outcome::consumed();
            }
            Key::Ctrl('t') => InsertKey::CtrlT,
            Key::Ctrl('d') => InsertKey::CtrlD,
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

    /// The key after Insert `ctrl+r` (Vim's `ins_reg()`). `"` and `0` put
    /// their text in; any other register name shows a note. `ctrl+r` again
    /// makes it literal; `ctrl+o` and `ctrl+p` are not supported and take
    /// one more key, whatever it is. The engine's own Insert chords are
    /// swallowed (a third `ctrl+r` too: Vim beeps at it as a register
    /// name), a foreign chord goes to the app (the half-typed `ctrl+r` is
    /// dropped), and any other key is swallowed, as Vim swallows it (plan
    /// 3b Deviation 5).
    fn insert_register<B: TextBuf>(&mut self, how: RegPending, ev: KeyEvent, buf: &mut B, st: &mut BufState) -> Outcome {
        let note = |text: String| Outcome::Consumed { changed: false, note: Some(Note::Unsupported(text)), request: None };
        match (how, Key::of(&ev)) {
            (RegPending::Unsupported(c), _) => note(format!("ctrl+r ctrl+{c} not supported")),
            (RegPending::Typed, Key::Ctrl('r')) => {
                self.reg_pending = Some(RegPending::Literal);
                Outcome::consumed()
            }
            (RegPending::Typed, Key::Ctrl(c @ ('o' | 'p'))) => {
                self.reg_pending = Some(RegPending::Unsupported(c));
                Outcome::consumed()
            }
            (_, Key::Ctrl('w' | 'u' | 'h' | 'r' | 't' | 'd')) => Outcome::consumed(),
            (_, Key::Ctrl(_) | Key::Other) => Outcome::Declined { count: None, keys: vec![ev] },
            (how, Key::Char(name @ ('"' | '0'))) => {
                self.insert_register_text(name, how == RegPending::Literal, buf, st);
                Outcome::consumed()
            }
            (_, Key::Char(c)) => note(format!("register \"{c} not supported")),
            _ => Outcome::consumed(),
        }
    }

    /// Types register `name`'s text into the session (Vim's `insert_reg()`
    /// and `stuffescaped()`). In the body each line break is an Enter
    /// (`autoindent` applies), a linewise register ends with one, and Tab,
    /// BS, `ctrl+w`, `ctrl+u`, `ctrl+t` and `ctrl+d` act as those keys, all
    /// but Tab going in literally when `literal`. Any other char goes in as
    /// it is, a run of them between those keys as one typed run (recorded
    /// as one, so `.` and a count put it in the same way), not a key per
    /// char, each of which would rebuild the line. A one-line field takes the text
    /// flattened, as `p` puts it (key list §5): with no line break or Tab
    /// left, it is one run.
    fn insert_register_text<B: TextBuf>(&mut self, name: char, literal: bool, buf: &mut B, st: &mut BufState) {
        let reg = self.regs.read(Some(name)).clone();
        if !B::MULTILINE {
            let text = match reg.kind {
                RegKind::Line => reg.text.strip_suffix('\n').unwrap_or(&reg.text),
                RegKind::Char => &reg.text,
            };
            let flat = flatten_paste(text);
            if !flat.is_empty() {
                self.insert_input(InsertKey::Paste(flat), buf, st);
            }
            return;
        }
        let mut run = String::new();
        for c in reg.text.chars() {
            let key = match c {
                '\n' | '\r' => InsertKey::Enter,
                '\t' => InsertKey::Tab,
                '\u{8}' if !literal => InsertKey::Backspace,
                '\u{17}' if !literal => InsertKey::CtrlW,
                '\u{15}' if !literal => InsertKey::CtrlU,
                '\u{14}' if !literal => InsertKey::CtrlT,
                '\u{4}' if !literal => InsertKey::CtrlD,
                c => {
                    run.push(c);
                    continue;
                }
            };
            self.flush_run(&mut run, buf, st);
            self.insert_input(key, buf, st);
        }
        self.flush_run(&mut run, buf, st);
    }

    /// Types `run` (a register's chars between its key-like ones) as one
    /// typed run and empties it; one char goes in as that char, as typed.
    fn flush_run<B: TextBuf>(&mut self, run: &mut String, buf: &mut B, st: &mut BufState) {
        let key = match run.chars().count() {
            0 => return,
            1 => InsertKey::Char(run.chars().next().unwrap()),
            _ => InsertKey::Paste(std::mem::take(run)),
        };
        run.clear();
        self.insert_input(key, buf, st);
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
        // Vim's `lastc` for the next key: a typed run counts as its last char.
        let last_key = match &key {
            InsertKey::Paste(text) => text.chars().last().map_or(InsertKey::Paste(String::new()), InsertKey::Char),
            other => other.clone(),
        };
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
                // In Replace the first space replaces a char (`ins_char()`)
                // and the rest are inserted, a marker each (`ins_str()`).
                let caret = buf.cursor();
                let v = vcol_of(&buf.line(caret.row), caret.col);
                let width = TABSTOP - v % TABSTOP;
                if self.session().replace.is_some() {
                    self.overwrite(" ", buf, st);
                    let caret = buf.cursor();
                    let rest = " ".repeat(width - 1);
                    Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, caret, &rest);
                    buf.set_cursor(Pos::new(caret.row, caret.col + width - 1));
                    let s = self.session();
                    s.replace.as_mut().expect("a Replace session").extend(std::iter::repeat_n(None, width - 1));
                    s.typed.push(InsertKey::Tab);
                    s.ai_row = None;
                } else {
                    self.type_text(&" ".repeat(width), InsertKey::Tab, buf, st);
                }
            }
            InsertKey::CtrlT => {
                self.ins_shift(false, buf, st);
                self.session().typed.push(InsertKey::CtrlT);
            }
            InsertKey::CtrlD => {
                self.ins_shift(true, buf, st);
                self.session().typed.push(InsertKey::CtrlD);
            }
            InsertKey::Backspace => self.backspace(Erase::Char, buf, st),
            InsertKey::CtrlW => self.backspace(Erase::Word, buf, st),
            InsertKey::CtrlU => self.backspace(Erase::Line, buf, st),
            InsertKey::Delete => self.delete_forward(buf, st),
        }
        let s = self.session();
        s.last_key = Some(last_key);
        s.key_done();
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
        if self.session().replace.is_some() {
            // Replace: each line of the text overwrites from the caret
            // (`ins_char_bytes()` per char, one splice per line). Only a
            // paste holds a line break (a register's go through `newline`
            // as Enter), and Vim pastes with `paste` set: the break adds no
            // indent and strips no blanks, so it pushes only `ins_eol()`'s
            // marker and `open_line()`'s end-of-blanks one.
            let mut first = true;
            for seg in text.split('\n') {
                if !first {
                    let caret = buf.cursor();
                    self.session().replace.as_mut().expect("a Replace session").extend([None, None]);
                    Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, caret, "\n");
                    buf.set_cursor(Pos::new(caret.row + 1, 0));
                }
                first = false;
                if !seg.is_empty() {
                    self.overwrite(seg, buf, st);
                }
            }
            let s = self.session();
            s.typed.push(record);
            s.ai_row = None;
            return;
        }
        let caret = buf.cursor();
        Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, caret, text);
        buf.set_cursor(end_of(caret, text));
        let s = self.session();
        s.typed.push(record);
        s.ai_row = None;
    }

    /// Replace mode's `ins_char_bytes()` for a run of chars with no line
    /// break: the chars under the caret are overwritten (each pushed after
    /// its NUL marker), the rest appended (a marker each). One splice.
    fn overwrite<B: TextBuf>(&mut self, text: &str, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let line = buf.line(caret.row).into_owned();
        let n = text.chars().count();
        let covered = n.min(line.len().saturating_sub(caret.col));
        let stack = self.session().replace.as_mut().expect("a Replace session");
        for i in 0..n {
            stack.push(None);
            if i < covered {
                stack.push(Some(line[caret.col + i]));
            }
        }
        let mut ed = Ed { buf: &mut *buf, hist: &mut st.history };
        if covered == n && line[caret.col..caret.col + n].iter().copied().eq(text.chars()) {
            // Each char over itself: no text changes, but Vim's
            // `u_save_cursor()` still opens the undo step at the caret.
            ed.save_cursor_line(caret);
        } else {
            ed.splice(caret, Pos::new(caret.row, caret.col + covered), text);
        }
        buf.set_cursor(Pos::new(caret.row, caret.col + n));
    }

    /// Vim's `replace_do_bs()` after the caret stepped back onto a char:
    /// `Some(c)`: the typed char goes and `c` comes back (plus any more
    /// chars down to the next marker, which is consumed, each put in before
    /// the ones already back: `replace_pop_ins()`); `None`: the char was
    /// typed over nothing, so it goes; an empty stack: nothing.
    fn replace_bs<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let stack = self.session().replace.as_mut().expect("a Replace session");
        match stack.pop() {
            Some(Some(c)) => {
                let mut back = String::from(c);
                while let Some(Some(more)) = stack.last() {
                    back.insert(0, *more);
                    stack.pop();
                }
                stack.pop(); // the marker
                let end = Pos::new(caret.row, (caret.col + 1).min(buf.line_len(caret.row)));
                Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, end, &back);
            }
            Some(None) => {
                let end = Pos::new(caret.row, (caret.col + 1).min(buf.line_len(caret.row)));
                Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, end, "");
            }
            None => {}
        }
        buf.set_cursor(caret);
    }

    /// Vim's `replace_pop_ins()` at the caret: the chars on the stack down
    /// to the next marker come back after the caret, the marker is consumed.
    pub(super) fn replace_restore_at<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        let caret = buf.cursor();
        let stack = self.session().replace.as_mut().expect("a Replace session");
        let mut back = String::new();
        while let Some(Some(c)) = stack.last().copied() {
            stack.pop();
            back.push(c);
        }
        if matches!(stack.last(), Some(None)) {
            stack.pop();
        }
        if !back.is_empty() {
            Ed { buf: &mut *buf, hist: &mut st.history }.splice(caret, caret, &back);
            buf.set_cursor(caret);
        }
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
        let mut indent = indent_before(&line, caret.col);
        // `ins_eol()` clears `old_indent` after every break, used or not.
        if let Some(old) = self.session().old_indent.take()
            && indent.is_empty()
        {
            // `^<C-d>` on the line above: `open_line(…, second_line_indent)`.
            indent = " ".repeat(old);
        }
        let mut cut = caret.col;
        if self.session().ai_row == Some(caret.row) {
            while cut > 0 && white(line[cut - 1]) {
                cut -= 1;
            }
        }
        let rest = caret.col + line[caret.col..].iter().take_while(|&&c| white(c)).count();
        if let Some(stack) = self.session().replace.as_mut() {
            // `ins_eol()`: the break replaces nothing; `open_line()`: a
            // marker ending the stripped blanks, the blanks themselves, and
            // a marker per char of the new line's indent.
            stack.push(None);
            stack.push(None);
            stack.extend(line[caret.col..rest].iter().map(|&c| Some(c)));
            stack.extend(std::iter::repeat_n(None, indent.len()));
        }
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
            let prev = buf.line_len(caret.row - 1);
            let to = Pos::new(caret.row - 1, prev);
            if let Some(stack) = self.session().replace.as_mut() {
                // `ins_bs()` in Replace: the top entry says what the break
                // covered. On the session's start line the caret only moves,
                // but Vim still saves both lines (`u_save`), so `u` comes
                // back here, and the session's start moves up with it (the
                // tail below).
                let top = stack.pop();
                if caret.row <= start.row {
                    if caret.row == start.row {
                        Ed { buf: &mut *buf, hist: &mut st.history }.save_rows(caret, caret.row - 1, caret.row);
                    }
                } else {
                    Ed { buf: &mut *buf, hist: &mut st.history }.splice_lines_joined(to, caret);
                    // Then the blanks `autoindent` stripped come back after
                    // the caret, and the break's own marker is consumed. Vim
                    // puts each popped blank in at the caret, before the ones
                    // already back, so they return in their old order.
                    let mut back = String::new();
                    let mut top = top;
                    while let Some(Some(c)) = top {
                        back.insert(0, c);
                        top = stack.pop();
                    }
                    if !back.is_empty() {
                        Ed { buf: &mut *buf, hist: &mut st.history }.splice(to, to, &back);
                    }
                    while let Some(Some(c)) = stack.last().copied() {
                        // Vim's `replace_pop_ins()`: anything left before the next marker.
                        stack.pop();
                        Ed { buf: &mut *buf, hist: &mut st.history }.splice(to, to, &c.to_string());
                    }
                    if matches!(stack.last(), Some(None)) {
                        stack.pop();
                    }
                }
            } else {
                // `backspace=eol`: join with the line above (no space).
                // `ins_bs()` and `do_join()` save both lines (`u_save`), so
                // `u` finds the saved caret in a block of two, not one.
                Ed { buf: &mut *buf, hist: &mut st.history }.splice_lines_joined(to, caret);
            }
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
            if self.session().replace.is_some() {
                // Replace: one `replace_do_bs()` per char stepped over. A
                // `BS` that only moves (an empty stack) changes nothing, but
                // `stop_arrow()` has saved the line at the caret first
                // (`u_save_cursor()`), so `u` comes back here.
                if !st.history.is_open() {
                    Ed { buf: &mut *buf, hist: &mut st.history }.save_cursor_line(caret);
                }
                let mut at = caret;
                while at.col > col {
                    at.col -= 1;
                    buf.set_cursor(at);
                    self.replace_bs(buf, st);
                }
            } else {
                Ed { buf: &mut *buf, hist: &mut st.history }.splice(to, caret, "");
            }
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
    /// the first line) only beeps in Vim, so nothing ends, but it is still
    /// the key before the next one (`lastc`).
    fn insert_arrow<B: TextBuf>(&mut self, key: Key, buf: &mut B, st: &mut BufState) {
        self.session().last_key = None;
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
        self.reg_pending = None;
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

    /// Insert `ctrl+o` (Vim's `ins_ctrl_o()` then `ins_esc()` with no count):
    /// the session ends as on Esc, except that the caret steps back only at
    /// a line's end, the wanted column stays the Insert caret's, and the
    /// restart is noted so one Normal command runs before Insert resumes.
    pub(super) fn ctrl_o<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        let replace = self.session().replace.is_some();
        let arrow_used = {
            let s = self.session();
            s.resumed && s.typed.is_empty()
        };
        let mut caret = buf.cursor();
        let at_eol = caret.col >= buf.line_len(caret.row);
        let want = self.want_at(buf, st, caret);
        if !arrow_used
            && let Some(at) = self.strip_autoindent(caret, buf, st)
        {
            caret = at;
        }
        st.history.marks.insert = Some(caret);
        st.history.commit();
        self.finish_record();
        self.insert = None;
        self.reg_pending = None;
        if caret.col > 0 && caret.col >= buf.line_len(caret.row) {
            caret.col -= 1;
        }
        buf.set_cursor(caret);
        st.set_want(want, caret);
        self.mode = Mode::InsertNormal { replace };
        self.restart = Some(Restart { replace, at_eol, row: caret.row, old_redo: !arrow_used });
    }

    /// Vim's `edit()` restarting after `ctrl+o`: the caret goes past the
    /// line's end when the Insert caret was there before and the line is
    /// the same, or when the wanted column is right of it; a wanted column a
    /// Normal command set is recomputed under Insert's tab rule; typing
    /// starts a fresh record and a fresh undo step.
    pub(super) fn resume_after_ctrl_o<B: TextBuf>(&mut self, buf: &mut B, st: &mut BufState) {
        let Some(r) = self.restart.take() else { return };
        self.mode = if r.replace { Mode::Replace } else { Mode::Insert };
        let mut caret = buf.cursor();
        let line = buf.line(caret.row);
        let want = self.want_at(buf, st, caret);
        let vcol = vcol_of(&line, caret.col);
        let past = match want {
            Want::End => true,
            Want::Col(w) => w > vcol,
        };
        if ((r.at_eol && caret.row == r.row) || past) && caret.col + 1 == line.len() {
            caret.col += 1;
            buf.set_cursor(caret);
        }
        st.set_want(want, caret);
        self.open_resumed(caret, true);
        if r.replace {
            self.session().replace = Some(Vec::new());
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
        if matches!(self.mode, Mode::InsertNormal { .. }) && !self.pending() {
            self.resume_after_ctrl_o(buf, state);
        }
        if !self.in_insert() {
            return Outcome::Declined { count: None, keys: Vec::new() };
        }
        self.reg_pending = None;
        self.clamp(buf);
        let before = buf.cursor();
        if let Some(key) = InsertKey::Paste(text.to_string()).for_buffer::<B>() {
            self.insert_input(key, buf, state);
        }
        let changed = self.finish_key(before, true, buf, state);
        Outcome::Consumed { changed, note: None, request: None }
    }
}
