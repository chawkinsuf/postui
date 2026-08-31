//! The split control: one row of five 3-cell chips, right-aligned in the
//! Editor pane's tab-bar row — fixed chrome at the top of the column, so
//! the control never moves out from under the pointer the way the old
//! per-pane clusters on the (repositioning) response header did. Each
//! chip jumps the column straight to one of its five settled states.
//!
//! Each chip's glyph is a two-tone mini-picture of its state: the
//! editor's share in one tone above the response's in another, drawn
//! with a lower-block glyph whose fg paints the response mass over an
//! editor-tone bg. The chips sit flush against each other — two glyph
//! cells each, no padding — so the row fuses into one continuous
//! staircase (`▁▁▂▂▄▄▆▆██`) reading left to right as the boundary
//! sliding down; the current state's picture lights up accent.

use crate::split::{SplitState, SplitStop};
use crate::theme::Theme;
use ratatui::{buffer::Buffer, layout::Rect};

/// Each chip is 2 cells: its glyph doubled, no padding — the chips fuse
/// into one continuous mini-picture of the boundary sliding down.
pub const SPLIT_SEGMENT_WIDTH: u16 = 2;
/// The whole control: five contiguous chips plus a one-cell control-fill
/// cap at each end, so the staircase reads as one pill-shaped control
/// face rather than a loose graphic.
pub const SPLIT_CONTROL_WIDTH: u16 = SPLIT_SEGMENT_WIDTH * 5 + 2;

/// `stop`'s mini-picture: the response mass painted bottom-up in fg over
/// the editor-tone bg. The ramp is the canonical `▁▂▄▆█` so the tall
/// steps stay even (a `▇` endpoint reserving a strip for the editor made
/// the last step visibly shallower than its neighbors).
pub fn split_glyph(stop: SplitStop) -> &'static str {
    match stop {
        SplitStop::EditorFull => "\u{2581}\u{2581}", // ▁▁ response strip
        SplitStop::EditorBig => "\u{2582}\u{2582}",  // ▂▂ 75/25
        SplitStop::Even => "\u{2584}\u{2584}",       // ▄▄ 50/50
        SplitStop::ResponseBig => "\u{2586}\u{2586}", // ▆▆ 25/75
        SplitStop::ResponseFull => "\u{2588}\u{2588}", // ██ response takes all
    }
}

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
    pub fn paint(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        theme: &Theme,
    ) -> [(Rect, SplitStop); 5] {
        let active = self.state.stop();
        // The end caps: control fill, so the staircase sits on a visible
        // button face and the whole strip reads as one control.
        crate::paint::fill(buf, Rect::new(x, y, 1, 1), theme.control);
        crate::paint::fill(
            buf,
            Rect::new(x + SPLIT_CONTROL_WIDTH - 1, y, 1, 1),
            theme.control,
        );
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
        SplitStop::ALL.map(|stop| {
            let rect = Rect::new(x + 1 + i * SPLIT_SEGMENT_WIDTH, y, SPLIT_SEGMENT_WIDTH, 1);
            i += 1;
            let hovered = self.hovered == Some(stop);
            // The mini-picture fully covers its own cells, so hover and
            // "lit" are the picture's own tones changing. The tones follow
            // the app's glyph-on-button idiom: the editor mass is the
            // control fill itself (one dark pill face with the caps), the
            // response mass a glyph-grey drawn on it — hover lifts both a
            // step like any button, and the active chip swaps into the
            // accent pair (accent mass on the dim selection tint) instead
            // of a solid glowing block.
            // Resting response mass: halfway between disabled and muted
            // text — `text_disabled` alone vanishes against the control
            // fill, full `text_muted` reads too hot as a solid 2-cell
            // block (it's a mass, not a glyph stroke).
            let resting_mass =
                crate::theme::mix(theme.text_disabled, theme.text_muted, 0.5);
            let (editor_tone, response_tone) = if active == stop {
                (theme.selection, theme.accent)
            } else if hovered {
                (theme.control_hover, theme.text_muted)
            } else {
                (theme.control, resting_mass)
            };
            let glyph = split_glyph(stop);
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

    fn paint(control: SplitControl) -> (Terminal<TestBackend>, [(Rect, SplitStop); 5]) {
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
    fn chips_are_contiguous_two_cell_buttons_in_stop_order() {
        let (_, rects) = paint(SplitControl {
            state: SplitState::default(),
            hovered: None,
        });
        assert_eq!(rects.map(|(_, s)| s), SplitStop::ALL);
        for (i, (rect, _)) in rects.iter().enumerate() {
            assert_eq!(rect.width, SPLIT_SEGMENT_WIDTH);
            assert_eq!(rect.height, 1);
            // Chips start one cell in: the control-fill cap comes first.
            assert_eq!(rect.x, 3 + i as u16 * SPLIT_SEGMENT_WIDTH);
        }
        // Both end caps carry the control fill, framing the staircase as
        // one button face.
        let theme = Theme::dark();
        let (term, _) = paint(SplitControl {
            state: SplitState::default(),
            hovered: None,
        });
        assert_eq!(cell(&term, 2, 0).bg, theme.control);
        assert_eq!(cell(&term, 2 + SPLIT_CONTROL_WIDTH - 1, 0).bg, theme.control);
    }

    #[test]
    fn resting_chips_paint_muted_two_tone_pictures_across_both_cells() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitControl {
            state: SplitState::default(),
            hovered: None,
        });
        for (rect, stop) in rects {
            if stop == SplitStop::Even {
                continue; // the lit chip, checked separately below
            }
            let glyph = split_glyph(stop);
            let mass = crate::theme::mix(theme.text_disabled, theme.text_muted, 0.5);
            let (want_fg, want_bg) = (mass, theme.control);
            // Both cells carry the doubled glyph — the chip is nothing
            // but its picture, fusing flush with its neighbors.
            let one_char = &glyph[..glyph.len() / 2];
            for dx in 0..SPLIT_SEGMENT_WIDTH {
                let c = cell(&term, rect.x + dx, 0);
                assert_eq!(c.symbol(), one_char, "{stop:?}");
                assert_eq!(c.fg, want_fg, "{stop:?}");
                assert_eq!(c.bg, want_bg, "{stop:?}");
            }
        }
    }

    #[test]
    fn the_current_stops_picture_glows_in_accent_tones() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitControl {
            state: SplitState::default(), // 50/50: the Even chip is lit
            hovered: None,
        });
        let even = rects[2].0;
        // ▄ over an editor-tone bg: response mass in accent over the dim
        // selection tint standing in for the editor's share.
        assert_eq!(cell(&term, even.x + 1, 0).bg, theme.selection);
        assert_eq!(cell(&term, even.x + 1, 0).fg, theme.accent);
        assert_eq!(
            cell(&term, rects[0].0.x + 1, 0).bg,
            theme.control,
            "the other chips' pictures stay quiet"
        );
    }

    #[test]
    fn the_minimized_endpoints_light_their_chip_too() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitControl {
            state: SplitState {
                editor_minimized: true,
                ..Default::default()
            },
            hovered: None,
        });
        // ResponseFull's █: the response mass in accent covering the
        // whole chip (its bg is the selection tint, fully hidden).
        let chip = rects[4].0;
        assert_eq!(cell(&term, chip.x + 1, 0).fg, theme.accent);
        assert_eq!(cell(&term, chip.x + 1, 0).bg, theme.selection);
    }

    #[test]
    fn hover_brightens_a_chips_tones_lit_or_not() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitControl {
            state: SplitState::default(),
            hovered: Some(SplitStop::EditorBig),
        });
        let chip = rects[1].0;
        assert_eq!(cell(&term, chip.x, 0).bg, theme.control_hover);
        assert_eq!(cell(&term, chip.x, 0).fg, theme.text_muted);

        // Hovering the lit chip changes nothing — its picture already
        // glows accent, and there is no face behind it to lift.
        let (term, rects) = paint(SplitControl {
            state: SplitState::default(),
            hovered: Some(SplitStop::Even),
        });
        let chip = rects[2].0;
        assert_eq!(cell(&term, chip.x, 0).bg, theme.selection);
        assert_eq!(cell(&term, chip.x, 0).fg, theme.accent);
    }

    #[test]
    fn the_boundary_slides_down_across_the_glyph_row() {
        // The pictures' response mass grows chip by chip with even tall
        // steps — flush, the row fuses into one staircase.
        let glyphs: Vec<_> = SplitStop::ALL.iter().map(|s| split_glyph(*s)).collect();
        assert_eq!(glyphs, ["▁▁", "▂▂", "▄▄", "▆▆", "██"]);
    }
}
