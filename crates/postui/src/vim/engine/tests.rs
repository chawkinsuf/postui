//! Engine S tests: behaviour with no Vim counterpart to compare with
//! (spec §8.2). Everything Vim can judge lives in the conformance corpus.

use super::*;
use crate::components::line_input::LineInput;
use edtui::{EditorMode, EditorState, Lines};
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

/// Review focus 2: a change outside the engine (reload, format, app undo)
/// leaves the caret past the text. `handle` clamps it for the mode before
/// the command runs, so the command acts on the last char of the last line
/// (clamping after would act on nothing and then land there), and never
/// panics.
#[test]
fn a_stale_caret_is_clamped_before_any_key() {
    let stale = || {
        let mut b = Body::new("one\ntwo\nthree", 2, 4);
        b.ed.lines = Lines::from("abc\ndef");
        b
    };
    let mut b = stale();
    b.keys("x");
    assert_eq!((b.text(), b.caret()), ("abc\nde".to_string(), Pos::new(1, 1)), "x takes the clamped caret's char");
    let mut b = stale();
    b.keys("iZ");
    b.key(esc());
    assert_eq!((b.text(), b.caret()), ("abc\ndeZf".to_string(), Pos::new(1, 2)), "i opens before the last char, as in Normal");
    let mut b = stale();
    b.keys("yl");
    assert_eq!(b.engine.registers().unnamed().text, "f");
    let mut b = stale();
    b.ed.lines = Lines::from("x");
    for key in ['l', 'x', 'j', 'k', 'w', 'b', 'e', '$', 'p', 'u', 'd'] {
        b.key(k(key));
        let (caret, len) = (b.caret(), b.text().chars().count());
        assert!(caret.row == 0 && caret.col <= len.saturating_sub(1), "{key}: {caret:?} rests on a char");
    }
    b.key(esc());
    assert_eq!(b.caret().row, 0);
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
fn one_line_put_and_join_rules() {
    let mut f = Field::new("ab", 0);
    f.keys("J");
    assert_eq!(f.text(), "ab", "J has no second line to join");
    // Vim's `3J` on its last line joins that line alone and goes to column
    // 0; the field's `J` stays a no-op whatever the count (spec §5).
    let mut f = Field::new("ab", 1);
    f.keys("3J");
    assert_eq!((f.text(), f.col(), f.state.can_undo()), ("ab", 1, false), "3J is a no-op too");

    let mut f = Field::new("ab", 0);
    f.engine.registers_mut().set_unnamed(Register { text: "q\n".into(), kind: RegKind::Line });
    f.keys("p");
    assert_eq!((f.text(), f.col()), ("aqb", 1), "a linewise register goes in charwise");

    let mut f = Field::new("ab", 1);
    f.engine.registers_mut().set_unnamed(Register { text: "x\ny".into(), kind: RegKind::Char });
    f.keys("P");
    assert_eq!(f.text(), "ax yb", "line breaks become spaces");

    let mut f = Field::new("  ab", 3);
    f.keys("yy");
    assert_eq!(f.engine.registers().unnamed(), &Register { text: "  ab\n".into(), kind: RegKind::Line });
    f.keys("dd");
    assert_eq!(f.text(), "", "dd takes the whole field");
}

/// Vim's `do_join()` decides the space from the previous line's text after
/// its leading blanks, not from the text joined so far, so a line ending in
/// a blank followed by an empty line still gets a space before the next.
/// Vim 9.1 probe: `3J` on ['a ', '', 'b'] gives 'a  b' at 1:3, on
/// ['a', '  ', 'b'] 'a b' at 1:2. No corpus text has these lines.
#[test]
fn join_spaces_follow_the_previous_line_not_the_joined_text() {
    let mut b = Body::new("a \n\nb", 0, 0);
    b.keys("3J");
    assert_eq!((b.text(), b.caret()), ("a  b".to_string(), Pos::new(0, 2)));
    let mut b = Body::new("a\n  \nb", 0, 0);
    b.keys("3J");
    assert_eq!((b.text(), b.caret()), ("a b".to_string(), Pos::new(0, 1)));
}

#[test]
fn one_register_is_shared_by_every_buffer() {
    let mut engine = Engine::new();
    let mut state = BufState::new();
    let mut field = LineInput::new("one two");
    field.set_cursor(0);
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state });
    for c in "yiw".chars() {
        engine.handle(k(c), Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state }, &ViewCtx::default());
    }
    let mut ed = EditorState::new(Lines::from("x"));
    let mut body_state = BufState::new();
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body_state });
    engine.handle(k('p'), Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body_state }, &ViewCtx::default());
    assert_eq!(BodyBuf::new(&mut ed, Style::default()).text(), "xone");
}

#[test]
fn visual_modes_track_their_anchor_and_paint_it() {
    let mut f = Field::new("abcdef", 1);
    f.keys("v");
    assert_eq!(f.engine.mode(), Mode::Visual(Shape::Char));
    assert_eq!(f.engine.visual_anchor(), Some(Pos::new(0, 1)));
    f.keys("ll");
    assert_eq!(f.input.paint_span(), Some((1, 4)), "the paint is what `d` would take");
    f.keys("o");
    assert_eq!((f.engine.visual_anchor(), f.col()), (Some(Pos::new(0, 3)), 1));
    f.keys("V");
    assert_eq!(f.engine.mode(), Mode::Visual(Shape::Line));
    f.keys("V");
    assert_eq!((f.engine.mode(), f.engine.visual_anchor()), (Mode::Normal, None));
    assert_eq!(f.input.paint_span(), None);
}

#[test]
fn one_line_visual_j_and_k_are_failed_motions_not_declines() {
    let mut f = Field::new("abc", 1);
    f.keys("vl");
    assert!(!declined(&f.keys("j")));
    assert!(!declined(&f.keys("k")));
    assert_eq!((f.engine.mode(), f.col()), (Mode::Visual(Shape::Char), 2));
}

#[test]
fn capital_v_y_then_p_is_a_characterwise_put_in_a_field() {
    let mut f = Field::new("ab", 1);
    f.keys("Vy");
    assert_eq!(f.engine.registers().unnamed().kind, RegKind::Line);
    f.keys("p");
    assert_eq!(f.text(), "aabb");
}

#[test]
fn visual_p_swaps_the_register_and_capital_p_keeps_it() {
    let mut f = Field::new("one two", 0);
    f.keys("yiwwviwp");
    assert_eq!(f.text(), "one one");
    assert_eq!(f.engine.registers().unnamed().text, "two");
    let mut f = Field::new("one two", 0);
    f.keys("yiwwviwP");
    assert_eq!(f.engine.registers().unnamed().text, "one");
}

/// `BodyBuf::show` paints a linewise selection on an empty row as
/// a `Highlight` from column 0 to column 0, past the row's last char; edtui
/// must render it without indexing out of range.
#[test]
fn a_linewise_highlight_on_an_empty_row_renders() {
    use edtui::EditorView;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;
    let mut b = Body::new("a\n\n\nb", 1, 0);
    b.keys("V");
    assert_eq!(b.ed.highlights.len(), 1);
    let area = Rect::new(0, 0, 20, 6);
    EditorView::new(&mut b.ed).render(area, &mut Buffer::empty(area));
    b.keys("j");
    EditorView::new(&mut b.ed).render(area, &mut Buffer::empty(area));
    let mut b = Body::new("", 0, 0);
    b.keys("V");
    EditorView::new(&mut b.ed).render(area, &mut Buffer::empty(area));
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

#[test]
fn dot_is_global_across_buffers() {
    let mut engine = Engine::new();
    let (mut s1, mut s2) = (BufState::new(), BufState::new());
    let mut url = LineInput::new("aaa bbb");
    url.set_cursor(0);
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut url), state: &mut s1 });
    for ev in [k('c'), k('w'), k('Z'), esc()] {
        engine.handle(ev, Target { buf: &mut OneLineBuf::new(&mut url), state: &mut s1 }, &ViewCtx::default());
    }
    let mut cell = LineInput::new("ccc ddd");
    cell.set_cursor(0);
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut cell), state: &mut s2 });
    engine.handle(k('.'), Target { buf: &mut OneLineBuf::new(&mut cell), state: &mut s2 }, &ViewCtx::default());
    assert_eq!((url.text(), cell.text()), ("Z bbb", "Z ddd"));
}

/// Spec §4.2: a replay types its keys as if
/// typed in the target buffer, so in a one-line field Enter and Tab are
/// dropped and the rest still goes in.
#[test]
fn a_body_insert_replayed_in_a_field_drops_enter_and_tab() {
    let body_ctx = ViewCtx { viewport_rows: Some(20) };
    let mut engine = Engine::new();
    let mut body = BufState::new();
    let mut ed = EditorState::new(Lines::from("a"));
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    for ev in [k('A'), k(','), code(KeyCode::Enter), k('x'), esc()] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    }
    assert_eq!(BodyBuf::new(&mut ed, Style::default()).text(), "a,\nx");
    let mut state = BufState::new();
    let mut field = LineInput::new("q");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state });
    engine.handle(k('.'), Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state }, &ViewCtx::default());
    assert_eq!(field.text(), "q,x");

    for ev in [k('j'), k('A'), code(KeyCode::Tab), k('y'), esc()] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    }
    assert_eq!(BodyBuf::new(&mut ed, Style::default()).text(), "a,\nx y");
    engine.handle(k('.'), Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state }, &ViewCtx::default());
    assert_eq!(field.text(), "q,xy");
}

/// The same ruling: a paste recorded in the body is flattened when `.`
/// replays it in a one-line field, as a paste typed there would be.
#[test]
fn a_body_paste_replayed_in_a_field_is_flattened() {
    let body_ctx = ViewCtx { viewport_rows: Some(20) };
    let mut engine = Engine::new();
    let mut body = BufState::new();
    let mut ed = EditorState::new(Lines::from("a"));
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    engine.handle(k('A'), Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    engine.paste("1\n2", Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    engine.handle(esc(), Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    assert_eq!(BodyBuf::new(&mut ed, Style::default()).text(), "a1\n2");
    let mut state = BufState::new();
    let mut field = LineInput::new("q");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state });
    engine.handle(k('.'), Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state }, &ViewCtx::default());
    let flat = crate::components::line_input::flatten_paste("1\n2");
    assert!(!flat.contains('\n'));
    assert_eq!(field.text(), format!("q{flat}"));
}

#[test]
fn dot_is_one_undo_step() {
    let mut f = Field::new("abcdef", 0);
    f.keys("2x");
    let steps = f.state.history.len();
    f.keys(".");
    assert_eq!((f.text(), f.state.history.len()), ("ef", steps + 1));
    f.keys("u");
    assert_eq!(f.text(), "cdef");
}

#[test]
fn enter_seats_the_caret() {
    for (start, seat, col) in [
        (Start::Normal, Seat::ColZero, 0),
        (Start::Normal, Seat::End, 4),
        (Start::Normal, Seat::FirstNonBlank, 2),
        (Start::Normal, Seat::Keep, 3),
        (Start::Insert, Seat::End, 5),
    ] {
        let mut f = Field::new("  abc", 3);
        f.engine.enter(start, seat, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
        assert_eq!(f.col(), col, "{start:?} {seat:?}");
    }
}

#[test]
fn buf_state_reports_the_session() {
    let mut f = Field::new("ab", 0);
    assert_eq!((f.state.text_at_start(), f.state.edited()), (Some("ab"), false));
    f.keys("x");
    assert!(f.state.edited() && f.state.can_undo());
    f.state.end_session();
    assert_eq!((f.state.text_at_start(), f.state.edited(), f.state.can_undo()), (None, false, false));
}

#[test]
fn the_body_history_survives_a_trip_away_but_not_an_outside_change() {
    let mut b = Body::new("abc", 0, 0);
    b.keys("x");
    b.engine.leave(Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state });
    b.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state });
    assert!(b.state.can_undo(), "unchanged text: `u` still undoes the last small edit");
    b.engine.leave(Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state });
    b.ed.lines = Lines::from("reloaded");
    b.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state });
    assert!(!b.state.can_undo(), "the text changed outside the engine");
}

#[test]
fn carry_keeps_insert_and_drops_visual_and_the_record() {
    let mut engine = Engine::new();
    let (mut s1, mut s2) = (BufState::new(), BufState::new());
    let (mut a, mut b) = (LineInput::new("ab"), LineInput::new("cd"));
    engine.enter(Start::Insert, Seat::End, Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 });
    engine.handle(k('X'), Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 }, &ViewCtx::default());
    b.set_cursor(0);
    engine.carry(Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 }, Target { buf: &mut OneLineBuf::new(&mut b), state: &mut s2 });
    assert_eq!(engine.mode(), Mode::Insert);
    assert!(s1.can_undo(), "the step open in the field left behind closed");
    engine.handle(k('Y'), Target { buf: &mut OneLineBuf::new(&mut b), state: &mut s2 }, &ViewCtx::default());
    assert_eq!((a.text(), b.text()), ("abX", "Ycd"));
    engine.handle(esc(), Target { buf: &mut OneLineBuf::new(&mut b), state: &mut s2 }, &ViewCtx::default());
    engine.handle(k('v'), Target { buf: &mut OneLineBuf::new(&mut b), state: &mut s2 }, &ViewCtx::default());
    assert!(b.paint_span().is_some());
    engine.carry(Target { buf: &mut OneLineBuf::new(&mut b), state: &mut s2 }, Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 });
    assert_eq!((engine.mode(), engine.visual_anchor()), (Mode::Normal, None));
    assert_eq!(b.paint_span(), None, "the field left behind no longer paints Visual");
}

/// Fix round 1 ruling: `carry` leaves the body as `leave` does. Its Visual
/// paint goes, and its text is remembered, so a later entry drops the
/// history if the text changed outside the engine.
#[test]
fn carry_out_of_the_body_clears_its_visual_and_keeps_the_history_check() {
    let body_ctx = ViewCtx { viewport_rows: Some(20) };
    let mut engine = Engine::new();
    let (mut body, mut cell) = (BufState::new(), BufState::new());
    let mut ed = EditorState::new(Lines::from("abc\ndef"));
    let mut field = LineInput::new("q");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    for ev in [k('x'), k('v'), k('j')] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    }
    assert_eq!((ed.mode, ed.highlights.len()), (EditorMode::Visual, 1));
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body },
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
    );
    assert_eq!((engine.mode(), ed.mode, ed.highlights.len()), (Mode::Normal, EditorMode::Normal, 0));
    engine.carry(
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
        Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body },
    );
    assert!(body.can_undo(), "unchanged text: the history stays");
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body },
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
    );
    ed.lines = Lines::from("reloaded");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    assert!(!body.can_undo(), "the text changed outside the engine after the carry");
}

/// `carry` out of an Insert session strips an autoindent nothing followed
/// in the buffer it leaves, as `leave` does, and closes its Insert paint.
#[test]
fn carry_strips_an_unused_autoindent_in_the_body_it_leaves() {
    let body_ctx = ViewCtx { viewport_rows: Some(20) };
    let mut engine = Engine::new();
    let (mut body, mut cell) = (BufState::new(), BufState::new());
    let mut ed = EditorState::new(Lines::from("  a"));
    let mut field = LineInput::new("q");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    engine.handle(k('o'), Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    assert_eq!(BodyBuf::new(&mut ed, Style::default()).text(), "  a\n  ");
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body },
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
    );
    assert_eq!(BodyBuf::new(&mut ed, Style::default()).text(), "  a\n");
    assert_eq!((engine.mode(), ed.mode), (Mode::Insert, EditorMode::Normal));
}

/// `carry` keeps Insert across buffer kinds: the new buffer takes keys as
/// its own kind does (`InsertKey::for_buffer`), and the body's unfinished
/// record (with its Enter) is dropped, so `.` repeats only what was typed
/// after the carry.
#[test]
fn carry_from_the_body_into_a_field_takes_the_fields_keys() {
    let body_ctx = ViewCtx { viewport_rows: Some(20) };
    let mut engine = Engine::new();
    let (mut body, mut cell) = (BufState::new(), BufState::new());
    let mut ed = EditorState::new(Lines::from("a"));
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    for ev in [k('A'), code(KeyCode::Enter), k('b')] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    }
    let mut field = LineInput::new("q");
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body },
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
    );
    assert!(body.can_undo());
    assert_eq!(engine.mode(), Mode::Insert);
    let out = engine.handle(code(KeyCode::Enter), Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell }, &ViewCtx::default());
    assert!(declined(&out), "a field declines Enter");
    for ev in [k('z'), esc()] {
        engine.handle(ev, Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell }, &ViewCtx::default());
    }
    assert_eq!(field.text(), "qz");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body });
    engine.handle(k('.'), Target { buf: &mut BodyBuf::new(&mut ed, Style::default()), state: &mut body }, &body_ctx);
    assert_eq!(BodyBuf::new(&mut ed, Style::default()).text(), "a\nzb");
}

/// Spec §4.2: a session opened by `enter(Start::Insert)` records as an `i`
/// for `.`, even when nothing is typed.
#[test]
fn a_session_entered_in_insert_records_as_i_for_dot() {
    let mut f = Field::new("abc", 0);
    f.keys("x");
    f.engine.enter(Start::Insert, Seat::ColZero, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    f.key(esc());
    f.keys(".");
    assert_eq!(f.text(), "bc", "`.` is a bare `i` now, not the `x`");
    f.engine.enter(Start::Insert, Seat::ColZero, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    f.keys("X");
    f.key(esc());
    f.keys(".");
    assert_eq!(f.text(), "XXbc", "`i` plus what was typed");
}

/// A query box never sets `.`, as typing on Vim's command line never
/// touches the redo buffer.
#[test]
fn a_query_box_session_never_records_for_dot() {
    let mut f = Field::new("abcd", 0);
    f.keys("x");
    let (mut query, mut qs) = (LineInput::new(""), BufState::new());
    f.engine.enter(Start::InsertOnly, Seat::End, Target { buf: &mut OneLineBuf::new(&mut query), state: &mut qs });
    for ev in [k('w'), k('q'), code(KeyCode::Left), k('z')] {
        f.engine.handle(ev, Target { buf: &mut OneLineBuf::new(&mut query), state: &mut qs }, &ViewCtx::default());
    }
    f.engine.leave(Target { buf: &mut OneLineBuf::new(&mut query), state: &mut qs });
    assert_eq!(query.text(), "wzq");
    f.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    f.keys(".");
    assert_eq!(f.text(), "cd", "`.` still repeats the x");
}

#[test]
fn a_mouse_sweep_is_adopted_as_visual_on_release() {
    let mut f = Field::new("abcdef", 0);
    f.input.set_cursor(1);
    f.input.begin_mouse_selection();
    f.input.extend_mouse_selection_to(4);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Sweep);
    assert_eq!(f.engine.mode(), Mode::Visual(Shape::Char));
    assert_eq!(f.input.selection(), Some((1, 4)), "mid-sweep the mouse owns the geometry");
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Release);
    assert_eq!((f.engine.visual_anchor(), f.col()), (Some(Pos::new(0, 1)), 3));
    assert_eq!(f.input.selection(), None);
    assert_eq!(f.input.paint_span(), Some((1, 4)), "the same chars, now painted as Visual");
    f.keys("d");
    assert_eq!(f.text(), "aef");
}

#[test]
fn a_click_ends_visual_and_a_gui_key_selection_is_adopted_from_normal() {
    let mut f = Field::new("abcdef", 0);
    f.keys("vl");
    f.input.set_cursor(4);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    assert_eq!((f.engine.mode(), f.col()), (Mode::Normal, 4));
    f.input.select_all();
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Key);
    assert_eq!((f.engine.mode(), f.engine.visual_anchor(), f.col()), (Mode::Visual(Shape::Char), Some(Pos::new(0, 0)), 5));
}

/// Fix round 1: a click that moves the caret in Insert splits the session
/// as Vim's `ins_mouse()` does. Before and after are separate undo steps,
/// and `.` repeats only what was typed after the click.
#[test]
fn a_click_in_insert_splits_the_session() {
    let mut f = Field::new("abcd", 0);
    f.keys("iXY");
    f.input.set_cursor(5);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.keys("Z");
    f.key(esc());
    assert_eq!((f.text(), f.col(), f.state.history.len()), ("XYabcZd", 5, 2));
    f.keys(".");
    assert_eq!(f.text(), "XYabcZZd", "`.` is `1i` + Z, not the whole session");
    f.keys("uu");
    assert_eq!(f.text(), "XYabcd");
    f.keys("u");
    assert_eq!(f.text(), "abcd");
}

/// The same split through a key piece 4 handled; and a settle that leaves
/// the caret where the engine rested it splits nothing.
#[test]
fn a_settle_that_does_not_move_the_caret_does_not_split() {
    let mut f = Field::new("ab", 0);
    f.keys("aX");
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Key);
    f.keys("Y");
    f.key(esc());
    assert_eq!((f.text(), f.state.history.len()), ("aXYb", 1));
    f.keys("A");
    f.input.set_cursor(0);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Key);
    f.keys("Q");
    f.key(esc());
    assert_eq!((f.text(), f.col()), ("QaXYb", 0));
    f.keys(".");
    assert_eq!(f.text(), "QQaXYb", "a moved caret splits on a Key settle too: `.` is `1i` + Q, not `A` + Q");
}

/// Vim's `start_arrow(&tpos)`: an autoindent nothing followed is stripped
/// where the caret was before the click, not where it went.
#[test]
fn a_click_off_an_unused_autoindent_strips_it() {
    let mut b = Body::new("  a\nb", 0, 0);
    b.keys("o");
    assert_eq!((b.text(), b.caret()), ("  a\n  \nb".to_string(), Pos::new(1, 2)));
    b.ed.cursor = edtui::Index2::new(2, 0);
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state }, Settled::Click);
    assert_eq!((b.text(), b.engine.mode()), ("  a\n\nb".to_string(), Mode::Insert));
    b.keys("Z");
    assert_eq!(b.text(), "  a\n\nZb");
}

/// A mouse selection made in Insert off an unused autoindent: the indent
/// goes where the caret was (Vim's `tpos`), and the selection is adopted.
#[test]
fn a_sweep_from_insert_strips_the_autoindent_where_the_caret_was() {
    use edtui::actions::{Execute, SwitchMode};
    let mut b = Body::new("  a\nbcd", 0, 0);
    b.keys("o");
    b.ed.cursor = edtui::Index2::new(2, 0);
    SwitchMode(EditorMode::Visual).execute(&mut b.ed);
    b.ed.selection.as_mut().expect("a selection").end = edtui::Index2::new(2, 2);
    b.ed.cursor = edtui::Index2::new(2, 2);
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state }, Settled::Release);
    assert_eq!(b.text(), "  a\n\nbcd");
    assert_eq!((b.engine.mode(), b.engine.visual_anchor(), b.caret()), (Mode::Visual(Shape::Char), Some(Pos::new(2, 0)), Pos::new(2, 2)));
}

/// A sweep that starts in Insert in a one-line field keeps the mouse's
/// selection while it runs (ending Insert does not reset the caret).
#[test]
fn a_sweep_from_insert_keeps_the_mouse_selection() {
    let mut f = Field::start("abcdef", 1, Start::Insert);
    f.input.begin_mouse_selection();
    f.input.extend_mouse_selection_to(4);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Sweep);
    assert_eq!((f.engine.mode(), f.input.selection()), (Mode::Visual(Shape::Char), Some((1, 4))));
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Release);
    assert_eq!((f.engine.visual_anchor(), f.col()), (Some(Pos::new(0, 1)), 3));
}

/// Fix round 1 minor: a click drops a half-typed command.
#[test]
fn a_click_drops_a_pending_operator() {
    let mut f = Field::new("abc def", 0);
    f.keys("d");
    assert!(f.engine.pending());
    f.input.set_cursor(4);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    assert!(!f.engine.pending());
    f.keys("w");
    assert_eq!(f.text(), "abc def", "`w` is a motion now, not `dw`");
}

/// Fix round 1 minor: in Visual a GUI-key settle drops the GUI selection,
/// so a later settle from Normal does not adopt it stale.
#[test]
fn a_gui_selection_left_in_visual_is_not_adopted_later() {
    use edtui::actions::{Execute, SwitchMode};
    let mut b = Body::new("abc\ndef", 0, 0);
    b.keys("v");
    SwitchMode(EditorMode::Visual).execute(&mut b.ed);
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state }, Settled::Key);
    assert_eq!(b.ed.selection, None);
    b.key(esc());
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, Style::default()), state: &mut b.state }, Settled::Key);
    assert_eq!(b.engine.mode(), Mode::Normal);
}

/// Fix round 1 minor: an external edit ends Visual and a pending command.
#[test]
fn external_edit_ends_visual_and_a_pending_command() {
    let mut f = Field::new("abcdef", 0);
    f.keys("vl");
    f.engine.external_edit(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, |s| {
        s.splice(Pos::new(0, 0), Pos::new(0, 3), "");
    });
    assert_eq!((f.text(), f.engine.mode(), f.engine.visual_anchor()), ("def", Mode::Normal, None));
    assert_eq!(f.input.paint_span(), None);
    f.keys("d");
    f.engine.external_edit(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, |s| {
        s.splice(Pos::new(0, 0), Pos::new(0, 0), "x");
    });
    assert!(!f.engine.pending());
}

#[test]
fn a_query_box_keeps_its_gui_selection_and_typing_replaces_it() {
    let mut f = Field::start("hello", 5, Start::InsertOnly);
    f.input.select_all();
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Key);
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.keys("x");
    assert_eq!(f.text(), "x");
}

#[test]
fn leave_during_insert_is_one_step_without_stepping_back() {
    let mut f = Field::new("ab", 0);
    f.keys("aXY");
    f.engine.leave(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!((f.text(), f.col(), f.engine.mode()), ("aXYb", 3, Mode::Normal));
    assert_eq!(f.state.history.len(), 1);
}

#[test]
fn external_edit_is_one_step_and_not_dot_repeatable() {
    let mut f = Field::new("abcd", 0);
    f.keys("x");
    let changed = f.engine.external_edit(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, |s| {
        s.splice(Pos::new(0, 0), Pos::new(0, 0), "{{t}}");
        s.set_cursor(Pos::new(0, 5));
    });
    assert!(changed);
    assert_eq!(f.text(), "{{t}}bcd");
    assert_eq!(f.state.history.len(), 2);
    f.keys(".");
    assert_eq!(f.text(), "{{t}}cd", "`.` still repeats the x");
    f.keys("uu");
    assert_eq!(f.text(), "bcd");
    let moved = f.engine.external_edit(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, |s| {
        assert_eq!(s.buf().text(), "bcd");
        s.set_cursor(Pos::new(0, 9));
    });
    assert!(!moved, "only the caret moved");
    assert_eq!((f.col(), f.state.history.len()), (2, 1), "clamped to Normal's last char; no new step");
}

/// In Insert an external edit splits the session as a cursor key does:
/// the typing before it, the edit, and the typing after it are three undo
/// steps, and `.` repeats only what was typed after it.
#[test]
fn external_edit_in_insert_splits_the_session() {
    let mut f = Field::new("ab", 0);
    f.keys("aX");
    f.engine.external_edit(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, |s| {
        s.splice(Pos::new(0, 2), Pos::new(0, 2), "{{t}}");
        s.set_cursor(Pos::new(0, 7));
    });
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.keys("Y");
    f.key(esc());
    assert_eq!((f.text(), f.col()), ("aX{{t}}Yb", 7));
    assert_eq!(f.state.history.len(), 3);
    f.keys(".");
    assert_eq!(f.text(), "aX{{t}}YYb");
    f.keys("uuu");
    assert_eq!(f.text(), "aXb");
    f.keys("u");
    assert_eq!(f.text(), "ab");
}

/// Vim's `ML_EMPTY` holds only while the buffer is still
/// blank. Text put in outside the engine (piece 4, a reload) ends it, so
/// `dd` deletes again.
#[test]
fn a_buffer_filled_outside_the_engine_is_no_longer_emptied() {
    let mut f = Field::new("x", 0);
    f.keys("dd");
    assert_eq!(f.text(), "");
    f.input.set_text("abc");
    f.keys("dd");
    assert_eq!(f.text(), "", "dd deletes the new text");
}

#[test]
fn the_history_keeps_undolevels_steps() {
    let n = settings::UNDOLEVELS;
    let mut f = Field::new(&"x".repeat(n + 5), 0);
    for _ in 0..n + 5 {
        f.keys("x");
    }
    assert_eq!(f.state.history.len(), n);
    for _ in 0..n {
        assert!(!declined(&f.keys("u")));
    }
    assert!(declined(&f.keys("u")));
    assert_eq!(f.text().len(), n);
}

/// Review focus 1: edtui holds an empty body as zero rows; every tier-1
/// edit must work on it and edtui must still render it.
#[test]
fn every_tier_one_edit_works_on_an_empty_body() {
    use edtui::EditorView;
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
    for keys in ["x", "X", "dd", "D", "dw", "cwX\u{1b}", "ccX\u{1b}", "p", "P", "J", "rX", "o\u{1b}",
                 "Ox\u{1b}", "iab\u{1b}", "A\u{1b}", "yyp", "vd", "Vd", "vy", "u", "x.", "v~", "Vp",
                 "di{", "ci\"x\u{1b}", "%", "G", "w", "e", "b", "$"] {
        let mut b = Body::new("", 0, 0);
        for c in keys.chars() {
            b.key(if c == '\u{1b}' { esc() } else { k(c) });
        }
        let lines = BodyBuf::new(&mut b.ed, Style::default()).line_count();
        assert!(b.caret().row < lines, "{keys:?} left the caret off the text");
        let area = Rect::new(0, 0, 20, 5);
        EditorView::new(&mut b.ed).render(area, &mut Buffer::empty(area));
    }
}

#[test]
fn a_declined_undo_or_redo_hands_back_the_key_as_typed() {
    let mut f = Field::new("abc", 0);
    let shifted_r = KeyEvent::new(KeyCode::Char('R'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    f.key(k('2'));
    assert_eq!(f.key(k('u')), Outcome::Declined { count: Some(2), keys: vec![k('u')] });
    assert_eq!(f.key(shifted_r), Outcome::Declined { count: None, keys: vec![shifted_r] });
}

/// The engine never panics (spec §8.2): the app's undo can shrink the body
/// behind the engine's back, leaving its history naming rows past the end.
#[test]
fn a_stale_redo_after_an_outside_undo_does_not_panic() {
    let mut b = Body::new("a\nb\nc\nd\ne", 4, 0);
    b.keys("dd");
    b.keys("u");
    b.keys("u");
    // The app's own undo shrinks the body without the engine knowing.
    b.ed.lines = Lines::from("a");
    b.ed.cursor = edtui::Index2::new(0, 0);
    b.key(ctrl('r'));
    let lines = BodyBuf::new(&mut b.ed, Style::default()).line_count();
    assert!(b.caret().row < lines, "caret row {} of {lines}", b.caret().row);
    let caret = b.caret();
    let len = BodyBuf::new(&mut b.ed, Style::default()).line_len(caret.row);
    assert!(caret.col <= len);
    // Every stale splice shape, straight on the buffer.
    for (s, e) in [((9, 3), (9, 5)), ((0, 0), (7, 0)), ((6, 1), (2, 0)), ((3, 0), (4, 0))] {
        let mut ed = EditorState::new(Lines::from("a\nb"));
        BodyBuf::new(&mut ed, Style::default()).splice(Pos::new(s.0, s.1), Pos::new(e.0, e.1), "x\ny");
        assert!(!ed.lines.is_empty());
    }
}

/// Spec §8.5: debug build, 5,000-line body. The limits are loose on
/// purpose: a debug build under parallel tests can be several times slower
/// than a quiet run, and the worst measured value was about 9 ms. They
/// still catch an accidental quadratic.
#[test]
fn a_5000_line_body_stays_fast() {
    let rows: Vec<String> = (0..5000).map(|i| format!("  \"k{i}\": {{\"v\": [{i}, 2]}},")).collect();
    let text = format!("{{\n{}\n}}", rows.join("\n"));
    for keys in ["dG", "dGu", "ci{"] {
        let mut b = Body::new(&text, 2500, 4);
        let started = std::time::Instant::now();
        b.keys(keys);
        let ms = started.elapsed().as_millis();
        assert!(ms < 250, "{keys} took {ms} ms");
    }
    let mut b = Body::new(&text, 2500, 4);
    b.keys("i");
    let started = std::time::Instant::now();
    for _ in 0..200 {
        b.key(k('x'));
    }
    let ms = started.elapsed().as_millis();
    assert!(ms < 1000, "typing 200 chars took {ms} ms");
}

/// Spec §2: no engine file names an app type, except `buf.rs` (and this
/// test file, which names them to look for them).
#[test]
fn no_engine_file_names_an_app_type() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/vim/engine");
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("the engine directory") {
        let path = entry.expect("a directory entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if !name.ends_with(".rs") || name == "buf.rs" || name == "tests.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a readable source file");
        for word in ["App", "Action", "Editor", "FieldId"] {
            for (i, _) in text.match_indices(word) {
                let before = text[..i].chars().next_back();
                let after = text[i + word.len()..].chars().next();
                assert!(ident(before) || ident(after), "{name} names the app type {word}");
            }
        }
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} engine files were checked");
}
