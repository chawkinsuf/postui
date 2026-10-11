use crate::components::toast::ToastKind;
use crate::components::varmanager::{VarEditOp, VarStructOp};
use crate::layout::PaneId;
use indexmap::IndexMap;

/// Where `Action::ConfirmExtractVariable` writes the extracted value (spec
/// §6): the shared declaration's `default`, the active environment's own
/// value, or the open request's own `[variables]` — the three choices the
/// extract prompt's destination field cycles through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractDestination {
    ProjectDefault,
    ActiveEnv,
    Request,
}

/// Where an extract reads the value from — and so what it replaces with
/// the token afterwards: the whole focused field (the palette/row-menu
/// flow) or one surface's selection (the text menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractSource {
    FocusedField,
    Selection(TextSurface),
}

/// One text surface a right-click context menu acts on (see
/// `App::text_surface_menu`): names which selection `Action::CopySelection`
/// reads, so a menu opened over the response pane copies *its* highlight
/// even while the body editor shows one too (the ctrl+c path resolves the
/// same ambiguity by focus priority instead — see
/// `App::active_selection_text`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSurface {
    Url,
    Body,
    Response,
    /// The table cell under edit (`TableEditorState::editing`).
    TableCell,
    /// The variable form's field under edit (`VarFormState::editing`).
    VmField,
    /// The selector-grid cell under edit (`OptionGridState::editing`).
    VmCell,
    /// The response pane's jq filter bar.
    Jq,
    /// The Settings tab's field under edit (`SettingsTab::editing`).
    Settings,
}

/// What [`Action::CopyToClipboard`] copies: the ready response body, one of
/// its headers by index, or the editor's URL as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTarget {
    ResponseBody,
    /// The response pane's active tab, as rendered: the pretty text on
    /// Pretty, the verbatim body on Raw, the header list on Headers — the
    /// toolbar 󰆏 button's target, following the tab like search does.
    ResponseView,
    ResponseHeader(usize),
    Url,
    /// One row of the request Headers tab's computed-headers section, by
    /// index into its own display order (see `Hit::AutoHeaderCopy`).
    ComputedHeader(usize),
    /// One row of the active editor tab's key/value table (Params,
    /// Headers or Vars), by index — the row's own copy button. Copies the
    /// value as typed, not as resolved.
    TableRow(usize),
}

/// Which user-editable config file an Edit…/Reset button targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFile {
    Config,
    Keys,
}

impl ConfigFile {
    pub fn name(self) -> &'static str {
        match self {
            ConfigFile::Config => "config.toml",
            ConfigFile::Keys => "keys.toml",
        }
    }
}

/// The answer to the startup modal raised when `config.toml` exists but
/// will not parse. Startup does not continue into a normal session until
/// one of these is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStartupChoice {
    /// Hand `config.toml` to `$EDITOR` (Task 7's round-trip). A valid
    /// save resumes startup; an invalid one returns to this modal.
    Edit,
    /// Reset the `UiSettings` keys (Task 14). `[projects]` survives only
    /// if the file parses far enough to recover it; the confirm says so
    /// when it does not.
    Reset,
    /// Run on defaults for this session. Every settings write stays
    /// refused, because `Config::edit` will not write an unparseable
    /// file — so nothing persists, including newly-registered projects.
    ContinueUnsaved,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    /// Quit without the unsaved-changes gate — what the gate's own choices
    /// dispatch once the user has decided.
    ForceQuit,
    /// `:q` (spec §5.9): `Quit` behind the unsaved-changes gate whatever
    /// is open under the line, a dialog included. `Quit` skips the gate
    /// with a modal up so ctrl+c stays an emergency exit; the line's is
    /// `:q!` (the final review's I-1).
    GatedQuit,
    /// `ZZ`, `:wq`, `:x` (spec §5.9, §5.11): quit, saving first when the
    /// request has unsaved edits. A never-saved request asks for a name;
    /// a refused or cancelled save does not quit.
    WriteQuit,
    /// `:w!` (`quit` false), and `:wq!` and `:x!` (piece 4 spec §5.9; plan
    /// M5b Deviation 18): `SaveRequest` and `WriteQuit`, but a file changed
    /// outside the app is written over without the drift dialog, as Vim's
    /// `!` writes. A never-saved request asks for a name; a write that
    /// fails does not quit.
    ForceWrite { quit: bool },
    Tick,
    Render,
    FocusNext,
    FocusPrev,
    FocusPane(PaneId),
    /// A vim window key (piece 4 spec §5.8): resolved against the screen,
    /// the focused pane and what `App` remembers. A direction with no pane
    /// does nothing. Typed in an open screen field, the field closes first,
    /// but only when there is a pane to move to: a refused move leaves the
    /// field open (the user's decision, 2026-10-05). On a closed surface
    /// there is no field to close (plan M5a Deviation 16).
    PaneMove(PaneDir),
    ScrollPane(PaneId, i16),
    OpenPalette,
    /// Vim's `:` (spec §5.9): the palette in ex mode, its query read as a
    /// verb against the surface under it (`App::ex_scope`).
    OpenExPalette,
    /// `:noh` (spec §5.9): the body's search highlight goes until the next
    /// search.
    NoHlSearch,
    Close,
    ShowToast(String, ToastKind),
    ShowAbout,
    /// Answers the broken-`config.toml` startup modal.
    ConfigStartupChoice(ConfigStartupChoice),
    /// Hand a config file to `$EDITOR` as a temp copy. Deferred into
    /// `App::pending_terminal_action` like the body editor, because
    /// applying it means suspending the terminal.
    EditConfigFile(ConfigFile),
    /// The editor exited and its text did not validate: raises the
    /// keep-editing/discard modal. `path` is the temp file, still on
    /// disk, so "keep editing" resumes with the user's work intact.
    ConfigEditInvalid {
        file: ConfigFile,
        path: std::path::PathBuf,
        error: String,
    },
    /// Discard the temp file at `path` and change nothing.
    ConfigEditDiscard {
        path: std::path::PathBuf,
    },
    /// User asked to reset `file`'s user-editable settings: raises the
    /// confirm (see `App::apply`). Config changes are outside the undo
    /// system, so that confirm is the only guard.
    ResetConfigFile(ConfigFile),
    /// Confirmed; resets `file`'s user-editable keys and reloads. For
    /// `Config`, removes just the UI keys and so preserves `[projects]`
    /// -- unless `config.toml` does not parse far enough to recover that
    /// table, in which case the confirm has already warned that the whole
    /// file, project list included, is about to be replaced.
    ForceResetConfigFile(ConfigFile),
    /// Persist and apply one boolean setting. There is no save step on
    /// the Settings tab: each of these three writes `config.toml`
    /// through `Config::edit` and applies the result immediately.
    SetUiFlag {
        key: &'static str,
        value: bool,
    },
    /// Persist and apply one string setting. An empty `value` clears the
    /// key back to its default rather than writing an empty string — an
    /// empty `clipboard_cmd` would otherwise swallow every copy.
    SetUiString {
        key: &'static str,
        value: String,
    },
    /// Persist and apply one integer setting.
    SetUiInt {
        key: &'static str,
        value: usize,
    },
    /// Choose the key profile (piece 4 spec §4.2–§4.3): writes `keymap`
    /// to `config.toml`, swaps the live keymap, and is one config undo
    /// step. A write that fails still applies for this session.
    SetKeyProfile(crate::config::KeyProfile),
    EditorTabSelect(usize),
    EditorTabCycle(i8),
    /// The one tab chord every pane shares (alt+←/→ by default): steps
    /// the tab strip of whatever has focus — the request editor's tabs,
    /// the response pane's views, or the Manage screen's strip — by
    /// `delta` enabled tabs, wrapping. Resolved by `App::tab_target`, as
    /// `GoTab` is, to `EditorTabSelect`, `ResponseViewMode` or
    /// `SelectManageTab`; with the sidebar focused it falls back to the
    /// editor's strip, the only one on screen with no pane of its own.
    CycleTabs(i8),
    /// A vim tab key (piece 4 spec §5.8), on the strip `CycleTabs` steps.
    /// A disabled or out-of-range tab, or the tab already showing, does
    /// nothing. With the editor focused, a tab it selects lands the caret
    /// in that tab's content. Typed in an open screen field, the field
    /// closes first, but only when there is a tab to go to: a refused key
    /// leaves the field open (the user's decision, plan M4a), and so does a
    /// close the field itself refuses (plan M5a Deviation 16).
    GoTab { to: TabTo },
    CycleMethod,
    FocusUrl,
    /// Toggles `App::table_collapsed` (params/headers table body vs. tab
    /// strip only). Session-only — never persisted.
    ToggleTableCollapse,
    /// Toggles the Response pane between hidden (collapsed to its header
    /// strip, editor taking the freed rows) and shown. Session-only —
    /// never persisted.
    ToggleResponseCollapse,
    /// One of the split control's five chips was pressed: jumps the
    /// column straight to that settled state
    /// ([`crate::split::SplitState::apply`]) and persists the result as
    /// the project's layout preference.
    SplitStop(crate::split::SplitStop),
    /// Steps the split to the next stop in on-screen order, wrapping —
    /// the keyboard route to the split control: one key (see the footer's
    /// `split` chip) walks the boundary down the column and back around.
    CycleSplit,
    /// Steps the split to the previous stop, wrapping — [`Self::CycleSplit`]
    /// run the other way (shift+alt+w).
    CycleSplitBack,
    /// Nudges the split one stop *without* wrapping — the response
    /// header's ▲ (`+1`, the response grows) and ▼ (`-1`) buttons
    /// ([`crate::split::SplitStop::step`]). A no-op at the endpoint the
    /// arrow points past.
    SplitStep(i8),
    /// Start a new row on the active table tab (Params/Headers/Vars):
    /// focuses the editor's table and begins editing the ghost row's key
    /// cell, exactly like clicking "+ Add …". Inert on the Body tab.
    TableAddRow,
    /// Flip row `i`'s enabled flag on the active params/headers/vars table —
    /// the footer chip twin of the space key and the row's ● toggle button.
    ToggleTableRow(usize),
    /// Delete row `i` from the active params/headers table (the
    /// `d`/`Delete` key or the row's `󰅖` affordance). No confirm — the
    /// deletion is an ordinary editor undo step.
    DeleteTableRow(usize),
    /// `{N}dd` on a table: rows `start..start+count` (clamped) as one
    /// editor undo step (piece 4 spec §5.3).
    DeleteTableRows { start: usize, count: usize },
    /// The row context menu's "Duplicate row" (Task 17, spec §5): copies row
    /// `i` of the active params/headers/vars table to `<key>-copy` (then
    /// `-copy-2`, …, same collision rule as `DuplicateRequest`/`DuplicateVar`)
    /// directly below it, with the same value and enabled flag.
    DuplicateTableRow(usize),
    /// `yy` / `y` on the sidebar: remember this request in `App::yank`.
    /// Invisible: no file, no undo step, no toast (piece 4 spec §5.1).
    YankRequest(String),
    /// `yy` / `y` on a table: remember row `i` of the active tab.
    YankTableRow(usize),
    /// `p` / `P` on the sidebar: duplicate the yanked request beside
    /// `anchor` (the cursor row; `None` falls back to the source's own
    /// slot), below it or `above` it, `count` times as one undo step.
    /// Gated on unsaved edits, once for the whole count: the first copy
    /// opens.
    PutRequest { anchor: Option<String>, above: bool, count: usize },
    /// `p` / `P` on a table: insert `count` copies of the yanked row from
    /// `at` down, as one editor change.
    PutTableRow { at: usize, count: usize },
    /// Open the method-selector dropdown, anchored below the method badge.
    OpenMethodDropdown,
    /// Set the editor's method directly (the dropdown's row action).
    SetMethod(postui_core::model::Method),
    /// Pretty-print the JSON body in place; a no-op on an empty body and a
    /// toast (leaving the buffer untouched) on invalid JSON.
    FormatBody,
    /// Compact the JSON body in place, with the same error handling as
    /// [`Action::FormatBody`].
    MinifyBody,
    /// Empty the request body (undoable like any other edit). A no-op on an
    /// already-empty body.
    BodyClear,
    /// Hand the body off to `$EDITOR`. Deferred by `App::update` into
    /// `App::pending_terminal_action` because applying it means suspending
    /// the terminal, which only the main loop may do.
    OpenBodyInEditor,
    /// User asked to open `slug` (e.g. Enter on a healthy sidebar row). If
    /// the editor is dirty this is intercepted into a `Modal::Confirm`
    /// rather than applied directly; see `App::update`.
    OpenRequest(String),
    /// Actually load `slug` into the editor, bypassing the dirty check
    /// (used directly, or as the tail action of a dirty-prompt choice).
    ForceOpenRequest(String),
    /// Save the request currently open in the editor. A no-name editor
    /// opens the save-as prompt instead of saving directly. When the file
    /// has moved on disk since the editor was seeded from it, this asks
    /// first (the drift confirm) instead of writing.
    SaveRequest,
    /// `SaveRequest` with a follow-on to run once the save actually
    /// lands — the dirty gate's "Save & quit/switch/open". The follow-on
    /// rides the drift confirm's Overwrite choice, so a "Save & quit" that
    /// hits an outside edit still quits after the user chooses.
    SaveRequestThen(Box<Action>),
    /// Write the editor over the open request's file, bypassing the
    /// drift check (used directly, or as the tail action of the drift
    /// confirm's "Overwrite" choice). Not bound to a key or a palette row.
    ForceSaveRequest,
    /// Re-seed the editor from the open request's file, discarding the
    /// buffer — the drift confirm's "Reload from disk" choice.
    ReloadOpenRequest,
    /// Show the stored parse/read error for a broken sidebar row.
    ShowRequestError(String),
    /// Re-read the project directory and rebuild the sidebar listing.
    RefreshSidebar,
    /// Open the "New request" name prompt.
    PromptNewRequest,
    /// Open the "New request" name prompt prefilled with `folder` + `/` —
    /// the folder context menu's "New request here…", so the typed name
    /// lands inside the folder that was right-clicked.
    PromptNewRequestIn(String),
    /// Create a fresh request at `name` (a slug), then open it. If the
    /// editor is dirty this is intercepted into a `Modal::Confirm` rather
    /// than applied directly; see `App::update`.
    CreateRequest(String),
    /// Actually create the request at `name`, bypassing the dirty check
    /// (used directly, or as the tail action of a dirty-prompt choice).
    ForceCreateRequest(String),
    /// Open the rename prompt, prefilled with the selected sidebar slug.
    PromptRenameRequest,
    /// Rename the request at `from` to `to` on disk.
    RenameRequest {
        from: String,
        to: String,
    },
    /// Copy the selected sidebar request to the next free `<slug>-copy…`
    /// name and open the copy.
    DuplicateRequest,
    /// Duplicates `slug` and opens the copy; `DuplicateRequest`'s dirty gate
    /// runs first. `anchor: None` is a plain Duplicate (`DuplicateRequest`):
    /// the copy sits beside its source and no order array is seeded.
    /// `Some(a)` is a put (piece 4 spec §5.2): the copy lands beside `a`
    /// (below, or above with `above`) and a missing order array is seeded,
    /// even when `a` is the source itself. A put makes `count` copies in
    /// one undo step; a plain Duplicate's is 1.
    ForceDuplicateRequest { slug: String, anchor: Option<String>, above: bool, count: usize },
    /// The scratch gate's save path: opens the Save-as name prompt with the
    /// deferred action to run once the save succeeds.
    PromptSaveScratch(Box<Action>),
    /// That prompt confirmed: save under the name, then run the deferred
    /// action — only if the save actually succeeded.
    SaveRequestAsThen(String, Box<Action>),
    /// The `discard` chip / palette / Alt+D: reload the editor from its
    /// saved snapshot. No confirm — the revert is itself an undo step.
    DiscardChanges,
    /// Delete the selected sidebar slug. No confirm — the delete records
    /// an undo step, and the toast advertises it.
    DeleteSelectedRequest,
    /// A list verb on the row under the cursor (piece 4 spec §6.1):
    /// resolved by `App::selection_action` on the surface the keyboard
    /// is on.
    OnSelection(Verb),
    /// Row menu "Move to space…": opens the chooser of the other spaces
    /// for the request at `slug`; picking one dispatches
    /// `MoveRequestToSpace`.
    PromptMoveRequestToSpace(String),
    /// The sidebar's `m` key / footer chip / palette: "Move to space…" for
    /// the selected sidebar request — resolves the selection, then runs
    /// `PromptMoveRequestToSpace`. A no-op when no request is selected.
    PromptMoveSelectedRequestToSpace,
    /// Move the request `slug` `delta` positions among its siblings
    /// (clamped) — alt+up/alt+down and the row menu's Move up/down. Writes
    /// `[space.<slug>] order`. Not an undo step.
    MoveRequest {
        slug: String,
        delta: i32,
    },
    /// `MoveRequest` for the selected sidebar row; a no-op when the
    /// selection is not a request.
    MoveSelectedRequest(i32),
    /// The Spaces tab's "Move all requests…" button: opens the chooser of
    /// the other spaces for the space `from`; picking one dispatches
    /// `MoveAllRequests`.
    PromptMoveAllRequests(String),
    /// Chooser pick after "Move to space…": rename into that space keeping
    /// the sub-path. Gated on unsaved edits when `slug` is the open request
    /// (following it reloads the editor from disk); an undo step.
    MoveRequestToSpace {
        slug: String,
        space: String,
    },
    /// Confirmed/clean: rename the file into `space` keeping its
    /// sub-path; a `FileStates` step; follows the open request.
    ForceMoveRequestToSpace {
        slug: String,
        space: String,
    },
    /// Delete the request at `slug` from disk.
    DeleteRequest(String),
    /// `{N}dd` on the sidebar: these requests as one undo step (piece 4
    /// spec §5.2). Built by `Sidebar::request_slugs_from`.
    DeleteRequests(Vec<String>),
    /// Save the request currently open in the editor as `name` (a slug).
    SaveRequestAs(String),
    /// User asked to send the current request. Validates first: an empty
    /// URL toasts and stops; a non-empty body that fails JSON validation
    /// opens a confirm-anyway modal instead of sending. Otherwise behaves
    /// like `ForceSend`.
    Send,
    /// Actually issue the request, bypassing the body-validity confirm
    /// (used directly, or as the tail action of the confirm modal's "send
    /// anyway" choice).
    ForceSend,
    /// Cancel the in-flight request, if any: aborts its task and marks the
    /// response pane `Cancelled`.
    CancelSend,
    /// Close the open text field keeping its text (spec 2026-09-15). On a
    /// screen field it is what Esc does there; on a dialog it closes the
    /// dialog's field to selected (`ModalStack::close_top_field`), never
    /// through Esc, which may cancel the dialog (plan M3b Deviation 7). The
    /// footer's `esc done` chip and vim's `Leave` rows dispatch it; a no-op
    /// when nothing is open.
    CloseField,
    /// Confirm the top dialog, as its Confirm button does, or press the
    /// aimed button on its button row (spec §5.10: vim's Enter on a
    /// one-field prompt; the prompt's `enter confirm` chip).
    ConfirmTopModal,
    /// Cancel the top dialog (spec §5.10: vim's Esc in a prompt field's idle
    /// Normal; the prompt's `esc cancel` chip).
    CancelTopModal,
    /// Open the top form's selected text field (vim's `i a I A o`, plan M3b
    /// Deviation 3). A no-op on the button row or a stop with no text.
    ReopenField,
    /// Confirmed the send-time secret prompt (spec §3): writes `name`'s
    /// value to `secrets.toml` under the active environment, then re-runs
    /// `Action::ForceSend` on success (prompting for the next missing
    /// secret, or proceeding). A write failure toasts (name only, never the
    /// value) and stops the chain.
    SetSecret {
        name: String,
        value: String,
    },
    /// A background send task completed successfully. `generation` ties the
    /// result back to the send that produced it; stale generations (a newer
    /// send started before this one finished) are dropped.
    ResponseArrived {
        generation: u64,
        data: Box<crate::http::ResponseData>,
    },
    /// A background send task failed. Same staleness handling as
    /// `ResponseArrived`.
    RequestFailed {
        generation: u64,
        error: String,
    },
    /// A background pretty-print finished: the parsed tree for the response
    /// of `generation`, or `None` when that body turned out not to be JSON.
    /// Delivered to whichever response slot is still waiting on it (on
    /// screen or cached); a superseded generation is dropped.
    PrettyParsed {
        generation: u64,
        tree: Option<Box<crate::components::json_tree::JsonTree>>,
    },
    /// The opened root has no `project.toml`; user chose to create one here
    /// (from the "Not a postui project" confirm modal).
    InitProjectHere,
    /// Toggle the currently selected sidebar folder row between collapsed
    /// and expanded.
    ToggleSelectedFolder,
    /// Open the project chooser: every registered project plus a final
    /// "open by path…" option.
    OpenProjectChooser,
    /// Open the theme picker (chooser modal listing the theme registry).
    OpenThemeChooser,
    /// Flip the open theme picker between its dark and light theme sets
    /// (the chooser toggle's action; Left/Right or a click on the label).
    ToggleThemePickerPolarity,
    /// Apply the named theme, persist it as the config `theme` key, and
    /// record it as the session's theme. Dispatched by the picker's Enter.
    ApplyTheme(String),
    /// `alt+z` / `alt+shift+z`: switch to the next (`1`) or previous
    /// (`-1`) registered project, wrapping; toasts "only one project
    /// registered" when there isn't one.
    CycleProject(i32),
    /// User asked to switch to `root`. If the editor is dirty this is
    /// intercepted into a `Modal::Confirm` rather than applied directly
    /// (see `App::dirty_gate`); a no-op when `root` equals the current
    /// project's root.
    SwitchProject(std::path::PathBuf),
    /// Actually switch to `root`, bypassing the dirty check (used directly,
    /// or as the tail action of a dirty-prompt choice).
    ForceSwitchProject(std::path::PathBuf),
    /// Open the folder picker for a project (the Projects chooser's
    /// "open by path…" row and the palette entry); confirming dispatches
    /// `OpenProjectByPath`.
    PromptOpenProjectPath,
    /// The New project modal's Browse button / alt+b: opens the folder
    /// picker on top of it; confirming fills the modal's path field.
    BrowseNewProjectDir,
    /// User typed a path in the open-by-path prompt. An existing project
    /// switches to it directly; anything else asks to create one there.
    OpenProjectByPath(String),
    /// Initialize a new project at `path` (from the open-by-path
    /// create-confirm), then switch to it.
    CreateProjectAt(std::path::PathBuf),
    /// Open the "new project" modal (name + prefilled path).
    PromptNewProject,
    /// User confirmed the new-project modal: create a project named `name`
    /// at `path`, register it, and switch to it (through the dirty gate
    /// when the editor is dirty).
    CreateProject {
        name: String,
        path: String,
    },
    /// Open the environment dropdown (anchored under the header's env
    /// chip): every `project.environments` option plus a "new
    /// environment…" row and a "manage environments…" row. With no
    /// environments the dropdown still opens (just those two rows) — the
    /// create row is the escape hatch from the empty state.
    OpenEnvChooser,
    /// Open the `PromptKind::NewEnvironment` name prompt.
    OpenNewEnvPrompt,
    /// Create an empty `environments/<name>.toml` and switch to it. Toasts
    /// (and changes nothing) on an invalid name or an existing file.
    CreateEnv(String),
    /// `alt+x` / `alt+shift+x`: switch to the next (`1`) or previous
    /// (`-1`) environment, wrapping; never lands on the "no environment"
    /// state (from `None`, starts at the first). Toasts when the project
    /// has no environments.
    CycleEnv(i32),
    /// Switch the active environment to `env` (`None` clears it), reload
    /// its values, persist the choice, and toast the result.
    SwitchEnv(Option<String>),
    /// Open the `PromptKind::RenameEnvironment` prompt, prefilled with the
    /// name.
    PromptRenameEnv(String),
    /// Rename the environment file, re-key its secrets/selections, and —
    /// when it was the active environment — follow it to the new name.
    /// Recorded as a `FileStates` step (env file + secrets file).
    RenameEnv {
        from: String,
        to: String,
    },
    /// Set or clear (`None`) environment `env`'s TLS force
    /// (`[environment.<slug>] tls` in project.toml), which overrides every
    /// request's own `insecure` flag while that environment is active.
    /// Recorded as a `FileStates` step (project.toml).
    SetEnvTls {
        env: String,
        policy: Option<postui_core::project::TlsPolicy>,
    },
    /// Move `name` `delta` positions in the environment list (clamped).
    /// Recorded as a project step; a burst of moves inside the coalesce
    /// window undoes as one.
    MoveEnv {
        name: String,
        delta: i32,
    },
    /// User asked to delete an environment: deletes it at once, no confirm
    /// (it's undoable).
    DeleteEnv(String),
    /// Trashes the environment file (undoable), drops its
    /// secrets/selections and clears the active env when it was this one.
    ForceDeleteEnv(String),
    /// Open the space dropdown: every space (numbered, ✓ on the active
    /// one), then "new space…" and "manage spaces…".
    OpenSpaceChooser,
    /// Open the `PromptKind::NewSpace` name prompt.
    OpenNewSpacePrompt,
    /// Create `requests/<name>/` (+ list entry). Does not switch to it —
    /// the Spaces list's cursor lands on the new row instead, the way
    /// `ForceMoveRequestToSpace` does not follow the request it moves.
    CreateSpace(String),
    /// Open the `PromptKind::RenameSpace` prompt, prefilled with the name.
    PromptRenameSpace(String),
    /// Rename the space on disk and cascade the new name through the
    /// editor's slug, the sidebar and local state.
    RenameSpace {
        from: String,
        to: String,
    },
    /// User asked to delete a space. Deletes at once, with no confirm and
    /// no unsaved-edits gate (OQ4): the step carries the open request's
    /// unsaved buffer, and undo brings the space and the edit back.
    DeleteSpace(String),
    /// Trashes the space's directory (undoable) and drops the list entry.
    ForceDeleteSpace(String),
    /// Move `name` `delta` positions in the space list (clamped).
    /// Recorded as a project step; a burst of moves inside the coalesce
    /// window undoes as one.
    MoveSpace {
        name: String,
        delta: i32,
    },
    /// Move every request in `from` into `to`. Gated on unsaved edits when
    /// the open request lives in `from` (following it reloads the editor
    /// from disk). Not an undo step.
    MoveAllRequests {
        from: String,
        to: String,
    },
    /// Confirmed/clean: moves every request of `from` into `to` and
    /// follows the open request into its new space.
    ForceMoveAllRequests {
        from: String,
        to: String,
    },
    /// User asked to switch spaces. Gated on unsaved edits like
    /// `OpenRequest`; see `App::update`.
    SwitchSpace(String),
    /// Actually switch: remember the outgoing space's open request, root
    /// the sidebar at the new space, restore its last-open request.
    ForceSwitchSpace(String),
    /// `alt+N`: switch to the Nth space (1-based). Out of range: no-op.
    JumpSpace(usize),
    /// `alt+c` / `alt+shift+c`: next/previous space, wrapping.
    CycleSpace(i32),
    /// Re-check the open project's files (project.toml, variables.toml,
    /// environments/, the active env file) against their recorded mtimes
    /// and reload anything that changed. Dispatched on terminal focus
    /// regain and before actions that read project state from a source
    /// that could have changed out from under the app (sending, opening a
    /// chooser). A no-op, redraw-wise, when nothing changed.
    ReloadProjectFiles,
    /// The user-triggered reload (`alt+shift+r`, the palette's "Reload from
    /// disk", the Manage bar's Reload All button): a forced re-read of the
    /// open project's files *and* of the user-editable XDG config files —
    /// `config.toml`, `keys.toml` and `themes/` — applied live. `ui.toml`
    /// is app-owned state and is not re-read. Unsaved editor edits are
    /// never touched: this reloads what is on disk around them, it does
    /// not revert them.
    ReloadFromDisk,
    /// Flip `editor.substitute_body` — whether `{{var}}` tokens in the body
    /// are substituted at send time.
    ToggleBodyVars,
    /// Flip `editor.insecure` — whether TLS certificate verification is
    /// skipped when sending this request.
    ToggleInsecure,
    /// Flip whether the Headers tab's computed-headers section shows
    /// secrets in the clear (`editor.computed.revealed`).
    ToggleHeaderReveal,
    /// Open the variable picker: reload project files, then (barring the
    /// selection-context redirect — see `open_select_picker`) list every
    /// defined name — project variables, selector fields, and the open
    /// request's own `[variables]` — scope-badged, with a "new variable…"
    /// row at the end (Task 15, spec §6). `completing` is true when
    /// triggered by typing `{{` in a text field (Enter inserts just the
    /// closing `name}}`) and false when triggered explicitly (Enter
    /// inserts the full `{{name}}` token). Always opens, even with nothing
    /// declared yet — the "new variable…" row makes it a creation flow
    /// too.
    OpenVarPicker {
        completing: bool,
    },
    /// The value control behind the variable tooltip's `󰏫` edit pill (a
    /// token's own click places the caret in the text): a selector
    /// field opens the `SelectOption` picker (pick an option, every linked
    /// field updates), a secret opens the masked secret prompt, a simple /
    /// request-scoped name opens the value-edit popup (value +
    /// write-destination scope), and an undefined name falls back to the
    /// insert picker seeded with the name (its "new variable…" row is the
    /// create flow).
    OpenVarTokenPopup(String),
    /// The value-edit popup's confirm: write `value` for `name` into the
    /// chosen destination — the request's `[variables]`, the active
    /// environment's flat value, or the declaration default.
    ConfirmEditVarValue {
        name: String,
        value: String,
        destination: ExtractDestination,
    },
    /// The value-edit popup's "Remove" button: delete `name`'s stored
    /// value at `destination` (drop the request `[variables]` entry, the
    /// env flat pair, or the declaration `default`), letting the next
    /// wider scope show through.
    RemoveVarValue {
        name: String,
        destination: ExtractDestination,
    },
    /// Insert `text` at the currently focused text field: the URL line, an
    /// in-progress table cell edit, or the body buffer (Body tab +
    /// content focus). Toasts "nowhere to insert" when focus isn't on a
    /// text field.
    InsertVarText(String),
    /// Copy `surface`'s live selection to the clipboard — the context
    /// menu's "Copy" row (right-click on a text surface). Toasts like the
    /// ctrl+c selection copy; a no-op when nothing is selected there.
    CopySelection(TextSurface),
    /// Paste the OS clipboard's text at the live caret (ctrl+v — GUI
    /// muscle memory; terminal-level pastes arrive as bracketed-paste
    /// events and skip the clipboard read). Routed by `App::paste_text`:
    /// the top modal's focused input, a Variable-Manager field, or the
    /// editor's cell edit / URL bar / body, replacing any live selection.
    Paste,
    /// The Insert-mode picker's "new variable…" row (Task 15): opens the
    /// new-variable prompt pre-filled with the typed filter text.
    /// `completing` carries the picker's own flag through, so confirming
    /// the prompt inserts the same completion/full-token form the picker
    /// would have. Replaces the picker modal (not stacked on top of it),
    /// so focus stays exactly where the picker found it.
    OpenNewVariablePrompt {
        prefill: String,
        completing: bool,
    },
    /// Switch the response pane's view (the tabs row's click target).
    ResponseViewMode(crate::components::response::ViewMode),
    /// Opens the response pane's in-pane search (Task 17, spec §5): the
    /// dispatchable form of the `Find` button / `/` key, so the footer's
    /// Response-pane search chip and the palette can reach it too. Vim's
    /// `?` opens it `backward` (plan M5a Deviation 10).
    OpenResponseSearch { backward: bool },
    /// A vim search key on the response (plan M5a Task 5).
    ResponseFind(ResponseFind),
    /// The response search's next (`1`) or previous (`-1`) match from the
    /// cursor: arrows' `n` and `N` (piece 4 spec §4.5; plan M5a Deviation
    /// 10), as the ▼/▲ buttons do.
    StepResponseSearch(i8),
    /// Arrows' `c` on the response's Headers view: copies the header under
    /// the cursor. Nothing on any other row (spec §4.5).
    CopyResponseHeader,
    /// Click on a JSON-tree body row: moves the cursor there, and — when
    /// `toggle` is set — collapses/expands the container it opens.
    JsonRowClicked {
        row: usize,
        toggle: bool,
    },
    /// `zo` `zc` `za` `zR` `zM` on the response's tree view (piece 4 spec
    /// §5.7): `count` levels, as Vim's counted fold keys. Nothing on Raw or
    /// Headers.
    TreeFold { op: FoldOp, count: usize },
    /// Vim's `zo` `zc` `za` `zR` `zM` on the sidebar's folders (key list §6
    /// `L.fold`; plan M5b Deviation 9): `count` levels, as on the response.
    SidebarFold { op: FoldOp, count: usize },
    /// `.` (key list §6 `L.dot`, spec §5.12; plan M5b Deviations 2 and 5):
    /// the last list change again, where it runs, with this count in place
    /// of its own when one was typed.
    RepeatChange { count: Option<usize> },
    /// A vim cursor or window key on the response (piece 4 spec §5.7; plan
    /// M5a Task 3).
    ResponseMove(ResponseMotion),
    /// A motion in the response's V: the cursor moves and the selection
    /// covers from its anchor to the cursor's line (plan M5a Deviation 11).
    ResponseExtend(ResponseMotion),
    /// Vim's line Visual and yank on the response (plan M5a Task 6).
    ResponseLines(LinesOp),
    /// Copy `target`'s text to the clipboard through `App::clipboard`'s
    /// tiered copy. Toasts "nothing to copy — send a request first" when
    /// there's no ready response and `target` needs one.
    CopyToClipboard(CopyTarget),
    /// Open the "save response body to file" prompt, prefilled with a
    /// `~/Downloads/{slug}-response.{ext}` path. Toasts the same
    /// "nothing to copy" warning with no ready response.
    PromptSaveBody,
    /// Write the ready response body to `path` (from the save-body prompt),
    /// expanding `~` and creating parent directories as needed.
    SaveBodyToFile(String),
    /// Open the response pane's active tab's text in `$EDITOR`, view-only
    /// (the temp file is discarded on exit). Parked in
    /// `App::pending_terminal_action` like `OpenBodyInEditor`, because
    /// applying it means suspending the terminal.
    OpenResponseInEditor,
    /// Like `PromptSaveBody` but for the response pane's active tab (the
    /// toolbar 💾 button): the prefilled extension follows the tab too
    /// (`.txt` on Headers).
    PromptSaveView,
    /// Write the active tab's text (`ReadyView::view_text`) to `path`.
    SaveViewToFile(String),
    /// The file picker's confirmed choice: `target` says who asked (save
    /// body / view, open project, new project's folder) and `path` is the
    /// full file or folder chosen. Save targets ask before overwriting an
    /// existing file.
    PickerConfirm {
        target: crate::components::file_picker::PickerTarget,
        path: std::path::PathBuf,
    },
    /// Open the Manage screen (spec §5): stores the current focus so
    /// `Action::CloseScreen` can restore it, then switches `App::screen`
    /// to `Screen::Manage` on `tab` — `None` meaning the last-used tab.
    /// Toggles the screen closed when it is already open on that tab, so
    /// `alt+r` and the header chip both work as an on/off switch.
    OpenManage {
        tab: Option<crate::components::manage::ManageTab>,
    },
    /// Switch the open Manage screen to another tab (the tab strip's
    /// click target and `alt+←/→`).
    SelectManageTab(crate::components::manage::ManageTab),
    /// Leave the current non-`Main` screen and return to `Screen::Main`,
    /// restoring the focus that was active when the screen was opened.
    CloseScreen,
    /// A committed Variable Manager value edit or ✓ selection (spec §5).
    /// `App::update` writes it through to whichever file owns it; a write
    /// failure toasts and leaves the field it came from untouched, so the
    /// typed text survives a retry.
    VarEdit(VarEditOp),

    // -- Variable Manager structural actions (spec §3.4/§5 action list) --
    /// `n` / the `+ Variable` button: open the new-variable name prompt.
    PromptNewVar,
    /// `a` / the `+ Group` button: open the new-selector prompt (name + a
    /// comma-separated field list).
    PromptNewSelector,
    /// `e`/`F2` on a variable row, or its context menu's "Rename…": open
    /// the rename prompt, prefilled.
    PromptRenameVar {
        from: String,
    },
    /// The left list's context-menu "Duplicate": copies `name`'s
    /// declaration under `<name>-copy` (then `-copy-2`, …). A variable
    /// keeps its description and default; a selector copies its field list
    /// only — options belong to an environment, not to the declaration, and
    /// are not duplicated with it.
    DuplicateVar {
        name: String,
    },
    /// Open the single-name add-field prompt for a selector.
    PromptAddSelectorField {
        selector: String,
    },
    /// Confirmed add-field prompt: append `field` to `selector`'s list.
    /// Toasts (and changes nothing) when it's already a field.
    AddSelectorField {
        selector: String,
        field: String,
    },
    /// `d`/`Delete` on a `SelectorField` row: strip `field`'s per-option values from
    /// every environment file first, then drop it from the selector's list
    /// and shared options in variables.toml (that order keeps each write
    /// valid against the model it's checked with).
    RemoveSelectorField {
        selector: String,
        field: String,
    },
    /// `d`/`Delete` on a left-list row: delete the declaration at once
    /// (undoable); a warning toast lists `varedit::scan_usage`'s
    /// referencing requests.
    DeleteVar {
        name: String,
    },
    /// `s` on a variable row: open the secret-flag transition
    /// confirm (spec §3) — its wording and the value(s) it offers for copy
    /// depend on which direction the flip goes.
    ToggleSecretVar {
        name: String,
    },
    /// Shows or hides the open secret's value on the variable form (the
    /// 👁 control and the form's `r`).
    ToggleVarReveal,
    /// Open the promote-target choice (spec
    /// §4) — declaration default, or the active environment.
    PromptPromoteVar {
        name: String,
    },
    /// A structural mutation; `App::apply_var_struct` applies it.
    VarStruct(VarStructOp),

    // -- Task 16: the selector options grid (spec §3.4) --
    /// The selector pane's `[Edit fields]` button / `m`: opens the field-list
    /// editor — a `Modal::MultiPrompt` with one text slot per current
    /// field, in order, plus a trailing empty "add field" slot.
    PromptGroupFields {
        selector: String,
    },
    /// The field-list editor's confirmed slots, positionally: slot `i` is
    /// `selector`'s current `i`th field (renamed when its text changed,
    /// removed when it was cleared), and any slot past the current list is
    /// a new field. Removals apply at once (undoable) — dropping a field
    /// deletes that column's values from every option, and the toast says
    /// so.
    ApplyGroupFields {
        selector: String,
        slots: Vec<String>,
    },
    /// The option-row context menu's "Rename": starts the inline edit of
    /// row `row`'s name cell — committing a changed name IS the rename
    /// (`VarStructOp::RenameOption`), the same way `Enter` on the cell
    /// works, so there is no rename modal.
    StartOptionNameEdit {
        row: usize,
    },
    /// The option-row context menu's "Delete" / `d` on a grid row: delete
    /// the option at once (undoable).
    DeleteEntry {
        env: String,
        selector: String,
        name: String,
    },
    /// The option row's 󰆏 button / `c` on a grid row: holds the row —
    /// name, description and every field value — in
    /// [`crate::components::VarManager::stash`] so it can be pasted into
    /// another environment. Writes nothing, so it is not an undo step.
    CopyOption {
        row: usize,
    },
    /// The `[+ Option]` button and the grid footer's "new option" chip:
    /// puts the grid cursor in the ghost row's name cell and starts typing
    /// (the `o` key's path). Toasts `NO_ENV_HINT` with no active env.
    StartNewOptionEdit,

    // -- Task 17: in-context flows (spec §6) --
    /// The `SelectOption` picker's "add new option…" ghost row: opens the
    /// name + one-input-per-field prompt for a new option on `owner`,
    /// closing the picker (not stacking on top of it) so focus returns to
    /// the field once the prompt itself confirms or cancels.
    OpenNewOptionInlinePrompt {
        owner: String,
    },
    /// `e` on a highlighted `SelectOption` row: opens the value(s)/
    /// description edit prompt, prefilled from the option's current
    /// content — `values` is `{"value": ...}` for a plain variable option,
    /// or one option per field for a selector option.
    OpenEditOptionPrompt {
        owner: String,
        key: String,
        description: Option<String>,
        values: IndexMap<String, String>,
    },
    /// Confirmed `PromptKind::NewOptionInline`: writes `key` with one
    /// value per selector field (`values`) and `description` to the ACTIVE
    /// environment's `[options.owner.key]` table (spec §1's merge rule
    /// makes it an env-specific addition) and selects it immediately.
    ConfirmNewOptionInline {
        owner: String,
        key: String,
        values: IndexMap<String, String>,
        description: Option<String>,
    },
    /// Confirmed `PromptKind::EditOption`: writes `values`/`description` to
    /// wherever `key` currently lives — the active env's override if it has
    /// one, `variables.toml`'s shared declaration otherwise.
    ConfirmEditOption {
        owner: String,
        key: String,
        values: IndexMap<String, String>,
        description: Option<String>,
    },
    /// Focused a line-input field or table cell with literal text and asked
    /// to extract it to a variable (palette + `ctrl+shift+e`): opens the
    /// name/destination prompt. Refused with a toast when focus is
    /// elsewhere, the field is empty, or the cursor is in the body (no body
    /// text selection yet this stage).
    ExtractToVariable,
    /// Confirmed `PromptKind::ExtractVariable`: declares/writes `name` at
    /// `destination`, then replaces the still-focused field's text with
    /// `{{name}}` — the origin field is re-read from current focus rather
    /// than carried in the prompt, since focus can't move while a modal has
    /// input captured. For `Request`, the request file is saved
    /// synchronously afterward (finding 2's ruling); the other two
    /// destinations leave the field edit save-on-demand, same as ordinary
    /// typing.
    ConfirmExtractVariable {
        name: String,
        destination: ExtractDestination,
    },
    /// The text menu's "Extract to variable…" (right-click on a text
    /// surface with a selection): opens the same name/destination prompt
    /// as `ExtractToVariable`, for the selected text of `surface` only.
    /// Refused with a toast when nothing is selected there.
    ExtractSelection(TextSurface),
    /// Confirmed `PromptKind::ExtractSelection`: like
    /// `ConfirmExtractVariable`, but the value is `surface`'s live
    /// selection (re-read at confirm time — the selection can't change
    /// while the modal has input captured) and only that selected range
    /// is replaced with `{{name}}`, the rest of the field kept. The
    /// response pane is read-only, so there the variable is created and
    /// nothing is replaced.
    ConfirmExtractSelection {
        name: String,
        destination: ExtractDestination,
        surface: TextSurface,
    },
    /// Palette "Extract to selector" / the row menu's "Extract value to
    /// selector…": like `ExtractToVariable`, but the prompt asks for a
    /// selector name, an option name and a scope, and confirming creates a
    /// new one-field selector whose only option holds the field's text.
    ExtractToSelector,
    /// The text menu's "Extract to selector…": the same prompt for the
    /// selected text of `surface` only (see `ExtractSelection`).
    ExtractSelectionToSelector(TextSurface),
    /// Confirmed `PromptKind::ExtractSelector`: declares selector `name`
    /// with the single field `name` (`shared` picks where its options
    /// live), adds option `option` with that field set to the extracted
    /// text, selects it in the active environment, and swaps the source
    /// text for `{{name}}` as the variable extract does.
    ConfirmExtractToSelector {
        name: String,
        option: String,
        shared: bool,
        source: ExtractSource,
    },

    // -- Stage 7: variable-format migration (spec §3.3) --
    /// The migration confirm modal's "Migrate": rewrites `variables.toml`
    /// and the environment files into the new format, leaving a `.bak`
    /// beside each one, then reloads the project.
    ApplyMigration,
    /// The migration confirm modal's "Not now": leaves the files as they
    /// are; the project stays open with its variables inert.
    DeclineMigration,

    // -- Undo/redo (spec: docs/superpowers/specs/2026-08-24-undo-redo-design.md) --
    /// Undo the last recorded change, wherever it happened. Inert while a
    /// modal is open.
    Undo,
    /// Re-apply the most recently undone change. Same modality rules as
    /// [`Action::Undo`].
    Redo,

    // -- jq response filter (spec: docs/superpowers/sdd/2026-09-03-jq-response-filter) --
    /// alt+shift+q / the header 󰈲 button: the filter's switch — opens a
    /// closed jq bar (filter on, focused) or closes an open one (filter
    /// off, text kept) whether or not the caret is in it.
    ToggleJqBar,
    /// alt+q / the palette / the footer's `filter` chip: focus the bar,
    /// switching it on if it was off. Never blurs and never switches off
    /// — alt+q always means "type a filter", so the switch is a different
    /// key.
    OpenJqBar,
    /// Replaces the bar text and applies it (structural menu "apply" items).
    JqApply(String),
    /// Replaces the bar text, focuses it with the cursor at `cursor`, runs
    /// nothing yet.
    JqTeeUp {
        text: String,
        cursor: usize,
    },
    /// A background run finished for the response of `generation`, run
    /// `run`.
    JqRunFinished {
        generation: u64,
        run: u64,
        result: Result<crate::components::response::JqRunOutput, postui_core::jq::JqError>,
    },
    /// A background completion key fetch finished for the response of
    /// `generation`, fetch `seq` (`Response::attach_jq_completion`).
    JqCompleteFinished {
        generation: u64,
        seq: u64,
        input_expr: String,
        keys: Vec<String>,
        /// The document the worker parsed when the view had none cached.
        doc: Option<postui_core::jq::JqDocument>,
    },
    /// Copies a tree line's jq path.
    CopyJqPath(String),
    /// Esc in the jq bar while an AI request is pending, or a new send:
    /// cancels the outstanding AI task and clears `ai_pending`.
    CancelJqDescribe,
    /// Palette/footer/AI button: opens the "Describe a filter" prompt
    /// (gated on the response being JSON and the configured `ai_cmd`'s
    /// program being found on PATH).
    OpenJqDescribe,
    /// The describe prompt's Enter: asks for confirmation before the first
    /// AI call ever made (`ui_settings.ai_confirmed`), then runs it.
    ConfirmJqDescribe(String),
    /// Sends `sentence` to the configured AI command with the response's
    /// shape and current filter on stdin; lands the reply (or its error)
    /// asynchronously as `JqAiFinished`.
    RunJqDescribe(String),
    /// The confirm modal's "Always send": persists `ai_confirmed = true`
    /// to config.toml so later requests skip the confirmation.
    SetAiConfirmed,
    /// A `RunJqDescribe` background task finished: `generation` and
    /// `request` must both still match the current view/task, or the
    /// result is dropped (a superseded response, or a cancelled request).
    JqAiFinished {
        generation: u64,
        request: u64,
        result: Result<String, String>,
    },
    /// Structural menu's "Pluck field…" on an array: opens a chooser of
    /// `keys`; picking `k` applies `compose(bar, path, "map(.k)")`.
    JqPluckPrompt {
        path: String,
        keys: Vec<String>,
    },
    /// Structural menu's "Where field…" on an array: opens a chooser of
    /// `keys`; picking `k` tees up `compose(bar, path, "map(select(.k == ))")`
    /// with the cursor before the closing `)`.
    JqWherePrompt {
        path: String,
        keys: Vec<String>,
    },
    /// Structural menu's "Collect into array": wraps the bar text in `[ … ]`.
    JqCollect,
}

/// Where `zt` `zz` `zb` put the cursor's line (piece 4 spec §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Top,
    Middle,
    Bottom,
}

/// A cursor or window command on the response viewer (piece 4 spec §5.7;
/// plan M5a Task 3): each is Vim's, on a window of the visible lines
/// (`vim::engine::View`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMotion {
    /// `j` `k` `<Down>` `<Up>`: this many lines, down when positive.
    Lines(i64),
    /// `gg` (`last` false), `G` and ctrl+Home: line N (1-based) when
    /// counted, else the first or the last line, on its first non-blank.
    Goto { line: Option<usize>, last: bool },
    /// ctrl+End: line N when counted, else the last line, on its last char.
    GotoEnd { line: Option<usize> },
    /// Enter (Vim's `+`): this many lines down, on the first non-blank.
    NextLine(usize),
    /// A move along the line, of Vim's hidden column (the response draws no
    /// caret): `h` `l` `0` `$` and their named keys, and Space.
    Col(ColMove),
    /// ctrl+d and ctrl+u: Vim's `halfpage()`; a count sets the view's
    /// `'scroll'`.
    HalfPage { down: bool, count: Option<usize> },
    /// ctrl+f and ctrl+b, PageDown and PageUp: Vim's `onepage()`, `count`
    /// pages.
    Page { down: bool, count: usize },
    /// `zt` `zz` `zb`: line N when counted (the cursor goes there), then
    /// the window placed around the cursor's line.
    Place { at: Place, line: Option<usize> },
}

/// A move along the response's line ([`ResponseMotion::Col`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColMove {
    /// `h` `<Left>`: this many chars left.
    Left(usize),
    /// `l` `<Right>`: this many chars right, not past the line's end.
    Right(usize),
    /// Space: this many chars right, on to the next line's start at a
    /// line's end (Vim's `whichwrap` `s`).
    Space(usize),
    /// `0` `<Home>`: the line's first char.
    Start,
    /// `$` `<End>`: the last char of the line this many lines down, less
    /// one, wanted on every line after.
    End(usize),
}

/// A vim search key on the response (piece 4 spec §5.7; plan M5a Task 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFind {
    /// `n` (`reverse` false) and `N`, counted: Vim's repeat, `N` against
    /// the last search's direction.
    Next { reverse: bool, count: usize },
    /// `*` (`backward` false) and `#`, counted: Vim's word under the cursor.
    Word { backward: bool, count: usize },
}

/// Vim's line Visual and yank on the response (plan M5a Task 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinesOp {
    /// `V`: a line selection on the cursor's line.
    Start,
    /// `V` again, or Esc: the selection ends, the cursor where it is.
    End,
    /// `o`: the cursor goes to the other end.
    Swap,
    /// `yy` (`count` lines from the cursor) or a V `y` (the selection):
    /// into the unnamed register, linewise.
    Yank { count: Option<usize> },
    /// `zo` (`open`) and `zc` in V: one level of folds over the selected
    /// lines, then the selection ends, as Vim's Visual `zo` `zc` do.
    Fold { open: bool },
}

/// Vim's fold keys on the response tree (piece 4 spec §5.7; plan M5a
/// Deviation 9): `zo` opens, `zc` closes, `za` toggles the folds at the
/// cursor's line; `zR` and `zM` open and close every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldOp {
    Open,
    Close,
    Toggle,
    OpenAll,
    CloseAll,
}

impl FoldOp {
    /// Vim's pick for `zo` `zc` `za` with a count, one rule for the
    /// response tree (`JsonTree::fold_at`, plan M5a Deviation 9) and the
    /// sidebar (`Sidebar::fold_change`, plan M5b Deviation 9). `closed` is
    /// the chain of folds holding the cursor's line, outermost first,
    /// `true` where closed. `zo`: the `count` outermost closed folds open.
    /// `zc`: the `count` innermost open folds outside the outermost closed
    /// one close, so `zc` on a fold already closed closes the one around
    /// it. `za`: on a closed fold as `zo`; on an open one as `zc` (Vim 9.1:
    /// `2za` closes two levels). Whether the picked folds open, and their
    /// indices in `closed`. `None` for `zR` and `zM`, which act on every
    /// fold, not on a chain.
    pub fn pick(self, closed: &[bool], count: usize) -> Option<(bool, Vec<usize>)> {
        let closed_at = closed.iter().position(|&c| c);
        let open = match self {
            FoldOp::Open => true,
            FoldOp::Close => false,
            FoldOp::Toggle => closed_at.is_some(),
            FoldOp::OpenAll | FoldOp::CloseAll => return None,
        };
        let n = count.max(1);
        let pick = if open {
            closed.iter().enumerate().filter(|&(_, &c)| c).map(|(i, _)| i).take(n).collect()
        } else {
            let k = closed_at.unwrap_or(closed.len());
            (k.saturating_sub(n)..k).collect()
        };
        Some((open, pick))
    }
}

/// Where `ctrl+w` moves the keyboard (piece 4 spec §5.8): `h` `j` `k` `l`
/// by the layout's geometry, `w` `W` the next and previous pane, `p` the
/// pane before this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDir {
    Left,
    Down,
    Up,
    Right,
    Next,
    Prev,
    Last,
}

/// Which tab a vim tab key goes to (piece 4 spec §5.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTo {
    /// `{N}gt`: tab N, 1-based in on-screen order.
    Nth(usize),
    /// `gt`, `gT` and `{N}gT`: this many enabled tabs along, wrapping.
    By(i64),
}

/// What a list or table key does to the row under the cursor (piece 4
/// spec §6.1). `App::selection_action` resolves it against the surface
/// to the parameterized action that surface already has; a verb with no
/// meaning there resolves to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// `a`: a new row of the surface's kind.
    Add,
    /// `A`: the other kind (the Variables list: a selector).
    AddAlt,
    Rename,
    /// `m`: move (a request to a space; a space's requests to another).
    Move,
    /// `dd` / `d`; `{N}dd` deletes N rows where that is one undo step.
    Delete { count: usize },
    /// `yy` / `y`: into the row register (the grid: its option stash).
    Yank,
    /// `p` / `P`: a copy of the register below, or `above`, the cursor;
    /// `{N}p` puts N copies as one undo step, as vim's `3p` does.
    Put { above: bool, count: usize },
    /// The options grid's Edit… prompt.
    EditValue,
    Promote,
    /// A selector's fields editor.
    Fields,
    Secret,
    /// Clear the active environment's value.
    ClearValue,
    /// Show or hide a secret's value.
    Reveal,
    /// Step an environment's TLS force.
    Tls,
}

impl Verb {
    /// Every verb, once (a count does not change what a verb reaches): the
    /// reachability test and the `:` line's scope walk it.
    pub const ALL: [Verb; 14] = [
        Verb::Add,
        Verb::AddAlt,
        Verb::Rename,
        Verb::Move,
        Verb::Delete { count: 1 },
        Verb::Yank,
        Verb::Put { above: false, count: 1 },
        Verb::EditValue,
        Verb::Promote,
        Verb::Fields,
        Verb::Secret,
        Verb::ClearValue,
        Verb::Reveal,
        Verb::Tls,
    ];

    /// The same verb whatever its count: `dd` and `3dd` spell alike.
    pub fn kind(self) -> Self {
        self.with_count(1)
    }

    /// This verb with the count a row passes (spec §3.5 `Pass`).
    pub fn with_count(self, n: usize) -> Self {
        match self {
            Verb::Delete { .. } => Verb::Delete { count: n },
            Verb::Put { above, .. } => Verb::Put { above, count: n },
            v => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Verb;

    /// `Verb::ALL` lists every verb once, in declaration order. `slot` has
    /// no wildcard arm, so a new verb stops this compiling until it gets a
    /// slot here and its place in `ALL`; the `:` line's scope and the
    /// reachability test only see verbs `ALL` lists.
    #[test]
    fn verb_all_lists_every_verb() {
        fn slot(v: Verb) -> usize {
            match v {
                Verb::Add => 0,
                Verb::AddAlt => 1,
                Verb::Rename => 2,
                Verb::Move => 3,
                Verb::Delete { .. } => 4,
                Verb::Yank => 5,
                Verb::Put { .. } => 6,
                Verb::EditValue => 7,
                Verb::Promote => 8,
                Verb::Fields => 9,
                Verb::Secret => 10,
                Verb::ClearValue => 11,
                Verb::Reveal => 12,
                Verb::Tls => 13,
            }
        }
        for (i, v) in Verb::ALL.into_iter().enumerate() {
            assert_eq!(slot(v), i, "{v:?}");
        }
    }
}
