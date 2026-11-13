//! The variable tooltip's text as the last frame painted it, and the mouse
//! selection over it (#294).
//!
//! The tip is read-only text, as the response view is, and its selection
//! follows the response view's rules: a press anchors on a char, a drag
//! sweeps an inclusive char range to the char under the pointer, a double
//! click takes the shared mouse word and a drag after it grows by words
//! ([`crate::text::mouse`]). The app drives it through the same sweep
//! (`TextDrag::Tip`) and copies it through the same ctrl+c path
//! (`App::active_selection_text`).

use std::borrow::Cow;

use crate::text::graphemes::{grapheme_at_col, grapheme_edges, last_grapheme, whole_graphemes};
use crate::text::mouse::{Span, grow_by_words, sweep, word_at};
use crate::text::{Lines, Pos};

/// One painted row of the tip: its text as painted, and what a copy puts
/// between the row above and this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TipLine {
    pub text: String,
    /// `"\n"` for a row that starts a line of its own, `""` for a value
    /// wrapped mid-text, `" "` for a description wrapped at a space.
    pub join: &'static str,
}

impl TipLine {
    /// A row that starts a line of its own.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), join: "\n" }
    }
}

/// The tip's painted rows and the selection over them.
#[derive(Debug, Default)]
pub struct TipText {
    /// The screen cell the first row's text starts at.
    origin: (u16, u16),
    lines: Vec<TipLine>,
    /// Anchor end, then head end: chars, both inclusive.
    sel: Option<(Pos, Pos)>,
    anchor: Option<Pos>,
    /// The word a double click took, which a drag after it grows from.
    word: Option<Span>,
}

impl TipText {
    /// Takes the rows a frame painted at `origin`. Rows or a place that
    /// differ from the last frame's (another tip, a reveal, a changed
    /// value) drop the selection, which was made over the old text.
    pub fn show(&mut self, origin: (u16, u16), lines: Vec<TipLine>) {
        if self.origin != origin || self.lines != lines {
            self.clear_selection();
            self.origin = origin;
            self.lines = lines;
        }
    }

    /// No tip drawn: no rows, no selection.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Drops the selection; returns whether there was one.
    pub fn clear_selection(&mut self) -> bool {
        let had = self.sel.is_some();
        self.sel = None;
        self.anchor = None;
        self.word = None;
        had
    }

    /// The display column `x` falls on, counted from the rows' left edge.
    fn disp_col(&self, x: u16) -> usize {
        usize::from(x.saturating_sub(self.origin.0))
    }

    /// The (row, char) cell under a screen position, pulled onto the rows
    /// from anywhere (the panel's padding, or past it in a sweep). Past a
    /// row's end, its last grapheme. `None` with no rows.
    fn cell_at(&self, x: u16, y: u16) -> Option<Pos> {
        let last = self.lines.len().checked_sub(1)?;
        let row = usize::from(y.saturating_sub(self.origin.1)).min(last);
        let text = &self.lines[row].text;
        let col = grapheme_at_col(text, self.disp_col(x), (0, 0, 0))
            .unwrap_or_else(|| last_grapheme(text));
        Some(Pos::new(row, col))
    }

    /// A press: anchors a sweep at the char under it, dropping any
    /// selection. Returns whether it anchored.
    pub fn begin_at(&mut self, x: u16, y: u16) -> bool {
        self.clear_selection();
        self.anchor = self.cell_at(x, y);
        self.anchor.is_some()
    }

    /// A double click: selects the mouse word under it and anchors a
    /// word-wise sweep. `false` on an empty row.
    pub fn select_word_at(&mut self, x: u16, y: u16) -> bool {
        self.clear_selection();
        let Some(at) = self.cell_at(x, y) else {
            return false;
        };
        let line = self.line(at.row);
        let Some((s, e)) = word_at(&line, at.col) else {
            return false;
        };
        let (s, e) = (Pos::new(at.row, s), Pos::new(at.row, e));
        self.sel = Some((s, e));
        self.anchor = Some(s);
        self.word = Some(Span { fixed: s, moving: e });
        true
    }

    /// A sweep to the pointer, by the response view's rules
    /// ([`crate::text::mouse::sweep`]). After a double click it grows by
    /// whole words.
    pub fn drag_to(&mut self, x: u16, y: u16) -> bool {
        let Some(head) = self.cell_at(x, y) else {
            return false;
        };
        if let Some(word) = self.word {
            let grown = grow_by_words(&*self, word, head);
            self.sel = Some((grown.fixed, grown.moving));
            return true;
        }
        let Some(anchor) = self.anchor else {
            return false;
        };
        let rows = &self.lines;
        self.sel = sweep(
            anchor,
            head,
            self.disp_col(x),
            |col| grapheme_at_col(&rows[head.row].text, col, (0, 0, 0)).is_some(),
            |row| last_grapheme(&rows[row].text),
        );
        true
    }

    /// The selection on row `i`, half-open chars `[from, to)` widened to
    /// whole graphemes. `None` when none of it is on the row.
    fn sel_span(&self, i: usize) -> Option<(usize, usize)> {
        let (a, b) = self.sel?;
        let (s, e) = (a.min(b), a.max(b));
        if i < s.row || i > e.row {
            return None;
        }
        let text = &self.lines.get(i)?.text;
        let len = text.chars().count();
        let from = if i == s.row { s.col } else { 0 };
        let to = if i == e.row { (e.col + 1).min(len) } else { len };
        if to <= from {
            return None;
        }
        Some(whole_graphemes(grapheme_edges(text, to).as_deref(), (from, to)))
    }

    /// The display columns `[start, end)` the selection covers on row `i`,
    /// counted from the rows' left edge, for the paint.
    pub fn selected_cells(&self, i: usize) -> Option<(usize, usize)> {
        let (from, to) = self.sel_span(i)?;
        let (mut start, mut end) = (0, 0);
        for g in crate::ui::char_graphemes(&self.lines[i].text) {
            if g.char < from {
                start += g.width;
            }
            if g.char < to {
                end += g.width;
            }
        }
        Some((start, end))
    }

    /// The selected text: each row's part as painted, joined as the rows
    /// join ([`TipLine::join`]). `None` without a selection, or when it
    /// covers no text (a sweep ended on a blank row it started on).
    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.sel?;
        let (s, e) = (a.min(b), a.max(b));
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate().take(e.row + 1).skip(s.row) {
            if i > s.row {
                out.push_str(line.join);
            }
            if let Some((from, to)) = self.sel_span(i) {
                out.extend(line.text.chars().skip(from).take(to - from));
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

/// The rows as the mouse words read them.
impl Lines for TipText {
    fn line_count(&self) -> usize {
        self.lines.len().max(1)
    }

    fn line(&self, row: usize) -> Cow<'_, [char]> {
        Cow::Owned(self.lines.get(row).map(|l| l.text.chars().collect()).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tip of `rows` at (10, 5), each row given as (text, join).
    fn tip(rows: &[(&str, &'static str)]) -> TipText {
        let mut t = TipText::default();
        let lines = rows.iter().map(|&(text, join)| TipLine { text: text.into(), join }).collect();
        t.show((10, 5), lines);
        t
    }

    #[test]
    fn a_sweep_selects_inclusively_and_back_on_the_anchor_selects_nothing() {
        let mut t = tip(&[("hello world", "\n")]);
        assert!(t.begin_at(10, 5));
        assert!(t.drag_to(14, 5));
        assert_eq!(t.selected_text().as_deref(), Some("hello"));
        assert_eq!(t.selected_cells(0), Some((0, 5)));
        t.drag_to(10, 5);
        assert_eq!(t.selected_text(), None, "back on the anchor");
        t.drag_to(8, 5);
        assert_eq!(t.selected_text(), None, "left of the rows is their first column");
    }

    #[test]
    fn rows_copy_with_their_joins() {
        let mut t = tip(&[
            ("abcdef", "\n"),
            ("ghi", ""),
            ("", "\n"),
            ("name  qa", "\n"),
            ("one", "\n"),
            ("two", " "),
        ]);
        t.begin_at(10, 5);
        t.drag_to(40, 10);
        assert_eq!(t.selected_text().as_deref(), Some("abcdefghi\n\nname  qa\none two"));
    }

    #[test]
    fn the_ragged_edges_are_boundaries() {
        let mut t = tip(&[("abc", "\n"), ("def", "\n")]);
        t.begin_at(11, 5);
        t.drag_to(10, 6);
        assert_eq!(t.selected_text().as_deref(), Some("bc"), "down onto a row's first cell");
        t.begin_at(11, 6);
        t.drag_to(30, 5);
        assert_eq!(t.selected_text().as_deref(), Some("de"), "up past a row's end");
    }

    #[test]
    fn a_sweep_from_a_blank_row_onto_the_next_rows_first_cell_selects_nothing() {
        let mut t = tip(&[("abc", "\n"), ("", "\n"), ("def", "\n")]);
        t.begin_at(10, 6);
        t.drag_to(10, 7);
        assert_eq!(t.selected_text(), None);
        assert_eq!(t.selected_cells(1), None);
    }

    #[test]
    fn a_double_click_takes_the_word_and_a_drag_grows_by_words() {
        let mut t = tip(&[("https://qa.example.com", "\n")]);
        assert!(t.select_word_at(19, 5));
        assert_eq!(t.selected_text().as_deref(), Some("qa"));
        t.drag_to(22, 5);
        assert_eq!(t.selected_text().as_deref(), Some("qa.example"));
        assert!(!tip(&[("", "\n")]).select_word_at(10, 5), "an empty row has no word");
    }

    #[test]
    fn wide_glyphs_select_and_paint_whole() {
        let mut t = tip(&[("a日b", "\n")]);
        t.begin_at(10, 5);
        t.drag_to(12, 5);
        assert_eq!(
            t.selected_text().as_deref(),
            Some("a日"),
            "the glyph's second cell is the glyph"
        );
        assert_eq!(t.selected_cells(0), Some((0, 3)));
    }

    #[test]
    fn new_text_or_a_new_place_drops_the_selection() {
        let mut t = tip(&[("abc", "\n")]);
        t.begin_at(10, 5);
        t.drag_to(12, 5);
        t.show((10, 5), vec![TipLine::new("abc")]);
        assert!(t.selected_text().is_some(), "the same frame again keeps it");
        t.show((10, 5), vec![TipLine::new("xyz")]);
        assert_eq!(t.selected_text(), None, "other text");
        t.begin_at(10, 5);
        t.drag_to(12, 5);
        t.show((11, 5), vec![TipLine::new("xyz")]);
        assert_eq!(t.selected_text(), None, "another place");
    }
}
