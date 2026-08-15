use super::line_input::LineInput;
use super::table_editor::TableEditorState;
use super::toast::ToastKind;
use super::{pane_block, Component, DrawCtx};
use crate::action::Action;
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::{Body, Entry, HttpRequest, Method};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Which editor tab is active. Params/Headers content is a placeholder
/// paragraph until Task 11 adds the shared table editor; Body is a
/// placeholder until Task 12 swaps in edtui.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTab {
    Params,
    Headers,
    Body,
}

impl EditorTab {
    pub fn index(self) -> usize {
        match self {
            EditorTab::Params => 0,
            EditorTab::Headers => 1,
            EditorTab::Body => 2,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i % 3 {
            0 => EditorTab::Params,
            1 => EditorTab::Headers,
            _ => EditorTab::Body,
        }
    }

    fn label(self) -> &'static str {
        match self {
            EditorTab::Params => "Params",
            EditorTab::Headers => "Headers",
            EditorTab::Body => "Body",
        }
    }
}

/// Which sub-region of the editor pane has keyboard focus: the URL line, or
/// the active tab's content (params table / headers table / body editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubFocus {
    Url,
    Content,
}

pub struct Editor {
    pub slug: Option<String>,
    pub saved: Option<HttpRequest>,
    pub method: Method,
    pub url: LineInput,
    pub params: IndexMap<String, Entry>,
    pub headers: IndexMap<String, Entry>,
    /// Placeholder for the request body text until Task 12 swaps in edtui
    /// state.
    pub body_text: String,
    pub active_tab: EditorTab,
    pub sub_focus: SubFocus,
    /// Shared cursor/edit state for the key/value table, reused by both the
    /// Params and Headers tabs (never holds the entry data itself).
    pub table: TableEditorState,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            slug: None,
            saved: None,
            method: Method::Get,
            url: LineInput::new(""),
            params: IndexMap::new(),
            headers: IndexMap::new(),
            body_text: String::new(),
            active_tab: EditorTab::Params,
            sub_focus: SubFocus::Url,
            table: TableEditorState::default(),
        }
    }
}

impl Editor {
    /// Loads `req` into the editor for editing, and records it as the
    /// last-saved state so `is_dirty` starts out `false`.
    pub fn load(&mut self, slug: Option<String>, req: HttpRequest) {
        self.slug = slug;
        self.method = req.method;
        self.url = LineInput::new(&req.url);
        self.params = req.params.clone();
        self.headers = req.headers.clone();
        self.body_text = match &req.body {
            Some(Body::Json { text }) => text.clone(),
            None => String::new(),
        };
        self.saved = Some(req);
    }

    /// Builds an `HttpRequest` from the editor's current field values.
    pub fn current_request(&self) -> HttpRequest {
        HttpRequest {
            method: self.method,
            url: self.url.text().to_string(),
            params: self.params.clone(),
            headers: self.headers.clone(),
            body: if self.body_text.is_empty() {
                None
            } else {
                Some(Body::Json { text: self.body_text.clone() })
            },
        }
    }

    /// A never-loaded (fresh scratch) editor is not dirty even though it has
    /// no saved snapshot to compare against; only a request that has been
    /// loaded and then changed counts as dirty.
    pub fn is_dirty(&self) -> bool {
        match &self.saved {
            Some(s) => *s != self.current_request(),
            None => false,
        }
    }

    pub fn mark_saved(&mut self) {
        self.saved = Some(self.current_request());
    }
}

impl Component for Editor {
    fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match self.sub_focus {
            SubFocus::Url => {
                if self.url.handle_key(ev) {
                    return Some(Action::Render);
                }
                if ev.code == KeyCode::Down {
                    self.sub_focus = SubFocus::Content;
                    return Some(Action::Render);
                }
                None
            }
            // Line-aware Up navigation within Body content arrives in Task 12;
            // for now Up always returns focus to the URL line, except on the
            // Params/Headers tabs where the table editor gets first crack at
            // every key (including Up/Down navigation within the table).
            SubFocus::Content => {
                if matches!(self.active_tab, EditorTab::Params | EditorTab::Headers) {
                    let map = match self.active_tab {
                        EditorTab::Params => &mut self.params,
                        EditorTab::Headers => &mut self.headers,
                        EditorTab::Body => unreachable!(),
                    };
                    let outcome = self.table.handle_key(ev, map);
                    if outcome.consumed {
                        return Some(match outcome.warning {
                            Some(w) => Action::ShowToast(w, ToastKind::Warning),
                            None => Action::Render,
                        });
                    }
                    // An unconsumed Up (empty table, or already at row 0)
                    // falls back to the Task-10 behavior instead of being a
                    // dead end with no keyboard path back to the URL line.
                    if ev.code == KeyCode::Up {
                        self.sub_focus = SubFocus::Url;
                        return Some(Action::Render);
                    }
                    return None;
                }
                if ev.code == KeyCode::Up {
                    self.sub_focus = SubFocus::Url;
                    return Some(Action::Render);
                }
                None
            }
        }
    }

    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Request", ctx);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // method badge + URL
                Constraint::Length(1), // tab bar
                Constraint::Min(0),    // active tab content
            ])
            .split(inner);

        self.draw_method_and_url(frame, rows[0], ctx.theme);
        self.draw_tab_bar(frame, rows[1], ctx.theme);
        self.draw_tab_content(frame, rows[2], ctx.theme);
    }
}

impl Editor {
    fn draw_method_and_url(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(8), Constraint::Min(0)])
            .split(area);
        let badge = Paragraph::new(Line::styled(
            format!("{:^7}", self.method.as_str()),
            Style::default().fg(theme.method_color(self.method)).bold(),
        ));
        frame.render_widget(badge, cols[0]);

        let url_focused = self.sub_focus == SubFocus::Url;
        frame.render_widget(Paragraph::new(self.url.draw_line(url_focused, theme)), cols[1]);
    }

    fn draw_tab_bar(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let spans: Vec<Span> = [EditorTab::Params, EditorTab::Headers, EditorTab::Body]
            .into_iter()
            .map(|tab| {
                let style = if tab == self.active_tab {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text_muted)
                };
                Span::styled(format!(" {} ", tab.label()), style)
            })
            .collect();
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_tab_content(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match self.active_tab {
            EditorTab::Params => {
                let ctx = DrawCtx { theme, focused: self.sub_focus == SubFocus::Content };
                self.table.draw(frame, area, &self.params, &ctx, "No params yet — press a to add");
            }
            EditorTab::Headers => {
                let ctx = DrawCtx { theme, focused: self.sub_focus == SubFocus::Content };
                self.table.draw(frame, area, &self.headers, &ctx, "No headers yet — press a to add");
            }
            // Placeholder content until Task 12 swaps in the edtui body
            // editor.
            EditorTab::Body => {
                frame.render_widget(
                    Paragraph::new("Body editing coming soon.").style(Style::default().fg(theme.text_muted)),
                    area,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use postui_core::model::{HttpRequest, Method};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn typing_into_url_marks_dirty_and_updates_request() {
        let mut e = Editor::default();
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
        assert!(!e.is_dirty());
        assert_eq!(e.sub_focus, SubFocus::Url, "load must not change sub_focus");
        e.handle_key(key(KeyCode::Char('/')));
        assert_eq!(e.current_request().url, "https://x/");
        assert!(e.is_dirty());
    }

    #[test]
    fn fresh_scratch_editor_is_not_dirty() {
        let e = Editor::default();
        assert!(!e.is_dirty(), "never-loaded editor must not be dirty");
    }

    #[test]
    fn mark_saved_clears_dirty_flag() {
        let mut e = Editor::default();
        e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
        e.handle_key(key(KeyCode::Char('/')));
        assert!(e.is_dirty());
        e.mark_saved();
        assert!(!e.is_dirty());
    }

    #[test]
    fn method_cycles_via_action_and_tabs_select() {
        let mut app = App::new_for_test();
        app.update(Action::CycleMethod);
        assert_eq!(app.editor.method, Method::Post);
        app.update(Action::EditorTabSelect(2));
        assert_eq!(app.editor.active_tab, EditorTab::Body);
        app.update(Action::EditorTabCycle(1));
        assert_eq!(app.editor.active_tab, EditorTab::Params, "cycle wraps");
    }

    #[test]
    fn tab_cycle_backward_wraps() {
        let mut app = App::new_for_test();
        assert_eq!(app.editor.active_tab, EditorTab::Params);
        app.update(Action::EditorTabCycle(-1));
        assert_eq!(app.editor.active_tab, EditorTab::Body, "backward wraps to last tab");
    }

    #[test]
    fn focus_url_action_focuses_editor_and_url() {
        use crate::layout::PaneId;
        let mut app = App::new_for_test();
        app.editor.sub_focus = SubFocus::Content;
        app.update(Action::FocusUrl);
        assert_eq!(app.focus, PaneId::Editor);
        assert_eq!(app.editor.sub_focus, SubFocus::Url);
    }

    #[test]
    fn up_down_moves_between_url_and_content() {
        // Default tab is Params with an empty table; Up on an empty table
        // is unconsumed by the table editor, so Editor falls back to the
        // Task-10 behavior instead of leaving the user stuck with no way
        // back to the URL line.
        let mut e = Editor::default();
        assert_eq!(e.sub_focus, SubFocus::Url, "default sub_focus starts on the URL line");
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn body_tab_up_returns_to_url() {
        // Body tab has no table editor to intercept Up at all; it keeps the
        // original Up-returns-to-url-line fallback unconditionally.
        let mut e = Editor { active_tab: EditorTab::Body, ..Editor::default() };
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn params_tab_up_at_row_zero_returns_to_url() {
        // A non-empty table still doesn't consume Up at row 0 (the top),
        // so Editor's fallback kicks in even though the table has rows.
        let mut e = Editor::default();
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.params.insert("b".into(), Entry { value: "2".into(), enabled: true });
        e.sub_focus = SubFocus::Content;
        assert_eq!(e.table.selected, 0);
        let action = e.handle_key(key(KeyCode::Up));
        assert_eq!(action, Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn params_tab_up_below_row_zero_navigates_within_table() {
        // Once selected has moved off row 0, Up navigates the table instead
        // of returning focus to the URL line.
        let mut e = Editor::default();
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.params.insert("b".into(), Entry { value: "2".into(), enabled: true });
        e.sub_focus = SubFocus::Content;
        e.table.selected = 1;
        let action = e.handle_key(key(KeyCode::Up));
        assert_eq!(action, Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::Content, "table navigation must not move focus");
        assert_eq!(e.table.selected, 0);
    }

    #[test]
    fn duplicate_key_commit_in_params_tab_shows_warning_toast() {
        let mut e = Editor::default();
        e.params.insert("a".into(), Entry { value: "1".into(), enabled: true });
        e.sub_focus = SubFocus::Content;
        // Append a new row keyed "a", which duplicates the existing entry.
        e.handle_key(key(KeyCode::Char('a')));
        e.handle_key(key(KeyCode::Char('a')));
        e.handle_key(key(KeyCode::Tab));
        e.handle_key(key(KeyCode::Char('9')));
        let action = e.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, Some(Action::ShowToast(_, ToastKind::Warning))),
            "expected a warning toast, got {action:?}"
        );
        assert_eq!(e.params.len(), 1);
        assert_eq!(e.params["a"].value, "9");
    }

    #[test]
    fn draw_shows_method_badge_url_and_tab_bar() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(
                r#"method = "POST"
url = "https://api.example.com/users""#,
            )
            .unwrap(),
        );
        let theme = Theme::dark();
        let ctx = DrawCtx { theme: &theme, focused: true };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| e.draw(f, f.area(), &ctx)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("POST"), "method badge: {content}");
        assert!(content.contains("https://api.example.com/users"), "url text: {content}");
        assert!(content.contains("Params"), "params tab label: {content}");
        assert!(content.contains("Headers"), "headers tab label: {content}");
        assert!(content.contains("Body"), "body tab label: {content}");
    }
}
