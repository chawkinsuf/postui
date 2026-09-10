//! The app's one control anatomy: a **flat** face with quarter-row caps
//! above and below it.
//!
//! Every full-size control in the app — [`crate::paint::Button`],
//! [`crate::paint::TextField`] and the request editor's fused address
//! bar — is painted through
//! [`capped`]. There is one geometry and one place that knows it, so a
//! control added later cannot quietly invent a second look.
//!
//! **Flat, not raised.** The app used to draw these as raised surfaces:
//! a light one-eighth bevel (`▔`) along the top row and a dark one (`▁`)
//! along the bottom, straddling the fill. The Manage screen's property
//! rows could not carry that — a one-row control has no spare rows for
//! edges — so the app briefly spoke two dialects, raised in dialogs and
//! flat in panes. It now speaks one: nothing is bevelled anywhere, and a
//! control separates itself from the surface by *height* rather than by
//! shading.
//!
//! **The caps.** A control's face covers its content row plus a quarter
//! of each neighbouring row, leaving three quarters of the surface
//! showing above and below. That is what makes it read as something
//! floating at its own size rather than as a block the layout has grown.
//! The two orientations are a block-element trick: the row above draws
//! `▂` (lower one quarter) in the face over the surface, and the row
//! below draws `▆` (lower three quarters) *inverted* — surface over the
//! face — since Unicode has no upper-quarter block to draw the right way
//! up.
//!
//! **The surface has to be right.** The three quarters of each cap row
//! that are not control must be the colour actually behind them, or the
//! caps fringe the control with a wedge of the wrong surface. Controls
//! sit on `page` in the detail panes, on `panel` in a list column, and
//! on a floating panel's own fill inside a modal, so no constant can
//! serve them all. Rather than make every call site pass one — the
//! thing a new call site would get wrong — [`backdrop`] reads it back
//! out of the buffer, from the cells the cap is about to cover. By the
//! time a control paints, whatever is behind it has already painted.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::theme::Theme;

/// The rows a capped control spans: a quarter-row cap, the content row,
/// a quarter-row cap.
pub const CAP_H: u16 = 3;

/// Lower one quarter block — the top cap, painted face-over-surface.
pub const CAP_TOP: &str = "\u{2582}";

/// Lower three quarters block — the bottom cap, painted
/// surface-over-face so the quarter touching the content row is the one
/// that shows the control.
pub const CAP_BOTTOM: &str = "\u{2586}";

/// The rows a slivered control spans: an eighth-row cap, the content row,
/// an eighth-row cap. Same three rows as [`CAP_H`] — the control is
/// smaller within them, not the block.
pub const SLIVER_H: u16 = 3;

/// Lower one eighth block — a sliver cap's top row, face over surface.
pub const SLIVER_TOP: &str = "\u{2581}";

/// Upper one eighth block — a sliver cap's bottom row. Unlike
/// [`CAP_BOTTOM`] this paints the right way up: Unicode has an upper
/// one-eighth block, so the sliver needs none of the quarter cap's
/// inversion trick.
pub const SLIVER_BOTTOM: &str = "\u{2594}";

/// The surface a control about to paint into `area` is sitting on, read
/// from the cell at the block's top-left — the first cell its top cap
/// will cover, and so by definition part of the surface behind it.
///
/// Falls back to `theme.page` when that cell has no background of its
/// own: a caller painting into a buffer nothing has filled yet (a unit
/// test, or a control laid out past the buffer's edge) would otherwise
/// cap against ratatui's `Reset`, which the terminal renders as the
/// user's own background rather than the app's.
pub fn backdrop(buf: &Buffer, area: Rect, theme: &Theme) -> Color {
    match buf.cell((area.x, area.y)) {
        Some(cell) if cell.bg != Color::Reset => cell.bg,
        _ => theme.page,
    }
}

/// Paints a flat `face` with a cap above and below it, and returns the
/// content row for the caller to draw its label or text into.
///
/// `area` must be at least [`CAP_H`] rows tall; the content row is
/// always `area.y + 1`, so a caller lays out from the row it wants its
/// content on, minus one. An `area` taller than [`CAP_H`] grows the face
/// downward — the caps stay on the block's first and last rows — which
/// is what lets a multi-row well cap the same way a one-row one does.
///
/// Returns a zero-height rect for an `area` too short to hold the
/// anatomy, having painted nothing: a pane squeezed by a tiny terminal
/// legitimately lands there, and half a control is worse than none.
pub fn capped(buf: &mut Buffer, area: Rect, face: Color, surface: Color) -> Rect {
    if area.height < CAP_H || area.width == 0 {
        return Rect { height: 0, ..area };
    }
    let content = Rect {
        y: area.y + 1,
        height: area.height - 2,
        ..area
    };
    super::fill(buf, content, face);

    let bottom_y = area.y + area.height - 1;
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_symbol(CAP_TOP);
            cell.set_fg(face);
            cell.set_bg(surface);
        }
        if let Some(cell) = buf.cell_mut((x, bottom_y)) {
            cell.set_symbol(CAP_BOTTOM);
            cell.set_fg(surface);
            cell.set_bg(face);
        }
    }
    content
}

/// Paints a flat `face` with an eighth-row sliver above and below it,
/// returning the content row for the caller to draw into.
///
/// The 1.25-row sibling of [`capped`]: same three-row block, a quarter as
/// much of it spent on the caps. This is the anatomy for the app bar's
/// one-row chips, which sit in a 3-row bar with a blank panel row above
/// and below the content row — enough for a sliver, not for a quarter cap
/// without the chips touching the bar's edges.
///
/// Refuses the same way [`capped`] does: an `area` too short paints
/// nothing and returns a zero-height rect.
pub fn slivered(buf: &mut Buffer, area: Rect, face: Color, surface: Color) -> Rect {
    if area.height < SLIVER_H || area.width == 0 {
        return Rect { height: 0, ..area };
    }
    let content = Rect {
        y: area.y + 1,
        height: area.height - 2,
        ..area
    };
    super::fill(buf, content, face);

    let bottom_y = area.y + area.height - 1;
    for x in area.x..area.x + area.width {
        for (y, glyph) in [(area.y, SLIVER_TOP), (bottom_y, SLIVER_BOTTOM)] {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(glyph);
                cell.set_fg(face);
                cell.set_bg(surface);
            }
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn buf(w: u16, h: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, w, h))
    }

    /// The sliver cap is the quarter cap's smaller sibling: an eighth of
    /// each neighbouring row instead of a quarter, for the one-row chips
    /// on the app bar. Unlike the quarter cap it needs no inversion trick
    /// — Unicode has both an upper and a lower one-eighth block, so both
    /// rows paint face-over-surface the right way up.
    #[test]
    fn a_sliver_cap_takes_an_eighth_of_each_neighbouring_row() {
        let mut b = buf(6, 3);
        let content = slivered(&mut b, Rect::new(0, 0, 6, 3), Color::Red, Color::Blue);

        assert_eq!(content, Rect::new(0, 1, 6, 1), "content is the middle row");
        assert_eq!(b[(0, 1)].bg, Color::Red, "the content row is the face");

        let top = &b[(0, 0)];
        assert_eq!(top.symbol(), SLIVER_TOP, "lower eighth: face at the bottom");
        assert_eq!(top.fg, Color::Red);
        assert_eq!(top.bg, Color::Blue);

        let bottom = &b[(0, 2)];
        assert_eq!(
            bottom.symbol(),
            SLIVER_BOTTOM,
            "upper eighth: face at the top, painted the right way up"
        );
        assert_eq!(
            bottom.fg,
            Color::Red,
            "no inversion — an upper-eighth block exists, unlike the quarter"
        );
        assert_eq!(bottom.bg, Color::Blue);
    }

    /// Same refusal as `capped`: too short to hold the anatomy paints
    /// nothing and reports a zero-height rect, so a caller registering the
    /// return value cannot leave an invisible control clickable.
    #[test]
    fn a_sliver_too_short_to_fit_paints_nothing() {
        let mut b = buf(6, 2);
        let r = slivered(&mut b, Rect::new(0, 0, 6, 2), Color::Red, Color::Blue);
        assert_eq!(r.height, 0);
        assert_eq!(b[(0, 0)].bg, Color::Reset, "nothing painted");
    }

    #[test]
    fn caps_leave_three_quarters_of_the_surface_showing() {
        let mut b = buf(6, 3);
        let content = capped(&mut b, Rect::new(0, 0, 6, 3), Color::Red, Color::Blue);

        assert_eq!(content, Rect::new(0, 1, 6, 1), "content is the middle row");
        // Top cap: a quarter of face, painted over the surface.
        let top = &b[(0, 0)];
        assert_eq!(top.symbol(), CAP_TOP);
        assert_eq!(top.fg, Color::Red);
        assert_eq!(top.bg, Color::Blue);
        // Bottom cap is inverted, so the quarter touching the content
        // row is the face and the rest is surface.
        let bottom = &b[(0, 2)];
        assert_eq!(bottom.symbol(), CAP_BOTTOM);
        assert_eq!(bottom.fg, Color::Blue);
        assert_eq!(bottom.bg, Color::Red);
        assert_eq!(b[(0, 1)].bg, Color::Red, "content row is solid face");
    }

    #[test]
    fn nothing_is_bevelled() {
        let mut b = buf(4, 3);
        capped(&mut b, Rect::new(0, 0, 4, 3), Color::Red, Color::Blue);
        for y in 0..3 {
            let s = b[(0, y)].symbol().to_string();
            assert!(
                s != "\u{2594}" && s != "\u{2581}",
                "row {y} painted a bevel glyph ({s:?}); the flat register has none"
            );
        }
    }

    #[test]
    fn a_taller_block_grows_the_face_not_the_caps() {
        let mut b = buf(4, 5);
        let content = capped(&mut b, Rect::new(0, 0, 4, 5), Color::Red, Color::Blue);
        assert_eq!(content, Rect::new(0, 1, 4, 3));
        assert_eq!(b[(0, 0)].symbol(), CAP_TOP);
        assert_eq!(b[(0, 4)].symbol(), CAP_BOTTOM, "cap stays on the last row");
        for y in 1..4 {
            assert_eq!(b[(0, y)].bg, Color::Red);
        }
    }

    #[test]
    fn a_block_too_short_paints_nothing() {
        let mut b = buf(4, 2);
        let content = capped(&mut b, Rect::new(0, 0, 4, 2), Color::Red, Color::Blue);
        assert_eq!(content.height, 0);
        assert_eq!(b[(0, 0)].bg, Color::Reset, "painted nothing at all");
    }

    #[test]
    fn backdrop_reads_what_is_already_painted_and_falls_back_to_page() {
        let theme = Theme::dark();
        let mut b = buf(4, 3);
        assert_eq!(
            backdrop(&b, Rect::new(0, 0, 4, 3), &theme),
            theme.page,
            "an unpainted buffer caps against the app's page, not Reset"
        );
        super::super::fill(&mut b, Rect::new(0, 0, 4, 3), theme.panel);
        assert_eq!(backdrop(&b, Rect::new(0, 0, 4, 3), &theme), theme.panel);
    }
}
