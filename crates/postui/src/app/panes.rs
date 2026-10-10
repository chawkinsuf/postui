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

    /// Vim leaves Visual whenever the window changes (plan M5a Task 6): in
    /// the vim profile, a V selection on the response ends once the
    /// keyboard is anywhere else, by a window key, a click, a palette
    /// command that moves focus, another screen, or the jq bar taking the
    /// caret. Run at the boundary of each input event, so every way of
    /// leaving counts. A dialog or the palette over the response leaves it
    /// up: their commands act on the selection (copy, extract). A drag's
    /// character selection is not Visual, and an arrows shift selection is
    /// a GUI one: both stay, as on main. Whether it ended one.
    pub(super) fn end_visual_off_response(&mut self) -> bool {
        let away = self.screen != Screen::Main
            || self.focus != PaneId::Response
            || self.session.response.jq_focused();
        if !(away && self.profile() == crate::config::KeyProfile::Vim && self.session.response.line_selection()) {
            return false;
        }
        self.session.response.lines(crate::action::LinesOp::End);
        true
    }

    /// `Action::PaneMove`: whether the keyboard moved. An open field closes
    /// first, but only when there is a pane to move to: a refused move
    /// leaves it open, caret and all (the user's decision, 2026-10-05, as
    /// amendment C1 rules for `{N}gt`), and so does a close the field
    /// itself refuses. A key on a closed surface has no open field to close.
    pub(super) fn pane_move(&mut self, dir: PaneDir) -> bool {
        match self.screen {
            Screen::Main => {
                let Some(to) = crate::keyroute::panes::toward(self.focus, dir, &self.panes) else {
                    return false;
                };
                // A V selection the move leaves behind ends at the event's
                // boundary (`end_visual_off_response`).
                self.leave_then_go(true, |app| app.update(Action::FocusPane(to)))
            }
            Screen::Manage => {
                let Some(to_detail) = self.manage_pane_target(dir) else {
                    return false;
                };
                self.leave_then_go(true, |app| app.manage_pane_move(to_detail))
            }
            Screen::Testbed => false,
        }
    }

    /// The order every window and tab key follows (spec §5.8): the caller
    /// has already settled where the key goes, and a key with nowhere to
    /// go never gets here. With `leave`, the open field closes first; a
    /// field that refuses (a Settings value that will not parse, a
    /// variable commit that fails) keeps the keyboard, and the key does
    /// nothing else. Then `go` moves. Whether anything changed: a field it
    /// closed counts.
    fn leave_then_go(&mut self, leave: bool, go: impl FnOnce(&mut Self) -> bool) -> bool {
        let closing = leave && self.field_open();
        if closing && !self.close_field_to_leave() {
            return false;
        }
        go(self) || closing
    }

    /// Closes the open field for a key that leaves it: whether it closed.
    fn close_field_to_leave(&mut self) -> bool {
        self.update(Action::CloseField);
        !self.field_open()
    }

    /// A tab key: vim's `GoTab` (`land`), and arrows' `CycleTabs`, which
    /// keeps the caret where it is (plan M4a Deviation 7). The strip's
    /// target comes first (a missing, disabled or already-showing tab
    /// does nothing), then [`Self::leave_then_go`]. Only a key that
    /// changed the tab lands the caret.
    pub(super) fn go_tab(&mut self, to: TabTo, leave: bool, land: bool) -> bool {
        let Some(select) = self.tab_target(to) else {
            return false;
        };
        self.leave_then_go(leave, |app| {
            if !app.update(select) {
                return false;
            }
            if land {
                app.land_editor_caret();
            }
            true
        })
    }

    /// Whether arrows' tab chord closes the open field before its tab
    /// goes: on the Manage screen, as a click on the strip commits it; on
    /// the main screen the caret stays where it is (spec §5.8).
    pub(super) fn tab_chord_leaves(&self) -> bool {
        self.screen == Screen::Manage
    }

    /// After a vim tab key selects a tab (spec §5.8): with the editor
    /// focused, the caret lands in that tab's content, so `gt a` adds a
    /// row. It never moves panes; the sidebar and the response step the
    /// strip from where they stand. Arrows' tab chord does not run it (plan
    /// M4a Deviation 7). Only a key that changed the tab gets here
    /// (`strip_target` names no already-showing tab), so a `{N}gt` onto
    /// the body's own tab never lands: the body's session stays whole, its
    /// engine history with it (final review M-1). Every landing that does
    /// run starts outside the tab it lands on, a table tab's included,
    /// and so flushes the field it leaves like any other.
    fn land_editor_caret(&mut self) {
        if self.screen != Screen::Main || self.focus != PaneId::Editor {
            return;
        }
        // The field it leaves closes as its own history step first.
        self.flush_field_session();
        self.editor.focus_content_from_app();
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
                tab => self.manage.list.detail_takes_keyboard(tab, project),
            };
            if !has_detail {
                return None;
            }
        }
        Some(to_detail)
    }

    /// Moves to the half [`Self::manage_pane_target`] named.
    fn manage_pane_move(&mut self, to_detail: bool) -> bool {
        use crate::components::manage::ManageTab;
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
            (Screen::Main, PaneId::Editor | PaneId::Sidebar) => Some(self.editor_tab_strip()),
        }
    }

    /// The editor's strip, whatever has focus: `EditorTabSelect` takes the
    /// on-screen position; the Body tab is disabled on a GET.
    fn editor_tab_strip(&self) -> (Vec<Option<Action>>, usize) {
        (
            (0..EditorTab::COUNT)
                .map(|i| self.editor.tab_enabled(EditorTab::from_draw_position(i)).then_some(Action::EditorTabSelect(i)))
                .collect(),
            self.editor.active_tab.draw_position(),
        )
    }

    /// `Action::EditorTabCycle` (`keys.toml`'s `editor_tab_next` and
    /// `editor_tab_prev`): the editor's strip from any focus, stepped as
    /// [`Self::tab_target`] steps.
    pub(super) fn editor_tab_step(&self, delta: i8) -> Option<Action> {
        strip_target(self.editor_tab_strip(), TabTo::By(i64::from(delta)))
    }

    /// The action that selects where a tab key goes on [`Self::tab_strip`],
    /// or `None`: no strip, a tab that is not there or is disabled, or the
    /// tab already showing, which is not selected again, as in vim (a
    /// reselect would reset the table's row; final review M-1). A step
    /// walks the enabled tabs only, wrapping, so any distance lands right
    /// (plan M4a Deviation 8).
    pub(super) fn tab_target(&self, to: TabTo) -> Option<Action> {
        strip_target(self.tab_strip()?, to)
    }
}

/// Where a tab key goes on a strip (see [`App::tab_target`]).
pub(super) fn strip_target((strip, showing): (Vec<Option<Action>>, usize), to: TabTo) -> Option<Action> {
    let target = match to {
        TabTo::Nth(n) => n.checked_sub(1).filter(|i| strip.get(*i).is_some_and(Option::is_some))?,
        TabTo::By(delta) => {
            let enabled: Vec<usize> = (0..strip.len()).filter(|i| strip[*i].is_some()).collect();
            if enabled.is_empty() {
                return None;
            }
            // A disabled tab showing (never expected: `App::sync_active_tab`
            // hops off one) steps from where it sits between its enabled
            // neighbours, so one step lands on the next of them.
            let at = match enabled.binary_search(&showing) {
                Ok(i) => i as i64,
                Err(i) if delta > 0 => i as i64 - 1,
                Err(i) => i as i64,
            };
            enabled[(at + delta.rem_euclid(enabled.len() as i64)).rem_euclid(enabled.len() as i64) as usize]
        }
    };
    if target == showing {
        return None;
    }
    strip.into_iter().nth(target).flatten()
}
