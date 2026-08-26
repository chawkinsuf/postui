use super::palette::fuzzy_match;
use crate::action::Action;
use crate::paint::{self, ControlState, FIELD_HEIGHT, ListRow, RowHighlight, TextField};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

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
        t: f32,
    ) {
        let width = 60.min(screen.width);
        // Chrome (everything but the list): 1 pad + 1 title + 1 ring-margin
        // gap + 3-row field + 1 ring-margin gap + 3 bottom pad (the old
        // hint-row space, kept so the shell doesn't crowd the list).
        const CHROME: u16 = 10;
        let content_rows = (self.filtered.len() as u16).clamp(1, 10);
        let height = (CHROME + content_rows).clamp(13, 26).min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        hits.register(area, crate::hit::Hit::ModalBody);
        paint::floating_panel_settling(frame.buffer_mut(), area, screen, theme, t);
        if t < 1.0 {
            return;
        }

        let title_y = area.y + 1;
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            title_y,
            &self.title,
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

        // No hover-fade animation is wired for popup lists (transient
        // surfaces — see the task report); a hovered row shows its full
        // hover fill immediately, same convention as `DrawCtx::hover_t`'s
        // own documented default when no fade is in flight.
        let hover_t = 1.0;
        for (i, &idx) in self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(list_h.max(1))
        {
            let item = &self.items[idx];
            let text_row = list_area.y + (i - self.scroll) as u16;
            let selected = i == self.selected;
            let row_hovered = hovered == Some(&crate::hit::Hit::ChooserRow(i));
            let highlight = if selected {
                RowHighlight::Selected
            } else if row_hovered {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            ListRow {
                highlight,
                zebra: None,
            }
            .paint(
                frame.buffer_mut(),
                text_row,
                list_area.x,
                list_area.width,
                theme.panel,
                hover_t,
                theme,
            );
            let row_fill = ListRow::resolve_fill(theme, highlight, theme.panel, hover_t);

            let text_x = list_area.x + 1;
            let mut x = text_x;
            let right = list_area.x + list_area.width;
            let label = item.label.as_str();
            let label_w = (label.chars().count() as u16).min(right.saturating_sub(x));
            paint::text(
                frame.buffer_mut(),
                x,
                text_row,
                label,
                theme.text,
                row_fill,
                selected,
            );
            x += label_w;
            if let Some(detail) = &item.detail {
                let detail = format!(" {detail}");
                let w = right.saturating_sub(x);
                paint::text(
                    frame.buffer_mut(),
                    x,
                    text_row,
                    clip(&detail, w),
                    theme.text_muted,
                    row_fill,
                    false,
                );
            }

            let row_rect = Rect {
                x: list_area.x,
                y: text_row,
                width: list_area.width,
                height: 1,
            };
            hits.register(row_rect, crate::hit::Hit::ChooserRow(i));
        }
    }
}

/// Clips `s` to at most `width` columns on a char boundary.
pub(super) fn clip(s: &str, width: u16) -> &str {
    match s.char_indices().nth(width as usize) {
        Some((byte, _)) => &s[..byte],
        None => s,
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
    fn no_key_hint_footer_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut c = ChooserState::new("Projects", items(&["a", "b"]));
        let theme = crate::theme::Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("enter select"), "{content}");
        assert!(!content.contains("esc cancel"), "{content}");
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
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Projects"), "title should render");
        assert!(content.contains("svc"), "first label should render");
        assert!(content.contains("web"), "second label should render");
        assert!(content.contains("/tmp/svc"), "detail should render");
    }

    #[test]
    fn selected_row_is_a_dense_selection_fill_with_an_accent_bar() {
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::ChooserRow(0)).unwrap();
        let buffer = terminal.backend().buffer();
        let bar = buffer[(row0.x, row0.y)].clone();
        assert_eq!(
            bar.symbol(),
            "\u{258c}",
            "the selected (row 0) row must carry the dense accent bar in its first column"
        );
        assert_eq!(bar.fg, theme.accent);
        let right_edge = buffer[(row0.x + row0.width - 1, row0.y)].clone();
        assert_eq!(
            right_edge.bg, theme.selection,
            "the selected row's dense fill must span the full row width"
        );
    }

    #[test]
    fn hovered_row_is_a_control_pill() {
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                c.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    Some(&crate::hit::Hit::ChooserRow(1)),
                    1.0,
                )
            })
            .unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::ChooserRow(1)).unwrap();
        let buffer = terminal.backend().buffer();
        let right_edge = buffer[(row1.x + row1.width - 1, row1.y)].clone();
        assert_eq!(
            right_edge.bg, theme.control,
            "the hovered (non-selected) row's pill fill must span the full row width"
        );
    }

    #[test]
    fn field_fill_and_gap_row_survive_the_list_draw() {
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
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
    fn rows_sit_on_a_dense_one_line_pitch() {
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::ChooserRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::ChooserRow(1)).unwrap();
        let row2 = hits.rect_of(&crate::hit::Hit::ChooserRow(2)).unwrap();
        assert_eq!(row1.y - row0.y, 1, "rows sit on a dense 1-row pitch");
        assert_eq!(row2.y - row1.y, 1, "rows sit on a dense 1-row pitch");
    }

    /// A dense chooser should fit noticeably more rows in the same modal
    /// height than the old 2-line-pitch pill list did — the point of this
    /// task. 13 items at a plain 80×24 terminal must all be reachable via
    /// scroll without the modal's height cap swallowing the tail.
    #[test]
    fn thirteen_items_all_scroll_into_view() {
        let labels: Vec<String> = (0..13).map(|i| format!("item-{i}")).collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut c = ChooserState::new("Projects", items(&label_refs));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        c.select(12);
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        assert!(
            hits.rect_of(&crate::hit::Hit::ChooserRow(12)).is_some(),
            "scrolling to the last of 13 items must bring it into view"
        );
    }
}
