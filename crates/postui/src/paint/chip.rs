//! Painted chips (small tinted pills used for method/status/count badges)
//! and the tab strip (labels row + underline row under the active tab).

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::paint::text;
use crate::theme::Theme;

/// A single-row tinted pill: `" label "` with a bg tinted toward the chip's
/// `color` and bold text in `color`. Used for method/status/count chips.
pub struct Chip<'a> {
    pub label: &'a str,
    pub color: Color,
}

impl Chip<'_> {
    /// Paints `" label "` at `(x, y)` on top of surface `on`. Returns the
    /// width (in columns) painted, so callers can lay out subsequent chips.
    pub fn paint(&self, buf: &mut Buffer, x: u16, y: u16, on: Color, theme: &Theme) -> u16 {
        let s = format!(" {} ", self.label);
        let width = s.chars().count() as u16;
        let bg = theme.tint(self.color, on);
        text(buf, x, y, &s, self.color, bg, true);
        width
    }
}

/// A horizontal strip of tabs: labels row plus an accent underline row
/// under the active tab only. Each tuple is `(label, has_badge)` — a badge
/// renders a trailing `" ✓"` on that tab's label.
pub struct TabStrip<'a> {
    pub tabs: &'a [(String, bool)],
    pub active: usize,
    /// The index of the tab currently under the mouse, if any. The hovered
    /// tab's label extent gets a `theme.control` fill behind it; text colors
    /// are unaffected by hover (active stays accent+bold, inactive stays
    /// `text_muted`) — hover is a background cue only.
    pub hovered: Option<usize>,
}

impl TabStrip<'_> {
    /// Paints the strip into the top 2 rows of `area` on top of surface
    /// `on`. Returns the x-extent [`Rect`] of each tab's label on the
    /// labels row, for hit registration by later tasks.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, on: Color, theme: &Theme) -> Vec<Rect> {
        let labels_y = area.y;
        let underline_y = area.y + 1;
        let mut x = area.x;
        let mut rects = Vec::with_capacity(self.tabs.len());

        for (i, (label, has_badge)) in self.tabs.iter().enumerate() {
            let mut s = label.clone();
            if *has_badge {
                s.push_str(" ✓");
            }
            let width = s.chars().count() as u16;
            let active = i == self.active;
            let fg = if active {
                theme.accent
            } else {
                theme.text_muted
            };
            let label_bg = if self.hovered == Some(i) {
                theme.control
            } else {
                on
            };

            text(buf, x, labels_y, &s, fg, label_bg, active);
            let rect = Rect::new(x, labels_y, width, 1);

            if active {
                for ux in rect.x..(rect.x + rect.width) {
                    if let Some(cell) = buf.cell_mut((ux, underline_y)) {
                        cell.set_symbol("▁");
                        cell.set_fg(theme.accent);
                        cell.set_bg(on);
                    }
                }
            }

            rects.push(rect);
            x += width + 2; // 2-column gap between tabs
        }

        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    #[test]
    fn chip_paints_tinted_pill() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(12, 1)).unwrap();
        term.draw(|f| {
            Chip {
                label: "GET",
                color: theme.success,
            }
            .paint(f.buffer_mut(), 0, 0, theme.panel, &theme);
        })
        .unwrap();
        let c = buf_cell(&term, 1, 0);
        assert_eq!(c.symbol(), "G");
        assert_eq!(c.bg, theme.tint(theme.success, theme.panel));
        assert!(c.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn tabstrip_underlines_active_tab_only() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &[
                    ("Params".to_string(), false),
                    ("Headers".to_string(), false),
                ],
                active: 0,
                hovered: None,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        // row 1 under "Params" is "▁" in accent; under "Headers" it is blank
        assert_eq!(buf_cell(&term, 1, 1).symbol(), "▁");
        assert_eq!(buf_cell(&term, 1, 1).fg, theme.accent);
        assert_eq!(buf_cell(&term, rects[1].x + 1, 1).symbol(), " ");
    }

    #[test]
    fn tabstrip_badge_appends_checkmark() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &[("Body".to_string(), true)],
                active: 0,
                hovered: None,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        assert_eq!(rects[0].width, "Body ✓".chars().count() as u16);
        assert_eq!(buf_cell(&term, 5, 0).symbol(), "✓");
    }

    #[test]
    fn tabstrip_hover_fills_control_behind_the_label_only_on_the_hovered_tab() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &[
                    ("Params".to_string(), false),
                    ("Headers".to_string(), false),
                ],
                active: 0,
                hovered: Some(1),
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        let hovered_cell = buf_cell(&term, rects[1].x, 0);
        assert_eq!(
            hovered_cell.bg, theme.control,
            "the hovered inactive tab's label cell bg must be theme.control"
        );
        assert_eq!(
            hovered_cell.fg, theme.text_muted,
            "hover must not change an inactive tab's text color"
        );
        let non_hovered_cell = buf_cell(&term, rects[0].x, 0);
        assert_eq!(
            non_hovered_cell.bg, theme.panel,
            "the non-hovered tab's label cell bg must stay the passed-in `on` surface"
        );
        assert_eq!(non_hovered_cell.fg, theme.accent);
    }
}
