pub mod chooser;
pub mod editor;
pub mod footer;
pub mod header_bar;
pub mod json_tree;
pub mod line_input;
pub mod modal;
pub mod palette;
pub mod response;
pub mod sidebar;
pub mod table_editor;
pub mod toast;
pub mod var_picker;
pub mod var_tokens;
pub mod varmanager;

use crate::action::Action;
use crate::paint::fill;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
    pub hovered: Option<&'a crate::hit::Hit>,
    /// True while this pane's scrollbar thumb is being dragged, so the thumb
    /// keeps its active styling even when the pointer leaves the column.
    pub dragging: bool,
}

pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        None
    }
    fn handle_scroll(&mut self, _delta: i16) {}
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &DrawCtx, hits: &mut crate::hit::HitMap);
}

/// Paints a pane's shared, borderless surface: a flat `theme.page` fill
/// across the whole pane rect, returning the 1-column-each-side horizontally
/// inset rect its content draws into. Panes carry no border or title of
/// their own — the address bar (Editor) and response strip (Response)
/// identify which pane is which, and a painted [`crate::paint::fill`] gutter
/// column (drawn by `ui::draw`) separates the sidebar from the main panes
/// instead of a `│` glyph.
pub fn pane_surface(buf: &mut Buffer, area: Rect, theme: &Theme) -> Rect {
    fill(buf, area, theme.page);
    Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn pane_surface_fills_page_and_insets_one_column_each_side() {
        let theme = Theme::dark();
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut inner = Rect::default();
        terminal
            .draw(|f| {
                let area = f.area();
                inner = pane_surface(f.buffer_mut(), area, &theme);
            })
            .unwrap();
        assert_eq!(inner, Rect::new(1, 0, 18, 5));
        let buf = terminal.backend().buffer();
        for y in 0..5u16 {
            for x in 0..20u16 {
                assert_eq!(buf[(x, y)].bg, theme.page);
            }
        }
        let content = format!("{buf:?}");
        for glyph in ['╭', '╮', '╰', '╯', '│'] {
            assert!(
                !content.contains(glyph),
                "no pane border glyph {glyph:?} expected: {content}"
            );
        }
    }
}
