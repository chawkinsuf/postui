use super::{pane_block, Component, DrawCtx};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

#[derive(Default)]
pub struct Editor;

impl Component for Editor {
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Request", ctx);
        let empty = Paragraph::new(vec![
            Line::raw(""),
            Line::raw("Select or create a request to edit it."),
        ])
        .style(Style::default().fg(ctx.theme.text_muted))
        .centered()
        .block(block);
        frame.render_widget(empty, area);
    }
}
