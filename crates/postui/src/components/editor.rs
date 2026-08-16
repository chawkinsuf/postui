use super::line_input::LineInput;
use super::table_editor::TableEditorState;
use super::toast::ToastKind;
use super::{Component, DrawCtx, pane_block};
use crate::action::Action;
use crate::theme::Theme;
use edtui::{
    EditorEventHandler, EditorMode, EditorState, EditorTheme, EditorView, LineNumbers, Lines,
    SyntaxHighlighter,
};
use indexmap::IndexMap;
use postui_core::model::{Body, Entry, HttpRequest, Method};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

/// Which editor tab is active.
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
    pub substitute_body: bool,
    pub params: IndexMap<String, Entry>,
    pub headers: IndexMap<String, Entry>,
    /// Enabled default headers inherited from the project, synced by `App`
    /// on every `update()` alongside `open_slug`. Draw-only: rendered above
    /// the request headers table but never edited directly here.
    pub inherited_headers: IndexMap<String, Entry>,
    /// The request body buffer. edtui owns the text, cursor and undo stack;
    /// it is only ever rewritten wholesale by an explicit user action
    /// (load / format / minify / external editor), so half-typed JSON
    /// survives a save verbatim.
    pub body: EditorState,
    /// Emacs-mode (modeless) key handling for `body`.
    body_handler: EditorEventHandler,
    pub active_tab: EditorTab,
    pub sub_focus: SubFocus,
    /// Shared cursor/edit state for the key/value table, reused by both the
    /// Params and Headers tabs (never holds the entry data itself).
    pub table: TableEditorState,
    /// The screen area the body editor was rendered into on the last frame,
    /// recorded by `draw_tab_content`'s Body arm; `None` on any other tab
    /// (including the very first frame before anything has drawn). Mouse
    /// events are hit-tested against this before being forwarded to edtui.
    pub last_body_area: Option<Rect>,
    /// Mirrors `App::in_flight.is_some()`, synced by `App::update` on every
    /// action alongside `open_slug`. Draw-only: swaps the URL row's button
    /// from `[ Send ]` to `[ Cancel ]` at the same width, no other effect.
    pub sending: bool,
    /// The method-badge cell's screen area, recorded on every draw; consumed
    /// by the click handling that opens the method dropdown (a later task).
    pub last_method_area: Option<Rect>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            slug: None,
            saved: None,
            method: Method::Get,
            url: LineInput::new(""),
            substitute_body: false,
            params: IndexMap::new(),
            headers: IndexMap::new(),
            inherited_headers: IndexMap::new(),
            body: new_body_state(""),
            body_handler: EditorEventHandler::emacs_mode(),
            active_tab: EditorTab::Params,
            sub_focus: SubFocus::Url,
            table: TableEditorState::default(),
            last_body_area: None,
            sending: false,
            last_method_area: None,
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
        self.substitute_body = req.substitute_body;
        self.params = req.params.clone();
        self.headers = req.headers.clone();
        self.set_body_text(match &req.body {
            Some(Body::Json { text }) => text,
            None => "",
        });
        self.saved = Some(req);
    }

    /// Builds an `HttpRequest` from the editor's current field values.
    pub fn current_request(&self) -> HttpRequest {
        HttpRequest {
            method: self.method,
            url: self.url.text().to_string(),
            substitute_body: self.substitute_body,
            params: self.params.clone(),
            headers: self.headers.clone(),
            body: {
                let text = self.body_text();
                if text.is_empty() {
                    None
                } else {
                    Some(Body::Json { text })
                }
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

    /// The body buffer's text, with lines joined by `\n`.
    pub fn body_text(&self) -> String {
        self.body.lines.to_string()
    }

    /// Feeds each char of `s` through the body's key handler as a
    /// synthesized plain `KeyCode::Char` event, as if the user had typed it.
    /// Used to splice a picked variable token into the body buffer, which
    /// (unlike `LineInput`) has no direct string-insertion API of its own.
    pub fn body_insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.body_handler.on_key_event(
                KeyEvent::new(
                    KeyCode::Char(c),
                    ratatui::crossterm::event::KeyModifiers::NONE,
                ),
                &mut self.body,
            );
        }
    }

    /// Replaces the whole body buffer. Only explicit user actions (loading a
    /// request, format, minify, the `$EDITOR` round-trip) may call this;
    /// typing goes through `body_handler`, so the text is never rewritten
    /// behind the user's back.
    pub fn set_body_text(&mut self, s: &str) {
        self.body = new_body_state(s);
    }

    /// Whether the body parses as JSON. An empty body is vacuously valid:
    /// there is nothing to be wrong with yet.
    fn body_is_valid(&self) -> bool {
        let text = self.body_text();
        text.is_empty() || postui_core::json::validate(&text).is_ok()
    }

    /// Forwards a raw mouse event to the body editor when the Body tab is
    /// active and the event landed inside the area it was last drawn into.
    /// Returns `true` when the event was consumed (edtui itself does its own
    /// narrower bounds check against the area it recorded at render time,
    /// which excludes the line-number gutter — this outer check only rules
    /// out events elsewhere in the app).
    pub fn handle_mouse(&mut self, m: ratatui::crossterm::event::MouseEvent) -> bool {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        use ratatui::layout::Position;

        if self.active_tab != EditorTab::Body {
            return false;
        }
        let Some(area) = self.last_body_area else {
            return false;
        };
        if !area.contains(Position {
            x: m.column,
            y: m.row,
        }) {
            return false;
        }
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            self.sub_focus = SubFocus::Content;
        }
        self.body_handler.on_mouse_event(m, &mut self.body);
        true
    }
}

/// edtui's emacs keybindings are all registered against `EditorMode::Insert`,
/// so a state left in the default `Normal` mode silently swallows every
/// keystroke. Building body buffers through here keeps that invariant in one
/// place.
fn new_body_state(text: &str) -> EditorState {
    let mut state = EditorState::new(Lines::from(text));
    state.mode = EditorMode::Insert;
    state
}

impl Component for Editor {
    fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match self.sub_focus {
            SubFocus::Url => {
                if self.url.handle_key(ev) {
                    if ev.code == KeyCode::Char('{') && self.url.ends_with_at_cursor("{{") {
                        return Some(Action::OpenVarPicker { completing: true });
                    }
                    return Some(Action::Render);
                }
                if ev.code == KeyCode::Down {
                    self.sub_focus = SubFocus::Content;
                    return Some(Action::Render);
                }
                None
            }
            // On the Params/Headers tabs the table editor gets first crack at
            // every key (including Up/Down navigation within the table); on
            // the Body tab edtui does, except for the two keys that are the
            // only keyboard route back out of the buffer.
            SubFocus::Content => {
                if matches!(self.active_tab, EditorTab::Params | EditorTab::Headers) {
                    let map = match self.active_tab {
                        EditorTab::Params => &mut self.params,
                        EditorTab::Headers => &mut self.headers,
                        EditorTab::Body => unreachable!(),
                    };
                    let outcome = self.table.handle_key(ev, map);
                    if outcome.consumed {
                        if ev.code == KeyCode::Char('{')
                            && self
                                .table
                                .editing
                                .as_ref()
                                .map(|e| &e.input)
                                .is_some_and(|i| i.ends_with_at_cursor("{{"))
                        {
                            return Some(Action::OpenVarPicker { completing: true });
                        }
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
                // Esc always leaves the buffer; Up only does so from the top
                // row, so it can still navigate a multi-line body. CTRL/ALT
                // combos the keymap binds to an app action are shadowed here
                // (the router hands those to the global keymap first); any
                // unbound modified combo falls through to this component and
                // reaches edtui's own emacs-style bindings (ctrl+a/e/k etc.)
                // deliberately, so those keep working for body editing.
                if ev.code == KeyCode::Esc || (ev.code == KeyCode::Up && self.body.cursor.row == 0)
                {
                    self.sub_focus = SubFocus::Url;
                    return Some(Action::Render);
                }
                self.body_handler.on_key_event(ev, &mut self.body);
                Some(Action::Render)
            }
        }
    }

    /// Wheel-over-an-unfocused-pane path (`Action::ScrollPane`). Only the
    /// Body tab has anything under edtui's control to scroll; synthesizes
    /// `|delta|` scroll events and forwards them through `handle_mouse` so
    /// both paths share edtui's own bounds check. Other tabs are a no-op
    /// here (params/headers scrolling isn't wired up).
    ///
    /// The synthesized column must land inside the area edtui itself
    /// recorded (`EditorState::view::screen_area`, set on every render),
    /// which — with a line-numbers gutter always on — is `last_body_area`
    /// *minus* the gutter's width on the left. That width is an edtui-
    /// internal calculation we have no public access to and would have to
    /// duplicate (and could silently drift out of sync with) if we aimed
    /// for the area's left edge, so instead this targets the area's last
    /// column: the gutter is only ever split off the left side, so the
    /// rightmost column of `last_body_area` is always inside edtui's
    /// content area, regardless of how wide the gutter is.
    fn handle_scroll(&mut self, delta: i16) {
        if self.active_tab != EditorTab::Body {
            return;
        }
        let Some(area) = self.last_body_area else {
            return;
        };
        let kind = if delta < 0 {
            ratatui::crossterm::event::MouseEventKind::ScrollUp
        } else {
            ratatui::crossterm::event::MouseEventKind::ScrollDown
        };
        let column = area.x + area.width.saturating_sub(1);
        for _ in 0..delta.unsigned_abs() {
            self.handle_mouse(ratatui::crossterm::event::MouseEvent {
                kind,
                column,
                row: area.y,
                modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
            });
        }
    }

    fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
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

        self.draw_method_and_url(frame, rows[0], ctx, hits);
        self.draw_tab_bar(frame, rows[1], ctx, hits);
        self.draw_tab_content(frame, rows[2], ctx, hits);
    }
}

/// `[ label ]` rendered at the same width whether it reads `Send` or
/// `Cancel`, padded to the wider of the two so the button never jumps.
fn send_button_label(sending: bool) -> String {
    const WIDTH: usize = 6; // "Cancel".chars().count()
    let label = if sending { "Cancel" } else { "Send" };
    format!("{label:<WIDTH$}")
}

impl Editor {
    fn draw_method_and_url(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        let theme = ctx.theme;
        let send_label = send_button_label(self.sending);
        let send_width = crate::hit::button_width(&send_label);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(8),
                Constraint::Min(0),
                Constraint::Length(0), // reserved for the Copy URL button (Task 6)
                Constraint::Length(send_width),
            ])
            .split(area);

        let badge = Paragraph::new(Line::styled(
            format!("{:^7}", self.method.as_str()),
            Style::default().fg(theme.method_color(self.method)).bold(),
        ));
        frame.render_widget(badge, cols[0]);
        hits.register(cols[0], crate::hit::Hit::MethodSelector);
        self.last_method_area = Some(cols[0]);

        let url_focused = self.sub_focus == SubFocus::Url;
        frame.render_widget(
            Paragraph::new(
                self.url
                    .draw_line_windowed(url_focused, theme, cols[1].width),
            ),
            cols[1],
        );

        crate::hit::button(
            frame,
            hits,
            cols[3],
            &send_label,
            crate::hit::Hit::SendButton,
            ctx.hovered,
            true,
            theme,
        );
    }

    fn draw_tab_bar(
        &self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        let theme = ctx.theme;
        let tabs = [EditorTab::Params, EditorTab::Headers, EditorTab::Body];
        let mut constraints: Vec<Constraint> = tabs
            .iter()
            .map(|t| Constraint::Length((t.label().chars().count() + 2) as u16))
            .collect();
        constraints.push(Constraint::Length(2)); // Body's validity glyph, e.g. "✓ "
        constraints.push(Constraint::Length(if self.substitute_body { 5 } else { 0 })); // "vars "
        constraints.push(Constraint::Min(0));
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);

        for (i, tab) in tabs.iter().enumerate() {
            let base = if *tab == self.active_tab {
                Style::default().fg(theme.accent).bold()
            } else {
                Style::default().fg(theme.text_muted)
            };
            crate::hit::chip(
                frame,
                hits,
                cols[i],
                &format!(" {} ", tab.label()),
                crate::hit::Hit::EditorTab(i),
                ctx.hovered,
                Some(base),
                theme,
            );
        }

        // The Body tab carries a live JSON validity badge, colored from the
        // semantic tokens so it also reads without the glyph.
        let (glyph, color) = if self.body_is_valid() {
            ('✓', theme.success)
        } else {
            ('✗', theme.error)
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("{glyph} "),
                Style::default().fg(color),
            )),
            cols[3],
        );
        if self.substitute_body {
            frame.render_widget(
                Paragraph::new(Line::styled("vars ", Style::default().fg(theme.accent))),
                cols[4],
            );
        }
    }

    /// Builds the muted status lines for enabled inherited (project-default)
    /// headers, shown above the request headers table. Each line notes
    /// whether the name is untouched by the request (`project`), overridden
    /// by an enabled request header (`overridden`), or shadowed by a
    /// disabled request header (`disabled`); the match against `self.headers`
    /// is case-insensitive.
    fn inherited_header_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        self.inherited_headers
            .iter()
            .filter(|(_, entry)| entry.enabled)
            .map(|(name, entry)| {
                let status = match self
                    .headers
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                {
                    Some((_, req_entry)) if req_entry.enabled => "overridden",
                    Some(_) => "disabled",
                    None => "project",
                };
                Line::styled(
                    format!("  ✓ {name}  {}  ({status})", entry.value),
                    Style::default().fg(theme.text_muted),
                )
            })
            .collect()
    }

    fn draw_tab_content(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &DrawCtx,
        hits: &mut crate::hit::HitMap,
    ) {
        let theme = ctx.theme;
        let focused = self.sub_focus == SubFocus::Content;
        self.last_body_area = None;
        match self.active_tab {
            EditorTab::Params => {
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                };
                self.table.draw(
                    frame,
                    area,
                    &self.params,
                    &table_ctx,
                    "No params yet — press a to add",
                    hits,
                );
            }
            EditorTab::Headers => {
                let inherited_lines = self.inherited_header_lines(theme);
                let table_area = if inherited_lines.is_empty() {
                    area
                } else {
                    let split = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(inherited_lines.len() as u16),
                            Constraint::Min(0),
                        ])
                        .split(area);
                    frame.render_widget(Paragraph::new(inherited_lines), split[0]);
                    split[1]
                };
                let table_ctx = DrawCtx {
                    theme,
                    focused,
                    hovered: ctx.hovered,
                };
                self.table.draw(
                    frame,
                    table_area,
                    &self.headers,
                    &table_ctx,
                    "No headers yet — press a to add",
                    hits,
                );
            }
            EditorTab::Body => {
                self.last_body_area = Some(area);
                hits.register(area, crate::hit::Hit::BodyEditor);
                let highlighter = json_highlighter(theme);
                let mut edtui_theme = EditorTheme::default()
                    .base(Style::default().bg(theme.surface).fg(theme.text))
                    .cursor_style(Style::default().add_modifier(Modifier::REVERSED))
                    .line_numbers_style(Style::default().bg(theme.surface).fg(theme.text_muted))
                    .hide_status_line();
                // A cursor block on an unfocused pane reads as "you are typing
                // here", so only the focused editor shows one.
                if !focused {
                    edtui_theme = edtui_theme.hide_cursor();
                }
                let view = EditorView::new(&mut self.body)
                    .theme(edtui_theme)
                    .wrap(true)
                    .line_numbers(LineNumbers::Absolute)
                    .syntax_highlighter(highlighter);
                frame.render_widget(view, area);
            }
        }
    }
}

/// The bundled syntect theme closest to the app palette. edtui's theme names
/// use dashes rather than syntect's dotted defaults. Matching the app's own
/// colors exactly is stage-6 polish; until then a missing theme or JSON
/// syntax definition degrades to unhighlighted text rather than failing the
/// draw.
///
/// Rebuilt per draw because `SyntaxHighlighter` is not `Clone` and the view
/// takes it by value. That is cheap: the theme and syntax *sets* behind it are
/// `Arc`-shared process-wide statics, so only one theme and one syntax
/// reference are cloned, and draws are event-driven rather than continuous.
fn json_highlighter(theme: &Theme) -> Option<SyntaxHighlighter> {
    let name = if theme.is_dark() {
        "base16-ocean-dark"
    } else {
        "base16-ocean-light"
    };
    SyntaxHighlighter::new(name, "json").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use postui_core::model::{HttpRequest, Method};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn typing_into_url_marks_dirty_and_updates_request() {
        let mut e = Editor::default();
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
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
        e.load(
            Some("a".into()),
            HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap(),
        );
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
        assert_eq!(
            app.editor.active_tab,
            EditorTab::Body,
            "backward wraps to last tab"
        );
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
        assert_eq!(
            e.sub_focus,
            SubFocus::Url,
            "default sub_focus starts on the URL line"
        );
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.handle_key(key(KeyCode::Up));
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn body_tab_up_returns_to_url() {
        // Body tab has no table editor to intercept Up at all; it keeps the
        // original Up-returns-to-url-line fallback unconditionally.
        let mut e = Editor {
            active_tab: EditorTab::Body,
            ..Editor::default()
        };
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
        e.params.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        e.params.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
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
        e.params.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        e.params.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        e.sub_focus = SubFocus::Content;
        e.table.selected = 1;
        let action = e.handle_key(key(KeyCode::Up));
        assert_eq!(action, Some(Action::Render));
        assert_eq!(
            e.sub_focus,
            SubFocus::Content,
            "table navigation must not move focus"
        );
        assert_eq!(e.table.selected, 0);
    }

    #[test]
    fn duplicate_key_commit_in_params_tab_shows_warning_toast() {
        let mut e = Editor::default();
        e.params.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
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
    fn body_text_roundtrip_and_empty_means_no_body() {
        let mut e = Editor::default();
        e.set_body_text("{\n  \"a\": 1\n}");
        assert_eq!(e.body_text(), "{\n  \"a\": 1\n}");
        assert!(matches!(e.current_request().body, Some(Body::Json { .. })));
        e.set_body_text("");
        assert!(e.current_request().body.is_none());
    }

    #[test]
    fn typing_in_body_tab_inserts_text_modelessly() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.handle_key(key(KeyCode::Char('{')));
        assert_eq!(
            e.body_text(),
            "{",
            "emacs mode: chars insert without entering a vim insert mode"
        );
    }

    #[test]
    fn esc_in_body_returns_focus_to_url_without_editing() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.set_body_text("{}");
        assert_eq!(e.handle_key(key(KeyCode::Esc)), Some(Action::Render));
        assert_eq!(e.sub_focus, SubFocus::Url);
        assert_eq!(e.body_text(), "{}");
    }

    #[test]
    fn body_up_leaves_editor_only_from_the_first_row() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            sub_focus: SubFocus::Content,
            ..Editor::default()
        };
        e.set_body_text("one\ntwo");
        e.handle_key(key(KeyCode::Down)); // move to row 1 inside the body
        assert_eq!(
            e.sub_focus,
            SubFocus::Content,
            "Up/Down navigate inside a multi-line body"
        );
        e.handle_key(key(KeyCode::Up)); // back to row 0, still inside
        assert_eq!(e.sub_focus, SubFocus::Content);
        e.handle_key(key(KeyCode::Up)); // at row 0 → leave for the URL line
        assert_eq!(e.sub_focus, SubFocus::Url);
    }

    #[test]
    fn save_preserves_invalid_body_verbatim() {
        let mut e = Editor::default();
        e.set_body_text("{ \"in-progress\": ");
        let req = e.current_request();
        let back = HttpRequest::from_toml_str(&req.to_toml_string()).unwrap();
        assert_eq!(
            back.body,
            Some(Body::Json {
                text: "{ \"in-progress\": ".into()
            })
        );
    }

    #[test]
    fn format_body_pretty_prints_only_valid_json() {
        let mut app = App::new_for_test();
        app.editor.set_body_text("{\"a\":1}");
        app.update(Action::FormatBody);
        assert!(app.editor.body_text().contains('\n'));
        app.editor.set_body_text("{oops");
        app.update(Action::FormatBody);
        assert_eq!(app.editor.body_text(), "{oops", "invalid body untouched");
        app.editor.set_body_text("{ \"a\": 1 }");
        app.update(Action::MinifyBody);
        assert_eq!(app.editor.body_text(), "{\"a\":1}");
    }

    #[test]
    fn format_body_on_invalid_json_toasts_position_and_empty_body_is_a_noop() {
        let mut app = App::new_for_test();
        app.editor.set_body_text("{\n  \"a\": oops\n}");
        assert!(app.update(Action::FormatBody));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| app.toasts.draw(f, f.area(), &theme))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("line 2"),
            "toast must carry the error position: {content}"
        );

        let mut app = App::new_for_test();
        assert!(
            app.update(Action::MinifyBody),
            "empty body is a no-op, not an error"
        );
        assert_eq!(app.editor.body_text(), "");
        assert!(app.toasts.is_empty(), "no toast for an empty body");
    }

    #[test]
    fn open_body_in_editor_defers_to_the_main_loop() {
        let mut app = App::new_for_test();
        assert!(app.update(Action::OpenBodyInEditor));
        assert_eq!(
            app.pending_terminal_action.take(),
            Some(Action::OpenBodyInEditor),
            "App::update must not touch the terminal itself"
        );
    }

    #[test]
    fn body_tab_label_shows_json_validity() {
        let render = |text: &str| {
            let mut e = Editor {
                active_tab: EditorTab::Body,
                ..Editor::default()
            };
            e.set_body_text(text);
            let theme = Theme::dark();
            let ctx = DrawCtx {
                theme: &theme,
                focused: true,
                hovered: None,
            };
            let backend = TestBackend::new(60, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal
                .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
                .unwrap();
            format!("{:?}", terminal.backend().buffer())
        };
        assert!(render("").contains("Body ✓"), "empty body counts as valid");
        assert!(render("{\"a\": 1}").contains("Body ✓"));
        assert!(render("{oops").contains("Body ✗"));
    }

    #[test]
    fn body_tab_renders_its_text() {
        let mut e = Editor {
            active_tab: EditorTab::Body,
            ..Editor::default()
        };
        e.set_body_text("{\"marker\": 1}");
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
        };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(
            content.contains("marker"),
            "body text must render: {content}"
        );
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
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
        };
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("POST"), "method badge: {content}");
        assert!(
            content.contains("https://api.example.com/users"),
            "url text: {content}"
        );
        assert!(content.contains("Params"), "params tab label: {content}");
        assert!(content.contains("Headers"), "headers tab label: {content}");
        assert!(content.contains("Body"), "body tab label: {content}");
    }

    #[test]
    fn headers_tab_shows_inherited_rows_with_status() {
        let mut e = Editor {
            active_tab: EditorTab::Headers,
            ..Editor::default()
        };
        e.inherited_headers.insert(
            "accept".into(),
            Entry {
                value: "application/json".into(),
                enabled: true,
            },
        );
        e.inherited_headers.insert(
            "x-a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        e.inherited_headers.insert(
            "x-b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        e.headers.insert(
            "X-A".into(),
            Entry {
                value: "9".into(),
                enabled: true,
            },
        );
        e.headers.insert(
            "X-B".into(),
            Entry {
                value: "n".into(),
                enabled: false,
            },
        );
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
        };
        let backend = TestBackend::new(70, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| e.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("(project)"), "{content}");
        assert!(content.contains("(overridden)"), "{content}");
        assert!(content.contains("(disabled)"), "{content}");
        assert!(content.contains("application/json"));
    }

    /// Regression test for a bug where the synthesized scroll events landed
    /// on the line-numbers gutter (always on, `LineNumbers::Absolute`)
    /// rather than inside edtui's own recorded content area, so
    /// `MouseEventHandler`'s bounds check silently dropped every event and
    /// the wheel over an unfocused Body tab did nothing at all.
    #[test]
    fn wheel_scroll_over_unfocused_body_tab_actually_scrolls() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Body;
        let many_lines: String = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.editor.set_body_text(&many_lines);
        assert_eq!(app.editor.body.cursor.row, 0, "starts at the top");

        // Render once for real so the view records its (gutter-inclusive)
        // area, exactly as `EditorView::render` does on every draw.
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert!(app.editor.last_body_area.is_some(), "body area recorded");

        // A large downward scroll, well past the visible page, should move
        // the viewport (and, via edtui's own clamp, the cursor) down from
        // row 0 -- not a "no panic" no-op.
        assert!(app.update(Action::ScrollPane(crate::layout::PaneId::Editor, 100)));
        assert!(
            app.editor.body.cursor.row > 0,
            "scrolling down a long body must move the cursor off row 0, got {}",
            app.editor.body.cursor.row
        );
    }
}
