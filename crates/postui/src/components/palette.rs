use crate::action::Action;
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};

#[derive(Clone)]
pub struct Command {
    pub name: &'static str,
    pub action: Action,
}

pub fn all_commands() -> Vec<Command> {
    vec![
        Command {
            name: "Focus: request tree",
            action: Action::FocusPane(PaneId::Sidebar),
        },
        Command {
            name: "Focus: editor",
            action: Action::FocusPane(PaneId::Editor),
        },
        Command {
            name: "Focus: response",
            action: Action::FocusPane(PaneId::Response),
        },
        Command {
            name: "Help: about postui",
            action: Action::ShowAbout,
        },
        Command {
            name: "Send request",
            action: Action::Send,
        },
        Command {
            name: "Request: new",
            action: Action::PromptNewRequest,
        },
        Command {
            name: "Request: save",
            action: Action::SaveRequest,
        },
        Command {
            name: "Request: rename",
            action: Action::PromptRenameRequest,
        },
        Command {
            name: "Request: delete",
            action: Action::ConfirmDeleteRequest,
        },
        Command {
            name: "Method: cycle",
            action: Action::CycleMethod,
        },
        Command {
            name: "Body: format JSON",
            action: Action::FormatBody,
        },
        Command {
            name: "Body: minify JSON",
            action: Action::MinifyBody,
        },
        Command {
            name: "Body: open in $EDITOR",
            action: Action::OpenBodyInEditor,
        },
        Command {
            name: "Body: toggle {{var}} substitution",
            action: Action::ToggleBodyVars,
        },
        Command {
            name: "Project: choose…",
            action: Action::OpenProjectChooser,
        },
        Command {
            name: "Project: next",
            action: Action::CycleProject,
        },
        Command {
            name: "Project: open by path…",
            action: Action::PromptOpenProjectPath,
        },
        Command {
            name: "Project: new…",
            action: Action::PromptNewProject,
        },
        Command {
            name: "Environment: choose…",
            action: Action::OpenEnvChooser,
        },
        Command {
            name: "Environment: next",
            action: Action::CycleEnv,
        },
        Command {
            name: "Variables: insert…",
            action: Action::OpenVarPicker { completing: false },
        },
        Command {
            name: "Quit",
            action: Action::Quit,
        },
    ]
}

pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let needle = needle.to_lowercase();
    let haystack = haystack.to_lowercase();
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

pub struct PaletteState {
    input: String,
    selected: usize,
    filtered: Vec<Command>,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self::new()
    }
}

impl PaletteState {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            selected: 0,
            filtered: all_commands(),
        }
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn filtered(&self) -> &[Command] {
        &self.filtered
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    fn refilter(&mut self) {
        self.filtered = all_commands()
            .into_iter()
            .filter(|c| fuzzy_match(&self.input, c.name))
            .collect();
        self.selected = 0;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<super::modal::ModalResult> {
        match key.code {
            KeyCode::Esc => {
                return Some(super::modal::ModalResult {
                    actions: vec![],
                    close: true,
                });
            }
            KeyCode::Enter => {
                let chosen = self.filtered.get(self.selected)?.action.clone();
                return Some(super::modal::ModalResult {
                    actions: vec![chosen],
                    close: true,
                });
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refilter();
            }
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                self.input.push(c);
                self.refilter();
            }
            _ => {}
        }
        None
    }

    pub fn draw(
        &self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        _hits: &mut crate::hit::HitMap,
    ) {
        let width = 50.min(screen.width);
        let height = (self.filtered.len() as u16 + 4)
            .clamp(5, 16)
            .min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border_focused))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(theme.surface_raised))
            .title(" Commands ")
            .title_style(Style::default().fg(theme.accent));
        frame.render_widget(Clear, area);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let prompt = Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(self.input.as_str(), Style::default().fg(theme.text)),
            Span::styled("▏", Style::default().fg(theme.accent)),
        ]);
        let prompt_area = Rect { height: 1, ..inner };
        frame.render_widget(Paragraph::new(prompt), prompt_area);

        let list_area = Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        };
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let style = if i == self.selected {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text)
                };
                let marker = if i == self.selected { "› " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled(c.name, style),
                ]))
            })
            .collect();
        frame.render_widget(List::new(items), list_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn fuzzy_match_is_case_insensitive_subsequence() {
        assert!(fuzzy_match("fre", "Focus: request editor"));
        assert!(fuzzy_match("QUIT", "Quit"));
        assert!(fuzzy_match("", "anything"));
        assert!(!fuzzy_match("xyz", "Quit"));
    }

    #[test]
    fn typing_filters_and_backspace_restores() {
        let mut p = PaletteState::new();
        let total = p.filtered().len();
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.filtered().len(), 1);
        assert_eq!(p.filtered()[0].name, "Quit");
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.filtered().len(), total);
    }

    #[test]
    fn arrows_move_selection_within_bounds() {
        let mut p = PaletteState::new();
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Up)); // clamped at top
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Down));
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn enter_returns_selected_action_and_closes() {
        let mut p = PaletteState::new();
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::Quit]);
    }

    #[test]
    fn enter_on_empty_results_does_nothing() {
        let mut p = PaletteState::new();
        for c in "zzzz".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert!(p.filtered().is_empty());
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
    }

    #[test]
    fn esc_closes_without_action() {
        let mut p = PaletteState::new();
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close);
        assert!(res.actions.is_empty());
    }

    #[test]
    fn selection_resets_when_filter_changes() {
        let mut p = PaletteState::new();
        p.handle_key(key(KeyCode::Down));
        p.handle_key(key(KeyCode::Char('q')));
        assert_eq!(p.selected(), 0);
    }
}
