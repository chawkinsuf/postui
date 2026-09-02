use super::chooser::clip;
use super::palette::fuzzy_match;
use crate::action::Action;
use crate::components::toast::ToastKind;
use crate::components::varmanager::VarEditOp;
use crate::paint::{self, ControlState, FIELD_HEIGHT, ListRow, RowHighlight, TextField};
use crate::theme::Theme;
use indexmap::IndexMap;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Which of the three name sources an Insert-mode [`VarEntry`] comes from
/// (spec §6: "scope-badged (request / project / selector member)"). A name
/// shadowed at a more specific scope (a request `[variables]` entry
/// overriding a project variable of the same name) is listed once, tagged
/// with the scope that actually resolves — `Request` beats `Project`/
/// `Group`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarScope {
    Request,
    Project,
    Group,
}

impl VarScope {
    fn badge(self) -> &'static str {
        match self {
            VarScope::Request => "req",
            VarScope::Project => "proj",
            VarScope::Group => "grp",
        }
    }
}

/// One declared variable, as offered by the Insert-mode picker: its name,
/// optional description (from `variables.toml`), resolved value (from
/// `prepare_context().vars`, when the variable has one), which scope
/// declares it, and whether it's a secret (its value is never shown even
/// when `value` is `Some` — the row renders a masked placeholder instead).
#[derive(Clone)]
pub struct VarEntry {
    pub name: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub scope: VarScope,
    pub secret: bool,
}

/// Builds the Insert-mode picker's entries (spec §6: "autocomplete over
/// all defined names") from every source that declares one: project
/// variables (`model.vars`, minus names that are actually selector fields —
/// a field may carry its own top-level table for a description, but it's
/// listed once, as `Group`), selector fields (`model.selectors`), and the open
/// request's own `[variables]` (`request_vars`; `None`/disabled entries
/// show as unset rather than being dropped). A name defined at more than
/// one scope (a request entry shadowing a project variable) appears once,
/// tagged with the scope that actually resolves — request wins.
pub fn insert_entries(
    model: &postui_core::varmodel::VarModel,
    resolved: &indexmap::IndexMap<String, String>,
    request_vars: &indexmap::IndexMap<String, postui_core::model::Entry>,
) -> Vec<VarEntry> {
    let group_fields: std::collections::HashSet<&str> = model
        .selectors
        .values()
        .flat_map(|g| g.fields.iter().map(String::as_str))
        .collect();

    let mut entries: Vec<VarEntry> = Vec::new();

    for (name, decl) in &model.vars {
        if group_fields.contains(name.as_str()) {
            continue;
        }
        entries.push(VarEntry {
            name: name.clone(),
            description: decl.description.clone(),
            value: resolved.get(name).cloned(),
            scope: VarScope::Project,
            secret: decl.secret,
        });
    }

    for selector in model.selectors.values() {
        for field in &selector.fields {
            let description = model.vars.get(field).and_then(|d| d.description.clone());
            entries.push(VarEntry {
                name: field.clone(),
                description,
                value: resolved.get(field).cloned(),
                scope: VarScope::Group,
                secret: false,
            });
        }
    }

    for (name, entry) in request_vars {
        let value = entry.enabled.then(|| entry.value.clone());
        match entries.iter_mut().find(|e| &e.name == name) {
            Some(existing) => {
                existing.scope = VarScope::Request;
                existing.value = value;
                existing.secret = false;
            }
            None => entries.push(VarEntry {
                name: name.clone(),
                description: None,
                value,
                scope: VarScope::Request,
                secret: false,
            }),
        }
    }

    entries
}

/// Which flow [`VarPickerState`] is running (spec §6's two picker
/// contexts). `Insert` is today's autocomplete-over-all-names behavior
/// (Task 15 upgrades its entries with scope badges). `SelectOption` is
/// Task 14's second context: the cursor sat on an existing `{{name}}`
/// token whose name is a selector field — `name` is that token's own name
/// and `selector` is the owning selector's. The rows shown are the selector's
/// *entries*, not the declared names, and `Enter` never touches the token
/// text — it records a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerMode {
    Insert,
    SelectOption { name: String, selector: String },
}

/// One entry row, as offered by the `SelectOption` picker: its name,
/// optional description, and the values the detail pane renders for it
/// (`values` for a selector option, `value` for a plain single-value one —
/// never both). `selected` marks the env's current selection for this
/// selector with a ✓.
#[derive(Clone)]
pub struct SelectOption {
    pub key: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub selected: bool,
    /// The raw per-field values this option carries — `None` for a plain
    /// variable option (whose single value lives in `value` instead);
    /// `Some(member -> new value)` for a selector option, in member order —
    /// what the detail pane renders for the highlighted row.
    pub values: Option<IndexMap<String, String>>,
}

/// A list of declared variables (`Insert` mode) or of one variable's/
/// selector's options (`SelectOption` mode). Insert mode mirrors
/// `ChooserState`: typed input fuzzy-filters the list. SelectOption mode
/// has no filter — its lists are a handful of rows, so typed text is inert
/// and a detail pane below the list shows the highlighted option's values.
/// In both, arrows move the selection; `Enter` confirms (inserts a token in
/// `Insert` mode, records a selection in `SelectOption` mode) and closes.
/// `completing` (only meaningful in `Insert` mode) distinguishes whether
/// the picker was triggered mid-`{{` (Enter inserts just the closing
/// `name}}`) or explicitly (Enter inserts the full `{{name}}` token);
/// `Esc` always just closes — a typed `{{` that triggered the picker is
/// left as literal text in that case.
pub struct VarPickerState {
    input: String,
    selected: usize,
    entries: Vec<VarEntry>,
    select_entries: Vec<SelectOption>,
    filtered: Vec<usize>,
    pub completing: bool,
    pub mode: PickerMode,
    /// The active environment's name, captured at open — `SelectOption`
    /// mode's target for `VarEditOp::SelectOption` and its toast. Unused (empty)
    /// in `Insert` mode.
    env: String,
    /// First visible row's index into `filtered`. See `ChooserState` for the
    /// `ensure_visible` contract this mirrors.
    scroll: usize,
    ensure_visible: bool,
}

impl VarPickerState {
    pub fn new(entries: Vec<VarEntry>, completing: bool) -> Self {
        let filtered = (0..entries.len()).collect();
        Self {
            input: String::new(),
            selected: 0,
            entries,
            select_entries: Vec::new(),
            filtered,
            completing,
            mode: PickerMode::Insert,
            env: String::new(),
            scroll: 0,
            ensure_visible: true,
        }
    }

    /// Opens `SelectOption` mode: `entries` are `selector`'s entries in the
    /// active environment (`name` is the token that led here — see
    /// [`PickerMode`]), with the env's current selection already marked.
    /// `env` is the active environment, captured for the `Enter` action.
    /// The current filter text (test-visible: the click-a-token flow seeds
    /// it, spec §7).
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Pastes into the fuzzy filter (the bracketed-paste/ctrl+v path),
    /// flattened to one line. Only Insert mode has a filter — mirroring
    /// the typed-char path, a paste in `SelectOption` mode is inert and
    /// reports unhandled.
    pub fn paste(&mut self, text: &str) -> bool {
        if self.mode != PickerMode::Insert {
            return false;
        }
        self.input
            .push_str(&crate::components::line_input::flatten_paste(text));
        self.refilter();
        true
    }

    /// Pre-seeds the fuzzy filter (and re-filters), so the picker opens
    /// already narrowed — clicking an inline `{{token}}` seeds it with that
    /// token's name (spec §7).
    pub fn seed_filter(&mut self, text: &str) {
        self.input = text.to_string();
        self.refilter();
    }

    pub fn new_select(
        entries: Vec<SelectOption>,
        name: String,
        selector: String,
        env: String,
    ) -> Self {
        let filtered = (0..entries.len()).collect();
        // Open with the cursor on the env's current option (the row already
        // carrying the ◉ mark), not row 0.
        let selected = entries.iter().position(|o| o.selected).unwrap_or(0);
        Self {
            input: String::new(),
            selected,
            entries: Vec::new(),
            select_entries: entries,
            filtered,
            completing: false,
            mode: PickerMode::SelectOption { name, selector },
            env,
            scroll: 0,
            ensure_visible: true,
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Total selectable rows: the filtered entries, plus one extra ghost
    /// row, always the last row regardless of what's typed — "new
    /// variable…" in `Insert` mode (spec §6: the autocomplete "ends with
    /// 'new variable…'"), "add new option…" in `SelectOption` mode (Task
    /// 17, spec §6's in-context "Add new option…" flow).
    pub(crate) fn row_count(&self) -> usize {
        self.filtered.len() + 1
    }

    /// Moves the cursor to row `i` (clamped in range, including the
    /// `Insert`-mode ghost row) and asks the next draw to scroll it into
    /// view.
    pub fn select(&mut self, i: usize) {
        if i < self.row_count() {
            self.selected = i;
            self.ensure_visible = true;
        }
    }

    /// The `ModalResult` an `Enter` (or a confirming click) on the current
    /// selection produces — `None` when nothing is selected.
    pub fn confirm(&self) -> Option<super::modal::ModalResult> {
        if self.selected == self.filtered.len() {
            // The ghost row: open the create-and-insert prompt (`Insert`
            // mode) or the "add new option…" prompt (`SelectOption` mode),
            // pre-filled with whatever was typed, and close this picker
            // (not stack on top of it) so focus stays exactly where it was
            // once the prompt itself confirms or cancels.
            return match &self.mode {
                PickerMode::Insert => Some(super::modal::ModalResult {
                    actions: vec![Action::OpenNewVariablePrompt {
                        prefill: self.input.clone(),
                        completing: self.completing,
                    }],
                    close: true,
                    ..Default::default()
                }),
                PickerMode::SelectOption { selector, .. } => {
                    let owner = selector.clone();
                    Some(super::modal::ModalResult {
                        actions: vec![Action::OpenNewOptionInlinePrompt { owner }],
                        close: true,
                        ..Default::default()
                    })
                }
            };
        }
        let &idx = self.filtered.get(self.selected)?;
        match &self.mode {
            PickerMode::Insert => {
                let name = &self.entries[idx].name;
                let text = if self.completing {
                    format!("{name}}}}}")
                } else {
                    format!("{{{{{name}}}}}")
                };
                Some(super::modal::ModalResult {
                    actions: vec![Action::InsertVarText(text)],
                    close: true,
                    ..Default::default()
                })
            }
            PickerMode::SelectOption { selector, .. } => {
                // The selection is recorded under the *selector's* name
                // (spec §3.1: one selected entry per selector, shared by
                // every field) — the token's own name is only for display
                // (the picker's title).
                let owner = selector.clone();
                let key = self.select_entries[idx].key.clone();
                let toast = format!("{owner} \u{2192} {key} ({})", self.env);
                Some(super::modal::ModalResult {
                    actions: vec![
                        Action::VarEdit(VarEditOp::SelectOption {
                            env: self.env.clone(),
                            selector: owner,
                            option: key,
                        }),
                        Action::ShowToast(toast, ToastKind::Success),
                    ],
                    close: true,
                    ..Default::default()
                })
            }
        }
    }

    /// Adjusts `scroll` by `delta` lines, clamped, without moving
    /// `selected`. A no-op on an empty list.
    pub fn scroll_by(&mut self, delta: i16) {
        if self.row_count() == 0 {
            return;
        }
        let max = self.row_count().saturating_sub(1);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
        self.ensure_visible = false;
    }

    fn refilter(&mut self) {
        self.filtered = match &self.mode {
            PickerMode::Insert => self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    let haystack = match &entry.description {
                        Some(desc) => format!("{} {}", entry.name, desc),
                        None => entry.name.clone(),
                    };
                    fuzzy_match(&self.input, &haystack)
                })
                .map(|(i, _)| i)
                .collect(),
            // SelectOption mode has no filter — the list is always the
            // full option set (nothing routes typed text here; see
            // `handle_key`'s Insert-only filter arms).
            PickerMode::SelectOption { .. } => (0..self.select_entries.len()).collect(),
        };
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
                if self.selected + 1 < self.row_count() {
                    self.selected += 1;
                }
                self.ensure_visible = true;
            }
            // SelectOption mode has no filter (its option lists are a
            // handful of rows; the detail pane owns the freed space), so
            // typed text is inert there — only Insert mode edits `input`.
            KeyCode::Backspace if self.mode == PickerMode::Insert => {
                self.input.pop();
                self.refilter();
            }
            KeyCode::Char(c)
                if self.mode == PickerMode::Insert
                    && key.modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
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
        // Insert-mode chrome: 1 pad + 1 title + 1 ring-margin gap + 3-row
        // filter field + 1 gap + 3 bottom pad. SelectOption mode has no
        // filter field; its chrome is 1 pad + 1 title + 1 gap + 1 gap +
        // 1 rule + the detail pane + 2 bottom pad, the pane sized to the
        // tallest option so the modal doesn't resize as the highlight moves.
        const CHROME: u16 = 10;
        let pane_h = match &self.mode {
            PickerMode::Insert => 0,
            PickerMode::SelectOption { .. } => self
                .select_entries
                .iter()
                .map(pane_line_count)
                .max()
                .unwrap_or(1)
                .clamp(1, 8) as u16,
        };
        let chrome = match &self.mode {
            PickerMode::Insert => CHROME,
            PickerMode::SelectOption { .. } => 7 + pane_h,
        };
        let content_rows = (self.row_count() as u16).clamp(1, 10);
        let height = match &self.mode {
            PickerMode::Insert => (chrome + content_rows).clamp(13, 26),
            PickerMode::SelectOption { .. } => (chrome + content_rows).min(26),
        }
        .min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        hits.register(area, crate::hit::Hit::ModalBody);
        paint::floating_panel_settling(frame.buffer_mut(), area, screen, theme, t);
        if t < 1.0 {
            return;
        }

        let title_y = area.y + 1;
        let title = match &self.mode {
            PickerMode::Insert => "Variables".to_string(),
            PickerMode::SelectOption { selector, .. } => {
                format!("Select \u{2014} {selector}")
            }
        };
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            title_y,
            &title,
            theme.text,
            theme.panel,
            true,
        );

        // Only Insert mode has a typed filter field: what's typed there can
        // become a new variable's name. SelectOption mode's list starts
        // right under the title — the editing is the selection.
        let list_y = match &self.mode {
            PickerMode::Insert => {
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
                field_area.y + FIELD_HEIGHT + 2
            }
            PickerMode::SelectOption { .. } => title_y + 2,
        };
        let list_area = Rect {
            x: area.x + 1,
            y: list_y,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(chrome),
        };
        let list_h = list_area.height as usize;
        if self.ensure_visible {
            if list_h > 0 {
                if self.selected < self.scroll {
                    self.scroll = self.selected;
                } else if self.selected >= self.scroll + list_h {
                    self.scroll = self.selected + 1 - list_h;
                }
                let max_scroll = self.row_count().saturating_sub(list_h);
                self.scroll = self.scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        // No hover-fade animation is wired for popup lists (transient
        // surfaces — see the task report); a hovered row shows its full
        // hover fill immediately, same convention as `DrawCtx::hover_t`'s
        // own documented default when no fade is in flight.
        let hover_t = 1.0;
        let row_count = self.row_count();
        for i in (self.scroll..row_count).take(list_h.max(1)) {
            let text_row = list_area.y + (i - self.scroll) as u16;
            let selected = i == self.selected;
            let row_hovered = hovered == Some(&crate::hit::Hit::VarPickerRow(i));
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

            let right = list_area.x + list_area.width;
            let mut x = list_area.x + 1;
            // The ghost row: sits one past the filtered entries at
            // `filtered.len()` — "new variable…" in `Insert` mode, "add
            // new option…" in `SelectOption` mode.
            let ghost_row = i == self.filtered.len();
            if ghost_row {
                let label = match &self.mode {
                    PickerMode::Insert => "+ new variable\u{2026}",
                    PickerMode::SelectOption { .. } => "+ add new option\u{2026}",
                };
                paint::text(
                    frame.buffer_mut(),
                    x,
                    text_row,
                    label,
                    theme.accent,
                    row_fill,
                    selected,
                );
                let row_rect = Rect {
                    x: list_area.x,
                    y: text_row,
                    width: list_area.width,
                    height: 1,
                };
                hits.register(row_rect, crate::hit::Hit::VarPickerRow(i));
                continue;
            }
            match &self.mode {
                PickerMode::Insert => {
                    let entry = &self.entries[self.filtered[i]];
                    // A fixed left column (wide enough for "proj", the
                    // longest origin tag) of quiet colored text — no chip
                    // fill — so every row's name starts at the same column
                    // regardless of which scope declared it.
                    const ORIGIN_COL_W: u16 = 5;
                    let tag = format!("{:<4} ", entry.scope.badge());
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        &tag,
                        theme.text_muted,
                        row_fill,
                        false,
                    );
                    x += ORIGIN_COL_W;
                    if entry.secret {
                        // The lock glyph is double-width in most terminals
                        // (unlike the ✓ used elsewhere in this file) — use
                        // its real display width, not its char count, so
                        // the name after it doesn't overlap the glyph's
                        // second cell.
                        const LOCK: &str = "\u{1f512} ";
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            LOCK,
                            theme.warning,
                            row_fill,
                            false,
                        );
                        x += Span::raw(LOCK).width() as u16;
                    }
                    let name_w = (entry.name.chars().count() as u16).min(right.saturating_sub(x));
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        &entry.name,
                        theme.text,
                        row_fill,
                        selected,
                    );
                    x += name_w;
                    if let Some(desc) = &entry.description {
                        let desc = format!(" {desc}");
                        let w = right.saturating_sub(x);
                        let clipped = clip(&desc, w);
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            clipped,
                            theme.text_muted,
                            row_fill,
                            false,
                        );
                        x += clipped.chars().count() as u16;
                    }
                    if entry.secret {
                        // Secret VALUES never render, per whether or not one
                        // is resolved — a masked placeholder stands in.
                        let w = right.saturating_sub(x);
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            clip(" \u{25cf}\u{25cf}\u{25cf}\u{25cf}", w),
                            theme.text_muted,
                            row_fill,
                            false,
                        );
                    } else {
                        match &entry.value {
                            Some(v) => {
                                let s = format!(" = {v}");
                                let w = right.saturating_sub(x);
                                paint::text(
                                    frame.buffer_mut(),
                                    x,
                                    text_row,
                                    clip(&s, w),
                                    theme.text_muted,
                                    row_fill,
                                    false,
                                );
                            }
                            None => {
                                let w = right.saturating_sub(x);
                                paint::text(
                                    frame.buffer_mut(),
                                    x,
                                    text_row,
                                    clip(" unset", w),
                                    theme.warning,
                                    row_fill,
                                    false,
                                );
                            }
                        }
                    }
                }
                PickerMode::SelectOption { .. } => {
                    let entry = &self.select_entries[self.filtered[i]];
                    // A fixed two-column check gutter (mirrors the accent
                    // bar's own column) so unchecked rows still line their
                    // keys up under checked ones.
                    let check = if entry.selected { "\u{2713} " } else { "  " };
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        check,
                        theme.success,
                        row_fill,
                        false,
                    );
                    x += 2;
                    let key_w = (entry.key.chars().count() as u16).min(right.saturating_sub(x));
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        text_row,
                        &entry.key,
                        theme.text,
                        row_fill,
                        selected,
                    );
                    x += key_w;
                    // Rows stay lean — key plus muted description. The
                    // option's values live in the detail pane below, which
                    // follows the highlight.
                    if let Some(desc) = &entry.description {
                        let desc = format!(" {desc}");
                        let w = right.saturating_sub(x);
                        paint::text(
                            frame.buffer_mut(),
                            x,
                            text_row,
                            clip(&desc, w),
                            theme.text_muted,
                            row_fill,
                            false,
                        );
                    }
                }
            }

            let row_rect = Rect {
                x: list_area.x,
                y: text_row,
                width: list_area.width,
                height: 1,
            };
            hits.register(row_rect, crate::hit::Hit::VarPickerRow(i));
        }

        // SelectOption mode's detail pane: a muted rule, then the
        // highlighted option's values unclipped — "what changes if I pick
        // this?". The rule stays put on the ghost row (the pane just goes
        // blank) so the modal doesn't jump as the highlight moves.
        if matches!(self.mode, PickerMode::SelectOption { .. }) {
            let rule_y = list_area.y + list_area.height + 1;
            let rule = "\u{2500}".repeat(list_area.width as usize);
            paint::text(
                frame.buffer_mut(),
                list_area.x,
                rule_y,
                &rule,
                theme.text_muted,
                theme.panel,
                false,
            );
            let entry = self
                .filtered
                .get(self.selected)
                .map(|&idx| &self.select_entries[idx]);
            if let Some(entry) = entry {
                let x = list_area.x + 1;
                let mut y = rule_y + 1;
                let mut line = |y: u16, name: &str, name_w: u16, value: &str| {
                    paint::text(
                        frame.buffer_mut(),
                        x,
                        y,
                        clip(name, name_w),
                        theme.text_muted,
                        theme.panel,
                        false,
                    );
                    let value_x = x + name_w;
                    let w = (list_area.x + list_area.width).saturating_sub(value_x);
                    paint::text(
                        frame.buffer_mut(),
                        value_x,
                        y,
                        clip(value, w),
                        theme.text,
                        theme.panel,
                        false,
                    );
                };
                match &entry.values {
                    Some(values) => {
                        // Values align in a column two cells past the
                        // longest member name.
                        let name_w =
                            values.keys().map(|k| k.chars().count()).max().unwrap_or(0) as u16 + 2;
                        for (member, value) in values.iter().take(pane_h as usize) {
                            line(y, member, name_w, value);
                            y += 1;
                        }
                    }
                    None => {
                        // A plain single-value option: its description (if
                        // any) then its value.
                        if let Some(desc) = &entry.description {
                            line(y, desc, list_area.width.saturating_sub(2), "");
                            y += 1;
                        }
                        if let Some(value) = &entry.value {
                            line(y, "", 0, value);
                        }
                    }
                }
            }
        }
    }
}

/// How many lines [`VarPickerState::draw`]'s detail pane needs for one
/// option: a member→value line each for a selector option, description +
/// value for a plain one. The pane is sized to the tallest option so the
/// modal doesn't resize as the highlight moves.
fn pane_line_count(o: &SelectOption) -> usize {
    match &o.values {
        Some(values) => values.len().max(1),
        None => (o.description.is_some() as usize + o.value.is_some() as usize).max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A `VarScope::Project`, non-secret entry — the common case for tests
    /// that don't care about badges.
    fn var_entry(name: &str, description: Option<&str>, value: Option<&str>) -> VarEntry {
        VarEntry {
            name: name.to_string(),
            description: description.map(str::to_string),
            value: value.map(str::to_string),
            scope: VarScope::Project,
            secret: false,
        }
    }

    #[test]
    fn insert_entries_merges_a_name_declared_at_both_project_and_request_scope_into_one_row() {
        let mut model = postui_core::varmodel::VarModel::default();
        model.vars.insert(
            "trace_id".to_string(),
            postui_core::varmodel::VarDecl {
                description: Some("project-level default".to_string()),
                default: Some("proj-value".to_string()),
                secret: false,
            },
        );
        let mut resolved = indexmap::IndexMap::new();
        resolved.insert("trace_id".to_string(), "proj-value".to_string());
        let mut request_vars = indexmap::IndexMap::new();
        request_vars.insert(
            "trace_id".to_string(),
            postui_core::model::Entry {
                value: "req-value".to_string(),
                enabled: true,
            },
        );

        let entries = insert_entries(&model, &resolved, &request_vars);

        assert_eq!(
            entries.len(),
            1,
            "one name defined at two scopes is one row, not two"
        );
        let entry = &entries[0];
        assert_eq!(entry.name, "trace_id");
        assert_eq!(entry.scope, VarScope::Request, "request shadows project");
        assert_eq!(
            entry.value.as_deref(),
            Some("req-value"),
            "the request's own value wins, not the resolved project value"
        );
        assert!(!entry.secret);
    }

    #[test]
    fn enter_emits_completion_or_full_token() {
        let entries = vec![var_entry("base_url", None, Some("x"))];
        let mut p = VarPickerState::new(entries.clone(), true);
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::InsertVarText("base_url}}".into())]
        );
        let mut p = VarPickerState::new(entries, false);
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::InsertVarText("{{base_url}}".into())]
        );
    }

    #[test]
    fn esc_closes_with_no_actions() {
        let mut p = VarPickerState::new(vec![var_entry("a", None, None)], true);
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn typing_filters_on_name_and_description() {
        let mut p = VarPickerState::new(
            vec![
                var_entry("base", Some("api root"), None),
                var_entry("tok", None, Some("secret")),
            ],
            false,
        );
        for c in "root".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(res.actions, vec![Action::InsertVarText("{{base}}".into())]);
    }

    #[test]
    fn no_key_hint_footer_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(vec![var_entry("a", None, None)], false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("enter insert"), "{content}");
        assert!(!content.contains("esc cancel"), "{content}");
    }

    #[test]
    fn draw_renders_names_values_and_unset_tag() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(
            vec![
                var_entry("base", Some("api root"), Some("http://x")),
                var_entry("tok", None, None),
            ],
            false,
        );
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("base"));
        assert!(content.contains("api root"));
        assert!(content.contains("http://x"));
        assert!(content.contains("unset"));
    }

    fn entries(names: &[&str]) -> Vec<VarEntry> {
        names.iter().map(|n| var_entry(n, None, None)).collect()
    }

    #[test]
    fn field_fill_and_gap_row_survive_the_list_draw() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
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
    fn selected_row_is_a_dense_selection_fill_with_an_accent_bar() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::VarPickerRow(0)).unwrap();
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
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
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
                    Some(&crate::hit::Hit::VarPickerRow(1)),
                    1.0,
                )
            })
            .unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::VarPickerRow(1)).unwrap();
        let buffer = terminal.backend().buffer();
        let right_edge = buffer[(row1.x + row1.width - 1, row1.y)].clone();
        assert_eq!(
            right_edge.bg, theme.control,
            "the hovered (non-selected) row's pill fill must span the full row width"
        );
    }

    #[test]
    fn select_mode_enter_emits_select_edit_and_toast() {
        let entries = vec![
            SelectOption {
                key: "alice".into(),
                description: Some("admin".into()),
                value: Some("qa-token".into()),
                selected: false,
                values: None,
            },
            SelectOption {
                key: "bob".into(),
                description: None,
                value: Some("qa-bob".into()),
                selected: true,
                values: None,
            },
        ];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        // The cursor opens on the env's current option (bob), not row 0.
        assert_eq!(p.selected(), 1);
        p.handle_key(key(KeyCode::Up));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![
                Action::VarEdit(VarEditOp::SelectOption {
                    env: "qa".into(),
                    selector: "user".into(),
                    option: "alice".into(),
                }),
                Action::ShowToast("user \u{2192} alice (qa)".into(), ToastKind::Success),
            ]
        );
        assert!(res.close);
    }

    #[test]
    fn select_mode_group_member_targets_the_group_name() {
        let entries = vec![SelectOption {
            key: "alice".into(),
            description: None,
            value: None,
            selected: false,
            values: None,
        }];
        let mut p =
            VarPickerState::new_select(entries, "user_id".into(), "identity".into(), "qa".into());
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![
                Action::VarEdit(VarEditOp::SelectOption {
                    env: "qa".into(),
                    selector: "identity".into(),
                    option: "alice".into(),
                }),
                Action::ShowToast("identity \u{2192} alice (qa)".into(), ToastKind::Success),
            ]
        );
    }

    #[test]
    fn select_mode_esc_closes_without_editing() {
        let entries = vec![SelectOption {
            key: "alice".into(),
            description: None,
            value: Some("x".into()),
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        let res = p.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn select_mode_draw_marks_current_selection_and_titles_the_selector() {
        let mut p = VarPickerState::new_select(
            identity_options(),
            "user_id".into(),
            "identity".into(),
            "qa".into(),
        );
        let (content, _) = draw_select(&mut p);
        assert!(content.contains("\u{2713}"), "checked row shows a ✓");
        assert!(content.contains("alice"));
        assert!(content.contains("bob"));
        assert!(content.contains("Select"));
        assert!(content.contains("identity"));
    }

    #[test]
    fn rows_sit_on_a_dense_one_line_pitch() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut p = VarPickerState::new(entries(&["base", "token", "env"]), false);
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::VarPickerRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::VarPickerRow(1)).unwrap();
        let row2 = hits.rect_of(&crate::hit::Hit::VarPickerRow(2)).unwrap();
        assert_eq!(row1.y - row0.y, 1, "rows sit on a dense 1-row pitch");
        assert_eq!(row2.y - row1.y, 1, "rows sit on a dense 1-row pitch");
    }

    #[test]
    fn origin_tags_sit_in_a_fixed_column_regardless_of_scope_label_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // "proj" (4 chars) is the longest origin tag; "req"/"grp" (3 chars)
        // must still leave every entry's name starting at the same column.
        let mut p = VarPickerState::new(
            vec![
                VarEntry {
                    name: "a".into(),
                    description: None,
                    value: None,
                    scope: VarScope::Project,
                    secret: false,
                },
                VarEntry {
                    name: "b".into(),
                    description: None,
                    value: None,
                    scope: VarScope::Group,
                    secret: false,
                },
            ],
            false,
        );
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::VarPickerRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::VarPickerRow(1)).unwrap();
        let buffer = terminal.backend().buffer();
        // Column x = list_area.x + 1 (row inset) + 5 (fixed origin column).
        let name_col = row0.x + 1 + 5;
        assert_eq!(buffer[(name_col, row0.y)].symbol(), "a");
        assert_eq!(buffer[(name_col, row1.y)].symbol(), "b");
        // The origin tag itself is plain colored text, not a filled chip:
        // row 1 (unselected, unhovered) shows the tag on the plain panel
        // fill, not a tinted chip color.
        assert_eq!(buffer[(row1.x + 1, row1.y)].bg, theme.panel);
    }

    // --- Task 17: in-context flows (spec §6) -------------------------------

    #[test]
    fn select_mode_ghost_row_opens_new_option_inline_prompt() {
        let entries = vec![SelectOption {
            key: "alice".into(),
            description: Some("admin".into()),
            value: Some("qa-token".into()),
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        // Down once lands on the ghost row (row 1, one past the one entry).
        p.handle_key(key(KeyCode::Down));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::OpenNewOptionInlinePrompt {
                owner: "user".into()
            }]
        );
        assert!(res.close);
    }

    #[test]
    fn select_mode_ghost_row_targets_the_group_name() {
        let entries = vec![SelectOption {
            key: "alice".into(),
            description: None,
            value: None,
            selected: false,
            values: Some(IndexMap::new()),
        }];
        let mut p =
            VarPickerState::new_select(entries, "user_id".into(), "identity".into(), "qa".into());
        p.handle_key(key(KeyCode::Down));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            res.actions,
            vec![Action::OpenNewOptionInlinePrompt {
                owner: "identity".into()
            }]
        );
    }

    #[test]
    fn select_mode_ghost_row_renders_add_new_option_label() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let entries = vec![SelectOption {
            key: "alice".into(),
            description: None,
            value: Some("x".into()),
            selected: false,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "user".into(), "user".into(), "qa".into());
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("add new option"), "{content}");
    }

    /// Two selector options carrying member values, bob pre-selected —
    /// the fixture for the filterless select mode's detail pane.
    fn identity_options() -> Vec<SelectOption> {
        let mut alice_values = IndexMap::new();
        alice_values.insert("role".to_string(), "admin".to_string());
        alice_values.insert("user_id".to_string(), "1001".to_string());
        let mut bob_values = IndexMap::new();
        bob_values.insert("role".to_string(), "reader".to_string());
        bob_values.insert("user_id".to_string(), "1002".to_string());
        vec![
            SelectOption {
                key: "alice".into(),
                description: None,
                value: None,
                selected: false,
                values: Some(alice_values),
            },
            SelectOption {
                key: "bob".into(),
                description: None,
                value: None,
                selected: true,
                values: Some(bob_values),
            },
        ]
    }

    fn draw_select(p: &mut VarPickerState) -> (String, crate::hit::HitMap) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| p.draw(f, f.area(), &theme, &mut hits, None, 1.0))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    #[test]
    fn select_mode_renders_no_filter_field() {
        let mut p = VarPickerState::new_select(
            identity_options(),
            "user_id".into(),
            "identity".into(),
            "qa".into(),
        );
        let (content, hits) = draw_select(&mut p);
        assert!(
            !content.contains("\u{1f50d}") && !content.contains("filter\u{2026}"),
            "select mode paints no filter field: {content}"
        );
        // With the field gone the list starts right under the title row.
        let modal = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::VarPickerRow(0)).unwrap();
        assert_eq!(row0.y, modal.y + 3, "list sits one gap row under the title");
    }

    #[test]
    fn select_mode_typing_is_inert() {
        let mut p = VarPickerState::new_select(
            identity_options(),
            "user_id".into(),
            "identity".into(),
            "qa".into(),
        );
        assert_eq!(p.selected(), 1, "opens on bob, the env's current option");
        for c in "alice".chars() {
            assert!(p.handle_key(key(KeyCode::Char(c))).is_none());
        }
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(
            p.selected(),
            1,
            "typing neither filters nor moves the cursor"
        );
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(
            res.actions.iter().any(|a| matches!(
                a,
                Action::VarEdit(VarEditOp::SelectOption { option, .. }) if option == "bob"
            )),
            "Enter still confirms the highlighted row: {:?}",
            res.actions
        );
    }

    #[test]
    fn select_mode_detail_pane_shows_only_the_highlighted_options_values() {
        let mut p = VarPickerState::new_select(
            identity_options(),
            "user_id".into(),
            "identity".into(),
            "qa".into(),
        );
        // bob is highlighted: his member values render (in the pane), and
        // alice's don't render anywhere — rows no longer carry a preview tail.
        let (content, _) = draw_select(&mut p);
        assert!(content.contains("reader"), "{content}");
        assert!(content.contains("1002"), "{content}");
        assert!(!content.contains("admin"), "{content}");
        assert!(!content.contains("1001"), "{content}");
        assert!(
            content.contains("\u{2500}"),
            "a rule separates list and pane"
        );
    }

    #[test]
    fn select_mode_detail_pane_follows_the_selection() {
        let mut p = VarPickerState::new_select(
            identity_options(),
            "user_id".into(),
            "identity".into(),
            "qa".into(),
        );
        p.handle_key(key(KeyCode::Up));
        let (content, _) = draw_select(&mut p);
        assert!(content.contains("admin"), "{content}");
        assert!(!content.contains("reader"), "{content}");
    }

    #[test]
    fn select_mode_plain_option_pane_shows_description_and_value() {
        let entries = vec![SelectOption {
            key: "staging".into(),
            description: Some("the shared box".into()),
            value: Some("https://stg.example.com".into()),
            selected: true,
            values: None,
        }];
        let mut p = VarPickerState::new_select(entries, "host".into(), "host".into(), "qa".into());
        let (content, _) = draw_select(&mut p);
        assert!(
            content.contains("https://stg.example.com"),
            "the value renders in the pane now that rows drop it: {content}"
        );
        assert!(content.contains("the shared box"), "{content}");
        assert!(
            !content.contains("= https"),
            "the row's old `= value` tail is gone: {content}"
        );
        assert!(content.contains("\u{2500}"), "the pane's rule renders");
    }

    #[test]
    fn select_mode_ghost_row_pane_is_blank() {
        let mut p = VarPickerState::new_select(
            identity_options(),
            "user_id".into(),
            "identity".into(),
            "qa".into(),
        );
        // Down from bob (row 1) lands on the ghost row.
        p.handle_key(key(KeyCode::Down));
        let (content, _) = draw_select(&mut p);
        assert!(
            !content.contains("reader") && !content.contains("admin"),
            "no option's values render while the ghost row is highlighted: {content}"
        );
        assert!(
            content.contains("\u{2500}"),
            "the rule stays put so the modal doesn't jump: {content}"
        );
    }

    #[test]
    fn e_in_insert_mode_types_into_the_filter_instead_of_editing() {
        let mut p = VarPickerState::new(entries(&["env"]), false);
        p.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        let res = p.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(res.actions, vec![Action::InsertVarText("{{env}}".into())]);
    }
}
