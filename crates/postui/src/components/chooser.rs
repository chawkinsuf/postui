use super::palette::fuzzy_match;
use crate::action::Action;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};

/// One selectable entry in a `ChooserState`: a label, an optional detail
/// string shown dimmed after the label, and the actions dispatched on
/// selection.
#[derive(Clone)]
pub struct ChooserItem {
    pub label: String,
    pub detail: Option<String>,
    pub actions: Vec<Action>,
}

/// A generic fuzzy-filterable chooser modal. Structure mirrors
/// `PaletteState`: typed input filters `items` by fuzzy-matching against
/// `label + " " + detail`; arrows move the selection; `Enter` dispatches the
/// selected item's actions and closes; `Esc` closes with no actions.
pub struct ChooserState {
    title: String,
    input: String,
    selected: usize,
    items: Vec<ChooserItem>,
    filtered: Vec<usize>,
    /// First visible row's index into `filtered`. Kept in view of `selected`
    /// on the next `draw` whenever `ensure_visible` is set; free to roam
    /// otherwise (wheel scrolling).
    scroll: usize,
    /// Set whenever `selected` changes via keys (or on refilter) so the next
    /// `draw` scrolls it back into view; wheel scrolling clears it so a free
    /// scroll survives the following draw untouched.
    ensure_visible: bool,
}

impl ChooserState {
    pub fn new(title: &str, items: Vec<ChooserItem>) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            title: title.to_string(),
            input: String::new(),
            selected: 0,
            items,
            filtered,
            scroll: 0,
            ensure_visible: true,
        }
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_label(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .map(|&i| self.items[i].label.as_str())
    }

    /// Moves the keyboard/mouse cursor to filtered row `i` (clamped in
    /// range) and asks the next draw to scroll it into view.
    pub fn select(&mut self, i: usize) {
        if i < self.filtered.len() {
            self.selected = i;
            self.ensure_visible = true;
        }
    }

    /// The `ModalResult` an `Enter` (or a confirming click) on the current
    /// selection produces — `None` when nothing is selected (empty filter).
    pub fn confirm(&self) -> Option<super::modal::ModalResult> {
        let &idx = self.filtered.get(self.selected)?;
        Some(super::modal::ModalResult {
            actions: self.items[idx].actions.clone(),
            close: true,
            ..Default::default()
        })
    }

    /// Adjusts `scroll` by `delta` lines, clamped to the filtered list's
    /// bounds, without moving `selected`. A no-op on an empty list.
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
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let haystack = match &item.detail {
                    Some(detail) => format!("{} {}", item.label, detail),
                    None => item.label.clone(),
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
        let height = (self.filtered.len() as u16 + 4)
            .clamp(5, 16)
            .min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        hits.register(area, crate::hit::Hit::ModalBody);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border_focused))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(theme.surface_raised))
            .title(format!(" {} ", self.title))
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
        let list_h = list_area.height as usize;
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

        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(list_h.max(1))
            .map(|(i, &idx)| {
                let item = &self.items[idx];
                let mut style = if i == self.selected {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text)
                };
                if hovered == Some(&crate::hit::Hit::ChooserRow(i)) {
                    style = style.bg(theme.surface_raised);
                }
                let marker = if i == self.selected { "› " } else { "  " };
                let mut spans = vec![
                    Span::styled(marker, style),
                    Span::styled(item.label.as_str(), style),
                ];
                if let Some(detail) = &item.detail {
                    spans.push(Span::styled(
                        format!(" {detail}"),
                        Style::default().fg(theme.text_muted),
                    ));
                }
                let row_area = Rect {
                    x: list_area.x,
                    y: list_area.y + (i - self.scroll) as u16,
                    width: list_area.width,
                    height: 1,
                };
                hits.register(row_area, crate::hit::Hit::ChooserRow(i));
                ListItem::new(Line::from(spans).style(style))
            })
            .collect();
        frame.render_widget(List::new(items), list_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn items(labels: &[&str]) -> Vec<ChooserItem> {
        labels
            .iter()
            .map(|l| ChooserItem {
                label: l.to_string(),
                detail: None,
                actions: vec![Action::Render],
            })
            .collect()
    }

    #[test]
    fn typing_filters_on_label_and_detail_and_enter_returns_actions() {
        let mut c = ChooserState::new(
            "Projects",
            vec![
                ChooserItem {
                    label: "svc".into(),
                    detail: Some("/tmp/svc".into()),
                    actions: vec![Action::Quit],
                },
                ChooserItem {
                    label: "web".into(),
                    detail: Some("/tmp/web".into()),
                    actions: vec![Action::Render],
                },
            ],
        );
        for ch in "tmp/w".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(
            c.selected_label(),
            Some("web"),
            "detail participates in the fuzzy match"
        );
        let res = c.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::Render]);
    }

    #[test]
    fn esc_closes_empty_enter_swallowed_arrows_clamp() {
        let mut c = ChooserState::new("t", items(&["a", "b"]));
        c.handle_key(key(KeyCode::Up));
        c.handle_key(key(KeyCode::Down));
        c.handle_key(key(KeyCode::Down)); // clamped at 1
        assert_eq!(c.selected_label(), Some("b"));
        for ch in "zz".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert!(
            c.handle_key(key(KeyCode::Enter)).is_none(),
            "no match: Enter swallowed"
        );
        let res = c.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn draw_renders_title_labels_and_dim_details() {
        let mut c = ChooserState::new(
            "Projects",
            vec![
                ChooserItem {
                    label: "svc".into(),
                    detail: Some("/tmp/svc".into()),
                    actions: vec![Action::Quit],
                },
                ChooserItem {
                    label: "web".into(),
                    detail: Some("/tmp/web".into()),
                    actions: vec![Action::Render],
                },
            ],
        );
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Projects"), "title should render");
        assert!(content.contains("svc"), "first label should render");
        assert!(content.contains("web"), "second label should render");
        assert!(content.contains("/tmp/svc"), "detail should render");
    }
}
