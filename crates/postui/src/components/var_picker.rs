use super::chooser::clip;
use super::palette::fuzzy_match;
use crate::action::Action;
use crate::components::toast::ToastKind;
use crate::components::varmanager::VarEditOp;
use crate::paint::{self, Chip, ControlState, FIELD_HEIGHT, PillRow, RowHighlight, TextField};
use crate::theme::Theme;
use indexmap::IndexMap;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Which of the three name sources an Insert-mode [`VarEntry`] comes from
/// (spec §6: "scope-badged (request / project / group member)"). A name
/// shadowed at a more specific scope (a request `[variables]` entry
/// overriding a project variable of the same name) is listed once, tagged
/// with the scope that actually resolves — `Request` beats `Project`/
/// `Group`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarScope {
    Request,
    Project,
    Group,
}

impl VarScope {
    fn badge(self) -> &'static str {
        match self {
            VarScope::Request => "req",
            VarScope::Project => "proj",
            VarScope::Group => "grp",
        }
    }
}

/// One declared variable, as offered by the Insert-mode picker: its name,
/// optional description (from `variables.toml`), resolved value (from
/// `prepare_context().vars`, when the variable has one), which scope
/// declares it, and whether it's a secret (its value is never shown even
/// when `value` is `Some` — the row renders a masked placeholder instead).
#[derive(Clone)]
pub struct VarEntry {
    pub name: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub scope: VarScope,
    pub secret: bool,
}

/// Builds the Insert-mode picker's entries (spec §6: "autocomplete over
/// all defined names") from every source that declares one: project
/// variables (`model.vars`, minus names that are actually group fields —
/// a field may carry its own top-level table for a description, but it's
/// listed once, as `Group`), group fields (`model.groups`), and the open
/// request's own `[variables]` (`request_vars`; `None`/disabled entries
/// show as unset rather than being dropped). A name defined at more than
/// one scope (a request entry shadowing a project variable) appears once,
/// tagged with the scope that actually resolves — request wins.
pub fn insert_entries(
    model: &postui_core::varmodel::VarModel,
    resolved: &indexmap::IndexMap<String, String>,
    request_vars: &indexmap::IndexMap<String, postui_core::model::Entry>,
) -> Vec<VarEntry> {
    let group_fields: std::collections::HashSet<&str> = model
        .groups
        .values()
        .flat_map(|g| g.fields.iter().map(String::as_str))
        .collect();

    let mut entries: Vec<VarEntry> = Vec::new();

    for (name, decl) in &model.vars {
        if group_fields.contains(name.as_str()) {
            continue;
        }
        entries.push(VarEntry {
            name: name.clone(),
            description: decl.description.clone(),
            value: resolved.get(name).cloned(),
            scope: VarScope::Project,
            secret: decl.secret,
        });
    }

    for group in model.groups.values() {
        for field in &group.fields {
            let description = model.vars.get(field).and_then(|d| d.description.clone());
            entries.push(VarEntry {
                name: field.clone(),
                description,
                value: resolved.get(field).cloned(),
                scope: VarScope::Group,
                secret: false,
            });
        }
    }

    for (name, entry) in request_vars {
        let value = entry.enabled.then(|| entry.value.clone());
        match entries.iter_mut().find(|e| &e.name == name) {
            Some(existing) => {
                existing.scope = VarScope::Request;
                existing.value = value;
                existing.secret = false;
            }
            None => entries.push(VarEntry {
                name: name.clone(),
                description: None,
                value,
                scope: VarScope::Request,
                secret: false,
            }),
        }
    }

    entries
}

/// Which flow [`VarPickerState`] is running (spec §6's two picker
/// contexts). `Insert` is today's autocomplete-over-all-names behavior
/// (Task 15 upgrades its entries with scope badges). `SelectOption` is
/// Task 14's second context: the cursor sat on an existing `{{name}}`
/// token whose name is a group field — `name` is that token's own name
/// and `group` is the owning group's. The rows shown are the group's
/// *entries*, not the declared names, and `Enter` never touches the token
/// text — it records a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerMode {
    Insert,
    SelectOption { name: String, group: String },
}

/// One entry row, as offered by the `SelectOption` picker: its name,
/// optional description, and a pre-formatted `preview` line (the entry's
/// per-field values, e.g. "admin · user_id 1001 · customer_id c-77").
/// `value` is the single-value form kept for a one-field preview — never
/// set alongside `preview`. `selected` marks the env's current selection
/// for this group with a ✓.
#[derive(Clone)]
pub struct SelectEntry {
    pub key: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub preview: Option<String>,
    pub selected: bool,
    /// The raw per-field values this option carries — `None` for a plain
    /// variable option (whose single value lives in `value` instead);
    /// `Some(member -> new value)` for a group option, in member order.
    /// `e` (Task 17, spec §6) prefills its edit-in-place fields from this
    /// rather than re-parsing `preview`'s formatted text.
    pub values: Option<IndexMap<String, String>>,
}

/// A fuzzy-filterable list of declared variables (`Insert` mode) or of one
/// variable's/group's options (`SelectOption` mode). Structure mirrors
/// `ChooserState`: typed input filters the active list by fuzzy-matching;
/// arrows move the selection; `Enter` confirms (inserts a token in
/// `Insert` mode, records a selection in `SelectOption` mode) and closes.
/// `completing` (only meaningful in `Insert` mode) distinguishes whether
/// the picker was triggered mid-`{{` (Enter inserts just the closing
/// `name}}`) or explicitly (Enter inserts the full `{{name}}` token);
/// `Esc` always just closes — a typed `{{` that triggered the picker is
/// left as literal text in that case.
pub struct VarPickerState {
    input: String,
    selected: usize,
    entries: Vec<VarEntry>,
    select_entries: Vec<SelectEntry>,
    filtered: Vec<usize>,
    pub completing: bool,
    pub mode: PickerMode,
    /// The active environment's name, captured at open — `SelectOption`
    /// mode's target for `VarEditOp::SelectEntry` and its toast. Unused (empty)
    /// in `Insert` mode.
    env: String,
    /// First visible row's index into `filtered`. See `ChooserState` for the
    /// `ensure_visible` contract this mirrors.
    scroll: usize,
    ensure_visible: bool,
}

impl VarPickerState {
    pub fn new(entries: Vec<VarEntry>, completing: bool) -> Self {
        let filtered = (0..entries.len()).collect();
        Self {
            input: String::new(),
            selected: 0,
            entries,
            select_entries: Vec::new(),
            filtered,
            completing,
            mode: PickerMode::Insert,
            env: String::new(),
            scroll: 0,
            ensure_visible: true,
        }
    }

    /// Opens `SelectOption` mode: `entries` are `group`'s entries in the
    /// active environment (`name` is the token that led here — see
    /// [`PickerMode`]), with the env's current selection already marked.
    /// `env` is the active environment, captured for the `Enter` action.
    /// The current filter text (test-visible: the click-a-token flow seeds
    /// it, spec §7).
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Pre-seeds the fuzzy filter (and re-filters), so the picker opens
    /// already narrowed — clicking an inline `{{token}}` seeds it with that
    /// token's name (spec §7).
    pub fn seed_filter(&mut self, text: &str) {
        self.input = text.to_string();
        self.refilter();
    }

    pub fn new_select(entries: Vec<SelectEntry>, name: String, group: String, env: String) -> Self {
        let filtered = (0..entries.len()).collect();
        Self {
            input: String::new(),
            selected: 0,
            entries: Vec::new(),
            select_entries: entries,
            filtered,
            completing: false,
            mode: PickerMode::SelectOption { name, group },
            env,
            scroll: 0,
            ensure_visible: true,
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Total selectable rows: the filtered entries, plus one extra ghost
    /// row, always the last row regardless of what's typed — "new
    /// variable…" in `Insert` mode (spec §6: the autocomplete "ends with
    /// 'new variable…'"), "add new option…" in `SelectOption` mode (Task
    /// 17, spec §6's in-context "Add new option…" flow).
    fn row_count(&self) -> usize {
        self.filtered.len() + 1
    }

    /// Moves the cursor to row `i` (clamped in range, including the
    /// `Insert`-mode ghost row) and asks the next draw to scroll it into
    /// view.
    pub fn select(&mut self, i: usize) {
        if i < self.row_count() {
            self.selected = i;
            self.ensure_visible = true;
        }
    }

    /// The `ModalResult` an `Enter` (or a confirming click) on the current
    /// selection produces — `None` when nothing is selected.
    pub fn confirm(&self) -> Option<super::modal::ModalResult> {
        if self.selected == self.filtered.len() {
            // The ghost row: open the create-and-insert prompt (`Insert`
            // mode) or the "add new option…" prompt (`SelectOption` mode),
            // pre-filled with whatever was typed, and close this picker
            // (not stack on top of it) so focus stays exactly where it was
            // once the prompt itself confirms or cancels.
            return match &self.mode {
                PickerMode::Insert => Some(super::modal::ModalResult {
                    actions: vec![Action::OpenNewVariablePrompt {
                        prefill: self.input.clone(),
                        completing: self.completing,
                    }],
                    close: true,
                    ..Default::default()
                }),
                PickerMode::SelectOption { group, .. } => {
                    let owner = group.clone();
                    Some(super::modal::ModalResult {
                        actions: vec![Action::OpenNewOptionInlinePrompt { owner }],
                        close: true,
                        ..Default::default()
                    })
                }
            };
        }
        let &idx = self.filtered.get(self.selected)?;
        match &self.mode {
            PickerMode::Insert => {
                let name = &self.entries[idx].name;
                let text = if self.completing {
                    format!("{name}}}}}")
                } else {
                    format!("{{{{{name}}}}}")
                };
                Some(super::modal::ModalResult {
                    actions: vec![Action::InsertVarText(text)],
                    close: true,
                    ..Default::default()
                })
            }
            PickerMode::SelectOption { group, .. } => {
                // The selection is recorded under the *group's* name
                // (spec §3.1: one selected entry per group, shared by
                // every field) — the token's own name is only for display
                // (the picker's title).
                let owner = group.clone();
                let key = self.select_entries[idx].key.clone();
                let toast = format!("{owner} \u{2192} {key} ({})", self.env);
                Some(super::modal::ModalResult {
                    actions: vec![
                        Action::VarEdit(VarEditOp::SelectEntry {
                            env: self.env.clone(),
                            group: owner,
                            entry: key,
                        }),
                        Action::ShowToast(toast, ToastKind::Success),
                    ],
                    close: true,
                    ..Default::default()
                })
            }
        }
    }

    /// Adjusts `scroll` by `delta` lines, clamped, without moving
    /// `selected`. A no-op on an empty list.
    pub fn scroll_by(&mut self, delta: i16) {
        if self.row_count() == 0 {
            return;
        }
        let max = self.row_count().saturating_sub(1);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
        self.ensure_visible = false;
    }

    fn refilter(&mut self) {
        self.filtered = match &self.mode {
            PickerMode::Insert => self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    let haystack = match &entry.description {
                        Some(desc) => format!("{} {}", entry.name, desc),
                        None => entry.name.clone(),
                    };
                    fuzzy_match(&self.input, &haystack)
                })
                .map(|(i, _)| i)
                .collect(),
            PickerMode::SelectOption { .. } => self
                .select_entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    let mut haystack = entry.key.clone();
                    if let Some(desc) = &entry.description {
                        haystack.push(' ');
                        haystack.push_str(desc);
                    }
                    if let Some(preview) = &entry.preview {
                        haystack.push(' ');
                        haystack.push_str(preview);
                    }
                    fuzzy_match(&self.input, &haystack)
                })
                .map(|(i, _)| i)
                .collect(),
        };
        self.selected = 0;
        self.scroll = 0;
        self.ensure_visible = true;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<super::modal::ModalResult> {
        match key.code {
            KeyCode::Esc => {
                return Some(super::modal::ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                });
            }
            KeyCode::Enter => return self.confirm(),
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.ensure_visible = true;
            }
            KeyCode::Down => {
                if self.selected + 1 < self.row_count() {
                    self.selected += 1;
                }
                self.ensure_visible = true;
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refilter();
            }
            // `e` on a highlighted option (Task 17, spec §6): edit its
            // value(s)/description in place. Unmodified `e` doubles as the
            // fuzzy filter's own text otherwise, so this only fires in
            // `SelectOption` mode, where the filter is over option keys
            // (not the far more `e`-prone declared variable names) and
            // "arrow to a row, then edit" is the expected flow.
            KeyCode::Char('e')
                if key.modifiers.is_empty()
                    && matches!(self.mode, PickerMode::SelectOption { .. }) =>
            {
                if self.selected == self.filtered.len() {
                    return None; // the ghost row itself has nothing to edit
                }
                let &idx = self.filtered.get(self.selected)?;
                let PickerMode::SelectOption { group, .. } = &self.mode else {
                    unreachable!("guarded above")
                };
                let owner = group.clone();
                let entry = &self.select_entries[idx];
                let values = entry.values.clone().unwrap_or_else(|| {
                    let mut m = IndexMap::new();
                    m.insert("value".to_string(), entry.value.clone().unwrap_or_default());
                    m
                });
                return Some(super::modal::ModalResult {
                    actions: vec![Action::OpenEditOptionPrompt {
                        owner,
                        key: entry.key.clone(),
                        description: entry.description.clone(),
                        values,
                    }],
                    close: true,
                    ..Default::default()
                });
            }
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                self.input.push(c);
                self.refilter();
            }
            _ => {}
        }
        None
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        hits: &mut crate::hit::HitMap,
        hovered: Option<&crate::hit::Hit>,
    ) {
        let width = 60.min(screen.width);
        const CHROME: u16 = 10;
        let content_rows = (self.row_count() as u16).clamp(1, 10) * 2;
        let height = (CHROME + content_rows).clamp(13, 26).min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        hits.register(area, crate::hit::Hit::ModalBody);
        paint::floating_panel(frame.buffer_mut(), area, screen, theme);

        let title_y = area.y + 1;
        let title = match &self.mode {
            PickerMode::Insert => "Variables".to_string(),
            PickerMode::SelectOption { group, .. } => {
                format!("Select \u{2014} {group}")
            }
        };
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            title_y,
            &title,
            theme.text,
            theme.panel,
            true,
        );

        let field_area = Rect {
            x: area.x + 1,
            y: title_y + 2,
            width: area.width.saturating_sub(2),
            height: FIELD_HEIGHT,
        };
        let content = Line::from(vec![
            Span::raw(self.input.clone()),
            Span::styled("▏", Style::default().fg(theme.accent)),
        ]);
        TextField {
            content,
            state: ControlState::Focused,
        }
        .paint(frame.buffer_mut(), field_area, theme);

        let list_area = Rect {
            x: area.x + 1,
            y: field_area.y + FIELD_HEIGHT + 2,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(CHROME),
        };
        let list_h = (list_area.height / 2) as usize;
        if self.ensure_visible {
            if list_h > 0 {
                if self.selected < self.scroll {
                    self.scroll = self.selected;
                } else if self.selected >= self.scroll + list_h {
                    self.scroll = self.selected + 1 - list_h;
                }
                let max_scroll = self.row_count().saturating_sub(list_h);
                self.scroll = self.scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        let row_count = self.row_count();
        for i in (self.scroll..row_count).take(list_h.max(1)) {
            let text_row = list_area.y + ((i - self.scroll) as u16) * 2;
            let selected = i == self.selected;
            let row_hovered = hovered == Some(&crate::hit::Hit::VarPickerRow(i));
            let highlight = if selected {
                RowHighlight::Selected
            } else if row_hovered {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            let row_fill = match highlight {
                RowHighlight::None => theme.panel,
                RowHighlight::Hover => theme.control,
                RowHighlight::Selected => theme.control_hover,
            };
            PillRow { highlight }.paint(
                frame.buffer_mut(),
                text_row,
                list_area.x,
                list_area.width,
                area,
                theme.panel,
                theme,
            );

            let right = list_area.x + list_area.width;
            let mut x = list_area.x + 1;
            // The ghost row: sits one past the filtered entries at
            // `filtered.len()` — "new variable…" in `Insert` mode, "add
            // new option…" in `SelectOption` mode.
            let ghost_row = i == self.filtered.len();
            if ghost_row {
                let label = match &self.mode {
                    PickerMode::Insert => "+ new variable\u{2026}",
                    PickerMode::SelectOption { .. } => "+ add new option\u{2026}",
                };
                paint::text(
                    frame.buffer_mut(),
                    x,
                    text_row,
                    label,
                    theme.accent,
                    row_fill,
                    selected,
                );
                let row_rect = Rect {
                    x: list_area.x,
                    y: text_row,
                    width: list_area.width,
                    height: 1,
                };
                hits.register(row_rect, crate::hit::Hit::VarPickerRow(i));
                continue;
            }
            match &self.mode {
                PickerMode::Insert => {
                    let entry = &self.entries[self.filtered[i]];
                    let badge_w = Chip {
                        label: entry.scope.badge(),
                        color: theme.text_muted,
                    }
                    .paint(frame.buffer_mut(), x, text_row, row_fill, theme);
                    x += badge_w;
                    if entry.secret {
                        // The lock glyph is double-width in most terminals
                        // (unlike the ✓ used elsewhere in this file) — use
                        // its real display width, not its char count, so
                        // the name after it doesn't overlap the glyph's
                        // second cell.
                        const LOCK: &str = "\u{1f512} ";
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            LOCK,
                            theme.warning,
                            row_fill,
                            false,
                        );
                        x += Span::raw(LOCK).width() as u16;
                    }
                    let name_w = (entry.name.chars().count() as u16).min(right.saturating_sub(x));
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        &entry.name,
                        theme.text,
                        row_fill,
                        selected,
                    );
                    x += name_w;
                    if let Some(desc) = &entry.description {
                        let desc = format!(" {desc}");
                        let w = right.saturating_sub(x);
                        let clipped = clip(&desc, w);
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            clipped,
                            theme.text_muted,
                            row_fill,
                            false,
                        );
                        x += clipped.chars().count() as u16;
                    }
                    if entry.secret {
                        // Secret VALUES never render, per whether or not one
                        // is resolved — a masked placeholder stands in.
                        let w = right.saturating_sub(x);
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            clip(" \u{25cf}\u{25cf}\u{25cf}\u{25cf}", w),
                            theme.text_muted,
                            row_fill,
                            false,
                        );
                    } else {
                        match &entry.value {
                            Some(v) => {
                                let s = format!(" = {v}");
                                let w = right.saturating_sub(x);
                                paint::text(
                                    frame.buffer_mut(),
                                    x,
                                    text_row,
                                    clip(&s, w),
                                    theme.text_muted,
                                    row_fill,
                                    false,
                                );
                            }
                            None => {
                                let w = right.saturating_sub(x);
                                paint::text(
                                    frame.buffer_mut(),
                                    x,
                                    text_row,
                                    clip(" unset", w),
                                    theme.warning,
                                    row_fill,
                                    false,
                                );
                            }
                        }
                    }
                }
                PickerMode::SelectOption { .. } => {
                    let entry = &self.select_entries[self.filtered[i]];
                    // A fixed two-column check gutter (mirrors the accent
                    // bar's own column) so unchecked rows still line their
                    // keys up under checked ones.
                    let check = if entry.selected { "\u{2713} " } else { "  " };
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        check,
                        theme.success,
                        row_fill,
                        false,
                    );
                    x += 2;
                    let key_w = (entry.key.chars().count() as u16).min(right.saturating_sub(x));
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        &entry.key,
                        theme.text,
                        row_fill,
                        selected,
                    );
                    x += key_w;
                    if let Some(preview) = &entry.preview {
                        // Group option: one pre-formatted preview line
                        // covering every member's new value.
                        let s = format!(" \u{2014} {preview}");
                        let w = right.saturating_sub(x);
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            clip(&s, w),
                            theme.text_muted,
                            row_fill,
                            false,
                        );
                    } else {
                        if let Some(desc) = &entry.description {
                            let desc = format!(" {desc}");
                            let w = right.saturating_sub(x);
                            let clipped = clip(&desc, w);
                            paint::text(
                                frame.buffer_mut(),
                                x,
                                text_row,
                                clipped,
                                theme.text_muted,
                                row_fill,
                                false,
                            );
                            x += clipped.chars().count() as u16;
                        }
                        if let Some(v) = &entry.value {
                            let s = format!(" = {v}");
                            let w = right.saturating_sub(x);
                            paint::text(
                                frame.buffer_mut(),
                                x,
                                text_row,
                                clip(&s, w),
                                theme.text_muted,
                                row_fill,
                                false,
                            );
                        }
                    }
                }
            }

            let row_rect = Rect {
                x: list_area.x,
                y: text_row,
                width: list_area.width,
                height: 1,
            };
            hits.register(row_rect, crate::hit::Hit::VarPickerRow(i));
        }

        let footer = match &self.mode {
            PickerMode::Insert => "enter insert  esc cancel",
            PickerMode::SelectOption { .. } => "enter select  esc cancel",
        };
        let footer_y = area.y + area.height.saturating_sub(2);
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            footer_y,
            footer,
            theme.text_muted,
            theme.panel,
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A `VarScope::Project`, non-secret entry — the common case for tests
    /// that don't care about badges.
    fn var_entry(name: &str, description: Option<&str>, value: Option<&str>) -> VarEntry {
        VarEntry {
            name: name.to_string(),
            description: description.map(str::to_string),
            value: value.map(str::to_string),
            scope: VarScope::Project,
            secret: false,
        }
    }

    #[test]
    fn insert_entries_merges_a_name_declared_at_both_project_and_request_scope_into_one_row() {
        let mut model = postui_core::varmodel::VarModel::default();
        model.vars.insert(
            "trace_id".to_string(),
            postui_core::varmodel::VarDecl {
                description: Some("project-level default".to_string()),
                default: Some("proj-value".to_string()),
                secret: false,
            },
        );
        let mut resolved = indexmap::IndexMap::new();
        resolved.insert("trace_id".to_string(), "proj-value".to_string());
        let mut request_vars = indexmap::IndexMap::new();
        request_vars.insert(
            "trace_id".to_string(),
            postui_core::model::Entry {
                value: "req-value".to_string(),
                enabled: true,
            },
        );

        let entries = insert_entries(&model, &resolved, &request_vars);

        assert_eq!(
            entries.len(),
            1,
            "one name defined at two scopes is one row, not two"
        );
        let entry = &entries[0];
        assert_eq!(entry.name, "trace_id");
        assert_eq!(entry.scope, VarScope::Request, "request shadows project");
        assert_eq!(
            entry.value.as_deref(),
            Some("req-value"),
            "the request's own value wins, not the resolved project value"
        );
        assert!(!entry.secret);
    }

    #[test]
    fn enter_emits_completion_or_full_token() {
        let entries = vec![var_entry("base_url", None, Some("x"))];
        let mut p = VarPickerState::new(entries.clone(), true);
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::InsertVarText("base_url}}".into())]
        );
        let mut p = VarPickerState::new(entries, false);
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::InsertVarText("{{base_url}}".into())]
        );
    }

    #[test]
    fn esc_closes_with_no_actions() {
        let mut p = VarPickerState::new(vec![var_entry("a", None, None)], true);
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn typing_filters_on_name_and_description() {
        let mut p = VarPickerState::new(
            vec![
                var_entry("base", Some("api root"), None),
                var_entry("tok", None, Some("secret")),
            ],
            false,
        );
        for c in "root".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(res.actions, vec![Action::InsertVarText("{{base}}".into())]);
    }

    #[test]
    fn draw_renders_names_values_and_unset_tag() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(
            vec![
                var_entry("base", Some("api root"), Some("http://x")),
                var_entry("tok", None, None),
            ],
            false,
        );
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("base"));
        assert!(content.contains("api root"));
        assert!(content.contains("http://x"));
        assert!(content.contains("unset"));
    }

    fn entries(names: &[&str]) -> Vec<VarEntry> {
        names.iter().map(|n| var_entry(n, None, None)).collect()
    }

    #[test]
    fn field_fill_and_gap_row_survive_the_list_draw() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let area = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let title_y = area.y + 1;
        let field_area = Rect {
            x: area.x + 1,
            y: title_y + 2,
            width: area.width.saturating_sub(2),
            height: FIELD_HEIGHT,
        };
        let buffer = terminal.backend().buffer();
        // The focused field's lifted fill reaches its bottom bevel row...
        let lifted = crate::theme::lift_color(theme.control, 0.12);
        let bevel = buffer[(field_area.x, field_area.y + FIELD_HEIGHT - 1)].clone();
        assert_eq!(bevel.bg, lifted, "field bottom row keeps the lifted fill");
        // ...and row 0's top pad must not creep into the gap row below it.
        let gap = buffer[(field_area.x, field_area.y + FIELD_HEIGHT)].clone();
        assert_eq!(gap.bg, theme.panel, "gap row below the field stays panel");
        assert_eq!(gap.symbol(), " ");
    }

    #[test]
    fn selected_row_is_a_control_hover_pill_with_an_accent_bar() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::VarPickerRow(0)).unwrap();
        let buffer = terminal.backend().buffer();
        let bar = buffer[(row0.x, row0.y)].clone();
        assert_eq!(
            bar.symbol(),
            "\u{2588}",
            "the selected (row 0) row must carry the full-block accent bar in its first column"
        );
        assert_eq!(bar.fg, theme.accent);
        let right_edge = buffer[(row0.x + row0.width - 1, row0.y)].clone();
        assert_eq!(
            right_edge.bg, theme.control_hover,
            "the selected row's pill fill must span the full row width"
        );
    }

    #[test]
    fn hovered_row_is_a_control_pill() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                p.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    Some(&crate::hit::Hit::VarPickerRow(1)),
                )
            })
            .unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::VarPickerRow(1)).unwrap();
        let buffer = terminal.backend().buffer();
        let right_edge = buffer[(row1.x + row1.width - 1, row1.y)].clone();
        assert_eq!(
            right_edge.bg, theme.control,
            "the hovered (non-selected) row's pill fill must span the full row width"
        );
    }

    #[test]
    fn select_mode_enter_emits_select_edit_and_toast() {
        let entries = vec![
            SelectEntry {
                key: "alice".into(),
                description: Some("admin".into()),
                value: Some("qa-token".into()),
                preview: None,
                selected: false,
                values: None,
            },
            SelectEntry {
                key: "bob".into(),
                description: None,
                value: Some("qa-bob".into()),
                preview: None,
                selected: true,
                values: None,
            },
        ];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![
                Action::VarEdit(VarEditOp::SelectEntry {
                    env: "qa".into(),
                    group: "user".into(),
                    entry: "alice".into(),
                }),
                Action::ShowToast("user \u{2192} alice (qa)".into(), ToastKind::Success),
            ]
        );
        assert!(res.close);
    }

    #[test]
    fn select_mode_group_member_targets_the_group_name() {
        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: None,
            value: None,
            preview: Some("admin \u{b7} user_id 1001 \u{b7} customer_id c-77".into()),
            selected: false,
            values: None,
        }];
        let mut p =
            VarPickerState::new_select(entries, "user_id".into(), "identity".into(), "qa".into());
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![
                Action::VarEdit(VarEditOp::SelectEntry {
                    env: "qa".into(),
                    group: "identity".into(),
                    entry: "alice".into(),
                }),
                Action::ShowToast("identity \u{2192} alice (qa)".into(), ToastKind::Success),
            ]
        );
    }

    #[test]
    fn select_mode_esc_closes_without_editing() {
        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: None,
            value: Some("x".into()),
            preview: None,
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn select_mode_typing_filters_options() {
        let entries = vec![
            SelectEntry {
                key: "alice".into(),
                description: Some("admin".into()),
                value: Some("qa-token".into()),
                preview: None,
                selected: false,
                values: None,
            },
            SelectEntry {
                key: "bob".into(),
                description: Some("reader".into()),
                value: Some("qa-bob".into()),
                preview: None,
                selected: false,
                values: None,
            },
        ];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        for c in "bob".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions[0],
            Action::VarEdit(VarEditOp::SelectEntry {
                env: "qa".into(),
                group: "user".into(),
                entry: "bob".into(),
            })
        );
    }

    #[test]
    fn select_mode_draw_marks_current_selection_and_renders_group_preview() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let entries = vec![
            SelectEntry {
                key: "alice".into(),
                description: None,
                value: None,
                preview: Some("admin \u{b7} user_id 1001 \u{b7} customer_id c-77".into()),
                selected: true,
                values: None,
            },
            SelectEntry {
                key: "bob".into(),
                description: None,
                value: None,
                preview: Some("reader \u{b7} user_id 1002 \u{b7} customer_id c-78".into()),
                selected: false,
                values: None,
            },
        ];
        let mut p =
            VarPickerState::new_select(entries, "user_id".into(), "identity".into(), "qa".into());
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("\u{2713}"), "checked row shows a ✓");
        assert!(content.contains("alice"));
        assert!(content.contains("admin"));
        assert!(content.contains("user_id 1001"));
        assert!(content.contains("customer_id c-77"));
        assert!(content.contains("Select"));
        assert!(content.contains("identity"));
    }

    #[test]
    fn rows_sit_on_the_sidebar_s_two_line_pitch() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::VarPickerRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::VarPickerRow(1)).unwrap();
        let row2 = hits.rect_of(&crate::hit::Hit::VarPickerRow(2)).unwrap();
        assert_eq!(row1.y - row0.y, 2, "rows sit on a 2-row pitch");
        assert_eq!(row2.y - row1.y, 2, "rows sit on a 2-row pitch");
    }

    // --- Task 17: in-context flows (spec §6) -------------------------------

    #[test]
    fn select_mode_ghost_row_opens_new_option_inline_prompt() {
        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: Some("admin".into()),
            value: Some("qa-token".into()),
            preview: None,
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        // Down once lands on the ghost row (row 1, one past the one entry).
        p.handle_key(key(KeyCode::Down));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::OpenNewOptionInlinePrompt {
                owner: "user".into()
            }]
        );
        assert!(res.close);
    }

    #[test]
    fn select_mode_ghost_row_targets_the_group_name() {
        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: None,
            value: None,
            preview: Some("admin".into()),
            selected: false,
            values: Some(IndexMap::new()),
        }];
        let mut p =
            VarPickerState::new_select(entries, "user_id".into(), "identity".into(), "qa".into());
        p.handle_key(key(KeyCode::Down));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::OpenNewOptionInlinePrompt {
                owner: "identity".into()
            }]
        );
    }

    #[test]
    fn select_mode_ghost_row_renders_add_new_option_label() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: None,
            value: Some("x".into()),
            preview: None,
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("add new option"), "{content}");
    }

    #[test]
    fn e_on_a_plain_var_option_opens_edit_prompt_prefilled_with_value_and_description() {
        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: Some("admin".into()),
            value: Some("qa-token".into()),
            preview: None,
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        let res = p
            .handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
            .unwrap();
        let mut values = IndexMap::new();
        values.insert("value".to_string(), "qa-token".to_string());
        assert_eq!(
            res.actions,
            vec![Action::OpenEditOptionPrompt {
                owner: "user".into(),
                key: "alice".into(),
                description: Some("admin".into()),
                values,
            }]
        );
        assert!(res.close);
    }

    #[test]
    fn e_on_a_group_option_opens_edit_prompt_prefilled_with_every_member() {
        let mut values = IndexMap::new();
        values.insert("user_id".to_string(), "1001".to_string());
        values.insert("customer_id".to_string(), "c-77".to_string());
        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: Some("admin".into()),
            value: None,
            preview: Some("admin \u{b7} user_id 1001 \u{b7} customer_id c-77".into()),
            selected: false,
            values: Some(values.clone()),
        }];
        let mut p =
            VarPickerState::new_select(entries, "user_id".into(), "identity".into(), "qa".into());
        let res = p
            .handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(
            res.actions,
            vec![Action::OpenEditOptionPrompt {
                owner: "identity".into(),
                key: "alice".into(),
                description: Some("admin".into()),
                values,
            }]
        );
    }

    #[test]
    fn e_on_the_ghost_row_is_a_no_op() {
        let entries = vec![SelectEntry {
            key: "alice".into(),
            description: None,
            value: Some("x".into()),
            preview: None,
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        p.handle_key(key(KeyCode::Down));
        assert!(
            p.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
                .is_none()
        );
    }

    #[test]
    fn e_in_insert_mode_types_into_the_filter_instead_of_editing() {
        let mut p = VarPickerState::new(entries(&["env"]), false);
        p.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(res.actions, vec![Action::InsertVarText("{{env}}".into())]);
    }
}
