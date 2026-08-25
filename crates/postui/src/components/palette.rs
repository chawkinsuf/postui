use super::chooser::clip;
use crate::action::{Action, CopyTarget};
use crate::layout::PaneId;
use crate::paint::{self, ControlState, FIELD_HEIGHT, ListRow, RowHighlight, TextField};
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
    /// A one-line muted caption painted on the row directly under `name`
    /// (spec: "bold title + muted description", two-line dense entries).
    pub description: &'static str,
    pub action: Action,
}

pub fn all_commands() -> Vec<Command> {
    vec![
        Command {
            id: "focus-sidebar",
            name: "Focus: request tree",
            description: "Move keyboard focus to the sidebar",
            action: Action::FocusPane(PaneId::Sidebar),
        },
        Command {
            id: "focus-editor",
            name: "Focus: editor",
            description: "Move keyboard focus to the request editor",
            action: Action::FocusPane(PaneId::Editor),
        },
        Command {
            id: "focus-response",
            name: "Focus: response",
            description: "Move keyboard focus to the response pane",
            action: Action::FocusPane(PaneId::Response),
        },
        Command {
            id: "about",
            name: "Help: about postui",
            description: "Show version and app info",
            action: Action::ShowAbout,
        },
        Command {
            id: "send",
            name: "Send request",
            description: "Send the open request",
            action: Action::Send,
        },
        Command {
            id: "request-new",
            name: "Request: new",
            description: "Create a new request in the current folder",
            action: Action::PromptNewRequest,
        },
        Command {
            id: "request-save",
            name: "Request: save",
            description: "Save the open request to disk",
            action: Action::SaveRequest,
        },
        Command {
            id: "request-rename",
            name: "Request: rename",
            description: "Rename the open request",
            action: Action::PromptRenameRequest,
        },
        Command {
            id: "request-duplicate",
            name: "Request: duplicate",
            description: "Copy the open request under a new name",
            action: Action::DuplicateRequest,
        },
        Command {
            id: "request-delete",
            name: "Request: delete",
            description: "Delete the open request, with confirmation",
            action: Action::ConfirmDeleteRequest,
        },
        Command {
            id: "method-cycle",
            name: "Method: cycle",
            description: "Step to the next HTTP method",
            action: Action::CycleMethod,
        },
        Command {
            id: "method-choose",
            name: "Method: choose…",
            description: "Pick an HTTP method from a list",
            action: Action::OpenMethodDropdown,
        },
        Command {
            id: "body-format",
            name: "Body: format JSON",
            description: "Pretty-print the request body",
            action: Action::FormatBody,
        },
        Command {
            id: "body-minify",
            name: "Body: minify JSON",
            description: "Collapse the request body to one line",
            action: Action::MinifyBody,
        },
        Command {
            id: "body-external-editor",
            name: "Body: open in $EDITOR",
            description: "Edit the body in your external editor",
            action: Action::OpenBodyInEditor,
        },
        Command {
            id: "body-toggle-vars",
            name: "Body: toggle {{var}} substitution",
            description: "Enable or disable variable substitution in the body",
            action: Action::ToggleBodyVars,
        },
        Command {
            id: "project-choose",
            name: "Project: choose…",
            description: "Switch to another registered project",
            action: Action::OpenProjectChooser,
        },
        Command {
            id: "project-next",
            name: "Project: next",
            description: "Cycle to the next registered project",
            action: Action::CycleProject,
        },
        Command {
            id: "project-open-path",
            name: "Project: open by path…",
            description: "Open a project directory by typing its path",
            action: Action::PromptOpenProjectPath,
        },
        Command {
            id: "project-new",
            name: "Project: new…",
            description: "Create a new project",
            action: Action::PromptNewProject,
        },
        Command {
            id: "env-choose",
            name: "Environment: choose…",
            description: "Switch to another environment",
            action: Action::OpenEnvChooser,
        },
        Command {
            id: "env-next",
            name: "Environment: next",
            description: "Cycle to the next environment",
            action: Action::CycleEnv,
        },
        Command {
            id: "env-new",
            name: "Environment: new…",
            description: "Create a new environment",
            action: Action::OpenNewEnvPrompt,
        },
        Command {
            id: "vars-insert",
            name: "Variables: insert…",
            description: "Insert a {{variable}} token at the cursor",
            action: Action::OpenVarPicker { completing: false },
        },
        Command {
            id: "response-copy-body",
            name: "Response: copy body",
            description: "Copy the response body to the clipboard",
            action: Action::CopyToClipboard(CopyTarget::ResponseBody),
        },
        Command {
            id: "request-copy-url",
            name: "Request: copy URL",
            description: "Copy the resolved request URL to the clipboard",
            action: Action::CopyToClipboard(CopyTarget::Url),
        },
        Command {
            id: "response-save-body",
            name: "Response: save body to file…",
            description: "Save the response body to a file on disk",
            action: Action::PromptSaveBody,
        },
        Command {
            id: "response-search",
            name: "Response: search",
            description: "Search within the response body",
            action: Action::OpenResponseSearch,
        },
        Command {
            id: "var-manager",
            name: "Variable Manager",
            description: "Open the full variable/environment manager",
            action: Action::OpenVarManager,
        },
        Command {
            id: "vars-new-variable",
            name: "Variables: new variable…",
            description: "Declare a new project variable",
            action: Action::PromptNewVar,
        },
        Command {
            id: "vars-new-group",
            name: "Variables: new group…",
            description: "Declare a new variable group",
            action: Action::PromptNewGroup,
        },
        Command {
            id: "vars-extract",
            name: "Extract to variable",
            description: "Turn the selected text into a variable",
            action: Action::ExtractToVariable,
        },
        Command {
            id: "undo",
            name: "Undo",
            description: "Undo the last change",
            action: Action::Undo,
        },
        Command {
            id: "redo",
            name: "Redo",
            description: "Redo the last undone change",
            action: Action::Redo,
        },
        Command {
            id: "quit",
            name: "Quit",
            description: "Exit postui",
            action: Action::Quit,
        },
    ]
}

/// Maps a palette [`Command::id`] to the corresponding [`crate::keys::
/// named_actions`] name, where one exists, so `draw` can ask
/// [`crate::keys::Keymap::combo_for`] for its bound combo. The two
/// namespaces are independent by design (`Command::id` is the frecency
/// key, stable across a command's display `name`; the keymap's names are
/// the rebind-by-name TOML keys) and happen to describe several of the
/// same actions under different spellings — this table is the one place
/// that reconciles them. Commands with no entry here (most of them: focus
/// moves, prompts, the palette itself has no self-referential binding,
/// …) simply show no keybinding column, which is correct — they have none.
fn keymap_action_name(command_id: &str) -> Option<&'static str> {
    match command_id {
        "quit" => Some("quit"),
        "send" => Some("send"),
        "request-save" => Some("save"),
        "request-duplicate" => Some("request_duplicate"),
        "method-cycle" => Some("cycle_method"),
        "method-choose" => Some("method_choose"),
        "body-format" => Some("format_body"),
        "body-minify" => Some("minify_body"),
        "body-external-editor" => Some("open_body_editor"),
        "body-toggle-vars" => Some("toggle_body_vars"),
        "project-choose" => Some("project_choose"),
        "project-next" => Some("project_cycle"),
        "project-new" => Some("project_new"),
        "env-choose" => Some("env_choose"),
        "env-next" => Some("env_cycle"),
        "vars-insert" => Some("pick_variable"),
        "var-manager" => Some("var_manager_open"),
        "vars-extract" => Some("extract_to_variable"),
        "undo" => Some("undo"),
        "redo" => Some("redo"),
        _ => None,
    }
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

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        hits: &mut crate::hit::HitMap,
        hovered: Option<&crate::hit::Hit>,
        keymap: &crate::keys::Keymap,
        t: f32,
    ) {
        let width = 50.min(screen.width);
        const CHROME: u16 = 8;
        let content_rows = (self.filtered.len() as u16).clamp(1, 10) * 2;
        let height = (CHROME + content_rows).clamp(11, 24).min(screen.height);
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

        // No hover-fade animation is wired for popup lists (transient
        // surfaces — see the task report); a hovered row shows its full
        // hover fill immediately, same convention as `DrawCtx::hover_t`'s
        // own documented default when no fade is in flight.
        let hover_t = 1.0;
        for (i, c) in self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(list_h.max(1))
        {
            let title_row = list_area.y + ((i - self.scroll) as u16) * 2;
            let desc_row = title_row + 1;
            let selected = i == self.selected;
            let row_hovered = hovered == Some(&crate::hit::Hit::PaletteRow(i));
            let highlight = if selected {
                RowHighlight::Selected
            } else if row_hovered {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            for row_y in [title_row, desc_row] {
                ListRow {
                    highlight,
                    zebra: None,
                }
                .paint(
                    frame.buffer_mut(),
                    row_y,
                    list_area.x,
                    list_area.width,
                    theme.panel,
                    hover_t,
                    theme,
                );
            }
            let row_fill = ListRow::resolve_fill(theme, highlight, theme.panel, hover_t);

            let right = list_area.x + list_area.width;
            let x = list_area.x + 1;
            let combo = keymap_action_name(c.id).and_then(|name| keymap.combo_for(name));
            let combo_w = combo
                .as_ref()
                .map(|s| s.chars().count() as u16 + 1)
                .unwrap_or(0);
            let title_right = right.saturating_sub(combo_w);
            paint::text(
                frame.buffer_mut(),
                x,
                title_row,
                clip(c.name, title_right.saturating_sub(x)),
                theme.text,
                row_fill,
                true,
            );
            if let Some(combo) = &combo {
                let combo_x = right.saturating_sub(combo.chars().count() as u16);
                paint::text(
                    frame.buffer_mut(),
                    combo_x,
                    title_row,
                    combo,
                    theme.text_muted,
                    row_fill,
                    false,
                );
            }
            paint::text(
                frame.buffer_mut(),
                x,
                desc_row,
                clip(c.description, right.saturating_sub(x)),
                theme.text_muted,
                row_fill,
                false,
            );

            let row_rect = Rect {
                x: list_area.x,
                y: title_row,
                width: list_area.width,
                height: 2,
            };
            hits.register(row_rect, crate::hit::Hit::PaletteRow(i));
        }

    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Puts the cursor on command `id`. A query is a subsequence match, so
    /// more than one command can survive it (e.g. "quit" also matches
    /// "Request: duplicate") — a test that means one command names it.
    fn select_id(p: &mut PaletteState, id: &str) {
        let i = p
            .filtered()
            .iter()
            .position(|c| c.id == id)
            .unwrap_or_else(|| panic!("{id} was filtered out"));
        p.select(i);
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
        assert!(p.filtered().len() < total, "the query narrowed the list");
        assert!(p.filtered().iter().any(|c| c.name == "Quit"));
        assert!(
            p.filtered().iter().all(|c| fuzzy_match("quit", c.name)),
            "every surviving row matches the query"
        );
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
        select_id(&mut p, "quit");
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

    /// Task 17, spec §5: palette audit — every command the sweep added
    /// (Response: search, Variables: new variable/group) is present with a
    /// stable kebab-case id, alongside the ids that already covered Body:
    /// format/minify/toggle-vars/open-in-$EDITOR and Request: duplicate.
    #[test]
    fn palette_covers_the_mouse_parity_sweep_gaps() {
        let commands = all_commands();
        let ids: Vec<&str> = commands.iter().map(|c| c.id).collect();
        for expected in [
            "body-format",
            "body-minify",
            "body-toggle-vars",
            "body-external-editor",
            "request-duplicate",
            "response-search",
            "vars-new-variable",
            "vars-new-group",
        ] {
            assert!(
                ids.contains(&expected),
                "missing palette command {expected:?}"
            );
        }
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "every command id must be unique");

        assert_eq!(
            commands
                .iter()
                .find(|c| c.id == "response-search")
                .unwrap()
                .action,
            Action::OpenResponseSearch
        );
        assert_eq!(
            commands
                .iter()
                .find(|c| c.id == "vars-new-variable")
                .unwrap()
                .action,
            Action::PromptNewVar
        );
        assert_eq!(
            commands
                .iter()
                .find(|c| c.id == "vars-new-group")
                .unwrap()
                .action,
            Action::PromptNewGroup
        );
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
    fn no_key_hint_footer_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("enter run"), "{content}");
        assert!(!content.contains("esc cancel"), "{content}");
    }

    #[test]
    fn field_fill_and_gap_row_survive_the_list_draw() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0))
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
        let keymap = crate::keys::Keymap::default_bindings();
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
                    &keymap,
                    1.0,
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

    #[test]
    fn entries_sit_on_a_dense_two_line_pitch_with_zero_gap() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::PaletteRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::PaletteRow(1)).unwrap();
        assert_eq!(row0.height, 2, "each entry's hit box covers both its lines");
        assert_eq!(
            row1.y - row0.y,
            2,
            "consecutive entries sit back-to-back with no gap row between them"
        );
    }

    #[test]
    fn selected_entry_paints_the_accent_bar_and_selection_fill_on_both_lines() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::PaletteRow(0)).unwrap();
        let buffer = terminal.backend().buffer();
        for dy in [0u16, 1] {
            let cell = &buffer[(row0.x, row0.y + dy)];
            assert_eq!(cell.symbol(), "\u{258c}", "accent bar on line {dy}");
            assert_eq!(cell.fg, theme.accent);
            assert_eq!(
                buffer[(row0.x + row0.width - 1, row0.y + dy)].bg,
                theme.selection,
                "selection fill spans the full row width on line {dy}"
            );
        }
    }

    #[test]
    fn title_line_shows_bold_name_and_description_line_shows_muted_text() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        let first = p.filtered()[0].clone();
        assert!(content.contains(first.name), "{content}");
        assert!(content.contains(first.description), "{content}");
    }

    #[test]
    fn keybinding_column_shows_the_bound_combo_right_aligned() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = PaletteState::new(&crate::usage::UsageStore::default(), 0);
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        select_id(&mut p, "quit");
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, &keymap, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("^C"),
            "the quit row's bound combo must render in caret notation: {content}"
        );
    }
}
