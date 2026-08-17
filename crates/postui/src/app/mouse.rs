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
                let hit = self.hits.hit_at(m.column, m.row).cloned();
                if hit != self.hovered {
                    self.hovered = hit;
                    return true;
                }
                false
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

    /// The central click dispatch: maps a resolved `Hit` (plus click count
    /// and the raw event, for hits that need to forward it) to app state
    /// changes. Only `Pane` and `BodyEditor` are wired up so far; later
    /// tasks extend this match as more hit kinds gain behavior.
    fn on_hit(&mut self, hit: Hit, clicks: u8, m: ratatui::crossterm::event::MouseEvent) -> bool {
        // A click anywhere that isn't the params/headers table itself (and
        // isn't inside a modal — e.g. this row's own delete confirm) clears
        // the table selection, so clicking around the app deselects.
        // Suppressed mid-edit: an in-progress cell edit owns the selection.
        let keeps_table_selection = matches!(
            hit,
            Hit::TableRow(_)
                | Hit::TableCheckbox(_)
                | Hit::TableDelete(_)
                | Hit::TableAdd
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
        );
        if !keeps_table_selection && self.editor.table.editing.is_none() {
            self.editor.table.selected = None;
        }
        // Likewise, clicking away deselects whichever editor input is
        // active (URL line / table / body). Hits that themselves place the
        // sub-focus (UrlBar, BodyEditor, the table hits) re-set it right
        // after; modal and scrollbar hits are excluded above so an open
        // popup or a scroll never blurs the input under it. Suppressed
        // mid-edit for the same reason as the selection: an in-progress
        // cell edit owns the focus.
        let keeps_editor_input =
            keeps_table_selection || matches!(hit, Hit::UrlBar | Hit::BodyEditor);
        if !keeps_editor_input && self.editor.table.editing.is_none() {
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
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::EditorTabSelect(i))
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
                self.editor.table.selected = Some(i);
                let map = match self.editor.active_tab {
                    EditorTab::Params => &mut self.editor.params,
                    EditorTab::Headers => &mut self.editor.headers,
                    EditorTab::Body => unreachable!("TableCheckbox only fires on Params/Headers"),
                };
                if let Some((_, e)) = map.get_index_mut(i) {
                    e.enabled = !e.enabled;
                }
                self.update(Action::Render)
            }
            Hit::TableRow(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                if clicks == 2 {
                    self.editor.table.selected = Some(i);
                    let map = match self.editor.active_tab {
                        EditorTab::Params => &mut self.editor.params,
                        EditorTab::Headers => &mut self.editor.headers,
                        EditorTab::Body => unreachable!("TableRow only fires on Params/Headers"),
                    };
                    self.editor.table.begin_edit_selected(map);
                } else if self.editor.table.editing.is_none()
                    && self.editor.table.selected == Some(i)
                {
                    // Clicking the already-selected row again deselects it.
                    self.editor.table.selected = None;
                } else {
                    self.editor.table.selected = Some(i);
                }
                self.update(Action::Render)
            }
            Hit::TableDelete(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                self.update(Action::ConfirmDeleteTableRow(i))
            }
            Hit::TableAdd => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.editor.sub_focus = SubFocus::Content;
                let map = match self.editor.active_tab {
                    EditorTab::Params => &self.editor.params,
                    EditorTab::Headers => &self.editor.headers,
                    EditorTab::Body => unreachable!("TableAdd only fires on Params/Headers"),
                };
                self.editor.table.begin_add(map);
                self.update(Action::Render)
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
                let Some((_, action)) = state.items.get(i).cloned() else {
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
            Hit::CopyBodyButton => self.update(Action::CopyToClipboard(CopyTarget::ResponseBody)),
            Hit::SaveBodyButton => self.update(Action::PromptSaveBody),
            Hit::HeaderCopy(i) => {
                self.update(Action::CopyToClipboard(CopyTarget::ResponseHeader(i)))
            }
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
        }
    }
}
