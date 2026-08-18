//! Painted chips (small tinted pills used for method/status/count badges)
//! and the tab strip (labels row + underline row under the active tab).

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::paint::{fill, half_cap_bottom, text};
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

/// A horizontal strip of GUI-style tabs: each tab is a padded filled block
/// (`" label "`) with a half-block cap row below, like a segmented control.
/// The active tab is accent-filled; inactive tabs sit on `theme.control`
/// and lift to `theme.control_hover` under the mouse. Each tuple is
/// `(label, badge)` — a badge renders a trailing colored glyph (e.g. the
/// Body tab's JSON-validity `✓`/`✗`) inside the tab's block.
pub struct TabStrip<'a> {
    pub tabs: &'a [(String, Option<(char, Color)>)],
    pub active: usize,
    /// The index of the tab currently under the mouse, if any.
    pub hovered: Option<usize>,
    /// Whether the strip itself holds keyboard focus (arrow keys switch
    /// tabs). Recolors the active tab's cap in the focus-ring color.
    pub focused: bool,
}

impl TabStrip<'_> {
    /// Paints the strip into the top 2 rows of `area` on top of surface
    /// `on`. Returns each tab's full 2-row block [`Rect`], for hit
    /// registration by the caller.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, on: Color, theme: &Theme) -> Vec<Rect> {
        let labels_y = area.y;
        let cap_y = area.y + 1;
        let mut x = area.x;
        let mut rects = Vec::with_capacity(self.tabs.len());

        for (i, (label, badge)) in self.tabs.iter().enumerate() {
            let label_w = label.chars().count() as u16;
            let width = label_w + 2 + badge.map_or(0, |_| 2);
            let active = i == self.active;
            let face = if active {
                theme.accent
            } else if self.hovered == Some(i) {
                theme.control_hover
            } else {
                theme.control
            };
            let fg = if active {
                theme.on_accent
            } else if self.hovered == Some(i) {
                theme.text
            } else {
                theme.text_muted
            };

            fill(buf, Rect::new(x, labels_y, width, 1), face);
            text(buf, x + 1, labels_y, label, fg, face, active);
            if let Some((glyph, color)) = badge {
                text(
                    buf,
                    x + 1 + label_w,
                    labels_y,
                    &format!(" {glyph}"),
                    *color,
                    face,
                    true,
                );
            }
            // Keyboard focus shows as a SHAPE change on the active tab:
            // its half-cap thickens into a solid accent row. A recolor
            // can't work here — focus_ring and accent are the same color,
            // so a "focus-colored" cap is indistinguishable from normal.
            if active && self.focused {
                fill(buf, Rect::new(x, cap_y, width, 1), face);
            } else {
                half_cap_bottom(buf, Rect::new(x, cap_y, width, 1), face, on);
            }

            rects.push(Rect::new(x, labels_y, width, 2));
            x += width + 1; // 1-column gap between tabs
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
    fn tabstrip_paints_block_tabs_with_accent_active_fill_and_half_cap() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &[("Params".to_string(), None), ("Headers".to_string(), None)],
                active: 0,
                hovered: None,
                focused: false,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        // Each tab is a padded filled block " label ", 2 rows tall.
        assert_eq!(rects[0].width, " Params ".chars().count() as u16);
        assert_eq!(rects[0].height, 2, "tab hit covers both rows");
        // Active tab: accent fill, on_accent bold label, accent half-cap.
        let active_cell = buf_cell(&term, rects[0].x, 0);
        assert_eq!(active_cell.bg, theme.accent);
        let label_cell = buf_cell(&term, rects[0].x + 1, 0);
        assert_eq!(label_cell.symbol(), "P");
        assert_eq!(label_cell.fg, theme.on_accent);
        assert!(label_cell.modifier.contains(Modifier::BOLD));
        let cap = buf_cell(&term, rects[0].x, 1);
        assert_eq!(cap.symbol(), "▀");
        assert_eq!(cap.fg, theme.accent);
        assert_eq!(cap.bg, theme.panel);
        // Inactive tab: control fill, muted label, control half-cap.
        let inactive_cell = buf_cell(&term, rects[1].x, 0);
        assert_eq!(inactive_cell.bg, theme.control);
        assert_eq!(buf_cell(&term, rects[1].x + 1, 0).fg, theme.text_muted);
        assert_eq!(buf_cell(&term, rects[1].x, 1).fg, theme.control);
    }

    #[test]
    fn focused_tabstrip_marks_the_active_tab_with_the_focus_ring_color() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &[("Params".to_string(), None), ("Headers".to_string(), None)],
                active: 0,
                hovered: None,
                focused: true,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        // The indicator must be a SHAPE change, not a recolor: focus_ring
        // and accent are the same color in the default themes, so a
        // half-cap recolored "to focus_ring" is indistinguishable from the
        // normal active cap.
        let cap = buf_cell(&term, rects[0].x, 1);
        assert_eq!(
            cap.bg, theme.accent,
            "keyboard focus thickens the active tab's half-cap into a \
             solid accent row, so the strip visibly holds the arrow keys"
        );
        assert_eq!(cap.symbol(), " ", "solid row, no half-block glyph");
        let inactive_cap = buf_cell(&term, rects[1].x, 1);
        assert_eq!(inactive_cap.symbol(), "▀", "inactive tabs keep half-caps");
        assert_eq!(inactive_cap.fg, theme.control);
    }

    #[test]
    fn tabstrip_badge_appends_colored_glyph_inside_the_tab() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &[("Body".to_string(), Some(('✓', theme.success)))],
                active: 0,
                hovered: None,
                focused: false,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        assert_eq!(rects[0].width, " Body ✓ ".chars().count() as u16);
        let glyph = buf_cell(&term, rects[0].x + 6, 0);
        assert_eq!(glyph.symbol(), "✓");
        assert_eq!(glyph.fg, theme.success, "badge keeps its own color");
        assert_eq!(glyph.bg, theme.accent, "badge sits inside the tab's fill");
    }

    #[test]
    fn tabstrip_hover_lifts_only_the_hovered_inactive_tab() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
        let mut rects = Vec::new();
        term.draw(|f| {
            rects = TabStrip {
                tabs: &[("Params".to_string(), None), ("Headers".to_string(), None)],
                active: 0,
                hovered: Some(1),
                focused: false,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 40, 2), theme.panel, &theme);
        })
        .unwrap();
        let hovered_cell = buf_cell(&term, rects[1].x, 0);
        assert_eq!(
            hovered_cell.bg, theme.control_hover,
            "the hovered inactive tab's fill lifts to control_hover"
        );
        let non_hovered_cell = buf_cell(&term, rects[0].x, 0);
        assert_eq!(
            non_hovered_cell.bg, theme.accent,
            "the active tab keeps its accent fill"
        );
    }
}
