//! Fractional vertical span helper: paints a horizontal band across fractional row boundaries
//! using lower-block glyphs (▁▂▃▄▅▆▇█) for precise vertical positioning.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

/// Paints a full-width horizontal band covering rows `y0..y1` (fractional row
/// coordinates, `y1 > y0`) across columns `x0..x1`. Whole-covered rows are
/// filled cells; the fractional top edge row shows `fill` in its lower
/// covered fraction and the fractional bottom edge row in its upper one,
/// both drawn with the lower-block family `▁▂▃▄▅▆▇` (see [`edge_row`]).
///
/// A band inside one row is one glyph its own height, set against whichever
/// edge of the row it is nearer: one glyph can show fill at a cell's top or
/// bottom but not in its middle, so the nearer edge is where it lands
/// closest to where it is. Clamps to buffer area.
///
/// Every row the band paints keeps nothing of what was under it, modifiers
/// included, so a struck or underlined page row never shows through. A row
/// the band covers less than half an eighth of is left as it was.
pub fn frac_vspan(buf: &mut Buffer, x0: u16, x1: u16, y0: f32, y1: f32, fill: Color, on: Color) {
    // Both ends in eighths of a row, the glyphs' step, rounded once before
    // the band is split into rows. The painted height is then the band's
    // own: rounding each edge's coverage on its own rounds a half-eighth up
    // at both ends, so a sliding band would grow by an eighth for that frame.
    //
    // An end within half an eighth of a row boundary lands on it, so a row
    // the band merely grazes isn't painted at all. Painting it as a blank
    // in `on` would flatten whatever the caller drew there (e.g. its own
    // zebra stripe) to `on` for a frame; `on` is a fixed colour the caller
    // passes in and isn't guaranteed to match a striped row's resting
    // background, so stomping it every frame the edge grazes that row reads
    // as a flicker distinct from the band's own motion (task 8e).
    //
    // The clamp keeps every row inside u16 and a band starting above the
    // buffer from being read as a partial first row.
    let eighths = |y: f32| (y.clamp(0.0, f32::from(u16::MAX)) * 8.0).round() as u32;
    let (e0, e1) = (eighths(y0), eighths(y1));
    if e1 <= e0 {
        return;
    }
    // Each end's row, and how many eighths into that row it sits.
    let (r0, f0) = ((e0 / 8) as u16, e0 % 8);
    let (r1, f1) = ((e1 / 8) as u16, e1 % 8);

    if r0 == r1 {
        // The whole band sits inside row r0. A band that starts on the
        // row's top (f0 == 0) is nearer the top by this test too. A thin
        // band sliding through the middle of a row jumps from one edge to
        // the other as it passes the middle; no single glyph can show it in
        // between, and no caller draws a band shorter than a row (a settling
        // modal is most of its height, the list band one row).
        let height = e1 - e0;
        let edge = if f0 <= 8 - f1 { Edge::Upper(height) } else { Edge::Lower(height) };
        edge_row(buf, x0, x1, r0, edge, fill, on);
        return;
    }

    // Top edge row: the band covers it from f0 down to the row's bottom.
    if f0 > 0 {
        edge_row(buf, x0, x1, r0, Edge::Lower(8 - f0), fill, on);
    }

    // Whole rows between the edges.
    let start_full = if f0 > 0 { r0 + 1 } else { r0 };
    if start_full < r1 {
        let full_rect = Rect::new(x0, start_full, x1.saturating_sub(x0), r1 - start_full);
        super::fill(buf, full_rect, fill);
    }

    // Bottom edge row: the band covers it from the row's top down to f1.
    if f1 > 0 {
        edge_row(buf, x0, x1, r1, Edge::Upper(f1), fill, on);
    }
}

/// The lower-block glyphs by eighths covered, from the bottom of the cell.
const GLYPHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// An edge row's fill: how many eighths of the cell (1..=7) it covers, at
/// the cell's top or bottom.
#[derive(Clone, Copy)]
enum Edge {
    /// The top of the cell: the band's bottom edge.
    Upper(u32),
    /// The bottom of the cell: the band's top edge.
    Lower(u32),
}

/// Paints one fractional edge row across columns `x0..x1` of row `y`,
/// clipped to the buffer: `fill` over the part of each cell `edge` covers,
/// and `on` over the rest.
///
/// There are only lower-block glyphs, so a `Lower` fill is the glyph for
/// its eighths in `fill` on `on`, and an `Upper` one is the glyph for the
/// *rest* of the cell in `on` on `fill`: the glyph is always the part at the
/// bottom, whichever colour that is (task #185: the bottom edge used its
/// own coverage's glyph in `on`, and showed the inverse fraction of fill).
///
/// Each cell is reset before it is painted, so it keeps nothing of what was
/// there: a strike or underline left from the page would otherwise run
/// through the band's edge (task #114: a settling modal's edge row over a
/// struck header row). The band's full rows get the same from
/// [`super::fill`], which clears the modifiers by hand rather than
/// resetting because it sets no fg and its many callers may paint over
/// the blank with only a symbol. An edge row sets every field it shows, so
/// a reset is the plainest way to replace the whole cell here.
fn edge_row(buf: &mut Buffer, x0: u16, x1: u16, y: u16, edge: Edge, fill: Color, on: Color) {
    let (glyph, fg, bg) = match edge {
        Edge::Lower(n) => (GLYPHS[n as usize], fill, on),
        Edge::Upper(n) => (GLYPHS[8 - n as usize], on, fill),
    };
    for x in x0..x1.min(buf.area().right()) {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.reset();
            cell.set_symbol(glyph);
            cell.set_fg(fg);
            cell.set_bg(bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{paint, theme::Theme};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    #[test]
    fn full_rows_fill_and_fractional_edges_use_lower_blocks() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(10, 6)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 10, 6), theme.panel);
            // band from y=1.5 to y=4.0: row1 bottom-half, rows 2..3 full, row 4 empty edge
            frac_vspan(f.buffer_mut(), 0, 10, 1.5, 4.0, theme.accent, theme.panel);
        })
        .unwrap();
        let c = |x, y| term.backend().buffer().cell((x, y)).unwrap().clone();
        assert_eq!(c(3, 1).symbol(), "▄"); // half coverage from below
        assert_eq!(c(3, 1).fg, theme.accent);
        assert_eq!(c(3, 1).bg, theme.panel);
        assert_eq!(c(3, 2).symbol(), " "); // fully covered row uses space
        assert_eq!(c(3, 2).bg, theme.accent);
        assert_eq!(c(3, 3).symbol(), " "); // fully covered row uses space
        assert_eq!(c(3, 3).bg, theme.accent);
        assert_eq!(c(3, 4).bg, theme.panel); // untouched below y1
    }

    #[test]
    fn coverage_is_monotone_as_the_band_sweeps() {
        // property-ish: sweeping y0 from 2.0 down to 1.0 in 1/8 steps never
        // decreases the number of accent-showing sub-rows in column 0.
        let theme = Theme::dark();
        let mut prev_glyph_order = 0usize;
        for step in 0..=8 {
            let y0 = 2.0 - step as f32 / 8.0;
            let mut term = Terminal::new(TestBackend::new(4, 4)).unwrap();
            term.draw(|f| {
                paint::fill(f.buffer_mut(), Rect::new(0, 0, 4, 4), theme.panel);
                frac_vspan(f.buffer_mut(), 0, 4, y0, 3.0, theme.accent, theme.panel);
            })
            .unwrap();
            let cell = term.backend().buffer().cell((0, 1)).unwrap();
            let sym = cell.symbol().to_string();
            // Full rows use " " + bg=fill; check background to recognize full coverage
            let order = if sym == " " && cell.bg == theme.accent {
                8 // Full row with filled background represents 100% coverage
            } else {
                GLYPHS.iter().position(|g| **g == *sym).unwrap_or(8)
            };
            assert!(order >= prev_glyph_order, "coverage never shrinks: {sym} at step {step}");
            prev_glyph_order = order;
        }
    }

    /// Draws a band `y0..y1` (fill = accent, on = panel) on a 4x8 panel.
    fn band(y0: f32, y1: f32) -> ratatui::buffer::Buffer {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(4, 8)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 4, 8), theme.panel);
            frac_vspan(f.buffer_mut(), 0, 4, y0, y1, theme.accent, theme.panel);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    /// How many eighths of row `y` (column 1) show the fill, and whether
    /// they sit at the cell's top (`true`) or bottom.
    fn fill_eighths(buf: &ratatui::buffer::Buffer, y: u16) -> (usize, bool) {
        let fill = Theme::dark().accent;
        let cell = &buf[(1, y)];
        let k = GLYPHS.iter().position(|g| *g == cell.symbol()).unwrap();
        if cell.fg == fill && cell.bg == fill {
            (8, true)
        } else if cell.fg == fill {
            // The glyph's lower k eighths are the fill.
            (k, false)
        } else if cell.bg == fill {
            // The glyph's lower k eighths cover the fill; the rest shows it.
            (8 - k, true)
        } else {
            (0, true)
        }
    }

    #[test]
    fn the_bottom_edge_shows_its_covered_fraction_at_the_top_of_the_row() {
        // Task #185: the bottom edge drew the glyph for its coverage in the
        // `on` colour, so it showed the inverse fraction of fill. 0.5 is
        // its own inverse, so these use other fractions.
        let theme = Theme::dark();
        // A quarter: the top quarter shows the fill, the rest `on`.
        let buf = band(1.5, 4.25);
        assert_eq!(buf[(1, 4)].symbol(), "▆");
        assert_eq!((buf[(1, 4)].fg, buf[(1, 4)].bg), (theme.panel, theme.accent));
        assert_eq!(fill_eighths(&buf, 4), (2, true));
        // Seven eighths: one eighth of `on` left at the bottom.
        let buf = band(1.5, 4.875);
        assert_eq!(buf[(1, 4)].symbol(), "▁");
        assert_eq!(fill_eighths(&buf, 4), (7, true));
        // Nearly whole: rounds to a full row of fill, not a full block of `on`.
        let buf = band(1.5, 4.95);
        assert_eq!(fill_eighths(&buf, 4), (8, true));
        // Nearly none: the row is left as it was.
        let buf = band(1.5, 4.03);
        assert_eq!(buf[(1, 4)].symbol(), " ");
        assert_eq!(buf[(1, 4)].bg, theme.panel);
    }

    #[test]
    fn a_sliding_one_row_band_keeps_its_height() {
        // A one-row band moving down shows the same total fill at every
        // step: what leaves the top edge arrives at the bottom edge. The
        // steps are sixteenths, so every other one puts both edges on a
        // half-eighth, where rounding each edge on its own would round both
        // up and paint nine eighths.
        for step in 0..=16 {
            let y0 = 2.0 + step as f32 / 16.0;
            let buf = band(y0, y0 + 1.0);
            let total: usize = (0..8).map(|y| fill_eighths(&buf, y).0).sum();
            assert_eq!(total, 8, "band at {y0} shows {total} eighths");
        }
    }

    #[test]
    fn a_band_inside_one_row_shows_its_own_height_against_the_nearer_edge() {
        let theme = Theme::dark();
        // Starts on the row's top: the top quarter, not the whole row.
        let buf = band(2.0, 2.25);
        assert_eq!(buf[(1, 2)].symbol(), "▆");
        assert_eq!((buf[(1, 2)].fg, buf[(1, 2)].bg), (theme.panel, theme.accent));
        assert_eq!(fill_eighths(&buf, 2), (2, true));
        // Nearer the top: its height (two eighths) from the top.
        assert_eq!(fill_eighths(&band(2.125, 2.375), 2), (2, true));
        // Nearer the bottom: its height from the bottom.
        let buf = band(1.625, 1.875);
        assert_eq!(buf[(1, 1)].symbol(), "▂");
        assert_eq!((buf[(1, 1)].fg, buf[(1, 1)].bg), (theme.accent, theme.panel));
        // Too thin to show (both ends snap to the same eighth): the row is
        // left as it was, not painted from the band's top to the row's bottom.
        let buf = band(1.5, 1.55);
        assert_eq!(buf[(1, 1)].symbol(), " ");
        assert_eq!(buf[(1, 1)].bg, theme.panel);
    }

    #[test]
    fn edge_rows_clear_the_modifiers_of_the_cells_under_them() {
        // Task #114: a settling modal's edge row painted over a struck
        // header row kept the strike, because the edge writes replaced
        // only the symbol and colours. Every edge case (top, bottom, and
        // both edges in one row) must leave a plain cell, as the full
        // rows' `paint::fill` does.
        use ratatui::style::Modifier;
        let theme = Theme::dark();
        let marks = Modifier::CROSSED_OUT | Modifier::BOLD | Modifier::UNDERLINED;
        let draw = |y0: f32, y1: f32| {
            let mut term = Terminal::new(TestBackend::new(4, 6)).unwrap();
            term.draw(|f| {
                let buf = f.buffer_mut();
                paint::fill(buf, Rect::new(0, 0, 4, 6), theme.page);
                for y in 0..6 {
                    for x in 0..4 {
                        buf[(x, y)].modifier = marks;
                    }
                }
                frac_vspan(buf, 0, 4, y0, y1, theme.panel, theme.page);
            })
            .unwrap();
            term.backend().buffer().clone()
        };
        // Top edge in row 1, full rows 2..3, bottom edge in row 4.
        let buf = draw(1.5, 4.5);
        for y in 1..=4 {
            for x in 0..4 {
                assert_eq!(buf[(x, y)].modifier, Modifier::empty(), "({x}, {y}) keeps a modifier");
            }
        }
        // The top edge's lower half shows the fill; the bottom edge swaps the
        // colours so its upper half does.
        assert_eq!(buf[(1, 1)].symbol(), "▄");
        assert_eq!((buf[(1, 1)].fg, buf[(1, 1)].bg), (theme.panel, theme.page));
        assert_eq!(buf[(1, 4)].symbol(), "▄");
        assert_eq!((buf[(1, 4)].fg, buf[(1, 4)].bg), (theme.page, theme.panel));
        // Rows the band doesn't reach keep theirs.
        assert_eq!(buf[(1, 0)].modifier, marks);
        assert_eq!(buf[(1, 5)].modifier, marks);
        // Both edges in row 2, against either edge of the row.
        let buf = draw(2.25, 2.875);
        assert_eq!(buf[(1, 2)].symbol(), "▅");
        assert_eq!((buf[(1, 2)].fg, buf[(1, 2)].bg), (theme.panel, theme.page));
        for x in 0..4 {
            assert_eq!(buf[(x, 2)].modifier, Modifier::empty());
        }
        let buf = draw(2.125, 2.375);
        assert_eq!(buf[(1, 2)].symbol(), "▆");
        assert_eq!((buf[(1, 2)].fg, buf[(1, 2)].bg), (theme.page, theme.panel));
        for x in 0..4 {
            assert_eq!(buf[(x, 2)].modifier, Modifier::empty());
        }
    }

    #[test]
    fn a_band_starting_above_the_buffer_fills_its_first_row() {
        let buf = band(-0.5, 1.25);
        assert_eq!(fill_eighths(&buf, 0), (8, true));
        assert_eq!(fill_eighths(&buf, 1), (2, true));
    }

    #[test]
    fn negligible_edge_coverage_leaves_the_row_underneath_untouched() {
        // Regression for the list-travel flicker (task 8e): a fractional
        // edge whose coverage rounds to the empty glyph used to still
        // stomp the cell to `on` (a flat color the caller passes in,
        // fixed regardless of what's actually painted there). On a
        // zebra-striped list this visibly flattened a stripe's tint to
        // `on` for the one frame the band's edge merely grazed that row,
        // then the next frame's own repaint restored the stripe —
        // alternating content is exactly what reads as flicker. The fix:
        // don't write a row the edge covers under half an eighth of,
        // leaving whatever the caller already painted (here, a distinct
        // "stripe" color) alone.
        let theme = Theme::dark();
        let stripe = theme.zebra_alt;
        let mut term = Terminal::new(TestBackend::new(4, 4)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 4, 4), theme.page);
            // Pre-paint row 1 as a "zebra" stripe, distinct from `on`.
            paint::fill(f.buffer_mut(), Rect::new(0, 1, 4, 1), stripe);
            // Top edge at y0=1.97: coverage 0.03 of row 1, under half an
            // eighth, so it lands on the row's bottom (2.0) and row 1 gets
            // nothing. y1=3.0
            // keeps the bottom edge integral (no second edge write to
            // confuse the assertion).
            frac_vspan(f.buffer_mut(), 0, 4, 1.97, 3.0, theme.accent, theme.page);
        })
        .unwrap();
        let c = |x, y| term.backend().buffer().cell((x, y)).unwrap().clone();
        assert_eq!(c(1, 1).symbol(), " ");
        assert_eq!(
            c(1, 1).bg,
            stripe,
            "negligible top-edge coverage must not flatten the row's own background"
        );
    }
}
