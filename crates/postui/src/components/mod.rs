use crate::action::Action;
use crate::theme::Theme;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Padding};
use ratatui::Frame;

#[allow(dead_code)]
pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
}

#[allow(dead_code)]
pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        None
    }
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx);
}

/// Standard pane chrome: rounded borders, interior padding, focus styling.
#[allow(dead_code)]
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn pane_block_renders_rounded_border_and_title() {
        let theme = Theme::dark();
        let ctx = DrawCtx { theme: &theme, focused: true };
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
