//! The Manage screen's Project tab, `App`'s side (spec 2026-11-15 project
//! settings): its keys, and the writes. The tab's own state is
//! `ProjectTab`; every change to the name or the default headers is
//! written to `project.toml` here, at once, as one undo step (R7, R9).
//! Split from `app.rs`; every method here is `impl App`.

use super::*;
use crate::components::project_tab::ProjectStop;
use crate::components::table_editor::{Stop, TableEditorState, TableOutcome};
use indexmap::IndexMap;
use postui_core::model::Entry;

impl App {
    /// How many default headers the open project has.
    pub(super) fn default_headers_len(&self) -> usize {
        self.project().map_or(0, |p| p.meta().default_headers.len())
    }

    /// Keys on the Project tab. `Some` is the redraw answer for a key the
    /// tab claimed; `None` hands it to the router's unclaimed-key step.
    /// A key the tab claims asks for the cursor back, as on the Settings
    /// tab; one it hands on leaves the cursor as it was.
    pub(super) fn handle_project_key(&mut self, ev: KeyEvent) -> Option<bool> {
        if self.project.is_none() {
            return (ev.code == KeyCode::Esc).then(|| self.update(Action::CloseScreen));
        }
        let answer = self.project_tab_key(ev);
        if answer.is_some() {
            self.project_tab.focused = true;
        }
        answer
    }

    /// [`Self::handle_project_key`] with a project open.
    fn project_tab_key(&mut self, ev: KeyEvent) -> Option<bool> {
        let plain = ev.modifiers.is_empty();
        let ctrl = ev.modifiers == KeyModifiers::CONTROL;
        // An open name owns the keyboard: Enter and Esc both commit (the
        // field rule, R6); a refusal keeps it open and fails the key.
        if self.project_tab.name_open() {
            match ev.code {
                KeyCode::Esc | KeyCode::Enter => {
                    if !self.commit_project_name() {
                        self.last_action_failed = true;
                    }
                }
                _ => {
                    if let Some(input) = self.project_tab.name_input_mut() {
                        input.handle_key(ev);
                    }
                }
            }
            return Some(self.update(Action::Render));
        }
        // An open cell owns the keyboard too: the table's own keys (Enter
        // and Esc commit, Tab walks, ↑↓ leave), each change written. A key
        // that only edits the cell's text writes nothing, so it goes
        // straight in. A key neither takes is swallowed, not handed on, as
        // on the Name row above, the Settings fields and the Variables grid.
        if self.project_tab.table.editing.is_some() {
            if self.project_tab.table.type_into_cell(ev).is_none()
                && self.run_header_table(|t, map| t.handle_key(ev, map)).1
            {
                self.last_action_failed = true;
            }
            return Some(self.update(Action::Render));
        }
        if ev.code == KeyCode::Esc {
            return Some(self.update(Action::CloseScreen));
        }
        let len = self.default_headers_len();
        // The Name row is the first stop and `+ Add header` the last: the
        // keys the table takes to its first or last stop go there.
        let first = matches!(ev.code, KeyCode::Home | KeyCode::PageUp) && (plain || ctrl)
            || ev.code == KeyCode::Char('b') && ctrl;
        let last = matches!(ev.code, KeyCode::End | KeyCode::PageDown) && (plain || ctrl)
            || ev.code == KeyCode::Char('f') && ctrl;
        // The half-page keys cross onto and off the Name row as ↑↓ do:
        // ctrl+d off it, and ctrl+u from the table's first stop.
        let half_down = ev.code == KeyCode::Char('d') && ctrl;
        let half_up = ev.code == KeyCode::Char('u') && ctrl;
        let at_top = self.project_tab.table.selected().map(|s| s.clamp(len, 0))
            == Some(Stop::of_row(0, len));
        match self.project_tab.stop {
            ProjectStop::Name => match ev.code {
                KeyCode::Down if plain => self.project_tab.to_table(len),
                _ if half_down => self.project_tab.to_table(len),
                _ if last => {
                    self.project_tab.to_table(len);
                    self.project_tab.table.set_selected(Some(Stop::Add));
                }
                KeyCode::Enter if plain => self.open_project_name(),
                KeyCode::Up if plain => {}
                _ if first || half_up => {}
                _ => return None,
            },
            // ↑ off the first row goes to the Name row too, below.
            ProjectStop::Table if first || half_up && at_top => self.project_tab.to_name(),
            ProjectStop::Table => {
                let (outcome, refused) = self.run_header_table(|t, map| t.handle_key(ev, map));
                if refused {
                    self.last_action_failed = true;
                }
                if let Some(i) = outcome.request_delete {
                    return Some(self.update(Action::DeleteDefaultHeader(i)));
                }
                if !outcome.consumed {
                    if ev.code != KeyCode::Up || !plain {
                        return None;
                    }
                    self.project_tab.to_name();
                }
            }
        }
        Some(self.update(Action::Render))
    }

    /// Opens the Name row's edit, seeded with the `name` the file holds.
    pub(super) fn open_project_name(&mut self) {
        let seed = self.project().and_then(|p| p.meta().name.clone()).unwrap_or_default();
        self.project_tab.begin_name(&seed);
    }

    /// Runs `f` on the header table and a copy of the default headers, and
    /// writes the copy when `f` changed it (R9). The outcome's warning is
    /// toasted when the change stands. A refused write puts the table back
    /// as it was, its open cell and typing included (R14), toasts no
    /// warning, and answers `true` beside `f`'s outcome; the caller decides
    /// whether that fails the action, as `commit_grid_edit`'s callers do.
    ///
    /// A cell `f` left open (a Tab walk commits one cell and opens the
    /// next) is taken out of the table across the write and put back
    /// after, so the write's `begin_op`, which closes the tab's input as
    /// any op does, finds nothing open: the change `f` made is already in
    /// the copy.
    ///
    /// Each run copies the default headers and the table's state. Typing
    /// into an open cell doesn't come here
    /// ([`TableEditorState::type_into_cell`]).
    pub(super) fn run_header_table(
        &mut self,
        f: impl FnOnce(&mut TableEditorState, &mut IndexMap<String, Entry>) -> TableOutcome,
    ) -> (TableOutcome, bool) {
        let mut map = self.project().map(|p| p.meta().default_headers.clone()).unwrap_or_default();
        let saved = self.project_tab.table.clone();
        let mut outcome = f(&mut self.project_tab.table, &mut map);
        let unchanged = self.project().is_none_or(|p| p.meta().default_headers == map);
        if !unchanged {
            let next = self.project_tab.table.editing.take();
            if !self.write_default_headers(&map) {
                self.project_tab.table = saved;
                return (outcome, true);
            }
            self.project_tab.table.editing = next;
        }
        if let Some(w) = outcome.warning.take() {
            self.toasts.push(w, ToastKind::Warning);
        }
        (outcome, false)
    }

    /// Writes `map` as the default headers, one undo step. `false` when
    /// the write was refused (toasted).
    fn write_default_headers(&mut self, map: &IndexMap<String, Entry>) -> bool {
        let t = self.begin_op();
        match self.project.as_mut().map(|p| p.set_default_headers(map)) {
            Some(Ok(())) => {
                self.record_project_step(t, crate::undo::StepLabel::change("default headers"));
                true
            }
            Some(Err(e)) => {
                self.op_in_flight = false;
                self.toasts.push(format!("cannot save default headers: {e}"), ToastKind::Error);
                false
            }
            None => {
                self.op_in_flight = false;
                false
            }
        }
    }

    /// Commits the Name row's edit (R7). One rule: it writes when what it
    /// would write, the trimmed typing or no name when that is empty,
    /// differs from the `name` the file holds; otherwise it writes nothing
    /// and starts no op. So a name the file holds with spaces round it, or
    /// blank, is trimmed or removed by a commit, and typing that trims to
    /// the file's name writes nothing.
    /// `false` when the write was refused: the edit stays open with the
    /// typing (R14), and the caller decides whether that fails the action.
    /// The edit is taken out before the write's `begin_op`, so that close
    /// finds nothing open.
    pub(crate) fn commit_project_name(&mut self) -> bool {
        let Some(input) = self.project_tab.take_name() else { return true };
        let Some(held) = self.project().map(|p| p.meta().name.clone()) else {
            return true;
        };
        let next = Some(input.text().trim()).filter(|n| !n.is_empty());
        if next == held.as_deref() {
            return true;
        }
        let t = self.begin_op();
        match self.project.as_mut().map(|p| p.set_project_name(next)) {
            Some(Ok(())) => {
                self.record_project_step(t, crate::undo::StepLabel::change("project name"));
                true
            }
            Some(Err(e)) => {
                self.op_in_flight = false;
                self.project_tab.restore_name(input);
                self.toasts.push(format!("cannot save the project name: {e}"), ToastKind::Error);
                false
            }
            None => {
                self.op_in_flight = false;
                true
            }
        }
    }

    /// Closes whatever is open on the Project tab, saved: the name or a
    /// header cell (close-input-first, R14). `false` when a write was
    /// refused and the input stays open.
    pub(crate) fn commit_project_tab_input(&mut self) -> bool {
        let name = self.commit_project_name();
        if self.project_tab.table.editing.is_none() {
            return name;
        }
        let refused = self.run_header_table(|t, map| t.commit(map)).1;
        name && !refused
    }

    /// The wheel over the tab, `delta` lines: the header table's view
    /// moves by the request table's rules ([`TableEditorState::scroll_view`],
    /// [`TableEditorState::view_scrolled`]).
    pub(super) fn scroll_project_table(&mut self, delta: i16) {
        let len = self.default_headers_len();
        let vim = self.profile() == crate::config::KeyProfile::Vim;
        let table = &mut self.project_tab.table;
        if table.scroll_view(delta, len, 0) {
            table.view_scrolled(vim, len, 0);
        }
    }

    /// `Action::AddDefaultHeader`: the `+ Add header` row's name opens,
    /// whatever was open committed first.
    pub(super) fn add_default_header(&mut self) {
        if !self.commit_project_tab_input() {
            self.last_action_failed = true;
            return;
        }
        self.project_tab.stop = ProjectStop::Table;
        self.project_tab.focused = true;
        self.run_header_table(|t, map| {
            t.begin_add(map);
            TableOutcome::default()
        });
    }

    /// `Action::DeleteDefaultHeader`: header `i` goes, one undo step, with
    /// the request table's "Deleted row" toast. The tab's cursor is drawn
    /// again, as for an add: a footer chip's click is a click away that
    /// dropped it. The cursor stays on what it was on: the Name row, `+ Add`,
    /// or the same header, which a delete above it moves up one. When the
    /// header under it is the one deleted (a delete key, or that row's
    /// pill), it lands on the row that moved up, as the table's own delete
    /// does.
    pub(super) fn delete_default_header(&mut self, i: usize) {
        if !self.commit_project_tab_input() {
            self.last_action_failed = true;
            return;
        }
        self.project_tab.focused = true;
        let key = self
            .project()
            .and_then(|p| p.meta().default_headers.get_index(i).map(|(k, _)| k.clone()));
        let refused = self
            .run_header_table(|t, map| {
                let kept = t.selected();
                t.delete_row(map, i);
                match kept {
                    Some(Stop::Row(k)) if k == i => {}
                    Some(Stop::Row(k)) => {
                        let k = if k > i { k - 1 } else { k };
                        t.set_selected(Some(Stop::Row(k).clamp(map.len(), 0)));
                    }
                    other => t.set_selected(other),
                }
                TableOutcome::default()
            })
            .1;
        if refused {
            self.last_action_failed = true;
        } else if let Some(key) = key {
            self.toast_deleted_row(&key, i);
        }
    }

    /// `Action::CopyDefaultHeader`: header `i`'s value to the clipboard, as
    /// a request row's copy pill copies (R13). The pill, `y` and vim's `yy`
    /// all come here: the tab has no put, so a yank is this copy, as on a
    /// request table's auto row.
    pub(super) fn copy_default_header(&mut self, i: usize) {
        let row = self.project().and_then(|p| {
            p.meta().default_headers.get_index(i).map(|(k, e)| (k.clone(), e.value.clone()))
        });
        if let Some((name, value)) = row {
            self.copy_text_with_toast(&value, format!("Copied {name}"));
        }
    }
}
