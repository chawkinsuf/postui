//! A painted, mouse-clickable button: a flat face on the middle row with a
//! quarter-row cap above and below, and a centered bold label — the
//! [`crate::paint::cap`] anatomy every full-size control in the app shares.

use ratatui::{buffer::Buffer, layout::Rect};

use crate::paint::{ControlState, cap, control_face, text};
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

/// Buttons are always exactly this many rows tall: a cap row, the label
/// row, a cap row. The same as [`crate::paint::FIELD_HEIGHT`] and
/// [`crate::paint::TALL_PILL_H`] — one block height for the whole
/// register, so a layout can swap any full-size control for any other
/// without moving what sits under it.
pub const BUTTON_HEIGHT: u16 = cap::CAP_H;

/// The minimum width a button needs to show `label` without truncation: the
/// label plus 2 columns of padding on each side.
pub fn button_min_width(label: &str) -> u16 {
    label.chars().count() as u16 + 4
}

impl Button<'_> {
    /// Paints this button into `area`, which must be exactly
    /// [`BUTTON_HEIGHT`] rows tall. The label is centered bold on the
    /// middle row; the rows above and below carry the button's caps over
    /// whatever surface it was laid out on (see [`cap::backdrop`]).
    ///
    /// Returns the rect a caller should register its hit over: the whole
    /// block, caps included. The caps *are* the button, and the rows
    /// they sit in are the layout's own padding — there is nothing else
    /// there for a click to have meant. An `area` too short for the
    /// block paints nothing and hands back a zero-height rect, so a
    /// caller that registers the return value cannot leave an
    /// invisible control clickable.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) -> Rect {
        let (face, label_fg) = control_face(theme, self.kind, self.state);
        let surface = cap::backdrop(buf, area, theme);
        let mid = cap::capped(buf, area, face, surface);
        if mid.height == 0 {
            return Rect { height: 0, ..area };
        }

        let width = self.label.chars().count() as u16;
        let start_x = area.x + area.width.saturating_sub(width) / 2;
        text(buf, start_x, mid.y, self.label, label_fg, face, true);
        area
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::cap::{CAP_BOTTOM, CAP_TOP};
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend, style::Color};

    fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    /// Paints a button over a page-filled buffer, the way every real
    /// caller does — the caps need a surface behind them to read.
    fn draw(kind: ButtonKind, state: ControlState, theme: &Theme) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            crate::paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.page);
            Button {
                label: "Send",
                kind,
                state,
            }
            .paint(f.buffer_mut(), Rect::new(0, 1, 20, 3), theme);
        })
        .unwrap();
        term
    }

    #[test]
    fn primary_button_centers_its_label_between_quarter_row_caps() {
        let theme = Theme::dark();
        let term = draw(ButtonKind::Primary, ControlState::Normal, &theme);

        let top = buf_cell(&term, 8, 1);
        assert_eq!(top.symbol(), CAP_TOP);
        assert_eq!(top.fg, theme.accent, "the cap's quarter is the button");
        assert_eq!(top.bg, theme.page, "the rest of the cap row is the page");

        let mid = buf_cell(&term, 8, 2);
        assert_eq!(mid.symbol(), "S");
        assert_eq!(mid.bg, theme.accent);
        assert_eq!(mid.fg, theme.on_accent);
        assert!(mid.modifier.contains(ratatui::style::Modifier::BOLD));

        let bottom = buf_cell(&term, 8, 3);
        assert_eq!(bottom.symbol(), CAP_BOTTOM);
        assert_eq!(bottom.fg, theme.page, "inverted: surface over the face");
        assert_eq!(bottom.bg, theme.accent);
    }

    #[test]
    fn no_button_paints_a_bevel_in_any_state() {
        let theme = Theme::dark();
        for kind in [ButtonKind::Primary, ButtonKind::Secondary] {
            for state in [
                ControlState::Normal,
                ControlState::Hover,
                ControlState::Focused,
                ControlState::Pressed,
                ControlState::Disabled,
            ] {
                let term = draw(kind, state, &theme);
                for y in 1..4 {
                    let s = buf_cell(&term, 2, y).symbol().to_string();
                    assert!(
                        s != "\u{2594}" && s != "\u{2581}",
                        "{kind:?}/{state:?} row {y} painted a bevel ({s:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn focused_outshines_hover_for_both_kinds() {
        let theme = Theme::dark();
        let l = |c: Color| crate::theme::oklab_l(crate::theme::rgb_of(c));
        for kind in [ButtonKind::Primary, ButtonKind::Secondary] {
            let hover = control_face(&theme, kind, ControlState::Hover).0;
            let focused = control_face(&theme, kind, ControlState::Focused).0;
            assert!(
                l(focused) > l(hover),
                "{kind:?}: Focused must read brighter than Hover"
            );
        }
    }

    #[test]
    fn a_disabled_buttons_label_blends_toward_its_own_fill() {
        let theme = Theme::dark();
        let term = draw(ButtonKind::Primary, ControlState::Disabled, &theme);
        let mid = buf_cell(&term, 8, 2);
        assert_eq!(mid.bg, theme.control, "disabled drops to the neutral face");
        assert_eq!(
            mid.fg,
            crate::theme::mix(
                theme.control,
                theme.text_muted,
                crate::paint::DISABLED_LABEL_MIX
            )
        );
    }

    #[test]
    fn a_button_caps_against_the_surface_it_was_laid_out_on() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            // A button in a list column sits on `panel`, not `page`.
            crate::paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
            Button {
                label: "Send",
                kind: ButtonKind::Secondary,
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), Rect::new(0, 1, 20, 3), &theme);
        })
        .unwrap();
        assert_eq!(
            buf_cell(&term, 2, 1).bg,
            theme.panel,
            "the cap must take the surface actually behind it"
        );
    }

    /// The caps are part of the button, and the rows they sit in are the
    /// layout's own padding — nothing else is there for a click to have
    /// meant. So the rect handed back for the hit map is the whole
    /// block, not just the label row.
    #[test]
    fn a_button_claims_its_caps_for_the_click() {
        let theme = Theme::dark();
        let area = Rect::new(2, 1, 10, BUTTON_HEIGHT);
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        let mut painted = None;
        term.draw(|f| {
            crate::paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.page);
            painted = Some(
                Button {
                    label: "Delete",
                    kind: ButtonKind::Secondary,
                    state: ControlState::Hover,
                }
                .paint(f.buffer_mut(), area, &theme),
            );
        })
        .unwrap();
        assert_eq!(painted, Some(area));
    }

    /// A pane too short for the block gets no button at all, and the
    /// rect handed back says so: registering it must not leave a
    /// control that is invisible but still clickable.
    #[test]
    fn a_refused_button_claims_nothing() {
        let theme = Theme::dark();
        let area = Rect::new(2, 1, 10, BUTTON_HEIGHT - 1);
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        let mut painted = None;
        term.draw(|f| {
            crate::paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.page);
            painted = Some(
                Button {
                    label: "Delete",
                    kind: ButtonKind::Secondary,
                    state: ControlState::Hover,
                }
                .paint(f.buffer_mut(), area, &theme),
            );
        })
        .unwrap();
        assert_eq!(painted.map(|r| r.height), Some(0));
    }

    /// The caps run the button's whole width — a cap that stopped short
    /// would leave the block notched at one end.
    #[test]
    fn the_caps_run_the_buttons_full_width() {
        let theme = Theme::dark();
        let area = Rect::new(2, 1, 10, BUTTON_HEIGHT);
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            crate::paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.page);
            Button {
                label: "Rename",
                kind: ButtonKind::Secondary,
                state: ControlState::Normal,
            }
            .paint(f.buffer_mut(), area, &theme);
        })
        .unwrap();
        let face = control_face(&theme, ButtonKind::Secondary, ControlState::Normal).0;
        for x in area.x..area.x + area.width {
            let top = buf_cell(&term, x, area.y);
            assert_eq!(top.symbol(), CAP_TOP, "top cap at x={x}");
            assert_eq!(top.fg, face);
            assert_eq!(top.bg, theme.page);
            assert_eq!(
                buf_cell(&term, x, area.y + 1).bg,
                face,
                "label row at x={x}"
            );
            let bottom = buf_cell(&term, x, area.y + 2);
            assert_eq!(bottom.symbol(), CAP_BOTTOM, "bottom cap at x={x}");
            assert_eq!(bottom.fg, theme.page);
            assert_eq!(bottom.bg, face);
        }
    }

    #[test]
    fn button_min_width_pads_the_label_by_two_columns_each_side() {
        assert_eq!(button_min_width("Send"), 8);
    }
}
