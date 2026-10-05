//! `ctrl+w` on `App` (piece 4 spec §5.8; plan M4a): the main panes by the
//! layout's geometry (`keyroute::panes`), and the two halves of a Manage
//! screen. Every method here is `impl App`.

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
    /// 2026-10-05, as amendment C1 rules for `{N}gt`).
    pub(super) fn pane_move(&mut self, dir: PaneDir, leave: bool) -> bool {
        match self.screen {
            Screen::Main => {
                let Some(to) = crate::keyroute::panes::toward(self.focus, dir, &self.panes) else {
                    return false;
                };
                if leave {
                    self.update(Action::CloseField);
                }
                self.update(Action::FocusPane(to))
            }
            Screen::Manage => {
                if leave {
                    if self.manage_pane_target(dir).is_none() {
                        return false;
                    }
                    self.update(Action::CloseField);
                }
                self.manage_pane_move(dir)
            }
            Screen::Testbed => false,
        }
    }

    /// Which half a Manage screen's window key goes to (plan M4a Deviation
    /// 5): `Some(true)` the detail pane, `Some(false)` the list, `None` no
    /// move: `l` into the detail pane, `h` back to the list, `w` `W` `p` to
    /// the other half. The Settings tab has one pane.
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
        (to_detail != in_detail).then_some(to_detail)
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
}
