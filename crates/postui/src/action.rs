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

/// What [`Action::CopyToClipboard`] copies: the ready response body, one of
/// its headers by index, or the editor's URL as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTarget {
    ResponseBody,
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
    /// Open the delete confirmation for row `i` of the active params/headers
    /// table (from the `d`/`Delete` key or the row's `✕` affordance).
    ConfirmDeleteTableRow(usize),
    /// Actually delete row `i` from the active params/headers table (the
    /// confirm modal's "Delete" choice).
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
    /// Open the delete confirmation for the selected sidebar slug.
    ConfirmDeleteRequest,
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
    /// "open by path…" entry.
    OpenProjectChooser,
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
    /// Open the environment chooser: every `project.environments` entry
    /// plus a final "no environment" entry and a "new environment…" entry.
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
    /// Flip whether the Headers tab's computed-headers section shows
    /// secrets in the clear (`editor.computed.revealed`).
    ToggleHeaderReveal,
    /// Open the variable picker: reload project files, then (barring the
    /// selection-context redirect — see `open_select_picker`) list every
    /// defined name — project variables, group members, and the open
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
    /// Opens the same insert picker with its filter pre-seeded to a
    /// variable name — what clicking an inline `{{token}}` does (spec §7),
    /// so the picker comes up already narrowed to the token clicked.
    OpenVarPickerFor(String),
    /// Insert `text` at the currently focused text field: the URL line, an
    /// in-progress table cell edit, or the body buffer (Body tab +
    /// content focus). Toasts "nowhere to insert" when focus isn't on a
    /// text field.
    InsertVarText(String),
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
    /// dispatchable form of the `⌕` button / `/` key, so the footer's
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
    /// Open the Variable Manager screen (spec §5): stores the current
    /// focus so `Action::CloseScreen` can restore it, then switches
    /// `App::screen` to `Screen::VarManager`. A no-op when the Manager is
    /// already open.
    OpenVarManager,
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
    /// `g` / the `+ Group` button: open the new-group prompt (name + a
    /// comma-separated field list).
    PromptNewGroup,
    /// `e`/`F2` on a variable row, or its context menu's "Rename…": open
    /// the rename prompt, prefilled.
    PromptRenameVar {
        from: String,
    },
    /// The left list's context-menu "Duplicate": copies `name`'s
    /// declaration under `<name>-copy` (then `-copy-2`, …). A variable
    /// keeps its description and default; a group copies its field list
    /// only — entries belong to an environment, not to the declaration, and
    /// are not duplicated with it.
    DuplicateVar {
        name: String,
    },
    /// Open the group's field-list prompt, prefilled.
    PromptEditGroupMembers {
        group: String,
    },
    /// Open the single-name add-field prompt for a group.
    PromptAddGroupMember {
        group: String,
    },
    /// Confirmed add-member prompt: append `member` to `group`'s list.
    /// Toasts (and changes nothing) when it's already a member.
    AddGroupMember {
        group: String,
        member: String,
    },
    /// `d`/`Delete` on a `GroupMember` row: open the remove confirm.
    ConfirmRemoveGroupMember {
        group: String,
        member: String,
    },
    /// Confirmed member removal: strip `member`'s per-option values from
    /// every environment file first, then drop it from the group's list
    /// and shared options in variables.toml (that order keeps each write
    /// valid against the model it's checked with).
    RemoveGroupMember {
        group: String,
        member: String,
    },
    /// `d`/`Delete` on a left-list row: open the delete confirm, its body
    /// listing `varedit::scan_usage`'s referencing requests.
    ConfirmDeleteVar {
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
    /// Open the demote confirm (spec §4), or a
    /// message modal refusing it (a secret or a group).
    ConfirmDemoteVar {
        name: String,
    },
    /// A confirmed structural mutation; `App::apply_var_struct` applies it.
    VarStruct(VarStructOp),

    // -- Task 16: the group entries grid (spec §3.4) --
    /// The group pane's `[Edit fields]` button / `m`: opens the field-list
    /// editor — a `Modal::MultiPrompt` with one text slot per current
    /// field, in order, plus a trailing empty "add field" slot.
    PromptGroupFields {
        group: String,
    },
    /// The field-list editor's confirmed slots, positionally: slot `i` is
    /// `group`'s current `i`th field (renamed when its text changed,
    /// removed when it was cleared), and any slot past the current list is
    /// a new field. `confirmed` is false on the way in; a removal bounces
    /// through a confirm modal that re-dispatches this with it set, since
    /// dropping a field deletes that column's values from every entry.
    ApplyGroupFields {
        group: String,
        slots: Vec<String>,
        confirmed: bool,
    },
    /// The entry-row context menu's "Rename…": opens a prompt seeded with
    /// the entry's current name.
    PromptRenameEntry {
        env: String,
        group: String,
        from: String,
    },
    /// The entry-row context menu's "Delete…": opens the delete confirm.
    ConfirmDeleteEntry {
        env: String,
        group: String,
        name: String,
    },

    // -- Task 17: in-context flows (spec §6) --
    /// The `SelectOption` picker's "add new option…" ghost row: opens the
    /// key/value/description prompt for a new option on `owner`, closing
    /// the picker (not stacking on top of it) so focus returns to the
    /// field once the prompt itself confirms or cancels.
    OpenNewOptionInlinePrompt {
        owner: String,
    },
    /// `e` on a highlighted `SelectOption` row: opens the value(s)/
    /// description edit prompt, prefilled from the option's current
    /// content — `values` is `{"value": ...}` for a plain variable option,
    /// or one entry per member for a group option.
    OpenEditOptionPrompt {
        owner: String,
        key: String,
        description: Option<String>,
        values: IndexMap<String, String>,
    },
    /// Confirmed `PromptKind::NewOptionInline`: writes `key`/`value`/
    /// `description` to the ACTIVE environment's `[options.owner.key]`
    /// table (spec §1's merge rule makes it an env-specific addition) and
    /// selects it immediately.
    ConfirmNewOptionInline {
        owner: String,
        key: String,
        value: String,
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

    // -- Stage 7: variable-format migration (spec §3.3) --
    /// The migration confirm modal's "Migrate": rewrites `variables.toml`
    /// and the environment files into the new format, leaving a `.bak`
    /// beside each one, then reloads the project.
    ApplyMigration,
    /// The migration confirm modal's "Not now": leaves the files as they
    /// are; the project stays open with its variables inert.
    DeclineMigration,
}
