//! The pane-split button cluster: a one-row segmented control of three
//! 3-cell buttons — minimize / half / expand — shared by the Editor and
//! Response panes' headers. Each segment is a solid painted fill like a
//! real GUI's window controls: neutral at rest, lifting on hover, with
//! the segment describing the pane's *current* share lit in accent so
//! the cluster doubles as a state indicator.
//!
//! The glyphs are a mini-map of the pane's share of the column: the
//! Response pane's fill grows bottom-up (`▁ ▄ █`), the Editor pane's
//! top-down (`▔ ▀ █`).

use crate::split::{SplitButton, SplitPane, SplitState};
use crate::theme::Theme;
use ratatui::{buffer::Buffer, layout::Rect};

/// Each segment is 3 cells: a padding cell, the glyph, a padding cell.
pub const SPLIT_SEGMENT_WIDTH: u16 = 3;
/// The whole cluster: three contiguous segments.
pub const SPLIT_CLUSTER_WIDTH: u16 = SPLIT_SEGMENT_WIDTH * 3;

/// The segments in on-screen order, left to right — the pane's share
/// grows along the cluster.
pub const SPLIT_BUTTONS: [SplitButton; 3] = [
    SplitButton::Minimize,
    SplitButton::Half,
    SplitButton::Expand,
];

/// The mini-map glyph for `pane`'s `button` segment.
pub fn split_glyph(pane: SplitPane, button: SplitButton) -> &'static str {
    match (pane, button) {
        (_, SplitButton::Expand) => "\u{2588}", // █
        (SplitPane::Response, SplitButton::Minimize) => "\u{2581}", // ▁
        (SplitPane::Response, SplitButton::Half) => "\u{2584}", // ▄
        (SplitPane::Editor, SplitButton::Minimize) => "\u{2594}", // ▔
        (SplitPane::Editor, SplitButton::Half) => "\u{2580}", // ▀
    }
}

/// A pane's split cluster, ready to paint: whose cluster it is, the
/// current split (for the lit segment), and which segment the pointer is
/// over, if any (resolved from the hit map by the caller).
pub struct SplitCluster {
    pub pane: SplitPane,
    pub state: SplitState,
    pub hovered: Option<SplitButton>,
}

impl SplitCluster {
    /// Paints the cluster with its left edge at `(x, y)` and returns each
    /// segment's rect with the button it triggers, for hit registration.
    pub fn paint(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        theme: &Theme,
    ) -> [(Rect, SplitButton); 3] {
        let active = self.state.active_button(self.pane);
        SPLIT_BUTTONS.map(|button| {
            let i = SPLIT_BUTTONS.iter().position(|b| *b == button).unwrap() as u16;
            let rect = Rect::new(x + i * SPLIT_SEGMENT_WIDTH, y, SPLIT_SEGMENT_WIDTH, 1);
            let hovered = self.hovered == Some(button);
            // The same face language `Button` uses, one row tall: lit
            // segments live on the accent (lifting on hover), neutral
            // ones on the control fill.
            let (fill, fg) = match (active == Some(button), hovered) {
                (true, false) => (theme.accent, theme.on_accent),
                (true, true) => (theme.accent_edge_light, theme.on_accent),
                (false, true) => (theme.control_hover, theme.text),
                (false, false) => (theme.control, theme.text_muted),
            };
            crate::paint::fill(buf, rect, fill);
            crate::paint::text(
                buf,
                rect.x + 1,
                y,
                split_glyph(self.pane, button),
                fg,
                fill,
                false,
            );
            (rect, button)
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

    fn paint(cluster: SplitCluster) -> (Terminal<TestBackend>, [(Rect, SplitButton); 3]) {
        let theme = Theme::dark();
        let mut term = Terminal::new(TestBackend::new(20, 1)).unwrap();
        let mut rects = None;
        term.draw(|f| {
            rects = Some(cluster.paint(f.buffer_mut(), 2, 0, &theme));
        })
        .unwrap();
        (term, rects.unwrap())
    }

    #[test]
    fn segments_are_contiguous_three_cell_buttons_in_share_order() {
        let (_, rects) = paint(SplitCluster {
            pane: SplitPane::Response,
            state: SplitState::default(),
            hovered: None,
        });
        assert_eq!(
            rects.map(|(_, b)| b),
            [
                SplitButton::Minimize,
                SplitButton::Half,
                SplitButton::Expand
            ]
        );
        for (i, (rect, _)) in rects.iter().enumerate() {
            assert_eq!(rect.width, SPLIT_SEGMENT_WIDTH);
            assert_eq!(rect.height, 1);
            assert_eq!(rect.x, 2 + i as u16 * SPLIT_SEGMENT_WIDTH);
        }
    }

    #[test]
    fn resting_segments_sit_on_the_control_fill_with_muted_glyphs() {
        let theme = Theme::dark();
        // Response at 75%: no segment of its own cluster is lit.
        let (term, rects) = paint(SplitCluster {
            pane: SplitPane::Response,
            state: SplitState {
                ratio: crate::split::SplitRatio::ResponseBig,
                ..Default::default()
            },
            hovered: None,
        });
        for (rect, button) in rects {
            let glyph_cell = cell(&term, rect.x + 1, 0);
            assert_eq!(
                glyph_cell.symbol(),
                split_glyph(SplitPane::Response, button)
            );
            assert_eq!(glyph_cell.bg, theme.control);
            assert_eq!(glyph_cell.fg, theme.text_muted);
            // The padding cells carry the fill too — the segment reads as
            // one solid button, not a floating glyph.
            assert_eq!(cell(&term, rect.x, 0).bg, theme.control);
            assert_eq!(cell(&term, rect.x + 2, 0).bg, theme.control);
        }
    }

    #[test]
    fn the_current_share_segment_is_lit_in_accent() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitCluster {
            pane: SplitPane::Response,
            state: SplitState::default(), // 50/50: half is lit
            hovered: None,
        });
        let half = rects[1].0;
        assert_eq!(cell(&term, half.x + 1, 0).bg, theme.accent);
        assert_eq!(cell(&term, half.x + 1, 0).fg, theme.on_accent);
        assert_eq!(
            cell(&term, rects[0].0.x + 1, 0).bg,
            theme.control,
            "the other segments stay neutral"
        );
    }

    #[test]
    fn hover_lifts_a_neutral_segment_and_the_lit_one() {
        let theme = Theme::dark();
        let (term, rects) = paint(SplitCluster {
            pane: SplitPane::Editor,
            state: SplitState::default(),
            hovered: Some(SplitButton::Expand),
        });
        let expand = rects[2].0;
        assert_eq!(cell(&term, expand.x + 1, 0).bg, theme.control_hover);
        assert_eq!(cell(&term, expand.x + 1, 0).fg, theme.text);

        // Hovering the active segment lifts its accent, like Button does.
        let (term, rects) = paint(SplitCluster {
            pane: SplitPane::Editor,
            state: SplitState::default(),
            hovered: Some(SplitButton::Half),
        });
        let half = rects[1].0;
        assert_eq!(cell(&term, half.x + 1, 0).bg, theme.accent_edge_light);
        assert_eq!(cell(&term, half.x + 1, 0).fg, theme.on_accent);
    }

    #[test]
    fn editor_and_response_clusters_mirror_their_glyph_direction() {
        assert_eq!(split_glyph(SplitPane::Response, SplitButton::Minimize), "▁");
        assert_eq!(split_glyph(SplitPane::Response, SplitButton::Half), "▄");
        assert_eq!(split_glyph(SplitPane::Editor, SplitButton::Minimize), "▔");
        assert_eq!(split_glyph(SplitPane::Editor, SplitButton::Half), "▀");
        assert_eq!(split_glyph(SplitPane::Editor, SplitButton::Expand), "█");
        assert_eq!(split_glyph(SplitPane::Response, SplitButton::Expand), "█");
    }
}
