//! A painted text field: a flat filled content row with a quarter-row cap
//! above and below, and left-padded content — the same
//! [`crate::paint::cap`] anatomy [`crate::paint::Button`] uses, so a field
//! and a button standing side by side in a dialog read as one register.
//! Focus lifts the field's own fill; there is no ring in surrounding cells
//! and no bevel.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};

use crate::paint::{ButtonKind, ControlState, cap, control_face};
use crate::theme::Theme;

/// Text fields are always exactly this many rows tall: a cap row, the
/// content row, a cap row — the same block every full-size control
/// occupies.
pub const FIELD_HEIGHT: u16 = cap::CAP_H;

/// How far a field's content is inset from its left edge.
pub const FIELD_PAD: u16 = 2;

/// A painted text field: its content line and current interaction state.
/// `paint` draws it into a [`FIELD_HEIGHT`]-row area.
pub struct TextField<'a> {
    pub content: Line<'a>,
    pub state: ControlState,
}

impl TextField<'_> {
    /// Paints this field into `area`, which must be at least
    /// [`FIELD_HEIGHT`] rows tall. Content is drawn on the row below the
    /// top cap, inset [`FIELD_PAD`] columns from the left.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        // A field is a Secondary control that never takes the accent
        // face, so it shares the button's ladder rather than keeping a
        // second copy of it.
        let (face, _) = control_face(theme, ButtonKind::Secondary, self.state);
        let surface = cap::backdrop(buf, area, theme);
        let content = cap::capped(buf, area, face, surface);
        if content.height == 0 {
            return;
        }

        let text_x = area.x + FIELD_PAD;
        let width = area.width.saturating_sub(FIELD_PAD);

        let line = if self.state == ControlState::Disabled {
            // Blended toward the field's own fill rather than the flat
            // `text_disabled` token — matches `Button`'s disabled label
            // treatment so both controls' disabled text reads at the same,
            // clearly dimmer contrast.
            let disabled_fg =
                crate::theme::mix(face, theme.text_muted, crate::paint::DISABLED_LABEL_MIX);
            Line::from(
                self.content
                    .spans
                    .iter()
                    .map(|s| Span::styled(s.content.clone(), Style::default().fg(disabled_fg)))
                    .collect::<Vec<_>>(),
            )
        } else {
            let mut l = self.content.clone();
            l.style = Style::default().fg(theme.text).bg(face).patch(l.style);
            l
        };
        buf.set_line(text_x, content.y, &line, width);
    }

    /// The fill a field of this state paints with — for the callers that
    /// paint their own content (a caret, a selection) on top of it and
    /// need to know the backdrop they are drawing against.
    pub fn face(state: ControlState, theme: &Theme) -> Color {
        control_face(theme, ButtonKind::Secondary, state).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::cap::{CAP_BOTTOM, CAP_TOP};
    use ratatui::{Terminal, backend::TestBackend};

    fn draw(state: ControlState, theme: &Theme) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            crate::paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.page);
            TextField {
                content: Line::from("hello"),
                state,
            }
            .paint(f.buffer_mut(), Rect::new(0, 1, 20, FIELD_HEIGHT), theme);
        })
        .unwrap();
        term
    }

    #[test]
    fn a_field_pads_its_content_between_quarter_row_caps() {
        let theme = Theme::dark();
        let term = draw(ControlState::Normal, &theme);
        let buf = term.backend().buffer();

        assert_eq!(buf.cell((0, 1)).unwrap().symbol(), CAP_TOP);
        assert_eq!(buf.cell((0, 1)).unwrap().fg, theme.control);
        assert_eq!(buf.cell((0, 1)).unwrap().bg, theme.page);
        assert_eq!(buf.cell((0, 3)).unwrap().symbol(), CAP_BOTTOM);

        assert_eq!(
            buf.cell((FIELD_PAD, 2)).unwrap().symbol(),
            "h",
            "content starts FIELD_PAD columns in"
        );
        assert_eq!(buf.cell((FIELD_PAD, 2)).unwrap().bg, theme.control);
    }

    #[test]
    fn focus_lifts_the_fields_own_fill_and_paints_no_ring() {
        let theme = Theme::dark();
        let term = draw(ControlState::Focused, &theme);
        let buf = term.backend().buffer();
        let lifted = crate::theme::lift_color(theme.control, 0.12);
        assert_eq!(buf.cell((FIELD_PAD, 2)).unwrap().bg, lifted);
        assert_eq!(
            buf.cell((0, 1)).unwrap().fg,
            lifted,
            "the cap follows the lifted fill"
        );
        // Nothing outside the block changed: no ring.
        assert_eq!(buf.cell((0, 0)).unwrap().bg, theme.page);
        assert_eq!(buf.cell((0, 4)).unwrap().bg, theme.page);
    }

    #[test]
    fn no_field_paints_a_bevel_in_any_state() {
        let theme = Theme::dark();
        for state in [
            ControlState::Normal,
            ControlState::Hover,
            ControlState::Focused,
            ControlState::Pressed,
            ControlState::Disabled,
        ] {
            let term = draw(state, &theme);
            for y in 1..4 {
                let s = term
                    .backend()
                    .buffer()
                    .cell((0, y))
                    .unwrap()
                    .symbol()
                    .to_string();
                assert!(
                    s != "\u{2594}" && s != "\u{2581}",
                    "{state:?} row {y} painted a bevel ({s:?})"
                );
            }
        }
    }

    #[test]
    fn a_disabled_fields_content_blends_toward_its_own_fill() {
        let theme = Theme::dark();
        let term = draw(ControlState::Disabled, &theme);
        let cell = term.backend().buffer().cell((FIELD_PAD, 2)).unwrap();
        assert_eq!(
            cell.fg,
            crate::theme::mix(
                theme.control,
                theme.text_muted,
                crate::paint::DISABLED_LABEL_MIX
            )
        );
    }

    #[test]
    fn a_field_and_a_button_occupy_the_same_block() {
        assert_eq!(FIELD_HEIGHT, crate::paint::BUTTON_HEIGHT);
        assert_eq!(FIELD_HEIGHT, crate::paint::TALL_PILL_H);
    }
}
