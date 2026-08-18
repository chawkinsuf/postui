use super::line_input::LineInput;
use crate::action::Action;
use crate::paint::{
    self, BUTTON_HEIGHT, Button, ButtonKind, ControlState, FIELD_HEIGHT, TextField,
};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Paragraph, Wrap};

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
            paint::dim_backdrop(frame.buffer_mut(), screen);
        }
        // Registered before the modal's own hits so any click landing
        // outside them (topmost-wins in `HitMap`) closes the modal, same as
        // Esc — live for every variant, not just Dropdown.
        hits.register(screen, crate::hit::Hit::ModalOutside);
        match top {
            Modal::Message { title, body } => {
                let area = centered_rect(screen, 60.min(screen.width), 13.min(screen.height));
                hits.register(area, crate::hit::Hit::ModalBody);
                paint::floating_panel(frame.buffer_mut(), area, screen, theme);

                let title_y = area.y + 1;
                paint::text(
                    frame.buffer_mut(),
                    area.x + 2,
                    title_y,
                    title,
                    theme.text,
                    theme.panel,
                    true,
                );

                let btn_label = "OK";
                let btn_w = button_row_width(&[btn_label]);
                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                let body_area = Rect {
                    x: area.x + 2,
                    y: title_y + 2,
                    width: area.width.saturating_sub(4),
                    height: buttons_y.saturating_sub(title_y + 2).saturating_sub(1),
                };
                frame.render_widget(
                    Paragraph::new(body.as_str())
                        .style(Style::default().fg(theme.text).bg(theme.panel))
                        .wrap(Wrap { trim: false }),
                    body_area,
                );

                let btn_area = Rect {
                    x: area.x + area.width.saturating_sub(2 + btn_w),
                    y: buttons_y,
                    width: btn_w,
                    height: BUTTON_HEIGHT,
                };
                let btn_state = if hovered == Some(&crate::hit::Hit::ModalConfirm) {
                    ControlState::Hover
                } else {
                    ControlState::Normal
                };
                Button {
                    label: btn_label,
                    kind: ButtonKind::Primary,
                    state: btn_state,
                }
                .paint(frame.buffer_mut(), btn_area, theme.panel, theme);
                hits.register(btn_area, crate::hit::Hit::ModalConfirm);
            }
            Modal::Confirm {
                title,
                body,
                choices,
            } => {
                let area = centered_rect(screen, 60.min(screen.width), 13.min(screen.height));
                hits.register(area, crate::hit::Hit::ModalBody);
                paint::floating_panel(frame.buffer_mut(), area, screen, theme);

                let title_y = area.y + 1;
                paint::text(
                    frame.buffer_mut(),
                    area.x + 2,
                    title_y,
                    title,
                    theme.text,
                    theme.panel,
                    true,
                );

                let labels: Vec<&str> = choices.iter().map(|(_, l, _)| l.as_str()).collect();
                let btn_row_w = button_row_width(&labels);
                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                let body_area = Rect {
                    x: area.x + 2,
                    y: title_y + 2,
                    width: area.width.saturating_sub(4),
                    height: buttons_y.saturating_sub(title_y + 2).saturating_sub(1),
                };
                frame.render_widget(
                    Paragraph::new(body.as_str())
                        .style(Style::default().fg(theme.text).bg(theme.panel))
                        .wrap(Wrap { trim: false }),
                    body_area,
                );

                // Each choice is its own clickable painted button
                // (`Hit::ConfirmChoice(c)`); `Esc` still closes the modal
                // (matching whichever choice text says "Cancel", if any) and
                // is documented in the muted helper line below the buttons.
                let mut x = area.x + area.width.saturating_sub(2 + btn_row_w);
                for (c, label, _) in choices.iter() {
                    let w = paint::button_min_width(label);
                    let btn_area = Rect {
                        x,
                        y: buttons_y,
                        width: w,
                        height: BUTTON_HEIGHT,
                    };
                    let choice_state = if hovered == Some(&crate::hit::Hit::ConfirmChoice(*c)) {
                        ControlState::Hover
                    } else {
                        ControlState::Normal
                    };
                    Button {
                        label,
                        kind: ButtonKind::Secondary,
                        state: choice_state,
                    }
                    .paint(frame.buffer_mut(), btn_area, theme.panel, theme);
                    hits.register(btn_area, crate::hit::Hit::ConfirmChoice(*c));
                    x += w + 2;
                }

                let hint_y = buttons_y.saturating_sub(1);
                paint::text(
                    frame.buffer_mut(),
                    area.x + 2,
                    hint_y,
                    "esc cancel",
                    theme.text_muted,
                    theme.panel,
                    false,
                );
            }
            Modal::Prompt { title, input, .. } => {
                let area = centered_rect(screen, 60.min(screen.width), 14.min(screen.height));
                hits.register(area, crate::hit::Hit::ModalBody);
                paint::floating_panel(frame.buffer_mut(), area, screen, theme);

                let title_y = area.y + 1;
                paint::text(
                    frame.buffer_mut(),
                    area.x + 2,
                    title_y,
                    title,
                    theme.text,
                    theme.panel,
                    true,
                );

                let field_area = Rect {
                    x: area.x + 2,
                    y: title_y + 2,
                    width: area.width.saturating_sub(4),
                    height: FIELD_HEIGHT,
                };
                TextField {
                    content: input.draw_line_windowed(
                        true,
                        theme,
                        field_area.width.saturating_sub(2),
                    ),
                    state: ControlState::Focused,
                }
                .paint(frame.buffer_mut(), field_area, theme);

                let hint_y = field_area.y + FIELD_HEIGHT + 1;
                paint::text(
                    frame.buffer_mut(),
                    area.x + 2,
                    hint_y,
                    "enter confirm  esc cancel",
                    theme.text_muted,
                    theme.panel,
                    false,
                );

                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                draw_cancel_confirm_row(frame, hits, theme, area, buttons_y, hovered);
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
                let area = centered_rect(screen, 60.min(screen.width), 19.min(screen.height));
                hits.register(area, crate::hit::Hit::ModalBody);
                paint::floating_panel(frame.buffer_mut(), area, screen, theme);

                let title_y = area.y + 1;
                paint::text(
                    frame.buffer_mut(),
                    area.x + 2,
                    title_y,
                    "New project",
                    theme.text,
                    theme.panel,
                    true,
                );

                let field_x = area.x + 2;
                let field_w = area.width.saturating_sub(4);

                let name_label_y = title_y + 2;
                paint::text(
                    frame.buffer_mut(),
                    field_x,
                    name_label_y,
                    "Name:",
                    theme.text_muted,
                    theme.panel,
                    false,
                );
                let name_area = Rect {
                    x: field_x,
                    y: name_label_y + 1,
                    width: field_w,
                    height: FIELD_HEIGHT,
                };
                TextField {
                    content: name.draw_line_windowed(!*on_path, theme, field_w.saturating_sub(2)),
                    state: if *on_path {
                        ControlState::Normal
                    } else {
                        ControlState::Focused
                    },
                }
                .paint(frame.buffer_mut(), name_area, theme);

                let path_label_y = name_area.y + FIELD_HEIGHT + 1;
                paint::text(
                    frame.buffer_mut(),
                    field_x,
                    path_label_y,
                    "Path:",
                    theme.text_muted,
                    theme.panel,
                    false,
                );
                let path_area = Rect {
                    x: field_x,
                    y: path_label_y + 1,
                    width: field_w,
                    height: FIELD_HEIGHT,
                };
                TextField {
                    content: path.draw_line_windowed(*on_path, theme, field_w.saturating_sub(2)),
                    state: if *on_path {
                        ControlState::Focused
                    } else {
                        ControlState::Normal
                    },
                }
                .paint(frame.buffer_mut(), path_area, theme);

                let hint_y = path_area.y + FIELD_HEIGHT + 1;
                paint::text(
                    frame.buffer_mut(),
                    field_x,
                    hint_y,
                    "tab switch  enter create  esc cancel",
                    theme.text_muted,
                    theme.panel,
                    false,
                );

                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                draw_cancel_confirm_row(frame, hits, theme, area, buttons_y, hovered);
            }
            Modal::Dropdown(state) => draw_dropdown(frame, screen, theme, hits, hovered, state),
        }
    }
}

/// Draws `state`'s popup at `anchor.x, anchor.y + 1`, flipping to
/// `anchor.y - height` when it would cross the screen bottom, clamped
/// horizontally (and vertically) to stay on screen. Registers
/// `Hit::ModalOutside` over the whole screen first (so any other click
/// closes the popup), then `Hit::DropdownRow(i)` per row. Rows sit on a
/// 1-line pitch (not the 2-line pill pitch the centered overlays use) —
/// anchored dropdowns are compact menus, and long lists (many methods,
/// many projects) need every row they can get.
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
    // The popup floats over undimmed live content, so it needs real
    // breathing room: 2 columns of panel each side of the rows and a blank
    // margin row above and below, on top of the 1-cell panel edge —
    // without it, glyphs on the page underneath sit flush against the
    // rows and read as part of the menu.
    let width = (max_label + 10).min(screen.width);
    let height = (state.items.len() as u16 + 4).min(screen.height);

    // Open 2 columns left of the anchor: the horizontal padding then hangs
    // over the anchor's own column, so content hugging it on the page (the
    // params table's checkmark column, under the method badge) ends up
    // covered by the panel instead of flush against the menu rows.
    let mut x = state.anchor.x.saturating_sub(2);
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
    hits.register(area, crate::hit::Hit::ModalBody);
    paint::floating_panel(frame.buffer_mut(), area, screen, theme);

    let inner = Rect {
        x: area.x + 2,
        y: area.y + 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(4),
    };

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
        let selected = i == state.selected;
        let row_hovered = hovered == Some(&crate::hit::Hit::DropdownRow(i));
        let row_fill = if selected {
            theme.control_hover
        } else if row_hovered {
            theme.control
        } else {
            theme.panel
        };
        paint::fill(frame.buffer_mut(), row_area, row_fill);

        // `current` (the value already in effect) gets the checkmark;
        // `selected` (the keyboard cursor) gets its own bold/accent
        // highlight — the two can differ once arrow keys move the cursor
        // away from the current value.
        let marker = if state.current == Some(i) {
            "\u{2713} "
        } else {
            "  "
        };
        let fg = if selected { theme.accent } else { theme.text };
        paint::text(
            frame.buffer_mut(),
            row_area.x,
            row_area.y,
            &format!("{marker}{label}"),
            fg,
            row_fill,
            selected,
        );
        hits.register(row_area, crate::hit::Hit::DropdownRow(i));
    }
}

/// The total width a right-aligned row of buttons needs: each button's
/// [`paint::button_min_width`], plus a 2-column gap between adjacent
/// buttons.
fn button_row_width(labels: &[impl AsRef<str>]) -> u16 {
    let widths: u16 = labels
        .iter()
        .map(|l| paint::button_min_width(l.as_ref()))
        .sum();
    let gaps = labels.len().saturating_sub(1) as u16 * 2;
    widths + gaps
}

/// Paints the right-aligned Secondary "Cancel" + Primary "Confirm" button
/// row shared by `Prompt` and `NewProject`, at `buttons_y` inside `area`.
/// Registers `Hit::ModalCancel`/`Hit::ModalConfirm` on the two button rects
/// — the app-side click handler dispatches both by synthesizing the same
/// `Esc`/`Enter` key event `ModalStack::handle_key` already handles for
/// whichever modal is on top, so this is pure painting: no new behavior.
fn draw_cancel_confirm_row(
    frame: &mut Frame,
    hits: &mut crate::hit::HitMap,
    theme: &Theme,
    area: Rect,
    buttons_y: u16,
    hovered: Option<&crate::hit::Hit>,
) {
    let buttons = [
        (
            "Cancel",
            ButtonKind::Secondary,
            crate::hit::Hit::ModalCancel,
        ),
        (
            "Confirm",
            ButtonKind::Primary,
            crate::hit::Hit::ModalConfirm,
        ),
    ];
    let labels: Vec<&str> = buttons.iter().map(|(l, ..)| *l).collect();
    let row_w = button_row_width(&labels);
    let mut x = area.x + area.width.saturating_sub(2 + row_w);
    for (label, kind, hit) in buttons {
        let w = paint::button_min_width(label);
        let btn_area = Rect {
            x,
            y: buttons_y,
            width: w,
            height: BUTTON_HEIGHT,
        };
        let state = if hovered == Some(&hit) {
            ControlState::Hover
        } else {
            ControlState::Normal
        };
        Button { label, kind, state }.paint(frame.buffer_mut(), btn_area, theme.panel, theme);
        hits.register(btn_area, hit);
        x += w + 2;
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
    fn draw_dims_the_backdrop_and_paints_the_panel_surface() {
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
            .draw(|f| {
                // Paint the whole screen `theme.page` first, as `ui::draw`
                // does before overlays run, so the backdrop dim has
                // something non-default to blend toward black.
                let area = f.area();
                crate::paint::fill(f.buffer_mut(), area, theme.page);
                m.draw(f, area, &theme, &mut hits, None)
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        // A corner cell is guaranteed to sit outside the centered panel.
        assert_ne!(
            buffer[(0, 0)].bg,
            theme.page,
            "a backdrop cell outside the panel must be dimmed"
        );
        let panel = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let center = (panel.x + panel.width / 2, panel.y + panel.height / 2);
        assert_eq!(
            buffer[center].bg, theme.panel,
            "the panel's own fill must be theme.panel"
        );
    }

    #[test]
    fn confirm_modal_paints_bevel_buttons_with_secondary_and_primary_faces() {
        let mut m = ModalStack::default();
        m.push(Modal::Confirm {
            title: "Delete request?".into(),
            body: "This cannot be undone.".into(),
            choices: vec![
                ('n', "Cancel".into(), vec![]),
                ('y', "Delete".into(), vec![Action::Quit]),
            ],
        });
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| m.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let buffer = terminal.backend().buffer();

        let cancel = hits.rect_of(&crate::hit::Hit::ConfirmChoice('n')).unwrap();
        assert_eq!(
            buffer[(cancel.x, cancel.y + 2)].symbol(),
            "\u{2580}",
            "the Cancel button's bottom row must be its half-block cap"
        );
        assert_eq!(
            buffer[(cancel.x + 1, cancel.y + 1)].bg,
            theme.control,
            "Cancel is painted with the Secondary (control) face"
        );

        let confirm = hits.rect_of(&crate::hit::Hit::ConfirmChoice('y')).unwrap();
        assert_eq!(
            buffer[(confirm.x, confirm.y + 2)].symbol(),
            "\u{2580}",
            "the confirm button's bottom row must be its half-block cap"
        );
    }

    #[test]
    fn palette_row_matches_the_selected_pill_and_accent_bar() {
        let mut m = ModalStack::default();
        m.push(Modal::Palette(
            crate::components::palette::PaletteState::new(&crate::usage::UsageStore::default(), 0),
        ));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| m.draw(f, f.area(), &theme, &mut hits, None))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row0 = hits.rect_of(&crate::hit::Hit::PaletteRow(0)).unwrap();
        assert_eq!(
            buffer[(row0.x, row0.y)].bg,
            theme.control_hover,
            "the selected row's pill fill must be theme.control_hover"
        );
        assert_eq!(
            buffer[(row0.x, row0.y)].symbol(),
            "\u{2588}",
            "the selected row must carry the full-block accent bar in its first column"
        );
        assert_eq!(buffer[(row0.x, row0.y)].fg, theme.accent);
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
    fn dropdown_pads_its_rows_away_from_the_panel_edge() {
        // The popup floats over live content with no backdrop dim; without
        // real padding the page's own glyphs (e.g. the params table's
        // checkmarks) sit flush against the rows and read as part of the
        // menu.
        let screen = Rect::new(0, 0, 80, 24);
        let state = DropdownState {
            anchor: Rect::new(10, 5, 8, 1),
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
        let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::DropdownRow(0)).unwrap();
        let last = hits
            .rect_of(&crate::hit::Hit::DropdownRow(dropdown_items().len() - 1))
            .unwrap();
        assert!(
            row0.y >= body.y + 2,
            "a blank margin row separates the first row from the top edge"
        );
        assert!(
            last.y + last.height + 2 <= body.y + body.height,
            "and from the bottom edge"
        );
        assert!(
            row0.x >= body.x + 2 && row0.x + row0.width + 2 <= body.x + body.width,
            "rows keep two columns of panel on both sides"
        );
        assert_eq!(
            body.x,
            state.anchor.x - 2,
            "the panel reaches left of its anchor so page content hugging \
             the anchor's column (the params table's checkmarks) is covered \
             rather than left flush against the menu"
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
            theme.control,
            "hovered row gets the control hover fill"
        );
        assert_eq!(
            buffer[(row0.x, row0.y)].bg,
            theme.control_hover,
            "the selected row (index 0) keeps its own selected fill"
        );
        assert_eq!(
            buffer[(row2.x, row2.y)].bg,
            theme.panel,
            "non-hovered, non-selected rows keep the popup's plain panel background"
        );
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
