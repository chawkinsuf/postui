use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Hints for the focused pane's own bindings, shown before the global part.
fn contextual_hint(focus: PaneId) -> &'static str {
    match focus {
        PaneId::Sidebar => "enter open · n new · r rename · d delete",
        PaneId::Editor => "ctrl+r send · ctrl+s save · alt+1/2/3 tabs",
        PaneId::Response => "r raw · h headers · / search",
    }
}

pub fn draw_footer(frame: &mut Frame, area: Rect, theme: &Theme, focus: PaneId) {
    let hint = |key: &'static str, desc: &'static str| {
        vec![
            Span::styled(format!(" {key} "), Style::default().fg(theme.accent)),
            Span::styled(desc, Style::default().fg(theme.text_muted)),
            Span::raw(" "),
        ]
    };
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!(" {} ", contextual_hint(focus)),
        Style::default().fg(theme.text_muted),
    ));
    spans.extend(hint("^P", "commands"));
    spans.extend(hint("q", "quit"));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface_raised)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(focus: PaneId) -> String {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, focus))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn sidebar_focus_shows_sidebar_hints() {
        let content = render(PaneId::Sidebar);
        assert!(content.contains("enter open"));
        assert!(content.contains("commands"));
        assert!(content.contains("quit"));
    }

    #[test]
    fn editor_focus_shows_editor_hints() {
        let content = render(PaneId::Editor);
        assert!(content.contains("ctrl+r send"));
    }

    #[test]
    fn response_focus_shows_response_hints() {
        let content = render(PaneId::Response);
        assert!(content.contains("r raw"));
        assert!(content.contains("/ search"));
    }
}
