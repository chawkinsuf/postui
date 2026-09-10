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

/// How far a hovered keycap's tint source is pushed away from the page, in
/// Oklab lightness. Tuned so the *painted* pill — which [`Chip::paint`]
/// tints at 22%, damping the move — lands about 0.12 from its resting fill
/// on every built-in theme.
const KEYCAP_WARM: f32 = 0.14;

/// How far past `theme.control_hover` a hovered solid control lifts, in
/// Oklab lightness. The bare `control` -> `control_hover` rung is only
/// about 0.04 apart — legible, but a whisper next to the keycap beside it;
/// this carries a hovered chip to about 0.07 so the two read as the same
/// strength of response.
const HOVER_LIFT: f32 = 0.03;

/// Which way this theme's surface ladder runs: `+1` on a dark theme, where
/// each rung is lighter than the last, `-1` on a light one, where it runs
/// down instead.
///
/// Both hover strengths are steps *along that ladder*, never absolute
/// lightenings — an absolute lift walks a light theme's control back toward
/// its page and cancels the hover rather than strengthening it. Mirrors the
/// `step` that `Theme::generate` builds the ladder with in the first place.
fn ladder_step(theme: &Theme) -> f32 {
    if crate::theme::is_light(theme.page) {
        -1.0
    } else {
        1.0
    }
}

/// The `(tint source, surface)` a clickable keycap pill paints with, given
/// whether the pointer is on it and how far the shared hover fade
/// ([`crate::components::DrawCtx::hover_t`]) has got.
///
/// A resting pill is [`Chip`]'s usual `text_muted` over `theme.control`.
/// A hovered one *warms*: the tint source eases toward `theme.text` while
/// the surface eases toward `theme.control_hover`. Both have to move —
/// [`Chip::paint`] tints at 22%, so lifting the surface alone shifts the
/// painted fill by about two RGB points, which is invisible. Warming the
/// tint source moves it by a lightness step you can actually see.
///
/// One funnel for every keycap in the app (the footer's chip row, the app
/// bar's cycle pills and composite buttons, the split control's `alt+w`),
/// so they cannot drift apart.
pub fn keycap_face(theme: &Theme, hovered: bool, hover_t: f32) -> (Color, Color) {
    if !hovered {
        return (theme.text_muted, theme.control);
    }
    let warmed = crate::theme::lift_color(theme.text_muted, ladder_step(theme) * KEYCAP_WARM);
    (
        crate::theme::mix(theme.text_muted, warmed, hover_t),
        crate::theme::mix(theme.control, theme.control_hover, hover_t),
    )
}

/// A solid-filled control's surface, easing from its resting `rest` toward
/// `theme.control_hover` across the shared hover fade.
///
/// Unlike a keycap pill, a solid fill is not tinted down, so the ordinary
/// `control` -> `control_hover` step is already visible — this only has to
/// animate it. `rest` is the caller's own resting fill: `theme.control` for
/// the app bar's selector chips, `theme.panel` for the label half of a
/// composite button, whose whole span must lift as one.
pub fn hover_surface(theme: &Theme, rest: Color, hovered: bool, hover_t: f32) -> Color {
    if !hovered {
        return rest;
    }
    let target = crate::theme::lift_color(theme.control_hover, ladder_step(theme) * HOVER_LIFT);
    crate::theme::mix(rest, target, hover_t)
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

    /// A resting keycap is `Chip`'s usual muted tint over `control`; a
    /// hovered one warms both ends of that pair together — the tint source
    /// pushed away from the page by a fixed lightness step, the surface
    /// eased to `control_hover`.
    ///
    /// The warm is a *lift*, not a blend toward `theme.text`: how far
    /// `text_muted` sits from `text` varies enormously by theme (0.055 on
    /// solarized against 0.189 on the default dark), so blending toward it
    /// made the same hover shout on one theme and whisper on another. A
    /// lift lands every theme within a hair of the same strength.
    #[test]
    fn a_keycap_warms_both_its_tint_source_and_its_surface() {
        let theme = Theme::dark();
        let warmed = crate::theme::lift_color(theme.text_muted, KEYCAP_WARM);
        assert_eq!(
            keycap_face(&theme, false, 1.0),
            (theme.text_muted, theme.control),
            "an unhovered keycap rests, however far the fade has got"
        );
        assert_eq!(
            keycap_face(&theme, true, 0.0),
            (theme.text_muted, theme.control),
            "the fade starts from the resting pair exactly"
        );
        assert_eq!(
            keycap_face(&theme, true, 0.5),
            (
                mix(theme.text_muted, warmed, 0.5),
                mix(theme.control, theme.control_hover, 0.5)
            ),
            "mid-fade both ends are halfway"
        );
        assert_eq!(
            keycap_face(&theme, true, 1.0),
            (warmed, theme.control_hover)
        );
    }

    /// A light theme's surface ladder runs *downward* in lightness, so the
    /// warm has to follow it down — lifting up would walk the keycap back
    /// toward the page and cancel the hover instead of strengthening it.
    #[test]
    fn the_warm_follows_each_themes_own_ladder_away_from_the_page() {
        for b in crate::theme::builtin::builtin_themes() {
            let theme = Theme::generate(&b.seeds);
            let (warmed, _) = keycap_face(&theme, true, 1.0);
            let away_from_page = (lum(warmed) - lum(theme.page)).abs()
                > (lum(theme.text_muted) - lum(theme.page)).abs();
            assert!(
                away_from_page,
                "{}: the warm must move away from the page, not toward it",
                b.name
            );
        }
    }

    /// The point of warming the tint source: the surface alone moves the
    /// painted pill by ~0.008 lightness, which no one can see. Every
    /// built-in theme must clear a real margin.
    #[test]
    fn a_warmed_keycap_is_visibly_different_on_every_builtin_theme() {
        for b in crate::theme::builtin::builtin_themes() {
            let theme = Theme::generate(&b.seeds);
            let pill = |hovered| {
                let (color, on) = keycap_face(&theme, hovered, 1.0);
                theme.tint(color, on)
            };
            let delta = (lum(pill(true)) - lum(pill(false))).abs();
            assert!(
                (0.09..0.15).contains(&delta),
                "{}: a hovered keycap moves {delta:.4} — every theme must land \
                 in the same band, neither invisible nor shouting",
                b.name
            );
        }
    }

    /// A solid fill is not tinted down, so it only needs animating. `rest`
    /// is the caller's own resting surface — `control` for a selector chip,
    /// `panel` for the label half of a composite button.
    #[test]
    fn a_solid_surface_eases_from_its_own_rest_toward_control_hover() {
        let theme = Theme::dark();
        for rest in [theme.control, theme.panel] {
            assert_eq!(hover_surface(&theme, rest, false, 1.0), rest, "no hover");
            assert_eq!(hover_surface(&theme, rest, true, 0.0), rest, "starts at rest");
            assert_eq!(
                hover_surface(&theme, rest, true, 0.5),
                mix(
                    rest,
                    crate::theme::lift_color(theme.control_hover, HOVER_LIFT),
                    0.5
                )
            );
            assert_eq!(
                hover_surface(&theme, rest, true, 1.0),
                crate::theme::lift_color(theme.control_hover, HOVER_LIFT),
                "and every resting surface lands on the same hover fill — one \
                 rung past `control_hover`, so the lift actually reads"
            );
        }
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
