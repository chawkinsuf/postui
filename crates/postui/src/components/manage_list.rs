//! The Manage screen's Environments and Spaces tabs: one list-edit face
//! for both (spec "Manage screen"), laid out like the Variables tab so the
//! three tabs read as one interface. Left: `+ New` and the item list.
//! Right: a title row (`Space: name` / `Environment: name`) with the
//! pane's buttons right-aligned on it, then a detail block — the env
//! file's path, or the space's requests by name.

use crate::action::Action;
use crate::components::manage::ManageTab;
use crate::hit::{Hit, HitMap};
use crate::paint::{
    BUTTON_HEIGHT, Button, ButtonKind, ControlState, ListRow, RowHighlight, button_min_width, fill,
    text,
};
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use std::collections::BTreeMap;

/// The left column's width — the Variable Manager's, so both faces of the
/// Manage screen line up as the tab strip switches between them.
pub const LEFT_W: u16 = crate::components::varmanager::LEFT_W;

#[derive(Default)]
pub struct ManageList {
    /// Index into the current tab's item list.
    pub cursor: usize,
    pub scroll: usize,
    visible_rows: usize,
    ensure_visible: bool,
}

impl ManageList {
    /// The tab's items: the project's spaces, or its environments.
    pub fn items(tab: ManageTab, ctx: &ProjectContext) -> &[String] {
        match tab {
            ManageTab::Spaces => &ctx.spaces,
            _ => &ctx.environments,
        }
    }

    pub fn selected<'a>(&self, tab: ManageTab, ctx: &'a ProjectContext) -> Option<&'a str> {
        Self::items(tab, ctx).get(self.cursor).map(String::as_str)
    }

    /// Keeps the cursor on `name` after a reorder/rename/reload.
    pub fn select_name(&mut self, tab: ManageTab, ctx: &ProjectContext, name: &str) {
        if let Some(i) = Self::items(tab, ctx).iter().position(|s| s == name) {
            self.cursor = i;
            self.ensure_visible = true;
        }
    }

    /// Drops every trace of the previous tab's list: its cursor and its
    /// scroll (the tab strip switches to a different set of items
    /// entirely).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn new_action(tab: ManageTab) -> Action {
        match tab {
            ManageTab::Spaces => Action::OpenNewSpacePrompt,
            _ => Action::OpenNewEnvPrompt,
        }
    }

    /// The rename prompt for `name` — the same prompt the header
    /// dropdowns and the Variables tab's Rename button use.
    pub fn rename_action(tab: ManageTab, name: &str) -> Action {
        match tab {
            ManageTab::Spaces => Action::PromptRenameSpace(name.to_string()),
            _ => Action::PromptRenameEnv(name.to_string()),
        }
    }

    /// The Spaces tab's move-all chooser for `name` (`m`, or the button).
    fn move_all_action(name: &str) -> Action {
        Action::PromptMoveAllRequests(name.to_string())
    }

    fn delete_action(tab: ManageTab, name: &str) -> Action {
        match tab {
            ManageTab::Spaces => Action::DeleteSpace(name.to_string()),
            _ => Action::DeleteEnv(name.to_string()),
        }
    }

    /// The list's own keys.
    pub fn handle_key(
        &mut self,
        ev: KeyEvent,
        tab: ManageTab,
        ctx: &ProjectContext,
    ) -> Option<Action> {
        let len = Self::items(tab, ctx).len();
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        match ev.code {
            KeyCode::Esc => Some(Action::CloseScreen),
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Up if alt && tab == ManageTab::Spaces => {
                let name = self.selected(tab, ctx)?.to_string();
                Some(Action::MoveSpace { name, delta: -1 })
            }
            KeyCode::Down if alt && tab == ManageTab::Spaces => {
                let name = self.selected(tab, ctx)?.to_string();
                Some(Action::MoveSpace { name, delta: 1 })
            }
            KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                self.ensure_visible = true;
                None
            }
            KeyCode::Down => {
                if self.cursor + 1 < len {
                    self.cursor += 1;
                }
                self.ensure_visible = true;
                None
            }
            KeyCode::Char('n') => Some(Self::new_action(tab)),
            KeyCode::Char('r') => Some(Self::rename_action(tab, self.selected(tab, ctx)?)),
            KeyCode::Char('m') if tab == ManageTab::Spaces => {
                Some(Self::move_all_action(self.selected(tab, ctx)?))
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                Some(Self::delete_action(tab, self.selected(tab, ctx)?))
            }
            _ => None,
        }
    }

    pub fn footer_chips(
        &self,
        tab: ManageTab,
        ctx: &ProjectContext,
    ) -> Vec<(&'static str, &'static str, Option<Action>)> {
        let selected = self.selected(tab, ctx);
        let mut chips = vec![
            ("n", "new", Some(Self::new_action(tab))),
            ("r", "rename", selected.map(|n| Self::rename_action(tab, n))),
            ("d", "delete", selected.map(|n| Self::delete_action(tab, n))),
        ];
        if tab == ManageTab::Spaces {
            chips.push(("m", "move all", selected.map(Self::move_all_action)));
            chips.push(("alt+↑↓", "move", None));
        }
        chips
    }

    pub fn handle_scroll(&mut self, delta: i16) {
        let max = self.scroll.saturating_add(self.visible_rows);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
        self.ensure_visible = false;
    }

    /// Paints both columns. `requests` maps each space to its requests'
    /// display names, in sidebar order (the Spaces tab's detail block).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        body: Rect,
        theme: &Theme,
        tab: ManageTab,
        ctx: &ProjectContext,
        requests: &BTreeMap<String, Vec<String>>,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        let items = Self::items(tab, ctx).to_vec();
        if self.cursor >= items.len() {
            self.cursor = items.len().saturating_sub(1);
        }
        let left = Rect {
            width: LEFT_W.min(body.width),
            ..body
        };
        let right = Rect {
            x: body.x + left.width,
            width: body.width - left.width,
            ..body
        };
        self.draw_left(frame, left, theme, tab, ctx, &items, hits, hovered);
        self.draw_right(
            frame, right, theme, tab, ctx, &items, requests, hits, hovered,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_left(
        &mut self,
        frame: &mut Frame,
        left: Rect,
        theme: &Theme,
        tab: ManageTab,
        ctx: &ProjectContext,
        items: &[String],
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        let buf = frame.buffer_mut();
        fill(buf, left, theme.panel);
        if left.width <= 2 || left.height < BUTTON_HEIGHT + 2 {
            self.visible_rows = 0;
            return;
        }
        let button = Rect {
            x: left.x + 1,
            y: left.y + 1,
            width: left.width - 2,
            height: BUTTON_HEIGHT,
        };
        let state = if hovered == Some(&Hit::ManageNew) {
            ControlState::Hover
        } else {
            ControlState::Normal
        };
        Button {
            label: "+ New",
            kind: ButtonKind::Primary,
            state,
        }
        .paint(buf, button, theme);
        hits.register(button, Hit::ManageNew);

        let list = Rect {
            x: left.x + 1,
            y: button.y + BUTTON_HEIGHT + 1,
            width: left.width - 2,
            height: left.height.saturating_sub(BUTTON_HEIGHT + 2),
        };
        self.visible_rows = list.height as usize;
        if self.ensure_visible && self.visible_rows > 0 {
            if self.cursor < self.scroll {
                self.scroll = self.cursor;
            } else if self.cursor >= self.scroll + self.visible_rows {
                self.scroll = self.cursor + 1 - self.visible_rows;
            }
            self.ensure_visible = false;
        }
        self.scroll = self
            .scroll
            .min(items.len().saturating_sub(self.visible_rows));
        for (row, i) in (self.scroll..items.len())
            .enumerate()
            .take(self.visible_rows)
        {
            let y = list.y + row as u16;
            let highlight = if i == self.cursor {
                RowHighlight::Selected
            } else if hovered == Some(&Hit::ManageRow(i)) {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            ListRow {
                highlight,
                zebra: None,
            }
            .paint(buf, y, list.x, list.width, theme.panel, 1.0, theme);
            let bg = ListRow::resolve_fill(theme, highlight, theme.panel, 1.0);
            // Spaces carry their `alt+<n>` jump number; the active item
            // is not marked here — the header chip already says which one
            // is active, and the list is for editing, not switching.
            let label = match tab {
                ManageTab::Spaces => format!("{}  {}", i + 1, ctx.space_name(&items[i])),
                _ => ctx.env_name(&items[i]),
            };
            text(
                buf,
                list.x + 2,
                y,
                super::chooser::clip(&label, list.width.saturating_sub(3)),
                theme.text,
                bg,
                i == self.cursor,
            );
            hits.register(
                Rect {
                    x: list.x,
                    y,
                    width: list.width,
                    height: 1,
                },
                Hit::ManageRow(i),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_right(
        &mut self,
        frame: &mut Frame,
        right: Rect,
        theme: &Theme,
        tab: ManageTab,
        ctx: &ProjectContext,
        items: &[String],
        requests: &BTreeMap<String, Vec<String>>,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        let buf = frame.buffer_mut();
        fill(buf, right, theme.page);
        let Some(name) = items.get(self.cursor) else {
            let hint = match tab {
                ManageTab::Spaces => "Select a space",
                _ => "Select an environment",
            };
            if right.width > 2 && right.height > 1 {
                text(
                    buf,
                    right.x + 2,
                    right.y + 1,
                    hint,
                    theme.text_muted,
                    theme.page,
                    false,
                );
            }
            return;
        };
        if right.width < 8 || right.height < 3 {
            return;
        }
        let x0 = right.x + 2;
        let bottom = right.y + right.height;
        let mut y = right.y + 1;

        // --- title row: name + the pane's buttons, right-aligned --------
        // The Variables pane's layout exactly: the title at the left, the
        // buttons laid out from the pane's right edge inward in
        // keep-priority order (Delete outermost, like the selector grid),
        // a button that would run into the title dropped rather than
        // painted over it. Dropped buttons stay reachable by key.
        if y + BUTTON_HEIGHT <= bottom {
            let title = match tab {
                ManageTab::Spaces => format!("Space: {}", ctx.space_name(name)),
                _ => format!("Environment: {}", ctx.env_name(name)),
            };
            text(buf, x0, y + 1, &title, theme.text, theme.page, true);
            let mut buttons: Vec<(&str, Hit)> =
                vec![("Delete", Hit::ManageDelete), ("Rename", Hit::ManageRename)];
            if tab == ManageTab::Spaces {
                buttons.push(("Move down", Hit::ManageMoveDown));
                buttons.push(("Move up", Hit::ManageMoveUp));
                buttons.push(("Move all requests\u{2026}", Hit::ManageMoveAll));
            }
            let mut bx = right.x + right.width;
            for (label, hit) in buttons {
                let w = button_min_width(label);
                if bx < x0 + title.chars().count() as u16 + w + 3 {
                    break;
                }
                bx -= w + 1;
                let rect = Rect {
                    x: bx,
                    y,
                    width: w,
                    height: BUTTON_HEIGHT,
                };
                let state = if hovered == Some(&hit) {
                    ControlState::Hover
                } else {
                    ControlState::Normal
                };
                Button {
                    label,
                    kind: ButtonKind::Secondary,
                    state,
                }
                .paint(buf, rect, theme);
                hits.register(rect, hit);
            }
            y += BUTTON_HEIGHT + 1;
        }

        // --- detail block ---------------------------------------------
        let clip_w = right.width.saturating_sub(4);
        match tab {
            ManageTab::Spaces => {
                // The space's requests by name — the pane has the room, and
                // names say far more than a count. As many as fit, then a
                // "+ n more" line for the rest.
                let names = requests.get(name).map(Vec::as_slice).unwrap_or(&[]);
                if y >= bottom {
                    return;
                }
                let heading = if names.is_empty() {
                    "No requests"
                } else {
                    "Requests"
                };
                text(buf, x0, y, heading, theme.text_muted, theme.page, false);
                y += 1;
                let room = (bottom - y) as usize;
                let shown = if names.len() > room {
                    room.saturating_sub(1)
                } else {
                    names.len()
                };
                for n in &names[..shown] {
                    text(
                        buf,
                        x0,
                        y,
                        super::chooser::clip(n, clip_w),
                        theme.text,
                        theme.page,
                        false,
                    );
                    y += 1;
                }
                if shown < names.len() && y < bottom {
                    let more = format!("+ {} more", names.len() - shown);
                    text(buf, x0, y, &more, theme.text_muted, theme.page, false);
                }
            }
            _ => {
                let path = postui_core::project::environment_path(&ctx.root, name)
                    .strip_prefix(&ctx.root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                if y < bottom {
                    text(
                        buf,
                        x0,
                        y,
                        super::chooser::clip(&path, clip_w),
                        theme.text_muted,
                        theme.page,
                        false,
                    );
                }
            }
        }
    }
}
