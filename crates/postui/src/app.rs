use crate::action::{Action, CopyTarget, ExtractSource};
use crate::anim::{AnimKey, Anims, Easing, ListId, StripId};
use crate::components::editor::{Editor, EditorTab, SubFocus};
use crate::components::line_input::LineInput;
use crate::components::modal::{Modal, ModalResult, ModalStack, PromptKind};
use crate::components::response::{ResponseState, SYNC_PRETTY_BYTES, ViewMode};
use crate::components::sidebar::Row;
use crate::components::toast::{ToastKind, Toasts};
use crate::components::varmanager::{
    VarEditOp, VarManager, VarStructOp, VmDetail, VmFocus, var_edit_op_for,
};
use crate::components::{Component, sidebar::Sidebar};
use crate::hit::{Hit, HitMap, PointerShape, ScrollbarSpec};
use crate::keys::{KeyCombo, Keymap};
use crate::layout::PaneId;
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// The migration confirm modal's title (spec §3.3) — also how
/// `prompt_migration_if_pending` recognizes its own modal already on the
/// stack.
const MIGRATION_TITLE: &str = "Migrate variables";

/// Wall-clock dwell the caret must rest inside a `{{token}}` before its
/// tooltip appears -- matching the original 2 ticks @ the tick's nominal
/// 100ms period. Wall-clock rather than tick-counted because the tick
/// period is adaptive (16ms while anything animates, 100ms otherwise --
/// see `main.rs`), so a tick count would race up to ~6x fast whenever
/// something else on screen was mid-animation.
const CARET_TIP_DWELL: Duration = Duration::from_millis(200);

/// A variable tooltip to draw: which name, and the on-screen span of the
/// token it belongs to (the tooltip is placed against it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTip {
    pub name: String,
    pub anchor: ratatui::layout::Rect,
}

/// An in-progress scrollbar drag: which pane's thumb is held, and how far
/// along the thumb the pointer grabbed it, so the thumb keeps its position
/// under the cursor instead of jumping its edge to the pointer.
/// `horizontal` picks the axis: rows down a vertical track, or columns
/// along the bottom horizontal bar.
pub struct Drag {
    pub pane: PaneId,
    pub grab_offset: u16,
    pub horizontal: bool,
}

/// Which text surface a left-button drag is sweeping a selection over,
/// armed by the `Down(Left)` that started the sweep and cleared on
/// `Up(Left)` — the text-selection sibling of the scrollbar [`Drag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDrag {
    Body,
    Url,
    Response,
    /// A sweep inside the response pane's jq filter bar.
    Jq,
    /// A sweep inside the top modal's text box `i` (see `Hit::ModalInput`).
    ModalInput(usize),
    /// A sweep inside the table cell under edit (`TableEditorState::editing`
    /// names which one).
    TableCell,
    /// A sweep inside the variable form's field under edit
    /// (`VarFormState::editing` names which one).
    VmField,
    /// A sweep inside the selector-grid cell under edit
    /// (`OptionGridState::editing` names which one).
    VmCell,
}

/// Whether `n` can't be a new declaration's name: one of the reserved
/// table names, or a variable / selector that already exists.
fn name_taken(ctx: &ProjectContext, n: &str) -> bool {
    n == "options"
        || n == "groups"
        || n == "entries"
        || n == "selectors"
        || ctx.model.vars.contains_key(n)
        || ctx.model.selectors.contains_key(n)
}

/// The option-name seed for "Extract to selector": the value itself when
/// it is short and reads as a name (letters, digits, `-`, `_`, `.`, spaces),
/// so `en` or `user 1` arrive ready to confirm; blank for a UUID, a URL or
/// anything long, which would make a poor option name.
fn option_name_seed(text: &str) -> String {
    let text = text.trim();
    let name_like = text.chars().count() <= 24
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '));
    if name_like {
        text.to_string()
    } else {
        String::new()
    }
}

/// Which full-frame screen is showing. `ui::draw` and `App::handle_key`
/// each branch on this once; every screen but `Main` replaces the three
/// panes with its own full-frame draw while the header and footer stay.
/// This is also where future screens (history, console) slot in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Main,
    /// The Manage screen (spec §5): a tabbed shell over Variables,
    /// Environments and Spaces. Which tab is up lives in `App::manage`.
    Manage,
    /// The hidden primitive showcase, entered at startup when the
    /// `POSTUI_TESTBED` env var is set (see [`App::new`]). A static grid of
    /// every painted primitive in every state, for judging the visual
    /// language against — never entered any other way, and Esc/`q` quit the
    /// app outright rather than returning to `Main`.
    Testbed,
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
    /// The Manage screen's shell state (which tab is up), shown
    /// full-frame while `screen == Screen::Manage`.
    pub manage: crate::components::manage::Manage,
    /// The Variables tab's own state — the list, detail pane and edits.
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
    /// The same `config.toml` path, for `Action::SetAiConfirmed` to persist
    /// through `save_ui_flag`. `None` in tests, same posture as
    /// `registry_path`.
    config_path: Option<PathBuf>,
    /// The tiered clipboard (external command / OS clipboard / OSC 52),
    /// configured from `ui_settings`.
    pub clipboard: crate::clipboard::Clipboard,
    /// Mouse-first-GUI UI settings (clipboard command, OSC 52 threshold),
    /// loaded from the same `config.toml` the registry uses.
    pub ui_settings: crate::config::UiSettings,
    /// The loaded theme registry (built-ins + custom `themes/*.toml`
    /// files) — rescanned when the picker opens, so a file dropped in
    /// mid-session shows up without a restart.
    pub themes: crate::theme::ThemeRegistry,
    /// The startup OSC answer, cached because the query can only run once,
    /// before crossterm's event reader exists.
    pub terminal_colors: crate::theme::QueriedColors,
    /// The currently-applied theme's registry name.
    pub theme_name: String,
    /// While the theme picker is open: the theme name to restore if the
    /// picker closes without a commit. `None` whenever the picker isn't up.
    theme_preview: Option<String>,
    /// The theme picker's polarity filter: `true` shows dark themes,
    /// `false` light ones. Set from the applied theme when the picker
    /// opens; flipped by `Action::ToggleThemePickerPolarity`. Meaningless
    /// while the picker is closed.
    theme_picker_dark: bool,
    /// The custom-themes directory, `None` when no config dir resolved for
    /// this platform.
    pub themes_dir: Option<PathBuf>,
    /// Eased animated values (tab underline, hover fade, ...), constructed
    /// from `ui_settings.animations`. Time is always passed in by the
    /// caller — `Action::Tick`'s handler and `DrawCtx::now` both sample
    /// `Instant::now()` themselves — so `Anims` stays fully deterministic
    /// and testable.
    pub anims: Anims,
    /// Whether [`Self::animating`] was still true as of the *previous*
    /// `Action::Tick`. An animation's very last tick always sees
    /// `animating() == false` by the time it runs (its duration has just
    /// elapsed), so `Action::Tick`'s redraw decision can't rely on
    /// `animating()` alone without missing that final settle frame — the
    /// one where a gated modal/dropdown finally reveals its contents (see
    /// `Action::Tick`'s handler). Tracking the prior tick's reading lets
    /// that transition (active → finished) still force one more redraw.
    animating_last_tick: bool,
    /// The active key bindings (defaults + `keys.toml` overrides), used at
    /// draw time for the palette's keybinding column (`keys::combo_for`).
    /// `main.rs`'s event loop loads its own copy for `handle_key` — this one
    /// exists purely for read-only lookups during drawing, since `App`
    /// otherwise has no way to reach the keymap from inside `Component::draw`.
    pub keymap: crate::keys::Keymap,
    /// Palette command frecency stats (recency + count per command id),
    /// loaded from `ui.toml` at startup and saved back on quit.
    pub usage: crate::usage::UsageStore,
    /// Where to save `usage` back to. `None` in tests, so test runs never
    /// touch the real `ui.toml`.
    usage_path: Option<PathBuf>,
    /// The HTTP clients used for every send (verifying + insecure, picked
    /// per request). Built eagerly and cheaply
    /// (`reqwest::Client::builder().build()` needs no running Tokio
    /// reactor — verified in `http::tests::client_builds_without_a_tokio_runtime`
    /// — so `App` stays constructible in the many plain `#[test]`s that
    /// never touch the network).
    pub clients: crate::http::Clients,
    /// Sender for background tasks (e.g. in-flight requests) to push
    /// `Action`s back into the main loop without blocking on it.
    pub tx: UnboundedSender<Action>,
    /// The outstanding "describe a filter" AI task, if any: its request
    /// counter (matched against `JqAiFinished::request`) and join handle
    /// (aborted by `CancelJqDescribe`, and by a new `RunJqDescribe`).
    pub ai_task: Option<(u64, tokio::task::JoinHandle<()>)>,
    /// Counts every AI request ever started, so a finished background task
    /// can be matched to the request it was started for (and only that
    /// one — a cancelled or superseded one is dropped).
    ai_request: u64,
    /// An action that can only be applied by suspending the terminal, parked
    /// here by `update` for the main loop to take and run. Keeps `update`
    /// itself terminal-free (and therefore testable without a TTY).
    pub pending_terminal_action: Option<Action>,
    /// Rebuilt every frame by `ui::draw`: maps screen regions to typed
    /// [`Hit`]s for mouse routing.
    pub hits: HitMap,
    /// The `Hit` currently under the pointer, if any, updated by
    /// `handle_mouse` on `Moved`. Read by `ui::draw` to style hovered
    /// buttons/chips. `Hit::VarToken` overlays are deliberately skipped
    /// here (see `HitMap::hit_at_ignoring_var_tokens`) — they are tracked
    /// by `hovered_token` instead, so crossing a `{{token}}` never takes a
    /// row's or a button's hover styling away from it.
    pub hovered: Option<Hit>,
    /// Whether the terminal can report Shift+Enter (kitty keyboard
    /// protocol). Set once by `main` after probing; only affects which
    /// send key the footer advertises — the bindings themselves are
    /// always active.
    pub shift_enter_send: bool,
    /// The drawn `{{token}}` under the pointer as of the last motion event.
    /// Only a redraw trigger: the tooltip itself re-resolves `pointer`
    /// against the *current* frame's hit map, so a token that scrolled or
    /// tabbed out from under a resting pointer takes its tooltip with it.
    hovered_token: Option<String>,
    /// Set when a sidebar right-click moved `sidebar.selected` onto the
    /// clicked row to open its context menu: the selection to restore
    /// (`Some(prev)`, itself possibly `None`) if that menu is dismissed
    /// without choosing anything — clicking off / Esc must not leave the
    /// cursor marker stranded on a row the user never acted on. Cleared
    /// whenever any dropdown closes; a menu item actually chosen keeps the
    /// moved selection, since its flow acts on (and usually re-points) it.
    pub(crate) sidebar_menu_revert: Option<Option<usize>>,
    /// Set while a confirmed modal's own actions are running, after its
    /// close emptied the stack: a modal those actions push is a *handoff*
    /// (the new-selector name prompt chaining into its first-option
    /// prompt, a save-as gate chaining onward), not a fresh open. Without
    /// it `push_modal` would see an empty stack and replay the open
    /// settle — the backdrop un-dimming and re-dimming behind a
    /// momentarily blank panel, which reads as a flash.
    modal_handoff: bool,
    /// The terminal pointer shape last emitted (Kitty OSC 22, task 8d).
    /// Starts at `Default` — the terminal's own cursor is already that, so
    /// startup emits nothing until the pointer first moves onto something
    /// else. Read and updated only by `pointer_shape_update` (`app/mouse.rs`).
    last_pointer_shape: PointerShape,
    /// Where the pointer last was, so the tooltip can be re-resolved every
    /// frame rather than trusting a rect captured at motion time.
    pointer: Option<(u16, u16)>,
    /// The token the keyboard caret is resting in, and when it started
    /// resting there. The tooltip appears once it's been resting
    /// [`CARET_TIP_DWELL`], so a caret merely passing through a token on
    /// its way somewhere else never flashes one up.
    caret_token: Option<String>,
    caret_token_since: Option<Instant>,
    /// Whether the tooltip for `caret_token` has already crossed
    /// [`CARET_TIP_DWELL`] and is showing -- tracked separately from
    /// `caret_token_since` so `track_caret_token` can report the one tick
    /// the dwell threshold is crossed (for the redraw) without re-reporting
    /// it on every later tick while still resting in the same token.
    caret_tip_shown: bool,
    /// An in-progress drag (e.g. a scrollbar thumb), if any.
    pub drag: Option<Drag>,
    /// A live text-selection sweep (which surface it is over), or `None`.
    pub text_drag: Option<TextDrag>,
    /// A left press on a sidebar request row: `(row index, slug)`. Armed
    /// by `on_hit`, cleared on release; becomes a live row drag the moment
    /// the pointer moves onto another row (see `mouse.rs`).
    pub sidebar_press: Option<(usize, String)>,
    /// A left press on a Manage screen Spaces row: `(row index, space)`.
    /// The Spaces-tab twin of `sidebar_press` — armed by `on_hit` (Spaces
    /// tab only), cleared on release, promoted to a live row drag the
    /// moment the pointer moves onto another row (see `mouse.rs`).
    pub manage_press: Option<(usize, String)>,
    /// Whether the active tab's params/headers table body is collapsed
    /// (tab strip + its count chip stay visible; only the table itself is
    /// hidden). Session-only — never persisted.
    pub table_collapsed: bool,
    /// The last `table_collapsed` value `AnimKey::PaneCollapse` was driven
    /// toward, tracked so `update` can tell when it flips
    /// (`Action::ToggleTableCollapse`) and start easing the layout split
    /// rather than snapping it every frame it's still settled.
    pane_collapsed_target: bool,
    /// Mirror of [`Self::pane_collapsed_target`] for the Response pane's
    /// collapse anim — see [`Self::sync_response_collapse_anim`].
    response_collapsed_target: bool,
    /// The editor's share of the main column while both panes are shown —
    /// the split's settled ratio stop (see [`crate::split`]). Sticky
    /// through minimize/expand so re-opening lands where the user left it.
    pub split_ratio: crate::split::SplitRatio,
    /// The last `split_ratio` value `AnimKey::SplitRatio` was driven
    /// toward — see [`Self::sync_split_ratio_anim`].
    split_ratio_target: crate::split::SplitRatio,
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
    /// The loose-file half of the last `list_requests` warning that was
    /// toasted. A file directly under `requests/` is a chronic state (it is
    /// never migrated for the user), so re-toasting it on every refresh
    /// would be a red banner on every save/open/delete. Kept here so the
    /// warning surfaces once per distinct set of loose files, and reset
    /// whenever the project root changes.
    last_loose_warning: Option<String>,
    /// The same warn-once channel as `last_loose_warning`, for entries in
    /// `project.toml`'s `spaces` that aren't valid space names. They are
    /// never rewritten away (the user's file stays the user's), so they
    /// too are a chronic state that must not re-toast on every refresh.
    last_spaces_warning: Option<String>,
    /// Keeps the test-only channel's receiver alive so `tx` doesn't become
    /// a dangling sender in `App::new_for_test()`. Always `None` outside
    /// of tests.
    _test_rx: Option<UnboundedReceiver<Action>>,
    /// Owns (and, on drop, removes) the throwaway project directory made by
    /// `App::new_for_test()`. Always `None` outside of tests.
    _test_dir: Option<tempfile::TempDir>,
    /// The undo/redo stacks. Populated by `capture_undo` and by later
    /// tasks' Undo/Redo apply arms.
    pub history: crate::undo::History,
    /// The open request as of the last `capture_undo` call (with its slug),
    /// diffed against the live editor each call to detect edits that never
    /// went through an `Action`. `None` before the first request is open.
    shadow: Option<(Option<String>, postui_core::model::HttpRequest)>,
    /// The cursor position captured alongside `shadow`, so a recorded step's
    /// `cursor_before` reflects where the cursor sat before this burst of
    /// edits began, not just before the immediately preceding keystroke.
    shadow_cursor: crate::undo::CursorPos,
    /// Set by wholesale-change arms (format/minify, discard, method change,
    /// insert-var, `$EDITOR` round-trip, table row delete/duplicate) so the
    /// next `capture_undo` records a standalone, non-coalescing step and
    /// clears redo, even mid-typing-burst. Consumed (reset to `false`) by
    /// `capture_undo` on every call.
    no_coalesce: bool,
    /// Step direction (`1` = downward, `-1` = upward) the testbed screen's
    /// two looping list-travel duration-comparison demos are each
    /// currently stepping in (`_plan` for the 100ms/plan copy, `_alt` for
    /// the 250ms alternative). Only ever read/written from
    /// `Screen::Testbed`'s tick path (`tick_testbed_demos`); meaningless
    /// (and untouched) everywhere else.
    testbed_list_dir_plan: i32,
    testbed_list_dir_alt: i32,
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

/// What `App::enter_space` should do with the space it is leaving.
#[derive(Debug, Clone, Copy)]
enum SpaceExit<'a> {
    /// The editor still holds the outgoing space's request: remember it
    /// (`None` — nothing open — clears the entry). Every normal switch.
    Remember(Option<&'a str>),
    /// Leave the outgoing space's remembered request untouched. Used by
    /// the undo-follow paths, where the editor has *already* been moved to
    /// the incoming space's slug and so describes the destination, not the
    /// space being left.
    Keep,
}

/// The theme picker's title-row toggle label for the given polarity —
/// names the set currently shown, with arrows advertising Left/Right.
fn theme_picker_toggle_label(dark: bool) -> String {
    if dark {
        "◂ dark ▸".into()
    } else {
        "◂ light ▸".into()
    }
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
        let themes_dir = crate::config::themes_dir_path();
        let (themes, theme_warnings) = crate::theme::ThemeRegistry::load(themes_dir.as_deref());
        let terminal_colors = {
            use crate::theme::TerminalPalette;
            crate::theme::OscQuery.query()
        };
        let (theme_name, theme) = match themes.resolve(&ui_settings.theme, &terminal_colors) {
            Some(t) => (ui_settings.theme.clone(), t),
            None => (
                "terminal".to_string(),
                themes
                    .resolve("terminal", &terminal_colors)
                    .expect("terminal is always registered"),
            ),
        };
        let mut ui_warnings = ui_warnings;
        ui_warnings.extend(theme_warnings); // malformed custom theme files surface as startup toasts
        if theme_name != ui_settings.theme {
            ui_warnings.push(format!(
                "unknown theme {:?} in config.toml; using terminal",
                ui_settings.theme
            ));
        }
        let usage_path = crate::config::ui_file_path();
        let usage = usage_path
            .as_deref()
            .map(crate::usage::UsageStore::load_from)
            .unwrap_or_default();

        let testbed = std::env::var_os("POSTUI_TESTBED").is_some();

        let Some((root, disposition, stale_last)) = resolve_startup(
            &registry,
            cli_root,
            postui_core::storage::default_project_dir(),
        ) else {
            let mut app = Self::bare(tx, PathBuf::new());
            app.registry = registry;
            app.registry_path = registry_path.clone();
            app.config_path = registry_path;
            app.themes = themes;
            app.terminal_colors = terminal_colors;
            app.themes_dir = themes_dir;
            app.apply_ui_settings(ui_settings, theme_name, theme);
            app.usage = usage;
            app.usage_path = usage_path;
            let (keymap, key_warnings) =
                crate::keys::Keymap::load_with_warnings(cfg!(target_os = "macos"));
            app.keymap = keymap;
            for w in ui_warnings.into_iter().chain(key_warnings) {
                app.toasts.push(w, ToastKind::Warning);
            }
            app.toasts.push(
                "could not determine a project directory for this platform",
                ToastKind::Error,
            );
            if testbed {
                app.screen = Screen::Testbed;
            }
            return app;
        };

        let mut app = Self::with_root(tx, root);
        app.registry = registry;
        app.registry_path = registry_path.clone();
        app.config_path = registry_path;
        app.themes = themes;
        app.terminal_colors = terminal_colors;
        app.themes_dir = themes_dir;
        app.apply_ui_settings(ui_settings, theme_name, theme);
        app.usage = usage;
        app.usage_path = usage_path;
        let (keymap, key_warnings) =
            crate::keys::Keymap::load_with_warnings(cfg!(target_os = "macos"));
        app.keymap = keymap;
        for w in ui_warnings.into_iter().chain(key_warnings) {
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
                app.init_default_project();
            }
            StartupDisposition::PromptCreate => {
                let path = app.project.root.display().to_string();
                let fallback_actions = match postui_core::storage::default_project_dir() {
                    Some(fallback) => vec![Action::SwitchProject(fallback)],
                    None => vec![],
                };
                app.push_modal(Modal::Confirm {
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

        if testbed {
            app.screen = Screen::Testbed;
        }

        app
    }

    /// The `StartupDisposition::InitDefault` tail: writes `project.toml`
    /// into the platform default directory and registers it.
    ///
    /// `with_root` already ran `ensure_project`, but on a *bare* directory
    /// — with no `project.toml` it can only make `requests/main`. The
    /// second `ensure_project` here, after `init_project` has written the
    /// file, is what seeds `spaces = ["main"]` (same order as
    /// `Action::InitProjectHere`).
    fn init_default_project(&mut self) {
        let _ = postui_core::project::init_project(&self.project.root, Some("default"));
        if let Err(e) = postui_core::storage::ensure_project(&self.project.root) {
            self.toasts
                .push(format!("could not open project: {e}"), ToastKind::Error);
        }
        self.project.reload_meta();
        self.project.reload_spaces();
        self.registry.register(self.project.root.clone());
        if let Some(path) = &self.registry_path {
            let _ = self.registry.save_to(path);
        }
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
                // The context was opened before `ensure_project` seeded
                // `main`, so the space list it read can be stale.
                app.project.reload_spaces();
                app.refresh_sidebar();
                // Restore this project's saved layout split alongside its
                // open request.
                app.seed_split_from_project();
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
        self.push_modal(Modal::Confirm {
            title: MIGRATION_TITLE.into(),
            body: lines.join("\n"),
            choices: vec![
                ('n', "Not now".into(), vec![Action::DeclineMigration]),
                ('y', "Migrate".into(), vec![Action::ApplyMigration]),
            ],
        });
    }

    /// Applies a freshly loaded `UiSettings` (and the `Theme` derived from
    /// it) to every field that depends on it: the clipboard tier, the
    /// animation state, and `ui_settings`/`theme` themselves. Both of
    /// `App::new`'s branches (a resolved project root, and the no-root
    /// fallback) call this instead of assigning each dependent field
    /// separately, so a new `UiSettings`-derived field can't be wired into
    /// one branch and silently forgotten in the other.
    fn apply_ui_settings(
        &mut self,
        ui_settings: crate::config::UiSettings,
        theme_name: String,
        theme: Theme,
    ) {
        self.clipboard = crate::clipboard::Clipboard::new(&ui_settings);
        self.anims = Anims::new(ui_settings.animations);
        self.theme = theme;
        self.theme_name = theme_name;
        self.session.response.set_jq_tab(ui_settings.jq_tab);
        self.ui_settings = ui_settings;
    }

    /// Applies the named theme from the registry (the Terminal option seeds
    /// from the startup query). An unknown name — a custom file deleted since
    /// the registry was built — degrades to the terminal theme.
    fn set_theme_by_name(&mut self, name: &str) {
        let (resolved, theme) = match self.themes.resolve(name, &self.terminal_colors) {
            Some(t) => (name.to_string(), t),
            None => (
                "terminal".to_string(),
                self.themes
                    .resolve("terminal", &self.terminal_colors)
                    .expect("terminal is always registered"),
            ),
        };
        self.theme = theme;
        self.theme_name = resolved;
    }

    /// Live preview: while the theme picker is open, keep the applied theme
    /// in lockstep with the picker's highlighted row. Called after any key
    /// or click that may have moved the chooser's selection.
    /// Builds the theme picker's rows for the current polarity filter
    /// (`theme_picker_dark`): one row per registry option whose seed
    /// background matches, in registry order.
    fn theme_picker_items(&self) -> Vec<crate::components::chooser::ChooserItem> {
        self.themes
            .entries()
            .iter()
            .filter(|e| {
                self.themes.entry_is_dark(e, &self.terminal_colors) == self.theme_picker_dark
            })
            .map(|e| {
                let detail = match &e.source {
                    crate::theme::ThemeSource::Terminal => "terminal colors",
                    crate::theme::ThemeSource::Builtin(_) => "built-in",
                    crate::theme::ThemeSource::Custom(_) => "custom",
                };
                crate::components::chooser::ChooserItem {
                    label: e.label.clone(),
                    detail: Some(detail.into()),
                    actions: vec![Action::ApplyTheme(e.name.clone())],
                    id: Some(e.name.clone()),
                }
            })
            .collect()
    }

    fn sync_theme_preview(&mut self) {
        if self.theme_preview.is_none() {
            return;
        }
        let Some(Modal::Chooser(c)) = self.modals.top() else {
            return;
        };
        // Filter matched nothing: keep the last previewed theme (and the
        // toggle label — the highlight hasn't actually moved anywhere).
        let Some(id) = c.selected_id().map(str::to_string) else {
            return;
        };
        if id != self.theme_name {
            self.set_theme_by_name(&id);
        }
        // The light/dark switch only exists for the highlighted theme when
        // it has a counterpart: hide its label otherwise (Terminal, lone
        // customs), so the picker never advertises a dead control.
        let label = if self
            .themes
            .get(&id)
            .is_some_and(|e| e.counterpart.is_some())
        {
            theme_picker_toggle_label(self.theme_picker_dark)
        } else {
            String::new()
        };
        if let Some(Modal::Chooser(state)) = self.modals.top_mut() {
            state.set_toggle_label(label);
        }
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
            manage: crate::components::manage::Manage::default(),
            varmanager: VarManager::default(),
            session: crate::session::Session::default(),
            toasts,
            modals: ModalStack::default(),
            project,
            registry: crate::config::ProjectsRegistry::default(),
            registry_path: None,
            config_path: None,
            clipboard: crate::clipboard::Clipboard::new(&crate::config::UiSettings::default()),
            ui_settings: crate::config::UiSettings::default(),
            themes: crate::theme::ThemeRegistry::builtin(),
            terminal_colors: crate::theme::QueriedColors::default(),
            theme_name: "terminal".into(),
            theme_preview: None,
            theme_picker_dark: true,
            themes_dir: None,
            anims: Anims::new(crate::config::UiSettings::default().animations),
            animating_last_tick: false,
            keymap: crate::keys::Keymap::default_bindings(),
            usage: crate::usage::UsageStore::default(),
            usage_path: None,
            clients: crate::http::Clients::new(),
            tx,
            ai_task: None,
            ai_request: 0,
            pending_terminal_action: None,
            hits: HitMap::default(),
            hovered: None,
            shift_enter_send: false,
            hovered_token: None,
            sidebar_menu_revert: None,
            modal_handoff: false,
            last_pointer_shape: PointerShape::Default,
            pointer: None,
            caret_token: None,
            caret_token_since: None,
            caret_tip_shown: false,
            drag: None,
            text_drag: None,
            sidebar_press: None,
            manage_press: None,
            table_collapsed: false,
            pane_collapsed_target: false,
            response_collapsed_target: false,
            split_ratio: crate::split::SplitRatio::default(),
            split_ratio_target: crate::split::SplitRatio::default(),
            last_click: None,
            last_action_failed: false,
            last_loose_warning: None,
            last_spaces_warning: None,
            _test_rx: None,
            _test_dir: None,
            history: crate::undo::History::new(),
            shadow: None,
            shadow_cursor: crate::undo::CursorPos::None,
            no_coalesce: false,
            testbed_list_dir_plan: 1,
            testbed_list_dir_alt: 1,
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

    /// Like [`Self::new_for_test`], but takes an explicit `testbed` flag
    /// instead of reading `POSTUI_TESTBED` — tests must never depend on
    /// process-global env state. `true` opens straight onto
    /// [`Screen::Testbed`], exactly like a real startup with the env var
    /// set.
    pub fn new_for_test_with_testbed(testbed: bool) -> Self {
        let mut app = Self::new_for_test();
        if testbed {
            app.screen = Screen::Testbed;
        }
        app
    }

    /// Like [`Self::new_for_test`], but with `self.anims` freshly built as
    /// `Anims::new(enabled)` — needed by tests that exercise an animated
    /// retarget (e.g. the tab-underline slide, Task 10) with animations
    /// deliberately on, regardless of `UiSettings::default().animations`.
    pub fn new_for_test_with_anims(enabled: bool) -> Self {
        let mut app = Self::new_for_test();
        app.anims = Anims::new(enabled);
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

/// The blocking-pool half of a background jq run: parse the body if the
/// view has no cached document yet, run, and flatten the outputs into
/// their tree (the slow part on a big output). Free-standing so tests can
/// call it without a runtime, and so `App::spawn_jq_run` can run it inline
/// when there is no runtime to spawn onto.
pub(crate) fn jq_worker(
    generation: u64,
    run: u64,
    code: String,
    doc: Option<postui_core::jq::JqDocument>,
    body: Option<String>,
) -> Action {
    use postui_core::jq::{JqDocument, run as jq_run};
    let result = (|| {
        let (doc, fresh) = match (doc, body) {
            (Some(d), _) => (d, None),
            (None, Some(b)) => {
                let d = JqDocument::parse(&b)?;
                (d.clone(), Some(d))
            }
            (None, None) => {
                return Err(postui_core::jq::JqError::Runtime {
                    message: "no document".into(),
                });
            }
        };
        jq_run(&code, &doc)
            .map(|out| crate::components::response::JqRunOutput::from_outputs(fresh, out))
    })();
    Action::JqRunFinished {
        generation,
        run,
        result,
    }
}

/// The blocking-pool half of a completion key fetch. Free-standing for
/// the same reasons as [`jq_worker`].
pub(crate) fn jq_complete_worker(
    generation: u64,
    seq: u64,
    input_expr: String,
    doc: postui_core::jq::JqDocument,
) -> Action {
    let keys = postui_core::jq::complete::keys_at(&input_expr, &doc);
    Action::JqCompleteFinished {
        generation,
        seq,
        input_expr,
        keys,
    }
}

impl App {
    /// Applies `action` to app state, then reconciles the jq filter bar
    /// with the editor and view (see [`Self::sync_jq`]) — the one place
    /// every action, key, mouse hit, and background result passes through,
    /// so the reconcile runs exactly once per action regardless of route.
    /// Returns `true` if state changed in a way that requires a redraw,
    /// `false` if the caller can skip drawing this iteration.
    pub fn update(&mut self, action: Action) -> bool {
        let changed = self.dispatch(action);
        self.sync_jq();
        changed
    }

    /// Keeps the response pane's jq bar and the editor's `jq` field in
    /// step, and the view in step with both — the one place the filter is
    /// applied. Bar edits flow into the editor (they are request edits);
    /// everything else (open request, undo, discard, a new response) flows
    /// the editor into the bar. Blurs the bar whenever focus has moved off
    /// the response pane. Cheap when nothing changed: a couple of string
    /// compares.
    pub(crate) fn sync_jq(&mut self) {
        if self.focus != PaneId::Response {
            self.session.response.set_jq_focus(false);
        }
        let response = &mut self.session.response;
        if response.take_jq_edited() {
            self.editor.jq = response.jq_text().to_string();
            self.editor.jq_enabled = response.jq_enabled();
        } else {
            if response.jq_text() != self.editor.jq {
                response.set_jq_text(&self.editor.jq);
            }
            if response.jq_enabled() != self.editor.jq_enabled {
                response.set_jq_enabled(self.editor.jq_enabled);
            }
        }
        // A switched-off filter is applied as no filter: the text stays in
        // the bar (closed), the tree shows the full body.
        let code = if self.editor.jq_enabled {
            self.editor.jq.clone()
        } else {
            String::new()
        };
        if let Some(req) = self
            .session
            .response
            .apply_jq(&code, crate::components::response::SYNC_PRETTY_BYTES)
        {
            self.spawn_jq_run(req);
        }
        if let Some(req) = self
            .session
            .response
            .refresh_jq_completion(crate::components::response::SYNC_PRETTY_BYTES)
        {
            self.spawn_jq_complete(req);
        }
    }

    /// Hands a body too big to parse on the UI thread to a blocking
    /// worker, whose result comes back as `PrettyParsed` tagged with
    /// `generation`. Outside a tokio runtime (`App::new_for_test`) the
    /// parse runs inline and its result is dispatched immediately.
    fn spawn_pretty_parse(&mut self, generation: u64, body: String) {
        use crate::components::json_tree::JsonTree;
        if tokio::runtime::Handle::try_current().is_err() {
            let tree = JsonTree::parse(&body);
            self.dispatch(Action::PrettyParsed {
                generation,
                tree: tree.map(Box::new),
            });
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let tree = tokio::task::spawn_blocking(move || JsonTree::parse(&body))
                .await
                .ok()
                .flatten();
            let _ = tx.send(Action::PrettyParsed {
                generation,
                tree: tree.map(Box::new),
            });
        });
    }

    /// Hands a background-pool jq run to the runtime. `App::new_for_test`
    /// builds its channel outside a tokio runtime, so tests run the worker
    /// inline and dispatch its result immediately instead of spawning.
    fn spawn_jq_run(&mut self, req: crate::components::response::JqRunRequest) {
        if tokio::runtime::Handle::try_current().is_err() {
            let action = jq_worker(req.generation, req.run, req.code, req.doc, req.body);
            self.dispatch(action);
            return;
        }
        let tx = self.tx.clone();
        let (generation, run) = (req.generation, req.run);
        tokio::spawn(async move {
            let action = tokio::task::spawn_blocking(move || {
                jq_worker(req.generation, req.run, req.code, req.doc, req.body)
            })
            .await
            .unwrap_or_else(|_| Action::JqRunFinished {
                generation,
                run,
                result: Err(postui_core::jq::JqError::Runtime {
                    message: "filter worker panicked".into(),
                }),
            });
            let _ = tx.send(action);
        });
    }

    /// Hands a completion key fetch to the runtime; inline (result
    /// dispatched immediately) outside one, like [`Self::spawn_jq_run`].
    fn spawn_jq_complete(&mut self, req: crate::components::response::JqCompleteRequest) {
        if tokio::runtime::Handle::try_current().is_err() {
            let action = jq_complete_worker(req.generation, req.seq, req.input_expr, req.doc);
            self.dispatch(action);
            return;
        }
        let tx = self.tx.clone();
        let (generation, seq) = (req.generation, req.seq);
        let expr = req.input_expr.clone();
        tokio::spawn(async move {
            let action = tokio::task::spawn_blocking(move || {
                jq_complete_worker(req.generation, req.seq, req.input_expr, req.doc)
            })
            .await
            .unwrap_or_else(|_| Action::JqCompleteFinished {
                generation,
                seq,
                input_expr: expr,
                keys: Vec::new(),
            });
            let _ = tx.send(action);
        });
    }

    /// Applies `action` to app state. Returns `true` if state changed in a
    /// way that requires a redraw, `false` if the caller can skip drawing
    /// this iteration.
    fn dispatch(&mut self, action: Action) -> bool {
        let changed = self.apply(action);
        // Keeps the sidebar's dirty dot and its notion of "which slug is
        // open" in lockstep with the editor after every action, rather than
        // threading that bookkeeping through each arm individually.
        self.sidebar.open_slug = self.editor.slug.clone();
        self.sidebar.open_dirty = self.editor.is_dirty();
        self.sidebar.in_flight = self
            .session
            .in_flight
            .iter()
            .filter_map(|f| f.slug.clone())
            .collect();
        self.editor.inherited_headers = self.project.meta.default_headers.clone();
        self.editor.shadowed = self.compute_shadowed();
        // The token-highlighting/tooltip snapshot, kept in lockstep with the
        // project's resolved values and the open request's own `[variables]`
        // the same way `shadowed` is.
        let vars = crate::components::var_tokens::VarView::from_context(
            &self.project,
            &self.editor.variables,
        );
        self.editor.vars = vars;
        // The response pane always shows the open request's response;
        // whenever an action changed which request is open (any route),
        // swap in that request's cached response — or an empty one. The
        // collapse flag rides along (a layout preference, not response
        // state), so a swap can't break the no-blank-screen rule.
        let swapped = self.session.sync_open(&self.editor.slug);
        // The swapped-in response's active tab is whatever it was when the
        // request was left (or a fresh default) — no `ResponseViewMode`
        // ran, so the underline glide from the outgoing response is stale.
        if swapped {
            self.reset_response_tab_underline();
            // A response that shed its tree while cached (see
            // `Session::KEEP_WARM`) gets the same background parse a
            // fresh arrival does.
            if let Some((generation, body)) = self.session.response.take_reparse() {
                self.spawn_pretty_parse(generation, body);
            }
        }
        // The send button shows "sending" only when the in-flight send
        // belongs to the request being looked at.
        let editor_in_flight = self.session.in_flight_for(&self.editor.slug);
        self.editor.sending = editor_in_flight.is_some();
        self.editor.send_started = editor_in_flight.map(|f| f.started);
        self.editor.table_collapsed = self.table_collapsed;
        // Display copies for the panes' split clusters — the app state
        // stays the authority, the components only light segments from it.
        self.editor.split = self.split_state();
        self.session.response.split = self.editor.split;
        self.sync_pane_collapse_anim();
        self.sync_response_collapse_anim();
        self.sync_split_ratio_anim();
        self.sync_editor_tab_underline();
        // Any toast pushed by `apply(action)` above gets its slide-in
        // started here rather than inside `Toasts::push` itself — `push`
        // is called from ~100 sites across this file, none of which
        // otherwise need `&mut self.anims`/`ui_settings` in scope.
        self.toasts.start_pending_anims(
            &mut self.anims,
            Instant::now(),
            self.ui_settings.anim_ms.toast,
        );
        changed || swapped
    }

    /// Keeps `AnimKey::PaneCollapse` chasing `table_collapsed` (the same
    /// condition `ui::draw` uses — hide applies on every tab, the Body
    /// buffer included): whenever it flips
    /// (`Action::ToggleTableCollapse`/the `⌄ hide` chip), retargets the
    /// anim from wherever it currently sits to the new pole over
    /// `ui_settings.anim_ms.pane_collapse` (120ms by default). Called on
    /// every `update`, not just the toggle action.
    fn sync_pane_collapse_anim(&mut self) {
        let target = self.table_collapsed;
        if target == self.pane_collapsed_target {
            return;
        }
        self.pane_collapsed_target = target;
        let now = Instant::now();
        let target_v = if target { 1.0 } else { 0.0 };
        if self.anims.value(AnimKey::PaneCollapse, now).is_none() {
            self.anims.snap(AnimKey::PaneCollapse, 1.0 - target_v);
        }
        self.anims.retarget(
            AnimKey::PaneCollapse,
            target_v,
            self.ui_settings.anim_ms.pane_collapse,
            now,
        );
    }

    /// Keeps `AnimKey::ResponseCollapse` chasing the response pane's
    /// `collapsed` flag the same way [`Self::sync_pane_collapse_anim`]
    /// chases `table_collapsed`. The flag lives on `session.response` but
    /// is a sticky layout preference (`session.sync_open` carries it
    /// across request switches).
    fn sync_response_collapse_anim(&mut self) {
        let target = self.session.response.collapsed;
        if target == self.response_collapsed_target {
            return;
        }
        self.response_collapsed_target = target;
        let now = Instant::now();
        let target_v = if target { 1.0 } else { 0.0 };
        if self.anims.value(AnimKey::ResponseCollapse, now).is_none() {
            self.anims.snap(AnimKey::ResponseCollapse, 1.0 - target_v);
        }
        self.anims.retarget(
            AnimKey::ResponseCollapse,
            target_v,
            self.ui_settings.anim_ms.pane_collapse,
            now,
        );
    }

    /// Records the current split as the project's persisted layout
    /// preference (`.local/state.toml`'s `main_split`). Best-effort, like
    /// every local-state save.
    fn persist_split(&mut self) {
        self.project.main_split = Some(self.split_state().to_token().to_string());
        self.project.persist_local_state_keep_open_request();
    }

    /// Seeds the split from the project's saved layout preference — the
    /// reopen half of [`Self::persist_split`]. An absent or unrecognized
    /// token leaves the default split in place.
    fn seed_split_from_project(&mut self) {
        let Some(s) = self
            .project
            .main_split
            .as_deref()
            .and_then(crate::split::SplitState::from_token)
        else {
            return;
        };
        self.table_collapsed = s.editor_minimized;
        self.session.response.collapsed = s.response_minimized;
        self.split_ratio = s.ratio;
    }

    /// The whole split state the split control reads and drives — the
    /// two endpoint flags plus the ratio stop, in one value.
    pub fn split_state(&self) -> crate::split::SplitState {
        crate::split::SplitState {
            editor_minimized: self.table_collapsed,
            response_minimized: self.session.response.collapsed,
            ratio: self.split_ratio,
        }
    }

    /// Jumps the split to `stop` — the shared body of `Action::SplitStop`
    /// (a control chip click) and `Action::CycleSplit` (the keyboard
    /// cycle). Returns whether anything actually moved.
    fn apply_split_stop(&mut self, stop: crate::split::SplitStop) -> bool {
        let prev = self.split_state();
        let next = prev.apply(stop);
        self.table_collapsed = next.editor_minimized;
        self.session.response.collapsed = next.response_minimized;
        self.split_ratio = next.ratio;
        // `sync_pane_collapse_anim` / `sync_response_collapse_anim` /
        // `sync_split_ratio_anim` (run on every `update`) ease whichever
        // of the three the jump actually moved.
        if next != prev {
            self.persist_split();
        }
        next != prev
    }

    /// Keeps `AnimKey::SplitRatio` chasing `split_ratio`'s editor share
    /// the same way [`Self::sync_pane_collapse_anim`] chases
    /// `table_collapsed`: whenever the stop changes, eases from wherever
    /// the share currently sits to the new stop.
    fn sync_split_ratio_anim(&mut self) {
        let target = self.split_ratio;
        if target == self.split_ratio_target {
            return;
        }
        let previous = self.split_ratio_target;
        self.split_ratio_target = target;
        let now = Instant::now();
        if self.anims.value(AnimKey::SplitRatio, now).is_none() {
            self.anims
                .snap(AnimKey::SplitRatio, previous.editor_share());
        }
        self.anims.retarget(
            AnimKey::SplitRatio,
            target.editor_share(),
            self.ui_settings.anim_ms.pane_collapse,
            now,
        );
    }

    /// Builds the Vars tab's shadow hint map: `name → "overrides <env>:
    /// <value>"` for every open-request `[variables]` option that shares a
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

    /// Opens the insert var picker, optionally with its fuzzy filter
    /// pre-seeded (clicking an *undefined* inline `{{token}}` opens the
    /// picker already narrowed to that name, its "new variable…" row being
    /// the create flow). Shared by `Action::OpenVarPicker`'s plain path and
    /// `Action::OpenVarTokenPopup`'s undefined-name fallback.
    fn open_insert_var_picker(&mut self, completing: bool, seed: Option<&str>) -> bool {
        use crate::components::modal::Modal;
        use crate::components::var_picker::{VarPickerState, insert_entries};
        let resolved = self.project.prepare_context().vars;
        let options = insert_entries(&self.project.model, &resolved, &self.editor.variables);
        let mut state = VarPickerState::new(options, completing);
        if let Some(seed) = seed {
            state.seed_filter(seed);
        }
        self.push_modal(Modal::VarPicker(state));
        true
    }

    /// Per-`Tick` bookkeeping for the caret-resting tooltip: tracks how long
    /// (wall-clock) the caret has sat inside one `{{token}}`, resetting
    /// whenever it moves to a different token (or out of every token).
    /// Returns whether the tooltip's visibility could have changed, so the
    /// tick redraws.
    fn track_caret_token(&mut self, now: Instant) -> bool {
        let token = self.editor.caret_token();
        if token != self.caret_token {
            let was_showing = self.caret_tip_shown;
            self.caret_token_since = token.is_some().then_some(now);
            self.caret_token = token;
            self.caret_tip_shown = false;
            return was_showing;
        }
        if self.caret_token.is_none() || self.caret_tip_shown {
            return false;
        }
        let Some(since) = self.caret_token_since else {
            return false;
        };
        if now.duration_since(since) >= CARET_TIP_DWELL {
            self.caret_tip_shown = true;
            return true;
        }
        false
    }

    /// The variable tooltip to draw this frame, if any: the token under the
    /// pointer, or — with no hover — the one the caret has been resting in
    /// for [`CARET_TIP_DWELL`], anchored at the span the last frame drew
    /// for it. Suppressed while a modal is up: the tooltip draws above
    /// everything else, and must not float over a dialog.
    pub fn var_token_tip(&self) -> Option<TokenTip> {
        if !self.modals.is_empty() {
            return None;
        }
        if let Some((x, y)) = self.pointer
            && let Some((name, anchor)) = self.hits.var_token_at(x, y)
        {
            return Some(TokenTip {
                name: name.to_string(),
                anchor,
            });
        }
        if !self.caret_tip_shown {
            return None;
        }
        let name = self.caret_token.clone()?;
        let anchor = self.hits.rect_of(&Hit::VarToken(name.clone()))?;
        Some(TokenTip { name, anchor })
    }

    /// Diffs the open request against the shadow copy and records an undo
    /// step for any change. Runs once per input event from the main loop —
    /// the one place keystroke- and mouse-path edits (which never become
    /// Actions) get captured. Returns whether a step was recorded.
    /// ` — ^Z undoes` (under whatever combo `undo` is currently bound
    /// to), appended to the toasts of destructive-but-undoable actions
    /// that act without a confirm modal.
    fn undo_hint(&self) -> String {
        self.keymap
            .combo_for("undo")
            .map(|c| format!(" — {c} undoes"))
            .unwrap_or_default()
    }

    pub fn capture_undo(&mut self) -> bool {
        let current_slug = self.editor.slug.clone();
        let cursor = self.editor.cursor_pos();
        match &self.shadow {
            // Which request is open changed (open/create/delete/rename/
            // save-as): re-seed, never record — the transition itself is
            // not an edit (its disk half is captured as its own step).
            Some((slug, _)) if *slug != current_slug => {}
            Some((_, prev)) => {
                let current = self.editor.current_request();
                if *prev != current {
                    let step = crate::undo::Step {
                        kind: crate::undo::StepKind::EditorDelta {
                            slug: current_slug.clone(),
                            before: Box::new(prev.clone()),
                            after: Box::new(current.clone()),
                        },
                        context: crate::undo::Context {
                            slug: current_slug.clone(),
                            cursor_before: std::mem::replace(
                                &mut self.shadow_cursor,
                                cursor.clone(),
                            ),
                            cursor_after: cursor.clone(),
                        },
                    };
                    if std::mem::take(&mut self.no_coalesce) {
                        self.history.record_no_coalesce(step);
                    } else {
                        self.history.record(step, std::time::Instant::now());
                    }
                    self.shadow = Some((current_slug, current));
                    self.shadow_cursor = cursor;
                    return true;
                }
            }
            None => {}
        }
        self.no_coalesce = false;
        self.shadow = Some((current_slug, self.editor.current_request()));
        self.shadow_cursor = cursor;
        false
    }

    fn apply(&mut self, action: Action) -> bool {
        match action {
            // An unsaved request gates quitting behind the same confirm as
            // opening/switching away from it — but only from the plain
            // screen: with any modal already up (this gate included),
            // ctrl+c stays a reliable, immediate exit, so pressing it
            // twice always leaves without saving.
            Action::Quit if self.editor_holds_unsaved() && self.modals.is_empty() => {
                self.dirty_gate("quit", Action::ForceQuit);
                true
            }
            Action::DiscardChanges => {
                self.no_coalesce = true;
                if !self.editor.is_dirty() {
                    return true;
                }
                if let (slug, Some(saved)) = (self.editor.slug.clone(), self.editor.saved.clone()) {
                    // A cell still under the caret is part of what's being
                    // thrown away — drop it without committing, and clear
                    // the selection so no stale row index survives the
                    // reload.
                    self.editor.table.editing = None;
                    self.editor.table.selected = None;
                    self.editor.load(slug, saved);
                    self.sync_active_tab();
                    self.toasts.push(
                        format!("Changes discarded{}", self.undo_hint()),
                        ToastKind::Info,
                    );
                }
                true
            }
            Action::Quit | Action::ForceQuit => {
                self.project
                    .persist_local_state(self.editor.slug.as_deref());
                if let Some(path) = &self.usage_path {
                    let _ = self.usage.save_to(path);
                }
                self.should_quit = true;
                true
            }
            Action::Tick => {
                // Only a pointer actually over the sidebar edge-scrolls it:
                // the response pane shares the list's bottom row, and a
                // drag parked there must not scroll a pane it is not over.
                let edge_scrolled = match self.pointer.and_then(|(x, y)| {
                    (self.hits.pane_at(x, y) == Some(PaneId::Sidebar))
                        .then(|| self.sidebar.drag_edge(y).map(|d| (d, x, y)))
                        .flatten()
                }) {
                    Some((delta, x, y)) => {
                        self.sidebar.handle_scroll(delta as i16);
                        self.sidebar_drag_to(x, y);
                        true
                    }
                    None => false,
                };
                let now = Instant::now();
                let tip_changed = self.track_caret_token(now);
                // The testbed's looping motion demos self-drive from here,
                // never from any other screen's tick path — see
                // `tick_testbed_demos`.
                if self.screen == Screen::Testbed {
                    self.tick_testbed_demos(now);
                } else {
                    // The production Send-cap breathe: a separate pingpong
                    // from the testbed's own (they never run on the same
                    // screen at once, so sharing `AnimKey::SendBreathe`
                    // between them is safe — see the testbed's own doc).
                    self.tick_send_breathe(now);
                }
                // `self.animating()` alone misses an animation's very last
                // tick: by the moment a tick fires at or after its
                // duration's end, `done()` already reads true, so
                // `animating()` reports false on the exact tick that needs
                // to redraw one final time to reveal whatever was gated on
                // it reaching t==1.0 (a modal's contents, a dropdown's
                // shadow — see `paint::floating_panel_settling` and
                // `components::modal::draw_dropdown`). OR-ing in whether it
                // was still active as of the previous tick catches that
                // active→finished transition without keeping ticks flowing
                // once truly idle.
                let now_animating = self.animating();
                let was_animating = self.animating_last_tick;
                self.animating_last_tick = now_animating;
                self.toasts.on_tick(&mut self.anims, now)
                    || self.in_flight_ticking()
                    || tip_changed
                    || now_animating
                    || was_animating
                    || edge_scrolled
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
                // While the Manager screen is up its left list owns the
                // sidebar's pane slot (see `App::scrollbar_spec`), so a
                // click on the drawn scrollbar's track pages that list.
                if self.screen == Screen::Manage {
                    self.varmanager.handle_scroll(delta);
                    return true;
                }
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
                self.push_modal(Modal::Palette(PaletteState::new(
                    &self.usage,
                    crate::usage::now(),
                )));
                true
            }
            Action::Close => {
                let popped_modal = self.modals.pop();
                let popped = popped_modal.is_some();
                // A dismissed context menu (clicked off / Close with
                // nothing chosen) undoes the sidebar pre-selection its
                // right-click made — same rule as `apply_modal_result`'s
                // empty-actions branch.
                if matches!(popped_modal, Some(Modal::Dropdown(_)))
                    && let Some(prev) = self.sidebar_menu_revert.take()
                {
                    self.sidebar.selected = prev;
                }
                // Closing the theme picker restores the pre-preview theme —
                // same rule as `apply_modal_result`'s Chooser-revert branch,
                // needed here too for a bare `Close` (the global esc
                // binding) that never reaches `apply_modal_result`. A click
                // outside the picker is an accept instead (see `on_hit`'s
                // `ModalOutside` arm).
                if matches!(popped_modal, Some(Modal::Chooser(_)))
                    && let Some(prior) = self.theme_preview.take()
                {
                    self.set_theme_by_name(&prior);
                }
                // Overlay close is always instant — no motion rule
                // exception for either open-settle key. Snapping
                // `ModalOpen` here also sets the next panel modal's open
                // baseline to 1 for when the stack goes empty→non-empty
                // again (see `push_modal`).
                self.anims.snap(AnimKey::DropdownOpen, 1.0);
                self.anims.snap(AnimKey::ModalOpen, 1.0);
                // With nothing to close, esc is the cancel shortcut: an
                // esc no component consumed falls through to here (the
                // global keymap binds esc → Close), and cancels the open
                // request's in-flight send, if any.
                if !popped && self.session.cancel() {
                    return true;
                }
                popped
            }
            Action::ShowToast(msg, kind) => {
                self.toasts.push(msg, kind);
                true
            }
            Action::ShowAbout => {
                use crate::components::modal::Modal;
                self.push_modal(Modal::Message {
                    title: "postui".into(),
                    body: "A fast, local-first terminal HTTP client.\n\nText selection: hold Shift while dragging (mouse capture is on).".into(),
                });
                true
            }
            Action::ResponseViewMode(mode) => {
                let prev_mode = self.session.response.view().map(|v| v.mode);
                self.session.response.set_view_mode(mode);
                self.retarget_response_tab_underline(prev_mode);
                true
            }
            Action::OpenResponseSearch => {
                self.update(Action::FocusPane(PaneId::Response));
                self.session.response.open_search();
                true
            }
            Action::JsonRowClicked { row, toggle } => {
                self.session.response.click_row(row, toggle);
                true
            }
            Action::CopySelection(surface) => {
                if let Some(text) = self.selection_text_of(surface) {
                    self.copy_text_with_toast(&text, "Copied selection".to_string());
                }
                true
            }
            Action::CopyToClipboard(target) => {
                let Some((text, success_msg)) = self.resolve_copy(&target) else {
                    self.toasts
                        .push("nothing to copy — send a request first", ToastKind::Warning);
                    return true;
                };
                self.copy_text_with_toast(&text, success_msg);
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
                self.open_save_picker(
                    "Save response body",
                    crate::components::file_picker::PickerTarget::SaveBody,
                    &format!("{slug}-response.{ext}"),
                )
            }
            Action::PickerConfirm { target, path } => {
                use crate::components::file_picker::PickerTarget;
                match target {
                    PickerTarget::SaveBody | PickerTarget::SaveView => {
                        let text = path.to_string_lossy().into_owned();
                        let write = if target == PickerTarget::SaveBody {
                            Action::SaveBodyToFile(text)
                        } else {
                            Action::SaveViewToFile(text)
                        };
                        // The save picker is still open beneath (it does
                        // not close itself in save mode): the overwrite
                        // question stacks on it, so `n` returns to the
                        // picker, and `y` closes the picker before writing.
                        let picker_open = matches!(self.modals.top(), Some(Modal::FilePicker(_)));
                        let close_picker = || {
                            picker_open
                                .then_some(Action::Close)
                                .into_iter()
                                .collect::<Vec<_>>()
                        };
                        if path.is_file() {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            let mut on_yes = close_picker();
                            on_yes.push(write);
                            self.push_modal(Modal::Confirm {
                                title: "File exists".into(),
                                body: format!("overwrite {name}?"),
                                choices: vec![
                                    ('y', "Overwrite".into(), on_yes),
                                    ('n', "Cancel".into(), vec![]),
                                ],
                            });
                            return true;
                        }
                        for a in close_picker() {
                            self.apply(a);
                        }
                        self.apply(write)
                    }
                    PickerTarget::OpenProject => self.apply(Action::OpenProjectByPath(
                        path.to_string_lossy().into_owned(),
                    )),
                    PickerTarget::NewProjectDir => {
                        let Some(Modal::NewProject {
                            name,
                            path: field,
                            on_path,
                            prefilled,
                        }) = self.modals.top_mut()
                        else {
                            return false;
                        };
                        let slug = crate::components::modal::slugify(name.text());
                        let full = if slug.is_empty() {
                            format!("{}{}", path.display(), std::path::MAIN_SEPARATOR)
                        } else {
                            path.join(slug).display().to_string()
                        };
                        *field = crate::components::line_input::LineInput::new(&full);
                        *on_path = true;
                        *prefilled = true;
                        true
                    }
                }
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
            Action::PromptSaveView => {
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
                // The extension follows the tab like the content does: the
                // header list is plain text whatever the body was.
                let on_headers = self
                    .session
                    .response
                    .view()
                    .is_some_and(|v| v.mode == crate::components::response::ViewMode::Headers);
                let ext = if !on_headers
                    && data
                        .content_type
                        .as_deref()
                        .is_some_and(|c| c.contains("json"))
                {
                    "json"
                } else {
                    "txt"
                };
                self.open_save_picker(
                    "Save response view",
                    crate::components::file_picker::PickerTarget::SaveView,
                    &format!("{slug}-response.{ext}"),
                )
            }
            Action::SaveViewToFile(path) => {
                let Some(view) = self.session.response.view() else {
                    return true;
                };
                let text = view.view_text();
                let expanded = crate::config::expand_tilde(&path);
                let result = (|| -> std::io::Result<()> {
                    if let Some(parent) = expanded.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&expanded, &text)
                })();
                match result {
                    Ok(()) => self.toasts.push(
                        format!("Saved to {}", expanded.display()),
                        ToastKind::Success,
                    ),
                    Err(e) => self
                        .toasts
                        .push(format!("could not save: {e}"), ToastKind::Error),
                }
                true
            }
            Action::EditorTabSelect(i) => {
                let target = EditorTab::from_index(i);
                if target == EditorTab::Body && self.editor.body_tab_disabled() {
                    return false;
                }
                // Leaving a tab commits whatever cell was being typed — the
                // reset that follows would otherwise drop it silently.
                self.commit_table_edit();
                let prev = self.editor.active_tab;
                self.editor.active_tab = target;
                self.editor.preferred_tab = target;
                self.editor.table.reset();
                self.retarget_editor_tab_underline(prev);
                true
            }
            Action::EditorTabCycle(delta) => {
                // Cycles the on-screen order (Params → Headers → Vars →
                // Body), not `EditorTab::index()`'s tab slot numbers
                // (bindable as `editor_tab_N`).
                self.commit_table_edit();
                let prev = self.editor.active_tab;
                let mut next = (prev.draw_position() as i8 + delta).rem_euclid(4);
                if EditorTab::from_draw_position(next as usize) == EditorTab::Body
                    && self.editor.body_tab_disabled()
                {
                    next = (next + delta.signum()).rem_euclid(4);
                }
                self.editor.active_tab = EditorTab::from_draw_position(next as usize);
                self.editor.preferred_tab = self.editor.active_tab;
                self.editor.table.reset();
                self.retarget_editor_tab_underline(prev);
                true
            }
            Action::CycleMethod => {
                self.no_coalesce = true;
                self.editor.method = self.editor.method.cycle();
                self.sync_active_tab();
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
                self.push_modal(Modal::Dropdown(DropdownState {
                    anchor,
                    items,
                    selected: current.unwrap_or(0),
                    current,
                }));
                self.begin_dropdown_open();
                true
            }
            Action::SetMethod(m) => {
                self.no_coalesce = true;
                self.editor.method = m;
                self.sync_active_tab();
                true
            }
            Action::FocusUrl => {
                // Only a focus that actually MOVES here restarts the fade:
                // re-focusing the already-focused URL bar (clicking the
                // well the caret is in) would snap the fill to its
                // unfocused color for a frame — a visible blink.
                let already =
                    self.focus == PaneId::Editor && self.editor.sub_focus == SubFocus::Url;
                self.focus = PaneId::Editor;
                self.editor.sub_focus = SubFocus::Url;
                if !already {
                    self.begin_focus_fade();
                }
                true
            }
            Action::ToggleTableCollapse => {
                self.table_collapsed = !self.table_collapsed;
                // Hiding the only expanded panel would leave the screen
                // blank: swap instead — the response expands as the editor
                // hides. (`sync_*_collapse_anim` ease both moves.)
                if self.table_collapsed && self.session.response.collapsed {
                    self.session.response.collapsed = false;
                }
                self.persist_split();
                true
            }
            Action::ToggleResponseCollapse => {
                // `sync_response_collapse_anim` (run on every `update`)
                // retargets the anim — here as well as when `sync_open`
                // swaps in another request's response with a different
                // collapsed state.
                self.session.response.collapsed = !self.session.response.collapsed;
                // Hiding the only expanded panel would leave the screen
                // blank: swap instead — the editor expands as the response
                // hides.
                if self.session.response.collapsed && self.table_collapsed {
                    self.table_collapsed = false;
                }
                self.persist_split();
                true
            }
            Action::SplitStop(stop) => self.apply_split_stop(stop),
            Action::CycleSplit => self.apply_split_stop(self.split_state().stop().next()),
            Action::CycleSplitBack => self.apply_split_stop(self.split_state().stop().prev()),
            Action::SplitStep(delta) => match self.split_state().stop().step(delta) {
                Some(stop) => self.apply_split_stop(stop),
                None => false,
            },
            Action::FormatBody => {
                self.no_coalesce = true;
                self.transform_body(postui_core::json::format)
            }
            Action::MinifyBody => {
                self.no_coalesce = true;
                self.transform_body(postui_core::json::minify)
            }
            Action::BodyClear => {
                self.no_coalesce = true;
                if !self.editor.body_text().is_empty() {
                    self.editor.set_body_text("");
                }
                true
            }
            Action::ToggleBodyVars => {
                self.no_coalesce = true;
                self.editor.substitute_body = !self.editor.substitute_body;
                true
            }
            Action::ToggleInsecure => {
                self.no_coalesce = true;
                self.editor.insecure = !self.editor.insecure;
                // Under an environment force the flag still flips and
                // saves with the request (it comes back into effect under
                // another environment), but the send won't follow it, and
                // the toast says so rather than claiming a change.
                let forced = self.project.env_tls();
                if let Some(policy) = forced {
                    let name = self.project.env_label_display();
                    let what = match policy {
                        postui_core::project::TlsPolicy::Verify => "forces",
                        postui_core::project::TlsPolicy::Insecure => "skips",
                    };
                    self.toasts.push(
                        format!("Saved, but {name} {what} TLS verification"),
                        ToastKind::Warning,
                    );
                } else if self.editor.insecure {
                    self.toasts.push(
                        "TLS verification disabled for this request",
                        ToastKind::Warning,
                    );
                } else {
                    self.toasts
                        .push("TLS verification enabled", ToastKind::Info);
                }
                true
            }
            Action::ToggleHeaderReveal => {
                self.editor.computed.revealed = !self.editor.computed.revealed;
                true
            }
            // Suspending the terminal is the main loop's job; park the action
            // and let it pick this up after the current key is handled.
            Action::OpenBodyInEditor => {
                self.no_coalesce = true;
                self.pending_terminal_action = Some(Action::OpenBodyInEditor);
                true
            }
            Action::OpenResponseInEditor => {
                // Unlike the body editor there may be nothing to show yet
                // (the palette can fire this any time), so gate here.
                if self.session.response.view().is_none() {
                    self.toasts
                        .push("nothing to open — send a request first", ToastKind::Warning);
                    return true;
                }
                self.no_coalesce = true;
                self.pending_terminal_action = Some(Action::OpenResponseInEditor);
                true
            }
            Action::OpenRequest(slug) => {
                if self.editor_holds_unsaved() {
                    self.dirty_gate("open", Action::ForceOpenRequest(slug));
                    true
                } else {
                    self.apply(Action::ForceOpenRequest(slug))
                }
            }
            Action::ForceOpenRequest(slug) => {
                // A slug from another space (palette, cross-space click)
                // switches spaces first, so the sidebar it lands in is the
                // one that actually contains it.
                let outgoing = self.editor.slug.clone();
                if let Some(space) = postui_core::storage::space_of(&slug).map(str::to_string)
                    && space != self.project.active_space
                    && !self.enter_space(&space, SpaceExit::Remember(outgoing.as_deref()))
                {
                    return true;
                }
                match postui_core::storage::load_request(&self.project.root, &slug) {
                    Ok(req) => {
                        self.editor.load(Some(slug.clone()), req);
                        self.sync_active_tab();
                        // Every open route (click, Enter, palette, restore)
                        // drags the sidebar selection along so it can't
                        // diverge from the open request. Queue ancestor
                        // folders open, rebuild so the row exists, then
                        // select it now that it's visible.
                        let prev = self.sidebar.open_row();
                        self.sidebar.select_slug(&slug);
                        self.refresh_sidebar();
                        self.sidebar.select_slug(&slug);
                        self.retarget_sidebar_travel(prev);
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
                // A cell still under the caret is part of the request the
                // user means to save.
                self.commit_table_edit();
                match self.editor.slug.clone() {
                    Some(slug) => {
                        let req = self.editor.current_request();
                        match postui_core::storage::save_request(&self.project.root, &slug, &req) {
                            Ok(()) => {
                                self.mark_saved_after_write();
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
                        self.push_modal(Modal::Prompt {
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
                self.push_modal(Modal::Message {
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
                self.push_modal(Modal::Prompt {
                    title: "New request".into(),
                    input: crate::components::line_input::LineInput::new(""),
                    kind: PromptKind::NewRequest,
                    revealed: false,
                });
                true
            }
            Action::PromptNewRequestIn(folder) => {
                // The prompt speaks folders *inside* the space, so the
                // space segment never shows up in the prefill.
                let folder = folder
                    .strip_prefix(&format!("{}/", self.project.active_space))
                    .unwrap_or("");
                let prefill = if folder.is_empty() {
                    String::new()
                } else {
                    format!("{folder}/")
                };
                self.push_modal(Modal::Prompt {
                    title: "New request".into(),
                    input: crate::components::line_input::LineInput::new(&prefill),
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
                        let new_path =
                            postui_core::storage::request_path(&self.project.root, &new_slug);
                        self.record_file_step(vec![(new_path.clone(), None)], &[new_path], None);
                        if let (Some((space, anchor_rel)), Some((_, rel))) =
                            (Self::split_rel(&slug), Self::split_rel(&new_slug))
                        {
                            let r = postui_core::order::order_insert_after(
                                &self.project.root,
                                space,
                                anchor_rel,
                                rel,
                            );
                            self.order_cascade("duplicate", r);
                        }
                        self.refresh_sidebar();
                        let display = self.request_display(&new_slug);
                        self.toasts
                            .push(format!("Duplicated to {display}"), ToastKind::Success);
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
                    // Prefill what the user reads: the display name, under
                    // its folder's slug path (never the leaf slug), with
                    // the space segment stripped — the name the user edits
                    // is relative to the space they're in.
                    let folder = match slug.rsplit_once('/') {
                        Some((folder, _)) => folder
                            .strip_prefix(&format!("{}/", self.project.active_space))
                            .unwrap_or(""),
                        None => "",
                    };
                    let display = self.request_display(&slug);
                    let prefill = if folder.is_empty() {
                        display
                    } else {
                        format!("{folder}/{display}")
                    };
                    self.push_modal(Modal::Prompt {
                        title: "Rename request".into(),
                        input: crate::components::line_input::LineInput::new(&prefill),
                        kind: PromptKind::RenameRequest { from: slug },
                        revealed: false,
                    });
                }
                true
            }
            Action::TableAddRow => {
                if self.editor.active_tab == EditorTab::Body {
                    return false;
                }
                self.commit_table_edit();
                self.focus = PaneId::Editor;
                self.editor.begin_add_row();
                true
            }
            Action::ToggleTableRow(i) => {
                self.no_coalesce = true;
                let map = match self.editor.active_tab {
                    EditorTab::Params => &mut self.editor.params,
                    EditorTab::Headers => &mut self.editor.headers,
                    EditorTab::Vars => &mut self.editor.variables,
                    EditorTab::Body => return true,
                };
                if let Some((_, e)) = map.get_index_mut(i) {
                    e.enabled = !e.enabled;
                }
                true
            }
            Action::DeleteTableRow(i) => {
                self.no_coalesce = true;
                let map = match self.editor.active_tab {
                    EditorTab::Params => &mut self.editor.params,
                    EditorTab::Headers => &mut self.editor.headers,
                    EditorTab::Vars => &mut self.editor.variables,
                    EditorTab::Body => return true,
                };
                self.editor.table.delete_row(map, i);
                true
            }
            Action::DuplicateTableRow(i) => {
                self.no_coalesce = true;
                let map = match self.editor.active_tab {
                    EditorTab::Params => &mut self.editor.params,
                    EditorTab::Headers => &mut self.editor.headers,
                    EditorTab::Vars => &mut self.editor.variables,
                    EditorTab::Body => return true,
                };
                let Some((key, option)) = map.get_index(i).map(|(k, e)| (k.clone(), e.clone()))
                else {
                    return true;
                };
                let mut new_key = format!("{key}-copy");
                let mut n = 2;
                while map.contains_key(&new_key) {
                    new_key = format!("{key}-copy-{n}");
                    n += 1;
                }
                let insert_at = (i + 1).min(map.len());
                map.shift_insert(insert_at, new_key, option);
                self.editor.table.selected = Some(insert_at);
                true
            }
            Action::DeleteSelectedRequest => {
                if let Some(slug) = self.sidebar.selected_slug() {
                    self.apply(Action::DeleteRequest(slug));
                }
                true
            }
            Action::PromptMoveSelectedRequestToSpace => {
                if let Some(slug) = self.sidebar.selected_slug() {
                    self.apply(Action::PromptMoveRequestToSpace(slug));
                }
                true
            }
            Action::MoveRequestToSpace { slug, space } => {
                if space == self.project.active_space {
                    self.toasts
                        .push(format!("already in {space}"), ToastKind::Warning);
                    self.last_action_failed = true;
                    return true;
                }
                if !self.project.spaces.contains(&space) {
                    self.toasts
                        .push(format!("no space named {space:?}"), ToastKind::Warning);
                    self.last_action_failed = true;
                    return true;
                }
                let moving_open = self.editor.slug.as_deref() == Some(slug.as_str());
                if moving_open && self.editor_holds_unsaved() {
                    self.dirty_gate("move", Action::ForceMoveRequestToSpace { slug, space });
                } else {
                    self.apply(Action::ForceMoveRequestToSpace { slug, space });
                }
                true
            }
            Action::ForceMoveRequestToSpace { slug, space } => {
                use postui_core::storage;
                let from_path = storage::request_path(&self.project.root, &slug);
                let old_content = std::fs::read_to_string(&from_path).ok();
                match storage::move_request_to_space(&self.project.root, &slug, &space) {
                    Ok(new_slug) => {
                        let to_path = storage::request_path(&self.project.root, &new_slug);
                        self.record_file_step(
                            vec![(from_path.clone(), old_content), (to_path.clone(), None)],
                            &[from_path, to_path],
                            None,
                        );
                        if let Some((from_space, from_rel)) = Self::split_rel(&slug) {
                            let r = postui_core::order::order_remove(
                                &self.project.root,
                                from_space,
                                from_rel,
                            );
                            self.order_cascade("move", r);
                        }
                        if let Some((to_space, to_rel)) = Self::split_rel(&new_slug) {
                            let r = postui_core::order::order_arrive(
                                &self.project.root,
                                to_space,
                                to_rel,
                            );
                            self.order_cascade("move", r);
                        }
                        // The move doesn't follow the request into its
                        // new space (user feedback: that made moving
                        // several in a row a chore). From this space's
                        // point of view it's a delete: the editor clears
                        // if it held the request, and the sidebar cursor
                        // lands on the nearest remaining request so the
                        // next `m` has something to act on.
                        let was_selected =
                            self.sidebar.selected_slug().as_deref() == Some(slug.as_str());
                        let from_row = self.sidebar.selected;
                        self.refresh_sidebar();
                        self.toasts.push(
                            format!(
                                "Moved {} to {}",
                                self.request_display(&new_slug),
                                self.project.space_name(&space)
                            ),
                            ToastKind::Success,
                        );
                        if self.editor.slug.as_deref() == Some(slug.as_str()) {
                            self.editor = Editor::default();
                            self.shadow = None;
                        }
                        if was_selected {
                            self.sidebar.select_nearest_request(from_row.unwrap_or(0));
                        }
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("could not move {slug}: {e}"), ToastKind::Error);
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::CreateRequest(name) => {
                self.create_or_save_as(&name, |_| postui_core::model::HttpRequest {
                    name: None,
                    method: postui_core::model::Method::Get,
                    url: String::new(),
                    substitute_body: false,
                    insecure: false,
                    jq: None,
                    jq_enabled: true,
                    params: Default::default(),
                    headers: Default::default(),
                    variables: Default::default(),
                    body: None,
                });
                true
            }
            Action::RenameRequest { from, to } => {
                use postui_core::storage::{self, StorageError};
                // The typed name is relative to the active space, same as
                // a create.
                let to = format!(
                    "{}/{}",
                    self.project.active_space,
                    to.trim_start_matches('/')
                );
                let from_path = storage::request_path(&self.project.root, &from);
                let old_content = std::fs::read_to_string(&from_path).ok();
                match storage::rename_request_named(&self.project.root, &from, &to) {
                    Ok((slug, leaf)) => {
                        let to_path = storage::request_path(&self.project.root, &slug);
                        self.record_file_step(
                            vec![(from_path.clone(), old_content), (to_path.clone(), None)],
                            &[from_path, to_path],
                            None,
                        );
                        if let (Some((space, from_rel)), Some((_, to_rel))) =
                            (Self::split_rel(&from), Self::split_rel(&slug))
                        {
                            let r = postui_core::order::order_rename(
                                &self.project.root,
                                space,
                                from_rel,
                                to_rel,
                            );
                            self.order_cascade("rename", r);
                        }
                        self.refresh_sidebar();
                        if self.editor.slug.as_deref() == Some(from.as_str()) {
                            self.editor.slug = Some(slug.clone());
                            // The rename wrote the new display name to
                            // disk; mirror it in both the live fields and
                            // the saved snapshot so a rename alone never
                            // reads as dirty.
                            self.editor.name = Some(leaf.clone());
                            if let Some(saved) = self.editor.saved.as_mut() {
                                saved.name = Some(leaf);
                            }
                            self.sidebar.open_slug = Some(slug);
                        }
                    }
                    Err(StorageError::AlreadyExists(taken)) => {
                        self.toasts.push(
                            format!("a request named {taken:?} already exists here"),
                            ToastKind::Error,
                        );
                        self.last_action_failed = true;
                    }
                    Err(StorageError::InvalidSlug(_)) => {
                        self.toasts
                            .push("request name cannot be empty", ToastKind::Error);
                        self.last_action_failed = true;
                    }
                    Err(e) => {
                        self.toasts.push(
                            format!("could not rename {}: {e}", self.request_display(&from)),
                            ToastKind::Error,
                        );
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::DeleteRequest(slug) => {
                let display = self.request_display(&slug);
                match postui_core::storage::delete_request(&self.project.root, &slug) {
                    Ok(trashed) => {
                        self.toasts.push(
                            format!("Deleted {display}{}", self.undo_hint()),
                            ToastKind::Info,
                        );
                        // Recorded before refresh_sidebar/editor-clearing
                        // reorder state: context.slug must still name the
                        // deleted request while self.editor.slug matches it.
                        self.record_trashed_step(vec![trashed], Vec::new(), &[], None);
                        if let Some((space, rel)) = Self::split_rel(&slug) {
                            let r =
                                postui_core::order::order_remove(&self.project.root, space, rel);
                            self.order_cascade("delete", r);
                        }
                        self.refresh_sidebar();
                        if self.editor.slug.as_deref() == Some(slug.as_str()) {
                            self.editor = Editor::default();
                            self.shadow = None;
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
                self.commit_table_edit();
                let req = self.editor.current_request();
                self.create_or_save_as(&name, move |_| req.clone());
                true
            }
            Action::PromptSaveScratch(then) => {
                self.commit_table_edit();
                self.push_modal(Modal::Prompt {
                    title: "Save request as".into(),
                    input: crate::components::line_input::LineInput::new(""),
                    kind: PromptKind::SaveAsThen(then),
                    revealed: false,
                });
                true
            }
            Action::SaveRequestAsThen(name, then) => {
                self.commit_table_edit();
                let req = self.editor.current_request();
                // The deferred step (quit, open another request, switch
                // project) runs only when the save lands — a bad name or a
                // write failure toasts and stays put, content intact.
                if self.create_or_save_as(&name, move |_| req.clone()) {
                    self.apply(*then);
                }
                true
            }
            Action::Send => {
                // A request already waiting on its result cannot be sent
                // again — the button is a Cancel while in flight, and the
                // send shortcuts go dead rather than superseding the send.
                // (Other requests can still be opened and sent.)
                if self.session.is_in_flight(&self.editor.slug) {
                    return false;
                }
                // Same for sending: the typed cell is part of the request
                // that goes out, not something to discard.
                self.commit_table_edit();
                if self.editor.url.text().trim().is_empty() {
                    self.toasts
                        .push("cannot send: URL is empty", ToastKind::Error);
                    return true;
                }
                let body = self.editor.body_text();
                if !body.is_empty() && postui_core::json::validate(&body).is_err() {
                    self.push_modal(Modal::Confirm {
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
                // Same in-flight gate as `Action::Send`: ForceSend is also
                // reachable directly (invalid-body confirm, SetSecret).
                if self.session.is_in_flight(&self.editor.slug) {
                    return false;
                }
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
                            self.push_modal(Modal::Prompt {
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
                        // a fix, so `alt+shift+v` is a visible next step
                        // rather than a dead end.
                        if let Some(name) = causes.iter().find_map(|(name, cause)| {
                            (*cause == postui_core::prepare::UnresolvedCause::NeedsSelection)
                                .then(|| name.clone())
                        }) {
                            msg.push_str(&format!(
                                " \u{2014} press {}+shift+v to select {name}",
                                crate::keys::alt_label()
                            ));
                        }
                        self.toasts.push(msg, ToastKind::Error);
                        return true;
                    }
                };
                for w in &warnings {
                    self.toasts.push(w.to_string(), ToastKind::Warning);
                }
                self.dispatch(Action::CancelJqDescribe);
                let generation = self.session.begin_send(&self.editor.slug);
                let tx = self.tx.clone();
                let client = self.clients.for_request(&prepared).clone();
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
                self.session.in_flight.push(crate::session::InFlight {
                    started: Instant::now(),
                    generation,
                    slug: self.editor.slug.clone(),
                    task,
                });
                true
            }
            Action::CancelSend => self.session.cancel(),
            Action::SetSecret { name, value } => {
                let before = self.read_file_states(&self.project.var_file_paths());
                match self.project.set_secret(&name, value) {
                    Ok(()) => {
                        self.record_var_file_step(before);
                        self.apply(Action::ForceSend)
                    }
                    Err(e) => {
                        self.toasts.push(
                            format!("could not save secret {name}: {e}"),
                            ToastKind::Error,
                        );
                        true
                    }
                }
            }
            Action::ResponseArrived { generation, data } => {
                // A body too big to parse on the UI thread is handed to a
                // blocking worker, whose result comes back as
                // `PrettyParsed`. The clone happens before delivery so the
                // response itself still moves into the session.
                let big = (data.body.len() > SYNC_PRETTY_BYTES).then(|| data.body.clone());
                // Whether this result lands on screen (its request is the
                // open one) — checked before delivery consumes the entry.
                let on_screen = self
                    .session
                    .in_flight
                    .iter()
                    .any(|f| f.generation == generation && f.slug == self.editor.slug);
                let delivered = self.session.arrived(generation, data);
                // The fresh view picked its own mode (Pretty for JSON, Raw
                // otherwise) with no `ResponseViewMode` action, so the
                // previous response's underline glide is stale.
                if delivered && on_screen {
                    self.reset_response_tab_underline();
                }
                if delivered && let Some(body) = big {
                    self.spawn_pretty_parse(generation, body);
                }
                delivered
            }
            Action::PrettyParsed { generation, tree } => {
                let on_screen = self.session.response.awaits_tree(generation);
                // "Not JSON after all" removes the Tree tab and forces the
                // mode to Raw — a tab-set change with no `ResponseViewMode`
                // action behind it.
                let removed_tree_tab = tree.is_none();
                let delivered = self.session.tree_arrived(generation, tree.map(|t| *t));
                if delivered && on_screen && removed_tree_tab {
                    self.reset_response_tab_underline();
                }
                delivered
            }
            Action::RequestFailed { generation, error } => self.session.failed(generation, error),
            Action::InitProjectHere => {
                match postui_core::project::init_project(&self.project.root, None) {
                    Ok(()) => {
                        self.registry.register(self.project.root.clone());
                        if let Some(path) = &self.registry_path {
                            let _ = self.registry.save_to(path);
                        }
                        if let Err(e) = postui_core::storage::ensure_project(&self.project.root) {
                            self.toasts
                                .push(format!("could not open project: {e}"), ToastKind::Error);
                        }
                        self.refresh_sidebar();
                        // The context was opened on a bare directory, before
                        // init wrote the stock `default` env: land in it now,
                        // as an open of the finished project would.
                        self.project.environments =
                            postui_core::project::list_environments(&self.project.root);
                        if self.project.active_env.is_none()
                            && let Some(first) = self.project.environments.first().cloned()
                        {
                            self.apply(Action::SwitchEnv(Some(first)));
                        }
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
                self.project.record_space_open(self.editor.slug.as_deref());
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
                        id: Some(path.display().to_string()),
                    });
                }
                items.push(ChooserItem {
                    label: "open by path…".into(),
                    detail: None,
                    actions: vec![Action::PromptOpenProjectPath],
                    ..Default::default()
                });
                // New project lives here rather than on a chord: it is
                // rare enough that a row in the place you look for
                // projects beats a key to remember.
                items.push(ChooserItem {
                    label: "new project…".into(),
                    detail: None,
                    actions: vec![Action::PromptNewProject],
                    ..Default::default()
                });
                let mut state = ChooserState::new("Projects", items);
                // Open on the current project, not row 0.
                state.select_id(&self.project.root.display().to_string());
                self.push_modal(Modal::Chooser(state));
                true
            }
            Action::OpenThemeChooser => {
                use crate::components::chooser::ChooserState;
                // Rescan the themes dir so a custom file edited or added since
                // startup shows up without a restart (spec: rescan on picker open).
                let (themes, warnings) =
                    crate::theme::ThemeRegistry::load(self.themes_dir.as_deref());
                for w in warnings {
                    self.toasts.push(w, ToastKind::Warning);
                }
                self.themes = themes;
                // The picker opens filtered to the current theme's
                // polarity: browsing themes must not flash the opposite
                // polarity's (much brighter/darker) palettes. Left/Right
                // or the title-row toggle flips to the other set.
                self.theme_picker_dark = self.theme.is_dark();
                let mut state = ChooserState::new("Theme", self.theme_picker_items()).with_toggle(
                    theme_picker_toggle_label(self.theme_picker_dark),
                    Action::ToggleThemePickerPolarity,
                );
                // Open on the currently-applied theme, not row 0 — the
                // highlight drives the live preview, so starting anywhere
                // else would instantly re-theme the app on open.
                state.select_id(&self.theme_name.clone());
                self.theme_preview = Some(self.theme_name.clone());
                self.push_modal(Modal::Chooser(state));
                // Settle the toggle label for the opening highlight (hidden
                // when the current theme is unpaired, e.g. Terminal).
                self.sync_theme_preview();
                true
            }
            Action::ToggleThemePickerPolarity => {
                // The switch means "this theme, other polarity": it only
                // acts when the highlighted theme has a counterpart, and
                // the highlight lands on that counterpart in the flipped
                // set. Unpaired themes (Terminal, lone customs) leave the
                // switch inert — the label is hidden for them too, via
                // `sync_theme_preview`.
                let highlighted = match self.modals.top() {
                    Some(Modal::Chooser(c)) => c.selected_id().map(str::to_string),
                    _ => None,
                };
                let Some(counterpart) = highlighted
                    .as_deref()
                    .and_then(|id| self.themes.get(id))
                    .and_then(|e| e.counterpart.clone())
                else {
                    return false;
                };
                self.theme_picker_dark = !self.theme_picker_dark;
                let items = self.theme_picker_items();
                if let Some(Modal::Chooser(state)) = self.modals.top_mut() {
                    state.set_items(items);
                    state.select_id(&counterpart);
                }
                self.sync_theme_preview();
                true
            }
            Action::ApplyTheme(name) => {
                self.set_theme_by_name(&name);
                self.ui_settings.theme = self.theme_name.clone();
                if let Some(path) = self.registry_path.clone()
                    && let Err(e) = crate::config::save_ui_theme(&path, &self.theme_name)
                {
                    self.toasts
                        .push(format!("could not save theme: {e}"), ToastKind::Error);
                }
                true
            }
            Action::CycleProject(delta) => {
                match self.registry.neighbor(&self.project.root, delta) {
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
                if self.editor_holds_unsaved() {
                    self.dirty_gate("switch", Action::ForceSwitchProject(target));
                } else {
                    self.apply(Action::ForceSwitchProject(target));
                }
                true
            }
            Action::ForceSwitchProject(target) => {
                // A switch can land with the mouse button still held (alt+z
                // cycles projects): every drag and armed press belongs to
                // the project being left — a press left armed would promote
                // into a drag of a same-named row in the next project.
                self.cancel_stale_drags(None);
                self.history.clear();
                self.shadow = None;
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
                // A different tree has a different set of loose files.
                self.last_loose_warning = None;
                self.last_spaces_warning = None;
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
                // The layout split is per-project local state too: restore
                // the incoming project's saved split alongside its open
                // request.
                self.seed_split_from_project();
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
                use crate::components::file_picker::{FilePickerState, PickerTarget};
                let start = self.registry.default_root();
                self.push_modal(Modal::FilePicker(FilePickerState::new(
                    "Open project",
                    PickerTarget::OpenProject,
                    &start,
                    "",
                )));
                true
            }
            Action::BrowseNewProjectDir => {
                use crate::components::file_picker::{FilePickerState, PickerTarget};
                let Some(Modal::NewProject { path, .. }) = self.modals.top() else {
                    return false;
                };
                // Start where the path field points (its nearest existing
                // folder), so browsing continues from the default root.
                let start = crate::config::expand_tilde(path.text().trim());
                self.push_modal(Modal::FilePicker(FilePickerState::new(
                    "Choose project folder",
                    PickerTarget::NewProjectDir,
                    &start,
                    "",
                )));
                true
            }
            Action::OpenProjectByPath(text) => {
                let path = crate::config::expand_tilde(&text);
                if postui_core::project::is_project(&path) {
                    self.apply(Action::SwitchProject(path));
                } else {
                    let display = path.display().to_string();
                    self.push_modal(Modal::Confirm {
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
                self.push_modal(Modal::NewProject {
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
                if self.editor_holds_unsaved() {
                    self.dirty_gate("create", Action::ForceSwitchProject(path));
                } else {
                    self.apply(Action::ForceSwitchProject(path));
                }
                true
            }
            Action::OpenEnvChooser => {
                self.apply(Action::ReloadProjectFiles);
                use crate::components::modal::{DropdownState, MenuItem};
                self.project.environments =
                    postui_core::project::list_environments(&self.project.root);
                let mut items: Vec<MenuItem> = self
                    .project
                    .environments
                    .iter()
                    .map(|slug| {
                        MenuItem::new(
                            self.project.env_name(slug),
                            Action::SwitchEnv(Some(slug.clone())),
                        )
                    })
                    .collect();
                items.push(MenuItem::new("new environment…", Action::OpenNewEnvPrompt));
                items.push(MenuItem::new(
                    "manage environments…",
                    Action::OpenManage {
                        tab: Some(crate::components::manage::ManageTab::Environments),
                    },
                ));
                // The ✓ (and the opening cursor) sits on the active
                // environment. There is no "no environment" row: a project
                // always has at least one env (a fresh one gets `default`),
                // so the no-env state is only ever reached by a file going
                // missing — in which case the cursor opens on row 0.
                let current =
                    self.project.active_env.as_deref().and_then(|active| {
                        self.project.environments.iter().position(|n| n == active)
                    });
                // Anchored under the header's env chip — the one env
                // button, visible on every screen. A keyboard open with
                // no frame drawn yet (bare test apps) falls back to the
                // method dropdown's zero-rect, which `draw_dropdown`
                // clamps on-screen.
                let anchor = self
                    .hits
                    .rect_of(&Hit::HeaderEnv)
                    .unwrap_or_else(|| ratatui::layout::Rect::new(0, 0, 0, 0));
                self.push_modal(Modal::Dropdown(DropdownState {
                    anchor,
                    items,
                    selected: current.unwrap_or(0),
                    current,
                }));
                self.begin_dropdown_open();
                true
            }
            Action::OpenNewEnvPrompt => {
                self.push_modal(Modal::Prompt {
                    title: "New environment".into(),
                    input: crate::components::line_input::LineInput::new(""),
                    kind: PromptKind::NewEnvironment,
                    revealed: false,
                });
                true
            }
            Action::CreateEnv(name) => {
                let prev_active = self.project.active_env.clone();
                // The name is free-form; the file is its slug, and
                // project.toml records the name — so it is part of the
                // step, or an undo would strand the `[environment.<slug>]`
                // table.
                let project_toml = self.project.root.join("project.toml");
                let before_meta = self.read_file_states(std::slice::from_ref(&project_toml));
                match postui_core::project::create_environment(&self.project.root, &name) {
                    Ok(slug) => {
                        self.project.reload_meta();
                        self.project.environments =
                            postui_core::project::list_environments(&self.project.root);
                        let path =
                            postui_core::project::environment_path(&self.project.root, &slug);
                        let mut before = vec![(path.clone(), None)];
                        before.extend(before_meta);
                        self.record_file_step(
                            before,
                            &[path, project_toml],
                            Some((prev_active, Some(slug.clone()))),
                        );
                        self.apply(Action::SwitchEnv(Some(slug)));
                    }
                    Err(postui_core::project::ProjectError::AlreadyExists(name)) => {
                        self.toasts.push(
                            format!("environment \"{name}\" already exists"),
                            ToastKind::Warning,
                        );
                        self.last_action_failed = true;
                    }
                    Err(e) => {
                        self.toasts.push(
                            format!("cannot create environment: {e}"),
                            ToastKind::Warning,
                        );
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::CycleEnv(delta) => {
                self.apply(Action::ReloadProjectFiles);
                self.project.environments =
                    postui_core::project::list_environments(&self.project.root);
                let envs = &self.project.environments;
                if envs.is_empty() {
                    self.toasts.push(
                        "no environments — create environments/<name>.toml in the project",
                        ToastKind::Warning,
                    );
                    return true;
                }
                let len = envs.len() as i32;
                let next = match self
                    .project
                    .active_env
                    .as_deref()
                    .and_then(|current| envs.iter().position(|e| e == current))
                {
                    Some(i) => envs[(i as i32 + delta).rem_euclid(len) as usize].clone(),
                    // From no env, step onto the list's first (or last, going
                    // back) rather than skipping one.
                    None if delta < 0 => envs[envs.len() - 1].clone(),
                    None => envs[0].clone(),
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
                // The Manager caches per-env rows; an env switched under it
                // (alt+x is whitelisted through its input capture) must show
                // the new env's values.
                if self.screen == Screen::Manage {
                    self.varmanager.sync(&self.project);
                }
                let label = self.project.env_label_display();
                self.toasts
                    .push(format!("env: {label}"), ToastKind::Success);
                true
            }
            Action::ApplyMigration => {
                let before = self.read_file_states(&self.project.var_file_paths());
                match self.project.apply_migration() {
                    Ok(notes) => {
                        self.record_var_file_step(before);
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
            Action::Undo => {
                if !self.modals.is_empty() {
                    return true;
                }
                // A live cell edit is part of what's being undone: commit it
                // so it becomes a step, then capture any pending delta.
                self.commit_table_edit();
                self.capture_undo();
                match self.history.pop_undo() {
                    None => {
                        self.toasts.push("Nothing to undo", ToastKind::Info);
                    }
                    Some(step) => {
                        if self.apply_undo_step(step, false) {
                            self.history.break_coalescing();
                        }
                    }
                }
                true
            }
            Action::Redo => {
                if !self.modals.is_empty() {
                    return true;
                }
                self.commit_table_edit();
                // A pending uncaptured edit means the user changed something
                // after the last undo; capturing it clears the redo stack,
                // which is exactly the linear-history contract.
                self.capture_undo();
                match self.history.pop_redo() {
                    None => {
                        self.toasts.push("Nothing to redo", ToastKind::Info);
                    }
                    Some(step) => {
                        if self.apply_undo_step(step, true) {
                            self.history.break_coalescing();
                        }
                    }
                }
                true
            }
            Action::ToggleJqBar => {
                if !self.session.response.jq_available() {
                    self.toasts
                        .push("The response is not JSON", ToastKind::Info);
                    return true;
                }
                // Open ⇄ closed, regardless of focus: an open bar closes
                // (filter off, text kept) whether or not the caret is in
                // it; a closed one opens switched on and focused. The
                // switch, not the way in — alt+q is `OpenJqBar`.
                if self.session.response.jq_open() {
                    self.session.response.close_jq();
                } else {
                    self.dispatch(Action::FocusPane(PaneId::Response));
                    self.session.response.open_jq();
                }
                true
            }
            Action::CancelJqEdit => {
                self.session.response.cancel_jq_edit();
                true // sync_jq lands the restored filter in the editor
            }
            Action::OpenJqBar => {
                if !self.session.response.jq_available() {
                    self.toasts
                        .push("The response is not JSON", ToastKind::Info);
                    return true;
                }
                self.dispatch(Action::FocusPane(PaneId::Response));
                self.session.response.open_jq();
                true
            }
            Action::JqApply(text) => {
                let cursor = text.chars().count();
                self.session.response.set_jq_text_with_cursor(&text, cursor);
                true // sync_jq applies it
            }
            Action::JqTeeUp { text, cursor } => {
                // Focus before the text lands, so Esc cancels the tee-up
                // back to the filter that was there.
                self.dispatch(Action::FocusPane(PaneId::Response));
                self.session.response.set_jq_focus(true);
                self.session.response.set_jq_text_with_cursor(&text, cursor);
                true
            }
            Action::JqRunFinished {
                generation,
                run,
                result,
            } => self
                .session
                .response
                .attach_jq_result(generation, run, result),
            Action::JqCompleteFinished {
                generation,
                seq,
                input_expr,
                keys,
            } => self
                .session
                .response
                .attach_jq_completion(generation, seq, input_expr, keys),
            Action::CopyJqPath(path) => {
                self.copy_text_with_toast(&path, "Copied path".to_string());
                true
            }
            Action::CancelJqDescribe => {
                if let Some((_, task)) = self.ai_task.take() {
                    task.abort();
                }
                // Invalidates any reply still in flight for the cancelled
                // request — `JqAiFinished` compares against this counter,
                // not against `ai_task` (already cleared above).
                self.ai_request += 1;
                self.session.response.jq_bar_mut().ai_pending = false;
                true
            }
            Action::OpenJqDescribe => {
                if !self.session.response.jq_available() {
                    self.toasts
                        .push("The response is not JSON", ToastKind::Info);
                    return true;
                }
                if !crate::ai::program_available(&self.ui_settings.ai_cmd) {
                    self.toasts.push(
                        format!(
                            "{} not found \u{2014} set ai_cmd in config.toml",
                            crate::ai::program_name(&self.ui_settings.ai_cmd)
                        ),
                        ToastKind::Error,
                    );
                    return true;
                }
                self.push_modal(Modal::Prompt {
                    title: "Describe a filter \u{2014} what do you want to see?".into(),
                    input: LineInput::new(""),
                    kind: PromptKind::JqDescribe,
                    revealed: false,
                });
                true
            }
            Action::ConfirmJqDescribe(sentence) => {
                if self.ui_settings.ai_confirmed {
                    return self.dispatch(Action::RunJqDescribe(sentence));
                }
                let program = crate::ai::program_name(&self.ui_settings.ai_cmd).to_string();
                self.push_modal(Modal::Confirm {
                    title: "Send to AI?".into(),
                    body: format!(
                        "The response's structure (key names and types, no values) will be sent to `{program}`."
                    ),
                    choices: vec![
                        ('s', "Send once".into(), vec![Action::RunJqDescribe(sentence.clone())]),
                        (
                            'a',
                            "Always send".into(),
                            vec![Action::SetAiConfirmed, Action::RunJqDescribe(sentence)],
                        ),
                        ('n', "Cancel".into(), vec![]),
                    ],
                });
                true
            }
            Action::SetAiConfirmed => {
                self.ui_settings.ai_confirmed = true;
                if let Some(path) = &self.config_path
                    && let Err(e) = crate::config::save_ui_flag(path, "ai_confirmed", true)
                {
                    self.toasts
                        .push(format!("could not save config: {e}"), ToastKind::Warning);
                }
                true
            }
            Action::RunJqDescribe(sentence) => {
                let Some(view) = self.session.response.view() else {
                    return true;
                };
                let body = view.body_text();
                let Some(shape) = postui_core::jq::shape::shape(&body, Default::default()) else {
                    self.toasts
                        .push("The response is not JSON", ToastKind::Info);
                    return true;
                };
                let stdin =
                    postui_core::jq::ai::prompt(&shape, self.session.response.jq_text(), &sentence);
                // No ambient runtime happens only in a plain `#[test]` that
                // reaches an already-confirmed `ConfirmJqDescribe` — the
                // real main loop, and every async test, always has one.
                let Ok(handle) = tokio::runtime::Handle::try_current() else {
                    return true;
                };
                if let Some((_, task)) = self.ai_task.take() {
                    task.abort();
                }
                self.ai_request += 1;
                let request = self.ai_request;
                let generation = view.generation;
                let cmd = self.ui_settings.ai_cmd.clone();
                let tx = self.tx.clone();
                let task = handle.spawn(async move {
                    let result = crate::ai::run_command(cmd, stdin).await;
                    let _ = tx.send(Action::JqAiFinished {
                        generation,
                        request,
                        result,
                    });
                });
                self.ai_task = Some((request, task));
                let bar = self.session.response.jq_bar_mut();
                bar.ai_pending = true;
                bar.ai_started = Instant::now();
                self.dispatch(Action::FocusPane(PaneId::Response));
                self.session.response.set_jq_focus(true);
                true
            }
            Action::JqAiFinished {
                generation,
                request,
                result,
            } => {
                // Dropped when superseded by a newer request (or cancelled
                // — `CancelJqDescribe` bumps `ai_request` too) or when the
                // response it was shaped from is gone. Compared against the
                // counter, not `ai_task`'s handle: a caller that already
                // took the handle to await it (tests) still gets a valid
                // reply landed.
                if request != self.ai_request
                    || self
                        .session
                        .response
                        .view()
                        .is_none_or(|v| v.generation != generation)
                {
                    return false;
                }
                self.ai_task = None;
                self.session.response.jq_bar_mut().ai_pending = false;
                match result.map(|reply| postui_core::jq::ai::extract_filter(&reply)) {
                    Ok(Some(filter)) => {
                        let cursor = filter.chars().count();
                        self.session
                            .response
                            .set_jq_text_with_cursor(&filter, cursor);
                        self.session.response.set_jq_focus(true);
                    }
                    Ok(None) => self
                        .toasts
                        .push("The AI command returned nothing", ToastKind::Warning),
                    Err(e) => self
                        .toasts
                        .push(format!("AI command failed: {e}"), ToastKind::Error),
                }
                true
            }
            Action::JqPluckPrompt { path, keys } => {
                use crate::components::chooser::{ChooserItem, ChooserState};
                use postui_core::jq::{PathSeg, compose, render_path};
                let bar = self.session.response.jq_text().to_string();
                let items = keys
                    .into_iter()
                    .map(|k| {
                        let key = render_path(&[PathSeg::Key(k.clone())]);
                        let expr = format!("map({key})");
                        ChooserItem {
                            label: k,
                            detail: None,
                            actions: vec![Action::JqApply(compose(&bar, &path, Some(&expr)))],
                            id: None,
                        }
                    })
                    .collect();
                self.push_modal(Modal::Chooser(ChooserState::new("Pluck field", items)));
                true
            }
            Action::JqWherePrompt { path, keys } => {
                use crate::components::chooser::{ChooserItem, ChooserState};
                use postui_core::jq::{PathSeg, compose, render_path};
                let bar = self.session.response.jq_text().to_string();
                let items = keys
                    .into_iter()
                    .map(|k| {
                        let key = render_path(&[PathSeg::Key(k.clone())]);
                        let expr = format!("map(select({key} == ))");
                        let text = compose(&bar, &path, Some(&expr));
                        let cursor = text.chars().count() - 2; // before "))"
                        ChooserItem {
                            label: k,
                            detail: None,
                            actions: vec![Action::JqTeeUp { text, cursor }],
                            id: None,
                        }
                    })
                    .collect();
                self.push_modal(Modal::Chooser(ChooserState::new("Where field", items)));
                true
            }
            Action::JqCollect => {
                let text = format!("[ {} ]", self.session.response.jq_text().trim());
                let cursor = text.chars().count();
                self.session.response.set_jq_text_with_cursor(&text, cursor);
                true
            }
            Action::ReloadProjectFiles => {
                let (changed, warnings) = self.project.reload_if_changed();
                if changed {
                    // The rows a live drag is rearranging are about to be
                    // rebuilt from a tree that changed on disk under it:
                    // its working order names siblings that may be gone,
                    // so it cancels rather than write an order nobody saw.
                    self.finish_sidebar_drag(false);
                    self.finish_manage_drag(false);
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
                    && let Some((name, selector)) =
                        Self::selection_picker_target(&self.project, &text, cursor)
                {
                    return self.open_select_picker(name, selector);
                }
                self.open_insert_var_picker(completing, None)
            }
            Action::OpenVarTokenPopup(name) => {
                self.apply(Action::ReloadProjectFiles);
                use postui_core::varmodel::VarMeta;
                match self.project.resolved.meta.get(&name).cloned() {
                    Some(VarMeta::SelectorMember { selector, .. }) => {
                        return self.open_select_picker(name, selector);
                    }
                    Some(VarMeta::NeedsSelection) => {
                        let Some(selector) = self
                            .project
                            .model
                            .selectors
                            .iter()
                            .find(|(_, s)| s.fields.contains(&name))
                            .map(|(n, _)| n.clone())
                        else {
                            return self.open_insert_var_picker(false, Some(&name));
                        };
                        return self.open_select_picker(name, selector);
                    }
                    Some(VarMeta::Secret) | Some(VarMeta::MissingSecret) => {
                        self.push_modal(Modal::Prompt {
                            title: format!("Secret {{{{{name}}}}}"),
                            input: LineInput::new(""),
                            kind: PromptKind::SecretValue {
                                name,
                                env: self.project.active_env.clone().unwrap_or_default(),
                            },
                            revealed: false,
                        });
                        return true;
                    }
                    Some(VarMeta::Simple) => return self.open_edit_value_popup(&name),
                    None => {}
                }
                // Undeclared: a request-scoped or stray env value still has
                // a value to edit; a name defined nowhere gets the insert
                // picker, whose "new variable…" row is the create flow.
                let has_value = self.editor.variables.contains_key(&name)
                    || self.project.resolved.values.contains_key(&name);
                if has_value {
                    self.open_edit_value_popup(&name)
                } else {
                    self.open_insert_var_picker(false, Some(&name))
                }
            }
            Action::ConfirmEditVarValue {
                name,
                value,
                destination,
            } => {
                use crate::action::ExtractDestination;
                let op = match destination {
                    ExtractDestination::Request => VarEditOp::SetRequestVar { name, value },
                    ExtractDestination::ActiveEnv => {
                        let Some(env) = self.project.active_env.clone() else {
                            self.toasts
                                .push("no active environment to write to", ToastKind::Warning);
                            return true;
                        };
                        VarEditOp::SetEnvValue { env, name, value }
                    }
                    ExtractDestination::ProjectDefault => VarEditOp::SetDefault { name, value },
                };
                self.apply(Action::VarEdit(op))
            }
            Action::RemoveVarValue { name, destination } => {
                use crate::action::ExtractDestination;
                use postui_core::varedit;
                match destination {
                    ExtractDestination::Request => {
                        self.no_coalesce = true;
                        self.editor.variables.shift_remove(&name);
                        self.toasts.push(
                            format!("removed this request's {name} override"),
                            ToastKind::Success,
                        );
                    }
                    ExtractDestination::ActiveEnv => {
                        let Some(env) = self.project.active_env.clone() else {
                            self.toasts
                                .push("no active environment", ToastKind::Warning);
                            self.last_action_failed = true;
                            return true;
                        };
                        // A secret's stored value lives in the secrets
                        // store, not the env file (the variable form's
                        // remove control reaches here for secrets too).
                        let secret = self.project.model.vars.get(&name).is_some_and(|d| d.secret);
                        if secret {
                            match self.project.remove_secret_for(&env, &name) {
                                Ok(()) => {
                                    self.toasts.push(
                                        format!("removed {name}'s value for env {env}"),
                                        ToastKind::Success,
                                    );
                                }
                                Err(msg) => {
                                    self.toasts.push(msg, ToastKind::Error);
                                    self.last_action_failed = true;
                                }
                            }
                            return true;
                        }
                        let before = self.read_file_states(&self.project.var_file_paths());
                        match self
                            .project
                            .edit_env(&env, |doc| varedit::set_env_value(doc, &name, None))
                        {
                            Ok(()) => {
                                self.record_var_file_step(before);
                                self.toasts.push(
                                    format!("removed {name} from env {env}"),
                                    ToastKind::Success,
                                );
                            }
                            Err(msg) => {
                                self.toasts.push(msg, ToastKind::Error);
                                self.last_action_failed = true;
                            }
                        }
                    }
                    ExtractDestination::ProjectDefault => {
                        let before = self.read_file_states(&self.project.var_file_paths());
                        match self
                            .project
                            .edit_variables(|doc| varedit::clear_default(doc, &name))
                        {
                            Ok(()) => {
                                self.record_var_file_step(before);
                                self.toasts
                                    .push(format!("removed {name}'s default"), ToastKind::Success);
                            }
                            Err(msg) => {
                                self.toasts.push(msg, ToastKind::Error);
                                self.last_action_failed = true;
                            }
                        }
                    }
                }
                true
            }
            Action::OpenNewVariablePrompt {
                prefill,
                completing,
            } => {
                self.push_modal(Modal::Prompt {
                    title: "New variable".into(),
                    input: LineInput::new(&prefill),
                    kind: PromptKind::NewVariableAndInsert { completing },
                    revealed: false,
                });
                true
            }
            Action::InsertVarText(text) => {
                self.no_coalesce = true;
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
            Action::Paste => {
                match self.clipboard.read() {
                    Ok(text) => {
                        self.paste_text(&text);
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("clipboard read failed: {e}"), ToastKind::Warning);
                    }
                }
                true
            }
            Action::OpenManage { tab } => {
                // A tab switch here `reset`s the list, which would drop a
                // live space drag on the floor with its press still
                // armed: cancel it first, as `SelectManageTab` does.
                self.finish_manage_drag(false);
                // A toggle: alt+v (and the header Manage chip) close the
                // screen they opened. A request for the tab that's already
                // up toggles too; a request for a different tab switches.
                let target = tab.unwrap_or(self.manage.tab);
                if self.screen == Screen::Manage && (tab.is_none() || self.manage.tab == target) {
                    return self.update(Action::CloseScreen);
                }
                if self.manage.tab != target {
                    self.manage.list.reset();
                }
                let prev = self.manage.tab;
                self.manage.tab = target;
                if self.screen != Screen::Manage {
                    self.prior_focus = self.focus;
                    self.screen = Screen::Manage;
                    // A freshly opened screen snaps its underline onto
                    // the active tab: a glide from wherever the strip
                    // last was would read as a switch that never happened.
                    self.anims.clear(AnimKey::TabUnderline(StripId::ManageTabs));
                    self.anims
                        .clear(AnimKey::TabUnderlineWidth(StripId::ManageTabs));
                } else if prev != target {
                    self.retarget_manage_tab_underline(prev);
                }
                true
            }
            Action::SelectManageTab(tab) => {
                // A live space drag belongs to the Spaces tab's list: the
                // tab strip switching out from under it cancels it (and
                // `reset` below would drop the drag on the floor anyway).
                self.finish_manage_drag(false);
                // Each tab lists something else: a cursor (and any name
                // edit) carried across would point at the wrong item.
                if self.manage.tab != tab {
                    self.manage.list.reset();
                    let prev = self.manage.tab;
                    self.manage.tab = tab;
                    self.retarget_manage_tab_underline(prev);
                }
                true
            }
            Action::CloseScreen => {
                // Leaving the screen mid-drag cancels it — there is no
                // list left to drop onto.
                self.finish_manage_drag(false);
                self.screen = Screen::Main;
                self.focus = self.prior_focus;
                true
            }
            Action::VarEdit(op) => {
                let before = self.read_file_states(&self.project.var_file_paths());
                match self.apply_var_edit(&op) {
                    Ok(()) => self.record_var_file_step(before),
                    Err(msg) => {
                        self.toasts.push(msg, ToastKind::Error);
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::PromptNewVar => {
                self.push_modal(Modal::Prompt {
                    title: "New variable".into(),
                    input: LineInput::new(""),
                    kind: PromptKind::NewVariable,
                    revealed: false,
                });
                true
            }
            Action::PromptNewSelector => {
                self.push_modal(Modal::Prompt {
                    title: "New selector".into(),
                    input: LineInput::new(""),
                    kind: PromptKind::NewSelector {
                        shared: false,
                        on_toggle: false,
                    },
                    revealed: false,
                });
                true
            }
            Action::PromptAddSelectorField { selector } => {
                self.push_modal(Modal::Prompt {
                    title: format!("Add field to {selector}"),
                    input: LineInput::new(""),
                    kind: PromptKind::AddSelectorField { selector },
                    revealed: false,
                });
                true
            }
            Action::AddSelectorField { selector, field } => {
                let current = self
                    .project
                    .model
                    .selectors
                    .get(&selector)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                if current.iter().any(|m| m == &field) {
                    self.toasts.push(
                        format!("\"{field}\" is already a field of {selector}"),
                        ToastKind::Warning,
                    );
                    self.last_action_failed = true;
                    return true;
                }
                let mut fields = current;
                fields.push(field);
                self.apply(Action::VarStruct(VarStructOp::SetFields {
                    selector,
                    fields,
                }));
                true
            }
            Action::RemoveSelectorField { selector, field } => {
                // Env files first: variables.toml's validation runs against
                // the active env, whose options must no longer carry the
                // field by the time the selector's field list changes.
                let Some(fields) = self
                    .project
                    .model
                    .selectors
                    .get(&selector)
                    .map(|g| g.fields.clone())
                else {
                    self.toasts
                        .push(format!("no selector \"{selector}\""), ToastKind::Error);
                    return true;
                };
                let before = self.read_file_states(&self.project.var_file_paths());
                let remaining: Vec<String> = fields.into_iter().filter(|f| f != &field).collect();
                let result = if self.selector_is_shared(&selector) {
                    // Options live beside the declaration: strip the field
                    // from them and rewrite the list in one write.
                    self.project.edit_variables(|doc| {
                        let stripped =
                            postui_core::varedit::strip_option_field(doc, &selector, &field)?;
                        postui_core::varedit::upsert_selector(
                            &stripped, &selector, None, &remaining,
                        )
                    })
                } else {
                    let envs = postui_core::project::list_environments(&self.project.root);
                    envs.iter()
                        .try_for_each(|env| {
                            self.project.edit_env(env, |doc| {
                                postui_core::varedit::strip_option_field(doc, &selector, &field)
                            })
                        })
                        .and_then(|()| {
                            self.project.edit_variables(|doc| {
                                postui_core::varedit::upsert_selector(
                                    doc, &selector, None, &remaining,
                                )
                            })
                        })
                };
                match result {
                    Ok(()) => {
                        self.record_var_file_step(before);
                        self.toasts.push(
                            format!("removed \"{field}\" from {selector}{}", self.undo_hint()),
                            ToastKind::Info,
                        );
                    }
                    Err(msg) => {
                        self.toasts.push(msg, ToastKind::Error);
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::PromptRenameVar { from } => {
                // Finding 7: surface `scan_usage`'s count the same way
                // `DeleteVar` already does — renaming doesn't break
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
                self.push_modal(Modal::Prompt {
                    title,
                    input: LineInput::new(&from),
                    kind: PromptKind::RenameVariable { from },
                    revealed: false,
                });
                true
            }
            Action::DuplicateVar { name } => {
                let before = self.read_file_states(&self.project.var_file_paths());
                if let Err(msg) = self.apply_duplicate_var(&name) {
                    self.toasts.push(msg, ToastKind::Error);
                    self.last_action_failed = true;
                } else {
                    self.record_var_file_step(before);
                    self.varmanager.sync(&self.project);
                }
                true
            }
            Action::DeleteVar { name } => {
                let usage = postui_core::varedit::scan_usage(&self.project.root, &name);
                self.apply(Action::VarStruct(VarStructOp::Delete {
                    name: name.clone(),
                }));
                // The struct arm toasts the delete itself; the deleted
                // declaration leaving dangling references is worth its own
                // warning on top.
                if !self.project.model.vars.contains_key(&name) && !usage.is_empty() {
                    self.toasts.push(
                        format!(
                            "\"{name}\" was referenced by {} request(s): {}",
                            usage.len(),
                            usage.join(", ")
                        ),
                        ToastKind::Warning,
                    );
                }
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
                // refusal. Refuse up front instead with a message modal.
                if self.project.model.vars.get(&name).is_some_and(|d| d.secret) {
                    self.push_modal(Modal::Message {
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
                self.push_modal(Modal::Confirm {
                    title: format!("Promote {name}"),
                    body: "Where should the value land?".into(),
                    choices,
                });
                true
            }
            Action::VarStruct(op) => {
                // Sampled before the mutation so the delete toasts below
                // stay quiet when the target was already gone (deleting a
                // missing entry is a no-op, not news).
                let existed = match &op {
                    VarStructOp::Delete { name } => self.project.model.vars.contains_key(name),
                    VarStructOp::DeleteOption {
                        env,
                        selector,
                        name,
                    } => self
                        .options_of_for(env, selector)
                        .is_some_and(|o| o.contains_key(name)),
                    _ => false,
                };
                let before = self.read_file_states(&self.project.var_file_paths());
                match self.apply_var_struct(&op) {
                    Ok(()) => {
                        // A rename carries the detail pane's selection over
                        // rather than emptying it: the user is still
                        // looking at the same declaration, under its new
                        // name (`sync` would otherwise drop it as gone).
                        if let VarStructOp::Rename { from, to } = &op {
                            let vm = &mut self.varmanager;
                            if vm.detail == VmDetail::Var(from.clone()) {
                                vm.detail = VmDetail::Var(to.clone());
                            } else if vm.detail == VmDetail::Group(from.clone()) {
                                vm.detail = VmDetail::Group(to.clone());
                            }
                        }
                        self.varmanager.sync(&self.project);
                        // A fresh declaration becomes the selected row —
                        // the user's next action is almost always on it.
                        if let VarStructOp::NewVar { name, .. }
                        | VarStructOp::NewSelector { name, .. } = &op
                        {
                            self.varmanager.select_name(name);
                        }
                        self.record_var_file_step(before);
                        // Deletes act without a confirm gate, so their
                        // toasts advertise the way back.
                        match &op {
                            _ if !existed => {}
                            VarStructOp::Delete { name } => self.toasts.push(
                                format!("Deleted \"{name}\"{}", self.undo_hint()),
                                ToastKind::Info,
                            ),
                            VarStructOp::DeleteOption { name, env, .. } => self.toasts.push(
                                format!("Deleted option \"{name}\" from {env}{}", self.undo_hint()),
                                ToastKind::Info,
                            ),
                            _ => {}
                        }
                    }
                    Err(msg) => {
                        self.toasts.push(msg, ToastKind::Error);
                        self.last_action_failed = true;
                    }
                }
                true
            }

            // -- Task 16: the selector options grid (spec §3.4) --
            Action::PromptGroupFields { selector } => {
                use crate::components::modal::FieldsEditorState;
                let current = self
                    .project
                    .model
                    .selectors
                    .get(&selector)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                self.push_modal(Modal::FieldsEditor(FieldsEditorState::new(
                    selector, &current,
                )));
                true
            }
            Action::ApplyGroupFields { selector, slots } => {
                let before = self.read_file_states(&self.project.var_file_paths());
                self.apply_group_fields(selector, slots);
                self.record_var_file_step(before);
                true
            }
            Action::StartOptionNameEdit { row } => {
                if !matches!(&self.varmanager.detail, VmDetail::Group(_))
                    || self.project.active_env.is_none()
                {
                    return false;
                }
                self.varmanager.start_cell_edit(&self.project, row, 0);
                true
            }
            Action::DeleteEntry {
                env,
                selector,
                name,
            } => {
                self.apply(Action::VarStruct(VarStructOp::DeleteOption {
                    env,
                    selector,
                    name,
                }));
                true
            }
            Action::StartNewOptionEdit => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                // A shared selector's options need no environment; anyone
                // else's have nowhere to live without one.
                if self.project.active_env.is_none() && !self.selector_is_shared(&selector) {
                    self.toasts.push(
                        crate::components::varmanager::NO_ENV_HINT,
                        ToastKind::Warning,
                    );
                    return true;
                }
                // The ghost row *is* the new-option affordance: put the
                // cursor in its name cell and start typing.
                let row = postui_core::varmodel::options_of(
                    &self.project.model,
                    &self.project.env_data,
                    &selector,
                )
                .map_or(0, indexmap::IndexMap::len);
                self.varmanager.start_cell_edit(&self.project, row, 0);
                true
            }

            // -- Task 17: in-context flows (spec §6) --
            Action::OpenNewOptionInlinePrompt { owner } => {
                use crate::components::modal::{NEW_OPTION_FIELD, PromptField};
                // One input per selector field, so the option is whole
                // when it lands: a selector's fields are meant to be set
                // together. A one-field selector's input just reads
                // "Value". No description field: the quick-create flow
                // stays lean; a description can be added later through
                // the option's edit prompt in the Manager.
                let selector_fields = self
                    .project
                    .model
                    .selectors
                    .get(&owner)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                let mut fields = vec![PromptField::text("key", "Name", "")];
                for field in &selector_fields {
                    let label = if selector_fields.len() == 1 {
                        "Value"
                    } else {
                        field.as_str()
                    };
                    fields.push(PromptField::text(
                        &format!("{NEW_OPTION_FIELD}{field}"),
                        label,
                        "",
                    ));
                }
                self.push_modal(Modal::MultiPrompt {
                    title: format!("Add option on {owner}"),
                    fields,
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
                self.push_modal(Modal::MultiPrompt {
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
                values,
                description,
            } => {
                if key.is_empty() {
                    self.toasts
                        .push("option name can't be empty", ToastKind::Error);
                    self.last_action_failed = true;
                    return true;
                }
                if key == postui_core::varmodel::OPTION_DESCRIPTION {
                    self.toasts.push(
                        format!(
                            "\"{key}\" is reserved for an option's own description and can't be used as an option name"
                        ),
                        ToastKind::Error,
                    );
                    self.last_action_failed = true;
                    return true;
                }
                // A shared selector's options don't live in an environment,
                // so it doesn't need one to be active.
                let shared = self.selector_is_shared(&owner);
                let env = match self.project.active_env.clone() {
                    Some(env) => env,
                    None if shared => String::new(),
                    None => {
                        self.toasts.push(
                            "no active environment \u{2014} switch to one first",
                            ToastKind::Warning,
                        );
                        return true;
                    }
                };
                // Create means create: writing over an existing option of
                // the same name from the add prompt would silently clobber
                // its values.
                if postui_core::varmodel::options_of(
                    &self.project.model,
                    &self.project.env_data,
                    &owner,
                )
                .is_some_and(|options| options.contains_key(&key))
                {
                    self.toasts.push(
                        format!("option \"{key}\" already exists on {owner}"),
                        ToastKind::Error,
                    );
                    self.last_action_failed = true;
                    return true;
                }
                // An option has to supply every field of its selector or
                // `validate_env` rejects it: the prompt collects one value
                // per field, and any field it didn't know about (a caller
                // passing a partial map) starts empty for the Manager.
                let mut values = values;
                let fields = self
                    .project
                    .model
                    .selectors
                    .get(&owner)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                for field in fields {
                    values.entry(field).or_default();
                }
                let before = self.read_file_states(&self.project.var_file_paths());
                match self.edit_options_home(&owner, &env, |doc| {
                    postui_core::varedit::upsert_option(
                        doc,
                        &owner,
                        &key,
                        description.as_deref(),
                        &values,
                    )
                }) {
                    Ok(()) => {
                        self.record_var_file_step(before);
                        self.project.set_selection_for(&env, &owner, &key);
                        let where_label = if shared { "all environments" } else { &env };
                        self.toasts.push(
                            format!("{owner} \u{2192} {key} ({where_label})"),
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
                // An option belongs to exactly one environment, so the
                // edit always lands in the active env's file.
                let Some(env) = self.project.active_env.clone() else {
                    self.toasts.push(
                        "no active environment \u{2014} switch to one first",
                        ToastKind::Warning,
                    );
                    return true;
                };
                let before = self.read_file_states(&self.project.var_file_paths());
                let result = self.project.edit_env(&env, |doc| {
                    // The prompt maps a cleared Description field to `None`,
                    // which means "remove the stored description" here —
                    // `upsert_option`'s own `None` deliberately preserves
                    // one (the inline cell edits rely on that).
                    let doc = postui_core::varedit::upsert_option(
                        doc,
                        &owner,
                        &key,
                        description.as_deref(),
                        &values,
                    )?;
                    if description.is_some() {
                        return Ok(doc);
                    }
                    postui_core::varedit::remove_option_description(&doc, &owner, &key)
                });
                match result {
                    Ok(()) => {
                        self.record_var_file_step(before);
                        self.toasts
                            .push(format!("{key} updated"), ToastKind::Success);
                    }
                    Err(msg) => self.toasts.push(msg, ToastKind::Error),
                }
                true
            }
            Action::ExtractToVariable => {
                if self.focused_extract_text().is_none() {
                    return true;
                }
                use crate::components::modal::PromptField;
                self.push_modal(Modal::MultiPrompt {
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
                self.confirm_extract(name, destination, ExtractSource::FocusedField)
            }
            Action::ConfirmExtractSelection {
                name,
                destination,
                surface,
            } => self.confirm_extract(name, destination, ExtractSource::Selection(surface)),
            Action::ExtractSelection(surface) => {
                if self.selection_extract_text(surface).is_none() {
                    return true;
                }
                use crate::components::modal::PromptField;
                self.push_modal(Modal::MultiPrompt {
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
                    kind: PromptKind::ExtractSelection(surface),
                });
                true
            }
            Action::ExtractToSelector => {
                if let Some(text) = self.focused_extract_text() {
                    self.open_extract_selector_prompt(&text, ExtractSource::FocusedField);
                }
                true
            }
            Action::ExtractSelectionToSelector(surface) => {
                if let Some(text) = self.selection_extract_text(surface) {
                    self.open_extract_selector_prompt(&text, ExtractSource::Selection(surface));
                }
                true
            }
            Action::ConfirmExtractToSelector {
                name,
                option,
                shared,
                source,
            } => self.confirm_extract_to_selector(name, option, shared, source),
            Action::SwitchSpace(name) => {
                if name == self.project.active_space {
                    return true;
                }
                if self.editor_holds_unsaved() {
                    self.dirty_gate("switch", Action::ForceSwitchSpace(name));
                    true
                } else {
                    self.apply(Action::ForceSwitchSpace(name))
                }
            }
            Action::ForceSwitchSpace(name) => {
                let outgoing = self.editor.slug.clone();
                if !self.enter_space(&name, SpaceExit::Remember(outgoing.as_deref())) {
                    return true;
                }
                // What the space was last left on, when that request still
                // exists; otherwise its first row; otherwise nothing.
                let target = self
                    .project
                    .space_open_for(&name)
                    .filter(|s| postui_core::storage::request_exists(&self.project.root, s))
                    .or_else(|| self.sidebar.first_request_slug());
                match target {
                    Some(slug) => self.apply(Action::ForceOpenRequest(slug)),
                    None => {
                        self.editor = Editor::default();
                        self.shadow = None;
                        self.sidebar.open_slug = None;
                        self.apply(Action::PersistLocalState)
                    }
                }
            }
            Action::JumpSpace(n) => {
                match n
                    .checked_sub(1)
                    .and_then(|i| self.project.spaces.get(i))
                    .cloned()
                {
                    Some(name) => self.apply(Action::SwitchSpace(name)),
                    None => true,
                }
            }
            Action::CycleSpace(delta) => {
                let spaces = &self.project.spaces;
                if spaces.is_empty() {
                    return true;
                }
                let idx = spaces
                    .iter()
                    .position(|s| *s == self.project.active_space)
                    .unwrap_or(0) as i32;
                let next = (idx + delta).rem_euclid(spaces.len() as i32) as usize;
                let name = spaces[next].clone();
                self.apply(Action::SwitchSpace(name))
            }
            Action::OpenSpaceChooser => {
                self.apply(Action::ReloadProjectFiles);
                use crate::components::modal::{DropdownState, MenuItem};
                let mut items: Vec<MenuItem> = self
                    .project
                    .spaces
                    .iter()
                    .enumerate()
                    .map(|(i, slug)| {
                        MenuItem::new(
                            format!("{}  {}", i + 1, self.project.space_name(slug)),
                            Action::SwitchSpace(slug.clone()),
                        )
                    })
                    .collect();
                items.push(MenuItem::new("new space…", Action::OpenNewSpacePrompt));
                items.push(MenuItem::new(
                    "manage spaces…",
                    Action::OpenManage {
                        tab: Some(crate::components::manage::ManageTab::Spaces),
                    },
                ));
                let current = self
                    .project
                    .spaces
                    .iter()
                    .position(|s| *s == self.project.active_space);
                let anchor = self
                    .hits
                    .rect_of(&Hit::HeaderSpace)
                    .unwrap_or_else(|| ratatui::layout::Rect::new(0, 0, 0, 0));
                self.push_modal(Modal::Dropdown(DropdownState {
                    anchor,
                    items,
                    selected: current.unwrap_or(0),
                    current,
                }));
                self.begin_dropdown_open();
                true
            }
            Action::PromptMoveRequestToSpace(slug) => {
                use crate::components::chooser::ChooserState;
                let own = postui_core::storage::space_of(&slug).map(str::to_string);
                let items =
                    self.other_space_items(own.as_deref(), |space| Action::MoveRequestToSpace {
                        slug: slug.clone(),
                        space,
                    });
                if items.is_empty() {
                    self.toasts
                        .push("No other space to move to", ToastKind::Info);
                    return true;
                }
                self.push_modal(Modal::Chooser(ChooserState::new("Move to space", items)));
                true
            }
            Action::PromptMoveAllRequests(from) => {
                use crate::components::chooser::ChooserState;
                let items = self.other_space_items(Some(&from), |to| Action::MoveAllRequests {
                    from: from.clone(),
                    to,
                });
                if items.is_empty() {
                    self.toasts
                        .push("No other space to move to", ToastKind::Info);
                    return true;
                }
                self.push_modal(Modal::Chooser(ChooserState::new(
                    "Move all requests to",
                    items,
                )));
                true
            }
            Action::OpenNewSpacePrompt => {
                self.push_modal(Modal::Prompt {
                    title: "New space".into(),
                    input: crate::components::line_input::LineInput::new(""),
                    kind: PromptKind::NewSpace,
                    revealed: false,
                });
                true
            }
            Action::CreateSpace(name) => {
                match postui_core::project::create_space(&self.project.root, &name) {
                    Ok(slug) => {
                        self.apply(Action::ReloadProjectFiles);
                        self.project.reload_meta();
                        self.project.reload_spaces();
                        self.toasts.push(
                            format!("Created space {}", self.project.space_name(&slug)),
                            ToastKind::Success,
                        );
                        self.apply(Action::SwitchSpace(slug))
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("cannot create space: {e}"), ToastKind::Warning);
                        self.last_action_failed = true;
                        true
                    }
                }
            }
            Action::PromptRenameEnv(name) => {
                self.push_modal(Modal::Prompt {
                    title: "Rename environment".into(),
                    input: crate::components::line_input::LineInput::new(
                        &self.project.env_name(&name),
                    ),
                    kind: PromptKind::RenameEnvironment { from: name },
                    revealed: false,
                });
                true
            }
            Action::RenameEnv { from, to } => {
                // `from` is a slug; `to` is the display name typed.
                if to.trim() == self.project.env_name(&from) {
                    return true;
                }
                let root = self.project.root.clone();
                let from_path = postui_core::project::environment_path(&root, &from);
                let to_slug =
                    postui_core::project::environment_slug_for(&root, to.trim(), Some(&from));
                let to_path = postui_core::project::environment_path(&root, &to_slug);
                let secrets_path = root.join(".local").join("secrets.toml");
                // `.local/state.toml` rides along: the per-env `selections`
                // table is re-keyed in memory by `rename_env_state` and only
                // ever reaches disk through `PersistLocalState`, so without
                // it in the step an undo would strand this env's selections
                // under the new name. So does project.toml, which holds the
                // display name.
                let state_path = root.join(".local").join("state.toml");
                let project_toml = root.join("project.toml");
                let paths = vec![from_path, to_path, secrets_path, state_path, project_toml];
                let before = self.read_file_states(&paths);
                let was_active = self.project.active_env.as_deref() == Some(from.as_str());
                match postui_core::project::rename_environment(&root, &from, &to) {
                    Ok(to) => {
                        self.project.reload_meta();
                        self.project.rename_env_state(&from, &to);
                        if let Err(e) =
                            postui_core::project::save_secrets(&root, &self.project.secrets)
                        {
                            self.toasts
                                .push(format!("could not save secrets: {e}"), ToastKind::Warning);
                        }
                        self.project.environments = postui_core::project::list_environments(&root);
                        if was_active {
                            // Reload data under the new name (set_env re-stamps too).
                            for w in self.project.set_env(Some(to.clone())) {
                                self.toasts.push(w, ToastKind::Warning);
                            }
                        }
                        // Persist first: the step's "after" side has to see
                        // the re-keyed selections already on disk.
                        self.apply(Action::PersistLocalState);
                        self.record_file_step(
                            before,
                            &paths,
                            was_active.then(|| (Some(from.clone()), Some(to.clone()))),
                        );
                        if self.screen == Screen::Manage {
                            self.varmanager.sync(&self.project);
                            self.manage
                                .list
                                .select_name(self.manage.tab, &self.project, &to);
                        }
                        self.toasts.push(
                            format!("Renamed environment to {}", self.project.env_name(&to)),
                            ToastKind::Success,
                        );
                    }
                    Err(e) => {
                        self.toasts.push(
                            format!("cannot rename environment: {e}"),
                            ToastKind::Warning,
                        );
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::SetEnvTls { env, policy } => {
                let root = self.project.root.clone();
                let paths = vec![root.join("project.toml")];
                let before = self.read_file_states(&paths);
                match postui_core::project::set_env_tls(&root, &env, policy) {
                    Ok(()) => {
                        self.project.reload_meta();
                        self.record_file_step(before, &paths, None);
                        let name = self.project.env_name(&env);
                        let msg = match policy {
                            Some(postui_core::project::TlsPolicy::Verify) => {
                                format!("{name} forces TLS verification")
                            }
                            Some(postui_core::project::TlsPolicy::Insecure) => {
                                format!("{name} skips TLS verification")
                            }
                            None => format!("{name} leaves TLS verification to each request"),
                        };
                        self.toasts.push(msg, ToastKind::Info);
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("cannot save TLS setting: {e}"), ToastKind::Warning);
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::DeleteEnv(name) => {
                // The last environment stays: the app has no "no
                // environment" state to fall back to, only a default env.
                if self.project.environments.len() <= 1 {
                    self.toasts.push(
                        "a project keeps at least one environment — rename it instead",
                        ToastKind::Warning,
                    );
                    return true;
                }
                self.push_modal(Modal::Confirm {
                    title: format!("Delete environment \"{}\"?", self.project.env_name(&name)),
                    body: "Its values and secrets are removed.".into(),
                    choices: vec![(
                        'd',
                        "Delete environment".into(),
                        vec![Action::ForceDeleteEnv(name)],
                    )],
                });
                true
            }
            Action::ForceDeleteEnv(name) => {
                let root = self.project.root.clone();
                let secrets_path = root.join(".local").join("secrets.toml");
                // See `Action::RenameEnv`: the dropped env's `selections`
                // live in `.local/state.toml`, so it is a companion file.
                let state_path = root.join(".local").join("state.toml");
                let project_toml = root.join("project.toml");
                let companions = [secrets_path, state_path, project_toml];
                let before = self.read_file_states(&companions);
                let prev_active = self.project.active_env.clone();
                let was_active = prev_active.as_deref() == Some(name.as_str());
                let display = self.project.env_name(&name);
                match postui_core::project::delete_environment(&root, &name) {
                    Ok(trashed) => {
                        self.project.reload_meta();
                        self.project.remove_env_state(&name);
                        if let Err(e) =
                            postui_core::project::save_secrets(&root, &self.project.secrets)
                        {
                            self.toasts
                                .push(format!("could not save secrets: {e}"), ToastKind::Warning);
                        }
                        self.project.environments = postui_core::project::list_environments(&root);
                        // The active env is gone: fall through to the
                        // first remaining one rather than to no env.
                        let fallback = self.project.environments.first().cloned();
                        if was_active {
                            for w in self.project.set_env(fallback.clone()) {
                                self.toasts.push(w, ToastKind::Warning);
                            }
                        }
                        self.toasts.push(
                            format!("Deleted environment {display}{}", self.undo_hint()),
                            ToastKind::Info,
                        );
                        // Persist first, so the step's "after" side records
                        // the state file without this environment.
                        self.apply(Action::PersistLocalState);
                        self.record_trashed_step(
                            vec![trashed],
                            before,
                            &companions,
                            was_active.then(|| (prev_active.clone(), fallback)),
                        );
                        if self.screen == Screen::Manage {
                            self.varmanager.sync(&self.project);
                        }
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("cannot delete environment: {e}"), ToastKind::Error);
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::PromptRenameSpace(name) => {
                self.push_modal(Modal::Prompt {
                    title: "Rename space".into(),
                    input: crate::components::line_input::LineInput::new(
                        &self.project.space_name(&name),
                    ),
                    kind: PromptKind::RenameSpace { from: name },
                    revealed: false,
                });
                true
            }
            Action::RenameSpace { from, to } => {
                // `from` is a slug; `to` is the display name typed.
                if to.trim() == self.project.space_name(&from) {
                    return true;
                }
                match postui_core::project::rename_space(&self.project.root, &from, &to) {
                    Ok(to) => {
                        // Re-key local state (the active space included)
                        // before anything re-lists: both
                        // `ReloadProjectFiles` and `reload_spaces` drop an
                        // active space they no longer find on disk, and
                        // the old name is gone by now.
                        self.project.rename_space_state(&from, &to);
                        self.apply(Action::ReloadProjectFiles);
                        self.project.reload_meta();
                        self.project.reload_spaces();
                        let from_prefix = format!("{from}/");
                        if let Some(rest) = self
                            .editor
                            .slug
                            .as_deref()
                            .and_then(|s| s.strip_prefix(&from_prefix))
                        {
                            let new_slug = format!("{to}/{rest}");
                            self.editor.slug = Some(new_slug.clone());
                            self.sidebar.open_slug = Some(new_slug);
                            if let Some((slug, _)) = self.shadow.as_mut() {
                                *slug = self.editor.slug.clone();
                            }
                        }
                        self.refresh_sidebar();
                        self.apply(Action::PersistLocalState);
                        self.toasts.push(
                            format!("Renamed space to {}", self.project.space_name(&to)),
                            ToastKind::Success,
                        );
                        if self.screen == Screen::Manage {
                            self.varmanager.sync(&self.project);
                            self.manage
                                .list
                                .select_name(self.manage.tab, &self.project, &to);
                        }
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("cannot rename space: {e}"), ToastKind::Warning);
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::DeleteSpace(name) => {
                let open_here = self
                    .editor
                    .slug
                    .as_deref()
                    .and_then(postui_core::storage::space_of)
                    == Some(name.as_str());
                if open_here && self.editor_holds_unsaved() {
                    self.dirty_gate("delete space", Action::PromptDeleteSpace(name));
                } else {
                    self.apply(Action::PromptDeleteSpace(name));
                }
                true
            }
            Action::PromptDeleteSpace(name) => {
                if self.project.spaces.len() <= 1 {
                    self.toasts
                        .push("cannot delete the last space", ToastKind::Warning);
                    return true;
                }
                let count = self.sidebar.space_counts().get(&name).copied().unwrap_or(0);
                let (body, label) = if count == 0 {
                    (String::new(), "Delete space".to_string())
                } else {
                    let noun = if count == 1 { "request" } else { "requests" };
                    (
                        format!("Its {count} {noun} will be deleted."),
                        format!("Delete {count} {noun}"),
                    )
                };
                self.push_modal(Modal::Confirm {
                    title: format!("Delete space \"{}\"?", self.project.space_name(&name)),
                    body,
                    choices: vec![('d', label, vec![Action::ForceDeleteSpace(name)])],
                });
                true
            }
            Action::ForceDeleteSpace(name) => {
                if self.project.spaces.len() <= 1 {
                    self.toasts
                        .push("cannot delete the last space", ToastKind::Warning);
                    return true;
                }
                // Leave the space before it goes: the switch restores the
                // other space's own open request and clears this one's.
                if self.project.active_space == name
                    && let Some(other) = self.project.spaces.iter().find(|s| **s != name).cloned()
                {
                    self.apply(Action::ForceSwitchSpace(other));
                }
                let project_toml = self.project.root.join("project.toml");
                let before = self.read_file_states(std::slice::from_ref(&project_toml));
                let display = self.project.space_name(&name);
                match postui_core::project::delete_space(&self.project.root, &name) {
                    Ok(trashed) => {
                        self.toasts.push(
                            format!("Deleted space {display}{}", self.undo_hint()),
                            ToastKind::Info,
                        );
                        self.record_trashed_step(
                            trashed.into_iter().collect(),
                            before,
                            &[project_toml],
                            None,
                        );
                        self.project.forget_space(&name);
                        self.reload_after_file_change();
                        self.apply(Action::PersistLocalState);
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("cannot delete space: {e}"), ToastKind::Error);
                        self.last_action_failed = true;
                    }
                }
                true
            }
            Action::MoveSpace { name, delta } => {
                match postui_core::project::move_space(&self.project.root, &name, delta) {
                    Ok(()) => {
                        self.apply(Action::ReloadProjectFiles);
                        // `ReloadProjectFiles` is mtime-gated, so a second
                        // reorder in the same clock tick would re-list from
                        // a stale `meta` and undo itself on screen. Read
                        // the file we just wrote instead of waiting for the
                        // stamp to move.
                        self.project.reload_meta();
                        self.project.reload_spaces();
                        // The Manage screen's list cursor follows the space
                        // that just moved, rather than staying on the row
                        // index the reorder swapped something else into.
                        if self.screen == Screen::Manage {
                            self.manage
                                .list
                                .select_name(self.manage.tab, &self.project, &name);
                        }
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("cannot move space: {e}"), ToastKind::Warning);
                    }
                }
                true
            }
            Action::MoveRequest { slug, delta } => {
                match postui_core::order::move_request(&self.project.root, &slug, delta) {
                    Ok(()) => {
                        // Same mtime hazard as `MoveSpace`: read the file
                        // we just wrote rather than waiting for the stamp.
                        self.project.reload_meta();
                        self.refresh_sidebar();
                        self.sidebar.select_slug(&slug);
                    }
                    Err(e) => {
                        self.toasts
                            .push(format!("cannot move request: {e}"), ToastKind::Warning);
                    }
                }
                true
            }
            Action::MoveSelectedRequest(delta) => {
                if let Some(slug) = self.sidebar.selected_slug() {
                    self.apply(Action::MoveRequest { slug, delta });
                }
                true
            }
            Action::MoveAllRequests { from, to } => {
                if from == to {
                    self.toasts
                        .push(format!("already in {to}"), ToastKind::Warning);
                    self.last_action_failed = true;
                    return true;
                }
                if !self.project.spaces.contains(&to) {
                    self.toasts
                        .push(format!("no space named {to:?}"), ToastKind::Warning);
                    self.last_action_failed = true;
                    return true;
                }
                // The open request follows the move, and following it
                // reloads the editor from disk — so unsaved edits to a
                // request living in `from` are gated exactly like a space
                // switch.
                let open_here = self
                    .editor
                    .slug
                    .as_deref()
                    .and_then(postui_core::storage::space_of)
                    == Some(from.as_str());
                if open_here && self.editor_holds_unsaved() {
                    self.dirty_gate("move", Action::ForceMoveAllRequests { from, to });
                } else {
                    self.apply(Action::ForceMoveAllRequests { from, to });
                }
                true
            }
            Action::ForceMoveAllRequests { from, to } => {
                let open = self.editor.slug.clone();
                let (moved, err) =
                    postui_core::storage::move_all_requests(&self.project.root, &from, &to);
                if let Some(e) = err {
                    self.toasts.push(
                        format!("moved {} request(s), then failed: {e}", moved.len()),
                        ToastKind::Error,
                    );
                    self.last_action_failed = true;
                } else {
                    self.toasts.push(
                        format!("Moved {} request(s) to {to}", moved.len()),
                        ToastKind::Success,
                    );
                }
                // Every moved request leaves a stale entry behind in the
                // source's order list and arrives unlisted in the
                // destination — entries the app itself made stale, so they
                // are cascaded, exactly as the single-request move does.
                for (old, new) in &moved {
                    if let Some((from_space, from_rel)) = Self::split_rel(old) {
                        let r = postui_core::order::order_remove(
                            &self.project.root,
                            from_space,
                            from_rel,
                        );
                        self.order_cascade("move", r);
                    }
                    if let Some((to_space, to_rel)) = Self::split_rel(new) {
                        let r =
                            postui_core::order::order_arrive(&self.project.root, to_space, to_rel);
                        self.order_cascade("move", r);
                    }
                }
                self.project.forget_space(&from);
                self.refresh_sidebar();
                if let Some(open) = open
                    && let Some((_, new_slug)) = moved.iter().find(|(old, _)| *old == open)
                {
                    // Follow the open request into its new space.
                    // `ForceOpenRequest` owns the editor, the sidebar's
                    // open row and the re-seeded shadow — pre-setting any
                    // of them here would defeat `capture_undo`'s
                    // "which request is open changed → re-seed, never
                    // record" branch and forge a phantom edit step.
                    self.apply(Action::ForceOpenRequest(new_slug.clone()));
                } else {
                    self.apply(Action::PersistLocalState);
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
    /// token whose name is a selector field in the active env
    /// (spec §6's cursor-on-token rule) — and if so, the `(name, selector)`
    /// pair `PickerMode::SelectOption` wants: `name` is the token's own
    /// name and `selector` is the owning selector's. `None` when the cursor
    /// isn't on a token, or the token's name isn't a selector field (a
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
            Some(VarMeta::SelectorMember { selector, .. }) => Some((token.name, selector.clone())),
            Some(VarMeta::NeedsSelection) => {
                let selector = ctx
                    .model
                    .selectors
                    .iter()
                    .find(|(_, g)| g.fields.contains(&token.name))
                    .map(|(n, _)| n.clone())?;
                Some((token.name, selector))
            }
            _ => None,
        }
    }

    /// The text an extract-from-focused-field gesture (palette / row menu)
    /// would take, or `None` after toasting why there is none: the body
    /// (not supported yet), no focused field, or an empty one. As a side
    /// effect, promotes a merely-selected table row to a live Value-cell
    /// edit, which is what `focused_field_text` needs — the row menu only
    /// ever *selects* a row (see `context_menu_for`).
    fn focused_extract_text(&mut self) -> Option<String> {
        if self.focus == PaneId::Editor
            && self.editor.active_tab == EditorTab::Body
            && self.editor.sub_focus == SubFocus::Content
        {
            self.toasts.push(
                "extract isn't available in the body yet",
                ToastKind::Warning,
            );
            return None;
        }
        if self.focus == PaneId::Editor
            && self.editor.sub_focus == SubFocus::Content
            && self.editor.table.editing.is_none()
            && let Some(row) = self.editor.table.selected
            && row < self.editor.table_len()
        {
            self.editor
                .click_table_cell(row, crate::components::table_editor::Col::Value);
        }
        let Some(text) = self.focused_field_text().map(|(t, _)| t.to_string()) else {
            self.toasts
                .push("focus a text field first", ToastKind::Warning);
            return None;
        };
        if text.trim().is_empty() {
            self.toasts.push(
                "nothing to extract \u{2014} the field is empty",
                ToastKind::Warning,
            );
            return None;
        }
        Some(text)
    }

    /// `surface`'s selected text for an extract, or `None` after toasting
    /// that nothing (or only whitespace) is selected there.
    fn selection_extract_text(&mut self, surface: crate::action::TextSurface) -> Option<String> {
        let Some(text) = self.selection_text_of(surface) else {
            self.toasts
                .push("select some text first", ToastKind::Warning);
            return None;
        };
        if text.trim().is_empty() {
            self.toasts.push(
                "nothing to extract \u{2014} the selection is blank",
                ToastKind::Warning,
            );
            return None;
        }
        Some(text)
    }

    /// The "Extract to selector" prompt: selector name, option name (seeded
    /// from the value when it reads as a name — see `option_name_seed`),
    /// and whether the selector's options are per-environment or shared.
    fn open_extract_selector_prompt(&mut self, text: &str, source: ExtractSource) {
        use crate::components::modal::PromptField;
        self.push_modal(Modal::MultiPrompt {
            title: "Extract to selector".into(),
            fields: vec![
                PromptField::text("name", "Name", ""),
                PromptField::text("option", "Option", &option_name_seed(text)),
                PromptField::choice("scope", "Options", &["Per environment", "Shared"]),
            ],
            focus: 0,
            kind: PromptKind::ExtractSelector(source),
        });
    }

    /// `Action::ConfirmExtractToSelector`: creates selector `name` with
    /// the one field `name`, adds `option` holding the extracted text in
    /// the active environment (or the shared options), selects it there,
    /// and rewrites the source text to `{{name}}`. Every name check runs
    /// before the first write, so a refusal leaves both files untouched;
    /// the two writes plus the selection are one undo step.
    fn confirm_extract_to_selector(
        &mut self,
        name: String,
        option: String,
        shared: bool,
        source: ExtractSource,
    ) -> bool {
        if !postui_core::vars::is_valid_var_name(&name) {
            self.toasts.push(
                format!("\"{name}\" is not a valid selector name"),
                ToastKind::Error,
            );
            self.last_action_failed = true;
            return true;
        }
        if name_taken(&self.project, &name) {
            self.toasts
                .push(format!("\"{name}\" already exists"), ToastKind::Error);
            self.last_action_failed = true;
            return true;
        }
        let option = option.trim().to_string();
        if option.is_empty() {
            self.toasts
                .push("give the option a name", ToastKind::Warning);
            self.last_action_failed = true;
            return true;
        }
        let text = match source {
            ExtractSource::FocusedField => self.focused_field_text().map(|(t, _)| t.to_string()),
            ExtractSource::Selection(surface) => self.selection_text_of(surface),
        };
        let Some(text) = text else {
            let msg = match source {
                ExtractSource::FocusedField => "focus a text field first",
                ExtractSource::Selection(_) => "select some text first",
            };
            self.toasts.push(msg, ToastKind::Warning);
            return true;
        };
        // A per-environment selector's option needs an environment file to
        // land in; a shared one writes variables.toml whatever is active,
        // and its selection is global too.
        let env = match self.project.active_env.clone() {
            Some(env) => env,
            None if shared => String::new(),
            None => {
                self.toasts
                    .push("no active environment", ToastKind::Warning);
                return true;
            }
        };
        let before = self.read_file_states(&self.project.var_file_paths());
        let result = self
            .apply_var_struct(&VarStructOp::NewSelector {
                name: name.clone(),
                fields: vec![name.clone()],
                shared,
            })
            .and_then(|()| {
                let mut values = indexmap::IndexMap::new();
                values.insert(name.clone(), text.clone());
                self.apply_var_struct(&VarStructOp::NewOption {
                    env: env.clone(),
                    selector: name.clone(),
                    name: option.clone(),
                    description: None,
                    values,
                })
            });
        match result {
            Ok(()) => {
                self.project.set_selection_for(&env, &name, &option);
                self.varmanager.sync(&self.project);
                self.record_var_file_step(before);
                match source {
                    ExtractSource::FocusedField => self.replace_focused_field_with_token(&name),
                    ExtractSource::Selection(surface) => {
                        self.replace_selection_with_token(surface, &name);
                    }
                }
                self.toasts
                    .push(format!("extracted to {{{{{name}}}}}"), ToastKind::Success);
            }
            Err(msg) => {
                // The selector may already be declared by the time the
                // option write fails; the recorded step lets undo peel it.
                self.varmanager.sync(&self.project);
                self.record_var_file_step(before);
                self.toasts.push(msg, ToastKind::Error);
                self.last_action_failed = true;
            }
        }
        true
    }

    /// The shared tail of `Action::ConfirmExtractVariable` and
    /// `Action::ConfirmExtractSelection`: validates `name`, reads the value
    /// from `source`, writes it at `destination` (with the per-destination
    /// collision guards), then swaps the origin text for `{{name}}` —
    /// the whole field for `FocusedField`, just the selected range for
    /// `Selection` (nothing at all on the read-only response).
    fn confirm_extract(
        &mut self,
        name: String,
        destination: crate::action::ExtractDestination,
        source: ExtractSource,
    ) -> bool {
        if !postui_core::vars::is_valid_var_name(&name) {
            self.toasts.push(
                format!("\"{name}\" is not a valid variable name"),
                ToastKind::Error,
            );
            self.last_action_failed = true;
            return true;
        }
        let text = match source {
            ExtractSource::FocusedField => self.focused_field_text().map(|(t, _)| t.to_string()),
            ExtractSource::Selection(surface) => self.selection_text_of(surface),
        };
        let Some(text) = text else {
            let msg = match source {
                ExtractSource::FocusedField => "focus a text field first",
                ExtractSource::Selection(_) => "select some text first",
            };
            self.toasts.push(msg, ToastKind::Warning);
            return true;
        };
        let before = self.read_file_states(&self.project.var_file_paths());
        use crate::action::ExtractDestination;
        let write_result: Result<(), String> = match destination {
            ExtractDestination::ProjectDefault => {
                if self.project.model.vars.contains_key(&name)
                    || self.project.model.selectors.contains_key(&name)
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
                // (a selector of this name would otherwise sit
                // alongside a same-named plain variable), plus the
                // one `validate_env` would reject outright if we
                // wrote a flat env value anyway: a secret variable
                // (env values for secrets are forbidden —
                // `ModelError::EnvValueForSecret`). Catching it
                // here — rather than letting `edit_env` fail after
                // the fact — keeps the refusal a clean toast
                // instead of a write attempt against a doc that
                // `validate_env` would then reject.
                if self.project.model.selectors.contains_key(&name) {
                    self.toasts
                        .push(format!("\"{name}\" already exists"), ToastKind::Error);
                    self.last_action_failed = true;
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
                        self.last_action_failed = true;
                        return true;
                    }
                } else if let Err(msg) = self
                    .project
                    .edit_variables(|doc| postui_core::varedit::upsert_var(doc, &name, None, None))
                {
                    self.toasts.push(msg, ToastKind::Error);
                    self.last_action_failed = true;
                    return true;
                }
                self.project.edit_env(&env, |doc| {
                    postui_core::varedit::set_env_value(doc, &name, Some(&text))
                })
            }
            ExtractDestination::Request => {
                // No structural-file hazard here — `[variables]`
                // options are a separate resolution layer (spec §2)
                // with no `validate_env`-style cross-checks — but an
                // existing option of the same name would otherwise be
                // silently clobbered, same as `ProjectDefault`'s
                // "already exists" refusal.
                if self.editor.variables.contains_key(&name) {
                    self.toasts.push(
                        format!("\"{name}\" already exists in this request's variables"),
                        ToastKind::Error,
                    );
                    self.last_action_failed = true;
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
                // The var-file half of the gesture (ProjectDefault/
                // ActiveEnv write variables.toml/an env file; a
                // Request destination touches neither, so this is a
                // no-op there). The editor-side half (the token
                // replacement, and a Request destination's
                // `[variables]` insert) is captured by the next
                // `capture_undo` as an EditorDelta — undo peels the
                // token-replacement, then the declaration.
                self.record_var_file_step(before);
                match source {
                    ExtractSource::FocusedField => self.replace_focused_field_with_token(&name),
                    ExtractSource::Selection(surface) => {
                        self.replace_selection_with_token(surface, &name);
                    }
                }
                // Finding 2, same ruling as promote: the
                // `Request` destination's write only exists so far
                // in the dirty editor buffer (both the new
                // `[variables]` option above and the field text
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
        self.commit_table_edit_with_enter();
    }

    /// Replaces `surface`'s live selection with `{{name}}`, keeping the
    /// rest of the text: the line inputs and the body all take it through
    /// their paste path (which replaces a selection); a table cell is then
    /// committed through the table's own `Enter` path, exactly like
    /// [`Self::replace_focused_field_with_token`]. The response is
    /// read-only — nothing to replace.
    fn replace_selection_with_token(&mut self, surface: crate::action::TextSurface, name: &str) {
        use crate::action::TextSurface;
        let token = format!("{{{{{name}}}}}");
        match surface {
            TextSurface::Url => self.editor.url.paste(&token),
            TextSurface::Body => {
                self.editor.paste_body(&token);
            }
            TextSurface::TableCell => {
                if let Some(edit) = self.editor.table.editing.as_mut() {
                    edit.input.paste(&token);
                    self.commit_table_edit_with_enter();
                }
            }
            TextSurface::Response => {}
            // Never offered on the Variable Manager's own surfaces, nor
            // the jq bar.
            TextSurface::VmField | TextSurface::VmCell | TextSurface::Jq => {}
        }
    }

    /// Commits the table cell under edit through the active tab's own
    /// `Enter` handling, so the new text lands in the map (not left as a
    /// pending edit) and rides the same dirty/save path as any row commit.
    fn commit_table_edit_with_enter(&mut self) {
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

    /// The value popup's remove — shared by the clicked "✕ remove"
    /// (`Hit::ModalRemove`) and its keyboard chord (`alt+d`): marks the
    /// chosen Write-to scope's stored value for removal on Confirm, then
    /// re-lands the popup on the next supplier with its value ready to
    /// edit. Inert (returns `false`) unless the top modal is the value
    /// popup and the chosen scope actually stores something — mirroring
    /// when the ✕ is painted at all.
    pub(crate) fn remove_from_value_popup(&mut self) -> bool {
        use crate::components::modal::{Modal, stage_value_removal};
        let Some(Modal::MultiPrompt { fields, kind, .. }) = self.modals.top_mut() else {
            return false;
        };
        // Staged, not written: the popup is a transaction, so the removal
        // waits for Confirm (and Cancel forgets it). The popup re-lands on
        // whichever scope would supply once the value is gone, with its
        // stored value ready to edit or remove in turn.
        stage_value_removal(fields, kind) && self.update(Action::Render)
    }

    /// Builds and opens the value-edit popup for a simple (or
    /// request-scoped / stray-env) variable's `{{token}}`: a `value` field
    /// seeded with the current effective value and a `destination` choice
    /// preselected to whichever scope supplies that value today — the
    /// request's `[variables]` overlay, the active environment's flat
    /// value, or the declaration default. Confirming dispatches
    /// `Action::ConfirmEditVarValue`, which writes the chosen scope.
    fn open_edit_value_popup(&mut self, name: &str) -> bool {
        use crate::components::modal::{Modal, PromptField, PromptKind};

        let request_value = self
            .editor
            .variables
            .get(name)
            .filter(|e| e.enabled)
            .map(|e| e.value.clone());
        let env_value = self.project.env_data.values.get(name).cloned();
        let has_env = self.project.active_env.is_some();

        let seed = request_value
            .clone()
            .or_else(|| self.project.resolved.values.get(name).cloned())
            .unwrap_or_default();

        let (choices, preselect) = crate::components::modal::value_popup_choices(
            request_value.is_some(),
            has_env,
            env_value.is_some(),
        );

        let mut destination = PromptField::choice("destination", "Write to", &choices);
        destination.input = LineInput::new(preselect);

        // What each destination currently stores (`None` = nothing), so
        // cycling the scope can reseed the value field, and the Remove
        // button knows whether there is anything to delete there.
        let default_value = self
            .project
            .model
            .vars
            .get(name)
            .and_then(|d| d.default.clone());
        let mut scope_values = vec![
            ("Project default".to_string(), default_value),
            ("This request".to_string(), request_value.clone()),
        ];
        if has_env {
            scope_values.push(("Active env value".to_string(), env_value.clone()));
        }

        self.push_modal(Modal::MultiPrompt {
            title: format!("{{{{{name}}}}}"),
            fields: vec![PromptField::text("value", "Value", &seed), destination],
            focus: 0,
            kind: PromptKind::EditVarValue {
                name: name.to_string(),
                scope_values,
                pending_removals: Vec::new(),
            },
        });
        true
    }

    /// Builds and opens the `SelectOption` picker (spec §6's first
    /// context) for `name`, a field of `selector`: rows are `selector`'s options
    /// in the active environment, the highlighted one's per-field values
    /// shown in the detail pane, with the current selection marked with a ✓.
    fn open_select_picker(&mut self, name: String, selector: String) -> bool {
        use crate::components::modal::Modal;
        use crate::components::var_picker::{SelectOption, VarPickerState};
        use postui_core::varmodel;

        let env_key = self.project.active_env.clone().unwrap_or_default();
        let selected_key = if self.selector_is_shared(&selector) {
            self.project.shared_selections().get(&selector).cloned()
        } else {
            self.project
                .selections_for(&env_key)
                .get(&selector)
                .cloned()
        };
        let options: Vec<SelectOption> =
            varmodel::options_of(&self.project.model, &self.project.env_data, &selector)
                .map(|options| {
                    options
                        .iter()
                        .map(|(key, decl)| SelectOption {
                            key: key.clone(),
                            description: decl.description.clone(),
                            value: None,
                            selected: selected_key.as_deref() == Some(key.as_str()),
                            values: Some(decl.values.clone()),
                        })
                        .collect()
                })
                .unwrap_or_default();
        // With no options yet the picker opens on its "add new option…"
        // ghost row alone — the user chooses to create, never gets walked
        // into a prompt.
        self.push_modal(Modal::VarPicker(VarPickerState::new_select(
            options, name, selector, env_key,
        )));
        true
    }

    /// Applies one committed Variable Manager op (spec §5), writing
    /// through to whichever file owns it. `Err(msg)` is always safe to
    /// toast (never a secret value); the caller (`Action::VarEdit`) toasts
    /// it and leaves the originating field untouched, so the typed text
    /// survives a retry.
    /// Commits whatever field the variable form's [`VarFormState::editing`]
    /// holds (click-away, click-another-field, or `Enter`): builds its
    /// `VarEditOp` and writes it through `apply_var_edit`. A no-op with
    /// nothing being edited. A write failure toasts and puts the edit back
    /// exactly as it was — the typed text is never lost to a failed write
    /// (spec §5's general write-failure rule), and a secret's value never
    /// appears in the toast.
    pub(crate) fn commit_var_form(&mut self) {
        let Some((field, input)) = self.varmanager.form.editing.take() else {
            return;
        };
        let VmDetail::Var(name) = self.varmanager.detail.clone() else {
            return;
        };
        let value = input.text().to_string();
        let op = var_edit_op_for(&self.project, &name, field, value);
        // This commit never routes through `self.apply` — it's called
        // directly from `handle_key` (click-away/Enter), so it needs its
        // own capture rather than relying on `Action::VarEdit`'s wrap.
        let before = self.read_file_states(&self.project.var_file_paths());
        match self.apply_var_edit(&op) {
            Ok(()) => self.record_var_file_step(before),
            Err(msg) => {
                self.varmanager.form.editing = Some((field, input));
                self.toasts.push(msg, ToastKind::Error);
            }
        }
    }

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
                    .selectors
                    .get(owner)
                    .map(|g| g.fields.clone())
                {
                    self.project.edit_variables(|doc| {
                        postui_core::varedit::upsert_selector(doc, owner, Some(value), &fields)
                    })
                } else {
                    Err(format!(
                        "\"{owner}\" is not a declared variable or selector"
                    ))
                }
            }
            VarEditOp::SetSecretValue { env, name, value } => {
                self.project.set_secret_for(env, name, value.clone())
            }
            VarEditOp::SetOptionValue {
                env,
                selector,
                option,
                field,
                value,
            } => {
                // An option's values live in one file — its selector's env
                // file, or variables.toml for a shared selector; the cell
                // being edited is one field of that option.
                let mut values = indexmap::IndexMap::new();
                values.insert(field.clone(), value.clone());
                self.edit_options_home(selector, env, |doc| {
                    postui_core::varedit::upsert_option(doc, selector, option, None, &values)
                })
            }
            VarEditOp::SetOptionDescription {
                env,
                selector,
                option,
                description,
            } => self.edit_options_home(selector, env, |doc| match description {
                Some(d) => postui_core::varedit::upsert_option(
                    doc,
                    selector,
                    option,
                    Some(d),
                    &indexmap::IndexMap::new(),
                ),
                None => postui_core::varedit::remove_option_description(doc, selector, option),
            }),
            VarEditOp::SetRequestVar { name, value } => {
                match self.editor.variables.get_mut(name) {
                    Some(option) => option.value = value.clone(),
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
            VarEditOp::SelectOption {
                env,
                selector,
                option,
            } => {
                self.project.set_selection_for(env, selector, option);
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
            // A selector (or a name that is not declared at all) has no
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
            self.push_modal(Modal::Confirm {
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
            self.push_modal(Modal::Confirm {
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

    /// Applies one confirmed Variable Manager structural mutation (spec
    /// §5's action list; §4's promote; §3's secret-flag
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
            VarStructOp::NewSelector {
                name,
                fields,
                shared,
            } => {
                if !is_valid_var_name(name) {
                    return Err(format!("\"{name}\" is not a valid selector name"));
                }
                if name_taken(&self.project, name) {
                    return Err(format!("\"{name}\" already exists"));
                }
                for f in fields {
                    if !is_valid_var_name(f) {
                        return Err(format!("\"{f}\" is not a valid field name"));
                    }
                }
                self.project.edit_variables(|doc| {
                    let out = varedit::upsert_selector(doc, name, None, fields)?;
                    if *shared {
                        varedit::set_selector_shared(&out, name, true)
                    } else {
                        Ok(out)
                    }
                })
            }
            VarStructOp::Rename { from, to } => {
                if !is_valid_var_name(to) {
                    return Err(format!("\"{to}\" is not a valid variable name"));
                }
                if name_taken(&self.project, to) {
                    return Err(format!("\"{to}\" already exists"));
                }
                if self.project.model.selectors.contains_key(from) {
                    return self.apply_rename_group(from, to);
                }
                self.project
                    .edit_variables(|doc| varedit::rename_var(doc, from, to))?;
                // `rename_var` only ever touches `variables.toml` — an
                // active env override for `from` would otherwise silently
                // degrade to the default post-rename (no error, no
                // warning, just a wrong-looking resolved value). Cascade
                // into every environment's flat pair and its
                // `[options.<from>]` table too; `rename_env_var` no-ops
                // for an environment with nothing to rename.
                for env in self.project.environments.clone() {
                    self.project
                        .edit_env(&env, |doc| varedit::rename_env_var(doc, from, to))?;
                }
                Ok(())
            }
            VarStructOp::Delete { name } => {
                let is_group = self.project.model.selectors.contains_key(name);
                if !is_group {
                    // Mirror `delete_var`'s own "still a selector field"
                    // conflict up front, using the already-loaded model —
                    // before any environment file is touched, so a refusal
                    // here leaves everything unchanged (`apply_var_struct`'s
                    // documented contract), matching what `delete_var`
                    // itself would have refused a moment later anyway.
                    if let Some(gname) = self
                        .project
                        .model
                        .selectors
                        .iter()
                        .find_map(|(gname, g)| g.fields.contains(name).then(|| gname.clone()))
                    {
                        return Err(format!(
                            "variable \"{name}\" is a field of selector \"{gname}\"; remove it from the selector first"
                        ));
                    }
                }
                // Finding 1: `delete_var`/`delete_selector` only ever touch
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
                // A shared selector's options live in variables.toml with
                // the declaration: both halves go in one write (an
                // `[options.<name>]` table without its declaration fails
                // validation in either order), and no env file holds
                // anything to strip.
                if is_group && self.selector_is_shared(name) {
                    self.project.edit_variables(|doc| {
                        let stripped = varedit::delete_selector_options(doc, name)?;
                        varedit::delete_selector(&stripped, name)
                    })?;
                    self.project.clear_selection_for("", name);
                    return Ok(());
                }
                for env in self.project.environments.clone() {
                    if is_group {
                        // The declaration's environment-side half: the whole
                        // `[options.<name>]` subtree, plus the recorded
                        // selection that named one of those options.
                        self.project
                            .edit_env(&env, |doc| varedit::delete_selector_options(doc, name))?;
                        self.project.clear_selection_for(&env, name);
                    } else {
                        self.project
                            .edit_env(&env, |doc| varedit::delete_env_var(doc, name))?;
                    }
                }
                if is_group {
                    self.project
                        .edit_variables(|doc| varedit::delete_selector(doc, name))
                } else {
                    self.project
                        .edit_variables(|doc| varedit::delete_var(doc, name))
                }
            }
            VarStructOp::ToggleSecret { name } => self.apply_toggle_secret(name),
            VarStructOp::SetFields { selector, fields } => {
                for f in fields {
                    if !is_valid_var_name(f) {
                        return Err(format!("\"{f}\" is not a valid field name"));
                    }
                }
                // A shared selector's options sit in the same file and
                // must supply exactly the declared fields, so the list
                // change carries them along in the one write (a non-shared
                // selector's env-side halves go through the fields editor's
                // `apply_group_fields` instead).
                let current: Vec<String> = self
                    .project
                    .model
                    .selectors
                    .get(selector)
                    .map(|g| g.fields.clone())
                    .unwrap_or_default();
                let shared = self.selector_is_shared(selector);
                self.project.edit_variables(|doc| {
                    let mut out = varedit::upsert_selector(doc, selector, None, fields)?;
                    if shared {
                        for field in fields.iter().filter(|f| !current.contains(f)) {
                            out = varedit::ensure_option_field(&out, selector, field)?;
                        }
                        for field in current.iter().filter(|f| !fields.contains(f)) {
                            out = varedit::strip_option_field(&out, selector, field)?;
                        }
                    }
                    Ok(out)
                })
            }
            VarStructOp::Promote { name, target } => self.apply_promote(name, *target),
            VarStructOp::NewOption {
                env,
                selector,
                name,
                description,
                values,
            } => self.edit_options_home(selector, env, |doc| {
                varedit::upsert_option(doc, selector, name, description.as_deref(), values)
            }),
            VarStructOp::RenameOption {
                env,
                selector,
                from,
                to,
            } => {
                self.edit_options_home(selector, env, |doc| {
                    varedit::rename_option(doc, selector, from, to)
                })?;
                // A selection names an option by key: carry it across the
                // rename rather than leaving a dangling one behind. (A
                // shared selector's selection is the global one;
                // `set_selection_for` routes there itself.)
                let selected = if self.selector_is_shared(selector) {
                    self.project.shared_selections().get(selector)
                } else {
                    self.project.selections_for(env).get(selector)
                };
                if selected.map(String::as_str) == Some(from) {
                    self.project.set_selection_for(env, selector, to);
                }
                Ok(())
            }
            VarStructOp::DeleteOption {
                env,
                selector,
                name,
            } => self.apply_delete_entry(env, selector, name),
            VarStructOp::DuplicateOption {
                env,
                selector,
                name,
            } => self.apply_duplicate_entry(env, selector, name),
        }
    }

    /// [`VarStructOp::Rename`] for a selector. Both halves of the declaration
    /// have to move at once: an environment's `[options.<old>]` table names
    /// a selector the renamed model no longer declares, and the new name has
    /// no options yet — so `validate_env` refuses whichever half lands
    /// first, in either order. `edit_variables_and_envs` builds and
    /// validates them together, then writes.
    ///
    /// Selections name a selector by key, so each environment's recorded
    /// selection is carried across the rename (the same repair
    /// [`VarStructOp::RenameOption`] makes for an option key) — otherwise a
    /// renamed selector would silently lose its "pick user 2" state
    /// everywhere.
    fn apply_rename_group(&mut self, from: &str, to: &str) -> Result<(), String> {
        use postui_core::varedit;
        // A shared selector renames wholly inside variables.toml — the
        // declaration and its `[options.<from>]` subtree in one write —
        // and carries its one global selection.
        if self.selector_is_shared(from) {
            self.project.edit_variables(|doc| {
                let renamed = varedit::rename_selector(doc, from, to)?;
                varedit::rename_selector_options(&renamed, from, to)
            })?;
            if let Some(key) = self.project.shared_selections().get(from).cloned() {
                self.project.clear_selection_for("", from);
                self.project.set_selection_for("", to, &key);
            }
            return Ok(());
        }
        self.project.edit_variables_and_envs(
            |doc| varedit::rename_selector(doc, from, to),
            |doc| varedit::rename_selector_options(doc, from, to),
        )?;
        for env in self.project.environments.clone() {
            if let Some(key) = self.project.selections_for(&env).get(from).cloned() {
                self.project.clear_selection_for(&env, from);
                self.project.set_selection_for(&env, to, &key);
            }
        }
        Ok(())
    }

    /// [`Action::ApplyGroupFields`]: turns the field editor's per-slot text
    /// into renames, additions and removals, and applies all three in one
    /// transaction across `variables.toml` and every environment.
    ///
    /// **Position is the identity.** Slot `i` *is* the selector's current
    /// `i`th field: changed text renames it, cleared text removes it, and a
    /// slot past the current list adds a field. Rows are therefore never
    /// reordered — swapping two names reads as two renames (and is refused
    /// as a collision), which is the price of being able to rename a field
    /// at all through a plain list of text boxes.
    ///
    /// Every one of the three needs both files at once: an option must
    /// supply exactly its selector's declared fields (`validate_env`), so a
    /// declaration whose new field list has landed alone is invalid until
    /// the options carry the same change.
    fn apply_group_fields(&mut self, selector: String, slots: Vec<String>) {
        use postui_core::varedit;
        use postui_core::vars::is_valid_var_name;

        let Some(current) = self
            .project
            .model
            .selectors
            .get(&selector)
            .map(|g| g.fields.clone())
        else {
            self.toasts.push(
                format!("\"{selector}\" is not a declared selector"),
                ToastKind::Error,
            );
            self.last_action_failed = true;
            return;
        };

        let mut renames: Vec<(String, String)> = Vec::new();
        let mut removals: Vec<String> = Vec::new();
        let mut additions: Vec<String> = Vec::new();
        let mut fields: Vec<String> = Vec::new();
        for (i, slot) in slots.iter().enumerate() {
            match current.get(i) {
                Some(old) if slot.is_empty() => removals.push(old.clone()),
                Some(old) => {
                    if slot != old {
                        renames.push((old.clone(), slot.clone()));
                    }
                    fields.push(slot.clone());
                }
                None if slot.is_empty() => {}
                None => {
                    additions.push(slot.clone());
                    fields.push(slot.clone());
                }
            }
        }
        // A prompt with fewer slots than the selector has fields drops the
        // trailing ones (today only reachable if the field list grew
        // between opening and confirming the modal).
        for old in current.iter().skip(slots.len()) {
            removals.push(old.clone());
        }
        if renames.is_empty() && removals.is_empty() && additions.is_empty() {
            return;
        }

        for name in additions.iter().chain(renames.iter().map(|(_, t)| t)) {
            if !is_valid_var_name(name) {
                self.toasts.push(
                    format!("\"{name}\" is not a valid field name"),
                    ToastKind::Error,
                );
                self.last_action_failed = true;
                return;
            }
            // A field belongs to exactly one selector, and shares the
            // declaration namespace with variables and selectors — a
            // {{token}} has to name one thing. The toast names the owner.
            let owner = self.project.model.selectors.iter().find_map(|(g, decl)| {
                if g == name {
                    Some(format!("\"{name}\" is already a selector"))
                } else if g != &selector && decl.fields.iter().any(|f| f == name) {
                    Some(format!("\"{name}\" already belongs to selector \"{g}\""))
                } else {
                    None
                }
            });
            if let Some(msg) = owner {
                self.toasts.push(msg, ToastKind::Error);
                self.last_action_failed = true;
                return;
            }
        }
        let mut seen = std::collections::HashSet::new();
        if let Some(dup) = fields.iter().find(|f| !seen.insert((*f).clone())) {
            self.toasts
                .push(format!("\"{dup}\" is listed twice"), ToastKind::Error);
            self.last_action_failed = true;
            return;
        }

        // A renamed field that has its own `[name]` declaration renames
        // through `rename_var` (which also rewrites the selector's `fields`
        // array); one that doesn't exist as a declaration only lives in
        // that array, which the closing `upsert_selector` rewrites wholesale
        // either way.
        let declared: Vec<bool> = renames
            .iter()
            .map(|(from, _)| self.project.model.vars.contains_key(from))
            .collect();
        let declaration_half = |doc: &str| {
            let mut out = doc.to_string();
            for ((from, to), declared) in renames.iter().zip(&declared) {
                if *declared {
                    out = varedit::rename_var(&out, from, to)?;
                }
            }
            varedit::upsert_selector(&out, &selector, None, &fields)
        };
        let options_half = |doc: &str| {
            let mut out = doc.to_string();
            for (from, to) in &renames {
                out = varedit::rename_option_field(&out, &selector, from, to)?;
            }
            for field in &removals {
                out = varedit::strip_option_field(&out, &selector, field)?;
            }
            for field in &additions {
                out = varedit::ensure_option_field(&out, &selector, field)?;
            }
            Ok(out)
        };
        // A shared selector's options sit beside the declaration in
        // variables.toml, so the reshape is one write to one file.
        let result = if self.selector_is_shared(&selector) {
            self.project
                .edit_variables(|doc| options_half(&declaration_half(doc)?))
        } else {
            self.project
                .edit_variables_and_envs(declaration_half, options_half)
        };
        match result {
            Ok(()) => {
                self.varmanager.sync(&self.project);
                // Removals delete that column's values from every option —
                // no confirm gate (the write is one undo step), so the
                // toast says what happened and the way back.
                if !removals.is_empty() {
                    self.toasts.push(
                        format!(
                            "Removed {} from {selector}{}",
                            removals.join(", "),
                            self.undo_hint()
                        ),
                        ToastKind::Info,
                    );
                }
            }
            Err(msg) => self.toasts.push(msg, ToastKind::Error),
        }
    }

    /// Commits whatever cell the selector grid's [`OptionGridState::editing`]
    /// holds (click-away, click-another-cell, or `Enter`) — Task 8's rules,
    /// on the three kinds of cell the grid has: the ghost row's name cell
    /// creates an option (with an empty value for every field, so the new
    /// record validates) and continues into its first field cell, a real
    /// option's name cell renames it, and a field cell writes that one
    /// value. Text that didn't change writes nothing.
    ///
    /// A write failure toasts and puts the edit back exactly as it was, so
    /// the typed text survives a retry (spec §5) — including the reserved
    /// option name `description`, which core refuses.
    pub(crate) fn commit_grid_edit(&mut self) {
        let Some(edit) = self.varmanager.grid.editing.take() else {
            return;
        };
        let VmDetail::Group(selector) = self.varmanager.detail.clone() else {
            return;
        };
        // A shared selector's grid works without an environment (its ops
        // ignore the env they carry); everyone else's needs one.
        let env = match self.project.active_env.clone() {
            Some(env) => env,
            None if self.selector_is_shared(&selector) => String::new(),
            None => return,
        };
        let value = edit.input.text().to_string();
        if value == edit.original {
            return;
        }
        let options: Vec<String> = postui_core::varmodel::options_of(
            &self.project.model,
            &self.project.env_data,
            &selector,
        )
        .map(|e| e.keys().cloned().collect())
        .unwrap_or_default();
        let fields = self
            .project
            .model
            .selectors
            .get(&selector)
            .map(|g| g.fields.clone())
            .unwrap_or_default();
        let ghost = edit.row >= options.len();

        // Same as `commit_var_form`: called directly from `handle_key`,
        // never through `self.apply`, so it needs its own capture.
        let before = self.read_file_states(&self.project.var_file_paths());
        let result = if ghost {
            // Only the ghost's name cell creates anything; an emptied name
            // creates nothing (and neither does a value typed into a row
            // that doesn't exist yet — the caller never starts one).
            if edit.col != 0 || value.is_empty() {
                return;
            }
            let values = fields
                .iter()
                .map(|f| (f.clone(), String::new()))
                .collect::<indexmap::IndexMap<_, _>>();
            self.apply_var_struct(&VarStructOp::NewOption {
                env,
                selector,
                name: value.clone(),
                description: None,
                values,
            })
        } else if edit.col == 0 {
            // Clearing a name is not a delete: leave the option as it was.
            if value.is_empty() {
                return;
            }
            self.apply_var_struct(&VarStructOp::RenameOption {
                env,
                selector,
                from: options[edit.row].clone(),
                to: value,
            })
        } else if edit.col == fields.len() + 1 {
            // The trailing description column: an emptied cell removes the
            // stored key (clearing a text value, not an entity remove).
            self.apply_var_edit(&VarEditOp::SetOptionDescription {
                env,
                selector,
                option: options[edit.row].clone(),
                description: (!value.is_empty()).then(|| value.clone()),
            })
        } else {
            let Some(field) = fields.get(edit.col - 1).cloned() else {
                return;
            };
            self.apply_var_edit(&VarEditOp::SetOptionValue {
                env,
                selector,
                option: options[edit.row].clone(),
                field,
                value,
            })
        };
        match result {
            Ok(()) => {
                self.varmanager.sync(&self.project);
                self.record_var_file_step(before);
                // The ghost flow keeps going left-to-right: the row that
                // was the ghost is now a real option (appended, so it keeps
                // its index) with its first field cell live.
                if ghost && !fields.is_empty() {
                    self.varmanager.start_cell_edit(&self.project, edit.row, 1);
                }
            }
            Err(msg) => {
                self.varmanager.grid.editing = Some(edit);
                self.toasts.push(msg, ToastKind::Error);
            }
        }
    }

    /// [`VarStructOp::DuplicateOption`]: copies one option's description and
    /// values to a fresh name in the same environment — `"<name> copy"`,
    /// then `"<name> copy-2"`, … while that is taken. Nothing else moves:
    /// the copy is unselected, and no other environment is touched.
    fn apply_duplicate_entry(
        &mut self,
        env: &str,
        selector: &str,
        name: &str,
    ) -> Result<(), String> {
        let options = self
            .options_of_for(env, selector)
            .ok_or_else(|| format!("selector \"{selector}\" has no options in {env}"))?;
        let source = options
            .get(name)
            .ok_or_else(|| format!("no option \"{name}\" in {selector}"))?
            .clone();
        let mut copy = format!("{name} copy");
        let mut n = 2;
        while options.contains_key(&copy) {
            copy = format!("{name} copy-{n}");
            n += 1;
        }
        self.edit_options_home(selector, env, |doc| {
            postui_core::varedit::upsert_option(
                doc,
                selector,
                &copy,
                source.description.as_deref(),
                &source.values,
            )
        })
    }

    /// [`Action::DuplicateVar`]: copies a declaration under `<name>-copy`
    /// (then `-copy-2`, …). A variable keeps its description, its default
    /// and its secret flag; a selector copies its field list only — options
    /// live in an environment, not in the declaration, and are left alone.
    fn apply_duplicate_var(&mut self, name: &str) -> Result<(), String> {
        use postui_core::varedit;
        let mut copy = format!("{name}-copy");
        let mut n = 2;
        while self.project.model.vars.contains_key(&copy)
            || self.project.model.selectors.contains_key(&copy)
        {
            copy = format!("{name}-copy-{n}");
            n += 1;
        }
        if let Some(selector) = self.project.model.selectors.get(name) {
            let (fields, description) = (selector.fields.clone(), selector.description.clone());
            return self.project.edit_variables(|doc| {
                varedit::upsert_selector(doc, &copy, description.as_deref(), &fields)
            });
        }
        let decl = self
            .project
            .model
            .vars
            .get(name)
            .ok_or_else(|| format!("no variable \"{name}\""))?;
        let (description, default, secret) =
            (decl.description.clone(), decl.default.clone(), decl.secret);
        self.project.edit_variables(|doc| {
            varedit::upsert_var(doc, &copy, description.as_deref(), default.as_deref())
        })?;
        if secret {
            // Safe on a just-created declaration: it has no value in any
            // environment for the flag flip to have to move.
            self.project
                .edit_variables(|doc| varedit::set_secret_flag(doc, &copy, true))?;
        }
        Ok(())
    }

    /// Whether `selector` is a shared selector — its options (and its one
    /// global selection) live in `variables.toml`, not per environment.
    fn selector_is_shared(&self, selector: &str) -> bool {
        self.project
            .model
            .selectors
            .get(selector)
            .is_some_and(|d| d.shared)
    }

    /// Applies an option-table edit to wherever `selector`'s options live:
    /// `variables.toml` for a shared selector (`env` is ignored — the same
    /// `[options.*]` verbs apply, just in the model's own file), otherwise
    /// `environments/<env>.toml`.
    fn edit_options_home(
        &mut self,
        selector: &str,
        env: &str,
        f: impl FnOnce(&str) -> Result<String, postui_core::varedit::EditError>,
    ) -> Result<(), String> {
        if self.selector_is_shared(selector) {
            self.project.edit_variables(f)
        } else {
            self.project.edit_env(env, f)
        }
    }

    /// `selector`'s options as they currently stand, from wherever they
    /// live: the model's own for a shared selector, `env`'s otherwise.
    fn options_of_for(
        &self,
        env: &str,
        selector: &str,
    ) -> Option<indexmap::IndexMap<String, postui_core::varmodel::OptionDecl>> {
        if self.selector_is_shared(selector) {
            self.project.model.options.get(selector).cloned()
        } else {
            postui_core::varmodel::selector_options(&self.env_data_for(env), selector).cloned()
        }
    }

    /// `env`'s data: the active environment's is already loaded on `ctx`;
    /// any other is read fresh, degrading to empty rather than erroring.
    fn env_data_for(&self, env: &str) -> postui_core::varmodel::EnvData {
        if self.project.active_env.as_deref() == Some(env) {
            self.project.env_data.clone()
        } else {
            postui_core::project::load_environment(&self.project.root, env).unwrap_or_default()
        }
    }

    /// [`VarStructOp::DeleteOption`]: deletes one option of `selector` from
    /// `env` (options belong to one environment each — spec §3.1). An option
    /// that is already gone is a quiet no-op success (a stale row — nothing
    /// left to do). Also clears any per-env selection naming the deleted
    /// option, in every environment, so local state doesn't accumulate dead
    /// selections (`resolve_env` already degrades a stale selection
    /// harmlessly, but there's no reason to leave it).
    fn apply_delete_entry(&mut self, env: &str, selector: &str, name: &str) -> Result<(), String> {
        let present = self
            .options_of_for(env, selector)
            .is_some_and(|options| options.contains_key(name));
        if present {
            self.edit_options_home(selector, env, |doc| {
                postui_core::varedit::delete_option(doc, selector, name)
            })?;
        }
        if self.selector_is_shared(selector) {
            if self
                .project
                .shared_selections()
                .get(selector)
                .map(String::as_str)
                == Some(name)
            {
                self.project.clear_selection_for(env, selector);
            }
            return Ok(());
        }
        for other in self.project.environments.clone() {
            if self
                .project
                .selections_for(&other)
                .get(selector)
                .map(String::as_str)
                == Some(name)
            {
                self.project.clear_selection_for(&other, selector);
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
    /// `[variables]` option into the project (default or the active
    /// environment), then removes it from the request now that the
    /// project owns it.
    fn apply_promote(
        &mut self,
        name: &str,
        target: postui_core::varedit::PromoteTarget,
    ) -> Result<(), String> {
        let option = self
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
            &option.value,
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
        // atomically). The compensating half — removing the option from the
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

    /// Synchronously persists the currently open request to disk, mirroring
    /// `Action::SaveRequest`'s slugged branch (no SaveAs prompt — every
    /// caller here already knows a slug is open). Used by ops (promote,
    /// extract-to-request) whose spec-mandated "writes
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
        self.mark_saved_after_write();
        self.refresh_sidebar();
        Ok(())
    }

    /// Marks the editor saved (resetting the baseline `is_dirty` compares
    /// against) and splits any in-flight typing burst so one undo lands
    /// exactly on the just-saved snapshot. Saving records no undo step:
    /// the baseline then always matches disk, keeping the dirty flag an
    /// honest "buffer differs from disk", and the redo stack survives (an
    /// undone edit stays redoable across a save) — matching how desktop
    /// editors treat save. Shared by `Action::SaveRequest`'s slugged
    /// branch and `save_open_request`.
    fn mark_saved_after_write(&mut self) {
        self.editor.mark_saved();
        self.history.break_coalescing();
    }

    /// Reads each path's current contents; an unreadable path (gone, or a
    /// permission error) reads as absent — these are small TOML files
    /// postui itself wrote, so "can't read it" and "it isn't there" are
    /// treated alike.
    fn read_file_states(&self, paths: &[PathBuf]) -> Vec<(PathBuf, Option<String>)> {
        paths
            .iter()
            .map(|p| (p.clone(), std::fs::read_to_string(p).ok()))
            .collect()
    }

    /// Reads `after_paths`' current contents, drops any pair whose content
    /// matches the corresponding `before` option (position-paired — callers
    /// pass both in the same path order), and — when anything real
    /// remains — records a `FileStates` undo step for the rest.
    /// `record_no_coalesce`: a disk write is never a burst-coalescing
    /// candidate and must clear the redo stack (spec: new steps invalidate
    /// stale redo options).
    fn record_file_step(
        &mut self,
        before: Vec<(PathBuf, Option<String>)>,
        after_paths: &[PathBuf],
        active_env: Option<(Option<String>, Option<String>)>,
    ) {
        let after = self.read_file_states(after_paths);
        debug_assert_eq!(before.len(), after.len(), "before/after paths must line up");
        let mut kept_before = Vec::new();
        let mut kept_after = Vec::new();
        for (b, a) in before.into_iter().zip(after) {
            if b.1 != a.1 {
                kept_before.push(b);
                kept_after.push(a);
            }
        }
        if kept_before.is_empty() {
            return;
        }
        self.history.record_no_coalesce(crate::undo::Step {
            kind: crate::undo::StepKind::FileStates {
                before: kept_before,
                after: kept_after,
                active_env,
            },
            context: crate::undo::Context {
                slug: self.editor.slug.clone(),
                cursor_before: crate::undo::CursorPos::None,
                cursor_after: crate::undo::CursorPos::None,
            },
        });
    }

    /// Writes each `(path, content)`: `Some` writes atomically, `None`
    /// removes (a missing file counts as removed). Stops at the first
    /// failure with a toast-ready message; earlier writes stand.
    fn write_file_states(
        &mut self,
        target: &[(PathBuf, Option<String>)],
        verb: &str,
    ) -> Result<(), String> {
        for (path, content) in target {
            let result: std::io::Result<()> = match content {
                Some(text) => crate::project_ctx::atomic_write(path, text),
                None => std::fs::remove_file(path).or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                }),
            };
            if let Err(e) = result {
                return Err(format!("{verb} failed at {}: {e}", path.display()));
            }
        }
        Ok(())
    }

    /// Records a `Trashed` step: reads `after_paths`' current contents as
    /// the companion files' "after" side. Never coalesces; clears redo.
    fn record_trashed_step(
        &mut self,
        items: Vec<postui_core::trash::Trashed>,
        files_before: Vec<(PathBuf, Option<String>)>,
        after_paths: &[PathBuf],
        active_env: Option<(Option<String>, Option<String>)>,
    ) {
        let files_after = self.read_file_states(after_paths);
        self.history.record_no_coalesce(crate::undo::Step {
            kind: crate::undo::StepKind::Trashed {
                items,
                files_before,
                files_after,
                active_env,
            },
            context: crate::undo::Context {
                slug: self.editor.slug.clone(),
                cursor_before: crate::undo::CursorPos::None,
                cursor_after: crate::undo::CursorPos::None,
            },
        });
    }

    /// Shared tail of every arm that changes files under the app — the
    /// file-level undo/redo arms and the forward-path ops that write
    /// through the trash (`ForceDeleteSpace`). Files changed underneath
    /// the app, so reload wholesale, and drop the editor if its file is
    /// gone. When the editor followed a rename into another space, the
    /// sidebar follows it there too (it is rooted at the active space).
    fn reload_after_file_change(&mut self) {
        self.project.reload_selections_from_disk();
        self.project.invalidate_stamps();
        self.apply(Action::ReloadProjectFiles);
        if let Some(w) = self.project.reload_spaces() {
            self.toasts.push(w, ToastKind::Warning);
        }
        if let Some(space) = self
            .editor
            .slug
            .as_deref()
            .and_then(postui_core::storage::space_of)
            .filter(|s| *s != self.project.active_space)
            .map(str::to_string)
        {
            // The editor has already followed its file into `space`, so
            // it says nothing about the space being left — recording it
            // would erase that space's remembered request.
            self.enter_space(&space, SpaceExit::Keep);
        }
        self.refresh_sidebar();
        if self.screen == Screen::Manage {
            self.varmanager.sync(&self.project);
        }
        if let Some(open) = self.editor.slug.clone()
            && !postui_core::storage::request_exists(&self.project.root, &open)
        {
            self.editor = Editor::default();
            self.shadow = None;
            self.sidebar.open_slug = None;
        }
    }

    /// The var-manager arms' capture helper: `before` is a
    /// `read_file_states(&self.project.var_file_paths())` snapshot taken
    /// before the op ran. `var_file_paths` is re-listed from disk, so an op
    /// that creates or deletes an environment file changes the path set
    /// between `before` and now — `record_file_step` position-pairs
    /// before/after, so this extends `before` with a `None` option for any
    /// current path it doesn't already cover (i.e. the union of the
    /// before- and after-side path sets) before handing both to
    /// `record_file_step`.
    fn record_var_file_step(&mut self, mut before: Vec<(PathBuf, Option<String>)>) {
        for path in self.project.var_file_paths() {
            if !before.iter().any(|(p, _)| *p == path) {
                before.push((path, None));
            }
        }
        let all_paths: Vec<PathBuf> = before.iter().map(|(p, _)| p.clone()).collect();
        self.record_file_step(before, &all_paths, None);
    }

    /// Makes `space` the active one without opening anything: records the
    /// outgoing space's open request (see [`SpaceExit`]), roots the
    /// sidebar, toasts. `false` (with a toast) for an unknown space.
    fn enter_space(&mut self, space: &str, outgoing: SpaceExit<'_>) -> bool {
        // A space switch can land with the mouse button still held (ctrl+1..9,
        // alt+z): cancel any live row drag against the space it started in
        // before the root changes, or its working order would be painted over
        // the new space's rows and written to the new space on release.
        self.finish_sidebar_drag(false);
        // Same for a Manage screen space drag: the list it is rearranging
        // is about to be re-read under it.
        self.finish_manage_drag(false);
        if let SpaceExit::Remember(slug) = outgoing {
            self.project.record_space_open(slug);
        }
        if !self.project.set_active_space(space) {
            self.toasts
                .push(format!("no space named {space:?}"), ToastKind::Warning);
            return false;
        }
        self.sidebar.selected = None;
        self.refresh_sidebar();
        self.toasts.push(
            format!("space: {}", self.project.space_name(space)),
            ToastKind::Success,
        );
        true
    }

    /// Applies an order-list cascade after a request file op succeeded.
    /// The file is the truth; a failed cascade only leaves a stale entry
    /// (ignored for display), so it warns rather than failing the op.
    fn order_cascade(&mut self, what: &str, r: Result<(), postui_core::project::ProjectError>) {
        if let Err(e) = r {
            self.toasts.push(
                format!("could not update request order after {what}: {e}"),
                ToastKind::Warning,
            );
        }
        self.project.reload_meta();
    }

    /// Splits a slug into its space and the path relative to that space,
    /// for the order-list cascades below.
    fn split_rel(slug: &str) -> Option<(&str, &str)> {
        let space = postui_core::storage::space_of(slug)?;
        let rel = postui_core::order::relative(slug, space)?;
        Some((space, rel))
    }

    /// Re-reads the project directory and rebuilds the sidebar tree,
    /// merging any ancestor folders `select_slug` needs opened into
    /// `project.expanded` first. Replaces every previous
    /// `list_requests` + `sidebar.refresh` pair so the tree/expansion
    /// state stays consistent at every call site.
    fn refresh_sidebar(&mut self) {
        let (listing, warning) = postui_core::storage::list_requests(&self.project.root);
        if let Some(warning) = warning {
            // Two different failures share one warning string: a walk error
            // (transient, worth an error toast every time) and the
            // loose-file lines (chronic by design — never migrated — so
            // they get a warning toast, and only when the set changes).
            let (loose, walk): (Vec<&str>, Vec<&str>) = warning.split("; ").partition(|line| {
                line.contains(" is not in a space ") || line.contains(" is not in a valid space ")
            });
            if !walk.is_empty() {
                self.toasts.push(
                    format!("could not fully list requests: {}", walk.join("; ")),
                    ToastKind::Error,
                );
            }
            let loose = (!loose.is_empty()).then(|| loose.join("; "));
            if loose.is_some() && loose != self.last_loose_warning {
                self.toasts.push(loose.clone().unwrap(), ToastKind::Warning);
            }
            self.last_loose_warning = loose;
        } else {
            self.last_loose_warning = None;
        }
        // An invalid name hand-written into `project.toml`'s `spaces` is
        // the same shape of problem: chronic (never rewritten for the
        // user — see `project::write_list`), so it warns once per change
        // through its own channel rather than on every refresh.
        let spaces_warning = {
            let (_, warnings) = postui_core::project::list_spaces_with_warnings(
                &self.project.root,
                &self.project.meta,
            );
            (!warnings.is_empty()).then(|| warnings.join("; "))
        };
        if spaces_warning.is_some() && spaces_warning != self.last_spaces_warning {
            self.toasts
                .push(spaces_warning.clone().unwrap(), ToastKind::Warning);
        }
        self.last_spaces_warning = spaces_warning;
        self.project
            .expanded
            .append(&mut self.sidebar.pending_expand);
        let expanded = self.project.expanded.clone();
        let space = self.project.active_space.clone();
        let order = postui_core::order::space_order(&self.project.meta, &space).to_vec();
        self.sidebar.refresh(listing, &space, &expanded, &order);
        // `refresh` can re-map the open request's row to a different index
        // (rows added/removed/reordered above it) without the open request
        // itself changing -- snap `ListTravel`'s value to match, or it
        // keeps easing toward the old index and paints a ghost selection
        // band there alongside the real one.
        self.snap_sidebar_travel();
    }

    /// Snaps `AnimKey::ListTravel(Sidebar)` to the open row's current
    /// index, if any request is open. Shared by `refresh_sidebar` and
    /// `rebuild_sidebar`: both re-index rows in ways that can move the
    /// open request to a different row without the open request itself
    /// changing, and without this the anim keeps easing toward wherever
    /// it used to be and `draw`'s crossfade paints a ghost band there.
    fn snap_sidebar_travel(&mut self) {
        if let Some(i) = self.sidebar.open_row() {
            self.anims
                .snap(AnimKey::ListTravel(ListId::Sidebar), i as f32);
        }
    }

    /// Rebuilds the sidebar rows from the listing it already holds — the
    /// per-motion path during a row drag, which must not re-read disk.
    ///
    /// The band tracks the open request, and the dragged row *is* the
    /// open request (a press opens it before a drag can start), so this
    /// snaps `ListTravel` exactly as `refresh_sidebar` does: without it
    /// the anim keeps easing toward the pressed row's index while
    /// `rebuild` moves that row from slot to slot, and the `draw`
    /// crossfade paints a partial band on both the stale target and the
    /// row the open request used to be on — a ghost that lingers for the
    /// rest of the drag.
    fn rebuild_sidebar(&mut self) {
        let expanded = self.project.expanded.clone();
        let space = self.project.active_space.clone();
        let order = postui_core::order::space_order(&self.project.meta, &space).to_vec();
        self.sidebar.rebuild(&space, &expanded, &order);
        self.snap_sidebar_travel();
    }

    /// Pointer motion during a row drag: maps the pointer's screen row to
    /// a row index (pinned to the dragged row's sibling group) and shows
    /// the resulting order live.
    ///
    /// A pointer *outside* the sidebar pane previews the cancel a release
    /// there would be: the working order snaps back to the order the drag
    /// started from, so the rows always show what letting go right now
    /// would leave behind. The test is the one `Up(Left)` commits on. The
    /// list's own edge rows are inside the pane, so edge auto-scroll is
    /// untouched. Motion back onto a row maps it again as usual.
    pub fn sidebar_drag_to(&mut self, x: u16, y: u16) -> bool {
        if !self.sidebar_drag_inside(x, y) {
            if self.sidebar.drag_reset() {
                self.rebuild_sidebar();
                return true;
            }
            return false;
        }
        let i = self.sidebar.row_at_y(y);
        if self.sidebar.drag_to_row(i) {
            self.rebuild_sidebar();
            return true;
        }
        false
    }

    /// Whether `(x, y)` is over the sidebar pane: where a row drag may be
    /// dropped. Commit-on-release and the drag's outside preview share it
    /// — the twin of `manage_drag_inside` — so a drop and the rows it
    /// previewed can never disagree.
    pub fn sidebar_drag_inside(&self, x: u16, y: u16) -> bool {
        self.hits.pane_at(x, y) == Some(PaneId::Sidebar)
    }

    /// Ends a row drag. `commit` writes the working order when it differs
    /// from the original; otherwise (release outside, Escape) the rows
    /// snap back. Either way the sidebar is rebuilt from disk truth and
    /// the armed press is disarmed — Escape ends the drag with the button
    /// still held, and a press left armed would let the next motion event
    /// promote straight back into a drag the user just cancelled.
    pub fn finish_sidebar_drag(&mut self, commit: bool) -> bool {
        self.sidebar_press = None;
        let Some(drag) = self.sidebar.drag.take() else {
            return false;
        };
        // A drag belongs to the space it started in. If the active space
        // moved on underneath it, there is nothing sane to write — the
        // working order names the *old* space's siblings — so this is a
        // cancel however the drag ended.
        let commit = commit && drag.space == self.project.active_space;
        if commit && drag.working != drag.original {
            let space = drag.space.clone();
            match postui_core::order::set_level_order(
                &self.project.root,
                &space,
                &drag.level,
                &drag.working,
            ) {
                Ok(()) => self.project.reload_meta(),
                Err(e) => self
                    .toasts
                    .push(format!("cannot reorder: {e}"), ToastKind::Warning),
            }
        }
        self.refresh_sidebar();
        if drag.space == self.project.active_space {
            self.sidebar.select_slug(&drag.slug);
        }
        true
    }

    /// Pointer motion during a Manage screen space-row drag: maps the
    /// pointer's screen row to a row index and shows the resulting order
    /// live.
    ///
    /// Outside the list the working order snaps back to the order the
    /// drag started from — the preview of the cancel a release there
    /// would be — using the same containment test `finish_manage_drag`
    /// commits on. See `sidebar_drag_to`, which does the same.
    pub fn manage_drag_to(&mut self, x: u16, y: u16) -> bool {
        if !self.manage_drag_inside(x, y) {
            return self.manage.list.drag_reset();
        }
        let i = self.manage.list.row_at_y(y);
        self.manage.list.drag_to_row(i)
    }

    /// Whether `(x, y)` is over the Manage screen's row list: the list
    /// rect the last draw recorded (the rect containment `pane_at` would
    /// do, which the list has no `Hit::Pane` for), plus the row hits
    /// themselves. Commit-on-release and the drag's outside preview share
    /// it, so a drop and the rows it previewed can never disagree.
    pub fn manage_drag_inside(&self, x: u16, y: u16) -> bool {
        let pos = ratatui::layout::Position { x, y };
        self.manage.list.list_rect().contains(pos)
            || matches!(self.hits.hit_at(x, y), Some(Hit::ManageRow(_)))
    }

    /// Ends a space-row drag. `commit` writes the working order when it
    /// differs from the original; otherwise (release outside, Escape, a
    /// right click, a tab switch) the rows snap back to disk truth. The
    /// armed press is disarmed either way — Escape ends the drag with the
    /// button still held, and a press left armed would let the next
    /// motion event promote straight back into the drag just cancelled.
    /// Like `Action::MoveSpace`, this is not an undo step.
    pub fn finish_manage_drag(&mut self, commit: bool) -> bool {
        self.manage_press = None;
        let Some(drag) = self.manage.list.drag.take() else {
            return false;
        };
        if commit && drag.working != drag.original {
            match postui_core::project::set_space_order(&self.project.root, &drag.working) {
                Ok(()) => {
                    // `ReloadProjectFiles` is mtime-gated (see
                    // `Action::MoveSpace`), so read the file just written
                    // rather than waiting for the stamp to move. Skipping
                    // it entirely (unlike `MoveSpace`, which runs it
                    // first) is deliberate: these two reloads cover
                    // everything a `spaces`-key rewrite can change.
                    self.project.reload_meta();
                    self.project.reload_spaces();
                }
                Err(e) => self
                    .toasts
                    .push(format!("cannot reorder: {e}"), ToastKind::Warning),
            }
        }
        // The list cursor follows the space that was dragged, wherever it
        // ended up — committed or snapped back.
        let tab = self.manage.tab;
        self.manage.list.select_name(tab, &self.project, &drag.name);
        true
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
        self.push_modal(Modal::Dropdown(DropdownState {
            anchor: ratatui::layout::Rect::new(x, y, 1, 1),
            items,
            selected,
            // Context menus are lists of commands, not of values, so no row
            // is "the current one" and nothing gets the ✓ marker.
            current: None,
        }));
        self.begin_dropdown_open();
        true
    }

    /// The right-click menu for a *text* surface under the pointer — Copy
    /// (greyed without a selection there) and, on editable surfaces,
    /// Paste — or `None` when `hit` isn't one: a table/grid cell that
    /// isn't the one under edit, a form field not being edited, a row
    /// background, chrome. The split rule for tables (see the right-click
    /// arm of `App::handle_mouse`): only the cell currently under edit
    /// offers this menu; every other part of a row keeps the row menu.
    ///
    /// Also claims focus for the surface (URL bar, body, the editor pane)
    /// the way a left click there would, minus the caret move, so the
    /// menu's Paste — routed by `App::paste_text` through focus — lands
    /// where the pointer was. The response pane is read-only: Copy only.
    /// Must run *before* the click-away commits, since a commit ends the
    /// edit whose selection Copy would read.
    fn text_surface_menu(&mut self, hit: &Hit) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::action::TextSurface;
        use crate::components::modal::MenuItem;
        let mut jq_row = None;
        let (surface, editable) = match hit {
            Hit::UrlBar => {
                self.update(Action::FocusUrl);
                (TextSurface::Url, true)
            }
            Hit::BodyEditor => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                (TextSurface::Body, true)
            }
            // Only once there is response text to select: an empty pane
            // (nothing sent yet, in flight, an error) offers no menu.
            Hit::Pane(PaneId::Response) if self.session.response.view().is_some() => {
                (TextSurface::Response, false)
            }
            Hit::JsonRow(row) | Hit::JsonArrow(row) if self.session.response.view().is_some() => {
                jq_row = Some(*row);
                (TextSurface::Response, false)
            }
            Hit::ResponseJqBar => {
                self.update(Action::FocusPane(PaneId::Response));
                if !self.session.response.set_jq_focus(true) {
                    return None;
                }
                (TextSurface::Jq, true)
            }
            Hit::TableCell { row, col } => {
                let cell_col = crate::components::table_editor::Col::from_index(*col);
                let editing_this = self
                    .editor
                    .table
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == *row && e.col == cell_col);
                if !editing_this {
                    return None;
                }
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                (TextSurface::TableCell, true)
            }
            Hit::VmFormField(field) => {
                let editing_this = self
                    .varmanager
                    .form
                    .editing
                    .as_ref()
                    .is_some_and(|(f, _)| f == field);
                if !editing_this {
                    return None;
                }
                (TextSurface::VmField, true)
            }
            Hit::VmEntryCell { row, col } => {
                let editing_this = self
                    .varmanager
                    .grid
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == *row && e.col == *col);
                if !editing_this {
                    return None;
                }
                (TextSurface::VmCell, true)
            }
            _ => return None,
        };
        let has_selection = self.selection_text_of(surface).is_some();
        let copy = if has_selection {
            MenuItem::new("Copy", Action::CopySelection(surface))
        } else {
            MenuItem::disabled("Copy")
        };
        let mut items = vec![copy];
        if editable {
            items.push(MenuItem::new("Paste", Action::Paste));
        }
        // Extracting a selection to a variable — offered on the request's
        // own text and on the response (where it only creates the
        // variable, there being nothing to rewrite); never on the Variable
        // Manager's surfaces, which already *are* variables, nor on the jq
        // bar, whose text is a filter rather than a value.
        if !matches!(
            surface,
            TextSurface::VmField | TextSurface::VmCell | TextSurface::Jq
        ) {
            items.push(if has_selection {
                MenuItem::new(
                    "Extract to variable\u{2026}",
                    Action::ExtractSelection(surface),
                )
            } else {
                MenuItem::disabled("Extract to variable\u{2026}")
            });
            items.push(if has_selection {
                MenuItem::new(
                    "Extract to selector\u{2026}",
                    Action::ExtractSelectionToSelector(surface),
                )
            } else {
                MenuItem::disabled("Extract to selector\u{2026}")
            });
        }
        if let Some(row) = jq_row {
            let structural = self.jq_menu_items(row);
            if !structural.is_empty() {
                let mut all = structural;
                all.push(MenuItem::disabled("\u{2500}\u{2500}"));
                all.extend(items);
                items = all;
            }
        }
        Some(items)
    }

    /// The structural (jq) items for visible tree row `row` of the Pretty
    /// view, or empty when the view is not the tree (Raw/Headers, non-JSON).
    fn jq_menu_items(&self, row: usize) -> Vec<crate::components::modal::MenuItem> {
        use crate::components::modal::MenuItem;
        use postui_core::jq::{compose, render_path};
        let response = &self.session.response;
        let Some(view) = response.view() else {
            return Vec::new();
        };
        if view.mode != crate::components::response::ViewMode::Pretty {
            return Vec::new();
        }
        let Some(tree) = response.active_tree() else {
            return Vec::new();
        };
        let Some(full) = tree.full_index_of_visible(row) else {
            return Vec::new();
        };
        let line = tree.line(full);
        // Compose onto the filter whose output is on screen, not the bar
        // text: while a null-yielding or broken filter leaves another tree
        // up, the clicked path belongs to *that* tree.
        let bar = response.jq_tree_code().unwrap_or("");
        let path = tree.jq_path_of(full);
        let multi = response.jq_output_count() > 1;
        let apply = |expr: Option<&str>| Action::JqApply(compose(bar, &path, expr));
        let gated = |label: &str, action: Action| {
            if multi {
                MenuItem::disabled(format!("{label}  (collect into array first)"))
            } else {
                MenuItem::new(label, action)
            }
        };
        let mut items = vec![
            gated("Filter to this", apply(None)),
            MenuItem::new("Copy path", Action::CopyJqPath(path.clone())),
        ];
        match &line.container {
            Some(c) if c.is_array => {
                items.push(gated("Count", apply(Some("length"))));
                let keys = tree.first_element_keys(full);
                if !keys.is_empty() {
                    items.push(gated(
                        "Pluck field\u{2026}",
                        Action::JqPluckPrompt {
                            path: path.clone(),
                            keys: keys.clone(),
                        },
                    ));
                    items.push(gated(
                        "Where field\u{2026}",
                        Action::JqWherePrompt {
                            path: path.clone(),
                            keys,
                        },
                    ));
                }
            }
            Some(_) => {}
            None => {
                if let (Some(value), Some((array_line, rel))) =
                    (line.scalar_text(), tree.nearest_array_ancestor(full))
                    && !rel.is_empty()
                {
                    let rel_path = render_path(&rel);
                    let array_path = tree.jq_path_of(array_line);
                    let key_label = rel_path.trim_start_matches('.').to_string();
                    let shown = if value.chars().count() > 24 {
                        format!("{}…", value.chars().take(23).collect::<String>())
                    } else {
                        value.to_string()
                    };
                    items.push(gated(
                        &format!("Only items where {key_label} == {shown}"),
                        Action::JqApply(compose(
                            bar,
                            &array_path,
                            Some(&format!("map(select({rel_path} == {value}))")),
                        )),
                    ));
                }
            }
        }
        if multi {
            items.push(MenuItem::new("Collect into array", Action::JqCollect));
        }
        let program = crate::ai::program_name(&self.ui_settings.ai_cmd);
        if crate::ai::program_available(&self.ui_settings.ai_cmd) {
            items.push(MenuItem::new(
                "Describe a filter\u{2026}",
                Action::OpenJqDescribe,
            ));
        } else {
            items.push(MenuItem::disabled(format!(
                "Describe a filter\u{2026}  ({program} not found)"
            )));
        }
        items
    }

    /// The selected text on one named surface, or `None` when it has no
    /// selection (or, for the edit-bound surfaces, no edit is live).
    fn selection_text_of(&self, surface: crate::action::TextSurface) -> Option<String> {
        use crate::action::TextSurface;
        match surface {
            TextSurface::Url => self.editor.url.selected_text(),
            TextSurface::Body => self.editor.body_selected_text(),
            TextSurface::Response => self.session.response.selected_text(),
            TextSurface::TableCell => self.editor.table.editing.as_ref()?.input.selected_text(),
            TextSurface::VmField => self.varmanager.form.editing.as_ref()?.1.selected_text(),
            TextSurface::VmCell => self.varmanager.grid.editing.as_ref()?.input.selected_text(),
            TextSurface::Jq => self.session.response.jq_bar().input.selected_text(),
        }
    }

    /// The context menu for a right-clicked `hit`, or `None` where a right
    /// click has nothing to offer (pane backgrounds, chrome, an already-open
    /// modal). The row-targeting flows the items dispatch
    /// (`PromptRenameRequest`, `DeleteSelectedRequest`, `DuplicateRequest`,
    /// `ToggleSelectedFolder`) read `sidebar.selected`, which the right-click
    /// handler has already moved onto the clicked row.
    fn context_menu_for(&mut self, hit: &Hit) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::components::modal::MenuItem;
        let (row_index, row) = match hit {
            Hit::SidebarRow(i) | Hit::SidebarFolderArrow(i) => {
                (Some(*i), self.sidebar.rows.get(*i)?)
            }
            Hit::VmLeftRow(i) => return self.varmanager.context_menu(*i),
            Hit::ManageRow(i) => {
                return crate::components::manage_list::ManageList::context_menu(
                    self.manage.tab,
                    &self.project,
                    *i,
                );
            }
            // An option row (either half of it — the radio and the cells all
            // belong to the same record).
            Hit::VmEntryRadio(row) | Hit::VmEntryCell { row, .. } => {
                return self.varmanager.entry_context_menu(&self.project, *row);
            }
            // A params/headers/vars row (Task 17, spec §5): the right-click
            // handler has already re-resolved `i` past any commit and
            // normalized the hit to `TableRow` regardless of which part of
            // the row was clicked (see `handle_mouse`), so `i` is a live
            // index into the active tab's map here.
            Hit::TableRow(i) => return self.table_row_context_menu(*i),
            _ => return None,
        };
        // One "Move to space…" row opening the chooser of the other
        // spaces — none at all in a single-space project, where there is
        // nowhere to move to. Computed into a local first so the closure's
        // borrow of `self` doesn't overlap the `vec![]` below (which also
        // borrows `self` via `Action::OpenRequest`/etc.).
        let move_rows = |slug: &str| -> Vec<MenuItem> {
            if self.project.spaces.len() < 2 {
                return Vec::new();
            }
            vec![MenuItem::new(
                "Move to space\u{2026}",
                Action::PromptMoveRequestToSpace(slug.to_string()),
            )]
        };
        Some(match row {
            Row::Request {
                slug, broken: None, ..
            } => {
                let moves = move_rows(slug);
                let (first, last) = row_index
                    .and_then(|i| self.sidebar.group_bounds(i).map(|b| (b.0 == i, b.1 == i)))
                    .unwrap_or((true, true));
                let mut items = vec![
                    MenuItem::new("Open", Action::OpenRequest(slug.clone())),
                    MenuItem::new("Duplicate", Action::DuplicateRequest),
                    if first {
                        MenuItem::disabled("Move up")
                    } else {
                        MenuItem::new(
                            "Move up",
                            Action::MoveRequest {
                                slug: slug.clone(),
                                delta: -1,
                            },
                        )
                    },
                    if last {
                        MenuItem::disabled("Move down")
                    } else {
                        MenuItem::new(
                            "Move down",
                            Action::MoveRequest {
                                slug: slug.clone(),
                                delta: 1,
                            },
                        )
                    },
                    MenuItem::new("Rename…", Action::PromptRenameRequest),
                ];
                items.extend(moves);
                items.push(MenuItem::new("Delete", Action::DeleteSelectedRequest));
                items
            }
            // A request whose file doesn't parse can't be loaded into the
            // editor, so "Open" is shown disabled rather than hidden — the
            // menu keeps its shape and the reason is one row away.
            Row::Request {
                slug,
                broken: Some(_),
                ..
            } => {
                let moves = move_rows(slug);
                let mut items = vec![
                    MenuItem::disabled("Open"),
                    MenuItem::new("Show error…", Action::ShowRequestError(slug.clone())),
                    MenuItem::new("Duplicate", Action::DuplicateRequest),
                    MenuItem::new("Rename…", Action::PromptRenameRequest),
                ];
                items.extend(moves);
                items.push(MenuItem::new("Delete", Action::DeleteSelectedRequest));
                items
            }
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

    /// The right-click menu for row `i` of the active params/headers/vars
    /// table (Task 17, spec §5): duplicate, delete, and extract its value to
    /// a variable. `None` for the Body tab (no table there) or a row index
    /// past the map's end (the ghost row, or one that vanished under a
    /// commit the caller already resolved past).
    fn table_row_context_menu(&self, i: usize) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::components::modal::MenuItem;
        let (map, noun) = match self.editor.active_tab {
            EditorTab::Params => (&self.editor.params, "param"),
            EditorTab::Headers => (&self.editor.headers, "header"),
            EditorTab::Vars => (&self.editor.variables, "variable"),
            EditorTab::Body => return None,
        };
        if i >= map.len() {
            return None;
        }
        Some(vec![
            MenuItem::new("Duplicate row", Action::DuplicateTableRow(i)),
            MenuItem::new(format!("Delete {noun}"), Action::DeleteTableRow(i)),
            MenuItem::new("Extract value to variable…", Action::ExtractToVariable),
            MenuItem::new("Extract value to selector…", Action::ExtractToSelector),
        ])
    }

    /// Whether leaving the editor's current content behind would lose
    /// work: edits to a saved request, or a never-saved scratch with real
    /// content. Every dirty gate checks this.
    fn editor_holds_unsaved(&self) -> bool {
        self.editor.is_dirty() || self.editor.is_scratch_dirty()
    }

    /// Push the standard unsaved-changes confirm. A slugged request's
    /// "save" path relies on SaveRequest completing synchronously; a
    /// never-saved scratch has no name yet, so its save path goes through
    /// the Save-as prompt, with `then` deferred until that save succeeds.
    fn dirty_gate(&mut self, verb: &str, then: Action) {
        if self.editor.slug.is_none() {
            self.push_modal(Modal::Confirm {
                title: "Unsaved request".into(),
                body: "This request has never been saved.".into(),
                choices: vec![
                    (
                        's',
                        format!("Save as… & {verb}"),
                        vec![Action::PromptSaveScratch(Box::new(then.clone()))],
                    ),
                    ('d', "Discard request".into(), vec![then]),
                ],
            });
            return;
        }
        let current = self.editor.slug.clone().unwrap_or_default();
        self.push_modal(Modal::Confirm {
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
    /// Returns whether the request was actually saved, so callers with a
    /// deferred follow-up (the scratch gate) only proceed on success.
    /// The display name of the request at `slug` — its `name` field when
    /// the file parses and has one, otherwise the slug leaf (legacy and
    /// broken files).
    fn request_display(&self, slug: &str) -> String {
        postui_core::storage::load_request(&self.project.root, slug)
            .ok()
            .and_then(|r| r.name)
            .unwrap_or_else(|| slug.rsplit('/').next().unwrap_or(slug).to_string())
    }

    fn create_or_save_as(
        &mut self,
        name: &str,
        build: impl FnOnce(&str) -> postui_core::model::HttpRequest,
    ) -> bool {
        use postui_core::storage::{self, StorageError};
        // Every new request lands inside the active space — the name the
        // user typed is relative to it.
        let name = format!(
            "{}/{}",
            self.project.active_space,
            name.trim_start_matches('/')
        );
        let name = name.as_str();
        let req = build(name);
        match storage::create_request_named(&self.project.root, name, req) {
            Ok((slug, leaf)) => {
                // Reload from disk so the editor holds exactly what was
                // written (display name included).
                if let Ok(saved) = storage::load_request(&self.project.root, &slug) {
                    self.editor.load(Some(slug.clone()), saved);
                    self.editor.mark_saved();
                }
                // A brand-new file never existed before this write, so
                // `before` is simply absent — no pre-read needed.
                let path = storage::request_path(&self.project.root, &slug);
                self.record_file_step(vec![(path.clone(), None)], &[path], None);
                if let Some((space, rel)) = Self::split_rel(&slug) {
                    let r = postui_core::order::order_arrive(&self.project.root, space, rel);
                    self.order_cascade("create", r);
                }
                self.toasts
                    .push(format!("Saved {leaf}"), ToastKind::Success);
                // Queue the slug's ancestor folders open, rebuild the tree
                // with them expanded (so the new row exists at all), then
                // select it now that it's actually visible.
                let prev = self.sidebar.open_row();
                self.sidebar.select_slug(&slug);
                self.refresh_sidebar();
                self.sidebar.select_slug(&slug);
                self.retarget_sidebar_travel(prev);
                self.apply(Action::PersistLocalState);
                true
            }
            Err(StorageError::AlreadyExists(taken)) => {
                self.toasts.push(
                    format!("a request named {taken:?} already exists here"),
                    ToastKind::Error,
                );
                self.last_action_failed = true;
                false
            }
            Err(StorageError::InvalidSlug(_)) => {
                self.toasts
                    .push("request name cannot be empty", ToastKind::Error);
                self.last_action_failed = true;
                false
            }
            Err(e) => {
                self.toasts
                    .push(format!("could not save {name}: {e}"), ToastKind::Error);
                self.last_action_failed = true;
                false
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

    /// Routes pasted text — read from the OS clipboard by `Action::Paste`
    /// (ctrl+v), or delivered whole by a terminal bracketed paste
    /// (`Event::Paste`: cmd+V on macOS, ctrl+shift+V on Linux) — to
    /// whatever text surface owns the caret, mirroring `handle_key`'s
    /// capture order: the top modal's text surface first (a filter-query
    /// modal's query — palette/chooser/var picker — or the focused
    /// `LineInput`), then a live Variable-Manager field or grid edit, then
    /// the response pane's live search input, then the editor's cell edit /
    /// URL bar / body. A live selection is replaced (GUI semantics);
    /// single-line surfaces flatten line breaks (`LineInput::paste`),
    /// the body takes them verbatim. Returns `false` when nothing
    /// focused accepts text.
    pub fn paste_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        if !self.modals.is_empty() {
            // The filter-query modals hold their query as a plain String
            // (no LineInput), so they sit outside `focused_input_index` —
            // but their queries took typed text, so they take pastes too.
            match self.modals.top_mut() {
                Some(crate::components::modal::Modal::Palette(p)) => {
                    p.paste(text);
                    return self.update(Action::Render);
                }
                Some(crate::components::modal::Modal::Chooser(c)) => {
                    c.paste(text);
                    return self.update(Action::Render);
                }
                Some(crate::components::modal::Modal::FilePicker(p)) => {
                    p.paste(text);
                    return self.update(Action::Render);
                }
                Some(crate::components::modal::Modal::VarPicker(v)) => {
                    return v.paste(text) && self.update(Action::Render);
                }
                _ => {}
            }
            if let Some(i) = self.modals.focused_input_index()
                && let Some(input) = self.modals.focus_input(i)
            {
                input.paste(text);
                return self.update(Action::Render);
            }
            return false;
        }
        if self.screen == Screen::Manage {
            if let Some((_, input)) = self.varmanager.form.editing.as_mut() {
                input.paste(text);
                return self.update(Action::Render);
            }
            if let Some(edit) = self.varmanager.grid.editing.as_mut() {
                edit.input.paste(text);
                return self.update(Action::Render);
            }
            return false;
        }
        if self.screen != Screen::Main {
            return false;
        }
        // The response pane's text surfaces: the jq bar while focused, else
        // its search input while live.
        if self.focus == PaneId::Response {
            if self.session.response.paste_into_jq(text) {
                return self.update(Action::Render);
            }
            return self.session.response.paste_into_search(text) && self.update(Action::Render);
        }
        if self.focus != PaneId::Editor {
            return false;
        }
        if let Some(edit) = self.editor.table.editing.as_mut() {
            edit.input.paste(text);
            return self.update(Action::Render);
        }
        match self.editor.sub_focus {
            SubFocus::Url => {
                self.editor.url.paste(text);
                self.update(Action::Render)
            }
            SubFocus::Content if self.editor.active_tab == EditorTab::Body => {
                self.editor.paste_body(text) && self.update(Action::Render)
            }
            _ => false,
        }
    }

    /// Copies `text` to the clipboard and toasts the outcome:
    /// `success_msg` on success, the shared size/availability warnings
    /// otherwise. The one clipboard-write path for both explicit copy
    /// actions and ctrl+c selection copies.
    fn copy_text_with_toast(&mut self, text: &str, success_msg: String) {
        match self.clipboard.copy(text) {
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
    }

    /// The text of whichever selection currently owns the keyboard, in
    /// focus-priority order: the top modal's text field, a Variable
    /// Manager edit, then (on the main screen) a table cell edit, the URL
    /// bar, the body editor, and the response view. `None` when nothing
    /// is selected anywhere — the caller falls back to the key's normal
    /// meaning (ctrl+c quits).
    fn active_selection_text(&self) -> Option<String> {
        if let Some(input) = self.modals.focused_input() {
            return input.selected_text();
        }
        if self.screen == Screen::Manage {
            if let Some((_, input)) = self.varmanager.form.editing.as_ref() {
                return input.selected_text();
            }
            if let Some(edit) = self.varmanager.grid.editing.as_ref() {
                return edit.input.selected_text();
            }
            return None;
        }
        if let Some(edit) = self.editor.table.editing.as_ref()
            && let Some(text) = edit.input.selected_text()
        {
            return Some(text);
        }
        if self.editor.sub_focus == SubFocus::Url
            && let Some(text) = self.editor.url.selected_text()
        {
            return Some(text);
        }
        // Body and response selections are visible highlights — copyable
        // whenever they exist, not only while their pane owns the
        // keyboard.
        if let Some(text) = self.editor.body_selected_text() {
            return Some(text);
        }
        if let Some(text) = self.session.response.selected_text() {
            return Some(text);
        }
        None
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
            CopyTarget::ResponseView => {
                let ResponseState::Ready(_) = self.session.response.state() else {
                    return None;
                };
                let view = self.session.response.view()?;
                let what = match view.mode {
                    crate::components::response::ViewMode::Headers => "response headers",
                    _ => "response body",
                };
                Some((view.view_text(), format!("Copied {what}")))
            }
            CopyTarget::ResponseHeader(i) => match self.session.response.state() {
                ResponseState::Ready(d) => d
                    .headers
                    .get(*i)
                    .map(|(name, value)| (value.clone(), format!("Copied {name}"))),
                _ => None,
            },
            CopyTarget::Url => Some((self.editor.url.text().to_string(), "Copied URL".to_string())),
            CopyTarget::ComputedHeader(i) => self
                .editor
                .computed
                .rows
                .iter()
                .filter(|r| r.origin != postui_core::prepare::HeaderOrigin::Request)
                .nth(*i)
                .map(|r| (r.value.clone(), format!("Copied {}", r.name))),
        }
    }

    /// Whether any in-flight HTTP request is still ticking (e.g. animating
    /// a spinner) and therefore needs a redraw.
    fn in_flight_ticking(&self) -> bool {
        !self.session.in_flight.is_empty()
            // A background pretty-print animates its own spinner, so ticks
            // must keep coming while one is running — and so does the jq
            // bar's, once a background run outlives its grace period.
            || self.session.response.view().is_some_and(|v| v.parsing)
            || self.session.response.jq_bar().pending.is_some()
    }

    /// Whether any tracked animation is still easing toward its target right
    /// now. Drives `Action::Tick`'s redraw decision and the main loop's
    /// adaptive tick period — both sample `Instant::now()` themselves so
    /// `Anims` stays deterministic and this stays cheap to call every frame.
    pub fn animating(&self) -> bool {
        self.anims.active(Instant::now())
    }

    /// Drives the production Send-cap breathe while a request is in
    /// flight: if `AnimKey::SendBreathe` has never been set, or a real
    /// send just started after it was last cleared, snaps it to 0 and
    /// starts easing to 1 over `ui_settings.anim_ms.send_breathe` (700ms
    /// by default); each tick after it finishes, retargets to the
    /// opposite pole over the same duration, so it ping-pongs for as long
    /// as `in_flight` stays set. Clears the key once nothing is in flight,
    /// so the next send starts a fresh breathe rather than resuming
    /// mid-pulse. Never called on `Screen::Testbed` — that screen drives
    /// the same `AnimKey` itself via `drive_testbed_pingpong`.
    fn tick_send_breathe(&mut self, now: Instant) {
        // The breathe animates the *open* request's Send cap; another
        // request's background send must not keep it pulsing.
        if !self.session.is_in_flight(&self.editor.slug) {
            self.anims.clear(AnimKey::SendBreathe);
            return;
        }
        let dur = self.ui_settings.anim_ms.send_breathe;
        if self.anims.value(AnimKey::SendBreathe, now).is_none() {
            self.anims.snap(AnimKey::SendBreathe, 0.0);
            self.anims.retarget(AnimKey::SendBreathe, 1.0, dur, now);
            return;
        }
        if self.anims.is_done(AnimKey::SendBreathe, now) {
            let cur = self.anims.value_or(AnimKey::SendBreathe, now, 0.0);
            let target = if cur >= 0.5 { 0.0 } else { 1.0 };
            self.anims.retarget(AnimKey::SendBreathe, target, dur, now);
        }
    }

    /// Starts the hover fade over from 0: snaps `AnimKey::Hover` to 0 and
    /// retargets it to 1 over `ui_settings.anim_ms.hover` (70ms by
    /// default, config-tunable). Called whenever `self.hovered`'s hit
    /// *changes* (see `app/mouse.rs`), so the newly hovered control's fill
    /// eases in rather than jumping.
    pub(crate) fn begin_hover_fade(&mut self) {
        let now = Instant::now();
        self.anims.snap(AnimKey::Hover, 0.0);
        self.anims
            .retarget(AnimKey::Hover, 1.0, self.ui_settings.anim_ms.hover, now);
    }

    /// Starts the focus fade over from 0: snaps `AnimKey::FocusFade` to 0
    /// and retargets it to 1 over `ui_settings.anim_ms.focus` (90ms by
    /// default, config-tunable). Called wherever keyboard focus actually
    /// moves onto a control the address bar animates (today, just
    /// `Action::FocusUrl` — the single option point both the URL keyboard
    /// shortcut and clicking `Hit::UrlBar` go through), so the newly
    /// focused control's lifted fill eases in rather than jumping.
    pub(crate) fn begin_focus_fade(&mut self) {
        let now = Instant::now();
        self.anims.snap(AnimKey::FocusFade, 0.0);
        self.anims
            .retarget(AnimKey::FocusFade, 1.0, self.ui_settings.anim_ms.focus, now);
    }

    /// Starts a dropdown's open-settle over from 0: snaps `AnimKey::DropdownOpen`
    /// to 0 and retargets it to 1 over `ui_settings.anim_ms.dropdown_open`
    /// (90ms by default, config-tunable). Called by both `Modal::Dropdown`
    /// push sites (`Action::OpenMethodDropdown` and `open_context_menu`) so
    /// the popup's panel fill grows in from its own top edge rather than
    /// appearing instantly. Closing is always instant — every modal-pop
    /// path snaps this key straight to 1 instead of retargeting it.
    pub(crate) fn begin_dropdown_open(&mut self) {
        let now = Instant::now();
        self.anims.snap(AnimKey::DropdownOpen, 0.0);
        self.anims.retarget(
            AnimKey::DropdownOpen,
            1.0,
            self.ui_settings.anim_ms.dropdown_open,
            now,
        );
    }

    /// Pushes `modal` onto the modal stack, driving `AnimKey::ModalOpen`
    /// (the panel-style shell's open-settle): an empty→non-empty push
    /// retargets it from 0 to 1 over `ui_settings.anim_ms.modal_open`
    /// (100ms by default, config-tunable) so the panel fades/settles in;
    /// pushing an additional modal onto an already non-empty stack snaps it
    /// straight to 1 instead — no re-animation for a modal opened on top of
    /// another, and likewise for a handoff push (`modal_handoff`), where
    /// the stack is only momentarily empty between two modals of one
    /// flow. A `Modal::Dropdown` push is exempted entirely: dropdowns
    /// settle via their own `AnimKey::DropdownOpen` (started separately by
    /// `begin_dropdown_open` at their two push sites), and often land on
    /// top of an existing modal stack, so touching `ModalOpen` for them
    /// would either double-animate or wrongly snap a panel modal's own
    /// baseline mid-flight.
    /// Opens the save picker in the Downloads folder (else home) with a
    /// suggested filename; confirming routes through `PickerConfirm`.
    fn open_save_picker(
        &mut self,
        title: &str,
        target: crate::components::file_picker::PickerTarget,
        suggested: &str,
    ) -> bool {
        use crate::components::file_picker::{FilePickerState, default_save_dir};
        self.push_modal(Modal::FilePicker(FilePickerState::new(
            title,
            target,
            &default_save_dir(),
            suggested,
        )));
        true
    }

    pub(crate) fn push_modal(&mut self, modal: Modal) {
        if !matches!(modal, Modal::Dropdown(_)) {
            let now = Instant::now();
            if self.modals.is_empty() && !self.modal_handoff {
                self.anims.snap(AnimKey::ModalOpen, 0.0);
                self.anims.retarget(
                    AnimKey::ModalOpen,
                    1.0,
                    self.ui_settings.anim_ms.modal_open,
                    now,
                );
            } else {
                self.anims.snap(AnimKey::ModalOpen, 1.0);
            }
        }
        self.modals.push(modal);
    }

    /// Drives every looping motion demo on the hidden testbed screen
    /// (Task 8b): each self-retargets once it finishes, so the demos
    /// animate continuously for as long as the screen is showing, with no
    /// interaction required. Called once per `Action::Tick` while
    /// `self.screen == Screen::Testbed`; every other screen's tick path
    /// never reaches this.
    ///
    /// The underline and hover demos are duration COMPARISON rows: the
    /// user wasn't sure whether the plan's 140ms/70ms felt right against a
    /// reference app with longer transitions, so several candidate
    /// durations are shown side by side, each labeled with its own actual
    /// duration (`components::testbed::draw_motion_section` renders the
    /// labels), so whichever one the user picks maps 1:1 to a config
    /// value. [`Self::drive_testbed_group`] is what keeps every row in a
    /// comparison starting its move at the exact same instant despite
    /// easing over different durations — the only way to compare durations
    /// fairly is to hold every other variable (including "when did it
    /// start moving") fixed.
    ///
    /// The reused `AnimKey`s here (`TabUnderline`/`TabUnderlineWidth` on
    /// `EditorTabs`/`ResponseTabs`, `Hover`, `SendBreathe`,
    /// `ListTravel(Sidebar|Palette)`, and `ToastFade` borrowed with demo-
    /// only integer ids for the duration-comparison rows — real toasts
    /// never reach ids in the 70-500 range, and none of these keys are
    /// wired to anything else yet) can't collide with real usage, since
    /// the testbed is a dead end no other surface is ever drawn alongside.
    ///
    /// `animations = false` (the config kill-switch) still freezes these:
    /// `Anims::retarget` collapses every duration to zero when disabled, so
    /// each demo's very first retarget already lands on its target with
    /// nothing left in flight — `Anims::active` goes false and the main
    /// loop simply stops ticking, exactly like every other animation in
    /// the app.
    fn tick_testbed_demos(&mut self, now: Instant) {
        // Underline slide: four durations compared side by side, all
        // sharing one cycle clock. 500ms was this task's own (unauthorized,
        // now-corrected) demo-slowdown value; kept as the fourth candidate
        // since the reference app the user is comparing against runs
        // noticeably longer than the 140ms plan.
        self.drive_testbed_group(
            &[
                (AnimKey::ToastFade(140), Duration::from_millis(140)),
                (AnimKey::ToastFade(250), Duration::from_millis(250)),
                (AnimKey::ToastFade(400), Duration::from_millis(400)),
                (AnimKey::ToastFade(500), Duration::from_millis(500)),
            ],
            Duration::from_millis(1800),
            now,
        );
        // Hover fade: three durations compared side by side, same
        // synced-cycle treatment.
        self.drive_testbed_group(
            &[
                (AnimKey::ToastFade(70), Duration::from_millis(70)),
                (AnimKey::ToastFade(150), Duration::from_millis(150)),
                (AnimKey::ToastFade(300), Duration::from_millis(300)),
            ],
            Duration::from_millis(1800),
            now,
        );
        // Send breathe: the in-flight breathe from the motion catalog, at
        // its config-tunable plan duration (700ms/pole by default) —
        // continuous, no dwell between poles. Not a duration comparison
        // (there's only ever one copy), so it reads straight from
        // `ui_settings.anim_ms` rather than a literal, keeping the field
        // from going dead ahead of the tasks that wire the rest.
        self.drive_testbed_pingpong(
            AnimKey::SendBreathe,
            0.0,
            1.0,
            self.ui_settings.anim_ms.send_breathe,
            Duration::ZERO,
            now,
        );
        // List travel: the plan duration alongside one alternative
        // (250ms/row) — not synced to each other (each has its own
        // dwell-then-step cadence; a shared clock isn't needed to compare
        // a *stepped* motion the way it is for a continuous ease). The
        // plan row reads `ui_settings.anim_ms.list_travel` (100ms by
        // default) rather than a literal, for the same reason as the
        // breathe demo above; the 250ms alternative stays a literal since
        // it exists purely to be compared against the plan value.
        let mut plan_dir = self.testbed_list_dir_plan;
        self.tick_testbed_list_travel(
            AnimKey::ListTravel(ListId::Sidebar),
            &mut plan_dir,
            self.ui_settings.anim_ms.list_travel,
            Duration::from_millis(900),
            now,
        );
        self.testbed_list_dir_plan = plan_dir;

        let mut alt_dir = self.testbed_list_dir_alt;
        self.tick_testbed_list_travel(
            AnimKey::ListTravel(ListId::Palette),
            &mut alt_dir,
            Duration::from_millis(250),
            Duration::from_millis(900),
            now,
        );
        self.testbed_list_dir_alt = alt_dir;
    }

    /// One looping demo's drive step: if `key` isn't tracked yet, starts it
    /// moving from `pole0` toward `pole1`. Once a move finishes, holds at
    /// the arrived-at value for `dwell_dur` (a `retarget` to the same
    /// value — see [`crate::anim::Anims::is_static`]); once a dwell
    /// finishes, starts the next move toward whichever pole isn't the
    /// current value. A zero `dwell_dur` still takes one extra (near-
    /// instant) tick to flip, since the dwell retarget's own `done()` isn't
    /// checked until the tick after it's set — imperceptible against a
    /// ~33ms tick period.
    fn drive_testbed_pingpong(
        &mut self,
        key: AnimKey,
        pole0: f32,
        pole1: f32,
        move_dur: Duration,
        dwell_dur: Duration,
        now: Instant,
    ) {
        if self.anims.value(key, now).is_none() {
            self.anims.snap(key, pole0);
            self.anims.retarget(key, pole1, move_dur, now);
            return;
        }
        if !self.anims.is_done(key, now) {
            return;
        }
        let cur = self.anims.value_or(key, now, pole0);
        if self.anims.is_static(key) {
            let target = if (cur - pole0).abs() <= (cur - pole1).abs() {
                pole1
            } else {
                pole0
            };
            self.anims.retarget(key, target, move_dur, now);
        } else {
            self.anims.retarget(key, cur, dwell_dur, now);
        }
    }

    /// A duration-comparison selector's drive step: every `(key, move_dur)`
    /// field retargets at the exact same instant, each easing over its
    /// own `move_dur` — so what's compared is purely "does this duration
    /// feel right", with every other variable (start time, target,
    /// underlying geometry) held fixed. Phase changes (move → dwell →
    /// move …) are decided off a single "clock" field — the one with the
    /// longest `move_dur` in the selector — rather than each field's own
    /// timer: by the time the clock's own move finishes, every faster
    /// field has already arrived and been idling at its target, so
    /// forcing the whole selector into a dwell at that instant doesn't cut
    /// any of them off mid-motion. `dwell_dur` is shared by every field.
    fn drive_testbed_group(
        &mut self,
        fields: &[(AnimKey, Duration)],
        dwell_dur: Duration,
        now: Instant,
    ) {
        let &(clock_key, _) = fields
            .iter()
            .max_by_key(|(_, dur)| *dur)
            .expect("a duration-comparison selector always has at least one field");

        if self.anims.value(clock_key, now).is_none() {
            for &(key, dur) in fields {
                self.anims.snap(key, 0.0);
                self.anims.retarget(key, 1.0, dur, now);
            }
            return;
        }
        if !self.anims.is_done(clock_key, now) {
            return;
        }
        let cur = self.anims.value_or(clock_key, now, 0.0);
        if self.anims.is_static(clock_key) {
            let target = if cur <= 0.5 { 1.0 } else { 0.0 };
            for &(key, dur) in fields {
                self.anims.retarget(key, target, dur, now);
            }
        } else {
            for &(key, _) in fields {
                let v = self.anims.value_or(key, now, 0.0);
                self.anims.retarget(key, v, dwell_dur, now);
            }
        }
    }

    /// One list-travel demo's drive step: steps one row at a time across a
    /// 5-row list (indices `0.0..=4.0`), reversing direction at either
    /// end, with a `dwell_dur` pause at each row so the step reads as a
    /// step rather than a blur. `dir` is the caller's own per-demo
    /// direction state (`1` down / `-1` up) — the one bit `Anims` alone
    /// can't recover, since both ends of the range otherwise look like the
    /// same "arrived and dwelling" moment.
    fn tick_testbed_list_travel(
        &mut self,
        key: AnimKey,
        dir: &mut i32,
        move_dur: Duration,
        dwell_dur: Duration,
        now: Instant,
    ) {
        const LAST_ROW: f32 = 4.0;
        if self.anims.value(key, now).is_none() {
            self.anims.snap(key, 0.0);
            *dir = 1;
            self.anims.retarget(key, 1.0, move_dur, now);
            return;
        }
        if !self.anims.is_done(key, now) {
            return;
        }
        let cur = self.anims.value_or(key, now, 0.0);
        if self.anims.is_static(key) {
            if cur <= 0.0 {
                *dir = 1;
            } else if cur >= LAST_ROW {
                *dir = -1;
            }
            let next = (cur + *dir as f32).clamp(0.0, LAST_ROW);
            self.anims.retarget(key, next, move_dur, now);
        } else {
            self.anims.retarget(key, cur, dwell_dur, now);
        }
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
    ///    quitting, cycling the active env, and the screen open/close
    ///    actions themselves) can
    ///    still fire on a CTRL/ALT combo; every other global shortcut
    ///    (send, save, cycle project, focus URL, …) is *not* reachable
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
        let ev = crate::keys::normalize_super_keys(ev);
        // cmd+c — SUPER+c, from terminals that report it — is copy-only:
        // copy the live selection, otherwise nothing. It is deliberately
        // not folded onto ctrl+c (whose selectionless meaning is quit): a
        // reflexive mac cmd+c must never quit the app.
        if ev.modifiers.contains(KeyModifiers::SUPER) && matches!(ev.code, KeyCode::Char('c' | 'C'))
        {
            if let Some(text) = self.active_selection_text() {
                self.copy_text_with_toast(&text, "Copied selection".to_string());
            }
            return true;
        }
        let combo = KeyCombo::from_event(&ev);
        let global = keymap.lookup(&combo);
        let modified = ev
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

        // 1. A modified quit combo is the escape hatch: it pre-empts
        // everything — except that when a text selection is live anywhere,
        // ctrl+c means "copy" (the GUI meaning) and quit keeps working the
        // moment nothing is selected.
        if modified && global == Some(Action::Quit) {
            if let Some(text) = self.active_selection_text() {
                self.copy_text_with_toast(&text, "Copied selection".to_string());
                return true;
            }
            return self.update(Action::Quit);
        }

        // A live row drag owns Escape: it cancels the drag, nothing else.
        if ev.code == KeyCode::Esc && self.sidebar.drag.is_some() {
            return self.finish_sidebar_drag(false);
        }
        if ev.code == KeyCode::Esc && self.manage.list.drag.is_some() {
            return self.finish_manage_drag(false);
        }

        // …and a live row drag owns the rest of the keyboard: with a row
        // in flight there is no sane meaning for a key that opens a
        // modal, moves the selection or reorders by keyboard underneath
        // it, so everything but the two escape hatches above (the
        // modified quit combo, Escape) is swallowed here — before modals
        // and before the per-screen routes. The footer says as much: while
        // a drag is live its chips are the cancel keys and nothing else.
        if self.sidebar.drag.is_some() || self.manage.list.drag.is_some() {
            return false;
        }

        // 1b. A bound paste combo digs past the layers below: modals and
        // non-Main screens capture all remaining input, but ctrl+v means
        // "insert at the live caret" wherever that caret is —
        // `paste_text` (via `Action::Paste`) does the routing.
        if modified && global == Some(Action::Paste) {
            return self.update(Action::Paste);
        }

        // 2. Modals capture all remaining input.
        if !self.modals.is_empty() {
            // alt+b is a toggle: over the open theme picker it closes it
            // (reverting any preview — `Action::Close`'s Chooser branch),
            // instead of being swallowed. `theme_preview` is `Some`
            // exactly while the theme picker is the open chooser, so a
            // project chooser is never mistaken for it.
            if modified
                && global == Some(Action::OpenThemeChooser)
                && self.theme_preview.is_some()
                && matches!(
                    self.modals.top(),
                    Some(crate::components::modal::Modal::Chooser(_))
                )
            {
                return self.update(Action::Close);
            }
            // The value popup's remove chord needs App (it removes, then
            // rebuilds the popup), so it can't live in the modal's own
            // key handler like the fields editor's chords do. Inert when
            // the chosen scope stores nothing — same as the unpainted ✕.
            if ev.modifiers.contains(KeyModifiers::ALT)
                && ev.code == KeyCode::Char('d')
                && matches!(
                    self.modals.top(),
                    Some(crate::components::modal::Modal::MultiPrompt {
                        kind: crate::components::modal::PromptKind::EditVarValue { .. },
                        ..
                    })
                )
            {
                self.remove_from_value_popup();
                return true; // swallowed even when inert — never typed
            }
            let Some(res) = self.modals.handle_key(ev) else {
                self.sync_theme_preview();
                return true; // typed into modal
            };
            let changed = self.apply_modal_result(res);
            self.sync_theme_preview();
            return changed;
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
            // The testbed is a dead end, not a screen with content of its
            // own to navigate: `q`/`Esc` quit the app outright rather than
            // returning to `Main` (there's no "prior" state to restore —
            // it's only ever entered at startup), and every other key is
            // swallowed like any other non-`Main` screen.
            if self.screen == Screen::Testbed {
                return match ev.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.update(Action::Quit),
                    _ => true,
                };
            }
            // alt+←/→ walk the Manage screen's tab strip, wrapping —
            // above every tab body's own keys, since the strip belongs to
            // the shell rather than to whichever tab is up.
            if self.screen == Screen::Manage
                && ev.modifiers.contains(KeyModifiers::ALT)
                && matches!(ev.code, KeyCode::Left | KeyCode::Right)
            {
                let delta = if ev.code == KeyCode::Right { 1 } else { -1 };
                return self.update(Action::SelectManageTab(self.manage.tab.cycle(delta)));
            }
            // The Environments and Spaces tabs: the list's own keys run
            // and anything they don't claim is swallowed like on any other
            // non-`Main` screen.
            if self.screen == Screen::Manage
                && self.manage.tab != crate::components::manage::ManageTab::Variables
            {
                let tab = self.manage.tab;
                if let Some(a) = self.manage.list.handle_key(ev, tab, &self.project) {
                    return self.update(a);
                }
                return true;
            }
            // A variable-form field under edit owns the keyboard: `Esc`
            // reverts, `Enter` commits (through `commit_var_form`, which
            // needs the mutable project access `VarManager::handle_key`'s
            // shared `&ProjectContext` can't give it), everything else is
            // forwarded straight to its `LineInput`.
            if self.screen == Screen::Manage && self.varmanager.form.editing.is_some() {
                return self.handle_var_form_key(ev);
            }
            if self.screen == Screen::Manage && self.varmanager.grid.editing.is_some() {
                return self.handle_grid_key(ev);
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

        // 4-exception: with the caret live in the body editor, alt+←/→
        // are the macOS word-jump spelling (option+arrow) and go to the
        // editor instead of the global tab-cycle binding; tab switching
        // from inside the body is mouse-only (ctrl-digits now switch
        // spaces), and everywhere else alt+arrows cycle as before. The
        // shifted variants and alt+backspace need no carve-out: those
        // combos are unbound, so step 4 already falls through to the
        // component.
        if ev.modifiers.contains(KeyModifiers::ALT)
            && matches!(ev.code, KeyCode::Left | KeyCode::Right)
            && self.focus == PaneId::Editor
            && self.editor.sub_focus == SubFocus::Content
            && self.editor.active_tab == EditorTab::Body
            && let Some(a) = self.focused_component_key(ev)
        {
            return self.update(a);
        }

        // 4. Modified combos prefer the global keymap (app shortcuts beat editors).
        if modified && let Some(a) = global {
            return self.update(a);
        }

        // 4b. A bound Shift+Enter (send, by default) also beats the focused
        // component: plain Enter belongs to editors (newline/commit), but
        // the shifted chord is a global. Only terminals speaking the kitty
        // keyboard protocol can report it; elsewhere it arrives as plain
        // Enter and the ctrl+r/ctrl+enter bindings are the ones that work.
        if ev.code == KeyCode::Enter
            && ev.modifiers.contains(KeyModifiers::SHIFT)
            && let Some(a) = global.clone()
        {
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

    /// Keys while a variable-form field owns the keyboard (Task 8's model,
    /// exactly): `Esc` reverts (drops the edit with nothing written —
    /// there's nothing to restore since the form only ever reads its
    /// resting text live from `self.project`, never caches it), `Enter`
    /// commits via `commit_var_form`, everything else goes to the field's
    /// own `LineInput`. Always reports a redraw, like a modal capturing
    /// every key while it's open.
    fn handle_var_form_key(&mut self, ev: KeyEvent) -> bool {
        match ev.code {
            KeyCode::Esc => {
                self.varmanager.form.editing = None;
            }
            KeyCode::Enter => self.commit_var_form(),
            _ => {
                if let Some((_, input)) = self.varmanager.form.editing.as_mut() {
                    input.handle_key(ev);
                }
            }
        }
        self.update(Action::Render)
    }

    /// Keys while a selector-grid cell owns the keyboard — the same contract
    /// as [`Self::handle_var_form_key`]: `Esc` reverts (nothing is written,
    /// and the cell's resting text is read live from the project either
    /// way), `Enter` commits, anything else goes to the cell's own
    /// `LineInput`. `Tab` commits and steps one column right on the same
    /// row, so a freshly created option can be filled in without reaching
    /// for the mouse; a commit that failed keeps its edit and stays put.
    fn handle_grid_key(&mut self, ev: KeyEvent) -> bool {
        match ev.code {
            KeyCode::Esc => {
                self.varmanager.grid.editing = None;
            }
            KeyCode::Enter => self.commit_grid_edit(),
            KeyCode::Tab => self.step_grid_edit(1),
            KeyCode::BackTab => self.step_grid_edit(-1),
            _ => {
                if let Some(edit) = self.varmanager.grid.editing.as_mut() {
                    edit.input.handle_key(ev);
                }
            }
        }
        self.update(Action::Render)
    }

    /// `Tab`/`BackTab` inside a live grid cell: commit, then open the cell
    /// one step away in reading order (Task 8's table parity) — off the end
    /// of a row wraps to the next row's first column, and off the front of
    /// one wraps back to the previous row's last. The ghost row is the far
    /// end in both directions: forward stops there (it becomes a real option
    /// only once its name commits), and there is nothing before row 0's
    /// name cell.
    ///
    /// A commit that failed keeps its own edit (spec §5) and this leaves it
    /// exactly where it is; a ghost-row commit that already walked the edit
    /// on into the new option's first field is likewise left alone.
    fn step_grid_edit(&mut self, dir: i32) {
        let at = self
            .varmanager
            .grid
            .editing
            .as_ref()
            .map(|e| (e.row, e.col));
        self.commit_grid_edit();
        let Some((row, col)) = at else { return };
        if self.varmanager.grid.editing.is_some() {
            return;
        }
        let VmDetail::Group(selector) = self.varmanager.detail.clone() else {
            return;
        };
        let ncols = 1 + self
            .project
            .model
            .selectors
            .get(&selector)
            .map_or(0, |g| g.fields.len());
        let last_row = postui_core::varmodel::options_of(
            &self.project.model,
            &self.project.env_data,
            &selector,
        )
        .map_or(0, indexmap::IndexMap::len);
        let flat = (row * ncols + col) as i32 + dir;
        let (next_row, next_col) = match flat {
            // Off either end of the grid: the walk stops rather than
            // closing the edit out from under the user — the same cell
            // re-opens, now showing what was just committed.
            f if f < 0 => (row, col),
            f => {
                let (r, c) = (f as usize / ncols, f as usize % ncols);
                if r > last_row { (row, col) } else { (r, c) }
            }
        };
        self.varmanager
            .start_cell_edit(&self.project, next_row, next_col);
    }

    fn focused_component_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match self.focus {
            PaneId::Sidebar => {
                // Arrow-key browsing moves only the cursor; the selection
                // band stays put on the open request (an Enter that opens
                // the cursor's row retargets the band via
                // `ForceOpenRequest`).
                self.sidebar.handle_key(ev)
            }
            PaneId::Editor => {
                let was_content =
                    self.editor.sub_focus == crate::components::editor::SubFocus::Content;
                let action = self.editor.handle_key(ev);
                if !was_content
                    && self.editor.sub_focus == crate::components::editor::SubFocus::Content
                {
                    self.begin_focus_fade();
                }
                action
            }
            PaneId::Response => self.session.response.handle_key(ev),
        }
    }

    /// Retargets `AnimKey::ListTravel(Sidebar)` from `prev`'s row (the row
    /// the selection band was on — the previously OPEN request) toward the
    /// newly open request's row, over the config-tunable
    /// `ui_settings.anim_ms.list_travel` (100ms by default). The band
    /// tracks the OPEN request, not the keyboard cursor, so this is called
    /// only after mutations that change which request is open (the
    /// `ForceOpenRequest`/create-request flows). A no-op when the open row
    /// didn't move, or when nothing is open (`draw`'s own fallback already
    /// snaps to the open row whenever the anim has no tracked value).
    fn retarget_sidebar_travel(&mut self, prev: Option<usize>) {
        // `sidebar.open_slug` is normally synced from the editor after the
        // full action applies (see `update`); the callers sit mid-arm, so
        // sync it here first to compute the band's real destination.
        self.sidebar.open_slug = self.editor.slug.clone();
        let Some(cur) = self.sidebar.open_row() else {
            return;
        };
        if prev == Some(cur) {
            return;
        }
        let now = Instant::now();
        let key = AnimKey::ListTravel(ListId::Sidebar);
        // An untracked anim has no "current position" to chain from, so
        // the very first move of the session seeds one at `prev`'s row (or
        // `cur`'s, absent a prior selection) before retargeting — a move
        // already in flight instead continues smoothly from wherever it
        // actually is, even if that's not exactly `prev` (e.g. rapid
        // repeats mid-animation).
        if self.anims.value(key, now).is_none() {
            self.anims.snap(key, prev.unwrap_or(cur) as f32);
        }
        // The band crossfades rather than slides: record the row it fades
        // out from (see `Sidebar::band_fade_from`).
        self.sidebar.band_fade_from = prev;
        self.anims
            .retarget(key, cur as f32, self.ui_settings.anim_ms.list_travel, now);
    }

    /// Re-derives the active editor tab from the user's preferred tab and
    /// the current method, through the same commit-and-retarget path a
    /// normal tab switch takes. Called after every path that can change
    /// the method (cycle, dropdown select, request load, undo snapshot):
    /// a preferred Body tab hops to the first tab while the method sends
    /// no body, and comes back the moment it's enabled again — so
    /// switching through a GET request never permanently loses the user's
    /// place (`Editor::preferred_tab`).
    fn sync_active_tab(&mut self) {
        let preferred = self.editor.preferred_tab;
        let target = if preferred == EditorTab::Body && self.editor.body_tab_disabled() {
            EditorTab::from_draw_position(0)
        } else {
            preferred
        };
        if target != self.editor.active_tab {
            self.commit_table_edit();
            let prev = self.editor.active_tab;
            self.editor.active_tab = target;
            self.editor.table.reset();
            self.retarget_editor_tab_underline(prev);
        }
    }

    /// Retargets `AnimKey::TabUnderline`/`TabUnderlineWidth(EditorTabs)`
    /// (Task 10: the strip's independent left/right edges — see the
    /// reinterpretation note on `AnimKey::TabUnderline`) from `prev`'s span
    /// toward the now-active tab's span, over the config-tunable
    /// `ui_settings.anim_ms.tab_slide` (250ms by default) with in-out-cubic
    /// easing. Called from both `Action::EditorTabSelect` and
    /// `Action::EditorTabCycle` — the single place both keyboard cycling
    /// and the click dispatch in `app/mouse.rs` funnel through — right
    /// after `self.editor.active_tab` has already moved to the new tab, so
    /// `prev` is the caller's only way to know where the strip glides from.
    fn retarget_editor_tab_underline(&mut self, prev: EditorTab) {
        let spans = self.editor.tab_strip_spans();
        let now = Instant::now();
        let left_key = AnimKey::TabUnderline(StripId::EditorTabs);
        let right_key = AnimKey::TabUnderlineWidth(StripId::EditorTabs);
        // An untracked pair has no "current position" to chain from (e.g.
        // the very first switch of the session) — seed both edges at
        // `prev`'s span first, exactly like `retarget_sidebar_travel` seeds
        // `ListTravel`; a move already in flight instead continues smoothly
        // from wherever it actually is.
        if self.anims.value(left_key, now).is_none()
            && let Some((x, w)) = spans.get(prev.draw_position())
        {
            self.anims.snap(left_key, *x as f32);
            self.anims.snap(right_key, (*x + *w) as f32);
        }
        let Some((x, w)) = spans.get(self.editor.active_tab.draw_position()) else {
            return;
        };
        let dur = self.ui_settings.anim_ms.tab_slide;
        self.anims
            .retarget_with(left_key, *x as f32, dur, now, Easing::InOutCubic);
        self.anims
            .retarget_with(right_key, (*x + *w) as f32, dur, now, Easing::InOutCubic);
    }

    /// Keeps the editor tab underline pinned to the ACTIVE tab's current
    /// span. A tab switch retargets it explicitly
    /// ([`Self::retarget_editor_tab_underline`]), but the spans also shift
    /// with no switch at all — the Headers/Params/Vars counts differ per
    /// request and change as rows are added, and the Body badge comes and
    /// goes — which used to leave the underline parked at the previous
    /// request's geometry after a request switch. Runs on every `update`
    /// alongside the other sync_* passes; a glide already headed to the
    /// right place is left alone, and an untracked pair simply snaps
    /// (`retarget_with` starts at the target when there's no current
    /// value).
    fn sync_editor_tab_underline(&mut self) {
        let spans = self.editor.tab_strip_spans();
        let Some(&(x, w)) = spans.get(self.editor.active_tab.draw_position()) else {
            return;
        };
        let left_key = AnimKey::TabUnderline(StripId::EditorTabs);
        let right_key = AnimKey::TabUnderlineWidth(StripId::EditorTabs);
        let (left, right) = (x as f32, (x + w) as f32);
        if self.anims.target(left_key) == Some(left) && self.anims.target(right_key) == Some(right)
        {
            return;
        }
        // An untracked pair (nothing has moved the underline yet this
        // session) just snaps into place — a target→target "glide" would
        // read as active for its whole duration and keep forcing redraws.
        if self.anims.target(left_key).is_none() {
            self.anims.snap(left_key, left);
            self.anims.snap(right_key, right);
            return;
        }
        let now = Instant::now();
        let dur = self.ui_settings.anim_ms.tab_slide;
        self.anims
            .retarget_with(left_key, left, dur, now, Easing::InOutCubic);
        self.anims
            .retarget_with(right_key, right, dur, now, Easing::InOutCubic);
    }

    /// The "move to space" choosers' rows: every space but `except`, in
    /// `project.spaces` order, each dispatching `action(space)` on pick.
    fn other_space_items(
        &self,
        except: Option<&str>,
        action: impl Fn(String) -> Action,
    ) -> Vec<crate::components::chooser::ChooserItem> {
        self.project
            .spaces
            .iter()
            .filter(|s| Some(s.as_str()) != except)
            .map(|s| crate::components::chooser::ChooserItem {
                label: self.project.space_name(s),
                detail: None,
                actions: vec![action(s.clone())],
                ..Default::default()
            })
            .collect()
    }

    /// Like [`Self::retarget_editor_tab_underline`], for the Manage
    /// screen's tab strip: called from `Action::SelectManageTab` (clicks
    /// and alt+arrows both funnel through it) and from `OpenManage` when
    /// it switches tabs on an already-open screen, after `manage.tab` has
    /// moved; `prev` is where the glide starts.
    fn retarget_manage_tab_underline(&mut self, prev: crate::components::manage::ManageTab) {
        let spans = crate::components::manage::ManageTab::strip_spans();
        let now = Instant::now();
        let left_key = AnimKey::TabUnderline(StripId::ManageTabs);
        let right_key = AnimKey::TabUnderlineWidth(StripId::ManageTabs);
        if self.anims.value(left_key, now).is_none()
            && let Some((x, w)) = spans.get(prev.index())
        {
            self.anims.snap(left_key, *x as f32);
            self.anims.snap(right_key, (*x + *w) as f32);
        }
        let Some((x, w)) = spans.get(self.manage.tab.index()) else {
            return;
        };
        let dur = self.ui_settings.anim_ms.tab_slide;
        self.anims
            .retarget_with(left_key, *x as f32, dur, now, Easing::InOutCubic);
        self.anims
            .retarget_with(right_key, (*x + *w) as f32, dur, now, Easing::InOutCubic);
    }

    /// Like [`Self::retarget_editor_tab_underline`], but for
    /// `AnimKey::TabUnderline`/`TabUnderlineWidth(ResponseTabs)`, called
    /// from `Action::ResponseViewMode` (the single action both keyboard
    /// switching and `app/mouse.rs`'s click dispatch on `Hit::ResponseTab`
    /// funnel through). `prev_mode` is `None` either pre-response (nothing
    /// to glide from) or on the very first response of a session; either
    /// way the untracked-pair seed below falls back to the newly active
    /// tab's own span, so the strip simply snaps rather than sliding from
    /// nowhere.
    /// Forgets the response tab strip's underline animation, so the next
    /// draw snaps the underline to the active tab's own static span — for
    /// the places the active tab changes *programmatically* (a response
    /// arriving, a background parse concluding not-JSON, a request switch
    /// swapping the pane) where no `Action::ResponseViewMode` runs to
    /// retarget the glide; a stale tracked value would otherwise leave the
    /// underline under the previous response's tab.
    fn reset_response_tab_underline(&mut self) {
        self.anims
            .clear(AnimKey::TabUnderline(StripId::ResponseTabs));
        self.anims
            .clear(AnimKey::TabUnderlineWidth(StripId::ResponseTabs));
    }

    fn retarget_response_tab_underline(&mut self, prev_mode: Option<ViewMode>) {
        let Some(view) = self.session.response.view() else {
            return;
        };
        let (tabs, modes) = crate::components::response::response_tab_defs(
            view,
            self.session.response.jq_bar(),
            &self.theme,
        );
        let spans = crate::paint::TabStrip::spans(&tabs);
        let active_idx = modes.iter().position(|m| *m == view.mode).unwrap_or(0);
        let now = Instant::now();
        let left_key = AnimKey::TabUnderline(StripId::ResponseTabs);
        let right_key = AnimKey::TabUnderlineWidth(StripId::ResponseTabs);
        if self.anims.value(left_key, now).is_none() {
            let prev_idx = prev_mode
                .and_then(|m| modes.iter().position(|mm| *mm == m))
                .unwrap_or(active_idx);
            if let Some((x, w)) = spans.get(prev_idx) {
                self.anims.snap(left_key, *x as f32);
                self.anims.snap(right_key, (*x + *w) as f32);
            }
        }
        let Some((x, w)) = spans.get(active_idx) else {
            return;
        };
        let dur = self.ui_settings.anim_ms.tab_slide;
        self.anims
            .retarget_with(left_key, *x as f32, dur, now, Easing::InOutCubic);
        self.anims
            .retarget_with(right_key, (*x + *w) as f32, dur, now, Easing::InOutCubic);
    }

    /// Sets `sidebar.selected` (the keyboard cursor / menu target) to row
    /// `i`. The selection band and its `ListTravel` anim track the OPEN
    /// request, not this cursor, so no anim bookkeeping happens here — the
    /// cursor's own marker (a steady control fill) just moves on the next
    /// draw.
    fn set_sidebar_selected(&mut self, i: usize) {
        self.sidebar.selected = Some(i);
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
        let mut popped = None;
        if res.close {
            popped = self.modals.pop();
            // Overlay close is always instant.
            self.anims.snap(AnimKey::DropdownOpen, 1.0);
            self.anims.snap(AnimKey::ModalOpen, 1.0);
            // A dropdown that closes without dispatching anything (clicked
            // off, Esc) undoes the sidebar pre-selection its right-click
            // made — the cursor marker must not stay stranded on a row the
            // user never acted on. A chosen item (`actions` non-empty)
            // keeps the moved selection instead: its flow reads it.
            if matches!(popped, Some(Modal::Dropdown(_))) {
                let revert = self.sidebar_menu_revert.take();
                if res.actions.is_empty()
                    && let Some(prev) = revert
                {
                    self.sidebar.selected = prev;
                }
            }
            // Closing the theme picker restores the pre-preview theme; a
            // committed choice (Enter → ApplyTheme in `res.actions`)
            // re-applies over this.
            if matches!(popped, Some(Modal::Chooser(_)))
                && let Some(prior) = self.theme_preview.take()
            {
                self.set_theme_by_name(&prior);
            }
        }
        if let Some(id) = &res.usage {
            self.usage.record(id, crate::usage::now());
        }
        let had_actions = !res.actions.is_empty();
        // A modal these actions push takes over from the one that just
        // closed, so it must not replay the open settle (see
        // `modal_handoff`).
        let outer_handoff = self.modal_handoff;
        self.modal_handoff = res.close && self.modals.is_empty();
        for a in res.actions {
            self.last_action_failed = false;
            changed |= self.update(a);
            if self.last_action_failed {
                break;
            }
        }
        self.modal_handoff = outer_handoff;
        // A refused confirm re-opens the editor it came from, typed state
        // intact — a validation error (name taken, field clash, …) must
        // never cost the user their input. Only text-entry modals come
        // back; a Confirm's choice failing just toasts.
        if had_actions
            && self.last_action_failed
            && matches!(
                popped,
                Some(
                    Modal::Prompt { .. }
                        | Modal::MultiPrompt { .. }
                        | Modal::FieldsEditor(_)
                        | Modal::NewProject { .. }
                )
            )
        {
            // Straight onto the stack, not `push_modal`: the modal never
            // visually left, so re-running the open animation would blink.
            self.modals.push(popped.expect("matched Some above"));
            changed = true;
        }
        changed
    }

    /// Switches the open editor to `target_slug` so an undo/redo step for a
    /// request that isn't the one currently open can proceed — the
    /// `EditorDelta` arm's jump-back handling. On failure
    /// (dirty gate opened, or the open itself failed) it puts `step` back
    /// where its caller popped it from and returns `false`; the caller
    /// must then return `false` without applying anything else.
    fn jump_to_request_for_undo(
        &mut self,
        target_slug: &str,
        redo: bool,
        step: &crate::undo::Step,
        noun: &str,
    ) -> bool {
        // Bypassing the dirty gate is safe only while capture provably
        // missed nothing: the departing editor's state must be exactly
        // what history last saw (the shadow, kept in lockstep with the
        // newest recorded delta). `capture_undo` ran at the top of the
        // Undo/Redo arm, so a mismatch here means a capture bug — degrade
        // to the normal prompt instead of silently dropping edits.
        let shadow_matches = self.shadow.as_ref().is_some_and(|(s, req)| {
            *s == self.editor.slug && *req == self.editor.current_request()
        });
        if self.editor_holds_unsaved() && !shadow_matches {
            // Put the step back on the stack matching its own direction —
            // a tripped guard on a redo must retry as a redo, or Ctrl+Y
            // would silently become an undo.
            if redo {
                self.history.push_redo(step.clone());
                self.dirty_gate("redo", Action::Redo);
            } else {
                self.history.push_undo_no_coalesce(step.clone());
                self.dirty_gate("undo", Action::Undo);
            }
            return false;
        }
        self.apply(Action::ForceOpenRequest(target_slug.to_string()));
        if self.editor.slug.as_deref() != Some(target_slug) {
            // The open failed (file gone/broken — ForceOpenRequest already
            // toasted the reason); drop the step.
            return false;
        }
        self.capture_undo(); // re-seed the shadow for the newly opened request
        let display = self.request_display(target_slug);
        self.toasts.push(
            format!(
                "{} {noun} in {display}",
                if redo { "Redid" } else { "Undid" }
            ),
            ToastKind::Info,
        );
        true
    }

    /// The kind of the step Undo would apply next. Test-only.
    #[cfg(test)]
    pub(crate) fn history_top_kind_for_test(&self) -> Option<&crate::undo::StepKind> {
        self.history.peek_undo().map(|s| &s.kind)
    }

    /// Applies one popped step in the given direction and pushes it onto
    /// the opposite stack. Returns false when the step could not be
    /// applied (it is then dropped — spec: failure handling).
    fn apply_undo_step(&mut self, step: crate::undo::Step, redo: bool) -> bool {
        use crate::undo::StepKind;
        match &step.kind {
            StepKind::EditorDelta {
                slug,
                before,
                after,
            } => {
                if *slug != self.editor.slug {
                    let Some(target_slug) = slug.clone() else {
                        // A scratch request has no slug to reopen; once it's
                        // been replaced its steps are unusable.
                        self.toasts.push(
                            "cannot undo: that unsaved request is gone",
                            ToastKind::Error,
                        );
                        return false;
                    };
                    if !self.jump_to_request_for_undo(&target_slug, redo, &step, "edit") {
                        return false;
                    }
                    // fall through to the normal same-request apply below
                }
                let (target, cursor) = if redo {
                    ((**after).clone(), step.context.cursor_after.clone())
                } else {
                    ((**before).clone(), step.context.cursor_before.clone())
                };
                self.editor.apply_snapshot(&target);
                self.editor.restore_cursor(&cursor);
                self.sync_active_tab();
                self.focus = PaneId::Editor;
                // The applied state IS the new shadow; without this the
                // capture hook would record the undo as a fresh edit.
                self.shadow = Some((self.editor.slug.clone(), target));
                self.shadow_cursor = cursor;
                if redo {
                    self.history.push_undo_no_coalesce(step.clone());
                } else {
                    self.history.push_redo(step.clone());
                }
                true
            }
            StepKind::FileStates {
                before,
                after,
                active_env,
            } => {
                let target = if redo { after } else { before };
                if let Err(msg) = self.write_file_states(target, if redo { "redo" } else { "undo" })
                {
                    self.toasts.push(msg, ToastKind::Error);
                    // Earlier writes in this step stand — the sidebar
                    // and Variable Manager must reflect them (and drop
                    // any stale pre-undo cache) even though the step
                    // itself is dropped, or the UI shows a state that
                    // no longer matches disk.
                    self.project.invalidate_stamps();
                    self.apply(Action::ReloadProjectFiles);
                    self.refresh_sidebar();
                    if self.screen == Screen::Manage {
                        self.varmanager.sync(&self.project);
                    }
                    return false; // step dropped; earlier writes in this step stand
                }
                // Before the `SwitchEnv` below, whose persist would write
                // the stale in-memory table straight back over the
                // `state.toml` these writes just restored.
                self.project.reload_selections_from_disk();
                if let Some((before_env, after_env)) = active_env {
                    let env = if redo { after_env } else { before_env };
                    self.apply(Action::SwitchEnv(env.clone()));
                }
                // Files changed under the app: reuse the wholesale reload +
                // refresh paths rather than guessing what the step touched.
                self.project.invalidate_stamps();
                self.apply(Action::ReloadProjectFiles);
                self.apply(Action::PersistLocalState);
                self.refresh_sidebar();
                // Mirrors `Action::VarStruct`'s success path: the Variable
                // Manager grid/form cache the current declarations and
                // won't otherwise notice a var/env/secrets file an undo or
                // redo just rewrote out from under them.
                if self.screen == Screen::Manage {
                    self.varmanager.sync(&self.project);
                }
                // If the open request's file went absent in this step, it
                // either moved (a rename — another option in the same
                // `target` gained content) or was genuinely deleted. A
                // move retitles in place, following the forward rename's
                // own behavior, rather than closing a still-open editor;
                // only a true delete closes it (mirroring
                // Action::DeleteRequest's own arm).
                if let Some(open) = self.editor.slug.clone() {
                    let open_path = postui_core::storage::request_path(&self.project.root, &open);
                    let went_absent = target.iter().any(|(p, c)| *p == open_path && c.is_none());
                    if went_absent {
                        let moved_to = target
                            .iter()
                            .find(|(p, c)| *p != open_path && c.is_some())
                            .and_then(|(p, _)| {
                                postui_core::storage::slug_for_path(&self.project.root, p)
                            });
                        match moved_to {
                            Some(new_slug) => {
                                self.editor.slug = Some(new_slug.clone());
                                if let Ok(reloaded) = postui_core::storage::load_request(
                                    &self.project.root,
                                    &new_slug,
                                ) {
                                    self.editor.name = reloaded.name.clone();
                                    if let Some(saved) = self.editor.saved.as_mut() {
                                        saved.name = reloaded.name;
                                    }
                                }
                                self.sidebar.open_slug = Some(new_slug.clone());
                                // A move-to-space undo/redo can land the
                                // open request back in a space that isn't
                                // the active one (e.g. undoing a
                                // `MoveRequestToSpace`) — the sidebar is
                                // rooted at the active space, so follow it
                                // there, same as `reload_after_file_change`.
                                if let Some(space) = postui_core::storage::space_of(&new_slug)
                                    .filter(|s| *s != self.project.active_space)
                                {
                                    // As in `reload_after_file_change`:
                                    // the editor already followed, so the
                                    // outgoing space keeps its memory.
                                    self.enter_space(space, SpaceExit::Keep);
                                }
                            }
                            None => {
                                self.editor = Editor::default();
                                self.shadow = None;
                            }
                        }
                    }
                }
                let verb = if redo { "Redid" } else { "Undid" };
                let msg = match &step.context.slug {
                    Some(slug) => format!("{verb} file change to {}", self.request_display(slug)),
                    None => format!("{verb} file change"),
                };
                self.toasts.push(msg, ToastKind::Info);
                if redo {
                    self.history.push_undo_no_coalesce(step.clone());
                } else {
                    self.history.push_redo(step.clone());
                }
                true
            }
            StepKind::Trashed {
                items,
                files_before,
                files_after,
                active_env,
            } => {
                let result: Result<(), String> = if redo {
                    items
                        .iter()
                        .try_for_each(|t| {
                            postui_core::trash::retrash(t).map_err(|e| {
                                format!("redo failed at {}: {e}", t.original.display())
                            })
                        })
                        .and_then(|()| self.write_file_states(files_after, "redo"))
                } else {
                    items
                        .iter()
                        .rev()
                        .try_for_each(|t| {
                            postui_core::trash::restore(t).map_err(|e| {
                                format!("undo failed at {}: {e}", t.original.display())
                            })
                        })
                        .and_then(|()| self.write_file_states(files_before, "undo"))
                };
                if let Err(msg) = result {
                    self.toasts.push(msg, ToastKind::Error);
                    self.reload_after_file_change();
                    return false; // step dropped; earlier renames stand
                }
                // See the `FileStates` arm: `SwitchEnv` persists, so the
                // restored table has to be in memory before it runs.
                self.project.reload_selections_from_disk();
                if let Some((before_env, after_env)) = active_env {
                    let env = if redo { after_env } else { before_env };
                    self.apply(Action::SwitchEnv(env.clone()));
                }
                self.reload_after_file_change();
                self.apply(Action::PersistLocalState);
                let what = items
                    .first()
                    .and_then(|t| t.original.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "delete".into());
                self.toasts.push(
                    if redo {
                        format!("Deleted {what} again")
                    } else {
                        format!("Restored {what}")
                    },
                    ToastKind::Info,
                );
                if redo {
                    self.history.push_undo_no_coalesce(step.clone());
                } else {
                    self.history.push_redo(step.clone());
                }
                true
            }
        }
    }
}

/// The only global actions a modified (ctrl/alt) combo may still trigger
/// while a non-`Main` screen (e.g. the Variable Manager) has captured
/// input: opening a modal on top of the screen (today, just the command
/// palette and the theme chooser — the spec's "the modal stack works on
/// top unchanged"), the
/// screen open/close actions themselves, quit, cycling the active
/// environment (alt+x) — the one Main shortcut whose target state, the
/// active env, is also meaningful inside the Variable Manager (it shows
/// per-env values; `SwitchEnv` re-syncs the Manager) — and the space
/// switchers (ctrl-digits, alt+c / alt+shift+c), since spaces are global
/// context that the Manage screen itself is scoped to. Everything else in
/// the global keymap (send, save, cycle project, focus URL, …) targets
/// panes that aren't even drawn while a non-`Main` screen is open, so it
/// must not be reachable from here — see the Task 9 review finding this
/// whitelist fixes: an unbounded carve-out let ctrl+enter send the loaded
/// request invisibly, alt+u silently reassign focus, etc.
fn screen_escape_whitelist(action: &Action) -> bool {
    matches!(
        action,
        Action::OpenPalette
            | Action::OpenThemeChooser
            | Action::OpenManage { .. }
            | Action::CloseScreen
            | Action::Quit
            | Action::Undo
            | Action::Redo
            | Action::CycleEnv(_)
            | Action::CycleSpace(_)
            | Action::JumpSpace(_)
            | Action::OpenSpaceChooser
    )
}

mod mouse;
#[cfg(test)]
mod tests;
