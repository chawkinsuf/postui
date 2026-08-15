use super::{pane_block, Component, DrawCtx};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

#[derive(Default)]
pub struct Sidebar;

impl Component for Sidebar {
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Requests", ctx);
        let empty = Paragraph::new(vec![
            Line::raw(""),
            Line::raw("No project open."),
            Line::raw(""),
            Line::raw("Projects and requests"),
            Line::raw("will appear here."),
        ])
        .style(Style::default().fg(ctx.theme.text_muted))
        .centered()
        .block(block);
        frame.render_widget(empty, area);
    }
}
