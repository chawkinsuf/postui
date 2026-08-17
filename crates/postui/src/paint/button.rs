//! A painted, mouse-clickable button: a centered label row between two
//! shaded half-block cap rows (light above, dark below), reading as 2 text
//! lines tall on the surface behind it.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::paint::{ControlState, fill, half_cap_bottom, half_cap_top, text};
use crate::theme::Theme;

/// Which visual family a button belongs to: `Primary` is the accent-filled
/// call-to-action look, `Secondary` is the neutral control look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonKind {
    Primary,
    Secondary,
}

/// A painted button: a label, its visual kind, and its current interaction
/// state. `paint` draws it into a 3-row-tall area.
pub struct Button<'a> {
    pub label: &'a str,
    pub kind: ButtonKind,
    pub state: ControlState,
}

/// Buttons are always exactly this many rows tall: a half-block cap row
/// above, the label row, a half-block cap row below. The caps fill only
/// half their cell, so the button reads as 2 text lines.
pub const BUTTON_HEIGHT: u16 = 3;

/// The minimum width a button needs to show `label` without truncation: the
/// label plus 2 columns of padding on each side.
pub fn button_min_width(label: &str) -> u16 {
    label.chars().count() as u16 + 4
}

/// The colors a button paints with for a given kind + state.
struct Face {
    fill: Color,
    label_fg: Color,
    /// `(top, bottom)` cap colors: a light/dark pair straddling the fill
    /// for the raised bevel look (swapped when Pressed), or the focus ring
    /// color on both when Focused, or the flat fill when Disabled.
    caps: (Color, Color),
}

impl Button<'_> {
    /// Paints this button into `area`, which must be exactly
    /// [`BUTTON_HEIGHT`] rows tall, on top of surface color `on`. The label
    /// is centered on the middle row; the cap rows above/below carry the
    /// light/dark shading.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, on: Color, theme: &Theme) {
        let face = self.face(theme);

        let top = Rect::new(area.x, area.y, area.width, 1);
        let mid = Rect::new(area.x, area.y + 1, area.width, 1);
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

        let (cap_top, cap_bottom) = face.caps;
        half_cap_top(buf, top, cap_top, on);
        fill(buf, mid, face.fill);
        half_cap_bottom(buf, bottom, cap_bottom, on);

        let width = self.label.chars().count() as u16;
        let start_x = area.x + area.width.saturating_sub(width) / 2;
        text(
            buf,
            start_x,
            mid.y,
            self.label,
            face.label_fg,
            face.fill,
            true,
        );
    }

    fn face(&self, theme: &Theme) -> Face {
        let fill = match (self.kind, self.state) {
            (ButtonKind::Primary, ControlState::Normal | ControlState::Focused) => theme.accent,
            (ButtonKind::Primary, ControlState::Hover) => theme.accent_edge_light,
            (ButtonKind::Primary, ControlState::Pressed) => theme.accent_edge_dark,
            (_, ControlState::Disabled) => theme.control,
            (ButtonKind::Secondary, ControlState::Normal | ControlState::Focused) => theme.control,
            (ButtonKind::Secondary, ControlState::Hover) => theme.control_hover,
            (ButtonKind::Secondary, ControlState::Pressed) => theme.control_pressed,
        };
        let label_fg = match (self.kind, self.state) {
            (_, ControlState::Disabled) => theme.text_disabled,
            (ButtonKind::Primary, _) => theme.on_accent,
            (ButtonKind::Secondary, _) => theme.text,
        };
        // Cap shading follows the currently shown fill (light/dark edges of
        // whatever face is painted), so the whole control visibly reacts to
        // hover and press — not just the label row.
        let (light, dark) = crate::paint::face_edges(fill, theme);
        let caps = match self.state {
            ControlState::Focused => (theme.focus_ring, theme.focus_ring),
            ControlState::Disabled => (fill, fill),
            // Pressed: sunken — dark on top, light on the bottom.
            ControlState::Pressed => (dark, light),
            _ => (light, dark),
        };
        Face {
            fill,
            label_fg,
            caps,
        }
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
    fn primary_button_centers_label_between_shaded_half_caps() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            let area = Rect::new(0, 1, 20, 3);
            Button {
                label: "Send",
                kind: ButtonKind::Primary,
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), area, theme.page, &theme);
        })
        .unwrap();
        // Top cap: lower-half block in the lightened fill over the surface.
        let top = buf_cell(&term, 8, 1);
        assert_eq!(top.symbol(), "▄");
        assert_eq!(top.fg, theme.accent_edge_light);
        assert_eq!(top.bg, theme.page);
        // Label centered on the middle row, bold on the accent fill.
        let mid = buf_cell(&term, 8, 2); // "Send" centered in 20 cols starts at 8
        assert_eq!(mid.symbol(), "S");
        assert_eq!(mid.bg, theme.accent);
        assert_eq!(mid.fg, theme.on_accent);
        assert!(mid.modifier.contains(ratatui::style::Modifier::BOLD));
        // Bottom cap: upper-half block in the darkened fill.
        let bottom = buf_cell(&term, 8, 3);
        assert_eq!(bottom.symbol(), "▀");
        assert_eq!(bottom.fg, theme.accent_edge_dark);
        assert_eq!(bottom.bg, theme.page);
    }

    #[test]
    fn pressed_button_swaps_cap_shading() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            Button {
                label: "Send",
                kind: ButtonKind::Primary,
                state: ControlState::Pressed,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), theme.page, &theme);
        })
        .unwrap();
        // Sunken: the pressed fill's dark edge on top, its light edge on
        // the bottom.
        let (p_light, p_dark) = crate::paint::face_edges(theme.accent_edge_dark, &theme);
        assert_eq!(buf_cell(&term, 8, 0).fg, p_dark);
        assert_eq!(buf_cell(&term, 8, 1).symbol(), "S");
        assert_eq!(buf_cell(&term, 8, 2).fg, p_light);
    }

    #[test]
    fn hovered_button_lifts_caps_along_with_the_face() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            Button {
                label: "Send",
                kind: ButtonKind::Primary,
                state: ControlState::Hover,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), theme.page, &theme);
        })
        .unwrap();
        // The whole control reacts to hover: the caps are the light/dark
        // edges of the *hovered* fill, not the base accent's pinned pair.
        let (h_light, h_dark) = crate::paint::face_edges(theme.accent_edge_light, &theme);
        assert_eq!(buf_cell(&term, 8, 1).bg, theme.accent_edge_light);
        assert_eq!(buf_cell(&term, 8, 0).fg, h_light);
        assert_eq!(buf_cell(&term, 8, 2).fg, h_dark);
    }

    #[test]
    fn focused_button_paints_caps_in_focus_ring_color() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            Button {
                label: "Send",
                kind: ButtonKind::Primary,
                state: ControlState::Focused,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), theme.page, &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 8, 0).fg, theme.focus_ring);
        assert_eq!(buf_cell(&term, 8, 2).fg, theme.focus_ring);
    }

    #[test]
    fn disabled_button_has_muted_label_and_flat_caps() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            Button {
                label: "Send",
                kind: ButtonKind::Secondary,
                state: ControlState::Disabled,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), theme.page, &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 8, 1).fg, theme.text_disabled);
        // Flat: both caps in the plain control fill, no light/dark pair.
        assert_eq!(buf_cell(&term, 8, 0).fg, theme.control);
        assert_eq!(buf_cell(&term, 8, 2).fg, theme.control);
    }
}
