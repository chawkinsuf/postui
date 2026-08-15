use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn draw_footer(frame: &mut Frame, area: Rect, theme: &Theme) {
    let hint = |key: &'static str, desc: &'static str| {
        vec![
            Span::styled(format!(" {key} "), Style::default().fg(theme.accent)),
            Span::styled(desc, Style::default().fg(theme.text_muted)),
            Span::raw(" "),
        ]
    };
    let mut spans = Vec::new();
    spans.extend(hint("Tab", "next pane"));
    spans.extend(hint("^P", "palette"));
    spans.extend(hint("q", "quit"));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface_raised)),
        area,
    );
}
