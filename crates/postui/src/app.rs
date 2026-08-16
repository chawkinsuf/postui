use crate::action::Action;
use crate::components::editor::{Editor, EditorTab, SubFocus};
use crate::components::modal::{Modal, ModalStack, PromptKind};
use crate::components::response::ResponseState;
use crate::components::sidebar::Row;
use crate::components::toast::{ToastKind, Toasts};
use crate::components::{Component, response::Response, sidebar::Sidebar};
use crate::keys::{KeyCombo, Keymap};
use crate::layout::PaneId;
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Bookkeeping for a request currently in flight: when it started (for a
/// future spinner), which generation it belongs to (so a stale result can
/// be told apart from the current one), and the task itself (so it can be
/// aborted on cancel or on a newer send superseding it).
pub struct InFlight {
    pub started: Instant,
    pub generation: u64,
    pub task: tokio::task::JoinHandle<()>,
}

pub struct App {
    pub should_quit: bool,
    pub focus: PaneId,
    pub theme: Theme,
    pub sidebar: Sidebar,
    pub editor: Editor,
    pub response: Response,
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
    /// The HTTP client used for every send. Built eagerly and cheaply
    /// (`reqwest::Client::builder().build()` needs no running Tokio
    /// reactor — verified in `http::tests::client_builds_without_a_tokio_runtime`
    /// — so `App` stays constructible in the many plain `#[test]`s that
    /// never touch the network).
    pub client: reqwest::Client,
    /// The currently in-flight send, if any.
    pub in_flight: Option<InFlight>,
    /// Bumped on every `ForceSend`; tags each spawned send so a result that
    /// arrives after a newer send has started can be told apart and dropped.
    pub send_generation: u64,
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
    OpenAsIs,
}

/// Picks the startup root and what to do about it, given the registry, an
/// optional CLI-supplied root, and the platform's default project
/// directory. Only touches the filesystem via `is_project`'s single-file
/// check, so it's covered directly by the unit tests below rather than
/// through `App::new` (which reads the real user config file).
///
/// Precedence: `cli_root`, then the registry's last-used project, then the
/// first known project that still exists on disk, then `default_dir`.
/// `None` means no candidate root exists at all.
fn resolve_startup(
    registry: &crate::config::ProjectsRegistry,
    cli_root: Option<PathBuf>,
    default_dir: Option<PathBuf>,
) -> Option<(PathBuf, StartupDisposition)> {
    if let Some(root) = cli_root {
        let disposition = if postui_core::project::is_project(&root) {
            StartupDisposition::OpenAsIs
        } else {
            StartupDisposition::PromptCreate
        };
        return Some((root, disposition));
    }
    if let Some(root) = registry.last.clone() {
        return Some((root, StartupDisposition::OpenAsIs));
    }
    if let Some(root) = registry.known.iter().find(|p| p.is_dir()).cloned() {
        return Some((root, StartupDisposition::OpenAsIs));
    }
    if let Some(root) = default_dir {
        let disposition = if postui_core::project::is_project(&root) {
            StartupDisposition::OpenAsIs
        } else {
            StartupDisposition::InitDefault
        };
        return Some((root, disposition));
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

        let Some((root, disposition)) = resolve_startup(
            &registry,
            cli_root,
            postui_core::storage::default_project_dir(),
        ) else {
            let mut app = Self::bare(tx, PathBuf::new());
            app.registry = registry;
            app.registry_path = registry_path;
            app.toasts.push(
                "could not determine a project directory for this platform",
                ToastKind::Error,
            );
            return app;
        };

        let mut app = Self::with_root(tx, root);
        app.registry = registry;
        app.registry_path = registry_path;

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
            StartupDisposition::OpenAsIs => {
                if !postui_core::project::is_project(&app.project.root) {
                    app.toasts.push(
                        format!(
                            "{} has no project.toml; opened as a bare directory",
                            app.project.root.display()
                        ),
                        ToastKind::Warning,
                    );
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
            theme: Theme::for_terminal(),
            sidebar: Sidebar::default(),
            editor: Editor::default(),
            response: Response::default(),
            toasts,
            modals: ModalStack::default(),
            project,
            registry: crate::config::ProjectsRegistry::default(),
            registry_path: None,
            client: crate::http::client(),
            in_flight: None,
            send_generation: 0,
            tx,
            pending_terminal_action: None,
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
                self.project
                    .persist_local_state(self.editor.slug.as_deref());
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
                    Ok(req) => self.editor.load(Some(slug), req),
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
                    Err(postui_core::prepare::PrepareError::Unresolved(names)) => {
                        let label = self.project.env_label();
                        let list = names.into_iter().collect::<Vec<_>>().join(", ");
                        self.toasts.push(
                            format!("unresolved variables ({label}): {list}"),
                            ToastKind::Error,
                        );
                        return true;
                    }
                };
                for w in &warnings {
                    self.toasts.push(w.to_string(), ToastKind::Warning);
                }
                if let Some(prev) = self.in_flight.take() {
                    prev.task.abort();
                }
                self.send_generation += 1;
                let generation = self.send_generation;
                self.response.set_state(ResponseState::InFlight {
                    started: Instant::now(),
                });
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
                self.in_flight = Some(InFlight {
                    started: Instant::now(),
                    generation,
                    task,
                });
                true
            }
            Action::CancelSend => match self.in_flight.take() {
                Some(inflight) => {
                    inflight.task.abort();
                    // Bump the generation too, not just abort the task: the
                    // task may have already raced past the abort point and
                    // queued a ResponseArrived/RequestFailed for the old
                    // generation. Without this, that stale result would
                    // still pass the `generation == self.send_generation`
                    // staleness check and silently overwrite Cancelled.
                    self.send_generation += 1;
                    self.response.set_state(ResponseState::Cancelled);
                    true
                }
                None => false,
            },
            Action::ResponseArrived { generation, data } => {
                if generation != self.send_generation {
                    return false; // stale: a newer send has already superseded it
                }
                self.in_flight = None;
                self.response.set_state(ResponseState::Ready(data));
                true
            }
            Action::RequestFailed { generation, error } => {
                if generation != self.send_generation {
                    return false; // stale, see above
                }
                self.in_flight = None;
                self.response.set_state(ResponseState::Failed(error));
                true
            }
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
                        let name = postui_core::project::load_meta(&target)
                            .map(|meta| postui_core::project::display_name(&target, &meta))
                            .unwrap_or_else(|_| {
                                postui_core::project::display_name(
                                    &target,
                                    &postui_core::project::ProjectMeta::default(),
                                )
                            });
                        self.apply(Action::SwitchProject(target));
                        self.toasts
                            .push(format!("Switched to {name}"), ToastKind::Success);
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
                let path = crate::config::expand_tilde(&path);
                if let Err(e) = postui_core::project::init_project(&path, Some(&name)) {
                    self.toasts.push(
                        format!("could not create project at {}: {e}", path.display()),
                        ToastKind::Error,
                    );
                    return true;
                }
                self.registry.register(path.clone());
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
                for w in warnings {
                    self.toasts.push(w, ToastKind::Warning);
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
        }
    }

    /// Re-reads the project directory and rebuilds the sidebar tree,
    /// merging any ancestor folders `select_slug` needs opened into
    /// `project.expanded` first. Replaces every previous
    /// `list_requests` + `sidebar.refresh` pair so the tree/expansion
    /// state stays consistent at every call site.
    fn refresh_sidebar(&mut self) {
        let listing = postui_core::storage::list_requests(&self.project.root);
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
        let existing = postui_core::storage::list_requests(&self.project.root);
        if existing.iter().any(|l| l.slug == name) {
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

    /// Whether any in-flight HTTP request is still ticking (e.g. animating
    /// a spinner) and therefore needs a redraw.
    fn in_flight_ticking(&self) -> bool {
        self.in_flight.is_some()
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
    fn resolve_startup_fresh_install_picks_default_dir_to_init_with_no_prompt() {
        let registry = crate::config::ProjectsRegistry::default();
        let default_dir = PathBuf::from("/nonexistent/postui-default-xyz");
        let (root, disposition) =
            resolve_startup(&registry, None, Some(default_dir.clone())).unwrap();
        assert_eq!(root, default_dir);
        assert_eq!(disposition, StartupDisposition::InitDefault);
    }

    #[test]
    fn resolve_startup_cli_non_project_root_prompts_create() {
        let dir = tempfile::tempdir().unwrap();
        let registry = crate::config::ProjectsRegistry::default();
        let (root, disposition) =
            resolve_startup(&registry, Some(dir.path().to_path_buf()), None).unwrap();
        assert_eq!(root, dir.path());
        assert_eq!(disposition, StartupDisposition::PromptCreate);
    }

    #[test]
    fn resolve_startup_registry_last_wins_over_known() {
        let registry = crate::config::ProjectsRegistry {
            known: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            last: Some(PathBuf::from("/c")),
            ..Default::default()
        };
        let (root, disposition) = resolve_startup(&registry, None, None).unwrap();
        assert_eq!(root, PathBuf::from("/c"));
        assert_eq!(disposition, StartupDisposition::OpenAsIs);
    }

    #[test]
    fn resolve_startup_cli_beats_registry_last() {
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), None).unwrap();
        let registry = crate::config::ProjectsRegistry {
            last: Some(PathBuf::from("/elsewhere")),
            ..Default::default()
        };
        let (root, disposition) =
            resolve_startup(&registry, Some(dir.path().to_path_buf()), None).unwrap();
        assert_eq!(root, dir.path());
        assert_eq!(disposition, StartupDisposition::OpenAsIs);
    }

    #[test]
    fn resolve_startup_uses_first_existing_known_when_no_last() {
        let dir_a = tempfile::tempdir().unwrap();
        let registry = crate::config::ProjectsRegistry {
            known: vec![PathBuf::from("/nonexistent-a"), dir_a.path().to_path_buf()],
            ..Default::default()
        };
        let (root, disposition) = resolve_startup(&registry, None, None).unwrap();
        assert_eq!(root, dir_a.path());
        assert_eq!(disposition, StartupDisposition::OpenAsIs);
    }

    #[test]
    fn resolve_startup_returns_none_when_nothing_available() {
        let registry = crate::config::ProjectsRegistry::default();
        assert!(resolve_startup(&registry, None, None).is_none());
    }

    #[test]
    fn init_project_here_creates_project_toml_at_current_root() {
        let mut app = App::new_for_test();
        assert!(!postui_core::project::is_project(&app.project.root));
        app.update(Action::InitProjectHere);
        assert!(postui_core::project::is_project(&app.project.root));
    }

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
        app.modals.push(Modal::Message {
            title: "t".into(),
            body: "b".into(),
        });
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
        app.update(Action::ShowToast(
            "hi".into(),
            crate::components::toast::ToastKind::Info,
        ));
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
        postui_core::storage::save_request(dir.path(), "auth/login", &req("https://x/login"))
            .unwrap();
        postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
        let mut app = App::with_root(tx, dir.path().to_path_buf());

        assert_eq!(
            app.sidebar.rows,
            vec![
                Row::Request {
                    slug: "ping".into(),
                    depth: 0,
                    broken: None
                },
                Row::Folder {
                    path: "auth".into(),
                    name: "auth".into(),
                    depth: 0,
                    expanded: false,
                },
            ]
        );

        // "ping" (index 0) -> "auth" folder (index 1): expand it, then
        // "auth/login" (index 2) becomes visible and Enter opens it.
        let keymap = Keymap::default_bindings();
        app.handle_key(&keymap, plain('j'));
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(&keymap, plain('j'));
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
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
        assert_eq!(
            app.editor.slug.as_deref(),
            Some("a"),
            "still on the original request"
        );

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
        assert_eq!(
            saved.url, "https://x/a/",
            "the edit was persisted before opening b"
        );
    }

    #[test]
    fn broken_file_shows_marker_and_error_modal() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        postui_core::storage::ensure_project(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("requests/bad.toml"),
            "url = \"x\"\nurl = \"dup\"\n",
        )
        .unwrap();
        let mut app = App::with_root(tx, dir.path().to_path_buf());

        let Row::Request { broken, .. } = &app.sidebar.rows[0] else {
            panic!("expected a request row")
        };
        assert!(broken.is_some());

        app.handle_key(
            &Keymap::default_bindings(),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        match app.modals.top() {
            Some(Modal::Message { body, .. }) => {
                assert!(body.contains('2') || body.to_lowercase().contains("duplicate"));
            }
            _ => panic!("expected a Message modal"),
        }
    }

    #[test]
    fn dirty_dot_renders_in_sidebar() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

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
        assert!(
            content.contains('\u{25cf}'),
            "expected a dirty dot in the sidebar: {content}"
        );
    }

    #[test]
    fn new_request_prompt_flow_creates_file_and_opens_it() {
        let mut app = App::new_for_test();
        let keymap = Keymap::default_bindings();
        app.focus = PaneId::Sidebar;
        app.handle_key(&keymap, plain('n'));
        assert!(matches!(
            app.modals.top(),
            Some(Modal::Prompt {
                kind: PromptKind::NewRequest,
                ..
            })
        ));
        for c in "api/ping".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modals.is_empty());
        assert_eq!(app.editor.slug.as_deref(), Some("api/ping"));
        assert!(postui_core::storage::load_request(&app.project.root, "api/ping").is_ok());
        assert!(
            app.sidebar
                .rows
                .iter()
                .any(|r| matches!(r, Row::Request { slug, .. } if slug == "api/ping")),
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
        assert!(
            app.modals.is_empty(),
            "modal closes even though the save is rejected"
        );
        assert!(!app.toasts.is_empty(), "an invalid name must toast");
        assert!(postui_core::storage::list_requests(&app.project.root).is_empty());
    }

    #[test]
    fn rename_request_updates_disk_and_open_slug() {
        let mut app = App::new_for_test();
        postui_core::storage::save_request(&app.project.root, "old", &req("https://x/old"))
            .unwrap();
        app.update(Action::RefreshSidebar);
        app.update(Action::ForceOpenRequest("old".into()));
        let keymap = Keymap::default_bindings();
        app.focus = PaneId::Sidebar;
        app.handle_key(&keymap, plain('r'));
        match app.modals.top() {
            Some(Modal::Prompt {
                kind: PromptKind::RenameRequest { from },
                ..
            }) => {
                assert_eq!(from, "old");
            }
            _ => panic!("expected a RenameRequest prompt"),
        }
        for _ in 0.."old".len() {
            app.handle_key(
                &keymap,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            );
        }
        for c in "new".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modals.is_empty());
        assert!(postui_core::storage::load_request(&app.project.root, "old").is_err());
        assert!(postui_core::storage::load_request(&app.project.root, "new").is_ok());
        assert_eq!(app.editor.slug.as_deref(), Some("new"));
        assert_eq!(app.sidebar.open_slug.as_deref(), Some("new"));
    }

    #[test]
    fn delete_open_request_clears_editor_and_removes_file() {
        let mut app = App::new_for_test();
        postui_core::storage::save_request(&app.project.root, "gone", &req("https://x/gone"))
            .unwrap();
        app.update(Action::RefreshSidebar);
        app.update(Action::ForceOpenRequest("gone".into()));
        let keymap = Keymap::default_bindings();
        app.focus = PaneId::Sidebar;
        app.handle_key(&keymap, plain('d'));
        assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
        app.handle_key(&keymap, plain('y'));
        assert!(app.modals.is_empty());
        assert!(
            app.editor.slug.is_none(),
            "editor must reset once its open request is deleted"
        );
        assert!(postui_core::storage::load_request(&app.project.root, "gone").is_err());
    }

    #[test]
    fn save_with_no_slug_opens_save_as_prompt() {
        let mut app = App::new_for_test();
        app.editor.url = crate::components::line_input::LineInput::new("https://x/new");
        let keymap = Keymap::default_bindings();
        app.update(Action::SaveRequest);
        assert!(matches!(
            app.modals.top(),
            Some(Modal::Prompt {
                kind: PromptKind::SaveAs,
                ..
            })
        ));
        for c in "fresh".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.modals.is_empty());
        assert_eq!(app.editor.slug.as_deref(), Some("fresh"));
        let saved = postui_core::storage::load_request(&app.project.root, "fresh").unwrap();
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

    #[tokio::test]
    async fn send_with_invalid_body_prompts_first() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9"); // unroutable, never actually hit
        app.editor.set_body_text("{oops");
        app.update(Action::Send);
        assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
        assert!(app.in_flight.is_none());
    }

    #[tokio::test]
    async fn stale_generation_results_are_ignored() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.send_generation = 5;
        app.update(Action::RequestFailed {
            generation: 4,
            error: "old".into(),
        });
        assert!(
            matches!(app.response.state(), ResponseState::Empty),
            "stale result dropped"
        );
    }

    #[tokio::test]
    async fn empty_url_toasts_instead_of_sending() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.update(Action::Send);
        assert!(app.in_flight.is_none());
        assert!(
            !app.toasts.is_empty(),
            "empty URL must toast rather than send"
        );
    }

    #[tokio::test]
    async fn force_send_with_empty_url_toasts_and_does_not_spawn() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.update(Action::ForceSend);
        assert!(
            app.in_flight.is_none(),
            "no task should be spawned for an empty URL"
        );
        assert!(
            !app.toasts.is_empty(),
            "empty URL must toast even via ForceSend directly"
        );
        assert_eq!(
            app.send_generation, 0,
            "generation must not advance without a send"
        );
    }

    #[tokio::test]
    async fn force_send_spawns_a_task_and_marks_response_in_flight() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9"); // unroutable, never actually hit
        app.update(Action::ForceSend);
        assert!(app.in_flight.is_some());
        assert!(matches!(
            app.response.state(),
            ResponseState::InFlight { .. }
        ));
        assert_eq!(app.send_generation, 1);
    }

    #[tokio::test]
    async fn cancel_send_aborts_task_and_marks_cancelled() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
        app.update(Action::ForceSend);
        assert!(app.in_flight.is_some());
        app.update(Action::CancelSend);
        assert!(app.in_flight.is_none());
        assert!(matches!(app.response.state(), ResponseState::Cancelled));
        // no-op when nothing is in flight
        assert!(!app.update(Action::CancelSend));
    }

    #[tokio::test]
    async fn cancelled_send_ignores_a_result_that_was_already_queued() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
        app.update(Action::ForceSend);
        let generation = app.send_generation;
        app.update(Action::CancelSend);
        assert!(matches!(app.response.state(), ResponseState::Cancelled));

        // Simulate the in-flight task's result landing after cancellation,
        // still tagged with the generation it was spawned under.
        let data = crate::http::ResponseData {
            status: 200,
            headers: vec![],
            body: "late".into(),
            elapsed: std::time::Duration::from_millis(1),
            size: 4,
            content_type: None,
        };
        app.update(Action::ResponseArrived {
            generation,
            data: Box::new(data),
        });
        assert!(
            matches!(app.response.state(), ResponseState::Cancelled),
            "a result racing the cancel must not overwrite it"
        );
    }

    #[tokio::test]
    async fn response_arrived_with_current_generation_clears_in_flight() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.send_generation = 1;
        let data = crate::http::ResponseData {
            status: 200,
            headers: vec![],
            body: "ok".into(),
            elapsed: std::time::Duration::from_millis(1),
            size: 2,
            content_type: None,
        };
        app.update(Action::ResponseArrived {
            generation: 1,
            data: Box::new(data.clone()),
        });
        assert!(app.in_flight.is_none());
        assert!(matches!(app.response.state(), ResponseState::Ready(d) if **d == data));
    }

    #[test]
    fn esc_on_in_flight_response_pane_requests_cancel() {
        let mut app = App::new_for_test();
        app.response.set_state(ResponseState::InFlight {
            started: std::time::Instant::now(),
        });
        let action = app
            .response
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, Some(Action::CancelSend));
    }

    #[test]
    fn plain_keys_reach_the_focused_response_pane() {
        let mut app = App::new_for_test();
        app.response
            .set_state(ResponseState::Ready(Box::new(crate::http::ResponseData {
                status: 200,
                headers: vec![],
                body: r#"{"a": 1}"#.into(),
                elapsed: std::time::Duration::from_millis(5),
                size: 8,
                content_type: None,
            })));
        app.focus = PaneId::Response;
        let keymap = Keymap::default_bindings();
        app.handle_key(&keymap, plain('j'));
        assert_eq!(
            app.response.view().unwrap().cursor,
            1,
            "j moved the response cursor"
        );
        // 'q' quits globally, but the pane's search input takes it first.
        app.handle_key(&keymap, plain('/'));
        app.handle_key(&keymap, plain('q'));
        assert!(
            !app.should_quit,
            "a key the pane consumed must not fall through"
        );
        assert_eq!(
            app.response
                .view()
                .unwrap()
                .search
                .as_ref()
                .unwrap()
                .input
                .text(),
            "q"
        );
    }

    #[test]
    fn esc_on_idle_response_pane_does_nothing() {
        let mut app = App::new_for_test();
        let action = app
            .response
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, None);
    }

    fn two_projects() -> (App, tempfile::TempDir, tempfile::TempDir) {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        postui_core::project::init_project(a.path(), Some("alpha")).unwrap();
        postui_core::project::init_project(b.path(), Some("beta")).unwrap();
        postui_core::storage::ensure_project(b.path()).unwrap();
        postui_core::storage::save_request(b.path(), "pong", &req("https://x/pong")).unwrap();
        let mut app = App::with_root(tx, a.path().to_path_buf());
        app.registry.register(a.path().to_path_buf());
        app.registry.register(b.path().to_path_buf());
        (app, a, b)
    }

    #[test]
    fn cycle_switches_to_next_project_and_lists_its_requests() {
        let (mut app, _a, b) = two_projects();
        app.update(Action::CycleProject);
        assert_eq!(app.project.root, b.path());
        assert!(
            app.sidebar
                .rows
                .iter()
                .any(|r| matches!(r, Row::Request { slug, .. } if slug == "pong"))
        );
        assert_eq!(app.project.display_name(), "beta");
    }

    #[test]
    fn switch_with_dirty_editor_prompts_and_discard_proceeds() {
        let (mut app, _a, b) = two_projects();
        postui_core::storage::save_request(&app.project.root, "r", &req("https://x/r")).unwrap();
        app.update(Action::RefreshSidebar);
        app.update(Action::ForceOpenRequest("r".into()));
        app.focus = PaneId::Editor;
        app.editor.sub_focus = SubFocus::Url;
        app.handle_key(&Keymap::default_bindings(), plain('/'));
        assert!(app.editor.is_dirty());
        app.update(Action::SwitchProject(b.path().to_path_buf()));
        assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
        assert_ne!(app.project.root, b.path(), "not switched yet");
        app.handle_key(&Keymap::default_bindings(), plain('d'));
        assert_eq!(app.project.root, b.path());
    }

    #[test]
    fn switch_restores_target_projects_open_request_and_saves_state() {
        let (mut app, a, b) = two_projects();
        postui_core::project::save_local_state(
            b.path(),
            &postui_core::project::LocalState {
                open_request: Some("pong".into()),
                ..Default::default()
            },
        )
        .unwrap();
        app.update(Action::SwitchProject(b.path().to_path_buf()));
        assert_eq!(app.editor.slug.as_deref(), Some("pong"));
        // and the old project's state got written on the way out
        let old = postui_core::project::load_local_state(a.path()).unwrap();
        assert_eq!(old.open_request, None);
    }

    #[test]
    fn project_chooser_lists_known_and_open_by_path_creates() {
        let (mut app, _a, _b) = two_projects();
        app.update(Action::OpenProjectChooser);
        let Some(Modal::Chooser(c)) = app.modals.top() else {
            panic!("expected chooser")
        };
        assert!(
            format!("{:?}", (c.input(), c.selected_label())).contains("alpha")
                || c.selected_label().is_some()
        );
        app.update(Action::Close);
        let fresh = tempfile::tempdir().unwrap();
        let target = fresh.path().join("newproj");
        app.update(Action::OpenProjectByPath(
            target.to_string_lossy().into_owned(),
        ));
        assert!(
            matches!(app.modals.top(), Some(Modal::Confirm { .. })),
            "non-project path asks to create"
        );
        app.handle_key(&Keymap::default_bindings(), plain('y'));
        assert!(postui_core::project::is_project(&target));
        assert_eq!(app.project.root, target);
    }

    #[test]
    fn new_project_modal_prefills_path_from_name_and_creates() {
        let mut app = App::new_for_test();
        let root = tempfile::tempdir().unwrap();
        app.registry.root = Some(root.path().to_path_buf());
        let keymap = Keymap::default_bindings();
        app.update(Action::PromptNewProject);
        for c in "My Svc".chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let Some(Modal::NewProject { path, .. }) = app.modals.top() else {
            panic!()
        };
        assert!(
            path.text().ends_with("/my-svc"),
            "slugified prefill: {}",
            path.text()
        );
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let expected = root.path().join("my-svc");
        assert!(postui_core::project::is_project(&expected));
        assert_eq!(app.project.root, expected);
        assert_eq!(app.project.display_name(), "My Svc");
        assert!(app.registry.known.contains(&expected));
    }

    #[test]
    fn new_project_empty_name_swallows_enter_and_esc_cancels() {
        let mut app = App::new_for_test();
        let keymap = Keymap::default_bindings();
        app.update(Action::PromptNewProject);
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.modals.is_empty(), "empty name: modal stays");
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.modals.is_empty());
    }

    fn app_with_envs() -> (App, tempfile::TempDir) {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
        std::fs::write(dir.path().join("environments/prod.toml"), "tok = \"p\"\n").unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"q\"\n").unwrap();
        (App::with_root(tx, dir.path().to_path_buf()), dir)
    }

    #[test]
    fn cycle_env_wraps_and_skips_no_env() {
        let (mut app, dir) = app_with_envs();
        assert_eq!(app.project.env_label(), "no env");
        app.update(Action::CycleEnv);
        assert_eq!(app.project.env_label(), "prod");
        app.update(Action::CycleEnv);
        assert_eq!(app.project.env_label(), "qa");
        app.update(Action::CycleEnv);
        assert_eq!(
            app.project.env_label(),
            "prod",
            "wraps directly, never through no-env"
        );
        assert_eq!(app.project.env_values["tok"], "p");
        let st = postui_core::project::load_local_state(dir.path()).unwrap();
        assert_eq!(st.environment.as_deref(), Some("prod"), "persisted");
    }

    #[test]
    fn env_chooser_includes_no_environment_entry() {
        let (mut app, _dir) = app_with_envs();
        app.update(Action::SwitchEnv(Some("qa".into())));
        app.update(Action::OpenEnvChooser);
        let Some(Modal::Chooser(_)) = app.modals.top() else {
            panic!("expected chooser")
        };
        app.update(Action::Close);
        app.update(Action::SwitchEnv(None));
        assert_eq!(app.project.env_label(), "no env");
    }

    #[test]
    fn cycle_env_with_no_environments_toasts() {
        let mut app = App::new_for_test();
        app.update(Action::CycleEnv);
        assert!(!app.toasts.is_empty());
        assert_eq!(app.project.env_label(), "no env");
    }

    #[tokio::test]
    async fn unresolved_variable_blocks_send_with_toast() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
        app.editor.url = crate::components::line_input::LineInput::new("http://x/{{gone}}");
        app.update(Action::ForceSend);
        assert!(app.in_flight.is_none());
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn toggle_body_vars_flips_flag_and_shows_badge() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new_for_test();
        app.update(Action::ToggleBodyVars);
        assert!(app.editor.substitute_body);

        app.editor.active_tab = EditorTab::Body;
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("vars"), "expected a vars badge: {content}");
    }
}
