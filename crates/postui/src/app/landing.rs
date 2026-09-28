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
    #[allow(dead_code)] // used from Task 9
    On(RowKey),
    /// The request row now at this index, else the nearest below, else
    /// the nearest above (`Sidebar::select_nearest_request`).
    #[allow(dead_code)] // used from Task 9
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

impl App {
    /// The view an undo step records (§4.1).
    #[allow(dead_code)] // used from Task 9 (`begin_op`)
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
            let Some(p) = self.project.as_mut() else { return false };
            if outgoing_exists {
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

        // 7. The travel band, and local state (only when it changed, so a
        //    landing that opens nothing never materialises state.toml).
        self.retarget_sidebar_travel(prev_open_row);
        let stale = self.project().is_some_and(|p| {
            p.local().open_request != self.editor.slug
                || p.space_open_for(&self.active_space()) != self.editor.slug
        });
        if open_changed && stale {
            self.persist_open_request();
        }
        landed
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
