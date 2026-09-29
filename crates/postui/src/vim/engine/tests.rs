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
