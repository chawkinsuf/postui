use crate::action::Action;
use crate::hit::{self, Hit, HitMap};
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

/// The context-sensitive chips for the focused pane, plus the global chips
/// always appended at the end. `None` actions render as plain (unregistered,
/// muted) text — they describe a binding with no single dispatchable
/// `Action` (e.g. multi-key hints).
fn footer_chips(focus: PaneId) -> Vec<(String, Option<Action>)> {
    let mut chips: Vec<(String, Option<Action>)> = match focus {
        PaneId::Sidebar => vec![
            ("enter open".into(), None),
            ("n new".into(), Some(Action::PromptNewRequest)),
            ("r rename".into(), Some(Action::PromptRenameRequest)),
            ("d delete".into(), Some(Action::ConfirmDeleteRequest)),
        ],
        PaneId::Editor => vec![
            ("ctrl+r send".into(), Some(Action::Send)),
            ("ctrl+s save".into(), Some(Action::SaveRequest)),
            ("alt+1/2/3 tabs".into(), None),
        ],
        PaneId::Response => vec![
            ("r raw".into(), None),
            ("h headers".into(), None),
            ("/ search".into(), None),
        ],
    };
    chips.push(("^P commands".into(), Some(Action::OpenPalette)));
    chips.push(("q quit".into(), Some(Action::Quit)));
    chips
}

pub fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    focus: PaneId,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(theme.surface_raised)),
        area,
    );
    let mut x = area.x;
    for (label, action) in footer_chips(focus) {
        let text = format!(" {label} ");
        let width = text.chars().count() as u16;
        if x + width > area.x + area.width {
            break;
        }
        let chip_area = Rect {
            x,
            y: area.y,
            width,
            height: 1,
        };
        match action {
            Some(action) => {
                hit::chip(
                    frame,
                    hits,
                    chip_area,
                    &text,
                    Hit::FooterChip(action),
                    hovered,
                    theme,
                );
            }
            None => {
                frame.render_widget(
                    Paragraph::new(text).style(Style::default().fg(theme.text_muted)),
                    chip_area,
                );
            }
        }
        x += width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(focus: PaneId) -> String {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, focus, &mut hits, None))
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

    #[test]
    fn action_chips_are_registered_as_hits() {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(120, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_footer(f, f.area(), &theme, PaneId::Sidebar, &mut hits, None))
            .unwrap();
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::PromptNewRequest))
                .is_some()
        );
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::OpenPalette))
                .is_some()
        );
        assert!(hits.rect_of(&Hit::FooterChip(Action::Quit)).is_some());
    }
}
