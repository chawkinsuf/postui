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

use crate::action::Action;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Padding};

pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
}

pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        None
    }
    fn handle_scroll(&mut self, _delta: i16) {}
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &DrawCtx);
}

/// Standard pane chrome: rounded borders, interior padding, focus styling.
pub fn pane_block<'a>(title: &'a str, ctx: &DrawCtx) -> Block<'a> {
    let t = ctx.theme;
    let (border_color, title_color) = if ctx.focused {
        (t.border_focused, t.accent)
    } else {
        (t.border, t.text_muted)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1))
        .title(format!(" {title} "))
        .title_style(Style::default().fg(title_color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn pane_block_renders_rounded_border_and_title() {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
        };
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let b = pane_block("Requests", &ctx);
                f.render_widget(b, f.area());
            })
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains('╭'), "rounded corner expected");
        assert!(content.contains("Requests"));
    }
}
