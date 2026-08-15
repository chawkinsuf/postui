#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Tick,
    #[allow(dead_code)]
    Render,
    FocusNext,
    FocusPrev,
    OpenPalette,
    Close,
}
