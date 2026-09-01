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
#[derive(Clone, Default)]
pub struct ChooserItem {
    pub label: String,
    pub detail: Option<String>,
    pub actions: Vec<Action>,
    /// A stable identifier the app can read back from the selection
    /// (`ChooserState::selected_id`) without parsing the display label —
    /// the theme picker's live-preview hook.
    pub id: Option<String>,
}

/// An optional two-state switch on a chooser (the theme picker's
/// dark/light filter): `label` renders right-aligned on the title row,
/// and Left/Right (or a click on the label) dispatch `action` without
/// closing the modal — the action's handler is expected to swap the
/// chooser's items via [`ChooserState::set_items`].
pub struct ChooserToggle {
    pub label: String,
    pub action: Action,
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
    toggle: Option<ChooserToggle>,
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
            toggle: None,
        }
    }

    /// Attaches a [`ChooserToggle`] (builder-style, for construction).
    pub fn with_toggle(mut self, label: impl Into<String>, action: Action) -> Self {
        self.toggle = Some(ChooserToggle {
            label: label.into(),
            action,
        });
        self
    }

    /// Updates the toggle's displayed label (e.g. after its action flipped
    /// the state it names). A no-op when no toggle is attached.
    pub fn set_toggle_label(&mut self, label: impl Into<String>) {
        if let Some(t) = &mut self.toggle {
            t.label = label.into();
        }
    }

    /// The toggle's action, if a toggle is attached — what a click on the
    /// toggle label dispatches.
    pub fn toggle_action(&self) -> Option<&Action> {
        self.toggle.as_ref().map(|t| &t.action)
    }

    /// Replaces the item list wholesale (the toggle's handler swapping in
    /// the other set), keeping the typed filter and re-running it. The
    /// selection resets to the first match; callers that want a specific
    /// row follow up with [`Self::select_id`].
    pub fn set_items(&mut self, items: Vec<ChooserItem>) {
        self.items = items;
        self.refilter();
    }

    /// Moves the selection to the filtered row whose item id is `id`, if
    /// one is visible under the current filter; otherwise leaves the
    /// selection where it is.
    pub fn select_id(&mut self, id: &str) {
        if let Some(pos) = self
            .filtered
            .iter()
            .position(|&i| self.items[i].id.as_deref() == Some(id))
        {
            self.select(pos);
        }
    }

    /// Pastes into the filter query (the bracketed-paste/ctrl+v path),
    /// flattened to one line like every single-line surface.
    pub fn paste(&mut self, text: &str) {
        self.input
            .push_str(&crate::components::line_input::flatten_paste(text));
        self.refilter();
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

    /// The `id` of the highlighted item, mapped through the filter — `None`
    /// when nothing is selected or the item carries no id.
    pub fn selected_id(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.items[i].id.as_deref())
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
            // Left/Right fire the toggle (when one is attached) without
            // closing — the filter input has no caret to move, so these
            // keys are otherwise unused here.
            KeyCode::Left | KeyCode::Right => {
                if let Some(t) = &self.toggle {
                    return Some(super::modal::ModalResult {
                        actions: vec![t.action.clone()],
                        close: false,
                        ..Default::default()
                    });
                }
            }
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
        // An empty toggle label means the control is currently inert (the
        // theme picker hides it while an unpaired theme is highlighted):
        // paint nothing and register no hit.
        if let Some(t) = self.toggle.as_ref().filter(|t| !t.label.is_empty()) {
            // Right-aligned on the title row, clickable and flippable with
            // Left/Right — mirrored by `handle_key`.
            let w = t.label.chars().count() as u16;
            let x = (area.x + area.width).saturating_sub(w + 2);
            paint::text(
                frame.buffer_mut(),
                x,
                title_y,
                &t.label,
                theme.accent,
                theme.panel,
                false,
            );
            hits.register(
                Rect {
                    x,
                    y: title_y,
                    width: w,
                    height: 1,
                },
                crate::hit::Hit::ChooserToggle,
            );
        }

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
            let label = clip(item.label.as_str(), right.saturating_sub(x));
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
                ..Default::default()
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
                    ..Default::default()
                },
                ChooserItem {
                    label: "web".into(),
                    detail: Some("/tmp/web".into()),
                    actions: vec![Action::Render],
                    ..Default::default()
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
                    ..Default::default()
                },
                ChooserItem {
                    label: "web".into(),
                    detail: Some("/tmp/web".into()),
                    actions: vec![Action::Render],
                    ..Default::default()
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
    fn left_right_fire_the_toggle_without_closing_and_are_inert_without_one() {
        let mut plain = ChooserState::new("t", items(&["a", "b"]));
        assert!(
            plain.handle_key(key(KeyCode::Left)).is_none(),
            "no toggle: Left is ignored"
        );
        let mut c =
            ChooserState::new("t", items(&["a", "b"])).with_toggle("◂ dark ▸", Action::Quit);
        for code in [KeyCode::Left, KeyCode::Right] {
            let res = c.handle_key(key(code)).unwrap();
            assert!(!res.close, "toggle must not close the modal");
            assert_eq!(res.actions, vec![Action::Quit]);
        }
    }

    #[test]
    fn set_items_keeps_the_typed_filter_and_select_id_finds_a_visible_row() {
        let mut c = ChooserState::new("t", items(&["alpha", "beta"]));
        for ch in "bet".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(c.selected_label(), Some("beta"));
        // Swapping in a new set re-runs the same typed filter over it.
        c.set_items(items(&["betamax", "gamma"]));
        assert_eq!(c.input(), "bet", "filter text survives the swap");
        assert_eq!(c.selected_label(), Some("betamax"));
        // select_id moves to a visible row; an id filtered out is a no-op.
        let mut c = ChooserState::new(
            "t",
            vec![
                ChooserItem {
                    label: "alpha".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("alpha-id".into()),
                },
                ChooserItem {
                    label: "beta".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("beta-id".into()),
                },
            ],
        );
        c.select_id("beta-id");
        assert_eq!(c.selected_id(), Some("beta-id"));
        c.select_id("no-such-id");
        assert_eq!(
            c.selected_id(),
            Some("beta-id"),
            "unknown id leaves selection"
        );
    }

    #[test]
    fn toggle_label_paints_right_aligned_on_the_title_row_and_registers_a_hit() {
        let mut c =
            ChooserState::new("Theme", items(&["a", "b"])).with_toggle("◂ dark ▸", Action::Quit);
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let toggle = hits
            .rect_of(&crate::hit::Hit::ChooserToggle)
            .expect("toggle label registers a clickable hit");
        let modal = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        assert_eq!(toggle.y, modal.y + 1, "sits on the title row");
        assert_eq!(
            toggle.x + toggle.width,
            modal.x + modal.width - 2,
            "right-aligned with the title row's margin"
        );
        let buffer = terminal.backend().buffer();
        let first = buffer[(toggle.x, toggle.y)].clone();
        assert_eq!(first.symbol(), "◂");
        assert_eq!(first.fg, theme.accent);
    }

    #[test]
    fn empty_toggle_label_paints_nothing_and_registers_no_hit() {
        let mut c = ChooserState::new("Theme", items(&["a"])).with_toggle("", Action::Quit);
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        assert!(
            hits.rect_of(&crate::hit::Hit::ChooserToggle).is_none(),
            "hidden toggle must not be clickable"
        );
    }

    #[test]
    fn selected_id_maps_through_the_filter() {
        let mut c = ChooserState::new(
            "Theme",
            vec![
                ChooserItem {
                    label: "alpha".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("alpha-id".into()),
                },
                ChooserItem {
                    label: "beta".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("beta-id".into()),
                },
            ],
        );
        assert_eq!(c.selected_id(), Some("alpha-id"));
        for ch in "bet".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(
            c.selected_id(),
            Some("beta-id"),
            "id follows the filtered selection"
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
