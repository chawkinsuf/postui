use super::{pane_block, Component, DrawCtx};
use crate::action::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Instant;

/// The response pane's lifecycle: nothing sent yet, a request in flight (and
/// since when — used to animate a spinner), a completed response, a failed
/// send, or a send the user cancelled. Real rendering of `Ready`/`Failed`
/// bodies arrives in Task 16; this task only wires the states through.
#[derive(Default)]
pub enum ResponseState {
    #[default]
    Empty,
    InFlight { started: Instant },
    Ready(Box<crate::http::ResponseData>),
    Failed(String),
    Cancelled,
}

#[derive(Default)]
pub struct Response {
    pub state: ResponseState,
}

impl Component for Response {
    fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        if ev.code == KeyCode::Esc && matches!(self.state, ResponseState::InFlight { .. }) {
            return Some(Action::CancelSend);
        }
        None
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Response", ctx);
        let lines = match &self.state {
            ResponseState::Empty => vec![
                Line::raw(""),
                Line::raw("Send a request — the response will appear here."),
            ],
            ResponseState::InFlight { .. } => vec![Line::raw(""), Line::raw("sending…")],
            ResponseState::Ready(data) => {
                vec![Line::raw(""), Line::raw(format!("{} — {} bytes", data.status, data.size))]
            }
            ResponseState::Failed(err) => vec![Line::raw(""), Line::raw(format!("Failed: {err}"))],
            ResponseState::Cancelled => vec![Line::raw(""), Line::raw("Cancelled")],
        };
        let widget = Paragraph::new(lines)
            .style(Style::default().fg(ctx.theme.text_muted))
            .centered()
            .block(block);
        frame.render_widget(widget, area);
    }
}
