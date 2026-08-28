//! The Variable Manager screen (spec §3.4): a master-detail layout —
//! a top bar (environment switcher + the two "new" buttons), a fixed-width
//! left list of every declared variable and selector, and a detail pane for
//! whatever the list has selected.
//!
//! Task 14 built the skeleton: the top bar, the left list and its
//! selection model, and the structural ops the list's own commands
//! dispatch. Task 15 fills the right pane's variable face — [`VarFormState`]
//! and [`draw_var_form`] — description/default/env-value fields edited in
//! place exactly like Task 8's tables, plus the secret toggle, rename/delete
//! buttons and the promote/demote button where the legacy `p`/`P`
//! preconditions hold. The selector face ([`OptionGridState`]) is still a
//! placeholder for Task 16.

use crate::action::Action;
use crate::components::line_input::LineInput;
use crate::hit::{Hit, HitMap, ScrollbarSpec};
use crate::layout::PaneId;
use crate::paint::{
    BUTTON_HEIGHT, Button, ButtonKind, ControlState, FIELD_HEIGHT, ListRow, RowHighlight,
    TextField, button_min_width, fill, text,
};
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::HttpRequest;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;

/// A committed value edit, dispatched as `Action::VarEdit` and applied by
/// `App` (spec §5: every commit writes atomically and immediately to
/// whichever file owns it; a write failure toasts and leaves the field it
/// came from untouched rather than losing the typed text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarEditOp {
    /// A simple (non-secret) variable's flat value in
    /// `env` — `varedit::set_env_value` via `ctx.edit_env`.
    SetEnvValue {
        env: String,
        name: String,
        value: String,
    },
    /// A variable's shared `default` in `variables.toml` —
    /// `varedit::upsert_var` via `ctx.edit_variables`.
    SetDefault { name: String, value: String },
    /// A variable's or selector's shared `description` in `variables.toml`.
    SetDescription { owner: String, value: String },
    /// A secret variable's value in `env` — `ctx.set_secret`, which writes
    /// `.local/secrets.toml` (never `variables.toml`/the env file).
    SetSecretValue {
        env: String,
        name: String,
        value: String,
    },
    /// One field of one option: the edit lands in `env`'s
    /// `[options.<selector>.<option>]` table (`varedit::upsert_option`), the
    /// only place an option's values live (spec §3.1). Every selector grid
    /// cell but the name column commits through this.
    SetOptionValue {
        env: String,
        selector: String,
        option: String,
        field: String,
        value: String,
    },
    /// A request-scoped `[variables]` option's value on the open request —
    /// mutates `Editor::variables` directly and rides the editor's existing
    /// dirty/save path (no immediate write of its own).
    SetRequestVar { name: String, value: String },
    /// Records `option` as `selector`'s selection in `env` — `ctx.set_selection`
    /// (the selector grid's radio column; also the var picker's confirm and
    /// the ✓ action). Picking an option is what makes every one of the
    /// selector's fields resolve, together, to that record's values.
    SelectOption {
        env: String,
        selector: String,
        option: String,
    },
}

/// A structural mutation dispatched by the Variable Manager: unlike
/// [`VarEditOp`] (one value), these add/remove/rename/reshape declarations
/// and options. Each applies through `ctx.edit_variables`/`edit_env` in
/// `App::apply_var_struct`.
///
/// The declaration ops (`NewVar`..`Demote`) write `variables.toml`; the
/// option ops (`NewOption`..`DuplicateOption`) write one environment file
/// each — options belong to exactly one environment (spec §3.1), so every
/// one of them names the `env` it targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarStructOp {
    /// A bare new variable declaration (`+ Variable` / `n`).
    NewVar {
        name: String,
        description: Option<String>,
    },
    /// A new selector and its field list (`+ Group` / `g`). Create-or-update:
    /// `varedit::upsert_selector` is the same verb either way.
    NewSelector { name: String, fields: Vec<String> },
    /// Rename a variable or a selector. A selector renames in both halves at
    /// once (`varedit::rename_selector` + `rename_selector_options` per
    /// environment): an environment's `[options.<old>]` table names a
    /// selector the renamed model no longer declares, so neither half is
    /// valid without the other. Any recorded selection follows the name.
    Rename { from: String, to: String },
    /// Delete a variable or a selector. A selector cascades: every environment's
    /// `[options.<name>]` subtree goes with the declaration, and every
    /// environment's selection for it is cleared.
    Delete { name: String },
    /// Flip a variable's `secret` flag (spec §3's two transitions).
    ToggleSecret { name: String },
    /// Replace a selector's field list.
    SetFields {
        selector: String,
        fields: Vec<String>,
    },
    /// Promote a request-scoped variable to the project (spec §4).
    Promote {
        name: String,
        target: postui_core::varedit::PromoteTarget,
    },
    /// Demote a project variable into the open request (spec §4).
    Demote { name: String },
    /// A new option of `selector` in `env` (`varedit::upsert_option`).
    NewOption {
        env: String,
        selector: String,
        name: String,
        description: Option<String>,
        values: IndexMap<String, String>,
    },
    /// Rename one option of `selector` within `env`.
    RenameOption {
        env: String,
        selector: String,
        from: String,
        to: String,
    },
    /// Delete one option of `selector` from `env`, clearing any selection that
    /// named it.
    DeleteOption {
        env: String,
        selector: String,
        name: String,
    },
    /// Copy one option of `selector` in `env` to a fresh name — `"<name> copy"`,
    /// then `"<name> copy-2"`, … on collision.
    DuplicateOption {
        env: String,
        selector: String,
        name: String,
    },
}

/// What the detail pane is showing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum VmDetail {
    #[default]
    None,
    Var(String),
    Group(String),
}

impl VmDetail {
    /// The declared name the pane has open, whichever face it is showing —
    /// what the shared `[Rename]`/`[Delete]` buttons act on.
    pub fn name(&self) -> Option<&str> {
        match self {
            VmDetail::Var(n) | VmDetail::Group(n) => Some(n),
            VmDetail::None => None,
        }
    }
}

/// One row of the left list, rebuilt from `&ProjectContext` at the top of
/// every `draw` (and after every structural write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmRow {
    SectionVars,
    Var(String),
    SectionGroups,
    Group(String),
}

impl VmRow {
    /// Whether the keyboard cursor stops here — section headers are labels,
    /// not selections.
    fn is_stop(&self) -> bool {
        matches!(self, VmRow::Var(_) | VmRow::Group(_))
    }

    /// The declared name this row addresses, if any.
    pub fn name(&self) -> Option<&str> {
        match self {
            VmRow::Var(n) | VmRow::Group(n) => Some(n),
            _ => None,
        }
    }
}

/// Which field of the variable form is under edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmField {
    Description,
    Default,
    EnvValue,
}

/// The detail pane's variable form (spec §3.4). Editing is always in place
/// (Task 8's model, exactly): a click seeds `editing` with the clicked
/// field's current text and a caret at the end; another click, `Enter`, or
/// `Esc` all leave it — the caller (`App`) commits or reverts, since a
/// commit writes through `ctx.edit_variables`/`edit_env` and only `App` can
/// reach those.
#[derive(Debug, Default)]
pub struct VarFormState {
    /// The field under edit and its live `LineInput`, or `None` when
    /// nothing in the form owns the keyboard — consulted by the
    /// command-key gate in [`VarManager::handle_key`].
    pub editing: Option<(VmField, LineInput)>,
    /// Whether the env-value field currently shows a secret's plaintext
    /// instead of `\u{25cf}` dots. Reset to `false` whenever the detail
    /// selection changes (mirrors Task 10's per-context reset precedent) —
    /// a reveal never survives moving on to a different variable.
    pub revealed: bool,
}

/// The text a field shows when nothing is being typed into it — the seed a
/// click begins editing from, and what a resting (non-edited) field
/// displays. `EnvValue` reads only what the environment actually *stores*
/// (the env file's flat value, or the secrets store for a secret) — never
/// the resolved fallback, which made a bare default masquerade as an env
/// value in this field. Unset reads empty, rendered "(not set)". With no
/// active environment it falls back to the declaration default, mirroring
/// [`var_edit_op_for`]'s write-target fallback.
fn field_seed_text(ctx: &ProjectContext, name: &str, field: VmField) -> String {
    let decl = ctx.model.vars.get(name);
    match field {
        VmField::Description => decl.and_then(|d| d.description.clone()).unwrap_or_default(),
        VmField::Default => decl.and_then(|d| d.default.clone()).unwrap_or_default(),
        VmField::EnvValue => match &ctx.active_env {
            Some(env) => {
                if decl.is_some_and(|d| d.secret) {
                    ctx.secrets
                        .get(env)
                        .and_then(|m| m.get(name))
                        .cloned()
                        .unwrap_or_default()
                } else {
                    ctx.env_data.values.get(name).cloned().unwrap_or_default()
                }
            }
            None => decl.and_then(|d| d.default.clone()).unwrap_or_default(),
        },
    }
}

/// The [`VarEditOp`] a committed `field` edit writes (spec §5's "every
/// commit writes atomically and immediately"). `EnvValue` targets the
/// active environment (masked through `SetSecretValue` for a secret var,
/// `.local/secrets.toml` rather than the env file); with no active
/// environment there is nowhere to write an override, so it targets the
/// declaration default instead — the same field a secret can't have, so
/// that edit fails and toasts rather than silently landing somewhere
/// unexpected (spec's general write-failure rule: the text stays put).
pub fn var_edit_op_for(
    ctx: &ProjectContext,
    name: &str,
    field: VmField,
    value: String,
) -> VarEditOp {
    match field {
        VmField::Description => VarEditOp::SetDescription {
            owner: name.to_string(),
            value,
        },
        VmField::Default => VarEditOp::SetDefault {
            name: name.to_string(),
            value,
        },
        VmField::EnvValue => match &ctx.active_env {
            Some(env) => {
                let secret = ctx.model.vars.get(name).is_some_and(|d| d.secret);
                if secret {
                    VarEditOp::SetSecretValue {
                        env: env.clone(),
                        name: name.to_string(),
                        value,
                    }
                } else {
                    VarEditOp::SetEnvValue {
                        env: env.clone(),
                        name: name.to_string(),
                        value,
                    }
                }
            }
            None => VarEditOp::SetDefault {
                name: name.to_string(),
                value,
            },
        },
    }
}

/// The promote/demote button's label and click action for `name` right now
/// — `None` when neither applies, which is what hides the button entirely.
/// Mirrors the legacy `p`/`P` preconditions exactly: a secret's value can
/// never move through either (its plaintext would otherwise land in a
/// git-tracked request file, or promoting a plain value onto it would make
/// the declaration invalid — `promote_var`'s own conflict cases, moot here
/// since a name reachable as `VmDetail::Var` is already a simple
/// declaration, never a selector name or field). Which direction applies
/// depends on whether the open request already overrides `name` in its own
/// `[variables]` — if so, "Promote" offers to move that override up into
/// the project (`apply_promote` requires exactly this); otherwise, with a
/// request open, "Demote" offers to copy the resolved project value down
/// into it.
pub fn promote_demote_action(
    ctx: &ProjectContext,
    open_request: Option<&HttpRequest>,
    name: &str,
) -> Option<(&'static str, Action)> {
    if ctx.model.vars.get(name).is_some_and(|d| d.secret) {
        return None;
    }
    let req = open_request?;
    if req.variables.contains_key(name) {
        Some((
            "Promote",
            Action::PromptPromoteVar {
                name: name.to_string(),
            },
        ))
    } else {
        Some((
            "Demote",
            Action::ConfirmDemoteVar {
                name: name.to_string(),
            },
        ))
    }
}

/// One grid cell's in-progress edit (Task 8's `CellEdit`, for the selector
/// grid): which cell, the live buffer, and the text it started from so
/// `Esc` can put it back.
#[derive(Debug)]
pub struct GridEdit {
    /// Index into the selector's options — or `options.len()`, the ghost row
    /// that becomes a real option the moment its name cell commits
    /// non-empty.
    pub row: usize,
    /// `0` is the option-name column; `n` is the selector's `n-1`th field.
    pub col: usize,
    pub input: LineInput,
    /// The cell's pre-edit text, for `Esc`-revert.
    pub original: String,
}

/// The detail pane's options grid (spec §3.4): one row per option of the
/// selected selector in the active environment, one column per selector field,
/// plus the radio column that says which option the environment has
/// selected. Editing is Task 8's in-place model, exactly — see
/// [`VarFormState`] for the same contract on the variable form.
#[derive(Debug, Default)]
pub struct OptionGridState {
    /// The keyboard cursor's `(row, col)`; `space`/`o`/`m` act here.
    pub cursor: (usize, usize),
    /// The cell under edit, or `None` when nothing in the grid owns the
    /// keyboard — consulted by the command-key gate in
    /// [`VarManager::handle_key`].
    pub editing: Option<GridEdit>,
    /// First visible option row.
    pub scroll: usize,
}

/// Which of the screen's two keyboard focus stops has the keyboard: the
/// left list, or the selector pane's options grid. Each keeps its own cursor,
/// so stepping out of the grid and back lands where it was. See
/// [`VarManager::handle_key`] for the keys that move between them.
///
/// The variable form has no stop of its own: its fields are reached by
/// clicking (and, once one is live, `App` routes every key to it), so
/// there is no second cursor for the keyboard to be lost in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VmFocus {
    #[default]
    List,
    Grid,
}

/// The full-frame Variable Manager screen.
#[derive(Debug, Default)]
pub struct VarManager {
    /// What the detail pane shows — set by [`VarManager::select_row`],
    /// whichever gesture (click or arrow key) got there.
    pub detail: VmDetail,
    /// Rebuilt from `&ProjectContext` at the top of every `draw`.
    pub left_rows: Vec<VmRow>,
    /// Index into `left_rows`.
    pub left_cursor: usize,
    /// First visible row of `left_rows`.
    pub left_scroll: usize,
    pub form: VarFormState,
    pub grid: OptionGridState,
    /// Which stop has the keyboard (see [`VmFocus`]).
    pub focus: VmFocus,
    /// The selector grid's option-row region as of the last `draw` — the wheel
    /// scrolls the grid when the pointer is inside it, the left list
    /// otherwise. Empty when no grid is on screen.
    grid_area: Rect,
    /// How many option rows that region showed, for the wheel's clamp.
    grid_visible: usize,
    /// Set by a keyboard move so the next `draw` snaps `left_scroll` to
    /// keep `left_cursor` visible; a wheel/drag gesture clears it (those
    /// place the viewport explicitly). Mirrors `Sidebar::ensure_visible`.
    ensure_visible: bool,
    /// How many rows the left list showed as of the last `draw` — the
    /// scrollbar's viewport, and the snap math's page size.
    visible_rows: usize,
}

/// The top bar's height, matching the header/footer's 3-row painted rhythm
/// (and exactly one painted button tall).
pub const TITLE_HEIGHT: u16 = BUTTON_HEIGHT;
/// The left list's fixed width (spec §3.4's mock).
pub const LEFT_W: u16 = 28;

const GLYPH_GROUP: &str = "\u{25b6}"; // ▶
const GLYPH_LOCK: &str = "\u{1f512}"; // 🔒
const GLYPH_UNRESOLVED: &str = "\u{25cf}"; // ●
const GLYPH_CARET: &str = "\u{25be}"; // ▾
const GLYPH_RADIO_ON: &str = "\u{25c9}"; // ◉
const GLYPH_RADIO_OFF: &str = "\u{25cb}"; // ○

/// The hint the selector pane shows instead of a grid when no environment is
/// active: options belong to one environment each (spec §3.1), so there is
/// nowhere for them to live until one is picked.
pub const NO_ENV_HINT: &str = "options live in environments \u{2014} pick or create one";

/// The ghost row's resting label.
const GHOST_LABEL: &str = "+ option";

/// Width of the grid's radio column (glyph + one column of gutter).
const RADIO_W: u16 = 3;

/// Where each grid column starts and how wide it is: `x[0]`/`w[0]` is the
/// option-name column, `x[n]` the selector's `n-1`th field. Columns that would
/// start past the pane's right edge are dropped, so a narrow pane simply
/// shows fewer columns rather than painting over its own border.
struct GridCols {
    radio_x: u16,
    x: Vec<u16>,
    w: Vec<u16>,
}

fn grid_columns(x0: u16, width: u16, ncols: usize) -> GridCols {
    let mut cols = GridCols {
        radio_x: x0,
        x: Vec::new(),
        w: Vec::new(),
    };
    let right = x0 + width;
    let avail = width.saturating_sub(RADIO_W);
    let n = ncols.max(1) as u16;
    let each = (avail / n).max(6);
    let mut cx = x0 + RADIO_W;
    for _ in 0..n {
        if cx + 4 > right {
            break;
        }
        let w = each.saturating_sub(1).min(right - cx);
        cols.x.push(cx);
        cols.w.push(w);
        cx += each;
    }
    cols
}

/// One selector's options in `ctx`'s active environment, as `(name, values)`
/// pairs in file order. Empty when the selector has no options here (or there
/// is no active environment).
fn entry_rows(ctx: &ProjectContext, selector: &str) -> Vec<(String, IndexMap<String, String>)> {
    if ctx.active_env.is_none() {
        return Vec::new();
    }
    postui_core::varmodel::selector_options(&ctx.env_data, selector)
        .map(|options| {
            options
                .iter()
                .map(|(name, e)| (name.clone(), e.values.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The text grid cell `(row, col)` currently shows — the seed a click
/// starts editing from. Empty for the ghost row and for a field an option
/// doesn't set.
fn grid_cell_text(ctx: &ProjectContext, selector: &str, row: usize, col: usize) -> String {
    let rows = entry_rows(ctx, selector);
    let Some((name, values)) = rows.get(row) else {
        return String::new();
    };
    if col == 0 {
        return name.clone();
    }
    let fields = group_fields(ctx, selector);
    fields
        .get(col - 1)
        .and_then(|f| values.get(f))
        .cloned()
        .unwrap_or_default()
}

/// `selector`'s declared field list (empty for an undeclared name).
fn group_fields(ctx: &ProjectContext, selector: &str) -> Vec<String> {
    ctx.model
        .selectors
        .get(selector)
        .map(|g| g.fields.clone())
        .unwrap_or_default()
}

/// The left list's rows: the VARIABLES section (every declared variable
/// that isn't a selector field — fields live inside their selector's options),
/// then the SELECTORS section.
pub fn build_left_rows(ctx: &ProjectContext) -> Vec<VmRow> {
    let mut rows = vec![VmRow::SectionVars];

    let group_fields: std::collections::HashSet<&str> = ctx
        .model
        .selectors
        .values()
        .flat_map(|g| g.fields.iter().map(String::as_str))
        .collect();
    for name in ctx.model.vars.keys() {
        if !group_fields.contains(name.as_str()) {
            rows.push(VmRow::Var(name.clone()));
        }
    }

    rows.push(VmRow::SectionGroups);
    for name in ctx.model.selectors.keys() {
        rows.push(VmRow::Group(name.clone()));
    }
    rows
}

/// A selector's current selection in the active environment, as the left list
/// shows it inline: the selected option's name, or `None` when the selector has
/// no (or a stale) selection here.
fn active_selection(ctx: &ProjectContext, selector: &str) -> Option<String> {
    let env = ctx.active_env.as_deref()?;
    let key = ctx.selections_for(env).get(selector)?;
    let options = postui_core::varmodel::selector_options(&ctx.env_data, selector)?;
    options.contains_key(key).then(|| key.clone())
}

/// Whether `name` currently resolves to a value in the active environment —
/// false for a selector field awaiting a selection, a secret with no value,
/// and a variable with neither default nor env value. Drives the left
/// list's red dot.
fn is_unresolved(ctx: &ProjectContext, name: &str) -> bool {
    !ctx.resolved.values.contains_key(name)
}

impl VarManager {
    /// Points the detail pane at `left_rows[i]` and moves the cursor there.
    /// A section header is a label, not a selection: the cursor still moves
    /// (a click landed there) but the detail pane keeps what it had.
    ///
    /// Picking a row is the left list acting, so the keyboard comes back to
    /// it — a grid the pane may no longer even be showing must never keep
    /// the arrows.
    pub fn select_row(&mut self, i: usize) {
        if i >= self.left_rows.len() {
            return;
        }
        self.left_cursor = i;
        self.focus = VmFocus::List;
        match &self.left_rows[i] {
            VmRow::Var(name) => {
                let target = VmDetail::Var(name.clone());
                if self.detail != target {
                    // A fresh selection starts with nothing mid-edit and no
                    // reveal carried over from whatever was open before.
                    self.form = VarFormState::default();
                }
                self.detail = target;
            }
            VmRow::Group(name) => {
                let target = VmDetail::Group(name.clone());
                if self.detail != target {
                    self.grid = OptionGridState::default();
                }
                self.detail = target;
            }
            VmRow::SectionVars | VmRow::SectionGroups => {}
        }
    }

    /// Click option point for a form field (`Hit::VmFormField`): seeds
    /// `field` with its current text and a caret at the end. A second click
    /// on the field already under edit is the caller's job to no-op (it
    /// must not restart the edit and lose what was typed) — this always
    /// (re)starts one, so the caller checks first. A no-op with nothing
    /// selected (`self.detail` isn't `Var`).
    pub fn start_field_edit(&mut self, ctx: &ProjectContext, field: VmField) {
        let VmDetail::Var(name) = &self.detail else {
            return;
        };
        let seed = field_seed_text(ctx, name, field);
        self.form.editing = Some((field, LineInput::new(&seed)));
    }

    /// Click option point for a grid cell (`Hit::VmEntryCell`): seeds
    /// `(row, col)` with its current text and a caret at the end, and puts
    /// the grid cursor there. Like [`Self::start_field_edit`], a second
    /// click on the cell already under edit is the caller's job to no-op.
    /// A no-op with no selector selected, and on a ghost-row cell other than
    /// the name column (there is no option yet for a value to belong to —
    /// the click is redirected to the name cell by the caller).
    pub fn start_cell_edit(&mut self, ctx: &ProjectContext, row: usize, col: usize) {
        let VmDetail::Group(selector) = &self.detail else {
            return;
        };
        let selector = selector.clone();
        let rows = entry_rows(ctx, &selector);
        let row = row.min(rows.len());
        let col = if row == rows.len() { 0 } else { col };
        let original = grid_cell_text(ctx, &selector, row, col);
        // Typing into a cell is the grid holding the keyboard, however the
        // edit was started — so `Esc` out of it lands in the grid, not back
        // in the left list.
        self.focus = VmFocus::Grid;
        self.grid.cursor = (row, col);
        self.grid.editing = Some(GridEdit {
            row,
            col,
            input: LineInput::new(&original),
            original,
        });
    }

    /// The option `row` names, or `None` for the ghost row / no selector.
    pub fn entry_at(&self, ctx: &ProjectContext, row: usize) -> Option<String> {
        let VmDetail::Group(selector) = &self.detail else {
            return None;
        };
        entry_rows(ctx, selector).get(row).map(|(n, _)| n.clone())
    }

    /// The row the commands act on — the left list's current selection.
    fn selected_row(&self) -> Option<&VmRow> {
        self.left_rows.get(self.left_cursor).filter(|r| r.is_stop())
    }

    /// Rebuilds `left_rows` from `ctx` and repairs the selection after a
    /// structural write: the cursor clamps into range, and a detail pane
    /// pointing at a name that no longer exists empties.
    pub fn sync(&mut self, ctx: &ProjectContext) {
        self.left_rows = build_left_rows(ctx);
        if self.left_cursor >= self.left_rows.len() {
            self.left_cursor = self.left_rows.len().saturating_sub(1);
        }
        let gone = match &self.detail {
            VmDetail::None => false,
            VmDetail::Var(name) => !ctx.model.vars.contains_key(name),
            VmDetail::Group(name) => !ctx.model.selectors.contains_key(name),
        };
        if gone {
            self.detail = VmDetail::None;
            self.form = VarFormState::default();
            self.grid = OptionGridState::default();
            self.focus = VmFocus::List;
        }
        self.ensure_visible = true;
    }

    /// Moves the cursor one selectable row in `dir` (`-1`/`1`), skipping
    /// section headers, and opens whatever it lands on.
    fn move_cursor(&mut self, dir: i32) {
        let mut i = self.left_cursor as i32;
        loop {
            i += dir;
            if i < 0 || i as usize >= self.left_rows.len() {
                return;
            }
            if self.left_rows[i as usize].is_stop() {
                self.select_row(i as usize);
                self.ensure_visible = true;
                return;
            }
        }
    }

    /// Handles a key while the Manager screen is open. `App::handle_key`
    /// routes every key here once an open modal and a modified global
    /// shortcut (e.g. ctrl+p for the palette) have had first refusal, and
    /// swallows anything this returns `None` for rather than falling
    /// through to the global keymap — so, for instance, plain `q` does not
    /// quit the app from this screen.
    ///
    /// The single-letter commands all target the left list's selection.
    /// They are ignored while a detail-pane cell edit owns the keyboard
    /// (spec §11: a command key that collides with typing yields to the
    /// text input) — `esc` and the arrows still work.
    ///
    /// This is never reached while a form field is under edit — `App`
    /// intercepts Esc (revert)/Enter (commit)/plain typing itself first,
    /// since a commit needs write access to the project that this method's
    /// `&ProjectContext` (shared, not mutable) can't give it. `form.editing`
    /// is still consulted below, for the single-letter command gate.
    ///
    /// # Keyboard focus (spec §4's keyboard parity)
    ///
    /// The screen has two keyboard focus stops, [`VmFocus`]: the left list
    /// and — when the detail pane is showing a selector — its options grid.
    /// The left list owns the keyboard by default; `Right`/`Tab` step into
    /// the grid, and `Left` from its first column, `Esc`, or `BackTab` step
    /// back out. Each keeps its own cursor, so returning to the list lands
    /// where it was. Inside the grid the arrows move the cell cursor,
    /// `Enter` starts editing the focused cell, `space` selects the cursor
    /// row's option, and `e`/`d` rename/delete *that option* rather than the
    /// selector (the list's own `e`/`d` still target the declaration — which
    /// stop has focus is what tells them apart).
    pub fn handle_key(&mut self, ev: KeyEvent, ctx: &ProjectContext) -> Option<Action> {
        // The grid is a focus stop of its own: while it has the keyboard,
        // the arrows drive its cell cursor rather than the left list, and
        // the commands act on the option under that cursor.
        if self.focus == VmFocus::Grid {
            if let VmDetail::Group(selector) = self.detail.clone() {
                return self.handle_grid_focus_key(ev, ctx, &selector);
            }
            // The pane stopped showing a selector under it (deleted, or the
            // selection moved): the grid is gone, so the focus goes home.
            self.focus = VmFocus::List;
        }
        match ev.code {
            KeyCode::Esc => return Some(Action::CloseScreen),
            KeyCode::Up => {
                self.move_cursor(-1);
                return None;
            }
            KeyCode::Down => {
                self.move_cursor(1);
                return None;
            }
            // Into the grid, when there is one to step into.
            KeyCode::Right | KeyCode::Tab => {
                if matches!(&self.detail, VmDetail::Group(g) if ctx.model.selectors.contains_key(g))
                    && ctx.active_env.is_some()
                    && self.form.editing.is_none()
                    && self.grid.editing.is_none()
                {
                    self.focus = VmFocus::Grid;
                }
                return None;
            }
            _ => {}
        }
        if self.form.editing.is_some() || self.grid.editing.is_some() {
            return None;
        }
        // The selector grid's own commands, on the selector the detail pane has
        // open (spec §3.4's "old keys"). They come first so `o`/`m`/`space`
        // never fall through to a left-list command that would act on a
        // different row than the grid the user is looking at. Unlike the
        // focused-grid keys above, these work straight from the left list —
        // `o` and `m` need no cursor, and `space` uses the grid's own.
        if let VmDetail::Group(selector) = self.detail.clone() {
            match ev.code {
                KeyCode::Char('o') => {
                    let row = entry_rows(ctx, &selector).len();
                    self.focus = VmFocus::Grid;
                    self.start_cell_edit(ctx, row, 0);
                    return None;
                }
                KeyCode::Char('m') => return Some(Action::PromptGroupFields { selector }),
                KeyCode::Char(' ') => return self.select_entry_action(ctx, &selector),
                _ => {}
            }
        }
        match ev.code {
            KeyCode::Char('n') => Some(Action::PromptNewVar),
            KeyCode::Char('g') => Some(Action::PromptNewSelector),
            KeyCode::Char('e') | KeyCode::F(2) => self.rename_action(),
            KeyCode::Char('d') | KeyCode::Delete => Some(Action::ConfirmDeleteVar {
                name: self.selected_row()?.name()?.to_string(),
            }),
            KeyCode::Char('s') => match self.selected_row()? {
                VmRow::Var(name) if ctx.model.vars.contains_key(name) => {
                    Some(Action::ToggleSecretVar { name: name.clone() })
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Keys while the options grid is the focus stop (see
    /// [`Self::handle_key`]'s focus notes). The cursor moves over the whole
    /// grid, ghost row included — `Enter` there starts a new option, exactly
    /// like clicking it. `Esc`, `BackTab`, and `Left` from the first column
    /// hand the keyboard back to the left list rather than closing the
    /// screen; the screen's own `Esc` is then one more press away, which is
    /// the same "leave the inner thing first" rhythm the modal stack has.
    fn handle_grid_focus_key(
        &mut self,
        ev: KeyEvent,
        ctx: &ProjectContext,
        selector: &str,
    ) -> Option<Action> {
        // A live cell edit owns every key (`App` routes those before this
        // is reached); nothing here may act behind its back.
        if self.grid.editing.is_some() {
            return None;
        }
        let last_row = entry_rows(ctx, selector).len(); // == the ghost row
        let last_col = group_fields(ctx, selector).len(); // == 1 + fields - 1
        let (row, col) = &mut self.grid.cursor;
        *row = (*row).min(last_row);
        *col = (*col).min(last_col);
        match ev.code {
            KeyCode::Esc | KeyCode::BackTab => {
                self.focus = VmFocus::List;
                None
            }
            KeyCode::Up => {
                *row = row.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                *row = (*row + 1).min(last_row);
                None
            }
            KeyCode::Left => {
                if *col == 0 {
                    self.focus = VmFocus::List;
                } else {
                    *col -= 1;
                }
                None
            }
            KeyCode::Right | KeyCode::Tab => {
                *col = (*col + 1).min(last_col);
                None
            }
            KeyCode::Enter => {
                let (row, col) = self.grid.cursor;
                self.start_cell_edit(ctx, row, col);
                None
            }
            KeyCode::Char('o') => {
                self.start_cell_edit(ctx, last_row, 0);
                None
            }
            KeyCode::Char('m') => Some(Action::PromptGroupFields {
                selector: selector.to_string(),
            }),
            KeyCode::Char(' ') => self.select_entry_action(ctx, selector),
            // `e`/`d` here act on the option under the cursor — the left
            // list's own `e`/`d`, which target the declaration, are one
            // focus stop away.
            KeyCode::Char('e') | KeyCode::F(2) => Some(Action::PromptRenameEntry {
                env: ctx.active_env.clone()?,
                selector: selector.to_string(),
                from: self.entry_at(ctx, self.grid.cursor.0)?,
            }),
            KeyCode::Char('d') | KeyCode::Delete => Some(Action::ConfirmDeleteEntry {
                env: ctx.active_env.clone()?,
                selector: selector.to_string(),
                name: self.entry_at(ctx, self.grid.cursor.0)?,
            }),
            KeyCode::Char('n') => Some(Action::PromptNewVar),
            KeyCode::Char('g') => Some(Action::PromptNewSelector),
            _ => None,
        }
    }

    /// `space`: select the option the grid cursor is on for the active
    /// environment. `None` on the ghost row (nothing to select yet) and
    /// with no active environment.
    fn select_entry_action(&self, ctx: &ProjectContext, selector: &str) -> Option<Action> {
        Some(Action::VarEdit(VarEditOp::SelectOption {
            env: ctx.active_env.clone()?,
            selector: selector.to_string(),
            option: self.entry_at(ctx, self.grid.cursor.0)?,
        }))
    }

    /// `e`/`F2` and the context menu's "Rename…". Both a variable and a
    /// selector rename through the same prompt: `VarStructOp::Rename` picks
    /// the right pair of core verbs from what the name is declared as.
    fn rename_action(&self) -> Option<Action> {
        Some(Action::PromptRenameVar {
            from: self.selected_row()?.name()?.to_string(),
        })
    }

    /// The right-click menu for left-list row `i` (spec §3.4: Rename /
    /// Duplicate / Delete). `None` for a section header.
    pub fn context_menu(&self, i: usize) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::components::modal::MenuItem;
        let row = self.left_rows.get(i).filter(|r| r.is_stop())?;
        let name = row.name()?.to_string();
        let (rename, duplicate) = match row {
            VmRow::Var(_) => (
                MenuItem::new(
                    "Rename\u{2026}",
                    Action::PromptRenameVar { from: name.clone() },
                ),
                MenuItem::new("Duplicate", Action::DuplicateVar { name: name.clone() }),
            ),
            // Duplicate is shown disabled rather than hidden, so the menu
            // keeps its shape: a field belongs to exactly one selector
            // (`ModelError::FieldInTwoGroups`), so a copy carrying the same
            // field list can never be a valid model — there is nothing to
            // duplicate a selector *into* until fields can be renamed as part
            // of the copy.
            _ => (
                MenuItem::new(
                    "Rename\u{2026}",
                    Action::PromptRenameVar { from: name.clone() },
                ),
                MenuItem::disabled("Duplicate"),
            ),
        };
        Some(vec![
            rename,
            duplicate,
            MenuItem::new("Delete\u{2026}", Action::ConfirmDeleteVar { name }),
        ])
    }

    /// The right-click menu for option row `i` of the open selector (spec
    /// §3.4). `None` for the ghost row (nothing to act on yet) and when no
    /// environment is active (there are no options at all then).
    pub fn entry_context_menu(
        &self,
        ctx: &ProjectContext,
        i: usize,
    ) -> Option<Vec<crate::components::modal::MenuItem>> {
        use crate::components::modal::MenuItem;
        let VmDetail::Group(selector) = &self.detail else {
            return None;
        };
        let env = ctx.active_env.clone()?;
        let name = self.entry_at(ctx, i)?;
        let (selector, n) = (selector.clone(), name.clone());
        let decl = postui_core::varmodel::selector_options(&ctx.env_data, &selector)
            .and_then(|options| options.get(&n))?;
        Some(vec![
            MenuItem::new(
                "Edit\u{2026}",
                Action::OpenEditOptionPrompt {
                    owner: selector.clone(),
                    key: n.clone(),
                    description: decl.description.clone(),
                    values: decl.values.clone(),
                },
            ),
            MenuItem::new(
                "Duplicate option",
                Action::VarStruct(VarStructOp::DuplicateOption {
                    env: env.clone(),
                    selector: selector.clone(),
                    name: n.clone(),
                }),
            ),
            MenuItem::new(
                "Rename\u{2026}",
                Action::PromptRenameEntry {
                    env: env.clone(),
                    selector: selector.clone(),
                    from: n,
                },
            ),
            MenuItem::new(
                "Delete\u{2026}",
                Action::ConfirmDeleteEntry {
                    env,
                    selector,
                    name,
                },
            ),
        ])
    }

    /// Free (unsnapped) wheel scroll over the left list — mirrors
    /// `Sidebar::handle_scroll`: moves the viewport without touching the
    /// cursor, and cancels any pending snap so the gesture isn't overridden
    /// on the next draw.
    pub fn handle_scroll(&mut self, delta: i16) {
        self.set_scroll((self.left_scroll as i32 + delta as i32).max(0) as usize);
    }

    /// A wheel gesture at `(col, row)`: the selector grid when the pointer is
    /// over its option rows, the left list everywhere else on the screen.
    /// (The grid is the only other scrollable region the Manager draws, and
    /// it has no scrollbar of its own — a grid tall enough to overflow is
    /// reached by wheeling over it.)
    pub fn handle_scroll_at(&mut self, col: u16, row: u16, delta: i16) {
        if self.grid_area.width > 0 && self.grid_area.contains((col, row).into()) {
            self.grid.scroll = (self.grid.scroll as i32 + delta as i32).max(0) as usize;
            return;
        }
        self.handle_scroll(delta);
    }

    /// Places the left list's viewport (the scrollbar drag's option point).
    pub fn set_scroll(&mut self, offset: usize) {
        let max = self.left_rows.len().saturating_sub(1);
        self.left_scroll = offset.min(max);
        self.ensure_visible = false;
    }

    /// The left list's scroll state, as of the last draw. `None` before the
    /// first frame (the viewport height is a render-time fact). Counted in
    /// logical rows, not lines: the 2-line pitch scales every row the same.
    ///
    /// It borrows `PaneId::Sidebar`, the pane whose column the list stands
    /// in: no sidebar is drawn while this screen is up, so the app's
    /// existing wheel/drag routing for that pane is free, and `App`
    /// redirects it here for the duration (see `App::scrollbar_spec`).
    pub fn scrollbar_spec(&self) -> Option<ScrollbarSpec> {
        if self.visible_rows == 0 {
            return None;
        }
        Some(ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset: self.left_scroll,
            content: self.left_rows.len(),
            viewport: self.visible_rows,
        })
    }

    /// Number of 1-line rows that fit in a list `height` lines tall.
    /// Mirrors `Sidebar::visible_rows`.
    fn rows_that_fit(height: u16) -> usize {
        height as usize
    }

    /// Paints the screen: the top bar (environment switcher + `+ Variable` /
    /// `+ Group`), the fixed-width left list, and the detail pane —
    /// [`draw_var_form`] for a selected variable, a placeholder otherwise
    /// (a selected selector is Task 16's job).
    ///
    /// `open_request` is the request-scope half of the variable form: the
    /// promote/demote button's precondition (whether the open request
    /// already overrides the selected name in its own `[variables]`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        self.left_rows = build_left_rows(ctx);
        if self.left_cursor >= self.left_rows.len() {
            self.left_cursor = self.left_rows.len().saturating_sub(1);
        }
        if area.width == 0 || area.height == 0 {
            self.visible_rows = 0;
            return;
        }

        let bar = Rect {
            height: TITLE_HEIGHT.min(area.height),
            ..area
        };
        let body = Rect {
            y: area.y + bar.height,
            height: area.height - bar.height,
            ..area
        };
        self.draw_top_bar(frame, bar, theme, ctx, hits, hovered);

        let left = Rect {
            width: LEFT_W.min(body.width),
            ..body
        };
        let right = Rect {
            x: body.x + left.width,
            width: body.width - left.width,
            ..body
        };
        self.draw_left(frame, left, theme, ctx, hits, hovered);
        self.grid_area = Rect::default();
        self.grid_visible = 0;
        match self.detail.clone() {
            VmDetail::Var(name) if ctx.model.vars.contains_key(&name) => {
                let buf = frame.buffer_mut();
                fill(buf, right, theme.page);
                self.draw_var_form(buf, right, theme, ctx, open_request, hits, hovered, &name);
            }
            VmDetail::Group(name) if ctx.model.selectors.contains_key(&name) => {
                let buf = frame.buffer_mut();
                fill(buf, right, theme.page);
                self.draw_entry_grid(buf, right, theme, ctx, hits, hovered, &name);
            }
            _ => draw_detail_placeholder(frame, right, theme),
        }
    }

    /// The right pane for `VmDetail::Group(name)` (spec §3.4): a title row
    /// (`Group: name  [+ Entry] [Edit fields] [Rename] [Delete]`), then the
    /// options grid for the active environment — a radio column saying
    /// which option this environment has selected, the option-name column,
    /// and one column per declared field — closed by the legend line. With
    /// no active environment there is nowhere for options to live, so the
    /// grid is replaced by [`NO_ENV_HINT`] (the top bar's environment
    /// switcher, drawn either way, is the way out of that state).
    #[allow(clippy::too_many_arguments)]
    fn draw_entry_grid(
        &mut self,
        buf: &mut Buffer,
        right: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
        selector: &str,
    ) {
        if right.width < 8 || right.height < 3 {
            return;
        }
        let state_of = |hit: &Hit| {
            if hovered == Some(hit) {
                ControlState::Hover
            } else {
                ControlState::Normal
            }
        };
        let x0 = right.x + 2;
        let inner_w = right.width.saturating_sub(4).max(1);
        let bottom = right.y + right.height;
        let mut y = right.y + 1;

        // --- title row: name + the pane's four buttons ------------------
        if y + BUTTON_HEIGHT <= bottom {
            let label = format!("Selector: {selector}");
            text(buf, x0, y + 1, &label, theme.text, theme.page, true);
            let mut bx = right.x + right.width;
            for (lbl, kind, hit) in [
                ("Delete", ButtonKind::Secondary, Hit::VmDelete),
                ("Rename", ButtonKind::Secondary, Hit::VmRename),
                ("Edit fields", ButtonKind::Secondary, Hit::VmEditFields),
                ("+ Option", ButtonKind::Primary, Hit::VmNewOption),
            ] {
                let w = button_min_width(lbl);
                if bx < x0 + label.chars().count() as u16 + w + 3 {
                    break;
                }
                bx -= w + 1;
                let rect = Rect {
                    x: bx,
                    y,
                    width: w,
                    height: BUTTON_HEIGHT,
                };
                let state = state_of(&hit);
                Button {
                    label: lbl,
                    kind,
                    state,
                }
                .paint(buf, rect, theme);
                hits.register(rect, hit);
            }
            y += BUTTON_HEIGHT + 1;
        }

        let Some(env) = ctx.active_env.clone() else {
            if y < bottom {
                text(
                    buf,
                    x0,
                    y,
                    super::chooser::clip(NO_ENV_HINT, inner_w),
                    theme.text_muted,
                    theme.page,
                    false,
                );
            }
            return;
        };

        // --- column headers ---------------------------------------------
        let fields = group_fields(ctx, selector);
        let cols = grid_columns(x0, inner_w, 1 + fields.len());
        if y >= bottom || cols.x.is_empty() {
            return;
        }
        fill(buf, Rect::new(right.x, y, right.width, 1), theme.panel);
        for (i, label) in std::iter::once("ENTRY".to_string())
            .chain(fields.iter().map(|f| f.to_uppercase()))
            .enumerate()
        {
            let (Some(cx), Some(cw)) = (cols.x.get(i), cols.w.get(i)) else {
                break;
            };
            text(
                buf,
                *cx,
                y,
                super::chooser::clip(&label, *cw),
                theme.text_muted,
                theme.panel,
                true,
            );
        }
        y += 1;

        // --- option rows (+ the always-present ghost row) ------------------
        let rows = entry_rows(ctx, selector);
        let selected = ctx.selections_for(&env).get(selector).cloned();
        // The last line of the pane belongs to the legend.
        let rows_bottom = bottom.saturating_sub(1).max(y);
        let visible = (rows_bottom - y) as usize;
        let total = rows.len() + 1;
        // A cursor that outlived the rows it pointed at (an option deleted,
        // a field removed) clamps back into the grid.
        self.grid.cursor.0 = self.grid.cursor.0.min(total - 1);
        self.grid.cursor.1 = self.grid.cursor.1.min(fields.len());
        let cursor = (self.focus == VmFocus::Grid).then_some(self.grid.cursor);
        self.grid_visible = visible;
        self.grid_area = Rect::new(right.x, y, right.width, (rows_bottom - y).max(1));
        // Keep whatever the keyboard is on (the cell under edit, else the
        // cursor) in view, then clamp — the wheel places the viewport
        // freely, but never past the last row.
        let focus_row = self
            .grid
            .editing
            .as_ref()
            .map_or(self.grid.cursor.0, |e| e.row);
        if visible > 0 {
            if focus_row < self.grid.scroll {
                self.grid.scroll = focus_row;
            } else if focus_row >= self.grid.scroll + visible {
                self.grid.scroll = focus_row + 1 - visible;
            }
            self.grid.scroll = self.grid.scroll.min(total.saturating_sub(visible));
        }

        for (pos, i) in (self.grid.scroll..total).enumerate() {
            let ry = y + pos as u16;
            if ry >= rows_bottom {
                break;
            }
            let ghost = i == rows.len();
            let hovered_row = match hovered {
                Some(Hit::VmEntryCell { row, .. }) | Some(Hit::VmEntryRadio(row)) => *row == i,
                _ => false,
            };
            // The keyboard cursor only paints while the grid actually has
            // the keyboard — a lift the arrows aren't driving would lie
            // about where keys land (Task 8's rule for its own row cursor).
            let cursor_row = cursor.is_some_and(|(r, _)| r == i);
            let bg = if hovered_row || cursor_row {
                theme.control_hover
            } else {
                theme.control
            };
            fill(buf, Rect::new(x0, ry, inner_w, 1), bg);

            if !ghost {
                let name = &rows[i].0;
                let on = selected.as_deref() == Some(name.as_str());
                let glyph = if on { GLYPH_RADIO_ON } else { GLYPH_RADIO_OFF };
                let fg = if on { theme.accent } else { theme.text_muted };
                text(buf, cols.radio_x, ry, glyph, fg, bg, false);
                hits.register(
                    Rect::new(cols.radio_x, ry, RADIO_W, 1),
                    Hit::VmEntryRadio(i),
                );
            }

            for col in 0..cols.x.len() {
                let (cx, cw) = (cols.x[col], cols.w[col]);
                // The focused cell keeps a lift of its own inside the
                // cursor row, so the keyboard's exact position — the cell
                // `Enter` would edit — is visible, not just its row. One
                // more hover-step up, the same direction focus lifts a
                // `TextField` (never darker: pressed reads as a click).
                let bg = if cursor == Some((i, col)) {
                    crate::theme::lift_color(bg, 0.06)
                } else {
                    bg
                };
                if cursor == Some((i, col)) {
                    fill(buf, Rect::new(cx, ry, cw, 1), bg);
                }
                let editing = self
                    .grid
                    .editing
                    .as_ref()
                    .filter(|e| e.row == i && e.col == col);
                if let Some(edit) = editing {
                    let line = edit.input.draw_line_windowed(true, theme, cw);
                    fill(buf, Rect::new(cx, ry, cw, 1), theme.control_hover);
                    buf.set_line(cx, ry, &line, cw);
                } else if ghost {
                    if col == 0 {
                        text(
                            buf,
                            cx,
                            ry,
                            super::chooser::clip(GHOST_LABEL, cw),
                            theme.text_muted,
                            bg,
                            false,
                        );
                    }
                } else {
                    let (name, values) = &rows[i];
                    let value = match col {
                        0 => Some(name),
                        n => fields.get(n - 1).and_then(|f| values.get(f)),
                    };
                    let (shown, fg) = match value {
                        Some(v) if !v.is_empty() => (v.as_str(), theme.text),
                        _ => ("(empty)", theme.text_muted),
                    };
                    text(buf, cx, ry, super::chooser::clip(shown, cw), fg, bg, false);
                }
                hits.register(Rect::new(cx, ry, cw, 1), Hit::VmEntryCell { row: i, col });
            }
        }

        // --- legend --------------------------------------------------------
        if rows_bottom < bottom {
            text(
                buf,
                x0,
                rows_bottom,
                super::chooser::clip(&format!("{GLYPH_RADIO_ON} = selected for {env}"), inner_w),
                theme.text_muted,
                theme.page,
                false,
            );
        }
    }

    /// The right pane for `VmDetail::Var(name)` (spec §3.4): a title row
    /// (`name  🔒?  [Rename] [Delete]`), then description/default/env-value
    /// fields as label + `TextField` rows (`Default` omitted for a secret —
    /// it can never hold one), the secret on/off toggle, the promote/demote
    /// button where [`promote_demote_action`] applies, and a dim `used by:`
    /// line. `name` is guaranteed declared by the caller.
    #[allow(clippy::too_many_arguments)]
    fn draw_var_form(
        &self,
        buf: &mut Buffer,
        right: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
        name: &str,
    ) {
        if right.width < 8 || right.height < 3 {
            return;
        }
        let secret = ctx.model.vars.get(name).is_some_and(|d| d.secret);
        let state_of = |hit: &Hit| {
            if hovered == Some(hit) {
                ControlState::Hover
            } else {
                ControlState::Normal
            }
        };

        let x0 = right.x + 2;
        let field_w = right.width.saturating_sub(4).max(1);
        let bottom = right.y + right.height;
        let mut y = right.y + 1;

        // --- title row: name, lock badge, Rename/Delete ---------------
        if y + BUTTON_HEIGHT <= bottom {
            let mid = y + 1;
            let label = if secret {
                format!("{name}  {GLYPH_LOCK}")
            } else {
                name.to_string()
            };
            text(buf, x0, mid, &label, theme.text, theme.page, true);
            let mut bx = right.x + right.width;
            for (lbl, hit) in [("Delete", Hit::VmDelete), ("Rename", Hit::VmRename)] {
                let w = button_min_width(lbl);
                if bx < x0 + label.chars().count() as u16 + w + 3 {
                    break;
                }
                bx -= w + 1;
                let rect = Rect {
                    x: bx,
                    y,
                    width: w,
                    height: BUTTON_HEIGHT,
                };
                Button {
                    label: lbl,
                    kind: ButtonKind::Secondary,
                    state: state_of(&hit),
                }
                .paint(buf, rect, theme);
                hits.register(rect, hit);
            }
            y += BUTTON_HEIGHT + 1;
        }

        // --- Description ------------------------------------------------
        y = self.draw_labeled_field(
            buf,
            hits,
            hovered,
            theme,
            x0,
            field_w,
            bottom,
            y,
            "Description",
            VmField::Description,
            ctx,
            name,
            false,
        );

        // --- Default (never for a secret: it can't hold one) -------------
        if !secret {
            y = self.draw_labeled_field(
                buf,
                hits,
                hovered,
                theme,
                x0,
                field_w,
                bottom,
                y,
                "Default",
                VmField::Default,
                ctx,
                name,
                false,
            );
        }

        // --- secret on/off toggle -----------------------------------------
        if y < bottom {
            text(buf, x0, y, "Secret", theme.text_muted, theme.page, false);
            let toggle_label = if secret { "[on]" } else { "[off]" };
            let hit = Hit::VmSecretToggle;
            let hovered_toggle = hovered == Some(&hit);
            let style = if hovered_toggle {
                Style::default().bg(theme.accent).fg(theme.on_accent)
            } else {
                Style::default().fg(theme.accent)
            };
            let tw = toggle_label.chars().count() as u16;
            let tx = (x0 + field_w).saturating_sub(tw);
            buf.set_string(tx, y, toggle_label, style);
            hits.register(Rect::new(tx, y, tw, 1), hit);
            y += 2;
        }

        // --- Value in <env> (masked + reveal for a secret) -----------------
        let value_label = match &ctx.active_env {
            Some(env) => format!("Value in {env}"),
            None => "(no environment)".to_string(),
        };
        if y < bottom {
            text(
                buf,
                x0,
                y,
                &value_label,
                theme.text_muted,
                theme.page,
                false,
            );
            if secret {
                let reveal_label = if self.form.revealed {
                    "\u{1f441} hide"
                } else {
                    "\u{1f441} reveal"
                };
                let hit = Hit::VmRevealToggle;
                let hovered_toggle = hovered == Some(&hit);
                let style = if hovered_toggle {
                    Style::default().bg(theme.accent).fg(theme.on_accent)
                } else {
                    Style::default().fg(theme.accent)
                };
                let rw = reveal_label.chars().count() as u16;
                let rx = (x0 + field_w).saturating_sub(rw);
                buf.set_string(rx, y, reveal_label, style);
                hits.register(Rect::new(rx, y, rw, 1), hit);
            }
            y += 1;
        }
        if y + FIELD_HEIGHT <= bottom {
            let masked = secret && !self.form.revealed;
            let area = Rect {
                x: x0,
                y,
                width: field_w,
                height: FIELD_HEIGHT,
            };
            self.draw_form_field(
                buf,
                hits,
                area,
                theme,
                hovered,
                VmField::EnvValue,
                ctx,
                name,
                masked,
            );
            y += FIELD_HEIGHT + 1;
        }

        // --- promote/demote --------------------------------------------
        if let Some((label, _)) = promote_demote_action(ctx, open_request, name)
            && y + BUTTON_HEIGHT <= bottom
        {
            let w = button_min_width(label).min(field_w);
            let rect = Rect {
                x: x0,
                y,
                width: w,
                height: BUTTON_HEIGHT,
            };
            let hit = Hit::VmPromoteBtn;
            Button {
                label,
                kind: ButtonKind::Secondary,
                state: state_of(&hit),
            }
            .paint(buf, rect, theme);
            hits.register(rect, hit);
            y += BUTTON_HEIGHT + 1;
        }

        // --- used by -----------------------------------------------------
        if y < bottom {
            let usage = postui_core::varedit::scan_usage(&ctx.root, name);
            let line = if usage.is_empty() {
                "used by: (none)".to_string()
            } else {
                format!("used by: {}", usage.join(", "))
            };
            text(
                buf,
                x0,
                y,
                super::chooser::clip(&line, field_w),
                theme.text_muted,
                theme.page,
                false,
            );
        }
    }

    /// One label + `TextField` row: the label on its own line, the field
    /// [`FIELD_HEIGHT`] rows below. Returns the next `y` past the field
    /// (unchanged, i.e. no gap consumed, when there isn't room to draw it —
    /// so a caller past the pane's bottom just stops drawing further rows).
    #[allow(clippy::too_many_arguments)]
    fn draw_labeled_field(
        &self,
        buf: &mut Buffer,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
        theme: &Theme,
        x0: u16,
        field_w: u16,
        bottom: u16,
        y: u16,
        label: &str,
        field: VmField,
        ctx: &ProjectContext,
        name: &str,
        masked: bool,
    ) -> u16 {
        if y >= bottom {
            return y;
        }
        text(buf, x0, y, label, theme.text_muted, theme.page, false);
        let field_y = y + 1;
        if field_y + FIELD_HEIGHT > bottom {
            return field_y;
        }
        let area = Rect {
            x: x0,
            y: field_y,
            width: field_w,
            height: FIELD_HEIGHT,
        };
        self.draw_form_field(buf, hits, area, theme, hovered, field, ctx, name, masked);
        field_y + FIELD_HEIGHT + 1
    }

    /// Paints one field's `TextField`: the live `LineInput` (windowed,
    /// masked when `masked`) while it's under edit, else the resting text
    /// `field_seed_text` reads from `ctx` (masked to dots when `masked`, a
    /// muted "(not set)" when empty). Registers `Hit::VmFormField(field)`
    /// over the whole painted area.
    #[allow(clippy::too_many_arguments)]
    fn draw_form_field(
        &self,
        buf: &mut Buffer,
        hits: &mut HitMap,
        area: Rect,
        theme: &Theme,
        hovered: Option<&Hit>,
        field: VmField,
        ctx: &ProjectContext,
        name: &str,
        masked: bool,
    ) {
        let hit = Hit::VmFormField(field);
        let editing = self.form.editing.as_ref().filter(|(f, _)| *f == field);
        let state = if editing.is_some() {
            ControlState::Focused
        } else if hovered == Some(&hit) {
            ControlState::Hover
        } else {
            ControlState::Normal
        };
        let inner_w = area.width.saturating_sub(2);
        let content = if let Some((_, input)) = editing {
            if masked {
                input.draw_line_windowed_masked(true, theme, inner_w)
            } else {
                input.draw_line_windowed(true, theme, inner_w)
            }
        } else {
            let text_value = field_seed_text(ctx, name, field);
            if masked {
                Line::raw(text_value.chars().map(|_| '\u{25cf}').collect::<String>())
            } else if text_value.is_empty() {
                Line::styled("(not set)", Style::default().fg(theme.text_muted))
            } else {
                Line::raw(text_value)
            }
        };
        TextField { content, state }.paint(buf, area, theme);
        hits.register(area, hit);
    }

    fn draw_top_bar(
        &self,
        frame: &mut Frame,
        bar: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        let buf = frame.buffer_mut();
        fill(buf, bar, theme.panel);
        if bar.height < BUTTON_HEIGHT {
            return;
        }

        let state_of = |hit: &Hit| {
            if hovered == Some(hit) {
                ControlState::Hover
            } else {
                ControlState::Normal
            }
        };

        let env_label = format!("Environment: {} {GLYPH_CARET}", ctx.env_label());
        let env_w = button_min_width(&env_label).min(bar.width);
        let env_area = Rect {
            x: bar.x + 1,
            y: bar.y,
            width: env_w,
            height: BUTTON_HEIGHT,
        };
        Button {
            label: &env_label,
            kind: ButtonKind::Secondary,
            state: state_of(&Hit::VmEnvSwitch),
        }
        .paint(buf, env_area, theme);
        hits.register(env_area, Hit::VmEnvSwitch);

        // Right-aligned selector, laid out from the bar's right edge inward so
        // the buttons stay put as the environment name changes width. The
        // close button rides along as the mouse's way back to the main
        // screen (the header's vars chip toggles it too), labelled with the
        // key that does the same thing.
        let mut x = bar.x + bar.width;
        for (label, kind, hit) in [
            (
                "Close (esc)",
                ButtonKind::Secondary,
                Hit::FooterChip(Action::CloseScreen),
            ),
            ("+ Selector", ButtonKind::Secondary, Hit::VmNewSelector),
            ("+ Variable", ButtonKind::Primary, Hit::VmNewVar),
        ] {
            let w = button_min_width(label);
            if x < env_area.x + env_area.width + w + 2 {
                break;
            }
            x -= w + 1;
            let rect = Rect {
                x,
                y: bar.y,
                width: w,
                height: BUTTON_HEIGHT,
            };
            let state = state_of(&hit);
            Button { label, kind, state }.paint(buf, rect, theme);
            hits.register(rect, hit);
        }
    }

    fn draw_left(
        &mut self,
        frame: &mut Frame,
        left: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        hits: &mut HitMap,
        hovered: Option<&Hit>,
    ) {
        fill(frame.buffer_mut(), left, theme.panel);
        if left.width <= 2 || left.height == 0 {
            self.visible_rows = 0;
            return;
        }

        // Rows keep a 1-column inset each side: the left column is the
        // selected row's accent lane, the right one hosts the scrollbar.
        let list = Rect {
            x: left.x + 1,
            y: left.y,
            width: left.width - 2,
            height: left.height,
        };
        self.visible_rows = Self::rows_that_fit(list.height);
        if self.ensure_visible {
            if self.visible_rows > 0 {
                if self.left_cursor < self.left_scroll {
                    self.left_scroll = self.left_cursor;
                } else if self.left_cursor >= self.left_scroll + self.visible_rows {
                    self.left_scroll = self.left_cursor + 1 - self.visible_rows;
                }
                let max_scroll = self.left_rows.len().saturating_sub(self.visible_rows);
                self.left_scroll = self.left_scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        if let Some(spec) = self.scrollbar_spec().filter(ScrollbarSpec::overflows) {
            let column = Rect {
                x: left.x + left.width - 1,
                width: 1,
                ..list
            };
            crate::hit::draw_scrollbar(frame, hits, column, &spec, hovered, false, theme);
        }

        let buf = frame.buffer_mut();
        for (pos, (i, row)) in self
            .left_rows
            .iter()
            .enumerate()
            .skip(self.left_scroll)
            .take(self.visible_rows.max(1))
            .enumerate()
        {
            let y = list.y + pos as u16;
            if y >= left.y + left.height {
                break;
            }
            let selected = match (&self.detail, row) {
                (VmDetail::Var(a), VmRow::Var(b)) | (VmDetail::Group(a), VmRow::Group(b)) => a == b,
                _ => false,
            };
            let highlight = if selected {
                RowHighlight::Selected
            } else if hovered == Some(&Hit::VmLeftRow(i)) || i == self.left_cursor {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            // Popup/master-detail convention (matches chooser/palette/var
            // picker): no zebra — the left list is section-divided and
            // typically short, so a stripe would add noise without the
            // long-uniform-list payoff zebra earns in the sidebar.
            let hover_t = 1.0;
            let row_bg = ListRow::resolve_fill(theme, highlight, theme.panel, hover_t);
            if row.is_stop() {
                ListRow {
                    highlight,
                    zebra: None,
                }
                .paint(buf, y, list.x, list.width, theme.panel, hover_t, theme);
            }
            paint_left_row(buf, ctx, row, y, list, row_bg, theme);

            let hit_top = y.max(left.y);
            let hit_bottom = (y + 1).min(left.y + left.height);
            hits.register(
                Rect {
                    x: list.x,
                    y: hit_top,
                    width: list.width,
                    height: hit_bottom.saturating_sub(hit_top),
                },
                Hit::VmLeftRow(i),
            );
        }
    }
}

/// One left-list row's content: the section labels, a variable (name, lock
/// badge, unresolved dot) or a selector (`▶ name (option)`).
fn paint_left_row(
    buf: &mut ratatui::buffer::Buffer,
    ctx: &ProjectContext,
    row: &VmRow,
    y: u16,
    list: Rect,
    bg: ratatui::style::Color,
    theme: &Theme,
) {
    let x = list.x + 1;
    // One column of inset on the left (the selected row's accent bar) —
    // the label may run to the list's right edge.
    let width = list.width.saturating_sub(1);
    match row {
        VmRow::SectionVars | VmRow::SectionGroups => {
            let label = if matches!(row, VmRow::SectionVars) {
                "VARIABLES"
            } else {
                "SELECTORS"
            };
            text(buf, list.x, y, label, theme.text_muted, bg, true);
        }
        VmRow::Var(name) => {
            let secret = ctx.model.vars.get(name).is_some_and(|d| d.secret);
            // The badges are right-aligned in their own columns, so names
            // stay left-aligned however long they are.
            let mut label_w = width;
            if secret {
                label_w = label_w.saturating_sub(3);
            }
            if is_unresolved(ctx, name) {
                label_w = label_w.saturating_sub(2);
            }
            text(
                buf,
                x,
                y,
                super::chooser::clip(name, label_w),
                theme.text,
                bg,
                false,
            );
            let mut badge_x = list.x + list.width;
            if is_unresolved(ctx, name) {
                badge_x = badge_x.saturating_sub(2);
                text(buf, badge_x, y, GLYPH_UNRESOLVED, theme.error, bg, false);
            }
            if secret {
                badge_x = badge_x.saturating_sub(3);
                text(buf, badge_x, y, GLYPH_LOCK, theme.text_muted, bg, false);
            }
        }
        VmRow::Group(name) => {
            let selection = match active_selection(ctx, name) {
                Some(option) => format!("({option})"),
                None => "(needs selection)".to_string(),
            };
            let label = format!("{GLYPH_GROUP} {name} {selection}");
            text(
                buf,
                x,
                y,
                super::chooser::clip(&label, width),
                theme.text,
                bg,
                false,
            );
        }
    }
}

/// The detail pane until Tasks 15/16 fill it: one muted instruction line.
fn draw_detail_placeholder(frame: &mut Frame, right: Rect, theme: &Theme) {
    let buf = frame.buffer_mut();
    fill(buf, right, theme.page);
    if right.width == 0 || right.height < 2 {
        return;
    }
    text(
        buf,
        right.x + 2,
        right.y + 1,
        super::chooser::clip(
            "select a variable or selector",
            right.width.saturating_sub(3),
        ),
        theme.text_muted,
        theme.page,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::project;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    /// A project with two envs (dev, qa; qa active); `base_url` simple with
    /// a default; `api_key` secret with no value anywhere (so it reads as
    /// unresolved); selector `creds` with two fields and two options in qa,
    /// `alice` selected there and nothing selected in dev.
    fn fixture() -> (tempfile::TempDir, ProjectContext) {
        let dir = tempfile::tempdir().unwrap();
        project::init_project(dir.path(), Some("demo")).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service key"
secret = true

[selectors.creds]
description = "paired ids"
fields = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.creds.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-77\"\n\n[options.creds.bob]\nuser_id = \"2002\"\ncustomer_id = \"c-91\"\n",
        )
        .unwrap();

        let mut selections = IndexMap::new();
        let mut qa_sel = IndexMap::new();
        qa_sel.insert("creds".to_string(), "alice".to_string());
        selections.insert("qa".to_string(), qa_sel);
        project::save_local_state(
            dir.path(),
            &project::LocalState {
                environment: Some("qa".into()),
                selections,
                ..Default::default()
            },
        )
        .unwrap();

        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(warns.is_empty(), "{warns:?}");
        (dir, ctx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render(vm: &mut VarManager, ctx: &ProjectContext) -> (String, HitMap) {
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, ctx, None, &mut hits, None))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    #[test]
    fn left_list_is_variables_then_groups_with_the_selection_inline() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (content, _) = render(&mut vm, &ctx);

        assert_eq!(
            vm.left_rows,
            vec![
                VmRow::SectionVars,
                VmRow::Var("base_url".into()),
                VmRow::Var("api_key".into()),
                VmRow::SectionGroups,
                VmRow::Group("creds".into()),
            ],
            "selector fields (user_id/customer_id) are not top-level rows"
        );
        assert!(content.contains("VARIABLES"), "{content}");
        assert!(content.contains("SELECTORS"), "{content}");
        assert!(content.contains("creds (alice)"), "{content}");
        assert!(content.contains(GLYPH_LOCK), "secret badge: {content}");
        assert!(
            content.contains(GLYPH_UNRESOLVED),
            "unresolved dot for the value-less secret: {content}"
        );
    }

    #[test]
    fn a_group_with_no_selection_says_so() {
        let (_dir, mut ctx) = fixture();
        ctx.clear_selection_for("qa", "creds");
        let mut vm = VarManager::default();
        let (content, _) = render(&mut vm, &ctx);
        assert!(content.contains("creds (needs selection)"), "{content}");
    }

    #[test]
    fn clicking_a_group_row_opens_it_in_the_detail_pane() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        let group_row = vm
            .left_rows
            .iter()
            .position(|r| r == &VmRow::Group("creds".into()))
            .unwrap();

        vm.select_row(group_row);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));
        assert_eq!(vm.left_cursor, group_row);

        // …and a variable row, likewise.
        vm.select_row(1);
        assert_eq!(vm.detail, VmDetail::Var("base_url".into()));
    }

    #[test]
    fn clicking_a_section_header_moves_the_cursor_but_keeps_the_detail() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        vm.select_row(1);
        vm.select_row(0);
        assert_eq!(vm.left_cursor, 0);
        assert_eq!(vm.detail, VmDetail::Var("base_url".into()));
    }

    #[test]
    fn every_row_registers_a_left_row_hit() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (_, hits) = render(&mut vm, &ctx);
        for i in 0..vm.left_rows.len() {
            assert!(
                hits.rect_of(&Hit::VmLeftRow(i)).is_some(),
                "row {i} has no hit"
            );
        }
    }

    #[test]
    fn arrows_skip_section_headers_and_open_what_they_land_on() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);

        assert!(vm.handle_key(key(KeyCode::Down), &ctx).is_none());
        assert_eq!(vm.detail, VmDetail::Var("base_url".into()));
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Var("api_key".into()));
        // Skips the SELECTORS header.
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));
        // …and stops at the end.
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));

        vm.handle_key(key(KeyCode::Up), &ctx);
        assert_eq!(vm.detail, VmDetail::Var("api_key".into()));
    }

    #[test]
    fn top_bar_registers_the_env_switch_and_both_new_buttons() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (content, hits) = render(&mut vm, &ctx);
        assert!(content.contains("Environment: qa"), "{content}");
        assert!(hits.rect_of(&Hit::VmEnvSwitch).is_some());
        assert!(hits.rect_of(&Hit::VmNewVar).is_some());
        assert!(hits.rect_of(&Hit::VmNewSelector).is_some());
        assert!(content.contains("+ Variable"), "{content}");
        assert!(content.contains("+ Selector"), "{content}");
        assert!(content.contains("Close (esc)"), "{content}");
        assert!(
            hits.rect_of(&Hit::FooterChip(Action::CloseScreen))
                .is_some(),
            "the close button is the mouse's way back"
        );
    }

    #[test]
    fn the_detail_pane_asks_for_a_selection_until_a_row_is_open() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        let (content, _) = render(&mut vm, &ctx);
        assert!(
            content.contains("select a variable or selector"),
            "{content}"
        );
    }

    #[test]
    fn commands_target_the_left_selection() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);

        assert_eq!(
            vm.handle_key(key(KeyCode::Char('n')), &ctx),
            Some(Action::PromptNewVar)
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('g')), &ctx),
            Some(Action::PromptNewSelector)
        );

        vm.select_row(1); // base_url
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('e')), &ctx),
            Some(Action::PromptRenameVar {
                from: "base_url".into()
            })
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::F(2)), &ctx),
            Some(Action::PromptRenameVar {
                from: "base_url".into()
            })
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('d')), &ctx),
            Some(Action::ConfirmDeleteVar {
                name: "base_url".into()
            })
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('s')), &ctx),
            Some(Action::ToggleSecretVar {
                name: "base_url".into()
            })
        );

        let group_row = vm.left_rows.len() - 1;
        vm.select_row(group_row);
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('d')), &ctx),
            Some(Action::ConfirmDeleteVar {
                name: "creds".into()
            }),
            "delete works on a selector row"
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('e')), &ctx),
            Some(Action::PromptRenameVar {
                from: "creds".into()
            }),
            "a selector renames through the same prompt a variable does"
        );
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('s')), &ctx),
            None,
            "a selector has no secret flag"
        );
    }

    #[test]
    fn commands_yield_to_an_active_cell_edit_but_navigation_does_not() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        vm.select_row(1);
        vm.grid.editing = Some(GridEdit {
            row: 0,
            col: 0,
            input: LineInput::new(""),
            original: String::new(),
        });

        assert_eq!(vm.handle_key(key(KeyCode::Char('n')), &ctx), None);
        assert_eq!(vm.handle_key(key(KeyCode::Char('d')), &ctx), None);
        assert_eq!(
            vm.handle_key(key(KeyCode::Esc), &ctx),
            Some(Action::CloseScreen)
        );
        vm.handle_key(key(KeyCode::Down), &ctx);
        assert_eq!(vm.detail, VmDetail::Var("api_key".into()));
    }

    #[test]
    fn context_menu_offers_rename_duplicate_delete() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);

        let items = vm.context_menu(1).expect("variable menu");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Rename\u{2026}", "Duplicate", "Delete\u{2026}"]
        );
        assert_eq!(
            items[0].action,
            Some(Action::PromptRenameVar {
                from: "base_url".into()
            })
        );
        assert_eq!(
            items[1].action,
            Some(Action::DuplicateVar {
                name: "base_url".into()
            })
        );
        assert_eq!(
            items[2].action,
            Some(Action::ConfirmDeleteVar {
                name: "base_url".into()
            })
        );

        let selector = vm
            .context_menu(vm.left_rows.len() - 1)
            .expect("selector menu");
        assert_eq!(
            selector[0].action,
            Some(Action::PromptRenameVar {
                from: "creds".into()
            }),
            "a selector's rename is live"
        );
        assert_eq!(
            selector[1].action, None,
            "so is duplicate: a field can only belong to one selector"
        );
        assert_eq!(
            selector[2].action,
            Some(Action::ConfirmDeleteVar {
                name: "creds".into()
            })
        );

        assert!(vm.context_menu(0).is_none(), "no menu on a section header");
    }

    #[test]
    fn sync_clamps_the_cursor_and_drops_a_deleted_detail() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        render(&mut vm, &ctx);
        vm.left_cursor = 99;
        vm.detail = VmDetail::Var("gone".into());
        vm.sync(&ctx);
        assert_eq!(vm.left_cursor, vm.left_rows.len() - 1);
        assert_eq!(vm.detail, VmDetail::None);

        vm.detail = VmDetail::Group("creds".into());
        vm.sync(&ctx);
        assert_eq!(vm.detail, VmDetail::Group("creds".into()));
    }

    #[test]
    fn a_long_list_scrolls_and_draws_a_scrollbar() {
        let (dir, _ctx) = fixture();
        let mut decls = String::new();
        for i in 0..40 {
            decls.push_str(&format!("[v{i:02}]\ndefault = \"x\"\n\n"));
        }
        std::fs::write(dir.path().join("variables.toml"), decls).unwrap();
        let (ctx, _) = ProjectContext::open(dir.path().to_path_buf());

        let mut vm = VarManager::default();
        let (content, hits) = render(&mut vm, &ctx);
        assert!(content.contains('\u{2588}'), "thumb glyph drawn: {content}");
        assert!(
            hits.rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar))
                .is_some()
        );

        vm.handle_scroll(5);
        assert_eq!(vm.left_scroll, 5);
        let (content, _) = render(&mut vm, &ctx);
        assert!(!content.contains("v00"), "scrolled past the first row");

        // The keyboard snaps the viewport back to its cursor.
        vm.select_row(1);
        vm.handle_key(key(KeyCode::Down), &ctx);
        let (content, _) = render(&mut vm, &ctx);
        assert!(content.contains("v01"), "cursor row is visible: {content}");
    }

    // --- Task 15: variable detail form -----------------------------------

    /// The form's full column of rows (title, three fields, the promote/
    /// demote button and the usage line) doesn't fit `render`'s 24-row
    /// screen all at once — plenty tall for a real terminal, but tests that
    /// need to see the whole column use this taller one instead.
    fn render_with_request(
        vm: &mut VarManager,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
    ) -> (String, HitMap) {
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, ctx, open_request, &mut hits, None))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    fn select_var(vm: &mut VarManager, ctx: &ProjectContext, name: &str) {
        render(vm, ctx); // populates left_rows
        let i = vm
            .left_rows
            .iter()
            .position(|r| r == &VmRow::Var(name.into()))
            .unwrap();
        vm.select_row(i);
    }

    #[test]
    fn selecting_a_var_renders_its_description_default_and_env_value() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        select_var(&mut vm, &ctx, "base_url");
        let (content, hits) = render_with_request(&mut vm, &ctx, None);
        assert!(content.contains("base_url"), "{content}");
        assert!(content.contains("Description"), "{content}");
        assert!(content.contains("API root"), "{content}");
        assert!(content.contains("Default"), "{content}");
        assert!(content.contains("http://localhost:8080"), "{content}");
        assert!(content.contains("Value in qa"), "{content}");
        assert!(hits.rect_of(&Hit::VmRename).is_some());
        assert!(hits.rect_of(&Hit::VmDelete).is_some());
        assert!(
            hits.rect_of(&Hit::VmFormField(VmField::Description))
                .is_some()
        );
        assert!(hits.rect_of(&Hit::VmFormField(VmField::Default)).is_some());
        assert!(hits.rect_of(&Hit::VmFormField(VmField::EnvValue)).is_some());
        assert!(content.contains("used by:"), "{content}");
    }

    #[test]
    fn a_secret_var_hides_the_default_row_and_masks_its_value_with_a_reveal_toggle() {
        let (dir, _ctx) = fixture();
        let mut secrets = IndexMap::new();
        let mut qa_secrets = IndexMap::new();
        qa_secrets.insert("api_key".to_string(), "sk-live-secret".to_string());
        secrets.insert("qa".to_string(), qa_secrets);
        project::save_secrets(dir.path(), &secrets).unwrap();
        let (ctx, _) = ProjectContext::open(dir.path().to_path_buf());

        let mut vm = VarManager::default();
        select_var(&mut vm, &ctx, "api_key");
        let (content, hits) = render(&mut vm, &ctx);
        assert!(!content.contains("Default"), "{content}");
        assert!(!content.contains("sk-live-secret"), "{content}");
        assert!(content.contains('\u{25cf}'), "masked dots: {content}");
        assert!(hits.rect_of(&Hit::VmRevealToggle).is_some());

        vm.form.revealed = true;
        let (content, _) = render(&mut vm, &ctx);
        assert!(content.contains("sk-live-secret"), "{content}");
    }

    #[test]
    fn no_active_environment_shows_a_hint_instead_of_a_value_field_target() {
        let (_dir, mut ctx) = fixture();
        ctx.active_env = None;
        let mut vm = VarManager::default();
        select_var(&mut vm, &ctx, "base_url");
        let (content, _) = render(&mut vm, &ctx);
        assert!(content.contains("(no environment)"), "{content}");
    }

    #[test]
    fn selecting_a_different_var_resets_the_form_but_a_reclick_does_not() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        select_var(&mut vm, &ctx, "base_url");
        vm.form.revealed = true;
        vm.form.editing = Some((VmField::Description, LineInput::new("typing...")));

        // Re-selecting the same row must not disturb an in-progress edit.
        let same = vm
            .left_rows
            .iter()
            .position(|r| r == &VmRow::Var("base_url".into()))
            .unwrap();
        vm.select_row(same);
        assert!(vm.form.editing.is_some(), "same-row reselect kept the edit");
        assert!(vm.form.revealed, "same-row reselect kept reveal");

        let other = vm
            .left_rows
            .iter()
            .position(|r| r == &VmRow::Var("api_key".into()))
            .unwrap();
        vm.select_row(other);
        assert!(vm.form.editing.is_none(), "a new selection drops the edit");
        assert!(!vm.form.revealed, "a new selection resets reveal");
    }

    #[test]
    fn field_seed_text_reads_description_default_and_the_stored_env_value() {
        let (_dir, mut ctx) = fixture();
        assert_eq!(
            field_seed_text(&ctx, "base_url", VmField::Description),
            "API root"
        );
        assert_eq!(
            field_seed_text(&ctx, "base_url", VmField::Default),
            "http://localhost:8080"
        );
        // qa stores nothing for base_url: the env-value field must read
        // empty (rendered "(not set)"), never fall back to the default —
        // showing the default here made it look like an env value existed.
        assert_eq!(field_seed_text(&ctx, "base_url", VmField::EnvValue), "");
        ctx.env_data
            .values
            .insert("base_url".into(), "https://qa.example.com".into());
        assert_eq!(
            field_seed_text(&ctx, "base_url", VmField::EnvValue),
            "https://qa.example.com"
        );

        // A secret's stored value lives in the secrets store, not env_data.
        assert_eq!(field_seed_text(&ctx, "api_key", VmField::EnvValue), "");
        ctx.secrets
            .entry("qa".to_string())
            .or_default()
            .insert("api_key".into(), "s3cret".into());
        assert_eq!(
            field_seed_text(&ctx, "api_key", VmField::EnvValue),
            "s3cret"
        );

        // No active environment: the field edits the declaration default
        // (var_edit_op_for's fallback), so it seeds from it.
        ctx.active_env = None;
        assert_eq!(
            field_seed_text(&ctx, "base_url", VmField::EnvValue),
            "http://localhost:8080"
        );
    }

    #[test]
    fn var_edit_op_for_targets_the_env_or_secret_store_and_falls_back_to_default_with_no_env() {
        let (_dir, mut ctx) = fixture();
        assert_eq!(
            var_edit_op_for(&ctx, "base_url", VmField::Description, "new desc".into()),
            VarEditOp::SetDescription {
                owner: "base_url".into(),
                value: "new desc".into()
            }
        );
        assert_eq!(
            var_edit_op_for(&ctx, "base_url", VmField::Default, "x".into()),
            VarEditOp::SetDefault {
                name: "base_url".into(),
                value: "x".into()
            }
        );
        assert_eq!(
            var_edit_op_for(&ctx, "base_url", VmField::EnvValue, "y".into()),
            VarEditOp::SetEnvValue {
                env: "qa".into(),
                name: "base_url".into(),
                value: "y".into()
            }
        );
        assert_eq!(
            var_edit_op_for(&ctx, "api_key", VmField::EnvValue, "z".into()),
            VarEditOp::SetSecretValue {
                env: "qa".into(),
                name: "api_key".into(),
                value: "z".into()
            },
            "a secret's value never lands in the env file"
        );

        ctx.active_env = None;
        assert_eq!(
            var_edit_op_for(&ctx, "base_url", VmField::EnvValue, "w".into()),
            VarEditOp::SetDefault {
                name: "base_url".into(),
                value: "w".into()
            },
            "no active environment: the value field targets the declaration default"
        );
    }

    fn req_with_var(name: &str, value: &str) -> HttpRequest {
        HttpRequest::from_toml_str(&format!(
            "url = \"https://x\"\n[variables]\n{name} = \"{value}\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn promote_demote_action_follows_whether_the_open_request_overrides_the_name() {
        let (_dir, ctx) = fixture();
        assert_eq!(
            promote_demote_action(&ctx, None, "base_url"),
            None,
            "no open request: neither applies"
        );

        let overriding = req_with_var("base_url", "http://elsewhere");
        assert_eq!(
            promote_demote_action(&ctx, Some(&overriding), "base_url").map(|(l, _)| l),
            Some("Promote"),
            "the open request already overrides it: offer to promote that override up"
        );

        let plain_req = HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap();
        assert_eq!(
            promote_demote_action(&ctx, Some(&plain_req), "base_url").map(|(l, _)| l),
            Some("Demote"),
            "no override: offer to push the project value down"
        );

        assert_eq!(
            promote_demote_action(&ctx, Some(&plain_req), "api_key"),
            None,
            "a secret can never move through either direction"
        );
    }

    #[test]
    fn the_promote_button_appears_only_when_its_precondition_holds() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        select_var(&mut vm, &ctx, "base_url");

        let (content, hits) = render_with_request(&mut vm, &ctx, None);
        assert!(hits.rect_of(&Hit::VmPromoteBtn).is_none());
        let _ = content;

        let overriding = req_with_var("base_url", "http://elsewhere");
        let (content, hits) = render_with_request(&mut vm, &ctx, Some(&overriding));
        assert!(content.contains("Promote"), "{content}");
        assert!(hits.rect_of(&Hit::VmPromoteBtn).is_some());

        let plain_req = HttpRequest::from_toml_str("url = \"https://x\"\n").unwrap();
        let (content, hits) = render_with_request(&mut vm, &ctx, Some(&plain_req));
        assert!(content.contains("Demote"), "{content}");
        assert!(hits.rect_of(&Hit::VmPromoteBtn).is_some());
    }

    // --- Task 16: the selector options grid ---------------------------------

    fn select_group(vm: &mut VarManager, ctx: &ProjectContext, name: &str) {
        render(vm, ctx); // populates left_rows
        let i = vm
            .left_rows
            .iter()
            .position(|r| r == &VmRow::Group(name.into()))
            .unwrap();
        vm.select_row(i);
    }

    #[test]
    fn selecting_a_group_renders_its_entries_against_the_active_envs_fields() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        select_group(&mut vm, &ctx, "creds");
        let (content, hits) = render(&mut vm, &ctx);

        assert!(content.contains("Selector: creds"), "{content}");
        // One column per declared field, one row per option of the active
        // environment, each cell holding that option's value.
        assert!(content.contains("USER_ID"), "{content}");
        assert!(content.contains("CUSTOMER_ID"), "{content}");
        for cell in ["alice", "1001", "c-77", "bob", "2002", "c-91"] {
            assert!(content.contains(cell), "missing {cell}: {content}");
        }
        assert!(content.contains(GHOST_LABEL), "ghost row: {content}");
        assert!(
            content.contains(&format!("{GLYPH_RADIO_ON} = selected for qa")),
            "legend: {content}"
        );

        for row in 0..2 {
            assert!(
                hits.rect_of(&Hit::VmEntryRadio(row)).is_some(),
                "row {row} has no radio"
            );
            for col in 0..3 {
                assert!(
                    hits.rect_of(&Hit::VmEntryCell { row, col }).is_some(),
                    "no hit for cell {row}/{col}"
                );
            }
        }
        // The ghost row's own name cell is clickable — that is the "start a
        // new option" gesture.
        assert!(
            hits.rect_of(&Hit::VmEntryCell { row: 2, col: 0 }).is_some(),
            "ghost row cell"
        );
        for hit in [
            Hit::VmNewOption,
            Hit::VmEditFields,
            Hit::VmRename,
            Hit::VmDelete,
        ] {
            assert!(hits.rect_of(&hit).is_some(), "{hit:?} has no button");
        }
    }

    #[test]
    fn the_selected_entrys_radio_is_the_filled_one() {
        let (_dir, mut ctx) = fixture();
        let mut vm = VarManager::default();
        select_group(&mut vm, &ctx, "creds");
        let (content, _) = render(&mut vm, &ctx);
        assert!(
            content.contains(GLYPH_RADIO_ON),
            "alice is selected: {content}"
        );
        assert!(content.contains(GLYPH_RADIO_OFF), "bob is not: {content}");

        // With nothing selected every radio is empty — the only filled
        // glyph left on screen is the legend's own.
        assert_eq!(content.matches(GLYPH_RADIO_ON).count(), 2, "{content}");
        ctx.clear_selection_for("qa", "creds");
        let (content, _) = render(&mut vm, &ctx);
        assert_eq!(content.matches(GLYPH_RADIO_ON).count(), 1, "{content}");
    }

    #[test]
    fn a_group_with_no_active_environment_shows_the_hint_instead_of_a_grid() {
        let (_dir, mut ctx) = fixture();
        ctx.active_env = None;
        let mut vm = VarManager::default();
        select_group(&mut vm, &ctx, "creds");
        let (content, hits) = render(&mut vm, &ctx);
        assert!(
            content.contains("options live in environments"),
            "{content}"
        );
        assert!(
            hits.rect_of(&Hit::VmEntryCell { row: 0, col: 0 }).is_none(),
            "no grid without an environment"
        );
        assert!(
            hits.rect_of(&Hit::VmEnvSwitch).is_some(),
            "the way out of that state is still on screen"
        );
    }

    #[test]
    fn grid_commands_act_on_the_open_group() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        select_group(&mut vm, &ctx, "creds");
        render(&mut vm, &ctx);

        // `space` selects the option the grid cursor is on.
        vm.grid.cursor = (1, 0);
        assert_eq!(
            vm.handle_key(key(KeyCode::Char(' ')), &ctx),
            Some(Action::VarEdit(VarEditOp::SelectOption {
                env: "qa".into(),
                selector: "creds".into(),
                option: "bob".into(),
            }))
        );
        // `m` opens the field-list editor.
        assert_eq!(
            vm.handle_key(key(KeyCode::Char('m')), &ctx),
            Some(Action::PromptGroupFields {
                selector: "creds".into()
            })
        );
        // `o` starts a new option in the ghost row, in place.
        assert!(vm.handle_key(key(KeyCode::Char('o')), &ctx).is_none());
        let edit = vm.grid.editing.as_ref().expect("ghost row is live");
        assert_eq!((edit.row, edit.col), (2, 0));
        assert_eq!(edit.input.text(), "");

        // …and every one of them yields to a cell edit in progress.
        assert_eq!(vm.handle_key(key(KeyCode::Char('o')), &ctx), None);
        assert_eq!(vm.handle_key(key(KeyCode::Char('m')), &ctx), None);
        assert_eq!(vm.handle_key(key(KeyCode::Char(' ')), &ctx), None);
    }

    #[test]
    fn start_cell_edit_seeds_the_cells_current_text() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        select_group(&mut vm, &ctx, "creds");
        vm.start_cell_edit(&ctx, 0, 2);
        let edit = vm.grid.editing.as_ref().unwrap();
        assert_eq!(edit.input.text(), "c-77");
        assert_eq!(edit.original, "c-77");
        assert_eq!(vm.grid.cursor, (0, 2));

        // A ghost-row click always lands in the name cell: there is no
        // option yet for a value to belong to.
        vm.start_cell_edit(&ctx, 2, 2);
        let edit = vm.grid.editing.as_ref().unwrap();
        assert_eq!((edit.row, edit.col), (2, 0));
        assert_eq!(edit.input.text(), "");
    }

    #[test]
    fn the_entry_row_menu_offers_edit_duplicate_rename_delete() {
        let (_dir, ctx) = fixture();
        let mut vm = VarManager::default();
        select_group(&mut vm, &ctx, "creds");
        let items = vm.entry_context_menu(&ctx, 1).expect("option menu");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Edit\u{2026}",
                "Duplicate option",
                "Rename\u{2026}",
                "Delete\u{2026}"
            ]
        );
        let Some(Action::OpenEditOptionPrompt {
            owner, key, values, ..
        }) = &items[0].action
        else {
            panic!("Edit… opens the option edit prompt: {:?}", items[0].action)
        };
        assert_eq!((owner.as_str(), key.as_str()), ("creds", "bob"));
        assert_eq!(values["user_id"], "2002");
        assert_eq!(
            items[1].action,
            Some(Action::VarStruct(VarStructOp::DuplicateOption {
                env: "qa".into(),
                selector: "creds".into(),
                name: "bob".into(),
            }))
        );
        assert_eq!(
            items[2].action,
            Some(Action::PromptRenameEntry {
                env: "qa".into(),
                selector: "creds".into(),
                from: "bob".into(),
            })
        );
        assert_eq!(
            items[3].action,
            Some(Action::ConfirmDeleteEntry {
                env: "qa".into(),
                selector: "creds".into(),
                name: "bob".into(),
            })
        );
        assert!(
            vm.entry_context_menu(&ctx, 2).is_none(),
            "the ghost row has nothing to act on yet"
        );
    }
}
