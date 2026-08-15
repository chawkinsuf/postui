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
    OpenPalette,
    Close,
    ShowToast(String, ToastKind),
    ShowAbout,
}
