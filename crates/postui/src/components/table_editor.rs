use super::DrawCtx;
use super::line_input::LineInput;
use super::var_tokens::{VarView, paint_var_tokens};
use crate::hit::{Hit, HitMap};
use crate::paint::{fill, text};
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::Entry;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Which cell of a row is under edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Key,
    Value,
}

impl Col {
    /// The `Hit::TableCell` column index this cell registers under.
    pub fn index(self) -> u8 {
        match self {
            Col::Key => 0,
            Col::Value => 1,
        }
    }

    /// The column a `Hit::TableCell` index names; anything else is the
    /// value cell (only 0 and 1 are ever registered).
    pub fn from_index(i: u8) -> Self {
        if i == 0 { Col::Key } else { Col::Value }
    }
}

/// The cell currently being typed into. Editing is always in place: the
/// clicked (or Enter'd) cell turns into a `LineInput` right where it sits.
///
/// The map is never mutated before the edit commits, so `original` — the
/// cell's text when the edit began — is still what the map holds; `Esc`
/// simply drops the edit (and, defensively, writes `original` back).
#[derive(Debug, Clone)]
pub struct CellEdit {
    /// Index into the map — or `map.len()`, the always-present ghost row
    /// that becomes a real entry the moment its key cell commits non-empty.
    pub row: usize,
    pub col: Col,
    pub input: LineInput,
    /// The cell's pre-edit text, for `Esc`-revert.
    pub original: String,
}

/// Result of a `TableEditorState` interaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableOutcome {
    pub consumed: bool,
    pub warning: Option<String>,
    /// `Some(i)` when the user asked to delete row `i` (`d`/`Delete`): the
    /// table never removes the row itself — the caller routes this through
    /// a confirmation modal first.
    pub request_delete: Option<usize>,
}

impl TableOutcome {
    fn consumed() -> Self {
        Self {
            consumed: true,
            ..Self::default()
        }
    }

    fn not_consumed() -> Self {
        Self::default()
    }

    fn maybe_warn(warning: Option<String>) -> Self {
        Self {
            consumed: true,
            warning,
            ..Self::default()
        }
    }
}

/// The toggle's zone at a row's left edge: the glyph plus a column of air
/// either side, which is also the row's left padding.
pub(crate) const TOGGLE_W: u16 = 3;

/// One right-edge action button's zone (copy, delete): a glyph centred in
/// three cells.
const ACTION_W: u16 = 3;

/// The columns a row's right-edge actions claim: copy, delete, and a
/// column of margin outside them. A cell edit's input stops short of this
/// so a long value never runs under the buttons.
const ACTIONS_W: u16 = ACTION_W * 2 + 1;

/// Column x-offsets, relative to the drawn area's own left edge. Every
/// row — plain, hovered or active — lays out from the same offsets, so
/// activating a row never jogs its text sideways.
///
/// The first [`TOGGLE_W`] columns are the enable/disable toggle's, which
/// every row carries whether the pointer is on it or not: it reports
/// state, and state that only appears under the pointer is state you have
/// to go looking for. The row's *actions* — copy, delete — stay at the
/// right edge, revealed on hover.
pub(crate) struct Columns {
    pub(crate) name_x: u16,
    pub(crate) divider_x: u16,
    pub(crate) value_x: u16,
}

pub(crate) fn columns(x0: u16, width: u16) -> Columns {
    let pad = TOGGLE_W.min(width);
    let remaining = width.saturating_sub(pad);
    let name_w = (remaining / 3)
        .max(4)
        .min(remaining.saturating_sub(2).max(4));
    Columns {
        name_x: x0 + pad,
        divider_x: x0 + pad + name_w,
        value_x: x0 + pad + name_w + 1,
    }
}

/// `1 (header) + rows + 1 (ghost row) + 1 (closing edge)`. `rows` is
/// `map.len()`; the ghost row is the constant `+ 1`.
///
/// Every row is one line, whatever the cursor is doing — rows edit in
/// place — so the table's height depends on nothing but how many rows it
/// has. Selecting or editing a row can never reflow the pane under it.
pub fn table_height(rows: usize) -> u16 {
    1 + rows as u16 + 1 + 1
}

/// The row a hit belongs to, for hover styling: every hit a table row
/// registers (its background, checkbox, cells and delete affordance) lights
/// that one row.
fn hovered_row(ctx: &DrawCtx) -> Option<usize> {
    match ctx.hovered? {
        Hit::TableRow(i) | Hit::TableCheckbox(i) | Hit::TableCopy(i) | Hit::TableDelete(i) => {
            Some(*i)
        }
        Hit::TableCell { row, .. } => Some(*row),
        _ => None,
    }
}

/// Shared cursor/edit state for a key/value table (Params, Headers or Vars).
/// One instance is reused across the tabs; the caller passes in whichever
/// `IndexMap` is currently active.
#[derive(Debug, Default)]
pub struct TableEditorState {
    /// The selected row (always the one drawn expanded, unless it's the
    /// ghost row). `None` means no row is selected — every row draws
    /// compact and the row-level keys (Enter/Space/d) are inert until
    /// Down/j or a click lands somewhere.
    pub selected: Option<usize>,
    pub editing: Option<CellEdit>,
    /// Value text typed into the ghost row before it has a key. A ghost
    /// VALUE commit can't create a row (only a key can), so the text is
    /// stashed here and attached when the ghost's key commits — instead of
    /// being silently dropped. Cleared whenever the edit leaves the ghost
    /// row without creating it.
    pending_ghost_value: Option<String>,
}

impl TableEditorState {
    /// Resets cursor/edit state; used when switching tabs so a selection
    /// index from one map can't be stale (and panic) against the other.
    pub fn reset(&mut self) {
        self.selected = None;
        self.editing = None;
        self.pending_ghost_value = None;
    }

    /// The text `row`/`col` currently shows. Empty for the ghost row.
    fn cell_text(map: &IndexMap<String, Entry>, row: usize, col: Col) -> String {
        match map.get_index(row) {
            Some((k, e)) => match col {
                Col::Key => k.clone(),
                Col::Value => e.value.clone(),
            },
            None => String::new(),
        }
    }

    /// Puts `row`/`col` under edit, seeded with its current text and the
    /// caret at the end. Any previous edit must already have been committed
    /// or reverted.
    fn start_edit(&mut self, row: usize, col: Col, map: &IndexMap<String, Entry>) {
        let row = row.min(map.len());
        let original = if row == map.len() && col == Col::Value {
            // Re-entering the ghost's value cell resumes the stashed text.
            self.pending_ghost_value.clone().unwrap_or_default()
        } else {
            if row < map.len() {
                // The edit moved onto a real row: the ghost was abandoned.
                self.pending_ghost_value = None;
            }
            Self::cell_text(map, row, col)
        };
        self.selected = Some(row);
        self.editing = Some(CellEdit {
            row,
            col,
            input: LineInput::new(&original),
            original,
        });
    }

    /// Leaves editing with the cursor parked on `row`.
    fn exit_editing(&mut self, row: usize, map: &IndexMap<String, Entry>) {
        self.editing = None;
        self.selected = Some(row.min(map.len()));
    }

    /// Click entry point: commits whatever was being edited (surfacing its
    /// warning), then begins editing `row`/`col` with the caret at the end.
    /// `row == map.len()` targets the ghost row. Clicking the cell already
    /// under edit is inert, so a double click is exactly one edit session.
    pub fn click_cell(
        &mut self,
        row: usize,
        col: Col,
        map: &mut IndexMap<String, Entry>,
    ) -> TableOutcome {
        let row = row.min(map.len());
        if self
            .editing
            .as_ref()
            .is_some_and(|e| e.row == row && e.col == col)
        {
            return TableOutcome::consumed();
        }
        let warning = self.commit(map).warning;
        // A commit can collapse rows, so re-clamp against the new length.
        let row = row.min(map.len());
        self.start_edit(row, col, map);
        TableOutcome::maybe_warn(warning)
    }

    /// Commits whatever is being edited (click-away, focus loss, `Enter`).
    /// A ghost row whose key is still empty is discarded silently.
    pub fn commit(&mut self, map: &mut IndexMap<String, Entry>) -> TableOutcome {
        let Some(edit) = self.editing.take() else {
            return TableOutcome::not_consumed();
        };
        let (row, warning) = self.commit_cell(map, &edit);
        self.selected = Some(row.unwrap_or(map.len()).min(map.len()));
        TableOutcome::maybe_warn(warning)
    }

    /// `Esc`: reverts the active cell to its pre-edit text and leaves
    /// editing. A row that existed survives; a ghost row that was being
    /// typed simply never happened.
    pub fn revert(&mut self, map: &mut IndexMap<String, Entry>) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        // The map is only ever written on commit, so the pre-edit text is
        // still in place; restoring it is belt-and-braces against any path
        // that wrote through the map mid-edit.
        if edit.col == Col::Value
            && let Some((_, e)) = map.get_index_mut(edit.row)
        {
            e.value.clone_from(&edit.original);
        }
        if edit.row >= map.len() {
            // Reverting a ghost edit: the row never happened, stash and all.
            self.pending_ghost_value = None;
        }
        self.selected = Some(edit.row.min(map.len()));
    }

    /// Writes one cell into the map. Returns the row index the edit
    /// resolved to (`None` when nothing was written — an empty ghost row)
    /// plus any warning to surface.
    fn commit_cell(
        &mut self,
        map: &mut IndexMap<String, Entry>,
        edit: &CellEdit,
    ) -> (Option<usize>, Option<String>) {
        let typed = edit.input.text().to_string();
        if edit.row < map.len() {
            return match edit.col {
                Col::Value => {
                    if let Some((_, e)) = map.get_index_mut(edit.row) {
                        e.value = typed;
                    }
                    (Some(edit.row), None)
                }
                Col::Key => Self::commit_key(map, edit.row, typed),
            };
        }
        // The ghost row: only a non-empty key can make it a real row. A
        // value typed with no key is stashed until one arrives.
        match edit.col {
            Col::Key if !typed.trim().is_empty() => {
                let pending = self.pending_ghost_value.take();
                if let Some(other) = map.get_index_of(&typed) {
                    return (
                        Some(other),
                        Some(format!("'{typed}' already exists — editing that row")),
                    );
                }
                map.insert(
                    typed,
                    Entry {
                        value: pending.unwrap_or_default(),
                        enabled: true,
                    },
                );
                (Some(map.len() - 1), None)
            }
            Col::Value => {
                self.pending_ghost_value = (!typed.is_empty()).then_some(typed);
                (None, None)
            }
            _ => (None, None),
        }
    }

    /// Renames row `idx` to `new_key`, keeping its position, value and
    /// enabled flag. Renaming onto a key that already exists collapses the
    /// two rows (the target keeps its slot and takes this row's value) and
    /// warns; blanking a key is refused with a warning.
    fn commit_key(
        map: &mut IndexMap<String, Entry>,
        idx: usize,
        new_key: String,
    ) -> (Option<usize>, Option<String>) {
        let Some((orig, entry)) = map.get_index(idx).map(|(k, e)| (k.clone(), e.clone())) else {
            return (None, None);
        };
        if new_key == orig {
            return (Some(idx), None);
        }
        if new_key.trim().is_empty() {
            return (
                Some(idx),
                Some(format!("a row needs a name — kept '{orig}'")),
            );
        }
        if let Some(other_idx) = map.get_index_of(&new_key) {
            map.shift_remove_index(idx);
            let adjusted = if other_idx > idx {
                other_idx - 1
            } else {
                other_idx
            };
            if let Some((_, e)) = map.get_index_mut(adjusted) {
                e.value = entry.value;
            }
            return (
                Some(adjusted),
                Some(format!(
                    "duplicate key '{new_key}' replaced the existing value"
                )),
            );
        }
        map.shift_remove_index(idx);
        map.shift_insert(idx, new_key, entry);
        (Some(idx), None)
    }

    /// Begins editing the selected row's key cell (the `Enter` path, and
    /// the app-side flows that seed a cell edit). A no-op with nothing
    /// selected.
    pub fn begin_edit_selected(&mut self, map: &IndexMap<String, Entry>) {
        let Some(sel) = self.selected else { return };
        self.start_edit(sel, Col::Key, map);
    }

    /// Starts a brand-new row: the ghost row's key cell, exactly like
    /// clicking it. Shared by the `a` key path.
    pub fn begin_add(&mut self, map: &IndexMap<String, Entry>) {
        self.start_edit(map.len(), Col::Key, map);
    }

    /// Deletes row `i` outright — no confirm gate (both the `d` key and
    /// the `󰅖` click land here directly): the deletion is an ordinary
    /// editor undo step.
    pub fn delete_row(&mut self, map: &mut IndexMap<String, Entry>, i: usize) {
        if i >= map.len() {
            return;
        }
        self.editing = None;
        map.shift_remove_index(i);
        self.selected = Some(i.min(map.len()));
    }

    /// Whether the cursor sits on the ghost row — one past the data rows —
    /// with no edit in progress.
    fn ghost_selected(&self, map: &IndexMap<String, Entry>) -> bool {
        self.selected == Some(map.len()) && self.editing.is_none()
    }

    /// Whether the ghost row (not any existing row) is the one under edit.
    pub fn editing_ghost(&self, map_len: usize) -> bool {
        self.editing.as_ref().is_some_and(|e| e.row >= map_len)
    }

    pub fn handle_key(&mut self, ev: KeyEvent, map: &mut IndexMap<String, Entry>) -> TableOutcome {
        match self.editing.take() {
            Some(edit) => self.handle_editing_key(ev, map, edit),
            None => self.handle_nav_key(ev, map),
        }
    }

    fn handle_nav_key(&mut self, ev: KeyEvent, map: &mut IndexMap<String, Entry>) -> TableOutcome {
        match ev.code {
            KeyCode::Char('j') | KeyCode::Down => {
                // The cursor's range is the data rows plus one: index
                // `map.len()` is the ghost row, so the keyboard can reach it
                // the same way the mouse can (and an empty table still has
                // that one stop to land on).
                self.selected = Some(match self.selected {
                    None => 0, // nothing selected: Down selects the first row
                    Some(s) => (s + 1).min(map.len()),
                });
                TableOutcome::consumed()
            }
            KeyCode::Char('k') | KeyCode::Up => {
                // Row 0 and no selection leave Up unconsumed so the caller
                // (Editor) can fall back to climbing out to the tab strip
                // instead of leaving the user stuck with no way back.
                match self.selected {
                    Some(s) if s > 0 => {
                        self.selected = Some(s - 1);
                        TableOutcome::consumed()
                    }
                    _ => TableOutcome::not_consumed(),
                }
            }
            KeyCode::Esc => {
                // Esc deselects (collapsing the expanded row); with nothing
                // selected it stays unconsumed for the caller.
                if self.selected.is_some() {
                    self.selected = None;
                    TableOutcome::consumed()
                } else {
                    TableOutcome::not_consumed()
                }
            }
            // `a` is the keyboard shorthand for "start a new row": it opens
            // the ghost row's key cell, exactly like clicking it.
            KeyCode::Char('a') => {
                self.begin_add(map);
                TableOutcome::consumed()
            }
            KeyCode::Enter => {
                if self.selected.is_none() {
                    return TableOutcome::not_consumed();
                }
                self.begin_edit_selected(map);
                TableOutcome::consumed()
            }
            KeyCode::Char(' ') => {
                if self.ghost_selected(map) {
                    return TableOutcome::not_consumed();
                }
                let Some((_, e)) = self.selected.and_then(|s| map.get_index_mut(s)) else {
                    return TableOutcome::not_consumed();
                };
                e.enabled = !e.enabled;
                TableOutcome::consumed()
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if self.ghost_selected(map) || self.selected.is_none_or(|s| s >= map.len()) {
                    return TableOutcome::not_consumed();
                }
                TableOutcome {
                    consumed: true,
                    warning: None,
                    request_delete: self.selected,
                }
            }
            _ => TableOutcome::not_consumed(),
        }
    }

    fn handle_editing_key(
        &mut self,
        ev: KeyEvent,
        map: &mut IndexMap<String, Entry>,
        mut edit: CellEdit,
    ) -> TableOutcome {
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        match ev.code {
            KeyCode::Esc => {
                self.editing = Some(edit);
                self.revert(map);
                TableOutcome::consumed()
            }
            KeyCode::Enter => {
                self.editing = Some(edit);
                let outcome = self.commit(map);
                // Enter is "I'm done editing": the selection drops too, so
                // the row collapses back to its compact line — unless the
                // commit warned (e.g. a duplicate key resolving to another
                // row), where the selection is the warning's pointer.
                if outcome.warning.is_none() {
                    self.selected = None;
                }
                outcome
            }
            // Up/Down leave the cell rather than falling through to
            // `LineInput` (which ignores them): they commit it and move the
            // cursor one row. Without this the pane's own "Up climbs out to
            // the tab strip" fallback could fire with an edit still open,
            // leaving `editing` set while the keyboard is elsewhere.
            KeyCode::Up | KeyCode::Down => {
                let (row, warning) = self.commit_cell(map, &edit);
                let here = row.unwrap_or(edit.row).min(map.len());
                let target = if ev.code == KeyCode::Up {
                    here.saturating_sub(1)
                } else {
                    here + 1
                };
                self.exit_editing(target, map);
                TableOutcome::maybe_warn(warning)
            }
            KeyCode::BackTab => self.walk_cell(map, &edit, false),
            KeyCode::Tab if shift => self.walk_cell(map, &edit, false),
            KeyCode::Tab => self.walk_cell(map, &edit, true),
            _ => {
                let consumed = edit.input.handle_key(ev);
                self.editing = Some(edit);
                if consumed {
                    TableOutcome::consumed()
                } else {
                    TableOutcome::not_consumed()
                }
            }
        }
    }

    /// Tab / Shift-Tab: commit the cell, then step one cell right
    /// (`forward`) or left, wrapping onto the next/previous row. Stepping
    /// off either end — past the ghost row, or back off the first cell —
    /// commits and leaves editing.
    fn walk_cell(
        &mut self,
        map: &mut IndexMap<String, Entry>,
        edit: &CellEdit,
        forward: bool,
    ) -> TableOutcome {
        let (row, warning) = self.commit_cell(map, edit);
        if forward {
            match (row, edit.col) {
                (Some(r), Col::Key) => self.start_edit(r, Col::Value, map),
                (Some(r), Col::Value) if r < map.len() => self.start_edit(r + 1, Col::Key, map),
                // Past the ghost row (and the discarded-ghost case): done.
                (Some(r), Col::Value) => self.exit_editing(r, map),
                (None, _) => self.exit_editing(map.len(), map),
            }
        } else {
            // A discarded ghost row still tells us where we were.
            let r = row.unwrap_or(edit.row).min(map.len());
            match edit.col {
                Col::Value => self.start_edit(r, Col::Key, map),
                Col::Key if r > 0 => self.start_edit(r - 1, Col::Value, map),
                Col::Key => self.exit_editing(r, map),
            }
        }
        TableOutcome::maybe_warn(warning)
    }

    /// Draws the table as one contiguous painted control: a muted-uppercase
    /// `NAME`/`VALUE` header row on `panel`, a `control` body of compact
    /// 1-line rows (the active row — selected, or being edited — keeps that
    /// height and reveals its own buttons at the value's right end), the
    /// ghost row (an empty row labelled by `add_label` until it is typed
    /// into), and a closing `▔` edge. Every cell registers a
    /// `Hit::TableCell`, so a click lands straight in that cell's editor.
    /// `shadow` is `Some` only on the Vars tab: `name → "overrides <env>:
    /// <value>"`, already formatted (masked for secrets) by the caller. A
    /// row whose key is present shows that note, dim, trailing its value on
    /// the same line. `None` on Params/Headers, which have no shadowing
    /// concept.
    /// `vars` is the variable snapshot every drawn cell's `{{tokens}}` are
    /// tinted and registered against (spec §7).
    #[allow(clippy::too_many_arguments)] // signature is the produced interface, verbatim
    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        map: &IndexMap<String, Entry>,
        ctx: &DrawCtx,
        add_label: &str,
        hits: &mut HitMap,
        shadow: Option<&IndexMap<String, String>>,
        vars: &VarView,
    ) {
        let theme = ctx.theme;
        let map_len = map.len();
        let ghost_editing = self.editing_ghost(map_len);
        let buf = frame.buffer_mut();
        let bottom = area.bottom();
        let mut y = area.y;

        // --- header ------------------------------------------------------
        if y < bottom {
            let cols = columns(area.x, area.width);
            fill(buf, Rect::new(area.x, y, area.width, 1), theme.panel);
            text(
                buf,
                cols.name_x,
                y,
                "NAME",
                theme.text_muted,
                theme.panel,
                true,
            );
            text(
                buf,
                cols.value_x,
                y,
                "VALUE",
                theme.text_muted,
                theme.panel,
                true,
            );
            if cols.divider_x < area.right() {
                text(
                    buf,
                    cols.divider_x,
                    y,
                    "\u{258F}",
                    theme.edge_dark,
                    theme.panel,
                    false,
                );
            }
            y += 1;
        }

        // --- data rows -----------------------------------------------------
        for (i, (k, e)) in map.iter().enumerate() {
            if y >= bottom {
                break;
            }
            let hint = shadow
                .and_then(|s| s.get(k))
                .map(|s| format!("overrides {s}"));
            self.draw_row(
                buf,
                hits,
                area,
                y,
                i,
                k,
                e,
                ctx,
                hint.as_deref(),
                vars,
                true,
                None,
            );
            y += 1;
        }

        // --- the ghost row -------------------------------------------------
        // Always present, one past the data rows: an empty row that becomes
        // a real entry the moment its key cell commits non-empty. It draws
        // like every other row — the add label stands in for its key until
        // it is typed into, and it carries no toggle or actions, having
        // nothing yet to toggle or delete.
        if y < bottom {
            let entry = Entry {
                // A value typed before the key shows while the key is
                // being typed, not just once the row commits.
                value: self.pending_ghost_value.clone().unwrap_or_default(),
                enabled: true,
            };
            let label = (!ghost_editing).then_some(add_label);
            self.draw_row(
                buf, hits, area, y, map_len, "", &entry, ctx, None, vars, false, label,
            );
            y += 1;
        }

        // --- closing edge --------------------------------------------------
        if y < bottom {
            crate::paint::rule(
                buf,
                Rect::new(area.x, y, area.width, 1),
                theme.edge_dark,
                theme.page,
            );
        }
    }

    /// Scrubs the strikethrough off a button zone before its glyph lands
    /// there: `text` patches styles, so a disabled row's struck name or
    /// value would otherwise bleed onto the glyphs painted over it.
    fn clear_strike(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, w: u16) {
        for x in x..x.saturating_add(w) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().remove_modifier(Modifier::CROSSED_OUT));
            }
        }
    }

    /// Paints the row's enable/disable toggle (`●` on / `○` off) in the
    /// [`TOGGLE_W`]-cell gutter at the row's left edge, `x` being that
    /// edge. Unlike the actions opposite it this is not hover-revealed:
    /// it is the row's own state, and every row shows it.
    #[allow(clippy::too_many_arguments)]
    fn draw_row_toggle(
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        x: u16,
        y: u16,
        i: usize,
        enabled: bool,
        bg: ratatui::style::Color,
        hovered: Option<&Hit>,
        theme: &Theme,
    ) {
        let hit = Hit::TableCheckbox(i);
        Self::clear_strike(buf, x, y, TOGGLE_W);
        let (glyph, state_fg) = if enabled {
            (" \u{25CF} ", theme.success)
        } else {
            (" \u{25CB} ", theme.text_muted)
        };
        let (fg, bg) = if hovered == Some(&hit) {
            (theme.on_accent, theme.accent)
        } else {
            (state_fg, bg)
        };
        text(buf, x, y, glyph, fg, bg, false);
        hits.register(Rect::new(x, y, TOGGLE_W, 1), hit);
    }

    /// Paints the row's right-edge actions — copy then delete, each a Nerd
    /// Font Material glyph centred in its own [`ACTION_W`]-cell zone —
    /// flush against `right` with one column of margin. A directly-hovered
    /// button inverts onto accent (error red for the trash), the same
    /// treatment the response pane's copy pills use.
    #[allow(clippy::too_many_arguments)]
    fn draw_row_actions(
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        right: u16,
        y: u16,
        i: usize,
        bg: ratatui::style::Color,
        hovered: Option<&Hit>,
        theme: &Theme,
    ) {
        let trash_x = right.saturating_sub(ACTION_W + 1);
        let copy_x = trash_x.saturating_sub(ACTION_W);
        Self::clear_strike(buf, copy_x, y, ACTIONS_W);

        let copy_hit = Hit::TableCopy(i);
        let (cfg, cbg) = if hovered == Some(&copy_hit) {
            (theme.on_accent, theme.accent)
        } else {
            (theme.text_muted, bg)
        };
        text(buf, copy_x, y, crate::glyph::COPY_PILL, cfg, cbg, false);

        let trash_hit = Hit::TableDelete(i);
        let (dfg, dbg) = if hovered == Some(&trash_hit) {
            (theme.on_accent, theme.error)
        } else {
            (theme.text_muted, bg)
        };
        text(buf, trash_x, y, crate::glyph::DELETE_PILL, dfg, dbg, false);

        hits.register(Rect::new(copy_x, y, ACTION_W, 1), copy_hit);
        hits.register(Rect::new(trash_x, y, ACTION_W, 1), trash_hit);
    }

    /// Strikes through `len` cells starting at `(x, y)` — the disabled
    /// row's name treatment, applied after the text is painted.
    fn strike_cells(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, len: u16) {
        for dx in 0..len {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_style(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
        }
    }

    /// Registers the key/value halves of one drawn row line. Called after
    /// the row's own background hit, so a click resolves to the cell.
    fn register_cells(hits: &mut HitMap, span: (u16, u16, u16, u16), y: u16, row: usize) {
        let (name_x, name_w, value_x, value_w) = span;
        if name_w > 0 {
            hits.register(
                Rect::new(name_x, y, name_w, 1),
                Hit::TableCell {
                    row,
                    col: Col::Key.index(),
                },
            );
        }
        if value_w > 0 {
            hits.register(
                Rect::new(value_x, y, value_w, 1),
                Hit::TableCell {
                    row,
                    col: Col::Value.index(),
                },
            );
        }
    }

    /// Draws one row of the table on its own single line at `y`: a real
    /// entry, or — with `real` false — the always-present ghost row,
    /// whose key cell carries `ghost_label` until it is typed into.
    ///
    /// One painter, one row height. A row being edited draws its
    /// `LineInput` in place, right where the text was, the way the
    /// Variable Manager's grid edits its own cells: nothing opens up, and
    /// no row below the cursor moves. `hint` is the Vars tab's
    /// "overrides <env>: <value>" shadow, trailing the value dim.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        area: Rect,
        y: u16,
        i: usize,
        key: &str,
        entry: &Entry,
        ctx: &DrawCtx,
        hint: Option<&str>,
        vars: &VarView,
        real: bool,
        ghost_label: Option<&str>,
    ) {
        use super::chooser::clip;
        let theme = ctx.theme;
        let cols = columns(area.x, area.width);
        let editing_col = self.editing.as_ref().filter(|e| e.row == i).map(|e| e.col);
        // The keyboard cursor lights its row only while the pane actually
        // holds the keyboard — a lift on an unfocused pane would claim keys
        // land here. A pointer resting on the row lights it either way.
        let cursor = self.selected == Some(i) || editing_col.is_some();
        let lit = hovered_row(ctx) == Some(i) || (cursor && ctx.focused);
        let bg = if lit {
            theme.control_hover
        } else {
            theme.control
        };
        fill(buf, Rect::new(area.x, y, area.width, 1), bg);

        // The actions crowd the value's right end, so the value — plain
        // text or live input — stops short of them instead of running
        // underneath. The ghost row has no actions, so it keeps the width.
        let show_actions = real && (lit || cursor) && area.width >= 10;
        let value_right = if show_actions {
            area.right().saturating_sub(ACTIONS_W)
        } else {
            area.right()
        };
        let value_w = value_right.saturating_sub(cols.value_x);
        let name_w = cols.divider_x.saturating_sub(cols.name_x);
        let fg = match (real, entry.enabled, lit) {
            (false, _, true) => theme.text,
            (false, _, false) => theme.text_muted,
            (true, true, _) => theme.text,
            (true, false, _) => theme.text_muted,
        };

        if editing_col == Some(Col::Key) {
            let edit = self.editing.as_ref().expect("editing_col implies editing");
            Self::paint_cell_edit(buf, cols.name_x, y, name_w, &edit.input, bg, theme);
        } else {
            let shown = ghost_label.unwrap_or(key);
            text(buf, cols.name_x, y, clip(shown, name_w), fg, bg, false);
        }

        if editing_col == Some(Col::Value) {
            let edit = self.editing.as_ref().expect("editing_col implies editing");
            Self::paint_cell_edit(buf, cols.value_x, y, value_w, &edit.input, bg, theme);
        } else {
            let shown = clip(&entry.value, value_w);
            text(buf, cols.value_x, y, shown, fg, bg, false);
            // The shadow hint trails the value on the same line, dim: it
            // is a note about that value, so it reads where the value
            // ends rather than on a row of its own.
            if let Some(hint) = hint {
                let used = shown.chars().count() as u16;
                let hint_x = cols.value_x + used + 3;
                let room = value_right.saturating_sub(hint_x);
                if room > 0 {
                    text(
                        buf,
                        hint_x,
                        y,
                        clip(hint, room),
                        theme.text_muted,
                        bg,
                        false,
                    );
                }
            }
        }

        if real && !entry.enabled {
            // The strike marks the text that is actually there: a value
            // too long for its cell was clipped, and the line has to stop
            // where the clip did rather than run on across empty cells
            // (and, on a narrow pane, into whatever is drawn beside it).
            if editing_col != Some(Col::Key) {
                let len = (key.chars().count() as u16).min(name_w);
                Self::strike_cells(buf, cols.name_x, y, len);
            }
            if editing_col != Some(Col::Value) {
                let len = (entry.value.chars().count() as u16).min(value_w);
                Self::strike_cells(buf, cols.value_x, y, len);
            }
        }

        if cols.divider_x < area.right() {
            text(
                buf,
                cols.divider_x,
                y,
                "\u{258F}",
                theme.edge_dark,
                bg,
                false,
            );
        }

        hits.register(Rect::new(area.x, y, area.width, 1), Hit::TableRow(i));
        Self::register_cells(hits, cols_span(&cols, value_right), y, i);
        // Only the cells drawn as plain text get token treatment: a cell
        // under edit is showing a live `LineInput` (caret and all), and
        // registering a `VarToken` over it would turn the next click into a
        // picker instead of a caret move. The ghost's add label is not a
        // value, so it gets none either.
        paint_cell_tokens(
            buf,
            hits,
            &cols,
            value_right,
            y,
            if real && editing_col != Some(Col::Key) {
                key
            } else {
                ""
            },
            if editing_col == Some(Col::Value) {
                ""
            } else {
                entry.value.as_str()
            },
            vars,
            theme,
        );
        // The toggle owns the left gutter on every real row; the actions
        // are revealed by hover or the cursor. Both paint (and register)
        // last so they win over the cells underneath.
        if real && area.width >= TOGGLE_W {
            Self::draw_row_toggle(
                buf,
                hits,
                area.x,
                y,
                i,
                entry.enabled,
                bg,
                ctx.hovered,
                theme,
            );
        }
        if show_actions {
            Self::draw_row_actions(buf, hits, area.right(), y, i, bg, ctx.hovered, theme);
        }
    }

    /// Paints a live cell edit in place: the cell lifts one step above its
    /// row — so the exact cell the keyboard sits in reads at a glance, the
    /// Variable Manager grid's rule for its own in-place edits — and the
    /// input draws into it, windowed to the cell's own width.
    fn paint_cell_edit(
        buf: &mut ratatui::buffer::Buffer,
        x: u16,
        y: u16,
        w: u16,
        input: &LineInput,
        row_bg: ratatui::style::Color,
        theme: &Theme,
    ) {
        if w == 0 {
            return;
        }
        let bg = crate::theme::lift_color(row_bg, 0.06);
        fill(buf, Rect::new(x, y, w, 1), bg);
        let mut line = input.draw_line_windowed(true, theme, w);
        line.style = Style::default().bg(bg).patch(line.style);
        buf.set_line(x, y, &line, w);
    }
}

/// Tints the `{{tokens}}` in one drawn row's key and value cells (spec §7),
/// registering each span over the `TableCell` hit beneath it. Both texts are
/// whatever that row actually drew, so an empty string paints nothing.
#[allow(clippy::too_many_arguments)]
fn paint_cell_tokens(
    buf: &mut ratatui::buffer::Buffer,
    hits: &mut HitMap,
    cols: &Columns,
    value_right: u16,
    y: u16,
    key: &str,
    value: &str,
    vars: &VarView,
    theme: &Theme,
) {
    let (name_x, name_w, value_x, value_w) = cols_span(cols, value_right);
    if name_w > 0 {
        paint_var_tokens(
            buf,
            Rect::new(name_x, y, name_w, 1),
            key,
            name_x,
            vars,
            theme,
            hits,
        );
    }
    if value_w > 0 {
        paint_var_tokens(
            buf,
            Rect::new(value_x, y, value_w, 1),
            value,
            value_x,
            vars,
            theme,
            hits,
        );
    }
}

/// `(name_x, name_w, value_x, value_w)` for a row's two clickable cells:
/// the name cell stops at the divider, the value cell runs to
/// `value_right` — where the value was actually drawn, which is short of
/// the area's right edge on a row showing its actions. The registered
/// width has to be the drawn width: a cell under edit windows its text to
/// it, and the click that places the caret reads the width back off the
/// hit rect.
fn cols_span(cols: &Columns, value_right: u16) -> (u16, u16, u16, u16) {
    let name_w = cols.divider_x.saturating_sub(cols.name_x);
    let value_w = value_right.saturating_sub(cols.value_x);
    (cols.name_x, name_w, cols.value_x, value_w)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn shift_tab() -> KeyEvent {
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
    }

    fn map_of(pairs: &[(&str, &str)]) -> IndexMap<String, Entry> {
        let mut map = IndexMap::new();
        for (k, v) in pairs {
            map.insert(
                (*k).to_string(),
                Entry {
                    value: (*v).to_string(),
                    enabled: true,
                },
            );
        }
        map
    }

    fn type_str(t: &mut TableEditorState, map: &mut IndexMap<String, Entry>, s: &str) {
        for c in s.chars() {
            t.handle_key(key(KeyCode::Char(c)), map);
        }
    }

    // --- ghost row selection ----------------------------------------------

    /// Arrowing onto the ghost row selects it like any other row — no edit
    /// yet; Enter is what opens the add edit, same as the real rows.
    #[test]
    fn down_onto_the_ghost_row_selects_it_and_enter_opens_the_add_edit() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Down), &mut map); // row 0
        t.handle_key(key(KeyCode::Down), &mut map); // ghost row
        assert_eq!(t.selected, Some(1));
        assert!(t.editing.is_none(), "selection only, like the real rows");
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(t.editing_ghost(map.len()), "Enter opens the add edit");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!(edit.col, Col::Key);
        assert_eq!(edit.input.text(), "");
    }

    /// Moving off a selected-but-untouched ghost creates nothing.
    #[test]
    fn leaving_the_selected_ghost_untouched_saves_no_row() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Down), &mut map);
        t.handle_key(key(KeyCode::Down), &mut map); // ghost selected
        t.handle_key(key(KeyCode::Up), &mut map); // straight back out
        assert_eq!(map.len(), 1, "no empty header appeared");
        assert_eq!(t.selected, Some(0));
        // Same through the edit: Enter in, Up straight out.
        t.handle_key(key(KeyCode::Down), &mut map);
        t.handle_key(key(KeyCode::Enter), &mut map);
        t.handle_key(key(KeyCode::Up), &mut map);
        assert!(t.editing.is_none());
        assert_eq!(map.len(), 1, "an untouched add edit saves nothing");
    }

    /// With the cursor resting on it, the ghost row lights like any other
    /// cursor row and keeps its add label until Enter.
    #[test]
    fn the_ghost_row_under_the_cursor_lights_and_keeps_its_add_label() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let t = TableEditorState {
            selected: Some(1), // the ghost
            ..TableEditorState::default()
        };
        let ctx = ctx(&theme, None);
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx, &mut hits);
        let row = hits
            .rect_of(&Hit::TableRow(1))
            .expect("the ghost row is registered");
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf.cell((row.x, row.y)).unwrap().bg,
            theme.control_hover,
            "the cursor row lifts its own fill"
        );
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("+ Add"),
            "the add label survives: {content}"
        );
    }

    // --- click entry point ------------------------------------------------

    #[test]
    fn click_cell_edits_that_cell_in_place_with_the_caret_at_the_end() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        let out = t.click_cell(0, Col::Value, &mut map);
        assert!(out.consumed);
        assert!(out.warning.is_none());
        let edit = t.editing.as_ref().expect("the click began an edit");
        assert_eq!(edit.row, 0);
        assert_eq!(edit.col, Col::Value);
        assert_eq!(edit.input.text(), "2", "seeded with the cell's own text");
        assert_eq!(edit.input.cursor(), 1, "caret at the end");
        assert_eq!(edit.original, "2");
        assert_eq!(t.selected, Some(0), "the clicked row is the selected row");
    }

    #[test]
    fn a_second_click_on_the_cell_under_edit_is_inert() {
        // Two fast clicks on a cell (a double click) must leave exactly one
        // edit session, with the typing so far untouched.
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "34");
        let out = t.click_cell(0, Col::Value, &mut map);
        assert!(out.consumed);
        let edit = t.editing.as_ref().expect("still one edit session");
        assert_eq!(edit.input.text(), "234", "typing survives the second click");
        assert_eq!(map["page"].value, "2", "nothing committed yet");
    }

    #[test]
    fn clicking_another_cell_commits_the_one_being_edited() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        t.click_cell(1, Col::Key, &mut map);
        assert_eq!(map["a"].value, "19", "the previous cell committed");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (1, Col::Key));
        assert_eq!(edit.input.text(), "b");
    }

    #[test]
    fn commit_writes_the_cell_and_ends_the_edit() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "34");
        let out = t.commit(&mut map);
        assert!(out.consumed);
        assert!(t.editing.is_none());
        assert_eq!(map["page"].value, "234");
        assert!(
            !t.commit(&mut map).consumed,
            "committing with no edit in progress is a no-op"
        );
    }

    #[test]
    fn revert_restores_the_cell_and_leaves_the_row_alone() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "999");
        t.revert(&mut map);
        assert!(t.editing.is_none());
        assert_eq!(map["page"].value, "2", "the pre-edit value is back");
        assert_eq!(map.len(), 1, "the row survives");
        assert_eq!(t.selected, Some(0), "the row stays selected");
    }

    #[test]
    fn ghost_row_click_and_commit_creates_the_row() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map); // row 0 == map.len(): the ghost
        assert_eq!(t.editing.as_ref().unwrap().row, 0);
        assert_eq!(t.editing.as_ref().unwrap().input.text(), "");
        type_str(&mut t, &mut map, "page");
        assert!(map.is_empty(), "nothing inserted until the commit");
        t.commit(&mut map);
        assert_eq!(
            map["page"],
            Entry {
                value: String::new(),
                enabled: true
            }
        );
    }

    #[test]
    fn ghost_row_left_empty_is_discarded_silently() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Key, &mut map); // the ghost row
        let out = t.commit(&mut map);
        assert_eq!(map.len(), 1, "no \"\" key inserted");
        assert!(out.warning.is_none(), "leaving it empty is silent");
        assert!(t.editing.is_none());

        // Same for the ghost's value cell: with no key there is no row.
        t.click_cell(1, Col::Value, &mut map);
        type_str(&mut t, &mut map, "orphan");
        let out = t.commit(&mut map);
        assert_eq!(map.len(), 1);
        assert!(out.warning.is_none());
    }

    #[test]
    fn ghost_value_typed_first_survives_the_hop_to_the_key_cell() {
        // Type into the ghost row's VALUE cell first, then click over to the
        // NAME cell: the typed value must ride along and land on the row the
        // key commit creates, not silently vanish.
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "42");
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "id");
        t.commit(&mut map);
        assert_eq!(map["id"].value, "42", "the value typed first is kept");
    }

    #[test]
    fn ghost_value_survives_walking_back_to_the_key_cell_by_keyboard() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "42");
        t.handle_key(shift_tab(), &mut map); // back to the key cell
        type_str(&mut t, &mut map, "id");
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert_eq!(map["id"].value, "42");
    }

    #[test]
    fn reclicking_the_ghost_value_cell_shows_the_stashed_text() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "42");
        t.click_cell(0, Col::Key, &mut map);
        t.click_cell(0, Col::Value, &mut map);
        assert_eq!(
            t.editing.as_ref().unwrap().input.text(),
            "42",
            "hopping away and back does not lose the typed value"
        );
    }

    #[test]
    fn a_stashed_ghost_value_is_dropped_when_the_edit_leaves_the_ghost_row() {
        // Typing a value with no key, then wandering off to a real row,
        // abandons the ghost: a later new row must not inherit stale text.
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Value, &mut map); // the ghost row
        type_str(&mut t, &mut map, "stale");
        t.click_cell(0, Col::Value, &mut map); // a real row
        t.commit(&mut map);
        t.click_cell(1, Col::Key, &mut map);
        type_str(&mut t, &mut map, "fresh");
        t.commit(&mut map);
        assert_eq!(map["fresh"].value, "", "no stale value resurfaces");
    }

    #[test]
    fn esc_on_the_ghost_key_also_discards_a_stashed_value() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "42");
        t.click_cell(0, Col::Key, &mut map);
        t.revert(&mut map); // Esc: the ghost row never happened
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "id");
        t.commit(&mut map);
        assert_eq!(map["id"].value, "", "Esc wiped the stash too");
    }

    // --- keyboard: navigation --------------------------------------------

    #[test]
    fn nav_moves_the_selection_over_the_data_rows_and_the_ghost() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        assert!(t.handle_key(key(KeyCode::Down), &mut map).consumed);
        assert_eq!(t.selected, Some(0), "Down from nowhere selects row 0");
        t.handle_key(key(KeyCode::Char('j')), &mut map);
        t.handle_key(key(KeyCode::Char('j')), &mut map);
        assert_eq!(t.selected, Some(2), "the ghost row is reachable");
        assert!(t.editing.is_none(), "selection only — Enter starts the add");
        assert!(
            t.handle_key(key(KeyCode::Down), &mut map).consumed,
            "clamped at the ghost row, still consumed"
        );
        assert_eq!(t.selected, Some(2));
        t.handle_key(key(KeyCode::Char('k')), &mut map);
        assert_eq!(t.selected, Some(1));
        t.handle_key(key(KeyCode::Up), &mut map);
        assert_eq!(t.selected, Some(0));
        assert!(
            !t.handle_key(key(KeyCode::Up), &mut map).consumed,
            "Up at row 0 is left to the caller (climb out to the tab strip)"
        );
        let out = t.handle_key(key(KeyCode::Esc), &mut map);
        assert!(out.consumed);
        assert_eq!(t.selected, None, "Esc deselects");
        assert!(!t.handle_key(key(KeyCode::Esc), &mut map).consumed);
    }

    #[test]
    fn enter_edits_the_key_cell_of_the_selected_row_including_the_ghost() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        assert!(
            !t.handle_key(key(KeyCode::Enter), &mut map).consumed,
            "Enter with nothing selected is inert"
        );
        t.selected = Some(0);
        assert!(t.handle_key(key(KeyCode::Enter), &mut map).consumed);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (0, Col::Key));
        assert_eq!(edit.input.text(), "a");

        t.editing = None;
        t.selected = Some(1); // the ghost row
        t.handle_key(key(KeyCode::Enter), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (1, Col::Key));
        assert_eq!(edit.input.text(), "");
    }

    #[test]
    fn enter_committing_an_edit_deselects_the_row() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        t.handle_key(key(KeyCode::Enter), &mut map); // begin editing the key
        t.handle_key(key(KeyCode::Enter), &mut map); // commit — "I'm done"
        assert!(t.editing.is_none());
        assert_eq!(t.selected, None, "Enter after editing drops the selection");
    }

    #[test]
    fn enter_committing_a_duplicate_key_keeps_the_resolved_row_selected() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState {
            selected: Some(2), // ghost row
            ..TableEditorState::default()
        };
        t.handle_key(key(KeyCode::Enter), &mut map);
        for c in "a".chars() {
            t.handle_key(key(KeyCode::Char(c)), &mut map);
        }
        let out = t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(out.warning.is_some(), "duplicate key warns");
        assert_eq!(
            t.selected,
            Some(0),
            "the warning points at the existing row, so it stays selected"
        );
    }

    #[test]
    fn space_toggles_and_d_requests_a_delete_confirm() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        assert!(!t.handle_key(key(KeyCode::Char(' ')), &mut map).consumed);
        assert!(!t.handle_key(key(KeyCode::Char('d')), &mut map).consumed);

        t.selected = Some(0);
        assert!(t.handle_key(key(KeyCode::Char(' ')), &mut map).consumed);
        assert!(!map["a"].enabled);
        let out = t.handle_key(key(KeyCode::Delete), &mut map);
        assert_eq!(out.request_delete, Some(0));
        assert_eq!(map.len(), 1, "the row survives until the confirm");
        t.delete_row(&mut map, 0);
        assert!(map.is_empty());

        // The ghost row has nothing to toggle or delete.
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState {
            selected: Some(1),
            ..TableEditorState::default()
        };
        assert!(!t.handle_key(key(KeyCode::Char(' ')), &mut map).consumed);
        assert_eq!(
            t.handle_key(key(KeyCode::Char('d')), &mut map)
                .request_delete,
            None
        );
        assert!(map["a"].enabled);
    }

    // --- keyboard: editing -------------------------------------------------

    #[test]
    fn tab_commits_the_cell_and_walks_right_wrapping_onto_the_next_row() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "x"); // "ax"
        assert!(t.handle_key(key(KeyCode::Tab), &mut map).consumed);
        assert_eq!(map.get_index(0).unwrap().0, "ax", "the key cell committed");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (0, Col::Value));
        assert_eq!(edit.input.text(), "1", "seeded with the value cell");

        t.handle_key(key(KeyCode::Tab), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!(
            (edit.row, edit.col),
            (1, Col::Key),
            "Tab past a value wraps onto the next row's key"
        );
        t.handle_key(key(KeyCode::Tab), &mut map); // b's value
        t.handle_key(key(KeyCode::Tab), &mut map); // wraps onto the ghost key
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (2, Col::Key));
        assert!(
            t.handle_key(key(KeyCode::Tab), &mut map).consumed,
            "Tab past the empty ghost commits and exits"
        );
        assert!(t.editing.is_none());
        assert_eq!(map.len(), 2, "the untouched ghost added nothing");
    }

    #[test]
    fn shift_tab_commits_the_cell_and_walks_left() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9"); // "29"
        t.handle_key(shift_tab(), &mut map);
        assert_eq!(map["b"].value, "29", "the value cell committed");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (1, Col::Key));

        t.handle_key(shift_tab(), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!(
            (edit.row, edit.col),
            (0, Col::Value),
            "Shift-Tab off a key wraps onto the previous row's value"
        );
        t.handle_key(shift_tab(), &mut map); // row 0 key
        assert!(
            t.handle_key(shift_tab(), &mut map).consumed,
            "Shift-Tab off the first cell commits and exits"
        );
        assert!(t.editing.is_none());
    }

    #[test]
    fn shift_tab_off_the_ghost_key_lands_on_the_last_rows_value() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Key, &mut map); // the ghost, left empty
        t.handle_key(shift_tab(), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (0, Col::Value));
        assert_eq!(map.len(), 1, "the empty ghost added nothing");
    }

    #[test]
    fn up_and_down_while_editing_commit_the_cell_and_move_the_cursor() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        assert!(t.handle_key(key(KeyCode::Down), &mut map).consumed);
        assert!(t.editing.is_none(), "Down leaves the cell");
        assert_eq!(map["a"].value, "19", "and commits it");
        assert_eq!(t.selected, Some(1));

        t.click_cell(1, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        assert!(t.handle_key(key(KeyCode::Up), &mut map).consumed);
        assert!(t.editing.is_none());
        assert_eq!(map["b"].value, "29");
        assert_eq!(t.selected, Some(0));
    }

    #[test]
    fn enter_commits_the_row_and_exits_editing() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "page");
        t.handle_key(key(KeyCode::Tab), &mut map);
        type_str(&mut t, &mut map, "2");
        let out = t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(out.consumed);
        assert!(t.editing.is_none());
        assert_eq!(
            map["page"],
            Entry {
                value: "2".into(),
                enabled: true
            }
        );
        assert_eq!(t.selected, None, "Enter is 'done editing': deselects too");
    }

    #[test]
    fn esc_reverts_the_cell_and_exits_editing_without_touching_the_row() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        assert!(t.handle_key(key(KeyCode::Esc), &mut map).consumed);
        assert!(t.editing.is_none());
        assert_eq!(map["a"].value, "1", "the cell reverted");
        assert_eq!(map.len(), 1, "the row survives");

        // Esc after a Tab reverts only the cell it is in: the already
        // committed key cell keeps its new text.
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "x");
        t.handle_key(key(KeyCode::Tab), &mut map);
        type_str(&mut t, &mut map, "8");
        t.handle_key(key(KeyCode::Esc), &mut map);
        assert_eq!(map.get_index(0).unwrap().0, "ax", "the rename stands");
        assert_eq!(map["ax"].value, "1", "the value cell reverted");
    }

    #[test]
    fn esc_on_a_ghost_row_being_typed_discards_it() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Key, &mut map);
        type_str(&mut t, &mut map, "new");
        t.handle_key(key(KeyCode::Esc), &mut map);
        assert!(t.editing.is_none());
        assert_eq!(map.len(), 1, "the abandoned ghost added nothing");
    }

    // --- renames, duplicates, warnings ------------------------------------

    #[test]
    fn rename_keeps_the_rows_position_value_and_enabled_flag() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        map[0].enabled = false;
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "x");
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert_eq!(map.get_index(0).unwrap().0, "ax", "position kept");
        assert_eq!(map["ax"].value, "1");
        assert!(!map["ax"].enabled, "the enabled flag rides along");
        assert_eq!(map.get_index(1).unwrap().0, "b");
    }

    #[test]
    fn renaming_onto_a_later_key_collapses_the_rows_and_warns() {
        let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map);
        for _ in 0..1 {
            t.handle_key(key(KeyCode::Backspace), &mut map);
        }
        type_str(&mut t, &mut map, "c");
        let out = t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(out.warning.is_some(), "the collapse warns");
        assert_eq!(map.len(), 2);
        assert_eq!(map.get_index(0).unwrap().0, "b", "b shifts down");
        assert_eq!(map.get_index(1).unwrap().0, "c");
        assert_eq!(map["c"].value, "1", "c takes a's value");
        assert_eq!(t.selected, Some(1), "the cursor follows the surviving row");
    }

    #[test]
    fn a_ghost_row_keyed_like_an_existing_row_warns_and_edits_that_row() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(2, Col::Key, &mut map); // the ghost
        type_str(&mut t, &mut map, "a");
        let out = t.handle_key(key(KeyCode::Tab), &mut map);
        assert!(out.warning.is_some(), "a duplicate key warns");
        assert_eq!(map.len(), 2, "no second 'a' row was created");
        assert_eq!(map["a"].value, "1", "the existing value is untouched");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!(
            (edit.row, edit.col),
            (0, Col::Value),
            "the caret lands in the existing row's value cell"
        );
    }

    #[test]
    fn blanking_an_existing_rows_key_warns_and_keeps_the_key() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map);
        t.handle_key(key(KeyCode::Backspace), &mut map);
        let out = t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(out.warning.is_some());
        assert_eq!(map.get_index(0).unwrap().0, "a", "the row keeps its name");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn a_opens_the_ghost_rows_key_cell() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        assert!(t.handle_key(key(KeyCode::Char('a')), &mut map).consumed);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (1, Col::Key));
        assert_eq!(edit.input.text(), "");
    }

    #[test]
    fn reset_clears_selection_and_any_edit() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map);
        t.reset();
        assert!(t.editing.is_none());
        assert_eq!(t.selected, None);
    }

    // --- drawing ------------------------------------------------------------

    /// A disabled (instantly-jumping) `Anims` shared by every test's
    /// `DrawCtx`, so tests stay deterministic without threading an owned
    /// `Anims` through each call site.
    fn test_anims() -> &'static crate::anim::Anims {
        static ANIMS: std::sync::OnceLock<crate::anim::Anims> = std::sync::OnceLock::new();
        ANIMS.get_or_init(|| crate::anim::Anims::new(false))
    }

    fn ctx<'a>(theme: &'a Theme, hovered: Option<&'a Hit>) -> DrawCtx<'a> {
        DrawCtx {
            theme,
            focused: true,
            hovered,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        }
    }

    fn draw_to(
        t: &TableEditorState,
        map: &IndexMap<String, Entry>,
        ctx: &DrawCtx,
        hits: &mut HitMap,
    ) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    f.area(),
                    map,
                    ctx,
                    "+ Add param",
                    hits,
                    None,
                    &VarView::default(),
                )
            })
            .unwrap();
        terminal
    }

    #[test]
    fn draw_registers_a_cell_hit_for_every_cell_including_the_ghost_row() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let t = TableEditorState::default(); // nothing selected: compact rows
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("NAME"), "header: {content}");
        assert!(content.contains("+ Add param"), "ghost label: {content}");

        let row = hits.rect_of(&Hit::TableRow(0)).unwrap();
        assert_eq!(row.height, 1, "no selection: rows stay compact");
        let k = hits.rect_of(&Hit::TableCell { row: 0, col: 0 }).unwrap();
        let v = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert_eq!(k.y, row.y);
        assert_eq!(v.y, row.y);
        assert!(k.x < v.x, "key cell sits left of the value cell");
        // The ghost row's own two cells, one row below the data row.
        let gk = hits.rect_of(&Hit::TableCell { row: 1, col: 0 }).unwrap();
        let gv = hits.rect_of(&Hit::TableCell { row: 1, col: 1 }).unwrap();
        assert_eq!(gk.y, row.y + 1);
        assert_eq!(gv.y, gk.y);
        // Clicks resolve to the cell, not the row underneath it.
        assert_eq!(
            hits.hit_at(k.x, k.y),
            Some(&Hit::TableCell { row: 0, col: 0 })
        );
        assert_eq!(
            hits.hit_at(gv.x, gv.y),
            Some(&Hit::TableCell { row: 1, col: 1 })
        );
    }

    #[test]
    fn the_edited_row_shows_its_input_on_its_own_line() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let row1 = hits.rect_of(&Hit::TableRow(1)).unwrap();
        assert_eq!(
            row1,
            Rect::new(0, 2, 40, 1),
            "the edited row stays where and what it was"
        );
        assert_eq!(
            hits.rect_of(&Hit::TableRow(2)).unwrap().y,
            row1.y + 1,
            "the row below is where it always was"
        );
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("29"), "the live input text: {content}");
        // The row's copy/trash buttons stay up through a live cell edit
        // (the input is clipped short of their zone).
        assert!(hits.rect_of(&Hit::TableDelete(1)).is_some());
        // The cells are registered on the row's one line.
        let k = hits.rect_of(&Hit::TableCell { row: 1, col: 0 }).unwrap();
        assert_eq!(k.y, row1.y);
    }

    #[test]
    fn the_ghost_row_takes_the_typed_key_in_place() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Key, &mut map);
        type_str(&mut t, &mut map, "new");
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("new"), "the typed key: {content}");
        assert!(
            !content.contains("+ Add param"),
            "the add label gives way to the row being typed: {content}"
        );
        let ghost = hits.rect_of(&Hit::TableRow(1)).unwrap();
        assert_eq!(ghost.height, 1, "the ghost row edits in place too");
        assert!(
            hits.rect_of(&Hit::TableDelete(1)).is_none(),
            "a row that doesn't exist yet has nothing to delete"
        );
    }

    /// The actions — copy and trash — are hover-revealed and belong to the
    /// hovered row alone. The toggle beside them is not: it reports state,
    /// so it stays up (see
    /// `every_row_carries_its_toggle_at_the_left_edge_without_hover`).
    #[test]
    fn compact_rows_reveal_their_actions_only_on_hover() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1"), ("b", "2")]);
        let t = TableEditorState::default();
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        assert!(
            hits.rect_of(&Hit::TableCopy(0)).is_none(),
            "no actions on an unhovered row"
        );
        assert!(hits.rect_of(&Hit::TableDelete(0)).is_none());

        let hovered = Hit::TableRow(0);
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, Some(&hovered)), &mut hits);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("\u{F01B4}"),
            "the delete button is a trash can: {content}"
        );
        let copy = hits.rect_of(&Hit::TableCopy(0)).expect("copy hit");
        let trash = hits.rect_of(&Hit::TableDelete(0)).expect("delete hit");
        assert!(
            copy.width >= 3 && trash.width >= 3,
            "buttons get comfortable click targets: {copy:?} {trash:?}"
        );
        assert!(copy.x < trash.x, "copy left of trash");
        assert!(
            hits.rect_of(&Hit::TableDelete(1)).is_none(),
            "only the hovered row shows actions"
        );
    }

    #[test]
    fn disabled_rows_read_dim_with_name_and_value_struck() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "1")]);
        map["a"].enabled = false;
        let t = TableEditorState::default();
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let buf = terminal.backend().buffer();
        let name = hits.rect_of(&Hit::TableCell { row: 0, col: 0 }).unwrap();
        let cell = buf.cell((name.x, name.y)).unwrap();
        assert_eq!(cell.fg, theme.text_muted, "disabled rows dim");
        assert!(
            cell.modifier.contains(Modifier::CROSSED_OUT),
            "the disabled name is struck through"
        );
        let value = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        let vcell = buf.cell((value.x, value.y)).unwrap();
        assert!(
            vcell.modifier.contains(Modifier::CROSSED_OUT),
            "the disabled value is struck through with the name"
        );
    }

    /// A disabled value too long for its cell is clipped, so the strike has
    /// to stop where the text did — not run on across empty cells and out
    /// of the table's own area into whatever is drawn beside it.
    #[test]
    fn a_long_disabled_value_is_struck_only_where_it_is_drawn() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "0123456789012345678901234567890123456789")]);
        map["a"].enabled = false;
        let t = TableEditorState::default();
        let mut hits = HitMap::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let area = Rect::new(0, 0, 24, 10);
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    area,
                    &map,
                    &ctx(&theme, None),
                    "+ Add param",
                    &mut hits,
                    None,
                    &VarView::default(),
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let y = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap().y;
        for x in area.right()..40 {
            assert!(
                !buf.cell((x, y))
                    .unwrap()
                    .modifier
                    .contains(Modifier::CROSSED_OUT),
                "the strike ran past the table's own area, at column {x}"
            );
        }
    }

    /// Selecting a row must not move anything: rows edit in place on their
    /// own single line, the way the Variable Manager's grid does, so no
    /// row below the cursor ever shifts under it.
    #[test]
    fn selecting_a_row_keeps_every_row_one_line_and_moves_nothing_below_it() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1"), ("b", "2")]);
        let mut resting = HitMap::default();
        draw_to(
            &TableEditorState::default(),
            &map,
            &ctx(&theme, None),
            &mut resting,
        );
        let before: Vec<_> = (0..3)
            .map(|i| resting.rect_of(&Hit::TableRow(i)).unwrap())
            .collect();

        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let after: Vec<_> = (0..3)
            .map(|i| hits.rect_of(&Hit::TableRow(i)).unwrap())
            .collect();
        assert_eq!(
            after[0].height, 1,
            "the selected row edits in place: still one line"
        );
        assert_eq!(before, after, "nothing moved when the row was selected");
    }

    /// The same holds for the ghost row: resting the cursor on it must not
    /// grow it either.
    #[test]
    fn selecting_the_ghost_row_keeps_it_one_line() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState {
            selected: Some(1),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let ghost = hits.rect_of(&Hit::TableRow(1)).unwrap();
        assert_eq!(ghost.height, 1, "the ghost row stays one line");
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("+ Add param"),
            "and keeps its add label: {content}"
        );
    }

    /// A cell edit draws its input on the row's own line, right where the
    /// text was — no block opening under it.
    #[test]
    fn a_cell_edit_draws_in_place_on_the_rows_own_line() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let row = hits.rect_of(&Hit::TableRow(0)).unwrap();
        assert_eq!(row.height, 1, "the edited row stays one line");
        let value = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert_eq!(value.y, row.y, "the input sits on the row's own line");
    }

    /// The enable/disable toggle is state, not an action: it sits at the
    /// row's left edge, before the key, and shows on every row whether the
    /// pointer is there or not.
    #[test]
    fn every_row_carries_its_toggle_at_the_left_edge_without_hover() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState::default();
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let row = hits.rect_of(&Hit::TableRow(0)).unwrap();
        let toggle = hits
            .rect_of(&Hit::TableCheckbox(0))
            .expect("the toggle is up on an untouched row");
        assert_eq!(toggle.x, row.x, "flush with the row's left edge");
        assert_eq!(toggle.y, row.y, "on the row's own line");
        let key = hits.rect_of(&Hit::TableCell { row: 0, col: 0 }).unwrap();
        assert!(
            toggle.right() <= key.x,
            "the toggle sits before the key text: {toggle:?} {key:?}"
        );
    }

    /// The row's context buttons are copy then delete, at the right edge,
    /// with the toggle now away on the left.
    #[test]
    fn the_active_rows_buttons_offer_copy_before_the_trash() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let copy = hits
            .rect_of(&Hit::TableCopy(0))
            .expect("the active row offers copy");
        let trash = hits.rect_of(&Hit::TableDelete(0)).unwrap();
        assert!(
            copy.right() <= trash.x,
            "copy left of trash: {copy:?} {trash:?}"
        );
        let value = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert!(value.x < copy.x, "the buttons sit past the value cell");
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains(crate::glyph::COPY),
            "the copy glyph is painted: {content}"
        );
    }

    /// A compact row reveals copy along with its trash on hover.
    #[test]
    fn hovering_a_compact_row_reveals_its_copy_button() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState::default();
        let hovered = Hit::TableRow(0);
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, Some(&hovered)), &mut hits);
        assert!(
            hits.rect_of(&Hit::TableCopy(0)).is_some(),
            "hover reveals copy"
        );
    }

    #[test]
    fn the_expanded_row_shows_toggle_and_trash_without_hover() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(hits.rect_of(&Hit::TableCheckbox(0)).is_some());
        assert!(hits.rect_of(&Hit::TableDelete(0)).is_some());
        assert!(content.contains("\u{F01B4}"), "trash, not 󰅖: {content}");
        assert!(
            !content.contains('\u{F0156}'),
            "the old 󰅖 delete glyph is gone: {content}"
        );
        assert!(
            !content.contains('\u{2713}') && !content.contains('\u{2717}'),
            "no left check column anywhere: {content}"
        );
    }

    #[test]
    fn a_live_cell_edit_keeps_the_buttons_visible() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        t.click_cell(0, Col::Value, &mut map);
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let toggle = hits
            .rect_of(&Hit::TableCheckbox(0))
            .expect("toggle stays up during a value edit");
        let trash = hits
            .rect_of(&Hit::TableDelete(0))
            .expect("trash stays up during a value edit");
        assert!(
            toggle.x < trash.x,
            "toggle left of trash: {toggle:?} {trash:?}"
        );
    }

    #[test]
    fn hovering_a_cell_lights_its_whole_row() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1"), ("b", "2")]);
        let t = TableEditorState::default();
        let mut probe = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut probe);
        let hovered = Hit::TableCell { row: 1, col: 1 };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, Some(&hovered)), &mut hits);
        let row1 = hits.rect_of(&Hit::TableRow(1)).unwrap();
        assert_eq!(row1.height, 1, "hover never expands a row");
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf.cell((5, row1.y)).unwrap().bg,
            theme.control_hover,
            "the hovered row gets the hover background"
        );
    }

    #[test]
    fn unfocused_pane_demotes_cursor_highlights() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let unfocused = DrawCtx {
            theme: &theme,
            focused: false,
            hovered: None,
            pointer: None,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };

        // Selected data row: the cursor lift is the only thing the cursor
        // paints, so an unfocused pane shows the resting fill.
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &unfocused, &mut hits);
        let buf = terminal.backend().buffer();
        let row = hits.rect_of(&Hit::TableRow(0)).unwrap();
        assert_ne!(
            buf.cell((row.x, row.y)).unwrap().fg,
            theme.accent,
            "no accent cursor bar unfocused"
        );
        assert_eq!(
            buf.cell((row.x + 2, row.y)).unwrap().bg,
            theme.control,
            "resting fill, not the lift"
        );

        // Ghost-row cursor: same rule — resting fill, muted add label.
        let t = TableEditorState {
            selected: Some(map.len()),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &unfocused, &mut hits);
        let buf = terminal.backend().buffer();
        let label = hits.rect_of(&Hit::TableCell { row: 1, col: 0 }).unwrap();
        let cell = buf.cell((label.x, label.y)).unwrap();
        assert_eq!(cell.bg, theme.control, "ghost cursor lift hidden");
        assert_eq!(cell.fg, theme.text_muted, "ghost label stays muted");
    }

    /// The cursor row carries its actions without hover — it is where the
    /// keys land — and stays one line doing it.
    #[test]
    fn the_selected_row_carries_its_actions_without_growing() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        assert_eq!(hits.rect_of(&Hit::TableRow(0)).unwrap().height, 1);
        assert!(hits.rect_of(&Hit::TableCopy(0)).is_some());
        assert!(hits.rect_of(&Hit::TableDelete(0)).is_some());
        assert!(hits.rect_of(&Hit::TableCheckbox(0)).is_some());
    }

    #[test]
    fn editing_the_ghost_row() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        assert!(!t.editing_ghost(map.len()));
        t.selected = Some(1); // the ghost, selected but not typed into
        assert!(!t.editing_ghost(map.len()));
        t.click_cell(1, Col::Key, &mut map);
        assert!(t.editing_ghost(map.len()), "the ghost row is under edit");
    }

    /// The table's height is its row count and nothing else: no selection,
    /// edit or shadow hint can add a line.
    #[test]
    fn table_height_is_header_ghost_edge_and_one_line_per_row() {
        assert_eq!(table_height(0), 3); // header + 0 rows + ghost + edge
        assert_eq!(table_height(3), 6);
    }
}
