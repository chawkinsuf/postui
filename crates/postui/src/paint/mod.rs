//! Paint layer core: low-level helpers for painting flat-color surfaces and
//! 1-cell "bevel" edges that give controls a raised/pressed look using half
//! block glyphs, plus the [`ControlState`] enum shared by all painted
//! controls.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::theme::Theme;

pub mod button;
pub mod chip;
pub mod field;
pub mod frac;
pub mod panel;
pub mod ring;
pub mod rows;

pub use button::{BUTTON_HEIGHT, Button, ButtonKind, button_min_width};
pub use chip::{Chip, TabStrip};
pub use field::{FIELD_HEIGHT, TextField};
pub use frac::frac_vspan;
pub use panel::{dim_backdrop, fade_to, floating_panel, floating_panel_settling};
pub use ring::ring;
pub use rows::{ListRow, RowHighlight};

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

/// Paints a run of `"▔"` (upper one-eighth block) across `row`, used as the
/// light bevel edge on the top row of a raised control.
pub fn bevel_top(buf: &mut Buffer, row: Rect, fg: Color, bg: Color) {
    for x in row.left()..row.right() {
        if let Some(cell) = buf.cell_mut((x, row.top())) {
            cell.set_symbol("▔");
            cell.set_fg(fg);
            cell.set_bg(bg);
        }
    }
}

/// Paints a run of `"▁"` (lower one-eighth block) across `row`, used as the
/// dark bevel edge on the bottom row of a raised control.
pub fn bevel_bottom(buf: &mut Buffer, row: Rect, fg: Color, bg: Color) {
    for x in row.left()..row.right() {
        if let Some(cell) = buf.cell_mut((x, row.top())) {
            cell.set_symbol("▁");
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
    buf.set_string(x, y, s, style);
}

/// The (light, dark) bevel edge colors for a control whose face is an
/// arbitrary colored fill (e.g. a method badge painted in `method_color`),
/// rather than one of the theme's own `control`/`accent` surfaces (which
/// already carry precomputed edge tokens). Edges are `face` lifted `±0.12`
/// in Oklab lightness, straddling the face's own lightness the same way
/// `accent_edge_light`/`accent_edge_dark` straddle `accent`.
pub fn face_edges(face: Color, _theme: &Theme) -> (Color, Color) {
    (
        crate::theme::lift_color(face, 0.12),
        crate::theme::lift_color(face, -0.12),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{oklab_l, rgb_of};

    #[test]
    fn face_edges_straddle_the_faces_lightness() {
        let theme = Theme::dark();
        let face = theme.method_color(postui_core::model::Method::Post); // accent (mid lightness)
        let (light, dark) = face_edges(face, &theme);
        let l = |c: Color| oklab_l(rgb_of(c));
        assert!(
            l(light) > l(face),
            "light edge must be lighter than the face"
        );
        assert!(l(dark) < l(face), "dark edge must be darker than the face");
    }
}
