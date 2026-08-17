//! A painted, mouse-clickable button: a 3-row filled rect with light/dark
//! bevel edges on the top/bottom rows and a centered label on the middle
//! row.

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

/// Buttons are always exactly this many rows tall: a bevel row on top, the
/// label row, a bevel row on the bottom.
pub const BUTTON_HEIGHT: u16 = 3;

/// The minimum width a button needs to show `label` without truncation: the
/// label plus 2 columns of padding on each side.
pub fn button_min_width(label: &str) -> u16 {
    label.chars().count() as u16 + 4
}

/// The face/edge colors a button paints with for a given kind + state.
struct Face {
    fill: Color,
    /// `None` when no bevel should be drawn (Disabled).
    edges: Option<(Color, Color)>,
    label_fg: Color,
}

impl Button<'_> {
    /// Paints this button into `area`, which must be exactly
    /// [`BUTTON_HEIGHT`] rows tall. The label is centered on the middle row.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        let face = self.face(theme);

        fill(buf, area, face.fill);

        let top = Rect::new(area.x, area.y, area.width, 1);
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

        if let Some((light, dark)) = face.edges {
            match self.state {
                ControlState::Pressed => {
                    // Pressed: bevel inverts — the "sunken" glyphs swap rows
                    // (▁ on top, ▔ on bottom) along with their edge colors
                    // (dark on top, light on bottom).
                    bevel_bottom(buf, top, dark, face.fill);
                    bevel_top(buf, bottom, light, face.fill);
                }
                _ => {
                    bevel_top(buf, top, light, face.fill);
                    bevel_bottom(buf, bottom, dark, face.fill);
                }
            }
        }

        let mid_y = area.y + 1;
        let width = self.label.chars().count() as u16;
        let start_x = area.x + area.width.saturating_sub(width) / 2;
        text(
            buf,
            start_x,
            mid_y,
            self.label,
            face.label_fg,
            face.fill,
            true,
        );
    }

    fn face(&self, theme: &Theme) -> Face {
        match self.kind {
            ButtonKind::Primary => match self.state {
                ControlState::Normal => Face {
                    fill: theme.accent,
                    edges: Some((theme.accent_edge_light, theme.accent_edge_dark)),
                    label_fg: theme.on_accent,
                },
                ControlState::Hover => Face {
                    fill: theme.accent_edge_light,
                    edges: Some((theme.accent_edge_light, theme.accent_edge_dark)),
                    label_fg: theme.on_accent,
                },
                ControlState::Pressed => Face {
                    fill: theme.accent_edge_dark,
                    edges: Some((theme.accent_edge_light, theme.accent_edge_dark)),
                    label_fg: theme.on_accent,
                },
                ControlState::Focused => Face {
                    fill: theme.accent,
                    edges: Some((theme.focus_ring, theme.focus_ring)),
                    label_fg: theme.on_accent,
                },
                ControlState::Disabled => Face {
                    fill: theme.control,
                    edges: None,
                    label_fg: theme.text_disabled,
                },
            },
            ButtonKind::Secondary => match self.state {
                ControlState::Normal => Face {
                    fill: theme.control,
                    edges: Some((theme.edge_light, theme.edge_dark)),
                    label_fg: theme.text,
                },
                ControlState::Hover => Face {
                    fill: theme.control_hover,
                    edges: Some((theme.edge_light, theme.edge_dark)),
                    label_fg: theme.text,
                },
                ControlState::Pressed => Face {
                    fill: theme.control_pressed,
                    edges: Some((theme.edge_light, theme.edge_dark)),
                    label_fg: theme.text,
                },
                ControlState::Focused => Face {
                    fill: theme.control,
                    edges: Some((theme.focus_ring, theme.focus_ring)),
                    label_fg: theme.text,
                },
                ControlState::Disabled => Face {
                    fill: theme.control,
                    edges: None,
                    label_fg: theme.text_disabled,
                },
            },
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
    fn primary_button_paints_fill_bevel_and_centered_bold_label() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            let area = Rect::new(0, 1, 20, 3);
            Button {
                label: "Send",
                kind: ButtonKind::Primary,
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), area, &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 0, 1).symbol(), "▔");
        assert_eq!(buf_cell(&term, 0, 3).symbol(), "▁");
        let mid = buf_cell(&term, 8, 2); // "Send" centered in 20 cols starts at 8
        assert_eq!(mid.symbol(), "S");
        assert_eq!(mid.bg, theme.accent);
        assert_eq!(mid.fg, theme.on_accent);
        assert!(mid.modifier.contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn pressed_button_inverts_bevel() {
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
        assert_eq!(buf_cell(&term, 0, 0).symbol(), "▁");
        assert_eq!(buf_cell(&term, 0, 2).symbol(), "▔");
    }

    #[test]
    fn disabled_button_has_muted_label_and_no_bevel() {
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
        assert_eq!(buf_cell(&term, 0, 0).symbol(), " ");
        assert_eq!(buf_cell(&term, 8, 1).fg, theme.text_disabled);
    }
}
