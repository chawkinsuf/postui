//! The Manage screen's Environments and Spaces tabs: one list-edit face
//! for both (spec "Manage screen"). Left: `+ New` and the item list.
//! Right: the selected item's name (edit in place = rename), a button
//! row, and a muted detail line.

use crate::action::Action;
use crate::components::line_input::LineInput;
use crate::components::manage::ManageTab;
use crate::hit::{Hit, HitMap};
use crate::paint::{
    BUTTON_HEIGHT, Button, ButtonKind, ControlState, FIELD_HEIGHT, ListRow, RowHighlight,
    TextField, button_min_width, fill, text,
};
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::collections::BTreeMap;

/// The left column's width — the Variable Manager's, so both faces of the
/// Manage screen line up as the tab strip switches between them.
pub const LEFT_W: u16 = crate::components::varmanager::LEFT_W;

#[derive(Default)]
pub struct ManageList {
    /// Index into the current tab's item list.
    pub cursor: usize,
    pub scroll: usize,
    /// The name field under edit (`None` while the list has the keyboard).
    pub editing: Option<LineInput>,
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

    /// Drops every trace of the previous tab's list: its cursor, its
    /// scroll, and any name edit in flight (the tab strip switches to a
    /// different set of items entirely).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn start_edit(&mut self, name: &str) {
        self.editing = Some(LineInput::new(name));
    }

    /// `Enter` on the name field: the rename action for a changed,
    /// non-empty name; `None` (and the edit closes) otherwise.
    pub fn commit_edit(&mut self, tab: ManageTab, ctx: &ProjectContext) -> Option<Action> {
        let input = self.editing.take()?;
        let to = input.text().trim().to_string();
        let from = self.selected(tab, ctx)?.to_string();
        if to.is_empty() || to == from {
            return None;
        }
        Some(match tab {
            ManageTab::Spaces => Action::RenameSpace { from, to },
            _ => Action::RenameEnv { from, to },
        })
    }

    fn new_action(tab: ManageTab) -> Action {
        match tab {
            ManageTab::Spaces => Action::OpenNewSpacePrompt,
            _ => Action::OpenNewEnvPrompt,
        }
    }

    fn delete_action(tab: ManageTab, name: &str) -> Action {
        match tab {
            ManageTab::Spaces => Action::DeleteSpace(name.to_string()),
            _ => Action::DeleteEnv(name.to_string()),
        }
    }

    /// The list's own keys. `None` while a name is under edit: `App` owns
    /// those (it needs the mutable project access a commit takes).
    pub fn handle_key(
        &mut self,
        ev: KeyEvent,
        tab: ManageTab,
        ctx: &ProjectContext,
    ) -> Option<Action> {
        if self.editing.is_some() {
            return None; // App owns the edit's keys
        }
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
            KeyCode::Enter => {
                if let Some(name) = self.selected(tab, ctx) {
                    let name = name.to_string();
                    self.start_edit(&name);
                }
                None
            }
            KeyCode::Char('n') => Some(Self::new_action(tab)),
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
        if self.editing.is_some() {
            return vec![("enter", "save", None), ("esc", "cancel", None)];
        }
        let mut chips = vec![
            ("enter", "rename", None),
            ("n", "new", Some(Self::new_action(tab))),
            (
                "d",
                "delete",
                self.selected(tab, ctx).map(|n| Self::delete_action(tab, n)),
            ),
        ];
        if tab == ManageTab::Spaces {
            chips.push(("alt+↑↓", "move", None));
        }
        chips
    }

    pub fn handle_scroll(&mut self, delta: i16) {
        let max = self.scroll.saturating_add(self.visible_rows);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
        self.ensure_visible = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        body: Rect,
        theme: &Theme,
        tab: ManageTab,
        ctx: &ProjectContext,
        counts: &BTreeMap<String, usize>,
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
        self.draw_right(frame, right, theme, tab, ctx, &items, counts, hits, hovered);
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
        let active = match tab {
            ManageTab::Spaces => Some(ctx.active_space.as_str()),
            _ => ctx.active_env.as_deref(),
        };
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
            let label = match tab {
                ManageTab::Spaces => format!("{}  {}", i + 1, items[i]),
                _ => items[i].clone(),
            };
            let label = if active == Some(items[i].as_str()) {
                format!("{label} \u{2713}")
            } else {
                label
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
        counts: &BTreeMap<String, usize>,
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
        if right.width < 20 || right.height < 12 {
            return;
        }
        let x = right.x + 2;
        let title = match tab {
            ManageTab::Spaces => format!("Space: {name}"),
            _ => format!("Environment: {name}"),
        };
        text(buf, x, right.y + 1, &title, theme.text, theme.page, true);

        let field = Rect {
            x,
            y: right.y + 3,
            width: right.width.saturating_sub(4).min(40),
            height: FIELD_HEIGHT,
        };
        let (content, state) = match &self.editing {
            Some(input) => (
                input.draw_line_windowed(true, theme, field.width.saturating_sub(2)),
                ControlState::Focused,
            ),
            None => (
                Line::raw(name.clone()),
                if hovered == Some(&Hit::ManageName) {
                    ControlState::Hover
                } else {
                    ControlState::Normal
                },
            ),
        };
        TextField { content, state }.paint(buf, field, theme);
        hits.register(field, Hit::ManageName);

        let mut bx = x;
        let mut by = field.y + FIELD_HEIGHT + 1;
        let mut buttons: Vec<(&str, Hit)> =
            vec![("Rename", Hit::ManageRename), ("Delete", Hit::ManageDelete)];
        if tab == ManageTab::Spaces {
            buttons.push(("Move up", Hit::ManageMoveUp));
            buttons.push(("Move down", Hit::ManageMoveDown));
            buttons.push(("Move all requests to \u{25be}", Hit::ManageMoveAll));
        }
        // The Spaces tab's five buttons rarely fit on one row: a button
        // that would run past the pane wraps onto the next row rather
        // than being dropped, so no capability goes missing on a narrow
        // pane (the detail line follows whatever row they end on).
        for (label, hit) in buttons {
            let w = button_min_width(label);
            if bx > x && bx + w > right.x + right.width {
                bx = x;
                by += BUTTON_HEIGHT;
            }
            if bx + w > right.x + right.width || by + BUTTON_HEIGHT > right.y + right.height {
                break;
            }
            let rect = Rect {
                x: bx,
                y: by,
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
            bx += w + 1;
        }

        let detail = match tab {
            ManageTab::Spaces => {
                let n = counts.get(name).copied().unwrap_or(0);
                format!("{n} request{}", if n == 1 { "" } else { "s" })
            }
            _ => postui_core::project::environment_path(&ctx.root, name)
                .strip_prefix(&ctx.root)
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        let detail_y = by + BUTTON_HEIGHT + 1;
        if detail_y < right.y + right.height {
            text(
                buf,
                x,
                detail_y,
                super::chooser::clip(&detail, right.width.saturating_sub(3)),
                theme.text_muted,
                theme.page,
                false,
            );
        }
    }
}
