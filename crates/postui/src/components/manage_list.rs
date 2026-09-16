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
    Button, ButtonKind, ControlState, ListRow, PROPERTY_MAX_W, Pill, PropertyRow, RowHighlight,
    TALL_PILL_H, button_min_width, fill, label_column, pill_min_width, text,
};
use crate::theme::Theme;
use postui_core::project::Project;
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
    /// Which half of the screen the keyboard is in: the row list, or the
    /// detail pane's controls (spec 2026-09-15 aliases round: `l`/Right
    /// enters, `h`/Left on the first control or Esc leaves -- the
    /// Variable Manager's list ⇄ detail rhythm).
    pub focus: ListFocus,
    /// The detail row the aim is on while `focus == Detail`.
    pub detail_row: DetailRow,
    /// The aimed control's visual index (left to right) within
    /// `detail_row`: a title button, or a TLS segment.
    pub detail_col: usize,
    /// How many title buttons the last draw painted, counted from the
    /// right (a narrow pane drops the leftmost first) -- the aim never
    /// lands on a dropped one.
    painted_buttons: usize,
    /// How many TLS segments the last draw painted, from the left.
    painted_tls: usize,
    /// A live row drag of the Spaces or Environments tab (spec §Space
    /// drag): while `Some`, `draw` lists `working` instead of the
    /// project's own list.
    pub drag: Option<ListDrag>,
    /// The row list's rect as of the last draw — with `scroll` it maps a
    /// pointer row back to a row index (`row_at_y`), and it is what the
    /// release handler tests a drop against.
    last_list: Rect,
    /// How many items the last draw listed, for `row_at_y`'s clamp.
    last_len: usize,
}

/// The Environments/Spaces tabs' two keyboard stops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ListFocus {
    #[default]
    List,
    Detail,
}

/// The detail pane's rows of controls, top to bottom. `Tls` exists only
/// on the Environments tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailRow {
    #[default]
    Buttons,
    Tls,
}

/// The detail pane's title buttons, in the order they read left to
/// right. Painted from the right edge inward, so a pane too narrow for
/// all of them drops the first of these first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailButton {
    MoveAll,
    Rename,
    Delete,
}

impl DetailButton {
    fn label(self) -> &'static str {
        match self {
            Self::MoveAll => "Move all requests\u{2026}",
            Self::Rename => "Rename",
            Self::Delete => "Delete",
        }
    }

    fn hit(self) -> Hit {
        match self {
            Self::MoveAll => Hit::ManageMoveAll,
            Self::Rename => Hit::ManageRename,
            Self::Delete => Hit::ManageDelete,
        }
    }

    /// The footer's verb for Enter on this button.
    fn verb(self) -> &'static str {
        match self {
            Self::MoveAll => "move all",
            Self::Rename => "rename",
            Self::Delete => "delete",
        }
    }
}

/// What the detail pane's aim is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AimedControl {
    Button(DetailButton),
    Tls(Option<postui_core::project::TlsPolicy>),
}

/// The TLS row's segments, left to right, with the policy each sets.
const TLS_SEGMENTS: [(&str, Option<postui_core::project::TlsPolicy>); 3] = [
    ("Per request", None),
    ("Verify", Some(postui_core::project::TlsPolicy::Verify)),
    ("Insecure", Some(postui_core::project::TlsPolicy::Insecure)),
];

/// A live drag of one list row: the working order the pointer has
/// arranged so far, over the displayed names of the tab it started on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListDrag {
    /// The tab the drag belongs to. A drag outlives no tab switch (that
    /// cancels it), but the commit reads it to know which list it is
    /// writing back.
    pub tab: ManageTab,
    /// The space or environment being dragged.
    pub name: String,
    /// Displayed order at drag start.
    pub original: Vec<String>,
    /// Current on-screen order.
    pub working: Vec<String>,
}

impl ManageList {
    /// The tab's items: the project's spaces, or its environments.
    pub fn items(tab: ManageTab, ctx: &Project) -> &[String] {
        match tab {
            ManageTab::Spaces => ctx.spaces(),
            _ => ctx.environments(),
        }
    }

    pub fn selected<'a>(&self, tab: ManageTab, ctx: &'a Project) -> Option<&'a str> {
        Self::items(tab, ctx).get(self.cursor).map(String::as_str)
    }

    /// Keeps the cursor on `name` after a reorder/rename/reload.
    pub fn select_name(&mut self, tab: ManageTab, ctx: &Project, name: &str) {
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

    /// The row index a screen row `y` maps to, given the last draw's
    /// scroll offset and list top — clamped to the drawn rows.
    pub fn row_at_y(&self, y: u16) -> usize {
        let rel = y.saturating_sub(self.last_list.y) as usize;
        (self.scroll + rel).min(self.last_len.saturating_sub(1))
    }

    /// The row list's rect as of the last draw — the area a row drag may
    /// be dropped on.
    pub fn list_rect(&self) -> Rect {
        self.last_list
    }

    /// Starts a drag of row `i`: records the displayed order as both
    /// `original` and the starting `working` order, and lands the cursor
    /// on the dragged row. Both list tabs reorder; the Variables tab has
    /// no list of its own here, and a row past the end refuses.
    pub fn begin_drag(&mut self, i: usize, tab: ManageTab, ctx: &Project) -> bool {
        if tab == ManageTab::Variables {
            return false;
        }
        let items = Self::items(tab, ctx);
        let Some(name) = items.get(i).cloned() else {
            return false;
        };
        let order = items.to_vec();
        self.drag = Some(ListDrag {
            tab,
            name,
            original: order.clone(),
            working: order,
        });
        self.cursor = i;
        true
    }

    /// Moves the dragged row to the slot under row `i` (clamped to the
    /// list) and takes the cursor with it. Returns whether the working
    /// order changed; the caller then repaints.
    pub fn drag_to_row(&mut self, i: usize) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        if drag.working.is_empty() {
            return false;
        }
        let Some(cur) = drag.working.iter().position(|n| *n == drag.name) else {
            return false;
        };
        let target = i.min(drag.working.len() - 1);
        if cur == target {
            return false;
        }
        let moved = drag.working.remove(cur);
        drag.working.insert(target, moved);
        self.cursor = target;
        true
    }

    /// Puts the working order back to the order the drag started from,
    /// without ending the drag: the preview of a cancel, shown while the
    /// pointer is outside the list (where a release would cancel). The
    /// cursor rides back with the dragged row, as it does on every other
    /// move. Returns whether anything changed.
    pub fn drag_reset(&mut self) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        if drag.working == drag.original {
            return false;
        }
        drag.working = drag.original.clone();
        let at = drag.working.iter().position(|n| *n == drag.name);
        if let Some(i) = at {
            self.cursor = i;
        }
        true
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

    /// The Environments tab's `t` key: steps env `name`'s TLS force
    /// through per request → verify → insecure.
    fn cycle_tls_action(ctx: &Project, name: &str) -> Action {
        use postui_core::project::{TlsPolicy, env_tls};
        Action::SetEnvTls {
            env: name.to_string(),
            policy: TlsPolicy::cycle(env_tls(ctx.meta(), name)),
        }
    }

    /// Moving `name` `delta` positions in its own list — the tab's
    /// reorder action, for alt+↑/↓ and the row menu's Move up/down.
    fn move_action(tab: ManageTab, name: &str, delta: i32) -> Action {
        match tab {
            ManageTab::Spaces => Action::MoveSpace {
                name: name.to_string(),
                delta,
            },
            _ => Action::MoveEnv {
                name: name.to_string(),
                delta,
            },
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

    /// The right-click menu for row `i` of `tab`'s list — the same
    /// actions the detail pane's buttons and footer keys offer, plus
    /// Move up/down, so a row can be worked on where it sits, like a
    /// Variables row can. `None` past the end of the list.
    pub fn context_menu(
        tab: ManageTab,
        ctx: &Project,
        i: usize,
    ) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::components::modal::MenuItem;
        let items = Self::items(tab, ctx);
        let name = items.get(i)?.as_str();
        let mut menu = vec![MenuItem::new(
            "Rename\u{2026}",
            Self::rename_action(tab, name),
        )];
        // The edge rows keep their move item, disabled, so the menu holds
        // its shape from row to row.
        let mv = |label: &str, delta: i32, can: bool| {
            if can {
                MenuItem::new(label, Self::move_action(tab, name, delta))
            } else {
                MenuItem::disabled(label)
            }
        };
        menu.push(mv("Move up", -1, i > 0));
        menu.push(mv("Move down", 1, i + 1 < items.len()));
        if tab == ManageTab::Spaces {
            menu.push(MenuItem::new(
                "Move all requests\u{2026}",
                Self::move_all_action(name),
            ));
        }
        menu.push(MenuItem::new("Delete", Self::delete_action(tab, name)));
        Some(menu)
    }

    /// The title buttons `tab`'s pane shows, left to right.
    fn buttons(tab: ManageTab) -> &'static [DetailButton] {
        match tab {
            ManageTab::Spaces => &[
                DetailButton::MoveAll,
                DetailButton::Rename,
                DetailButton::Delete,
            ],
            _ => &[DetailButton::Rename, DetailButton::Delete],
        }
    }

    /// The pane's rows of controls, top to bottom.
    fn detail_rows(tab: ManageTab) -> &'static [DetailRow] {
        match tab {
            ManageTab::Environments => &[DetailRow::Buttons, DetailRow::Tls],
            _ => &[DetailRow::Buttons],
        }
    }

    /// The first and last aimable column of `row` -- only painted
    /// controls count, so the aim never sits on something invisible.
    fn col_range(&self, tab: ManageTab, row: DetailRow) -> (usize, usize) {
        match row {
            DetailRow::Buttons => {
                let n = Self::buttons(tab).len();
                (n - self.painted_buttons.min(n), n.saturating_sub(1))
            }
            DetailRow::Tls => (0, self.painted_tls.saturating_sub(1)),
        }
    }

    fn clamp_detail_col(&mut self, tab: ManageTab) {
        let (first, last) = self.col_range(tab, self.detail_row);
        self.detail_col = self.detail_col.clamp(first, last.max(first));
    }

    /// Moves the keyboard into the detail pane, aimed at its first
    /// painted control. A no-op with nothing selected (the pane is empty).
    fn enter_detail(&mut self, tab: ManageTab, ctx: &Project) {
        if self.selected(tab, ctx).is_none() {
            return;
        }
        self.focus = ListFocus::Detail;
        self.detail_row = DetailRow::Buttons;
        self.detail_col = 0;
        self.clamp_detail_col(tab);
    }

    /// Hands the keyboard back to the row list (a click on a row does
    /// this too, so the next arrow moves what the click just selected).
    pub fn leave_detail(&mut self) {
        self.focus = ListFocus::List;
    }

    /// The aimed control, as the button it stands for and the verb the
    /// footer names Enter with.
    fn aimed(&self, tab: ManageTab) -> Option<(&'static str, AimedControl)> {
        match self.detail_row {
            DetailRow::Buttons => {
                let b = *Self::buttons(tab).get(self.detail_col)?;
                Some((b.verb(), AimedControl::Button(b)))
            }
            DetailRow::Tls => {
                let (label, policy) = TLS_SEGMENTS.get(self.detail_col)?;
                Some((label, AimedControl::Tls(*policy)))
            }
        }
    }

    /// What Enter on the aim dispatches -- the click's action exactly.
    fn aimed_action(&self, tab: ManageTab, ctx: &Project) -> Option<Action> {
        let name = self.selected(tab, ctx)?;
        Some(match self.aimed(tab)?.1 {
            AimedControl::Button(DetailButton::MoveAll) => Self::move_all_action(name),
            AimedControl::Button(DetailButton::Rename) => Self::rename_action(tab, name),
            AimedControl::Button(DetailButton::Delete) => Self::delete_action(tab, name),
            AimedControl::Tls(policy) => Action::SetEnvTls {
                env: name.to_string(),
                policy,
            },
        })
    }

    /// Keys while the detail pane has the keyboard. `Some` is a claimed
    /// key (with the action it dispatches, if any); `None` falls through
    /// to the list's command letters and the alt+↑/↓ reorder, which act
    /// on the same selected row from either stop. Page keys are inert
    /// here, as in the Variable Manager's grid.
    fn handle_detail_key(
        &mut self,
        ev: KeyEvent,
        tab: ManageTab,
        ctx: &Project,
    ) -> Option<Option<Action>> {
        if ev.modifiers.contains(KeyModifiers::ALT) {
            return None;
        }
        let plain = ev.modifiers.is_empty();
        let ctrl = ev.modifiers == KeyModifiers::CONTROL;
        let rows = Self::detail_rows(tab);
        let at = rows
            .iter()
            .position(|r| *r == self.detail_row)
            .unwrap_or(0);
        // A row change lands on the row's first painted control (the
        // Settings tab's rule): the columns of unrelated rows don't
        // correspond, so carrying one across would aim at random.
        let set_row = |this: &mut Self, i: usize| {
            this.detail_row = rows[i.min(rows.len() - 1)];
            this.detail_col = 0;
            this.clamp_detail_col(tab);
        };
        match ev.code {
            KeyCode::Esc | KeyCode::BackTab => {
                self.focus = ListFocus::List;
                Some(None)
            }
            KeyCode::Up | KeyCode::Char('k') if plain => {
                set_row(self, at.saturating_sub(1));
                Some(None)
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                set_row(self, at + 1);
                Some(None)
            }
            KeyCode::Home | KeyCode::Char('g') if plain => {
                set_row(self, 0);
                Some(None)
            }
            KeyCode::End | KeyCode::Char('G') => {
                set_row(self, rows.len() - 1);
                Some(None)
            }
            KeyCode::Left | KeyCode::Char('h') if plain => {
                let (first, _) = self.col_range(tab, self.detail_row);
                if self.detail_col <= first {
                    self.focus = ListFocus::List;
                } else {
                    self.detail_col -= 1;
                }
                Some(None)
            }
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') if plain => {
                self.detail_col += 1;
                self.clamp_detail_col(tab);
                Some(None)
            }
            KeyCode::PageUp | KeyCode::PageDown => Some(None),
            KeyCode::Char('d' | 'u' | 'f' | 'b') if ctrl => Some(None),
            KeyCode::Enter | KeyCode::Char(' ') if plain => Some(self.aimed_action(tab, ctx)),
            _ => None,
        }
    }

    /// The list's own keys.
    pub fn handle_key(
        &mut self,
        ev: KeyEvent,
        tab: ManageTab,
        ctx: &Project,
    ) -> Option<Action> {
        let len = Self::items(tab, ctx).len();
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        if self.focus == ListFocus::Detail {
            if self.selected(tab, ctx).is_none() {
                // The pane emptied under the aim (its row was deleted):
                // the keyboard goes home.
                self.focus = ListFocus::List;
            } else if let Some(claimed) = self.handle_detail_key(ev, tab, ctx) {
                return claimed;
            }
        }
        match ev.code {
            KeyCode::Esc => Some(Action::CloseScreen),
            KeyCode::Char('q') => Some(Action::Quit),
            // Into the detail pane's controls (the Variable Manager's
            // `l`/Right/Tab exactly).
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l')
                if ev.modifiers.is_empty() && self.focus == ListFocus::List =>
            {
                self.enter_detail(tab, ctx);
                None
            }
            KeyCode::Up if alt => Some(Self::move_action(tab, self.selected(tab, ctx)?, -1)),
            KeyCode::Down if alt => Some(Self::move_action(tab, self.selected(tab, ctx)?, 1)),
            KeyCode::Char('j') | KeyCode::Down if crate::keys::plain_letter(&ev) => {
                self.step(1, len);
                None
            }
            KeyCode::Char('k') | KeyCode::Up if crate::keys::plain_letter(&ev) => {
                self.step(-1, len);
                None
            }
            KeyCode::Char('g') | KeyCode::Home if crate::keys::plain_letter(&ev) => {
                self.step(i32::MIN / 2, len);
                None
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.step(i32::MAX / 2, len);
                None
            }
            KeyCode::PageDown => {
                self.step(self.page(), len);
                None
            }
            KeyCode::PageUp => {
                self.step(-self.page(), len);
                None
            }
            KeyCode::Char('d') if ev.modifiers == KeyModifiers::CONTROL => {
                self.step((self.page() / 2).max(1), len);
                None
            }
            KeyCode::Char('u') if ev.modifiers == KeyModifiers::CONTROL => {
                self.step(-(self.page() / 2).max(1), len);
                None
            }
            KeyCode::Char('f') if ev.modifiers == KeyModifiers::CONTROL => {
                self.step(self.page(), len);
                None
            }
            KeyCode::Char('b') if ev.modifiers == KeyModifiers::CONTROL => {
                self.step(-self.page(), len);
                None
            }
            KeyCode::Char('n') => Some(Self::new_action(tab)),
            KeyCode::Char('r') => Some(Self::rename_action(tab, self.selected(tab, ctx)?)),
            KeyCode::Char('m') if tab == ManageTab::Spaces => {
                Some(Self::move_all_action(self.selected(tab, ctx)?))
            }
            KeyCode::Char('t') if tab == ManageTab::Environments => {
                Some(Self::cycle_tls_action(ctx, self.selected(tab, ctx)?))
            }
            KeyCode::Char('d') | KeyCode::Delete if ev.modifiers.is_empty() => {
                Some(Self::delete_action(tab, self.selected(tab, ctx)?))
            }
            // `u` / `:` (undo, palette) are deliberately not named: an
            // unclaimed plain key reaches the router's whitelist from the
            // keymap (`App::unclaimed_screen_key`), so `keys.toml` governs
            // them here as everywhere.
            _ => None,
        }
    }

    /// Moves `cursor` by `delta` rows, clamped to `0..len` (or the single
    /// slot `0` when the list is empty), and marks it for scroll-into-view.
    /// The shared body behind Up/Down and every g/G/page-key alias.
    fn step(&mut self, delta: i32, len: usize) {
        let bound = len.max(1) as i32 - 1;
        self.cursor = (self.cursor as i32 + delta).clamp(0, bound.max(0)) as usize;
        self.ensure_visible = true;
    }

    /// Height of the row list as of the last draw, as a page size for
    /// ctrl+d/u/f/b and PageUp/PageDown (never zero, so a page always moves).
    fn page(&self) -> i32 {
        self.visible_rows.max(1) as i32
    }

    pub fn footer_chips(
        &self,
        tab: ManageTab,
        ctx: &Project,
    ) -> Vec<(&'static str, &'static str, Option<Action>)> {
        let selected = self.selected(tab, ctx);
        if self.focus == ListFocus::Detail && selected.is_some() {
            // The pane's stop: the arrows aim, Enter fires the aim (named
            // by what it does, like the Settings tab's file buttons), Esc
            // is back to the list rather than out of the screen.
            let mut chips = Vec::new();
            if Self::detail_rows(tab).len() > 1 {
                chips.push(("↑↓", "move", None));
            }
            chips.push(("←→", "select", None));
            if let Some((verb, _)) = self.aimed(tab) {
                chips.push(("enter", verb, self.aimed_action(tab, ctx)));
            }
            chips.push(("esc", "back", None));
            return chips;
        }
        let mut chips = vec![
            ("n", "new", Some(Self::new_action(tab))),
            ("r", "rename", selected.map(|n| Self::rename_action(tab, n))),
            ("d", "delete", selected.map(|n| Self::delete_action(tab, n))),
        ];
        if tab == ManageTab::Spaces {
            chips.push(("m", "move all", selected.map(Self::move_all_action)));
        } else {
            chips.push(("t", "tls", selected.map(|n| Self::cycle_tls_action(ctx, n))));
        }
        // Both lists reorder with the same keys.
        chips.push(("alt+↑↓", "reorder", None));
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
        ctx: &Project,
        requests: &BTreeMap<String, Vec<String>>,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        // A live drag paints the order the pointer has arranged so far;
        // disk truth only comes back once the drag is committed or
        // cancelled.
        let items = match self.drag.as_ref() {
            Some(d) if d.tab == tab => d.working.clone(),
            _ => Self::items(tab, ctx).to_vec(),
        };
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

    /// The Environments tab's `TLS` row: a label and three segments,
    /// `Per request` / `Verify` / `Insecure`, the current one filled.
    /// Clicking a segment sets the force (`Hit::ManageEnvTls`).
    #[allow(clippy::too_many_arguments)]
    fn draw_tls_control(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
        theme: &Theme,
        x0: u16,
        y: u16,
        bottom: u16,
        row_w: u16,
        label_w: u16,
        ctx: &Project,
        name: &str,
    ) {
        use postui_core::project::env_tls;
        if y >= bottom {
            return;
        }
        let current = env_tls(ctx.meta(), name);
        let slot = PropertyRow {
            label: "TLS",
            label_w,
            hovered: false,
            disabled: false,
            trailing: &[],
        }
        .paint(buf, hits, Rect::new(x0, y, row_w, 1), theme);
        let mut x = slot.rect.x;
        for (col, (seg, policy)) in TLS_SEGMENTS.into_iter().enumerate() {
            let w = pill_min_width(seg);
            // A segment that would run past the row is dropped rather
            // than clipped; it stays reachable by key.
            if x + w > slot.rect.x + slot.rect.width {
                break;
            }
            let hit = Hit::ManageEnvTls(policy);
            let rect = Rect {
                x,
                width: w,
                ..slot.rect
            };
            Pill {
                label: seg,
                kind: if policy == current {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                },
                state: if self.focus == ListFocus::Detail
                    && self.detail_row == DetailRow::Tls
                    && self.detail_col == col
                {
                    ControlState::Focused
                } else if hovered == Some(&hit) {
                    ControlState::Hover
                } else {
                    ControlState::Normal
                },
            }
            .paint(buf, rect, theme);
            hits.register(rect, hit);
            self.painted_tls += 1;
            x += w + 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_left(
        &mut self,
        frame: &mut Frame,
        left: Rect,
        theme: &Theme,
        tab: ManageTab,
        ctx: &Project,
        items: &[String],
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        let buf = frame.buffer_mut();
        fill(buf, left, theme.panel);
        if left.width <= 2 || left.height < TALL_PILL_H + 2 {
            self.visible_rows = 0;
            return;
        }
        // `left.y`, not `left.y + 1`: the block straddles its label row,
        // so starting a row down would put the label on `left.y + 2`
        // while the detail pane beside it puts its title-row buttons'
        // labels on `right.y + 1`.
        let button = Rect {
            x: left.x + 1,
            y: left.y,
            width: left.width - 2,
            height: TALL_PILL_H,
        };
        let state = if hovered == Some(&Hit::ManageNew) {
            ControlState::Hover
        } else {
            ControlState::Normal
        };
        // A full-size `Button`, the same one the dialogs use. Its caps
        // take the list column's `panel` straight from the buffer, so
        // sitting off the page needs no special handling here.
        let painted = Button {
            label: "+ New",
            kind: ButtonKind::Primary,
            state,
        }
        .paint(buf, button, theme);
        hits.register(painted, Hit::ManageNew);

        let list = Rect {
            x: left.x + 1,
            y: button.y + TALL_PILL_H + 1,
            width: left.width - 2,
            height: left.height.saturating_sub(TALL_PILL_H + 1),
        };
        self.visible_rows = list.height as usize;
        self.last_list = list;
        self.last_len = items.len();
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
            // The dragged row keeps the selected fill while it travels
            // (the cursor rides with it, so this only matters if the two
            // ever part company). Tab-gated exactly like `items` above:
            // only the drag's own tab lists the names it holds.
            let dragged = self
                .drag
                .as_ref()
                .is_some_and(|d| d.tab == tab && d.name == items[i]);
            let highlight = if dragged || i == self.cursor {
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
            // A row being dragged paints its grip glyph in the row's
            // first cell, which the label never uses — nothing shifts.
            if dragged {
                text(buf, list.x, y, "\u{22ee}", theme.accent, bg, false);
            }
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
        ctx: &Project,
        items: &[String],
        requests: &BTreeMap<String, Vec<String>>,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        let buf = frame.buffer_mut();
        fill(buf, right, theme.page);
        self.painted_buttons = 0;
        self.painted_tls = 0;
        let Some(name) = items.get(self.cursor) else {
            self.focus = ListFocus::List;
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
        let row_w = right.width.saturating_sub(4).clamp(1, PROPERTY_MAX_W);
        let label_w = label_column(&["File", "TLS", "Requests"]);
        let bottom = right.y + right.height;
        let mut y = right.y + 1;

        // --- title row: name + the pane's buttons, right-aligned --------
        // The Variables pane's layout exactly: the title at the left, the
        // buttons laid out from the pane's right edge inward in
        // keep-priority order (Delete outermost, like the selector grid),
        // one that would run into the title dropped rather than painted
        // over it. Dropped buttons stay reachable by key.
        //
        // A full-size `Button`, not a `Pill`: these act on the item the pane is
        // showing, not on one of its fields, and at a property row's
        // height they read as one more row of the grid below. They span
        // the blank row above the title and the blank row below it, so
        // nothing under them moves.
        // The block is `y - 1 ..= y + 1`, so the pane needs one row of
        // padding below the title -- which its own `height < 3` guard
        // above already promises.
        if y + 2 <= bottom {
            let title = match tab {
                ManageTab::Spaces => format!("Space: {}", ctx.space_name(name)),
                _ => format!("Environment: {}", ctx.env_name(name)),
            };
            text(buf, x0, y, &title, theme.text, theme.page, true);
            let buttons = Self::buttons(tab);
            let mut bx = right.x + right.width;
            for (col, button) in buttons.iter().enumerate().rev() {
                let (label, hit) = (button.label(), button.hit());
                let w = button_min_width(label);
                if bx < x0 + title.chars().count() as u16 + w + 3 {
                    break;
                }
                bx -= w + 1;
                let rect = Rect {
                    x: bx,
                    y: y - 1,
                    width: w,
                    height: TALL_PILL_H,
                };
                let state = if self.focus == ListFocus::Detail
                    && self.detail_row == DetailRow::Buttons
                    && self.detail_col == col
                {
                    ControlState::Focused
                } else if hovered == Some(&hit) {
                    ControlState::Hover
                } else {
                    ControlState::Normal
                };
                let painted = Button {
                    label,
                    kind: ButtonKind::Secondary,
                    state,
                }
                .paint(buf, rect, theme);
                hits.register(painted, hit);
                self.painted_buttons += 1;
            }
            y += 2;
        }

        // --- detail block ---------------------------------------------
        match tab {
            ManageTab::Spaces => {
                // The space's requests by name — the pane has the room, and
                // names say far more than a count. As many as fit, then a
                // "+ n more" line for the rest.
                let names = requests.get(name).map(Vec::as_slice).unwrap_or(&[]);
                if y >= bottom {
                    return;
                }
                let slot = PropertyRow {
                    label: "Requests",
                    label_w,
                    hovered: false,
                    disabled: false,
                    trailing: &[],
                }
                .paint(buf, hits, Rect::new(x0, y, row_w, 1), theme);
                let list_x = slot.rect.x;
                if names.is_empty() {
                    text(buf, list_x, y, "(none)", theme.text_muted, slot.bg, false);
                    return;
                }
                // The first name shares the label's row; the rest stack
                // under it in the same column.
                let room = (bottom - y) as usize;
                let shown = if names.len() > room {
                    room.saturating_sub(1)
                } else {
                    names.len()
                };
                for n in &names[..shown] {
                    text(
                        buf,
                        list_x,
                        y,
                        super::chooser::clip(n, slot.rect.width),
                        theme.text,
                        slot.bg,
                        false,
                    );
                    y += 1;
                }
                if shown < names.len() && y < bottom {
                    let more = format!("+ {} more", names.len() - shown);
                    text(buf, list_x, y, &more, theme.text_muted, slot.bg, false);
                }
            }
            _ => {
                let path = format!("environments/{name}.toml");
                if y < bottom {
                    let slot = PropertyRow {
                        label: "File",
                        label_w,
                        hovered: false,
                        disabled: false,
                        trailing: &[],
                    }
                    .paint(buf, hits, Rect::new(x0, y, row_w, 1), theme);
                    text(
                        buf,
                        slot.rect.x,
                        y,
                        super::chooser::clip(&path, slot.rect.width),
                        theme.text_muted,
                        slot.bg,
                        false,
                    );
                    y += 2;
                }
                self.draw_tls_control(
                    buf, hits, hovered, theme, x0, y, bottom, row_w, label_w, ctx, name,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project with three spaces (`main`, `auth`, `billing`) and the
    /// context that lists them.
    fn ctx() -> (Project, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (mut ctx, _) = Project::init(dir.path(), None).unwrap();
        ctx.create_space("auth").unwrap();
        ctx.create_space("billing").unwrap();
        assert_eq!(ctx.spaces(), ["main", "auth", "billing"]);
        (ctx, dir)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn vim_aliases_are_strict_synonyms_in_the_manage_list() {
        let (ctx, _dir) = ctx();
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let pairs = [
            (key(KeyCode::Char('j')), key(KeyCode::Down)),
            (key(KeyCode::Char('k')), key(KeyCode::Up)),
            (key(KeyCode::Char('g')), key(KeyCode::Home)),
            (
                KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
                key(KeyCode::End),
            ),
            (ctrl('f'), key(KeyCode::PageDown)),
            (ctrl('b'), key(KeyCode::PageUp)),
        ];
        for (alias, canonical) in pairs {
            let (mut a, mut b) = (ManageList::default(), ManageList::default());
            a.cursor = 1;
            b.cursor = 1;
            a.visible_rows = 2;
            b.visible_rows = 2;
            let ra = a.handle_key(alias, ManageTab::Spaces, &ctx);
            let rb = b.handle_key(canonical, ManageTab::Spaces, &ctx);
            assert_eq!(ra, rb, "{alias:?}");
            assert_eq!(a.cursor, b.cursor, "{alias:?}");
        }
        let mut l = ManageList {
            visible_rows: 2,
            ..Default::default()
        };
        l.handle_key(ctrl('f'), ManageTab::Spaces, &ctx);
        assert_eq!(l.cursor, 2, "three spaces: clamped to the last");
        l.handle_key(ctrl('b'), ManageTab::Spaces, &ctx);
        assert_eq!(l.cursor, 0);
        // Half pages: ctrl+d/ctrl+u move by half the visible rows.
        l.handle_key(ctrl('d'), ManageTab::Spaces, &ctx);
        assert_eq!(l.cursor, 1, "half of a 2-row page");
        l.handle_key(ctrl('u'), ManageTab::Spaces, &ctx);
        assert_eq!(l.cursor, 0);
    }

    fn draw_at(l: &mut ManageList, tab: ManageTab, ctx: &Project, w: u16) -> HitMap {
        let theme = Theme::dark();
        let backend = ratatui::backend::TestBackend::new(w, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        let requests = BTreeMap::new();
        terminal
            .draw(|f| l.draw(f, f.area(), &theme, tab, ctx, &requests, &mut hits, None))
            .unwrap();
        hits
    }

    /// The detail pane is a focus stop like the Variable Manager's: `l`
    /// (Right) enters it from the list, `h` (Left) on its first control
    /// hands the keyboard back, and Esc there is "back", not "close".
    #[test]
    fn l_enters_the_detail_pane_and_h_on_its_first_control_leaves() {
        let (ctx, _dir) = ctx();
        let mut l = ManageList::default();
        draw_at(&mut l, ManageTab::Environments, &ctx, 100);
        assert_eq!(l.focus, ListFocus::List);
        assert_eq!(l.handle_key(key(KeyCode::Char('l')), ManageTab::Environments, &ctx), None);
        assert_eq!(l.focus, ListFocus::Detail);
        // Environments: the buttons row reads Rename, Delete; the aim
        // starts on the first one.
        assert_eq!(l.detail_col, 0);
        assert_eq!(l.handle_key(key(KeyCode::Char('l')), ManageTab::Environments, &ctx), None);
        assert_eq!(l.detail_col, 1);
        assert_eq!(l.handle_key(key(KeyCode::Char('l')), ManageTab::Environments, &ctx), None);
        assert_eq!(l.detail_col, 1, "clamped to the last button");
        l.handle_key(key(KeyCode::Char('h')), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_col, 0);
        assert_eq!(l.focus, ListFocus::Detail);
        l.handle_key(key(KeyCode::Char('h')), ManageTab::Environments, &ctx);
        assert_eq!(l.focus, ListFocus::List, "h on the first control leaves");
        l.handle_key(key(KeyCode::Right), ManageTab::Environments, &ctx);
        assert_eq!(l.focus, ListFocus::Detail);
        assert_eq!(
            l.handle_key(key(KeyCode::Esc), ManageTab::Environments, &ctx),
            None,
            "Esc in the pane goes back to the list, not out of the screen"
        );
        assert_eq!(l.focus, ListFocus::List);
        assert_eq!(
            l.handle_key(key(KeyCode::Esc), ManageTab::Environments, &ctx),
            Some(Action::CloseScreen)
        );
    }

    #[test]
    fn vim_aliases_are_strict_synonyms_in_the_detail_pane() {
        let (ctx, _dir) = ctx();
        let pairs = [
            (key(KeyCode::Char('j')), key(KeyCode::Down)),
            (key(KeyCode::Char('k')), key(KeyCode::Up)),
            (key(KeyCode::Char('h')), key(KeyCode::Left)),
            (key(KeyCode::Char('l')), key(KeyCode::Right)),
            (key(KeyCode::Char('g')), key(KeyCode::Home)),
            (
                KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
                key(KeyCode::End),
            ),
        ];
        for (alias, canonical) in pairs {
            let (mut a, mut b) = (ManageList::default(), ManageList::default());
            for l in [&mut a, &mut b] {
                draw_at(l, ManageTab::Environments, &ctx, 100);
                l.focus = ListFocus::Detail;
                l.detail_row = DetailRow::Tls;
                l.detail_col = 1;
            }
            assert_eq!(
                a.handle_key(alias, ManageTab::Environments, &ctx),
                b.handle_key(canonical, ManageTab::Environments, &ctx),
                "{alias:?}"
            );
            assert_eq!(a.focus, b.focus, "{alias:?}");
            assert_eq!(a.detail_row, b.detail_row, "{alias:?}");
            assert_eq!(a.detail_col, b.detail_col, "{alias:?}");
        }
    }

    /// ↑/↓ walk the pane's rows (buttons, then TLS) on the Environments
    /// tab; the Spaces pane has only its buttons row, so they are inert.
    #[test]
    fn detail_rows_walk_between_the_buttons_and_the_tls_segments() {
        let (ctx, _dir) = ctx();
        let mut l = ManageList::default();
        draw_at(&mut l, ManageTab::Environments, &ctx, 100);
        l.handle_key(key(KeyCode::Right), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_row, DetailRow::Buttons);
        l.handle_key(key(KeyCode::Down), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_row, DetailRow::Tls);
        l.handle_key(key(KeyCode::Down), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_row, DetailRow::Tls, "clamped to the last row");
        l.handle_key(key(KeyCode::Up), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_row, DetailRow::Buttons);
        l.handle_key(key(KeyCode::End), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_row, DetailRow::Tls);
        l.handle_key(key(KeyCode::Home), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_row, DetailRow::Buttons);

        let mut s = ManageList::default();
        draw_at(&mut s, ManageTab::Spaces, &ctx, 100);
        s.handle_key(key(KeyCode::Right), ManageTab::Spaces, &ctx);
        s.handle_key(key(KeyCode::Down), ManageTab::Spaces, &ctx);
        assert_eq!(s.detail_row, DetailRow::Buttons);
        assert_eq!(s.focus, ListFocus::Detail);
    }

    /// Enter (and space) fire whatever the aim is on: a title button, or a
    /// TLS segment -- the very actions a click on them dispatches.
    #[test]
    fn enter_in_the_detail_pane_fires_the_aimed_control() {
        use postui_core::project::TlsPolicy;
        let (ctx, _dir) = ctx();
        let env = ctx.environments()[0].clone();
        let mut l = ManageList::default();
        draw_at(&mut l, ManageTab::Environments, &ctx, 100);
        l.handle_key(key(KeyCode::Right), ManageTab::Environments, &ctx);
        assert_eq!(
            l.handle_key(key(KeyCode::Enter), ManageTab::Environments, &ctx),
            Some(Action::PromptRenameEnv(env.clone()))
        );
        l.handle_key(key(KeyCode::Right), ManageTab::Environments, &ctx);
        assert_eq!(
            l.handle_key(key(KeyCode::Char(' ')), ManageTab::Environments, &ctx),
            Some(Action::DeleteEnv(env.clone()))
        );
        l.handle_key(key(KeyCode::Down), ManageTab::Environments, &ctx);
        l.handle_key(key(KeyCode::Home), ManageTab::Environments, &ctx);
        l.handle_key(key(KeyCode::Down), ManageTab::Environments, &ctx);
        assert_eq!(l.detail_row, DetailRow::Tls);
        assert_eq!(l.detail_col, 0);
        l.handle_key(key(KeyCode::Right), ManageTab::Environments, &ctx);
        assert_eq!(
            l.handle_key(key(KeyCode::Enter), ManageTab::Environments, &ctx),
            Some(Action::SetEnvTls {
                env: env.clone(),
                policy: Some(TlsPolicy::Verify)
            })
        );
        // The Spaces pane: Move all, Rename, Delete from the left.
        let mut s = ManageList::default();
        draw_at(&mut s, ManageTab::Spaces, &ctx, 100);
        s.handle_key(key(KeyCode::Right), ManageTab::Spaces, &ctx);
        assert_eq!(
            s.handle_key(key(KeyCode::Enter), ManageTab::Spaces, &ctx),
            Some(Action::PromptMoveAllRequests("main".into()))
        );
    }

    /// The command letters and the reorder chord keep working from the
    /// pane: the aim is about *which* control Enter fires, not a mode.
    #[test]
    fn command_letters_and_reorder_still_work_from_the_detail_pane() {
        let (ctx, _dir) = ctx();
        let mut l = ManageList::default();
        draw_at(&mut l, ManageTab::Spaces, &ctx, 100);
        l.cursor = 1;
        l.handle_key(key(KeyCode::Right), ManageTab::Spaces, &ctx);
        assert_eq!(
            l.handle_key(key(KeyCode::Char('r')), ManageTab::Spaces, &ctx),
            Some(Action::PromptRenameSpace("auth".into()))
        );
        assert_eq!(
            l.handle_key(key(KeyCode::Char('u')), ManageTab::Spaces, &ctx),
            None,
            "`u` is the router's (keymap) alias, left unclaimed here"
        );
        let alt_down = KeyEvent::new(KeyCode::Down, KeyModifiers::ALT);
        assert!(matches!(
            l.handle_key(alt_down, ManageTab::Spaces, &ctx),
            Some(Action::MoveSpace { .. })
        ));
        assert_eq!(l.focus, ListFocus::Detail);
    }

    /// A pane too narrow for every title button drops the leftmost ones
    /// (Move all first); the aim only ever lands on a painted control.
    #[test]
    fn the_aim_skips_buttons_the_narrow_pane_dropped() {
        let (ctx, _dir) = ctx();
        let mut l = ManageList::default();
        // The widest pane that drops Move all but still fits Rename.
        let w = (40..120)
            .rev()
            .find(|w| {
                let mut probe = ManageList::default();
                let hits = draw_at(&mut probe, ManageTab::Spaces, &ctx, *w);
                hits.rect_of(&Hit::ManageMoveAll).is_none()
                    && hits.rect_of(&Hit::ManageRename).is_some()
            })
            .expect("some width drops exactly the first button");
        draw_at(&mut l, ManageTab::Spaces, &ctx, w);
        l.handle_key(key(KeyCode::Right), ManageTab::Spaces, &ctx);
        assert_eq!(
            l.handle_key(key(KeyCode::Enter), ManageTab::Spaces, &ctx),
            Some(Action::PromptRenameSpace("main".into())),
            "the first painted button, not the dropped one"
        );
        l.handle_key(key(KeyCode::Left), ManageTab::Spaces, &ctx);
        assert_eq!(l.focus, ListFocus::List);
    }

    /// The aimed control paints focused, exactly as the Settings tab's
    /// aimed file button does.
    #[test]
    fn the_aimed_control_paints_focused() {
        let (ctx, _dir) = ctx();
        let theme = Theme::dark();
        let mut l = ManageList::default();
        draw_at(&mut l, ManageTab::Environments, &ctx, 100);
        l.handle_key(key(KeyCode::Right), ManageTab::Environments, &ctx);
        l.handle_key(key(KeyCode::Down), ManageTab::Environments, &ctx);
        let backend = ratatui::backend::TestBackend::new(100, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        let requests = BTreeMap::new();
        terminal
            .draw(|f| {
                l.draw(
                    f,
                    f.area(),
                    &theme,
                    ManageTab::Environments,
                    &ctx,
                    &requests,
                    &mut hits,
                    None,
                )
            })
            .unwrap();
        let seg = hits.rect_of(&Hit::ManageEnvTls(None)).expect("aimed segment");
        let buf = terminal.backend().buffer();
        let (focused, _) =
            crate::paint::control_face(&theme, ButtonKind::Primary, ControlState::Focused);
        let (normal, _) = crate::paint::control_face(&theme, ButtonKind::Primary, ControlState::Normal);
        assert_ne!(focused, normal, "the theme distinguishes focus");
        assert_eq!(buf.cell((seg.x + 1, seg.y)).unwrap().bg, focused);
    }

    #[test]
    fn detail_pane_chips_name_the_aimed_control() {
        let (ctx, _dir) = ctx();
        let env = ctx.environments()[0].clone();
        let mut l = ManageList::default();
        draw_at(&mut l, ManageTab::Environments, &ctx, 100);
        l.handle_key(key(KeyCode::Right), ManageTab::Environments, &ctx);
        let chips = l.footer_chips(ManageTab::Environments, &ctx);
        assert!(chips.iter().any(|(k, l, _)| *k == "↑↓" && *l == "move"), "{chips:?}");
        assert!(chips.iter().any(|(k, l, _)| *k == "←→" && *l == "select"), "{chips:?}");
        assert!(
            chips.iter().any(|(k, l, a)| *k == "enter"
                && *l == "rename"
                && *a == Some(Action::PromptRenameEnv(env.clone()))),
            "{chips:?}"
        );
        assert!(chips.iter().any(|(k, l, _)| *k == "esc" && *l == "back"), "{chips:?}");
        let mut s = ManageList::default();
        draw_at(&mut s, ManageTab::Spaces, &ctx, 100);
        s.handle_key(key(KeyCode::Right), ManageTab::Spaces, &ctx);
        let chips = s.footer_chips(ManageTab::Spaces, &ctx);
        assert!(!chips.iter().any(|(k, _, _)| *k == "↑↓"), "one row: {chips:?}");
    }

    #[test]
    fn a_drag_rearranges_the_displayed_order_and_the_cursor_rides_along() {
        let (ctx, _dir) = ctx();
        let mut l = ManageList::default();
        assert!(l.begin_drag(0, ManageTab::Spaces, &ctx));
        let d = l.drag.as_ref().unwrap();
        assert_eq!(d.name, "main");
        assert_eq!(d.original, ["main", "auth", "billing"]);
        assert_eq!(d.working, ["main", "auth", "billing"]);

        assert!(l.drag_to_row(2));
        assert_eq!(
            l.drag.as_ref().unwrap().working,
            ["auth", "billing", "main"]
        );
        assert_eq!(l.cursor, 2, "the cursor follows the dragged row");
        assert!(!l.drag_to_row(2), "no change reports false");
        assert!(l.drag_to_row(0));
        assert_eq!(
            l.drag.as_ref().unwrap().working,
            ["main", "auth", "billing"]
        );
        assert!(l.drag_to_row(9), "past the end clamps to the last slot");
        assert_eq!(
            l.drag.as_ref().unwrap().working,
            ["auth", "billing", "main"]
        );
        assert_eq!(l.cursor, 2);
    }

    #[test]
    fn begin_drag_takes_either_list_tab_and_refuses_a_row_past_the_end() {
        let (ctx, _dir) = ctx();
        let mut l = ManageList::default();
        assert!(l.begin_drag(0, ManageTab::Environments, &ctx));
        let d = l.drag.take().expect("the environments list drags too");
        assert_eq!(d.tab, ManageTab::Environments);
        assert_eq!(d.original, ctx.environments());
        assert!(!l.begin_drag(0, ManageTab::Variables, &ctx));
        assert!(!l.begin_drag(3, ManageTab::Spaces, &ctx));
        assert!(l.drag.is_none());
    }

    #[test]
    fn draw_lists_the_working_order_with_a_grip_on_the_dragged_row() {
        let (ctx, _dir) = ctx();
        let mut l = ManageList::default();
        assert!(l.begin_drag(0, ManageTab::Spaces, &ctx));
        assert!(l.drag_to_row(1));

        let theme = Theme::dark();
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        let requests = BTreeMap::new();
        terminal
            .draw(|f| {
                l.draw(
                    f,
                    f.area(),
                    &theme,
                    ManageTab::Spaces,
                    &ctx,
                    &requests,
                    &mut hits,
                    None,
                )
            })
            .unwrap();

        let dragged = hits.rect_of(&Hit::ManageRow(1)).expect("dragged row hit");
        let other = hits.rect_of(&Hit::ManageRow(0)).expect("other row hit");
        let buf = terminal.backend().buffer();
        let line = |r: Rect| {
            (r.x..r.x + r.width)
                .map(|x| buf[(x, r.y)].symbol())
                .collect::<String>()
        };
        assert!(
            line(other).contains("auth"),
            "the working order is what is drawn: {}",
            line(other)
        );
        assert!(line(dragged).contains("main"), "{}", line(dragged));
        assert_eq!(
            buf[(dragged.x, dragged.y)].symbol(),
            "\u{22ee}",
            "the dragged row shows the grip glyph"
        );
        assert_ne!(
            buf[(other.x, other.y)].symbol(),
            "\u{22ee}",
            "other rows never show the grip glyph"
        );
        assert_eq!(l.row_at_y(dragged.y), 1, "row_at_y maps the drawn rows");
        assert!(l.list_rect().contains(ratatui::layout::Position {
            x: dragged.x,
            y: dragged.y
        }));
    }

    /// The env detail pane is properties of one environment, so no band
    /// — and its TLS segments are one row now, like every other Manage
    /// control.
    #[test]
    fn the_env_detail_pane_paints_property_rows_not_a_band() {
        use crate::hit::HitMap;
        use postui_core::project::TlsPolicy;
        let (project, _dir) = ctx();
        let theme = Theme::dark();
        let mut list = ManageList::default();
        let mut hits = HitMap::default();
        let requests: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        term.draw(|f| {
            list.draw(
                f,
                Rect::new(0, 0, 120, 30),
                &theme,
                ManageTab::Environments,
                &project,
                &requests,
                &mut hits,
                None,
            );
        })
        .unwrap();

        let buf = term.backend().buffer();
        for y in 0..30 {
            for x in LEFT_W..120 {
                assert_ne!(
                    buf.cell((x, y)).unwrap().bg,
                    theme.selection,
                    "band in the detail pane at {x},{y}"
                );
            }
        }
        let seg = hits
            .rect_of(&Hit::ManageEnvTls(Some(TlsPolicy::Verify)))
            .expect("the TLS segments are hittable");
        assert_eq!(seg.height, 1, "one-row pills, not three-row buttons");
    }

    /// The `+ New` button sits in the left list column, which is painted
    /// `panel` -- not the `page` the detail pane beside it uses. Its caps
    /// take their surface from the buffer rather than from an argument,
    /// so this is the test that catches a caller laying a button out
    /// before the surface under it is painted: the cap would fringe the
    /// button with a wedge of the wrong colour, or of `page`.
    #[test]
    fn the_column_button_caps_against_the_panel_it_sits_on() {
        use crate::hit::HitMap;
        let (project, _dir) = ctx();
        let theme = Theme::dark();
        let mut list = ManageList::default();
        let mut hits = HitMap::default();
        let requests: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        term.draw(|f| {
            list.draw(
                f,
                Rect::new(0, 0, 120, 30),
                &theme,
                ManageTab::Environments,
                &project,
                &requests,
                &mut hits,
                None,
            );
        })
        .unwrap();

        let btn = hits.rect_of(&Hit::ManageNew).expect("the + New button");
        assert_eq!(btn.height, TALL_PILL_H, "a full-size button block");
        let buf = term.backend().buffer();
        assert_ne!(theme.panel, theme.page, "the test is vacuous otherwise");
        for x in btn.x..btn.x + btn.width {
            let top = buf.cell((x, btn.y)).unwrap();
            assert_eq!(top.symbol(), crate::paint::cap::CAP_TOP, "top cap x={x}");
            assert_eq!(top.bg, theme.panel, "top cap surface at x={x}");
            let bottom = buf.cell((x, btn.y + 2)).unwrap();
            assert_eq!(
                bottom.symbol(),
                crate::paint::cap::CAP_BOTTOM,
                "bottom cap x={x}"
            );
            assert_eq!(bottom.fg, theme.panel, "bottom cap surface at x={x}");
        }
    }

    /// The Spaces detail pane paints its requests as one property row:
    /// the first name shares the `Requests` label's row, the rest stack
    /// under it in the same column, and the list fills every row it has
    /// — the `+ n more` line lands on the pane's last row, not one short
    /// of it.
    #[test]
    fn the_spaces_pane_lists_requests_from_the_label_row_and_fills_the_pane() {
        use crate::hit::HitMap;
        let (project, _dir) = ctx();
        let theme = Theme::dark();
        let mut list = ManageList::default();
        let mut hits = HitMap::default();
        // Ten names into a pane with room for seven rows of list.
        let names: Vec<String> = (0..10).map(|i| format!("req{i}")).collect();
        let mut requests: BTreeMap<String, Vec<String>> = BTreeMap::new();
        requests.insert("main".to_string(), names.clone());
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 10)).unwrap();
        term.draw(|f| {
            list.draw(
                f,
                Rect::new(0, 0, 120, 10),
                &theme,
                ManageTab::Spaces,
                &project,
                &requests,
                &mut hits,
                None,
            );
        })
        .unwrap();

        let row = |y: u16| {
            let buf = term.backend().buffer();
            (LEFT_W..120)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        };
        // The label row: `Requests` at the pane's inset, the first name
        // beside it in the label column — not a row below it.
        let label_y = (0..10)
            .find(|y| row(*y).trim_start().starts_with("Requests"))
            .expect("the Requests label row");
        let x0 = LEFT_W + 2;
        let list_x = x0 + label_column(&["File", "TLS", "Requests"]);
        let at = |x: u16, y: u16, len: usize| {
            let buf = term.backend().buffer();
            (x..x + len as u16)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        };
        assert_eq!(at(x0, label_y, 8), "Requests");
        assert_eq!(
            at(list_x, label_y, 4),
            "req0",
            "the first name shares the row"
        );
        // The rest stack under it, aligned to the same column, and the
        // `+ n more` line takes the pane's very last row: every row the
        // pane had is used, none dropped to an off-by-one.
        for (i, name) in names.iter().enumerate().take(6) {
            assert_eq!(at(list_x, label_y + i as u16, 4), name.as_str(), "row {i}");
        }
        assert_eq!(
            at(list_x, label_y + 6, 8),
            "+ 4 more",
            "the overflow line, in the request column"
        );
        assert_eq!(label_y + 6, 9, "and on the pane's last row");
    }

    /// A space with nothing in it says `(none)` in the control column,
    /// so the row still reads as a property with an empty value rather
    /// than as a heading that changed its wording.
    #[test]
    fn a_space_with_no_requests_paints_none_in_the_control_column() {
        use crate::hit::HitMap;
        let (project, _dir) = ctx();
        let theme = Theme::dark();
        let mut list = ManageList::default();
        let mut hits = HitMap::default();
        let requests: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 10)).unwrap();
        term.draw(|f| {
            list.draw(
                f,
                Rect::new(0, 0, 120, 10),
                &theme,
                ManageTab::Spaces,
                &project,
                &requests,
                &mut hits,
                None,
            );
        })
        .unwrap();

        let buf = term.backend().buffer();
        let row = |y: u16| {
            (LEFT_W..120)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        };
        let label_y = (0..10)
            .find(|y| row(*y).trim_start().starts_with("Requests"))
            .expect("the Requests label row survives an empty space");
        let list_x = LEFT_W + 2 + label_column(&["File", "TLS", "Requests"]);
        let cell = |x: u16, len: usize| {
            (x..x + len as u16)
                .map(|x| buf[(x, label_y)].symbol())
                .collect::<String>()
        };
        assert_eq!(cell(list_x, 6), "(none)");
        for y in 0..10 {
            assert!(!row(y).contains("No requests"), "the old heading is gone");
        }
    }
}
