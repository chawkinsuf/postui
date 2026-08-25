//! Painted list rows: one dense, single-line row per entry, with an
//! optional zebra stripe and hover/selected states layered on top.

use ratatui::{buffer::Buffer, style::Color};

use crate::theme::Theme;

/// Which visual state a [`ListRow`] paints with. `None` paints the row's
/// base (zebra or `base`) fill and nothing more.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowHighlight {
    None,
    Hover,
    /// A keyboard-cursor / menu-target marker: a steady `control_hover`
    /// fill, one step brighter than a plain hover so it reads against the
    /// zebra stripes, but still clearly weaker than `Selected`'s band.
    Cursor,
    Selected,
}

/// One dense, single-line list row: a flat fill on `row_y` with an optional
/// zebra stripe underneath and hover/selected states layered on top.
pub struct ListRow {
    pub highlight: RowHighlight,
    /// Some(true|false) = zebra stripe parity for this row; None = no zebra.
    pub zebra: Option<bool>,
}

impl ListRow {
    /// The resolved background fill for a row painted with `highlight` on
    /// top of `base` (the row's own zebra/plain fill), given the current
    /// hover-fade progress `hover_t`. Replicates [`ListRow::paint`]'s own
    /// fill computation, so a caller that paints row content as a second
    /// pass on top of the row's fill (text, badges, …) knows what
    /// background it actually landed on. Used both by [`ListRow::paint`]
    /// itself and by callers that build their own `ListRow`-styled rows
    /// without zebra (chooser/palette/var_picker/var-manager master lists).
    pub fn resolve_fill(
        theme: &Theme,
        highlight: RowHighlight,
        base: Color,
        hover_t: f32,
    ) -> Color {
        match highlight {
            RowHighlight::None => base,
            RowHighlight::Hover => crate::theme::mix(base, theme.control, hover_t),
            RowHighlight::Cursor => theme.control_hover,
            RowHighlight::Selected => theme.selection,
        }
    }

    /// Paints one single-line row across `[x, x + width)` on `row_y`.
    ///
    /// Layering: zebra base (parity `true` = `theme.zebra_alt`, `false` =
    /// `base`), then `Hover` blends toward `theme.control` via
    /// [`crate::theme::mix`] at `hover_t` (1.0 = fully hovered), and
    /// `Selected` paints a flat `theme.selection` fill with a 1-col accent
    /// bar (glyph `"▌"`, fg `theme.accent`) at the left edge.
    #[allow(clippy::too_many_arguments)] // signature is the produced interface, verbatim
    pub fn paint(
        &self,
        buf: &mut Buffer,
        row_y: u16,
        x: u16,
        width: u16,
        base: Color,
        hover_t: f32,
        theme: &Theme,
    ) {
        let zebra_fill = match self.zebra {
            Some(true) => theme.zebra_alt,
            Some(false) | None => base,
        };

        let fill = Self::resolve_fill(theme, self.highlight, zebra_fill, hover_t);
        let selected = self.highlight == RowHighlight::Selected;

        for col in x..x.saturating_add(width) {
            let is_bar = selected && col == x;
            if let Some(cell) = buf.cell_mut((col, row_y)) {
                if is_bar {
                    cell.set_symbol("▌");
                    cell.set_fg(theme.accent);
                } else {
                    cell.set_symbol(" ");
                }
                cell.set_bg(fill);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    #[test]
    fn selected_list_row_paints_selection_fill_with_left_accent_bar() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 3), theme.panel);
            ListRow {
                highlight: RowHighlight::Selected,
                zebra: None,
            }
            .paint(f.buffer_mut(), 1, 0, 20, theme.panel, 1.0, &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 0, 1).symbol(), "▌");
        assert_eq!(buf_cell(&term, 0, 1).fg, theme.accent);
        assert_eq!(buf_cell(&term, 5, 1).bg, theme.selection);
        assert_eq!(
            buf_cell(&term, 5, 0).bg,
            theme.panel,
            "single-line: no pads above"
        );
        assert_eq!(
            buf_cell(&term, 5, 2).bg,
            theme.panel,
            "single-line: no pads below"
        );
    }

    #[test]
    fn zebra_parity_alternates_base_fill() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 2)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 2), theme.panel);
            ListRow {
                highlight: RowHighlight::None,
                zebra: Some(false),
            }
            .paint(f.buffer_mut(), 0, 0, 20, theme.panel, 1.0, &theme);
            ListRow {
                highlight: RowHighlight::None,
                zebra: Some(true),
            }
            .paint(f.buffer_mut(), 1, 0, 20, theme.panel, 1.0, &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 5, 0).bg, theme.panel);
        assert_eq!(buf_cell(&term, 5, 1).bg, theme.zebra_alt);
        assert_ne!(theme.zebra_alt, theme.panel);
    }

    #[test]
    fn hover_t_blends_between_base_and_hover_fill() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 1)).unwrap();
        term.draw(|f| {
            paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 1), theme.panel);
            ListRow {
                highlight: RowHighlight::Hover,
                zebra: None,
            }
            .paint(f.buffer_mut(), 0, 0, 20, theme.panel, 0.5, &theme);
        })
        .unwrap();
        let bg = buf_cell(&term, 5, 0).bg;
        assert_ne!(bg, theme.panel);
        assert_ne!(bg, theme.control, "mid-fade sits between the two fills");
    }

    #[test]
    fn resolve_fill_matches_paints_own_fill_computation() {
        let theme = Theme::dark();
        let base = theme.panel;
        let hover_t = 0.5;
        for highlight in [
            RowHighlight::None,
            RowHighlight::Hover,
            RowHighlight::Selected,
        ] {
            let mut term = Terminal::new(TestBackend::new(4, 1)).unwrap();
            term.draw(|f| {
                paint::fill(f.buffer_mut(), Rect::new(0, 0, 4, 1), base);
                ListRow {
                    highlight,
                    zebra: None,
                }
                .paint(f.buffer_mut(), 0, 0, 4, base, hover_t, &theme);
            })
            .unwrap();
            let painted = buf_cell(&term, 2, 0).bg;
            assert_eq!(
                painted,
                ListRow::resolve_fill(&theme, highlight, base, hover_t),
                "resolve_fill must match paint()'s own fill for {highlight:?}"
            );
        }
    }
}
