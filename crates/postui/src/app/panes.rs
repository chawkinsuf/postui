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

    /// `Action::PaneMove`: whether the keyboard moved.
    pub(super) fn pane_move(&mut self, dir: PaneDir) -> bool {
        match self.screen {
            Screen::Main => match crate::keyroute::panes::toward(self.focus, dir, &self.panes) {
                Some(to) => self.update(Action::FocusPane(to)),
                None => false,
            },
            Screen::Manage => self.manage_pane_move(dir),
            Screen::Testbed => false,
        }
    }

    /// A Manage screen's two halves (plan M4a Deviation 5): `l` into the
    /// detail pane, `h` back to the list, `w` `W` `p` to the other half.
    /// The Settings tab has one pane.
    fn manage_pane_move(&mut self, dir: PaneDir) -> bool {
        use crate::components::manage::ManageTab;
        use crate::components::manage_list::ListFocus;
        use crate::components::varmanager::VmFocus;
        let tab = self.manage.tab;
        let in_detail = match tab {
            ManageTab::Settings => return false,
            ManageTab::Variables => self.varmanager.focus != VmFocus::List,
            _ => self.manage.list.focus == ListFocus::Detail,
        };
        let to_detail = match dir {
            PaneDir::Right => true,
            PaneDir::Left => false,
            PaneDir::Next | PaneDir::Prev | PaneDir::Last => !in_detail,
            PaneDir::Up | PaneDir::Down => return false,
        };
        if to_detail == in_detail {
            return false;
        }
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
