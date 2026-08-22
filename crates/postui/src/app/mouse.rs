//! Mouse routing: raw event handling, hover tracking, click dispatch
//! (`on_hit`), and scrollbar-thumb drags. Split from `app.rs`; every
//! method here is `impl App` and shares its state directly.

use super::*;

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

        match m.kind {
            // Terminals report pointer motion with a button held as `Drag`,
            // not `Moved`, so a thumb drag arrives as either depending on
            // whether the terminal tracks button state; both drive the drag.
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                if self.drag.is_some() {
                    return self.drag_to(m.row);
                }
                if m.kind != MouseEventKind::Moved {
                    // Button-held motion with no drag of ours in progress
                    // (e.g. a text selection sweep) is not a hover update.
                    return false;
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
                // The pointer's exact position is recorded but not part of
                // "changed": the tooltip is anchored at the token's own
                // drawn rect, so moving within one token changes nothing.
                let changed = hit != self.hovered || token != self.hovered_token;
                self.hovered = hit;
                self.hovered_token = token;
                self.pointer = Some((m.column, m.row));
                changed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(hit) = self.hits.hit_at(m.column, m.row).cloned() else {
                    return false;
                };
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
                self.on_hit(hit, clicks, m)
            }
            // A right click targets the row under the pointer: it moves the
            // selection there first (so the menu's flows, which all read the
            // selection, act on what was clicked), then opens whatever menu
            // that hit offers. Hits with no menu still get the selection
            // move — right-clicking a row selects it, menu or not.
            MouseEventKind::Down(MouseButton::Right) => {
                // Tokens are a left-click affordance only: a right click
                // belongs to the row/cell under them and its context menu.
                let Some(hit) = self
                    .hits
                    .hit_at_ignoring_var_tokens(m.column, m.row)
                    .cloned()
                else {
                    return false;
                };
                let mut changed = false;
                match &hit {
                    Hit::SidebarRow(i) | Hit::SidebarFolderArrow(i) => {
                        changed |= self.update(Action::FocusPane(PaneId::Sidebar));
                        changed |= self.sidebar.selected != Some(*i);
                        self.sidebar.selected = Some(*i);
                    }
                    Hit::TableRow(i)
                    | Hit::TableCheckbox(i)
                    | Hit::TableDelete(i)
                    | Hit::TableCell { row: i, .. }
                        if self.editor.table.editing.is_none() =>
                    {
                        changed |= self.update(Action::FocusPane(PaneId::Editor));
                        changed |= self.editor.table.selected != Some(*i);
                        self.editor.sub_focus = SubFocus::Content;
                        self.editor.table.selected = Some(*i);
                    }
                    _ => {}
                }
                match self.context_menu_for(&hit) {
                    Some(items) => self.open_context_menu(m.column, m.row, items) || changed,
                    None => changed,
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag.take().is_some(),
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
                if self.screen == Screen::VarManager {
                    let d = if m.kind == MouseEventKind::ScrollUp {
                        -3
                    } else {
                        3
                    };
                    self.varmanager.handle_scroll(d);
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
            _ => false,
        }
    }

    /// The scroll state `pane` would draw a scrollbar from right now — the
    /// same [`ScrollbarSpec`] its `draw` builds, so drag math and the drawn
    /// thumb can never disagree. `None` when the pane has nothing scrollable
    /// (or has not been drawn yet).
    pub fn scrollbar_spec(&self, pane: PaneId) -> Option<ScrollbarSpec> {
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

    /// The `▼`/`▲` search buttons: focus the response pane and step the
    /// match cycle, exactly as `n`/`N` do.
    fn step_response_search(&mut self, delta: i32) -> bool {
        self.update(Action::FocusPane(PaneId::Response));
        self.session.response.step_search(delta);
        self.update(Action::Render)
    }

    /// The central click dispatch: maps a resolved `Hit` (plus click count
    /// and the raw event, for hits that need to forward it) to app state
    /// changes. Only `Pane` and `BodyEditor` are wired up so far; later
    /// tasks extend this match as more hit kinds gain behavior.
    fn on_hit(&mut self, hit: Hit, clicks: u8, m: ratatui::crossterm::event::MouseEvent) -> bool {
        // A click anywhere that isn't the params/headers table itself (and
        // isn't inside a modal — e.g. this row's own delete confirm) is a
        // click away: it commits whatever cell was being edited (typing is
        // never silently thrown away) and clears the table selection.
        let keeps_table_selection = matches!(
            hit,
            Hit::TableRow(_)
                | Hit::TableCheckbox(_)
                | Hit::TableDelete(_)
                | Hit::TableCell { .. }
                | Hit::TableCollapse
                | Hit::ModalCancel
                | Hit::ModalConfirm
                | Hit::ConfirmChoice(_)
                | Hit::ModalBody
                | Hit::ModalOutside
                | Hit::DropdownRow(_)
                | Hit::ChooserRow(_)
                | Hit::PaletteRow(_)
                | Hit::VarPickerRow(_)
                | Hit::ScrollbarThumb(_)
                | Hit::ScrollbarTrack(..)
                // A token sits *on* a cell: hovering or clicking one must
                // neither commit the cell under edit nor drop the selection.
                | Hit::VarToken(_)
        );
        if !keeps_table_selection {
            self.commit_table_edit();
            self.editor.table.selected = None;
        }
        // Likewise, clicking away blurs whichever editor input is active
        // (URL line / table / body). Hits that themselves place the
        // sub-focus (UrlBar, BodyEditor, the table hits) re-set it right
        // after; modal and scrollbar hits are excluded above so an open
        // popup or a scroll never blurs the input under it.
        let keeps_editor_input = keeps_table_selection
            || matches!(hit, Hit::UrlBar | Hit::BodyEditor | Hit::VarToken(_));
        if !keeps_editor_input {
            self.editor.sub_focus = SubFocus::None;
        }
        match hit {
            Hit::Pane(p) => self.update(Action::FocusPane(p)),
            Hit::BodyEditor => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.handle_mouse(m);
                self.update(Action::Render)
            }
            Hit::HeaderProject => self.update(Action::OpenProjectChooser),
            Hit::HeaderEnv => self.update(Action::OpenEnvChooser),
            Hit::HeaderVars => {
                if self.screen == crate::app::Screen::VarManager {
                    self.update(Action::CloseScreen)
                } else {
                    self.update(Action::OpenVarManager)
                }
            }
            Hit::FooterChip(action) => self.update(action),
            Hit::SidebarNewRequest => self.update(Action::PromptNewRequest),
            Hit::SidebarFolderArrow(i) => {
                self.update(Action::FocusPane(PaneId::Sidebar));
                self.sidebar.selected = Some(i);
                self.update(Action::ToggleSelectedFolder)
            }
            Hit::SidebarRow(i) => {
                self.update(Action::FocusPane(PaneId::Sidebar));
                self.sidebar.selected = Some(i);
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
                // `EditorTabSelect`'s stable index (kept unchanged so
                // alt+1/2/3 still land on Params/Headers/Body); convert.
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::EditorTabSelect(
                    EditorTab::from_draw_position(i).index(),
                ))
            }
            Hit::SendButton => {
                if self.session.in_flight.is_some() {
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
                self.editor.table.selected = Some(i);
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
                let outcome = self
                    .editor
                    .click_table_cell(row, crate::components::table_editor::Col::from_index(col));
                if let Some(w) = outcome.warning {
                    self.toasts.push(w, ToastKind::Warning);
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
                self.update(Action::ConfirmDeleteTableRow(i))
            }
            Hit::TableCollapse => self.update(Action::ToggleTableCollapse),
            Hit::UrlBar => {
                self.update(Action::FocusPane(PaneId::Editor));
                let was_focused = self.editor.sub_focus == SubFocus::Url;
                self.editor.sub_focus = SubFocus::Url;
                if let Some(area) = self.editor.last_url_text_area {
                    // Map the clicked column back to a char index: the drawn
                    // window starts at 0 unfocused, or scrolls to keep the
                    // caret visible when already focused (mirroring
                    // `LineInput::draw_line_windowed`).
                    let start = if was_focused {
                        (self.editor.url.cursor() + 1).saturating_sub(area.width.max(1) as usize)
                    } else {
                        0
                    };
                    let col = m.column.saturating_sub(area.x) as usize;
                    self.editor.url.set_cursor(start + col);
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
                self.update(action)
            }
            Hit::ModalOutside => self.update(Action::Close),
            // A click on the modal's own chrome (body/borders/query line)
            // — not one of its interactive hits, which register on top and
            // so win first. Inert: neither closes the modal nor dispatches
            // anything.
            Hit::ModalBody => false,
            // The painted Cancel/Confirm buttons deliver exactly what
            // Esc/Enter already dispatch for whichever modal is on top: a
            // synthesized key event routed through the same
            // `ModalStack::handle_key` match, rather than duplicating its
            // per-variant logic here. Message's only button ("OK") also
            // maps to `ModalConfirm` — Enter and Esc already produce the
            // same close-with-no-actions result for `Modal::Message`.
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
            Hit::CopyBodyButton => self.update(Action::CopyToClipboard(CopyTarget::ResponseBody)),
            Hit::SaveBodyButton => self.update(Action::PromptSaveBody),
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
                });
                // Redraw so the thumb picks up its dragged styling.
                self.update(Action::Render)
            }
            Hit::ScrollbarTrack(pane, delta) => {
                self.update(Action::ScrollPane(pane, delta.clamp(-30, 30)))
            }
            // Clicking a drawn `{{token}}` opens the var picker already
            // filtered to that name (spec §7) — the shortest path from
            // "what is this?" to the variable itself.
            Hit::VarToken(name) => self.update(Action::OpenVarPickerFor(name)),
            Hit::VarRow(i) => match self.varmanager.click_row(i) {
                Some(action) => self.update(action),
                None => self.update(Action::Render),
            },
            Hit::VarName(i) => match self.varmanager.click_name(i, clicks == 2, &self.project) {
                Some(action) => self.update(action),
                None => self.update(Action::Render),
            },
            Hit::VarCell { row, col } => {
                let open_request = self
                    .editor
                    .slug
                    .is_some()
                    .then(|| self.editor.current_request());
                match self.varmanager.click_cell(
                    row,
                    col,
                    clicks == 2,
                    &self.project,
                    open_request.as_ref(),
                ) {
                    Some(action) => self.update(action),
                    None => self.update(Action::Render),
                }
            }
        }
    }
}
