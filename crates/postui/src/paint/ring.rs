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
/// `▏` (left one-eighth — hugs the content just to its left). The four
/// corner cells combine the two strokes that meet there: e.g. the top-left
/// corner joins the top edge's *lower* stroke with the left edge's *right*
/// stroke, so it uses the "right and lower" eighth-block glyph.
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

    // Top/bottom edges, excluding the corner columns.
    for x in (left + 1)..right {
        put(buf, x, top, "▁");
        put(buf, x, bottom, "▔");
    }
    // Left/right edges, excluding the corner rows.
    for y in (top + 1)..bottom {
        put(buf, left, y, "▕");
        put(buf, right, y, "▏");
    }

    // Corners: each combines the vertical edge's side with the horizontal
    // edge's side that meet there.
    put(buf, left, top, "\u{1FB7F}"); // right and lower
    put(buf, right, top, "\u{1FB7C}"); // left and lower
    put(buf, left, bottom, "\u{1FB7E}"); // right and upper
    put(buf, right, bottom, "\u{1FB7D}"); // left and upper
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

        // Top edge (non-corner columns 1..5), row 0.
        for x in 1..5 {
            let c = buf_cell(&term, x, 0);
            assert_eq!(c.symbol(), "▁", "top edge at x={x}");
            assert_eq!(c.fg, theme.accent);
            assert_eq!(c.bg, theme.panel);
        }
        // Bottom edge (non-corner columns 1..5), row 3.
        for x in 1..5 {
            let c = buf_cell(&term, x, 3);
            assert_eq!(c.symbol(), "▔", "bottom edge at x={x}");
        }
        // Left edge (non-corner rows 1..3), col 0.
        for y in 1..3 {
            let c = buf_cell(&term, 0, y);
            assert_eq!(c.symbol(), "▕", "left edge at y={y}");
        }
        // Right edge (non-corner rows 1..3), col 5.
        for y in 1..3 {
            let c = buf_cell(&term, 5, y);
            assert_eq!(c.symbol(), "▏", "right edge at y={y}");
        }

        // Corners.
        assert_eq!(buf_cell(&term, 0, 0).symbol(), "\u{1FB7F}"); // top-left
        assert_eq!(buf_cell(&term, 5, 0).symbol(), "\u{1FB7C}"); // top-right
        assert_eq!(buf_cell(&term, 0, 3).symbol(), "\u{1FB7E}"); // bottom-left
        assert_eq!(buf_cell(&term, 5, 3).symbol(), "\u{1FB7D}"); // bottom-right
        for (x, y) in [(0, 0), (5, 0), (0, 3), (5, 3)] {
            let c = buf_cell(&term, x, y);
            assert_eq!(c.fg, theme.accent);
            assert_eq!(c.bg, theme.panel);
        }

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
