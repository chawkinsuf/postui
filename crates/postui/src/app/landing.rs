//! The one landing path (spec §4.2) and the view an undo step records
//! (§4.1, §4.3). Split from `app.rs`; every method here is `impl App`.
//!
//! `land` is the only code that changes what the editor holds, which
//! space is active and where the sidebar cursor sits — keyboard browsing,
//! clicks and the context-menu revert aside (spec G3).

use super::*;
use crate::components::manage::ManageTab;
use crate::components::sidebar::RowKey;
use crate::undo::{ListRow, Open, View};

/// Where a landing puts the sidebar cursor (§4.2 step 5).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CursorAim {
    /// On the open request's row; the first request row, then the first
    /// row, when nothing is open or its row is not in this space.
    OnOpen,
    /// On this row; [`CursorAim::OnOpen`] when it is not in this space.
    On(RowKey),
    /// The request row now at this index, else the nearest below, else
    /// the nearest above (`Sidebar::select_nearest_request`).
    Neighbour(usize),
}

/// What a landing changes. Every `None` part is left as it is.
#[derive(Debug, Clone, Default)]
pub(crate) struct Landing {
    pub space: Option<String>,
    pub open: Option<Open>,
    pub cursor: Option<CursorAim>,
    pub row: Option<ListRow>,
}

/// Proof that an op captured its before-view: `record_project_step` takes
/// one, so a step without a before-view does not compile (spec §4.3).
pub(crate) struct OpToken {
    before: View,
}

impl App {
    /// Opens a journaled op: flushes the open field session (its keys
    /// become their own step *before* the op, so nothing lands between
    /// the journal entry and its marker), then captures the before-view.
    /// The dirty gate's Discard reaches here with the edit still in the
    /// editor, so `before` carries it (OQ1).
    pub(super) fn begin_op(&mut self) -> OpToken {
        self.flush_field_session();
        self.op_in_flight = true;
        OpToken { before: self.view() }
    }

    /// Records a marker for the journal entry the op produced, if it
    /// produced a new one, with the view as the op left it. Called at the
    /// end of every journaled arm's success path, after its landing. A
    /// merged burst (a held alt+↓) leaves the top id unchanged and records
    /// nothing, so it stays one undo step; a burst that netted to nothing
    /// is popped by the journal and the marker already recorded for it is
    /// skipped as stale on undo.
    ///
    /// The step's toast reads `label` (spec R2), captured by the caller
    /// when the op ran so it never names a request that merely happened
    /// to be open, nor reads a name after the file is gone.
    pub(super) fn record_project_step(&mut self, t: OpToken, label: crate::undo::StepLabel) {
        self.op_in_flight = false;
        let top = self.journal_top();
        if top == self.marked_entry {
            // Nothing was journaled (a no-op call), or the call merged
            // into the entry the marker on top already covers.
            return;
        }
        // The journal's top went *backwards*: this call merged into the
        // entry the marker on top covers and netted to identity, so the
        // journal dropped that entry. Its marker goes with it — the entry
        // now on top already has one. (When something else was recorded
        // in between, the marker is not on top to pop; it is left where
        // it is and undo skips it as stale.) Either way this call records
        // nothing: the entry the journal fell back to already has a
        // marker somewhere in the stack, and a second one for it would
        // sit above the step that was recorded in between and undo out of
        // order.
        if self.marked_entry.is_some() && top.is_none_or(|t| Some(t) < self.marked_entry) {
            if matches!(
                self.history.peek_undo().map(|s| &s.kind),
                Some(crate::undo::StepKind::Project { id, .. }) if Some(*id) == self.marked_entry
            ) {
                self.history.pop_undo();
            }
            self.marked_entry = top;
            return;
        }
        self.marked_entry = top;
        let Some(id) = top else { return };
        let after = self.view();
        self.history.record_no_coalesce(crate::undo::Step {
            kind: crate::undo::StepKind::Project {
                id,
                label,
                before: Box::new(t.before),
                after: Box::new(after),
            },
            context: crate::undo::Context { slug: self.editor.slug.clone() },
        });
    }

    /// `change to {open request}` (spec R2's request-file row), or
    /// `fallback` when nothing is open.
    pub(super) fn open_request_label(&self, fallback: crate::undo::StepLabel) -> crate::undo::StepLabel {
        match self.editor.slug.as_deref() {
            Some(slug) => crate::undo::StepLabel::change(self.request_display(slug)),
            None => fallback,
        }
    }

    /// Brings the view to what an undo (or redo) of a project step says,
    /// after `Project` replayed its entry (spec §4.4). Replaces the
    /// special cases `after_undone` used to layer. `space_before` and
    /// `env_before` are the active space and environment as the app saw
    /// them just before the replay: core moves both by itself (the
    /// restore of `.local/state.toml`, and the entry's own `active_env`
    /// transition), so the view follows and says so in a switch's words.
    pub(super) fn after_replay(
        &mut self,
        u: &postui_core::project::Undone,
        before: &View,
        after: &View,
        reorder: bool,
        space_before: &str,
        env_before: Option<&str>,
    ) {
        // 1. Warnings, session renames, the Manager, the env toast. The
        //    landing below cancels live drags and rebuilds the sidebar.
        for w in &u.warnings {
            self.toasts.push(w.clone(), ToastKind::Warning);
        }
        // Every request the entry moved changes slug again: the session's
        // cache and in-flight entries follow, as they did for the forward
        // op.
        for (old, new) in &u.meta.moves {
            if u.redo {
                self.session.rename(old, new);
            } else {
                self.session.rename(new, old);
            }
        }
        // The Variable Manager's grid/form cache the declarations and
        // won't otherwise notice a file the replay rewrote under them.
        if self.screen == Screen::Manage {
            self.sync_varmanager();
        }
        // Core switched the active environment (creating one activates
        // it; deleting the active one falls to the next): announce it in
        // `Action::SwitchEnv`'s own words.
        if self.active_env() != env_before {
            let label = self.env_label_display();
            self.toasts.push(format!("env: {label}"), ToastKind::Success);
        }
        // 2. The open request moved (a rename, a move, a move-all): follow
        //    its slug without reloading, so an outside edit carried along
        //    is never laundered into "clean". The name comes from the
        //    listing the replay's reload rebuilt, not `open_request`,
        //    which would re-stamp the held entry without re-seeding the
        //    buffer.
        if let Some(open) = self.editor.slug.clone() {
            let moved_to = u.meta.moves.iter().find_map(|(old, new)| {
                if u.redo {
                    (*old == open).then(|| new.clone())
                } else {
                    (*new == open).then(|| old.clone())
                }
            });
            if let Some(new_slug) = moved_to {
                self.editor.slug = Some(new_slug.clone());
                let name = self
                    .project()
                    .and_then(|p| p.requests().iter().find(|l| l.slug == new_slug))
                    .and_then(|l| l.name.clone());
                if let Some(name) = name {
                    self.editor.name = Some(name.clone());
                    if let Some(saved) = self.editor.saved.as_mut() {
                        saved.name = Some(name);
                    }
                }
                // The shadow follows too, name included, so the next
                // capture sees no edit in the re-key.
                if let Some((s, req)) = self.shadow.as_mut()
                    && s.as_deref() == Some(open.as_str())
                {
                    *s = Some(new_slug.clone());
                    req.name = self.editor.name.clone();
                }
            }
        }
        // 3. The request the editor holds is gone: a scratch, which the
        //    `open` part below then corrects.
        let mut reset = false;
        if let Some(open) = self.editor.slug.clone()
            && !self.request_exists(&open)
        {
            self.editor = Editor::default();
            self.shadow = None;
            reset = true;
        }
        // 4. Land on each part the op changed, from the side we are going to.
        let target = if u.redo { after } else { before };
        let mut l = Landing::default();
        if before.space != after.space {
            l.space = Some(target.space.clone());
        }
        // The open part is re-landed only when the editor does not already
        // hold it: a request step 2 re-keyed in place is never reloaded
        // (C4), or an outside edit it carried would read as clean.
        if (reset || !before.open.same_target(&after.open))
            && !self.view().open.same_target(&target.open)
        {
            l.open = Some(target.open.clone());
        }
        if before.cursor != after.cursor {
            l.cursor = target.cursor.clone().map(CursorAim::On);
        } else if l.open.is_some() {
            // `land` defaults the cursor to the open row whenever `open`
            // changes; a cursor the op did not move stays where the user
            // has it (R4).
            l.cursor = self.sidebar.selected_key().map(CursorAim::On);
        }
        // A reorder keeps the row's identity but moves its index: always
        // re-place it.
        if before.row != after.row || reorder {
            l.row = target.row.clone();
        }
        if l.space.is_none()
            && l.open.is_none()
            && let Some(space) = self.editor.slug.as_deref().and_then(postui_core::storage::space_of)
            && space != self.active_space()
        {
            l.space = Some(space.to_string());
        }
        // 5. Land. Core may have moved the active space itself (the
        //    state.toml restore); say so if the landing did not.
        let active_before_land = self.active_space();
        self.land(l);
        if self.active_space() == active_before_land && active_before_land != space_before {
            self.toasts.push(
                format!("space: {}", self.space_name(&active_before_land)),
                ToastKind::Success,
            );
        }
        // The replay may have restored a journaled `state.toml` (a space
        // delete's), and a re-key or reset the landing left alone changed
        // what the editor holds: either way local state follows the
        // editor, as after every replay.
        if self.open_state_stale() {
            self.persist_open_request();
        }
    }

    /// The view an undo step records (§4.1).
    pub(super) fn view(&self) -> View {
        let buffer = self
            .editor_holds_unsaved()
            .then(|| Box::new(self.editor.current_request()));
        let open = match self.editor.slug.clone() {
            Some(slug) => Open::Request { slug, buffer },
            None => Open::Scratch { buffer },
        };
        View {
            space: self.active_space(),
            open,
            cursor: self.sidebar.selected_key(),
            row: self.list_row(),
        }
    }

    /// The Manage/Variables row under the cursor of the list on screen.
    fn list_row(&self) -> Option<ListRow> {
        if self.screen != Screen::Manage {
            return None;
        }
        match self.manage.tab {
            ManageTab::Environments => self.manage_selected(ManageTab::Environments).map(ListRow::Env),
            ManageTab::Spaces => self.manage_selected(ManageTab::Spaces).map(ListRow::Space),
            ManageTab::Variables => self.varmanager.detail.name().map(|n| ListRow::Var(n.to_string())),
            ManageTab::Settings => None,
        }
    }

    /// The one landing path (spec §4.2). Never refuses: callers that land
    /// before any write check for unsaved edits first; a journaled arm
    /// lands after its own dirty gate already ran. `false` only when the
    /// request `l.open` names failed to open (toasted; the editor is left
    /// where it was) or `l.space` names no space.
    pub(super) fn land(&mut self, l: Landing) -> bool {
        debug_assert!(
            !(self.op_in_flight && self.field_gate()),
            "a journaled op must flush the field session before landing (begin_op does)"
        );
        // 1. A live drag's working order names rows this may rewrite. Only
        //    a drag in progress is cancelled: an armed press stays armed,
        //    since a row press dispatches its own open through here.
        if self.sidebar.drag.is_some() {
            self.finish_sidebar_drag(false);
        }
        if self.manage.list.drag.is_some() {
            self.finish_manage_drag(false);
        }

        // 2. The space: the open request's, else the asked one, else the
        //    active one. The outgoing space remembers its open request.
        let target_space = l
            .open
            .as_ref()
            .and_then(|o| o.slug())
            .and_then(postui_core::storage::space_of)
            .map(str::to_string)
            .or_else(|| l.space.clone())
            .unwrap_or_else(|| self.active_space());
        if target_space != self.active_space() {
            let outgoing = self.active_space();
            let outgoing_exists = self.spaces().contains(&outgoing);
            let open_slug = self.editor.slug.clone();
            // An editor an undo re-keyed into another space (`after_replay`)
            // describes the destination, not the space being left: that
            // space keeps what it was left on.
            let describes_outgoing = open_slug
                .as_deref()
                .is_none_or(|s| postui_core::storage::space_of(s) == Some(outgoing.as_str()));
            let Some(p) = self.project.as_mut() else { return false };
            if outgoing_exists && describes_outgoing {
                p.record_space_open(open_slug.as_deref());
            }
            if !p.set_active_space(&target_space) {
                self.toasts
                    .push(format!("no space named {target_space:?}"), ToastKind::Warning);
                return false;
            }
            self.toasts.push(
                format!("space: {}", self.space_name(&target_space)),
                ToastKind::Success,
            );
        }

        // 3. What the editor holds. The band fades out from the open
        //    row only when that row is one of the active space's: after a
        //    switch the rows still show the outgoing space, and its index
        //    names nothing in the space being entered.
        let open_in_active = self.editor.slug.as_deref().and_then(postui_core::storage::space_of)
            == Some(self.active_space().as_str());
        let prev_open_row = self.sidebar.open_row().filter(|_| open_in_active);
        let mut landed = true;
        let asked_open = l.open.is_some();
        match l.open {
            Some(Open::Request { slug, buffer }) => {
                self.flush_field_session();
                let outgoing = self.editor.slug.clone();
                let Some(p) = self.project.as_mut() else { return false };
                if let Some(prev) = outgoing.as_deref().filter(|s| *s != slug) {
                    p.close_request(prev);
                }
                match p.open_request(&slug).cloned() {
                    Ok(req) => {
                        self.editor.load(Some(slug.clone()), req);
                        self.sync_active_tab();
                        if let Some(buf) = buffer {
                            self.editor.apply_snapshot(&buf);
                            self.shadow = Some((Some(slug.clone()), *buf));
                        }
                    }
                    Err(e) => {
                        // Seat nothing: the editor stays exactly where it
                        // was (05a3b4d).
                        self.toasts
                            .push(format!("could not open {slug}: {e}"), ToastKind::Error);
                        landed = false;
                    }
                }
            }
            Some(Open::Scratch { buffer }) => {
                self.flush_field_session();
                if let Some(prev) = self.editor.slug.clone()
                    && let Some(p) = self.project.as_mut()
                {
                    p.close_request(&prev);
                }
                self.editor = Editor::default();
                self.shadow = None;
                if let Some(buf) = buffer {
                    self.editor.apply_snapshot(&buf);
                    self.shadow = Some((None, *buf));
                }
            }
            None => {}
        }
        self.sidebar.open_slug = self.editor.slug.clone();
        // A failed open left the editor unchanged (§4.2 step 3), so the
        // open request did not change: no default aim, no persist.
        let open_changed = asked_open && landed;

        // 4. Rebuild once, with the cursor row's folders queued open.
        let aim = l.cursor.or(open_changed.then_some(CursorAim::OnOpen));
        let expand_for = match &aim {
            Some(CursorAim::On(RowKey::Request(slug))) => Some(slug.clone()),
            Some(CursorAim::OnOpen) => self.editor.slug.clone(),
            _ => None,
        };
        if let Some(slug) = expand_for {
            self.sidebar.expand_to(&slug);
        }
        self.refresh_sidebar();

        // 5. The cursor. An explicit aim wins; OnOpen is the default when
        //    the open request changed; with no aim, identity is kept.
        match aim {
            Some(CursorAim::On(key)) => {
                if !self.sidebar.select_key(&key) {
                    self.cursor_on_open();
                }
            }
            Some(CursorAim::OnOpen) => self.cursor_on_open(),
            Some(CursorAim::Neighbour(i)) => self.sidebar.select_nearest_request(i),
            None => {}
        }

        // 6. The Manage/Variables row, in its own list even when parked.
        if let Some(row) = l.row {
            self.place_list_row(row);
        }

        // 7. The travel band, and local state whenever it disagrees with
        //    the editor (a rename re-keys the open request without opening
        //    anything). A fresh project with nothing open is not stale, so
        //    a landing that opens nothing never materialises state.toml.
        self.retarget_sidebar_travel(prev_open_row);
        if self.open_state_stale() {
            self.persist_open_request();
        }
        landed
    }

    /// Whether local state names another open request than the editor's.
    /// The active space's memory counts only while the editor holds one
    /// of its requests (or nothing): see [`App::editor_in_active_space`].
    fn open_state_stale(&self) -> bool {
        self.project().is_some_and(|p| {
            p.local().open_request != self.editor.slug
                || (self.editor_in_active_space()
                    && p.space_open_for(&self.active_space()) != self.editor.slug)
        })
    }

    /// Whether what the editor holds belongs to the active space: a
    /// request of it, or nothing. Not so only after a failed cross-space
    /// open, which commits the switch but leaves the editor on the old
    /// space's request; that slug is never the new space's to remember.
    pub(super) fn editor_in_active_space(&self) -> bool {
        self.editor
            .slug
            .as_deref()
            .is_none_or(|s| postui_core::storage::space_of(s) == Some(self.active_space().as_str()))
    }

    /// The cursor on the open request's row, else the first request row,
    /// else the first row.
    fn cursor_on_open(&mut self) {
        let on_open = self
            .editor
            .slug
            .clone()
            .is_some_and(|s| self.sidebar.select_key(&RowKey::Request(s)));
        if !on_open {
            self.sidebar.select_first();
        }
    }

    /// Puts `row` under its own list's cursor, live or parked (spec R5).
    fn place_list_row(&mut self, row: ListRow) {
        match row {
            ListRow::Env(n) => self.select_list_row(ManageTab::Environments, &n),
            ListRow::Space(n) => self.select_list_row(ManageTab::Spaces, &n),
            ListRow::Var(n) => {
                let Self { project, varmanager, .. } = self;
                if let Some(p) = project.as_ref() {
                    varmanager.sync(p);
                    varmanager.select_name(&n);
                }
            }
        }
    }
}
