//! A painted text field: a 3-row filled rect with light/dark bevel edges on
//! the top/bottom rows and left-padded content on the middle row. Focus
//! lifts the field's own fill (bevel following), matching the address bar's
//! focus language — no ring in surrounding cells.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};

use crate::paint::{ControlState, bevel_bottom, bevel_top, fill};
use crate::theme::Theme;

/// Text fields are always exactly this many rows tall: a bevel row on top,
/// the content row, a bevel row on the bottom.
pub const FIELD_HEIGHT: u16 = 3;

/// A painted text field: its content line and current interaction state.
/// `paint` draws it into a 3-row-tall area. [`ControlState::Focused`]
/// brightens the fill two hover-steps (the same lift the address bar's URL
/// well uses), with the bevel following the lifted fill.
pub struct TextField<'a> {
    pub content: Line<'a>,
    pub state: ControlState,
}

/// The face/edge colors a text field paints with for a given state.
struct Face {
    fill: Color,
    /// `None` when no bevel should be drawn (Disabled).
    edges: Option<(Color, Color)>,
}

impl TextField<'_> {
    /// Paints this field into `area`, which must be at least [`FIELD_HEIGHT`]
    /// rows tall. Content is drawn on the middle row with a 2-column left
    /// padding.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        let face = self.face(theme);

        fill(buf, area, face.fill);

        let top = Rect::new(area.x, area.y, area.width, 1);
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

        if let Some((light, dark)) = face.edges {
            bevel_top(buf, top, light, face.fill);
            bevel_bottom(buf, bottom, dark, face.fill);
        }

        let mid_y = area.y + 1;
        let text_x = area.x + 2;
        let width = area.width.saturating_sub(2);

        let line = if self.state == ControlState::Disabled {
            Line::from(
                self.content
                    .spans
                    .iter()
                    .map(|s| {
                        Span::styled(s.content.clone(), Style::default().fg(theme.text_disabled))
                    })
                    .collect::<Vec<_>>(),
            )
        } else {
            let mut l = self.content.clone();
            l.style = Style::default().fg(theme.text).bg(face.fill).patch(l.style);
            l
        };
        buf.set_line(text_x, mid_y, &line, width);
    }

    fn face(&self, theme: &Theme) -> Face {
        use crate::theme::lift_color;
        match self.state {
            ControlState::Normal => Face {
                fill: theme.control,
                edges: Some((theme.edge_light, theme.edge_dark)),
            },
            // Two hover-steps up, like the address bar's focused URL well —
            // one step is nearly invisible on a dark fill. Bevel follows
            // the lifted fill at the same ±0.08 the resting bevel uses.
            ControlState::Focused => {
                let fill = lift_color(theme.control, 0.12);
                Face {
                    fill,
                    edges: Some((lift_color(fill, 0.08), lift_color(fill, -0.08))),
                }
            }
            ControlState::Hover => Face {
                fill: theme.control_hover,
                edges: Some((theme.edge_light, theme.edge_dark)),
            },
            ControlState::Pressed => Face {
                fill: theme.control_pressed,
                edges: Some((theme.edge_light, theme.edge_dark)),
            },
            ControlState::Disabled => Face {
                fill: theme.control,
                edges: None,
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
    fn focused_field_lifts_its_own_fill() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(30, 7)).unwrap();
        term.draw(|f| {
            let inner = Rect::new(2, 2, 26, 3);
            TextField {
                content: Line::raw("hello"),
                state: ControlState::Focused,
            }
            .paint(f.buffer_mut(), inner, &theme);
        })
        .unwrap();
        let lifted = crate::theme::lift_color(theme.control, 0.12);
        let mid = buf_cell(&term, 4, 3);
        assert_eq!(mid.symbol(), "h"); // 2-col padding
        assert_eq!(mid.bg, lifted, "focused fill outshines control_hover");
        assert_ne!(lifted, theme.control_hover);
        // Bevel follows the lifted fill, not the resting edge tokens.
        assert_eq!(
            buf_cell(&term, 2, 2).fg,
            crate::theme::lift_color(lifted, 0.08)
        );
        // No ring: surrounding cells stay untouched.
        assert_eq!(buf_cell(&term, 1, 1).symbol(), " ");
    }

    #[test]
    fn normal_field_paints_fill_and_bevel() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            TextField {
                content: Line::raw("hi"),
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), Rect::new(0, 1, 20, 3), &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 0, 1).symbol(), "▔");
        assert_eq!(buf_cell(&term, 0, 3).symbol(), "▁");
        let mid = buf_cell(&term, 2, 2);
        assert_eq!(mid.symbol(), "h");
        assert_eq!(mid.bg, theme.control);
    }

    #[test]
    fn hover_field_uses_hover_fill() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            TextField {
                content: Line::raw("hi"),
                state: ControlState::Hover,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 0, 1).bg, theme.control_hover);
    }

    #[test]
    fn disabled_field_recolors_content_and_drops_bevel() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
        term.draw(|f| {
            TextField {
                content: Line::raw("hi"),
                state: ControlState::Disabled,
            }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 0, 0).symbol(), " ");
        assert_eq!(buf_cell(&term, 2, 1).fg, theme.text_disabled);
    }
}
