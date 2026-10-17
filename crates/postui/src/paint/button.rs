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

/// The buttons on a Manage detail pane's title row, laid out from the
/// pane's right edge inward. Each block straddles the title's row
/// (`y - 1 ..= y + 1`), and one that would come within two columns of
/// the title — measured in the cells the title paints — is dropped
/// rather than painted over it, and so is every button after it. A
/// dropped button stays reachable by its key.
pub struct TitleButtons {
    /// The right edge of the next button's gap.
    x: u16,
    /// The first column a button may start at.
    clear: u16,
    /// The title's row.
    y: u16,
}

impl TitleButtons {
    pub fn new(title_x: u16, title: &str, right_edge: u16, y: u16) -> Self {
        let w = u16::try_from(crate::ui::cells(title)).unwrap_or(u16::MAX);
        Self { x: right_edge, clear: title_x.saturating_add(w).saturating_add(2), y }
    }

    /// The block for a button showing `label`, with `gap` columns between
    /// it and whatever is to its right; `None` once there is no room.
    pub fn next(&mut self, label: &str, gap: u16) -> Option<Rect> {
        let w = button_min_width(label);
        let x = self.x.saturating_sub(w + gap);
        if x < self.clear {
            // Sticky: a later, narrower button must not jump the queue.
            self.x = 0;
            return None;
        }
        self.x = x;
        Some(Rect { x, y: self.y.saturating_sub(1), width: w, height: BUTTON_HEIGHT })
    }
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
            Button { label: "Send", kind, state }.paint(
                f.buffer_mut(),
                Rect::new(0, 1, 20, 3),
                theme,
            );
        })
        .unwrap();
        term
    }

    /// Buttons come off the right edge one gap apart, and the title is
    /// measured in the cells it paints: `本番` at 10 takes 4, so a button
    /// may start at 16 and no further left.
    #[test]
    fn title_buttons_stop_two_cells_past_the_painted_title() {
        let mut row = TitleButtons::new(10, "本番", 40, 1);
        // "Delete" is 10 wide: 40 - 11.
        assert_eq!(row.next("Delete", 1), Some(Rect::new(29, 0, 10, BUTTON_HEIGHT)));
        assert_eq!(row.next("Rename", 1), Some(Rect::new(18, 0, 10, BUTTON_HEIGHT)));
        assert_eq!(row.next("Move", 1), None, "8 wide would start at 9, inside the title");
        let mut exact = TitleButtons::new(10, "本番", 27, 1);
        assert_eq!(exact.next("Delete", 1).map(|r| r.x), Some(16), "two cells of air exactly");
    }

    /// Once a button is dropped, every one after it is too: a narrower
    /// one further down the list must not take the slot out of order.
    #[test]
    fn title_buttons_stay_dropped_after_the_first_that_does_not_fit() {
        let mut row = TitleButtons::new(10, "ab", 40, 1);
        assert_eq!(row.next("a button label far too long to fit", 1), None);
        assert_eq!(row.next("OK", 1), None, "OK alone would fit at 33");
    }

    /// A title past the edge of a `u16` saturates rather than wrapping
    /// into a small width that would let the buttons over it.
    #[test]
    fn title_buttons_saturate_on_a_runaway_title() {
        let mut row = TitleButtons::new(u16::MAX - 1, "abc", u16::MAX, 1);
        assert_eq!(row.next("OK", 1), None);
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
            assert!(l(focused) > l(hover), "{kind:?}: Focused must read brighter than Hover");
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
            crate::theme::mix(theme.control, theme.text_muted, crate::paint::DISABLED_LABEL_MIX)
        );
    }

    #[test]
    fn a_button_caps_against_the_surface_it_was_laid_out_on() {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
        term.draw(|f| {
            // A button in a list column sits on `panel`, not `page`.
            crate::paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
            Button { label: "Send", kind: ButtonKind::Secondary, state: ControlState::Normal }
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
                Button { label: "Delete", kind: ButtonKind::Secondary, state: ControlState::Hover }
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
                Button { label: "Delete", kind: ButtonKind::Secondary, state: ControlState::Hover }
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
            Button { label: "Rename", kind: ButtonKind::Secondary, state: ControlState::Normal }
                .paint(f.buffer_mut(), area, &theme);
        })
        .unwrap();
        let face = control_face(&theme, ButtonKind::Secondary, ControlState::Normal).0;
        for x in area.x..area.x + area.width {
            let top = buf_cell(&term, x, area.y);
            assert_eq!(top.symbol(), CAP_TOP, "top cap at x={x}");
            assert_eq!(top.fg, face);
            assert_eq!(top.bg, theme.page);
            assert_eq!(buf_cell(&term, x, area.y + 1).bg, face, "label row at x={x}");
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
