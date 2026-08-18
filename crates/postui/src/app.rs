use crate::action::{Action, CopyTarget};
use crate::components::editor::{Editor, EditorTab, SubFocus};
use crate::components::modal::{Modal, ModalResult, ModalStack, PromptKind};
use crate::components::response::ResponseState;
use crate::components::sidebar::Row;
use crate::components::toast::{ToastKind, Toasts};
use crate::components::varmanager::VarManager;
use crate::components::{Component, sidebar::Sidebar};
use crate::hit::{Hit, HitMap, ScrollbarSpec};
use crate::keys::{KeyCombo, Keymap};
use crate::layout::PaneId;
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// An in-progress scrollbar drag: which pane's thumb is held, and how far
/// down the thumb the pointer grabbed it, so the thumb keeps its position
/// under the cursor instead of jumping its top to the pointer.
pub struct Drag {
    pub pane: PaneId,
    pub grab_offset: u16,
}

/// Which full-frame screen is showing. `ui::draw` and `App::handle_key`
/// each branch on this once; every screen but `Main` replaces the three
/// panes with its own full-frame draw while the header and footer stay.
/// This is also where future screens (history, console) slot in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Main,
    VarManager,
}

pub struct App {
    pub should_quit: bool,
    pub focus: PaneId,
    /// Which full-frame screen is showing (spec §5). `Screen::Main` is the
    /// normal three-pane layout.
    pub screen: Screen,
    /// The focus `Screen::Main` had when a non-`Main` screen was opened, so
    /// `Action::CloseScreen` can restore it exactly as left.
    prior_focus: PaneId,
    pub theme: Theme,
    pub sidebar: Sidebar,
    pub editor: Editor,
    /// The Variable Manager screen's own state, shown full-frame while
    /// `screen == Screen::VarManager`.
    pub varmanager: VarManager,
    /// The request session: the open request's on-screen response, the
    /// per-request response cache, and the in-flight send.
    pub session: crate::session::Session,
    pub toasts: Toasts,
    pub modals: ModalStack,
    /// The open project: root directory, metadata, variables, environments,
    /// and the active environment's resolved values.
    pub project: ProjectContext,
    /// The global registry of known projects (config.toml's `[projects]`
    /// table): cycle order, configured root, last-used project.
    pub registry: crate::config::ProjectsRegistry,
    /// Where to save `registry` back to. `None` in tests, so test runs never
    /// touch the real global config file.
    registry_path: Option<PathBuf>,
    /// The tiered clipboard (external command / OS clipboard / OSC 52),
    /// configured from `ui_settings`.
    pub clipboard: crate::clipboard::Clipboard,
    /// Mouse-first-GUI UI settings (clipboard command, OSC 52 threshold),
    /// loaded from the same `config.toml` the registry uses.
    pub ui_settings: crate::config::UiSettings,
    /// Palette command frecency stats (recency + count per command id),
    /// loaded from `ui.toml` at startup and saved back on quit.
    pub usage: crate::usage::UsageStore,
    /// Where to save `usage` back to. `None` in tests, so test runs never
    /// touch the real `ui.toml`.
    usage_path: Option<PathBuf>,
    /// The HTTP client used for every send. Built eagerly and cheaply
    /// (`reqwest::Client::builder().build()` needs no running Tokio
    /// reactor — verified in `http::tests::client_builds_without_a_tokio_runtime`
    /// — so `App` stays constructible in the many plain `#[test]`s that
    /// never touch the network).
    pub client: reqwest::Client,
    /// Sender for background tasks (e.g. in-flight requests) to push
    /// `Action`s back into the main loop without blocking on it.
    pub tx: UnboundedSender<Action>,
    /// An action that can only be applied by suspending the terminal, parked
    /// here by `update` for the main loop to take and run. Keeps `update`
    /// itself terminal-free (and therefore testable without a TTY).
    pub pending_terminal_action: Option<Action>,
    /// Rebuilt every frame by `ui::draw`: maps screen regions to typed
    /// [`Hit`]s for mouse routing.
    pub hits: HitMap,
    /// The `Hit` currently under the pointer, if any, updated by
    /// `handle_mouse` on `Moved`. Read by `ui::draw` to style hovered
    /// buttons/chips.
    pub hovered: Option<Hit>,
    /// An in-progress drag (e.g. a scrollbar thumb), if any.
    pub drag: Option<Drag>,
    /// Whether the active tab's params/headers table body is collapsed
    /// (tab strip + its count chip stay visible; only the table itself is
    /// hidden). Session-only — never persisted.
    pub table_collapsed: bool,
    /// The most recent left-click's hit and when it landed, used to detect
    /// a double-click (same hit, within 400ms).
    last_click: Option<(Hit, std::time::Instant)>,
    /// Keeps the test-only channel's receiver alive so `tx` doesn't become
    /// a dangling sender in `App::new_for_test()`. Always `None` outside
    /// of tests.
    _test_rx: Option<UnboundedReceiver<Action>>,
    /// Owns (and, on drop, removes) the throwaway project directory made by
    /// `App::new_for_test()`. Always `None` outside of tests.
    _test_dir: Option<tempfile::TempDir>,
}

/// What to do with the chosen startup root once its source is known.
///
/// The confirm-to-create prompt is reserved for a CLI-supplied root: per the
/// spec, `postui <dir>` opens `<dir>` and asks before creating a project
/// there if it lacks `project.toml`. The app's *own* fallback pick (the
/// platform default project directory, used only when nothing else applies)
/// self-initializes silently instead — the user never typed that path, so
/// asking them to confirm it would be asking about an implementation detail.
/// A root that came from the registry (last-used / known) and no longer has
/// a `project.toml` (deleted or moved outside postui) doesn't prompt either:
/// it opens as a bare directory with a warning toast, since it *was* a
/// project when registered and re-creating it silently could be surprising.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupDisposition {
    /// A CLI-supplied root without `project.toml`: prompt before creating.
    PromptCreate,
    /// The platform default project dir, picked because nothing else was
    /// configured, without `project.toml`: self-initialize silently.
    InitDefault,
    /// Open as-is: already a project, or a registry-sourced root that no
    /// longer is (bare-dir open + warning toast, handled by the caller).
    /// `register` is set when the root came straight from the CLI and isn't
    /// already known — the caller must add it to the registry so `postui
    /// <dir>` on an existing project actually registers it (spec §1 CLI).
    OpenAsIs { register: bool },
}

/// Picks the startup root and what to do about it, given the registry, an
/// optional CLI-supplied root, and the platform's default project
/// directory. Only touches the filesystem via `is_project`'s single-file
/// check, so it's covered directly by the unit tests below rather than
/// through `App::new` (which reads the real user config file).
///
/// Precedence: `cli_root`, then the registry's last-used project if it still
/// exists (a `last` pointing at a deleted directory is skipped — the third
/// return value carries that path so the caller can toast about it), then
/// the first known project that still exists on disk, then `default_dir`.
/// `None` means no candidate root exists at all.
fn resolve_startup(
    registry: &crate::config::ProjectsRegistry,
    cli_root: Option<PathBuf>,
    default_dir: Option<PathBuf>,
) -> Option<(PathBuf, StartupDisposition, Option<PathBuf>)> {
    if let Some(root) = cli_root {
        let disposition = if postui_core::project::is_project(&root) {
            StartupDisposition::OpenAsIs { register: true }
        } else {
            StartupDisposition::PromptCreate
        };
        return Some((root, disposition, None));
    }
    let stale_last = match &registry.last {
        Some(root) if root.is_dir() => {
            return Some((
                root.clone(),
                StartupDisposition::OpenAsIs { register: false },
                None,
            ));
        }
        Some(root) => Some(root.clone()),
        None => None,
    };
    if let Some(root) = registry.known.iter().find(|p| p.is_dir()).cloned() {
        return Some((
            root,
            StartupDisposition::OpenAsIs { register: false },
            stale_last,
        ));
    }
    if let Some(root) = default_dir {
        let disposition = if postui_core::project::is_project(&root) {
            StartupDisposition::OpenAsIs { register: false }
        } else {
            StartupDisposition::InitDefault
        };
        return Some((root, disposition, stale_last));
    }
    None
}

impl App {
    /// Resolves the project to open (see [`resolve_startup`]) and opens it,
    /// self-initializing or prompting to create as its disposition says.
    pub fn new(tx: UnboundedSender<Action>, cli_root: Option<PathBuf>) -> Self {
        let registry_path = crate::config::config_file_path();
        let registry = registry_path
            .as_deref()
            .map(crate::config::ProjectsRegistry::load_from)
            .unwrap_or_default();
        let (ui_settings, ui_warnings) = registry_path
            .as_deref()
            .map(crate::config::load_ui_settings)
            .unwrap_or_default();
        let theme = Theme::from_environment(ui_settings.theme, &mut crate::theme::OscQuery);
        let usage_path = crate::config::ui_file_path();
        let usage = usage_path
            .as_deref()
            .map(crate::usage::UsageStore::load_from)
            .unwrap_or_default();

        let Some((root, disposition, stale_last)) = resolve_startup(
            &registry,
            cli_root,
            postui_core::storage::default_project_dir(),
        ) else {
            let mut app = Self::bare(tx, PathBuf::new());
            app.registry = registry;
            app.registry_path = registry_path;
            app.clipboard = crate::clipboard::Clipboard::new(&ui_settings);
            app.ui_settings = ui_settings;
            app.theme = theme;
            app.usage = usage;
            app.usage_path = usage_path;
            for w in ui_warnings {
                app.toasts.push(w, ToastKind::Warning);
            }
            app.toasts.push(
                "could not determine a project directory for this platform",
                ToastKind::Error,
            );
            return app;
        };

        let mut app = Self::with_root(tx, root);
        app.registry = registry;
        app.registry_path = registry_path;
        app.clipboard = crate::clipboard::Clipboard::new(&ui_settings);
        app.ui_settings = ui_settings;
        app.theme = theme;
        app.usage = usage;
        app.usage_path = usage_path;
        for w in ui_warnings {
            app.toasts.push(w, ToastKind::Warning);
        }

        if let Some(missing) = stale_last {
            app.toasts.push(
                format!(
                    "last project {} no longer exists; skipped",
                    missing.display()
                ),
                ToastKind::Warning,
            );
        }

        match disposition {
            StartupDisposition::InitDefault => {
                let _ = postui_core::project::init_project(&app.project.root, Some("default"));
                app.registry.register(app.project.root.clone());
                if let Some(path) = &app.registry_path {
                    let _ = app.registry.save_to(path);
                }
            }
            StartupDisposition::PromptCreate => {
                let path = app.project.root.display().to_string();
                let fallback_actions = match postui_core::storage::default_project_dir() {
                    Some(fallback) => vec![Action::SwitchProject(fallback)],
                    None => vec![],
                };
                app.modals.push(Modal::Confirm {
                    title: "Not a postui project".into(),
                    body: format!("{path} has no project.toml — create one here?"),
                    choices: vec![
                        ('y', "Create project".into(), vec![Action::InitProjectHere]),
                        ('n', "Open default project".into(), fallback_actions),
                    ],
                });
            }
            StartupDisposition::OpenAsIs { register } => {
                if !postui_core::project::is_project(&app.project.root) {
                    app.toasts.push(
                        format!(
                            "{} has no project.toml; opened as a bare directory",
                            app.project.root.display()
                        ),
                        ToastKind::Warning,
                    );
                } else if register {
                    app.registry.register(app.project.root.clone());
                    if let Some(path) = &app.registry_path {
                        let _ = app.registry.save_to(path);
                    }
                }
            }
        }

        app
    }

    /// Opens `root` as the project directory: ensures `root/requests/`
    /// exists and populates the sidebar from it. A failure to create the
    /// directory surfaces as a toast rather than a crash; the app keeps
    /// working with whatever the sidebar already had (empty, on a fresh
    /// app).
    pub fn with_root(tx: UnboundedSender<Action>, root: PathBuf) -> Self {
        let mut app = Self::bare(tx, root);
        match postui_core::storage::ensure_project(&app.project.root) {
            Ok(()) => {
                app.refresh_sidebar();
                // Restore the request that was open when this project was
                // last used — the same restore a project *switch* already
                // performs. Without it the sidebar draws a selection whose
                // data never made it into the editor.
                if let Some(slug) = app.project.local_open_request()
                    && postui_core::storage::load_request(&app.project.root, &slug).is_ok()
                {
                    app.update(Action::ForceOpenRequest(slug));
                }
            }
            Err(e) => {
                app.toasts
                    .push(format!("could not open project: {e}"), ToastKind::Error);
            }
        }
        app
    }

    fn bare(tx: UnboundedSender<Action>, root: PathBuf) -> Self {
        let (project, warnings) = ProjectContext::open(root);
        let mut toasts = Toasts::default();
        for w in warnings {
            toasts.push(w, ToastKind::Warning);
        }
        Self {
            should_quit: false,
            focus: PaneId::Sidebar,
            screen: Screen::default(),
            prior_focus: PaneId::Sidebar,
            theme: Theme::for_terminal(),
            sidebar: Sidebar::default(),
            editor: Editor::default(),
            varmanager: VarManager,
            session: crate::session::Session::default(),
            toasts,
            modals: ModalStack::default(),
            project,
            registry: crate::config::ProjectsRegistry::default(),
            registry_path: None,
            clipboard: crate::clipboard::Clipboard::new(&crate::config::UiSettings::default()),
            ui_settings: crate::config::UiSettings::default(),
            usage: crate::usage::UsageStore::default(),
            usage_path: None,
            client: crate::http::client(),
            tx,
            pending_terminal_action: None,
            hits: HitMap::default(),
            hovered: None,
            drag: None,
            table_collapsed: false,
            last_click: None,
            _test_rx: None,
            _test_dir: None,
        }
    }

    /// Constructs an `App` for tests, with its own channel so `tx` is
    /// always a live sender, and a fresh throwaway project directory the
    /// `App` owns and deletes on drop. A name derived from pid + a counter
    /// is *not* enough: this leaves the directory behind, and a later run
    /// that happens to reuse the pid inherits its files.
    pub fn new_for_test() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().expect("a temp dir for the test project");
        let mut app = Self::with_root(tx, dir.path().to_path_buf());
        app._test_rx = Some(rx);
        app._test_dir = Some(dir);
        app
    }

    /// Swaps in a test-configured clipboard (e.g. `Clipboard::new_for_test`)
    /// so copy tests can exercise the cmd/OSC-52 tiers deterministically.
    /// Gated on the `test-util` feature (see `Clipboard::new_for_test`) so
    /// integration tests under `tests/` can use it while a plain build
    /// never exposes it.
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_clipboard_for_test(&mut self, c: crate::clipboard::Clipboard) {
        self.clipboard = c;
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
        self.editor.inherited_headers = self.project.meta.default_headers.clone();
        // The response pane always shows the open request's response;
        // whenever an action changed which request is open (any route),
        // swap in that request's cached response — or an empty one.
        let swapped = self.session.sync_open(&self.editor.slug);
        // The send button shows "sending" only when the in-flight send
        // belongs to the request being looked at.
        self.editor.sending = self
            .session
            .in_flight
            .as_ref()
            .is_some_and(|f| f.slug == self.editor.slug);
        self.editor.table_collapsed = self.table_collapsed;
        changed || swapped
    }

    fn apply(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => {
                self.project
                    .persist_local_state(self.editor.slug.as_deref());
                if let Some(path) = &self.usage_path {
                    let _ = self.usage.save_to(path);
                }
                self.should_quit = true;
                true
            }
            Action::Tick => {
                self.editor.on_tick();
                self.toasts.on_tick() || self.in_flight_ticking()
            }
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
                    PaneId::Response => self.session.response.handle_scroll(delta),
                }
                true
            }
            Action::OpenPalette => {
                use crate::components::modal::Modal;
                use crate::components::palette::PaletteState;
                self.modals.push(Modal::Palette(PaletteState::new(
                    &self.usage,
                    crate::usage::now(),
                )));
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
                    body: "A fast, local-first terminal HTTP client.\n\nText selection: hold Shift while dragging (mouse capture is on).".into(),
                });
                true
            }
            Action::ResponseViewMode(mode) => {
                self.session.response.set_view_mode(mode);
                true
            }
            Action::JsonRowClicked { row, toggle } => {
                self.session.response.click_row(row, toggle);
                true
            }
            Action::CopyToClipboard(target) => {
                let Some((text, success_msg)) = self.resolve_copy(&target) else {
                    self.toasts
                        .push("nothing to copy — send a request first", ToastKind::Warning);
                    return true;
                };
                match self.clipboard.copy(&text) {
                    crate::clipboard::CopyResult::Copied { .. } => {
                        self.toasts.push(success_msg, ToastKind::Success);
                    }
                    crate::clipboard::CopyResult::OscTooLarge => {
                        self.toasts.push(
                            "Too large for the terminal clipboard — use Save body to file, or set clipboard_cmd in config",
                            ToastKind::Warning,
                        );
                    }
                    crate::clipboard::CopyResult::Failed(_) => {
                        self.toasts.push(
                            "Clipboard unavailable — try Shift+drag to select",
                            ToastKind::Error,
                        );
                    }
                }
                true
            }
            Action::PromptSaveBody => {
                let ResponseState::Ready(data) = self.session.response.state() else {
                    self.toasts
                        .push("nothing to copy — send a request first", ToastKind::Warning);
                    return true;
                };
                let slug = self
                    .editor
                    .slug
                    .clone()
                    .unwrap_or_else(|| "response".to_string())
                    .replace('/', "-");
                let ext = if data
                    .content_type
                    .as_deref()
                    .is_some_and(|c| c.contains("json"))
                {
                    "json"
                } else {
                    "txt"
                };
                let prefill = format!("~/Downloads/{slug}-response.{ext}");
                self.modals.push(Modal::Prompt {
                    title: "Save response body".into(),
                    input: crate::components::line_input::LineInput::new(&prefill),
                    kind: PromptKind::SaveBodyAs,
                });
                true
            }
            Action::SaveBodyToFile(path) => {
                let ResponseState::Ready(data) = self.session.response.state() else {
                    return true;
                };
                let expanded = crate::config::expand_tilde(&path);
                let result = (|| -> std::io::Result<()> {
                    if let Some(parent) = expanded.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&expanded, &data.body)
                })();
                match result {
                    Ok(()) => self.toasts.push(
                        format!("Saved body to {}", expanded.display()),
                        ToastKind::Success,
                    ),
                    Err(e) => self
                        .toasts
                        .push(format!("could not save body: {e}"), ToastKind::Error),
                }
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
            Action::OpenMethodDropdown => {
                use crate::components::modal::DropdownState;
                use postui_core::model::Method;
                let items: Vec<(String, Action)> = Method::ALL
                    .iter()
                    .map(|&m| (m.as_str().to_string(), Action::SetMethod(m)))
                    .collect();
                let current = Method::ALL.iter().position(|&m| m == self.editor.method);
                let anchor = self
                    .editor
                    .last_method_area
                    .unwrap_or_else(|| ratatui::layout::Rect::new(0, 0, 0, 0));
                self.modals.push(Modal::Dropdown(DropdownState {
                    anchor,
                    items,
                    selected: current.unwrap_or(0),
                    current,
                }));
                true
            }
            Action::SetMethod(m) => {
                self.editor.method = m;
                true
            }
            Action::FocusUrl => {
                self.focus = PaneId::Editor;
                self.editor.sub_focus = SubFocus::Url;
                true
            }
            Action::ToggleTableCollapse => {
                self.table_collapsed = !self.table_collapsed;
                true
            }
            Action::FormatBody => self.transform_body(postui_core::json::format),
            Action::MinifyBody => self.transform_body(postui_core::json::minify),
            Action::ToggleBodyVars => {
                self.editor.substitute_body = !self.editor.substitute_body;
                true
            }
            // Suspending the terminal is the main loop's job; park the action
            // and let it pick this up after the current key is handled.
            Action::OpenBodyInEditor => {
                self.pending_terminal_action = Some(Action::OpenBodyInEditor);
                true
            }
            Action::OpenRequest(slug) => {
                if self.editor.is_dirty() {
                    self.dirty_gate("open", Action::ForceOpenRequest(slug));
                    true
                } else {
                    self.apply(Action::ForceOpenRequest(slug))
                }
            }
            Action::ForceOpenRequest(slug) => {
                match postui_core::storage::load_request(&self.project.root, &slug) {
                    Ok(req) => {
                        self.editor.load(Some(slug.clone()), req);
                        // Every open route (click, Enter, palette, restore)
                        // drags the sidebar selection along so it can't
                        // diverge from the open request. Queue ancestor
                        // folders open, rebuild so the row exists, then
                        // select it now that it's visible.
                        self.sidebar.select_slug(&slug);
                        self.refresh_sidebar();
                        self.sidebar.select_slug(&slug);
                        self.apply(Action::PersistLocalState);
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("could not open {slug}: {e}"), ToastKind::Error);
                    }
                }
                true
            }
            Action::SaveRequest => {
                match self.editor.slug.clone() {
                    Some(slug) => {
                        let req = self.editor.current_request();
                        match postui_core::storage::save_request(&self.project.root, &slug, &req) {
                            Ok(()) => {
                                self.editor.mark_saved();
                                self.toasts
                                    .push(format!("Saved {slug}"), ToastKind::Success);
                                self.refresh_sidebar();
                            }
                            Err(e) => {
                                self.toasts
                                    .push(format!("could not save {slug}: {e}"), ToastKind::Error);
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
                        Row::Request {
                            slug: s,
                            broken: Some(b),
                            ..
                        } if *s == slug => Some(b.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "unknown error".to_string());
                self.modals.push(Modal::Message {
                    title: format!("{slug}: parse error"),
                    body,
                });
                true
            }
            Action::RefreshSidebar => {
                self.refresh_sidebar();
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
            Action::ConfirmDeleteTableRow(i) => {
                let (map, noun) = match self.editor.active_tab {
                    EditorTab::Params => (&self.editor.params, "param"),
                    EditorTab::Headers => (&self.editor.headers, "header"),
                    EditorTab::Body => return true,
                };
                if let Some((key, _)) = map.get_index(i) {
                    self.modals.push(Modal::Confirm {
                        title: format!("Delete {noun}"),
                        body: format!("Delete {noun} \"{key}\"?"),
                        choices: vec![
                            ('y', "Delete".into(), vec![Action::DeleteTableRow(i)]),
                            ('n', "Keep".into(), vec![]),
                        ],
                    });
                }
                true
            }
            Action::DeleteTableRow(i) => {
                let map = match self.editor.active_tab {
                    EditorTab::Params => &mut self.editor.params,
                    EditorTab::Headers => &mut self.editor.headers,
                    EditorTab::Body => return true,
                };
                self.editor.table.delete_row(map, i);
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
                    substitute_body: false,
                    params: Default::default(),
                    headers: Default::default(),
                    variables: Default::default(),
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
                match postui_core::storage::rename_request(&self.project.root, &from, &to) {
                    Ok(()) => {
                        self.refresh_sidebar();
                        if self.editor.slug.as_deref() == Some(from.as_str()) {
                            self.editor.slug = Some(to.clone());
                            self.sidebar.open_slug = Some(to);
                        }
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("could not rename {from}: {e}"), ToastKind::Error);
                    }
                }
                true
            }
            Action::DeleteRequest(slug) => {
                match postui_core::storage::delete_request(&self.project.root, &slug) {
                    Ok(()) => {
                        self.refresh_sidebar();
                        if self.editor.slug.as_deref() == Some(slug.as_str()) {
                            self.editor = Editor::default();
                        }
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("could not delete {slug}: {e}"), ToastKind::Error);
                    }
                }
                true
            }
            Action::SaveRequestAs(name) => {
                let req = self.editor.current_request();
                self.create_or_save_as(&name, move |_| req.clone());
                true
            }
            Action::Send => {
                if self.editor.url.text().trim().is_empty() {
                    self.toasts
                        .push("cannot send: URL is empty", ToastKind::Error);
                    return true;
                }
                let body = self.editor.body_text();
                if !body.is_empty() && postui_core::json::validate(&body).is_err() {
                    self.modals.push(Modal::Confirm {
                        title: "Invalid body".into(),
                        body: "Body is not valid JSON — send anyway?".into(),
                        choices: vec![
                            ('y', "Send anyway".into(), vec![Action::ForceSend]),
                            ('n', "Cancel".into(), vec![]),
                        ],
                    });
                    return true;
                }
                self.apply(Action::ForceSend)
            }
            Action::ForceSend => {
                self.apply(Action::ReloadProjectFiles);
                if self.editor.url.text().trim().is_empty() {
                    self.toasts
                        .push("cannot send: URL is empty", ToastKind::Error);
                    return true;
                }
                let (prepared, warnings) = match postui_core::prepare::prepare(
                    &self.editor.current_request(),
                    &self.project.prepare_context(),
                ) {
                    Ok(x) => x,
                    Err(err @ postui_core::prepare::PrepareError::Unresolved(_)) => {
                        let label = self.project.env_label();
                        self.toasts
                            .push(format!("{err} ({label})"), ToastKind::Error);
                        return true;
                    }
                };
                for w in &warnings {
                    self.toasts.push(w.to_string(), ToastKind::Warning);
                }
                let generation = self.session.begin_send();
                let tx = self.tx.clone();
                let client = self.client.clone();
                let task = tokio::spawn(async move {
                    match crate::http::send(&client, &prepared).await {
                        Ok(data) => {
                            let _ = tx.send(Action::ResponseArrived {
                                generation,
                                data: Box::new(data),
                            });
                        }
                        Err(error) => {
                            let _ = tx.send(Action::RequestFailed { generation, error });
                        }
                    }
                });
                self.session.in_flight = Some(crate::session::InFlight {
                    started: Instant::now(),
                    generation,
                    slug: self.editor.slug.clone(),
                    task,
                });
                true
            }
            Action::CancelSend => self.session.cancel(),
            Action::ResponseArrived { generation, data } => self.session.arrived(generation, data),
            Action::RequestFailed { generation, error } => self.session.failed(generation, error),
            Action::InitProjectHere => {
                match postui_core::project::init_project(&self.project.root, None) {
                    Ok(()) => {
                        self.registry.register(self.project.root.clone());
                        if let Some(path) = &self.registry_path {
                            let _ = self.registry.save_to(path);
                        }
                        self.refresh_sidebar();
                    }
                    Err(e) => {
                        self.toasts.push(
                            format!("could not create project here: {e}"),
                            ToastKind::Error,
                        );
                    }
                }
                true
            }
            Action::ToggleSelectedFolder => {
                if let Some((path, now_open)) = self.sidebar.toggle_selected_folder() {
                    if now_open {
                        self.project.expanded.insert(path);
                    } else {
                        self.project.expanded.remove(&path);
                    }
                    self.refresh_sidebar();
                    self.apply(Action::PersistLocalState);
                }
                true
            }
            Action::PersistLocalState => {
                self.project
                    .persist_local_state(self.editor.slug.as_deref());
                true
            }
            Action::OpenProjectChooser => {
                self.apply(Action::ReloadProjectFiles);
                use crate::components::chooser::{ChooserItem, ChooserState};
                let mut items: Vec<ChooserItem> = Vec::new();
                for path in &self.registry.known {
                    if !path.is_dir() {
                        self.toasts.push(
                            format!("{} no longer exists; skipped", path.display()),
                            ToastKind::Warning,
                        );
                        continue;
                    }
                    let label = match postui_core::project::load_meta(path) {
                        Ok(meta) => postui_core::project::display_name(path, &meta),
                        Err(_) => postui_core::project::display_name(
                            path,
                            &postui_core::project::ProjectMeta::default(),
                        ),
                    };
                    items.push(ChooserItem {
                        label,
                        detail: Some(path.display().to_string()),
                        actions: vec![Action::SwitchProject(path.clone())],
                    });
                }
                items.push(ChooserItem {
                    label: "open by path…".into(),
                    detail: None,
                    actions: vec![Action::PromptOpenProjectPath],
                });
                self.modals
                    .push(Modal::Chooser(ChooserState::new("Projects", items)));
                true
            }
            Action::CycleProject => {
                match self.registry.next_after(&self.project.root) {
                    None => {
                        self.toasts
                            .push("only one project registered", ToastKind::Warning);
                    }
                    Some(target) => {
                        self.apply(Action::SwitchProject(target));
                    }
                }
                true
            }
            Action::SwitchProject(target) => {
                if target == self.project.root {
                    return false;
                }
                if self.editor.is_dirty() {
                    self.dirty_gate("switch", Action::ForceSwitchProject(target));
                } else {
                    self.apply(Action::ForceSwitchProject(target));
                }
                true
            }
            Action::ForceSwitchProject(target) => {
                self.project
                    .persist_local_state(self.editor.slug.as_deref());
                // Slugs are project-relative: a cached response carried
                // across the switch could resurface under an unrelated
                // request with the same slug.
                self.session.reset();
                let name = postui_core::project::load_meta(&target)
                    .map(|meta| postui_core::project::display_name(&target, &meta))
                    .unwrap_or_else(|_| {
                        postui_core::project::display_name(
                            &target,
                            &postui_core::project::ProjectMeta::default(),
                        )
                    });
                let (project, warnings) = ProjectContext::open(target.clone());
                self.project = project;
                for w in warnings {
                    self.toasts.push(w, ToastKind::Warning);
                }
                match postui_core::storage::ensure_project(&self.project.root) {
                    Ok(()) => self.refresh_sidebar(),
                    Err(e) => {
                        self.toasts
                            .push(format!("could not open project: {e}"), ToastKind::Error);
                    }
                }
                match self.project.local_open_request() {
                    Some(slug)
                        if postui_core::storage::load_request(&self.project.root, &slug)
                            .is_ok() =>
                    {
                        self.apply(Action::ForceOpenRequest(slug));
                    }
                    _ => {
                        self.editor = Editor::default();
                    }
                }
                self.registry.register(target);
                if let Some(path) = &self.registry_path {
                    let _ = self.registry.save_to(path);
                }
                self.toasts
                    .push(format!("Switched to {name}"), ToastKind::Success);
                true
            }
            Action::PromptOpenProjectPath => {
                self.modals.push(Modal::Prompt {
                    title: "Open project at path".into(),
                    input: crate::components::line_input::LineInput::new(""),
                    kind: PromptKind::OpenProjectPath,
                });
                true
            }
            Action::OpenProjectByPath(text) => {
                let path = crate::config::expand_tilde(&text);
                if postui_core::project::is_project(&path) {
                    self.apply(Action::SwitchProject(path));
                } else {
                    let display = path.display().to_string();
                    self.modals.push(Modal::Confirm {
                        title: "Not a postui project".into(),
                        body: format!("create project at {display}?"),
                        choices: vec![
                            ('y', "Create".into(), vec![Action::CreateProjectAt(path)]),
                            ('n', "Cancel".into(), vec![]),
                        ],
                    });
                }
                true
            }
            Action::CreateProjectAt(path) => {
                if let Err(e) = postui_core::project::init_project(&path, None) {
                    self.toasts.push(
                        format!("could not create project at {}: {e}", path.display()),
                        ToastKind::Error,
                    );
                    return true;
                }
                self.apply(Action::ForceSwitchProject(path))
            }
            Action::PromptNewProject => {
                let prefill = format!("{}/", self.registry.default_root().display());
                self.modals.push(Modal::NewProject {
                    name: crate::components::line_input::LineInput::new(""),
                    path: crate::components::line_input::LineInput::new(&prefill),
                    on_path: false,
                    prefilled: false,
                });
                true
            }
            Action::CreateProject { name, path } => {
                if path.trim().is_empty() {
                    self.toasts
                        .push("project path is empty — enter a path", ToastKind::Error);
                    return true;
                }
                let path = crate::config::expand_tilde(&path);
                if let Err(e) = postui_core::project::init_project(&path, Some(&name)) {
                    self.toasts.push(
                        format!("could not create project at {}: {e}", path.display()),
                        ToastKind::Error,
                    );
                    return true;
                }
                self.registry.add_known(path.clone());
                if let Some(p) = &self.registry_path {
                    let _ = self.registry.save_to(p);
                }
                if self.editor.is_dirty() {
                    self.dirty_gate("create", Action::ForceSwitchProject(path));
                } else {
                    self.apply(Action::ForceSwitchProject(path));
                }
                true
            }
            Action::OpenEnvChooser => {
                self.apply(Action::ReloadProjectFiles);
                use crate::components::chooser::{ChooserItem, ChooserState};
                self.project.environments =
                    postui_core::project::list_environments(&self.project.root);
                if self.project.environments.is_empty() {
                    self.toasts.push(
                        "no environments — create environments/<name>.toml in the project",
                        ToastKind::Warning,
                    );
                    return true;
                }
                let mut items: Vec<ChooserItem> = self
                    .project
                    .environments
                    .iter()
                    .map(|name| ChooserItem {
                        label: name.clone(),
                        detail: None,
                        actions: vec![Action::SwitchEnv(Some(name.clone()))],
                    })
                    .collect();
                items.push(ChooserItem {
                    label: "no environment".into(),
                    detail: None,
                    actions: vec![Action::SwitchEnv(None)],
                });
                self.modals
                    .push(Modal::Chooser(ChooserState::new("Environments", items)));
                true
            }
            Action::CycleEnv => {
                self.apply(Action::ReloadProjectFiles);
                self.project.environments =
                    postui_core::project::list_environments(&self.project.root);
                if self.project.environments.is_empty() {
                    self.toasts.push(
                        "no environments — create environments/<name>.toml in the project",
                        ToastKind::Warning,
                    );
                    return true;
                }
                let next = match &self.project.active_env {
                    None => self.project.environments[0].clone(),
                    Some(current) => {
                        let idx = self.project.environments.iter().position(|e| e == current);
                        match idx {
                            Some(i) => self.project.environments
                                [(i + 1) % self.project.environments.len()]
                            .clone(),
                            None => self.project.environments[0].clone(),
                        }
                    }
                };
                self.apply(Action::SwitchEnv(Some(next)))
            }
            Action::SwitchEnv(env) => {
                let warnings = self.project.set_env(env);
                if !warnings.is_empty() {
                    for w in warnings {
                        self.toasts.push(w, ToastKind::Warning);
                    }
                    return true;
                }
                self.apply(Action::PersistLocalState);
                let label = self.project.env_label();
                self.toasts
                    .push(format!("env: {label}"), ToastKind::Success);
                true
            }
            Action::ReloadProjectFiles => {
                let (changed, warnings) = self.project.reload_if_changed();
                if changed {
                    self.refresh_sidebar();
                }
                for w in warnings {
                    self.toasts.push(w, ToastKind::Warning);
                }
                changed
            }
            Action::OpenVarPicker { completing } => {
                self.apply(Action::ReloadProjectFiles);
                if self.project.model.vars.is_empty() {
                    self.toasts.push(
                        "no variables declared — edit variables.toml",
                        ToastKind::Warning,
                    );
                    return true;
                }
                let resolved = self.project.prepare_context().vars;
                use crate::components::modal::Modal;
                use crate::components::var_picker::{VarEntry, VarPickerState};
                let entries: Vec<VarEntry> = self
                    .project
                    .model
                    .vars
                    .keys()
                    .map(|name| VarEntry {
                        name: name.clone(),
                        description: self.project.model.vars[name].description.clone(),
                        value: resolved.get(name).cloned(),
                    })
                    .collect();
                self.modals
                    .push(Modal::VarPicker(VarPickerState::new(entries, completing)));
                true
            }
            Action::InsertVarText(text) => {
                if self.focus == PaneId::Editor && self.editor.sub_focus == SubFocus::Url {
                    self.editor.url.insert_str(&text);
                } else if self.focus == PaneId::Editor
                    && matches!(
                        self.editor.active_tab,
                        EditorTab::Params | EditorTab::Headers
                    )
                    && self.editor.sub_focus == SubFocus::Content
                    && self.editor.table.editing.is_some()
                {
                    let edit = self.editor.table.editing.as_mut().unwrap();
                    edit.input.insert_str(&text);
                } else if self.focus == PaneId::Editor
                    && self.editor.active_tab == EditorTab::Body
                    && self.editor.sub_focus == SubFocus::Content
                {
                    self.editor.body_insert_str(&text);
                    if !self.editor.substitute_body {
                        self.editor.substitute_body = true;
                        self.toasts
                            .push("body {{var}} substitution enabled", ToastKind::Success);
                    }
                } else {
                    self.toasts.push(
                        "nowhere to insert — focus a text field first",
                        ToastKind::Warning,
                    );
                }
                true
            }
            Action::OpenVarManager => {
                if self.screen != Screen::VarManager {
                    self.prior_focus = self.focus;
                    self.screen = Screen::VarManager;
                }
                true
            }
            Action::CloseScreen => {
                self.screen = Screen::Main;
                self.focus = self.prior_focus;
                true
            }
        }
    }

    /// Re-reads the project directory and rebuilds the sidebar tree,
    /// merging any ancestor folders `select_slug` needs opened into
    /// `project.expanded` first. Replaces every previous
    /// `list_requests` + `sidebar.refresh` pair so the tree/expansion
    /// state stays consistent at every call site.
    fn refresh_sidebar(&mut self) {
        let (listing, walk_err) = postui_core::storage::list_requests(&self.project.root);
        if let Some(e) = walk_err {
            self.toasts.push(
                format!("could not fully list requests: {e}"),
                ToastKind::Error,
            );
        }
        self.project
            .expanded
            .append(&mut self.sidebar.pending_expand);
        let expanded = self.project.expanded.clone();
        self.sidebar.refresh(listing, &expanded);
    }

    /// Push the standard unsaved-changes confirm whose "save" path relies on
    /// SaveRequest completing synchronously (dirty implies a slugged request).
    fn dirty_gate(&mut self, verb: &str, then: Action) {
        let current = self.editor.slug.clone().unwrap_or_default();
        self.modals.push(Modal::Confirm {
            title: "Unsaved changes".into(),
            body: format!("\"{current}\" has unsaved changes."),
            choices: vec![
                (
                    's',
                    format!("Save & {verb}"),
                    vec![Action::SaveRequest, then.clone()],
                ),
                ('d', "Discard changes".into(), vec![then]),
            ],
        });
    }

    /// Shared validate/exists-check/save/refresh/open path for `CreateRequest`
    /// and `SaveRequestAs`: both save a fresh `HttpRequest` (a default one, or
    /// the editor's current one) to a brand-new slug and switch the editor
    /// over to it. `build` receives the slug in case a future caller needs it;
    /// today's callers ignore it.
    fn create_or_save_as(
        &mut self,
        name: &str,
        build: impl FnOnce(&str) -> postui_core::model::HttpRequest,
    ) {
        if postui_core::storage::validate_slug(name).is_err() {
            self.toasts.push(
                "invalid name: lowercase letters, digits, - _ and / only",
                ToastKind::Error,
            );
            return;
        }
        if postui_core::storage::request_exists(&self.project.root, name) {
            self.toasts.push(
                format!("request already exists: {name:?}"),
                ToastKind::Error,
            );
            return;
        }
        let req = build(name);
        match postui_core::storage::save_request(&self.project.root, name, &req) {
            Ok(()) => {
                self.editor.load(Some(name.to_string()), req);
                self.editor.mark_saved();
                self.toasts
                    .push(format!("Saved {name}"), ToastKind::Success);
                // Queue name's ancestor folders open, rebuild the tree with
                // them expanded (so the new row exists at all), then select
                // it now that it's actually visible.
                self.sidebar.select_slug(name);
                self.refresh_sidebar();
                self.sidebar.select_slug(name);
                self.apply(Action::PersistLocalState);
            }
            Err(e) => {
                self.toasts
                    .push(format!("could not save {name}: {e}"), ToastKind::Error);
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
                self.toasts
                    .push(e.to_string(), crate::components::toast::ToastKind::Error);
                true
            }
        }
    }

    /// Resolves `target` to the text to copy and the success toast to show
    /// on a successful copy. `None` when `target` needs a ready response
    /// and there isn't one (or the header index is out of range).
    fn resolve_copy(&self, target: &CopyTarget) -> Option<(String, String)> {
        match target {
            CopyTarget::ResponseBody => match self.session.response.state() {
                ResponseState::Ready(d) => {
                    Some((d.body.clone(), "Copied response body".to_string()))
                }
                _ => None,
            },
            CopyTarget::ResponseHeader(i) => match self.session.response.state() {
                ResponseState::Ready(d) => d
                    .headers
                    .get(*i)
                    .map(|(name, value)| (value.clone(), format!("Copied {name}"))),
                _ => None,
            },
            CopyTarget::Url => Some((self.editor.url.text().to_string(), "Copied URL".to_string())),
        }
    }

    /// Whether any in-flight HTTP request is still ticking (e.g. animating
    /// a spinner) and therefore needs a redraw.
    fn in_flight_ticking(&self) -> bool {
        self.session.in_flight.is_some()
    }

    /// Central key router. Order (each step tested):
    /// 1. A CTRL/ALT combo the keymap maps to Quit pre-empts everything,
    ///    including open modals — ctrl+c must always quit.
    /// 2. An open modal stack captures all remaining input (swallowed keys
    ///    still count as "handled" — they return true).
    /// 3. With no modal open and a non-`Main` screen showing (e.g. the
    ///    Variable Manager), that screen captures all remaining input like
    ///    a modal does: only the small [`screen_escape_whitelist`] of
    ///    global actions (opening the palette on top of the screen,
    ///    quitting, and the screen open/close actions themselves) can
    ///    still fire on a CTRL/ALT combo; every other global shortcut
    ///    (send, save, cycle project/env, focus URL, …) is *not* reachable
    ///    from here, since none of it is meaningful with the panes it
    ///    targets not even drawn. Anything the screen's own component
    ///    doesn't claim is swallowed rather than falling through to the
    ///    global keymap, so e.g. plain `q` does not quit the app from a
    ///    non-`Main` screen.
    /// 4. On `Screen::Main`, a CTRL/ALT combo prefers the global keymap
    ///    over the focused component (app shortcuts beat editors), falling
    ///    through to the component if unbound.
    /// 5. Plain keys (and unbound modified ones) go to the focused
    ///    component first.
    /// 6. Anything the component ignores falls back to the global keymap.
    ///
    /// Returns whether an action was applied or a modal consumed the key
    /// (i.e. whether the caller should redraw): the OR of every
    /// `self.update(..)` call's result along the branch taken, plus any
    /// modal state change (close/typing) that bypasses `update`.
    pub fn handle_key(&mut self, keymap: &Keymap, ev: KeyEvent) -> bool {
        let combo = KeyCombo::from_event(&ev);
        let global = keymap.lookup(&combo);
        let modified = ev
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

        // 1. A modified quit combo is the escape hatch: it pre-empts everything.
        if modified && global == Some(Action::Quit) {
            return self.update(Action::Quit);
        }

        // 2. Modals capture all remaining input.
        if !self.modals.is_empty() {
            let Some(res) = self.modals.handle_key(ev) else {
                return true; // typed into modal
            };
            return self.apply_modal_result(res);
        }

        // 3. A non-Main screen captures all remaining input, like a modal —
        // except the small whitelist of global actions that legitimately
        // work "on top of" any screen still gets first refusal.
        if self.screen != Screen::Main {
            if modified
                && let Some(a) = global.clone()
                && screen_escape_whitelist(&a)
            {
                return self.update(a);
            }
            if let Some(a) = self.varmanager.handle_key(ev) {
                return self.update(a);
            }
            return true; // swallowed: no fallback to the global keymap
        }

        // 4. Modified combos prefer the global keymap (app shortcuts beat editors).
        if modified && let Some(a) = global {
            return self.update(a);
        }

        // 5. The focused component gets plain keys (and unbound modified ones) next.
        if let Some(a) = self.focused_component_key(ev) {
            return self.update(a);
        }

        // 6. Global fallback for plain keys the component ignored.
        if let Some(a) = global {
            return self.update(a);
        }

        false
    }

    fn focused_component_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match self.focus {
            PaneId::Sidebar => self.sidebar.handle_key(ev),
            PaneId::Editor => self.editor.handle_key(ev),
            PaneId::Response => self.session.response.handle_key(ev),
        }
    }

    /// Pops the top modal on `close` and dispatches each of `res`'s
    /// actions, exactly like the key path used to inline — shared by
    /// `handle_key`'s modal branch and `on_hit`'s modal click arms so a
    /// click and the equivalent keypress can never disagree.
    fn apply_modal_result(&mut self, res: ModalResult) -> bool {
        let mut changed = res.close;
        if res.close {
            self.modals.pop();
        }
        if let Some(id) = &res.usage {
            self.usage.record(id, crate::usage::now());
        }
        for a in res.actions {
            changed |= self.update(a);
        }
        changed
    }
}

/// The only global actions a modified (ctrl/alt) combo may still trigger
/// while a non-`Main` screen (e.g. the Variable Manager) has captured
/// input: opening a modal on top of the screen (today, just the command
/// palette — the spec's "the modal stack works on top unchanged"), the
/// screen open/close actions themselves, and quit. Everything else in the
/// global keymap (send, save, cycle project/env, focus URL, …) targets
/// panes that aren't even drawn while a non-`Main` screen is open, so it
/// must not be reachable from here — see the Task 9 review finding this
/// whitelist fixes: an unbounded carve-out let ctrl+enter send the loaded
/// request invisibly, alt+u silently reassign focus, etc.
fn screen_escape_whitelist(action: &Action) -> bool {
    matches!(
        action,
        Action::OpenPalette | Action::OpenVarManager | Action::CloseScreen | Action::Quit
    )
}

mod mouse;
#[cfg(test)]
mod tests;
