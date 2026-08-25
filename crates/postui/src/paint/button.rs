//! A painted, mouse-clickable button: a 3-row solid fill with a thin bevel
//! edge on the top/bottom rows (light above, dark below) and a centered
//! bold label on the middle row — the same anatomy `TextField` uses.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::paint::{ControlState, bevel_bottom, bevel_top, fill, text};
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

/// Buttons are always exactly this many rows tall: a thin bevel row on top,
/// the label row, a thin bevel row on the bottom — all three rows are the
/// button's own solid fill.
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
    /// `(top, bottom)` bevel edge colors: a light/dark pair straddling the
    /// fill for the raised look (swapped when Pressed), or `None` when
    /// Disabled (flat fill, no edges).
    edges: Option<(Color, Color)>,
}

impl Button<'_> {
    /// Paints this button into `area`, which must be exactly
    /// [`BUTTON_HEIGHT`] rows tall. The whole area is filled with the
    /// button's own face color; the label is centered bold on the middle
    /// row, and the top/bottom rows carry a thin bevel edge on that same
    /// fill (skipped when Disabled).
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        let face = self.face(theme);

        fill(buf, area, face.fill);

        let top = Rect::new(area.x, area.y, area.width, 1);
        let mid = Rect::new(area.x, area.y + 1, area.width, 1);
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

        if let Some((light, dark)) = face.edges {
            bevel_top(buf, top, light, face.fill);
            bevel_bottom(buf, bottom, dark, face.fill);
        }

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
        // Focused lifts like Hover: the app's focus language is a control's
        // own surface brightening, never a ring or a recolored edge.
        let fill = match (self.kind, self.state) {
            (ButtonKind::Primary, ControlState::Normal) => theme.accent,
            (ButtonKind::Primary, ControlState::Hover | ControlState::Focused) => {
                theme.accent_edge_light
            }
            (ButtonKind::Primary, ControlState::Pressed) => theme.accent_edge_dark,
            (_, ControlState::Disabled) => theme.control,
            (ButtonKind::Secondary, ControlState::Normal) => theme.control,
            (ButtonKind::Secondary, ControlState::Hover | ControlState::Focused) => {
                theme.control_hover
            }
            (ButtonKind::Secondary, ControlState::Pressed) => theme.control_pressed,
        };
        let label_fg = match (self.kind, self.state) {
            // Blended toward the control's own fill rather than the flat
            // `text_disabled` token — reads clearly dimmer than a resting
            // muted label, matching the same treatment `TextField` uses so
            // disabled controls read consistently across the app.
            (_, ControlState::Disabled) => {
                crate::theme::mix(fill, theme.text_muted, crate::paint::DISABLED_LABEL_MIX)
            }
            (ButtonKind::Primary, _) => theme.on_accent,
            (ButtonKind::Secondary, _) => theme.text,
        };
        // Bevel edges follow the currently shown fill (light/dark edges of
        // whatever face is painted), so the whole control visibly reacts to
        // hover and press — not just the label row. The bevel delta matches
        // the theme's own convention per surface family: ±0.12 around the
        // accent, but only ±0.08 around the neutral control fill — the
        // stronger delta pushes an already-dark Secondary face to near
        // black and reads as a hard line rather than shading.
        let delta = match self.kind {
            ButtonKind::Primary => 0.12,
            ButtonKind::Secondary => 0.08,
        };
        let light = crate::theme::lift_color(fill, delta);
        let dark = crate::theme::lift_color(fill, -delta);
        let edges = match self.state {
            ControlState::Disabled => None,
            // Pressed: sunken — dark on top, light on the bottom.
            ControlState::Pressed => Some((dark, light)),
            _ => Some((light, dark)),
        };
        Face {
            fill,
            label_fg,
            edges,
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
    fn primary_button_centers_label_between_thin_bevel_edges() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            Button {
                label: "Send",
                kind: ButtonKind::Primary,
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), Rect::new(0, 1, 20, 3), &theme);
        })
        .unwrap();
        let top = buf_cell(&term, 8, 1);
        assert_eq!(top.symbol(), "▔");
        assert_eq!(top.fg, crate::theme::lift_color(theme.accent, 0.12));
        assert_eq!(
            top.bg, theme.accent,
            "edge rows sit on the button's own fill"
        );
        let mid = buf_cell(&term, 8, 2);
        assert_eq!(mid.symbol(), "S");
        assert_eq!(mid.bg, theme.accent);
        assert_eq!(mid.fg, theme.on_accent);
        assert!(mid.modifier.contains(ratatui::style::Modifier::BOLD));
        let bottom = buf_cell(&term, 8, 3);
        assert_eq!(bottom.symbol(), "▁");
        assert_eq!(bottom.fg, crate::theme::lift_color(theme.accent, -0.12));
        assert_eq!(bottom.bg, theme.accent);
    }

    #[test]
    fn secondary_button_uses_the_softer_control_bevel() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            Button {
                label: "Cancel",
                kind: ButtonKind::Secondary,
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
        })
        .unwrap();
        // The neutral control face uses the theme's ±0.08 bevel around its
        // own fill — the stronger ±0.12 delta reads as a black line under
        // an already-dark fill.
        assert_eq!(
            buf_cell(&term, 8, 0).fg,
            crate::theme::lift_color(theme.control, 0.08)
        );
        assert_eq!(buf_cell(&term, 8, 0).bg, theme.control);
        assert_eq!(
            buf_cell(&term, 8, 2).fg,
            crate::theme::lift_color(theme.control, -0.08)
        );
        assert_eq!(buf_cell(&term, 8, 2).bg, theme.control);
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
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
        })
        .unwrap();
        // Sunken: the pressed fill's dark edge on top, its light edge on
        // the bottom — both drawn on the pressed fill itself.
        let light = crate::theme::lift_color(theme.accent_edge_dark, 0.12);
        let dark = crate::theme::lift_color(theme.accent_edge_dark, -0.12);
        let top = buf_cell(&term, 8, 0);
        assert_eq!(top.symbol(), "▔");
        assert_eq!(top.fg, dark);
        assert_eq!(top.bg, theme.accent_edge_dark);
        assert_eq!(buf_cell(&term, 8, 1).symbol(), "S");
        let bottom = buf_cell(&term, 8, 2);
        assert_eq!(bottom.symbol(), "▁");
        assert_eq!(bottom.fg, light);
        assert_eq!(bottom.bg, theme.accent_edge_dark);
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
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
        })
        .unwrap();
        // The whole control reacts to hover: the edges are the light/dark
        // shades of the *hovered* fill, not the base accent's pinned pair.
        let light = crate::theme::lift_color(theme.accent_edge_light, 0.12);
        let dark = crate::theme::lift_color(theme.accent_edge_light, -0.12);
        assert_eq!(buf_cell(&term, 8, 1).bg, theme.accent_edge_light);
        assert_eq!(buf_cell(&term, 8, 0).fg, light);
        assert_eq!(buf_cell(&term, 8, 0).bg, theme.accent_edge_light);
        assert_eq!(buf_cell(&term, 8, 2).fg, dark);
        assert_eq!(buf_cell(&term, 8, 2).bg, theme.accent_edge_light);
    }

    #[test]
    fn focused_button_lifts_like_hover() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            Button {
                label: "Send",
                kind: ButtonKind::Primary,
                state: ControlState::Focused,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
        })
        .unwrap();
        // Focus is the same surface lift hover uses — fill up one step,
        // edges following the shown fill; no special edge recolor.
        assert_eq!(buf_cell(&term, 8, 1).bg, theme.accent_edge_light);
        let light = crate::theme::lift_color(theme.accent_edge_light, 0.12);
        let dark = crate::theme::lift_color(theme.accent_edge_light, -0.12);
        assert_eq!(buf_cell(&term, 8, 0).fg, light);
        assert_eq!(buf_cell(&term, 8, 2).fg, dark);
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
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
        })
        .unwrap();
        assert_eq!(
            buf_cell(&term, 8, 1).fg,
            crate::theme::mix(
                theme.control,
                theme.text_muted,
                crate::paint::DISABLED_LABEL_MIX
            ),
            "disabled label blends text_muted toward the fill, dimmer than a resting muted label"
        );
        // Flat: no bevel glyphs at all — top/bottom rows are plain fill.
        let top = buf_cell(&term, 8, 0);
        assert_eq!(top.symbol(), " ");
        assert_eq!(top.bg, theme.control);
        let bottom = buf_cell(&term, 8, 2);
        assert_eq!(bottom.symbol(), " ");
        assert_eq!(bottom.bg, theme.control);
    }
}
