use super::DrawCtx;
use super::line_input::LineInput;
use crate::hit::{Hit, HitMap};
use crate::paint::{PillRow, RowHighlight, fill, text};
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::Entry;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Which cell of the selected row is under edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Key,
    Value,
}

/// In-progress edit of a single cell. The map itself is never mutated until
/// the edit is committed (Tab/Enter), so `Esc` naturally discards it.
#[derive(Debug, Clone)]
pub struct CellEdit {
    pub col: Col,
    pub input: LineInput,
    /// `Some(k)` when editing an existing row (its original key, so a
    /// same-key commit is a no-op rename); `None` for a brand-new row
    /// appended via `a` that has not been written into the map yet.
    pub original_key: Option<String>,
    /// The key text already typed and tabbed past, kept until the value
    /// cell's commit finishes the row. `None` while the key cell itself is
    /// being edited.
    pending_key: Option<String>,
}

/// Result of a `TableEditorState::handle_key` call.
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

    fn warn(warning: String) -> Self {
        Self {
            consumed: true,
            warning: Some(warning),
            ..Self::default()
        }
    }
}

/// Column x-offsets (relative to the drawn area's own left edge, i.e.
/// *before* the active row's 1-column accent-bar indent is applied).
struct Columns {
    check_x: u16,
    name_x: u16,
    divider_x: u16,
    value_x: u16,
}

fn columns(x0: u16, width: u16) -> Columns {
    let check_w = 2u16.min(width);
    let remaining = width.saturating_sub(check_w);
    let name_w = (remaining / 3)
        .max(4)
        .min(remaining.saturating_sub(2).max(4));
    Columns {
        check_x: x0,
        name_x: x0 + check_w,
        divider_x: x0 + check_w + name_w,
        value_x: x0 + check_w + name_w + 1,
    }
}

/// `1 (header) + rows + (2 if a row is expanded) + 1 (ghost add row) + 1
/// (closing edge)`. `rows` is the total number of data-row *lines* about to
/// be drawn (`map.len()`, plus one more while a brand-new not-yet-inserted
/// row is being typed). `active` is `Some(_)` whenever exactly one of those
/// rows is drawn expanded (hovered or being edited) — its value is unused by
/// the height math, only its presence.
pub fn table_height(rows: usize, active: Option<usize>) -> u16 {
    1 + rows as u16 + active.map_or(0, |_| 2) + 1 + 1
}

/// Shared cursor/edit state for a key/value table (Params or Headers tab).
/// One instance is reused across both tabs; the caller passes in whichever
/// `IndexMap` is currently active.
#[derive(Debug, Default)]
pub struct TableEditorState {
    /// The selected row (always the one drawn expanded). `None` means no
    /// row is selected — every row draws compact and the row-level keys
    /// (Enter/Space/d) are inert until Down/j or a click selects one.
    pub selected: Option<usize>,
    pub editing: Option<CellEdit>,
}

impl TableEditorState {
    /// Resets cursor/edit state; used when switching tabs so a selection
    /// index from one map can't be stale (and panic) against the other.
    pub fn reset(&mut self) {
        self.selected = None;
        self.editing = None;
    }

    /// Begins editing the selected row's key cell, seeded with its current
    /// key text. A no-op on an empty map. Shared by the keyboard `Enter`
    /// path and the mouse double-click-a-row path.
    pub fn begin_edit_selected(&mut self, map: &IndexMap<String, Entry>) {
        if map.is_empty() {
            return;
        }
        self.clamp_selected(map);
        let Some(sel) = self.selected else { return };
        let key = map.get_index(sel).map(|(k, _)| k.clone()).unwrap();
        self.editing = Some(CellEdit {
            col: Col::Key,
            input: LineInput::new(&key),
            original_key: Some(key),
            pending_key: None,
        });
    }

    /// Starts a brand-new (not-yet-inserted) row, exactly like pressing `a`.
    /// Shared by the `a` key path and a click on the ghost "+ Add" row.
    pub fn begin_add(&mut self, map: &IndexMap<String, Entry>) {
        self.selected = Some(map.len());
        self.editing = Some(CellEdit {
            col: Col::Key,
            input: LineInput::new(""),
            original_key: None,
            pending_key: None,
        });
    }

    /// Deletes row `i` outright. Only ever called after the user confirmed
    /// the delete (both the `d` key and the `✕` click route through a
    /// confirmation modal first).
    pub fn delete_row(&mut self, map: &mut IndexMap<String, Entry>, i: usize) {
        if i >= map.len() {
            return;
        }
        self.selected = Some(i);
        self.editing = None;
        map.shift_remove_index(i);
        self.clamp_selected(map);
    }

    fn clamp_selected(&mut self, map: &IndexMap<String, Entry>) {
        self.selected = match self.selected {
            Some(_) if map.is_empty() => None,
            Some(s) => Some(s.min(map.len() - 1)),
            None => None,
        };
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
                if map.is_empty() {
                    return TableOutcome::not_consumed();
                }
                self.selected = Some(match self.selected {
                    None => 0, // nothing selected: Down selects the first row
                    Some(s) => (s + 1).min(map.len() - 1),
                });
                TableOutcome::consumed()
            }
            KeyCode::Char('k') | KeyCode::Up => {
                // Row 0, no selection, and an empty table leave Up
                // unconsumed so the caller (Editor) can fall back to moving
                // focus to the URL line instead of leaving the user stuck
                // with no way back.
                match self.selected {
                    Some(s) if s > 0 && !map.is_empty() => {
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
            KeyCode::Char('a') => {
                self.begin_add(map);
                TableOutcome::consumed()
            }
            KeyCode::Enter => {
                if map.is_empty() || self.selected.is_none() {
                    return TableOutcome::not_consumed();
                }
                self.begin_edit_selected(map);
                TableOutcome::consumed()
            }
            KeyCode::Char(' ') => {
                if map.is_empty() || self.selected.is_none() {
                    return TableOutcome::not_consumed();
                }
                self.clamp_selected(map);
                if let Some((_, e)) = self.selected.and_then(|s| map.get_index_mut(s)) {
                    e.enabled = !e.enabled;
                }
                TableOutcome::consumed()
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if map.is_empty() || self.selected.is_none() {
                    return TableOutcome::not_consumed();
                }
                self.clamp_selected(map);
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
        match ev.code {
            KeyCode::Esc => TableOutcome::consumed(),
            KeyCode::Tab if edit.col == Col::Key => {
                self.commit_key_and_move_to_value(&mut edit, map);
                self.editing = Some(edit);
                TableOutcome::consumed()
            }
            KeyCode::Tab | KeyCode::Enter => {
                let key_text = match edit.col {
                    Col::Key => edit.input.text(),
                    Col::Value => edit.pending_key.as_deref().unwrap_or(""),
                };
                if key_text.trim().is_empty() {
                    // An empty key is never a valid row: behave like Esc and
                    // discard the edit rather than inserting a "" key.
                    return TableOutcome::consumed();
                }
                let warning = self.commit_row(map, &edit);
                match warning {
                    Some(w) => TableOutcome::warn(w),
                    None => TableOutcome::consumed(),
                }
            }
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

    /// Stashes the just-typed key text and switches the edit to the value
    /// cell, seeding it with the row's current value (empty for a brand
    /// new, not-yet-inserted row).
    fn commit_key_and_move_to_value(&mut self, edit: &mut CellEdit, map: &IndexMap<String, Entry>) {
        let current_value = if edit.original_key.is_some() {
            self.selected
                .and_then(|s| map.get_index(s))
                .map(|(_, e)| e.value.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        edit.pending_key = Some(edit.input.text().to_string());
        edit.col = Col::Value;
        edit.input = LineInput::new(&current_value);
    }

    fn commit_row(&mut self, map: &mut IndexMap<String, Entry>, edit: &CellEdit) -> Option<String> {
        match &edit.original_key {
            Some(orig) => self.commit_existing_row(map, edit, orig.clone()),
            None => self.commit_new_row(map, edit),
        }
    }

    fn commit_existing_row(
        &mut self,
        map: &mut IndexMap<String, Entry>,
        edit: &CellEdit,
        orig: String,
    ) -> Option<String> {
        let idx = self.selected.unwrap_or(0);
        let existing = map.get_index(idx).map(|(_, e)| e.clone());
        let (final_key, final_value, enabled) = match edit.col {
            Col::Key => {
                let value = existing
                    .as_ref()
                    .map(|e| e.value.clone())
                    .unwrap_or_default();
                let enabled = existing.as_ref().map(|e| e.enabled).unwrap_or(true);
                (edit.input.text().to_string(), value, enabled)
            }
            Col::Value => {
                let key = edit.pending_key.clone().unwrap_or_else(|| orig.clone());
                let enabled = existing.as_ref().map(|e| e.enabled).unwrap_or(true);
                (key, edit.input.text().to_string(), enabled)
            }
        };

        if final_key != orig
            && let Some(other_idx) = map.get_index_of(&final_key)
        {
            map.shift_remove_index(idx);
            let adjusted = if other_idx > idx {
                other_idx - 1
            } else {
                other_idx
            };
            if let Some((_, e)) = map.get_index_mut(adjusted) {
                e.value = final_value;
            }
            self.clamp_selected(map);
            return Some(format!(
                "duplicate key '{final_key}' replaced the existing value"
            ));
        }
        map.shift_remove_index(idx);
        map.shift_insert(
            idx,
            final_key,
            Entry {
                value: final_value,
                enabled,
            },
        );
        self.clamp_selected(map);
        None
    }

    fn commit_new_row(
        &mut self,
        map: &mut IndexMap<String, Entry>,
        edit: &CellEdit,
    ) -> Option<String> {
        let (final_key, final_value) = match edit.col {
            Col::Key => (edit.input.text().to_string(), String::new()),
            Col::Value => {
                let key = edit.pending_key.clone().unwrap_or_default();
                (key, edit.input.text().to_string())
            }
        };
        if let Some(other_idx) = map.get_index_of(&final_key) {
            if let Some((_, e)) = map.get_index_mut(other_idx) {
                e.value = final_value;
            }
            self.clamp_selected(map);
            return Some(format!(
                "duplicate key '{final_key}' replaced the existing value"
            ));
        }
        map.insert(
            final_key,
            Entry {
                value: final_value,
                enabled: true,
            },
        );
        self.selected = Some(map.len() - 1);
        None
    }

    /// The row (existing, by map index) currently drawn expanded: the row
    /// being edited, or — when nothing is being edited — the selected row.
    /// Hover never expands a row (it only tints its background), so what's
    /// selected is always the one visibly expanded row. `None` when nothing
    /// is expanded (empty map, or a brand-new row being typed).
    pub fn active_index(&self, map_len: usize) -> Option<usize> {
        if let Some(edit) = &self.editing
            && edit.original_key.is_none()
        {
            return None; // the new-row line is handled separately, always expanded
        }
        if map_len == 0 {
            return None;
        }
        self.selected.map(|s| s.min(map_len - 1))
    }

    /// Draws the table as one contiguous painted control: a muted-uppercase
    /// `NAME`/`VALUE` header row on `panel`, a `control` body of compact
    /// 1-line rows (the active row — being edited, or hovered — expands to
    /// 3 with a full-row pill and a `✕` delete affordance), a ghost
    /// `+ Add …` row, and a closing `▔` edge.
    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        map: &IndexMap<String, Entry>,
        ctx: &DrawCtx,
        add_label: &str,
        hits: &mut HitMap,
    ) {
        let theme = ctx.theme;
        let map_len = map.len();
        let active = self.active_index(map_len);
        let new_row_pending = self
            .editing
            .as_ref()
            .is_some_and(|e| e.original_key.is_none());
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
                y = self.draw_active_row(buf, hits, area, y, bottom, i, k, e, true, theme);
            } else {
                let hovered = ctx.hovered == Some(&Hit::TableRow(i));
                self.draw_plain_row(buf, hits, area, y, i, k, e, hovered, theme);
                y += 1;
            }
        }

        // --- brand-new row, always drawn expanded --------------------------
        if new_row_pending && y < bottom {
            let edit = self.editing.as_ref().expect("new_row_pending checked Some");
            let key = edit.pending_key.as_deref().unwrap_or("");
            let entry = Entry {
                value: String::new(),
                enabled: true,
            };
            y = self.draw_active_row(
                buf, hits, area, y, bottom, map_len, key, &entry,
                false, // brand-new row: no delete affordance yet
                theme,
            );
        }

        // --- ghost "+ Add" row -----------------------------------------
        if y < bottom {
            let ghost_hovered = ctx.hovered == Some(&Hit::TableAdd);
            let bg = if ghost_hovered {
                theme.control_hover
            } else {
                theme.control
            };
            let fg = if ghost_hovered {
                theme.text
            } else {
                theme.text_muted
            };
            fill(buf, Rect::new(area.x, y, area.width, 1), bg);
            text(buf, area.x + 1, y, add_label, fg, bg, false);
            hits.register(Rect::new(area.x, y, area.width, 1), Hit::TableAdd);
            y += 1;
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
        hovered: bool,
        theme: &Theme,
    ) {
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
        let check = if entry.enabled {
            "\u{2713}"
        } else {
            "\u{2717}"
        };
        text(buf, cols.check_x, y, check, fg, bg, false);
        text(buf, cols.name_x, y, key, fg, bg, false);
        text(buf, cols.value_x, y, &entry.value, fg, bg, false);
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
        if area.width >= 3 {
            hits.register(Rect::new(cols.check_x, y, 1, 1), Hit::TableCheckbox(i));
        }
    }

    /// Draws row `i` expanded to 3 lines (pad/text/pad) with the full-row
    /// pill treatment. `show_delete` gates the `✕` affordance (a brand-new,
    /// not-yet-inserted row has nothing to delete yet). Returns the next `y`.
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
        theme: &Theme,
    ) -> u16 {
        let text_row = y + 1;
        PillRow {
            highlight: RowHighlight::Selected,
        }
        .paint(
            buf,
            text_row,
            area.x,
            area.width,
            area,
            theme.control,
            theme,
        );

        // The accent bar occupies column `area.x`; cell content is indented
        // one column past it.
        let content_x = area.x + 1;
        let content_w = area.width.saturating_sub(1);
        let cols = columns(content_x, content_w);
        let bg = theme.control_hover;
        let fg = if entry.enabled {
            theme.text
        } else {
            theme.text_muted
        };

        let editing_col = self
            .editing
            .as_ref()
            .filter(|e| {
                e.original_key.as_deref() == Some(key)
                    || (e.original_key.is_none() && self.selected == Some(i))
            })
            .map(|e| e.col);

        let check = if entry.enabled {
            "\u{2713}"
        } else {
            "\u{2717}"
        };
        text(buf, cols.check_x, text_row, check, fg, bg, false);

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
                let value_w = area.x + area.width - cols.value_x;
                buf.set_line(cols.value_x, text_row, &line, value_w);
            }
            None => {
                text(buf, cols.name_x, text_row, key, fg, bg, false);
                text(buf, cols.value_x, text_row, &entry.value, fg, bg, false);
            }
        }

        hits.register(Rect::new(area.x, y, area.width, 3), Hit::TableRow(i));
        if content_w >= 3 {
            hits.register(
                Rect::new(cols.check_x, text_row, 1, 1),
                Hit::TableCheckbox(i),
            );
        }
        if show_delete && area.width >= 2 {
            let del_x = area.x + area.width - 1;
            text(
                buf,
                del_x,
                text_row,
                "\u{2715}",
                theme.text_muted,
                bg,
                false,
            );
            hits.register(Rect::new(del_x, text_row, 1, 1), Hit::TableDelete(i));
        }

        (y + 3).min(bottom)
    }
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

    #[test]
    fn add_edit_commit_creates_entry() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Char('a')), &mut map);
        for c in "page".chars() {
            t.handle_key(key(KeyCode::Char(c)), &mut map);
        }
        t.handle_key(key(KeyCode::Tab), &mut map); // key -> value
        t.handle_key(key(KeyCode::Char('2')), &mut map);
        t.handle_key(key(KeyCode::Enter), &mut map);
        assert_eq!(
            map["page"],
            Entry {
                value: "2".into(),
                enabled: true
            }
        );
        assert!(t.editing.is_none());
    }

    #[test]
    fn committing_an_empty_key_cancels_the_row_like_esc() {
        let mut map = IndexMap::new();
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Char('a')), &mut map); // start a new row
        let out = t.handle_key(key(KeyCode::Enter), &mut map); // commit with empty key
        assert!(map.is_empty(), "no \"\" key must be inserted");
        assert!(t.editing.is_none(), "editing ends, same as Esc");
        assert!(out.consumed);
        assert!(out.warning.is_none());
    }

    #[test]
    fn duplicate_key_commit_replaces_and_warns() {
        let mut map = IndexMap::new();
        map.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        map.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        let mut t = TableEditorState::default();
        // add new row keyed "a" with value "9"
        t.handle_key(key(KeyCode::Char('a')), &mut map);
        t.handle_key(key(KeyCode::Char('a')), &mut map);
        t.handle_key(key(KeyCode::Tab), &mut map);
        t.handle_key(key(KeyCode::Char('9')), &mut map);
        let out = t.handle_key(key(KeyCode::Enter), &mut map);
        assert!(out.warning.is_some());
        assert_eq!(map.len(), 2);
        assert_eq!(map["a"].value, "9");
        assert_eq!(map.get_index(0).unwrap().0, "a", "original position kept");
    }

    #[test]
    fn space_toggles_d_requests_delete_esc_cancels() {
        let mut map = IndexMap::new();
        map.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        let mut t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        t.handle_key(key(KeyCode::Char(' ')), &mut map);
        assert!(!map["a"].enabled);
        t.handle_key(key(KeyCode::Enter), &mut map); // start editing
        t.handle_key(key(KeyCode::Char('x')), &mut map);
        t.handle_key(key(KeyCode::Esc), &mut map); // cancel
        assert_eq!(map["a"].value, "1", "esc discards the edit");
        // 'd' never deletes directly: it asks the caller to confirm first.
        let out = t.handle_key(key(KeyCode::Char('d')), &mut map);
        assert!(out.consumed);
        assert_eq!(out.request_delete, Some(0));
        assert_eq!(map.len(), 1, "the row survives until the confirm");
        // The confirmed path deletes for real.
        t.delete_row(&mut map, 0);
        assert!(map.is_empty());
    }

    #[test]
    fn rename_existing_key_keeps_its_position() {
        let mut map = IndexMap::new();
        map.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        map.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        let mut t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        t.handle_key(key(KeyCode::Enter), &mut map); // edit "a"'s key cell (seeded with "a")
        t.handle_key(key(KeyCode::Char('x')), &mut map);
        t.handle_key(key(KeyCode::Enter), &mut map); // commit rename to "ax"
        assert_eq!(
            map.get_index(0).unwrap().0,
            "ax",
            "renamed key keeps original position"
        );
        assert_eq!(map.get_index(1).unwrap().0, "b");
    }

    #[test]
    fn rename_onto_later_key_merges_and_shifts_index() {
        // a, b, c; rename "a" -> "c" (a key that appears AFTER it in the
        // map). Removing "a" shifts every later index down by one, so the
        // duplicate-merge target's index must be adjusted accordingly.
        let mut map = IndexMap::new();
        map.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        map.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        map.insert(
            "c".into(),
            Entry {
                value: "3".into(),
                enabled: true,
            },
        );
        let mut t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        t.handle_key(key(KeyCode::Enter), &mut map); // edit "a"'s key cell (seeded with "a")
        t.handle_key(key(KeyCode::Backspace), &mut map); // clear it
        t.handle_key(key(KeyCode::Char('c')), &mut map);
        let out = t.handle_key(key(KeyCode::Enter), &mut map); // commit rename a -> c
        assert!(out.warning.is_some());
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get_index(0).unwrap().0,
            "b",
            "b shifts down to fill a's old slot"
        );
        assert_eq!(
            map.get_index(1).unwrap().0,
            "c",
            "c keeps its relative position after b"
        );
        assert_eq!(map["c"].value, "1", "c takes a's value");
    }

    fn ctx<'a>(theme: &'a Theme, hovered: Option<&'a Hit>) -> DrawCtx<'a> {
        DrawCtx {
            theme,
            focused: true,
            hovered,
            dragging: false,
        }
    }

    #[test]
    fn draw_shows_header_ghost_row_and_data_rows() {
        let theme = Theme::dark();
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        let empty_map: IndexMap<String, Entry> = IndexMap::new();
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    f.area(),
                    &empty_map,
                    &ctx(&theme, None),
                    "+ Add param",
                    &mut hits,
                )
            })
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("NAME"), "header: {content}");
        assert!(content.contains("VALUE"), "header: {content}");
        assert!(content.contains("+ Add param"), "ghost row: {content}");

        let mut map = IndexMap::new();
        map.insert(
            "page".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    f.area(),
                    &map,
                    &ctx(&theme, None),
                    "+ Add param",
                    &mut hits,
                )
            })
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("page"), "key text: {content}");
        assert!(content.contains('2'), "value text: {content}");
        // header is row 0, so the data row is row 1; it's the selected row,
        // so it is drawn expanded (3 lines).
        assert_eq!(
            hits.rect_of(&Hit::TableRow(0)),
            Some(Rect::new(0, 1, 40, 3))
        );
        assert!(hits.rect_of(&Hit::TableCheckbox(0)).is_some());
    }

    #[test]
    fn active_row_expands_to_three_lines() {
        let theme = Theme::dark();
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut map = IndexMap::new();
        map.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        map.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        map.insert(
            "c".into(),
            Entry {
                value: "3".into(),
                enabled: true,
            },
        );
        let mut t = TableEditorState {
            selected: Some(1),
            ..TableEditorState::default()
        };
        t.begin_edit_selected(&map); // editing row 1 ("b")
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    f.area(),
                    &map,
                    &ctx(&theme, None),
                    "+ Add param",
                    &mut hits,
                )
            })
            .unwrap();

        // row 0 ("a") is above the header at y=0, so it's at y=1, compact.
        // row 1 ("b") is the active row: expands to 3 lines starting y=2.
        let row1 = hits.rect_of(&Hit::TableRow(1)).unwrap();
        assert_eq!(row1, Rect::new(0, 2, 40, 3), "active row spans 3 lines");
        // row 2 ("c") follows immediately after, compact again, at y=5.
        let row2 = hits.rect_of(&Hit::TableRow(2)).unwrap();
        assert_eq!(row2.height, 1, "inactive rows stay compact");
        assert_eq!(row2.y, 5);

        let buf = terminal.backend().buffer();
        let text_row = row1.y + 1;
        // full-row control_hover fill on the active row's text line
        assert_eq!(buf.cell((5, text_row)).unwrap().bg, theme.control_hover);
        // the pad row above shows the "▄" cap
        assert_eq!(buf.cell((5, row1.y)).unwrap().symbol(), "\u{2584}");
        // the pad row below shows the "▀" cap
        assert_eq!(buf.cell((5, row1.y + 2)).unwrap().symbol(), "\u{2580}");
    }

    #[test]
    fn hovered_row_stays_compact_with_a_hover_highlight_only() {
        let theme = Theme::dark();
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut map = IndexMap::new();
        for (k, v) in [("a", "1"), ("b", "2"), ("c", "3")] {
            map.insert(
                k.to_string(),
                Entry {
                    value: v.into(),
                    enabled: true,
                },
            );
        }
        let t = TableEditorState {
            selected: Some(0),
            ..TableEditorState::default()
        };
        let hovered = Hit::TableRow(2);
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    f.area(),
                    &map,
                    &ctx(&theme, Some(&hovered)),
                    "+ Add param",
                    &mut hits,
                )
            })
            .unwrap();

        // The hovered row (2) stays compact — hover is a background cue only.
        let row2 = hits.rect_of(&Hit::TableRow(2)).unwrap();
        assert_eq!(row2.height, 1, "hovered row must not expand");
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf.cell((5, row2.y)).unwrap().bg,
            theme.control_hover,
            "hovered row gets the hover background"
        );
        // The selected row (0) is the expanded one.
        let row0 = hits.rect_of(&Hit::TableRow(0)).unwrap();
        assert_eq!(row0.height, 3, "selected row is drawn expanded");
        assert!(
            hits.rect_of(&Hit::TableDelete(0)).is_some(),
            "the selected row carries the ✕ delete affordance"
        );
    }

    #[test]
    fn no_selection_draws_every_row_compact_with_no_delete_affordance() {
        let theme = Theme::dark();
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut map = IndexMap::new();
        map.insert(
            "page".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        let t = TableEditorState::default(); // no selection
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    f.area(),
                    &map,
                    &ctx(&theme, None),
                    "+ Add param",
                    &mut hits,
                )
            })
            .unwrap();
        let row = hits.rect_of(&Hit::TableRow(0)).unwrap();
        assert_eq!(row.height, 1, "no selection: rows stay compact");
        assert!(
            hits.rect_of(&Hit::TableDelete(0)).is_none(),
            "no delete affordance without a selection"
        );
    }

    #[test]
    fn down_selects_first_row_and_esc_deselects() {
        let mut map = IndexMap::new();
        map.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        let mut t = TableEditorState::default();
        assert_eq!(t.selected, None);
        // Space/Enter/d are inert with nothing selected.
        assert!(!t.handle_key(key(KeyCode::Char(' ')), &mut map).consumed);
        assert!(!t.handle_key(key(KeyCode::Char('d')), &mut map).consumed);
        assert!(map["a"].enabled);

        let out = t.handle_key(key(KeyCode::Down), &mut map);
        assert!(out.consumed);
        assert_eq!(t.selected, Some(0), "Down selects the first row");

        let out = t.handle_key(key(KeyCode::Esc), &mut map);
        assert!(out.consumed);
        assert_eq!(t.selected, None, "Esc deselects");
    }

    #[test]
    fn table_height_accounts_for_header_ghost_edge_and_expansion() {
        assert_eq!(table_height(0, None), 3); // header + 0 rows + ghost + edge
        assert_eq!(table_height(3, None), 6);
        assert_eq!(table_height(3, Some(1)), 8); // + 2 for the expanded row
    }
}
