//! Painted list rows: a "pill" fill on a 2-line pitch, with half-block pads
//! above/below the text row so adjacent pills compose into a continuous
//! shape across the shared spacing line between them.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::theme::Theme;

/// Which visual state a [`PillRow`] paints with. `None` paints nothing at
/// all — the row sits on whatever surface is already behind it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowHighlight {
    None,
    Hover,
    Selected,
}

/// One logical list row on a 2-line pitch. `text_row` is the line holding
/// content; the half-row pads live in `text_row - 1` / `text_row + 1`
/// (drawn only when inside `bounds`). `base` is the surface behind the
/// list — currently unused by the fill/pad math itself (pads read the
/// buffer's existing bg directly so two pills compose correctly) but kept
/// on the call signature per the produced interface.
pub struct PillRow {
    pub highlight: RowHighlight,
}

impl PillRow {
    /// Paints this row's fill (and, for `Selected`, its accent bar) across
    /// `[x, x + width)` on `text_row`, plus half-block pad caps in the rows
    /// immediately above/below when they lie inside `bounds`.
    #[allow(clippy::too_many_arguments)] // signature is the produced interface, verbatim
    pub fn paint(
        &self,
        buf: &mut Buffer,
        text_row: u16,
        x: u16,
        width: u16,
        bounds: Rect,
        _base: Color,
        theme: &Theme,
    ) {
        let fill = match self.highlight {
            RowHighlight::None => return,
            RowHighlight::Hover => theme.control,
            RowHighlight::Selected => theme.control_hover,
        };
        let selected = self.highlight == RowHighlight::Selected;

        for col in x..x.saturating_add(width) {
            let is_bar = selected && col == x;
            if let Some(cell) = buf.cell_mut((col, text_row)) {
                if is_bar {
                    cell.set_symbol("█");
                    cell.set_fg(theme.accent);
                } else {
                    cell.set_symbol(" ");
                }
                cell.set_bg(fill);
            }
        }

        if let Some(top_row) = text_row.checked_sub(1)
            && row_in_bounds(top_row, bounds)
        {
            for col in x..x.saturating_add(width) {
                let own_fill = if selected && col == x {
                    theme.accent
                } else {
                    fill
                };
                paint_pad(buf, col, top_row, PadGlyph::Top, own_fill);
            }
        }

        let bottom_row = text_row + 1;
        if row_in_bounds(bottom_row, bounds) {
            for col in x..x.saturating_add(width) {
                let own_fill = if selected && col == x {
                    theme.accent
                } else {
                    fill
                };
                paint_pad(buf, col, bottom_row, PadGlyph::Bottom, own_fill);
            }
        }
    }
}

fn row_in_bounds(row: u16, bounds: Rect) -> bool {
    row >= bounds.top() && row < bounds.bottom()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PadGlyph {
    /// `▄` — a top pad (this pill's upper cap, sits above its text row).
    Top,
    /// `▀` — a bottom pad (this pill's lower cap, sits below its text row).
    Bottom,
}

/// Paints one pad cell, composing with whatever pad the *other* pill sharing
/// this spacing line already painted there (if any) so the final state is
/// the same regardless of paint order: the upper pill's `▀` wins the glyph,
/// with its own fill as fg and the lower pill's fill as bg.
fn paint_pad(buf: &mut Buffer, x: u16, y: u16, glyph: PadGlyph, own_fill: Color) {
    let Some(cell) = buf.cell_mut((x, y)) else {
        return;
    };
    let existing_symbol = cell.symbol().to_string();
    let existing_fg = cell.fg;
    let existing_bg = cell.bg;

    let opposite_present = match glyph {
        PadGlyph::Top => existing_symbol == "▀",
        PadGlyph::Bottom => existing_symbol == "▄",
    };

    if opposite_present {
        match glyph {
            // I'm the upper pill's bottom pad; the lower pill's top pad is
            // already here with its fill in `fg`. Take the glyph, put my
            // own fill in fg, and pull the lower pill's fill from its fg.
            PadGlyph::Bottom => {
                cell.set_symbol("▀");
                cell.set_fg(own_fill);
                cell.set_bg(existing_fg);
            }
            // I'm the lower pill's top pad; the upper pill's bottom pad is
            // already here. Keep its glyph/fg as-is and just take the bg
            // slot with my own fill.
            PadGlyph::Top => {
                cell.set_symbol("▀");
                cell.set_fg(existing_fg);
                cell.set_bg(own_fill);
            }
        }
    } else {
        let symbol = match glyph {
            PadGlyph::Top => "▄",
            PadGlyph::Bottom => "▀",
        };
        cell.set_symbol(symbol);
        cell.set_fg(own_fill);
        cell.set_bg(existing_bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    #[test]
    fn selected_pill_extends_half_rows_and_bar() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
            PillRow {
                highlight: RowHighlight::Selected,
            }
            .paint(
                f.buffer_mut(),
                2,
                0,
                20,
                Rect::new(0, 0, 20, 5),
                theme.panel,
                &theme,
            );
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 5, 1).symbol(), "▄");
        assert_eq!(buf_cell(&term, 5, 1).fg, theme.control_hover);
        assert_eq!(buf_cell(&term, 5, 2).bg, theme.control_hover);
        assert_eq!(buf_cell(&term, 0, 2).symbol(), "█"); // accent bar, full block on text row
        assert_eq!(buf_cell(&term, 0, 2).fg, theme.accent);
        assert_eq!(buf_cell(&term, 5, 3).symbol(), "▀");
    }

    #[test]
    fn adjacent_pills_share_spacing_line() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
            // selected at text_row 1, hovered at text_row 3 → they share row 2
            PillRow {
                highlight: RowHighlight::Hover,
            }
            .paint(
                f.buffer_mut(),
                3,
                0,
                20,
                Rect::new(0, 0, 20, 5),
                theme.panel,
                &theme,
            );
            PillRow {
                highlight: RowHighlight::Selected,
            }
            .paint(
                f.buffer_mut(),
                1,
                0,
                20,
                Rect::new(0, 0, 20, 5),
                theme.panel,
                &theme,
            );
        })
        .unwrap();
        let shared = buf_cell(&term, 5, 2);
        assert_eq!(shared.symbol(), "▀"); // selected pill's bottom cap …
        assert_eq!(shared.fg, theme.control_hover); // … in selection fill …
        assert_eq!(shared.bg, theme.control); // … over the hover pill's fill
    }

    /// The brief's `adjacent_pills_share_spacing_line` test paints the lower
    /// pill (Hover, text_row 3) before the upper pill (Selected, text_row
    /// 1). This test paints them in the opposite order — upper first, then
    /// lower — and asserts the shared spacing line converges on the exact
    /// same final cell either way.
    #[test]
    fn adjacent_pills_share_spacing_line_regardless_of_paint_order() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
            PillRow {
                highlight: RowHighlight::Selected,
            }
            .paint(
                f.buffer_mut(),
                1,
                0,
                20,
                Rect::new(0, 0, 20, 5),
                theme.panel,
                &theme,
            );
            PillRow {
                highlight: RowHighlight::Hover,
            }
            .paint(
                f.buffer_mut(),
                3,
                0,
                20,
                Rect::new(0, 0, 20, 5),
                theme.panel,
                &theme,
            );
        })
        .unwrap();
        let shared = buf_cell(&term, 5, 2);
        assert_eq!(shared.symbol(), "▀");
        assert_eq!(shared.fg, theme.control_hover);
        assert_eq!(shared.bg, theme.control);
    }

    #[test]
    fn none_highlight_paints_nothing() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
            PillRow {
                highlight: RowHighlight::None,
            }
            .paint(
                f.buffer_mut(),
                2,
                0,
                20,
                Rect::new(0, 0, 20, 5),
                theme.panel,
                &theme,
            );
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 5, 2).bg, theme.panel);
        assert_eq!(buf_cell(&term, 5, 2).symbol(), " ");
    }

    #[test]
    fn pads_clipped_to_bounds() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
            // text_row 0: top pad (row -1) doesn't exist / is out of bounds.
            PillRow {
                highlight: RowHighlight::Hover,
            }
            .paint(
                f.buffer_mut(),
                0,
                0,
                20,
                Rect::new(0, 0, 20, 5),
                theme.panel,
                &theme,
            );
        })
        .unwrap();
        // no panic, and the text row itself still painted.
        assert_eq!(buf_cell(&term, 5, 0).bg, theme.control);
    }
}
