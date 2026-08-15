use crate::components::toast::ToastKind;
use crate::layout::PaneId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Tick,
    #[allow(dead_code)]
    Render,
    FocusNext,
    FocusPrev,
    FocusPane(PaneId),
    OpenPalette,
    Close,
    #[allow(dead_code)]
    ShowToast(String, ToastKind),
    ShowAbout,
}
