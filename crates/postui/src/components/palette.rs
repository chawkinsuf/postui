use super::chooser::clip;
use crate::action::{Action, CopyTarget};
use crate::layout::PaneId;
use crate::paint::{self, ControlState, FIELD_HEIGHT, PillRow, RowHighlight, TextField};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

#[derive(Clone)]
pub struct Command {
    /// A stable kebab-case identifier, independent of the display `name`,
    /// used as the key for palette frecency stats (`UsageStore`).
    pub id: &'static str,
    pub name: &'static str,
    pub action: Action,
}

pub fn all_commands() -> Vec<Command> {
    vec![
        Command {
            id: "focus-sidebar",
            name: "Focus: request tree",
            action: Action::FocusPane(PaneId::Sidebar),
        },
        Command {
            id: "focus-editor",
            name: "Focus: editor",
            action: Action::FocusPane(PaneId::Editor),
        },
        Command {
            id: "focus-response",
            name: "Focus: response",
            action: Action::FocusPane(PaneId::Response),
        },
        Command {
            id: "about",
            name: "Help: about postui",
            action: Action::ShowAbout,
        },
        Command {
            id: "send",
            name: "Send request",
            action: Action::Send,
        },
        Command {
            id: "request-new",
            name: "Request: new",
            action: Action::PromptNewRequest,
        },
        Command {
            id: "request-save",
            name: "Request: save",
            action: Action::SaveRequest,
        },
        Command {
            id: "request-rename",
            name: "Request: rename",
            action: Action::PromptRenameRequest,
        },
        Command {
            id: "request-delete",
            name: "Request: delete",
            action: Action::ConfirmDeleteRequest,
        },
        Command {
            id: "method-cycle",
            name: "Method: cycle",
            action: Action::CycleMethod,
        },
        Command {
            id: "method-choose",
            name: "Method: choose…",
            action: Action::OpenMethodDropdown,
        },
        Command {
            id: "body-format",
            name: "Body: format JSON",
            action: Action::FormatBody,
        },
        Command {
            id: "body-minify",
            name: "Body: minify JSON",
            action: Action::MinifyBody,
        },
        Command {
            id: "body-external-editor",
            name: "Body: open in $EDITOR",
            action: Action::OpenBodyInEditor,
        },
        Command {
            id: "body-toggle-vars",
            name: "Body: toggle {{var}} substitution",
            action: Action::ToggleBodyVars,
        },
        Command {
            id: "project-choose",
            name: "Project: choose…",
            action: Action::OpenProjectChooser,
        },
        Command {
            id: "project-next",
            name: "Project: next",
            action: Action::CycleProject,
        },
        Command {
            id: "project-open-path",
            name: "Project: open by path…",
            action: Action::PromptOpenProjectPath,
        },
        Command {
            id: "project-new",
            name: "Project: new…",
            action: Action::PromptNewProject,
        },
        Command {
            id: "env-choose",
            name: "Environment: choose…",
            action: Action::OpenEnvChooser,
        },
        Command {
            id: "env-next",
            name: "Environment: next",
            action: Action::CycleEnv,
        },
        Command {
            id: "vars-insert",
            name: "Variables: insert…",
            action: Action::OpenVarPicker { completing: false },
        },
        Command {
            id: "response-copy-body",
            name: "Response: copy body",
            action: Action::CopyToClipboard(CopyTarget::ResponseBody),
        },
        Command {
            id: "request-copy-url",
            name: "Request: copy URL",
            action: Action::CopyToClipboard(CopyTarget::Url),
        },
        Command {
            id: "response-save-body",
            name: "Response: save body to file…",
            action: Action::PromptSaveBody,
        },
        Command {
            id: "var-manager",
            name: "Variable Manager",
            action: Action::OpenVarManager,
        },
        Command {
            id: "quit",
            name: "Quit",
            action: Action::Quit,
        },
    ]
}

pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let needle = needle.to_lowercase();
    let haystack = haystack.to_lowercase();
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

pub struct PaletteState {
    input: String,
    selected: usize,
    /// `all_commands()` sorted by frecency score descending (stable, so
    /// zero-score commands keep declaration order) as of the moment the
    /// palette opened. `refilter` filters *this* order rather than
    /// re-deriving it, so an empty query shows frecency order and a typed
    /// query fuzzy-filters within it (frecency only breaks ties — spec §6).
    base: Vec<Command>,
    filtered: Vec<Command>,
    /// First visible row's index into `filtered`. See `ChooserState` for the
    /// `ensure_visible` contract this mirrors.
    scroll: usize,
    ensure_visible: bool,
}

impl PaletteState {
    /// Builds the palette's base (frecency-sorted) order from `usage`/
    /// `now_secs` and opens with an empty query, so `filtered()` starts out
    /// equal to `base`.
    pub fn new(usage: &crate::usage::UsageStore, now_secs: i64) -> Self {
        let mut base = all_commands();
        base.sort_by(|a, b| {
            usage
                .score(b.id, now_secs)
                .partial_cmp(&usage.score(a.id, now_secs))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Self {
            input: String::new(),
            selected: 0,
            filtered: base.clone(),
            base,
            scroll: 0,
            ensure_visible: true,
        }
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn filtered(&self) -> &[Command] {
        &self.filtered
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
        let chosen = self.filtered.get(self.selected)?;
        Some(super::modal::ModalResult {
            actions: vec![chosen.action.clone()],
            close: true,
            usage: Some(chosen.id.to_string()),
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
            .base
            .iter()
            .filter(|c| fuzzy_match(&self.input, c.name))
            .cloned()
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
        let width = 50.min(screen.width);
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
            "Commands",
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

        // NOTE: the spec calls for a right-aligned muted keybinding column
        // on each row, matching a bound key to the command. `Command` (and
        // `Keymap`) currently expose no reverse action->combo lookup, so
        // there is no data to paint there yet; see the task report.
        for (i, c) in self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(list_h.max(1))
        {
            let text_row = list_area.y + ((i - self.scroll) as u16) * 2;
            let selected = i == self.selected;
            let row_hovered = hovered == Some(&crate::hit::Hit::PaletteRow(i));
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
            let x = list_area.x + 1;
            paint::text(
                frame.buffer_mut(),
                x,
                text_row,
                clip(c.name, right.saturating_sub(x)),
                theme.text,
                row_fill,
                selected,
            );

            let row_rect = Rect {
                x: list_area.x,
                y: text_row,
                width: list_area.width,
                height: 1,
            };
            hits.register(row_rect, crate::hit::Hit::PaletteRow(i));
        }

        let footer_y = area.y + area.height.saturating_sub(2);
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            footer_y,
            "enter run  esc cancel",
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
    fn fuzzy_match_is_case_insensitive_subsequence() {
        assert!(fuzzy_match("fre", "Focus: request editor"));
        assert!(fuzzy_match("QUIT", "Quit"));
        assert!(fuzzy_match("", "anything"));
        assert!(!fuzzy_match("xyz", "Quit"));
    }

    #[test]
    fn typing_filters_and_backspace_restores() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let total = p.filtered().len();
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.filtered().len(), 1);
        assert_eq!(p.filtered()[0].name, "Quit");
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.filtered().len(), total);
    }

    #[test]
    fn arrows_move_selection_within_bounds() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Up)); // clamped at top
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Down));
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn enter_returns_selected_action_and_closes() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::Quit]);
        assert_eq!(res.usage.as_deref(), Some("quit"));
    }

    #[test]
    fn heavily_used_command_sorts_to_top_with_empty_query() {
        let mut usage = crate::usage::UsageStore::default();
        // "quit" is declared last but heavy usage should put it first when
        // the query is empty.
        for _ in 0..50 {
            usage.record("quit", 1_000_000);
        }
        let p = PaletteState::new(&usage, 1_000_000);
        assert_eq!(p.filtered()[0].id, "quit");
    }

    #[test]
    fn typing_still_fuzzy_filters_within_frecency_order() {
        let mut usage = crate::usage::UsageStore::default();
        for _ in 0..50 {
            usage.record("quit", 1_000_000);
        }
        let mut p = PaletteState::new(&usage, 1_000_000);
        for c in "focus".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert!(
            p.filtered().iter().all(|c| c.id.starts_with("focus-")),
            "typed query must still fuzzy-filter, frecency only breaks ties"
        );
        assert_eq!(p.filtered().len(), 3);
    }

    #[test]
    fn zero_score_commands_keep_declaration_order() {
        let p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let ids: Vec<&str> = p.filtered().iter().map(|c| c.id).collect();
        let declared: Vec<&str> = all_commands().iter().map(|c| c.id).collect();
        assert_eq!(ids, declared, "stable sort must preserve order among ties");
    }

    #[test]
    fn enter_on_empty_results_does_nothing() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        for c in "zzzz".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert!(p.filtered().is_empty());
        assert!(p.handle_key(key(KeyCode::Enter)).is_none());
    }

    #[test]
    fn esc_closes_without_action() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close);
        assert!(res.actions.is_empty());
    }

    #[test]
    fn selection_resets_when_filter_changes() {
        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        p.handle_key(key(KeyCode::Down));
        p.handle_key(key(KeyCode::Char('q')));
        assert_eq!(p.selected(), 0);
    }

    #[test]
    fn field_fill_and_gap_row_survive_the_list_draw() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
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
    fn hovered_row_background_fills_the_full_row_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                // Row 1 (not the default-selected row 0) so this exercises
                // the plain Hover fill rather than Selected.
                p.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    Some(&crate::hit::Hit::PaletteRow(1)),
                )
            })
            .unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::PaletteRow(1)).unwrap();
        let buffer = terminal.backend().buffer();
        // The label ("Focus: editor") is well short of the row's right
        // edge, so a cell out there only picks up the pill's background if
        // the fill spans the whole row, not just the label glyphs.
        let right_edge = (row1.x + row1.width - 1, row1.y);
        assert_eq!(
            buffer[right_edge].bg, theme.control,
            "the pill fill must span the full row width, not just the label glyphs"
        );
    }
}
