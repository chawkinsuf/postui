//! The floating panel shell: a backdrop dim (for modal-style overlays) plus
//! a filled panel with a soft drop shadow along its right and bottom edges.

use ratatui::{buffer::Buffer, layout::Rect};

use crate::paint::fill;
use crate::theme::{Theme, dim55};

/// Dims every cell in `area` — fg and bg each blended 55% toward black —
/// used to push a backdrop behind a floating panel out of visual focus.
pub fn dim_backdrop(buf: &mut Buffer, area: Rect) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                let fg = cell.fg;
                let bg = cell.bg;
                cell.set_fg(dim55(fg));
                cell.set_bg(dim55(bg));
            }
        }
    }
}

/// Fills `area` with `theme.panel` and darkens a 1-cell drop-shadow band
/// along its right edge (offset 1 down) and bottom edge (offset 2 right),
/// clipped to `screen` so the shadow never paints outside the terminal.
pub fn floating_panel(buf: &mut Buffer, area: Rect, screen: Rect, theme: &Theme) {
    fill(buf, area, theme.panel);

    let darken = |buf: &mut Buffer, x: u16, y: u16| {
        if x < screen.left() || x >= screen.right() || y < screen.top() || y >= screen.bottom() {
            return;
        }
        if let Some(cell) = buf.cell_mut((x, y)) {
            let fg = cell.fg;
            let bg = cell.bg;
            cell.set_fg(dim55(fg));
            cell.set_bg(dim55(bg));
        }
    };

    // Right-edge shadow band: one column right of the panel, offset one
    // row down from its top.
    let shadow_x = area.right();
    for y in (area.top() + 1)..=area.bottom() {
        darken(buf, shadow_x, y);
    }

    // Bottom-edge shadow band: one row below the panel, offset two columns
    // right of its left edge.
    let shadow_y = area.bottom();
    for x in (area.left() + 2)..=area.right() {
        darken(buf, x, shadow_y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    #[test]
    fn floating_panel_darkens_shadow_band() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 20, 10), theme.page);
            floating_panel(
                f.buffer_mut(),
                Rect::new(2, 2, 10, 5),
                Rect::new(0, 0, 20, 10),
                &theme,
            );
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 5, 4).bg, theme.panel); // panel fill
        let shadow = buf_cell(&term, 12, 3).bg; // band right of panel
        let page = theme.page;
        assert_ne!(shadow, page); // darkened
    }

    #[test]
    fn dim_backdrop_blends_every_cell_toward_black() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(10, 5)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 10, 5), theme.page);
            dim_backdrop(f.buffer_mut(), Rect::new(0, 0, 10, 5));
        })
        .unwrap();
        assert_ne!(buf_cell(&term, 3, 2).bg, theme.page);
        assert_eq!(buf_cell(&term, 3, 2).bg, dim55(theme.page));
    }

    #[test]
    fn shadow_bands_clip_to_screen_without_panic() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(12, 7)).unwrap();
        term.draw(|f| {
            fill(f.buffer_mut(), Rect::new(0, 0, 12, 7), theme.page);
            // panel touches the right/bottom screen edge, so its shadow
            // bands fall entirely outside `screen` — must not panic.
            floating_panel(
                f.buffer_mut(),
                Rect::new(2, 2, 10, 5),
                Rect::new(0, 0, 12, 7),
                &theme,
            );
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 5, 4).bg, theme.panel);
    }
}
