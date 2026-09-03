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
}

/// What [`Action::CopyToClipboard`] copies: the ready response body, one of
/// its headers by index, or the editor's URL as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTarget {
    ResponseBody,
    /// The response pane's active tab, as rendered: the pretty text on
    /// Pretty, the verbatim body on Raw, the header list on Headers — the
    /// toolbar ❐ button's target, following the tab like search does.
    ResponseView,
    ResponseHeader(usize),
    Url,
    /// One row of the request Headers tab's computed-headers section, by
    /// index into its own display order (see `Hit::AutoHeaderCopy`).
    ComputedHeader(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    /// Quit without the unsaved-changes gate — what the gate's own choices
    /// dispatch once the user has decided.
    ForceQuit,
    Tick,
    Render,
    FocusNext,
    FocusPrev,
    FocusPane(PaneId),
    ScrollPane(PaneId, i16),
    OpenPalette,
    Close,
    ShowToast(String, ToastKind),
    ShowAbout,
    EditorTabSelect(usize),
    EditorTabCycle(i8),
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
    /// `d`/`Delete` key or the row's `✕` affordance). No confirm — the
    /// deletion is an ordinary editor undo step.
    DeleteTableRow(usize),
    /// The row context menu's "Duplicate row" (Task 17, spec §5): copies row
    /// `i` of the active params/headers/vars table to `<key>-copy` (then
    /// `-copy-2`, …, same collision rule as `DuplicateRequest`/`DuplicateVar`)
    /// directly below it, with the same value and enabled flag.
    DuplicateTableRow(usize),
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
    /// opens the save-as prompt instead of saving directly.
    SaveRequest,
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
    /// Create a fresh request at `name` (a slug), then open it.
    CreateRequest(String),
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
    /// Row menu "Move to space…": opens the chooser of the other spaces
    /// for the request at `slug`; picking one dispatches
    /// `MoveRequestToSpace`.
    PromptMoveRequestToSpace(String),
    /// The sidebar's `m` key / footer chip / palette: "Move to space…" for
    /// the selected sidebar request — resolves the selection, then runs
    /// `PromptMoveRequestToSpace`. A no-op when no request is selected.
    PromptMoveSelectedRequestToSpace,
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
    /// Write `.local/state.toml` (expanded folders, active environment, the
    /// currently open request) from current app state. Fired after any
    /// change to that state and on quit.
    PersistLocalState,
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
    /// Switch to the next registered project after the current one,
    /// wrapping; toasts "only one project registered" when there isn't one.
    CycleProject,
    /// User asked to switch to `root`. If the editor is dirty this is
    /// intercepted into a `Modal::Confirm` rather than applied directly
    /// (see `App::dirty_gate`); a no-op when `root` equals the current
    /// project's root.
    SwitchProject(std::path::PathBuf),
    /// Actually switch to `root`, bypassing the dirty check (used directly,
    /// or as the tail action of a dirty-prompt choice).
    ForceSwitchProject(std::path::PathBuf),
    /// Open the "open project by path" text prompt.
    PromptOpenProjectPath,
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
    /// Open the environment chooser: every `project.environments` option
    /// plus a final "no environment" option and a "new environment…" option.
    /// With no environments the chooser still opens (just those two rows) —
    /// the create row is the escape hatch from the empty state.
    OpenEnvChooser,
    /// Open the `PromptKind::NewEnvironment` name prompt.
    OpenNewEnvPrompt,
    /// Create an empty `environments/<name>.toml` and switch to it. Toasts
    /// (and changes nothing) on an invalid name or an existing file.
    CreateEnv(String),
    /// Switch to the next environment after the active one, wrapping;
    /// skips the "no environment" state (from `None`, starts at the
    /// first). Toasts when the project has no environments.
    CycleEnv,
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
    /// User asked to delete an environment: opens the confirm.
    DeleteEnv(String),
    /// Confirmed; trashes the environment file (undoable), drops its
    /// secrets/selections and clears the active env when it was this one.
    ForceDeleteEnv(String),
    /// Open the space dropdown: every space (numbered, ✓ on the active
    /// one), then "new space…" and "manage spaces…".
    OpenSpaceChooser,
    /// Open the `PromptKind::NewSpace` name prompt.
    OpenNewSpacePrompt,
    /// Create `requests/<name>/` (+ list entry) and switch to it.
    CreateSpace(String),
    /// Open the `PromptKind::RenameSpace` prompt, prefilled with the name.
    PromptRenameSpace(String),
    /// Rename the space on disk and cascade the new name through the
    /// editor's slug, the sidebar and local state.
    RenameSpace {
        from: String,
        to: String,
    },
    /// User asked to delete a space. Gated on unsaved edits when the open
    /// request lives there; otherwise goes straight to the confirm.
    DeleteSpace(String),
    /// The delete confirm, whose body/label carry the request count.
    PromptDeleteSpace(String),
    /// Confirmed; trashes the space's directory (undoable) and drops the
    /// list entry.
    ForceDeleteSpace(String),
    /// Move `name` `delta` positions in the space list (clamped). Not an
    /// undo step.
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
    /// `alt+l` / `alt+shift+l`: next/previous space, wrapping.
    CycleSpace(i32),
    /// Re-check the open project's files (project.toml, variables.toml,
    /// environments/, the active env file) against their recorded mtimes
    /// and reload anything that changed. Dispatched on terminal focus
    /// regain and before actions that read project state from a source
    /// that could have changed out from under the app (sending, opening a
    /// chooser). A no-op, redraw-wise, when nothing changed.
    ReloadProjectFiles,
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
    /// The value control behind clicking an inline `{{token}}`: a selector
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
    /// Response-pane search chip and the palette can reach it too.
    OpenResponseSearch,
    /// Click on a JSON-tree body row: moves the cursor there, and — when
    /// `toggle` is set — collapses/expands the container it opens.
    JsonRowClicked {
        row: usize,
        toggle: bool,
    },
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
    /// Open the Manage screen (spec §5): stores the current focus so
    /// `Action::CloseScreen` can restore it, then switches `App::screen`
    /// to `Screen::Manage` on `tab` — `None` meaning the last-used tab.
    /// Toggles the screen closed when it is already open on that tab, so
    /// `alt+v` and the header chip both work as an on/off switch.
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
    /// `g` / the `+ Group` button: open the new-selector prompt (name + a
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
}
