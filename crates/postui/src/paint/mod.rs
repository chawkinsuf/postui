//! Paint layer core: low-level helpers for painting flat-color surfaces,
//! the [`ControlState`] enum shared by every painted control, and
//! [`control_face`] — the single ladder from a control's kind and state to
//! the colors it paints with.
//!
//! The app has one control register and it is flat. [`cap`] owns its
//! anatomy; every full-size control ([`Button`], [`TextField`]) is that
//! anatomy with different content on the middle row,
//! and the compact one-row controls in [`property`] take their faces from
//! the same [`control_face`] ladder. Adding a control means picking a
//! height and a content painter, never inventing a look.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::theme::Theme;

pub mod button;
pub mod cap;
pub mod chip;
pub mod field;
pub mod frac;
pub mod panel;
pub mod property;
pub mod ring;
pub mod rows;
pub mod split_control;

pub use button::{BUTTON_HEIGHT, Button, ButtonKind, button_min_width};
pub use cap::{CAP_H, capped};
pub use chip::{Chip, TabStrip};
pub use field::{FIELD_HEIGHT, FIELD_PAD, TextField};
pub use frac::frac_vspan;
pub use panel::{dim_backdrop, fade_to, floating_panel, floating_panel_settling};
pub use property::{
    ControlSlot, HOVER_WASH, LABEL_W_MAX, LABEL_W_MIN, PILL_PAD, PROPERTY_MAX_W, Pill, PropertyRow,
    TALL_PILL_H, TOGGLE_W, Toggle, TrailingPill, WELL_PAD, Well, label_column, pill_min_width,
};
pub use ring::ring;
pub use rows::{ListRow, RowHighlight};
pub use split_control::{
    SPLIT_CONTROL_WIDTH, SPLIT_SEGMENT_WIDTH, STEP_CONTROL_WIDTH, STEP_SEGMENT_WIDTH, SplitControl,
    StepControl, split_glyph,
};

/// How far a Disabled control's label/content blends toward its own fill
/// from `theme.text_muted` (via `theme::mix`). Shared by [`Button`] and
/// [`TextField`] so both controls' disabled text reads at the same,
/// clearly-dimmer-than-resting-muted contrast.
pub const DISABLED_LABEL_MIX: f32 = 0.55;

/// The interaction state of a painted control. Determines which face/edge
/// colors a control paints with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlState {
    Normal,
    Hover,
    Pressed,
    Focused,
    Disabled,
}

/// The fill and content colors a control paints with, for every control
/// in the app.
///
/// One ladder, deliberately. `Button` used to carry its own copy in which
/// `Focused` and `Hover` were the same color, so a focused button and a
/// hovered one were indistinguishable while the Manage screen's controls
/// told them apart. `Focused` outshines `Hover` here — that is the app's
/// focus language: the control lifts its own surface, further than a
/// pointer resting on it does, and never draws a ring.
pub(crate) fn control_face(theme: &Theme, kind: ButtonKind, state: ControlState) -> (Color, Color) {
    use crate::theme::lift_color;
    let fill = match (kind, state) {
        (_, ControlState::Disabled) => theme.control,
        (ButtonKind::Primary, ControlState::Normal) => theme.accent,
        (ButtonKind::Primary, ControlState::Hover) => theme.accent_edge_light,
        (ButtonKind::Primary, ControlState::Focused) => lift_color(theme.accent, 0.20),
        (ButtonKind::Primary, ControlState::Pressed) => theme.accent_edge_dark,
        (ButtonKind::Secondary, ControlState::Normal) => theme.control,
        (ButtonKind::Secondary, ControlState::Hover) => theme.control_hover,
        (ButtonKind::Secondary, ControlState::Focused) => lift_color(theme.control, 0.12),
        (ButtonKind::Secondary, ControlState::Pressed) => theme.control_pressed,
    };
    let fg = match (kind, state) {
        (_, ControlState::Disabled) => {
            crate::theme::mix(fill, theme.text_muted, DISABLED_LABEL_MIX)
        }
        (ButtonKind::Primary, _) => theme.on_accent,
        (ButtonKind::Secondary, _) => theme.text,
    };
    (fill, fg)
}

/// Fills every cell in `area` with a blank (" ") glyph on `bg`.
pub fn fill(buf: &mut Buffer, area: Rect, bg: Color) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_bg(bg);
            }
        }
    }
}

/// A hairline divider: a run of `▔` (upper one-eighth block) across
/// `row`.
///
/// A separator *between regions* — the line that closes a table, say.
/// Not a control edge: the app's controls are flat and shade nothing,
/// and this is the one remaining use of the glyph that used to draw
/// their bevels. Kept apart under its own name so a control never
/// reaches for it again.
pub fn rule(buf: &mut Buffer, row: Rect, fg: Color, bg: Color) {
    for x in row.left()..row.right() {
        if let Some(cell) = buf.cell_mut((x, row.top())) {
            cell.set_symbol("▔");
            cell.set_fg(fg);
            cell.set_bg(bg);
        }
    }
}

/// Paints `s` starting at `(x, y)` with the given fg/bg, optionally bold.
pub fn text(buf: &mut Buffer, x: u16, y: u16, s: &str, fg: Color, bg: Color, bold: bool) {
    use ratatui::style::{Modifier, Style};
    let mut style = Style::default().fg(fg).bg(bg);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    // `Buffer::set_string` panics on a cell outside the buffer; a caller
    // laying out for a tiny terminal can legitimately land there.
    if y < buf.area.top() || y >= buf.area.bottom() || x < buf.area.left() || x >= buf.area.right()
    {
        return;
    }
    buf.set_string(x, y, s, style);
}

/// A plain (unbracketed) clickable text action painted on `surface`:
/// accent fg at rest; inverted (accent fill, `on_accent` fg, bold) while
/// `hovered == Some(&hit)`. The response toolbar's icons, its search
/// arrows, and the variable tooltip's pills all share this treatment.
pub fn action(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    hit: crate::hit::Hit,
    hovered: Option<&crate::hit::Hit>,
    surface: Color,
    theme: &Theme,
) {
    if hovered == Some(&hit) {
        fill(buf, area, theme.accent);
        text(
            buf,
            area.x,
            area.y,
            label,
            theme.on_accent,
            theme.accent,
            true,
        );
    } else {
        text(buf, area.x, area.y, label, theme.accent, surface, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{mix, oklab_l, rgb_of};

    fn lum(c: Color) -> f32 {
        oklab_l(rgb_of(c))
    }

    #[test]
    fn focused_outshines_hover_on_the_shared_ladder() {
        let theme = Theme::dark();
        for kind in [ButtonKind::Primary, ButtonKind::Secondary] {
            let hover = control_face(&theme, kind, ControlState::Hover).0;
            let focused = control_face(&theme, kind, ControlState::Focused).0;
            assert!(
                lum(focused) > lum(hover),
                "{kind:?}: a focused control must read brighter than a hovered one"
            );
        }
    }

    #[test]
    fn every_control_state_lands_on_a_distinct_face() {
        let theme = Theme::dark();
        for kind in [ButtonKind::Primary, ButtonKind::Secondary] {
            let faces: Vec<Color> = [
                ControlState::Normal,
                ControlState::Hover,
                ControlState::Focused,
                ControlState::Pressed,
            ]
            .iter()
            .map(|s| control_face(&theme, kind, *s).0)
            .collect();
            for (i, a) in faces.iter().enumerate() {
                for b in faces.iter().skip(i + 1) {
                    assert_ne!(a, b, "{kind:?} paints two states the same");
                }
            }
        }
    }

    #[test]
    fn disabled_drops_to_the_neutral_face_for_both_kinds() {
        let theme = Theme::dark();
        for kind in [ButtonKind::Primary, ButtonKind::Secondary] {
            let (fill, fg) = control_face(&theme, kind, ControlState::Disabled);
            assert_eq!(fill, theme.control);
            assert_eq!(fg, mix(fill, theme.text_muted, DISABLED_LABEL_MIX));
        }
    }
}
