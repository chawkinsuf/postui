use crate::action::Action;
use crate::components::{editor::Editor, response::Response, sidebar::Sidebar};
use crate::layout::PaneId;
use crate::theme::Theme;

pub struct App {
    pub should_quit: bool,
    pub focus: PaneId,
    pub theme: Theme,
    pub sidebar: Sidebar,
    pub editor: Editor,
    pub response: Response,
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            focus: PaneId::Sidebar,
            theme: Theme::for_terminal(),
            sidebar: Sidebar,
            editor: Editor,
            response: Response,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Tick | Action::Render => {}
            Action::FocusNext => self.focus = self.focus.next(),
            Action::FocusPrev => self.focus = self.focus.prev(),
            Action::OpenPalette | Action::Close => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_action_sets_should_quit() {
        let mut app = App::new();
        assert!(!app.should_quit);
        app.update(Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn tick_does_not_quit() {
        let mut app = App::new();
        app.update(Action::Tick);
        assert!(!app.should_quit);
    }

    #[test]
    fn focus_next_moves_focus() {
        let mut app = App::new();
        let start = app.focus;
        app.update(Action::FocusNext);
        assert_ne!(app.focus, start);
        app.update(Action::FocusPrev);
        assert_eq!(app.focus, start);
    }
}
