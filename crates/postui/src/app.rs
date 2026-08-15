use crate::action::Action;
use crate::components::editor::{Editor, EditorTab, SubFocus};
use crate::components::modal::ModalStack;
use crate::components::toast::Toasts;
use crate::components::{response::Response, sidebar::Sidebar, Component};
use crate::keys::{KeyCombo, Keymap};
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub struct App {
    pub should_quit: bool,
    pub focus: PaneId,
    pub theme: Theme,
    pub sidebar: Sidebar,
    pub editor: Editor,
    pub response: Response,
    pub toasts: Toasts,
    pub modals: ModalStack,
    /// Sender for background tasks (e.g. in-flight requests) to push
    /// `Action`s back into the main loop without blocking on it.
    pub tx: UnboundedSender<Action>,
    /// Keeps the test-only channel's receiver alive so `tx` doesn't become
    /// a dangling sender in `App::new_for_test()`. Always `None` outside
    /// of tests.
    _test_rx: Option<UnboundedReceiver<Action>>,
}

impl App {
    pub fn new(tx: UnboundedSender<Action>) -> Self {
        Self {
            should_quit: false,
            focus: PaneId::Sidebar,
            theme: Theme::for_terminal(),
            sidebar: Sidebar,
            editor: Editor::default(),
            response: Response,
            toasts: Toasts::default(),
            modals: ModalStack::default(),
            tx,
            _test_rx: None,
        }
    }

    /// Constructs an `App` for tests, with its own channel so `tx` is
    /// always a live sender. Task 13 later extends this with a temp
    /// project root; keep it compiling forward from here.
    pub fn new_for_test() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = Self::new(tx);
        app._test_rx = Some(rx);
        app
    }
}

impl App {
    /// Applies `action` to app state. Returns `true` if state changed in a
    /// way that requires a redraw, `false` if the caller can skip drawing
    /// this iteration.
    pub fn update(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => {
                self.should_quit = true;
                true
            }
            Action::Tick => self.toasts.on_tick() || self.in_flight_ticking(),
            // No state change; forces a redraw. Background tasks use this
            // to wake the main loop when they've mutated state directly
            // (rather than through an `Action`) and just need a repaint.
            Action::Render => true,
            Action::FocusNext => {
                self.focus = self.focus.next();
                true
            }
            Action::FocusPrev => {
                self.focus = self.focus.prev();
                true
            }
            Action::FocusPane(pane) => {
                self.focus = pane;
                true
            }
            Action::ScrollPane(pane, delta) => {
                match pane {
                    PaneId::Sidebar => self.sidebar.handle_scroll(delta),
                    PaneId::Editor => self.editor.handle_scroll(delta),
                    PaneId::Response => self.response.handle_scroll(delta),
                }
                true
            }
            Action::OpenPalette => {
                use crate::components::modal::Modal;
                use crate::components::palette::PaletteState;
                self.modals.push(Modal::Palette(PaletteState::new()));
                true
            }
            Action::Close => self.modals.pop().is_some(),
            Action::ShowToast(msg, kind) => {
                self.toasts.push(msg, kind);
                true
            }
            Action::ShowAbout => {
                use crate::components::modal::Modal;
                self.modals.push(Modal::Message {
                    title: "postui".into(),
                    body: "A fast, local-first terminal HTTP client.".into(),
                });
                true
            }
            Action::EditorTabSelect(i) => {
                self.editor.active_tab = EditorTab::from_index(i);
                self.editor.table.reset();
                true
            }
            Action::EditorTabCycle(delta) => {
                let cur = self.editor.active_tab.index() as i8;
                let next = (cur + delta).rem_euclid(3);
                self.editor.active_tab = EditorTab::from_index(next as usize);
                self.editor.table.reset();
                true
            }
            Action::CycleMethod => {
                self.editor.method = self.editor.method.cycle();
                true
            }
            Action::FocusUrl => {
                self.focus = PaneId::Editor;
                self.editor.sub_focus = SubFocus::Url;
                true
            }
        }
    }

    /// Whether any in-flight HTTP request is still ticking (e.g. animating
    /// a spinner) and therefore needs a redraw. Always `false` until
    /// Task 15 introduces in-flight request state.
    fn in_flight_ticking(&self) -> bool {
        false
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
    /// (i.e. whether the caller should redraw): the OR of every
    /// `self.update(..)` call's result along the branch taken, plus any
    /// modal state change (close/typing) that bypasses `update`.
    pub fn handle_key(&mut self, keymap: &Keymap, ev: KeyEvent) -> bool {
        let combo = KeyCombo::from_event(&ev);
        let global = keymap.lookup(&combo);
        let modified = ev.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

        // 1. A modified quit combo is the escape hatch: it pre-empts everything.
        if modified && global == Some(Action::Quit) {
            return self.update(Action::Quit);
        }

        // 2. Modals capture all remaining input.
        if !self.modals.is_empty() {
            let Some(res) = self.modals.handle_key(ev) else {
                return true; // typed into modal
            };
            let mut changed = res.close;
            if res.close {
                self.modals.pop();
            }
            for a in res.actions {
                changed |= self.update(a);
            }
            return changed;
        }

        // 3. Modified combos prefer the global keymap (app shortcuts beat editors).
        if modified && let Some(a) = global {
            return self.update(a);
        }

        // 4. The focused component gets plain keys (and unbound modified ones) next.
        if let Some(a) = self.focused_component_key(ev) {
            return self.update(a);
        }

        // 5. Global fallback for plain keys the component ignored.
        if let Some(a) = global {
            return self.update(a);
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
        let mut app = App::new_for_test();
        assert!(!app.should_quit);
        app.update(Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn tick_does_not_quit() {
        let mut app = App::new_for_test();
        app.update(Action::Tick);
        assert!(!app.should_quit);
    }

    #[test]
    fn focus_next_moves_focus() {
        let mut app = App::new_for_test();
        let start = app.focus;
        app.update(Action::FocusNext);
        assert_ne!(app.focus, start);
        app.update(Action::FocusPrev);
        assert_eq!(app.focus, start);
    }

    #[test]
    fn close_pops_modal_instead_of_quitting() {
        use crate::components::modal::Modal;
        let mut app = App::new_for_test();
        app.modals.push(Modal::Message { title: "t".into(), body: "b".into() });
        app.update(Action::Close);
        assert!(app.modals.is_empty());
        assert!(!app.should_quit);
    }

    #[test]
    fn open_palette_pushes_modal() {
        let mut app = App::new_for_test();
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

    #[test]
    fn tick_requests_no_redraw_when_idle() {
        let mut app = App::new_for_test();
        assert!(!app.update(Action::Tick), "idle tick must not redraw");
    }

    #[test]
    fn tick_requests_redraw_while_toast_visible() {
        let mut app = App::new_for_test();
        app.update(Action::ShowToast("hi".into(), crate::components::toast::ToastKind::Info));
        assert!(app.update(Action::Tick));
    }

    #[test]
    fn render_action_requests_redraw() {
        let mut app = App::new_for_test();
        assert!(app.update(Action::Render));
    }

    #[test]
    fn scroll_dispatches_without_changing_focus() {
        let mut app = App::new_for_test();
        let before = app.focus;
        assert!(app.update(Action::ScrollPane(PaneId::Response, 3)));
        assert_eq!(app.focus, before, "scrolling must not steal focus");
    }
}
