use super::DrawCtx;
use super::line_input::LineInput;
use super::var_tokens::{VarView, paint_field_tokens};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Col {
    #[default]
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
/// Esc commits like Enter; the cell's history is the input's own.
#[derive(Debug, Clone)]
pub struct CellEdit {
    /// Index into the map — or `map.len()`, the always-present ghost row
    /// that becomes a real entry the moment its key cell commits non-empty.
    pub row: usize,
    pub col: Col,
    pub input: LineInput,
}

/// What committing one cell did ([`TableEditorState::commit_cell`]). It
/// names two rows because a `+ Add` name that is already taken makes none:
/// the row it names is the existing one, while the caret was on `+ Add`.
///
/// `here` equals `row` except on `+ Add`, so a caller could work it out
/// itself, but only from the map as it was before the commit: each mover
/// would have to remember to read it first. Shift-Tab (task #111) and
/// Up/Down (task #176) were both wrong for want of that, so the commit,
/// which sees both rows, says both.
struct Committed {
    /// The row the edit resolved to: the row written, or for a `+ Add` name
    /// that is already taken, the existing row it matched (Tab opens that
    /// row's value, Enter and Esc park the cursor on it). `None` when
    /// nothing was written: an empty `+ Add` name, or a value cell left
    /// past the end by a map that shrank under it.
    row: Option<usize>,
    /// Where the caret's row is now, which the moves step from (Up/Down,
    /// Shift-Tab; tasks #111, #176). For `+ Add` that is its index before
    /// the commit, whatever the commit did: a new row lands there, an empty
    /// name is discarded there, a taken name leaves it there. For a real
    /// row it is `row`: a rename onto a taken name merges into that row.
    here: usize,
    warning: Option<String>,
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
        Self { consumed: true, ..Self::default() }
    }

    fn not_consumed() -> Self {
        Self::default()
    }

    fn maybe_warn(warning: Option<String>) -> Self {
        Self { consumed: true, warning, ..Self::default() }
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
    let name_w = (remaining / 3).max(4).min(remaining.saturating_sub(2).max(4));
    Columns { name_x: x0 + pad, divider_x: x0 + pad + name_w, value_x: x0 + pad + name_w + 1 }
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

/// What one row says about itself, given to [`TableEditorState::draw`] per
/// row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowNote {
    /// A dim note trailing the value on its line, such as the Vars tab's
    /// `overrides …`.
    Dim(String),
    /// The row is kept but never used (a header row the client owns,
    /// named here in its usual spelling): an enabled row's text draws
    /// struck through in the error colour, and nothing trails it. The
    /// footer says why instead (task #59), from the name carried here.
    Unsent(&'static str),
}

/// A stop the table's keyboard cursor sits on (task #56): a request row,
/// the `+ Add` row under them, or a row of the read-only section the tab
/// draws under the table (the Headers tab's auto rows, the Vars tab's
/// referenced variables). Each reader matches the kind it acts on, so a
/// section row can never be taken for a request row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Request row `i`.
    Row(usize),
    /// The `+ Add` row, one past the request rows.
    Add,
    /// Row `j` of the section under the table.
    Section(usize),
}

impl Stop {
    /// The stop a row index names, where `len` (one past the rows) is the
    /// `+ Add` row: how a click's row hit, or a cell edit's row, becomes a
    /// stop.
    pub fn of_row(i: usize, len: usize) -> Stop {
        if i < len { Stop::Row(i) } else { Stop::Add }
    }

    /// The stop's place in the order the keys walk (rows, `+ Add`, then
    /// the section), for `len` rows: the one place that order is written
    /// down. The moves step and page through it; [`Stop::at`] reads it
    /// back.
    pub(crate) fn ordinal(self, len: usize) -> usize {
        match self {
            Stop::Row(i) => i,
            Stop::Add => len,
            Stop::Section(j) => len + 1 + j,
        }
    }

    /// The stop at place `ord` of that order.
    pub(crate) fn at(ord: usize, len: usize) -> Stop {
        match ord.checked_sub(len) {
            None => Stop::Row(ord),
            Some(0) => Stop::Add,
            Some(n) => Stop::Section(n - 1),
        }
    }

    /// The stop this one names once the table has `len` rows and the
    /// section `section_len`: a row or section row the rows went from
    /// under comes back to the last one still there (`+ Add` when none is),
    /// as the Variable Manager grid's cursor does.
    pub(crate) fn clamp(self, len: usize, section_len: usize) -> Stop {
        match self {
            Stop::Row(i) if i >= len => len.checked_sub(1).map_or(Stop::Add, Stop::Row),
            Stop::Section(j) if j >= section_len => {
                section_len.checked_sub(1).map_or(Stop::Add, Stop::Section)
            }
            stop => stop,
        }
    }
}

/// Shared cursor/edit state for a key/value table (Params, Headers or Vars).
/// One instance is reused across the tabs; the caller passes in whichever
/// `IndexMap` is currently active.
#[derive(Debug, Default)]
pub struct TableEditorState {
    /// The stop under the keyboard cursor. `None` means nothing is
    /// selected — every row draws compact and the row-level keys
    /// (Enter/Space/d) are inert until Down/j or a click lands somewhere.
    /// Private: read it with [`Self::selected`], write it with
    /// [`Self::set_selected`], the one place a cursor move is known.
    selected: Option<Stop>,
    /// The cursor's column: the cell Enter opens. ←/→ (h/l) move it in
    /// nav mode; it follows whatever cell was last open, so Esc then
    /// Enter reopens the same cell. It persists across row moves.
    pub col: Col,
    pub editing: Option<CellEdit>,
    /// The tab content's first drawn line (task #56): the table's lines
    /// (header, rows, `+ Add`, closing edge) and then the read-only
    /// section under it scroll as one column. The wheel and the scrollbar
    /// move it, and every frame otherwise keeps the cursor in sight
    /// (`Editor::settle_table_scroll`, spec 2026-10-25 S22). Where the user
    /// scrolled can't be worked out from anything else, so it is kept;
    /// nothing derived from the cursor or the layout is.
    pub scroll: usize,
    /// The arrows wheel or scrollbar scrolled the view away from the
    /// cursor (S23): while set, frames keep `scroll` as it is. Private, as
    /// `selected` is, so its writers are the ones named here. `App` sets it
    /// ([`Self::mark_scrolled_away`]) when a scroll moves the view, and
    /// [`Self::set_selected`] clears it
    /// when the cursor moves, so whatever moves the cursor brings the view
    /// back and nothing else does. A move key that lands on the stop it is
    /// on clears it too ([`Self::set_selected_showing`]): the key asked for
    /// the cursor, so it finds it. Both go through one write
    /// ([`Self::write_cursor`]). A key that acts on the
    /// cursor's row without moving the cursor (`y`, Space, Delete) leaves
    /// the view where the wheel put it: the cost S6 and S23 accept for a
    /// rule that reads only whether the cursor moved, not which key came
    /// in. Delete's toast names the row it took, and undo brings it back.
    /// The vim profile never sets it: its wheel moves the cursor instead
    /// (S24).
    scrolled_away: bool,
    /// Lines the tab content had on screen as of the last draw: the
    /// scrollbar's viewport. 0 while no table tab is drawn.
    pub viewport: usize,
}

/// The cell that opens for `col` on `row`: `col` itself, except on the
/// ghost row (`row == map_len`), where it is always the name (task #68).
/// A value needs a row's name to belong to, and the ghost has none yet, so
/// its value cell never opens; a value typed there first would have
/// nowhere in the request to live (no dirty gate, save or load could see
/// it). The cost is value-first entry: the name comes first. The Variable
/// Manager grid's ghost row redirects the same way (`start_cell_edit`),
/// but moves its cursor to the name as well; this table keeps the cursor
/// column asked for, since its column persists across row moves (see
/// `start_edit`), and only draws the ghost's lift on the name.
fn open_col(row: usize, col: Col, map_len: usize) -> Col {
    if row >= map_len { Col::Key } else { col }
}

impl TableEditorState {
    /// The stop under the keyboard cursor.
    pub fn selected(&self) -> Option<Stop> {
        self.selected
    }

    /// Moves the keyboard cursor to `stop`: the one way the cursor moves
    /// ([`Self::clamp_cursor`] only re-places a stale stop on the one drawn
    /// lit). A write that changes the stop is a cursor move, so a view
    /// the arrows wheel scrolled away comes back to the cursor (spec
    /// 2026-10-25 S23), known here at the moment of the move rather than
    /// guessed from the input that caused it.
    pub fn set_selected(&mut self, stop: Option<Stop>) {
        self.write_cursor(stop, false);
    }

    /// A move key's write: [`Self::set_selected`], and the view comes back
    /// even when the key lands on the stop the cursor is on (End on the
    /// last stop, ↓ at the end). The key asked for the cursor, so it finds
    /// it (S23).
    fn set_selected_showing(&mut self, stop: Option<Stop>) {
        self.write_cursor(stop, true);
    }

    /// The cursor's write, and so the one place a cursor move clears
    /// `scrolled_away`: on a stop that changes, or when `show` asks.
    fn write_cursor(&mut self, stop: Option<Stop>, show: bool) {
        if show || stop != self.selected {
            self.scrolled_away = false;
        }
        self.selected = stop;
    }

    /// Whether the arrows wheel or bar has scrolled the view away from the
    /// cursor (S23).
    pub fn scrolled_away(&self) -> bool {
        self.scrolled_away
    }

    /// The arrows wheel or bar moved the view: frames leave it where it is
    /// until the cursor moves (S23).
    pub fn mark_scrolled_away(&mut self) {
        self.scrolled_away = true;
    }

    /// Resets cursor/edit state; used when switching tabs so a selection
    /// index from one map can't be stale (and panic) against the other.
    pub fn reset(&mut self) {
        self.set_selected(None);
        self.col = Col::Key;
        self.editing = None;
        self.reset_view();
    }

    /// Puts the view back at the top, attached to the cursor: the view's
    /// share of [`Self::reset`], which a load runs on its own (spec
    /// 2026-10-25 S9).
    pub fn reset_view(&mut self) {
        self.scroll = 0;
        self.scrolled_away = false;
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
    /// caret at the end; on the ghost row that is always its name
    /// ([`open_col`]). Every way a cell opens (click, Enter, Tab, the add
    /// verb, and vim's keys, which reach the table as these) comes through
    /// here. Any previous edit must already have been committed.
    fn start_edit(&mut self, row: usize, col: Col, map: &IndexMap<String, Entry>) {
        let row = row.min(map.len());
        // The cursor column is the one asked for, even when the ghost row
        // opens its name instead: the column persists across row moves, so
        // passing through the ghost leaves the column the user chose on
        // the real rows. The ghost draws its lift on the name whatever the
        // column (`draw_row`), so Esc then Enter still reopens what was open.
        self.col = col;
        let col = open_col(row, col, map.len());
        let text = Self::cell_text(map, row, col);
        self.set_selected(Some(Stop::of_row(row, map.len())));
        self.editing = Some(CellEdit { row, col, input: LineInput::new(&text) });
    }

    /// Leaves editing with the cursor parked on `row` (`+ Add` past the
    /// rows).
    fn exit_editing(&mut self, row: usize, map: &IndexMap<String, Entry>) {
        self.editing = None;
        self.set_selected(Some(Stop::of_row(row, map.len())));
    }

    /// Click entry point: commits whatever was being edited (surfacing its
    /// warning), then begins editing `row`/`col` with the caret at the end.
    /// `row == map.len()` targets the ghost row. Clicking the cell already
    /// under edit is inert, so a double click is exactly one edit session.
    ///
    /// A click on the ghost row's value cell opens its name ([`open_col`]),
    /// so the inert check compares the clicked cell, not the redirected
    /// one, on purpose: with a name typed, a click on that value cell
    /// commits the name (the new row lands at the ghost's index) and opens
    /// the new row's value, which is name-then-value by mouse. With the
    /// name still empty, the click reopens the empty name: no change, so a
    /// double click there is two sessions of the same empty name rather
    /// than one. That is the cost of keeping name-then-value by mouse.
    pub fn click_cell(
        &mut self,
        row: usize,
        col: Col,
        map: &mut IndexMap<String, Entry>,
    ) -> TableOutcome {
        let row = row.min(map.len());
        if self.editing.as_ref().is_some_and(|e| e.row == row && e.col == col) {
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
        let Committed { row, here, warning } = self.commit_cell(map, &edit);
        self.set_selected(Some(Stop::of_row(row.unwrap_or(here), map.len())));
        TableOutcome::maybe_warn(warning)
    }

    /// Writes one cell into the map, and says which row that names and
    /// where the caret's row is now ([`Committed`]), plus any warning to
    /// surface.
    fn commit_cell(&mut self, map: &mut IndexMap<String, Entry>, edit: &CellEdit) -> Committed {
        let typed = edit.input.text().to_string();
        if let Some((key, e)) = map.get_index_mut(edit.row) {
            let (row, warning) = match edit.col {
                Col::Value => {
                    e.value = typed;
                    (edit.row, None)
                }
                Col::Key => {
                    let (orig, entry) = (key.clone(), e.clone());
                    Self::commit_key(map, edit.row, orig, entry, typed)
                }
            };
            return Committed { row: Some(row), here: row, warning };
        }
        // Past the rows is `+ Add`, at `map.len()`: `edit.row` itself can be
        // further on when the map shrank under the open cell.
        let here = map.len();
        // The ghost row: only a non-empty key can make it a real row (its
        // value cell never opens, see `open_col`). A name that is taken
        // makes none; the warning says only that, since what is open after
        // the commit depends on what closed the cell (Tab opens the
        // existing row's value, a click away opens nothing there). A value
        // edit can only land here if the map shrank under an open value
        // cell, leaving its row index one past the end; it names no row,
        // so it writes nothing, as it never made a row before either.
        if edit.col != Col::Key || typed.trim().is_empty() {
            return Committed { row: None, here, warning: None };
        }
        if let Some(other) = map.get_index_of(&typed) {
            let warning = format!("'{typed}' already exists \u{2014} no row added");
            return Committed { row: Some(other), here, warning: Some(warning) };
        }
        map.insert(typed, Entry { value: String::new(), enabled: true });
        Committed { row: Some(here), here, warning: None }
    }

    /// Renames row `idx` (named `orig`, holding `entry`) to `new_key`,
    /// keeping its position, value and enabled flag. Renaming onto a key
    /// that already exists collapses the two rows (the target keeps its
    /// slot and takes this row's value) and warns; blanking a key is
    /// refused with a warning. Returns where the row is now.
    fn commit_key(
        map: &mut IndexMap<String, Entry>,
        idx: usize,
        orig: String,
        entry: Entry,
        new_key: String,
    ) -> (usize, Option<String>) {
        if new_key == orig {
            return (idx, None);
        }
        if new_key.trim().is_empty() {
            return (idx, Some(format!("a row needs a name — kept '{orig}'")));
        }
        if let Some(other_idx) = map.get_index_of(&new_key) {
            map.shift_remove_index(idx);
            let adjusted = if other_idx > idx { other_idx - 1 } else { other_idx };
            if let Some((_, e)) = map.get_index_mut(adjusted) {
                e.value = entry.value;
            }
            return (
                adjusted,
                Some(format!("duplicate key '{new_key}' replaced the existing value")),
            );
        }
        map.shift_remove_index(idx);
        map.shift_insert(idx, new_key, entry);
        (idx, None)
    }

    /// Begins editing the selected row's cursor cell (the `Enter` path, and
    /// the app-side flows that seed a cell edit). A no-op with nothing
    /// selected, or with the cursor on a row of the read-only section
    /// under the table (past `+ Add`): no cell opens there (task #56).
    pub fn begin_edit_selected(&mut self, map: &IndexMap<String, Entry>) {
        let row = match self.selected {
            Some(Stop::Row(i)) => i,
            Some(Stop::Add) => map.len(),
            Some(Stop::Section(_)) | None => return,
        };
        self.start_edit(row, self.col, map);
    }

    /// Starts a brand-new row: the ghost row's key cell, exactly like
    /// clicking it. Shared by the `+ Add` click and `Action::TableAddRow`.
    pub fn begin_add(&mut self, map: &IndexMap<String, Entry>) {
        self.start_edit(map.len(), Col::Key, map);
    }

    /// Deletes row `i` outright — no confirm gate (the `d`/`dd` rows reach
    /// it through `Action::DeleteTableRow`, and the `󰅖` click dispatches the
    /// same action): the deletion is an ordinary editor undo step.
    pub fn delete_row(&mut self, map: &mut IndexMap<String, Entry>, i: usize) {
        self.delete_rows(map, i, 1);
    }

    /// Deletes up to `n` rows from `start` down as one change (piece 4
    /// spec §5.3: a counted `dd`); a count past the end is clamped. The
    /// cursor lands on the row that moved up into `start`, or, when the run
    /// reached the end, on the new last row, as Vim's `dd` does, in both
    /// profiles (the user, 2026-10-11). Only an emptied table lands on the
    /// add row. Returns how many rows went.
    pub fn delete_rows(
        &mut self,
        map: &mut IndexMap<String, Entry>,
        start: usize,
        n: usize,
    ) -> usize {
        if start >= map.len() || n == 0 {
            return 0;
        }
        let end = start.saturating_add(n).min(map.len());
        self.editing = None;
        map.drain(start..end);
        // An emptied table has only `+ Add` left (`of_row(0, 0)`).
        self.set_selected(Some(Stop::of_row(start.min(map.len().saturating_sub(1)), map.len())));
        end - start
    }

    /// Whether the ghost row (not any existing row) is the one under edit.
    pub fn editing_ghost(&self, map_len: usize) -> bool {
        self.editing.as_ref().is_some_and(|e| e.row >= map_len)
    }

    /// Clamps a stop the rows went from under (the map or the section
    /// shrank before a frame drew it) onto the last one still there, the
    /// stop that is drawn lit ([`Stop::clamp`]). Run where the table's keys
    /// come in, before anything reads the cursor (spec 2026-10-25 S25).
    ///
    /// It writes the stop past [`Self::set_selected`] on purpose: the
    /// clamped stop is the one already drawn lit, so nothing the user sees
    /// moves, and a view the arrows wheel scrolled away stays where the
    /// wheel put it (S23). Only a cursor move brings the view back.
    pub fn clamp_cursor(&mut self, len: usize, tail: usize) {
        self.selected = self.selected.map(|s| s.clamp(len, tail));
    }

    /// [`Self::handle_key_with_tail`] for a table with no section under it:
    /// a closed table's keys treat a section stop as stale and clamp it onto
    /// `+ Add`. The editor's table tabs always pass their section's length;
    /// this is for a cell's own keys (`commit_table_edit_with_enter`) and
    /// tables that draw no section.
    pub fn handle_key(&mut self, ev: KeyEvent, map: &mut IndexMap<String, Entry>) -> TableOutcome {
        self.handle_key_with_tail(ev, map, 0)
    }

    /// [`Self::handle_key`] with `tail` read-only stops after `+ Add`: the
    /// rows of the section the tab draws under the table (the Headers
    /// tab's auto rows, the Vars tab's referenced rows; task #56), each a
    /// [`Stop::Section`]. The cursor moves onto them like any row, and the
    /// page keys reach them, but nothing here acts on one: no cell opens,
    /// and Enter, Space, ←/→ and Delete stay unconsumed (spec 2026-10-25
    /// S21).
    pub fn handle_key_with_tail(
        &mut self,
        ev: KeyEvent,
        map: &mut IndexMap<String, Entry>,
        tail: usize,
    ) -> TableOutcome {
        match self.editing.take() {
            Some(edit) => self.handle_editing_key(ev, map, edit),
            None => self.handle_nav_key(ev, map, tail),
        }
    }

    fn handle_nav_key(
        &mut self,
        ev: KeyEvent,
        map: &mut IndexMap<String, Entry>,
        tail: usize,
    ) -> TableOutcome {
        // A named key acts bare only: any other chord is unconsumed rather
        // than acting as the bare key (piece 4 spec §5.1).
        let plain = ev.modifiers.is_empty();
        let len = map.len();
        // The stops, in the order the keys walk them: the data rows, the
        // ghost row (so the keyboard can reach it the same way the mouse
        // can, and an empty table still has that one stop to land on), then
        // the section's rows. Moves step through their places in it.
        let last = len + tail;
        // Every arm below (the moves, Enter, Space, Delete, ←/→) acts on the
        // stop that is drawn lit (spec 2026-10-25 S25).
        self.clamp_cursor(len, tail);
        let place = self.selected.map(|s| s.ordinal(len));
        // A move key asks for the cursor even when it lands on the stop it
        // is on (End on the last stop, ↓ at the end), so it brings a view
        // the wheel scrolled away back as any move does (S23): otherwise
        // the key meant to find the cursor would do nothing.
        let go = |this: &mut Self, ord: usize| {
            this.set_selected_showing(Some(Stop::at(ord.min(last), len)));
            TableOutcome::consumed()
        };
        // ←/→ move between the name and value cells of a row or `+ Add`.
        // A section row has no cells, so there they do nothing (S21) but
        // stay consumed, as everywhere in the table, so nothing leaks to
        // the pane. With no row selected there is no cell to move between,
        // so both stay unconsumed like Up does.
        let has_cells = matches!(self.selected, Some(Stop::Row(_) | Stop::Add));
        match ev.code {
            // Nothing selected: Down selects the first row.
            KeyCode::Down if plain => go(self, place.map_or(0, |p| p + 1)),
            KeyCode::Up if plain => {
                // Row 0 and no selection leave Up unconsumed so the caller
                // (Editor) can fall back to climbing out to the tab strip
                // instead of leaving the user stuck with no way back.
                match place {
                    Some(p) if p > 0 => go(self, p - 1),
                    _ => TableOutcome::not_consumed(),
                }
            }
            KeyCode::Left | KeyCode::Right if plain => {
                if self.selected.is_none() {
                    return TableOutcome::not_consumed();
                }
                if has_cells {
                    self.col = if ev.code == KeyCode::Left { Col::Key } else { Col::Value };
                }
                TableOutcome::consumed()
            }
            KeyCode::Esc => {
                // Esc deselects (collapsing the expanded row); with nothing
                // selected it stays unconsumed for the caller.
                if self.selected.is_some() {
                    self.set_selected(None);
                    TableOutcome::consumed()
                } else {
                    TableOutcome::not_consumed()
                }
            }
            KeyCode::Enter if plain => {
                // A section row opens no cell: Enter there is the caller's
                // (the auto row's take-over), else nothing.
                if !has_cells {
                    return TableOutcome::not_consumed();
                }
                self.begin_edit_selected(map);
                TableOutcome::consumed()
            }
            KeyCode::Char(' ') if plain => {
                let Some(Stop::Row(i)) = self.selected else {
                    return TableOutcome::not_consumed();
                };
                let Some((_, e)) = map.get_index_mut(i) else {
                    return TableOutcome::not_consumed();
                };
                e.enabled = !e.enabled;
                TableOutcome::consumed()
            }
            // The page keys move through the stops, not the view (spec
            // 2026-10-25 S10): the view follows the cursor. Their "page" is
            // the stop count: Home → row 0, End → the last stop,
            // ctrl+f/PageDown → the last stop, ctrl+b/PageUp → row 0,
            // ctrl+d/u ± half the stops.
            KeyCode::Char('f') if ev.modifiers == KeyModifiers::CONTROL => {
                self.handle_nav_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), map, tail)
            }
            KeyCode::Char('b') if ev.modifiers == KeyModifiers::CONTROL => {
                self.handle_nav_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), map, tail)
            }
            // ctrl+Home/End are a second spelling of Home/End, and what
            // macOS's cmd+↑/↓ fold to (`keys::normalize_super_keys`).
            KeyCode::Home | KeyCode::End if ev.modifiers == KeyModifiers::CONTROL => {
                self.handle_nav_key(KeyEvent::new(ev.code, KeyModifiers::NONE), map, tail)
            }
            KeyCode::Home | KeyCode::PageUp if plain => go(self, 0),
            KeyCode::End | KeyCode::PageDown if plain => go(self, last),
            // Half of the places past the first (`last`), as main stepped
            // half the rows.
            KeyCode::Char('d') if ev.modifiers == KeyModifiers::CONTROL => {
                let half = last.div_ceil(2).max(1);
                go(self, place.unwrap_or(0) + half)
            }
            KeyCode::Char('u') if ev.modifiers == KeyModifiers::CONTROL => {
                let half = last.div_ceil(2).max(1);
                go(self, place.unwrap_or(0).saturating_sub(half))
            }
            KeyCode::Delete if plain => {
                let Some(Stop::Row(i)) = self.selected else {
                    return TableOutcome::not_consumed();
                };
                TableOutcome { consumed: true, warning: None, request_delete: Some(i) }
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
            // The field rule: Esc closes the cell keeping its text, and the
            // cursor stays on the row — Enter does the same (spec
            // 2026-09-16), it just also commits. Discard is undo.
            KeyCode::Esc => {
                let Committed { row, here, warning } = self.commit_cell(map, &edit);
                self.exit_editing(row.unwrap_or(here), map);
                TableOutcome::maybe_warn(warning)
            }
            KeyCode::Enter => {
                self.editing = Some(edit);
                // The field rule (spec 2026-09-16): Enter closes the cell
                // like Esc does, keeping the row selected — it no longer
                // drops the selection. `commit` already leaves `selected`
                // parked on the row (or on the warning's pointer row), so
                // no explicit set is needed here either way.
                self.commit(map)
            }
            // Up/Down leave the cell rather than falling through to
            // `LineInput` (which ignores them): they commit it and move the
            // cursor one row. Without this the pane's own "Up climbs out to
            // the tab strip" fallback could fire with an edit still open,
            // leaving `editing` set while the keyboard is elsewhere. They
            // move within the table's rows and `+ Add`: an open cell's keys
            // don't count the section under the table (on Vars that is a
            // scan of every field per keystroke), so ↓ from an open `+ Add`
            // closes it onto `+ Add`, and the next ↓ goes on to the section.
            // They step from where the caret was (`here`), so a `+ Add` name
            // that is taken moves from `+ Add`, not from the row it matched
            // (task #176).
            KeyCode::Up | KeyCode::Down => {
                let Committed { here, warning, .. } = self.commit_cell(map, &edit);
                let target = if ev.code == KeyCode::Up { here.saturating_sub(1) } else { here + 1 };
                self.exit_editing(target, map);
                TableOutcome::maybe_warn(warning)
            }
            KeyCode::BackTab => self.walk_cell(map, &edit, false),
            KeyCode::Tab if shift => self.walk_cell(map, &edit, false),
            KeyCode::Tab => self.walk_cell(map, &edit, true),
            _ => {
                let consumed = edit.input.handle_key(ev);
                self.editing = Some(edit);
                if consumed { TableOutcome::consumed() } else { TableOutcome::not_consumed() }
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
        let Committed { row, here, warning } = self.commit_cell(map, edit);
        if forward {
            match (row, edit.col) {
                (Some(r), Col::Key) => self.start_edit(r, Col::Value, map),
                // A value cell is always a real row's (`open_col`), so the
                // next row is at most the ghost, whose name opens.
                (Some(r), Col::Value) => self.start_edit(r + 1, Col::Key, map),
                // Past the ghost row, its name left empty: done.
                (None, _) => self.exit_editing(map.len(), map),
            }
        } else {
            // Step back from where the caret was (`here`), not from the
            // existing row a taken `+ Add` name matched, which forward Tab
            // opens (task #111).
            match edit.col {
                Col::Value => self.start_edit(here, Col::Key, map),
                Col::Key if here > 0 => self.start_edit(here - 1, Col::Value, map),
                Col::Key => self.exit_editing(here, map),
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
    /// `notes[i]` is row `i`'s [`RowNote`] (rows past the end have none):
    /// the Vars tab's dim `overrides …` trailing the value, or the Headers
    /// tab's mark on a row that is never sent, struck in red with nothing
    /// trailing it.
    /// `vars` is the variable snapshot every drawn cell's `{{tokens}}` are
    /// tinted and registered against (spec §7).
    ///
    /// `skip` is how many of the table's lines (header first) sit above
    /// the view (task #56): line `L` draws on `area.y + L - skip`, and a
    /// line above or below the view is neither painted nor registered, so
    /// a click only ever finds rows that are on screen and the cell rects
    /// it reads back are where the cells were drawn.
    #[allow(clippy::too_many_arguments)] // signature is the produced interface, verbatim
    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        map: &IndexMap<String, Entry>,
        ctx: &DrawCtx,
        add_label: &str,
        hits: &mut HitMap,
        notes: &[Option<RowNote>],
        vars: &VarView,
        skip: usize,
    ) {
        let theme = ctx.theme;
        let map_len = map.len();
        let ghost_editing = self.editing_ghost(map_len);
        let buf = frame.buffer_mut();
        // The screen row of the table's line `line`, when it is on screen.
        let row_y = |line: usize| {
            let d = line.checked_sub(skip)?;
            (d < usize::from(area.height)).then(|| area.y + d as u16)
        };

        // --- header ------------------------------------------------------
        if let Some(y) = row_y(0) {
            let cols = columns(area.x, area.width);
            fill(buf, Rect::new(area.x, y, area.width, 1), theme.panel);
            text(buf, cols.name_x, y, "NAME", theme.text_muted, theme.panel, true);
            text(buf, cols.value_x, y, "VALUE", theme.text_muted, theme.panel, true);
            if cols.divider_x < area.right() {
                text(buf, cols.divider_x, y, "\u{258F}", theme.edge_dark, theme.panel, false);
            }
        }

        // --- data rows -----------------------------------------------------
        // Row `i` is line `i + 1`; only the rows in view are visited.
        let first = skip.saturating_sub(1);
        for (i, (k, e)) in map.iter().enumerate().skip(first) {
            let Some(y) = row_y(i + 1) else { break };
            let note = notes.get(i).and_then(Option::as_ref);
            self.draw_row(buf, hits, area, y, i, k, e, ctx, note, vars, true, None);
        }

        // --- the ghost row -------------------------------------------------
        // Always present, one past the data rows: an empty row that becomes
        // a real entry the moment its key cell commits non-empty. It draws
        // like every other row — the add label stands in for its key until
        // it is typed into, and it carries no toggle or actions, having
        // nothing yet to toggle or delete.
        if let Some(y) = row_y(map_len + 1) {
            // Its value cell never opens (`open_col`), so it stays blank.
            let entry = Entry { value: String::new(), enabled: true };
            let label = (!ghost_editing).then_some(add_label);
            self.draw_row(buf, hits, area, y, map_len, "", &entry, ctx, None, vars, false, label);
        }

        // --- closing edge --------------------------------------------------
        if let Some(y) = row_y(map_len + 2) {
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
        let (glyph, state_fg) =
            if enabled { (" \u{25CF} ", theme.success) } else { (" \u{25CB} ", theme.text_muted) };
        let (fg, bg) =
            if hovered == Some(&hit) { (theme.on_accent, theme.accent) } else { (state_fg, bg) };
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
                Hit::TableCell { row, col: Col::Key.index() },
            );
        }
        if value_w > 0 {
            hits.register(
                Rect::new(value_x, y, value_w, 1),
                Hit::TableCell { row, col: Col::Value.index() },
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
    /// no row below the cursor moves. `note` trails the value, or marks
    /// an enabled row unsent: struck and error-red (see [`RowNote`]).
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
        note: Option<&RowNote>,
        vars: &VarView,
        real: bool,
        ghost_label: Option<&str>,
    ) {
        use crate::ui::clip;
        let theme = ctx.theme;
        let cols = columns(area.x, area.width);
        let editing_col = self.editing.as_ref().filter(|e| e.row == i).map(|e| e.col);
        // The keyboard cursor lights its row only while the pane actually
        // holds the keyboard — a lift on an unfocused pane would claim keys
        // land here. A pointer resting on the row lights it either way.
        let on_cursor = self.selected == Some(if real { Stop::Row(i) } else { Stop::Add });
        let cursor = on_cursor || editing_col.is_some();
        let lit = hovered_row(ctx) == Some(i) || (cursor && ctx.focused);
        let bg = if lit { theme.control_hover } else { theme.control };
        fill(buf, Rect::new(area.x, y, area.width, 1), bg);

        // The actions crowd the value's right end, so the value — plain
        // text or live input — stops short of them instead of running
        // underneath. The ghost row has no actions, so it keeps the width.
        let show_actions = real && (lit || cursor) && area.width >= 10;
        let value_right =
            if show_actions { area.right().saturating_sub(ACTIONS_W) } else { area.right() };
        let value_w = value_right.saturating_sub(cols.value_x);
        let name_w = cols.divider_x.saturating_sub(cols.name_x);
        let unsent = matches!(note, Some(RowNote::Unsent(_)));
        let fg = match (real, entry.enabled, lit) {
            (false, _, true) => theme.text,
            (false, _, false) => theme.text_muted,
            (true, true, _) if unsent => theme.error,
            (true, true, _) => theme.text,
            (true, false, _) => theme.text_muted,
        };
        // The cursor cell keeps a lift of its own inside the cursor row
        // (the Variable Manager grid's rule), so the exact cell Enter
        // would open reads at a glance, not just its row. Keyboard only:
        // an unfocused pane has no cursor to show, and an open cell
        // paints its own lift. On the ghost row Enter opens the name
        // whatever the cursor column (`open_col`), so the name lifts and
        // the value cell, which never opens, keeps the row's fill.
        let cursor_cell = (on_cursor && ctx.focused && editing_col.is_none()).then_some(if real {
            self.col
        } else {
            Col::Key
        });
        let cell_bg = |col: Col| {
            if cursor_cell == Some(col) { crate::theme::lift_color(bg, 0.06) } else { bg }
        };
        let (name_bg, value_bg) = (cell_bg(Col::Key), cell_bg(Col::Value));
        match cursor_cell {
            Some(Col::Key) => fill(buf, Rect::new(cols.name_x, y, name_w, 1), name_bg),
            Some(Col::Value) => fill(buf, Rect::new(cols.value_x, y, value_w, 1), value_bg),
            None => {}
        }

        // Each cell at rest is drawn by a field holding its text, as that
        // field draws at rest: it looks the same once open, and the click
        // that opens it lands on the char it points at. The strike below
        // covers exactly what it drew.
        let key_rest = LineInput::new(ghost_label.unwrap_or(key));
        let value_rest = LineInput::new(&entry.value);

        let mut key_drew = 0;
        if editing_col == Some(Col::Key) {
            let edit = self.editing.as_ref().expect("editing_col implies editing");
            Self::paint_cell_edit(buf, cols.name_x, y, name_w, &edit.input, bg, theme);
        } else {
            let cell = Rect::new(cols.name_x, y, name_w, 1);
            key_drew = key_rest.paint_at_rest(buf, cell, theme, fg, name_bg);
        }

        let mut value_drew = 0;
        if editing_col == Some(Col::Value) {
            let edit = self.editing.as_ref().expect("editing_col implies editing");
            Self::paint_cell_edit(buf, cols.value_x, y, value_w, &edit.input, bg, theme);
        } else {
            let cell = Rect::new(cols.value_x, y, value_w, 1);
            value_drew = value_rest.paint_at_rest(buf, cell, theme, fg, value_bg);
            // The note trails the value on the same line: it is about
            // that value, so it reads where the value ends rather than on
            // a row of its own.
            if let Some(RowNote::Dim(note)) = note {
                let note_x = cols.value_x + value_drew + 3;
                let room = value_right.saturating_sub(note_x);
                if room > 0 {
                    text(buf, note_x, y, clip(note, room), theme.text_muted, value_bg, false);
                }
            }
        }

        if real && (!entry.enabled || unsent) {
            // The strike marks the text that is actually there: a value
            // too long for its cell was clipped, and the line has to stop
            // where the clip did rather than run on across empty cells
            // (and, on a narrow pane, into whatever is drawn beside it).
            Self::strike_cells(buf, cols.name_x, y, key_drew);
            Self::strike_cells(buf, cols.value_x, y, value_drew);
        }

        if cols.divider_x < area.right() {
            text(buf, cols.divider_x, y, "\u{258F}", theme.edge_dark, bg, false);
        }

        hits.register(Rect::new(area.x, y, area.width, 1), Hit::TableRow(i));
        Self::register_cells(hits, cols_span(&cols, value_right), y, i);
        // Token treatment over whatever each cell drew: the field at rest,
        // or for the cell under edit the live input — the URL bar's rule,
        // so a token stays a token while it is being typed. A left click
        // resolves past `VarToken` spans, so the caret move underneath is
        // undisturbed. The ghost's add label is not a value, so it gets
        // none.
        let open = |col| {
            self.editing.as_ref().filter(|_| editing_col == Some(col)).map(|e| (&e.input, true))
        };
        let key_cell = open(Col::Key).or(real.then_some((&key_rest, false)));
        let value_cell = open(Col::Value).or(Some((&value_rest, false)));
        paint_cell_tokens(buf, hits, &cols, value_right, y, key_cell, value_cell, vars, theme);
        // The toggle owns the left gutter on every real row; the actions
        // are revealed by hover or the cursor. Both paint (and register)
        // last so they win over the cells underneath.
        if real && area.width >= TOGGLE_W {
            Self::draw_row_toggle(buf, hits, area.x, y, i, entry.enabled, bg, ctx.hovered, theme);
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
/// registering each span over the `TableCell` hit beneath it. Each cell is
/// the field that drew it and whether it is open (drawn focused) or at
/// rest; `None` for a cell whose text is not a value (the ghost's label).
#[allow(clippy::too_many_arguments)]
fn paint_cell_tokens(
    buf: &mut ratatui::buffer::Buffer,
    hits: &mut HitMap,
    cols: &Columns,
    value_right: u16,
    y: u16,
    key: Option<(&LineInput, bool)>,
    value: Option<(&LineInput, bool)>,
    vars: &VarView,
    theme: &Theme,
) {
    let (name_x, name_w, value_x, value_w) = cols_span(cols, value_right);
    let mut paint = |cell: Rect, field: Option<(&LineInput, bool)>| {
        if let Some((input, open)) = field {
            paint_field_tokens(buf, cell, input, open, vars, theme, hits)
        }
    };
    if name_w > 0 {
        paint(Rect::new(name_x, y, name_w, 1), key);
    }
    if value_w > 0 {
        paint(Rect::new(value_x, y, value_w, 1), value);
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

    /// Spec 2026-10-25 S23: clamping a stop the rows went from under is no
    /// cursor move (the clamped stop is the one already drawn lit), so a
    /// view the wheel scrolled away stays where the wheel put it.
    #[test]
    fn clamping_a_stale_stop_keeps_a_scrolled_away_view() {
        let mut t = TableEditorState::default();
        t.set_selected(Some(Stop::Section(3)));
        t.mark_scrolled_away();
        t.clamp_cursor(2, 2);
        assert_eq!(t.selected(), Some(Stop::Section(1)));
        assert!(t.scrolled_away(), "the clamp brought the view back");
    }

    /// Spec §5.3: a counted delete takes a run of rows as one change; a
    /// count past the end is clamped; the cursor lands where the run was.
    #[test]
    fn delete_rows_clamps_a_count_past_the_end() {
        let mut map: IndexMap<String, Entry> = IndexMap::new();
        for k in ["a", "b", "c", "d"] {
            map.insert(k.into(), Entry { value: "1".into(), enabled: true });
        }
        let mut t =
            TableEditorState { selected: Some(Stop::Row(1)), ..TableEditorState::default() };
        assert_eq!(t.delete_rows(&mut map, 1, 2), 2);
        assert_eq!(map.keys().map(String::as_str).collect::<Vec<_>>(), ["a", "d"]);
        assert_eq!(t.selected, Some(Stop::Row(1)));
        assert_eq!(t.delete_rows(&mut map, 1, 9), 1, "clamped");
        assert_eq!(map.keys().map(String::as_str).collect::<Vec<_>>(), ["a"]);
        assert_eq!(t.selected, Some(Stop::Row(0)), "a run to the end lands on the new last row");
        assert_eq!(t.delete_rows(&mut map, 5, 1), 0, "past the end: nothing");
        assert_eq!(t.delete_rows(&mut map, 0, 0), 0, "no count: nothing");
        assert_eq!(t.delete_rows(&mut map, 0, 1), 1);
        assert!(map.is_empty());
        assert_eq!(t.selected, Some(Stop::Add), "an emptied table lands on the add row");
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    /// Spec 2026-10-25 S10, S11, S13 and S21: the section's rows are
    /// stops after `+ Add`. ↓ goes onto them and stops at the last, ↑
    /// comes back, End and PageDown reach the last stop; Enter opens no
    /// cell there, and Space and Delete do nothing, as on `+ Add`.
    #[test]
    fn the_section_rows_are_stops_after_the_add_row() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState { selected: Some(Stop::Add), ..TableEditorState::default() };
        let mut press =
            |t: &mut TableEditorState, code| t.handle_key_with_tail(key(code), &mut map, 2);
        assert!(press(&mut t, KeyCode::Down).consumed);
        assert_eq!(t.selected, Some(Stop::Section(0)), "onto the first section row");
        press(&mut t, KeyCode::Down);
        press(&mut t, KeyCode::Down);
        assert_eq!(t.selected, Some(Stop::Section(1)), "the last section row is the last stop");
        press(&mut t, KeyCode::Up);
        assert_eq!(t.selected, Some(Stop::Section(0)));
        assert!(!press(&mut t, KeyCode::Enter).consumed, "no cell opens on a section row");
        assert!(t.editing.is_none());
        assert!(!press(&mut t, KeyCode::Char(' ')).consumed);
        assert!(!press(&mut t, KeyCode::Delete).consumed);
        let col = t.col;
        assert!(press(&mut t, KeyCode::Right).consumed, "swallowed: nothing leaks to the pane");
        assert_eq!(t.col, col, "no cells to move between");
        assert_eq!(t.selected, Some(Stop::Section(0)));
        press(&mut t, KeyCode::Up);
        assert_eq!(t.selected, Some(Stop::Add), "↑ comes back to + Add");
        press(&mut t, KeyCode::Home);
        assert_eq!(t.selected, Some(Stop::Row(0)));
        press(&mut t, KeyCode::End);
        assert_eq!(t.selected, Some(Stop::Section(1)));
        press(&mut t, KeyCode::PageUp);
        press(&mut t, KeyCode::PageDown);
        assert_eq!(t.selected, Some(Stop::Section(1)));
        // With no section, the add row is still the last stop.
        let mut t = TableEditorState { selected: Some(Stop::Add), ..TableEditorState::default() };
        t.handle_key(key(KeyCode::Down), &mut map);
        assert_eq!(t.selected, Some(Stop::Add));
    }

    /// A row the map shrank from under (before a frame clamped it) steps
    /// from the last real row, never from a place that now names a
    /// section row.
    #[test]
    fn a_stale_row_cursor_steps_from_the_last_real_row() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t =
            TableEditorState { selected: Some(Stop::Row(4)), ..TableEditorState::default() };
        t.handle_key_with_tail(key(KeyCode::Down), &mut map, 3);
        assert_eq!(t.selected, Some(Stop::Add), "from row 1, ↓ is + Add");
        // Every arm sees the clamped stop (S25): Delete asks for the row
        // that is drawn lit, row 1, not the row that went.
        t.selected = Some(Stop::Row(4));
        let out = t.handle_key_with_tail(key(KeyCode::Delete), &mut map, 3);
        assert_eq!(out.request_delete, Some(1));
        assert_eq!(t.selected, Some(Stop::Row(1)));
    }

    fn shift_tab() -> KeyEvent {
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
    }

    fn map_of(pairs: &[(&str, &str)]) -> IndexMap<String, Entry> {
        let mut map = IndexMap::new();
        for (k, v) in pairs {
            map.insert((*k).to_string(), Entry { value: (*v).to_string(), enabled: true });
        }
        map
    }

    fn type_str(t: &mut TableEditorState, map: &mut IndexMap<String, Entry>, s: &str) {
        for c in s.chars() {
            t.handle_key(key(KeyCode::Char(c)), map);
        }
    }

    #[test]
    fn page_chords_are_strict_synonyms_and_the_vim_letters_are_gone_in_the_table() {
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        for letter in [
            key(KeyCode::Char('g')),
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            key(KeyCode::Char('i')),
            key(KeyCode::Char('j')),
            key(KeyCode::Char('k')),
            key(KeyCode::Char('h')),
            key(KeyCode::Char('l')),
        ] {
            let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
            let mut t =
                TableEditorState { selected: Some(Stop::Row(1)), ..TableEditorState::default() };
            let outcome = t.handle_key(letter, &mut map);
            assert!(!outcome.consumed, "{letter:?}");
            assert_eq!(t.selected, Some(Stop::Row(1)), "{letter:?} moved the cursor");
            assert!(t.editing.is_none(), "{letter:?} opened a cell");
        }
        let pairs = [(ctrl('f'), key(KeyCode::PageDown)), (ctrl('b'), key(KeyCode::PageUp))];
        for (alias, canonical) in pairs {
            let mut ma = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
            let mut mb = ma.clone();
            let (mut a, mut b) = (TableEditorState::default(), TableEditorState::default());
            a.selected = Some(Stop::Row(1));
            b.selected = Some(Stop::Row(1));
            a.handle_key(alias, &mut ma);
            b.handle_key(canonical, &mut mb);
            assert_eq!(a.selected, b.selected, "{alias:?}");
        }
        let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::End), &mut map);
        assert_eq!(t.selected, Some(Stop::Add), "End is the ghost row");
        t.handle_key(ctrl('u'), &mut map);
        assert_eq!(t.selected, Some(Stop::Row(1)), "half of four stops");
        t.handle_key(ctrl('d'), &mut map);
        assert_eq!(t.selected, Some(Stop::Add));
    }

    // --- cell cursor ------------------------------------------------------

    /// The nav cursor has a column as well as a row: ←/→ move it between
    /// the name and value cells, clamping at both ends and staying
    /// consumed so nothing leaks to the pane.
    #[test]
    fn left_and_right_move_the_cursor_between_name_and_value_and_clamp() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t =
            TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        assert_eq!(t.col, Col::Key, "the cursor starts on the name cell");
        assert!(t.handle_key(key(KeyCode::Left), &mut map).consumed);
        assert_eq!(t.col, Col::Key, "← on the name cell stays put");
        assert!(t.handle_key(key(KeyCode::Right), &mut map).consumed);
        assert_eq!(t.col, Col::Value);
        assert!(t.handle_key(key(KeyCode::Right), &mut map).consumed);
        assert_eq!(t.col, Col::Value, "→ on the value cell stays put");
        assert!(t.handle_key(key(KeyCode::Left), &mut map).consumed);
        assert_eq!(t.col, Col::Key);
        assert_eq!(t.selected, Some(Stop::Row(0)), "the row never moves");
    }

    /// ←/→ move the cell cursor in the table's nav mode (h/l are the
    /// router's, see `page_chords_are_strict_synonyms_and_the_vim_letters…`).
    #[test]
    fn left_and_right_move_the_cell_cursor() {
        for (code, from, to) in
            [(KeyCode::Right, Col::Key, Col::Value), (KeyCode::Left, Col::Value, Col::Key)]
        {
            let mut map = map_of(&[("a", "1")]);
            let mut t = TableEditorState {
                selected: Some(Stop::Row(0)),
                col: from,
                ..TableEditorState::default()
            };
            let out = t.handle_key(key(code), &mut map);
            assert!(out.consumed, "{code:?}");
            assert_eq!(t.col, to, "{code:?}");
        }
    }

    /// The column survives a row move, so →, ↓, ↓ lands on the third
    /// row's value cell.
    #[test]
    fn the_cursor_column_persists_across_row_moves() {
        let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Down), &mut map);
        t.handle_key(key(KeyCode::Right), &mut map);
        t.handle_key(key(KeyCode::Down), &mut map);
        t.handle_key(key(KeyCode::Down), &mut map);
        assert_eq!((t.selected, t.col), (Some(Stop::Row(2)), Col::Value));
    }

    /// With nothing selected ←/→ have no cell to move between: they stay
    /// unconsumed like Up does, so the pane keeps its own meaning for them.
    #[test]
    fn left_and_right_with_no_selection_are_unconsumed() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        assert!(!t.handle_key(key(KeyCode::Right), &mut map).consumed);
        assert!(!t.handle_key(key(KeyCode::Left), &mut map).consumed);
        assert_eq!(t.col, Col::Key);
    }

    /// Enter opens the cell under the cursor, not always the name.
    #[test]
    fn enter_opens_the_cell_under_the_cursor() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState {
            selected: Some(Stop::Row(0)),
            col: Col::Value,
            ..TableEditorState::default()
        };
        t.handle_key(key(KeyCode::Enter), &mut map);
        let edit = t.editing.as_ref().expect("a cell is open");
        assert_eq!((edit.row, edit.col), (0, Col::Value));
        assert_eq!(edit.input.text(), "1");
    }

    /// Task #68: the ghost row's value cell never opens, since a value
    /// needs a row's name to belong to. Enter with the cursor on that
    /// column opens the ghost's name instead, and the name's Tab then
    /// walks into the new row's own value.
    #[test]
    fn enter_on_the_ghost_value_column_opens_its_name() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::End), &mut map); // the ghost
        t.handle_key(key(KeyCode::Right), &mut map);
        t.handle_key(key(KeyCode::Enter), &mut map);
        let edit = t.editing.as_ref().expect("the ghost's name cell is open");
        assert_eq!((edit.row, edit.col), (1, Col::Key));
        assert_eq!(edit.input.text(), "");
        type_str(&mut t, &mut map, "k");
        t.handle_key(key(KeyCode::Tab), &mut map);
        let edit = t.editing.as_ref().expect("the new row's value is open");
        assert_eq!((edit.row, edit.col), (1, Col::Value));
        type_str(&mut t, &mut map, "v");
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert_eq!(map.get("k").map(|e| e.value.as_str()), Some("v"));
    }

    /// The cursor column follows the last open cell: leaving a value cell
    /// by Esc, Enter, ↑/↓ or Tab parks the cursor on that column, so Esc
    /// then Enter reopens the same cell.
    #[test]
    fn leaving_a_cell_parks_the_cursor_on_its_column() {
        // Esc from the value cell.
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        assert_eq!(t.col, Col::Value, "a click moves the cursor column");
        t.handle_key(key(KeyCode::Esc), &mut map);
        assert_eq!((t.selected, t.col), (Some(Stop::Row(0)), Col::Value));
        t.handle_key(key(KeyCode::Enter), &mut map);
        let edit = t.editing.as_ref().expect("reopened");
        assert_eq!(edit.col, Col::Value, "Esc then Enter reopens the same cell");

        // Enter commits and parks on the column.
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert_eq!((t.editing.is_none(), t.col), (true, Col::Value));

        // Tab walks: key → value → next row's key; the cursor follows.
        t.click_cell(0, Col::Key, &mut map);
        t.handle_key(key(KeyCode::Tab), &mut map);
        assert_eq!(t.col, Col::Value);
        t.handle_key(key(KeyCode::Tab), &mut map);
        assert_eq!((t.selected, t.col), (Some(Stop::Row(1)), Col::Key));

        // ↓ from a value cell keeps the column on the next row.
        t.handle_key(key(KeyCode::Tab), &mut map); // row 1 value
        t.handle_key(key(KeyCode::Down), &mut map);
        assert_eq!(
            (t.selected, t.col, t.editing.is_none()),
            (Some(Stop::Add), Col::Value, true),
            "two rows: ↓ from row 1 is + Add"
        );
    }

    /// A reset drops the column along with the row.
    #[test]
    fn reset_returns_the_cursor_to_the_name_column() {
        let mut t = TableEditorState { col: Col::Value, ..TableEditorState::default() };
        t.reset();
        assert_eq!(t.col, Col::Key);
    }

    /// The cursor cell lifts one step inside the lit row (the Variable
    /// Manager grid's rule), so the exact cell Enter would open reads at a
    /// glance. Only the cursor cell lifts; the other cell keeps the row's
    /// fill.
    #[test]
    fn the_cursor_cell_lifts_inside_the_cursor_row() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let lifted = crate::theme::lift_color(theme.control_hover, 0.06);
        for (col, other) in [(Col::Key, Col::Value), (Col::Value, Col::Key)] {
            let t = TableEditorState {
                selected: Some(Stop::Row(0)),
                col,
                ..TableEditorState::default()
            };
            let ctx = ctx(&theme, None);
            let mut hits = HitMap::default();
            let terminal = draw_to(&t, &map, &ctx, &mut hits);
            let buf = terminal.backend().buffer();
            let cell = |c: Col| {
                hits.rect_of(&Hit::TableCell { row: 0, col: c.index() }).expect("cell registered")
            };
            let here = cell(col);
            let there = cell(other);
            assert_eq!(buf.cell((here.x, here.y)).unwrap().bg, lifted, "{col:?} lifts");
            assert_eq!(
                buf.cell((there.x, there.y)).unwrap().bg,
                theme.control_hover,
                "{other:?} keeps the row fill"
            );
        }
    }

    /// No lift without the keyboard: an unfocused pane, or a cell open
    /// for edit (the edit paints its own lift), draw the cursor cell flat.
    #[test]
    fn the_cursor_cell_does_not_lift_unfocused_or_while_editing() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let lifted = crate::theme::lift_color(theme.control_hover, 0.06);
        let t = TableEditorState {
            selected: Some(Stop::Row(0)),
            col: Col::Value,
            ..TableEditorState::default()
        };
        let unfocused = DrawCtx { focused: false, ..ctx(&theme, None) };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &unfocused, &mut hits);
        let cell = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert_eq!(
            terminal.backend().buffer().cell((cell.x, cell.y)).unwrap().bg,
            theme.control,
            "unfocused: resting fill"
        );

        let mut t = t;
        let mut map = map;
        t.click_cell(0, Col::Key, &mut map);
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let cell = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert_ne!(
            terminal.backend().buffer().cell((cell.x, cell.y)).unwrap().bg,
            lifted,
            "while editing the name cell, the value cell does not lift"
        );
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
        assert_eq!(t.selected, Some(Stop::Add));
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
        assert_eq!(t.selected, Some(Stop::Row(0)));
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
        let t = TableEditorState { selected: Some(Stop::Add), ..TableEditorState::default() };
        let ctx = ctx(&theme, None);
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx, &mut hits);
        let row = hits.rect_of(&Hit::TableRow(1)).expect("the ghost row is registered");
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf.cell((row.x, row.y)).unwrap().bg,
            theme.control_hover,
            "the cursor row lifts its own fill"
        );
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("+ Add"), "the add label survives: {content}");
    }

    /// The cursor-cell lift marks the cell Enter opens. On the ghost row
    /// that is always the name (task #68), so with the cursor column on
    /// the value the name lifts and the value cell keeps the row's fill:
    /// blank, unlit, inactive, and the same width as ever.
    #[test]
    fn the_ghost_rows_value_cell_never_takes_the_cursor_lift() {
        let theme = Theme::dark();
        let map = map_of(&[("page", "2")]);
        let lifted = crate::theme::lift_color(theme.control_hover, 0.06);
        let t = TableEditorState {
            selected: Some(Stop::Add),
            col: Col::Value,
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let buf = terminal.backend().buffer();
        let name = hits.rect_of(&Hit::TableCell { row: 1, col: 0 }).unwrap();
        let value = hits.rect_of(&Hit::TableCell { row: 1, col: 1 }).unwrap();
        assert_eq!(buf.cell((name.x, name.y)).unwrap().bg, lifted, "the name lifts");
        assert_eq!(
            buf.cell((value.x, value.y)).unwrap().bg,
            theme.control_hover,
            "the value keeps the row fill"
        );
        let real = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert_eq!((value.x, value.width), (real.x, real.width), "no layout shift");
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
        assert_eq!(t.selected, Some(Stop::Row(0)), "the clicked row is the selected row");
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
        assert!(!t.commit(&mut map).consumed, "committing with no edit in progress is a no-op");
    }

    #[test]
    fn esc_on_a_ghost_key_cell_creates_the_row_like_enter() {
        let mut map = map_of(&[]);
        let mut t = TableEditorState::default();
        t.begin_add(&map);
        for c in "k".chars() {
            t.handle_key(key(KeyCode::Char(c)), &mut map);
        }
        t.handle_key(key(KeyCode::Esc), &mut map);
        assert!(map.contains_key("k"));
        assert_eq!(t.selected, Some(Stop::Row(0)));
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
        assert_eq!(map["page"], Entry { value: String::new(), enabled: true });
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

        // The ghost's value cell opens its name (task #68): left empty,
        // that is discarded the same way.
        t.click_cell(1, Col::Value, &mut map);
        let out = t.commit(&mut map);
        assert_eq!(map.len(), 1);
        assert!(out.warning.is_none());
    }

    /// Task #68: a click on the ghost row's value cell puts the caret in
    /// the ghost's name, empty, since a value with no name has no row to
    /// belong to. What is typed there is the new row's name.
    #[test]
    fn a_click_on_the_ghost_value_cell_opens_its_name_cell() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        let edit = t.editing.as_ref().expect("a cell is open");
        assert_eq!((edit.row, edit.col), (0, Col::Key), "the name, not the value");
        assert_eq!(edit.input.text(), "");
        assert_eq!(t.col, Col::Value, "the cursor column is the clicked one");
        type_str(&mut t, &mut map, "id");
        t.commit(&mut map);
        assert_eq!(map["id"].value, "", "the typing named the row");
    }

    /// Tab walks onto the ghost row's name, never its value, and from a
    /// typed name on into the new row's value: name first, then value.
    #[test]
    fn tab_through_the_ghost_row_goes_name_then_the_new_rows_value() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        t.handle_key(key(KeyCode::Tab), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (1, Col::Key), "onto the ghost's name");
        type_str(&mut t, &mut map, "id");
        t.handle_key(key(KeyCode::Tab), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (1, Col::Value), "the new row's own value");
        assert!(map.contains_key("id"), "the name made the row");
        type_str(&mut t, &mut map, "42");
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert_eq!(map["id"].value, "42");
    }

    /// With a name typed on the ghost row, a click on its value cell
    /// commits the name (the new row lands at the ghost's index) and opens
    /// that new row's value, so name-then-value works by mouse too.
    #[test]
    fn clicking_the_ghost_value_after_typing_its_name_opens_the_new_rows_value() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "id");
        t.click_cell(0, Col::Value, &mut map);
        assert!(map.contains_key("id"), "the name committed");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (0, Col::Value), "the new row's value is open");
        type_str(&mut t, &mut map, "42");
        t.commit(&mut map);
        assert_eq!(map["id"].value, "42");
    }

    /// A ghost name that already exists makes no row (task #68): the
    /// toast says so in words that hold whatever is left open after the
    /// commit, here a click on the ghost's value cell, which reopens the
    /// ghost's empty name, not the existing row.
    #[test]
    fn a_duplicate_ghost_name_says_no_row_was_added() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Key, &mut map);
        type_str(&mut t, &mut map, "a");
        let out = t.click_cell(1, Col::Value, &mut map);
        assert_eq!(out.warning.as_deref(), Some("'a' already exists \u{2014} no row added"));
        assert_eq!(map.len(), 1);
        assert_eq!(map["a"].value, "1", "the existing row is untouched");
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col, edit.input.text()), (1, Col::Key, ""));
    }

    /// Esc on the ghost's name, opened from its value cell, leaves
    /// nothing behind: no row, and nothing waiting for the next add.
    #[test]
    fn esc_on_the_ghost_opened_from_its_value_cell_leaves_nothing() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        let out = t.handle_key(key(KeyCode::Esc), &mut map);
        assert!(out.warning.is_none());
        assert!(map.is_empty());
        assert_eq!(t.selected, Some(Stop::Add));
        t.handle_key(key(KeyCode::Enter), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col, edit.input.text()), (0, Col::Key, ""));
    }

    /// The cursor column persists across row moves, through the ghost row
    /// too: opening the ghost's name from the value column leaves the
    /// column on the value, so the rows above still open their values.
    #[test]
    fn the_cursor_column_survives_opening_the_ghost_row() {
        let mut map = map_of(&[("a", "1")]);
        let mut t =
            TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        t.handle_key(key(KeyCode::Right), &mut map);
        t.handle_key(key(KeyCode::Down), &mut map); // the ghost
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert_eq!(t.editing.as_ref().unwrap().col, Col::Key, "the ghost opens its name");
        t.handle_key(key(KeyCode::Esc), &mut map);
        t.handle_key(key(KeyCode::Up), &mut map);
        t.handle_key(key(KeyCode::Enter), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (0, Col::Value), "row 0 opens its value again");
    }

    // --- keyboard: navigation --------------------------------------------

    #[test]
    fn nav_moves_the_selection_over_the_data_rows_and_the_ghost() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState::default();
        assert!(t.handle_key(key(KeyCode::Down), &mut map).consumed);
        assert_eq!(t.selected, Some(Stop::Row(0)), "Down from nowhere selects row 0");
        t.handle_key(key(KeyCode::Down), &mut map);
        t.handle_key(key(KeyCode::Down), &mut map);
        assert_eq!(t.selected, Some(Stop::Add), "the ghost row is reachable");
        assert!(t.editing.is_none(), "selection only — Enter starts the add");
        assert!(
            t.handle_key(key(KeyCode::Down), &mut map).consumed,
            "clamped at the ghost row, still consumed"
        );
        assert_eq!(t.selected, Some(Stop::Add));
        t.handle_key(key(KeyCode::Up), &mut map);
        assert_eq!(t.selected, Some(Stop::Row(1)));
        t.handle_key(key(KeyCode::Up), &mut map);
        assert_eq!(t.selected, Some(Stop::Row(0)));
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
        t.selected = Some(Stop::Row(0));
        assert!(t.handle_key(key(KeyCode::Enter), &mut map).consumed);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (0, Col::Key));
        assert_eq!(edit.input.text(), "a");

        t.editing = None;
        t.selected = Some(Stop::Add);
        t.handle_key(key(KeyCode::Enter), &mut map);
        let edit = t.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (1, Col::Key));
        assert_eq!(edit.input.text(), "");
    }

    #[test]
    fn enter_committing_an_edit_keeps_the_row_selected() {
        let mut map = map_of(&[("a", "1")]);
        let mut t =
            TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        t.handle_key(key(KeyCode::Enter), &mut map); // begin editing the key
        t.handle_key(key(KeyCode::Enter), &mut map); // commit, like Esc
        assert!(t.editing.is_none());
        assert_eq!(t.selected, Some(Stop::Row(0)), "Enter keeps the row selected, like Esc");
    }

    #[test]
    fn enter_in_an_open_cell_commits_and_keeps_the_row_selected() {
        let mut map = map_of(&[("a", "1")]);
        let mut t =
            TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        t.begin_edit_selected(&map);
        t.handle_key(key(KeyCode::Char('x')), &mut map);
        let outcome = t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(outcome.warning.is_none());
        assert_eq!(t.selected, Some(Stop::Row(0)), "Enter keeps the row selected, like Esc");
        assert!(t.editing.is_none());
    }

    #[test]
    fn enter_committing_a_duplicate_key_keeps_the_resolved_row_selected() {
        let mut map = map_of(&[("a", "1"), ("b", "2")]);
        let mut t = TableEditorState { selected: Some(Stop::Add), ..TableEditorState::default() };
        t.handle_key(key(KeyCode::Enter), &mut map);
        for c in "a".chars() {
            t.handle_key(key(KeyCode::Char(c)), &mut map);
        }
        let out = t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(out.warning.is_some(), "duplicate key warns");
        assert_eq!(
            t.selected,
            Some(Stop::Row(0)),
            "the warning points at the existing row, so it stays selected"
        );
    }

    #[test]
    fn space_toggles_and_d_requests_a_delete_confirm() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        assert!(!t.handle_key(key(KeyCode::Char(' ')), &mut map).consumed);

        t.selected = Some(Stop::Row(0));
        assert!(t.handle_key(key(KeyCode::Char(' ')), &mut map).consumed);
        assert!(!map["a"].enabled);
        let out = t.handle_key(key(KeyCode::Delete), &mut map);
        assert_eq!(out.request_delete, Some(0));
        assert_eq!(map.len(), 1, "the row survives until the confirm");
        t.delete_row(&mut map, 0);
        assert!(map.is_empty());

        // The ghost row has nothing to toggle or delete.
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState { selected: Some(Stop::Add), ..TableEditorState::default() };
        assert!(!t.handle_key(key(KeyCode::Char(' ')), &mut map).consumed);
        assert_eq!(t.handle_key(key(KeyCode::Delete), &mut map).request_delete, None);
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

    /// Task #111: a ghost name that already exists makes no row, and
    /// Shift-Tab still steps back from the ghost, where the caret was, not
    /// from the existing row the name matched (forward Tab opens that row's
    /// value; see `a_ghost_row_keyed_like_an_existing_row_warns_and_edits_that_row`).
    #[test]
    fn shift_tab_off_a_ghost_key_that_exists_lands_on_the_last_rows_value() {
        for name in ["c", "b", "a"] {
            let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
            let mut t = TableEditorState::default();
            t.click_cell(3, Col::Key, &mut map); // the ghost
            type_str(&mut t, &mut map, name);
            let out = t.handle_key(shift_tab(), &mut map);
            assert!(out.warning.is_some(), "{name}: a duplicate key warns");
            assert_eq!(map.len(), 3, "{name}: no row was added");
            let edit = t.editing.as_ref().unwrap_or_else(|| panic!("{name}: still editing"));
            assert_eq!((edit.row, edit.col), (2, Col::Value), "{name}: c's value opens");
        }
    }

    /// With the map shrunk under an open ghost name, Shift-Tab steps back
    /// from the ghost's index as it is now: past the row a new name makes,
    /// and from the ghost itself for a name that is taken.
    #[test]
    fn shift_tab_off_a_ghost_key_after_the_map_shrank_steps_back_from_the_ghost() {
        for (name, removed, before) in [("x", 1, 1), ("x", 2, 0), ("a", 1, 1)] {
            let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
            let mut t = TableEditorState::default();
            t.click_cell(3, Col::Key, &mut map); // the ghost
            type_str(&mut t, &mut map, name);
            for _ in 0..removed {
                map.pop();
            }
            t.handle_key(shift_tab(), &mut map);
            assert_eq!(
                map.get_index(map.len() - 1).unwrap().0,
                if name == "x" { "x" } else { "b" }
            );
            let edit = t.editing.as_ref().unwrap();
            assert_eq!((edit.row, edit.col), (before, Col::Value), "{name} less {removed}");
        }
    }

    /// Task #176: Up/Down off a ghost name that already exists make no row
    /// and move from the ghost, where the caret was, not from the existing
    /// row the name matched: Up onto the last row, Down onto `+ Add`, as
    /// with an empty name.
    #[test]
    fn up_and_down_off_a_ghost_key_that_exists_move_from_the_ghost() {
        for name in ["", "a", "b", "c"] {
            for (code, lands) in [(KeyCode::Up, Stop::Row(2)), (KeyCode::Down, Stop::Add)] {
                let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
                let mut t = TableEditorState::default();
                t.click_cell(3, Col::Key, &mut map); // the ghost
                type_str(&mut t, &mut map, name);
                let out = t.handle_key(key(code), &mut map);
                assert!(out.consumed, "{name:?} {code:?}");
                assert_eq!(out.warning.is_some(), !name.is_empty(), "{name:?} {code:?}: warns");
                assert_eq!(map.len(), 3, "{name:?} {code:?}: no row was added");
                assert!(t.editing.is_none(), "{name:?} {code:?}: the cell closed");
                assert_eq!(t.selected, Some(lands), "{name:?} {code:?}");
            }
        }
    }

    /// With the map shrunk under an open ghost name, Up/Down move from the
    /// ghost's index as it is now: from the row a new name makes (`c` is
    /// new once its row went), and from the ghost itself for a name that
    /// is taken.
    #[test]
    fn up_and_down_off_a_ghost_key_after_the_map_shrank_move_from_the_ghost() {
        for (name, code, lands) in [
            ("x", KeyCode::Up, Stop::Row(1)),
            ("x", KeyCode::Down, Stop::Add),
            ("c", KeyCode::Up, Stop::Row(1)),
            ("c", KeyCode::Down, Stop::Add),
            ("a", KeyCode::Up, Stop::Row(1)),
            ("a", KeyCode::Down, Stop::Add),
        ] {
            let mut map = map_of(&[("a", "1"), ("b", "2"), ("c", "3")]);
            let mut t = TableEditorState::default();
            t.click_cell(3, Col::Key, &mut map); // the ghost
            type_str(&mut t, &mut map, name);
            map.pop();
            let out = t.handle_key(key(code), &mut map);
            assert_eq!(out.warning.is_some(), name == "a", "{name} {code:?}: warns");
            assert!(t.editing.is_none(), "{name} {code:?}: the cell closed");
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            let want: &[&str] = if name == "a" { &["a", "b"] } else { &["a", "b", name] };
            assert_eq!(keys, want, "{name} {code:?}");
            assert_eq!(t.selected, Some(lands), "{name} {code:?}");
        }
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
        assert_eq!(t.selected, Some(Stop::Row(1)));

        t.click_cell(1, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        assert!(t.handle_key(key(KeyCode::Up), &mut map).consumed);
        assert!(t.editing.is_none());
        assert_eq!(map["b"].value, "29");
        assert_eq!(t.selected, Some(Stop::Row(0)));
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
        assert_eq!(map["page"], Entry { value: "2".into(), enabled: true });
        assert_eq!(t.selected, Some(Stop::Row(0)), "Enter keeps the row selected, like Esc");
    }

    #[test]
    fn esc_commits_the_cell_and_exits_editing_leaving_the_row_selected() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(0, Col::Value, &mut map);
        type_str(&mut t, &mut map, "9");
        assert!(t.handle_key(key(KeyCode::Esc), &mut map).consumed);
        assert!(t.editing.is_none());
        assert_eq!(map["a"].value, "19", "the typed text stands");
        assert_eq!(map.len(), 1, "the row survives");
        assert_eq!(t.selected, Some(Stop::Row(0)), "the row stays selected");

        // Esc after a Tab commits only the cell it is in: the already
        // committed key cell keeps its new text too.
        t.click_cell(0, Col::Key, &mut map);
        type_str(&mut t, &mut map, "x");
        t.handle_key(key(KeyCode::Tab), &mut map);
        type_str(&mut t, &mut map, "8");
        t.handle_key(key(KeyCode::Esc), &mut map);
        assert_eq!(map.get_index(0).unwrap().0, "ax", "the rename stands");
        assert_eq!(map["ax"].value, "198", "the value cell's edit stands");
    }

    #[test]
    fn esc_on_a_ghost_row_being_typed_creates_it_like_enter() {
        let mut map = map_of(&[("a", "1")]);
        let mut t = TableEditorState::default();
        t.click_cell(1, Col::Key, &mut map);
        type_str(&mut t, &mut map, "new");
        t.handle_key(key(KeyCode::Esc), &mut map);
        assert!(t.editing.is_none());
        assert_eq!(map.len(), 2, "the ghost became a real row");
        assert!(map.contains_key("new"));
        assert_eq!(t.selected, Some(Stop::Row(1)), "the new row stays selected");
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
        assert_eq!(t.selected, Some(Stop::Row(1)), "the cursor follows the surviving row");
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

    /// Spec §4.5: `a` and `d` are the router's; Delete stays the table's.
    #[test]
    fn the_table_has_no_verb_letters() {
        let mut map = map_of(&[("k", "v")]);
        let mut t =
            TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        for c in ['a', 'd'] {
            let out = t.handle_key(key(KeyCode::Char(c)), &mut map);
            assert!(!out.consumed, "{c}");
        }
        assert!(t.editing.is_none(), "a did not open the add row");
        let del = t.handle_key(key(KeyCode::Delete), &mut map);
        assert_eq!(del.request_delete, Some(0), "Delete stays");
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
                t.draw(f, f.area(), map, ctx, "+ Add param", hits, &[], &VarView::default(), 0)
            })
            .unwrap();
        terminal
    }

    /// Task #56: a table drawn `skip` lines down puts row `i` on line
    /// `1 + i - skip` and registers nothing for the lines above the view
    /// or below it, so a click only finds a row that is on screen.
    #[test]
    fn a_skipped_draw_shifts_the_rows_and_registers_only_what_shows() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]);
        let t = TableEditorState::default();
        let mut hits = HitMap::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let area = Rect::new(0, 2, 40, 3);
        terminal
            .draw(|f| {
                let ctx = ctx(&theme, None);
                t.draw(f, area, &map, &ctx, "+ Add param", &mut hits, &[], &VarView::default(), 2)
            })
            .unwrap();
        // Lines 2..5 show: rows 1, 2, 3 (the header and row 0 are above).
        assert_eq!(hits.rect_of(&Hit::TableRow(0)), None, "row 0 is above the view");
        assert_eq!(hits.rect_of(&Hit::TableRow(1)).map(|r| r.y), Some(2));
        assert_eq!(hits.rect_of(&Hit::TableRow(3)).map(|r| r.y), Some(4));
        assert_eq!(hits.rect_of(&Hit::TableRow(4)), None, "+ Add is below the view");
        let buf = terminal.backend().buffer();
        let row = |y: u16| (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(!row(2).contains("NAME"), "the header scrolled away: {}", row(2));
        assert!(row(5).trim().is_empty(), "nothing drawn under the area: {}", row(5));
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
        assert_eq!(hits.hit_at(k.x, k.y), Some(&Hit::TableCell { row: 0, col: 0 }));
        assert_eq!(hits.hit_at(gv.x, gv.y), Some(&Hit::TableCell { row: 1, col: 1 }));
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
        assert_eq!(row1, Rect::new(0, 2, 40, 1), "the edited row stays where and what it was");
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
        assert!(hits.rect_of(&Hit::TableCopy(0)).is_none(), "no actions on an unhovered row");
        assert!(hits.rect_of(&Hit::TableDelete(0)).is_none());

        let hovered = Hit::TableRow(0);
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, Some(&hovered)), &mut hits);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("\u{F01B4}"), "the delete button is a trash can: {content}");
        let copy = hits.rect_of(&Hit::TableCopy(0)).expect("copy hit");
        let trash = hits.rect_of(&Hit::TableDelete(0)).expect("delete hit");
        assert!(
            copy.width >= 3 && trash.width >= 3,
            "buttons get comfortable click targets: {copy:?} {trash:?}"
        );
        assert!(copy.x < trash.x, "copy left of trash");
        assert!(hits.rect_of(&Hit::TableDelete(1)).is_none(), "only the hovered row shows actions");
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
                    &[],
                    &VarView::default(),
                    0,
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let y = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap().y;
        for x in area.right()..40 {
            assert!(
                !buf.cell((x, y)).unwrap().modifier.contains(Modifier::CROSSED_OUT),
                "the strike ran past the table's own area, at column {x}"
            );
        }
    }

    /// Draws `map` into the left `width` columns of a 40-wide terminal.
    fn draw_narrow(
        map: &IndexMap<String, Entry>,
        width: u16,
        notes: &[Option<RowNote>],
        hits: &mut HitMap,
    ) -> Terminal<TestBackend> {
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let area = Rect::new(0, 0, width, 10);
        let t = TableEditorState::default();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    area,
                    map,
                    &ctx(&theme, None),
                    "+ Add param",
                    hits,
                    notes,
                    &VarView::default(),
                    0,
                )
            })
            .unwrap();
        terminal
    }

    /// A value in 2-cell glyphs is cut in cells: on a narrow pane it stops
    /// at the table's edge instead of painting twice its room past it.
    #[test]
    fn a_wide_value_stops_at_the_table_s_edge() {
        let value = "本番環境".repeat(5);
        let map = map_of(&[("a", value.as_str())]);
        let mut hits = HitMap::default();
        let terminal = draw_narrow(&map, 24, &[], &mut hits);
        let buf = terminal.backend().buffer();
        let y = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap().y;
        // Every 2-cell glyph ends by the table's edge: one straddling it
        // would leave only a blank cell past it.
        for x in 0..40 {
            let s = buf[(x, y)].symbol();
            if !s.is_empty() && "本番環境".contains(s) {
                assert!(x + 2 <= 24, "{s} at column {x} runs past the table");
            }
        }
    }

    /// The shadow hint trails the value by 3 cells, counted from where
    /// the value's cells end, so it never lands on a wide value's glyphs.
    #[test]
    fn the_shadow_hint_trails_a_wide_value_s_cells() {
        let map = map_of(&[("a", "本番")]);
        let notes = [Some(RowNote::Dim("overrides qa: 1".into()))];
        let mut hits = HitMap::default();
        let terminal = draw_narrow(&map, 40, &notes, &mut hits);
        let buf = terminal.backend().buffer();
        let v = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert_eq!(buf[(v.x + 2, v.y)].symbol(), "番", "the value keeps its second glyph");
        assert_eq!(buf[(v.x + 7, v.y)].symbol(), "o", "the hint starts 3 cells past the value");
        assert_eq!(buf[(v.x + 7, v.y)].fg, Theme::dark().text_muted, "a dim note");
    }

    /// An unsent row (a header row that is never sent) draws its name and
    /// value error-red and struck through, exactly over their text, with
    /// nothing trailing the value: the footer says why (task #59). A row
    /// without a note keeps the plain text colour.
    #[test]
    fn an_unsent_row_is_struck_in_the_error_colour_with_no_trailing_text() {
        let theme = Theme::dark();
        let map = map_of(&[("Host", "other.test"), ("X-A", "1")]);
        let notes = [Some(RowNote::Unsent("Host"))];
        let mut hits = HitMap::default();
        let terminal = draw_narrow(&map, 40, &notes, &mut hits);
        let buf = terminal.backend().buffer();
        let cell = |row, col| hits.rect_of(&Hit::TableCell { row, col }).unwrap();
        let (k, v) = (cell(0, 0), cell(0, 1));
        let struck = |x, y| buf[(x, y)].modifier.contains(Modifier::CROSSED_OUT);
        assert_eq!(buf[(k.x, k.y)].fg, theme.error, "the name");
        assert_eq!(buf[(v.x, v.y)].fg, theme.error, "the value");
        assert!(struck(k.x, k.y) && struck(k.x + 3, k.y), "the name is struck");
        assert!(struck(v.x, v.y) && struck(v.x + 9, v.y), "the value is struck");
        assert!(!struck(v.x + 10, v.y), "the strike stops where the value does");
        let rest: String = (v.x + 10..v.right()).map(|x| buf[(x, v.y)].symbol()).collect();
        assert_eq!(rest.trim(), "", "no note trails the value");
        let plain = cell(1, 0);
        assert_eq!(buf[(plain.x, plain.y)].fg, theme.text, "a row with no note is plain");
        assert!(!struck(plain.x, plain.y), "and it is not struck");
    }

    /// A disabled wide value is struck across exactly the cells it fills.
    /// A wide glyph's style lives on its first cell (the terminal draws
    /// it across both), so those are the cells to check.
    #[test]
    fn a_disabled_wide_value_is_struck_across_its_cells() {
        let mut map = map_of(&[("a", "本番")]);
        map["a"].enabled = false;
        let mut hits = HitMap::default();
        let terminal = draw_narrow(&map, 40, &[], &mut hits);
        let buf = terminal.backend().buffer();
        let v = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        let struck = |x: u16| buf[(x, v.y)].modifier.contains(Modifier::CROSSED_OUT);
        assert!(struck(v.x) && struck(v.x + 2), "both glyphs of the value are struck");
        assert!(!struck(v.x + 4), "the strike stops where the value does");
    }

    /// The strike covers what the cell drew at rest, a code's cells too:
    /// `a^Ib` is four.
    #[test]
    fn a_disabled_value_is_struck_across_its_codes() {
        let mut map = map_of(&[("a", "a\tb")]);
        map["a"].enabled = false;
        let mut hits = HitMap::default();
        let terminal = draw_narrow(&map, 40, &[], &mut hits);
        let buf = terminal.backend().buffer();
        let v = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        let struck = |x: u16| buf[(x, v.y)].modifier.contains(Modifier::CROSSED_OUT);
        assert_eq!(
            (0..6).map(|dx| struck(v.x + dx)).collect::<Vec<_>>(),
            [true, true, true, true, false, false]
        );
    }

    /// Selecting a row must not move anything: rows edit in place on their
    /// own single line, the way the Variable Manager's grid does, so no
    /// row below the cursor ever shifts under it.
    #[test]
    fn selecting_a_row_keeps_every_row_one_line_and_moves_nothing_below_it() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1"), ("b", "2")]);
        let mut resting = HitMap::default();
        draw_to(&TableEditorState::default(), &map, &ctx(&theme, None), &mut resting);
        let before: Vec<_> = (0..3).map(|i| resting.rect_of(&Hit::TableRow(i)).unwrap()).collect();

        let t = TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let after: Vec<_> = (0..3).map(|i| hits.rect_of(&Hit::TableRow(i)).unwrap()).collect();
        assert_eq!(after[0].height, 1, "the selected row edits in place: still one line");
        assert_eq!(before, after, "nothing moved when the row was selected");
    }

    /// The same holds for the ghost row: resting the cursor on it must not
    /// grow it either.
    #[test]
    fn selecting_the_ghost_row_keeps_it_one_line() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState { selected: Some(Stop::Add), ..TableEditorState::default() };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let ghost = hits.rect_of(&Hit::TableRow(1)).unwrap();
        assert_eq!(ghost.height, 1, "the ghost row stays one line");
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("+ Add param"), "and keeps its add label: {content}");
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
        let toggle =
            hits.rect_of(&Hit::TableCheckbox(0)).expect("the toggle is up on an untouched row");
        assert_eq!(toggle.x, row.x, "flush with the row's left edge");
        assert_eq!(toggle.y, row.y, "on the row's own line");
        let key = hits.rect_of(&Hit::TableCell { row: 0, col: 0 }).unwrap();
        assert!(toggle.right() <= key.x, "the toggle sits before the key text: {toggle:?} {key:?}");
    }

    /// The row's context buttons are copy then delete, at the right edge,
    /// with the toggle now away on the left.
    #[test]
    fn the_active_rows_buttons_offer_copy_before_the_trash() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let copy = hits.rect_of(&Hit::TableCopy(0)).expect("the active row offers copy");
        let trash = hits.rect_of(&Hit::TableDelete(0)).unwrap();
        assert!(copy.right() <= trash.x, "copy left of trash: {copy:?} {trash:?}");
        let value = hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
        assert!(value.x < copy.x, "the buttons sit past the value cell");
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains(crate::glyph::COPY), "the copy glyph is painted: {content}");
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
        assert!(hits.rect_of(&Hit::TableCopy(0)).is_some(), "hover reveals copy");
    }

    #[test]
    fn the_expanded_row_shows_toggle_and_trash_without_hover() {
        let theme = Theme::dark();
        let map = map_of(&[("a", "1")]);
        let t = TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        let mut hits = HitMap::default();
        let terminal = draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(hits.rect_of(&Hit::TableCheckbox(0)).is_some());
        assert!(hits.rect_of(&Hit::TableDelete(0)).is_some());
        assert!(content.contains("\u{F01B4}"), "trash, not 󰅖: {content}");
        assert!(!content.contains('\u{F0156}'), "the old 󰅖 delete glyph is gone: {content}");
        assert!(
            !content.contains('\u{2713}') && !content.contains('\u{2717}'),
            "no left check column anywhere: {content}"
        );
    }

    #[test]
    fn a_live_cell_edit_keeps_the_buttons_visible() {
        let theme = Theme::dark();
        let mut map = map_of(&[("a", "1")]);
        let mut t =
            TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
        t.click_cell(0, Col::Value, &mut map);
        let mut hits = HitMap::default();
        draw_to(&t, &map, &ctx(&theme, None), &mut hits);
        let toggle =
            hits.rect_of(&Hit::TableCheckbox(0)).expect("toggle stays up during a value edit");
        let trash = hits.rect_of(&Hit::TableDelete(0)).expect("trash stays up during a value edit");
        assert!(toggle.x < trash.x, "toggle left of trash: {toggle:?} {trash:?}");
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
        let t = TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
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
        let t = TableEditorState { selected: Some(Stop::Add), ..TableEditorState::default() };
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
        let t = TableEditorState { selected: Some(Stop::Row(0)), ..TableEditorState::default() };
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
        t.selected = Some(Stop::Add); // the ghost, selected but not typed into
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
