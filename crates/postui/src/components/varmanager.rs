//! The Variable Manager screen (spec §3.4): a master-detail layout —
//! a top bar (environment switcher + the two "new" buttons), a fixed-width
//! left list of every declared variable and group, and a detail pane for
//! whatever the list has selected.
//!
//! This task builds the skeleton: the top bar, the left list and its
//! selection model, and the structural ops the list's own commands
//! dispatch. The detail pane's two faces — the variable form (Task 15) and
//! the group entries table (Task 16) — are placeholders here
//! ([`VarFormState`] / [`EntryGridState`]), and the pane paints an
//! instruction line until one lands.

use crate::action::Action;
use crate::hit::{Hit, HitMap, ScrollbarSpec};
use crate::layout::PaneId;
use crate::paint::{
    BUTTON_HEIGHT, Button, ButtonKind, ControlState, PillRow, RowHighlight, button_min_width, fill,
    text,
};
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::HttpRequest;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// A committed value edit, dispatched as `Action::VarEdit` and applied by
/// `App` (spec §5: every commit writes atomically and immediately to
/// whichever file owns it; a write failure toasts and leaves the field it
/// came from untouched rather than losing the typed text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarEditOp {
    /// A simple (non-secret) variable's flat value in
    /// `env` — `varedit::set_env_value` via `ctx.edit_env`.
    SetEnvValue {
        env: String,
        name: String,
        value: String,
    },
    /// A variable's shared `default` in `variables.toml` —
    /// `varedit::upsert_var` via `ctx.edit_variables`.
    SetDefault { name: String, value: String },
    /// A variable's or group's shared `description` in `variables.toml`.
    SetDescription { owner: String, value: String },
    /// A secret variable's value in `env` — `ctx.set_secret`, which writes
    /// `.local/secrets.toml` (never `variables.toml`/the env file).
    SetSecretValue {
        env: String,
        name: String,
        value: String,
    },
    /// One field of one entry: the edit lands in `env`'s
    /// `[entries.owner.key]` table (`varedit::upsert_entry`), the only
    /// place an entry's values live (spec §3.1). `member` names the field;
    /// `None` has nothing to write and is refused.
    SetOptionValue {
        env: String,
        owner: String,
        key: String,
        member: Option<String>,
        value: String,
    },
    /// A request-scoped `[variables]` entry's value on the open request —
    /// mutates `Editor::variables` directly and rides the editor's existing
    /// dirty/save path (no immediate write of its own).
    SetRequestVar { name: String, value: String },
    /// Records `name`'s selection as `key` for `env` — `ctx.set_selection`
    /// (the ✓ action; also the var picker's confirm).
    Select {
        env: String,
        name: String,
        key: String,
    },
}

/// A structural mutation dispatched by the Variable Manager: unlike
/// [`VarEditOp`] (one value), these add/remove/rename/reshape declarations
/// and entries. Each applies through `ctx.edit_variables`/`edit_env` in
/// `App::apply_var_struct`.
///
/// The declaration ops (`NewVar`..`Demote`) write `variables.toml`; the
/// entry ops (`NewEntry`..`DuplicateEntry`) write one environment file
/// each — entries belong to exactly one environment (spec §3.1), so every
/// one of them names the `env` it targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarStructOp {
    /// A bare new variable declaration (`+ Variable` / `n`).
    NewVar {
        name: String,
        description: Option<String>,
    },
    /// A new group and its field list (`+ Group` / `g`). Create-or-update:
    /// `varedit::upsert_group` is the same verb either way.
    NewGroup { name: String, fields: Vec<String> },
    /// Rename a variable (groups have no core rename verb yet).
    Rename { from: String, to: String },
    /// Delete a variable or a group. A group cascades: every environment's
    /// `[entries.<name>]` subtree goes with the declaration, and every
    /// environment's selection for it is cleared.
    Delete { name: String },
    /// Flip a variable's `secret` flag (spec §3's two transitions).
    ToggleSecret { name: String },
    /// Replace a group's field list.
    SetFields { group: String, fields: Vec<String> },
    /// Promote a request-scoped variable to the project (spec §4).
    Promote {
        name: String,
        target: postui_core::varedit::PromoteTarget,
    },
    /// Demote a project variable into the open request (spec §4).
    Demote { name: String },
    /// A new entry of `group` in `env` (`varedit::upsert_entry`).
    NewEntry {
        env: String,
        group: String,
        name: String,
        description: Option<String>,
        values: IndexMap<String, String>,
    },
    /// Rename one entry of `group` within `env`.
    RenameEntry {
        env: String,
        group: String,
        from: String,
        to: String,
    },
    /// Delete one entry of `group` from `env`, clearing any selection that
    /// named it.
    DeleteEntry {
        env: String,
        group: String,
        name: String,
    },
    /// Copy one entry of `group` in `env` to a fresh name — `"<name> copy"`,
    /// then `"<name> copy-2"`, … on collision.
    DuplicateEntry {
        env: String,
        group: String,
        name: String,
    },
}

/// What the detail pane is showing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum VmDetail {
    #[default]
    None,
    Var(String),
    Group(String),
}

/// One row of the left list, rebuilt from `&ProjectContext` at the top of
/// every `draw` (and after every structural write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmRow {
    SectionVars,
    Var(String),
    SectionGroups,
    Group(String),
}

impl VmRow {
    /// Whether the keyboard cursor stops here — section headers are labels,
    /// not selections.
    fn is_stop(&self) -> bool {
        matches!(self, VmRow::Var(_) | VmRow::Group(_))
    }

    /// The declared name this row addresses, if any.
    pub fn name(&self) -> Option<&str> {
        match self {
            VmRow::Var(n) | VmRow::Group(n) => Some(n),
            _ => None,
        }
    }
}

/// The detail pane's variable form (Task 15). A placeholder this task: only
/// `editing` — whether one of its inputs currently owns the keyboard — is
/// consulted, by the command-key gate in [`VarManager::handle_key`].
#[derive(Debug, Default)]
pub struct VarFormState {
    /// True while a form field has an in-progress edit.
    pub editing: bool,
}

/// The detail pane's entries table (Task 16). Same placeholder shape as
/// [`VarFormState`].
#[derive(Debug, Default)]
pub struct EntryGridState {
    /// True while a grid cell has an in-progress edit.
    pub editing: bool,
}

/// The full-frame Variable Manager screen.
#[derive(Debug, Default)]
pub struct VarManager {
    /// What the detail pane shows — set by [`VarManager::select_row`],
    /// whichever gesture (click or arrow key) got there.
    pub detail: VmDetail,
    /// Rebuilt from `&ProjectContext` at the top of every `draw`.
    pub left_rows: Vec<VmRow>,
    /// Index into `left_rows`.
    pub left_cursor: usize,
    /// First visible row of `left_rows`.
    pub left_scroll: usize,
    pub form: VarFormState,
    pub grid: EntryGridState,
    /// Set by a keyboard move so the next `draw` snaps `left_scroll` to
    /// keep `left_cursor` visible; a wheel/drag gesture clears it (those
    /// place the viewport explicitly). Mirrors `Sidebar::ensure_visible`.
    ensure_visible: bool,
    /// How many rows the left list showed as of the last `draw` — the
    /// scrollbar's viewport, and the snap math's page size.
    visible_rows: usize,
}

/// The top bar's height, matching the header/footer's 3-row painted rhythm
/// (and exactly one painted button tall).
pub const TITLE_HEIGHT: u16 = BUTTON_HEIGHT;
/// The left list's fixed width (spec §3.4's mock).
pub const LEFT_W: u16 = 28;

const GLYPH_GROUP: &str = "\u{25b6}"; // ▶
const GLYPH_LOCK: &str = "\u{1f512}"; // 🔒
const GLYPH_UNRESOLVED: &str = "\u{25cf}"; // ●
const GLYPH_CARET: &str = "\u{25be}"; // ▾

/// The left list's rows: the VARIABLES section (every declared variable
/// that isn't a group field — fields live inside their group's entries),
/// then the GROUPS section.
pub fn build_left_rows(ctx: &ProjectContext) -> Vec<VmRow> {
    let mut rows = vec![VmRow::SectionVars];

    let group_fields: std::collections::HashSet<&str> = ctx
        .model
        .groups
        .values()
        .flat_map(|g| g.fields.iter().map(String::as_str))
        .collect();
    for name in ctx.model.vars.keys() {
        if !group_fields.contains(name.as_str()) {
            rows.push(VmRow::Var(name.clone()));
        }
    }

    rows.push(VmRow::SectionGroups);
    for name in ctx.model.groups.keys() {
        rows.push(VmRow::Group(name.clone()));
    }
    rows
}

/// A group's current selection in the active environment, as the left list
/// shows it inline: the selected entry's name, or `None` when the group has
/// no (or a stale) selection here.
fn active_selection(ctx: &ProjectContext, group: &str) -> Option<String> {
    let env = ctx.active_env.as_deref()?;
    let key = ctx.selections_for(env).get(group)?;
    let entries = postui_core::varmodel::group_entries(&ctx.env_data, group)?;
    entries.contains_key(key).then(|| key.clone())
}

/// Whether `name` currently resolves to a value in the active environment —
/// false for a group field awaiting a selection, a secret with no value,
/// and a variable with neither default nor env value. Drives the left
/// list's red dot.
fn is_unresolved(ctx: &ProjectContext, name: &str) -> bool {
    !ctx.resolved.values.contains_key(name)
}

impl VarManager {
    /// Points the detail pane at `left_rows[i]` and moves the cursor there.
    /// A section header is a label, not a selection: the cursor still moves
    /// (a click landed there) but the detail pane keeps what it had.
    pub fn select_row(&mut self, i: usize) {
        if i >= self.left_rows.len() {
            return;
        }
        self.left_cursor = i;
        match &self.left_rows[i] {
            VmRow::Var(name) => self.detail = VmDetail::Var(name.clone()),
            VmRow::Group(name) => self.detail = VmDetail::Group(name.clone()),
            VmRow::SectionVars | VmRow::SectionGroups => {}
        }
    }

    /// The row the commands act on — the left list's current selection.
    fn selected_row(&self) -> Option<&VmRow> {
        self.left_rows.get(self.left_cursor).filter(|r| r.is_stop())
    }

    /// Rebuilds `left_rows` from `ctx` and repairs the selection after a
    /// structural write: the cursor clamps into range, and a detail pane
    /// pointing at a name that no longer exists empties.
    pub fn sync(&mut self, ctx: &ProjectContext) {
        self.left_rows = build_left_rows(ctx);
        if self.left_cursor >= self.left_rows.len() {
            self.left_cursor = self.left_rows.len().saturating_sub(1);
        }
        let gone = match &self.detail {
            VmDetail::None => false,
            VmDetail::Var(name) => !ctx.model.vars.contains_key(name),
            VmDetail::Group(name) => !ctx.model.groups.contains_key(name),
        };
        if gone {
            self.detail = VmDetail::None;
        }
        self.ensure_visible = true;
    }

    /// Moves the cursor one selectable row in `dir` (`-1`/`1`), skipping
    /// section headers, and opens whatever it lands on.
    fn move_cursor(&mut self, dir: i32) {
        let mut i = self.left_cursor as i32;
        loop {
            i += dir;
            if i < 0 || i as usize >= self.left_rows.len() {
                return;
            }
            if self.left_rows[i as usize].is_stop() {
                self.select_row(i as usize);
                self.ensure_visible = true;
                return;
            }
        }
    }

    /// Handles a key while the Manager screen is open. `App::handle_key`
    /// routes every key here once an open modal and a modified global
    /// shortcut (e.g. ctrl+p for the palette) have had first refusal, and
    /// swallows anything this returns `None` for rather than falling
    /// through to the global keymap — so, for instance, plain `q` does not
    /// quit the app from this screen.
    ///
    /// The single-letter commands all target the left list's selection.
    /// They are ignored while a detail-pane cell edit owns the keyboard
    /// (spec §11: a command key that collides with typing yields to the
    /// text input) — `esc` and the arrows still work.
    pub fn handle_key(&mut self, ev: KeyEvent, ctx: &ProjectContext) -> Option<Action> {
        match ev.code {
            KeyCode::Esc => return Some(Action::CloseScreen),
            KeyCode::Up => {
                self.move_cursor(-1);
                return None;
            }
            KeyCode::Down => {
                self.move_cursor(1);
                return None;
            }
            _ => {}
        }
        if self.form.editing || self.grid.editing {
            return None;
        }
        match ev.code {
            KeyCode::Char('n') => Some(Action::PromptNewVar),
            KeyCode::Char('g') => Some(Action::PromptNewGroup),
            KeyCode::Char('e') | KeyCode::F(2) => self.rename_action(),
            KeyCode::Char('d') | KeyCode::Delete => Some(Action::ConfirmDeleteVar {
                name: self.selected_row()?.name()?.to_string(),
            }),
            KeyCode::Char('s') => match self.selected_row()? {
                VmRow::Var(name) if ctx.model.vars.contains_key(name) => {
                    Some(Action::ToggleSecretVar { name: name.clone() })
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// `e`/`F2` and the context menu's "Rename…": variables only — a group
    /// rename has no core verb behind it (`varedit` renames variables and
    /// entries, not group declarations), so the menu shows it disabled
    /// rather than offering a no-op.
    fn rename_action(&self) -> Option<Action> {
        match self.selected_row()? {
            VmRow::Var(name) => Some(Action::PromptRenameVar { from: name.clone() }),
            _ => None,
        }
    }

    /// The right-click menu for left-list row `i` (spec §3.4: Rename /
    /// Duplicate / Delete). `None` for a section header.
    pub fn context_menu(&self, i: usize) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::components::modal::MenuItem;
        let row = self.left_rows.get(i).filter(|r| r.is_stop())?;
        let name = row.name()?.to_string();
        let (rename, duplicate) = match row {
            VmRow::Var(_) => (
                MenuItem::new(
                    "Rename\u{2026}",
                    Action::PromptRenameVar { from: name.clone() },
                ),
                MenuItem::new("Duplicate", Action::DuplicateVar { name: name.clone() }),
            ),
            // Both are shown disabled rather than hidden, so the menu keeps
            // its shape. Rename: no core verb renames a group declaration
            // (`varedit` renames variables and entries). Duplicate: a field
            // belongs to exactly one group (`ModelError::FieldInTwoGroups`),
            // so a copy carrying the same field list can never be a valid
            // model — there is nothing to duplicate a group *into* until
            // fields can be renamed as part of the copy.
            _ => (
                MenuItem::disabled("Rename\u{2026}"),
                MenuItem::disabled("Duplicate"),
            ),
        };
        Some(vec![
            rename,
            duplicate,
            MenuItem::new("Delete\u{2026}", Action::ConfirmDeleteVar { name }),
        ])
    }

    /// Free (unsnapped) wheel scroll over the left list — mirrors
    /// `Sidebar::handle_scroll`: moves the viewport without touching the
    /// cursor, and cancels any pending snap so the gesture isn't overridden
    /// on the next draw.
    pub fn handle_scroll(&mut self, delta: i16) {
        self.set_scroll((self.left_scroll as i32 + delta as i32).max(0) as usize);
    }

    /// Places the left list's viewport (the scrollbar drag's entry point).
    pub fn set_scroll(&mut self, offset: usize) {
        let max = self.left_rows.len().saturating_sub(1);
        self.left_scroll = offset.min(max);
        self.ensure_visible = false;
    }

    /// The left list's scroll state, as of the last draw. `None` before the
    /// first frame (the viewport height is a render-time fact). Counted in
    /// logical rows, not lines: the 2-line pitch scales every row the same.
    ///
    /// It borrows `PaneId::Sidebar`, the pane whose column the list stands
    /// in: no sidebar is drawn while this screen is up, so the app's
    /// existing wheel/drag routing for that pane is free, and `App`
    /// redirects it here for the duration (see `App::scrollbar_spec`).
    pub fn scrollbar_spec(&self) -> Option<ScrollbarSpec> {
        if self.visible_rows == 0 {
            return None;
        }
        Some(ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset: self.left_scroll,
            content: self.left_rows.len(),
            viewport: self.visible_rows,
        })
    }

    /// Number of 2-line rows that fit in a list `height` lines tall: a
    /// trailing odd line still fits one more row's text line (its bottom pad
    /// is clipped), so this rounds up. Mirrors `Sidebar::visible_rows`.
    fn rows_that_fit(height: u16) -> usize {
        (height as usize).div_ceil(2)
    }

    /// Paints the screen: the top bar (environment switcher + `+ Variable` /
    /// `+ Group`), the fixed-width left list, and the detail pane.
    ///
    /// `_open_request` is the request-scope half of the detail pane (the
    /// promote/demote buttons of Task 15's variable form); unused while the
    /// pane is a placeholder, but kept on the signature so the call site in
    /// `ui::draw` doesn't churn twice.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        _open_request: Option<&HttpRequest>,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        self.left_rows = build_left_rows(ctx);
        if self.left_cursor >= self.left_rows.len() {
            self.left_cursor = self.left_rows.len().saturating_sub(1);
        }
        if area.width == 0 || area.height == 0 {
            self.visible_rows = 0;
            return;
        }

        let bar = Rect {
            height: TITLE_HEIGHT.min(area.height),
            ..area
        };
        let body = Rect {
            y: area.y + bar.height,
            height: area.height - bar.height,
            ..area
        };
        self.draw_top_bar(frame, bar, theme, ctx, hits, hovered);

        let left = Rect {
            width: LEFT_W.min(body.width),
            ..body
        };
        let right = Rect {
            x: body.x + left.width,
            width: body.width - left.width,
            ..body
        };
        self.draw_left(frame, left, theme, ctx, hits, hovered);
        draw_detail_placeholder(frame, right, theme);
    }

    fn draw_top_bar(
        &self,
        frame: &mut Frame,
        bar: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        let buf = frame.buffer_mut();
        fill(buf, bar, theme.panel);
        if bar.height < BUTTON_HEIGHT {
            return;
        }

        let state_of = |hit: &Hit| {
            if hovered == Some(hit) {
                ControlState::Hover
            } else {
                ControlState::Normal
            }
        };

        let env_label = format!("Environment: {} {GLYPH_CARET}", ctx.env_label());
        let env_w = button_min_width(&env_label).min(bar.width);
        let env_area = Rect {
            x: bar.x + 1,
            y: bar.y,
            width: env_w,
            height: BUTTON_HEIGHT,
        };
        Button {
            label: &env_label,
            kind: ButtonKind::Secondary,
            state: state_of(&Hit::VmEnvSwitch),
        }
        .paint(buf, env_area, theme.panel, theme);
        hits.register(env_area, Hit::VmEnvSwitch);

        // Right-aligned group, laid out from the bar's right edge inward so
        // the buttons stay put as the environment name changes width. The
        // close button rides along as the mouse's way back to the main
        // screen (the header's vars chip toggles it too), labelled with the
        // key that does the same thing.
        let mut x = bar.x + bar.width;
        for (label, kind, hit) in [
            (
                "Close (esc)",
                ButtonKind::Secondary,
                Hit::FooterChip(Action::CloseScreen),
            ),
            ("+ Group", ButtonKind::Secondary, Hit::VmNewGroup),
            ("+ Variable", ButtonKind::Primary, Hit::VmNewVar),
        ] {
            let w = button_min_width(label);
            if x < env_area.x + env_area.width + w + 2 {
                break;
            }
            x -= w + 1;
            let rect = Rect {
                x,
                y: bar.y,
                width: w,
                height: BUTTON_HEIGHT,
            };
            let state = state_of(&hit);
            Button { label, kind, state }.paint(buf, rect, theme.panel, theme);
            hits.register(rect, hit);
        }
    }

    fn draw_left(
        &mut self,
        frame: &mut Frame,
        left: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        fill(frame.buffer_mut(), left, theme.panel);
        if left.width <= 2 || left.height == 0 {
            self.visible_rows = 0;
            return;
        }

        // Rows keep a 1-column inset each side: the left column is the
        // selected pill's accent lane, the right one hosts the scrollbar.
        let list = Rect {
            x: left.x + 1,
            y: left.y,
            width: left.width - 2,
            height: left.height,
        };
        self.visible_rows = Self::rows_that_fit(list.height);
        if self.ensure_visible {
            if self.visible_rows > 0 {
                if self.left_cursor < self.left_scroll {
                    self.left_scroll = self.left_cursor;
                } else if self.left_cursor >= self.left_scroll + self.visible_rows {
                    self.left_scroll = self.left_cursor + 1 - self.visible_rows;
                }
                let max_scroll = self.left_rows.len().saturating_sub(self.visible_rows);
                self.left_scroll = self.left_scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        if let Some(spec) = self.scrollbar_spec().filter(ScrollbarSpec::overflows) {
            let column = Rect {
                x: left.x + left.width - 1,
                width: 1,
                ..list
            };
            crate::hit::draw_scrollbar(frame, hits, column, &spec, hovered, false, theme);
        }

        let buf = frame.buffer_mut();
        for (pos, (i, row)) in self
            .left_rows
            .iter()
            .enumerate()
            .skip(self.left_scroll)
            .take(self.visible_rows.max(1))
            .enumerate()
        {
            let y = list.y + (pos as u16) * 2;
            if y >= left.y + left.height {
                break;
            }
            let selected = match (&self.detail, row) {
                (VmDetail::Var(a), VmRow::Var(b)) | (VmDetail::Group(a), VmRow::Group(b)) => a == b,
                _ => false,
            };
            let highlight = if selected {
                RowHighlight::Selected
            } else if hovered == Some(&Hit::VmLeftRow(i)) || i == self.left_cursor {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            let row_bg = match highlight {
                RowHighlight::None => theme.panel,
                RowHighlight::Hover => theme.control,
                RowHighlight::Selected => theme.control_hover,
            };
            if row.is_stop() {
                PillRow { highlight }.paint(buf, y, list.x, list.width, left, theme.panel, theme);
            }
            paint_left_row(buf, ctx, row, y, list, row_bg, theme);

            let hit_top = y.saturating_sub(1).max(left.y);
            let hit_bottom = (y + 2).min(left.y + left.height);
            hits.register(
                Rect {
                    x: list.x,
                    y: hit_top,
                    width: list.width,
                    height: hit_bottom.saturating_sub(hit_top),
                },
                Hit::VmLeftRow(i),
            );
        }
    }
}

/// One left-list row's content: the section labels, a variable (name, lock
/// badge, unresolved dot) or a group (`▶ name (entry)`).
fn paint_left_row(
    buf: &mut ratatui::buffer::Buffer,
    ctx: &ProjectContext,
    row: &VmRow,
    y: u16,
    list: Rect,
    bg: ratatui::style::Color,
    theme: &Theme,
) {
    let x = list.x + 1;
    // One column of inset on the left (the selected pill's accent bar) —
    // the label may run to the list's right edge.
    let width = list.width.saturating_sub(1);
    match row {
        VmRow::SectionVars | VmRow::SectionGroups => {
            let label = if matches!(row, VmRow::SectionVars) {
                "VARIABLES"
            } else {
                "GROUPS"
            };
            text(buf, list.x, y, label, theme.text_muted, bg, true);
        }
        VmRow::Var(name) => {
            let secret = ctx.model.vars.get(name).is_some_and(|d| d.secret);
            // The badges are right-aligned in their own columns, so names
            // stay left-aligned however long they are.
            let mut label_w = width;
            if secret {
                label_w = label_w.saturating_sub(3);
            }
            if is_unresolved(ctx, name) {
                label_w = label_w.saturating_sub(2);
            }
            text(
                buf,
                x,
                y,
                super::chooser::clip(name, label_w),
                theme.text,
                bg,
                false,
            );
            let mut badge_x = list.x + list.width;
            if is_unresolved(ctx, name) {
                badge_x = badge_x.saturating_sub(2);
                text(buf, badge_x, y, GLYPH_UNRESOLVED, theme.error, bg, false);
            }
            if secret {
                badge_x = badge_x.saturating_sub(3);
                text(buf, badge_x, y, GLYPH_LOCK, theme.text_muted, bg, false);
            }
        }
        VmRow::Group(name) => {
            let selection = match active_selection(ctx, name) {
                Some(entry) => format!("({entry})"),
                None => "(needs selection)".to_string(),
            };
            let label = format!("{GLYPH_GROUP} {name} {selection}");
            text(
                buf,
                x,
                y,
                super::chooser::clip(&label, width),
                theme.text,
                bg,
                false,
            );
        }
    }
}

/// The detail pane until Tasks 15/16 fill it: one muted instruction line.
fn draw_detail_placeholder(frame: &mut Frame, right: Rect, theme: &Theme) {
    let buf = frame.buffer_mut();
    fill(buf, right, theme.page);
    if right.width == 0 || right.height < 2 {
        return;
    }
    text(
        buf,
        right.x + 2,
        right.y + 1,
        super::chooser::clip("select a variable or group", right.width.saturating_sub(3)),
        theme.text_muted,
        theme.page,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::project;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    /// A project with two envs (dev, qa; qa active); `base_url` simple with
    /// a default; `api_key` secret with no value anywhere (so it reads as
    /// unresolved); group `creds` with two fields and two entries in qa,
    /// `alice` selected there and nothing selected in dev.
    fn fixture() -> (tempfile::TempDir, ProjectContext) {
        let dir = tempfile::tempdir().unwrap();
        project::init_project(dir.path(), Some("demo")).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service key"
secret = true

[groups.creds]
description = "paired ids"
fields = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[entries.creds.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-77\"\n\n[entries.creds.bob]\nuser_id = \"2002\"\ncustomer_id = \"c-91\"\n",
        )
        .unwrap();

        let mut selections = IndexMap::new();
        let mut qa_sel = IndexMap::new();
        qa_sel.insert("creds".to_string(), "alice".to_string());
        selections.insert("qa".to_string(), qa_sel);
        project::save_local_state(
            dir.path(),
            &project::LocalState {
                environment: Some("qa".into()),
                selections,
                ..Default::default()
            },
        )
        .unwrap();

        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(warns.is_empty(), "{warns:?}");
        (dir, ctx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render(vm: &mut VarManager, ctx: &ProjectContext) -> (String, HitMap) {
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, ctx, None, &mut hits, None))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    #[test]
    fn left_list_is_variables_then_groups_with_the_selection_inline() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (content, _) = render(&mut vm, &ctx);

        assert_eq!(
            vm.left_rows,
            vec![
                VmRow::SectionVars,
                VmRow::Var("base_url".into()),
                VmRow::Var("api_key".into()),
                VmRow::SectionGroups,
                VmRow::Group("creds".into()),
            ],
            "group fields (user_id/customer_id) are not top-level rows"
        );
        assert!(content.contains("VARIABLES"), "{content}");
        assert!(content.contains("GROUPS"), "{content}");
        assert!(content.contains("creds (alice)"), "{content}");
        assert!(content.contains(GLYPH_LOCK), "secret badge: {content}");
        assert!(
            content.contains(GLYPH_UNRESOLVED),
            "unresolved dot for the value-less secret: {content}"
        );
    }

    #[test]
    fn a_group_with_no_selection_says_so() {
        let (_dir, mut ctx) = fixture();
        ctx.clear_selection_for("qa", "creds");
        let mut vm = VarManager::default();
        let (content, _) = render(&mut vm, &ctx);
        assert!(content.contains("creds (needs selection)"), "{content}");
    }

    #[test]
    fn clicking_a_group_row_opens_it_in_the_detail_pane() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        let group_row = vm
            .left_rows
            .iter()
            .position(|r| r == &VmRow::Group("creds".into()))
            .unwrap();

        vm.select_row(group_row);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));
        assert_eq!(vm.left_cursor, group_row);

        // …and a variable row, likewise.
        vm.select_row(1);
        assert_eq!(vm.detail, VmDetail::Var("base_url".into()));
    }

    #[test]
    fn clicking_a_section_header_moves_the_cursor_but_keeps_the_detail() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        vm.select_row(1);
        vm.select_row(0);
        assert_eq!(vm.left_cursor, 0);
        assert_eq!(vm.detail, VmDetail::Var("base_url".into()));
    }

    #[test]
    fn every_row_registers_a_left_row_hit() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (_, hits) = render(&mut vm, &ctx);
        for i in 0..vm.left_rows.len() {
            assert!(
                hits.rect_of(&Hit::VmLeftRow(i)).is_some(),
                "row {i} has no hit"
            );
        }
    }

    #[test]
    fn arrows_skip_section_headers_and_open_what_they_land_on() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);

        assert!(vm.handle_key(key(KeyCode::Down), &ctx).is_none());
        assert_eq!(vm.detail, VmDetail::Var("base_url".into()));
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Var("api_key".into()));
        // Skips the GROUPS header.
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));
        // …and stops at the end.
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));

        vm.handle_key(key(KeyCode::Up), &ctx);
        assert_eq!(vm.detail, VmDetail::Var("api_key".into()));
    }

    #[test]
    fn top_bar_registers_the_env_switch_and_both_new_buttons() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (content, hits) = render(&mut vm, &ctx);
        assert!(content.contains("Environment: qa"), "{content}");
        assert!(hits.rect_of(&Hit::VmEnvSwitch).is_some());
        assert!(hits.rect_of(&Hit::VmNewVar).is_some());
        assert!(hits.rect_of(&Hit::VmNewGroup).is_some());
        assert!(content.contains("+ Variable"), "{content}");
        assert!(content.contains("+ Group"), "{content}");
        assert!(content.contains("Close (esc)"), "{content}");
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::CloseScreen))
                .is_some(),
            "the close button is the mouse's way back"
        );
    }

    #[test]
    fn the_detail_pane_asks_for_a_selection_until_a_row_is_open() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (content, _) = render(&mut vm, &ctx);
        assert!(content.contains("select a variable or group"), "{content}");
    }

    #[test]
    fn commands_target_the_left_selection() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);

        assert_eq!(
            vm.handle_key(key(KeyCode::Char('n')), &ctx),
            Some(Action::PromptNewVar)
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('g')), &ctx),
            Some(Action::PromptNewGroup)
        );

        vm.select_row(1); // base_url
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('e')), &ctx),
            Some(Action::PromptRenameVar {
                from: "base_url".into()
            })
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::F(2)), &ctx),
            Some(Action::PromptRenameVar {
                from: "base_url".into()
            })
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('d')), &ctx),
            Some(Action::ConfirmDeleteVar {
                name: "base_url".into()
            })
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('s')), &ctx),
            Some(Action::ToggleSecretVar {
                name: "base_url".into()
            })
        );

        let group_row = vm.left_rows.len() - 1;
        vm.select_row(group_row);
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('d')), &ctx),
            Some(Action::ConfirmDeleteVar {
                name: "creds".into()
            }),
            "delete works on a group row"
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('e')), &ctx),
            None,
            "no core verb renames a group"
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('s')), &ctx),
            None,
            "a group has no secret flag"
        );
    }

    #[test]
    fn commands_yield_to_an_active_cell_edit_but_navigation_does_not() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        vm.select_row(1);
        vm.grid.editing = true;

        assert_eq!(vm.handle_key(key(KeyCode::Char('n')), &ctx), None);
        assert_eq!(vm.handle_key(key(KeyCode::Char('d')), &ctx), None);
        assert_eq!(
            vm.handle_key(key(KeyCode::Esc), &ctx),
            Some(Action::CloseScreen)
        );
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Var("api_key".into()));
    }

    #[test]
    fn context_menu_offers_rename_duplicate_delete() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);

        let items = vm.context_menu(1).expect("variable menu");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Rename\u{2026}", "Duplicate", "Delete\u{2026}"]
        );
        assert_eq!(
            items[0].action,
            Some(Action::PromptRenameVar {
                from: "base_url".into()
            })
        );
        assert_eq!(
            items[1].action,
            Some(Action::DuplicateVar {
                name: "base_url".into()
            })
        );
        assert_eq!(
            items[2].action,
            Some(Action::ConfirmDeleteVar {
                name: "base_url".into()
            })
        );

        let group = vm.context_menu(vm.left_rows.len() - 1).expect("group menu");
        assert_eq!(group[0].action, None, "group rename is shown disabled");
        assert_eq!(
            group[1].action, None,
            "so is duplicate: a field can only belong to one group"
        );
        assert_eq!(
            group[2].action,
            Some(Action::ConfirmDeleteVar {
                name: "creds".into()
            })
        );

        assert!(vm.context_menu(0).is_none(), "no menu on a section header");
    }

    #[test]
    fn sync_clamps_the_cursor_and_drops_a_deleted_detail() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        vm.left_cursor = 99;
        vm.detail = VmDetail::Var("gone".into());
        vm.sync(&ctx);
        assert_eq!(vm.left_cursor, vm.left_rows.len() - 1);
        assert_eq!(vm.detail, VmDetail::None);

        vm.detail = VmDetail::Group("creds".into());
        vm.sync(&ctx);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));
    }

    #[test]
    fn a_long_list_scrolls_and_draws_a_scrollbar() {
        let (dir, _ctx) = fixture();
        let mut decls = String::new();
        for i in 0..40 {
            decls.push_str(&format!("[v{i:02}]\ndefault = \"x\"\n\n"));
        }
        std::fs::write(dir.path().join("variables.toml"), decls).unwrap();
        let (ctx, _) = ProjectContext::open(dir.path().to_path_buf());

        let mut vm = VarManager::default();
        let (content, hits) = render(&mut vm, &ctx);
        assert!(content.contains('\u{2588}'), "thumb glyph drawn: {content}");
        assert!(
            hits.rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar))
                .is_some()
        );

        vm.handle_scroll(5);
        assert_eq!(vm.left_scroll, 5);
        let (content, _) = render(&mut vm, &ctx);
        assert!(!content.contains("v00"), "scrolled past the first row");

        // The keyboard snaps the viewport back to its cursor.
        vm.select_row(1);
        vm.handle_key(key(KeyCode::Down), &ctx);
        let (content, _) = render(&mut vm, &ctx);
        assert!(content.contains("v01"), "cursor row is visible: {content}");
    }
}
