//! Engine S tests: behaviour with no Vim counterpart to compare with
//! (spec §8.2). Everything Vim can judge lives in the conformance corpus.

use super::*;
use crate::components::line_input::LineInput;
use edtui::{EditorState, Lines};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;

pub(super) fn k(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE })
}

pub(super) fn code(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

pub(super) fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

pub(super) fn esc() -> KeyEvent {
    code(KeyCode::Esc)
}

/// A one-line field with its own engine.
pub(super) struct Field {
    pub input: LineInput,
    pub engine: Engine,
    pub state: BufState,
}

impl Field {
    pub fn new(text: &str, col: usize) -> Self {
        Self::start(text, col, Start::Normal)
    }

    pub fn start(text: &str, col: usize, start: Start) -> Self {
        let mut input = LineInput::new(text);
        input.set_cursor(col);
        let mut f = Self { input, engine: Engine::new(), state: BufState::new() };
        f.engine.enter(start, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
        f
    }

    pub fn key(&mut self, ev: KeyEvent) -> Outcome {
        self.engine.handle(ev, Target { buf: &mut OneLineBuf::new(&mut self.input), state: &mut self.state }, &ViewCtx::default())
    }

    /// Plain chars, one key each; returns the last outcome.
    pub fn keys(&mut self, s: &str) -> Outcome {
        let mut last = Outcome::consumed();
        for c in s.chars() {
            last = self.key(k(c));
        }
        last
    }

    pub fn text(&self) -> &str {
        self.input.text()
    }

    pub fn col(&self) -> usize {
        self.input.cursor()
    }
}

/// A body buffer with its own engine.
pub(super) struct Body {
    pub ed: EditorState,
    pub engine: Engine,
    pub state: BufState,
}

impl Body {
    pub fn new(text: &str, row: usize, col: usize) -> Self {
        let mut ed = EditorState::new(Lines::from(text));
        ed.cursor = edtui::Index2::new(row, col);
        let mut b = Self { ed, engine: Engine::new(), state: BufState::new() };
        b.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state });
        b
    }

    pub fn key(&mut self, ev: KeyEvent) -> Outcome {
        let ctx = ViewCtx { viewport_rows: Some(20) };
        self.engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut self.ed, Style::default()), state: &mut self.state }, &ctx)
    }

    /// Plain chars, one key each; returns the last outcome.
    pub fn keys(&mut self, s: &str) -> Outcome {
        let mut last = Outcome::consumed();
        for c in s.chars() {
            last = self.key(k(c));
        }
        last
    }

    pub fn text(&mut self) -> String {
        BodyBuf::new(&mut self.ed, Style::default()).text()
    }

    pub fn caret(&self) -> Pos {
        Pos::new(self.ed.cursor.row, self.ed.cursor.col)
    }
}

fn declined(out: &Outcome) -> bool {
    matches!(out, Outcome::Declined { .. })
}

#[test]
fn esc_in_normal_is_declined_but_cancels_a_pending_count() {
    let mut f = Field::new("abc", 1);
    assert_eq!(f.key(esc()), Outcome::Declined { count: None, keys: vec![esc()] });
    f.keys("3");
    assert!(f.engine.pending());
    assert!(!declined(&f.key(esc())), "Esc cancels the count first");
    assert!(!f.engine.pending());
    assert!(declined(&f.key(esc())));
}

#[test]
fn a_one_line_field_declines_row_keys_and_the_body_keeps_them() {
    let mut f = Field::new("abc", 1);
    assert_eq!(f.keys("5j"), Outcome::Declined { count: Some(5), keys: vec![k('j')] });
    for key in [k('k'), code(KeyCode::Up), code(KeyCode::Down), code(KeyCode::Enter), code(KeyCode::Tab), code(KeyCode::BackTab)] {
        assert!(declined(&f.key(key)), "{key:?}");
    }
    let mut b = Body::new("a\nb", 0, 0);
    assert!(!declined(&b.key(k('j'))));
    assert!(!declined(&b.key(code(KeyCode::Enter))), "body Enter is consumed until plan 3b's motion");
}

#[test]
fn piece_fours_keys_are_declined_with_their_count() {
    let mut f = Field::new("abc", 0);
    assert_eq!(f.keys("3gt"), Outcome::Declined { count: Some(3), keys: vec![k('g'), k('t')] });
    assert_eq!(f.keys(":"), Outcome::Declined { count: None, keys: vec![k(':')] });
    for c in ['Z', 'q', '@', 'm', '\'', '`'] {
        assert!(declined(&f.key(k(c))), "{c}");
    }
    assert_eq!(f.key(ctrl('o')), Outcome::Declined { count: None, keys: vec![ctrl('o')] });
    f.keys("2d");
    assert_eq!(f.key(ctrl('c')), Outcome::Declined { count: None, keys: vec![ctrl('c')] });
    assert!(!f.engine.pending(), "the chord discards the pending operator");
}

#[test]
fn inert_keys_show_their_note_and_arm_nothing() {
    let mut f = Field::new("abc", 0);
    let note = |out: Outcome| match out {
        Outcome::Consumed { note: Some(Note::Unsupported(n)), .. } => n,
        other => panic!("no note: {other:?}"),
    };
    assert_eq!(note(f.key(k('U'))), "U not supported");
    assert_eq!(note(f.key(k('K'))), "K not supported");
    assert_eq!(note(f.key(k('Q'))), "Q not supported");
    assert_eq!(note(f.key(k('&'))), "& not supported");
    assert_eq!(note(f.keys("gJ")), "gJ not supported");
    assert_eq!(note(f.keys("d:")), "d: not supported");
    assert_eq!(note(f.key(k('/'))), "/ not supported yet");
    assert_eq!(note(f.keys("\"a")), "register \"a not supported");
    assert!(!f.engine.pending());
    assert_eq!(f.text(), "abc");
}

#[test]
fn echo_and_pending_follow_the_half_typed_command() {
    let mut f = Field::new("abc", 0);
    for (keys, echo) in [("3", "3"), ("2d", "2d"), ("d3", "d3"), ("di", "di"), ("f", "f"), ("r", "r")] {
        f.keys(keys);
        assert_eq!(f.engine.echo(), echo, "{keys:?}");
        assert!(f.engine.pending());
        f.key(esc());
        assert!(!f.engine.pending());
        assert_eq!(f.engine.echo(), "");
    }
}

/// Vim: `<Del>` while a count is typed drops its last digit; the count is
/// still pending, and `<Del>` with none typed is `x`.
#[test]
fn del_edits_a_typed_count_instead_of_being_x() {
    let mut f = Field::new("abc", 0);
    f.keys("34");
    assert!(!declined(&f.key(code(KeyCode::Delete))));
    assert_eq!(f.engine.echo(), "3");
    f.key(code(KeyCode::Delete));
    assert!(!f.engine.pending(), "the count is gone");
    assert!(!declined(&f.key(code(KeyCode::Delete))), "with no count typed, Delete is x, not a decline");
    assert!(!f.engine.pending());
}

#[test]
fn modes_start_where_enter_says() {
    assert_eq!(Field::new("abc", 0).engine.mode(), Mode::Normal);
    assert_eq!(Field::start("abc", 0, Start::Insert).engine.mode(), Mode::Insert);
    assert_eq!(Field::start("abc", 0, Start::InsertOnly).engine.mode(), Mode::Insert);
}

/// Review focus 2: a change outside the engine leaves the caret past the
/// text; the next key must clamp, not panic.
#[test]
fn a_stale_caret_is_clamped_before_any_key() {
    let mut b = Body::new("one\ntwo\nthree", 2, 4);
    b.ed.lines = Lines::from("x");
    for key in ['l', 'x', 'j', 'k', 'w', 'b', 'e', '$', 'p', 'u', 'd'] {
        b.key(k(key));
    }
    b.key(esc());
    assert!(b.caret().row == 0 && b.caret().col <= 1);
}

#[test]
fn u_is_declined_once_the_history_is_empty() {
    let mut f = Field::new("abc", 0);
    assert_eq!(f.keys("u"), Outcome::Declined { count: None, keys: vec![k('u')] });
    assert_eq!(f.keys("3u"), Outcome::Declined { count: Some(3), keys: vec![k('u')] });
    assert_eq!(f.key(ctrl('r')), Outcome::Declined { count: None, keys: vec![ctrl('r')] });
    f.keys("x");
    assert!(f.state.can_undo() && !f.state.can_redo());
    assert!(!declined(&f.keys("u")));
    assert_eq!(f.text(), "abc");
    assert!(f.state.can_redo());
    assert!(declined(&f.keys("u")), "the app history takes the next u");
    assert!(!declined(&f.key(ctrl('r'))));
    assert_eq!(f.text(), "bc");
}

/// Review probe P1: Vim records `dd` on a blank field as a step, so the
/// next `u` is the engine's, not the app history's.
#[test]
fn dd_on_a_blank_field_is_undone_by_the_engine() {
    let mut f = Field::new("", 0);
    assert_eq!(f.keys("dd"), Outcome::Consumed { changed: false, note: None, request: None });
    assert!(!declined(&f.keys("u")));
    assert!(declined(&f.keys("u")), "then the history is empty");
}

/// A text object that fails cancels its operator: no change and no undo
/// step, so the next `u` is the app history's. The caret still rests where
/// Vim's word walk stopped (the corpus checks the column).
#[test]
fn a_failed_object_records_no_step() {
    let mut f = Field::new("foo.bar(baz, qux);", 0);
    assert_eq!(f.keys("d3aW"), Outcome::Consumed { changed: false, note: None, request: None });
    assert_eq!((f.text(), f.input.cursor()), ("foo.bar(baz, qux);", 17));
    assert!(declined(&f.keys("u")));
    let mut b = Body::new("{\n  x\n}", 1, 2);
    b.keys("di(");
    assert_eq!(b.text(), "{\n  x\n}");
    assert!(!b.state.can_undo());
}

#[test]
fn changed_reports_a_text_change_and_edited_remembers_it() {
    let mut f = Field::new("abc", 0);
    assert_eq!(f.keys("l"), Outcome::Consumed { changed: false, note: None, request: None });
    assert!(!f.state.edited());
    assert_eq!(f.keys("x"), Outcome::Consumed { changed: true, note: None, request: None });
    assert!(f.state.edited());
    assert_eq!(f.keys("yw"), Outcome::Consumed { changed: false, note: None, request: None });
}

#[test]
fn every_edit_goes_through_one_splice_path_and_one_step() {
    let mut b = Body::new("one two three\nfour", 0, 0);
    b.keys("d2w");
    assert_eq!(b.text(), "three\nfour");
    assert_eq!(b.state.history.len(), 1);
    b.keys("dd");
    assert_eq!(b.state.history.len(), 2);
    b.keys("uu");
    assert_eq!(b.text(), "one two three\nfour");
}

#[test]
fn start_insert_types_and_esc_steps_back() {
    let mut f = Field::start("abc", 3, Start::Insert);
    f.keys("xy");
    assert_eq!(f.text(), "abcxy");
    assert_eq!(f.col(), 5);
    f.key(esc());
    assert_eq!(f.engine.mode(), Mode::Normal);
    assert_eq!(f.col(), 4);
    assert!(declined(&f.key(esc())), "the second Esc goes to piece 4");
}

/// Spec §5 and §4.3: `o`/`O` in a one-line field are consumed with no
/// effect (not declined): no text, no mode change, no undo step, caret kept.
#[test]
fn a_one_line_field_has_no_second_line() {
    for keys in ["o", "O"] {
        let mut f = Field::new("abc", 1);
        assert_eq!(f.keys(keys), Outcome::Consumed { changed: false, note: None, request: None }, "{keys}");
        assert_eq!((f.text(), f.engine.mode(), f.col()), ("abc", Mode::Normal, 1), "{keys}");
        assert!(!f.state.can_undo(), "{keys}: no undo step");
    }
}

#[test]
fn one_line_insert_declines_the_keys_the_field_does_not_own() {
    let mut f = Field::start("abc", 3, Start::Insert);
    for key in [code(KeyCode::Enter), code(KeyCode::Tab), code(KeyCode::BackTab), code(KeyCode::Up), code(KeyCode::Down), ctrl('o')] {
        assert!(declined(&f.key(key)), "{key:?}");
        assert_eq!((f.text(), f.engine.mode()), ("abc", Mode::Insert));
    }
}

#[test]
fn insert_only_declines_esc() {
    let mut f = Field::start("q", 1, Start::InsertOnly);
    assert!(declined(&f.key(esc())));
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.keys("x");
    assert_eq!(f.text(), "qx");
}

#[test]
fn paste_in_insert_is_part_of_the_session() {
    let mut f = Field::new("ab", 0);
    f.keys("a");
    let out = f.engine.paste("XY", Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(out, Outcome::Consumed { changed: true, note: None, request: None });
    f.keys("Z");
    f.key(esc());
    assert_eq!(f.text(), "aXYZb");
    f.keys("u");
    assert_eq!(f.text(), "ab", "the paste and the typing are one step");
}

/// Review focus 4: CRLF, CR and tab in pasted text.
#[test]
fn pasted_line_breaks_flatten_in_a_field_and_split_in_the_body() {
    let mut f = Field::start("", 0, Start::Insert);
    f.engine.paste("a\r\nb\tc", Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(f.text(), "a b c");
    let mut f = Field::start("", 0, Start::Insert);
    f.engine.paste("a\rb\r\n\r\nc", Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(f.text(), "a b c", "a run of breaks is one space");
    let mut b = Body::new("", 0, 0);
    b.keys("i");
    b.engine.paste("x\r\ny\rz", Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state });
    assert_eq!(b.text(), "x\ny\nz");
    assert_eq!(b.caret(), Pos::new(2, 1));
    b.engine.paste("\tw", Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state });
    assert_eq!(b.text(), "x\ny\nz\tw", "a pasted tab stays a tab in the body (no expandtab)");
}

#[test]
fn paste_in_normal_is_declined_for_piece_four() {
    let mut f = Field::new("ab", 0);
    let out = f.engine.paste("X", Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(out, Outcome::Declined { count: None, keys: vec![] });
    assert_eq!(f.text(), "ab");
}

#[test]
fn a_cursor_key_in_insert_splits_the_undo_step() {
    let mut f = Field::new("", 0);
    f.keys("iab");
    f.key(code(KeyCode::Left));
    f.keys("X");
    f.key(esc());
    assert_eq!(f.text(), "aXb");
    f.keys("u");
    assert_eq!(f.text(), "ab");
    f.keys("u");
    assert_eq!(f.text(), "");
}
