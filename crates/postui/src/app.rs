use crate::action::{Action, CopyTarget};
use crate::components::editor::{Editor, EditorTab, SubFocus};
use crate::components::line_input::LineInput;
use crate::components::modal::{Modal, ModalResult, ModalStack, PromptKind};
use crate::components::response::ResponseState;
use crate::components::sidebar::Row;
use crate::components::toast::{ToastKind, Toasts};
use crate::components::varmanager::{self, VarEditOp, VarManager, VarStructOp};
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

/// The migration confirm modal's title (spec §3.3) — also how
/// `prompt_migration_if_pending` recognizes its own modal already on the
/// stack.
const MIGRATION_TITLE: &str = "Migrate variables";

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
    /// Set by `Action::VarStruct`'s handler when `apply_var_struct` returns
    /// `Err` (and cleared before every action `update` dispatches), so
    /// `apply_modal_result` can stop a sequenced `ModalResult` — e.g.
    /// `PromptKind::NewVariableAndInsert`'s `[NewVar, InsertVarText]` — from
    /// running its later actions after an earlier one failed. Nothing else
    /// reads or sets this; every other action leaves it `false`.
    last_action_failed: bool,
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

    /// Offers the stage-6 → stage-7 conversion (spec §3.3) when the
    /// project that was just opened or reloaded is still in the old
    /// format. Idempotent: a confirm already on the stack isn't stacked
    /// on top of, so a reload while the modal is up doesn't duplicate it.
    fn prompt_migration_if_pending(&mut self) {
        let Some(outcome) = self.project.pending_migration() else {
            return;
        };
        if self
            .modals
            .iter()
            .any(|m| matches!(m, Modal::Confirm { title, .. } if title == MIGRATION_TITLE))
        {
            return;
        }
        let mut lines = vec![
            "This project's variables still use the old format. Convert them now? A .bak copy of each rewritten file is left beside it.".to_string(),
        ];
        // The confirm modal is a fixed, small box: list the first few
        // notes and count the rest rather than overflowing it silently
        // (every note is repeated in the toast once the migration runs).
        const SHOWN: usize = 4;
        lines.extend(
            outcome
                .notes
                .iter()
                .take(SHOWN)
                .map(|n| format!("\u{2022} {n}")),
        );
        if let Some(rest) = outcome.notes.len().checked_sub(SHOWN).filter(|n| *n > 0) {
            lines.push(format!("\u{2022} \u{2026} and {rest} more"));
        }
        self.modals.push(Modal::Confirm {
            title: MIGRATION_TITLE.into(),
            body: lines.join("\n"),
            choices: vec![
                ('n', "Not now".into(), vec![Action::DeclineMigration]),
                ('y', "Migrate".into(), vec![Action::ApplyMigration]),
            ],
        });
    }

    fn bare(tx: UnboundedSender<Action>, root: PathBuf) -> Self {
        let (project, warnings) = ProjectContext::open(root);
        let mut toasts = Toasts::default();
        for w in warnings {
            toasts.push(w, ToastKind::Warning);
        }
        let mut app = Self {
            should_quit: false,
            focus: PaneId::Sidebar,
            screen: Screen::default(),
            prior_focus: PaneId::Sidebar,
            theme: Theme::for_terminal(),
            sidebar: Sidebar::default(),
            editor: Editor::default(),
            varmanager: VarManager::default(),
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
            last_action_failed: false,
            _test_rx: None,
            _test_dir: None,
        };
        app.prompt_migration_if_pending();
        app
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
        self.editor.shadowed = self.compute_shadowed();
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

    /// Builds the Vars tab's shadow hint map: `name → "overrides <env>:
    /// <value>"` for every open-request `[variables]` entry that shares a
    /// name with a resolved project variable — masked (spec §3: secrets are
    /// masked everywhere by default) rather than the real value when the
    /// project variable is a secret.
    fn compute_shadowed(&self) -> indexmap::IndexMap<String, String> {
        use postui_core::varmodel::VarMeta;
        let env_label = self.project.env_label();
        self.editor
            .variables
            .keys()
            .filter_map(|name| {
                let value = self.project.resolved.values.get(name)?;
                let display = if matches!(
                    self.project.resolved.meta.get(name),
                    Some(VarMeta::Secret) | Some(VarMeta::MissingSecret)
                ) {
                    "\u{25cf}\u{25cf}\u{25cf}\u{25cf}"
                } else {
                    value.as_str()
                };
                Some((name.clone(), format!("{env_label}: {display}")))
            })
            .collect()
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
                    revealed: false,
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
                // Cycles the on-screen order (Params → Headers → Vars →
                // Body), not `EditorTab::index()`'s alt+1/2/3 slot numbers.
                let cur = self.editor.active_tab.draw_position() as i8;
                let next = (cur + delta).rem_euclid(4);
                self.editor.active_tab = EditorTab::from_draw_position(next as usize);
                self.editor.table.reset();
                true
            }
            Action::CycleMethod => {
                self.editor.method = self.editor.method.cycle();
                true
            }
            Action::OpenMethodDropdown => {
                use crate::components::modal::{DropdownState, MenuItem};
                use postui_core::model::Method;
                let items: Vec<MenuItem> = Method::ALL
                    .iter()
                    .map(|&m| MenuItem::new(m.as_str(), Action::SetMethod(m)))
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
                            revealed: false,
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
                    revealed: false,
                });
                true
            }
            Action::PromptNewRequestIn(folder) => {
                self.modals.push(Modal::Prompt {
                    title: "New request".into(),
                    input: crate::components::line_input::LineInput::new(&format!("{folder}/")),
                    kind: PromptKind::NewRequest,
                    revealed: false,
                });
                true
            }
            Action::DuplicateRequest => {
                let Some(slug) = self.sidebar.selected_slug() else {
                    return true;
                };
                match postui_core::storage::duplicate_request(&self.project.root, &slug) {
                    Ok(new_slug) => {
                        self.refresh_sidebar();
                        self.toasts
                            .push(format!("Duplicated to {new_slug}"), ToastKind::Success);
                        // Copies land next to the original and open, so the
                        // edit that motivated the duplicate can start at once.
                        self.apply(Action::OpenRequest(new_slug))
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("could not duplicate {slug}: {e}"), ToastKind::Error);
                        true
                    }
                }
            }
            Action::PromptRenameRequest => {
                if let Some(slug) = self.sidebar.selected_slug() {
                    self.modals.push(Modal::Prompt {
                        title: "Rename request".into(),
                        input: crate::components::line_input::LineInput::new(&slug),
                        kind: PromptKind::RenameRequest { from: slug },
                        revealed: false,
                    });
                }
                true
            }
            Action::ConfirmDeleteTableRow(i) => {
                let (map, noun) = match self.editor.active_tab {
                    EditorTab::Params => (&self.editor.params, "param"),
                    EditorTab::Headers => (&self.editor.headers, "header"),
                    EditorTab::Vars => (&self.editor.variables, "variable"),
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
                    EditorTab::Vars => &mut self.editor.variables,
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
                    Err(postui_core::prepare::PrepareError::Unresolved(causes)) => {
                        // Send-time secret prompt (spec §3): a missing
                        // secret pauses the send with a masked prompt
                        // instead of the usual toast — confirming it
                        // (`Action::SetSecret`) re-runs `ForceSend`, which
                        // either prompts for the next missing secret or,
                        // once every secret is resolved, falls through to
                        // the ordinary unresolved-variable toast for
                        // whatever's left (`causes` is a `BTreeMap`, so the
                        // first name found here is the alphabetically first
                        // one, same tie-break as the ctrl+v hint below).
                        if let Some(name) = causes.iter().find_map(|(name, cause)| {
                            (*cause == postui_core::prepare::UnresolvedCause::MissingSecret)
                                .then(|| name.clone())
                        }) {
                            self.modals.push(Modal::Prompt {
                                title: format!(
                                    "Value for `{name}` (secret, env `{}`)",
                                    self.project.env_label()
                                ),
                                input: LineInput::new(""),
                                kind: PromptKind::SecretValue {
                                    name,
                                    env: self.project.env_label(),
                                },
                                revealed: false,
                            });
                            return true;
                        }
                        let label = self.project.env_label();
                        let mut msg = format!(
                            "{} ({label})",
                            postui_core::prepare::PrepareError::Unresolved(causes.clone())
                        );
                        // Name the first (alphabetically — `causes` is a
                        // `BTreeMap`) variable that just needs a pick, not
                        // a fix, so `ctrl+v` is a visible next step rather
                        // than a dead end.
                        if let Some(name) = causes.iter().find_map(|(name, cause)| {
                            (*cause == postui_core::prepare::UnresolvedCause::NeedsSelection)
                                .then(|| name.clone())
                        }) {
                            msg.push_str(&format!(" \u{2014} press ctrl+v to select {name}"));
                        }
                        self.toasts.push(msg, ToastKind::Error);
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
            Action::SetSecret { name, value } => match self.project.set_secret(&name, value) {
                Ok(()) => self.apply(Action::ForceSend),
                Err(e) => {
                    self.toasts.push(
                        format!("could not save secret {name}: {e}"),
                        ToastKind::Error,
                    );
                    true
                }
            },
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
                self.prompt_migration_if_pending();
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
                    revealed: false,
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
                items.push(ChooserItem {
                    label: "new environment…".into(),
                    detail: None,
                    actions: vec![Action::OpenNewEnvPrompt],
                });
                self.modals
                    .push(Modal::Chooser(ChooserState::new("Environments", items)));
                true
            }
            Action::OpenNewEnvPrompt => {
                self.modals.push(Modal::Prompt {
                    title: "New environment (a-z 0-9 - _)".into(),
                    input: crate::components::line_input::LineInput::new(""),
                    kind: PromptKind::NewEnvironment,
                    revealed: false,
                });
                true
            }
            Action::CreateEnv(name) => {
                match postui_core::project::create_environment(&self.project.root, &name) {
                    Ok(()) => {
                        self.project.environments =
                            postui_core::project::list_environments(&self.project.root);
                        self.apply(Action::SwitchEnv(Some(name)));
                    }
                    Err(e) => {
                        let msg = if self
                            .project
                            .root
                            .join("environments")
                            .join(format!("{name}.toml"))
                            .is_file()
                        {
                            format!("environment \"{name}\" already exists")
                        } else {
                            format!("cannot create environment: {e}")
                        };
                        self.toasts.push(msg, ToastKind::Warning);
                    }
                }
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
            Action::ApplyMigration => {
                match self.project.apply_migration() {
                    Ok(notes) => {
                        self.refresh_sidebar();
                        let summary = if notes.is_empty() {
                            "variables migrated \u{2014} a .bak of each rewritten file is beside it"
                                .to_string()
                        } else {
                            format!(
                                "variables migrated ({}) \u{2014} a .bak of each rewritten file is beside it",
                                notes.join("; ")
                            )
                        };
                        self.toasts.push(summary, ToastKind::Success);
                    }
                    Err(msg) => self
                        .toasts
                        .push(format!("could not migrate: {msg}"), ToastKind::Error),
                }
                true
            }
            Action::DeclineMigration => {
                self.project.decline_migration();
                self.toasts.push(
                    "left the old variable files alone \u{2014} variables stay unavailable until they're migrated",
                    ToastKind::Warning,
                );
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
                self.prompt_migration_if_pending();
                changed
            }
            Action::OpenVarPicker { completing } => {
                self.apply(Action::ReloadProjectFiles);
                if !completing
                    && let Some((text, cursor)) =
                        self.focused_field_text().map(|(t, c)| (t.to_string(), c))
                    && let Some((name, group)) =
                        Self::selection_picker_target(&self.project, &text, cursor)
                {
                    return self.open_select_picker(name, group);
                }
                let resolved = self.project.prepare_context().vars;
                use crate::components::modal::Modal;
                use crate::components::var_picker::{VarPickerState, insert_entries};
                let entries =
                    insert_entries(&self.project.model, &resolved, &self.editor.variables);
                self.modals
                    .push(Modal::VarPicker(VarPickerState::new(entries, completing)));
                true
            }
            Action::OpenNewVariablePrompt {
                prefill,
                completing,
            } => {
                self.modals.push(Modal::Prompt {
                    title: "New variable".into(),
                    input: LineInput::new(&prefill),
                    kind: PromptKind::NewVariableAndInsert { completing },
                    revealed: false,
                });
                true
            }
            Action::InsertVarText(text) => {
                if self.focus == PaneId::Editor && self.editor.sub_focus == SubFocus::Url {
                    self.editor.url.insert_str(&text);
                } else if self.focus == PaneId::Editor
                    && matches!(
                        self.editor.active_tab,
                        EditorTab::Params | EditorTab::Headers | EditorTab::Vars
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
            Action::VarEdit(op) => {
                match self.apply_var_edit(&op) {
                    Ok(()) => self.varmanager.editing = None,
                    Err(msg) => self.toasts.push(msg, ToastKind::Error),
                }
                true
            }
            Action::PromptNewVar => {
                self.modals.push(Modal::Prompt {
                    title: "New variable".into(),
                    input: LineInput::new(""),
                    kind: PromptKind::NewVariable,
                    revealed: false,
                });
                true
            }
            Action::PromptNewGroup => {
                self.modals.push(Modal::Prompt {
                    title: "New group".into(),
                    input: LineInput::new(""),
                    kind: PromptKind::NewGroup,
                    revealed: false,
                });
                true
            }
            Action::OpenSelectDropdown {
                owner,
                env,
                row,
                col,
            } => {
                let env_data = if self.project.active_env.as_deref() == Some(env.as_str()) {
                    self.project.env_data.clone()
                } else {
                    postui_core::project::load_environment(&self.project.root, &env)
                        .unwrap_or_default()
                };
                let selections = self.project.selections_for(&env).get(&owner).cloned();
                let entries: Vec<(String, String)> =
                    postui_core::varmodel::group_entries(&env_data, &owner)
                        .map(|entries| {
                            entries
                                .keys()
                                .map(|name| (name.clone(), name.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                if entries.is_empty() {
                    return true;
                }
                let current = entries
                    .iter()
                    .position(|(k, _)| Some(k) == selections.as_ref());
                use crate::components::modal::MenuItem;
                let items: Vec<MenuItem> = entries
                    .into_iter()
                    .map(|(k, label)| {
                        MenuItem::new(
                            label,
                            Action::VarEdit(VarEditOp::Select {
                                env: env.clone(),
                                name: owner.clone(),
                                key: k,
                            }),
                        )
                    })
                    .collect();
                let anchor = self
                    .hits
                    .rect_of(&crate::hit::Hit::VarCell { row, col })
                    .unwrap_or_else(|| ratatui::layout::Rect::new(0, 0, 0, 0));
                use crate::components::modal::DropdownState;
                self.modals.push(Modal::Dropdown(DropdownState {
                    anchor,
                    items,
                    selected: current.unwrap_or(0),
                    current,
                }));
                true
            }
            Action::PromptAddGroupMember { group } => {
                self.modals.push(Modal::Prompt {
                    title: format!("Add member to {group}"),
                    input: LineInput::new(""),
                    kind: PromptKind::AddGroupMember { group },
                    revealed: false,
                });
                true
            }
            Action::AddGroupMember { group, member } => {
                let current = self
                    .project
                    .model
                    .groups
                    .get(&group)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                if current.iter().any(|m| m == &member) {
                    self.toasts.push(
                        format!("\"{member}\" is already a member of {group}"),
                        ToastKind::Warning,
                    );
                    return true;
                }
                let mut members = current;
                members.push(member);
                self.apply(Action::VarStruct(VarStructOp::SetMembers {
                    group,
                    members,
                }));
                true
            }
            Action::ConfirmRemoveGroupMember { group, member } => {
                self.modals.push(Modal::Confirm {
                    title: format!("Remove {member}"),
                    body: format!(
                        "Remove \"{member}\" from group \"{group}\"? Its values in the group's options are removed too."
                    ),
                    choices: vec![
                        ('n', "Cancel".into(), vec![]),
                        (
                            'y',
                            "Remove".into(),
                            vec![Action::RemoveGroupMember { group, member }],
                        ),
                    ],
                });
                true
            }
            Action::RemoveGroupMember { group, member } => {
                // Env files first: variables.toml's validation runs against
                // the active env, whose entries must no longer carry the
                // field by the time the group's field list changes.
                let Some(fields) = self
                    .project
                    .model
                    .groups
                    .get(&group)
                    .map(|g| g.fields.clone())
                else {
                    self.toasts
                        .push(format!("no group \"{group}\""), ToastKind::Error);
                    return true;
                };
                let remaining: Vec<String> = fields.into_iter().filter(|f| f != &member).collect();
                let envs = postui_core::project::list_environments(&self.project.root);
                let result = envs
                    .iter()
                    .try_for_each(|env| {
                        self.project.edit_env(env, |doc| {
                            postui_core::varedit::strip_entry_field(doc, &group, &member)
                        })
                    })
                    .and_then(|()| {
                        self.project.edit_variables(|doc| {
                            postui_core::varedit::upsert_group(doc, &group, None, &remaining)
                        })
                    });
                match result {
                    Ok(()) => self.toasts.push(
                        format!("removed \"{member}\" from {group}"),
                        ToastKind::Info,
                    ),
                    Err(msg) => self.toasts.push(msg, ToastKind::Error),
                }
                true
            }
            Action::PromptNewOption { owner } => {
                let member_names = self
                    .project
                    .model
                    .groups
                    .get(&owner)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                let title = if member_names.is_empty() {
                    format!("New option on {owner} \u{2014} key, value")
                } else {
                    let fields = member_names
                        .iter()
                        .map(|m| format!("{m}=value"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("New option on {owner} \u{2014} key, {fields}")
                };
                self.modals.push(Modal::Prompt {
                    title,
                    input: LineInput::new(""),
                    kind: PromptKind::NewOption {
                        owner,
                        member_names,
                    },
                    revealed: false,
                });
                true
            }
            Action::PromptRenameVar { from } => {
                // Finding 7: surface `scan_usage`'s count the same way
                // `ConfirmDeleteVar` already does — renaming doesn't break
                // those requests (references keep the old name until
                // someone edits them), but the user should still know the
                // name isn't as free-standing as it looks.
                let usage = postui_core::varedit::scan_usage(&self.project.root, &from);
                let title = if usage.is_empty() {
                    format!("Rename {from}")
                } else {
                    format!(
                        "Rename {from} \u{2014} referenced by {} request(s): {} (references keep the old name)",
                        usage.len(),
                        usage.join(", ")
                    )
                };
                self.modals.push(Modal::Prompt {
                    title,
                    input: LineInput::new(&from),
                    kind: PromptKind::RenameVariable { from },
                    revealed: false,
                });
                true
            }
            Action::PromptEditGroupMembers { group } => {
                let seed = self
                    .project
                    .model
                    .groups
                    .get(&group)
                    .map(|g| g.fields.join(", "))
                    .unwrap_or_default();
                self.modals.push(Modal::Prompt {
                    title: format!("Members of {group}"),
                    input: LineInput::new(&seed),
                    kind: PromptKind::GroupMembers { group },
                    revealed: false,
                });
                true
            }
            Action::ConfirmDeleteVar { name } => {
                let usage = postui_core::varedit::scan_usage(&self.project.root, &name);
                let body = if usage.is_empty() {
                    format!("Delete \"{name}\"? This cannot be undone.")
                } else {
                    format!(
                        "Delete \"{name}\"? Referenced by {} request(s): {}.",
                        usage.len(),
                        usage.join(", ")
                    )
                };
                self.modals.push(Modal::Confirm {
                    title: format!("Delete {name}"),
                    body,
                    choices: vec![
                        ('n', "Cancel".into(), vec![]),
                        (
                            'y',
                            "Delete".into(),
                            vec![Action::VarStruct(VarStructOp::Delete { name })],
                        ),
                    ],
                });
                true
            }
            Action::ConfirmDeleteOption { owner, key } => {
                // Entries live in one environment each (spec §3.1), so
                // there is nothing "shared" to fall back to: the delete
                // removes this environment's entry.
                let env_label = self.project.env_label();
                let body = format!(
                    "Delete entry \"{key}\" of \"{owner}\" in {env_label}? This cannot be undone."
                );
                self.modals.push(Modal::Confirm {
                    title: format!("Delete {key}"),
                    body,
                    choices: vec![
                        ('n', "Cancel".into(), vec![]),
                        (
                            'y',
                            "Delete".into(),
                            vec![Action::VarStruct(VarStructOp::DeleteOption { owner, key })],
                        ),
                    ],
                });
                true
            }
            Action::ToggleSecretVar { name } => {
                self.open_toggle_secret_confirm(name);
                true
            }
            Action::PromptPromoteVar { name } => {
                // Finding 8: a secret already declared at `name` used to
                // fall through to `upsert_var` writing a `default`
                // alongside `secret = true` — `edit_variables` then
                // rejected the resulting `variables.toml` on re-parse
                // (`ModelError::SecretWithDefault`), surfacing as an
                // incidental parse-error toast instead of an intentional
                // refusal. Refuse up front instead, mirroring
                // `open_demote_confirm`'s refusal modals.
                if self.project.model.vars.get(&name).is_some_and(|d| d.secret) {
                    self.modals.push(Modal::Message {
                        title: "Can't promote".into(),
                        body: format!(
                            "\"{name}\" is already declared as a secret; promoting a plain value onto it would either commit a secret to variables.toml or make the declaration invalid."
                        ),
                    });
                    return true;
                }
                let mut choices = vec![(
                    'd',
                    "Default value".to_string(),
                    vec![Action::VarStruct(VarStructOp::Promote {
                        name: name.clone(),
                        target: postui_core::varedit::PromoteTarget::Default,
                    })],
                )];
                if let Some(env) = self.project.active_env.clone() {
                    choices.push((
                        'e',
                        format!("Env value ({env})"),
                        vec![Action::VarStruct(VarStructOp::Promote {
                            name: name.clone(),
                            target: postui_core::varedit::PromoteTarget::Env,
                        })],
                    ));
                }
                choices.push(('c', "Cancel".to_string(), vec![]));
                self.modals.push(Modal::Confirm {
                    title: format!("Promote {name}"),
                    body: "Where should the value land?".into(),
                    choices,
                });
                true
            }
            Action::ConfirmDemoteVar { name } => {
                self.open_demote_confirm(name);
                true
            }
            Action::VarStruct(op) => {
                match self.apply_var_struct(&op) {
                    Ok(()) => {
                        let open_request = self
                            .editor
                            .slug
                            .is_some()
                            .then(|| self.editor.current_request());
                        let rows = varmanager::build_rows(
                            &self.project,
                            open_request.as_ref(),
                            &self.varmanager.expanded,
                        );
                        self.varmanager.cursor.0 =
                            self.varmanager.cursor.0.min(rows.len().saturating_sub(1));
                        self.varmanager.cursor.1 = self
                            .varmanager
                            .cursor
                            .1
                            .min(self.project.environments.len());
                        self.varmanager.ensure_visible = true;
                    }
                    Err(msg) => {
                        self.toasts.push(msg, ToastKind::Error);
                        self.last_action_failed = true;
                    }
                }
                true
            }

            // -- Task 17: in-context flows (spec §6) --
            Action::OpenNewOptionInlinePrompt { owner } => {
                use crate::components::modal::PromptField;
                self.modals.push(Modal::MultiPrompt {
                    title: format!("Add option on {owner}"),
                    fields: vec![
                        PromptField::text("key", "Key", ""),
                        PromptField::text("value", "Value", ""),
                        PromptField::text("description", "Description", ""),
                    ],
                    focus: 0,
                    kind: PromptKind::NewOptionInline { owner },
                });
                true
            }
            Action::OpenEditOptionPrompt {
                owner,
                key,
                description,
                values,
            } => {
                use crate::components::modal::PromptField;
                let mut fields: Vec<PromptField> = values
                    .iter()
                    .map(|(k, v)| {
                        let label = if k == "value" { "Value" } else { k.as_str() };
                        PromptField::text(k, label, v)
                    })
                    .collect();
                fields.push(PromptField::text(
                    "description",
                    "Description",
                    description.as_deref().unwrap_or(""),
                ));
                self.modals.push(Modal::MultiPrompt {
                    title: format!("Edit {key} on {owner}"),
                    fields,
                    focus: 0,
                    kind: PromptKind::EditOption { owner, key },
                });
                true
            }
            Action::ConfirmNewOptionInline {
                owner,
                key,
                value,
                description,
            } => {
                if !postui_core::vars::is_valid_var_name(&key) {
                    self.toasts.push(
                        format!("\"{key}\" is not a valid option key"),
                        ToastKind::Error,
                    );
                    return true;
                }
                let Some(env) = self.project.active_env.clone() else {
                    self.toasts.push(
                        "no active environment \u{2014} switch to one first",
                        ToastKind::Warning,
                    );
                    return true;
                };
                // The inline prompt collects one value, but an entry has
                // to supply every field of its group or `validate_env`
                // rejects it: the typed value fills the first field and
                // the rest start empty, for the Manager to fill in.
                let fields = self
                    .project
                    .model
                    .groups
                    .get(&owner)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                let mut values = indexmap::IndexMap::new();
                for (i, field) in fields.into_iter().enumerate() {
                    values.insert(field, if i == 0 { value.clone() } else { String::new() });
                }
                match self.project.edit_env(&env, |doc| {
                    postui_core::varedit::upsert_entry(
                        doc,
                        &owner,
                        &key,
                        description.as_deref(),
                        &values,
                    )
                }) {
                    Ok(()) => {
                        self.project.set_selection_for(&env, &owner, &key);
                        self.toasts.push(
                            format!("{owner} \u{2192} {key} ({env})"),
                            ToastKind::Success,
                        );
                    }
                    Err(msg) => self.toasts.push(msg, ToastKind::Error),
                }
                true
            }
            Action::ConfirmEditOption {
                owner,
                key,
                values,
                description,
            } => {
                // An entry belongs to exactly one environment, so the
                // edit always lands in the active env's file.
                let Some(env) = self.project.active_env.clone() else {
                    self.toasts.push(
                        "no active environment \u{2014} switch to one first",
                        ToastKind::Warning,
                    );
                    return true;
                };
                let result = self.project.edit_env(&env, |doc| {
                    postui_core::varedit::upsert_entry(
                        doc,
                        &owner,
                        &key,
                        description.as_deref(),
                        &values,
                    )
                });
                match result {
                    Ok(()) => self
                        .toasts
                        .push(format!("{key} updated"), ToastKind::Success),
                    Err(msg) => self.toasts.push(msg, ToastKind::Error),
                }
                true
            }
            Action::ExtractToVariable => {
                if self.focus == PaneId::Editor
                    && self.editor.active_tab == EditorTab::Body
                    && self.editor.sub_focus == SubFocus::Content
                {
                    self.toasts.push(
                        "extract to variable isn't available in the body yet",
                        ToastKind::Warning,
                    );
                    return true;
                }
                let Some(text) = self.focused_field_text().map(|(t, _)| t.to_string()) else {
                    self.toasts
                        .push("focus a text field first", ToastKind::Warning);
                    return true;
                };
                if text.trim().is_empty() {
                    self.toasts.push(
                        "nothing to extract \u{2014} the field is empty",
                        ToastKind::Warning,
                    );
                    return true;
                }
                use crate::components::modal::PromptField;
                self.modals.push(Modal::MultiPrompt {
                    title: "Extract to variable".into(),
                    fields: vec![
                        PromptField::text("name", "Name", ""),
                        PromptField::choice(
                            "destination",
                            "Destination",
                            &["Project default", "Active env value", "This request"],
                        ),
                    ],
                    focus: 0,
                    kind: PromptKind::ExtractVariable,
                });
                true
            }
            Action::ConfirmExtractVariable { name, destination } => {
                if !postui_core::vars::is_valid_var_name(&name) {
                    self.toasts.push(
                        format!("\"{name}\" is not a valid variable name"),
                        ToastKind::Error,
                    );
                    return true;
                }
                let Some(text) = self.focused_field_text().map(|(t, _)| t.to_string()) else {
                    self.toasts
                        .push("focus a text field first", ToastKind::Warning);
                    return true;
                };
                use crate::action::ExtractDestination;
                let write_result: Result<(), String> = match destination {
                    ExtractDestination::ProjectDefault => {
                        if self.project.model.vars.contains_key(&name)
                            || self.project.model.groups.contains_key(&name)
                        {
                            Err(format!("\"{name}\" already exists"))
                        } else {
                            self.project.edit_variables(|doc| {
                                postui_core::varedit::upsert_var(doc, &name, None, Some(&text))
                            })
                        }
                    }
                    ExtractDestination::ActiveEnv => {
                        let Some(env) = self.project.active_env.clone() else {
                            self.toasts
                                .push("no active environment", ToastKind::Warning);
                            return true;
                        };
                        // Same namespace-collision guard as `ProjectDefault`
                        // (a group of this name would otherwise sit
                        // alongside a same-named plain variable), plus the
                        // one `validate_env` would reject outright if we
                        // wrote a flat env value anyway: a secret variable
                        // (env values for secrets are forbidden —
                        // `ModelError::EnvValueForSecret`). Catching it
                        // here — rather than letting `edit_env` fail after
                        // the fact — keeps the refusal a clean toast
                        // instead of a write attempt against a doc that
                        // `validate_env` would then reject.
                        if self.project.model.groups.contains_key(&name) {
                            self.toasts
                                .push(format!("\"{name}\" already exists"), ToastKind::Error);
                            return true;
                        }
                        if let Some(decl) = self.project.model.vars.get(&name) {
                            if decl.secret {
                                self.toasts.push(
                                    format!(
                                        "\"{name}\" is a secret variable \u{2014} can't set a plain env value for it"
                                    ),
                                    ToastKind::Error,
                                );
                                return true;
                            }
                        } else if let Err(msg) = self.project.edit_variables(|doc| {
                            postui_core::varedit::upsert_var(doc, &name, None, None)
                        }) {
                            self.toasts.push(msg, ToastKind::Error);
                            return true;
                        }
                        self.project.edit_env(&env, |doc| {
                            postui_core::varedit::set_env_value(doc, &name, Some(&text))
                        })
                    }
                    ExtractDestination::Request => {
                        // No structural-file hazard here — `[variables]`
                        // entries are a separate resolution layer (spec §2)
                        // with no `validate_env`-style cross-checks — but an
                        // existing entry of the same name would otherwise be
                        // silently clobbered, same as `ProjectDefault`'s
                        // "already exists" refusal.
                        if self.editor.variables.contains_key(&name) {
                            self.toasts.push(
                                format!("\"{name}\" already exists in this request's variables"),
                                ToastKind::Error,
                            );
                            return true;
                        }
                        self.editor.variables.insert(
                            name.clone(),
                            postui_core::model::Entry {
                                value: text.clone(),
                                enabled: true,
                            },
                        );
                        Ok(())
                    }
                };
                let wrote_to_request = matches!(destination, ExtractDestination::Request);
                match write_result {
                    Ok(()) => {
                        self.replace_focused_field_with_token(&name);
                        // Finding 2, same ruling as demote/promote: the
                        // `Request` destination's write only exists so far
                        // in the dirty editor buffer (both the new
                        // `[variables]` entry above and the field text
                        // `replace_focused_field_with_token` just
                        // committed) — save it synchronously rather than
                        // leaving it save-on-demand, so "extract to
                        // request, then quit" can't lose it.
                        if wrote_to_request && let Err(e) = self.save_open_request() {
                            self.toasts.push(
                                format!(
                                    "extracted to {{{{{name}}}}} but {e} \u{2014} save the request manually"
                                ),
                                ToastKind::Error,
                            );
                            return true;
                        }
                        self.toasts
                            .push(format!("extracted to {{{{{name}}}}}"), ToastKind::Success);
                    }
                    Err(msg) => self.toasts.push(msg, ToastKind::Error),
                }
                true
            }
        }
    }

    /// The `(text, cursor)` of whichever plain text field currently holds
    /// the caret — the URL line or a table cell under edit — for
    /// cursor-on-token detection (spec §6's first picker context). `None`
    /// when focus is elsewhere (a different pane, the Body tab's edtui
    /// buffer, or a table row that's selected but not under edit).
    fn focused_field_text(&self) -> Option<(&str, usize)> {
        if self.focus != PaneId::Editor {
            return None;
        }
        if self.editor.sub_focus == SubFocus::Url {
            return Some((self.editor.url.text(), self.editor.url.cursor()));
        }
        if self.editor.sub_focus == SubFocus::Content
            && let Some(edit) = &self.editor.table.editing
        {
            return Some((edit.input.text(), edit.input.cursor()));
        }
        None
    }

    /// Whether `cursor` (a char index into `text`) sits on a `{{name}}`
    /// token whose name is a group field in the active env
    /// (spec §6's cursor-on-token rule) — and if so, the `(name, group)`
    /// pair `PickerMode::SelectOption` wants: `name` is the token's own
    /// name and `group` is the owning group's. `None` when the cursor
    /// isn't on a token, or the token's name isn't a group field (a
    /// simple/secret/undeclared name — `ctrl+v` there falls back to
    /// ordinary `Insert` autocomplete).
    fn selection_picker_target(
        ctx: &ProjectContext,
        text: &str,
        cursor: usize,
    ) -> Option<(String, String)> {
        let byte_off = text
            .char_indices()
            .nth(cursor)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        let token = postui_core::vars::find_tokens(text)
            .into_iter()
            .find(|t| byte_off >= t.start && byte_off <= t.end)?;
        use postui_core::varmodel::VarMeta;
        match ctx.resolved.meta.get(&token.name) {
            Some(VarMeta::GroupMember { group, .. }) => Some((token.name, group.clone())),
            Some(VarMeta::NeedsSelection) => {
                let group = ctx
                    .model
                    .groups
                    .iter()
                    .find(|(_, g)| g.fields.contains(&token.name))
                    .map(|(n, _)| n.clone())?;
                Some((token.name, group))
            }
            _ => None,
        }
    }

    /// `Action::ConfirmExtractVariable`'s tail: replaces whichever field
    /// `focused_field_text` found (unchanged since the extract flow opened
    /// — modals capture all input) with `{{name}}`. For the URL, that's a
    /// direct field replacement (already "dirty" the instant it differs
    /// from the saved snapshot); for a table cell under edit, the new text
    /// is committed through the table's own `Enter` path so it lands in the
    /// map (not left as a pending, uncommitted edit) and rides the same
    /// dirty/save path as any other row commit.
    fn replace_focused_field_with_token(&mut self, name: &str) {
        let token = format!("{{{{{name}}}}}");
        if self.editor.sub_focus == SubFocus::Url {
            self.editor.url = LineInput::new(&token);
            return;
        }
        if let Some(edit) = self.editor.table.editing.as_mut() {
            edit.input = LineInput::new(&token);
        } else {
            return;
        }
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        match self.editor.active_tab {
            EditorTab::Params => {
                self.editor.table.handle_key(enter, &mut self.editor.params);
            }
            EditorTab::Headers => {
                self.editor
                    .table
                    .handle_key(enter, &mut self.editor.headers);
            }
            EditorTab::Vars => {
                self.editor
                    .table
                    .handle_key(enter, &mut self.editor.variables);
            }
            EditorTab::Body => {}
        }
    }

    /// Builds and opens the `SelectOption` picker (spec §6's first
    /// context) for `name`, a field of `group`: rows are `group`'s entries
    /// in the active environment, each previewed as its per-field values,
    /// with the current selection marked with a ✓.
    fn open_select_picker(&mut self, name: String, group: String) -> bool {
        use crate::components::modal::Modal;
        use crate::components::var_picker::{SelectEntry, VarPickerState};
        use postui_core::varmodel;

        let env_key = self.project.active_env.clone().unwrap_or_default();
        let selected_key = self.project.selections_for(&env_key).get(&group).cloned();
        let entries: Vec<SelectEntry> = varmodel::group_entries(&self.project.env_data, &group)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(key, decl)| {
                        let mut parts: Vec<String> = Vec::new();
                        if let Some(desc) = &decl.description {
                            parts.push(desc.clone());
                        }
                        for (field, value) in &decl.values {
                            parts.push(format!("{field} {value}"));
                        }
                        SelectEntry {
                            key: key.clone(),
                            description: decl.description.clone(),
                            value: None,
                            preview: Some(parts.join(" \u{b7} ")),
                            selected: selected_key.as_deref() == Some(key.as_str()),
                            values: Some(decl.values.clone()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if entries.is_empty() {
            self.toasts
                .push(format!("{group} has no entries here"), ToastKind::Warning);
            return true;
        }
        self.modals
            .push(Modal::VarPicker(VarPickerState::new_select(
                entries, name, group, env_key,
            )));
        true
    }

    /// Applies one committed Variable Manager op (spec §5), writing
    /// through to whichever file owns it. `Err(msg)` is always safe to
    /// toast (never a secret value) and, per the caller
    /// (`Action::VarEdit`), leaves `varmanager.editing` untouched so the
    /// typed text survives a retry.
    fn apply_var_edit(&mut self, op: &VarEditOp) -> Result<(), String> {
        match op {
            VarEditOp::SetEnvValue { env, name, value } => self.project.edit_env(env, |doc| {
                postui_core::varedit::set_env_value(doc, name, Some(value))
            }),
            VarEditOp::SetDefault { name, value } => self.project.edit_variables(|doc| {
                postui_core::varedit::upsert_var(doc, name, None, Some(value))
            }),
            VarEditOp::SetDescription { owner, value } => {
                if self.project.model.vars.contains_key(owner) {
                    self.project.edit_variables(|doc| {
                        postui_core::varedit::upsert_var(doc, owner, Some(value), None)
                    })
                } else if let Some(fields) = self
                    .project
                    .model
                    .groups
                    .get(owner)
                    .map(|g| g.fields.clone())
                {
                    self.project.edit_variables(|doc| {
                        postui_core::varedit::upsert_group(doc, owner, Some(value), &fields)
                    })
                } else {
                    Err(format!("\"{owner}\" is not a declared variable or group"))
                }
            }
            VarEditOp::SetSecretValue { env, name, value } => {
                self.project.set_secret_for(env, name, value.clone())
            }
            VarEditOp::SetOptionValue {
                env,
                owner,
                key,
                member,
                value,
            } => {
                // An entry's values live in one environment's file; the
                // cell being edited is one field of that entry.
                let Some(field) = member.clone() else {
                    return Err(format!(
                        "entry \"{key}\" of \"{owner}\" has no single value to edit"
                    ));
                };
                let mut values = indexmap::IndexMap::new();
                values.insert(field, value.clone());
                self.project.edit_env(env, |doc| {
                    postui_core::varedit::upsert_entry(doc, owner, key, None, &values)
                })
            }
            VarEditOp::SetRequestVar { name, value } => {
                match self.editor.variables.get_mut(name) {
                    Some(entry) => entry.value = value.clone(),
                    None => {
                        self.editor.variables.insert(
                            name.clone(),
                            postui_core::model::Entry {
                                value: value.clone(),
                                enabled: true,
                            },
                        );
                    }
                }
                Ok(())
            }
            VarEditOp::Select { env, name, key } => {
                self.project.set_selection_for(env, name, key);
                Ok(())
            }
        }
    }

    /// `s` on a `Var` row (spec §3's two transitions): opens
    /// the direction-appropriate confirm. Un-marking secret shows the
    /// current value(s) for the user to copy — deliberately, per spec, the
    /// one place a secret value is displayed outside substituted request
    /// content — and moves nothing; marking secret lists which environment
    /// values will move into `.local/secrets.toml` and be stripped from
    /// their env files.
    fn open_toggle_secret_confirm(&mut self, name: String) {
        let Some(decl) = self.project.model.vars.get(&name) else {
            // A group (or a name that is not declared at all) has no
            // secret flag to flip: only a variable declaration carries one.
            self.toasts.push(
                format!("\"{name}\" is not a variable; only a variable can be secret"),
                ToastKind::Error,
            );
            return;
        };
        let is_secret = decl.secret;
        if is_secret {
            let mut lines: Vec<String> = Vec::new();
            for env in &self.project.environments {
                if let Some(v) = self.project.secrets.get(env).and_then(|m| m.get(&name)) {
                    lines.push(format!("{env}: {v}"));
                }
            }
            let body = if lines.is_empty() {
                format!("Turn off secret for \"{name}\"? No value is stored yet.")
            } else {
                format!(
                    "Turn off secret for \"{name}\"? Copy the current value(s) first \u{2014} nothing is moved automatically:\n{}",
                    lines.join("\n")
                )
            };
            self.modals.push(Modal::Confirm {
                title: format!("Un-mark {name} as secret"),
                body,
                choices: vec![
                    ('n', "Cancel".into(), vec![]),
                    (
                        'y',
                        "Turn off".into(),
                        vec![Action::VarStruct(VarStructOp::ToggleSecret { name })],
                    ),
                ],
            });
        } else {
            let mut envs_with_values: Vec<String> = Vec::new();
            for env in self.project.environments.clone() {
                let env_data = if self.project.active_env.as_deref() == Some(env.as_str()) {
                    self.project.env_data.clone()
                } else {
                    postui_core::project::load_environment(&self.project.root, &env)
                        .unwrap_or_default()
                };
                if env_data.values.contains_key(&name) {
                    envs_with_values.push(env);
                }
            }
            let body = if envs_with_values.is_empty() {
                format!("Mark \"{name}\" as secret? No environment values to move.")
            } else {
                format!(
                    "Mark \"{name}\" as secret? These environments' values move into .local/secrets.toml and are stripped from their env files: {}.",
                    envs_with_values.join(", ")
                )
            };
            self.modals.push(Modal::Confirm {
                title: format!("Mark {name} as secret"),
                body,
                choices: vec![
                    ('n', "Cancel".into(), vec![]),
                    (
                        'y',
                        "Mark secret".into(),
                        vec![Action::VarStruct(VarStructOp::ToggleSecret { name })],
                    ),
                ],
            });
        }
    }

    /// `P` on a `Var` row (spec §4): refuses (with a message modal, no
    /// mutation) a secret name — its resolved value would otherwise land in
    /// a git-tracked request file — or a group (request scope is
    /// simple-only per spec); otherwise opens the demote confirm,
    /// its body naming any *other* requests already referencing it.
    fn open_demote_confirm(&mut self, name: String) {
        if self.editor.slug.is_none() {
            self.toasts
                .push("open a request to demote into", ToastKind::Warning);
            return;
        }
        let is_secret = self.project.model.vars.get(&name).is_some_and(|d| d.secret);
        if is_secret {
            self.modals.push(Modal::Message {
                title: "Can't demote".into(),
                body: format!(
                    "\"{name}\" is secret; its value can't be written into a request file."
                ),
            });
            return;
        }
        if self.project.model.groups.contains_key(&name) {
            self.modals.push(Modal::Message {
                title: "Can't demote".into(),
                body: format!("\"{name}\" is a group; request scope is simple values only."),
            });
            return;
        }
        let this_slug = self.editor.slug.clone();
        let others: Vec<String> = postui_core::varedit::scan_usage(&self.project.root, &name)
            .into_iter()
            .filter(|s| this_slug.as_deref() != Some(s.as_str()))
            .collect();
        let body = if others.is_empty() {
            format!("Demote \"{name}\" into this request?")
        } else {
            format!(
                "Demote \"{name}\" into this request? Referenced by {} other request(s): {}.",
                others.len(),
                others.join(", ")
            )
        };
        self.modals.push(Modal::Confirm {
            title: format!("Demote {name}"),
            body,
            choices: vec![
                ('n', "Cancel".into(), vec![]),
                (
                    'y',
                    "Demote".into(),
                    vec![Action::VarStruct(VarStructOp::Demote { name })],
                ),
            ],
        });
    }

    /// Applies one confirmed Variable Manager structural mutation (spec
    /// §5's action list; §4's promote/demote; §3's secret-flag
    /// transitions). `Err(msg)` — safe to toast, never a secret value —
    /// leaves everything unchanged; the caller (`Action::VarStruct`) never
    /// clears any modal/editing state on failure of its own accord, and
    /// also sets `last_action_failed` so `apply_modal_result` breaks off a
    /// sequenced `ModalResult` before running any action after this one
    /// (e.g. `PromptKind::NewVariableAndInsert`'s trailing `InsertVarText`,
    /// which must never fire for a variable that failed to declare).
    fn apply_var_struct(&mut self, op: &VarStructOp) -> Result<(), String> {
        use postui_core::varedit;
        use postui_core::vars::is_valid_var_name;

        let name_taken = |ctx: &ProjectContext, n: &str| {
            n == "options"
                || n == "groups"
                || ctx.model.vars.contains_key(n)
                || ctx.model.groups.contains_key(n)
        };

        match op {
            VarStructOp::NewVar { name, description } => {
                if !is_valid_var_name(name) {
                    return Err(format!("\"{name}\" is not a valid variable name"));
                }
                if name_taken(&self.project, name) {
                    return Err(format!("\"{name}\" already exists"));
                }
                self.project.edit_variables(|doc| {
                    varedit::upsert_var(doc, name, description.as_deref(), None)
                })
            }
            VarStructOp::NewGroup { name, members } => {
                if !is_valid_var_name(name) {
                    return Err(format!("\"{name}\" is not a valid group name"));
                }
                if name_taken(&self.project, name) {
                    return Err(format!("\"{name}\" already exists"));
                }
                for m in members {
                    if !is_valid_var_name(m) {
                        return Err(format!("\"{m}\" is not a valid member name"));
                    }
                }
                self.project
                    .edit_variables(|doc| varedit::upsert_group(doc, name, None, members))
            }
            VarStructOp::NewOption {
                owner,
                key,
                description,
                values,
            } => {
                if !is_valid_var_name(key) {
                    return Err(format!("\"{key}\" is not a valid entry name"));
                }
                // Entries belong to one environment (spec §3.1).
                let Some(env) = self.project.active_env.clone() else {
                    return Err(
                        "no active environment \u{2014} switch to one before adding an entry"
                            .to_string(),
                    );
                };
                self.project.edit_env(&env, |doc| {
                    varedit::upsert_entry(doc, owner, key, description.as_deref(), values)
                })
            }
            VarStructOp::Rename { from, to } => {
                if !is_valid_var_name(to) {
                    return Err(format!("\"{to}\" is not a valid variable name"));
                }
                if name_taken(&self.project, to) {
                    return Err(format!("\"{to}\" already exists"));
                }
                self.project
                    .edit_variables(|doc| varedit::rename_var(doc, from, to))?;
                // `rename_var` only ever touches `variables.toml` — an
                // active env override for `from` would otherwise silently
                // degrade to the default post-rename (no error, no
                // warning, just a wrong-looking resolved value). Cascade
                // into every environment's flat pair and `[options.*]`
                // table too; `rename_env_var` no-ops for an environment
                // with nothing to rename.
                for env in self.project.environments.clone() {
                    self.project
                        .edit_env(&env, |doc| varedit::rename_env_var(doc, from, to))?;
                }
                Ok(())
            }
            VarStructOp::Delete { name } => {
                let is_group = self.project.model.groups.contains_key(name);
                if !is_group {
                    // Mirror `delete_var`'s own "still a group member"
                    // conflict up front, using the already-loaded model —
                    // before any environment file is touched, so a refusal
                    // here leaves everything unchanged (`apply_var_struct`'s
                    // documented contract), matching what `delete_var`
                    // itself would have refused a moment later anyway.
                    if let Some(gname) = self
                        .project
                        .model
                        .groups
                        .iter()
                        .find_map(|(gname, g)| g.fields.contains(name).then(|| gname.clone()))
                    {
                        return Err(format!(
                            "variable \"{name}\" is a field of group \"{gname}\"; remove it from the group first"
                        ));
                    }
                }
                // Finding 1: `delete_var`/`delete_group` only ever touch
                // `variables.toml`. An env's `[options.<name>]` table for
                // the deleted name would otherwise strand that env file —
                // refused by `validate_env` in the ACTIVE env (a confusing
                // parse-style toast), or silently left invalid with no GUI
                // repair path in a NON-active one. Cascade into every
                // environment FIRST — a strip can only shrink an env file,
                // so it can never itself fail `validate_env` — and only
                // THEN remove the declaration: doing it in the other order
                // would have the declaration-removal's own `edit_variables`
                // call validate the ACTIVE env's *not-yet-stripped*
                // `[options.<name>]` table against a model that already
                // doesn't declare `name`, reproducing the exact "confusing
                // parse-style toast" this fix removes. `delete_env_var`
                // no-ops for an environment with nothing to remove.
                for env in self.project.environments.clone() {
                    self.project
                        .edit_env(&env, |doc| varedit::delete_env_var(doc, name))?;
                }
                if is_group {
                    self.project
                        .edit_variables(|doc| varedit::delete_group(doc, name))
                } else {
                    self.project
                        .edit_variables(|doc| varedit::delete_var(doc, name))
                }
            }
            VarStructOp::ToggleSecret { name } => self.apply_toggle_secret(name),
            VarStructOp::SetMembers { group, members } => {
                for m in members {
                    if !is_valid_var_name(m) {
                        return Err(format!("\"{m}\" is not a valid member name"));
                    }
                }
                self.project
                    .edit_variables(|doc| varedit::upsert_group(doc, group, None, members))
            }
            VarStructOp::Promote { name, target } => self.apply_promote(name, *target),
            VarStructOp::Demote { name } => self.apply_demote(name),
            VarStructOp::DeleteOption { owner, key } => self.apply_delete_option(owner, key),
        }
    }

    /// [`VarStructOp::DeleteOption`]: deletes one entry of `owner` from
    /// the active environment (entries belong to one environment each —
    /// spec §3.1). No active environment, or no such entry, is a quiet
    /// no-op success (a stale row — nothing left to do). Also clears any
    /// per-env selection naming the deleted entry, in every environment,
    /// so local state doesn't accumulate dead selections (`resolve_env`
    /// already degrades a stale selection harmlessly, but there's no
    /// reason to leave it).
    fn apply_delete_option(&mut self, owner: &str, key: &str) -> Result<(), String> {
        let present = self.project.active_env.clone().filter(|_| {
            postui_core::varmodel::group_entries(&self.project.env_data, owner)
                .is_some_and(|entries| entries.contains_key(key))
        });
        if let Some(env) = present {
            self.project.edit_env(&env, |doc| {
                postui_core::varedit::delete_entry(doc, owner, key)
            })?;
        }
        for env in self.project.environments.clone() {
            if self
                .project
                .selections_for(&env)
                .get(owner)
                .map(String::as_str)
                == Some(key)
            {
                self.project.clear_selection_for(&env, owner);
            }
        }
        Ok(())
    }

    /// [`VarStructOp::ToggleSecret`]'s two directions (spec §3). Off->on
    /// moves every environment's flat value for `name` into that
    /// environment's `.local/secrets.toml` slot and strips it from the env
    /// file; on->off only flips the flag — the local secret value is left
    /// exactly where it is (never silently promoted into a git-tracked
    /// file).
    fn apply_toggle_secret(&mut self, name: &str) -> Result<(), String> {
        let currently_secret = self.project.model.vars.get(name).is_some_and(|d| d.secret);
        if currently_secret {
            return self
                .project
                .edit_variables(|doc| postui_core::varedit::set_secret_flag(doc, name, false));
        }
        let mut to_move: Vec<(String, String)> = Vec::new();
        for env in self.project.environments.clone() {
            let env_data = if self.project.active_env.as_deref() == Some(env.as_str()) {
                self.project.env_data.clone()
            } else {
                postui_core::project::load_environment(&self.project.root, &env).unwrap_or_default()
            };
            if let Some(v) = env_data.values.get(name) {
                to_move.push((env, v.clone()));
            }
        }
        // Order matters: `edit_variables` validates the flipped flag against
        // the *active* env's current data, so a still-present flat value
        // there (a flat value for a secret variable is a §1.2 error) would
        // reject the flag flip. Move each value into secrets.toml and strip
        // it from its env file first — harmless against the not-yet-secret
        // model — so the flag flip last sees a model already consistent
        // with every environment's (now-empty) flat value.
        for (env, value) in &to_move {
            self.project.set_secret_for(env, name, value.clone())?;
        }
        for (env, _) in &to_move {
            self.project.edit_env(env, |doc| {
                postui_core::varedit::set_env_value(doc, name, None)
            })?;
        }
        self.project
            .edit_variables(|doc| postui_core::varedit::set_secret_flag(doc, name, true))?;
        Ok(())
    }

    /// [`VarStructOp::Promote`] (spec §4): writes the request's own
    /// `[variables]` entry into the project (default or the active
    /// environment), then removes it from the request now that the
    /// project owns it.
    fn apply_promote(
        &mut self,
        name: &str,
        target: postui_core::varedit::PromoteTarget,
    ) -> Result<(), String> {
        let entry = self
            .editor
            .variables
            .get(name)
            .cloned()
            .ok_or_else(|| format!("\"{name}\" is not a request-scope variable"))?;
        let vars_path = self.project.root.join("variables.toml");
        let vars_text = std::fs::read_to_string(&vars_path).unwrap_or_default();
        let env_name = self.project.active_env.clone();
        let env_text = match &env_name {
            Some(env) => Some(
                std::fs::read_to_string(
                    self.project
                        .root
                        .join("environments")
                        .join(format!("{env}.toml")),
                )
                .unwrap_or_default(),
            ),
            None => None,
        };
        let (new_vars, new_env) = postui_core::varedit::promote_var(
            &vars_text,
            env_text.as_deref(),
            name,
            &entry.value,
            target,
        )
        .map_err(|e| e.to_string())?;
        self.project.edit_variables(|_| Ok(new_vars))?;
        if let (Some(new_env), Some(env)) = (new_env, env_name) {
            self.project.edit_env(&env, |_| Ok(new_env))?;
        }
        self.editor.variables.shift_remove(name);
        // Finding 2: the project side of the promote is durable the moment
        // `edit_variables`/`edit_env` above return `Ok` (both write
        // atomically). The compensating half — removing the entry from the
        // request's own `[variables]` — only exists in the dirty editor
        // buffer until now; save it synchronously so "promote, then quit"
        // can't leave the old value stranded in both places. The project
        // write already succeeded, so a save failure here is reported as a
        // toast rather than an `Err` (which would incorrectly roll back an
        // op that, on the project side, already committed) — the removal
        // stays live in the editor buffer, just not yet on disk.
        if let Err(e) = self.save_open_request() {
            self.toasts.push(
                format!("promoted \"{name}\" but {e} \u{2014} save the request manually"),
                ToastKind::Error,
            );
        }
        Ok(())
    }

    /// [`VarStructOp::Demote`] (spec §4): writes the currently resolved
    /// value into the open request's `[variables]`, deletes the project
    /// declaration, and strips any flat value left behind in every
    /// environment (best-effort — an environment with nothing to strip is
    /// not an error). The caller (`open_demote_confirm`) has already
    /// refused a secret name or a group before this ever runs.
    fn apply_demote(&mut self, name: &str) -> Result<(), String> {
        let value = self
            .project
            .resolved
            .values
            .get(name)
            .cloned()
            .ok_or_else(|| format!("\"{name}\" has no resolved value to demote"))?;
        if self.editor.slug.is_none() {
            return Err("open a request to demote into".to_string());
        }
        // The fallible write goes first: `apply_var_struct`'s documented
        // "Err leaves everything unchanged" contract means the editor must
        // not gain a demoted entry unless the project actually lost the
        // declaration.
        self.project
            .edit_variables(|doc| postui_core::varedit::delete_var(doc, name))?;
        self.editor.variables.insert(
            name.to_string(),
            postui_core::model::Entry {
                value,
                enabled: true,
            },
        );
        // Finding 2: the destructive half (the declaration is gone) is
        // already durable via `edit_variables` above. The compensating
        // half — the request now carrying the value — only exists in the
        // dirty editor buffer so far; save it synchronously right after the
        // buffer mutation, and before the (best-effort, already-`let _ =`)
        // env strip below, so "demote, then quit" can't lose the value
        // everywhere. A save failure here is reported as a toast, not an
        // `Err` — the declaration is genuinely gone either way, and rolling
        // that back would be a second, separate write this function isn't
        // set up to attempt; the value is still safe in the dirty editor
        // buffer for a manual save.
        if let Err(e) = self.save_open_request() {
            self.toasts.push(
                format!("demoted \"{name}\" but {e} \u{2014} save the request manually"),
                ToastKind::Error,
            );
        }
        for env in self.project.environments.clone() {
            let _ = self.project.edit_env(&env, |doc| {
                postui_core::varedit::set_env_value(doc, name, None)
            });
        }
        Ok(())
    }

    /// Synchronously persists the currently open request to disk, mirroring
    /// `Action::SaveRequest`'s slugged branch (no SaveAs prompt — every
    /// caller here already knows a slug is open). Used by ops (demote,
    /// promote, extract-to-request) whose spec-mandated "writes
    /// immediately" (spec §5) binds the request-file half of a
    /// MANAGER-driven mutation — unlike ordinary Vars-tab typing, which
    /// stays save-on-demand (plan-mandated) and never calls this.
    fn save_open_request(&mut self) -> Result<(), String> {
        let slug = self
            .editor
            .slug
            .clone()
            .ok_or_else(|| "no request is open".to_string())?;
        let req = self.editor.current_request();
        postui_core::storage::save_request(&self.project.root, &slug, &req)
            .map_err(|e| format!("could not save {slug}: {e}"))?;
        self.editor.mark_saved();
        self.refresh_sidebar();
        Ok(())
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

    /// Pushes a context menu anchored at the pointer: a `Modal::Dropdown`
    /// over a 1x1 anchor at `(x, y)`, so `draw_dropdown`'s existing
    /// flip-near-the-bottom and clamp-to-screen logic places it. Returns
    /// `false` (opening nothing) for an empty item list, so callers can
    /// hand over whatever they built without a guard of their own.
    pub fn open_context_menu(
        &mut self,
        x: u16,
        y: u16,
        items: Vec<crate::components::modal::MenuItem>,
    ) -> bool {
        use crate::components::modal::DropdownState;
        if items.is_empty() {
            return false;
        }
        let selected = DropdownState::first_enabled(&items);
        self.modals.push(Modal::Dropdown(DropdownState {
            anchor: ratatui::layout::Rect::new(x, y, 1, 1),
            items,
            selected,
            // Context menus are lists of commands, not of values, so no row
            // is "the current one" and nothing gets the ✓ marker.
            current: None,
        }));
        true
    }

    /// The context menu for a right-clicked `hit`, or `None` where a right
    /// click has nothing to offer (pane backgrounds, chrome, an already-open
    /// modal). The row-targeting flows the items dispatch
    /// (`PromptRenameRequest`, `ConfirmDeleteRequest`, `DuplicateRequest`,
    /// `ToggleSelectedFolder`) read `sidebar.selected`, which the right-click
    /// handler has already moved onto the clicked row.
    fn context_menu_for(&mut self, hit: &Hit) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::components::modal::MenuItem;
        let row = match hit {
            Hit::SidebarRow(i) | Hit::SidebarFolderArrow(i) => self.sidebar.rows.get(*i)?,
            _ => return None,
        };
        Some(match row {
            Row::Request {
                slug, broken: None, ..
            } => vec![
                MenuItem::new("Open", Action::OpenRequest(slug.clone())),
                MenuItem::new("Duplicate", Action::DuplicateRequest),
                MenuItem::new("Rename…", Action::PromptRenameRequest),
                MenuItem::new("Delete…", Action::ConfirmDeleteRequest),
            ],
            // A request whose file doesn't parse can't be loaded into the
            // editor, so "Open" is shown disabled rather than hidden — the
            // menu keeps its shape and the reason is one row away.
            Row::Request {
                slug,
                broken: Some(_),
                ..
            } => vec![
                MenuItem::disabled("Open"),
                MenuItem::new("Show error…", Action::ShowRequestError(slug.clone())),
                MenuItem::new("Duplicate", Action::DuplicateRequest),
                MenuItem::new("Rename…", Action::PromptRenameRequest),
                MenuItem::new("Delete…", Action::ConfirmDeleteRequest),
            ],
            Row::Folder { path, expanded, .. } => vec![
                MenuItem::new(
                    "New request here…",
                    Action::PromptNewRequestIn(path.clone()),
                ),
                MenuItem::new(
                    if *expanded { "Collapse" } else { "Expand" },
                    Action::ToggleSelectedFolder,
                ),
            ],
        })
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
            let open_request = self
                .editor
                .slug
                .is_some()
                .then(|| self.editor.current_request());
            if let Some(a) = self
                .varmanager
                .handle_key(ev, &self.project, open_request.as_ref())
            {
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
    ///
    /// Stops dispatching the remaining actions the moment one of them is a
    /// failed `Action::VarStruct` (`last_action_failed`) — a sequenced
    /// result like `PromptKind::NewVariableAndInsert`'s `[NewVar,
    /// InsertVarText]` must not run its later, dependent actions (e.g.
    /// inserting a `{{name}}` token) after the mutation they depend on
    /// (declaring `name`) was refused.
    fn apply_modal_result(&mut self, res: ModalResult) -> bool {
        let mut changed = res.close;
        if res.close {
            self.modals.pop();
        }
        if let Some(id) = &res.usage {
            self.usage.record(id, crate::usage::now());
        }
        for a in res.actions {
            self.last_action_failed = false;
            changed |= self.update(a);
            if self.last_action_failed {
                break;
            }
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
