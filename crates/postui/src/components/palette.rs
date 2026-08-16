use crate::action::{Action, CopyTarget};
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};

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
            .title(" Commands ")
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
            .map(|(i, c)| {
                let mut style = if i == self.selected {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text)
                };
                if hovered == Some(&crate::hit::Hit::PaletteRow(i)) {
                    style = style.bg(theme.surface_raised);
                }
                let marker = if i == self.selected { "› " } else { "  " };
                let row_area = Rect {
                    x: list_area.x,
                    y: list_area.y + (i - self.scroll) as u16,
                    width: list_area.width,
                    height: 1,
                };
                hits.register(row_area, crate::hit::Hit::PaletteRow(i));
                ListItem::new(
                    Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled(c.name, style),
                    ])
                    .style(style),
                )
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
                p.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    Some(&crate::hit::Hit::PaletteRow(0)),
                )
            })
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::PaletteRow(0)).unwrap();
        let buffer = terminal.backend().buffer();
        // The label ("Focus: request tree") is well short of the row's
        // right edge, so a cell out there only picks up the hover
        // background if it comes from the whole `Line`'s style rather than
        // just the marker/label spans.
        let right_edge = (row0.x + row0.width - 1, row0.y);
        assert_eq!(
            buffer[right_edge].bg, theme.surface_raised,
            "hover background must fill the full row width, not just the label glyphs"
        );
    }
}
