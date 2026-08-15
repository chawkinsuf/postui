use crate::action::Action;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

pub enum Modal {
    Message { title: String, body: String },
    /// A choice prompt: each entry in `choices` is `(key, label, actions)` —
    /// pressing `key` (case-insensitive) dispatches `actions` and closes the
    /// modal; `Esc` closes with no actions.
    Confirm { title: String, body: String, choices: Vec<(char, String, Vec<Action>)> },
    Palette(crate::components::palette::PaletteState),
}

/// The outcome of a modal handling a key event: any actions the caller
/// should dispatch, and whether the modal should be popped off the stack.
/// The stack never pops itself — the caller pops on `close`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModalResult {
    pub actions: Vec<Action>,
    pub close: bool,
}

#[derive(Default)]
pub struct ModalStack {
    stack: Vec<Modal>,
}

impl ModalStack {
    pub fn push(&mut self, modal: Modal) {
        self.stack.push(modal);
    }

    pub fn pop(&mut self) -> Option<Modal> {
        self.stack.pop()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn top(&self) -> Option<&Modal> {
        self.stack.last()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ModalResult> {
        let top = self.stack.last_mut()?;
        match top {
            Modal::Message { .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    Some(ModalResult { actions: vec![], close: true })
                }
                _ => None, // swallowed: modals capture all input
            },
            Modal::Confirm { choices, .. } => match key.code {
                KeyCode::Esc => Some(ModalResult { actions: vec![], close: true }),
                KeyCode::Char(c) => {
                    let c = c.to_ascii_lowercase();
                    choices
                        .iter()
                        .find(|(choice, _, _)| choice.to_ascii_lowercase() == c)
                        .map(|(_, _, actions)| ModalResult { actions: actions.clone(), close: true })
                }
                _ => None, // swallowed: modals capture all input
            },
            Modal::Palette(state) => state.handle_key(key),
        }
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let Some(top) = self.stack.last() else { return };
        dim_backdrop(frame, screen);
        match top {
            Modal::Message { title, body } => {
                let area = centered_rect(screen, 60.min(screen.width), 9);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.border_focused))
                    .padding(Padding::uniform(1))
                    .style(Style::default().bg(theme.surface_raised))
                    .title(format!(" {title} "))
                    .title_style(Style::default().fg(theme.accent));
                frame.render_widget(Clear, area);
                frame.render_widget(
                    Paragraph::new(body.as_str())
                        .style(Style::default().fg(theme.text))
                        .wrap(Wrap { trim: false })
                        .block(block),
                    area,
                );
            }
            Modal::Confirm { title, body, choices } => {
                let area = centered_rect(screen, 60.min(screen.width), 9);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.border_focused))
                    .padding(Padding::uniform(1))
                    .style(Style::default().bg(theme.surface_raised))
                    .title(format!(" {title} "))
                    .title_style(Style::default().fg(theme.accent));
                let mut hint =
                    choices.iter().map(|(c, label, _)| format!("[{c}] {label}")).collect::<Vec<_>>();
                hint.push("[esc] Cancel".to_string());
                let text = format!("{body}\n\n{}", hint.join("   "));
                frame.render_widget(Clear, area);
                frame.render_widget(
                    Paragraph::new(text)
                        .style(Style::default().fg(theme.text))
                        .wrap(Wrap { trim: false })
                        .block(block),
                    area,
                );
            }
            Modal::Palette(state) => state.draw(frame, screen, theme),
        }
    }
}

pub fn centered_rect(screen: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(screen.width);
    let h = height.min(screen.height);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}

pub fn dim_backdrop(frame: &mut Frame, screen: Rect) {
    frame
        .buffer_mut()
        .set_style(screen, Style::default().add_modifier(Modifier::DIM));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn centered_rect_is_centered_and_clamped() {
        let screen = Rect::new(0, 0, 100, 40);
        let r = centered_rect(screen, 60, 10);
        assert_eq!(r, Rect::new(20, 15, 60, 10));
        let clamped = centered_rect(screen, 200, 90);
        assert_eq!(clamped.width, 100);
        assert_eq!(clamped.height, 40);
    }

    #[test]
    fn esc_closes_top_modal_only() {
        let mut m = ModalStack::default();
        m.push(Modal::Message { title: "A".into(), body: "a".into() });
        m.push(Modal::Message { title: "B".into(), body: "b".into() });
        let res = m.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close);
        assert!(res.actions.is_empty());
        // the stack does not pop itself: the caller pops on close.
        assert_eq!(m.stack.len(), 2);
    }

    #[test]
    fn other_keys_are_swallowed_by_message_modal() {
        let mut m = ModalStack::default();
        m.push(Modal::Message { title: "A".into(), body: "a".into() });
        assert!(m.handle_key(key(KeyCode::Char('q'))).is_none(),
            "keys must not leak through a modal to global bindings");
    }

    #[test]
    fn palette_enter_returns_action_and_closes() {
        let mut m = ModalStack::default();
        m.push(Modal::Palette(crate::components::palette::PaletteState::new()));
        for c in "quit".chars() {
            assert!(m.handle_key(key(KeyCode::Char(c))).is_none());
        }
        let res = m.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::Quit]);
        // note: the STACK does not pop itself — the caller pops on close.
        assert!(!m.is_empty());
    }

    #[test]
    fn message_modal_closes_without_action() {
        let mut m = ModalStack::default();
        m.push(Modal::Message { title: "t".into(), body: "b".into() });
        let res = m.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn top_returns_the_top_modal() {
        let mut m = ModalStack::default();
        assert!(m.top().is_none());
        m.push(Modal::Message { title: "t".into(), body: "b".into() });
        assert!(matches!(m.top(), Some(Modal::Message { .. })));
    }

    #[test]
    fn draw_renders_title_and_body() {
        let mut m = ModalStack::default();
        m.push(Modal::Message { title: "About".into(), body: "hello world".into() });
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| m.draw(f, f.area(), &theme)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("About"));
        assert!(content.contains("hello world"));
    }
}
