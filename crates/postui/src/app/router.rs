//! The router's side of `App` (piece 4 spec §3.1–§3.3): where a key
//! lands, read once per key. Split from `app.rs`; every method here is
//! `impl App`.

use super::*;
use crate::keyroute::table::Surfaces;
use crate::keyroute::{FieldCaret, FieldId, KeyCtx, Step, Surface};

impl App {
    /// One snapshot of where `ev` lands (spec §3.2).
    pub fn key_ctx(&self, ev: &KeyEvent) -> KeyCtx {
        let profile = self.profile();
        let surface = self.surface();
        KeyCtx {
            profile,
            surface,
            field: self.field_caret(),
            user_bare: self.user_bare(ev, profile, surface),
            cap: self.repeat_cap(surface),
        }
    }

    /// In vim, what `keys.toml` binds `ev` to when it is a bare char (spec
    /// §4.4), where today's keymap step would reach it (plan M1 Deviation
    /// 21): no modal open, a navigation surface, and either the Main
    /// screen or a Manage screen whose whitelist admits the action. A
    /// dialog's letters and a screen's captured keys stay theirs.
    fn user_bare(
        &self,
        ev: &KeyEvent,
        profile: crate::config::KeyProfile,
        surface: Surface,
    ) -> Option<Action> {
        let bare = matches!(ev.code, KeyCode::Char(_))
            && ev.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        if profile != crate::config::KeyProfile::Vim
            || !bare
            || !self.modals.is_empty()
            || !Surfaces::NAV.has(surface)
        {
            return None;
        }
        let action = self.keymap.lookup(&KeyCombo::from_event(ev))?;
        match self.screen {
            Screen::Main => Some(action),
            Screen::Manage => screen_escape_whitelist(&action).then_some(action),
            Screen::Testbed => None,
        }
    }

    /// The most a repeated key can usefully run on `surface` (plan M1
    /// Deviation 11): one pass over what the surface holds is all a count
    /// can use, so a 9 999 count costs no more than that. It counts the
    /// sideways steps too (`{N}l`), never 0 (a plain `j` is one step):
    /// - the sidebar: two per row (an `h` collapses a folder, the next
    ///   steps to its parent);
    /// - a table: its rows and the add row, and at least its two columns;
    /// - the response: its lines, or its widest line as last drawn (the
    ///   whole count while that is unmeasured);
    /// - Settings: its rows;
    /// - a Manage list: its items; a Manage detail pane: its controls;
    /// - the Variables screen: its left rows, the grid's rows and columns,
    ///   or the form's stops.
    pub(super) fn repeat_cap(&self, surface: Surface) -> usize {
        use crate::vim::engine::settings::MAX_COUNT;
        let most = match surface {
            Surface::Sidebar => self.sidebar.rows.len().saturating_mul(2),
            Surface::Table(_) => (self.editor.table_len() + 1).max(2),
            Surface::Response(_) => self.session.response.view().map_or(0, |v| {
                v.visible_len().max(v.measured_width().unwrap_or(MAX_COUNT))
            }),
            Surface::Settings => crate::components::settings::SettingsTab::rows().len(),
            Surface::ManageList(tab) => self.manage_items(tab).len(),
            Surface::ManageDetail(tab) => {
                crate::components::manage_list::ManageList::detail_span(tab)
            }
            Surface::VarList | Surface::VarGrid | Surface::VarForm => self
                .project()
                .map_or(0, |p| self.varmanager.repeat_cap(p)),
            _ => MAX_COUNT,
        };
        most.max(1)
    }

    /// Runs a resolved key's steps in order (spec §3.1): a named key
    /// through `handle_key_routed`, an action through `update`. A run of
    /// replayed keys stops once the surface changes under it (plan M1
    /// Deviation 13): `3j` from the URL line enters the table and stops.
    /// Actions run on whatever the surface: `3u` undoes three. A counted
    /// undo or redo stops at the first repeat that stepped nothing, so
    /// `10u` with three steps undoes three and says "Nothing to undo"
    /// once (the final review's ruling, spec §9 M1).
    pub(super) fn run_steps(&mut self, steps: Vec<Step>) -> bool {
        let surface = self.surface();
        let mut changed = false;
        for step in steps {
            match step {
                Step::Key(_) if self.surface() != surface => break,
                Step::Key(ev) => changed |= self.handle_key_routed(ev),
                Step::Action(a) => {
                    let history = matches!(a, Action::Undo | Action::Redo);
                    changed |= self.update(a);
                    if history && !self.history_stepped {
                        break;
                    }
                }
            }
        }
        changed
    }

    /// The spelling context of where the keyboard is now (spec §4.6).
    pub fn spell_ctx(&self) -> crate::keyroute::SpellCtx {
        crate::keyroute::SpellCtx {
            profile: self.profile(),
            surface: self.surface(),
            field_live: self.field_caret().is_live(),
        }
    }

    /// The router's half-typed keys for the footer (spec §3.8). Keys typed
    /// on a surface the keyboard has since left with no key (a modal
    /// raised by a background result, focus moved by a send) are not
    /// shown: the next key there drops them (`Router::resolve`).
    pub fn key_echo(&self) -> String {
        if self.router.typed_on() != Some(self.surface()) {
            return String::new();
        }
        self.router.echo()
    }

    /// The vim mode word (spec §3.8): what is taking the keys. A field
    /// with the caret types, a picker's query included (`INSERT`, until M3
    /// gives fields their Normal mode); everything else, a dropdown's
    /// list among them, is `NORMAL`. `None` in the arrows profile.
    pub fn mode_indicator(&self) -> Option<&'static str> {
        if self.profile() != crate::config::KeyProfile::Vim {
            return None;
        }
        Some(if self.field_caret().is_live() { "INSERT" } else { "NORMAL" })
    }

    /// The mode and the echo as the footer paints them: `NORMAL  3`.
    pub fn vim_status(&self) -> Option<String> {
        let mode = self.mode_indicator()?;
        let echo = self.key_echo();
        Some(if echo.is_empty() {
            mode.to_string()
        } else {
            format!("{mode}  {echo}")
        })
    }

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
