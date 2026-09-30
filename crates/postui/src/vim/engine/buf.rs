//! Positions and the buffer abstraction (spec §3.2–§3.3). The only engine
//! file that names app types: it adapts `LineInput` (one-line fields) and
//! edtui's `EditorState` (the request body).

use crate::components::line_input::{LineInput, VisualShape};
use edtui::{EditorMode, EditorState, Index2, RowIndex};
use std::borrow::Cow;

/// A position in chars, never bytes (spec §3.2). A one-line buffer always
/// has `row == 0`. Ordered by row, then column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// What a buffer paints for the engine's mode (see [`TextBuf::show`]): in
/// Visual, the selection (inclusive, the caret is the moving end). Paint
/// only: the engine never reads it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Normal,
    Insert,
    Visual { anchor: Pos, line: bool },
}

/// A selection the mouse or a GUI key made (never the engine's own
/// paint), in Vim's inclusive geometry: `head` is the caret, on the last
/// selected char. `Engine::settle` adopts it as Visual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiSel {
    pub anchor: Pos,
    pub head: Pos,
}

/// The engine's only view of text (spec §3.2). Every change goes through
/// [`TextBuf::splice`], so recording undo and `.` happens in one place.
pub trait TextBuf {
    /// `false` for a one-line field: `splice` never receives `'\n'`.
    const MULTILINE: bool;
    /// At least 1: an empty buffer is one empty line.
    fn line_count(&self) -> usize;
    fn line(&self, row: usize) -> Cow<'_, [char]>;
    fn cursor(&self) -> Pos;
    /// Moves the caret and nothing else of the text. `BodyBuf` stores `at`
    /// raw, unclamped. `OneLineBuf` goes through `LineInput::set_cursor`,
    /// which clamps the column to the text and drops any GUI selection.
    fn set_cursor(&mut self, at: Pos);
    /// The one mutation: replaces `[start, end)` with `text`. `end` may be
    /// `(row + 1, 0)` to take the line break after `row`; `text` may hold
    /// `'\n'` only when [`TextBuf::MULTILINE`].
    fn splice(&mut self, start: Pos, end: Pos, text: &str);
    /// Paint only: the Visual span, and for the body edtui's mode (its
    /// caret shape). A one-line field has no caret shape of its own; the
    /// app draws its caret from `Engine::mode` (spec §3.3).
    fn show(&mut self, paint: Paint);
    /// Rows scrolled off the top (always 0 for a one-line field).
    fn top(&self) -> usize;
    fn set_top(&mut self, row: usize);
    /// A selection the mouse or a GUI key made, if any (deviation 2).
    fn gui_selection(&self) -> Option<GuiSel>;
    fn clear_gui_selection(&mut self);

    fn line_len(&self, row: usize) -> usize {
        self.line(row).len()
    }

    /// The text of `[start, end)`, with `'\n'` between rows.
    fn slice(&self, start: Pos, end: Pos) -> String {
        let mut out = String::new();
        for row in start.row..=end.row.min(self.line_count() - 1) {
            let line = self.line(row);
            let from = if row == start.row { start.col.min(line.len()) } else { 0 };
            let to = if row == end.row { end.col.min(line.len()) } else { line.len() };
            out.extend(&line[from..to.max(from)]);
            if row < end.row {
                out.push('\n');
            }
        }
        out
    }

    fn text(&self) -> String {
        let last = self.line_count() - 1;
        self.slice(Pos::new(0, 0), Pos::new(last, self.line_len(last)))
    }
}

/// A one-line field: the URL, a table cell, a prompt, a form field.
pub struct OneLineBuf<'a>(&'a mut LineInput);

impl<'a> OneLineBuf<'a> {
    pub fn new(input: &'a mut LineInput) -> Self {
        Self(input)
    }
}

impl TextBuf for OneLineBuf<'_> {
    const MULTILINE: bool = false;

    fn line_count(&self) -> usize {
        1
    }

    fn line(&self, _row: usize) -> Cow<'_, [char]> {
        Cow::Owned(self.0.text().chars().collect())
    }

    fn line_len(&self, _row: usize) -> usize {
        self.0.text().chars().count()
    }

    fn text(&self) -> String {
        self.0.text().to_string()
    }

    fn cursor(&self) -> Pos {
        Pos::new(0, self.0.cursor())
    }

    fn set_cursor(&mut self, at: Pos) {
        self.0.set_cursor(at.col);
    }

    fn splice(&mut self, start: Pos, end: Pos, text: &str) {
        debug_assert!(!text.contains('\n'), "a one-line buffer never takes a line break");
        self.0.splice_raw(start.col, end.col, text);
    }

    fn show(&mut self, paint: Paint) {
        self.0.set_visual(match paint {
            Paint::Visual { anchor, line } => {
                Some((anchor.col, if line { VisualShape::Line } else { VisualShape::Char }))
            }
            Paint::Normal | Paint::Insert => None,
        });
    }

    fn top(&self) -> usize {
        0
    }

    fn set_top(&mut self, _row: usize) {}

    /// The GUI `anchor` only: the engine's Visual paint lives apart from it
    /// (`LineInput::vim_visual`), so it is never read back as one.
    fn gui_selection(&self) -> Option<GuiSel> {
        let (start, end) = self.0.selection()?;
        let (anchor, head) = if self.0.cursor() == end { (start, end - 1) } else { (end - 1, start) };
        Some(GuiSel { anchor: Pos::new(0, anchor), head: Pos::new(0, head) })
    }

    fn clear_gui_selection(&mut self) {
        self.0.clear_selection();
    }
}

/// The body's Visual span as [`BodyBuf::show`] paints it: inclusive, in
/// edtui's coordinates, `None` outside Visual. edtui's own `Selection`
/// can't be built from outside the crate, so the editor draws this as a
/// `Highlight` ahead of its JSON colours (deviation 1). The editor is the
/// only writer of `EditorState::highlights`.
pub type BodyVisual = Option<(Index2, Index2)>;

/// The request body. Uses only edtui's public surface: `lines`, `cursor`,
/// `mode`, `selection` and the viewport offset. Never `execute`, edtui's
/// undo, its event handler or its clipboard, and never `highlights`.
pub struct BodyBuf<'a> {
    state: &'a mut EditorState,
    visual: &'a mut BodyVisual,
}

impl<'a> BodyBuf<'a> {
    pub fn new(state: &'a mut EditorState, visual: &'a mut BodyVisual) -> Self {
        Self { state, visual }
    }
}

impl TextBuf for BodyBuf<'_> {
    const MULTILINE: bool = true;

    fn line_count(&self) -> usize {
        self.state.lines.len().max(1)
    }

    fn line(&self, row: usize) -> Cow<'_, [char]> {
        Cow::Borrowed(self.state.lines.get(RowIndex::new(row)).map(Vec::as_slice).unwrap_or(&[]))
    }

    fn cursor(&self) -> Pos {
        Pos::new(self.state.cursor.row, self.state.cursor.col)
    }

    fn set_cursor(&mut self, at: Pos) {
        self.state.cursor = Index2::new(at.row, at.col);
    }

    fn splice(&mut self, start: Pos, end: Pos, text: &str) {
        let lines = &mut self.state.lines;
        if lines.is_empty() {
            lines.push(Vec::<char>::new());
        }
        // The engine never panics: a stale undo step (the text changed
        // outside the engine) may name rows past the end, so clamp the
        // range into the text (an `end` past the last row means "to the end
        // of the last row") and keep `start <= end`.
        let last = lines.len() - 1;
        let start = Pos::new(start.row.min(last), start.col);
        let end = if end.row > last { Pos::new(last, usize::MAX) } else { end };
        let end = end.max(start);
        let head: Vec<char> = lines
            .get(RowIndex::new(start.row))
            .map(|r| r[..start.col.min(r.len())].to_vec())
            .unwrap_or_default();
        let tail: Vec<char> = lines
            .get(RowIndex::new(end.row))
            .map(|r| r[end.col.min(r.len())..].to_vec())
            .unwrap_or_default();
        let mut rows: Vec<Vec<char>> = text.split('\n').map(|s| s.chars().collect()).collect();
        let mut first = head;
        first.append(&mut rows[0]);
        rows[0] = first;
        rows.last_mut().expect("split yields at least one row").extend(tail);
        let mut after = lines.split_off(Index2::new(end.row + 1, 0));
        let _ = lines.extract_rows(start.row..);
        for row in rows {
            lines.push(row);
        }
        lines.append(&mut after);
        // Keep edtui's viewport inside the text (review focus 3).
        let (x, y) = self.state.viewport_offset();
        let last = self.state.lines.len().saturating_sub(1);
        if y > last {
            self.state.set_viewport_offset(x, last);
        }
    }

    fn show(&mut self, paint: Paint) {
        self.state.mode = match paint {
            Paint::Normal => EditorMode::Normal,
            Paint::Insert => EditorMode::Insert,
            Paint::Visual { .. } => EditorMode::Visual,
        };
        *self.visual = match paint {
            Paint::Visual { anchor, line } => {
                let caret = self.cursor();
                let (lo, hi) = if anchor <= caret { (anchor, caret) } else { (caret, anchor) };
                let (from, to) = if line {
                    (Pos::new(lo.row, 0), Pos::new(hi.row, self.line_len(hi.row).saturating_sub(1)))
                } else {
                    (lo, hi)
                };
                Some((Index2::new(from.row, from.col), Index2::new(to.row, to.col)))
            }
            Paint::Normal | Paint::Insert => None,
        };
    }

    fn top(&self) -> usize {
        self.state.viewport_offset().1
    }

    fn set_top(&mut self, row: usize) {
        let x = self.state.viewport_offset().0;
        self.state.set_viewport_offset(x, row);
    }

    fn gui_selection(&self) -> Option<GuiSel> {
        let sel = self.state.selection.as_ref()?;
        Some(GuiSel {
            anchor: Pos::new(sel.start.row, sel.start.col),
            head: Pos::new(sel.end.row, sel.end.col),
        })
    }

    fn clear_gui_selection(&mut self) {
        self.state.selection = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edtui::actions::{Execute, SwitchMode};
    use edtui::{EditorView, Lines};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    fn rows<B: TextBuf>(b: &B) -> Vec<String> {
        (0..b.line_count()).map(|r| b.line(r).iter().collect()).collect()
    }

    /// The part of the contract a one-line buffer can meet (spec §8.3).
    /// `b` starts as "héllo wörld".
    fn one_line_contract<B: TextBuf>(b: &mut B) {
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line_len(0), 11);
        b.splice(Pos::new(0, 1), Pos::new(0, 2), "e");
        assert_eq!(rows(b), ["hello wörld"]);
        b.splice(Pos::new(0, 6), Pos::new(0, 11), "");
        assert_eq!(rows(b), ["hello "]);
        b.splice(Pos::new(0, 0), Pos::new(0, 0), "日本 ");
        assert_eq!(rows(b), ["日本 hello "]);
        assert_eq!(b.slice(Pos::new(0, 0), Pos::new(0, 2)), "日本");
        assert_eq!(b.text(), "日本 hello ");
        b.set_cursor(Pos::new(0, 3));
        assert_eq!(b.cursor(), Pos::new(0, 3));
        assert_eq!(b.top(), 0);
    }

    #[test]
    fn one_line_buf_meets_the_contract() {
        let mut input = LineInput::new("héllo wörld");
        one_line_contract(&mut OneLineBuf::new(&mut input));
    }

    #[test]
    fn body_buf_meets_the_one_line_contract() {
        let mut state = EditorState::new(Lines::from("héllo wörld"));
        one_line_contract(&mut BodyBuf::new(&mut state, &mut None));
    }

    #[test]
    fn body_splices_across_line_breaks() {
        let mut state = EditorState::new(Lines::from("ab\ncd\nef"));
        let mut visual = None;
        let mut b = BodyBuf::new(&mut state, &mut visual);
        b.splice(Pos::new(0, 1), Pos::new(1, 1), "");
        assert_eq!(rows(&b), ["ad", "ef"]);
        b.splice(Pos::new(0, 2), Pos::new(1, 0), "");
        assert_eq!(rows(&b), ["adef"], "an end of (row + 1, 0) takes the line break");
        b.splice(Pos::new(0, 2), Pos::new(0, 2), "X\nY\n");
        assert_eq!(rows(&b), ["adX", "Y", "ef"]);
        assert_eq!(b.slice(Pos::new(0, 2), Pos::new(2, 1)), "X\nY\ne");
        assert_eq!(b.text(), "adX\nY\nef");
        b.splice(Pos::new(0, 0), Pos::new(2, 2), "");
        assert_eq!(rows(&b), [""]);
    }

    /// Review focus 1: edtui holds an empty text as zero rows.
    #[test]
    fn an_empty_body_is_one_empty_line_and_still_renders() {
        let mut state = EditorState::new(Lines::from(""));
        assert_eq!(state.lines.len(), 0);
        let mut visual = None;
        let mut b = BodyBuf::new(&mut state, &mut visual);
        assert_eq!(b.line_count(), 1);
        assert!(b.line(0).is_empty());
        assert_eq!(b.text(), "");
        b.splice(Pos::new(0, 0), Pos::new(0, 0), "x");
        assert_eq!(rows(&b), ["x"]);
        b.splice(Pos::new(0, 0), Pos::new(0, 1), "");
        assert_eq!(rows(&b), [""]);
        let area = Rect::new(0, 0, 20, 5);
        EditorView::new(&mut state).render(area, &mut Buffer::empty(area));
    }

    /// Review focus 3: a big delete leaves edtui's viewport past the end.
    #[test]
    fn a_render_after_a_big_delete_does_not_panic() {
        let text: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
        let mut state = EditorState::new(Lines::from(text.join("\n")));
        state.set_viewport_offset(0, 50);
        state.cursor = Index2::new(55, 0);
        let mut visual = None;
        let mut b = BodyBuf::new(&mut state, &mut visual);
        b.splice(Pos::new(5, 0), Pos::new(59, 7), "");
        b.set_cursor(Pos::new(5, 0));
        assert_eq!(b.line_count(), 6);
        let area = Rect::new(0, 0, 40, 10);
        EditorView::new(&mut state).render(area, &mut Buffer::empty(area));
        assert!(state.viewport_offset().1 <= 5);
    }

    #[test]
    fn one_line_show_paints_and_clears_the_engine_visual() {
        let mut input = LineInput::new("abcdef");
        input.set_cursor(3);
        let mut b = OneLineBuf::new(&mut input);
        b.show(Paint::Visual { anchor: Pos::new(0, 1), line: false });
        assert_eq!(input.visual(), Some((1, VisualShape::Char)));
        assert_eq!(input.paint_span(), Some((1, 4)));
        OneLineBuf::new(&mut input).show(Paint::Normal);
        assert_eq!(input.visual(), None);
    }

    #[test]
    fn body_show_records_the_visual_span_and_leaves_selection_and_highlights_alone() {
        let mut state = EditorState::new(Lines::from("abc\ndef\nghi"));
        let mut visual = None;
        state.cursor = Index2::new(1, 1);
        let mut b = BodyBuf::new(&mut state, &mut visual);
        b.show(Paint::Visual { anchor: Pos::new(0, 2), line: false });
        assert_eq!(state.mode, EditorMode::Visual);
        assert_eq!(state.selection, None);
        assert!(state.highlights.is_empty());
        assert_eq!(visual, Some((Index2::new(0, 2), Index2::new(1, 1))));
        BodyBuf::new(&mut state, &mut visual).show(Paint::Visual { anchor: Pos::new(2, 1), line: true });
        assert_eq!(visual, Some((Index2::new(1, 0), Index2::new(2, 2))));
        BodyBuf::new(&mut state, &mut visual).show(Paint::Insert);
        assert_eq!(state.mode, EditorMode::Insert);
        assert_eq!(visual, None);
    }

    #[test]
    fn gui_selection_reads_vims_inclusive_geometry() {
        let mut input = LineInput::new("abcdef");
        input.set_cursor(1);
        input.begin_mouse_selection();
        input.extend_mouse_selection_to(4);
        let mut b = OneLineBuf::new(&mut input);
        assert_eq!(b.gui_selection(), Some(GuiSel { anchor: Pos::new(0, 1), head: Pos::new(0, 3) }));
        b.clear_gui_selection();
        assert_eq!(b.gui_selection(), None);

        let mut input = LineInput::new("abcdef");
        input.set_cursor(4);
        input.begin_mouse_selection();
        input.extend_mouse_selection_to(1);
        assert_eq!(
            OneLineBuf::new(&mut input).gui_selection(),
            Some(GuiSel { anchor: Pos::new(0, 3), head: Pos::new(0, 1) }),
            "a leftward sweep: the caret is the low end"
        );

        let mut input = LineInput::new("abcdef");
        input.set_cursor(1);
        input.set_visual(Some((3, VisualShape::Char)));
        assert_eq!(OneLineBuf::new(&mut input).gui_selection(), None, "the engine's own paint is not a GUI selection");

        let mut state = EditorState::new(Lines::from("abc\ndef"));
        state.cursor = Index2::new(0, 1);
        SwitchMode(EditorMode::Visual).execute(&mut state);
        state.selection.as_mut().unwrap().end = Index2::new(1, 2);
        let mut visual = None;
        let mut b = BodyBuf::new(&mut state, &mut visual);
        assert_eq!(b.gui_selection(), Some(GuiSel { anchor: Pos::new(0, 1), head: Pos::new(1, 2) }));
        b.clear_gui_selection();
        assert_eq!(state.selection, None);
    }
}
