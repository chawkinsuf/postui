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
}
