use super::{pane_block, Component, DrawCtx};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

#[derive(Default)]
pub struct Response;

impl Component for Response {
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Response", ctx);
        let empty = Paragraph::new(vec![
            Line::raw(""),
            Line::raw("Send a request — the response will appear here."),
        ])
        .style(Style::default().fg(ctx.theme.text_muted))
        .centered()
        .block(block);
        frame.render_widget(empty, area);
    }
}
