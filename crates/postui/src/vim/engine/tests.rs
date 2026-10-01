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
    pub visual: BodyVisual,
    pub engine: Engine,
    pub state: BufState,
}

impl Body {
    pub fn new(text: &str, row: usize, col: usize) -> Self {
        let mut ed = EditorState::new(Lines::from(text));
        ed.cursor = edtui::Index2::new(row, col);
        let mut b = Self { ed, visual: None, engine: Engine::new(), state: BufState::new() };
        b.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
        b
    }

    pub fn key(&mut self, ev: KeyEvent) -> Outcome {
        let ctx = ViewCtx { viewport_rows: Some(20) };
        self.engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut self.ed, &mut self.visual), state: &mut self.state }, &ctx)
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
        BodyBuf::new(&mut self.ed, &mut self.visual).text()
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
    assert!(!declined(&b.key(code(KeyCode::Enter))), "body Enter is the + motion (plan 3b)");
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
    for key in [code(KeyCode::Enter), code(KeyCode::Tab), code(KeyCode::BackTab), code(KeyCode::Up), code(KeyCode::Down)] {
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
    b.engine.paste("x\r\ny\rz", Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    assert_eq!(b.text(), "x\ny\nz");
    assert_eq!(b.caret(), Pos::new(2, 1));
    b.engine.paste("\tw", Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
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
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body_state });
    engine.handle(k('p'), Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body_state }, &ViewCtx::default());
    assert_eq!(BodyBuf::new(&mut ed, &mut None).text(), "xone");
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

/// `BodyBuf::show` records a linewise selection on an empty row as a span
/// from column 0 to column 0, past the row's last char; edtui must render
/// that as a `Highlight` (how the editor draws it) without indexing out of
/// range.
#[test]
fn a_linewise_highlight_on_an_empty_row_renders() {
    use edtui::{EditorView, Highlight};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;
    let area = Rect::new(0, 0, 20, 6);
    let render = |b: &mut Body| {
        let (from, to) = b.visual.expect("Visual records a span");
        b.ed.highlights = vec![Highlight::new(from, to, Style::default())];
        EditorView::new(&mut b.ed).render(area, &mut Buffer::empty(area));
    };
    let mut b = Body::new("a\n\n\nb", 1, 0);
    b.keys("V");
    assert_eq!(b.visual, Some((edtui::Index2::new(1, 0), edtui::Index2::new(1, 0))));
    render(&mut b);
    b.keys("j");
    render(&mut b);
    let mut b = Body::new("", 0, 0);
    b.keys("V");
    render(&mut b);
}

/// The editor owns `EditorState::highlights` (its JSON colours live there);
/// painting never touches the list, whatever the mode.
#[test]
fn painting_leaves_the_editors_highlights_alone() {
    let syntax = vec![edtui::Highlight::new(
        edtui::Index2::new(0, 0),
        edtui::Index2::new(0, 2),
        Style::default().fg(ratatui::style::Color::Red),
    )];
    let mut b = Body::new("abc\ndef", 0, 0);
    b.ed.highlights = syntax.clone();
    for ev in [k('l'), k('j'), k('v'), k('l'), esc(), k('V'), esc(), k('x'), k('u')] {
        b.key(ev);
        assert_eq!(b.ed.highlights, syntax, "after {ev:?}");
        assert_eq!(b.visual.is_some(), matches!(b.engine.mode(), Mode::Visual(_)), "after {ev:?}");
    }
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
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body });
    for ev in [k('A'), k(','), code(KeyCode::Enter), k('x'), esc()] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body }, &body_ctx);
    }
    assert_eq!(BodyBuf::new(&mut ed, &mut None).text(), "a,\nx");
    let mut state = BufState::new();
    let mut field = LineInput::new("q");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state });
    engine.handle(k('.'), Target { buf: &mut OneLineBuf::new(&mut field), state: &mut state }, &ViewCtx::default());
    assert_eq!(field.text(), "q,x");

    for ev in [k('j'), k('A'), code(KeyCode::Tab), k('y'), esc()] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body }, &body_ctx);
    }
    assert_eq!(BodyBuf::new(&mut ed, &mut None).text(), "a,\nx y");
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
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body });
    engine.handle(k('A'), Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body }, &body_ctx);
    engine.paste("1\n2", Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body });
    engine.handle(esc(), Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body }, &body_ctx);
    assert_eq!(BodyBuf::new(&mut ed, &mut None).text(), "a1\n2");
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
    b.engine.leave(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    b.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    assert!(b.state.can_undo(), "unchanged text: `u` still undoes the last small edit");
    b.engine.leave(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    b.ed.lines = Lines::from("reloaded");
    b.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
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
    let mut vis = None;
    let mut field = LineInput::new("q");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut vis), state: &mut body });
    for ev in [k('x'), k('v'), k('j')] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, &mut vis), state: &mut body }, &body_ctx);
    }
    assert_eq!((ed.mode, vis.is_some()), (EditorMode::Visual, true));
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, &mut vis), state: &mut body },
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
    );
    assert_eq!((engine.mode(), ed.mode, vis), (Mode::Normal, EditorMode::Normal, None));
    engine.carry(
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
        Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body },
    );
    assert!(body.can_undo(), "unchanged text: the history stays");
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body },
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
    );
    ed.lines = Lines::from("reloaded");
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body });
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
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body });
    engine.handle(k('o'), Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body }, &body_ctx);
    assert_eq!(BodyBuf::new(&mut ed, &mut None).text(), "  a\n  ");
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body },
        Target { buf: &mut OneLineBuf::new(&mut field), state: &mut cell },
    );
    assert_eq!(BodyBuf::new(&mut ed, &mut None).text(), "  a\n");
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
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body });
    for ev in [k('A'), code(KeyCode::Enter), k('b')] {
        engine.handle(ev, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body }, &body_ctx);
    }
    let mut field = LineInput::new("q");
    engine.carry(
        Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body },
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
    engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body });
    engine.handle(k('.'), Target { buf: &mut BodyBuf::new(&mut ed, &mut None), state: &mut body }, &body_ctx);
    assert_eq!(BodyBuf::new(&mut ed, &mut None).text(), "a\nzb");
}

/// A session opened by `enter(Start::Insert)` is Vim's `:startinsert`: `.`
/// takes it as an `i` plus what was typed, and only once something is
/// typed (user ruling 2026-09-30, "match vim"; Vim 9.1: `x`,
/// `:startinsert`, `<Esc>`, `0.` gives `bdef` from `abcdef`, and with `Z`
/// typed gives `ZabZdef`).
#[test]
fn a_session_entered_in_insert_records_as_i_once_something_is_typed() {
    let enter = |f: &mut Field| {
        f.engine.enter(Start::Insert, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    };
    let mut f = Field::new("abcdef", 2);
    f.keys("x");
    enter(&mut f);
    f.key(esc());
    f.keys("0.");
    assert_eq!(f.text(), "bdef", "nothing typed: `.` is still the `x`");

    let mut f = Field::new("abcdef", 2);
    f.keys("x");
    enter(&mut f);
    f.keys("Z");
    f.key(esc());
    f.keys("0.");
    assert_eq!(f.text(), "ZabZdef", "`i` plus what was typed");
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
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state }, Settled::Click);
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
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state }, Settled::Release);
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
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state }, Settled::Key);
    assert_eq!(b.ed.selection, None);
    b.key(esc());
    b.engine.settle(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state }, Settled::Key);
    assert_eq!(b.engine.mode(), Mode::Normal);
}

/// A field in Visual settles a GUI selection as the body does: a click's
/// selection (a double-click word) is adopted, a GUI key's is dropped so
/// no later settle adopts it stale.
#[test]
fn a_field_in_visual_adopts_a_clicked_selection_and_drops_a_gui_keys() {
    let settle = |f: &mut Field, how| {
        f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, how);
    };
    let mut f = Field::new("one two three", 0);
    f.keys("v");
    f.input.select_word_at(5);
    settle(&mut f, Settled::Click);
    assert_eq!((f.engine.mode(), f.engine.visual_anchor(), f.col()), (Mode::Visual(Shape::Char), Some(Pos::new(0, 4)), 6));
    assert_eq!(f.input.selection(), None, "adopted, so the GUI selection goes");
    assert_eq!(f.input.paint_span(), Some((4, 7)));

    let mut f = Field::new("one two three", 0);
    f.keys("vl");
    f.input.select_all();
    settle(&mut f, Settled::Key);
    assert_eq!(f.engine.visual_anchor(), Some(Pos::new(0, 0)), "the engine's selection stands");
    assert_eq!(f.input.selection(), None, "the GUI key's selection goes");
    f.key(esc());
    settle(&mut f, Settled::Key);
    assert_eq!(f.engine.mode(), Mode::Normal, "nothing stale is adopted after Esc");
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
        let lines = BodyBuf::new(&mut b.ed, &mut b.visual).line_count();
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
    // `u` is a plain key: every swallowed key comes back, a register
    // prefix included. `ctrl+r` is a chord: it comes back alone.
    f.keys("\"\"");
    assert_eq!(f.key(k('u')), Outcome::Declined { count: None, keys: vec![k('"'), k('"'), k('u')] });
    f.keys("\"\"3");
    assert_eq!(f.key(ctrl('r')), Outcome::Declined { count: None, keys: vec![ctrl('r')] });
    assert!(!f.engine.pending());
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
    let lines = BodyBuf::new(&mut b.ed, &mut b.visual).line_count();
    assert!(b.caret().row < lines, "caret row {} of {lines}", b.caret().row);
    let caret = b.caret();
    let len = BodyBuf::new(&mut b.ed, &mut b.visual).line_len(caret.row);
    assert!(caret.col <= len);
    // Every stale splice shape, straight on the buffer.
    for (s, e) in [((9, 3), (9, 5)), ((0, 0), (7, 0)), ((6, 1), (2, 0)), ((3, 0), (4, 0))] {
        let mut ed = EditorState::new(Lines::from("a\nb"));
        BodyBuf::new(&mut ed, &mut None).splice(Pos::new(s.0, s.1), Pos::new(e.0, e.1), "x\ny");
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

/// Plan 3b Task 1: the register prefix takes `0` as well as `"`, and the
/// echo shows it; other registers still show a note and arm nothing.
#[test]
fn the_yank_register_prefix_arms_and_echoes() {
    let mut f = Field::new("abc", 0);
    f.keys("\"0");
    assert_eq!(f.engine.echo(), "\"0");
    f.keys("2d");
    assert_eq!(f.engine.echo(), "\"02d");
    f.key(esc());
    assert!(!f.engine.pending());
    let note = |out: Outcome| match out {
        Outcome::Consumed { note: Some(Note::Unsupported(n)), .. } => n,
        other => panic!("no note: {other:?}"),
    };
    assert_eq!(note(f.keys("\"1")), "register \"1 not supported");
    assert!(!f.engine.pending());
    assert_eq!(f.engine.registers().zero(), &Register::default(), "a fresh engine's \"0 is empty");
}

/// Key list §5 in a field: a linewise `"0` goes in charwise, as the unnamed
/// register does (the conformance test skips these one-line runs).
#[test]
fn a_linewise_zero_register_puts_charwise_in_a_field() {
    let mut f = Field::new("ab", 0);
    f.keys("yy");
    assert_eq!(f.engine.registers().zero(), &Register { text: "ab\n".into(), kind: RegKind::Line });
    f.keys("x");
    assert_eq!(f.engine.registers().unnamed().text, "a", "a delete writes only the unnamed register");
    f.keys("\"0p");
    assert_eq!((f.text(), f.col()), ("bab", 2));
}

/// Plan 3b Task 2: the case operators echo and cancel as Vim's do.
#[test]
fn case_operators_echo_and_cancel() {
    let mut f = Field::new("abc", 0);
    for (keys, echo) in [("gU", "gU"), ("g~g", "g~g"), ("2gu3", "2gu3"), ("\"0g~", "\"0g~")] {
        f.keys(keys);
        assert_eq!(f.engine.echo(), echo, "{keys:?}");
        f.key(esc());
        assert!(!f.engine.pending());
    }
    for keys in ["gUu", "g~u", "gu~", "gUd", "dgU"] {
        assert_eq!(f.keys(keys), Outcome::Consumed { changed: false, note: None, request: None }, "{keys:?} cancels");
        assert!(!f.engine.pending(), "{keys:?}");
        assert_eq!(f.text(), "abc");
    }
}

/// Plan 3b Task 3: the shift operators echo; in a one-line field `>>`
/// indents the field as Vim indents a one-line buffer (Deviation 3).
#[test]
fn shift_operators_echo_and_indent_a_field() {
    let mut f = Field::new("ab", 1);
    f.keys("2>");
    assert_eq!(f.engine.echo(), "2>");
    f.key(esc());
    f.keys(">>");
    assert_eq!((f.text(), f.col()), ("  ab", 2));
    f.keys("<<");
    assert_eq!((f.text(), f.col()), ("ab", 0));
}

/// Deviation 8: a counted insert repeats on Esc only; `leave` ends it
/// once, as Vim's `:stopinsert` drops the count.
#[test]
fn a_counted_insert_repeats_on_esc_only() {
    let mut f = Field::new("ab", 0);
    f.keys("3iX");
    f.key(esc());
    assert_eq!(f.text(), "XXXab");
    let mut f = Field::new("ab", 0);
    f.keys("3iX");
    f.engine.leave(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(f.text(), "Xab");
}

/// Review focus 1: the largest count repeats an insert quickly, as one undo
/// step. The limit is loose on purpose (a debug build under parallel
/// tests); it catches an accidental quadratic.
#[test]
fn a_huge_counted_insert_finishes() {
    let started = std::time::Instant::now();
    let mut b = Body::new("{\n  \"a\": 1\n}", 1, 2);
    b.keys("9999iX");
    b.key(esc());
    assert_eq!(b.text().matches('X').count(), 9999);
    b.keys("u");
    assert_eq!(b.text(), "{\n  \"a\": 1\n}", "one undo step");
    let mut f = Field::new("ab", 0);
    f.keys("999aYZ");
    f.key(esc());
    assert_eq!(f.text().chars().count(), 2 + 999 * 2);
    let ms = started.elapsed().as_millis();
    assert!(ms < 5000, "took {ms} ms");
}

/// Deviation 7: `leave` sets `'^`, so `gi` after coming back resumes where
/// Insert ended (Vim's `:stopinsert` sets it too).
#[test]
fn gi_resumes_where_leave_ended_insert() {
    let mut f = Field::new("abcd", 0);
    f.keys("lliX");
    f.engine.leave(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    f.engine.enter(Start::Normal, Seat::ColZero, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    f.keys("giY");
    f.key(esc());
    assert_eq!(f.text(), "abXYcd");
}

/// Deviation 7: `carry` sets `'^` in the buffer it leaves.
#[test]
fn gi_resumes_where_carry_left_a_buffer() {
    let mut engine = Engine::new();
    let (mut s1, mut s2) = (BufState::new(), BufState::new());
    let (mut a, mut b) = (LineInput::new("ab"), LineInput::new("cd"));
    engine.enter(Start::Insert, Seat::End, Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 });
    engine.handle(k('X'), Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 }, &ViewCtx::default());
    b.set_cursor(0);
    engine.carry(Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 }, Target { buf: &mut OneLineBuf::new(&mut b), state: &mut s2 });
    engine.leave(Target { buf: &mut OneLineBuf::new(&mut b), state: &mut s2 });
    engine.enter(Start::Normal, Seat::ColZero, Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 });
    for ev in [k('g'), k('i'), k('Y'), esc()] {
        engine.handle(ev, Target { buf: &mut OneLineBuf::new(&mut a), state: &mut s1 }, &ViewCtx::default());
    }
    assert_eq!(a.text(), "abXY");
}

/// Review focus 3: a change outside the engine. With no `enter`, a `'^`
/// past the text clamps into it; after an `enter` that sees the change it
/// is gone with the history, and `gi` is a plain `i`.
#[test]
fn gi_after_an_outside_change_clamps_or_forgets() {
    let mut b = Body::new("abc\ndef\nghi", 2, 1);
    b.keys("aX");
    b.key(esc());
    b.ed.lines = Lines::from("a");
    b.ed.cursor = edtui::Index2::new(0, 0);
    b.keys("giY");
    b.key(esc());
    assert_eq!(b.text(), "aY", "clamped to the end of the only line");
    b.engine.leave(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    b.ed.lines = Lines::from("reloaded");
    b.engine.enter(Start::Normal, Seat::ColZero, Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    b.keys("giZ");
    b.key(esc());
    assert_eq!(b.text(), "Zreloaded");
}

/// Deviation 7: `leave` in Visual remembers the area, so `gv` after
/// coming back reselects it.
#[test]
fn gv_reselects_what_leave_ended() {
    let mut f = Field::new("abcdef", 1);
    f.keys("vl");
    f.engine.leave(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(f.engine.mode(), Mode::Normal);
    f.engine.enter(Start::Normal, Seat::ColZero, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    f.keys("gv");
    assert_eq!((f.engine.mode(), f.engine.visual_anchor(), f.col()), (Mode::Visual(Shape::Char), Some(Pos::new(0, 1)), 2));
}

/// Deviation 7: a click that ends Visual remembers the area as it was
/// before the click moved the caret.
#[test]
fn a_click_ending_visual_remembers_the_area_before_the_click() {
    let mut f = Field::new("abcdef", 1);
    f.keys("vl");
    f.input.set_cursor(5);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    assert_eq!(f.engine.mode(), Mode::Normal);
    f.keys("gv");
    assert_eq!((f.engine.visual_anchor(), f.col()), (Some(Pos::new(0, 1)), 2));
}

/// Review focus 3: `gv` after an outside change. With no `enter`, an area
/// past the text clamps into it; after an `enter` that sees the change,
/// there is nothing to reselect.
#[test]
fn gv_after_an_outside_change_clamps_or_does_nothing() {
    let mut b = Body::new("abc\ndef\nghi", 1, 0);
    b.keys("vjl");
    b.key(esc());
    b.ed.lines = Lines::from("xy\nz");
    b.ed.cursor = edtui::Index2::new(0, 0);
    b.keys("gv");
    assert_eq!(b.engine.mode(), Mode::Visual(Shape::Char));
    assert_eq!((b.engine.visual_anchor(), b.caret()), (Some(Pos::new(1, 0)), Pos::new(1, 1)));
    b.key(esc());
    b.engine.leave(Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    b.ed.lines = Lines::from("reloaded");
    b.engine.enter(Start::Normal, Seat::ColZero, Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    b.keys("gv");
    assert_eq!(b.engine.mode(), Mode::Normal, "the area went with the history");
}

/// Plan 3b Task 10: Insert `ctrl+r` waits for a register name. The echo
/// and `pending()` show it; anything but `"` or `0` is a note or is
/// swallowed, and a foreign chord reaches the app.
#[test]
fn insert_ctrl_r_takes_the_next_key_as_the_register() {
    let note = |out: Outcome| match out {
        Outcome::Consumed { note: Some(Note::Unsupported(n)), .. } => n,
        other => panic!("no note: {other:?}"),
    };
    let mut f = Field::new("ab", 0);
    f.keys("yiwA");
    f.key(ctrl('r'));
    assert!(f.engine.pending());
    assert_eq!(f.engine.echo(), "^R");
    assert_eq!(f.keys("\""), Outcome::Consumed { changed: true, note: None, request: None });
    assert_eq!(f.text(), "abab");
    assert!(!f.engine.pending());
    f.key(ctrl('r'));
    assert_eq!(note(f.keys("a")), "register \"a not supported");
    f.key(ctrl('r'));
    f.key(ctrl('r'));
    assert_eq!(f.engine.echo(), "^R^R");
    f.keys("0");
    assert_eq!(f.text(), "ababab");
    f.key(ctrl('r'));
    f.key(ctrl('o'));
    assert_eq!(f.engine.echo(), "^R^O");
    assert_eq!(note(f.keys("\"")), "ctrl+r ctrl+o not supported");
    f.key(ctrl('r'));
    assert_eq!(f.key(esc()), Outcome::Consumed { changed: false, note: None, request: None }, "Esc names no register");
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.key(ctrl('r'));
    assert_eq!(f.key(ctrl('c')), Outcome::Declined { count: None, keys: vec![ctrl('c')] });
    assert!(!f.engine.pending());
    f.keys("Z");
    assert_eq!(f.text(), "abababZ");
}

/// Review focus 2: a register with line breaks, tabs or control chars goes
/// into a one-line field or a query box flattened, as `p` puts it; nothing
/// is declined, so no Enter reaches the app.
#[test]
fn insert_ctrl_r_flattens_into_a_field_and_a_query_box() {
    for start in [Start::Insert, Start::InsertOnly] {
        let mut f = Field::start("", 0, start);
        f.engine.registers_mut().set_unnamed(Register { text: "a\nb\tc\n".into(), kind: RegKind::Line });
        f.key(ctrl('r'));
        assert!(!declined(&f.key(k('"'))), "{start:?}");
        assert_eq!(f.text(), "a b c", "{start:?}");
    }
    let mut f = Field::start("", 0, Start::Insert);
    f.engine.registers_mut().set_unnamed(Register { text: "x\u{8}y".into(), kind: RegKind::Char });
    f.key(ctrl('r'));
    f.keys("\"");
    assert_eq!(f.text(), "x\u{8}y", "a field takes a control char as a char, as a paste does");
}

/// Final review M3: Insert `ctrl+r` types the flattened register into a
/// one-line field as one run, not a key per char (each of which rebuilt
/// the field: quadratic). The limit is loose on purpose (a debug build
/// under parallel tests); char by char took seconds. `.` and a counted
/// insert still put the same text in.
#[test]
fn insert_ctrl_r_into_a_field_is_one_run() {
    let big = "abcd".repeat(10_000);
    let mut f = Field::start("", 0, Start::Insert);
    f.engine.registers_mut().set_unnamed(Register { text: big.clone(), kind: RegKind::Char });
    let started = std::time::Instant::now();
    f.key(ctrl('r'));
    f.keys("\"");
    let ms = started.elapsed().as_millis();
    assert_eq!(f.text(), big);
    assert!(ms < 1000, "a 40,000-char register took {ms} ms");
    let mut f = Field::new("ab", 0);
    f.engine.registers_mut().set_unnamed(Register { text: "x\ny".into(), kind: RegKind::Char });
    f.keys("2a");
    f.key(ctrl('r'));
    f.keys("\"");
    f.key(esc());
    assert_eq!((f.text(), f.col()), ("ax yx yb", 6));
    f.keys(".");
    assert_eq!((f.text(), f.col()), ("ax yx yx yx yb", 12));
    f.keys("u");
    assert_eq!(f.text(), "ax yx yb", "the repeat is one undo step");
}

/// Review (2026-10-01): the body too takes a register's plain chars as one
/// run per line, not a key per char (each a splice of the line). The limit
/// is loose on purpose (a debug build under parallel tests). Line breaks
/// still go in as Enter (`autoindent`), and `.` repeats the whole insert.
#[test]
fn insert_ctrl_r_into_the_body_is_one_run_per_line() {
    let big = "abcd".repeat(10_000);
    let mut b = Body::new("{\n  x\n}", 1, 3);
    b.engine.registers_mut().set_unnamed(Register { text: big.clone(), kind: RegKind::Char });
    b.keys("a");
    let started = std::time::Instant::now();
    b.key(ctrl('r'));
    b.keys("\"");
    let ms = started.elapsed().as_millis();
    assert_eq!(b.text(), format!("{{\n  x{big}\n}}"));
    assert!(ms < 1000, "a 40,000-char register took {ms} ms");
    let mut b = Body::new("{\n  x\n}", 1, 3);
    b.engine.registers_mut().set_unnamed(Register { text: "ab\ncd\n".into(), kind: RegKind::Line });
    b.keys("a");
    b.key(ctrl('r'));
    b.keys("\"");
    b.key(esc());
    assert_eq!(b.text(), "{\n  xab\n  cd\n\n}", "Esc drops an indent-only line's autoindent");
    assert_eq!(b.caret(), Pos::new(3, 0));
    b.keys("j.");
    assert_eq!(b.text(), "{\n  xab\n  cd\n\n}ab\ncd\n");
    b.keys("u");
    assert_eq!(b.text(), "{\n  xab\n  cd\n\n}", "the repeat is one undo step");
}

/// Review (2026-10-01): a third `ctrl+r` is an invalid register name (Vim
/// beeps), not a chord for the app; `ctrl+r ctrl+o` takes any next key,
/// chord or not, and shows its note.
#[test]
fn insert_ctrl_r_swallows_a_third_ctrl_r_and_ctrl_o_takes_any_key() {
    let mut f = Field::new("ab", 0);
    f.keys("A");
    f.key(ctrl('r'));
    f.key(ctrl('r'));
    assert_eq!(f.key(ctrl('r')), Outcome::Consumed { changed: false, note: None, request: None });
    assert!(!f.engine.pending());
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.key(ctrl('r'));
    f.key(ctrl('o'));
    match f.key(ctrl('w')) {
        Outcome::Consumed { note: Some(Note::Unsupported(n)), .. } => assert_eq!(n, "ctrl+r ctrl+o not supported"),
        other => panic!("no note: {other:?}"),
    }
    assert!(!f.engine.pending());
    f.key(ctrl('r'));
    f.key(ctrl('p'));
    match f.key(ctrl('c')) {
        Outcome::Consumed { note: Some(Note::Unsupported(n)), .. } => assert_eq!(n, "ctrl+r ctrl+p not supported"),
        other => panic!("no note: {other:?}"),
    }
    f.keys("Z");
    assert_eq!(f.text(), "abZ");
}

/// Review (2026-10-01): a linewise Visual case op lands on the same letter
/// when a `ß` before the caret became "SS", as Normal `gUj` does (Vim keeps
/// the caret's byte offset; `recased_caret`).
#[test]
fn visual_linewise_case_keeps_the_caret_on_its_letter_past_a_grown_sz() {
    let mut b = Body::new("ß ǅxy\n  straße", 1, 4);
    b.keys("VkU");
    assert_eq!(b.text(), "SS ǄXY\n  STRASSE");
    assert_eq!(b.caret(), Pos::new(0, 5), "on the Y");
    let mut b = Body::new("ß ǅxy\n  straße", 1, 4);
    b.keys("Vk~");
    assert_eq!(b.text(), "ß ǄXY\n  STRAßE", "`~` leaves ß alone");
    assert_eq!(b.caret(), Pos::new(0, 4), "nothing grew: the column is kept");
}

/// Review focus 4: a half-typed `ctrl+r` never outlives a click, a paste
/// or the buffer.
#[test]
fn a_half_typed_ctrl_r_is_dropped_at_every_edge() {
    let mut f = Field::start("ab", 2, Start::Insert);
    f.key(ctrl('r'));
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    assert!(!f.engine.pending());
    f.keys("\"");
    assert_eq!(f.text(), "ab\"", "the quote is typed, not read as a register");
    f.key(ctrl('r'));
    f.engine.paste("X", Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert!(!f.engine.pending());
    f.key(ctrl('r'));
    let (mut other, mut os) = (LineInput::new("q"), BufState::new());
    other.set_cursor(0);
    f.engine.carry(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os });
    assert!(!f.engine.pending());
    f.engine.handle(k('0'), Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os }, &ViewCtx::default());
    assert_eq!(other.text(), "0q", "typed in the new field, not read as \"0");
    f.engine.handle(ctrl('r'), Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os }, &ViewCtx::default());
    f.engine.leave(Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os });
    assert!(!f.engine.pending());
}

/// A key spelled as one char, for the sweeps below: `\u{1}` is ctrl+a,
/// `\u{18}` ctrl+x, `\u{12}` ctrl+r, `\u{1b}` Esc, `\r` Enter.
fn key_of(c: char) -> KeyEvent {
    match c {
        '\u{1}' => ctrl('a'),
        '\u{18}' => ctrl('x'),
        '\u{12}' => ctrl('r'),
        '\u{1b}' => esc(),
        '\r' => code(KeyCode::Enter),
        c => k(c),
    }
}

/// Review focus 5: edtui holds an empty body as zero rows; every
/// first-wave key must work on it and edtui must still render it.
#[test]
fn every_first_wave_key_works_on_an_empty_body() {
    use edtui::EditorView;
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
    for keys in ["~", "3~", "g~~", "gUw", "guiw", ">>", "<<", ">j", "}", "{", "d}", "ge", "dge", "+", "-", "\r",
                 "dip", "dap", "vip", "yap", "vipd", "\u{1}", "\u{18}", "3iX\u{1b}", "3o\u{1b}", "3OY\u{1b}",
                 "gv", "vl\u{1b}gv", "giX\u{1b}", "aX\u{1b}giY\u{1b}", "A\u{12}\"\u{1b}", "yyA\u{12}0\u{1b}",
                 "\"0p", "yy\"0P", "vp", "\"0yiw", "x.", "3iX\u{1b}u"] {
        let mut b = Body::new("", 0, 0);
        for c in keys.chars() {
            b.key(key_of(c));
        }
        let lines = BodyBuf::new(&mut b.ed, &mut b.visual).line_count();
        assert!(b.caret().row < lines, "{keys:?} left the caret off the text");
        let area = Rect::new(0, 0, 20, 5);
        EditorView::new(&mut b.ed).render(area, &mut Buffer::empty(area));
    }
}

/// Review focus 1: the largest count on every first-wave command finishes
/// quickly and never panics. The limit is loose on purpose (a debug build
/// under parallel tests); it catches an accidental quadratic.
#[test]
fn first_wave_commands_survive_the_largest_count() {
    let rows: Vec<String> = (0..200).map(|i| format!("  \"k{i}\": [{i}, 0x1f],")).collect();
    let text = format!("{{\n{}\n}}", rows.join("\n"));
    for keys in ["9999~", "9999g~~", "9999>>", "9999<<", "9999}", "9999{", "9999ge", "9999+", "9999-",
                 "9999\u{1}", "9999\u{18}", "d9999ip", "v9999ap", "9999gv", "9999giX\u{1b}", "x9999."] {
        let mut b = Body::new(&text, 100, 4);
        let started = std::time::Instant::now();
        for c in keys.chars() {
            b.key(key_of(c));
        }
        let ms = started.elapsed().as_millis();
        assert!(ms < 2000, "{keys:?} took {ms} ms");
    }
}

/// Plan 3c Task 2: `R` opens a Replace session. Typing overwrites, Esc
/// steps back and sets `'^`, and the mode is reported as Replace.
#[test]
fn replace_mode_overwrites_and_reports_its_mode() {
    let mut f = Field::new("abcd", 1);
    f.keys("R");
    assert_eq!(f.engine.mode(), Mode::Replace);
    f.keys("xy");
    assert_eq!((f.text(), f.col()), ("axyd", 3));
    f.keys("zw");
    assert_eq!(f.text(), "axyzw", "past the end it appends");
    f.key(code(KeyCode::Backspace));
    f.key(code(KeyCode::Backspace));
    assert_eq!(f.text(), "axyd", "an appended char is deleted, a replaced one comes back");
    f.key(esc());
    assert_eq!((f.engine.mode(), f.col()), (Mode::Normal, 2));
    assert_eq!(f.state.history.marks.insert, Some(Pos::new(0, 3)));
    f.keys("u");
    assert_eq!(f.text(), "abcd", "one undo step");
}

/// Review focus 4: Replace at the edges never panics and matches Vim's
/// rules: a tab and a multibyte char are one char each, `BS` before the
/// session's start only moves, a paste with line breaks splits lines in the
/// body and flattens in a field.
#[test]
fn replace_edges() {
    let mut b = Body::new("", 0, 0);
    b.keys("Rab");
    assert_eq!(b.text(), "ab");
    let mut b = Body::new("a\tb", 0, 0);
    b.keys("Rxy");
    assert_eq!(b.text(), "xyb");
    b.key(code(KeyCode::Backspace));
    assert_eq!(b.text(), "x\tb", "the tab comes back");
    let mut b = Body::new("héllo", 0, 1);
    b.keys("Rxy");
    b.key(code(KeyCode::Backspace));
    assert_eq!((b.text(), b.caret()), ("hxllo".into(), Pos::new(0, 2)));
    let mut b = Body::new("abc", 0, 2);
    b.keys("Rx");
    b.key(code(KeyCode::Backspace));
    b.key(code(KeyCode::Backspace));
    assert_eq!((b.text(), b.caret()), ("abc".into(), Pos::new(0, 1)), "only moves before the start");
    let mut b = Body::new("abcdef\nxy", 0, 1);
    b.keys("R");
    b.engine.paste("12\n34", Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    // Vim's `ins_eol()` replaces nothing, so "34" goes over "de" (probed:
    // Vim 9.1 `R12<CR>34<Esc>` on "abcdef" gives "a12" / "34f").
    assert_eq!(b.text(), "a12\n34f\nxy", "a paste replaces char by char and a line break splits");
    let mut f = Field::new("abcdef", 1);
    f.keys("R");
    f.engine.paste("12\n34", Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(f.text(), "a12 34", "a field takes it flattened");
    assert!(!declined(&f.key(code(KeyCode::Enter))) || f.engine.mode() == Mode::Replace, "Enter is the field's");
}

/// `BS` over a Replace line break puts the blanks `Enter` stripped back in
/// their old order (probed in Vim 9.1: `R<CR><BS>` on "x \tb" at the space
/// keeps "x \tb"; the corpus texts have no mixed blanks after a non-blank).
#[test]
fn replace_bs_over_a_break_restores_mixed_blanks_in_order() {
    for text in ["x \tb", "x\t b"] {
        let mut b = Body::new(text, 0, 1);
        b.keys("R");
        b.key(code(KeyCode::Enter));
        assert_eq!(b.text(), "x\nb");
        b.key(code(KeyCode::Backspace));
        assert_eq!((b.text(), b.caret()), (text.to_string(), Pos::new(0, 1)));
    }
}

/// A Replace paste never autoindents (Vim pastes with `paste` set): a line
/// break in it adds no indent and strips no blanks, and `BS` back over it
/// joins the lines with nothing restored (Vim 9.1: `R` + paste "12\n  34"
/// at [1,4] of "  abcdef" / "xy" gives "  a12" / "  34" / "xy").
#[test]
fn a_replace_paste_never_autoindents() {
    let mut b = Body::new("  abcdef\nxy", 0, 3);
    b.keys("R");
    b.engine.paste("12\n  34", Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state });
    assert_eq!((b.text(), b.caret()), ("  a12\n  34\nxy".into(), Pos::new(1, 4)));
    for _ in 0..5 {
        b.key(code(KeyCode::Backspace));
    }
    assert_eq!((b.text(), b.caret()), ("  a12def\nxy".into(), Pos::new(0, 5)), "the paste's break joins back, nothing restored");
}

/// A cursor key or a click flushes the replace stack but keeps Replace; a
/// Replace session carries into the next field as Replace, and `gi` after
/// it opens Insert.
#[test]
fn replace_survives_a_split_and_a_carry() {
    let mut f = Field::new("abcd", 0);
    f.keys("Rxy");
    f.key(code(KeyCode::Left));
    assert_eq!(f.engine.mode(), Mode::Replace);
    f.key(code(KeyCode::Backspace));
    assert_eq!((f.text(), f.col()), ("xycd", 0), "the flushed stack has nothing to put back");
    f.keys("Z");
    assert_eq!(f.text(), "Zycd");
    let (mut other, mut os) = (LineInput::new("pq"), BufState::new());
    other.set_cursor(0);
    f.engine.carry(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os });
    assert_eq!(f.engine.mode(), Mode::Replace);
    f.engine.handle(k('W'), Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os }, &ViewCtx::default());
    assert_eq!(other.text(), "Wq");
    f.engine.handle(esc(), Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os }, &ViewCtx::default());
    f.engine.handle(k('0'), Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os }, &ViewCtx::default());
    f.engine.handle(k('g'), Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os }, &ViewCtx::default());
    f.engine.handle(k('i'), Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os }, &ViewCtx::default());
    assert_eq!(f.engine.mode(), Mode::Insert, "gi is always Insert");
}

/// Plan 3c Task 3: `ctrl+t` and `ctrl+d` indent a one-line field as they
/// indent the body (Deviation 11), rounding to shiftwidth, and are not
/// declined there. `0<C-d>` removes the indent; `.` repeats the keys.
#[test]
fn insert_ctrl_t_and_ctrl_d_indent_a_field() {
    let mut f = Field::new("   abc", 3);
    f.keys("i");
    assert!(!declined(&f.key(ctrl('t'))));
    assert_eq!((f.text(), f.col()), ("    abc", 4));
    f.key(ctrl('d'));
    f.key(ctrl('d'));
    assert_eq!((f.text(), f.col()), ("abc", 0));
    f.keys("0");
    f.key(ctrl('d'));
    assert_eq!(f.text(), "abc", "0 then ctrl+d: the 0 goes with the indent");
    f.key(ctrl('t'));
    f.keys("x");
    f.key(esc());
    assert_eq!(f.text(), "  xabc");
    // The record is every key: `<C-t><C-d><C-d>0<C-d><C-t>x`, so the
    // repeat ends on one shiftwidth (probed: Vim 9.1 gives "  xxabc").
    f.keys(".");
    assert_eq!(f.text(), "  xxabc", "the repeat replays the keys and types x");
    let mut q = Field::start("", 0, Start::InsertOnly);
    assert!(!declined(&q.key(ctrl('t'))), "a query box takes it too");
    assert_eq!(q.text(), "  ");
}

/// Task 3 review: Insert `ctrl+r` runs a register's `ctrl+t` and `ctrl+d`
/// chars in the body (Vim's `stuffescaped()`), and `ctrl+r ctrl+r` puts
/// them in literally. Probed in Vim 9.1 on "abc": register "x^Ty" gives
/// "  xyabc" (caret on y after Esc); literally, "x^Tyabc"; register
/// "^T^Tx^D" then `j.` (j fails on one line) gives "    xxabc". After
/// `ctrl+r`, `ctrl+t` and `ctrl+d` are the engine's own chords, swallowed
/// as Vim swallows them, never handed to the app.
#[test]
fn insert_ctrl_r_runs_ctrl_t_and_ctrl_d_from_a_register() {
    let mut b = Body::new("abc", 0, 0);
    b.engine.registers_mut().set_unnamed(Register { text: "x\u{14}y".into(), kind: RegKind::Char });
    b.keys("i");
    b.key(ctrl('r'));
    b.keys("\"");
    b.key(esc());
    assert_eq!((b.text(), b.caret()), ("  xyabc".into(), Pos::new(0, 3)));
    let mut b = Body::new("abc", 0, 0);
    b.engine.registers_mut().set_unnamed(Register { text: "x\u{14}y".into(), kind: RegKind::Char });
    b.keys("i");
    b.key(ctrl('r'));
    b.key(ctrl('r'));
    b.keys("\"");
    b.key(esc());
    assert_eq!((b.text(), b.caret()), ("x\u{14}yabc".into(), Pos::new(0, 2)), "ctrl+r ctrl+r: literal");
    let mut b = Body::new("abc", 0, 0);
    b.engine.registers_mut().set_unnamed(Register { text: "\u{14}\u{14}x\u{4}".into(), kind: RegKind::Char });
    b.keys("i");
    b.key(ctrl('r'));
    b.keys("\"");
    b.key(esc());
    assert_eq!(b.text(), "  xabc");
    b.keys("j.");
    assert_eq!((b.text(), b.caret()), ("    xxabc".into(), Pos::new(0, 4)), "the record holds the register's keys");
    for chord in ['t', 'd'] {
        let mut b = Body::new("  abc", 0, 2);
        b.keys("i");
        b.key(ctrl('r'));
        assert!(!declined(&b.key(ctrl(chord))), "ctrl+r ctrl+{chord} is swallowed");
        assert_eq!(b.text(), "  abc", "ctrl+r ctrl+{chord} does nothing");
    }
}

/// Plan 3c Task 4: the mode during `ctrl+o`, its echo, and the keys the
/// engine does not own there: Esc is inert and Insert resumes, a declined
/// key resumes Insert first, a query box declines `ctrl+o` itself.
#[test]
fn ctrl_o_runs_one_normal_command_then_resumes() {
    let mut f = Field::new("abc def", 0);
    f.keys("iX");
    f.key(ctrl('o'));
    assert_eq!(f.engine.mode(), Mode::InsertNormal { replace: false });
    f.keys("2");
    assert_eq!((f.engine.echo(), f.engine.mode()), ("2".into(), Mode::InsertNormal { replace: false }));
    f.keys("w");
    // Vim 9.1: `iX<C-o>2wY<Esc>` on "abc def" gives "Xabc deYf".
    assert_eq!((f.engine.mode(), f.col()), (Mode::Insert, 7));
    f.key(ctrl('o'));
    assert_eq!(f.key(esc()), Outcome::Consumed { changed: false, note: None, request: None }, "Esc is inert");
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.key(ctrl('o'));
    assert_eq!(f.keys("j"), Outcome::Declined { count: None, keys: vec![k('j')] }, "declined after resuming");
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.key(ctrl('o'));
    assert_eq!(f.key(ctrl('c')), Outcome::Declined { count: None, keys: vec![ctrl('c')] });
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.key(ctrl('o'));
    assert_eq!(f.key(ctrl('o')), Outcome::Declined { count: None, keys: vec![ctrl('o')] }, "Normal ctrl+o is the app's");
    assert_eq!(f.engine.mode(), Mode::Insert);
    let mut q = Field::start("ab", 1, Start::InsertOnly);
    assert!(declined(&q.key(ctrl('o'))));
    let mut r = Field::new("abc", 0);
    r.keys("Rx");
    r.key(ctrl('o'));
    assert_eq!(r.engine.mode(), Mode::InsertNormal { replace: true });
    r.keys("l");
    assert_eq!(r.engine.mode(), Mode::Replace);
    r.keys("Y");
    assert_eq!(r.text(), "xbY");
}

/// Review focus 3: a pending restart never outlives a session edge, and a
/// click resumes Insert as a command would.
#[test]
fn ctrl_o_at_the_session_edges() {
    let mut f = Field::new("abc", 0);
    f.keys("iX");
    f.key(ctrl('o'));
    f.engine.leave(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!(f.engine.mode(), Mode::Normal);
    f.engine.enter(Start::Normal, Seat::Keep, Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    f.keys("l");
    assert_eq!(f.engine.mode(), Mode::Normal, "nothing resumed");
    f.keys("iY");
    f.key(ctrl('o'));
    let (mut other, mut os) = (LineInput::new("pq"), BufState::new());
    other.set_cursor(0);
    f.engine.carry(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Target { buf: &mut OneLineBuf::new(&mut other), state: &mut os });
    assert_eq!(f.engine.mode(), Mode::Normal, "a restart carries as Normal");
    let mut f = Field::new("abc", 0);
    f.keys("iX");
    f.key(ctrl('o'));
    f.input.set_cursor(2);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    assert_eq!(f.engine.mode(), Mode::Insert, "a click is the command");
    f.keys("Z");
    assert_eq!(f.text(), "XaZbc");
    let mut f = Field::new("abc", 0);
    f.keys("iX");
    f.key(ctrl('o'));
    f.keys("v");
    f.engine.external_edit(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, |ed| {
        ed.splice(Pos::new(0, 0), Pos::new(0, 0), "Q");
        ed.set_cursor(Pos::new(0, 2));
    });
    assert_eq!(f.engine.mode(), Mode::Insert);
    f.keys("W");
    assert_eq!(f.text(), "QXWabc", "typing resumes at the caret");
    let mut f = Field::new("abc", 0);
    f.keys("iX");
    f.key(ctrl('o'));
    f.engine.paste("P", Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state });
    assert_eq!((f.engine.mode(), f.text()), (Mode::Insert, "XPabc"), "a paste resumes and types");
}

/// Task 4 review: a click that ends Visual opened inside `ctrl+o` is the
/// command, so Insert resumes where the click put the caret (Vim restarts
/// Insert once Visual is off).
#[test]
fn a_click_ending_visual_inside_ctrl_o_resumes_insert() {
    let mut f = Field::new("abc", 0);
    f.keys("iX");
    f.key(ctrl('o'));
    f.keys("v");
    assert_eq!(f.engine.mode(), Mode::Visual(Shape::Char));
    f.input.set_cursor(3);
    f.engine.settle(Target { buf: &mut OneLineBuf::new(&mut f.input), state: &mut f.state }, Settled::Click);
    assert_eq!((f.engine.mode(), f.engine.visual_anchor()), (Mode::Insert, None));
    f.keys("Z");
    assert_eq!(f.text(), "XabZc", "typing lands at the clicked caret");
}

/// Plan 3c Task 5: the body's `top` follows Vim's `update_topline` for the
/// viewport the app passes; a one-line field and a body with no viewport
/// never scroll.
#[test]
fn the_body_top_follows_the_caret() {
    let text: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
    let mut b = Body::new(&text.join("\n"), 0, 0);
    b.keys("G");
    assert_eq!(b.ed.viewport_offset().1, 80, "a 20-row view shows 81..100");
    b.keys("50G");
    assert_eq!(b.ed.viewport_offset().1, 40, "centred: 50 - 20/2");
    b.keys("gg");
    assert_eq!(b.ed.viewport_offset().1, 0);
    b.keys("21j");
    assert_eq!(b.ed.viewport_offset().1, 2, "one row below the window scrolls two");
    let mut f = Field::new("abc", 0);
    f.keys("$");
    assert_eq!(f.input.cursor(), 2);
}

/// Review focus 2: a tiny or missing viewport, and a text that shrank
/// under `top`, never panic and keep the caret visible.
#[test]
fn the_view_survives_odd_viewports_and_outside_changes() {
    let text: Vec<String> = (1..=50).map(|i| format!("l{i}")).collect();
    for rows in [Some(0), Some(1), Some(2), None] {
        let mut b = Body::new(&text.join("\n"), 0, 0);
        let ctx = ViewCtx { viewport_rows: rows };
        for key in [k('G'), k('g'), k('g'), k('2'), k('5'), k('G'), k('j'), k('k')] {
            b.engine.handle(key, Target { buf: &mut BodyBuf::new(&mut b.ed, &mut b.visual), state: &mut b.state }, &ctx);
        }
        let top = b.ed.viewport_offset().1;
        assert!(top <= b.caret().row, "{rows:?}: top {top} past the caret");
    }
    let mut b = Body::new(&text.join("\n"), 49, 0);
    b.keys("G");
    assert!(b.ed.viewport_offset().1 > 0);
    b.ed.lines = Lines::from("a\nb");
    b.ed.cursor = edtui::Index2::new(1, 0);
    b.keys("k");
    assert_eq!(b.ed.viewport_offset().1, 0, "clamped into the shrunk text");
}

/// Plan 3c Task 6: the scroll chords in a one-line field are declined with
/// their count in Normal (piece 4 pages the surface) and consumed as failed
/// motions in Visual; in the body with an operator pending they cancel it.
#[test]
fn scroll_chords_in_a_field_and_after_an_operator() {
    let mut f = Field::new("abc", 1);
    for c in ['d', 'u', 'f', 'b'] {
        assert_eq!(f.keys("3").clone(), Outcome::consumed());
        assert_eq!(f.key(ctrl(c)), Outcome::Declined { count: Some(3), keys: vec![ctrl(c)] }, "{c}");
    }
    f.keys("v");
    assert!(!declined(&f.key(ctrl('d'))));
    assert_eq!((f.engine.mode(), f.col()), (Mode::Visual(Shape::Char), 1));
    let mut b = Body::new("a\nb\nc", 0, 0);
    b.keys("d");
    assert_eq!(b.key(ctrl('d')), Outcome::Consumed { changed: false, note: None, request: None });
    assert!(!b.engine.pending(), "the operator is cancelled");
    assert_eq!(b.text(), "a\nb\nc");
    b.keys("3");
    b.key(ctrl('d'));
    assert_eq!(b.state.scroll, Some(3));
}
