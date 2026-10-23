//! The router's side of `App` (piece 4 spec §3.1–§3.3): where a key
//! lands, read once per key. Split from `app.rs`; every method here is
//! `impl App`.

use super::*;
use crate::keyroute::table::Surfaces;
use crate::keyroute::{FieldCaret, FieldId, KeyCtx, OpenAs, Step, Surface};

/// What routing one key did (spec §3.1): whether to redraw, the open mode
/// a table row asked for (plan M3a Deviation 3), and whether a step other
/// than the engine ran, which makes the end-of-event sync settle the
/// session (spec §3.3: "a key the router ran itself").
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Ran {
    pub changed: bool,
    pub open_as: Option<OpenAs>,
    pub routed: bool,
}

impl Ran {
    /// Today's routing ran.
    pub(crate) fn routed(changed: bool) -> Self {
        Self { changed, routed: true, open_as: None }
    }

    /// The router or the engine took the key.
    pub(crate) fn consumed() -> Self {
        Self { changed: true, ..Self::default() }
    }

    pub(crate) fn and(self, o: Ran) -> Self {
        Self {
            changed: self.changed || o.changed,
            open_as: o.open_as.or(self.open_as),
            routed: self.routed || o.routed,
        }
    }
}

impl App {
    /// One snapshot of where `ev` lands (spec §3.2).
    /// A field with the caret never reads the keymap. While the engine
    /// runs it, the snapshot carries the engine's mode and pending state
    /// for the field lookahead (spec §3.7), and the surface's row count
    /// caps a field row's move (`3j` leaving a cell).
    pub fn key_ctx(&self, ev: &KeyEvent) -> KeyCtx {
        let profile = self.profile();
        let surface = self.surface();
        let field = self.field_caret();
        if field.is_live() {
            // A live field's keys go to its engine (vim), on either glue, or
            // type (arrows, and a field before its session syncs): no keymap
            // letter applies.
            let engine = self.vim_engine_view();
            return KeyCtx {
                profile,
                surface,
                field,
                user_bare: None,
                cap: self.repeat_cap(surface),
                engine,
                answers: Vec::new(),
            };
        }
        KeyCtx {
            profile,
            surface,
            field,
            user_bare: self.user_bare(ev, profile, surface),
            cap: self.repeat_cap(surface),
            engine: None,
            answers: if surface == Surface::Dialog {
                self.modals.answer_letters()
            } else {
                Vec::new()
            },
        }
    }

    /// What `keys.toml` binds `ev` to when it is a bare char (spec §4.4),
    /// where today's keymap step would reach it (plan M1 Deviation 21): no
    /// modal open, a navigation surface, and either the Main screen or a
    /// Manage screen whose whitelist admits the action. A dialog's letters
    /// and a screen's captured keys stay theirs. In vim any keymap binding
    /// counts; in arrows only a `keys.toml` override does, since an arrows
    /// default must not shadow the router's verb rows (plan M2 Deviation
    /// 18). Arrows' override also wins with a shift+↑/↓ selection up, as on
    /// main; vim's V swallows the letters it does not bind (plan M5a
    /// Deviation 11).
    fn user_bare(
        &self,
        ev: &KeyEvent,
        profile: crate::config::KeyProfile,
        surface: Surface,
    ) -> Option<Action> {
        let bare = matches!(ev.code, KeyCode::Char(_))
            && ev.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        let on =
            if profile == crate::config::KeyProfile::Vim { Surfaces::NAV } else { Surfaces::KEYED };
        if !bare || !self.modals.is_empty() || !on.has(surface) {
            return None;
        }
        let combo = KeyCombo::from_event(ev);
        let action = if profile == crate::config::KeyProfile::Vim {
            self.keymap.lookup(&combo)?
        } else {
            self.keymap.lookup_override(&combo)?
        };
        match self.screen {
            // The binding takes the key's context, as in today's routing
            // (`App::key_action`, spec S6a).
            Screen::Main => Some(self.key_action(action)),
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
        use crate::keyroute::MAX_COUNT;
        let most = match surface {
            Surface::Sidebar => self.sidebar.rows.len().saturating_mul(2),
            Surface::Table(_) => (self.editor.table_len() + 1).max(2),
            Surface::Response(_) | Surface::ResponseLines(_) => self
                .session
                .response
                .view()
                .map_or(0, |v| v.visible_len().max(v.measured_width().unwrap_or(MAX_COUNT))),
            Surface::Settings => crate::components::settings::SettingsTab::rows().len(),
            Surface::ManageList(tab) => self.manage_items(tab).len(),
            Surface::ManageDetail(tab) => {
                crate::components::manage_list::ManageList::detail_span(tab)
            }
            Surface::VarList | Surface::VarGrid | Surface::VarForm => {
                self.project().map_or(0, |p| self.varmanager.repeat_cap(p))
            }
            _ => MAX_COUNT,
        };
        most.max(1)
    }

    /// Runs a resolved key's steps in order (spec §3.1): a named key
    /// through `handle_key_routed`, an action through `update`, an engine
    /// key into the live field, an open mode into the event's `Ran`. A run
    /// of replayed named keys stops once the surface changes under it (plan
    /// M1 Deviation 13): `3j` from the URL line enters the table and stops.
    /// An engine run stops once its field closes, and any run stops at a
    /// `CloseField` the field refused. Actions run on whatever the
    /// surface: `3u` undoes three. A counted undo or redo stops at the
    /// first repeat that stepped nothing, so `10u` with three steps undoes
    /// three and says "Nothing to undo" once (the final review's ruling,
    /// spec §9 M1). A counted list verb is never repeated here: `{N}dd` and
    /// `{N}p` carry their count as one action, so `9p` with the wrong
    /// register is refused, and toasts, once.
    pub(super) fn run_steps(&mut self, steps: Vec<Step>) -> Ran {
        let surface = self.surface();
        let mut ran = Ran::default();
        for step in steps {
            match step {
                Step::OpenAs(open) => ran.open_as = Some(open),
                Step::Sync => self.vim_sync_now(ran.open_as),
                Step::Engine(ev) => match self.engine_key(ev) {
                    Some(r) => ran = ran.and(r),
                    None => break,
                },
                Step::Key(_) if self.surface() != surface => break,
                Step::Key(ev) => ran = ran.and(Ran::routed(self.handle_key_routed(ev))),
                Step::Action(a) => {
                    let history = matches!(a, Action::Undo | Action::Redo);
                    let close = a == Action::CloseField;
                    if close {
                        self.last_action_failed = false;
                    }
                    ran = ran.and(Ran::routed(self.update(a)));
                    if history && !self.history_stepped {
                        break;
                    }
                    // A refused close leaves the field open, and nothing
                    // after it runs: `ZZ` does not save or quit, `j` does
                    // not move (the final review's ruling I-2).
                    if close && self.last_action_failed {
                        break;
                    }
                }
            }
        }
        ran
    }

    /// The spelling context of where the keyboard is now (spec §4.6).
    pub fn spell_ctx(&self) -> crate::keyroute::SpellCtx {
        use crate::keyroute::FieldSpell;
        let surface = self.surface();
        let field = match self.vim_field_spell(surface) {
            Some(field) => field,
            None if self.field_caret().is_live() => FieldSpell::Typing,
            None => FieldSpell::None,
        };
        crate::keyroute::SpellCtx { profile: self.profile(), surface, field }
    }

    /// The half-typed keys for the footer (spec §3.8): the router's, then
    /// the engine's, then the recording note. Keys the router holds for a
    /// surface the keyboard has since left with no key are not shown: the
    /// next key there drops them.
    pub fn key_echo(&self) -> String {
        if self.router.typed_on() == Some(self.surface()) {
            let pending = self.router.echo();
            if !pending.is_empty() {
                return pending;
            }
        }
        let echo = self.vim_field_echo();
        if !echo.is_empty() {
            return echo;
        }
        // Third, the recording note (spec §3.8, §5.11).
        self.router
            .recording()
            .map_or_else(String::new, |reg| format!("recording @{reg} — not supported"))
    }

    /// The thing the keys go to right now (spec §3.3).
    pub fn surface(&self) -> Surface {
        match self.modals.top() {
            Some(Modal::Prompt { .. }) => Surface::Prompt,
            Some(Modal::NewProject { .. } | Modal::MultiPrompt { .. } | Modal::FieldsEditor(_)) => {
                Surface::Form
            }
            Some(
                Modal::Palette(_)
                | Modal::Chooser(_)
                | Modal::FilePicker(_)
                | Modal::VarPicker(_)
                | Modal::Dropdown(_),
            ) => Surface::Picker,
            Some(
                Modal::Message { .. }
                | Modal::Confirm { .. }
                | Modal::ConfigStartup { .. }
                | Modal::ConfigEditInvalid { .. }
                | Modal::KeymapChoice { .. },
            ) => Surface::Dialog,
            None => self.screen_surface(),
        }
    }

    /// The screen's own surface, under any modal: where the keys go once
    /// the modals are answered.
    pub(crate) fn screen_surface(&self) -> Surface {
        use crate::components::manage::ManageTab;
        use crate::components::manage_list::ListFocus;
        use crate::components::varmanager::VmFocus;
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
                PaneId::Response => {
                    let mode = self
                        .session
                        .response
                        .view()
                        .map_or(crate::components::response::ViewMode::Pretty, |v| v.mode);
                    if self.session.response.line_selection() {
                        Surface::ResponseLines(mode)
                    } else {
                        Surface::Response(mode)
                    }
                }
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
            Modal::Chooser(c) if c.has_filter() => Some(FieldId::ChooserFilter),
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

    /// The screen's own field with the caret, modals aside: the one place
    /// that knows which edit is live where (a Variables edit left behind
    /// another Manage tab is not that tab's field). `App::field_open` and
    /// `App::open_text_field_mut` read it rather than repeat the rules.
    pub(super) fn screen_field(&self) -> Option<FieldId> {
        use crate::components::manage::ManageTab;
        use crate::components::table_editor::Col;
        match self.screen {
            Screen::Manage if self.manage.tab == ManageTab::Settings => {
                self.settings.editing.map(FieldId::Settings)
            }
            // A Variables edit left live by a failed click-away commit
            // stays behind a tab switch; key routing only reaches it on
            // its own tab, and so does the caret.
            Screen::Manage if self.manage.tab != ManageTab::Variables => None,
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
