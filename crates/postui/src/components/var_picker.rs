use super::chooser::clip;
use super::palette::fuzzy_match;
use crate::action::Action;
use crate::components::toast::ToastKind;
use crate::components::varmanager::VarEditOp;
use crate::paint::{self, Chip, ControlState, FIELD_HEIGHT, PillRow, RowHighlight, TextField};
use crate::theme::Theme;
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
/// variables (`model.vars`, minus names that are actually group members —
/// a member may carry its own top-level table for a description, but it's
/// listed once, as `Group`), group members (`model.groups`), and the open
/// request's own `[variables]` (`request_vars`; `None`/disabled entries
/// show as unset rather than being dropped). A name defined at more than
/// one scope (a request entry shadowing a project variable) appears once,
/// tagged with the scope that actually resolves — request wins.
pub fn insert_entries(
    model: &postui_core::varmodel::VarModel,
    resolved: &indexmap::IndexMap<String, String>,
    request_vars: &indexmap::IndexMap<String, postui_core::model::Entry>,
) -> Vec<VarEntry> {
    let group_members: std::collections::HashSet<&str> = model
        .groups
        .values()
        .flat_map(|g| g.members.iter().map(String::as_str))
        .collect();

    let mut entries: Vec<VarEntry> = Vec::new();

    for (name, decl) in &model.vars {
        if group_members.contains(name.as_str()) {
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
        for member in &group.members {
            let description = model.vars.get(member).and_then(|d| d.description.clone());
            entries.push(VarEntry {
                name: member.clone(),
                description,
                value: resolved.get(member).cloned(),
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
/// token whose name resolves to an enumerated variable or a group member
/// — `name` is that token's own name; `group` is the owning group's name
/// for a group member (`None` for a plain enumerated variable). Either
/// way the rows shown are the *options*, not the declared names, and
/// `Enter` never touches the token text — it records a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerMode {
    Insert,
    SelectOption { name: String, group: Option<String> },
}

/// One option row, as offered by the `SelectOption` picker: its key,
/// optional description, and either a single resolved `value` (a plain
/// enumerated variable's option) or a pre-formatted `preview` line (a
/// group option's per-member preview, e.g. "admin · user_id 1001 ·
/// customer_id c-77") — never both. `selected` marks the env's current
/// selection for this name/group with a ✓.
#[derive(Clone)]
pub struct SelectEntry {
    pub key: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub preview: Option<String>,
    pub selected: bool,
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
    /// mode's target for `VarEditOp::Select` and its toast. Unused (empty)
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

    /// Opens `SelectOption` mode: `entries` are the merged options for
    /// `name` (or, for a group member, its group — see [`PickerMode`]),
    /// with the env's current selection already marked. `env` is the
    /// active environment, captured for the `Enter` action.
    pub fn new_select(
        entries: Vec<SelectEntry>,
        name: String,
        group: Option<String>,
        env: String,
    ) -> Self {
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
    /// row in `Insert` mode — "new variable…", always the last row
    /// regardless of what's typed (spec §6: the autocomplete "ends with
    /// 'new variable…'").
    fn row_count(&self) -> usize {
        self.filtered.len()
            + if matches!(self.mode, PickerMode::Insert) {
                1
            } else {
                0
            }
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
        if self.mode == PickerMode::Insert && self.selected == self.filtered.len() {
            // The ghost "new variable…" row: open the create-and-insert
            // prompt pre-filled with whatever was typed, and close this
            // picker (not stack on top of it) so focus stays exactly
            // where it was once the prompt itself confirms or cancels.
            return Some(super::modal::ModalResult {
                actions: vec![Action::OpenNewVariablePrompt {
                    prefill: self.input.clone(),
                    completing: self.completing,
                }],
                close: true,
                ..Default::default()
            });
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
            PickerMode::SelectOption { name, group } => {
                // A group member's selection is recorded under the
                // *group's* name (spec §1.2: one selection per group,
                // shared by every member) — the token's own name is only
                // for display (the picker's title).
                let owner = group.clone().unwrap_or_else(|| name.clone());
                let key = self.select_entries[idx].key.clone();
                let toast = format!("{owner} \u{2192} {key} ({})", self.env);
                Some(super::modal::ModalResult {
                    actions: vec![
                        Action::VarEdit(VarEditOp::Select {
                            env: self.env.clone(),
                            name: owner,
                            key,
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
            PickerMode::SelectOption { name, group } => {
                format!("Select \u{2014} {}", group.as_deref().unwrap_or(name))
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
            // The Insert-mode ghost "new variable…" row: sits one past the
            // filtered entries at `filtered.len()`.
            let new_var_row = self.mode == PickerMode::Insert && i == self.filtered.len();
            match &self.mode {
                PickerMode::Insert if new_var_row => {
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        "+ new variable\u{2026}",
                        theme.accent,
                        row_fill,
                        selected,
                    );
                }
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
            },
            SelectEntry {
                key: "bob".into(),
                description: None,
                value: Some("qa-bob".into()),
                preview: None,
                selected: true,
            },
        ];
        let mut p = VarPickerState::new_select(entries, "user".into(), None, "qa".into());
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![
                Action::VarEdit(VarEditOp::Select {
                    env: "qa".into(),
                    name: "user".into(),
                    key: "alice".into(),
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
        }];
        let mut p = VarPickerState::new_select(
            entries,
            "user_id".into(),
            Some("identity".into()),
            "qa".into(),
        );
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![
                Action::VarEdit(VarEditOp::Select {
                    env: "qa".into(),
                    name: "identity".into(),
                    key: "alice".into(),
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
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), None, "qa".into());
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
            },
            SelectEntry {
                key: "bob".into(),
                description: Some("reader".into()),
                value: Some("qa-bob".into()),
                preview: None,
                selected: false,
            },
        ];
        let mut p = VarPickerState::new_select(entries, "user".into(), None, "qa".into());
        for c in "bob".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions[0],
            Action::VarEdit(VarEditOp::Select {
                env: "qa".into(),
                name: "user".into(),
                key: "bob".into(),
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
            },
            SelectEntry {
                key: "bob".into(),
                description: None,
                value: None,
                preview: Some("reader \u{b7} user_id 1002 \u{b7} customer_id c-78".into()),
                selected: false,
            },
        ];
        let mut p = VarPickerState::new_select(
            entries,
            "user_id".into(),
            Some("identity".into()),
            "qa".into(),
        );
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
}
