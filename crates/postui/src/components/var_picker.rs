use super::palette::fuzzy_match;
use crate::action::Action;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};

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
        }
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
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<super::modal::ModalResult> {
        match key.code {
            KeyCode::Esc => {
                return Some(super::modal::ModalResult {
                    actions: vec![],
                    close: true,
                });
            }
            KeyCode::Enter => {
                let &idx = self.filtered.get(self.selected)?;
                let name = &self.entries[idx].name;
                let text = if self.completing {
                    format!("{name}}}}}")
                } else {
                    format!("{{{{{name}}}}}")
                };
                return Some(super::modal::ModalResult {
                    actions: vec![Action::InsertVarText(text)],
                    close: true,
                });
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
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
        &self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        _hits: &mut crate::hit::HitMap,
    ) {
        let width = 60.min(screen.width);
        let height = (self.filtered.len() as u16 + 4)
            .clamp(5, 16)
            .min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border_focused))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(theme.surface_raised))
            .title(" Variables ")
            .title_style(Style::default().fg(theme.accent));
        frame.render_widget(Clear, area);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let prompt = Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(self.input.as_str(), Style::default().fg(theme.text)),
            Span::styled("▏", Style::default().fg(theme.accent)),
        ]);
        let prompt_area = Rect { height: 1, ..inner };
        frame.render_widget(Paragraph::new(prompt), prompt_area);

        let list_area = Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        };
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(i, &idx)| {
                let entry = &self.entries[idx];
                let style = if i == self.selected {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text)
                };
                let marker = if i == self.selected { "› " } else { "  " };
                let mut spans = vec![
                    Span::styled(marker, style),
                    Span::styled(entry.name.clone(), style),
                ];
                if let Some(desc) = &entry.description {
                    spans.push(Span::styled(
                        format!(" {desc}"),
                        Style::default().fg(theme.text_muted),
                    ));
                }
                match &entry.value {
                    Some(v) => spans.push(Span::styled(
                        format!(" = {v}"),
                        Style::default().fg(theme.text_muted),
                    )),
                    None => spans.push(Span::styled(" unset", Style::default().fg(theme.warning))),
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        frame.render_widget(List::new(items), list_area);
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

        let p = VarPickerState::new(
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
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("base"));
        assert!(content.contains("api root"));
        assert!(content.contains("http://x"));
        assert!(content.contains("unset"));
    }
}
