//! The split control: two 2-cell chips, right-aligned in the Editor
//! pane's tab-bar row — fixed chrome at the top of the column, so the
//! control never moves out from under the pointer the way the old
//! per-pane clusters on the (repositioning) response header did. Each
//! chip jumps the column to one end: "editor only" (the response
//! minimized) and "response only" (the editor minimized). The chip of
//! the end the column sits at is lit, and clicking it again restores the
//! split. Everything between the ends is the drag handle's and the
//! keyboard cycle's.
//!
//! Each chip's glyph is a two-tone mini-picture of its state: the
//! editor's share in one tone above the response's in another, drawn
//! with a lower-block glyph whose bg paints the editor mass above a
//! response-tone fg. The chips sit flush against each other — two glyph
//! cells each, no padding — so the pair fuses into one control face
//! (`▁▁▇▇`) reading as the boundary at the top and at the bottom; the
//! lit chip's picture goes accent.

use crate::split::{SplitState, SplitStop};
use crate::theme::Theme;
use ratatui::{buffer::Buffer, layout::Rect};

/// Each chip is 2 cells: its glyph doubled, no padding — the chips fuse
/// into one continuous mini-picture of the boundary.
pub const SPLIT_SEGMENT_WIDTH: u16 = 2;
/// The whole control: two contiguous chips plus a one-cell control-fill
/// cap at each end, so the pair reads as one pill-shaped control face
/// rather than a loose graphic.
pub const SPLIT_CONTROL_WIDTH: u16 = SPLIT_SEGMENT_WIDTH * 2 + 2;

/// The two chips, left to right, each with its mini-picture: the response
/// mass painted bottom-up in fg under the editor-tone bg (the lit share).
/// The lit editor mass is `7/8` of the cell at "editor only" and `1/8` at
/// "response only": both keep a one-eighth strip of the other tone (a `█`
/// or bare cell there left the chip with a single tone, so it read as an
/// empty slot rather than the last stop), and the strips match.
const CHIPS: [(SplitStop, &str); 2] = [
    (SplitStop::EditorFull, "\u{2581}\u{2581}"), // ▁▁ editor takes all
    (SplitStop::ResponseFull, "\u{2587}\u{2587}"), // ▇▇ response takes all
];

/// The split control, ready to paint: the current split (for the lit
/// chip) and which chip the pointer is over, if any (resolved from the
/// hit map by the caller).
pub struct SplitControl {
    pub state: SplitState,
    pub hovered: Option<SplitStop>,
}

impl SplitControl {
    /// Paints the control with its left edge at `(x, y)` and returns each
    /// chip's rect with the stop it jumps to, for hit registration.
    pub fn paint(&self, buf: &mut Buffer, x: u16, y: u16, theme: &Theme) -> [(Rect, SplitStop); 2] {
        let active = self.state.stop();
        // The end caps: control fill, so the pair sits on a visible
        // button face and reads as one control.
        crate::paint::fill(buf, Rect::new(x, y, 1, 1), theme.control);
        crate::paint::fill(buf, Rect::new(x + SPLIT_CONTROL_WIDTH - 1, y, 1, 1), theme.control);
        // Half-cell eaves above and below: the pill face bleeds half a
        // character past its row with half-block glyphs over whatever the
        // neighbor rows already painted, so the control gets some vertical
        // breathing room instead of being a one-row sliver. Skipped at the
        // buffer edges (the tests paint into a one-row buffer).
        for dx in 0..SPLIT_CONTROL_WIDTH {
            if y > buf.area.top()
                && let Some(cell) = buf.cell_mut((x + dx, y - 1))
            {
                cell.set_symbol("\u{2584}"); // ▄ lower half
                cell.set_fg(theme.control);
            }
            if let Some(cell) = buf.cell_mut((x + dx, y + 1)) {
                cell.set_symbol("\u{2580}"); // ▀ upper half
                cell.set_fg(theme.control);
            }
        }
        let mut i = 0u16;
        CHIPS.map(|(stop, glyph)| {
            let rect = Rect::new(x + 1 + i * SPLIT_SEGMENT_WIDTH, y, SPLIT_SEGMENT_WIDTH, 1);
            i += 1;
            let hovered = self.hovered == Some(stop);
            let lit = active == Some(stop);
            // The mini-picture fully covers its own cells, so hover and
            // "lit" are the picture's own tones changing. The tones follow
            // the app's glyph-on-button idiom, with the editor's share as
            // the lit mass (the control sets the upper pane's size): the
            // response mass is the control fill itself (one dark pill
            // face with the caps), the editor mass a glyph-grey above it
            // — hover lifts both a step like any button, and the lit
            // chip swaps into the accent pair (accent mass over the dim
            // selection tint) instead of a solid glowing block.
            // Resting editor mass: halfway between disabled and muted
            // text — `text_disabled` alone vanishes against the control
            // fill, full `text_muted` reads too hot as a solid 2-cell
            // block (it's a mass, not a glyph stroke).
            let resting_mass = crate::theme::mix(theme.text_disabled, theme.text_muted, 0.5);
            let (editor_tone, response_tone) = if lit {
                (theme.accent, theme.selection)
            } else if hovered {
                (theme.text_muted, theme.control_hover)
            } else {
                (resting_mass, theme.control)
            };
            crate::paint::text(buf, rect.x, y, glyph, response_tone, editor_tone, false);
            (rect, stop)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
        term.backend().buffer().cell((x, y)).unwrap()
    }

    fn paint(control: SplitControl) -> (Terminal<TestBackend>, [(Rect, SplitStop); 2]) {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 1)).unwrap();
        let mut rects = None;
        term.draw(|f| {
            rects = Some(control.paint(f.buffer_mut(), 2, 0, &theme));
        })
        .unwrap();
        (term, rects.unwrap())
    }

    #[test]
    fn control_paints_the_two_end_chips() {
        let (term, rects) = paint(SplitControl { state: SplitState::default(), hovered: None });
        assert_eq!(rects.map(|(_, s)| s), [SplitStop::EditorFull, SplitStop::ResponseFull]);
        for (i, (rect, _)) in rects.iter().enumerate() {
            assert_eq!(rect.width, SPLIT_SEGMENT_WIDTH);
            assert_eq!(rect.height, 1);
            // Chips start one cell in: the control-fill cap comes first.
            assert_eq!(rect.x, 3 + i as u16 * SPLIT_SEGMENT_WIDTH);
        }
        let row: String =
            (2..2 + SPLIT_CONTROL_WIDTH).map(|x| cell(&term, x, 0).symbol()).collect();
        assert_eq!(row, " \u{2581}\u{2581}\u{2587}\u{2587} ");
        // Both end caps carry the control fill, framing the pair as one
        // button face.
        let theme = Theme::dark();
        assert_eq!(cell(&term, 2, 0).bg, theme.control);
        assert_eq!(cell(&term, 2 + SPLIT_CONTROL_WIDTH - 1, 0).bg, theme.control);
    }

    #[test]
    fn resting_chips_paint_muted_two_tone_pictures_across_both_cells() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitControl { state: SplitState::default(), hovered: None });
        let mass = crate::theme::mix(theme.text_disabled, theme.text_muted, 0.5);
        for ((rect, stop), glyph) in rects.into_iter().zip(["\u{2581}", "\u{2587}"]) {
            // The editor mass (the bg, above the glyph) is the resting
            // grey; the response mass below is the control fill. Both
            // cells carry the glyph: the chip is nothing but its picture.
            for dx in 0..SPLIT_SEGMENT_WIDTH {
                let c = cell(&term, rect.x + dx, 0);
                assert_eq!(c.symbol(), glyph, "{stop:?}");
                assert_eq!(c.fg, theme.control, "{stop:?}");
                assert_eq!(c.bg, mass, "{stop:?}");
            }
        }
    }

    #[test]
    fn the_lit_chip_is_the_minimized_end() {
        let theme = Theme::dark();
        // Response minimized: the EditorFull chip is lit, the other quiet.
        let (term, rects) = paint(SplitControl {
            state: SplitState { response_minimized: true, ..Default::default() },
            hovered: None,
        });
        assert_eq!(cell(&term, rects[0].0.x + 1, 0).bg, theme.accent);
        assert_eq!(cell(&term, rects[0].0.x + 1, 0).fg, theme.selection);
        assert_ne!(cell(&term, rects[1].0.x + 1, 0).bg, theme.accent);
        // Editor minimized: the mirror.
        let (term, rects) = paint(SplitControl {
            state: SplitState { editor_minimized: true, ..Default::default() },
            hovered: None,
        });
        assert_eq!(cell(&term, rects[1].0.x + 1, 0).bg, theme.accent);
        assert_eq!(cell(&term, rects[1].0.x + 1, 0).fg, theme.selection);
        assert_ne!(cell(&term, rects[0].0.x + 1, 0).bg, theme.accent);
        // Both panes showing: neither chip is accent.
        let (term, rects) = paint(SplitControl { state: SplitState::default(), hovered: None });
        for (rect, _) in rects {
            assert_ne!(cell(&term, rect.x + 1, 0).bg, theme.accent);
        }
    }

    #[test]
    fn hover_brightens_a_chips_tones_lit_or_not() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitControl {
            state: SplitState::default(),
            hovered: Some(SplitStop::ResponseFull),
        });
        let chip = rects[1].0;
        assert_eq!(cell(&term, chip.x, 0).bg, theme.text_muted);
        assert_eq!(cell(&term, chip.x, 0).fg, theme.control_hover);

        // Hovering the lit chip changes nothing — its picture already
        // glows accent, and there is no face behind it to lift.
        let (term, rects) = paint(SplitControl {
            state: SplitState { response_minimized: true, ..Default::default() },
            hovered: Some(SplitStop::EditorFull),
        });
        let chip = rects[0].0;
        assert_eq!(cell(&term, chip.x, 0).bg, theme.accent);
        assert_eq!(cell(&term, chip.x, 0).fg, theme.selection);
    }
}
