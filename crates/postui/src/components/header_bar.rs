use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub fn draw_header(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    project: &str,
    env: &str,
    _hits: &mut crate::hit::HitMap,
) {
    let env_style = if env == "no env" {
        Style::default().fg(theme.text_muted).italic()
    } else {
        Style::default().fg(theme.text_muted)
    };
    let line = Line::from(vec![
        Span::styled(
            " postui ",
            Style::default().fg(theme.surface).bg(theme.accent).bold(),
        ),
        Span::raw("  "),
        Span::styled(project.to_string(), Style::default().fg(theme.text)),
        Span::styled(" · ", Style::default().fg(theme.text_muted)),
        Span::styled(env.to_string(), env_style),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.surface_raised)),
        area,
    );
}
