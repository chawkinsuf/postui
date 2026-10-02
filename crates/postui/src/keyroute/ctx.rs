//! Where a key lands (piece 4 spec §3.2–§3.3): the surface taking the
//! keys and the text field with the caret. `App::surface` and
//! `App::field_caret` are the one place that reads them off the app.

use crate::components::editor::EditorTab;
use crate::components::manage::ManageTab;
use crate::components::response::ViewMode;
use crate::components::settings::SettingsField;
use crate::components::varmanager::VmField;

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
