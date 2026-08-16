use crate::components::toast::ToastKind;
use crate::layout::PaneId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
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
    /// Create a fresh request at `name` (a slug), then open it.
    CreateRequest(String),
    /// Open the rename prompt, prefilled with the selected sidebar slug.
    PromptRenameRequest,
    /// Rename the request at `from` to `to` on disk.
    RenameRequest {
        from: String,
        to: String,
    },
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
}
