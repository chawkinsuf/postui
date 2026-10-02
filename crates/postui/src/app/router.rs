//! The router's side of `App` (piece 4 spec §3.1–§3.3): where a key
//! lands, read once per key. Split from `app.rs`; every method here is
//! `impl App`.

use super::*;
use crate::keyroute::{FieldCaret, FieldId, Surface};

impl App {
    /// The thing the keys go to right now (spec §3.3).
    pub fn surface(&self) -> Surface {
        use crate::components::manage::ManageTab;
        use crate::components::manage_list::ListFocus;
        use crate::components::varmanager::VmFocus;
        if let Some(top) = self.modals.top() {
            return match top {
                Modal::Prompt { .. } => Surface::Prompt,
                Modal::NewProject { .. } | Modal::MultiPrompt { .. } | Modal::FieldsEditor(_) => {
                    Surface::Form
                }
                Modal::Palette(_)
                | Modal::Chooser(_)
                | Modal::FilePicker(_)
                | Modal::VarPicker(_)
                | Modal::Dropdown(_) => Surface::Picker,
                Modal::Message { .. }
                | Modal::Confirm { .. }
                | Modal::ConfigStartup { .. }
                | Modal::ConfigEditInvalid { .. }
                | Modal::KeymapChoice { .. } => Surface::Dialog,
            };
        }
        match self.screen {
            Screen::Testbed => Surface::Other,
            Screen::Manage => match self.manage.tab {
                ManageTab::Settings => Surface::Settings,
                ManageTab::Variables => match self.varmanager.focus {
                    VmFocus::List => Surface::VarList,
                    VmFocus::Grid => Surface::VarGrid,
                    VmFocus::Form => Surface::VarForm,
                },
                tab => match self.manage.list.focus {
                    ListFocus::List => Surface::ManageList(tab),
                    ListFocus::Detail => Surface::ManageDetail(tab),
                },
            },
            Screen::Main => match self.focus {
                PaneId::Sidebar => Surface::Sidebar,
                PaneId::Response => Surface::Response(
                    self.session
                        .response
                        .view()
                        .map_or(crate::components::response::ViewMode::Pretty, |v| v.mode),
                ),
                PaneId::Editor => match self.editor.sub_focus {
                    SubFocus::Method => Surface::Method,
                    SubFocus::Url | SubFocus::None => Surface::UrlLine,
                    SubFocus::Content if self.editor.active_tab == EditorTab::Body => Surface::Body,
                    SubFocus::Content => Surface::Table(self.editor.active_tab),
                },
            },
        }
    }

    /// The field that takes typed keys (spec §3.3; plan M1 Deviation 6).
    /// A modal with its own field has the caret; a modal without one
    /// parks the field under it.
    pub fn field_caret(&self) -> FieldCaret {
        let base = self.screen_field();
        let Some(top) = self.modals.top() else {
            return base.map_or(FieldCaret::None, FieldCaret::Live);
        };
        let own = match top {
            Modal::Palette(_) => Some(FieldId::Palette),
            Modal::Chooser(_) => Some(FieldId::ChooserFilter),
            Modal::VarPicker(s) if s.mode == crate::components::var_picker::PickerMode::Insert => {
                Some(FieldId::VarPickerFilter)
            }
            Modal::FilePicker(_) => Some(FieldId::FilePicker),
            _ => self.modals.caret_field().map(FieldId::ModalInput),
        };
        match (own, base) {
            (Some(id), _) => FieldCaret::Live(id),
            (None, Some(id)) => FieldCaret::Parked(id),
            (None, None) => FieldCaret::None,
        }
    }

    /// The screen's own field with the caret, modals aside.
    fn screen_field(&self) -> Option<FieldId> {
        use crate::components::manage::ManageTab;
        use crate::components::table_editor::Col;
        match self.screen {
            Screen::Manage if self.manage.tab == ManageTab::Settings => {
                self.settings.editing.map(FieldId::Settings)
            }
            Screen::Manage => {
                if let Some(e) = &self.varmanager.form.editing {
                    return Some(FieldId::VmForm(e.field));
                }
                self.varmanager
                    .grid
                    .editing
                    .as_ref()
                    .map(|e| FieldId::VmGrid { row: e.row, col: e.col })
            }
            Screen::Main => match self.focus {
                PaneId::Editor => {
                    if let Some(e) = &self.editor.table.editing {
                        return Some(FieldId::TableCell {
                            row: e.row,
                            col: usize::from(e.col == Col::Value),
                        });
                    }
                    if self.editor.url_open() {
                        return Some(FieldId::Url);
                    }
                    (self.editor.sub_focus == SubFocus::Content
                        && self.editor.active_tab == EditorTab::Body)
                        .then_some(FieldId::Body)
                }
                PaneId::Response if self.session.response.jq_focused() => Some(FieldId::Jq),
                PaneId::Response if self.session.response.field_open() => Some(FieldId::Search),
                PaneId::Response | PaneId::Sidebar => None,
            },
            Screen::Testbed => None,
        }
    }
}
