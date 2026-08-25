//! Fractional vertical span helper: paints a horizontal band across fractional row boundaries
//! using lower-block glyphs (▁▂▃▄▅▆▇█) for precise vertical positioning.

use ratatui::{buffer::Buffer, style::Color};

/// Paints a full-width horizontal band covering rows `y0..y1` (fractional row
/// coordinates, `y1 > y0`) across columns `x0..x1`. Whole-covered rows are
/// filled cells; the fractional top edge row uses the lower-block family
/// `▁▂▃▄▅▆▇` sized to the covered fraction (fg=`fill`, bg=`on`); the
/// fractional bottom edge row uses the same glyphs with fg/bg swapped
/// (fg=`on`, bg=`fill`) so the *upper* fraction shows `fill`.
///
/// If the top and bottom fractional edges land in the same row, picks the
/// closest single glyph (top-fraction wins). Clamps to buffer area.
pub fn frac_vspan(buf: &mut Buffer, x0: u16, x1: u16, y0: f32, y1: f32, fill: Color, on: Color) {
    const GLYPHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

    let y0_int = y0.floor() as u16;
    let y1_int = y1.floor() as u16;

    let y0_frac = y0 - y0.floor(); // fractional part of y0
    let y1_frac = y1 - y1.floor(); // fractional part of y1

    // Top fractional row: shows the covered portion (from y0 to next integer)
    if y0_frac > 0.0 {
        // Coverage is from y0 to (y0_int + 1), which is (1.0 - y0_frac)
        let coverage = 1.0 - y0_frac;
        let glyph_index = (coverage * 8.0).round() as usize;
        let glyph_index = glyph_index.min(8);
        for x in x0..x1.min(buf.area().right()) {
            if let Some(cell) = buf.cell_mut((x, y0_int)) {
                cell.set_symbol(GLYPHS[glyph_index]);
                cell.set_fg(fill);
                cell.set_bg(on);
            }
        }
    }

    // Full rows between the fractional edges
    // If y0_frac == 0.0, row y0_int is fully covered; otherwise start from y0_int + 1
    let start_full = if y0_frac > 0.0 { y0_int + 1 } else { y0_int };
    for y in start_full..y1_int {
        for x in x0..x1.min(buf.area().right()) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol("█");
                cell.set_bg(fill);
            }
        }
    }

    // Bottom fractional row: shows the upper portion as fill
    if y1_frac > 0.0 && y1_int > y0_int {
        // Coverage is from y1_int to y1, which is y1_frac
        let glyph_index = (y1_frac * 8.0).round() as usize;
        let glyph_index = glyph_index.min(8);
        for x in x0..x1.min(buf.area().right()) {
            if let Some(cell) = buf.cell_mut((x, y1_int)) {
                cell.set_symbol(GLYPHS[glyph_index]);
                cell.set_fg(on);
                cell.set_bg(fill);
            }
        }
    } else if y1_frac > 0.0 && y1_int == y0_int {
        // Top and bottom fractions land in the same row: pick the closest glyph
        // Top fraction wins the priority
        let top_coverage = 1.0 - y0_frac;
        let top_index = (top_coverage * 8.0).round() as usize;
        let bottom_index = (y1_frac * 8.0).round() as usize;
        let glyph_index = top_index.min(8).max(bottom_index.min(8));
        for x in x0..x1.min(buf.area().right()) {
            if let Some(cell) = buf.cell_mut((x, y0_int)) {
                cell.set_symbol(GLYPHS[glyph_index]);
                cell.set_fg(fill);
                cell.set_bg(on);
            }
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
        assert_eq!(c(3, 2).bg, theme.accent); // fully covered row
        assert_eq!(c(3, 3).bg, theme.accent);
        assert_eq!(c(3, 4).bg, theme.panel); // untouched below y1
    }

    #[test]
    fn coverage_is_monotone_as_the_band_sweeps() {
        // property-ish: sweeping y0 from 2.0 down to 1.0 in 1/8 steps never
        // decreases the number of accent-showing sub-rows in column 0.
        let theme = Theme::dark();
        let mut prev_glyph_order = 0usize;
        const ORDER: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        for step in 0..=8 {
            let y0 = 2.0 - step as f32 / 8.0;
            let mut term = Terminal::new(TestBackend::new(4, 4)).unwrap();
            term.draw(|f| {
                paint::fill(f.buffer_mut(), Rect::new(0, 0, 4, 4), theme.panel);
                frac_vspan(f.buffer_mut(), 0, 4, y0, 3.0, theme.accent, theme.panel);
            })
            .unwrap();
            let sym = term
                .backend()
                .buffer()
                .cell((0, 1))
                .unwrap()
                .symbol()
                .to_string();
            let order = ORDER.iter().position(|g| **g == *sym).unwrap_or(8);
            assert!(
                order >= prev_glyph_order,
                "coverage never shrinks: {sym} at step {step}"
            );
            prev_glyph_order = order;
        }
    }
}
