//! Mouse routing: raw event handling, hover tracking, click dispatch
//! (`on_hit`), and scrollbar-thumb drags. Split from `app.rs`; every
//! method here is `impl App` and shares its state directly.

use super::*;
use crate::components::manage::ManageTab;

impl App {
    /// Routes a raw terminal mouse event against `self.hits`, the `HitMap`
    /// `ui::draw` rebuilt on the last frame. No layout is needed any more:
    /// every clickable region — pane background, button, chip — was
    /// registered there already, topmost-wins.
    ///
    /// - `Moved` resolves the hit under the pointer and, if it differs from
    ///   `self.hovered`, stores it and asks for a redraw (so hover styling
    ///   updates); the same hit twice in a row is a no-op.
    /// - `Down(Left)` resolves the hit, tracks single vs. double click (same
    ///   hit within 400ms), and dispatches through `on_hit`.
    /// - `Up(Left)` clears any in-progress drag.
    /// - Wheel events scroll the body editor when over it, else scroll the
    ///   pane under the pointer. While a modal is open, wheel is a no-op
    ///   here — modal-list scrolling is a later task.
    pub fn handle_mouse(&mut self, m: ratatui::crossterm::event::MouseEvent) -> bool {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};

        // Every event carries the pointer's position, motion reports or
        // not: a press moves the pointer just as truly as a `Moved` does.
        // Recording it here keeps the `{{token}}` tooltip (which is drawn
        // from `pointer`) honest in terminals that report no motion, where
        // a tip opened by one hover would otherwise hang over the UI
        // through every later click — including the clicks it covers.
        self.pointer = Some((m.column, m.row));

        match m.kind {
            // Terminals report pointer motion with a button held as `Drag`,
            // not `Moved`, so a thumb drag arrives as either depending on
            // whether the terminal tracks button state; both drive the drag.
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = self.drag.as_ref() {
                    return if drag.horizontal {
                        self.drag_to_h(m.column)
                    } else {
                        self.drag_to(m.row)
                    };
                }
                if m.kind != MouseEventKind::Moved {
                    // Button-held motion: a text-selection sweep if one was
                    // armed by the press that started it, otherwise not a
                    // hover update.
                    return match self.text_drag {
                        Some(TextDrag::Body) => self.editor.body_drag_to(m.column, m.row),
                        Some(TextDrag::Response) => {
                            self.session.response.drag_selection_to(m.column, m.row)
                        }
                        Some(TextDrag::Url) => self.url_drag_to(m.column),
                        Some(TextDrag::Jq) => self.jq_drag_to(m.column),
                        Some(TextDrag::ModalInput(i)) => self.modal_input_drag_to(i, m.column),
                        Some(TextDrag::TableCell) => self.table_cell_drag_to(m.column),
                        Some(TextDrag::VmField) => self.vm_field_drag_to(m.column),
                        Some(TextDrag::VmCell) => self.vm_cell_drag_to(m.column),
                        None => false,
                    };
                }
                // Two independent hover tracks: the control under the
                // pointer (styling), and any `{{token}}` drawn on top of it
                // (the value tooltip). A token must not steal its control's
                // hover styling, and leaving one must drop the tooltip on
                // the very next motion event.
                let hit = self
                    .hits
                    .hit_at_ignoring_var_tokens(m.column, m.row)
                    .cloned();
                let token = self
                    .hits
                    .var_token_at(m.column, m.row)
                    .map(|(name, _)| name.to_string());
                // The pointer's exact position (recorded above) is not part
                // of "changed": the tooltip is anchored at the token's own
                // drawn rect, so moving within one token changes nothing.
                let hit_changed = hit != self.hovered;
                let changed = hit_changed || token != self.hovered_token;
                self.hovered = hit;
                self.hovered_token = token;
                if hit_changed {
                    self.begin_hover_fade();
                }
                changed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self.hits.hit_at(m.column, m.row).cloned();
                // A click anywhere but the jq bar itself is Enter for a
                // focused bar: the filter is already applied (every edit
                // re-ran it), so all that is left to do is leave the bar.
                // Modal and scrollbar hits are exempt, like every other
                // click-away rule: a popup over the pane or a scroll must
                // not pull the caret out from under what was being typed.
                let jq_blurred = self.session.response.jq_focused()
                    && self.modals.is_empty()
                    && !matches!(
                        hit,
                        Some(
                            Hit::ResponseJqBar
                                | Hit::ScrollbarThumb(_)
                                | Hit::ScrollbarTrack(..)
                                | Hit::HScrollThumb(_)
                                | Hit::HScrollTrack(..)
                        )
                    );
                if jq_blurred {
                    self.session.response.set_jq_focus(false);
                }
                let Some(hit) = hit else {
                    // Bare background is still a click *away* from an
                    // in-place edit. The Variable Manager has plenty of it
                    // — under the last grid row, beside the title buttons —
                    // and landing there must save what was typed, the same
                    // as landing on a control (`on_hit`'s commit-first
                    // rule). Without this the typed text just hangs there
                    // with no way to tell it isn't saved.
                    let live = self.varmanager.form.editing.is_some()
                        || self.varmanager.grid.editing.is_some();
                    if !live {
                        return jq_blurred;
                    }
                    self.commit_var_form();
                    self.commit_grid_edit();
                    return self.update(Action::Render);
                };
                // The testbed is a dead end for the mouse exactly like it is
                // for the keyboard (see `App::handle_key`'s `Screen::Testbed`
                // branch): every click is inert except the footer's quit
                // chip. Without this, the header/footer chrome — drawn and
                // hit-registered unconditionally on every screen — would let
                // a click open the palette, a project/env chooser, or the
                // Variable Manager from underneath the showcase, with no way
                // back short of quitting.
                if self.screen == Screen::Testbed && hit != Hit::FooterChip(Action::Quit) {
                    return false;
                }
                let now = std::time::Instant::now();
                let clicks = match &self.last_click {
                    Some((last_hit, at))
                        if *last_hit == hit && now.duration_since(*at).as_millis() < 400 =>
                    {
                        2
                    }
                    _ => 1,
                };
                // Clear on a double so a third click within the window
                // starts a fresh count as a single, rather than pairing
                // with the second click and double-firing (e.g. a fast
                // triple-click toggling a folder twice, net reverting it).
                self.last_click = if clicks == 2 {
                    None
                } else {
                    Some((hit.clone(), now))
                };
                let was_content =
                    self.editor.sub_focus == crate::components::editor::SubFocus::Content;
                let changed = self.on_hit(hit, clicks, m) | jq_blurred;
                if !was_content
                    && self.editor.sub_focus == crate::components::editor::SubFocus::Content
                {
                    self.begin_focus_fade();
                }
                changed
            }
            // A right click targets the row under the pointer: it moves the
            // selection there first (so the menu's flows, which all read the
            // selection, act on what was clicked), then opens whatever menu
            // that hit offers. Hits with no menu still get the selection
            // move — right-clicking a row selects it, menu or not. For
            // sidebar rows the move is provisional: dismissing the menu
            // without choosing anything restores the previous selection
            // (see `App::sidebar_menu_revert`).
            MouseEventKind::Down(MouseButton::Right) => {
                // Tokens are a left-click affordance only: a right click
                // belongs to the row/cell under them and its context menu.
                let Some(mut hit) = self
                    .hits
                    .hit_at_ignoring_var_tokens(m.column, m.row)
                    .cloned()
                else {
                    return false;
                };
                let mut changed = false;
                // An open context menu doesn't own the right button the way
                // it owns the left: a second right click re-targets. It
                // dismisses the menu through `Action::Close` (running the
                // same sidebar-selection revert any click-away does), then
                // acts on whatever sits under the pointer beneath the
                // overlay — opening that row's menu, or nothing if there's
                // nothing to menu. Only the dropdown gets this treatment;
                // dialog modals keep the right button inert.
                if hit == Hit::ModalOutside && matches!(self.modals.top(), Some(Modal::Dropdown(_)))
                {
                    changed |= self.update(Action::Close);
                    match self.hits.hit_at_under_modal(m.column, m.row).cloned() {
                        Some(under) => hit = under,
                        None => return changed,
                    }
                }
                // A text surface — the URL bar, the body, the response
                // text, or the one cell/field currently under edit — gets
                // the Copy/Paste menu instead of a row menu, and skips the
                // click-away commits below (which would end the edit whose
                // selection Copy reads). Any other part of a row falls
                // through to the row menu exactly as before.
                if let Some(items) = self.text_surface_menu(&hit) {
                    return self.open_context_menu(m.column, m.row, items) || changed;
                }
                // A right click is a click away from whatever detail-pane
                // cell was being typed into, exactly like a left click
                // (see `on_hit`'s blanket rule): commit it *before*
                // anything below reads or reshapes the rows underneath.
                // Skipping this let a menu action (Delete…, Rename…) shift
                // the option rows out from under a still-live `GridEdit`,
                // whose row index would then address a different record —
                // and the next click-away would write the typed text into
                // that wrong option.
                self.commit_var_form();
                self.commit_grid_edit();
                // A commit that *failed* keeps its edit (spec §5: the typed
                // text survives), so the row indices it holds must stay
                // meaningful: offer no menu that could renumber them until
                // the user has dealt with it. The failure already toasted.
                if self.varmanager.grid.editing.is_some()
                    && matches!(hit, Hit::VmEntryCell { .. } | Hit::VmEntryRadio(_))
                {
                    return self.update(Action::Render);
                }
                // Same rule for the left list: selecting a different row
                // after a failed form/grid commit would reset `form`/`grid`
                // and discard the typed text the failure left live.
                if (self.varmanager.form.editing.is_some()
                    || self.varmanager.grid.editing.is_some())
                    && matches!(hit, Hit::VmLeftRow(_))
                {
                    return self.update(Action::Render);
                }
                // A table hit is normalized to `TableRow(resolved)` for the
                // menu lookup below: right-clicking any part of the row (a
                // cell, the checkbox, the ✕) opens the same row menu, and
                // `resolve_table_row_across_commit` re-numbers `i` past
                // whatever commit just landed — see its own doc comment.
                let mut menu_hit = hit.clone();
                // For sidebar rows: the selection to restore if the menu
                // opens and is then dismissed without choosing anything
                // (see `App::sidebar_menu_revert`).
                let mut menu_revert = None;
                match &hit {
                    Hit::SidebarRow(i) | Hit::SidebarFolderArrow(i) => {
                        changed |= self.update(Action::FocusPane(PaneId::Sidebar));
                        changed |= self.sidebar.selected != Some(*i);
                        menu_revert = Some(self.sidebar.selected);
                        self.set_sidebar_selected(*i);
                    }
                    Hit::VmLeftRow(i) => {
                        changed |= self.varmanager.left_cursor != *i;
                        self.varmanager.select_row(*i);
                    }
                    // An Environments/Spaces row: the click lands the
                    // list's cursor on it (the detail pane follows) before
                    // its menu opens, like a left click would.
                    Hit::ManageRow(i) => {
                        changed |= self.manage.list.cursor != *i;
                        self.manage.list.cursor = *i;
                    }
                    // Same rule for a grid row: the click moves the grid's
                    // cursor onto it (and the keyboard with it) before its
                    // menu opens.
                    Hit::VmEntryCell { row, .. } | Hit::VmEntryRadio(row) => {
                        let col = match &hit {
                            Hit::VmEntryCell { col, .. } => *col,
                            _ => 0,
                        };
                        changed |= self.varmanager.grid.cursor != (*row, col)
                            || self.varmanager.focus != VmFocus::Grid;
                        self.varmanager.grid.cursor = (*row, col);
                        self.varmanager.focus = VmFocus::Grid;
                    }
                    Hit::TableRow(i)
                    | Hit::TableCheckbox(i)
                    | Hit::TableDelete(i)
                    | Hit::TableCell { row: i, .. } => {
                        let i = *i;
                        changed |= self.update(Action::FocusPane(PaneId::Editor));
                        self.editor.sub_focus = SubFocus::Content;
                        match self.resolve_table_row_across_commit(i) {
                            Some(resolved) => {
                                changed |= self.editor.table.selected != Some(resolved);
                                self.editor.table.selected = Some(resolved);
                                menu_hit = Hit::TableRow(resolved);
                            }
                            // The clicked row was a ghost row that just
                            // discarded itself on commit: nothing left to
                            // menu.
                            None => return self.update(Action::Render) || changed,
                        }
                    }
                    _ => {}
                }
                match self.context_menu_for(&menu_hit) {
                    Some(items) => {
                        let opened = self.open_context_menu(m.column, m.row, items);
                        if opened {
                            self.sidebar_menu_revert = menu_revert;
                        }
                        opened || changed
                    }
                    None => changed,
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Releasing the button ends both drag kinds; a finished
                // text sweep keeps its selection, only the sweep state ends.
                let had_text = self.text_drag.take().is_some();
                self.drag.take().is_some() || had_text
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if !self.modals.is_empty() {
                    let d = if m.kind == MouseEventKind::ScrollUp {
                        -3
                    } else {
                        3
                    };
                    return self.modals.scroll_top(d);
                }
                if matches!(self.hits.hit_at(m.column, m.row), Some(Hit::BodyEditor))
                    && self.editor.handle_mouse(m)
                {
                    return self.update(Action::Render);
                }
                if self.screen == Screen::Manage {
                    let d = if m.kind == MouseEventKind::ScrollUp {
                        -3
                    } else {
                        3
                    };
                    if self.manage.tab == ManageTab::Variables {
                        self.varmanager.handle_scroll_at(m.column, m.row, d);
                    } else {
                        self.manage.list.handle_scroll(d);
                    }
                    return self.update(Action::Render);
                }
                if let Some(pane) = self.hits.pane_at(m.column, m.row) {
                    let d = if m.kind == MouseEventKind::ScrollUp {
                        -3
                    } else {
                        3
                    };
                    return self.update(Action::ScrollPane(pane, d));
                }
                false
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                // A sideways wheel notch (shift+wheel in most terminals)
                // moves the response viewport over its clipped columns.
                // Modals and the Variable Manager have nothing to scroll
                // sideways, and neither does any other pane.
                if !self.modals.is_empty() || self.screen == Screen::Manage {
                    return false;
                }
                if self.hits.pane_at(m.column, m.row) == Some(PaneId::Response) {
                    let d = if m.kind == MouseEventKind::ScrollLeft {
                        -crate::components::response::H_SCROLL_STEP
                    } else {
                        crate::components::response::H_SCROLL_STEP
                    };
                    self.session.response.handle_scroll_h(d);
                    return self.update(Action::Render);
                }
                false
            }
            _ => false,
        }
    }

    /// Re-resolves `hovered`/`hovered_token` from the last known pointer
    /// position against the just-rebuilt frame's hit map. The move handler
    /// only hit-tests on actual motion, so without this a control that
    /// appears (or vanishes) *under* a stationary pointer — a hover-revealed
    /// row button, a pane's action pills after a view switch — would keep
    /// stale hover styling until the mouse next moved. Called after every
    /// frame draw; returns whether anything changed (the caller schedules a
    /// repaint). Inert mid-drag, matching the move handler.
    pub fn resync_hover(&mut self) -> bool {
        if self.drag.is_some() || self.text_drag.is_some() {
            return false;
        }
        let Some((x, y)) = self.pointer else {
            return false;
        };
        let hit = self.hits.hit_at_ignoring_var_tokens(x, y).cloned();
        let token = self
            .hits
            .var_token_at(x, y)
            .map(|(name, _)| name.to_string());
        let hit_changed = hit != self.hovered;
        let changed = hit_changed || token != self.hovered_token;
        self.hovered = hit;
        self.hovered_token = token;
        if hit_changed {
            self.begin_hover_fade();
        }
        changed
    }

    /// Recomputes the pointer shape (task 8d) from `self.hovered` — the same
    /// hover state `ui::draw` already styles from, so this piggybacks on the
    /// existing hover-change path rather than running its own hit test.
    /// Returns the new shape only when it differs from the one last emitted,
    /// which the caller (main.rs's event loop) writes as a Kitty OSC 22
    /// escape after the frame draws; `None` means nothing to write this
    /// frame. Pure aside from updating `last_pointer_shape`, so it's
    /// testable without a terminal.
    pub fn pointer_shape_update(&mut self) -> Option<PointerShape> {
        let shape = PointerShape::for_hit(self.hovered.as_ref());
        if shape == self.last_pointer_shape {
            return None;
        }
        self.last_pointer_shape = shape;
        Some(shape)
    }

    /// The scroll state `pane` would draw a scrollbar from right now — the
    /// same [`ScrollbarSpec`] its `draw` builds, so drag math and the drawn
    /// thumb can never disagree. `None` when the pane has nothing scrollable
    /// (or has not been drawn yet).
    pub fn scrollbar_spec(&self, pane: PaneId) -> Option<ScrollbarSpec> {
        // The Variable Manager screen replaces the whole body, sidebar
        // included: while it is up, the sidebar's pane slot belongs to its
        // left list (see `VarManager::scrollbar_spec`).
        if self.screen == Screen::Manage {
            // The other tabs' list draws no scrollbar of its own, so the
            // sidebar's pane slot has nothing to report there.
            if self.manage.tab != ManageTab::Variables {
                return None;
            }
            return self.varmanager.scrollbar_spec().filter(|s| s.pane == pane);
        }
        match pane {
            PaneId::Sidebar => self.sidebar.scrollbar_spec(),
            PaneId::Editor => self.editor.scrollbar_spec(),
            PaneId::Response => self.session.response.scrollbar_spec(),
        }
    }

    /// Applies an in-progress thumb drag: turns the pointer's row into a
    /// thumb top within the dragged pane's track, maps that back to a content
    /// offset, and moves the pane there. Returns true when it moved.
    fn drag_to(&mut self, row: u16) -> bool {
        let Some(drag) = self.drag.as_ref() else {
            return false;
        };
        let pane = drag.pane;
        let Some(track) = self.hits.track_of(pane) else {
            return false;
        };
        let Some(spec) = self.scrollbar_spec(pane) else {
            return false;
        };
        let top = row
            .saturating_sub(track.y)
            .saturating_sub(drag.grab_offset)
            .min(track.height);
        let offset = crate::hit::offset_for_thumb_top(&spec, track.height, top);
        if offset == spec.offset {
            return false;
        }
        if self.screen == Screen::Manage {
            if self.manage.tab != ManageTab::Variables {
                return false;
            }
            self.varmanager.set_scroll(offset);
            return true;
        }
        match pane {
            PaneId::Sidebar => {
                self.sidebar.scroll = offset;
                // Dragging the viewport is an explicit gesture, exactly like
                // the wheel: the selection must not drag it back.
                self.sidebar.ensure_visible = false;
                true
            }
            PaneId::Response => self.session.response.set_scroll(offset),
            PaneId::Editor => {
                // edtui owns the body's viewport and only exposes moving it
                // by one wheel notch at a time (which also keeps its cursor
                // inside the viewport); feed it the difference.
                let delta =
                    (offset as i64 - spec.offset as i64).clamp(i16::MIN as i64, i16::MAX as i64);
                self.editor.handle_scroll(delta as i16);
                self.editor.scrollbar_spec().map(|s| s.offset) != Some(spec.offset)
            }
        }
    }

    /// Applies an in-progress horizontal thumb drag: turns the pointer's
    /// column into a thumb left-edge within the dragged pane's bottom
    /// track, maps that back to a column offset, and moves the pane there.
    /// The sideways twin of `drag_to`; `thumb_geometry`/`offset_for_thumb_top`
    /// are axis-agnostic lengths, so the same math serves both.
    fn drag_to_h(&mut self, column: u16) -> bool {
        let Some(drag) = self.drag.as_ref() else {
            return false;
        };
        let pane = drag.pane;
        let Some(track) = self.hits.h_track_of(pane) else {
            return false;
        };
        // Only the Response pane draws a horizontal bar today.
        let Some(spec) = (match pane {
            PaneId::Response => self.session.response.h_scrollbar_spec(),
            _ => None,
        }) else {
            return false;
        };
        let left = column
            .saturating_sub(track.x)
            .saturating_sub(drag.grab_offset)
            .min(track.width);
        let offset = crate::hit::offset_for_thumb_top(&spec, track.width, left);
        if offset == spec.offset {
            return false;
        }
        self.session.response.set_scroll_h(offset)
    }

    /// Extends a URL-bar selection sweep to the pointer's column: maps it
    /// back through the same window math the click used (the input is
    /// focused mid-sweep, so the window follows its caret) and moves the
    /// caret while keeping the anchor.
    fn url_drag_to(&mut self, column: u16) -> bool {
        let Some(area) = self.editor.last_url_text_area else {
            return false;
        };
        if area.width == 0 {
            return false;
        }
        let start = self.editor.url.window_start(true, area.width);
        let col = usize::from(column.clamp(area.x, area.x + area.width - 1) - area.x);
        self.editor.url.extend_mouse_selection_to(start + col);
        true
    }

    /// Like [`Self::url_drag_to`], for the jq filter bar.
    fn jq_drag_to(&mut self, column: u16) -> bool {
        let Some(area) = self.hits.rect_of(&Hit::ResponseJqBar) else {
            return false;
        };
        let (text_x, inner_w) = jq_bar_text_geometry(area);
        if inner_w == 0 {
            return false;
        }
        let input = &mut self.session.response.jq_bar_mut().input;
        let start = input.window_start(true, inner_w);
        let col = usize::from(column.clamp(text_x, text_x + inner_w - 1) - text_x);
        input.extend_mouse_selection_to(start + col);
        true
    }

    /// Like [`Self::url_drag_to`], for the top modal's text box `i`: maps
    /// the pointer's column back through the same window math the click
    /// used (the input draws focused mid-sweep) and extends the selection.
    fn modal_input_drag_to(&mut self, i: usize, column: u16) -> bool {
        let Some(area) = self.hits.rect_of(&Hit::ModalInput(i)) else {
            return false;
        };
        let inner_w = area.width.saturating_sub(2);
        if inner_w == 0 {
            return false;
        }
        let Some(input) = self.modals.focus_input(i) else {
            return false;
        };
        let start = input.window_start(true, inner_w);
        let text_x = area.x + 2;
        let col = usize::from(column.clamp(text_x, text_x + inner_w - 1) - text_x);
        input.extend_mouse_selection_to(start + col);
        true
    }

    /// Like [`Self::url_drag_to`], for the table cell under edit. The cell
    /// draws its input unwindowed at the hit rect's left edge, so the
    /// mapping is a plain column offset.
    fn table_cell_drag_to(&mut self, column: u16) -> bool {
        let Some(edit) = self.editor.table.editing.as_mut() else {
            return false;
        };
        let hit = Hit::TableCell {
            row: edit.row,
            col: edit.col.index(),
        };
        let Some(area) = self.hits.rect_of(&hit) else {
            return false;
        };
        if area.width == 0 {
            return false;
        }
        let col = usize::from(column.clamp(area.x, area.x + area.width - 1) - area.x);
        edit.input.extend_mouse_selection_to(col);
        true
    }

    /// Like [`Self::modal_input_drag_to`], for the variable form's field
    /// under edit — the same `TextField` geometry (text starts 2 columns
    /// in, windowed to `width - 2`).
    fn vm_field_drag_to(&mut self, column: u16) -> bool {
        let Some((field, input)) = self.varmanager.form.editing.as_mut() else {
            return false;
        };
        let Some(area) = self.hits.rect_of(&Hit::VmFormField(*field)) else {
            return false;
        };
        let inner_w = area.width.saturating_sub(2);
        if inner_w == 0 {
            return false;
        }
        let start = input.window_start(true, inner_w);
        let text_x = area.x + 2;
        let col = usize::from(column.clamp(text_x, text_x + inner_w - 1) - text_x);
        input.extend_mouse_selection_to(start + col);
        true
    }

    /// Like [`Self::url_drag_to`], for the selector-grid cell under edit
    /// (drawn windowed at the hit rect's own left edge).
    fn vm_cell_drag_to(&mut self, column: u16) -> bool {
        let Some(edit) = self.varmanager.grid.editing.as_mut() else {
            return false;
        };
        let hit = Hit::VmEntryCell {
            row: edit.row,
            col: edit.col,
        };
        let Some(area) = self.hits.rect_of(&hit) else {
            return false;
        };
        if area.width == 0 {
            return false;
        }
        let start = edit.input.window_start(true, area.width);
        let col = usize::from(column.clamp(area.x, area.x + area.width - 1) - area.x);
        edit.input.extend_mouse_selection_to(start + col);
        true
    }

    /// Commits any in-progress table cell edit, surfacing its warning as a
    /// toast. The one place typing is turned into map data outside the
    /// table's own key handling — click-away, focus loss, tab switch, save
    /// and send all go through here so a typed cell is never dropped.
    pub(crate) fn commit_table_edit(&mut self) {
        if self.editor.table.editing.is_none() {
            return;
        }
        if let Some(w) = self.editor.commit_table().warning {
            self.toasts.push(w, ToastKind::Warning);
        }
    }

    /// Commits the in-progress edit and re-resolves row `i` — an index from
    /// the last frame's hit map — afterwards. A duplicate-key commit
    /// `shift_remove`s a row, shifting every later index down, so acting on
    /// the raw `i` would hit the neighbour. `None` when the row the user
    /// clicked no longer exists (it was the one collapsed away).
    fn resolve_table_row_across_commit(&mut self, i: usize) -> Option<usize> {
        let edited_row = self.editor.table.editing.as_ref().map(|e| e.row);
        let key = self.editor.table_key_at(i);
        self.commit_table_edit();
        match edited_row {
            // Nothing was being edited: the index is still good.
            None => Some(i),
            // The clicked row is the one that just committed — its key may
            // have changed under us, so take the row the commit resolved to
            // (a discarded ghost row resolves to nothing to act on).
            Some(r) if r == i => self
                .editor
                .table
                .selected
                .filter(|s| *s < self.editor.table_len()),
            // Another row committed; that commit may have collapsed rows,
            // so re-resolve by the key `i` named before it ran.
            Some(_) => match key {
                Some(k) => self.editor.table_index_of(&k),
                // The ghost row has no key; it sits at the new length.
                None => Some(self.editor.table_len()),
            },
        }
    }

    /// A left click landing on response body text: a single click anchors
    /// a cell-wise sweep, a double click selects the word under it (and
    /// arms a word-wise sweep). Returns whether anything anchored — the
    /// caller arms `TextDrag::Response` on `true`.
    fn response_click_selection(
        &mut self,
        clicks: u8,
        m: ratatui::crossterm::event::MouseEvent,
    ) -> bool {
        if clicks == 2 {
            self.session.response.select_word_at(m.column, m.row)
        } else {
            self.session.response.begin_selection_at(m.column, m.row)
        }
    }

    /// The `▼`/`▲` search buttons: focus the response pane and step the
    /// match cycle, exactly as `n`/`N` do.
    fn step_response_search(&mut self, delta: i32) -> bool {
        self.update(Action::FocusPane(PaneId::Response));
        self.session.response.step_search(delta);
        self.update(Action::Render)
    }

    /// Mirrors every keybound (`keys::named_actions`) action that `on_hit`
    /// below dispatches directly — i.e. reachable by a click that isn't
    /// already counted via a footer/toolbar chip, a context menu, or a
    /// palette command. Feeds `app::tests`'s mouse-parity sweep (spec §5):
    /// that test can't reflect over `on_hit`'s match arms, so this list is
    /// hand-kept beside it instead. Add a `Hit` arm below that fires a
    /// keybound action directly, and add the same action here in the same
    /// change — otherwise the parity test starts failing for it.
    #[cfg(test)]
    pub(crate) fn mouse_dispatch_mirror() -> Vec<Action> {
        vec![
            Action::Close,              // Hit::ModalOutside
            Action::OpenProjectChooser, // Hit::HeaderProject
            Action::CycleProject(1),    // Hit::HeaderProjectCycle
            Action::OpenSpaceChooser,   // Hit::HeaderSpace
            Action::CycleSpace(1),      // Hit::HeaderSpaceCycle
            Action::OpenEnvChooser,     // Hit::HeaderEnv
            Action::CycleEnv(1),        // Hit::HeaderEnvCycle
            // The split control's own keycap pill (editor tab-bar row)
            // registers Hit::FooterChip(CycleSplit) outside `footer_chips`,
            // so the parity sweep's Group A never sees it.
            Action::CycleSplit,
            Action::SplitStep(1),  // Hit::SplitStep (response header ▲)
            Action::SplitStep(-1), // Hit::SplitStep (response header ▼)
            Action::OpenManage { tab: None }, // Hit::HeaderManage
            Action::OpenThemeChooser, // Hit::HeaderTheme
            Action::OpenMethodDropdown, // Hit::MethodSelector
            Action::FocusUrl,      // Hit::UrlBar
            Action::Send,          // Hit::SendButton (not in flight)
            Action::EditorTabSelect(0), // Hit::EditorTab, any draw position
            Action::EditorTabSelect(1), // (converted through
            Action::EditorTabSelect(2), //  EditorTab::from_draw_position(..).index())
            Action::EditorTabSelect(3),
            // The Environments/Spaces tabs' `+ New` button (Hit::ManageNew);
            // the rest of that face's hits either dispatch per-item actions
            // built from the live project (rename/delete/move/move-all) or
            // change list state only.
            Action::OpenNewEnvPrompt,
            Action::OpenNewSpacePrompt,
        ]
    }

    /// The central click dispatch: maps a resolved `Hit` (plus click count
    /// and the raw event, for hits that need to forward it) to app state
    /// changes. Only `Pane` and `BodyEditor` are wired up so far; later
    /// tasks extend this match as more hit kinds gain behavior.
    fn on_hit(&mut self, hit: Hit, clicks: u8, m: ratatui::crossterm::event::MouseEvent) -> bool {
        // A click anywhere that isn't the params/headers table itself (and
        // isn't inside a modal) is a
        // click away: it commits whatever cell was being edited (typing is
        // never silently thrown away) and clears the table selection.
        let keeps_table_selection = matches!(
            hit,
            Hit::TableRow(_)
                | Hit::TableCheckbox(_)
                | Hit::TableDelete(_)
                | Hit::TableCell { .. }
                | Hit::SplitStop(_)
                | Hit::SplitStep(_)
                | Hit::ModalCancel
                | Hit::ModalConfirm
                | Hit::ConfirmChoice(_)
                | Hit::ModalBody
                | Hit::ModalField(_)
                | Hit::ModalChoiceArrow { .. }
                | Hit::ModalRowToggle(_)
                | Hit::ModalAddRow
                | Hit::ModalSharedToggle
                | Hit::ModalRemove
                | Hit::ModalOutside
                | Hit::DropdownRow(_)
                | Hit::ChooserRow(_)
                | Hit::ChooserToggle
                | Hit::PaletteRow(_)
                | Hit::VarPickerRow(_)
                | Hit::ScrollbarThumb(_)
                | Hit::ScrollbarTrack(..)
                | Hit::HScrollThumb(_)
                | Hit::HScrollTrack(..)
                // A token sits *on* a cell: hovering or clicking one must
                // neither commit the cell under edit nor drop the selection.
                | Hit::VarToken(_)
                // The `{{ }} vars` chip inserts into whatever text field is
                // live, so it must not be treated as a click away: blurring
                // the URL line (or committing the cell) first would leave
                // the picker it opens with nowhere to insert.
                | Hit::FooterChip(Action::OpenVarPicker { .. })
        );
        if !keeps_table_selection {
            self.commit_table_edit();
            self.editor.table.selected = None;
        }
        // Same rule for the variable form's own in-place field: any click
        // that isn't on the field already under edit (or the reveal
        // toggle, which must not disturb an edit in progress) commits
        // whatever is being typed there first — including a click on a
        // *different* form field, which must never silently overwrite
        // `form.editing` out from under the field that was live.
        let editing_this_field = matches!(hit, Hit::VmFormField(field)
            if self.varmanager.form.editing.as_ref().is_some_and(|(f, _)| *f == field));
        if !editing_this_field && !matches!(hit, Hit::VmRevealToggle) {
            self.commit_var_form();
        }
        // …and the selector grid's cell, likewise: any click that isn't on the
        // cell already under edit commits it first, so clicking straight
        // from one cell to another never drops what was typed into the
        // first (Task 8's commit-first rule).
        let editing_this_cell = matches!(hit, Hit::VmEntryCell { row, col }
            if self.varmanager.grid.editing.as_ref().is_some_and(|e| e.row == row && e.col == col));
        if !editing_this_cell {
            self.commit_grid_edit();
        }
        // Likewise, clicking away blurs whichever editor input is active
        // (URL line / table / body). Hits that themselves place the
        // sub-focus (UrlBar, BodyEditor, the table hits) re-set it right
        // after; modal and scrollbar hits are excluded above so an open
        // popup or a scroll never blurs the input under it.
        let keeps_editor_input = keeps_table_selection
            || matches!(
                hit,
                Hit::UrlBar | Hit::BodyEditor | Hit::VarToken(_) | Hit::CopyUrl
            );
        if !keeps_editor_input {
            self.editor.sub_focus = SubFocus::None;
        }
        match hit {
            Hit::Pane(p) => {
                // A click on the response pane's bare content (Raw and
                // Headers register no per-row hits) anchors a selection
                // sweep, exactly like a JsonRow click does in Pretty.
                if p == PaneId::Response && self.response_click_selection(clicks, m) {
                    self.text_drag = Some(TextDrag::Response);
                }
                self.update(Action::FocusPane(p))
            }
            Hit::BodyEditor => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.handle_mouse(m);
                if clicks == 2 {
                    // Double click selects the word under it; the sweep
                    // armed below then extends by whole words.
                    self.editor.body_select_word_at(m.column, m.row);
                }
                // Arm a selection sweep: if the button now moves before it
                // releases, the drag extends a selection from this click.
                self.text_drag = Some(TextDrag::Body);
                self.update(Action::Render)
            }
            Hit::HeaderProject => self.update(Action::OpenProjectChooser),
            Hit::HeaderProjectCycle => self.update(Action::CycleProject(1)),
            Hit::HeaderSpace => self.update(Action::OpenSpaceChooser),
            Hit::HeaderSpaceCycle => self.update(Action::CycleSpace(1)),
            Hit::HeaderEnv => self.update(Action::OpenEnvChooser),
            Hit::HeaderEnvCycle => self.update(Action::CycleEnv(1)),
            Hit::HeaderTheme => self.update(Action::OpenThemeChooser),
            Hit::HeaderManage => self.update(Action::OpenManage { tab: None }),
            Hit::ManageTab(i) => self.update(Action::SelectManageTab(
                crate::components::manage::ManageTab::from_index(i),
            )),
            Hit::ManageRow(i) => {
                self.manage.list.cursor = i;
                self.update(Action::Render)
            }
            Hit::ManageNew => self.update(match self.manage.tab {
                ManageTab::Spaces => Action::OpenNewSpacePrompt,
                _ => Action::OpenNewEnvPrompt,
            }),
            Hit::ManageRename => {
                let tab = self.manage.tab;
                match self
                    .manage
                    .list
                    .selected(tab, &self.project)
                    .map(str::to_string)
                {
                    Some(name) => self.update(
                        crate::components::manage_list::ManageList::rename_action(tab, &name),
                    ),
                    None => false,
                }
            }
            Hit::ManageDelete => {
                let tab = self.manage.tab;
                match self
                    .manage
                    .list
                    .selected(tab, &self.project)
                    .map(str::to_string)
                {
                    Some(name) if tab == ManageTab::Spaces => {
                        self.update(Action::DeleteSpace(name))
                    }
                    Some(name) => self.update(Action::DeleteEnv(name)),
                    None => false,
                }
            }
            Hit::ManageMoveUp | Hit::ManageMoveDown => {
                let delta = if matches!(hit, Hit::ManageMoveUp) {
                    -1
                } else {
                    1
                };
                match self
                    .manage
                    .list
                    .selected(ManageTab::Spaces, &self.project)
                    .map(str::to_string)
                {
                    Some(name) => self.update(Action::MoveSpace { name, delta }),
                    None => false,
                }
            }
            Hit::ManageEnvTls(policy) => {
                match self
                    .manage
                    .list
                    .selected(ManageTab::Environments, &self.project)
                    .map(str::to_string)
                {
                    Some(env) => self.update(Action::SetEnvTls { env, policy }),
                    None => false,
                }
            }
            Hit::ManageMoveAll => {
                match self
                    .manage
                    .list
                    .selected(ManageTab::Spaces, &self.project)
                    .map(str::to_string)
                {
                    Some(from) => self.update(Action::PromptMoveAllRequests(from)),
                    None => false,
                }
            }
            Hit::FooterChip(action) => {
                // Chips that live inside the request panel (the Body
                // toolbar row, the address bar's TLS lock, the split
                // control's alt+w pill) focus the panel like any of its
                // buttons. The footer's own chips are contextual — they
                // describe the pane already focused — so they never move
                // focus themselves.
                if matches!(
                    action,
                    Action::FormatBody
                        | Action::MinifyBody
                        | Action::ToggleBodyVars
                        | Action::OpenBodyInEditor
                        | Action::BodyClear
                        | Action::ToggleInsecure
                        | Action::CycleSplit
                ) {
                    self.update(Action::FocusPane(PaneId::Editor));
                }
                self.update(action)
            }
            Hit::SidebarNewRequest => self.update(Action::PromptNewRequest),
            Hit::SidebarFolderArrow(i) => {
                self.update(Action::FocusPane(PaneId::Sidebar));
                self.set_sidebar_selected(i);
                self.update(Action::ToggleSelectedFolder)
            }
            Hit::SidebarRow(i) => {
                self.update(Action::FocusPane(PaneId::Sidebar));
                self.set_sidebar_selected(i);
                match self.sidebar.rows.get(i).cloned() {
                    Some(Row::Request {
                        slug, broken: None, ..
                    }) => self.update(Action::OpenRequest(slug)),
                    Some(Row::Request {
                        slug,
                        broken: Some(_),
                        ..
                    }) => self.update(Action::ShowRequestError(slug)),
                    Some(Row::Folder { .. }) => {
                        if clicks == 2 {
                            self.update(Action::ToggleSelectedFolder)
                        } else {
                            self.update(Action::Render)
                        }
                    }
                    None => false,
                }
            }
            Hit::EditorTab(i) => {
                // `i` is the tab's on-screen (draw-order) position — Params,
                // Headers, Vars, Body — which is not the same numbering as
                // `EditorTabSelect`'s stable index (kept unchanged so the
                // bound `editor_tab_N` numbers still land on
                // Params/Headers/Body); convert.
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::EditorTabSelect(
                    EditorTab::from_draw_position(i).index(),
                ))
            }
            Hit::SendButton => {
                self.update(Action::FocusPane(PaneId::Editor));
                if self.session.is_in_flight(&self.editor.slug) {
                    self.update(Action::CancelSend)
                } else {
                    self.update(Action::Send)
                }
            }
            Hit::TableCheckbox(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                // A checkbox click during another row's edit commits that
                // edit first — and that commit can collapse rows, so `i`
                // (baked into the last frame's hit map) is re-resolved by
                // the key it named before the toggle lands.
                let Some(i) = self.resolve_table_row_across_commit(i) else {
                    return self.update(Action::Render);
                };
                // A toggle click is just a toggle — it must not select (or
                // expand) the row.
                let map = match self.editor.active_tab {
                    EditorTab::Params => &mut self.editor.params,
                    EditorTab::Headers => &mut self.editor.headers,
                    EditorTab::Vars => &mut self.editor.variables,
                    EditorTab::Body => {
                        unreachable!("TableCheckbox only fires on Params/Headers/Vars")
                    }
                };
                if let Some((_, e)) = map.get_index_mut(i) {
                    e.enabled = !e.enabled;
                }
                self.update(Action::Render)
            }
            // The row background is only reachable at the slivers its cells
            // don't cover (the accent-bar column, the key/value divider):
            // it selects the row, nothing more — editing is the cells' job.
            Hit::TableRow(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                // The background of the row being edited is that row's own
                // chrome (the pad lines its expansion added): clicking it
                // is inert, so the second click of a double click can't
                // cancel the edit the first one started.
                if self
                    .editor
                    .table
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == i)
                {
                    return self.update(Action::Render);
                }
                let Some(i) = self.resolve_table_row_across_commit(i) else {
                    return self.update(Action::Render);
                };
                self.editor.table.selected = Some(i);
                self.update(Action::Render)
            }
            // A cell click edits that cell in place, committing whatever
            // was being edited before it.
            Hit::TableCell { row, col } => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                let cell_col = crate::components::table_editor::Col::from_index(col);
                let already_editing = self
                    .editor
                    .table
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == row && e.col == cell_col);
                if !already_editing {
                    let outcome = self.editor.click_table_cell(row, cell_col);
                    if let Some(w) = outcome.warning {
                        self.toasts.push(w, ToastKind::Warning);
                    }
                }
                // Place the caret at the clicked column (the cell draws its
                // text unwindowed at the hit rect's left edge) and anchor a
                // possible drag sweep; a double click selects the word,
                // exactly like the body editor. Skipped when a commit
                // collapsed rows out from under the click — the edit then
                // isn't the cell the pointer was on, so the seeded
                // caret-at-end stays.
                if let Some(area) = self.hits.rect_of(&Hit::TableCell { row, col })
                    && let Some(edit) = self
                        .editor
                        .table
                        .editing
                        .as_mut()
                        .filter(|e| e.row == row && e.col == cell_col)
                {
                    let idx = usize::from(m.column.saturating_sub(area.x));
                    if clicks == 2 {
                        edit.input.select_word_at(idx);
                    } else {
                        edit.input.set_cursor(idx);
                        edit.input.begin_mouse_selection();
                    }
                    // Either click anchors a possible drag sweep —
                    // word-wise after a double click.
                    self.text_drag = Some(TextDrag::TableCell);
                }
                self.update(Action::Render)
            }
            Hit::TableDelete(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                // Same as the checkbox: commit first, then re-resolve the
                // row so the confirm never names the wrong one.
                let Some(i) = self.resolve_table_row_across_commit(i) else {
                    return self.update(Action::Render);
                };
                self.update(Action::DeleteTableRow(i))
            }
            Hit::SplitStop(stop) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::SplitStop(stop))
            }
            Hit::SplitStep(delta) => {
                self.update(Action::FocusPane(PaneId::Response));
                self.update(Action::SplitStep(delta))
            }
            Hit::UrlBar => {
                let was_focused = self.editor.sub_focus == SubFocus::Url;
                // `Action::FocusUrl` is exactly "focus Editor, sub-focus
                // Url" (see its handler) — dispatching it here rather than
                // setting both fields by hand is what makes the mouse-parity
                // test's job possible: `focus_url` genuinely goes through
                // this arm rather than merely resembling it.
                self.update(Action::FocusUrl);
                if let Some(area) = self.editor.last_url_text_area {
                    // Map the clicked column back to a char index: the
                    // drawn window starts at 0 unfocused, or scrolls to
                    // keep the caret visible when already focused
                    // (mirroring `LineInput::draw_line_windowed`).
                    let start = if was_focused {
                        (self.editor.url.cursor() + 1).saturating_sub(area.width.max(1) as usize)
                    } else {
                        0
                    };
                    let col = m.column.saturating_sub(area.x) as usize;
                    if clicks == 2 {
                        // Double click selects the word under the pointer,
                        // exactly like the body editor.
                        self.editor.url.select_word_at(start + col);
                    } else {
                        self.editor.url.set_cursor(start + col);
                        self.editor.url.begin_mouse_selection();
                    }
                    // Either click also anchors a possible drag sweep —
                    // word-wise after a double click.
                    self.text_drag = Some(TextDrag::Url);
                }
                self.update(Action::Render)
            }
            Hit::MethodSelector => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::OpenMethodDropdown)
            }
            Hit::DropdownRow(i) => {
                let Some(Modal::Dropdown(state)) = self.modals.top_mut() else {
                    return false;
                };
                // A disabled row swallows its click: nothing runs, and the
                // menu stays open rather than closing on a dead press.
                let Some(action) = state.items.get(i).and_then(|it| it.action.clone()) else {
                    return false;
                };
                self.modals.pop();
                // Overlay close is always instant.
                self.anims.snap(AnimKey::DropdownOpen, 1.0);
                self.anims.snap(AnimKey::ModalOpen, 1.0);
                self.update(action)
            }
            Hit::ModalOutside => {
                // The option Edit prompt is an editing surface, not a
                // question: clicking off it SAVES, like the grid's
                // commit-on-click-away. Routed through the same path Enter
                // takes so a refused commit keeps the prompt (and the typed
                // text) exactly as the keyboard flow would.
                //
                // The theme picker too: its highlight is already applied
                // live, so the theme on screen when the user clicks away
                // is the one they chose — clicking off keeps it (Enter),
                // where Esc is the explicit "put it back".
                let theme_picker = self.theme_preview.is_some()
                    && matches!(
                        self.modals.top(),
                        Some(crate::components::modal::Modal::Chooser(_))
                    );
                if theme_picker
                    || matches!(
                        self.modals.top(),
                        Some(crate::components::modal::Modal::MultiPrompt {
                            kind: crate::components::modal::PromptKind::EditOption { .. },
                            ..
                        })
                    )
                {
                    let enter = ratatui::crossterm::event::KeyEvent::new(
                        ratatui::crossterm::event::KeyCode::Enter,
                        ratatui::crossterm::event::KeyModifiers::NONE,
                    );
                    if let Some(res) = self.modals.handle_key(enter) {
                        return self.apply_modal_result(res);
                    }
                    return true;
                }
                self.update(Action::Close)
            }
            // A click on the modal's own chrome (body/borders/query line)
            // — not one of its interactive hits, which register on top and
            // so win first. Inert: neither closes the modal nor dispatches
            // anything.
            Hit::ModalBody => false,
            Hit::ModalField(i) => {
                if let Some(crate::components::modal::Modal::MultiPrompt {
                    focus,
                    fields,
                    kind,
                    ..
                }) = self.modals.top_mut()
                {
                    *focus = i;
                    // A fixed-choice field has no caret to place — clicking
                    // it advances to the next choice (wrapping), so the
                    // mouse can drive it without knowing about ←/→. The
                    // same kind-specific follow-up as the keyboard cycle
                    // runs after it (the value popup reseeds its value
                    // box from what the new scope stores).
                    if let Some(field) = fields.get_mut(i)
                        && !field.choices.is_empty()
                    {
                        field.cycle(1);
                        crate::components::modal::resync_after_choice_cycle(kind, fields);
                    }
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalChoiceArrow { field, dir } => {
                if let Some(crate::components::modal::Modal::MultiPrompt {
                    focus,
                    fields,
                    kind,
                    ..
                }) = self.modals.top_mut()
                {
                    *focus = field;
                    if let Some(f) = fields.get_mut(field)
                        && !f.choices.is_empty()
                    {
                        f.cycle(dir.into());
                        crate::components::modal::resync_after_choice_cycle(kind, fields);
                    }
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalInput(i) => {
                let Some(area) = self.hits.rect_of(&Hit::ModalInput(i)) else {
                    return false;
                };
                let inner_w = area.width.saturating_sub(2);
                let was_focused = self.modals.focused_input_index() == Some(i);
                let Some(input) = self.modals.focus_input(i) else {
                    return false;
                };
                // Map the clicked column back to a char index through the
                // same window math the field drew with at click time
                // (unfocused fields draw from 0).
                let start = input.window_start(was_focused, inner_w.max(1));
                let col = usize::from(m.column.saturating_sub(area.x + 2));
                if clicks == 2 {
                    // Double click selects the word under the pointer,
                    // exactly like the body editor.
                    input.select_word_at(start + col);
                } else {
                    input.set_cursor(start + col);
                    input.begin_mouse_selection();
                }
                // Either click also anchors a possible drag sweep —
                // word-wise after a double click.
                self.text_drag = Some(TextDrag::ModalInput(i));
                self.update(Action::Render)
            }
            // The painted Cancel/Confirm buttons deliver exactly what
            // Esc/Enter already dispatch for whichever modal is on top: a
            // synthesized key event routed through the same
            // `ModalStack::handle_key` match, rather than duplicating its
            // per-variant logic here. Message's only button ("OK") also
            // maps to `ModalConfirm` — Enter and Esc already produce the
            // same close-with-no-actions result for `Modal::Message`.
            Hit::ModalRowToggle(i) => {
                if let Some(crate::components::modal::Modal::FieldsEditor(state)) =
                    self.modals.top_mut()
                {
                    state.toggle(i);
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalAddRow => {
                if let Some(crate::components::modal::Modal::FieldsEditor(state)) =
                    self.modals.top_mut()
                {
                    state.add_row();
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalSharedToggle => {
                if let Some(crate::components::modal::Modal::Prompt {
                    kind: crate::components::modal::PromptKind::NewSelector { shared, on_toggle },
                    ..
                }) = self.modals.top_mut()
                {
                    *shared = !*shared;
                    // Clicking the row focuses it too, so space keeps
                    // working from where the mouse left off.
                    *on_toggle = true;
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalRemove => self.remove_from_value_popup(),
            Hit::ModalCancel => {
                let synth = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
                let Some(res) = self.modals.handle_key(synth) else {
                    return false;
                };
                self.apply_modal_result(res)
            }
            Hit::ModalConfirm => {
                let synth = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
                let Some(res) = self.modals.handle_key(synth) else {
                    return false;
                };
                self.apply_modal_result(res)
            }
            Hit::PaletteRow(i) => {
                let Some(Modal::Palette(state)) = self.modals.top_mut() else {
                    return false;
                };
                // Single click runs the command (spec §6) — no
                // select-then-confirm step for the palette.
                state.select(i);
                let Some(res) = state.confirm() else {
                    return false;
                };
                self.apply_modal_result(res)
            }
            Hit::ChooserToggle => {
                let Some(Modal::Chooser(state)) = self.modals.top_mut() else {
                    return false;
                };
                let Some(action) = state.toggle_action().cloned() else {
                    return false;
                };
                self.update(action)
            }
            Hit::ChooserRow(i) => {
                let Some(Modal::Chooser(state)) = self.modals.top_mut() else {
                    return false;
                };
                if state.selected() == i || clicks == 2 {
                    let Some(res) = state.confirm() else {
                        return false;
                    };
                    self.apply_modal_result(res)
                } else {
                    state.select(i);
                    self.sync_theme_preview();
                    self.update(Action::Render)
                }
            }
            Hit::VarPickerRow(i) => {
                let Some(Modal::VarPicker(state)) = self.modals.top_mut() else {
                    return false;
                };
                if state.selected() == i || clicks == 2 {
                    let Some(res) = state.confirm() else {
                        return false;
                    };
                    self.apply_modal_result(res)
                } else {
                    state.select(i);
                    self.update(Action::Render)
                }
            }
            Hit::ConfirmChoice(c) => {
                let Some(Modal::Confirm { choices, .. }) = self.modals.top() else {
                    return false;
                };
                let Some((_, _, actions)) = choices.iter().find(|(choice, _, _)| *choice == c)
                else {
                    return false;
                };
                let res = ModalResult {
                    actions: actions.clone(),
                    close: true,
                    ..Default::default()
                };
                self.apply_modal_result(res)
            }
            Hit::ResponseTab(mode) => {
                self.update(Action::FocusPane(PaneId::Response));
                self.update(Action::ResponseViewMode(mode))
            }
            Hit::JsonRow(i) => {
                self.update(Action::FocusPane(PaneId::Response));
                // The click also anchors a possible selection sweep (and
                // collapses any previous selection), like any text click.
                if self.response_click_selection(clicks, m) {
                    self.text_drag = Some(TextDrag::Response);
                }
                self.update(Action::JsonRowClicked {
                    row: i,
                    toggle: false,
                })
            }
            Hit::JsonArrow(i) => {
                self.update(Action::FocusPane(PaneId::Response));
                self.update(Action::JsonRowClicked {
                    row: i,
                    toggle: true,
                })
            }
            Hit::ResponseSearchButton => {
                self.update(Action::FocusPane(PaneId::Response));
                self.session.response.open_search();
                self.update(Action::Render)
            }
            Hit::ResponseSearchNext => self.step_response_search(1),
            Hit::ResponseSearchPrev => self.step_response_search(-1),
            Hit::CopyUrl => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::CopyToClipboard(CopyTarget::Url))
            }
            Hit::CopyBodyButton => self.update(Action::CopyToClipboard(CopyTarget::ResponseView)),
            Hit::SaveBodyButton => self.update(Action::PromptSaveView),
            Hit::ResponseEditorButton => self.update(Action::OpenResponseInEditor),
            Hit::ResponseJqButton => self.update(Action::ToggleJqBar),
            Hit::ResponseJqBar => {
                let Some(area) = self.hits.rect_of(&Hit::ResponseJqBar) else {
                    return false;
                };
                let was_focused = self.session.response.jq_focused();
                self.update(Action::FocusPane(PaneId::Response));
                if !self.session.response.set_jq_focus(true) {
                    return false;
                }
                // The text sits after the `jq ` chip; map the clicked
                // column back through the window math the bar drew with
                // (from 0 unfocused, following the caret once focused),
                // exactly like the URL bar and a modal's text box.
                let (text_x, inner_w) = jq_bar_text_geometry(area);
                let input = &mut self.session.response.jq_bar_mut().input;
                let start = input.window_start(was_focused, inner_w.max(1));
                let col = usize::from(m.column.saturating_sub(text_x));
                if clicks == 2 {
                    input.select_word_at(start + col);
                } else {
                    input.set_cursor(start + col);
                    input.begin_mouse_selection();
                }
                self.text_drag = Some(TextDrag::Jq);
                self.update(Action::Render)
            }
            Hit::ResponseJqAiButton => self.update(Action::OpenJqDescribe),
            Hit::HeaderCopy(i) => {
                self.update(Action::CopyToClipboard(CopyTarget::ResponseHeader(i)))
            }
            Hit::AutoHeaderCopy(i) => {
                self.update(Action::CopyToClipboard(CopyTarget::ComputedHeader(i)))
            }
            Hit::AutoHeaderReveal => self.update(Action::ToggleHeaderReveal),
            Hit::ScrollbarThumb(pane) => {
                let Some(thumb) = self.hits.rect_of(&Hit::ScrollbarThumb(pane)) else {
                    return false;
                };
                self.drag = Some(Drag {
                    pane,
                    grab_offset: m.row.saturating_sub(thumb.y),
                    horizontal: false,
                });
                // Redraw so the thumb picks up its dragged styling.
                self.update(Action::Render)
            }
            Hit::ScrollbarTrack(pane, delta) => {
                self.update(Action::ScrollPane(pane, delta.clamp(-30, 30)))
            }
            Hit::HScrollThumb(pane) => {
                let Some(thumb) = self.hits.rect_of(&Hit::HScrollThumb(pane)) else {
                    return false;
                };
                self.drag = Some(Drag {
                    pane,
                    grab_offset: m.column.saturating_sub(thumb.x),
                    horizontal: true,
                });
                self.update(Action::Render)
            }
            // Only the Response pane draws a horizontal bar today; a track
            // click pages the viewport a full width toward the click.
            Hit::HScrollTrack(PaneId::Response, delta) => {
                self.session.response.handle_scroll_h(delta);
                self.update(Action::Render)
            }
            Hit::HScrollTrack(..) => false,
            // Clicking a drawn `{{token}}` opens the var picker already
            // filtered to that name (spec §7) — the shortest path from
            // "what is this?" to the variable itself.
            Hit::VarToken(name) => self.update(Action::OpenVarTokenPopup(name)),
            // Like `VmFormField`/`VmEntryCell` above: the commit attempts at
            // the top of this function just ran, and a write failure
            // restores the original edit (still live) with its typed text.
            // Selecting a different row would reset `form`/`grid` and throw
            // that text away, so the click is absorbed instead — render
            // only, edit and detail stay put.
            Hit::VmLeftRow(i) => {
                if self.varmanager.form.editing.is_some() || self.varmanager.grid.editing.is_some()
                {
                    return self.update(Action::Render);
                }
                self.varmanager.select_row(i);
                self.update(Action::Render)
            }
            // The Manager's environment switcher is the header env chip's
            // chooser, reached from the screen that replaced the header.
            Hit::VmNewVar => self.update(Action::PromptNewVar),
            Hit::VmNewSelector => self.update(Action::PromptNewSelector),
            // A second click on the field already under edit is inert (the
            // top-of-`on_hit` guard above left it alone precisely so this
            // check still sees the live edit). A click on any other field
            // lands here only after that same guard already tried to
            // commit whatever was being edited: on success `form.editing`
            // is now `None` and this starts a fresh edit on the clicked
            // field; on a write failure it restores the *original* field's
            // edit (spec's write-failure rule: the typed text stays put),
            // and this must not clobber that with the newly clicked field
            // — the click is absorbed and the original edit stays live.
            Hit::VmFormField(field) => {
                let already_editing = self
                    .varmanager
                    .form
                    .editing
                    .as_ref()
                    .is_some_and(|(f, _)| *f == field);
                if !already_editing {
                    if self.varmanager.form.editing.is_some() {
                        return self.update(Action::Render);
                    }
                    self.varmanager.start_field_edit(&self.project, field);
                }
                // Map the click through the field's `TextField` geometry
                // (text 2 columns in, windowed to `width - 2` — the drawn
                // window starts at 0 unless the field was already under
                // edit): caret at the clicked column, anchor a possible
                // drag sweep, word select on double click.
                if let Some(area) = self.hits.rect_of(&Hit::VmFormField(field))
                    && let Some((_, input)) = self
                        .varmanager
                        .form
                        .editing
                        .as_mut()
                        .filter(|(f, _)| *f == field)
                {
                    let inner_w = area.width.saturating_sub(2).max(1);
                    let start = input.window_start(already_editing, inner_w);
                    let col = usize::from(m.column.saturating_sub(area.x + 2));
                    let idx = start + col;
                    if clicks == 2 {
                        input.select_word_at(idx);
                    } else {
                        input.set_cursor(idx);
                        input.begin_mouse_selection();
                    }
                    // Either click anchors a possible drag sweep —
                    // word-wise after a double click.
                    self.text_drag = Some(TextDrag::VmField);
                }
                self.update(Action::Render)
            }
            // The grid's cells follow `VmFormField`'s rules exactly: a
            // second click on the live cell is inert, and a click on
            // another cell after a *failed* commit is absorbed so the
            // original edit (holding the text that couldn't be written)
            // stays live.
            Hit::VmEntryCell { row, col } => {
                let already_editing = self
                    .varmanager
                    .grid
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == row && e.col == col);
                if !already_editing {
                    if self.varmanager.grid.editing.is_some() {
                        return self.update(Action::Render);
                    }
                    self.varmanager.grid.cursor = (row, col);
                    self.varmanager.start_cell_edit(&self.project, row, col);
                }
                // Caret at the clicked column (the cell draws windowed at
                // the hit rect's own left edge; the resting text draws from
                // 0), anchor a possible drag sweep, word select on double
                // click. A ghost-row click gets redirected to the empty
                // name cell by `start_cell_edit`, so the filter skips it.
                if let Some(area) = self.hits.rect_of(&Hit::VmEntryCell { row, col })
                    && let Some(edit) = self
                        .varmanager
                        .grid
                        .editing
                        .as_mut()
                        .filter(|e| e.row == row && e.col == col)
                {
                    let start = edit.input.window_start(already_editing, area.width.max(1));
                    let col = usize::from(m.column.saturating_sub(area.x));
                    let idx = start + col;
                    if clicks == 2 {
                        edit.input.select_word_at(idx);
                    } else {
                        edit.input.set_cursor(idx);
                        edit.input.begin_mouse_selection();
                    }
                    // Either click anchors a possible drag sweep —
                    // word-wise after a double click.
                    self.text_drag = Some(TextDrag::VmCell);
                }
                self.update(Action::Render)
            }
            Hit::VmEntryRadio(row) => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                let (Some(env), Some(option)) = (
                    self.project.active_env.clone(),
                    self.varmanager.entry_at(&self.project, row),
                ) else {
                    return false;
                };
                self.varmanager.grid.cursor = (row, 0);
                self.varmanager.focus = VmFocus::Grid;
                self.update(Action::VarEdit(VarEditOp::SelectOption {
                    env,
                    selector,
                    option,
                }))
            }
            Hit::VmEntryDelete(row) => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                let (Some(env), Some(name)) = (
                    self.project.active_env.clone(),
                    self.varmanager.entry_at(&self.project, row),
                ) else {
                    return false;
                };
                self.varmanager.grid.cursor = (row, 0);
                self.varmanager.focus = VmFocus::Grid;
                self.update(Action::DeleteEntry {
                    env,
                    selector,
                    name,
                })
            }
            Hit::VmNewOption => {
                self.update(Action::StartNewOptionEdit);
                self.update(Action::Render)
            }
            Hit::VmEditFields => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                self.update(Action::PromptGroupFields { selector })
            }
            Hit::VmSecretToggle => {
                let crate::components::varmanager::VmDetail::Var(name) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                self.update(Action::ToggleSecretVar { name })
            }
            Hit::VmRevealToggle => {
                self.varmanager.form.revealed = !self.varmanager.form.revealed;
                self.update(Action::Render)
            }
            Hit::VmRemoveEnvValue => {
                let crate::components::varmanager::VmDetail::Var(name) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                self.update(Action::RemoveVarValue {
                    name,
                    destination: crate::action::ExtractDestination::ActiveEnv,
                })
            }
            // Both buttons are on the variable form *and* the selector pane;
            // the rename/delete flows behind them already branch on what
            // the name is declared as.
            Hit::VmRename => {
                let Some(name) = self.varmanager.detail.name().map(str::to_string) else {
                    return false;
                };
                self.update(Action::PromptRenameVar { from: name })
            }
            Hit::VmDelete => {
                let Some(name) = self.varmanager.detail.name().map(str::to_string) else {
                    return false;
                };
                self.update(Action::DeleteVar { name })
            }
            Hit::VmPromoteBtn => {
                let crate::components::varmanager::VmDetail::Var(name) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                let open_request = self
                    .editor
                    .slug
                    .is_some()
                    .then(|| self.editor.current_request());
                let Some((_, action)) = crate::components::varmanager::promote_action(
                    &self.project,
                    open_request.as_ref(),
                    &name,
                ) else {
                    return false;
                };
                self.update(action)
            }
        }
    }
}

/// Where the jq bar's text starts and how wide it is, given the bar row's
/// hit rect: the text follows the 3-cell `jq ` chip
/// (`response::draw_jq_bar`).
fn jq_bar_text_geometry(area: ratatui::layout::Rect) -> (u16, u16) {
    (area.x + 3, area.width.saturating_sub(3))
}
