use crate::action::Action;
use crate::components::modal::ModalStack;
use crate::components::toast::Toasts;
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
    pub toasts: Toasts,
    pub modals: ModalStack,
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
            toasts: Toasts::default(),
            modals: ModalStack::default(),
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
            Action::Tick => self.toasts.on_tick(),
            Action::Render => {}
            Action::FocusNext => self.focus = self.focus.next(),
            Action::FocusPrev => self.focus = self.focus.prev(),
            Action::FocusPane(pane) => self.focus = pane,
            Action::OpenPalette => {
                use crate::components::modal::Modal;
                use crate::components::palette::PaletteState;
                self.modals.push(Modal::Palette(PaletteState::new()));
            }
            Action::Close => {
                let _ = self.modals.pop(); // no-op when empty
            }
            Action::ShowToast(msg, kind) => self.toasts.push(msg, kind),
            Action::ShowAbout => {
                use crate::components::modal::Modal;
                self.modals.push(Modal::Message {
                    title: "postui".into(),
                    body: "A fast, local-first terminal HTTP client.".into(),
                });
            }
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

    #[test]
    fn close_pops_modal_instead_of_quitting() {
        use crate::components::modal::Modal;
        let mut app = App::new();
        app.modals.push(Modal::Message { title: "t".into(), body: "b".into() });
        app.update(Action::Close);
        assert!(app.modals.is_empty());
        assert!(!app.should_quit);
    }

    #[test]
    fn open_palette_pushes_modal() {
        let mut app = App::new();
        app.update(Action::OpenPalette);
        assert!(!app.modals.is_empty());
    }
}
