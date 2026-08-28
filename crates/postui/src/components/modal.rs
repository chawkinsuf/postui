use super::line_input::LineInput;
use crate::action::{Action, ExtractDestination};
use crate::components::varmanager::VarStructOp;
use crate::paint::{
    self, BUTTON_HEIGHT, Button, ButtonKind, ControlState, FIELD_HEIGHT, TextField,
};
use crate::theme::Theme;
use indexmap::IndexMap;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

/// What a `Modal::Prompt`'s confirmed text becomes: which `Action` it maps
/// to, and (for rename) which slug is prefilled/being renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    NewRequest,
    RenameRequest {
        from: String,
    },
    SaveAs,
    /// The never-saved-scratch gate's save path: a Save-as whose success
    /// chains the gate's deferred action (quit, open, switch…).
    SaveAsThen(Box<Action>),
    OpenProjectPath,
    SaveBodyAs,
    /// The toolbar 💾 button's save: writes the active tab's text rather
    /// than always the raw body.
    SaveViewAs,
    /// The env chooser's "new environment…" row: the text is the new
    /// environment's name (slug rules).
    NewEnvironment,
    /// `n` / the `+ Variable` button: a bare variable name — the detail
    /// pane's form sets its default/description afterward.
    NewVariable,
    /// The Insert-mode picker's "new variable…" row (Task 15, spec §6): the
    /// text is the new variable's name, pre-filled with what was typed in
    /// the picker. Confirming both declares the variable (`VarStructOp::
    /// NewVar`) and inserts its `{{name}}` token — `completing` picks
    /// between the completion form (`name}}`, closing an already-typed
    /// `{{`) and the full token, mirroring `VarPickerState::confirm`.
    NewVariableAndInsert {
        completing: bool,
    },
    /// `g` / the `+ Selector` button: a single name prompt. Confirming
    /// declares a one-field selector whose field is the name itself (the
    /// common selection-set shape); more fields grow through the fields
    /// editor afterward.
    NewSelector,
    /// One field name, appended to `selector`'s list.
    AddSelectorField {
        selector: String,
    },
    /// `e`/`F2` on a variable row.
    RenameVariable {
        from: String,
    },
    /// The option-row context menu's "Rename…" (Task 16): the text is the
    /// option's new name within `selector` in `env`.
    RenameOption {
        env: String,
        selector: String,
        from: String,
    },
    /// Send-time secret prompt (spec §3): `prepare()` reported `name`
    /// missing for the active environment (`env`, display only — never a
    /// secret value). Confirming dispatches `Action::SetSecret`. The
    /// modal's `revealed` flag (not part of this enum — see
    /// `Modal::Prompt`) controls whether the input renders masked.
    SecretValue {
        name: String,
        env: String,
    },
    /// The `SelectOption` picker's "add new option…" ghost row (Task 17,
    /// spec §6): a `Modal::MultiPrompt` with `key`/`value`/`description`
    /// fields (in that order — see `PromptField`'s `key`s). Confirming
    /// emits `Action::ConfirmNewOptionInline`, which writes to the active
    /// environment and selects it.
    NewOptionInline {
        owner: String,
    },
    /// `e` on a highlighted `SelectOption` row (Task 17, spec §6): a
    /// `Modal::MultiPrompt` with one field per value (the option's own
    /// `value`, or one per selector field) plus a trailing `description`
    /// field. Confirming emits `Action::ConfirmEditOption`, which writes to
    /// wherever the option currently lives.
    EditOption {
        owner: String,
        key: String,
    },
    /// `Action::ExtractToVariable` (Task 17, spec §6): a `Modal::MultiPrompt`
    /// with a `name` field and a `destination` choice field (project
    /// default / active env value / this request). Confirming emits
    /// `Action::ConfirmExtractVariable`; the origin field to rewrite is
    /// re-read from current focus rather than carried here.
    ExtractVariable,
    /// Clicking a simple variable's inline `{{token}}`
    /// (`Action::OpenVarTokenPopup`): a `Modal::MultiPrompt` with a `value`
    /// field and a `destination` choice field preselected to whichever
    /// scope supplies the value today. `scope_values` holds what each
    /// destination currently stores (destination label → value, `None`
    /// when that scope stores nothing); cycling the destination reseeds
    /// the value field from it, so the box always shows what the chosen
    /// scope holds, and the Remove button shows only where there is a
    /// stored value to delete. Confirming emits
    /// `Action::ConfirmEditVarValue`.
    EditVarValue {
        name: String,
        scope_values: Vec<(String, Option<String>)>,
    },
}

/// One row of the fields editor: the field's current on-disk name (`None`
/// for a row added in this session), its editable text (typing renames),
/// and whether the row is marked for removal (the ✕ button; a marked row
/// shows ↩ to restore instead).
pub struct FieldRow {
    pub original: Option<String>,
    pub input: LineInput,
    pub removed: bool,
}

/// The `Modal::FieldsEditor` state (the selector pane's "Fields of X"):
/// one editable row per current field plus any added rows, applied as one
/// `Action::ApplyGroupFields` transaction on confirm. Position is still
/// the identity underneath — the original rows are emitted in order, a
/// removed row as an empty slot — but removal and addition are explicit
/// buttons rather than blank-the-text conventions.
pub struct FieldsEditorState {
    pub selector: String,
    pub rows: Vec<FieldRow>,
    pub focus: usize,
}

impl FieldsEditorState {
    pub fn new(selector: String, fields: &[String]) -> Self {
        let rows = fields
            .iter()
            .map(|f| FieldRow {
                original: Some(f.clone()),
                input: LineInput::new(f),
                removed: false,
            })
            .collect();
        Self {
            selector,
            rows,
            focus: 0,
        }
    }

    /// The ✕/↩ button: flip row `i`'s removal mark. An added (never-saved)
    /// row is simply dropped — there is nothing to restore.
    pub fn toggle(&mut self, i: usize) {
        let Some(row) = self.rows.get_mut(i) else {
            return;
        };
        if row.original.is_none() {
            self.rows.remove(i);
            if self.focus >= self.rows.len() && self.focus > 0 {
                self.focus = self.rows.len() - 1;
            }
            return;
        }
        row.removed = !row.removed;
        if self.focus == i && row.removed {
            self.focus_step(1);
        }
    }

    /// The "+ Add field" button: append an empty row and focus it.
    pub fn add_row(&mut self) {
        self.rows.push(FieldRow {
            original: None,
            input: LineInput::new(""),
            removed: false,
        });
        self.focus = self.rows.len() - 1;
    }

    /// Moves focus by `dir`, skipping removed rows, wrapping.
    fn focus_step(&mut self, dir: i32) {
        let n = self.rows.len() as i32;
        if n == 0 {
            return;
        }
        let mut i = self.focus as i32;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if !self.rows[i as usize].removed {
                self.focus = i as usize;
                return;
            }
        }
    }

    /// The slots `Action::ApplyGroupFields` wants: original rows in order
    /// (removed = empty slot), then non-empty added rows.
    fn slots(&self) -> Vec<String> {
        let mut slots: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.original.is_some())
            .map(|r| {
                if r.removed {
                    String::new()
                } else {
                    r.input.text().trim().to_string()
                }
            })
            .collect();
        for row in self.rows.iter().filter(|r| r.original.is_none()) {
            let text = row.input.text().trim();
            if !row.removed && !text.is_empty() {
                slots.push(text.to_string());
            }
        }
        slots
    }
}

/// One field of a `Modal::MultiPrompt`: a stable domain `key` (e.g.
/// `"value"`, `"description"`, or a selector field's own name) distinct from
/// its display `label`, a text buffer, and — for a fixed-choice field like
/// `ExtractVariable`'s destination — the list of choices Left/Right cycle
/// through instead of free typing (`choices.is_empty()` is an ordinary text
/// field).
pub struct PromptField {
    pub key: String,
    pub label: String,
    pub input: LineInput,
    pub choices: Vec<String>,
}

impl PromptField {
    pub fn text(key: &str, label: &str, seed: &str) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            input: LineInput::new(seed),
            choices: Vec::new(),
        }
    }

    pub fn choice(key: &str, label: &str, choices: &[&str]) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            input: LineInput::new(choices.first().copied().unwrap_or("")),
            choices: choices.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Steps a choice field's selection by `dir` (`-1`/`1`), clamped at
    /// the ends — a bounded stepper, not a loop, so the two ends orient
    /// the user in the list. A no-op on an ordinary text field, and at
    /// the end the step points past.
    pub(crate) fn cycle(&mut self, dir: i32) {
        if self.choices.is_empty() {
            return;
        }
        let cur = self.choice_index() as i32;
        let next = (cur + dir).clamp(0, self.choices.len() as i32 - 1);
        self.input = LineInput::new(&self.choices[next as usize]);
    }

    /// The index of the currently selected choice (0 when the text
    /// matches none — the seed always comes from the list).
    pub(crate) fn choice_index(&self) -> usize {
        self.choices
            .iter()
            .position(|c| c == self.input.text())
            .unwrap_or(0)
    }
}

impl PromptKind {
    /// Whether this prompt's input must render masked (`●` per char) and
    /// accepts the reveal toggle — currently just the send-time secret
    /// prompt (spec §3: masked everywhere by default).
    fn is_secret(&self) -> bool {
        matches!(self, PromptKind::SecretValue { .. })
    }
}

/// Kind-specific follow-up to cycling a choice field. For
/// `EditVarValue`, the value field is reseeded with what the newly chosen
/// destination currently stores — the box always shows what's being
/// edited, not a leftover from the previous scope.
pub(crate) fn resync_after_choice_cycle(kind: &PromptKind, fields: &mut [PromptField]) {
    let PromptKind::EditVarValue { scope_values, .. } = kind else {
        return;
    };
    let Some(dest) = fields
        .iter()
        .find(|f| f.key == "destination")
        .map(|f| f.input.text().to_string())
    else {
        return;
    };
    let seed = scope_values
        .iter()
        .find(|(label, _)| *label == dest)
        .and_then(|(_, v)| v.clone())
        .unwrap_or_default();
    if let Some(value_field) = fields.iter_mut().find(|f| f.key == "value") {
        value_field.input = LineInput::new(&seed);
    }
}

/// The value popup's empty value box: a muted "(not set)" (the variable
/// form's own wording), led by the same reversed-cell caret an empty
/// `LineInput` draws when the field is focused — without it, a focused
/// empty box gave no hint the input was selected.
fn value_placeholder_line(focused: bool, theme: &Theme) -> Line<'static> {
    use ratatui::style::Modifier;
    use ratatui::text::Span;
    let muted = Style::default().fg(theme.text_muted);
    if focused {
        Line::from(vec![
            Span::styled(
                " ",
                Style::default()
                    .fg(theme.text)
                    .add_modifier(Modifier::REVERSED),
            ),
            Span::styled("(not set)", muted),
        ])
    } else {
        Line::styled("(not set)", muted)
    }
}

/// The `ExtractDestination` a Write-to label stands for — shared by the
/// value popup's confirm and its Remove button.
pub(crate) fn destination_from_label(label: &str) -> ExtractDestination {
    match label {
        "Active env value" => ExtractDestination::ActiveEnv,
        "This request" => ExtractDestination::Request,
        _ => ExtractDestination::ProjectDefault,
    }
}

pub enum Modal {
    Message {
        title: String,
        body: String,
    },
    /// A choice prompt: each option in `choices` is `(key, label, actions)` —
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
        /// For a masked prompt (`kind.is_secret()`), whether the reveal
        /// toggle (`ctrl+r`) currently shows the typed text in plaintext
        /// instead of `●` per char. Ignored (and always effectively
        /// false-masked) for every other kind.
        revealed: bool,
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
    /// A multi-field prompt (Task 17, spec §6): the in-context option/
    /// extract flows' modal — one `LineInput` per `PromptField`, Tab/Down
    /// (or Shift-Tab/Up) switching focus, mirroring `NewProject`'s two-field
    /// layout generalized to N fields. `Enter` builds and dispatches
    /// `kind`'s action from the fields' current text; `Esc` closes with no
    /// action.
    MultiPrompt {
        title: String,
        fields: Vec<PromptField>,
        focus: usize,
        kind: PromptKind,
    },
    /// The selector fields editor ("Fields of X"): one text row per field
    /// with a ✕/↩ removal toggle, a "+ Add field" button, applied as one
    /// `Action::ApplyGroupFields` on confirm.
    FieldsEditor(FieldsEditorState),
}

/// One row of a `Modal::Dropdown` — a value in a select popup, or an option
/// in a right-click context menu. `action: None` marks the row *disabled*:
/// it still paints (so the menu's shape doesn't shift with context) but in
/// the muted text color, takes no hover fill, and neither a click nor Enter
/// activates it or closes the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub action: Option<Action>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        Self {
            label: label.into(),
            action: Some(action),
        }
    }

    /// A row that is shown but cannot be chosen — e.g. "Open" on a request
    /// whose file doesn't parse.
    pub fn disabled(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.action.is_some()
    }
}

/// State for `Modal::Dropdown`: the cell (or pointer position) it opens
/// from, its [`MenuItem`] rows, which row the keyboard cursor is on, and
/// (separately) which row reflects the value already in effect (e.g. the
/// method that was active when the dropdown opened). `selected` moves as
/// the user arrows through the list; `current` does not — it stays put so
/// the `✓` marker keeps pointing at the actual current value even while the
/// highlight is elsewhere.
pub struct DropdownState {
    pub anchor: Rect,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub current: Option<usize>,
}

impl DropdownState {
    /// The next enabled row `delta` steps from `selected` in that direction,
    /// or `None` when there is none (edge of the list, or nothing but
    /// disabled rows beyond it) — in which case the cursor stays put.
    fn step(&self, delta: isize) -> Option<usize> {
        let mut i = self.selected as isize;
        loop {
            i += delta;
            if i < 0 || i as usize >= self.items.len() {
                return None;
            }
            if self.items[i as usize].is_enabled() {
                return Some(i as usize);
            }
        }
    }

    /// The first row a keyboard cursor may usefully land on.
    pub fn first_enabled(items: &[MenuItem]) -> usize {
        items.iter().position(MenuItem::is_enabled).unwrap_or(0)
    }
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

    /// The `LineInput` that currently owns the keyboard in the top modal,
    /// if that modal is a text-option kind — used by the app's ctrl+c
    /// copy-selection interception.
    pub fn focused_input(&self) -> Option<&LineInput> {
        match self.stack.last()? {
            Modal::Prompt { input, .. } => Some(input),
            Modal::NewProject {
                name,
                path,
                on_path,
                ..
            } => Some(if *on_path { path } else { name }),
            Modal::MultiPrompt { fields, focus, .. } => fields.get(*focus).map(|f| &f.input),
            Modal::FieldsEditor(state) => state
                .rows
                .get(state.focus)
                .filter(|r| !r.removed)
                .map(|r| &r.input),
            _ => None,
        }
    }

    /// The `Hit::ModalInput` index of the text box that currently holds
    /// the top modal's field focus, if any — what a click-time window
    /// mapping needs to know *before* `focus_input` moves the focus.
    pub fn focused_input_index(&self) -> Option<usize> {
        match self.stack.last()? {
            Modal::Prompt { .. } => Some(0),
            Modal::NewProject { on_path, .. } => Some(usize::from(*on_path)),
            Modal::MultiPrompt { fields, focus, .. } => fields
                .get(*focus)
                .filter(|f| f.choices.is_empty())
                .map(|_| *focus),
            Modal::FieldsEditor(state) => state
                .rows
                .get(state.focus)
                .filter(|r| !r.removed)
                .map(|_| state.focus),
            _ => None,
        }
    }

    /// Moves the top modal's field focus to text box `i` (a
    /// `Hit::ModalInput` index) and returns that box's `LineInput` — the
    /// mouse path's counterpart to Tab/Down field switching. `None` when
    /// the top modal has no text box `i`.
    pub fn focus_input(&mut self, i: usize) -> Option<&mut LineInput> {
        match self.stack.last_mut()? {
            Modal::Prompt { input, .. } if i == 0 => Some(input),
            Modal::NewProject { name, on_path, .. } if i == 0 => {
                *on_path = false;
                Some(name)
            }
            Modal::NewProject { path, on_path, .. } if i == 1 => {
                *on_path = true;
                Some(path)
            }
            Modal::MultiPrompt { fields, focus, .. } => {
                let field = fields.get_mut(i)?;
                if !field.choices.is_empty() {
                    return None;
                }
                *focus = i;
                Some(&mut field.input)
            }
            Modal::FieldsEditor(state) => {
                let row = state.rows.get_mut(i)?;
                if row.removed {
                    return None;
                }
                state.focus = i;
                Some(&mut row.input)
            }
            _ => None,
        }
    }

    pub fn top(&self) -> Option<&Modal> {
        self.stack.last()
    }

    /// Every modal on the stack, bottom first — for asking "is one of
    /// these already open?" before pushing another.
    pub fn iter(&self) -> impl Iterator<Item = &Modal> {
        self.stack.iter()
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
            Modal::Prompt {
                input,
                kind,
                revealed,
                ..
            } => match key.code {
                KeyCode::Esc => Some(ModalResult {
                    // The send-time secret prompt (spec §3) cancels the
                    // whole send, not just this one field — surfaced so the
                    // user isn't left wondering whether anything happened.
                    actions: if kind.is_secret() {
                        vec![Action::ShowToast(
                            "send canceled".to_string(),
                            crate::components::toast::ToastKind::Warning,
                        )]
                    } else {
                        vec![]
                    },
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Char('r' | 'R')
                    if kind.is_secret() && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    *revealed = !*revealed;
                    None // swallowed: modals capture all input
                }
                KeyCode::Enter => {
                    let text = input.text().trim();
                    if text.is_empty() {
                        return None; // swallowed: nothing to confirm yet
                    }
                    let actions: Option<Vec<Action>> = match kind {
                        PromptKind::NewRequest => {
                            Some(vec![Action::CreateRequest(text.to_string())])
                        }
                        PromptKind::RenameRequest { from } => Some(vec![Action::RenameRequest {
                            from: from.clone(),
                            to: text.to_string(),
                        }]),
                        PromptKind::SaveAs => Some(vec![Action::SaveRequestAs(text.to_string())]),
                        PromptKind::SaveAsThen(then) => Some(vec![Action::SaveRequestAsThen(
                            text.to_string(),
                            then.clone(),
                        )]),
                        PromptKind::OpenProjectPath => {
                            Some(vec![Action::OpenProjectByPath(text.to_string())])
                        }
                        PromptKind::SaveBodyAs => {
                            Some(vec![Action::SaveBodyToFile(text.to_string())])
                        }
                        PromptKind::SaveViewAs => {
                            Some(vec![Action::SaveViewToFile(text.to_string())])
                        }
                        PromptKind::NewEnvironment => {
                            Some(vec![Action::CreateEnv(text.to_string())])
                        }
                        PromptKind::NewVariable => {
                            Some(vec![Action::VarStruct(VarStructOp::NewVar {
                                name: text.to_string(),
                                description: None,
                            })])
                        }
                        PromptKind::NewVariableAndInsert { completing } => {
                            let name = text.to_string();
                            let insert_text = if *completing {
                                format!("{name}}}}}")
                            } else {
                                format!("{{{{{name}}}}}")
                            };
                            Some(vec![
                                Action::VarStruct(VarStructOp::NewVar {
                                    name,
                                    description: None,
                                }),
                                Action::InsertVarText(insert_text),
                            ])
                        }
                        PromptKind::AddSelectorField { selector } => {
                            Some(vec![Action::AddSelectorField {
                                selector: selector.clone(),
                                field: text.to_string(),
                            }])
                        }
                        PromptKind::RenameVariable { from } => {
                            Some(vec![Action::VarStruct(VarStructOp::Rename {
                                from: from.clone(),
                                to: text.to_string(),
                            })])
                        }
                        PromptKind::NewSelector => {
                            Some(vec![Action::VarStruct(VarStructOp::NewSelector {
                                name: text.to_string(),
                                fields: vec![text.to_string()],
                            })])
                        }
                        PromptKind::RenameOption {
                            env,
                            selector,
                            from,
                        } => Some(vec![Action::VarStruct(VarStructOp::RenameOption {
                            env: env.clone(),
                            selector: selector.clone(),
                            from: from.clone(),
                            to: text.to_string(),
                        })]),
                        PromptKind::SecretValue { name, .. } => Some(vec![Action::SetSecret {
                            name: name.clone(),
                            value: text.to_string(),
                        }]),
                        // These kinds are `Modal::MultiPrompt` only — never a
                        // single-input `Modal::Prompt`.
                        PromptKind::NewOptionInline { .. }
                        | PromptKind::EditOption { .. }
                        | PromptKind::ExtractVariable
                        | PromptKind::EditVarValue { .. } => {
                            unreachable!(
                                "multi-field prompt kinds only ever back Modal::MultiPrompt"
                            )
                        }
                    };
                    // A well-formed-but-incomplete comma prompt (e.g. a
                    // selector option still missing a field) swallows Enter
                    // rather than closing on nonsense — same "not ready
                    // yet" treatment as the empty-text case above.
                    actions.map(|actions| ModalResult {
                        actions,
                        close: true,
                        ..Default::default()
                    })
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
                // Arrows step over disabled rows rather than parking the
                // cursor on a dead end, so Enter always has something to do.
                KeyCode::Up => {
                    if let Some(i) = state.step(-1) {
                        state.selected = i;
                    }
                    None // swallowed: modals capture all input
                }
                KeyCode::Down => {
                    if let Some(i) = state.step(1) {
                        state.selected = i;
                    }
                    None // swallowed: modals capture all input
                }
                // A disabled row has no action: swallowed, menu stays open.
                KeyCode::Enter => state
                    .items
                    .get(state.selected)?
                    .action
                    .clone()
                    .map(|action| ModalResult {
                        actions: vec![action],
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
            Modal::MultiPrompt {
                fields,
                kind,
                focus,
                ..
            } => match key.code {
                KeyCode::Esc => Some(ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Tab | KeyCode::Down => {
                    *focus = (*focus + 1) % fields.len();
                    None // swallowed: modals capture all input
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *focus = (*focus + fields.len() - 1) % fields.len();
                    None // swallowed: modals capture all input
                }
                KeyCode::Left if !fields[*focus].choices.is_empty() => {
                    fields[*focus].cycle(-1);
                    resync_after_choice_cycle(kind, fields);
                    None // swallowed: modals capture all input
                }
                KeyCode::Right if !fields[*focus].choices.is_empty() => {
                    fields[*focus].cycle(1);
                    resync_after_choice_cycle(kind, fields);
                    None // swallowed: modals capture all input
                }
                KeyCode::Enter => {
                    let get = |k: &str| {
                        fields
                            .iter()
                            .find(|f| f.key == k)
                            .map(|f| f.input.text().trim())
                    };
                    let actions = match kind {
                        PromptKind::NewOptionInline { owner } => {
                            let key_text = get("key").filter(|s| !s.is_empty())?.to_string();
                            let value = get("value").unwrap_or("").to_string();
                            let description = get("description")
                                .filter(|s| !s.is_empty())
                                .map(str::to_string);
                            vec![Action::ConfirmNewOptionInline {
                                owner: owner.clone(),
                                key: key_text,
                                value,
                                description,
                            }]
                        }
                        PromptKind::EditOption { owner, key } => {
                            let mut values = IndexMap::new();
                            for f in fields.iter() {
                                if f.key != "description" {
                                    values.insert(f.key.clone(), f.input.text().to_string());
                                }
                            }
                            let description = get("description")
                                .filter(|s| !s.is_empty())
                                .map(str::to_string);
                            vec![Action::ConfirmEditOption {
                                owner: owner.clone(),
                                key: key.clone(),
                                values,
                                description,
                            }]
                        }
                        PromptKind::ExtractVariable => {
                            let name = get("name").filter(|s| !s.is_empty())?.to_string();
                            let destination = match get("destination") {
                                Some("Active env value") => ExtractDestination::ActiveEnv,
                                Some("This request") => ExtractDestination::Request,
                                _ => ExtractDestination::ProjectDefault,
                            };
                            vec![Action::ConfirmExtractVariable { name, destination }]
                        }
                        PromptKind::EditVarValue { name, .. } => {
                            // An emptied value is a legitimate edit, so no
                            // non-empty filter here; removing the stored
                            // value outright is the Remove button.
                            let value = fields
                                .iter()
                                .find(|f| f.key == "value")
                                .map(|f| f.input.text().to_string())?;
                            let destination =
                                destination_from_label(get("destination").unwrap_or_default());
                            vec![Action::ConfirmEditVarValue {
                                name: name.clone(),
                                value,
                                destination,
                            }]
                        }
                        _ => return None, // not a MultiPrompt kind
                    };
                    Some(ModalResult {
                        actions,
                        close: true,
                        ..Default::default()
                    })
                }
                _ => {
                    if fields[*focus].choices.is_empty() {
                        fields[*focus].input.handle_key(key);
                    }
                    None // swallowed: modals capture all input
                }
            },
            Modal::FieldsEditor(state) => match key.code {
                KeyCode::Esc => Some(ModalResult {
                    actions: vec![],
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Enter => Some(ModalResult {
                    actions: vec![Action::ApplyGroupFields {
                        selector: state.selector.clone(),
                        slots: state.slots(),
                        confirmed: false,
                    }],
                    close: true,
                    ..Default::default()
                }),
                KeyCode::Tab | KeyCode::Down => {
                    state.focus_step(1);
                    None // swallowed: modals capture all input
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.focus_step(-1);
                    None // swallowed: modals capture all input
                }
                _ => {
                    if let Some(row) = state.rows.get_mut(state.focus)
                        && !row.removed
                    {
                        row.input.handle_key(key);
                    }
                    None // swallowed: modals capture all input
                }
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

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        hits: &mut crate::hit::HitMap,
        hovered: Option<&crate::hit::Hit>,
        keymap: &crate::keys::Keymap,
        anims: &crate::anim::Anims,
        now: std::time::Instant,
    ) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        // Every variant dims the backdrop except Dropdown: it's a small
        // anchored popup (e.g. the method selector), not a screen-owning
        // modal, so dimming everything behind it would be jarring.
        let is_dropdown = matches!(top, Modal::Dropdown(_));
        // `AnimKey::ModalOpen` is the panel-style shell's own open-settle —
        // a `Dropdown` keeps only its `DropdownOpen` settle (see
        // `App::push_modal`, which never retargets `ModalOpen` for a
        // `Dropdown` push), so it always reads as fully open here.
        let t = if is_dropdown {
            1.0
        } else {
            anims.value_or(crate::anim::AnimKey::ModalOpen, now, 1.0)
        };
        if !is_dropdown {
            paint::dim_backdrop(frame.buffer_mut(), screen, t);
        }
        // Registered before the modal's own hits so any click landing
        // outside them (topmost-wins in `HitMap`) closes the modal, same as
        // Esc — live for every variant, not just Dropdown.
        hits.register(screen, crate::hit::Hit::ModalOutside);
        match top {
            Modal::Message { title, body } => {
                let area = centered_rect(screen, 60.min(screen.width), 13.min(screen.height));
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
                .paint(frame.buffer_mut(), btn_area, theme);
                hits.register(btn_area, crate::hit::Hit::ModalConfirm);
            }
            Modal::Confirm {
                title,
                body,
                choices,
            } => {
                let area = centered_rect(screen, 60.min(screen.width), 13.min(screen.height));
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
                // (matching whichever choice text says "Cancel", if any).
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
                    .paint(frame.buffer_mut(), btn_area, theme);
                    hits.register(btn_area, crate::hit::Hit::ConfirmChoice(*c));
                    x += w + 2;
                }
            }
            Modal::Prompt {
                title,
                input,
                kind,
                revealed,
            } => {
                let masked = kind.is_secret() && !*revealed;
                // Height unchanged from the hint-row days — the secret
                // prompt still uses that row for its reveal toggle hint,
                // and everywhere else the space keeps the shell airy.
                let area = centered_rect(screen, 60.min(screen.width), 14.min(screen.height));
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
                let content = if masked {
                    input.draw_line_windowed_masked(true, theme, field_area.width.saturating_sub(2))
                } else {
                    input.draw_line_windowed(true, theme, field_area.width.saturating_sub(2))
                };
                TextField {
                    content,
                    state: ControlState::Focused,
                }
                .paint(frame.buffer_mut(), field_area, theme);
                hits.register(field_area, crate::hit::Hit::ModalInput(0));

                if kind.is_secret() {
                    let hint_y = field_area.y + FIELD_HEIGHT + 1;
                    let hint = if *revealed {
                        "ctrl+r hide"
                    } else {
                        "ctrl+r reveal"
                    };
                    paint::text(
                        frame.buffer_mut(),
                        area.x + 2,
                        hint_y,
                        hint,
                        theme.text_muted,
                        theme.panel,
                        false,
                    );
                }

                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                draw_cancel_confirm_row(frame, hits, theme, area, buttons_y, hovered);
            }
            Modal::Palette(state) => state.draw(frame, screen, theme, hits, hovered, keymap, t),
            Modal::Chooser(state) => state.draw(frame, screen, theme, hits, hovered, t),
            Modal::VarPicker(state) => state.draw(frame, screen, theme, hits, hovered, t),
            Modal::NewProject {
                name,
                path,
                on_path,
                ..
            } => {
                let area = centered_rect(screen, 60.min(screen.width), 19.min(screen.height));
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
                hits.register(name_area, crate::hit::Hit::ModalInput(0));

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
                hits.register(path_area, crate::hit::Hit::ModalInput(1));

                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                draw_cancel_confirm_row(frame, hits, theme, area, buttons_y, hovered);
            }
            Modal::Dropdown(state) => {
                draw_dropdown(frame, screen, theme, hits, hovered, state, anims, now)
            }
            Modal::MultiPrompt {
                title,
                fields,
                focus,
                kind,
            } => {
                // Each field costs a label row plus a `FIELD_HEIGHT` box;
                // around them sit the top pad, the title, a blank row, a
                // gap row, the button row and the bottom pad. Counting the
                // boxes (not just 2 rows per field) is what keeps the
                // bottom-anchored buttons off the last field — with two
                // fields the old estimate had them painting straight
                // through it.
                let per_field = 1 + FIELD_HEIGHT;
                let height = (3 + fields.len() as u16 * per_field + 1 + BUTTON_HEIGHT + 1)
                    .min(screen.height);
                let area = centered_rect(screen, 60.min(screen.width), height);
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
                    title,
                    theme.text,
                    theme.panel,
                    true,
                );

                let field_x = area.x + 2;
                let field_w = area.width.saturating_sub(4);
                let mut y = title_y + 2;
                for (i, field) in fields.iter().enumerate() {
                    let focused = i == *focus;
                    let label = format!("{}:", field.label);
                    paint::text(
                        frame.buffer_mut(),
                        field_x,
                        y,
                        &label,
                        theme.text_muted,
                        theme.panel,
                        false,
                    );
                    let field_area = Rect {
                        x: field_x,
                        y: y + 1,
                        width: field_w,
                        height: FIELD_HEIGHT,
                    };
                    // Label row + input box together: click focuses the
                    // field (registered after ModalBody, so it wins).
                    hits.register(
                        Rect {
                            y,
                            height: 1 + FIELD_HEIGHT,
                            ..field_area
                        },
                        crate::hit::Hit::ModalField(i),
                    );
                    if field.choices.is_empty() {
                        // The value popup's empty value box reads "(not
                        // set)" (the variable form's own wording) — the
                        // chosen scope stores nothing, and typing replaces
                        // the placeholder the moment the field is non-empty.
                        let placeholder = matches!(kind, PromptKind::EditVarValue { .. })
                            && field.key == "value"
                            && field.input.text().is_empty();
                        let content = if placeholder {
                            value_placeholder_line(focused, theme)
                        } else {
                            field.input.draw_line_windowed(
                                focused,
                                theme,
                                field_w.saturating_sub(2),
                            )
                        };
                        TextField {
                            content,
                            state: if focused {
                                ControlState::Focused
                            } else {
                                ControlState::Normal
                            },
                        }
                        .paint(frame.buffer_mut(), field_area, theme);
                        // On top of `ModalField`, so the box itself gets
                        // the full text-input mouse treatment while the
                        // label row keeps the plain focus-click.
                        hits.register(field_area, crate::hit::Hit::ModalInput(i));
                    } else {
                        // The label leaves room for the `‹`/`›` arrows,
                        // overlaid after the field paints: each is its own
                        // clickable control cycling one step in that
                        // direction (the box's own click still steps
                        // forward). No painted bg of their own — they sit
                        // on the field's fill, whatever the focus state.
                        let content = Line::from(format!("  {}", field.input.text()));
                        TextField {
                            content,
                            state: if focused {
                                ControlState::Focused
                            } else {
                                ControlState::Normal
                            },
                        }
                        .paint(frame.buffer_mut(), field_area, theme);
                        let arrow_y = field_area.y + 1;
                        let at = field.choice_index();
                        for (glyph, x, dir, live) in [
                            ("\u{2039}", field_x + 2, -1i8, at > 0),
                            (
                                "\u{203a}",
                                field_x + field_w.saturating_sub(3),
                                1,
                                at + 1 < field.choices.len(),
                            ),
                        ] {
                            // An arrow with nowhere to go greys out and
                            // takes no clicks — the end stops are what
                            // orient the user in the (unwrapped) list.
                            if !live {
                                frame.buffer_mut().set_string(
                                    x,
                                    arrow_y,
                                    glyph,
                                    Style::default().fg(theme.text_muted),
                                );
                                continue;
                            }
                            let hit = crate::hit::Hit::ModalChoiceArrow { field: i, dir };
                            let style = if hovered == Some(&hit) {
                                Style::default().bg(theme.accent).fg(theme.on_accent)
                            } else {
                                Style::default().fg(theme.accent)
                            };
                            frame.buffer_mut().set_string(x, arrow_y, glyph, style);
                            // A cell either side pads the click target.
                            hits.register(Rect::new(x.saturating_sub(1), arrow_y, 3, 1), hit);
                        }
                    }
                    y += FIELD_HEIGHT + 1;
                }

                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                draw_cancel_confirm_row(frame, hits, theme, area, buttons_y, hovered);
                // The value popup's remove: only when the chosen Write-to
                // scope actually stores a value to delete. Painted as the
                // same one-row "✕ remove" accent control the variable
                // form uses, right-aligned on the value field's label row
                // (registered after `ModalField(0)`, so it wins the hit).
                if let PromptKind::EditVarValue { scope_values, .. } = kind {
                    let chosen = fields
                        .iter()
                        .find(|f| f.key == "destination")
                        .map(|f| f.input.text().to_string())
                        .unwrap_or_default();
                    let stored = scope_values
                        .iter()
                        .find(|(label, _)| *label == chosen)
                        .and_then(|(_, v)| v.as_ref())
                        .is_some();
                    if stored {
                        let label = "\u{2715} remove";
                        let remove_hit = crate::hit::Hit::ModalRemove;
                        let style = if hovered == Some(&remove_hit) {
                            Style::default().bg(theme.accent).fg(theme.on_accent)
                        } else {
                            Style::default().bg(theme.panel).fg(theme.accent)
                        };
                        let w = label.chars().count() as u16;
                        let rect = Rect {
                            x: (field_x + field_w).saturating_sub(w),
                            y: title_y + 2,
                            width: w,
                            height: 1,
                        };
                        frame.buffer_mut().set_string(rect.x, rect.y, label, style);
                        hits.register(rect, remove_hit);
                    }
                }
            }
            Modal::FieldsEditor(state) => {
                // Top pad + title + blank, one `FIELD_HEIGHT` box per row,
                // the add button, a gap, the cancel/confirm row, bottom pad.
                let height = (3
                    + state.rows.len() as u16 * FIELD_HEIGHT
                    + BUTTON_HEIGHT
                    + 1
                    + BUTTON_HEIGHT
                    + 1)
                .min(screen.height);
                let area = centered_rect(screen, 60.min(screen.width), height);
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
                    &format!("Fields of {}", state.selector),
                    theme.text,
                    theme.panel,
                    true,
                );

                let field_x = area.x + 2;
                // The row's text box stops short of a 4-column ✕/↩ zone.
                let toggle_w: u16 = 4;
                let field_w = area.width.saturating_sub(4 + toggle_w);
                let mut y = title_y + 2;
                for (i, row) in state.rows.iter().enumerate() {
                    let focused = i == state.focus && !row.removed;
                    let field_area = Rect {
                        x: field_x,
                        y,
                        width: field_w,
                        height: FIELD_HEIGHT,
                    };
                    if row.removed {
                        // Marked for removal: the name, dim and struck,
                        // where the box was.
                        let name = row.input.text().to_string();
                        let line = Line::from(ratatui::text::Span::styled(
                            name,
                            Style::default()
                                .fg(theme.text_disabled)
                                .add_modifier(ratatui::style::Modifier::CROSSED_OUT),
                        ));
                        TextField {
                            content: line,
                            state: ControlState::Disabled,
                        }
                        .paint(frame.buffer_mut(), field_area, theme);
                    } else {
                        TextField {
                            content: row.input.draw_line_windowed(
                                focused,
                                theme,
                                field_w.saturating_sub(2),
                            ),
                            state: if focused {
                                ControlState::Focused
                            } else {
                                ControlState::Normal
                            },
                        }
                        .paint(frame.buffer_mut(), field_area, theme);
                        hits.register(field_area, crate::hit::Hit::ModalInput(i));
                    }
                    // The ✕ (or ↩ restore) button, on the box's middle row.
                    let toggle_area = Rect {
                        x: field_x + field_w + 1,
                        y,
                        width: toggle_w.saturating_sub(1),
                        height: FIELD_HEIGHT,
                    };
                    let toggle_hit = crate::hit::Hit::ModalRowToggle(i);
                    let toggle_hovered = hovered == Some(&toggle_hit);
                    let glyph = if row.removed { "\u{21a9}" } else { "\u{2715}" };
                    let fg = if toggle_hovered {
                        theme.text
                    } else {
                        theme.text_muted
                    };
                    let bg = if toggle_hovered {
                        theme.control_hover
                    } else {
                        theme.panel
                    };
                    paint::fill(frame.buffer_mut(), toggle_area, bg);
                    paint::text(
                        frame.buffer_mut(),
                        toggle_area.x + 1,
                        y + 1,
                        glyph,
                        fg,
                        bg,
                        false,
                    );
                    hits.register(toggle_area, toggle_hit);
                    y += FIELD_HEIGHT;
                }

                // "+ Add field" under the rows, left-aligned.
                let add_label = "+ Add field";
                let add_area = Rect {
                    x: field_x,
                    y,
                    width: paint::button_min_width(add_label).min(area.width.saturating_sub(4)),
                    height: BUTTON_HEIGHT,
                };
                let add_hit = crate::hit::Hit::ModalAddRow;
                Button {
                    label: add_label,
                    kind: ButtonKind::Secondary,
                    state: if hovered == Some(&add_hit) {
                        ControlState::Hover
                    } else {
                        ControlState::Normal
                    },
                }
                .paint(frame.buffer_mut(), add_area, theme);
                hits.register(add_area, add_hit);

                let buttons_y = area.y + area.height.saturating_sub(1 + BUTTON_HEIGHT);
                draw_cancel_confirm_row(frame, hits, theme, area, buttons_y, hovered);
            }
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
#[allow(clippy::too_many_arguments)]
fn draw_dropdown(
    frame: &mut Frame,
    screen: Rect,
    theme: &Theme,
    hits: &mut crate::hit::HitMap,
    hovered: Option<&crate::hit::Hit>,
    state: &DropdownState,
    anims: &crate::anim::Anims,
    now: std::time::Instant,
) {
    let max_label = state
        .items
        .iter()
        .map(|item| item.label.chars().count() as u16)
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

    // Open-settle: the popup's panel fill grows down from its own top edge
    // over `AnimKey::DropdownOpen` (retargeted 0→1 in `app.rs` on open,
    // snapped straight to 1 on every close path — closing is always
    // instant). `frac_vspan` paints the whole covered rows solid and gives
    // the partial row at the growing edge its fractional glyph; at t=0 the
    // edge sits exactly on `area`'s top row with zero coverage, which
    // `frac_vspan`'s negligible-coverage skip leaves untouched (no flash of
    // `on` on the very first frame).
    let t = anims
        .value_or(crate::anim::AnimKey::DropdownOpen, now, 1.0)
        .clamp(0.0, 1.0);
    let settle_bottom = area.top() as f32 + area.height as f32 * t;
    paint::frac_vspan(
        frame.buffer_mut(),
        area.x,
        area.right(),
        area.top() as f32,
        settle_bottom,
        theme.panel,
        theme.page,
    );
    // The drop shadow reads as noise while the popup is still growing in,
    // so it only appears once settled (a 90ms window by default — this
    // simply skips it for that brief span rather than scaling it too).
    if t >= 1.0 {
        paint::floating_panel(frame.buffer_mut(), area, screen, theme);
    }
    paint::ring(frame.buffer_mut(), area, theme.accent, theme.panel);

    let inner = Rect {
        x: area.x + 2,
        y: area.y + 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(4),
    };
    let visible_bottom = (settle_bottom.floor() as u16).clamp(area.top(), area.bottom());

    for (i, item) in state.items.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let label = &item.label;
        let row_area = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        // The hit registers at its final position regardless of the
        // open-settle animation's progress — a click landing mid-animation
        // (the 90ms window is easy to beat with a fast double-click, and
        // tests draw a single frame at whatever `now` they pass) must
        // resolve exactly as it would once settled. Only the *paint* below
        // is conditional on visibility.
        hits.register(row_area, crate::hit::Hit::DropdownRow(i));
        // The still-growing tail of rows below the settle edge doesn't
        // paint yet — they reveal as the panel fill grows down past them.
        if row_area.y >= visible_bottom {
            continue;
        }
        let enabled = item.is_enabled();
        let selected = enabled && i == state.selected;
        let row_hovered = enabled && hovered == Some(&crate::hit::Hit::DropdownRow(i));
        // A disabled row takes no fill at all: no cursor highlight, no hover
        // response — the only affordance it has is looking muted.
        let highlight = if selected {
            paint::RowHighlight::Selected
        } else if row_hovered {
            paint::RowHighlight::Hover
        } else {
            paint::RowHighlight::None
        };
        // No hover-fade animation is wired for popup lists (transient
        // surfaces), same convention as the var-picker/palette/chooser
        // popups: a hovered row shows its full hover fill immediately.
        let hover_t = 1.0;
        paint::ListRow {
            highlight,
            zebra: None,
        }
        .paint(
            frame.buffer_mut(),
            row_area.y,
            row_area.x,
            row_area.width,
            theme.panel,
            hover_t,
            theme,
        );
        let row_fill = match highlight {
            paint::RowHighlight::None => theme.panel,
            paint::RowHighlight::Hover => theme.control,
            paint::RowHighlight::Cursor => theme.control_hover,
            paint::RowHighlight::Selected => theme.selection,
        };

        // `current` (the value already in effect) gets the checkmark;
        // `selected` (the keyboard cursor) gets its own bold/accent
        // highlight — the two can differ once arrow keys move the cursor
        // away from the current value. Painted from `row_area.x` — the
        // same column `ListRow::Selected` would otherwise put its own left
        // accent bar on — so the marker glyph (or its leading blank) simply
        // overwrites that column: menus show a full-width selection fill
        // with no left bar, unlike the sidebar/palette lists that keep one.
        let marker = if state.current == Some(i) {
            "\u{2713} "
        } else {
            "  "
        };
        let fg = if !enabled {
            theme.text_muted
        } else if selected {
            theme.accent
        } else {
            theme.text
        };
        paint::text(
            frame.buffer_mut(),
            row_area.x,
            row_area.y,
            &format!("{marker}{label}"),
            fg,
            row_fill,
            selected,
        );
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
        Button { label, kind, state }.paint(frame.buffer_mut(), btn_area, theme);
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

    /// User finding: the "(not set)" placeholder swallowed the caret, so a
    /// focused-but-empty value box gave no hint the input was selected.
    /// Focused, the line leads with the same reversed-cell caret an empty
    /// `LineInput` draws; unfocused it's just the muted placeholder.
    #[test]
    fn the_value_placeholder_shows_a_caret_when_focused() {
        let theme = Theme::dark();
        use ratatui::style::Modifier;

        let focused = value_placeholder_line(true, &theme);
        assert_eq!(focused.spans.len(), 2, "caret cell + placeholder text");
        assert!(
            focused.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
            "the leading cell is the caret"
        );
        assert_eq!(focused.spans[1].content.as_ref(), "(not set)");

        let resting = value_placeholder_line(false, &theme);
        assert_eq!(resting.spans.len(), 1);
        assert_eq!(resting.spans[0].content.as_ref(), "(not set)");
        assert!(
            !resting.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
            "no caret while unfocused"
        );
    }

    /// A disabled (instantly-jumping) `Anims` shared by every test's draw
    /// call, so a dropdown's open-settle animation never intrudes on
    /// geometry/behavior assertions that don't care about it — untracked
    /// keys (never `retarget`ed here) read as fully settled via
    /// `value_or`'s default.
    fn test_anims() -> &'static crate::anim::Anims {
        static ANIMS: std::sync::OnceLock<crate::anim::Anims> = std::sync::OnceLock::new();
        ANIMS.get_or_init(|| crate::anim::Anims::new(false))
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

    /// A two-field `MultiPrompt` (extract-to-variable, new selector, …) must
    /// leave room for its bottom-anchored buttons: the old height estimate
    /// counted 2 rows per field instead of the label + `FIELD_HEIGHT` box
    /// it actually paints, so Cancel/Confirm landed *inside* the second
    /// field. Found by the stage-7 tmux sweep.
    #[test]
    fn multi_prompt_buttons_sit_below_the_last_field() {
        let screen = Rect::new(0, 0, 120, 40);
        let mut m = ModalStack::default();
        m.push(Modal::MultiPrompt {
            title: "Extract to variable".into(),
            fields: vec![
                PromptField::text("name", "Name", ""),
                PromptField::text("dest", "Destination", "here"),
            ],
            focus: 0,
            kind: PromptKind::NewSelector,
        });

        let theme = Theme::for_terminal();
        let keymap = crate::keys::Keymap::default_bindings();
        let mut hits = crate::hit::HitMap::default();
        let mut terminal = Terminal::new(TestBackend::new(screen.width, screen.height)).unwrap();
        terminal
            .draw(|f| {
                m.draw(
                    f,
                    screen,
                    &theme,
                    &mut hits,
                    None,
                    &keymap,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
            .unwrap();

        let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let confirm = hits.rect_of(&crate::hit::Hit::ModalConfirm).unwrap();
        // Two fields, each a label row plus a FIELD_HEIGHT box, starting
        // two rows under the title.
        let fields_end = body.y + 3 + 2 * (1 + FIELD_HEIGHT);
        assert!(
            confirm.y >= fields_end,
            "buttons at {} overlap the fields, which end at {fields_end}",
            confirm.y
        );
        assert!(
            confirm.y + confirm.height <= body.y + body.height,
            "...and they stay inside the panel"
        );
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
        // The query is a subsequence match and survives on more than one
        // command, so put the cursor on Quit itself.
        if let Some(Modal::Palette(p)) = m.top_mut() {
            let i = p.filtered().iter().position(|c| c.id == "quit").unwrap();
            p.select(i);
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

    fn draw_modal(m: &mut ModalStack) -> String {
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                m.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    None,
                    &keymap,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn prompts_and_confirms_show_no_key_hint_row() {
        // Enter/Esc/Tab behavior is implied; the hint rows are gone.
        let mut m = ModalStack::default();
        m.push(Modal::Prompt {
            title: "New request".into(),
            input: LineInput::new(""),
            kind: PromptKind::NewRequest,
            revealed: false,
        });
        let content = draw_modal(&mut m);
        assert!(!content.contains("esc cancel"), "{content}");
        assert!(!content.contains("enter confirm"), "{content}");

        let mut m = ModalStack::default();
        m.push(Modal::Confirm {
            title: "Unsaved changes".into(),
            body: "Save before closing?".into(),
            choices: vec![('s', "Save".into(), vec![])],
        });
        let content = draw_modal(&mut m);
        assert!(!content.contains("esc cancel"), "{content}");

        let mut m = ModalStack::default();
        m.push(Modal::MultiPrompt {
            title: "New option".into(),
            fields: vec![PromptField::text("key", "Key", "")],
            focus: 0,
            kind: PromptKind::NewOptionInline {
                owner: "host".into(),
            },
        });
        let content = draw_modal(&mut m);
        assert!(!content.contains("tab switch"), "{content}");
        assert!(!content.contains("esc cancel"), "{content}");
    }

    #[test]
    fn secret_prompt_keeps_the_reveal_hint_only() {
        // ctrl+r reveal is not discoverable, so that hint alone survives.
        let mut m = ModalStack::default();
        m.push(Modal::Prompt {
            title: "Secret".into(),
            input: LineInput::new(""),
            kind: PromptKind::SecretValue {
                name: "token".into(),
                env: "dev".into(),
            },
            revealed: false,
        });
        let content = draw_modal(&mut m);
        assert!(content.contains("ctrl+r reveal"), "{content}");
        assert!(!content.contains("esc cancel"), "{content}");
        assert!(!content.contains("enter confirm"), "{content}");
    }

    #[test]
    fn draw_renders_title_and_body() {
        let mut m = ModalStack::default();
        m.push(Modal::Message {
            title: "About".into(),
            body: "hello world".into(),
        });
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                m.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    None,
                    &keymap,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
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
        let keymap = crate::keys::Keymap::default_bindings();
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
                m.draw(
                    f,
                    area,
                    &theme,
                    &mut hits,
                    None,
                    &keymap,
                    test_anims(),
                    std::time::Instant::now(),
                )
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
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                m.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    None,
                    &keymap,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        let cancel = hits.rect_of(&crate::hit::Hit::ConfirmChoice('n')).unwrap();
        assert_eq!(
            buffer[(cancel.x, cancel.y + 2)].symbol(),
            "\u{2581}",
            "the Cancel button's bottom row must be its thin bevel edge"
        );
        assert_eq!(
            buffer[(cancel.x + 1, cancel.y + 1)].bg,
            theme.control,
            "Cancel is painted with the Secondary (control) face"
        );

        let confirm = hits.rect_of(&crate::hit::Hit::ConfirmChoice('y')).unwrap();
        assert_eq!(
            buffer[(confirm.x, confirm.y + 2)].symbol(),
            "\u{2581}",
            "the confirm button's bottom row must be its thin bevel edge"
        );
    }

    #[test]
    fn palette_row_matches_the_selected_row_and_accent_bar() {
        let mut m = ModalStack::default();
        m.push(Modal::Palette(
            crate::components::palette::PaletteState::new(&crate::usage::UsageStore::default(), 0),
        ));
        let theme = Theme::dark();
        let keymap = crate::keys::Keymap::default_bindings();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                m.draw(
                    f,
                    f.area(),
                    &theme,
                    &mut hits,
                    None,
                    &keymap,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row0 = hits.rect_of(&crate::hit::Hit::PaletteRow(0)).unwrap();
        assert_eq!(
            buffer[(row0.x, row0.y)].bg,
            theme.selection,
            "the selected row's dense fill must be theme.selection"
        );
        assert_eq!(
            buffer[(row0.x, row0.y)].symbol(),
            "\u{258c}",
            "the selected row must carry the dense accent bar in its first column"
        );
        assert_eq!(buffer[(row0.x, row0.y)].fg, theme.accent);
        // The accent bar spans both content lines of the two-line option.
        assert_eq!(
            buffer[(row0.x, row0.y + 1)].symbol(),
            "\u{258c}",
            "the accent bar must also cover the description line"
        );
        assert_eq!(buffer[(row0.x, row0.y + 1)].fg, theme.accent);
    }

    #[test]
    fn slugify_lowercases_and_maps_spaces() {
        assert_eq!(slugify("My Svc"), "my-svc");
        assert_eq!(slugify("Weird!! Na@me_1"), "weird-name_1");
    }

    fn dropdown_items() -> Vec<MenuItem> {
        vec![
            MenuItem::new("GET", Action::Render),
            MenuItem::new("POST", Action::Render),
            MenuItem::new("PUT", Action::Render),
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
            .draw(|f| {
                draw_dropdown(
                    f,
                    screen,
                    &Theme::dark(),
                    &mut hits,
                    None,
                    &state,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
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
            .draw(|f| {
                draw_dropdown(
                    f,
                    screen,
                    &Theme::dark(),
                    &mut hits,
                    None,
                    &state,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
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
                    test_anims(),
                    std::time::Instant::now(),
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
            theme.selection,
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

    #[test]
    fn dropdown_draws_an_accent_ring_around_the_settled_popup() {
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
                    None,
                    &state,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
            .unwrap();
        let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let buffer = terminal.backend().buffer();
        // Top edge stroke, excluding the corner column.
        let top_cell = &buffer[(body.x + 1, body.y)];
        assert_eq!(top_cell.symbol(), "─");
        assert_eq!(top_cell.fg, theme.accent);
        // Left edge stroke, excluding the corner row.
        let left_cell = &buffer[(body.x, body.y + 1)];
        assert_eq!(left_cell.symbol(), "│");
        assert_eq!(left_cell.fg, theme.accent);
        // Top-left corner: a square box-drawing corner glyph.
        let corner = &buffer[(body.x, body.y)];
        assert_eq!(corner.symbol(), "┌");
        assert_eq!(corner.fg, theme.accent);
    }

    #[test]
    fn dropdown_rows_are_dense_single_line_pitch_on_consecutive_rows() {
        // A window of N rows sits on consecutive screen lines (1-line
        // pitch), painted with `ListRow`.
        let screen = Rect::new(0, 0, 80, 24);
        let items: Vec<MenuItem> = (0..5)
            .map(|i| MenuItem::new(format!("Item {i}"), Action::Render))
            .collect();
        let state = DropdownState {
            anchor: Rect::new(10, 5, 8, 1),
            items,
            selected: 0,
            current: None,
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
                    None,
                    &state,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
            .unwrap();
        let rows: Vec<Rect> = (0..5)
            .map(|i| hits.rect_of(&crate::hit::Hit::DropdownRow(i)).unwrap())
            .collect();
        for w in rows.windows(2) {
            assert_eq!(
                w[1].y,
                w[0].y + 1,
                "rows sit on a dense, consecutive 1-line pitch"
            );
        }
    }

    #[test]
    fn dropdown_selected_row_has_no_left_accent_bar() {
        let screen = Rect::new(0, 0, 80, 24);
        let state = DropdownState {
            anchor: Rect::new(10, 5, 8, 1),
            items: dropdown_items(),
            selected: 0,
            current: None,
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
                    None,
                    &state,
                    test_anims(),
                    std::time::Instant::now(),
                )
            })
            .unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::DropdownRow(0)).unwrap();
        let buffer = terminal.backend().buffer();
        let leading = &buffer[(row0.x, row0.y)];
        assert_ne!(
            leading.symbol(),
            "▌",
            "menus don't keep the list-row left accent bar"
        );
        assert_eq!(leading.bg, theme.selection);
    }

    #[test]
    fn dropdown_open_settle_grows_from_the_top_and_snaps_open_when_settled() {
        // At t=0 the popup shows nothing yet (frac_vspan's negligible-edge
        // skip leaves the page underneath alone); at t=1 (this module's
        // `test_anims()` default) it's fully drawn, ring included.
        let screen = Rect::new(0, 0, 80, 24);
        let state = DropdownState {
            anchor: Rect::new(10, 5, 8, 1),
            items: dropdown_items(),
            selected: 0,
            current: Some(0),
        };
        let theme = Theme::dark();

        let mut anims = crate::anim::Anims::new(true);
        let now = std::time::Instant::now();
        anims.snap(crate::anim::AnimKey::DropdownOpen, 0.0);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                crate::paint::fill(f.buffer_mut(), screen, theme.page);
                draw_dropdown(f, screen, &theme, &mut hits, None, &state, &anims, now)
            })
            .unwrap();
        let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(body.x + 1, body.y + 1)].bg,
            theme.page,
            "t=0: nothing painted yet, the page underneath still shows"
        );

        anims.snap(crate::anim::AnimKey::DropdownOpen, 1.0);
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                crate::paint::fill(f.buffer_mut(), screen, theme.page);
                draw_dropdown(f, screen, &theme, &mut hits, None, &state, &anims, now)
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(body.x + 1, body.y + 1)].bg,
            theme.panel,
            "t=1: fully settled, panel fill covers the whole popup"
        );
    }
}
