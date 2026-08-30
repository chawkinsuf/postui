use super::DrawCtx;
use super::line_input::LineInput;
use super::var_tokens::{VarView, paint_var_tokens};
use crate::hit::{Hit, HitMap};
use crate::paint::{ListRow, RowHighlight, fill, text};
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

/// Column x-offsets (relative to the drawn area's own left edge, i.e.
/// *before* the active row's 1-column accent-bar indent is applied).
/// There is no checkbox column any more: enabled/disabled reads from the
/// row's own styling (dim + struck name), and toggling happens through the
/// hover-revealed buttons at the row's right edge.
pub(crate) struct Columns {
    pub(crate) name_x: u16,
    pub(crate) divider_x: u16,
    pub(crate) value_x: u16,
}

pub(crate) fn columns(x0: u16, width: u16) -> Columns {
    let pad = 2u16.min(width);
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

/// `1 (header) + rows + (2 if a row is expanded, 3 if it also carries a
/// shadow hint line) + 1 (ghost row) + 1 (closing edge)`. `rows` is
/// `map.len()`; the ghost row is the constant `+ 1`. `active` is `Some(_)`
/// whenever exactly one row is drawn expanded — the selected/edited data
/// row, or the ghost row while it is being typed. `active_hint` is whether
/// that expanded row also shows a shadow hint (the Vars tab's "overrides
/// <env>: <value>" line) — ignored when `active` is `None`.
pub fn table_height(rows: usize, active: Option<usize>, active_hint: bool) -> u16 {
    let expanded_extra = active.map_or(0, |_| if active_hint { 3 } else { 2 });
    1 + rows as u16 + expanded_extra + 1 + 1
}

/// The row a hit belongs to, for hover styling: every hit a table row
/// registers (its background, checkbox, cells and delete affordance) lights
/// that one row.
fn hovered_row(ctx: &DrawCtx) -> Option<usize> {
    match ctx.hovered? {
        Hit::TableRow(i) | Hit::TableCheckbox(i) | Hit::TableDelete(i) => Some(*i),
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

    /// Deletes row `i` outright. Only ever called after the user confirmed
    /// the delete (both the `d` key and the `✕` click route through a
    /// confirmation modal first).
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
                let next = match self.selected {
                    None => 0, // nothing selected: Down selects the first row
                    Some(s) => (s + 1).min(map.len()),
                };
                if next == map.len() {
                    // Landing on the ghost row opens its key cell for
                    // typing right away, like the real rows' in-place
                    // editing; an untouched ghost commits to nothing.
                    self.begin_add(map);
                } else {
                    self.selected = Some(next);
                }
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
                // Down onto the ghost row keeps the typing flow open —
                // same auto-edit as arriving there in nav mode. Up never
                // re-opens it, so the cursor can always climb out (on an
                // empty table the ghost is row 0 and Up must escape).
                if ev.code == KeyCode::Down && target >= map.len() {
                    self.begin_add(map);
                } else {
                    self.exit_editing(target, map);
                }
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

    /// The existing row (by map index) currently drawn expanded: the row
    /// being edited, or — when nothing is being edited — the selected row.
    /// Hover never expands a row (it only tints its background), so what's
    /// selected is always the one visibly expanded row. `None` when nothing
    /// in the map is expanded (empty map, or the ghost row under edit —
    /// see [`Self::editing_ghost`]).
    pub fn active_index(&self, map_len: usize) -> Option<usize> {
        match &self.editing {
            Some(edit) => (edit.row < map_len).then_some(edit.row),
            None => self.selected.filter(|s| *s < map_len),
        }
    }

    /// Draws the table as one contiguous painted control: a muted-uppercase
    /// `NAME`/`VALUE` header row on `panel`, a `control` body of compact
    /// 1-line rows (the active row — selected, or being edited — expands to
    /// 3 with a full-row pill and a `✕` delete affordance), the ghost row
    /// (an empty row labelled by `add_label` until it is typed into), and a
    /// closing `▔` edge. Every cell registers a `Hit::TableCell`, so a
    /// click lands straight in that cell's editor.
    /// `shadow` is `Some` only on the Vars tab: `name → "overrides <env>:
    /// <value>"`, already formatted (masked for secrets) by the caller. A
    /// row whose key is present shows that line, dim, under its expanded
    /// form. `None` on Params/Headers, which have no shadowing concept.
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
        let active = self.active_index(map_len);
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
            if active == Some(i) {
                let hint = shadow
                    .and_then(|s| s.get(k))
                    .map(|s| format!("overrides {s}"));
                y = self.draw_active_row(
                    buf,
                    hits,
                    area,
                    y,
                    bottom,
                    i,
                    k,
                    e,
                    true,
                    ctx,
                    hint.as_deref(),
                    vars,
                );
            } else {
                self.draw_plain_row(buf, hits, area, y, i, k, e, ctx, vars);
                y += 1;
            }
        }

        // --- the ghost row -------------------------------------------------
        // Always present, one past the data rows: an empty row that becomes
        // a real entry as soon as its key cell commits non-empty. While it
        // is being typed it draws like any other active row.
        if y < bottom {
            if ghost_editing {
                let entry = Entry {
                    // A value typed before the key shows while the key is
                    // being typed, not just once the row commits.
                    value: self.pending_ghost_value.clone().unwrap_or_default(),
                    enabled: true,
                };
                y = self.draw_active_row(
                    buf, hits, area, y, bottom, map_len, "", &entry, false,
                    // a row that doesn't exist yet has nothing to delete
                    ctx, None, // no shadow hint until the row has a real key
                    vars,
                );
            } else {
                self.draw_ghost_row(buf, hits, area, y, map_len, add_label, ctx);
                y += 1;
            }
        }

        // --- closing edge --------------------------------------------------
        if y < bottom {
            crate::paint::bevel_top(
                buf,
                Rect::new(area.x, y, area.width, 1),
                theme.edge_dark,
                theme.page,
            );
        }
    }

    /// The ghost row at rest: an empty row carrying the `+ Add …` label in
    /// its name cell. Both its cells are clickable — clicking either starts
    /// typing a new row.
    #[allow(clippy::too_many_arguments)]
    fn draw_ghost_row(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        area: Rect,
        y: u16,
        row: usize,
        add_label: &str,
        ctx: &DrawCtx,
    ) {
        let theme = ctx.theme;
        let hovered = hovered_row(ctx) == Some(row);
        // The keyboard cursor can rest here too; it shows with the same
        // lift as hover, held. It's a pure cursor, so it only paints while
        // the pane actually has the keyboard — an unfocused lift would say
        // keys land here.
        let cursor = ctx.focused && self.selected == Some(row);
        let bg = if hovered || cursor {
            theme.control_hover
        } else {
            theme.control
        };
        let fg = if hovered || cursor {
            theme.text
        } else {
            theme.text_muted
        };
        let cols = columns(area.x, area.width);
        fill(buf, Rect::new(area.x, y, area.width, 1), bg);
        text(buf, cols.name_x, y, add_label, fg, bg, false);
        hits.register(Rect::new(area.x, y, area.width, 1), Hit::TableRow(row));
        Self::register_cells(hits, cols_span(&cols, area), y, row);
    }

    /// Paints the row's two right-edge buttons — the enable/disable toggle
    /// (`●` on / `○` off, a 3-cell zone) and the `🗑` delete (a 4-cell
    /// zone: the trash is forced to emoji presentation, which terminals
    /// render two cells wide, so its pill is space+glyph+space = 4) —
    /// flush against the row's right edge with one column of margin. A
    /// directly-hovered button inverts onto accent (error red for the
    /// trash), the same treatment the response pane's copy pills use.
    #[allow(clippy::too_many_arguments)]
    fn draw_row_buttons(
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        right: u16,
        y: u16,
        i: usize,
        enabled: bool,
        bg: ratatui::style::Color,
        hovered: Option<&Hit>,
        theme: &Theme,
    ) {
        let trash_x = right.saturating_sub(5);
        let toggle_x = trash_x.saturating_sub(3);
        let toggle_hit = Hit::TableCheckbox(i);
        let trash_hit = Hit::TableDelete(i);

        // `text` patches styles, so a disabled row's strikethrough would
        // bleed onto the glyphs when the value runs under this zone —
        // scrub it first.
        for x in toggle_x..trash_x + 4 {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().remove_modifier(Modifier::CROSSED_OUT));
            }
        }

        let (glyph, state_fg) = if enabled {
            (" \u{25CF} ", theme.success)
        } else {
            (" \u{25CB} ", theme.text_muted)
        };
        let (tfg, tbg) = if hovered == Some(&toggle_hit) {
            (theme.on_accent, theme.accent)
        } else {
            (state_fg, bg)
        };
        text(buf, toggle_x, y, glyph, tfg, tbg, false);

        let (dfg, dbg) = if hovered == Some(&trash_hit) {
            (theme.on_accent, theme.error)
        } else {
            (theme.text_muted, bg)
        };
        // VS16 (`\u{FE0F}`) pins the trash to emoji presentation: the
        // terminal was already drawing it two cells wide, and the selector
        // makes unicode-width agree, so the padding actually centers it.
        text(buf, trash_x, y, " \u{1F5D1}\u{FE0F} ", dfg, dbg, false);

        hits.register(Rect::new(toggle_x, y, 3, 1), toggle_hit);
        hits.register(Rect::new(trash_x, y, 4, 1), trash_hit);
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

    /// Draws row `i` at its compact 1-line height. Returns nothing; the
    /// caller advances `y` itself by 1.
    #[allow(clippy::too_many_arguments)]
    fn draw_plain_row(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        area: Rect,
        y: u16,
        i: usize,
        key: &str,
        entry: &Entry,
        ctx: &DrawCtx,
        vars: &VarView,
    ) {
        let theme = ctx.theme;
        let hovered = hovered_row(ctx) == Some(i);
        let cols = columns(area.x, area.width);
        let bg = if hovered {
            theme.control_hover
        } else {
            theme.control
        };
        fill(buf, Rect::new(area.x, y, area.width, 1), bg);
        let fg = if entry.enabled {
            theme.text
        } else {
            theme.text_muted
        };
        text(buf, cols.name_x, y, key, fg, bg, false);
        text(buf, cols.value_x, y, &entry.value, fg, bg, false);
        if !entry.enabled {
            Self::strike_cells(buf, cols.name_x, y, key.chars().count() as u16);
            Self::strike_cells(buf, cols.value_x, y, entry.value.chars().count() as u16);
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
        Self::register_cells(hits, cols_span(&cols, area), y, i);
        paint_cell_tokens(buf, hits, &cols, area, y, key, &entry.value, vars, theme);
        // Hover-revealed toggle/delete, painted (and registered) last so
        // they win over the value cell underneath.
        if hovered && area.width >= 10 {
            Self::draw_row_buttons(
                buf,
                hits,
                area.right(),
                y,
                i,
                entry.enabled,
                bg,
                ctx.hovered,
                theme,
            );
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

    /// Draws row `i` expanded to 3 lines (pad/text/pad) — 4 (pad/text/hint/
    /// pad) when `hint` is `Some`, adding a dim shadow line ("overrides qa:
    /// 1001") right under the value row. `show_delete` gates the `✕`
    /// affordance (the ghost row has nothing to delete yet). Returns the
    /// next `y`.
    ///
    /// The expansion itself persists when the pane loses focus (it feeds
    /// `table_geometry`, so collapsing would shift the layout every focus
    /// change, and its affordances stay mouse-usable) — but the cursor
    /// styling demotes: no accent bar, resting fill instead of the lift.
    /// The text row's own fill/bar comes from `ListRow` (`Selected` while
    /// focused, `Hover` otherwise — which, base and target both being
    /// `theme.control`, paints as a flat `theme.control` regardless of
    /// `hover_t`, matching the old resting-fill demotion exactly); the pad
    /// rows above/below just carry that same resolved fill flat, with no
    /// half-block cap glyph.
    #[allow(clippy::too_many_arguments)]
    fn draw_active_row(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        hits: &mut HitMap,
        area: Rect,
        y: u16,
        bottom: u16,
        i: usize,
        key: &str,
        entry: &Entry,
        show_delete: bool,
        ctx: &DrawCtx,
        hint: Option<&str>,
        vars: &VarView,
    ) -> u16 {
        let theme = ctx.theme;
        let text_row = y + 1;
        let highlight = if ctx.focused {
            RowHighlight::Selected
        } else {
            RowHighlight::Hover
        };
        let hover_t = ctx.hover_t();
        let bg = ListRow::resolve_fill(theme, highlight, theme.control, hover_t);
        if y < bottom {
            fill(buf, Rect::new(area.x, y, area.width, 1), bg);
        }
        ListRow {
            highlight,
            zebra: None,
        }
        .paint(
            buf,
            text_row,
            area.x,
            area.width,
            theme.control,
            hover_t,
            theme,
        );
        if text_row + 1 < bottom {
            fill(buf, Rect::new(area.x, text_row + 1, area.width, 1), bg);
        }

        // The accent bar occupies column `area.x`; cell content is indented
        // one column past it.
        let content_x = area.x + 1;
        let content_w = area.width.saturating_sub(1);
        let cols = columns(content_x, content_w);
        let fg = if entry.enabled {
            theme.text
        } else {
            theme.text_muted
        };

        let editing_col = self.editing.as_ref().filter(|e| e.row == i).map(|e| e.col);
        // The toggle/delete buttons stay up for the whole active row, cell
        // edits included, so a value edit's input must stop short of their
        // 8-cell zone (toggle 3 + trash 4 + right margin 1) instead of
        // running under it. The ghost row has no buttons, so its input
        // keeps the full width.
        let show_buttons = show_delete && area.width >= 10;
        let value_right = if show_buttons {
            (area.x + area.width).saturating_sub(8)
        } else {
            area.x + area.width
        };

        match editing_col {
            Some(Col::Key) => {
                let edit = self.editing.as_ref().expect("editing_col implies editing");
                let mut line = edit.input.draw_line(true, theme);
                line.style = Style::default().bg(bg).patch(line.style);
                buf.set_line(
                    cols.name_x,
                    text_row,
                    &line,
                    cols.divider_x.saturating_sub(cols.name_x),
                );
                text(buf, cols.value_x, text_row, &entry.value, fg, bg, false);
            }
            Some(Col::Value) => {
                text(buf, cols.name_x, text_row, key, fg, bg, false);
                let edit = self.editing.as_ref().expect("editing_col implies editing");
                let mut line = edit.input.draw_line(true, theme);
                line.style = Style::default().bg(bg).patch(line.style);
                let value_w = value_right.saturating_sub(cols.value_x);
                buf.set_line(cols.value_x, text_row, &line, value_w);
            }
            None => {
                text(buf, cols.name_x, text_row, key, fg, bg, false);
                text(buf, cols.value_x, text_row, &entry.value, fg, bg, false);
            }
        }
        if !entry.enabled {
            if editing_col != Some(Col::Key) {
                Self::strike_cells(buf, cols.name_x, text_row, key.chars().count() as u16);
            }
            if editing_col != Some(Col::Value) {
                Self::strike_cells(
                    buf,
                    cols.value_x,
                    text_row,
                    entry.value.chars().count() as u16,
                );
            }
        }

        let row_height = if hint.is_some() { 4 } else { 3 };
        hits.register(
            Rect::new(area.x, y, area.width, row_height),
            Hit::TableRow(i),
        );
        Self::register_cells(hits, cols_span(&cols, area), text_row, i);
        // Only the cells drawn as plain text get token treatment: a cell
        // under edit is showing a live `LineInput` (caret and all), and
        // registering a `VarToken` over it would turn the next click into a
        // picker instead of a caret move.
        paint_cell_tokens(
            buf,
            hits,
            &cols,
            area,
            text_row,
            if editing_col == Some(Col::Key) {
                ""
            } else {
                key
            },
            if editing_col == Some(Col::Value) {
                ""
            } else {
                entry.value.as_str()
            },
            vars,
            theme,
        );
        // The expanded row keeps its toggle/delete visible without hover —
        // it is the active row — including while a cell edit is live (the
        // value input was clipped to `value_right` above so they never
        // collide). `show_delete` gates both: the ghost row has nothing to
        // toggle or delete yet.
        if show_buttons {
            Self::draw_row_buttons(
                buf,
                hits,
                area.x + area.width,
                text_row,
                i,
                entry.enabled,
                bg,
                ctx.hovered,
                theme,
            );
        }

        // A shadow hint replaces the pad-bottom row already flat-filled
        // above (`text_row + 1`) with a dim "overrides <env>: <value>" line
        // on the same fill, then adds one more flat row to close the block —
        // one row taller overall (4 instead of 3).
        if let Some(hint) = hint {
            let hint_row = text_row + 1;
            if hint_row < bottom {
                fill(buf, Rect::new(area.x, hint_row, area.width, 1), bg);
                text(buf, content_x, hint_row, hint, theme.text_muted, bg, false);
            }
            let closing_row = hint_row + 1;
            if closing_row < bottom {
                fill(buf, Rect::new(area.x, closing_row, area.width, 1), bg);
            }
            return (closing_row + 1).min(bottom);
        }

        (y + 3).min(bottom)
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
    area: Rect,
    y: u16,
    key: &str,
    value: &str,
    vars: &VarView,
    theme: &Theme,
) {
    let (name_x, name_w, value_x, value_w) = cols_span(cols, area);
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
/// the name cell stops at the divider, the value cell runs to the drawn
/// area's right edge.
fn cols_span(cols: &Columns, area: Rect) -> (u16, u16, u16, u16) {
    let name_w = cols.divider_x.saturating_sub(cols.name_x);
    let value_w = area.right().saturating_sub(cols.value_x);
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

    // --- ghost row auto-edit ----------------------------------------------

    /// Arrowing onto the ghost row opens its key-cell edit right away —
    /// the same immediate typing the real rows offer — instead of a bare
    /// selection that still needs Enter.
    #[test]
    fn down_onto_the_ghost_row_opens_the_add_edit() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Down), &mut map); // row 0
        assert!(t.editing.is_none(), "a real row: selection only");
        t.handle_key(key(KeyCode::Down), &mut map); // ghost row
        assert!(t.editing_ghost(map.len()), "the ghost opened for typing");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!(edit.col, Col::Key);
        assert_eq!(edit.input.text(), "");
    }

    /// Same when the cursor carries out of a live cell edit: Down from the
    /// last real row's cell lands on the ghost already editing.
    #[test]
    fn down_out_of_a_cell_edit_onto_the_ghost_row_opens_the_add_edit() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        t.handle_key(key(KeyCode::Down), &mut map);
        assert!(t.editing_ghost(map.len()), "the ghost opened for typing");
        assert_eq!(map.len(), 1, "the real row committed unchanged");
    }

    /// Leaving the auto-opened ghost untouched creates nothing.
    #[test]
    fn leaving_the_auto_opened_ghost_untouched_saves_no_row() {
        let mut map = map_of(&[("page", "2")]);
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Down), &mut map);
        t.handle_key(key(KeyCode::Down), &mut map); // ghost, editing
        t.handle_key(key(KeyCode::Up), &mut map); // straight back out
        assert!(t.editing.is_none());
        assert_eq!(map.len(), 1, "no empty header appeared");
        assert_eq!(t.selected, Some(0), "cursor parked back on the real row");
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
        assert!(
            t.editing_ghost(map.len()),
            "and opens for typing on arrival"
        );
        assert!(
            t.handle_key(key(KeyCode::Down), &mut map).consumed,
            "clamped at the ghost row, still consumed"
        );
        assert_eq!(t.selected, Some(2));
        t.handle_key(key(KeyCode::Up), &mut map);
        assert!(t.editing.is_none(), "Up leaves the untouched ghost edit");
        assert_eq!(t.selected, Some(1));
        assert_eq!(map.len(), 2, "no empty row appeared");
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
    fn the_edited_row_expands_and_shows_its_input() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let row1 = hits.rect_of(&Hit::TableRow(1)).unwrap();
        assert_eq!(row1, Rect::new(0, 2, 40, 3), "the edited row expands");
        assert_eq!(
            hits.rect_of(&Hit::TableRow(2)).unwrap().height,
            1,
            "other rows stay compact"
        );
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("29"), "the live input text: {content}");
        // The row's toggle/trash buttons stay up through a live cell edit
        // (the input is clipped short of their zone).
        assert!(hits.rect_of(&Hit::TableDelete(1)).is_some());
        // The expanded row's own cells are registered on its text line.
        let k = hits.rect_of(&Hit::TableCell { row: 1, col: 0 }).unwrap();
        assert_eq!(k.y, row1.y + 1);
    }

    #[test]
    fn the_ghost_row_expands_while_it_is_being_typed() {
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
        assert_eq!(ghost.height, 3, "the ghost row expands under edit");
        assert!(
            hits.rect_of(&Hit::TableDelete(1)).is_none(),
            "a row that doesn't exist yet has nothing to delete"
        );
    }

    #[test]
    fn compact_rows_reveal_toggle_and_trash_only_on_hover() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1"), ("b", "2")]);
        let t = TableEditorState::default();
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        assert!(
            hits.rect_of(&Hit::TableCheckbox(0)).is_none(),
            "no buttons on an unhovered row"
        );
        assert!(hits.rect_of(&Hit::TableDelete(0)).is_none());

        let hovered = Hit::TableRow(0);
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, Some(&hovered)), &mut hits);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("\u{1F5D1}"),
            "the delete button is a trash can: {content}"
        );
        let toggle = hits.rect_of(&Hit::TableCheckbox(0)).expect("toggle hit");
        let trash = hits.rect_of(&Hit::TableDelete(0)).expect("delete hit");
        assert!(
            toggle.width >= 3 && trash.width >= 3,
            "buttons get comfortable click targets: {toggle:?} {trash:?}"
        );
        assert!(toggle.x < trash.x, "toggle left of trash");
        assert!(
            hits.rect_of(&Hit::TableCheckbox(1)).is_none(),
            "only the hovered row shows buttons"
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
        assert!(content.contains("\u{1F5D1}"), "trash, not ✕: {content}");
        assert!(
            !content.contains('\u{2715}'),
            "the old ✕ delete glyph is gone: {content}"
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
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
        };

        // Selected data row: expansion persists (it feeds table_geometry),
        // but the cursor styling demotes — no accent bar, resting fill.
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &unfocused, &mut hits);
        let buf = terminal.backend().buffer();
        let row = hits.rect_of(&Hit::TableRow(0)).unwrap();
        assert_eq!(row.height, 3, "expanded row survives losing pane focus");
        assert_ne!(
            buf.cell((row.x, row.y + 1)).unwrap().fg,
            theme.accent,
            "no accent cursor bar unfocused"
        );
        assert_eq!(
            buf.cell((row.x + 2, row.y + 1)).unwrap().bg,
            theme.control,
            "resting fill, not the lift"
        );

        // Ghost-row cursor: a pure cursor, so it vanishes entirely.
        let t = TableEditorState {
            selected: Some(map.len()),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &unfocused, &mut hits);
        let buf = terminal.backend().buffer();
        let ghost = hits.rect_of(&Hit::TableCell { row: 1, col: 0 }).unwrap();
        let cell = buf.cell((ghost.x, ghost.y)).unwrap();
        assert_eq!(cell.bg, theme.control, "ghost cursor lift hidden");
        assert_eq!(cell.fg, theme.text_muted, "ghost label stays muted");
    }

    #[test]
    fn selected_row_is_the_expanded_one_and_carries_the_delete_affordance() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        assert_eq!(hits.rect_of(&Hit::TableRow(0)).unwrap().height, 3);
        assert!(hits.rect_of(&Hit::TableDelete(0)).is_some());
        assert!(hits.rect_of(&Hit::TableCheckbox(0)).is_some());
    }

    #[test]
    fn active_index_and_editing_the_ghost_row() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        assert_eq!(t.active_index(map.len()), None);
        t.selected = Some(0);
        assert_eq!(t.active_index(map.len()), Some(0));
        t.selected = Some(1); // the ghost
        assert_eq!(
            t.active_index(map.len()),
            None,
            "the ghost row expands nothing in the map"
        );
        assert!(!t.editing_ghost(map.len()));
        t.click_cell(1, Col::Key, &mut map);
        assert_eq!(t.active_index(map.len()), None);
        assert!(t.editing_ghost(map.len()), "the ghost row is under edit");
    }

    #[test]
    fn table_height_accounts_for_header_ghost_edge_and_expansion() {
        assert_eq!(table_height(0, None, false), 3); // header + 0 rows + ghost + edge
        assert_eq!(table_height(3, None, false), 6);
        assert_eq!(table_height(3, Some(1), false), 8); // + 2 for the expanded row
        assert_eq!(
            table_height(3, Some(1), true),
            9,
            "+ 3 for the expanded row plus its shadow hint line"
        );
    }
}
