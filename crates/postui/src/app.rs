use crate::action::Action;
use crate::components::modal::ModalStack;
use crate::components::toast::Toasts;
use crate::components::{editor::Editor, response::Response, sidebar::Sidebar, Component};
use crate::keys::{KeyCombo, Keymap};
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

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

    /// Constructs an `App` for tests. Task 13 later extends this with a
    /// channel + temp project root; keep it compiling forward from here.
    pub fn new_for_test() -> Self {
        Self::new()
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

    /// Central key router. Order (each step tested):
    /// 1. A CTRL/ALT combo the keymap maps to Quit pre-empts everything,
    ///    including open modals — ctrl+c must always quit.
    /// 2. An open modal stack captures all remaining input (swallowed keys
    ///    still count as "handled" — they return true).
    /// 3. With no modal open, a CTRL/ALT combo prefers the global keymap
    ///    over the focused component (app shortcuts beat editors), falling
    ///    through to the component if unbound.
    /// 4. Plain keys (and unbound modified ones) go to the focused
    ///    component first.
    /// 5. Anything the component ignores falls back to the global keymap.
    ///
    /// Returns whether an action was applied or a modal consumed the key
    /// (i.e. whether the caller should redraw). NOTE: `App::update` still
    /// returns `()` in this task; `self.apply(a)` from the brief becomes
    /// real in Task 8 when `update` itself returns `bool`.
    pub fn handle_key(&mut self, keymap: &Keymap, ev: KeyEvent) -> bool {
        let combo = KeyCombo::from_event(&ev);
        let global = keymap.lookup(&combo);
        let modified = ev.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

        // 1. A modified quit combo is the escape hatch: it pre-empts everything.
        if modified && global == Some(Action::Quit) {
            self.update(Action::Quit);
            return true;
        }

        // 2. Modals capture all remaining input.
        if !self.modals.is_empty() {
            let Some(res) = self.modals.handle_key(ev) else {
                return true; // typed into modal
            };
            if res.close {
                self.modals.pop();
            }
            for a in res.actions {
                self.update(a);
            }
            return true;
        }

        // 3. Modified combos prefer the global keymap (app shortcuts beat editors).
        if modified && let Some(a) = global {
            self.update(a);
            return true;
        }

        // 4. The focused component gets plain keys (and unbound modified ones) next.
        if let Some(a) = self.focused_component_key(ev) {
            self.update(a);
            return true;
        }

        // 5. Global fallback for plain keys the component ignored.
        if let Some(a) = global {
            self.update(a);
            return true;
        }

        false
    }

    fn focused_component_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match self.focus {
            PaneId::Sidebar => self.sidebar.handle_key(ev),
            PaneId::Editor => self.editor.handle_key(ev),
            PaneId::Response => self.response.handle_key(ev),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyCode;

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

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn ctrl_c_quits_even_with_modal_open() {
        let mut app = App::new_for_test();
        app.update(Action::OpenPalette);
        app.handle_key(&Keymap::default_bindings(), ctrl('c'));
        assert!(app.should_quit);
    }

    #[test]
    fn plain_q_types_into_palette_instead_of_quitting() {
        let mut app = App::new_for_test();
        app.update(Action::OpenPalette);
        app.handle_key(&Keymap::default_bindings(), plain('q'));
        assert!(!app.should_quit);
        assert!(!app.modals.is_empty());
    }

    #[test]
    fn ctrl_char_does_not_type_into_palette() {
        let mut app = App::new_for_test();
        app.update(Action::OpenPalette);
        app.handle_key(&Keymap::default_bindings(), ctrl('x')); // unbound ctrl combo
        // palette input must still be empty: filter list unchanged
        let crate::components::modal::Modal::Palette(p) = app.modals.top().unwrap() else {
            panic!()
        };
        assert_eq!(p.input(), "");
    }

    #[test]
    fn plain_q_quits_when_no_modal_and_component_ignores_it() {
        let mut app = App::new_for_test();
        app.handle_key(&Keymap::default_bindings(), plain('q'));
        assert!(app.should_quit);
    }
}
