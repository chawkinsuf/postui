//! `ctrl+w` and the tab keys on `App` (piece 4 spec §5.8; plan M4a): the
//! main panes by the layout's geometry (`keyroute::panes`), the two halves
//! of a Manage screen, and the tab strip of whatever has focus. Every
//! method here is `impl App`.

use super::*;
use crate::action::PaneDir;

impl App {
    /// Learns where the keyboard is at the start of each input event, a key
    /// (`handle_key`) or a mouse event (`handle_mouse`) (plan M4a Deviation
    /// 6), so a focus change by click or Tab counts for `<C-w>l` and `p`.
    /// A paste never moves focus, so it needs no note.
    pub(super) fn note_pane(&mut self) {
        self.panes.note(self.focus);
    }

    /// `Action::PaneMove`: whether the keyboard moved. With `leave`, the
    /// open field closes first, but only when there is a pane to move to:
    /// a refused move leaves it open, caret and all (the user's decision,
    /// 2026-10-05, as amendment C1 rules for `{N}gt`), and so does a close
    /// the field itself refuses.
    pub(super) fn pane_move(&mut self, dir: PaneDir, leave: bool) -> bool {
        match self.screen {
            Screen::Main => {
                let Some(to) = crate::keyroute::panes::toward(self.focus, dir, &self.panes) else {
                    return false;
                };
                if leave && !self.close_field_to_leave() {
                    return false;
                }
                self.update(Action::FocusPane(to))
            }
            Screen::Manage => {
                if leave {
                    if self.manage_pane_target(dir).is_none() {
                        return false;
                    }
                    if !self.close_field_to_leave() {
                        return false;
                    }
                }
                self.manage_pane_move(dir)
            }
            Screen::Testbed => false,
        }
    }

    /// Closes the open field for a key that leaves it (a window or tab
    /// key): whether it closed. A field may refuse (a Settings value that
    /// will not parse keeps its edit live, and says so), and then the key
    /// does nothing else.
    pub(super) fn close_field_to_leave(&mut self) -> bool {
        self.update(Action::CloseField);
        !self.field_open()
    }

    /// Which half a Manage screen's window key goes to (plan M4a Deviation
    /// 5): `Some(true)` the detail pane, `Some(false)` the list, `None` no
    /// move: `l` into the detail pane, `h` back to the list, `w` `W` `p` to
    /// the other half. The Settings tab has one pane, and a detail pane
    /// with nothing to show (no row selected; a selector group that has no
    /// grid to edit) takes no keyboard.
    fn manage_pane_target(&self, dir: PaneDir) -> Option<bool> {
        use crate::components::manage::ManageTab;
        use crate::components::manage_list::ListFocus;
        use crate::components::varmanager::VmFocus;
        let in_detail = match self.manage.tab {
            ManageTab::Settings => return None,
            ManageTab::Variables => self.varmanager.focus != VmFocus::List,
            _ => self.manage.list.focus == ListFocus::Detail,
        };
        let to_detail = match dir {
            PaneDir::Right => true,
            PaneDir::Left => false,
            PaneDir::Next | PaneDir::Prev | PaneDir::Last => !in_detail,
            PaneDir::Up | PaneDir::Down => return None,
        };
        if to_detail == in_detail {
            return None;
        }
        if to_detail {
            let project = self.project.as_ref()?;
            let has_detail = match self.manage.tab {
                ManageTab::Variables => self.varmanager.detail_takes_keyboard(project),
                tab => self.manage.list.selected(tab, project).is_some(),
            };
            if !has_detail {
                return None;
            }
        }
        Some(to_detail)
    }

    /// Moves to the half [`Self::manage_pane_target`] names.
    fn manage_pane_move(&mut self, dir: PaneDir) -> bool {
        use crate::components::manage::ManageTab;
        let Some(to_detail) = self.manage_pane_target(dir) else {
            return false;
        };
        let tab = self.manage.tab;
        let Self { project, varmanager, manage, .. } = self;
        match (tab, to_detail) {
            (ManageTab::Variables, true) => project.as_ref().is_some_and(|p| varmanager.enter_detail(p)),
            (ManageTab::Variables, false) => varmanager.leave_detail(),
            (_, true) => project.as_ref().is_some_and(|p| manage.list.enter_detail(tab, p)),
            (_, false) => {
                manage.list.leave_detail();
                true
            }
        }
    }

    /// The strip a tab key steps, resolved against what has focus, as
    /// drawn: each tab's select action in on-screen order (`None` where
    /// the tab is disabled), and the position of the one showing. The
    /// Manage screen's strip when that screen is up (it belongs to the
    /// shell, above every tab body's own keys), otherwise the focused
    /// pane's: the sidebar has none, so it borrows the editor's, the only
    /// other strip on the request screen. Any other screen (the testbed)
    /// draws no strip.
    fn tab_strip(&self) -> Option<(Vec<Option<Action>>, usize)> {
        use crate::components::manage::ManageTab;
        match (self.screen, self.focus) {
            (Screen::Manage, _) => Some((
                ManageTab::ALL.iter().map(|tab| Some(Action::SelectManageTab(*tab))).collect(),
                self.manage.tab.index(),
            )),
            (Screen::Testbed, _) => None,
            // A body that is not JSON has no Tree tab.
            (Screen::Main, PaneId::Response) => {
                let modes = self.session.response.tab_modes()?;
                let showing = self.session.response.view()?.mode;
                let at = modes.iter().position(|m| *m == showing)?;
                Some((modes.into_iter().map(|m| Some(Action::ResponseViewMode(m))).collect(), at))
            }
            // `EditorTabSelect` takes the on-screen position; the Body tab
            // is disabled on a GET.
            (Screen::Main, PaneId::Editor | PaneId::Sidebar) => Some((
                (0..EditorTab::COUNT)
                    .map(|i| self.editor.tab_enabled(EditorTab::from_draw_position(i)).then_some(Action::EditorTabSelect(i)))
                    .collect(),
                self.editor.active_tab.draw_position(),
            )),
        }
    }

    /// The action that selects where a tab key goes on [`Self::tab_strip`],
    /// or `None`: no strip, a tab that is not there or is disabled, or the
    /// tab already showing, which is not selected again, as in vim (a
    /// reselect would reset the table's row; final review M-1). A step
    /// walks the enabled tabs only, wrapping, so any distance lands right
    /// (plan M4a Deviation 8).
    pub(super) fn tab_target(&self, to: TabTo) -> Option<Action> {
        let (strip, showing) = self.tab_strip()?;
        let target = match to {
            TabTo::Nth(n) => n.checked_sub(1).filter(|i| strip.get(*i).is_some_and(Option::is_some))?,
            TabTo::By(delta) => {
                let enabled: Vec<usize> = (0..strip.len()).filter(|i| strip[*i].is_some()).collect();
                if enabled.is_empty() {
                    return None;
                }
                let at = enabled.iter().position(|i| *i == showing).unwrap_or(0) as i64;
                enabled[(at + i64::from(delta)).rem_euclid(enabled.len() as i64) as usize]
            }
        };
        if target == showing {
            return None;
        }
        strip.into_iter().nth(target).flatten()
    }
}
