use super::chooser::clip;
use super::palette::fuzzy_match;
use crate::action::Action;
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

/// A fuzzy-filterable list of declared variables. Structure mirrors
/// `ChooserState`: typed input filters `entries` by fuzzy-matching against
/// `name + " " + description`; arrows move the selection; `Enter` inserts
/// the picked variable's text and closes. `completing` distinguishes
/// whether the picker was triggered mid-`{{` (Enter inserts just the
/// closing `name}}`) or explicitly (Enter inserts the full `{{name}}`
/// token); `Esc` always just closes — a typed `{{` that triggered the
/// picker is left as literal text in that case.
pub struct VarPickerState {
    input: String,
    selected: usize,
    entries: Vec<VarEntry>,
    filtered: Vec<usize>,
    pub completing: bool,
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
            filtered,
            completing,
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
        self.filtered = self
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
            .collect();
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
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            title_y,
            "Variables",
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
            let entry = &self.entries[idx];
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

            let row_rect = Rect {
                x: list_area.x,
                y: text_row,
                width: list_area.width,
                height: 1,
            };
            hits.register(row_rect, crate::hit::Hit::VarPickerRow(i));
        }

        let footer_y = area.y + area.height.saturating_sub(2);
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            footer_y,
            "enter insert  esc cancel",
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
