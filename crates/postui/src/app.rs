use crate::action::Action;
use crate::components::editor::{Editor, EditorTab, SubFocus};
use crate::components::modal::{Modal, ModalStack, PromptKind};
use crate::components::sidebar::Row;
use crate::components::toast::{ToastKind, Toasts};
use crate::components::{response::Response, sidebar::Sidebar, Component};
use crate::keys::{KeyCombo, Keymap};
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// The project's root directory (`requests/**/*.toml` lives under it).
    pub project_root: PathBuf,
    /// Sender for background tasks (e.g. in-flight requests) to push
    /// `Action`s back into the main loop without blocking on it.
    pub tx: UnboundedSender<Action>,
    /// An action that can only be applied by suspending the terminal, parked
    /// here by `update` for the main loop to take and run. Keeps `update`
    /// itself terminal-free (and therefore testable without a TTY).
    pub pending_terminal_action: Option<Action>,
    /// Keeps the test-only channel's receiver alive so `tx` doesn't become
    /// a dangling sender in `App::new_for_test()`. Always `None` outside
    /// of tests.
    _test_rx: Option<UnboundedReceiver<Action>>,
}

impl App {
    /// Resolves the default project directory and opens it. If it cannot be
    /// determined at all (no home/config dir on this platform), the app
    /// still starts, with an empty sidebar and a toast explaining why.
    pub fn new(tx: UnboundedSender<Action>) -> Self {
        match postui_core::storage::default_project_dir() {
            Some(root) => Self::with_root(tx, root),
            None => {
                let mut app = Self::bare(tx, PathBuf::new());
                app.toasts.push(
                    "could not determine a project directory for this platform",
                    ToastKind::Error,
                );
                app
            }
        }
    }

    /// Opens `root` as the project directory: ensures `root/requests/`
    /// exists and populates the sidebar from it. A failure to create the
    /// directory surfaces as a toast rather than a crash; the app keeps
    /// working with whatever the sidebar already had (empty, on a fresh
    /// app).
    pub fn with_root(tx: UnboundedSender<Action>, root: PathBuf) -> Self {
        let mut app = Self::bare(tx, root);
        match postui_core::storage::ensure_project(&app.project_root) {
            Ok(()) => {
                let listing = postui_core::storage::list_requests(&app.project_root);
                app.sidebar.refresh(listing);
            }
            Err(e) => {
                app.toasts.push(format!("could not open project: {e}"), ToastKind::Error);
            }
        }
        app
    }

    fn bare(tx: UnboundedSender<Action>, root: PathBuf) -> Self {
        Self {
            should_quit: false,
            focus: PaneId::Sidebar,
            theme: Theme::for_terminal(),
            sidebar: Sidebar::default(),
            editor: Editor::default(),
            response: Response,
            toasts: Toasts::default(),
            modals: ModalStack::default(),
            project_root: root,
            tx,
            pending_terminal_action: None,
            _test_rx: None,
        }
    }

    /// Constructs an `App` for tests, with its own channel so `tx` is
    /// always a live sender, and a fresh throwaway project directory under
    /// the system temp dir (unique per call, so tests never collide).
    pub fn new_for_test() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("postui-test-{}-{n}", std::process::id()));
        let mut app = Self::with_root(tx, root);
        app._test_rx = Some(rx);
        app
    }
}

impl App {
    /// Applies `action` to app state. Returns `true` if state changed in a
    /// way that requires a redraw, `false` if the caller can skip drawing
    /// this iteration.
    pub fn update(&mut self, action: Action) -> bool {
        let changed = self.apply(action);
        // Keeps the sidebar's dirty dot and its notion of "which slug is
        // open" in lockstep with the editor after every action, rather than
        // threading that bookkeeping through each arm individually.
        self.sidebar.open_slug = self.editor.slug.clone();
        self.sidebar.open_dirty = self.editor.is_dirty();
        changed
    }

    fn apply(&mut self, action: Action) -> bool {
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
            Action::FormatBody => self.transform_body(postui_core::json::format),
            Action::MinifyBody => self.transform_body(postui_core::json::minify),
            // Suspending the terminal is the main loop's job; park the action
            // and let it pick this up after the current key is handled.
            Action::OpenBodyInEditor => {
                self.pending_terminal_action = Some(Action::OpenBodyInEditor);
                true
            }
            Action::OpenRequest(slug) => {
                if self.editor.is_dirty() {
                    let current = self.editor.slug.clone().unwrap_or_default();
                    self.modals.push(Modal::Confirm {
                        title: "Unsaved changes".into(),
                        body: format!("\"{current}\" has unsaved changes."),
                        choices: vec![
                            (
                                's',
                                "Save & open".into(),
                                vec![Action::SaveRequest, Action::ForceOpenRequest(slug.clone())],
                            ),
                            ('d', "Discard changes".into(), vec![Action::ForceOpenRequest(slug)]),
                        ],
                    });
                    true
                } else {
                    self.apply(Action::ForceOpenRequest(slug))
                }
            }
            Action::ForceOpenRequest(slug) => {
                match postui_core::storage::load_request(&self.project_root, &slug) {
                    Ok(req) => self.editor.load(Some(slug), req),
                    Err(e) => {
                        self.toasts.push(format!("could not open {slug}: {e}"), ToastKind::Error);
                    }
                }
                true
            }
            Action::SaveRequest => {
                match self.editor.slug.clone() {
                    Some(slug) => {
                        let req = self.editor.current_request();
                        match postui_core::storage::save_request(&self.project_root, &slug, &req) {
                            Ok(()) => {
                                self.editor.mark_saved();
                                self.toasts.push(format!("Saved {slug}"), ToastKind::Success);
                                let listing = postui_core::storage::list_requests(&self.project_root);
                                self.sidebar.refresh(listing);
                            }
                            Err(e) => {
                                self.toasts.push(format!("could not save {slug}: {e}"), ToastKind::Error);
                            }
                        }
                    }
                    None => {
                        self.modals.push(Modal::Prompt {
                            title: "Save request as".into(),
                            input: crate::components::line_input::LineInput::new(""),
                            kind: PromptKind::SaveAs,
                        });
                    }
                }
                true
            }
            Action::ShowRequestError(slug) => {
                let body = self
                    .sidebar
                    .rows
                    .iter()
                    .find_map(|r| match r {
                        Row::Request { slug: s, broken: Some(b) } if *s == slug => Some(b.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "unknown error".to_string());
                self.modals.push(Modal::Message { title: format!("{slug}: parse error"), body });
                true
            }
            Action::RefreshSidebar => {
                let listing = postui_core::storage::list_requests(&self.project_root);
                self.sidebar.refresh(listing);
                true
            }
            Action::PromptNewRequest => {
                self.modals.push(Modal::Prompt {
                    title: "New request".into(),
                    input: crate::components::line_input::LineInput::new(""),
                    kind: PromptKind::NewRequest,
                });
                true
            }
            Action::PromptRenameRequest => {
                if let Some(slug) = self.sidebar.selected_slug() {
                    self.modals.push(Modal::Prompt {
                        title: "Rename request".into(),
                        input: crate::components::line_input::LineInput::new(&slug),
                        kind: PromptKind::RenameRequest { from: slug },
                    });
                }
                true
            }
            Action::ConfirmDeleteRequest => {
                if let Some(slug) = self.sidebar.selected_slug() {
                    self.modals.push(Modal::Confirm {
                        title: "Delete request".into(),
                        body: format!("Delete \"{slug}\"? This cannot be undone."),
                        choices: vec![
                            ('y', "Delete".into(), vec![Action::DeleteRequest(slug)]),
                            ('n', "Keep".into(), vec![]),
                        ],
                    });
                }
                true
            }
            Action::CreateRequest(name) => {
                self.create_or_save_as(&name, |_| postui_core::model::HttpRequest {
                    method: postui_core::model::Method::Get,
                    url: String::new(),
                    params: Default::default(),
                    headers: Default::default(),
                    body: None,
                });
                true
            }
            Action::RenameRequest { from, to } => {
                if postui_core::storage::validate_slug(&to).is_err() {
                    self.toasts.push(
                        "invalid name: lowercase letters, digits, - _ and / only",
                        ToastKind::Error,
                    );
                    return true;
                }
                match postui_core::storage::rename_request(&self.project_root, &from, &to) {
                    Ok(()) => {
                        let listing = postui_core::storage::list_requests(&self.project_root);
                        self.sidebar.refresh(listing);
                        if self.editor.slug.as_deref() == Some(from.as_str()) {
                            self.editor.slug = Some(to.clone());
                            self.sidebar.open_slug = Some(to);
                        }
                    }
                    Err(e) => {
                        self.toasts.push(format!("could not rename {from}: {e}"), ToastKind::Error);
                    }
                }
                true
            }
            Action::DeleteRequest(slug) => {
                match postui_core::storage::delete_request(&self.project_root, &slug) {
                    Ok(()) => {
                        let listing = postui_core::storage::list_requests(&self.project_root);
                        self.sidebar.refresh(listing);
                        if self.editor.slug.as_deref() == Some(slug.as_str()) {
                            self.editor = Editor::default();
                        }
                    }
                    Err(e) => {
                        self.toasts.push(format!("could not delete {slug}: {e}"), ToastKind::Error);
                    }
                }
                true
            }
            Action::SaveRequestAs(name) => {
                let req = self.editor.current_request();
                self.create_or_save_as(&name, move |_| req.clone());
                true
            }
        }
    }

    /// Shared validate/exists-check/save/refresh/open path for `CreateRequest`
    /// and `SaveRequestAs`: both save a fresh `HttpRequest` (a default one, or
    /// the editor's current one) to a brand-new slug and switch the editor
    /// over to it. `build` receives the slug in case a future caller needs it;
    /// today's callers ignore it.
    fn create_or_save_as(&mut self, name: &str, build: impl FnOnce(&str) -> postui_core::model::HttpRequest) {
        if postui_core::storage::validate_slug(name).is_err() {
            self.toasts.push(
                "invalid name: lowercase letters, digits, - _ and / only",
                ToastKind::Error,
            );
            return;
        }
        let existing = postui_core::storage::list_requests(&self.project_root);
        if existing.iter().any(|l| l.slug == name) {
            self.toasts.push(format!("request already exists: {name:?}"), ToastKind::Error);
            return;
        }
        let req = build(name);
        match postui_core::storage::save_request(&self.project_root, name, &req) {
            Ok(()) => {
                self.editor.load(Some(name.to_string()), req);
                self.editor.mark_saved();
                self.toasts.push(format!("Saved {name}"), ToastKind::Success);
                let listing = postui_core::storage::list_requests(&self.project_root);
                self.sidebar.refresh(listing);
                self.sidebar.select_slug(name);
            }
            Err(e) => {
                self.toasts.push(format!("could not save {name}: {e}"), ToastKind::Error);
            }
        }
    }

    /// Rewrites the body through a JSON transform. An empty body is a no-op
    /// (nothing to format), and invalid JSON leaves the buffer exactly as the
    /// user typed it, reporting the parse position in a toast.
    fn transform_body(
        &mut self,
        transform: fn(&str) -> Result<String, postui_core::json::JsonError>,
    ) -> bool {
        let text = self.editor.body_text();
        if text.is_empty() {
            return true;
        }
        match transform(&text) {
            Ok(formatted) => {
                self.editor.set_body_text(&formatted);
                true
            }
            Err(e) => {
                self.toasts.push(e.to_string(), crate::components::toast::ToastKind::Error);
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

    fn req(url: &str) -> postui_core::model::HttpRequest {
        postui_core::model::HttpRequest::from_toml_str(&format!(r#"url = "{url}""#)).unwrap()
    }

    #[test]
    fn sidebar_lists_requests_grouped_and_enter_opens() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        postui_core::storage::ensure_project(dir.path()).unwrap();
        postui_core::storage::save_request(dir.path(), "auth/login", &req("https://x/login")).unwrap();
        postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
        let mut app = App::with_root(tx, dir.path().to_path_buf());

        assert_eq!(
            app.sidebar.rows,
            vec![
                Row::Request { slug: "ping".into(), broken: None },
                Row::Dir("auth".into()),
                Row::Request { slug: "auth/login".into(), broken: None },
            ]
        );

        // navigate from "ping" (index 0) down to "auth/login" (index 2, Dir skipped)
        app.handle_key(&Keymap::default_bindings(), plain('j'));
        app.handle_key(&Keymap::default_bindings(), KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.editor.slug.as_deref(), Some("auth/login"));
    }

    #[test]
    fn opening_over_dirty_editor_prompts_save_discard_cancel() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        postui_core::storage::ensure_project(dir.path()).unwrap();
        postui_core::storage::save_request(dir.path(), "a", &req("https://x/a")).unwrap();
        postui_core::storage::save_request(dir.path(), "b", &req("https://x/b")).unwrap();
        let mut app = App::with_root(tx, dir.path().to_path_buf());
        let keymap = Keymap::default_bindings();

        // Open "a", then edit its URL so the editor becomes dirty.
        app.update(Action::ForceOpenRequest("a".into()));
        app.focus = PaneId::Editor;
        app.editor.sub_focus = SubFocus::Url;
        app.handle_key(&keymap, plain('/'));
        assert!(app.editor.is_dirty());

        // Requesting to open "b" while dirty must prompt instead of opening.
        app.update(Action::OpenRequest("b".into()));
        assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
        assert_eq!(app.editor.slug.as_deref(), Some("a"), "still on the original request");

        // 'd' discards the edit and opens "b".
        app.handle_key(&keymap, plain('d'));
        assert_eq!(app.editor.slug.as_deref(), Some("b"));
        assert!(!app.editor.is_dirty());

        // Back to "a", dirty it again, this time choose 's' to save & open.
        let mut app = App::with_root(app.tx.clone(), dir.path().to_path_buf());
        app.update(Action::ForceOpenRequest("a".into()));
        app.focus = PaneId::Editor;
        app.editor.sub_focus = SubFocus::Url;
        app.handle_key(&keymap, plain('/'));
        assert!(app.editor.is_dirty());
        app.update(Action::OpenRequest("b".into()));
        assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
        app.handle_key(&keymap, plain('s'));
        assert_eq!(app.editor.slug.as_deref(), Some("b"));
        let saved = postui_core::storage::load_request(dir.path(), "a").unwrap();
        assert_eq!(saved.url, "https://x/a/", "the edit was persisted before opening b");
    }

    #[test]
    fn broken_file_shows_marker_and_error_modal() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        postui_core::storage::ensure_project(dir.path()).unwrap();
        std::fs::write(dir.path().join("requests/bad.toml"), "url = \"x\"\nurl = \"dup\"\n").unwrap();
        let mut app = App::with_root(tx, dir.path().to_path_buf());

        let Row::Request { broken, .. } = &app.sidebar.rows[0] else { panic!("expected a request row") };
        assert!(broken.is_some());

        app.handle_key(&Keymap::default_bindings(), KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match app.modals.top() {
            Some(Modal::Message { body, .. }) => {
                assert!(body.contains('2') || body.to_lowercase().contains("duplicate"));
            }
            _ => panic!("expected a Message modal"),
        }
    }

    #[test]
    fn dirty_dot_renders_in_sidebar() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        postui_core::storage::ensure_project(dir.path()).unwrap();
        postui_core::storage::save_request(dir.path(), "a", &req("https://x/a")).unwrap();
        let mut app = App::with_root(tx, dir.path().to_path_buf());
        app.update(Action::ForceOpenRequest("a".into()));
        app.focus = PaneId::Editor;
        app.editor.sub_focus = SubFocus::Url;
        app.handle_key(&Keymap::default_bindings(), plain('/'));
        assert!(app.editor.is_dirty());

        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains('\u{25cf}'), "expected a dirty dot in the sidebar: {content}");
    }

    #[test]
    fn new_request_prompt_flow_creates_file_and_opens_it() {
        let mut app = App::new_for_test();
        let keymap = Keymap::default_bindings();
        app.focus = PaneId::Sidebar;
        app.handle_key(&keymap, plain('n'));
        assert!(matches!(app.modals.top(), Some(Modal::Prompt { kind: PromptKind::NewRequest, .. })));
        for c in "api/ping".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modals.is_empty());
        assert_eq!(app.editor.slug.as_deref(), Some("api/ping"));
        assert!(postui_core::storage::load_request(&app.project_root, "api/ping").is_ok());
        assert!(
            app.sidebar.rows.iter().any(|r| matches!(r, Row::Request { slug, .. } if slug == "api/ping")),
            "sidebar should list the new request: {:?}",
            app.sidebar.rows
        );
    }

    #[test]
    fn new_request_invalid_name_toasts_and_creates_nothing() {
        let mut app = App::new_for_test();
        let keymap = Keymap::default_bindings();
        app.update(Action::PromptNewRequest);
        for c in "Bad Name".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modals.is_empty(), "modal closes even though the save is rejected");
        assert!(!app.toasts.is_empty(), "an invalid name must toast");
        assert!(postui_core::storage::list_requests(&app.project_root).is_empty());
    }

    #[test]
    fn rename_request_updates_disk_and_open_slug() {
        let mut app = App::new_for_test();
        postui_core::storage::save_request(&app.project_root, "old", &req("https://x/old")).unwrap();
        app.update(Action::RefreshSidebar);
        app.update(Action::ForceOpenRequest("old".into()));
        let keymap = Keymap::default_bindings();
        app.focus = PaneId::Sidebar;
        app.handle_key(&keymap, plain('r'));
        match app.modals.top() {
            Some(Modal::Prompt { kind: PromptKind::RenameRequest { from }, .. }) => {
                assert_eq!(from, "old");
            }
            _ => panic!("expected a RenameRequest prompt"),
        }
        for _ in 0.."old".len() {
            app.handle_key(&keymap, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        for c in "new".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modals.is_empty());
        assert!(postui_core::storage::load_request(&app.project_root, "old").is_err());
        assert!(postui_core::storage::load_request(&app.project_root, "new").is_ok());
        assert_eq!(app.editor.slug.as_deref(), Some("new"));
        assert_eq!(app.sidebar.open_slug.as_deref(), Some("new"));
    }

    #[test]
    fn delete_open_request_clears_editor_and_removes_file() {
        let mut app = App::new_for_test();
        postui_core::storage::save_request(&app.project_root, "gone", &req("https://x/gone")).unwrap();
        app.update(Action::RefreshSidebar);
        app.update(Action::ForceOpenRequest("gone".into()));
        let keymap = Keymap::default_bindings();
        app.focus = PaneId::Sidebar;
        app.handle_key(&keymap, plain('d'));
        assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
        app.handle_key(&keymap, plain('y'));
        assert!(app.modals.is_empty());
        assert!(app.editor.slug.is_none(), "editor must reset once its open request is deleted");
        assert!(postui_core::storage::load_request(&app.project_root, "gone").is_err());
    }

    #[test]
    fn save_with_no_slug_opens_save_as_prompt() {
        let mut app = App::new_for_test();
        app.editor.url = crate::components::line_input::LineInput::new("https://x/new");
        let keymap = Keymap::default_bindings();
        app.update(Action::SaveRequest);
        assert!(matches!(app.modals.top(), Some(Modal::Prompt { kind: PromptKind::SaveAs, .. })));
        for c in "fresh".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modals.is_empty());
        assert_eq!(app.editor.slug.as_deref(), Some("fresh"));
        let saved = postui_core::storage::load_request(&app.project_root, "fresh").unwrap();
        assert_eq!(saved.url, "https://x/new");
    }

    #[test]
    fn rename_and_delete_on_empty_sidebar_do_nothing() {
        let mut app = App::new_for_test();
        let keymap = Keymap::default_bindings();
        app.focus = PaneId::Sidebar;
        app.handle_key(&keymap, plain('r'));
        assert!(app.modals.is_empty());
        app.handle_key(&keymap, plain('d'));
        assert!(app.modals.is_empty());
    }
}
