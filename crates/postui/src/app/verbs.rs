//! `Action::OnSelection` (piece 4 spec §6.1): a list verb resolved
//! against the surface the keyboard is on, to the parameterized action
//! that surface already has. Each surface's own component decides,
//! through its `verb_action`, which its footer chips read too. Split from
//! `app.rs`; every method here is `impl App`.

use super::*;
use crate::action::Verb;
use crate::keyroute::Surface;

impl App {
    /// What the `:` line can act on (spec §5.9; plan M4b Deviation 10):
    /// the surface under it and the list verbs that resolve there now.
    /// Read when `:` opens the line.
    pub(crate) fn ex_scope(&self) -> crate::keyroute::ex::Scope {
        let surface = self.surface();
        let verbs = Verb::ALL
            .into_iter()
            .filter(|v| self.selection_action(surface, *v).is_some())
            .collect();
        crate::keyroute::ex::Scope { surface, verbs }
    }

    /// The one table from (surface, verb) to an action (spec §6.1).
    /// `None` means the verb has no meaning there; nothing happens.
    pub(crate) fn selection_action(&self, surface: Surface, verb: Verb) -> Option<Action> {
        match surface {
            Surface::Sidebar => self.sidebar.verb_action(verb),
            Surface::Table(_) => self.editor.table_verb_action(verb),
            Surface::ManageList(tab) | Surface::ManageDetail(tab) => {
                self.manage.list.verb_action(tab, self.project()?, verb)
            }
            Surface::VarList | Surface::VarGrid | Surface::VarForm => {
                let open_request =
                    self.editor.slug.is_some().then(|| self.editor.current_request());
                self.varmanager.verb_action(self.project()?, open_request.as_ref(), verb)
            }
            // A dropdown's highlighted row: its own rename or delete.
            Surface::Menu => self.modals.dropdown_verb_action(verb),
            _ => None,
        }
    }
}
