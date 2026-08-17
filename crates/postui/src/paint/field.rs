//! A painted text field: a 3-row filled rect with light/dark bevel edges on
//! the top/bottom rows and left-padded content on the middle row, plus a
//! standalone focus ring painted in the cells surrounding a focused control.

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
/// `paint` draws it into a 3-row-tall area. When `state` is
/// [`ControlState::Focused`], the caller is responsible for also calling
/// [`focus_ring`] on the surrounding cells — `paint` itself only draws the
/// normal face.
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
        match self.state {
            ControlState::Normal | ControlState::Focused => Face {
                fill: theme.control,
                edges: Some((theme.edge_light, theme.edge_dark)),
            },
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

/// Paints an accent ring in the cells one step out from `inner` on all
/// sides, over `surround_bg`. Used by [`TextField`] (when `state ==
/// Focused`) and by any other standalone focused control — the caller is
/// expected to have reserved that surrounding ring of cells already.
pub fn focus_ring(buf: &mut Buffer, inner: Rect, surround_bg: Color, theme: &Theme) {
    let fg = theme.focus_ring;
    let left = inner.x.saturating_sub(1);
    let top = inner.y.saturating_sub(1);
    let right = inner.x + inner.width;
    let bottom = inner.y + inner.height;

    let set = |buf: &mut Buffer, x: u16, y: u16, s: &str| {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(s);
            cell.set_fg(fg);
            cell.set_bg(surround_bg);
        }
    };

    set(buf, left, top, "┌");
    set(buf, right, top, "┐");
    set(buf, left, bottom, "└");
    set(buf, right, bottom, "┘");

    for x in (left + 1)..right {
        set(buf, x, top, "─");
        set(buf, x, bottom, "─");
    }
    for y in (top + 1)..bottom {
        set(buf, left, y, "│");
        set(buf, right, y, "│");
    }
}

/// Builds the content line for a select-style text field: `label` left
/// aligned, padded, with a right-aligned `▾` glyph in `theme.text_muted`
/// marking it as a dropdown. `width` is the printable width the line will
/// be drawn into (i.e. the field's content width, after the 2-column left
/// padding [`TextField::paint`] applies).
pub fn select_line(label: &str, width: u16, theme: &Theme) -> Line<'static> {
    let label = label.to_string();
    let label_width = label.chars().count() as u16;
    let used = label_width.saturating_add(1); // +1 for the arrow glyph
    let pad = width.saturating_sub(used);

    let mut left = label;
    for _ in 0..pad {
        left.push(' ');
    }

    Line::from(vec![
        Span::raw(left),
        Span::styled("▾", Style::default().fg(theme.text_muted)),
    ])
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
    fn focused_field_draws_ring_in_surrounding_cells() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(30, 7)).unwrap();
        term.draw(|f| {
            let inner = Rect::new(2, 2, 26, 3);
            TextField {
                content: Line::raw("hello"),
                state: ControlState::Focused,
            }
            .paint(f.buffer_mut(), inner, &theme);
            focus_ring(f.buffer_mut(), inner, theme.panel, &theme);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 1, 1).symbol(), "┌");
        assert_eq!(buf_cell(&term, 1, 1).fg, theme.focus_ring);
        assert_eq!(buf_cell(&term, 28, 5).symbol(), "┘");
        assert_eq!(buf_cell(&term, 4, 3).symbol(), "h"); // 2-col padding
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

    #[test]
    fn select_line_right_aligns_arrow_in_muted_text() {
        let theme = Theme::dark();
        let line = select_line("GET", 10, &theme);
        let mut term = Terminal::new(TestBackend::new(10, 1)).unwrap();
        term.draw(|f| {
            f.buffer_mut().set_line(0, 0, &line, 10);
        })
        .unwrap();
        assert_eq!(buf_cell(&term, 0, 0).symbol(), "G");
        let arrow = buf_cell(&term, 9, 0);
        assert_eq!(arrow.symbol(), "▾");
        assert_eq!(arrow.fg, theme.text_muted);
        assert!(!arrow.modifier.contains(Modifier::BOLD));
    }
}
