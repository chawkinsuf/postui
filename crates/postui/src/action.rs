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
}
