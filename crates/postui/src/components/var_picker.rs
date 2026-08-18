use super::chooser::clip;
use super::palette::fuzzy_match;
use crate::action::Action;
use crate::components::toast::ToastKind;
use crate::components::varmanager::VarEditOp;
use crate::paint::{self, ControlState, FIELD_HEIGHT, PillRow, RowHighlight, TextField};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// One declared variable, as offered by the picker: its name, optional
/// description (from `variables.toml`), and resolved value (from
/// `prepare_context().vars`, when the variable has one).
#[derive(Clone)]
pub struct VarEntry {
    pub name: String,
    pub description: Option<String>,
    pub value: Option<String>,
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

    /// Moves the cursor to filtered row `i` (clamped in range) and asks the
    /// next draw to scroll it into view.
    pub fn select(&mut self, i: usize) {
        if i < self.filtered.len() {
            self.selected = i;
            self.ensure_visible = true;
        }
    }

    /// The `ModalResult` an `Enter` (or a confirming click) on the current
    /// selection produces — `None` when nothing is selected.
    pub fn confirm(&self) -> Option<super::modal::ModalResult> {
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
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len().saturating_sub(1);
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
                if self.selected + 1 < self.filtered.len() {
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
        let content_rows = (self.filtered.len() as u16).clamp(1, 10) * 2;
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
                let max_scroll = self.filtered.len().saturating_sub(list_h);
                self.scroll = self.scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        for (i, &idx) in self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(list_h.max(1))
        {
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
            match &self.mode {
                PickerMode::Insert => {
                    let entry = &self.entries[idx];
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
                PickerMode::SelectOption { .. } => {
                    let entry = &self.select_entries[idx];
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

    #[test]
    fn enter_emits_completion_or_full_token() {
        let entries = vec![VarEntry {
            name: "base_url".into(),
            description: None,
            value: Some("x".into()),
        }];
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
        let mut p = VarPickerState::new(
            vec![VarEntry {
                name: "a".into(),
                description: None,
                value: None,
            }],
            true,
        );
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn typing_filters_on_name_and_description() {
        let mut p = VarPickerState::new(
            vec![
                VarEntry {
                    name: "base".into(),
                    description: Some("api root".into()),
                    value: None,
                },
                VarEntry {
                    name: "tok".into(),
                    description: None,
                    value: Some("secret".into()),
                },
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
                VarEntry {
                    name: "base".into(),
                    description: Some("api root".into()),
                    value: Some("http://x".into()),
                },
                VarEntry {
                    name: "tok".into(),
                    description: None,
                    value: None,
                },
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
        names
            .iter()
            .map(|n| VarEntry {
                name: n.to_string(),
                description: None,
                value: None,
            })
            .collect()
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
