//! Where a key lands (piece 4 spec §3.2–§3.3): the surface taking the
//! keys and the text field with the caret. `App::surface` and
//! `App::field_caret` are the one place that reads them off the app.

use crate::action::Action;
use crate::components::editor::EditorTab;
use crate::components::manage::ManageTab;
use crate::components::response::ViewMode;
use crate::components::settings::SettingsField;
use crate::components::varmanager::VmField;
use crate::config::KeyProfile;

/// One snapshot of where a key lands (spec §3.2). `App::key_ctx` builds
/// it fresh for every key; the router reads nothing else from the app.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyCtx {
    pub profile: KeyProfile,
    pub surface: Surface,
    pub field: FieldCaret,
    /// In vim, what `keys.toml` binds this key to when it is a bare char
    /// (spec §4.4): it wins over the table while nothing is pending.
    pub user_bare: Option<Action>,
    /// The most rows a repeated motion can move on this surface: a
    /// replay past it would change nothing (plan M1 Deviation 11).
    pub cap: usize,
}

/// What a keycap's spelling depends on (spec §4.6): the profile, the
/// surface, and whether a field has the caret (a live field takes every
/// key the table would otherwise bind there). The `Copy` part of the
/// context, for the footer and the hover hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpellCtx {
    pub profile: KeyProfile,
    pub surface: Surface,
    pub field_live: bool,
}

/// The thing the keys go to. It never names a stale pane: `App::surface`
/// reads the modal stack, the screen, the focus and the editor's
/// sub-focus every time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Surface {
    Sidebar,
    /// The URL line, selected or open, and the editor pane with nothing
    /// selected (plan M1 Deviation 14).
    UrlLine,
    Method,
    /// The Params, Headers or Vars table.
    Table(EditorTab),
    Body,
    Response(ViewMode),
    VarList,
    VarGrid,
    VarForm,
    ManageList(ManageTab),
    ManageDetail(ManageTab),
    Settings,
    /// A one-field prompt (`Modal::Prompt`).
    Prompt,
    /// A multi-field form (`NewProject`, `MultiPrompt`, `FieldsEditor`).
    Form,
    /// A message, a confirm, the startup gates, the keyboard question.
    Dialog,
    /// The palette, a chooser, the file picker, the `{{` picker, a dropdown.
    Picker,
    /// The testbed.
    #[default]
    Other,
}

/// Every text field the caret can be in, named once (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldId {
    Url,
    /// `col` 0 is the name cell, 1 the value cell.
    TableCell { row: usize, col: usize },
    Body,
    VmForm(VmField),
    VmGrid { row: usize, col: usize },
    Settings(SettingsField),
    /// A form modal's field, by `ModalStack::caret_field`'s index.
    ModalInput(usize),
    FilePicker,
    Jq,
    Search,
    Palette,
    ChooserFilter,
    VarPickerFilter,
}

/// Which field takes typed keys (spec §3.3; plan M1 Deviation 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldCaret {
    None,
    /// This field has the caret: keys type into it.
    Live(FieldId),
    /// This field is open under a modal that has no field of its own (a
    /// dropdown, a confirm): it keeps its text and comes back when the
    /// modal goes.
    Parked(FieldId),
}

impl FieldCaret {
    pub fn is_live(self) -> bool {
        matches!(self, FieldCaret::Live(_))
    }
}

/// The engine buffer a field runs as in the vim profile (piece 3 §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// A one-line field: `OneLineBuf` over its `LineInput`.
    Line,
    /// The request body: `BodyBuf` over edtui's `EditorState`.
    Body,
}

impl FieldId {
    /// The engine buffer this field runs as in the vim profile, or `None`
    /// for one that still types modelessly: modal fields and the query
    /// boxes wait for M3b (plan M3a Deviation 1).
    pub fn engine_kind(self) -> Option<EngineKind> {
        match self {
            FieldId::Url
            | FieldId::TableCell { .. }
            | FieldId::VmForm(_)
            | FieldId::VmGrid { .. }
            | FieldId::Settings(_) => Some(EngineKind::Line),
            FieldId::Body => Some(EngineKind::Body),
            FieldId::ModalInput(_)
            | FieldId::FilePicker
            | FieldId::Jq
            | FieldId::Search
            | FieldId::Palette
            | FieldId::ChooserFilter
            | FieldId::VarPickerFilter => None,
        }
    }
}

/// The engine's state the router needs (spec §3.2): its mode, and whether
/// one of its commands is half-typed. `KeyCtx.engine` carries it while an
/// engine session is live on the field with the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EngineView {
    pub mode: super::engine_glue::Mode,
    pub pending: bool,
}

impl Surface {
    /// One surface of every kind, for tests that walk the table.
    #[cfg(test)]
    pub(crate) const SAMPLES: [Surface; 17] = [
        Surface::Sidebar,
        Surface::UrlLine,
        Surface::Method,
        Surface::Table(EditorTab::Params),
        Surface::Body,
        Surface::Response(ViewMode::Pretty),
        Surface::VarList,
        Surface::VarGrid,
        Surface::VarForm,
        Surface::ManageList(ManageTab::Environments),
        Surface::ManageDetail(ManageTab::Environments),
        Surface::Settings,
        Surface::Prompt,
        Surface::Form,
        Surface::Dialog,
        Surface::Picker,
        Surface::Other,
    ];
}
