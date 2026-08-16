use super::line_input::LineInput;
use crate::action::Action;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

/// What a `Modal::Prompt`'s confirmed text becomes: which `Action` it maps
/// to, and (for rename) which slug is prefilled/being renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    NewRequest,
    RenameRequest { from: String },
    SaveAs,
    OpenProjectPath,
    SaveBodyAs,
}

pub enum Modal {
    Message {
        title: String,
        body: String,
    },
    /// A choice prompt: each entry in `choices` is `(key, label, actions)` —
    /// pressing `key` (case-insensitive) dispatches `actions` and closes the
    /// modal; `Esc` closes with no actions.
    Confirm {
        title: String,
        body: String,
        choices: Vec<(char, String, Vec<Action>)>,
    },
    /// A single-line text prompt (new request name, rename, save-as).
    /// `Enter` on non-empty text closes and dispatches the action matching
    /// `kind`; `Enter` on empty text is swallowed; `Esc` closes with no
    /// action.
    Prompt {
        title: String,
        input: LineInput,
        kind: PromptKind,
    },
    Palette(crate::components::palette::PaletteState),
    Chooser(crate::components::chooser::ChooserState),
    VarPicker(crate::components::var_picker::VarPickerState),
    /// The "new project" prompt: a name field and a path field, tab/down
    /// (or shift-tab/up) switching focus between them. On the first hop
    /// off the name field, if the path still ends with `/`, the name is
    /// slugified and appended so the path stays a sensible default while
    /// still being freely editable afterward.
    NewProject {
        name: LineInput,
        path: LineInput,
        on_path: bool,
        /// Whether the one-shot name->path prefill has already happened.
        prefilled: bool,
    },
    /// An anchored popup list (currently just the method selector): opens
    /// just below `anchor` (flipping above it when that would cross the
    /// screen bottom), Up/Down move `selected`, Enter dispatches the
    /// selected row's action and closes.
    Dropdown(DropdownState),
}

/// State for `Modal::Dropdown`: the cell it opens from, its `(label,
/// action)` rows, which row the keyboard cursor is on, and (separately)
/// which row reflects the value already in effect (e.g. the method that
/// was active when the dropdown opened). `selected` moves as the user
/// arrows through the list; `current` does not — it stays put so the `✓`
/// marker keeps pointing at the actual current value even while the
/// highlight is elsewhere.
pub struct DropdownState {
    pub anchor: Rect,
    pub items: Vec<(String, Action)>,
    pub selected: usize,
    pub current: Option<usize>,
}

/// The outcome of a modal handling a key event: any actions the caller
/// should dispatch, and whether the modal should be popped off the stack.
/// The stack never pops itself — the caller pops on `close`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModalResult {
    pub actions: Vec<Action>,
    pub close: bool,
    /// The command id to record a use of, when this result came from
    /// confirming a command palette row. `None` for every other modal.
    pub usage: Option<String>,
}

#[derive(Default)]
pub struct ModalStack {
    stack: Vec<Modal>,
}

impl ModalStack {
    pub fn push(&mut self, modal: Modal) {
        self.stack.push(modal);
    }

    pub fn pop(&mut self) -> Option<Modal> {
        self.stack.pop()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn top(&self) -> Option<&Modal> {
        self.stack.last()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ModalResult> {
        let top = self.stack.last_mut()?;
        match top {
            Modal::Message { .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter => Some(ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                }),
                _ => None, // swallowed: modals capture all input
            },
            Modal::Confirm { choices, .. } => match key.code {
                KeyCode::Esc => Some(ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Char(c) => {
                    let c = c.to_ascii_lowercase();
                    choices
                        .iter()
                        .find(|(choice, _, _)| choice.to_ascii_lowercase() == c)
                        .map(|(_, _, actions)| ModalResult {
                            actions: actions.clone(),
                            close: true,
                            ..Default::default()
                        })
                }
                _ => None, // swallowed: modals capture all input
            },
            Modal::Prompt { input, kind, .. } => match key.code {
                KeyCode::Esc => Some(ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Enter => {
                    let text = input.text().trim();
                    if text.is_empty() {
                        None // swallowed: nothing to confirm yet
                    } else {
                        let action = match kind {
                            PromptKind::NewRequest => Action::CreateRequest(text.to_string()),
                            PromptKind::RenameRequest { from } => Action::RenameRequest {
                                from: from.clone(),
                                to: text.to_string(),
                            },
                            PromptKind::SaveAs => Action::SaveRequestAs(text.to_string()),
                            PromptKind::OpenProjectPath => {
                                Action::OpenProjectByPath(text.to_string())
                            }
                            PromptKind::SaveBodyAs => Action::SaveBodyToFile(text.to_string()),
                        };
                        Some(ModalResult {
                            actions: vec![action],
                            close: true,
                            ..Default::default()
                        })
                    }
                }
                _ => {
                    input.handle_key(key);
                    None // swallowed: modals capture all input
                }
            },
            Modal::Palette(state) => state.handle_key(key),
            Modal::Chooser(state) => state.handle_key(key),
            Modal::VarPicker(state) => state.handle_key(key),
            Modal::NewProject {
                name,
                path,
                on_path,
                prefilled,
            } => match key.code {
                KeyCode::Esc => Some(ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Enter => {
                    let name_text = name.text().trim();
                    if name_text.is_empty() {
                        None // swallowed: nothing to confirm yet
                    } else {
                        Some(ModalResult {
                            actions: vec![Action::CreateProject {
                                name: name_text.to_string(),
                                path: path.text().trim().to_string(),
                            }],
                            close: true,
                            ..Default::default()
                        })
                    }
                }
                KeyCode::Tab | KeyCode::Down => {
                    if !*on_path && !*prefilled {
                        *prefilled = true;
                        let slug = slugify(name.text());
                        if path.text().ends_with('/') && !slug.is_empty() {
                            let mut new_path = path.text().to_string();
                            new_path.push_str(&slug);
                            *path = LineInput::new(&new_path);
                        }
                    }
                    *on_path = true;
                    None // swallowed: modals capture all input
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *on_path = false;
                    None // swallowed: modals capture all input
                }
                _ => {
                    if *on_path {
                        path.handle_key(key);
                    } else {
                        name.handle_key(key);
                    }
                    None // swallowed: modals capture all input
                }
            },
            Modal::Dropdown(state) => match key.code {
                KeyCode::Up => {
                    state.selected = state.selected.saturating_sub(1);
                    None // swallowed: modals capture all input
                }
                KeyCode::Down => {
                    if state.selected + 1 < state.items.len() {
                        state.selected += 1;
                    }
                    None // swallowed: modals capture all input
                }
                KeyCode::Enter => Some(ModalResult {
                    actions: vec![state.items[state.selected].1.clone()],
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Esc => Some(ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                }),
                _ => None, // swallowed: modals capture all input
            },
        }
    }

    /// The top modal, mutably — used by `App::on_hit` to read (and clone)
    /// the action for a clicked `DropdownRow` before popping the modal.
    pub fn top_mut(&mut self) -> Option<&mut Modal> {
        self.stack.last_mut()
    }

    /// Scrolls the top modal's list by `delta` lines (positive = down),
    /// without moving its selection — the wheel-over-an-open-modal
    /// behavior. A no-op (returns `false`) for modals with no scrollable
    /// list, or when the stack is empty.
    pub fn scroll_top(&mut self, delta: i16) -> bool {
        match self.stack.last_mut() {
            Some(Modal::Palette(state)) => {
                state.scroll_by(delta);
                true
            }
            Some(Modal::Chooser(state)) => {
                state.scroll_by(delta);
                true
            }
            Some(Modal::VarPicker(state)) => {
                state.scroll_by(delta);
                true
            }
            _ => false,
        }
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        hits: &mut crate::hit::HitMap,
        hovered: Option<&crate::hit::Hit>,
    ) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        // Every variant dims the backdrop except Dropdown: it's a small
        // anchored popup (e.g. the method selector), not a screen-owning
        // modal, so dimming everything behind it would be jarring.
        if !matches!(top, Modal::Dropdown(_)) {
            dim_backdrop(frame, screen);
        }
        // Registered before the modal's own hits so any click landing
        // outside them (topmost-wins in `HitMap`) closes the modal, same as
        // Esc — live for every variant, not just Dropdown.
        hits.register(screen, crate::hit::Hit::ModalOutside);
        match top {
            Modal::Message { title, body } => {
                let area = centered_rect(screen, 60.min(screen.width), 9);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.border_focused))
                    .padding(Padding::uniform(1))
                    .style(Style::default().bg(theme.surface_raised))
                    .title(format!(" {title} "))
                    .title_style(Style::default().fg(theme.accent));
                frame.render_widget(Clear, area);
                frame.render_widget(
                    Paragraph::new(body.as_str())
                        .style(Style::default().fg(theme.text))
                        .wrap(Wrap { trim: false })
                        .block(block),
                    area,
                );
            }
            Modal::Confirm {
                title,
                body,
                choices,
            } => {
                let area = centered_rect(screen, 60.min(screen.width), 9);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.border_focused))
                    .padding(Padding::uniform(1))
                    .style(Style::default().bg(theme.surface_raised))
                    .title(format!(" {title} "))
                    .title_style(Style::default().fg(theme.accent));
                frame.render_widget(Clear, area);
                let inner = block.inner(area);
                frame.render_widget(block, area);

                let body_area = Rect {
                    height: inner.height.saturating_sub(2),
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(body.as_str())
                        .style(Style::default().fg(theme.text))
                        .wrap(Wrap { trim: false }),
                    body_area,
                );

                // Each `[c] Label` choice is its own clickable chip
                // (`Hit::ConfirmChoice(c)`); `[esc] Cancel` stays plain text
                // since Esc already closes the modal from anywhere.
                let hint_area = Rect {
                    y: inner.y + inner.height.saturating_sub(1),
                    height: 1,
                    ..inner
                };
                let mut x = hint_area.x;
                for (c, label, _) in choices.iter() {
                    let text = format!("[{c}] {label}");
                    let w = (text.chars().count() as u16).min(hint_area.right().saturating_sub(x));
                    let chip_area = Rect {
                        x,
                        y: hint_area.y,
                        width: w,
                        height: 1,
                    };
                    crate::hit::chip(
                        frame,
                        hits,
                        chip_area,
                        &text,
                        crate::hit::Hit::ConfirmChoice(*c),
                        hovered,
                        None,
                        theme,
                    );
                    x = (x + w + 3).min(hint_area.right());
                }
                let esc_area = Rect {
                    x,
                    y: hint_area.y,
                    width: hint_area.right().saturating_sub(x),
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "[esc] Cancel",
                        Style::default().fg(theme.text_muted),
                    )),
                    esc_area,
                );
            }
            Modal::Prompt { title, input, .. } => {
                let area = centered_rect(screen, 60.min(screen.width), 6);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.border_focused))
                    .padding(Padding::uniform(1))
                    .style(Style::default().bg(theme.surface_raised))
                    .title(format!(" {title} "))
                    .title_style(Style::default().fg(theme.accent));
                frame.render_widget(Clear, area);
                let inner = block.inner(area);
                frame.render_widget(block, area);

                let input_area = Rect { height: 1, ..inner };
                frame.render_widget(
                    Paragraph::new(input.draw_line_windowed(true, theme, input_area.width)),
                    input_area,
                );

                let hint_area = Rect {
                    y: inner.y + 2,
                    height: 1,
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "[enter] confirm  [esc] cancel",
                        Style::default().fg(theme.text_muted),
                    ))),
                    hint_area,
                );
            }
            Modal::Palette(state) => state.draw(frame, screen, theme, hits, hovered),
            Modal::Chooser(state) => state.draw(frame, screen, theme, hits, hovered),
            Modal::VarPicker(state) => state.draw(frame, screen, theme, hits, hovered),
            Modal::NewProject {
                name,
                path,
                on_path,
                ..
            } => {
                let area = centered_rect(screen, 60.min(screen.width), 8);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.border_focused))
                    .padding(Padding::uniform(1))
                    .style(Style::default().bg(theme.surface_raised))
                    .title(" New project ")
                    .title_style(Style::default().fg(theme.accent));
                frame.render_widget(Clear, area);
                let inner = block.inner(area);
                frame.render_widget(block, area);

                let name_label_area = Rect { height: 1, ..inner };
                frame.render_widget(
                    Paragraph::new(Span::styled("Name:", Style::default().fg(theme.text_muted))),
                    name_label_area,
                );
                let name_area = Rect {
                    y: inner.y + 1,
                    height: 1,
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(name.draw_line_windowed(!*on_path, theme, name_area.width)),
                    name_area,
                );

                let path_label_area = Rect {
                    y: inner.y + 2,
                    height: 1,
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(Span::styled("Path:", Style::default().fg(theme.text_muted))),
                    path_label_area,
                );
                let path_area = Rect {
                    y: inner.y + 3,
                    height: 1,
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(path.draw_line_windowed(*on_path, theme, path_area.width)),
                    path_area,
                );

                let hint_area = Rect {
                    y: inner.y + 4,
                    height: 1,
                    ..inner
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "[tab] switch  [enter] create  [esc] cancel",
                        Style::default().fg(theme.text_muted),
                    ))),
                    hint_area,
                );
            }
            Modal::Dropdown(state) => draw_dropdown(frame, screen, theme, hits, hovered, state),
        }
    }
}

/// Draws `state`'s popup at `anchor.x, anchor.y + 1`, flipping to
/// `anchor.y - height` when it would cross the screen bottom, clamped
/// horizontally (and vertically) to stay on screen. Registers
/// `Hit::ModalOutside` over the whole screen first (so any other click
/// closes the popup), then `Hit::DropdownRow(i)` per row. `hovered` gets
/// the row-hover treatment other lists use (`bg(theme.surface_raised)`
/// against the popup's plain `theme.surface` base — see sidebar rows).
fn draw_dropdown(
    frame: &mut Frame,
    screen: Rect,
    theme: &Theme,
    hits: &mut crate::hit::HitMap,
    hovered: Option<&crate::hit::Hit>,
    state: &DropdownState,
) {
    let max_label = state
        .items
        .iter()
        .map(|(label, _)| label.chars().count() as u16)
        .max()
        .unwrap_or(0);
    let width = (max_label + 4).min(screen.width);
    let height = (state.items.len() as u16 + 2).min(screen.height);

    let mut x = state.anchor.x;
    if x + width > screen.x + screen.width {
        x = (screen.x + screen.width).saturating_sub(width);
    }
    x = x.max(screen.x);

    let below_y = state.anchor.y + 1;
    let y = if below_y + height > screen.y + screen.height {
        state.anchor.y.saturating_sub(height)
    } else {
        below_y
    };
    let y = y.clamp(screen.y, (screen.y + screen.height).saturating_sub(height));

    let area = Rect {
        x,
        y,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_focused))
        .style(Style::default().bg(theme.surface));
    frame.render_widget(Clear, area);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    for (i, (label, _)) in state.items.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let row_area = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        // `current` (the value already in effect) gets the checkmark;
        // `selected` (the keyboard cursor) gets its own bold/accent
        // highlight — the two can differ once arrow keys move the cursor
        // away from the current value.
        let marker = if state.current == Some(i) {
            "✓ "
        } else {
            "  "
        };
        let mut style = if i == state.selected {
            Style::default().fg(theme.accent).bold()
        } else {
            Style::default().fg(theme.text)
        };
        if hovered == Some(&crate::hit::Hit::DropdownRow(i)) {
            style = style.bg(theme.surface_raised);
        }
        frame.render_widget(
            Paragraph::new(Line::styled(format!("{marker}{label}"), style)),
            row_area,
        );
        hits.register(row_area, crate::hit::Hit::DropdownRow(i));
    }
}

/// Lowercases `s`, maps spaces to `-`, and keeps only `[a-z0-9_-]`
/// characters — used to prefill the new-project path from its name.
pub fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-')
        .collect()
}

pub fn centered_rect(screen: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(screen.width);
    let h = height.min(screen.height);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}

pub fn dim_backdrop(frame: &mut Frame, screen: Rect) {
    frame
        .buffer_mut()
        .set_style(screen, Style::default().add_modifier(Modifier::DIM));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn centered_rect_is_centered_and_clamped() {
        let screen = Rect::new(0, 0, 100, 40);
        let r = centered_rect(screen, 60, 10);
        assert_eq!(r, Rect::new(20, 15, 60, 10));
        let clamped = centered_rect(screen, 200, 90);
        assert_eq!(clamped.width, 100);
        assert_eq!(clamped.height, 40);
    }

    #[test]
    fn esc_closes_top_modal_only() {
        let mut m = ModalStack::default();
        m.push(Modal::Message {
            title: "A".into(),
            body: "a".into(),
        });
        m.push(Modal::Message {
            title: "B".into(),
            body: "b".into(),
        });
        let res = m.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close);
        assert!(res.actions.is_empty());
        // the stack does not pop itself: the caller pops on close.
        assert_eq!(m.stack.len(), 2);
    }

    #[test]
    fn other_keys_are_swallowed_by_message_modal() {
        let mut m = ModalStack::default();
        m.push(Modal::Message {
            title: "A".into(),
            body: "a".into(),
        });
        assert!(
            m.handle_key(key(KeyCode::Char('q'))).is_none(),
            "keys must not leak through a modal to global bindings"
        );
    }

    #[test]
    fn palette_enter_returns_action_and_closes() {
        let mut m = ModalStack::default();
        m.push(Modal::Palette(
            crate::components::palette::PaletteState::new(&crate::usage::UsageStore::default(), 0),
        ));
        for c in "quit".chars() {
            assert!(m.handle_key(key(KeyCode::Char(c))).is_none());
        }
        let res = m.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::Quit]);
        // note: the STACK does not pop itself — the caller pops on close.
        assert!(!m.is_empty());
    }

    #[test]
    fn message_modal_closes_without_action() {
        let mut m = ModalStack::default();
        m.push(Modal::Message {
            title: "t".into(),
            body: "b".into(),
        });
        let res = m.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn top_returns_the_top_modal() {
        let mut m = ModalStack::default();
        assert!(m.top().is_none());
        m.push(Modal::Message {
            title: "t".into(),
            body: "b".into(),
        });
        assert!(matches!(m.top(), Some(Modal::Message { .. })));
    }

    #[test]
    fn draw_renders_title_and_body() {
        let mut m = ModalStack::default();
        m.push(Modal::Message {
            title: "About".into(),
            body: "hello world".into(),
        });
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| m.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("About"));
        assert!(content.contains("hello world"));
    }

    #[test]
    fn slugify_lowercases_and_maps_spaces() {
        assert_eq!(slugify("My Svc"), "my-svc");
        assert_eq!(slugify("Weird!! Na@me_1"), "weird-name_1");
    }

    fn dropdown_items() -> Vec<(String, Action)> {
        vec![
            ("GET".into(), Action::Render),
            ("POST".into(), Action::Render),
            ("PUT".into(), Action::Render),
        ]
    }

    #[test]
    fn dropdown_up_down_clamp_and_enter_returns_selected_action() {
        let mut m = ModalStack::default();
        m.push(Modal::Dropdown(DropdownState {
            anchor: Rect::new(0, 0, 8, 1),
            items: dropdown_items(),
            selected: 0,
            current: Some(0),
        }));
        assert!(
            m.handle_key(key(KeyCode::Up)).is_none(),
            "clamped at top, swallowed"
        );
        assert!(m.handle_key(key(KeyCode::Down)).is_none());
        let res = m.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::Render]); // row 1
    }

    #[test]
    fn dropdown_esc_closes_without_action_and_swallows_other_keys() {
        let mut m = ModalStack::default();
        m.push(Modal::Dropdown(DropdownState {
            anchor: Rect::new(0, 0, 8, 1),
            items: dropdown_items(),
            selected: 0,
            current: Some(0),
        }));
        assert!(
            m.handle_key(key(KeyCode::Char('q'))).is_none(),
            "keys must not leak through a dropdown to global bindings"
        );
        let res = m.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn top_mut_returns_the_top_modal_mutably() {
        let mut m = ModalStack::default();
        m.push(Modal::Dropdown(DropdownState {
            anchor: Rect::new(0, 0, 8, 1),
            items: dropdown_items(),
            selected: 0,
            current: Some(0),
        }));
        let Some(Modal::Dropdown(state)) = m.top_mut() else {
            panic!("expected a Dropdown on top");
        };
        state.selected = 2;
        let Some(Modal::Dropdown(state)) = m.top() else {
            panic!("expected a Dropdown on top");
        };
        assert_eq!(state.selected, 2, "mutation through top_mut must persist");
    }

    #[test]
    fn dropdown_flips_upward_near_the_screen_bottom() {
        let screen = Rect::new(0, 0, 80, 24);
        // Anchor sits one row above the bottom: drawing below it would
        // cross the screen edge, so the popup must flip above instead.
        let anchor = Rect::new(10, 23, 8, 1);
        let state = DropdownState {
            anchor,
            items: dropdown_items(),
            selected: 0,
            current: Some(0),
        };
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| draw_dropdown(f, screen, &Theme::dark(), &mut hits, None, &state))
            .unwrap();
        let row0 = hits
            .rect_of(&crate::hit::Hit::DropdownRow(0))
            .expect("row 0 registered");
        assert!(
            row0.y < anchor.y,
            "flipped-up popup rows must sit above the anchor row, got y={}",
            row0.y
        );
    }

    #[test]
    fn dropdown_hovered_row_gets_raised_background_others_dont() {
        let screen = Rect::new(0, 0, 80, 24);
        let state = DropdownState {
            anchor: Rect::new(10, 5, 8, 1),
            items: dropdown_items(),
            selected: 0,
            current: Some(0),
        };
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                draw_dropdown(
                    f,
                    screen,
                    &theme,
                    &mut hits,
                    Some(&crate::hit::Hit::DropdownRow(1)),
                    &state,
                )
            })
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::DropdownRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::DropdownRow(1)).unwrap();
        let row2 = hits.rect_of(&crate::hit::Hit::DropdownRow(2)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(row1.x, row1.y)].bg,
            theme.surface_raised,
            "hovered row gets the raised hover background"
        );
        assert_ne!(
            buffer[(row0.x, row0.y)].bg,
            theme.surface_raised,
            "non-hovered rows keep the popup's plain background"
        );
        assert_ne!(buffer[(row2.x, row2.y)].bg, theme.surface_raised);
    }

    #[test]
    fn dropdown_checkmark_stays_on_current_when_cursor_moves() {
        let mut m = ModalStack::default();
        m.push(Modal::Dropdown(DropdownState {
            anchor: Rect::new(0, 0, 8, 1),
            items: dropdown_items(),
            selected: 0,
            current: Some(0),
        }));
        // Move the keyboard cursor down without confirming — `current`
        // (the checkmark) must not follow it.
        m.handle_key(key(KeyCode::Down));
        let Some(Modal::Dropdown(state)) = m.top() else {
            panic!("expected a Dropdown on top");
        };
        assert_eq!(state.selected, 1, "cursor moved");
        assert_eq!(
            state.current,
            Some(0),
            "checkmark stays on the original row"
        );
    }
}
