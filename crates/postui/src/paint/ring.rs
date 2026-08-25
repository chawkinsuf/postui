//! Cell-tight accent ring: a 1-cell-thick border stroked with one-eighth
//! block glyphs, hugging the *inside* of `area`'s own border cells so the
//! accent reads as a hairline hugging the content rather than a full-block
//! frame. Used for focus/open affordances (dropdown popups, the
//! body-editor's focus outline) where a heavier border would be too loud.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

/// Strokes a 1-cell accent ring inside `area`'s own border cells: the top
/// row gets `▁` (lower one-eighth block — hangs down toward the content
/// just below it), the bottom row gets `▔` (upper one-eighth — hangs up
/// toward the content just above it), the left column gets `▕` (right
/// one-eighth — hugs the content just to its right), the right column gets
/// `▏` (left one-eighth — hugs the content just to its left).
///
/// The top and bottom edges own the corner cells outright, running the
/// full width of `area`. The left and right edges span only the rows
/// strictly between the top and bottom edges (one cell shorter at each
/// end), so the vertical strokes stop short of the corner rather than
/// overlapping a corner glyph there. This keeps each corner cell a single
/// clean horizontal stroke instead of a glyph that reads as a cross where
/// horizontal and vertical strokes visually overshoot into it.
///
/// `color` (fg) is the accent color; `on` (bg) is the surface the ring
/// paints over (so it composes with whatever's already behind `area`).
/// A degenerate `area` (width or height under 2) paints nothing.
pub fn ring(buf: &mut Buffer, area: Rect, color: Color, on: Color) {
    if area.width < 2 || area.height < 2 {
        return;
    }

    let left = area.left();
    let right = area.right() - 1;
    let top = area.top();
    let bottom = area.bottom() - 1;

    let put = |buf: &mut Buffer, x: u16, y: u16, glyph: &str| {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(glyph);
            cell.set_fg(color);
            cell.set_bg(on);
        }
    };

    // Top/bottom edges, full width — these own the corner cells.
    for x in left..=right {
        put(buf, x, top, "▁");
        put(buf, x, bottom, "▔");
    }
    // Left/right edges, excluding the corner rows so nothing overlaps the
    // corner cells the horizontal edges already own.
    for y in (top + 1)..bottom {
        put(buf, left, y, "▕");
        put(buf, right, y, "▏");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::fill;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    #[test]
    fn ring_strokes_edges_and_corners_on_a_6x4_rect() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(6, 4)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 6, 4), theme.panel);
            ring(
                f.buffer_mut(),
                Rect::new(0, 0, 6, 4),
                theme.accent,
                theme.panel,
            );
        })
        .unwrap();

        // Top edge, full width including corner columns, row 0.
        for x in 0..6 {
            let c = buf_cell(&term, x, 0);
            assert_eq!(c.symbol(), "▁", "top edge at x={x}");
            assert_eq!(c.fg, theme.accent);
            assert_eq!(c.bg, theme.panel);
        }
        // Bottom edge, full width including corner columns, row 3.
        for x in 0..6 {
            let c = buf_cell(&term, x, 3);
            assert_eq!(c.symbol(), "▔", "bottom edge at x={x}");
        }
        // Left edge, shortened by one row at each end (rows 1..3), col 0 —
        // does not reach into the corner rows the top/bottom edges own.
        for y in 1..3 {
            let c = buf_cell(&term, 0, y);
            assert_eq!(c.symbol(), "▕", "left edge at y={y}");
        }
        // Right edge, shortened by one row at each end (rows 1..3), col 5.
        for y in 1..3 {
            let c = buf_cell(&term, 5, y);
            assert_eq!(c.symbol(), "▏", "right edge at y={y}");
        }

        // No Legacy Computing corner glyphs anywhere — the top/bottom
        // edges own the corner cells outright (asserted above as part of
        // the full-width top/bottom rows).

        // Interior stays untouched.
        assert_eq!(buf_cell(&term, 2, 1).bg, theme.panel);
        assert_eq!(buf_cell(&term, 2, 1).symbol(), " ");
    }

    #[test]
    fn degenerate_area_paints_nothing() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(4, 4)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 4, 4), theme.panel);
            ring(
                f.buffer_mut(),
                Rect::new(1, 1, 1, 1),
                theme.accent,
                theme.panel,
            );
            ring(
                f.buffer_mut(),
                Rect::new(1, 1, 0, 3),
                theme.accent,
                theme.panel,
            );
        })
        .unwrap();
        for y in 0..4 {
            for x in 0..4 {
                let c = buf_cell(&term, x, y);
                assert_eq!(c.symbol(), " ");
                assert_eq!(c.bg, theme.panel);
            }
        }
    }
}
