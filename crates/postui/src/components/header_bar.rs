use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub fn draw_header(frame: &mut Frame, area: Rect, theme: &Theme) {
    let line = Line::from(vec![
        Span::styled(
            " postui ",
            Style::default().fg(theme.surface).bg(theme.accent).bold(),
        ),
        Span::raw("  "),
        Span::styled("env: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            "No environment",
            Style::default().fg(theme.text_muted).italic(),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.surface_raised)),
        area,
    );
}
