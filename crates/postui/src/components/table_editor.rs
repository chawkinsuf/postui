use super::DrawCtx;
use super::line_input::LineInput;
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::Entry;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

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
}

impl TableOutcome {
    fn consumed() -> Self {
        Self {
            consumed: true,
            warning: None,
        }
    }

    fn not_consumed() -> Self {
        Self::default()
    }

    fn warn(warning: String) -> Self {
        Self {
            consumed: true,
            warning: Some(warning),
        }
    }
}

/// Shared cursor/edit state for a key/value table (Params or Headers tab).
/// One instance is reused across both tabs; the caller passes in whichever
/// `IndexMap` is currently active.
#[derive(Debug, Default)]
pub struct TableEditorState {
    pub selected: usize,
    pub editing: Option<CellEdit>,
}

impl TableEditorState {
    /// Resets cursor/edit state; used when switching tabs so a selection
    /// index from one map can't be stale (and panic) against the other.
    pub fn reset(&mut self) {
        self.selected = 0;
        self.editing = None;
    }

    fn clamp_selected(&mut self, map: &IndexMap<String, Entry>) {
        if map.is_empty() {
            self.selected = 0;
        } else if self.selected >= map.len() {
            self.selected = map.len() - 1;
        }
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
                self.selected = (self.selected + 1).min(map.len() - 1);
                TableOutcome::consumed()
            }
            KeyCode::Char('k') | KeyCode::Up => {
                // Row 0 (and an empty table) leaves Up unconsumed so the
                // caller (Editor) can fall back to moving focus to the URL
                // line instead of leaving the user stuck with no way back.
                if map.is_empty() || self.selected == 0 {
                    return TableOutcome::not_consumed();
                }
                self.selected -= 1;
                TableOutcome::consumed()
            }
            KeyCode::Char('a') => {
                self.selected = map.len();
                self.editing = Some(CellEdit {
                    col: Col::Key,
                    input: LineInput::new(""),
                    original_key: None,
                    pending_key: None,
                });
                TableOutcome::consumed()
            }
            KeyCode::Enter => {
                if map.is_empty() {
                    return TableOutcome::not_consumed();
                }
                self.clamp_selected(map);
                let key = map
                    .get_index(self.selected)
                    .map(|(k, _)| k.clone())
                    .unwrap();
                self.editing = Some(CellEdit {
                    col: Col::Key,
                    input: LineInput::new(&key),
                    original_key: Some(key),
                    pending_key: None,
                });
                TableOutcome::consumed()
            }
            KeyCode::Char(' ') => {
                if map.is_empty() {
                    return TableOutcome::not_consumed();
                }
                self.clamp_selected(map);
                if let Some((_, e)) = map.get_index_mut(self.selected) {
                    e.enabled = !e.enabled;
                }
                TableOutcome::consumed()
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if map.is_empty() {
                    return TableOutcome::not_consumed();
                }
                self.clamp_selected(map);
                map.shift_remove_index(self.selected);
                self.clamp_selected(map);
                TableOutcome::consumed()
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
            map.get_index(self.selected)
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
        let idx = self.selected;
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
        self.selected = map.len() - 1;
        None
    }

    /// Draws the table: columns `[✓/✗] key  value`, selected row
    /// accent-highlighted with a `›` marker, disabled rows muted, the
    /// editing cell showing the `LineInput` cursor, and an empty-state
    /// message when `map` has no rows and no append is in progress.
    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        map: &IndexMap<String, Entry>,
        ctx: &DrawCtx,
        empty_label: &str,
    ) {
        let theme = ctx.theme;
        if map.is_empty() && self.editing.is_none() {
            frame.render_widget(
                Paragraph::new(empty_label).style(Style::default().fg(theme.text_muted)),
                area,
            );
            return;
        }

        let selected = self.selected.min(map.len().saturating_sub(1));
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, (k, e)) in map.iter().enumerate() {
            let is_selected = !map.is_empty() && i == selected;
            let editing_this = is_selected
                && self
                    .editing
                    .as_ref()
                    .map(|ed| ed.original_key.is_some())
                    .unwrap_or(false);
            lines.push(self.render_row(k, e, is_selected, editing_this, theme));
        }
        if let Some(edit) = &self.editing
            && edit.original_key.is_none()
        {
            lines.push(self.render_new_row(edit, theme));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_row(
        &self,
        key: &str,
        entry: &Entry,
        selected: bool,
        editing_this: bool,
        theme: &Theme,
    ) -> Line<'static> {
        let marker = if selected { "\u{203a} " } else { "  " };
        let check = if entry.enabled {
            "\u{2713}"
        } else {
            "\u{2717}"
        };
        let base_style = if !entry.enabled {
            Style::default().fg(theme.text_muted)
        } else if selected {
            Style::default().fg(theme.accent).bold()
        } else {
            Style::default().fg(theme.text)
        };

        let (key_line, value_line) = if editing_this {
            let edit = self
                .editing
                .as_ref()
                .expect("editing_this implies editing is Some");
            match edit.col {
                Col::Key => (
                    edit.input.draw_line(true, theme),
                    Line::styled(entry.value.clone(), base_style),
                ),
                Col::Value => (
                    Line::styled(
                        edit.pending_key.clone().unwrap_or_else(|| key.to_string()),
                        base_style,
                    ),
                    edit.input.draw_line(true, theme),
                ),
            }
        } else {
            (
                Line::styled(key.to_string(), base_style),
                Line::styled(entry.value.clone(), base_style),
            )
        };

        let mut spans = vec![Span::styled(format!("{marker}{check} "), base_style)];
        spans.extend(key_line.spans);
        spans.push(Span::raw("  "));
        spans.extend(value_line.spans);
        Line::from(spans)
    }

    fn render_new_row(&self, edit: &CellEdit, theme: &Theme) -> Line<'static> {
        let base_style = Style::default().fg(theme.accent).bold();
        let (key_line, value_line) = match edit.col {
            Col::Key => (
                edit.input.draw_line(true, theme),
                Line::styled(String::new(), base_style),
            ),
            Col::Value => (
                Line::styled(edit.pending_key.clone().unwrap_or_default(), base_style),
                edit.input.draw_line(true, theme),
            ),
        };
        let mut spans = vec![Span::styled("\u{203a} \u{2713} ".to_string(), base_style)];
        spans.extend(key_line.spans);
        spans.push(Span::raw("  "));
        spans.extend(value_line.spans);
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn space_toggles_d_deletes_esc_cancels() {
        let mut map = IndexMap::new();
        map.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        let mut t = TableEditorState::default();
        t.handle_key(key(KeyCode::Char(' ')), &mut map);
        assert!(!map["a"].enabled);
        t.handle_key(key(KeyCode::Enter), &mut map); // start editing
        t.handle_key(key(KeyCode::Char('x')), &mut map);
        t.handle_key(key(KeyCode::Esc), &mut map); // cancel
        assert_eq!(map["a"].value, "1", "esc discards the edit");
        t.handle_key(key(KeyCode::Char('d')), &mut map);
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
            selected: 0,
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
            selected: 0,
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

    #[test]
    fn draw_shows_empty_state_and_rows() {
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
        };
        let backend = TestBackend::new(40, 5);
        let mut terminal = Terminal::new(backend).unwrap();

        let empty_map: IndexMap<String, Entry> = IndexMap::new();
        let t = TableEditorState::default();
        terminal
            .draw(|f| {
                t.draw(
                    f,
                    f.area(),
                    &empty_map,
                    &ctx,
                    "No params yet — press a to add",
                )
            })
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("No params yet"), "empty state: {content}");

        let mut map = IndexMap::new();
        map.insert(
            "page".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        terminal
            .draw(|f| t.draw(f, f.area(), &map, &ctx, "unused"))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("page"), "key text: {content}");
        assert!(content.contains('2'), "value text: {content}");
    }
}
