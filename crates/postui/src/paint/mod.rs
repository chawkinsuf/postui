//! Paint layer core: low-level helpers for painting flat-color surfaces and
//! 1-cell "bevel" edges that give controls a raised/pressed look using half
//! block glyphs, plus the [`ControlState`] enum shared by all painted
//! controls.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

pub mod button;

pub use button::{BUTTON_HEIGHT, Button, ButtonKind, button_min_width};

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
