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
    /// Save the request currently open in the editor. A no-name editor is a
    /// stub until Task 14 adds save-as.
    SaveRequest,
    /// Show the stored parse/read error for a broken sidebar row.
    ShowRequestError(String),
    /// Re-read the project directory and rebuild the sidebar listing.
    RefreshSidebar,
}
