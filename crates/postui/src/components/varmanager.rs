use super::line_input::LineInput;
use crate::action::Action;
use crate::hit::HitMap;
use crate::paint::{fill, text};
use crate::project_ctx::ProjectContext;
use crate::theme::Theme;
use indexmap::IndexMap;
use postui_core::model::HttpRequest;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use std::collections::BTreeSet;

/// In-progress edit of a single grid cell (spec §5). `row` indexes
/// `VarManager::rows`; `col` matches the cursor's — 0 is the shared
/// name/desc block, 1 is the Default column, 2.. are environment
/// columns. `masked` is set for a
/// secret cell's value: the typed text renders as `●` per char
/// ([`LineInput::draw_line_masked`]) and the secret string itself is
/// never toasted.
#[derive(Debug, Clone)]
pub struct CellEdit {
    pub row: usize,
    pub col: usize,
    pub input: LineInput,
    pub masked: bool,
}

/// A committed cell edit, dispatched as `Action::VarEdit` and applied by
/// `App` (spec §5: every commit writes atomically and immediately to
/// whichever file owns it; a write failure toasts and leaves the cell in
/// edit rather than losing the typed text).
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
    /// A variable's or group's shared `description` in `variables.toml`.
    SetDescription { owner: String, value: String },
    /// A secret variable's value in `env` — `ctx.set_secret`, which writes
    /// `.local/secrets.toml` (never `variables.toml`/the env file).
    SetSecretValue {
        env: String,
        name: String,
        value: String,
    },
    /// One field of one entry: the edit lands in `env`'s
    /// `[entries.owner.key]` table (`varedit::upsert_entry`), the only
    /// place an entry's values live (spec §3.1). `member` names the field;
    /// `None` has nothing to write and is refused.
    SetOptionValue {
        env: String,
        owner: String,
        key: String,
        member: Option<String>,
        value: String,
    },
    /// A request-scoped `[variables]` entry's value on the open request —
    /// mutates `Editor::variables` directly and rides the editor's existing
    /// dirty/save path (no immediate write of its own).
    SetRequestVar { name: String, value: String },
    /// Records `name`'s selection as `key` for `env` — `ctx.set_selection`
    /// (the ✓ action; also the picker's confirm in Task 14).
    Select {
        env: String,
        name: String,
        key: String,
    },
}

/// The title bar's height, matching the header/footer's 3-row painted
/// rhythm (a blank panel row, the content row, a blank panel row).
pub const TITLE_HEIGHT: u16 = 3;
/// The footer hint row's height: a single line, since the Manager already
/// carries its own title bar and doesn't need the app footer's blank
/// panel padding rows around it.
pub const HINT_HEIGHT: u16 = 1;

/// A row of the Variable Manager grid (spec §5). Structural only — this
/// says *which* row it is, not its per-environment cell content (that's
/// computed at draw time against each visible environment column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// "This request" / "Project".
    SectionHeader(&'static str),
    /// A request-scoped `[variables]` override on the open request.
    RequestVar {
        name: String,
    },
    /// A declared variable: simple or secret alike.
    Var {
        name: String,
    },
    GroupHeader {
        name: String,
    },
    /// Indented under its `GroupHeader`.
    GroupMember {
        group: String,
        name: String,
    },
    /// An entry sub-row, indented under an expanded `GroupHeader`.
    /// Temporary shape for the stage-7 model — the whole Manager is
    /// replaced by the master-detail layout (spec §3.4) shortly.
    EntryRow {
        group: String,
        entry: String,
    },
    /// Ghost action rows at the end of the project section.
    AddVar,
    AddGroup,
}

/// A structural mutation dispatched by the Variable Manager (spec §5
/// action list; §4 promote/demote; §3 secret flag transitions). Unlike
/// [`VarEditOp`] (one cell's value), these add/remove/rename/reshape
/// declarations — each still applies through `ctx.edit_variables`/
/// `edit_env`/the request editor, per `App::apply_var_struct`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarStructOp {
    /// `n`: a bare new variable declaration (no default yet — the grid's
    /// existing cell edit sets it afterward).
    NewVar {
        name: String,
        description: Option<String>,
    },
    /// `g`: a new group (or, reused for `m`'s member edit, an update to an
    /// existing one — `varedit::upsert_group` is create-or-update either
    /// way).
    NewGroup { name: String, members: Vec<String> },
    /// `o` on a group/entry row: a new entry, written to the active
    /// environment (`varedit::upsert_entry`).
    NewOption {
        owner: String,
        key: String,
        description: Option<String>,
        values: IndexMap<String, String>,
    },
    /// `e`/`F2`/`=` (or double-click on the name) on a `Var` row
    /// (variables only — see the module doc on why groups aren't
    /// renameable this task).
    Rename { from: String, to: String },
    /// `d`/`Delete` on a `Var` or `GroupHeader` row.
    Delete { name: String },
    /// `d`/`Delete` on an `EntryRow`: removes that entry from the active
    /// environment (`App::apply_delete_option`), the only environment it
    /// lives in.
    DeleteOption { owner: String, key: String },
    /// `s` on a `Var` row: flips `secret` (spec §3's two transitions —
    /// `App::apply_var_struct` does the value-moving work).
    ToggleSecret { name: String },
    /// `m` on a `GroupHeader` row: replaces the group's field list.
    SetMembers { group: String, members: Vec<String> },
    /// `p` on a `RequestVar` row (spec §4).
    Promote {
        name: String,
        target: postui_core::varedit::PromoteTarget,
    },
    /// `P` on a `Var` row (spec §4); refused for secret names and groups
    /// before this is ever dispatched.
    Demote { name: String },
}

/// The structural action a key/chip asks for — see
/// `VarManager::struct_action_target`. Not the wire type ([`VarStructOp`]
/// is that, built once the modal collecting the rest of its fields
/// confirms); this only picks *which* modal/confirm to open for the
/// cursor's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StructKind {
    NewOption,
    Rename,
    Delete,
    ToggleSecret,
    EditMembers,
    AddMember,
    Promote,
    Demote,
    /// `e` on an `EntryRow`: the multi-field value(s)/description prompt
    /// (the same one the picker's `e` opens), prefilled from the ACTIVE
    /// env's entry.
    EditOptionPrompt,
}

/// The full-frame Variable Manager screen (spec §5): a title bar, the
/// read-only variable/environment grid, and a footer hint row. Editing
/// (Task 11) will extend `handle_key` and add mutation actions; this task
/// only renders the resolution truth.
#[derive(Debug, Default)]
pub struct VarManager {
    /// Rebuilt from `&ProjectContext` (+ the open request + `expanded`) at
    /// the top of every `draw` call.
    pub rows: Vec<RowKind>,
    /// `(row, col)`: `row` indexes `rows`; `col` 0 is the shared name/desc
    /// block, `col` 1 is the Default column, `col` 2.. are environment
    /// columns (relative to `env_scroll`).
    pub cursor: (usize, usize),
    /// Index into the project's environment list: the first visible env
    /// column.
    pub env_scroll: usize,
    /// First visible row's index into `rows`.
    pub scroll: usize,
    /// Names (variable or group) whose option sub-rows are shown.
    pub expanded: BTreeSet<String>,
    /// Set by a keyboard move (arrows/Tab) so the next `draw` snaps `scroll`
    /// to keep `cursor.0` visible; a mouse click or wheel scroll never sets
    /// it, since those gestures place the viewport (or cursor) explicitly.
    /// Cleared once `draw` has applied it. Mirrors `Sidebar::ensure_visible`.
    pub ensure_visible: bool,
    /// The row list's height as of the last `draw` — `move_row`'s and
    /// `draw`'s snap-into-view math must agree, so `draw` records it here
    /// rather than each recomputing it independently.
    pub visible_rows: usize,
    /// How many environment columns fit as of the last `draw` — used the
    /// same way as `visible_rows`, for horizontal cursor movement.
    pub env_capacity: usize,
    /// The in-progress edit of one cell, if any (spec §5). `Esc` with this
    /// `Some` cancels the edit and is swallowed (the first `Esc` "eats" —
    /// closing the screen needs a second, now-`None` press).
    pub editing: Option<CellEdit>,
    /// The one secret cell currently shown in plaintext instead of `●`s
    /// (`(variable name, environment name)`), toggled by `r` on a secret
    /// cell without entering edit. At most one at a time.
    pub revealed: Option<(String, String)>,
}

/// Builds the grid's row list (order, not content): the request-scope
/// section (only when `open_request` carries any `[variables]`), then the
/// project section — declared variables in file order, then groups (a
/// header row followed by its indented members) in file order — each
/// var/group's option rows spliced in right after it when its name is in
/// `expanded`, and finally the two ghost action rows.
pub fn build_rows(
    ctx: &ProjectContext,
    open_request: Option<&HttpRequest>,
    expanded: &BTreeSet<String>,
) -> Vec<RowKind> {
    let mut rows = Vec::new();

    if let Some(req) = open_request
        && !req.variables.is_empty()
    {
        rows.push(RowKind::SectionHeader("This request"));
        for name in req.variables.keys() {
            rows.push(RowKind::RequestVar { name: name.clone() });
        }
    }

    rows.push(RowKind::SectionHeader("Project"));

    // Group fields are rendered under their `GroupHeader`, not as their
    // own top-level `Var` row, even though a field may also have its own
    // (value-less) table in `model.vars`.
    let group_fields: std::collections::HashSet<&str> = ctx
        .model
        .groups
        .values()
        .flat_map(|g| g.fields.iter().map(String::as_str))
        .collect();

    for name in ctx.model.vars.keys() {
        if group_fields.contains(name.as_str()) {
            continue;
        }
        rows.push(RowKind::Var { name: name.clone() });
    }

    for (group_name, decl) in &ctx.model.groups {
        rows.push(RowKind::GroupHeader {
            name: group_name.clone(),
        });
        if expanded.contains(group_name) {
            for entry in union_entry_names(ctx, group_name) {
                rows.push(RowKind::EntryRow {
                    group: group_name.clone(),
                    entry,
                });
            }
        }
        for field in &decl.fields {
            rows.push(RowKind::GroupMember {
                group: group_name.clone(),
                name: field.clone(),
            });
        }
    }

    rows.push(RowKind::AddVar);
    rows.push(RowKind::AddGroup);

    rows
}

/// `env`'s data: the active environment's is read straight off `ctx`
/// (already loaded); any other environment is loaded fresh. A missing or
/// unparseable file degrades to empty rather than erroring — the Manager
/// is a read-only truth display, and a broken environment is surfaced
/// elsewhere (toasts on load), not here.
fn env_data_for(ctx: &ProjectContext, env: &str) -> postui_core::varmodel::EnvData {
    if ctx.active_env.as_deref() == Some(env) {
        ctx.env_data.clone()
    } else {
        postui_core::project::load_environment(&ctx.root, env).unwrap_or_default()
    }
}

/// The union of `group`'s entry names across every environment — entries
/// belong to one environment each (spec §3.1), so a group's rows are the
/// union of what every environment declares for it. Environment-list
/// order; order doesn't matter for correctness (row content is looked up
/// by name), only determinism.
pub(crate) fn union_entry_names(ctx: &ProjectContext, group: &str) -> Vec<String> {
    let mut seen: IndexMap<String, ()> = IndexMap::new();
    for env in &ctx.environments {
        let env_data = env_data_for(ctx, env);
        if let Some(entries) = postui_core::varmodel::group_entries(&env_data, group) {
            for name in entries.keys() {
                seen.entry(name.clone()).or_insert(());
            }
        }
    }
    seen.into_keys().collect()
}

/// Fixed column widths (spec §5: "Fixed left columns: name, description;
/// then one column per environment side by side" — plus the shared
/// Default column between them, `ENV_W` wide like the env columns).
const NAME_W: u16 = 20;
const DESC_W: u16 = 20;
const ENV_W: u16 = 26;

const GLYPH_EXPANDED: &str = "\u{2304}"; // ⌄
const GLYPH_COLLAPSED: &str = "\u{203a}"; // ›

/// One environment's data, resolved once per `draw` call and reused for
/// every row's cell in that column (spec's per-env resolution truth).
/// Built by `env_column`: the active environment's is read straight off
/// `ctx` (already loaded/resolved); any other environment is loaded and
/// resolved fresh here. Cheap: `variables.toml` and `environments/*.toml`
/// are tiny, and this runs once per draw, not once per cell.
struct EnvColumn<'a> {
    name: &'a str,
    env_data: postui_core::varmodel::EnvData,
    resolved: postui_core::varmodel::Resolved,
    selections: &'a IndexMap<String, String>,
}

fn env_column<'a>(ctx: &'a ProjectContext, name: &'a str) -> EnvColumn<'a> {
    let selections = ctx.selections_for(name);
    if ctx.active_env.as_deref() == Some(name) {
        return EnvColumn {
            name,
            env_data: ctx.env_data.clone(),
            resolved: ctx.resolved.clone(),
            selections,
        };
    }
    let env_data = env_data_for(ctx, name);
    let empty_secrets = IndexMap::new();
    let secrets = ctx.secrets.get(name).unwrap_or(&empty_secrets);
    let resolved = postui_core::varmodel::resolve_env(&ctx.model, &env_data, selections, secrets);
    EnvColumn {
        name,
        env_data,
        resolved,
        selections,
    }
}

/// One rendered cell: its text and foreground color.
struct Cell {
    text: String,
    fg: Color,
}

impl Cell {
    fn blank() -> Self {
        Cell {
            text: String::new(),
            fg: Color::Reset,
        }
    }
}

/// Computes an env column's cell for `row`. `theme` supplies the palette;
/// `open_request` is only consulted for `RequestVar` rows. `revealed` is
/// the one secret cell (`(variable name, environment name)`) currently
/// shown in plaintext instead of `●`s, toggled by `r` (spec §5) — `None`
/// for every other draw.
/// The Default column's cell (col 1): a `Var`'s shared `default` from
/// `variables.toml`, or a request var's single value. Blank for every
/// other row — groups, options, and members have no shared flat value,
/// and a secret's value never lives in `variables.toml`.
fn default_cell(
    ctx: &ProjectContext,
    row: &RowKind,
    open_request: Option<&HttpRequest>,
    theme: &Theme,
) -> Cell {
    match row {
        RowKind::Var { name } => {
            let Some(decl) = ctx.model.vars.get(name) else {
                return Cell::blank();
            };
            if decl.secret {
                return Cell::blank();
            }
            match &decl.default {
                Some(v) => Cell {
                    text: v.clone(),
                    fg: theme.text,
                },
                None => Cell {
                    text: "\u{2014}".into(),
                    fg: theme.text_muted,
                },
            }
        }
        RowKind::RequestVar { name } => {
            let Some(entry) = open_request.and_then(|r| r.variables.get(name)) else {
                return Cell::blank();
            };
            let fg = if entry.enabled {
                theme.text
            } else {
                theme.text_muted
            };
            Cell {
                text: entry.value.clone(),
                fg,
            }
        }
        _ => Cell::blank(),
    }
}

fn env_cell(
    ctx: &ProjectContext,
    row: &RowKind,
    col: &EnvColumn,
    theme: &Theme,
    revealed: Option<&(String, String)>,
) -> Cell {
    use postui_core::varmodel::VarMeta;

    match row {
        RowKind::SectionHeader(_) | RowKind::AddVar | RowKind::AddGroup => Cell::blank(),

        // A request var's single value renders in the Default column
        // (`default_cell`); a muted dash in each env cell marks that it
        // has no per-env variants to set.
        RowKind::RequestVar { .. } => Cell {
            text: "\u{2014}".into(),
            fg: theme.text_muted,
        },

        RowKind::Var { name } => {
            let Some(decl) = ctx.model.vars.get(name) else {
                return Cell::blank();
            };
            if decl.secret {
                return match col.resolved.meta.get(name) {
                    Some(VarMeta::Secret) => {
                        let shown = revealed.is_some_and(|(n, e)| n == name && e == col.name);
                        if shown {
                            Cell {
                                text: col.resolved.values.get(name).cloned().unwrap_or_default(),
                                fg: theme.text,
                            }
                        } else {
                            Cell {
                                text: "\u{25cf}\u{25cf}\u{25cf}\u{25cf}".into(),
                                fg: theme.text,
                            }
                        }
                    }
                    _ => Cell {
                        text: "\u{26a0} secret".into(),
                        fg: theme.warning,
                    },
                };
            }
            match col.resolved.values.get(name) {
                Some(v) => {
                    let is_default = !col.env_data.values.contains_key(name);
                    Cell {
                        text: v.clone(),
                        fg: if is_default {
                            theme.text_muted
                        } else {
                            theme.text
                        },
                    }
                }
                None => Cell {
                    text: "\u{2014}".into(),
                    fg: theme.text_muted,
                },
            }
        }

        RowKind::GroupHeader { name } => {
            let Some(entries) = postui_core::varmodel::group_entries(&col.env_data, name) else {
                return Cell::blank();
            };
            if entries.is_empty() {
                return Cell::blank();
            }
            match col.selections.get(name) {
                Some(entry) if entries.contains_key(entry) => Cell {
                    text: entry.clone(),
                    fg: theme.text,
                },
                _ => Cell {
                    text: "\u{26a0} select".into(),
                    fg: theme.warning,
                },
            }
        }

        RowKind::GroupMember { name, .. } => match col.resolved.meta.get(name) {
            Some(VarMeta::GroupMember { selected, .. }) => {
                let value = col.resolved.values.get(name).cloned().unwrap_or_default();
                Cell {
                    text: format!("{selected} \u{b7} {value}"),
                    fg: theme.text,
                }
            }
            _ => Cell {
                text: "\u{26a0} select".into(),
                fg: theme.warning,
            },
        },

        RowKind::EntryRow { group, entry } => {
            let Some(decl) = postui_core::varmodel::group_entries(&col.env_data, group)
                .and_then(|entries| entries.get(entry))
            else {
                return Cell::blank();
            };
            let mut text = decl
                .values
                .iter()
                .map(|(f, v)| format!("{f}={v}"))
                .collect::<Vec<_>>()
                .join(", ");
            if col.selections.get(group) == Some(entry) {
                text = format!("\u{2713} {text}");
            }
            Cell {
                text,
                fg: theme.text,
            }
        }
    }
}

/// The name-column glyph (expand/collapse) for a row that owns option
/// sub-rows, or `None` for rows that never expand.
fn expand_glyph(
    ctx: &ProjectContext,
    row: &RowKind,
    expanded: &BTreeSet<String>,
) -> Option<&'static str> {
    let RowKind::GroupHeader { name } = row else {
        return None;
    };
    if union_entry_names(ctx, name).is_empty() {
        return None;
    }
    Some(if expanded.contains(name) {
        GLYPH_EXPANDED
    } else {
        GLYPH_COLLAPSED
    })
}

/// The context-sensitive action chips for the Manager's hint row (spec §5:
/// "every mutation ... has a keyboard action and a painted button"),
/// registered as `Hit::FooterChip` — the same generic click-dispatch the
/// main footer's chips already use, so no new hit-routing code is needed.
/// `n`/`g` (new variable/new group) are always offered; the rest come from
/// [`VarManager::struct_action_target`] against the cursor's row, so a key
/// and its chip can never disagree about which rows accept it.
fn action_chips(
    ctx: &ProjectContext,
    row: Option<&RowKind>,
) -> Vec<(&'static str, &'static str, Action)> {
    let mut chips = vec![
        ("n", "new var", Action::PromptNewVar),
        ("g", "new group", Action::PromptNewGroup),
    ];
    let Some(row) = row else { return chips };
    for (key, label, kind) in [
        ("o", "add option", StructKind::NewOption),
        ("e", "rename", StructKind::Rename),
        ("e", "edit", StructKind::EditOptionPrompt),
        ("d", "delete", StructKind::Delete),
        ("s", "secret", StructKind::ToggleSecret),
        ("a", "member", StructKind::AddMember),
        ("m", "members", StructKind::EditMembers),
        ("p", "promote", StructKind::Promote),
        ("P", "demote", StructKind::Demote),
    ] {
        if let Some(action) = VarManager::struct_action_target(row, kind, ctx) {
            chips.push((key, label, action));
        }
    }
    chips
}

/// The name column's indent (in cells) and label for `row`.
fn name_and_indent(row: &RowKind) -> (u16, String) {
    match row {
        RowKind::SectionHeader(s) => (0, s.to_string()),
        RowKind::RequestVar { name } => (0, name.clone()),
        RowKind::Var { name } => (0, name.clone()),
        RowKind::GroupHeader { name } => (0, name.clone()),
        RowKind::GroupMember { name, .. } => (2, name.clone()),
        RowKind::EntryRow { entry, .. } => (4, entry.clone()),
        RowKind::AddVar => (0, "+ Add variable".to_string()),
        RowKind::AddGroup => (0, "+ Add group".to_string()),
    }
}

/// The description column's text for `row` (blank for rows with none).
fn description_for(ctx: &ProjectContext, row: &RowKind) -> String {
    match row {
        RowKind::Var { name } => ctx
            .model
            .vars
            .get(name)
            .and_then(|d| d.description.clone())
            .unwrap_or_default(),
        RowKind::GroupHeader { name } => ctx
            .model
            .groups
            .get(name)
            .and_then(|g| g.description.clone())
            .unwrap_or_default(),
        RowKind::EntryRow { group, entry } => {
            postui_core::varmodel::group_entries(&ctx.env_data, group)
                .and_then(|entries| entries.get(entry))
                .and_then(|e| e.description.clone())
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// `row`'s name, when clicking/Entering it (on any column) should
/// toggle expand rather than edit a cell — a `Var`/`GroupHeader` that owns
/// option sub-rows ([`union_var_option_keys`]/[`union_group_option_keys`]
/// non-empty), matching [`expand_glyph`]'s own "has options" test.
/// `row`'s name, when clicking/Entering it (on any column) should toggle
/// expand rather than edit a cell — a `GroupHeader` that owns entry
/// sub-rows ([`union_entry_names`] non-empty), matching [`expand_glyph`]'s
/// own test.
fn expandable_name<'a>(ctx: &ProjectContext, row: &'a RowKind) -> Option<&'a str> {
    match row {
        RowKind::GroupHeader { name } if !union_entry_names(ctx, name).is_empty() => Some(name),
        _ => None,
    }
}

impl VarManager {
    /// Handles a key while the Manager screen is open. `App::handle_key`
    /// routes every key here once an open modal and a modified global
    /// shortcut (e.g. ctrl+p for the palette) have had first refusal, and
    /// swallows anything this returns `None` for rather than falling
    /// through to the global keymap — so, for instance, plain `q` does not
    /// quit the app from this screen.
    ///
    /// `Esc` with no cell under edit asks the app to leave the screen
    /// (`Action::CloseScreen`); with one, it cancels the edit instead and
    /// is swallowed — the first `Esc` "eats", closing the screen needs a
    /// second, now-`editing: None` press (spec §5's key model, verbatim in
    /// the task brief). Arrows/Tab move the cursor (skipping header/ghost
    /// rows vertically, landing on the nearest editable row); Enter begins
    /// an edit or toggles expand, depending on the cursor's cell; `Space`
    /// is the ✓ action on an option row; `r` toggles plaintext reveal on a
    /// secret cell without entering edit.
    pub fn handle_key(
        &mut self,
        ev: KeyEvent,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
    ) -> Option<Action> {
        if self.editing.is_some() {
            return match ev.code {
                KeyCode::Esc => {
                    self.editing = None;
                    None
                }
                KeyCode::Enter => self.commit_edit(ctx),
                _ => {
                    if let Some(edit) = self.editing.as_mut() {
                        edit.input.handle_key(ev);
                    }
                    None
                }
            };
        }

        match ev.code {
            KeyCode::Esc => Some(Action::CloseScreen),
            KeyCode::Up => {
                self.move_row(-1);
                None
            }
            KeyCode::Down => {
                self.move_row(1);
                None
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.move_col(-1, ctx);
                None
            }
            KeyCode::Right | KeyCode::Tab => {
                self.move_col(1, ctx);
                None
            }
            KeyCode::Enter => self.activate_cursor(ctx, open_request),
            KeyCode::Char(' ') => self.select_cursor(ctx),
            KeyCode::Char('r' | 'R') => {
                self.toggle_reveal(ctx);
                None
            }
            KeyCode::Char('n') => Some(Action::PromptNewVar),
            KeyCode::Char('g') => Some(Action::PromptNewGroup),
            KeyCode::Char('o') => self.struct_action(ctx, StructKind::NewOption),
            KeyCode::Char('e') | KeyCode::F(2) | KeyCode::Char('=') => self
                .struct_action(ctx, StructKind::Rename)
                .or_else(|| self.struct_action(ctx, StructKind::EditOptionPrompt)),
            KeyCode::Char('d') | KeyCode::Delete => self.struct_action(ctx, StructKind::Delete),
            KeyCode::Char('s') => self.struct_action(ctx, StructKind::ToggleSecret),
            KeyCode::Char('m') => self.struct_action(ctx, StructKind::EditMembers),
            KeyCode::Char('a') => self.struct_action(ctx, StructKind::AddMember),
            KeyCode::Char('p') => self.struct_action(ctx, StructKind::Promote),
            KeyCode::Char('P') => self.struct_action(ctx, StructKind::Demote),
            _ => None,
        }
    }

    /// Whether `row` is a stop for vertical cursor movement — every row
    /// except the section headers and the trailing ghost action rows (spec
    /// §5's key model: "skip headers/ghost rows vertically into nearest
    /// editable").
    fn is_stop_row(row: &RowKind) -> bool {
        !matches!(
            row,
            RowKind::SectionHeader(_) | RowKind::AddVar | RowKind::AddGroup
        )
    }

    /// Moves `cursor.0` one stop row in `dir` (`-1`/`1`), skipping
    /// non-stop rows; stays put at either end of the list. Any editing
    /// state was already gone by the time this runs (`handle_key` routes
    /// arrows to the editing input instead while `editing.is_some()`).
    fn move_row(&mut self, dir: i32) {
        if self.rows.is_empty() {
            return;
        }
        let mut i = self.cursor.0 as i32;
        loop {
            i += dir;
            if i < 0 || i as usize >= self.rows.len() {
                return;
            }
            if Self::is_stop_row(&self.rows[i as usize]) {
                self.cursor.0 = i as usize;
                self.ensure_visible = true;
                return;
            }
        }
    }

    /// Moves `cursor.1` one column in `dir` (`-1`/`1`), clamped to
    /// `[0, 1 + environments.len()]` (0 = name/desc block, 1 = Default,
    /// 2.. = envs), snapping `env_scroll` to keep the target env column
    /// visible (mirrors `move_row`'s row snap, horizontally).
    fn move_col(&mut self, dir: i32, ctx: &ProjectContext) {
        let max = 1 + ctx.environments.len() as i32;
        let next = (self.cursor.1 as i32 + dir).clamp(0, max);
        self.cursor.1 = next as usize;
        if self.cursor.1 >= 2 && self.env_capacity > 0 {
            let abs = self.cursor.1 - 2;
            if abs < self.env_scroll {
                self.env_scroll = abs;
            } else if abs >= self.env_scroll + self.env_capacity {
                self.env_scroll = abs + 1 - self.env_capacity;
            }
        }
    }

    /// The environment name `cursor.1 == col` addresses, or `None` for
    /// `col == 0` (the shared name/desc block) and `col == 1` (the shared
    /// Default column).
    fn env_at(&self, ctx: &ProjectContext, col: usize) -> Option<String> {
        if col <= 1 {
            return None;
        }
        ctx.environments.get(self.env_scroll + col - 2).cloned()
    }

    fn toggle_expand(&mut self, name: &str) {
        if !self.expanded.remove(name) {
            self.expanded.insert(name.to_string());
        }
    }

    fn begin_edit(&mut self, col: usize, seed: &str, masked: bool) {
        self.editing = Some(CellEdit {
            row: self.cursor.0,
            col,
            input: LineInput::new(seed),
            masked,
        });
    }

    /// `Enter` (or a click-selected-again) on the cursor's current cell:
    /// toggles expand for a `GroupHeader` with entries, else begins
    /// editing whichever value that cell shows (masked for a secret), else
    /// (a cell with nothing to edit — `GroupMember`, ghost rows, an
    /// `OptionRow`'s shared name column, a group `OptionRow`'s per-member
    /// value) does nothing.
    pub fn activate_cursor(
        &mut self,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
    ) -> Option<Action> {
        let row = self.rows.get(self.cursor.0)?.clone();
        let col = self.cursor.1;

        // Expand/collapse is the *name column's* activation; an env cell
        // of the same expandable row offers the selection dropdown below.
        if col == 0
            && let Some(name) = expandable_name(ctx, &row)
        {
            self.toggle_expand(name);
            return None;
        }

        match &row {
            RowKind::Var { name } => {
                let decl = ctx.model.vars.get(name)?;
                if col == 0 {
                    let seed = decl.description.clone().unwrap_or_default();
                    self.begin_edit(0, &seed, false);
                    return None;
                }
                if col == 1 {
                    // The shared default lives in variables.toml, where a
                    // secret's value must never be written — no edit.
                    if decl.secret {
                        return None;
                    }
                    let seed = decl.default.clone().unwrap_or_default();
                    self.begin_edit(1, &seed, false);
                    return None;
                }
                let env = self.env_at(ctx, col)?;
                let column = env_column(ctx, &env);
                let seed = column
                    .resolved
                    .values
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                self.begin_edit(col, &seed, decl.secret);
                None
            }
            RowKind::GroupHeader { name } => {
                if col == 0 {
                    // Only a group with no entries reaches here (expand
                    // claimed the rest): edit its shared description.
                    let seed = ctx
                        .model
                        .groups
                        .get(name)
                        .and_then(|g| g.description.clone())
                        .unwrap_or_default();
                    self.begin_edit(0, &seed, false);
                    return None;
                }
                if col == 1 {
                    return None;
                }
                let env = self.env_at(ctx, col)?;
                let column = env_column(ctx, &env);
                if postui_core::varmodel::group_entries(&column.env_data, name)
                    .is_none_or(indexmap::IndexMap::is_empty)
                {
                    return None;
                }
                Some(Action::OpenSelectDropdown {
                    owner: name.clone(),
                    env,
                    row: self.cursor.0,
                    col,
                })
            }
            RowKind::GroupMember { group, name } => {
                if col <= 1 {
                    return None;
                }
                let env = self.env_at(ctx, col)?;
                let column = env_column(ctx, &env);
                let entries =
                    postui_core::varmodel::group_entries(&column.env_data, group).cloned();
                let entries = entries.filter(|e| !e.is_empty())?;
                let Some(selected) = column
                    .selections
                    .get(group)
                    .filter(|k| entries.contains_key(*k))
                else {
                    // No option chosen for this env yet — offer the choice
                    // instead of an edit that has no target.
                    return Some(Action::OpenSelectDropdown {
                        owner: group.clone(),
                        env,
                        row: self.cursor.0,
                        col,
                    });
                };
                let seed = entries
                    .get(selected)
                    .and_then(|e| e.values.get(name))
                    .cloned()
                    .unwrap_or_default();
                self.begin_edit(col, &seed, false);
                None
            }
            RowKind::RequestVar { name } => {
                // A request var is one flat value (no per-env variants):
                // it shows and edits in the Default column only.
                if col != 1 {
                    return None;
                }
                let seed = open_request
                    .and_then(|r| r.variables.get(name))
                    .map(|e| e.value.clone())
                    .unwrap_or_default();
                self.begin_edit(1, &seed, false);
                None
            }
            // An entry row carries one value per group field rather than
            // one flat value the grid's single-line cell could edit as
            // free text; selecting the entry is still reachable via
            // `Space`/click (`select_cursor`). The entries table (spec
            // §3.4) replaces this row shortly.
            RowKind::EntryRow { .. } => None,
            _ => None,
        }
    }

    /// `Space` (or a click) on an entry row's cell: the ✓ action —
    /// records `entry` as `group`'s selection for that column's
    /// environment. A no-op anywhere else (col 0, or a row that isn't an
    /// `EntryRow`).
    pub fn select_cursor(&mut self, ctx: &ProjectContext) -> Option<Action> {
        let row = self.rows.get(self.cursor.0)?;
        let RowKind::EntryRow { group, entry } = row else {
            return None;
        };
        let env = self.env_at(ctx, self.cursor.1)?;
        Some(Action::VarEdit(VarEditOp::Select {
            env,
            name: group.clone(),
            key: entry.clone(),
        }))
    }

    /// Which structural action a key/chip is asking for — routed through
    /// [`Self::struct_action`] so the keyboard and the footer chip
    /// ([`action_chips`]) share exactly one "is this row a valid target"
    /// gate rather than risking the two disagreeing.
    fn struct_action_target(
        row: &RowKind,
        kind: StructKind,
        ctx: &ProjectContext,
    ) -> Option<Action> {
        match (kind, row) {
            (
                StructKind::NewOption,
                RowKind::GroupHeader { name } | RowKind::EntryRow { group: name, .. },
            ) => Some(Action::PromptNewOption {
                owner: name.clone(),
            }),
            (StructKind::Rename, RowKind::Var { name }) => {
                Some(Action::PromptRenameVar { from: name.clone() })
            }
            (StructKind::Delete, RowKind::Var { name } | RowKind::GroupHeader { name }) => {
                Some(Action::ConfirmDeleteVar { name: name.clone() })
            }
            (StructKind::Delete, RowKind::GroupMember { group, name }) => {
                Some(Action::ConfirmRemoveGroupMember {
                    group: group.clone(),
                    member: name.clone(),
                })
            }
            (
                StructKind::AddMember,
                RowKind::GroupHeader { name: group } | RowKind::GroupMember { group, .. },
            ) => Some(Action::PromptAddGroupMember {
                group: group.clone(),
            }),
            (StructKind::Delete, RowKind::EntryRow { group, entry }) => {
                Some(Action::ConfirmDeleteOption {
                    owner: group.clone(),
                    key: entry.clone(),
                })
            }
            (StructKind::ToggleSecret, RowKind::Var { name }) => {
                Some(Action::ToggleSecretVar { name: name.clone() })
            }
            (StructKind::EditOptionPrompt, RowKind::EntryRow { group, entry }) => {
                postui_core::varmodel::group_entries(&ctx.env_data, group)
                    .and_then(|entries| entries.get(entry))
                    .map(|decl| Action::OpenEditOptionPrompt {
                        owner: group.clone(),
                        key: entry.clone(),
                        description: decl.description.clone(),
                        values: decl.values.clone(),
                    })
            }
            (StructKind::EditMembers, RowKind::GroupHeader { name }) => {
                Some(Action::PromptEditGroupMembers {
                    group: name.clone(),
                })
            }
            (StructKind::Promote, RowKind::RequestVar { name }) => {
                Some(Action::PromptPromoteVar { name: name.clone() })
            }
            (StructKind::Demote, RowKind::Var { name }) => {
                Some(Action::ConfirmDemoteVar { name: name.clone() })
            }
            _ => None,
        }
    }

    /// The cursor-row version of [`Self::struct_action_target`] — the
    /// keyboard path.
    fn struct_action(&self, ctx: &ProjectContext, kind: StructKind) -> Option<Action> {
        let row = self.rows.get(self.cursor.0)?;
        Self::struct_action_target(row, kind, ctx)
    }

    /// `r` on a secret `Var`'s env cell: toggles that one `(name, env)`
    /// pair between masked and plaintext display, replacing any other cell
    /// currently revealed (spec §5: `r` toggles reveal *without* entering
    /// edit). A no-op on any other cell.
    fn toggle_reveal(&mut self, ctx: &ProjectContext) {
        let Some(RowKind::Var { name }) = self.rows.get(self.cursor.0) else {
            return;
        };
        if !ctx.model.vars.get(name).is_some_and(|d| d.secret) {
            return;
        }
        let Some(env) = self.env_at(ctx, self.cursor.1) else {
            return;
        };
        let pair = (name.clone(), env);
        self.revealed = if self.revealed.as_ref() == Some(&pair) {
            None
        } else {
            Some(pair)
        };
    }

    /// `Enter` while a cell is under edit: builds the `VarEditOp` its kind
    /// and column call for and dispatches `Action::VarEdit`. Does **not**
    /// clear `self.editing` itself — `App` does that only once the write
    /// actually succeeds (spec §5: a failed write "leave[s] the cell in
    /// edit" with the typed text intact, so a retry doesn't need retyping).
    fn commit_edit(&mut self, ctx: &ProjectContext) -> Option<Action> {
        let edit = self.editing.as_ref()?;
        let row = self.rows.get(edit.row)?.clone();
        let value = edit.input.text().to_string();
        let col = edit.col;

        let op = match &row {
            RowKind::Var { name } => {
                let decl = ctx.model.vars.get(name)?;
                if col == 0 {
                    VarEditOp::SetDescription {
                        owner: name.clone(),
                        value,
                    }
                } else if col == 1 {
                    VarEditOp::SetDefault {
                        name: name.clone(),
                        value,
                    }
                } else {
                    let env = self.env_at(ctx, col)?;
                    if decl.secret {
                        VarEditOp::SetSecretValue {
                            env,
                            name: name.clone(),
                            value,
                        }
                    } else {
                        VarEditOp::SetEnvValue {
                            env,
                            name: name.clone(),
                            value,
                        }
                    }
                }
            }
            RowKind::GroupHeader { name } if col == 0 => VarEditOp::SetDescription {
                owner: name.clone(),
                value,
            },
            RowKind::GroupMember { group, name } => {
                let env = self.env_at(ctx, col)?;
                let column = env_column(ctx, &env);
                let selected = column.selections.get(group)?.clone();
                VarEditOp::SetOptionValue {
                    env,
                    owner: group.clone(),
                    key: selected,
                    member: Some(name.clone()),
                    value,
                }
            }
            RowKind::RequestVar { name } => VarEditOp::SetRequestVar {
                name: name.clone(),
                value,
            },
            _ => return None,
        };
        Some(Action::VarEdit(op))
    }

    /// Mouse click routing for one cell (spec §5: "mouse click selects,
    /// ... click-selected-again edits in place"). Always moves the cursor
    /// to `(row, col)` first (and clears any *other* cell's edit — clicking
    /// away from an in-progress edit discards it, same as `Esc`). An
    /// option row's cell is its own ✓ checkbox (spec §5: "setting the
    /// selected option per env (✓ in the env column)"), so any click on it
    /// dispatches `Select` immediately rather than needing a second click;
    /// every other row's second click within the double-click window
    /// (`double`) does what `Enter` would.
    pub fn click_cell(
        &mut self,
        row: usize,
        col: usize,
        double: bool,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
    ) -> Option<Action> {
        if self
            .editing
            .as_ref()
            .is_some_and(|e| e.row != row || e.col != col)
        {
            self.editing = None;
        }
        self.cursor = (row, col);
        self.ensure_visible = false;
        match self.rows.get(row).cloned() {
            // The ghost action rows render like buttons; a single click
            // anywhere on them acts immediately.
            Some(RowKind::AddVar) => return Some(Action::PromptNewVar),
            Some(RowKind::AddGroup) => return Some(Action::PromptNewGroup),
            Some(RowKind::EntryRow { .. }) => return self.select_cursor(ctx),
            // A choice cell acts on the first click too — it opens a
            // dropdown, like the method selector.
            Some(RowKind::GroupHeader { .. }) if col >= 2 => {
                return self.activate_cursor(ctx, open_request);
            }
            _ => {}
        }
        if double {
            return self.activate_cursor(ctx, open_request);
        }
        None
    }

    /// Mouse click on a row's name region (the first `NAME_W` cells —
    /// [`crate::hit::Hit::VarName`]). Single click moves the cursor to the
    /// row's col 0 (discarding any in-progress edit) — except on the ghost
    /// action rows, which act immediately like buttons. A double click
    /// expands an expandable row, or opens the rename prompt for a plain
    /// variable — the name's own "edit", mirroring `F2`.
    pub fn click_name(&mut self, row: usize, double: bool, ctx: &ProjectContext) -> Option<Action> {
        self.editing = None;
        let kind = self.rows.get(row)?.clone();
        self.cursor = (row, 0);
        self.ensure_visible = false;
        match &kind {
            RowKind::AddVar => return Some(Action::PromptNewVar),
            RowKind::AddGroup => return Some(Action::PromptNewGroup),
            _ => {}
        }
        if !double {
            return None;
        }
        if let Some(name) = expandable_name(ctx, &kind) {
            self.toggle_expand(name);
            return None;
        }
        Self::struct_action_target(&kind, StructKind::Rename, ctx)
    }

    /// Mouse click on a row's background (outside any specific cell):
    /// moves `cursor.0` there, keeping the current column, and discards
    /// any in-progress edit (same as clicking away from it in
    /// `click_cell`). The ghost action rows act immediately, whole-row.
    pub fn click_row(&mut self, row: usize) -> Option<Action> {
        if row < self.rows.len() {
            self.cursor.0 = row;
        }
        self.editing = None;
        self.ensure_visible = false;
        match self.rows.get(row) {
            Some(RowKind::AddVar) => Some(Action::PromptNewVar),
            Some(RowKind::AddGroup) => Some(Action::PromptNewGroup),
            _ => None,
        }
    }

    /// Free (unsnapped) wheel scroll over the grid — mirrors
    /// `Sidebar::handle_scroll`: moves the viewport without touching the
    /// cursor, and cancels any pending `ensure_visible` snap so the wheel
    /// gesture isn't immediately overridden on the next draw.
    pub fn handle_scroll(&mut self, delta: i16) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len().saturating_sub(1);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
        self.ensure_visible = false;
    }

    /// Paints the full-screen grid into `area`: a `theme.panel` title bar
    /// reading "Variables — `<project>` · `<env>`", a column-header strip,
    /// the variable/environment grid itself (rebuilt from `ctx` +
    /// `open_request` + `self.expanded` at the top of every call), and a
    /// muted footer hint row.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
        hits: &mut HitMap,
        hovered: Option<&crate::hit::Hit>,
    ) {
        self.rows = build_rows(ctx, open_request, &self.expanded);

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(TITLE_HEIGHT),
                Constraint::Min(0),
                Constraint::Length(HINT_HEIGHT),
            ])
            .split(area);
        let title_row = sections[0];
        let grid_area = sections[1];
        let hint_row = sections[2];

        let buf = frame.buffer_mut();

        fill(buf, title_row, theme.panel);
        if title_row.height > 0 {
            let mid_y = title_row.y + title_row.height / 2;
            let title = format!(
                "Variables \u{2014} {} \u{b7} {}",
                ctx.display_name(),
                ctx.env_label()
            );
            text(
                buf,
                title_row.x + 3,
                mid_y,
                &title,
                theme.text,
                theme.panel,
                true,
            );
        }

        fill(buf, grid_area, theme.page);
        if grid_area.height >= 1 && grid_area.width > 0 {
            let header_row = Rect {
                height: 1,
                ..grid_area
            };
            let list_area = Rect {
                y: grid_area.y + 1,
                height: grid_area.height.saturating_sub(1),
                ..grid_area
            };

            // How many env columns fit after the fixed name/desc/Default
            // block, and which ones (via env_scroll).
            let env_capacity =
                ((list_area.width.saturating_sub(NAME_W + DESC_W + ENV_W)) / ENV_W) as usize;
            self.env_capacity = env_capacity;
            self.visible_rows = list_area.height as usize;
            if self.ensure_visible {
                if self.visible_rows > 0 {
                    if self.cursor.0 < self.scroll {
                        self.scroll = self.cursor.0;
                    } else if self.cursor.0 >= self.scroll + self.visible_rows {
                        self.scroll = self.cursor.0 + 1 - self.visible_rows;
                    }
                    let max_scroll = self.rows.len().saturating_sub(self.visible_rows);
                    self.scroll = self.scroll.min(max_scroll);
                }
                self.ensure_visible = false;
            }
            let visible_envs: Vec<&String> = ctx
                .environments
                .iter()
                .skip(self.env_scroll)
                .take(env_capacity)
                .collect();
            let columns: Vec<EnvColumn> = visible_envs
                .iter()
                .map(|name| env_column(ctx, name.as_str()))
                .collect();

            // Column header strip.
            fill(buf, header_row, theme.panel);
            text(
                buf,
                header_row.x + 1,
                header_row.y,
                "Name",
                theme.text_muted,
                theme.panel,
                true,
            );
            text(
                buf,
                header_row.x + 1 + NAME_W,
                header_row.y,
                "Description",
                theme.text_muted,
                theme.panel,
                true,
            );
            text(
                buf,
                header_row.x + NAME_W + DESC_W,
                header_row.y,
                "Default",
                theme.text_muted,
                theme.panel,
                true,
            );
            for (i, col) in columns.iter().enumerate() {
                let x = header_row.x + NAME_W + DESC_W + ENV_W + (i as u16) * ENV_W;
                if x >= header_row.x + header_row.width {
                    break;
                }
                text(
                    buf,
                    x,
                    header_row.y,
                    col.name,
                    theme.text_muted,
                    theme.panel,
                    true,
                );
            }

            // Data rows.
            for (i, row) in self
                .rows
                .iter()
                .enumerate()
                .skip(self.scroll)
                .take(list_area.height as usize)
            {
                let y = list_area.y + (i - self.scroll) as u16;
                let row_rect = Rect {
                    x: list_area.x,
                    y,
                    width: list_area.width,
                    height: 1,
                };
                let row_fill = if i == self.cursor.0 {
                    theme.control_hover
                } else {
                    theme.page
                };
                fill(buf, row_rect, row_fill);
                hits.register(row_rect, crate::hit::Hit::VarRow(i));

                // The exact cursor cell gets its own, slightly stronger
                // fill on top of the row highlight, so the column under
                // keyboard/mouse focus reads distinctly from the rest of
                // the (also-highlighted) row.
                let cell_fill = |col: usize| -> Color {
                    if i == self.cursor.0 && col == self.cursor.1 {
                        theme.control_pressed
                    } else {
                        row_fill
                    }
                };

                // Col 0's cell highlight covers the description region
                // only — that's what its edit targets; the name region
                // stays on the row fill (its actions are rename/expand,
                // not a cell edit).
                let name_fill = row_fill;
                let desc_fill = cell_fill(0);
                if desc_fill != row_fill {
                    fill(
                        buf,
                        Rect {
                            x: list_area.x + NAME_W,
                            y,
                            width: DESC_W,
                            height: 1,
                        },
                        desc_fill,
                    );
                }
                // Two hit regions over one logical column: the name half
                // (rename/expand on double-click) and the description half
                // (edit the description on double-click).
                hits.register(
                    Rect {
                        x: list_area.x,
                        y,
                        width: NAME_W,
                        height: 1,
                    },
                    crate::hit::Hit::VarName(i),
                );
                hits.register(
                    Rect {
                        x: list_area.x + NAME_W,
                        y,
                        width: DESC_W,
                        height: 1,
                    },
                    crate::hit::Hit::VarCell { row: i, col: 0 },
                );

                let editing_here = self.editing.as_ref().filter(|e| e.row == i && e.col == 0);

                let is_header = matches!(row, RowKind::SectionHeader(_));
                let (indent, label) = name_and_indent(row);
                let mut x = list_area.x + 1 + indent;
                if let Some(glyph) = expand_glyph(ctx, row, &self.expanded) {
                    text(buf, x, y, glyph, theme.text_muted, name_fill, false);
                }
                x += 2;
                let name_fg = if is_header || matches!(row, RowKind::AddVar | RowKind::AddGroup) {
                    theme.text_muted
                } else {
                    theme.text
                };
                let name_w = (list_area.x + NAME_W).saturating_sub(x).saturating_sub(1);
                text(
                    buf,
                    x,
                    y,
                    super::chooser::clip(&label, name_w),
                    name_fg,
                    name_fill,
                    is_header,
                );

                // A col-0 edit is a *description* edit — the input lives in
                // the description column, and the name stays put.
                let desc_x = list_area.x + NAME_W;
                let desc_w = DESC_W.saturating_sub(1);
                if let Some(edit) = editing_here {
                    let mut line = edit.input.draw_line_windowed(true, theme, desc_w);
                    line.style = Style::default().bg(desc_fill).patch(line.style);
                    buf.set_line(desc_x, y, &line, desc_w);
                } else {
                    let desc = description_for(ctx, row);
                    if !desc.is_empty() {
                        text(
                            buf,
                            desc_x,
                            y,
                            super::chooser::clip(&desc, desc_w),
                            theme.text_muted,
                            desc_fill,
                            false,
                        );
                    }
                }

                // Default column (col 1): the shared project-level value.
                let dx = list_area.x + NAME_W + DESC_W;
                if dx < list_area.x + list_area.width {
                    let default_fill = cell_fill(1);
                    let default_rect = Rect {
                        x: dx,
                        y,
                        width: ENV_W,
                        height: 1,
                    };
                    if default_fill != row_fill {
                        fill(buf, default_rect, default_fill);
                    }
                    let w = ENV_W.saturating_sub(1);
                    let editing_default =
                        self.editing.as_ref().filter(|e| e.row == i && e.col == 1);
                    if let Some(edit) = editing_default {
                        let mut line = edit.input.draw_line_windowed(true, theme, w);
                        line.style = Style::default().bg(default_fill).patch(line.style);
                        buf.set_line(dx, y, &line, w);
                    } else {
                        let cell = default_cell(ctx, row, open_request, theme);
                        if !cell.text.is_empty() {
                            text(
                                buf,
                                dx,
                                y,
                                super::chooser::clip(&cell.text, w),
                                cell.fg,
                                default_fill,
                                false,
                            );
                        }
                    }
                    hits.register(default_rect, crate::hit::Hit::VarCell { row: i, col: 1 });
                }

                for (ci, col) in columns.iter().enumerate() {
                    let cx = list_area.x + NAME_W + DESC_W + ENV_W + (ci as u16) * ENV_W;
                    if cx >= list_area.x + list_area.width {
                        break;
                    }
                    let env_col = 2 + ci;
                    let this_fill = cell_fill(env_col);
                    let cell_rect = Rect {
                        x: cx,
                        y,
                        width: ENV_W,
                        height: 1,
                    };
                    if this_fill != row_fill {
                        fill(buf, cell_rect, this_fill);
                    }
                    let editing_here = self
                        .editing
                        .as_ref()
                        .filter(|e| e.row == i && e.col == env_col);
                    if let Some(edit) = editing_here {
                        let w = ENV_W.saturating_sub(1);
                        let mut line = if edit.masked {
                            edit.input.draw_line_windowed_masked(true, theme, w)
                        } else {
                            edit.input.draw_line_windowed(true, theme, w)
                        };
                        line.style = Style::default().bg(this_fill).patch(line.style);
                        buf.set_line(cx, y, &line, w);
                    } else {
                        let cell = env_cell(ctx, row, col, theme, self.revealed.as_ref());
                        if !cell.text.is_empty() {
                            let w = ENV_W.saturating_sub(1);
                            text(
                                buf,
                                cx,
                                y,
                                super::chooser::clip(&cell.text, w),
                                cell.fg,
                                this_fill,
                                false,
                            );
                        }
                    }
                    hits.register(
                        cell_rect,
                        crate::hit::Hit::VarCell {
                            row: i,
                            col: env_col,
                        },
                    );
                }
            }
        }

        fill(buf, hint_row, theme.panel);
        if hint_row.height > 0 {
            let y = hint_row.y;
            let esc_label = " esc back ";
            let esc_area = Rect {
                x: hint_row.x + 1,
                y,
                width: esc_label.chars().count() as u16,
                height: 1,
            };
            let esc_fill = if hovered == Some(&crate::hit::Hit::FooterChip(Action::CloseScreen)) {
                theme.control_hover
            } else {
                theme.panel
            };
            if esc_fill != theme.panel {
                fill(buf, esc_area, esc_fill);
            }
            text(
                buf,
                esc_area.x,
                y,
                esc_label,
                theme.text_muted,
                esc_fill,
                false,
            );
            hits.register(esc_area, crate::hit::Hit::FooterChip(Action::CloseScreen));

            let mut x = esc_area.x + esc_area.width + 1;
            let right_limit = hint_row.x + hint_row.width;
            for (key, label, action) in action_chips(ctx, self.rows.get(self.cursor.0)) {
                let key_text = format!(" {key}");
                let label_text = format!(" {label} ");
                let width = key_text.chars().count() as u16 + label_text.chars().count() as u16;
                if x + width > right_limit {
                    break;
                }
                let chip_area = Rect {
                    x,
                    y,
                    width,
                    height: 1,
                };
                let chip_fill = if hovered == Some(&crate::hit::Hit::FooterChip(action.clone())) {
                    theme.control_hover
                } else {
                    theme.control
                };
                fill(buf, chip_area, chip_fill);
                text(buf, x, y, &key_text, theme.accent, chip_fill, true);
                text(
                    buf,
                    x + key_text.chars().count() as u16,
                    y,
                    &label_text,
                    theme.text_muted,
                    chip_fill,
                    false,
                );
                hits.register(chip_area, crate::hit::Hit::FooterChip(action));
                x += width + 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use postui_core::model::Entry;
    use postui_core::project;
    use ratatui::crossterm::event::KeyModifiers;

    /// A project with: two envs (dev, qa); `base_url` simple with a
    /// default (no env value in dev → falls back; qa sets its own);
    /// `user` simple with a default; `api_key` secret, with a value in qa
    /// only; group `creds` with two fields and two entries in qa, the
    /// first selected there and nothing selected in dev. Plus a
    /// `trace_id` request-scope override on the returned `HttpRequest`.
    fn fixture() -> (tempfile::TempDir, ProjectContext, HttpRequest) {
        let dir = tempfile::tempdir().unwrap();
        project::init_project(dir.path(), Some("demo")).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[user]
description = "acting user"
default = "1001"

[api_key]
description = "service key"
secret = true

[groups.creds]
description = "paired ids"
fields = ["user_id", "customer_id"]
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "base_url = \"https://qa.example.com\"\n\n[entries.creds.alice]\ndescription = \"admin\"\nuser_id = \"1001\"\ncustomer_id = \"c-77\"\n\n[entries.creds.bob]\nuser_id = \"2002\"\ncustomer_id = \"c-91\"\n",
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

        let mut secrets = IndexMap::new();
        let mut qa_secrets = IndexMap::new();
        qa_secrets.insert("api_key".to_string(), "sk-qa-secret-value".to_string());
        secrets.insert("qa".to_string(), qa_secrets);
        project::save_secrets(dir.path(), &secrets).unwrap();

        let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
        assert!(warns.is_empty(), "{warns:?}");

        let mut req = HttpRequest {
            method: postui_core::model::Method::Get,
            url: "https://example.com".into(),
            substitute_body: false,
            params: IndexMap::new(),
            headers: IndexMap::new(),
            variables: IndexMap::new(),
            body: None,
        };
        req.variables.insert(
            "trace_id".to_string(),
            Entry {
                value: "abc-123".to_string(),
                enabled: true,
            },
        );
        req.variables.insert(
            "disabled_flag".to_string(),
            Entry {
                value: "should-be-muted".to_string(),
                enabled: false,
            },
        );

        (dir, ctx, req)
    }

    // -----------------------------------------------------------------
    // build_rows: structure/order
    // -----------------------------------------------------------------

    #[test]
    fn row_order_request_section_then_project_vars_then_groups_then_ghosts() {
        let (_dir, ctx, req) = fixture();
        let rows = build_rows(&ctx, Some(&req), &BTreeSet::new());
        assert_eq!(
            rows,
            vec![
                RowKind::SectionHeader("This request"),
                RowKind::RequestVar {
                    name: "trace_id".into()
                },
                RowKind::RequestVar {
                    name: "disabled_flag".into()
                },
                RowKind::SectionHeader("Project"),
                RowKind::Var {
                    name: "base_url".into()
                },
                RowKind::Var {
                    name: "user".into()
                },
                RowKind::Var {
                    name: "api_key".into()
                },
                RowKind::GroupHeader {
                    name: "creds".into()
                },
                RowKind::GroupMember {
                    group: "creds".into(),
                    name: "user_id".into()
                },
                RowKind::GroupMember {
                    group: "creds".into(),
                    name: "customer_id".into()
                },
                RowKind::AddVar,
                RowKind::AddGroup,
            ]
        );
    }

    #[test]
    fn no_request_section_when_open_request_has_no_variables() {
        let (_dir, ctx, _req) = fixture();
        let bare = HttpRequest {
            method: postui_core::model::Method::Get,
            url: "https://example.com".into(),
            substitute_body: false,
            params: IndexMap::new(),
            headers: IndexMap::new(),
            variables: IndexMap::new(),
            body: None,
        };
        let rows = build_rows(&ctx, Some(&bare), &BTreeSet::new());
        assert!(!rows.contains(&RowKind::SectionHeader("This request")));
        let rows_none = build_rows(&ctx, None, &BTreeSet::new());
        assert!(!rows_none.contains(&RowKind::SectionHeader("This request")));
    }

    #[test]
    fn expanding_a_group_splices_its_entry_rows_after_the_header() {
        let (_dir, ctx, _req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("creds".to_string());
        let rows = build_rows(&ctx, None, &expanded);
        let header_idx = rows
            .iter()
            .position(|r| {
                r == &RowKind::GroupHeader {
                    name: "creds".into(),
                }
            })
            .unwrap();
        assert_eq!(
            rows[header_idx + 1],
            RowKind::EntryRow {
                group: "creds".into(),
                entry: "alice".into()
            }
        );
        assert_eq!(
            rows[header_idx + 2],
            RowKind::EntryRow {
                group: "creds".into(),
                entry: "bob".into()
            }
        );
        // fields still follow, after the entry rows:
        assert_eq!(
            rows[header_idx + 3],
            RowKind::GroupMember {
                group: "creds".into(),
                name: "user_id".into()
            }
        );
    }

    // -----------------------------------------------------------------
    // draw: cell content rules, via buffer cells
    // -----------------------------------------------------------------

    fn render(
        ctx: &ProjectContext,
        req: Option<&HttpRequest>,
        expanded: BTreeSet<String>,
        width: u16,
        env_scroll: usize,
    ) -> (String, ratatui::Terminal<ratatui::backend::TestBackend>) {
        let theme = Theme::for_terminal();
        let backend = ratatui::backend::TestBackend::new(width, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        let mut vm = VarManager {
            expanded,
            env_scroll,
            ..Default::default()
        };
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, ctx, req, &mut hits, None))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        (content, terminal)
    }

    #[test]
    fn simple_var_default_fallback_is_muted_env_value_is_normal() {
        let (_dir, ctx, req) = fixture();
        let theme = Theme::for_terminal();
        let (content, term) = render(&ctx, Some(&req), BTreeSet::new(), 120, 0);
        assert!(content.contains("http://localhost:8080"), "{content}");
        assert!(content.contains("https://qa.example.com"), "{content}");

        let rows = build_rows(&ctx, Some(&req), &BTreeSet::new());
        let y = row_y(
            &rows,
            &RowKind::Var {
                name: "base_url".into(),
            },
        );
        let (default_text, default_fg) = cell_at(&term, NAME_W + DESC_W, y, ENV_W);
        assert_eq!(default_text, "http://localhost:8080");
        assert_eq!(
            default_fg, theme.text,
            "the Default column is the value's home"
        );
        let (dev_text, dev_fg) = cell_at(&term, env_col_x(&ctx, "dev"), y, ENV_W);
        assert_eq!(dev_text, "http://localhost:8080");
        assert_eq!(dev_fg, theme.text_muted, "default fallback must be muted");
        let (qa_text, qa_fg) = cell_at(&term, env_col_x(&ctx, "qa"), y, ENV_W);
        assert_eq!(qa_text, "https://qa.example.com");
        assert_eq!(qa_fg, theme.text, "explicit env value is normal fg");
    }

    #[test]
    fn secret_is_masked_never_shows_value_and_warns_when_missing() {
        let (_dir, ctx, req) = fixture();
        let (content, _term) = render(&ctx, Some(&req), BTreeSet::new(), 120, 0);
        assert!(
            content.contains("\u{25cf}\u{25cf}\u{25cf}\u{25cf}"),
            "{content}"
        );
        assert!(content.contains("\u{26a0} secret"), "{content}");
        assert!(
            !content.contains("sk-qa-secret-value"),
            "secret value must never render: {content}"
        );
    }

    #[test]
    fn expanded_entry_row_shows_name_and_values_never_the_secret() {
        let (_dir, ctx, req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("creds".to_string());
        let (content, _term) = render(&ctx, Some(&req), expanded, 160, 0);
        assert!(content.contains("alice"), "{content}");
        assert!(content.contains("bob"), "{content}");
        assert!(content.contains("user_id=1001"), "{content}");
        assert!(content.contains("user_id=2002"), "{content}");
        assert!(
            !content.contains("sk-qa-secret-value"),
            "secret value must never render: {content}"
        );
    }

    #[test]
    fn env_scroll_hides_the_first_env_column() {
        let (_dir, ctx, req) = fixture();
        let (all, _t) = render(&ctx, Some(&req), BTreeSet::new(), 120, 0);
        assert!(all.contains("dev"), "{all}");
        assert!(all.contains("qa"), "{all}");

        let (scrolled, _t2) = render(&ctx, Some(&req), BTreeSet::new(), 120, 1);
        // "dev" the env-column header is gone (env name string alone,
        // distinct from any substring collisions in this fixture).
        assert!(!scrolled.contains("dev"), "{scrolled}");
        assert!(scrolled.contains("qa"), "{scrolled}");
    }

    #[test]
    fn masked_secret_buffer_never_contains_the_value_anywhere() {
        let (_dir, ctx, req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("creds".to_string());
        let (content, _term) = render(&ctx, Some(&req), expanded, 200, 0);
        assert!(!content.contains("sk-qa-secret-value"));
    }

    #[test]
    fn disabled_request_var_cell_is_muted_enabled_one_is_normal() {
        let (_dir, ctx, req) = fixture();
        let theme = Theme::for_terminal();
        let (_content, term) = render(&ctx, Some(&req), BTreeSet::new(), 120, 0);
        let enabled_fgs = fgs_of(&term, "abc-123");
        assert!(!enabled_fgs.is_empty());
        assert!(
            enabled_fgs.iter().all(|fg| *fg == theme.text),
            "an enabled request var's value must render in normal fg: {enabled_fgs:?}"
        );
        let disabled_fgs = fgs_of(&term, "should-be-muted");
        assert!(!disabled_fgs.is_empty());
        assert!(
            disabled_fgs.iter().all(|fg| *fg == theme.text_muted),
            "a disabled request var's value must render muted: {disabled_fgs:?}"
        );
    }

    #[test]
    fn selected_entry_row_shows_a_check_mark_the_unselected_one_does_not() {
        // Base fixture already selects `creds = "alice"` in `qa`.
        let (_dir, ctx, req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("creds".to_string());
        let (content, _term) = render(&ctx, Some(&req), expanded, 160, 0);
        assert!(
            content.contains("\u{2713} user_id=1001"),
            "the selected entry's row must carry the check mark: {content}"
        );
        assert!(
            !content.contains("\u{2713} user_id=2002"),
            "the unselected entry's row must not: {content}"
        );
    }

    // -----------------------------------------------------------------
    // A group whose entries exist in only ONE environment must still be
    // expandable and show its entry row(s) — the row set is the union
    // across every environment (spec §3.1: entries belong to one env).
    // -----------------------------------------------------------------

    /// Group `region` (one field), with entries declared only by `qa`,
    /// selected there; `dev` has nothing for it at all.
    fn fixture_env_only_entries() -> (tempfile::TempDir, ProjectContext) {
        let dir = tempfile::tempdir().unwrap();
        project::init_project(dir.path(), Some("env-only")).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[groups.region]\ndescription = \"deploy region\"\nfields = [\"region\"]\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[entries.region.east]\ndescription = \"US East\"\nregion = \"us-east-1\"\n",
        )
        .unwrap();

        let mut selections = IndexMap::new();
        let mut qa_sel = IndexMap::new();
        qa_sel.insert("region".to_string(), "east".to_string());
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

    #[test]
    fn group_with_entries_in_one_env_only_gets_an_expand_glyph() {
        let (_dir, ctx) = fixture_env_only_entries();
        let header = RowKind::GroupHeader {
            name: "region".into(),
        };
        assert_eq!(
            expand_glyph(&ctx, &header, &BTreeSet::new()),
            Some(GLYPH_COLLAPSED),
            "only qa.toml declares entries for it — must still be expandable"
        );
    }

    #[test]
    fn group_with_entries_in_one_env_only_expands_to_that_env_s_entry_row() {
        let (_dir, ctx) = fixture_env_only_entries();
        let header = RowKind::GroupHeader {
            name: "region".into(),
        };
        let mut expanded = BTreeSet::new();
        expanded.insert("region".to_string());
        let rows = build_rows(&ctx, None, &expanded);
        let idx = rows.iter().position(|r| r == &header).unwrap();
        assert_eq!(
            rows[idx + 1],
            RowKind::EntryRow {
                group: "region".into(),
                entry: "east".into()
            }
        );

        // and collapsed, no entry row appears anywhere:
        let rows_collapsed = build_rows(&ctx, None, &BTreeSet::new());
        assert!(
            !rows_collapsed
                .iter()
                .any(|r| matches!(r, RowKind::EntryRow { group, .. } if group == "region"))
        );
    }

    #[test]
    fn group_with_entries_in_one_env_only_shows_selection_truth_in_that_env() {
        let (_dir, ctx) = fixture_env_only_entries();
        let mut expanded = BTreeSet::new();
        expanded.insert("region".to_string());
        let (content, _term) = render(&ctx, None, expanded, 120, 0);
        // The field row, resolved in qa (the active/selected env):
        assert!(content.contains("east \u{b7} us-east-1"), "{content}");
        // The spliced-in entry row, check-marked as selected in qa:
        assert!(content.contains("\u{2713} region=us-east-1"), "{content}");
    }

    // -----------------------------------------------------------------
    // Finding 2 (review): group selected-state cell rules, asserted
    // against actual buffer content (a group option genuinely selected
    // in one environment, not selected in another).
    // -----------------------------------------------------------------

    /// Group `creds` (fields `user_id`, `customer_id`) with entry `alice`
    /// in `qa`, selected there; `dev` declares the same entry but has no
    /// selection.
    fn fixture_group_selected() -> (tempfile::TempDir, ProjectContext) {
        let dir = tempfile::tempdir().unwrap();
        project::init_project(dir.path(), Some("group-selected")).unwrap();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[groups.creds]\ndescription = \"paired ids\"\nfields = [\"user_id\", \"customer_id\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/dev.toml"),
            "[entries.creds.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-77\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[entries.creds.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-99\"\n",
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

    /// The list area's first data row's `y`, matching `draw`'s own
    /// layout math (title bar, then a 1-line column-header strip).
    const LIST_TOP: u16 = TITLE_HEIGHT + 1;

    fn row_y(rows: &[RowKind], target: &RowKind) -> u16 {
        let idx = rows
            .iter()
            .position(|r| r == target)
            .unwrap_or_else(|| panic!("row not found: {target:?}"));
        LIST_TOP + idx as u16
    }

    fn env_col_x(ctx: &ProjectContext, env: &str) -> u16 {
        let ci = ctx
            .environments
            .iter()
            .position(|e| e == env)
            .unwrap_or_else(|| panic!("env not found: {env:?}"));
        NAME_W + DESC_W + ENV_W + (ci as u16) * ENV_W
    }

    /// The text (trimmed of trailing padding) and fg of the first
    /// non-blank cell in a `w`-wide run starting at `(x, y)`.
    fn cell_at(
        term: &ratatui::Terminal<ratatui::backend::TestBackend>,
        x: u16,
        y: u16,
        w: u16,
    ) -> (String, Color) {
        let buf = term.backend().buffer();
        let mut s = String::new();
        let mut fg = Color::Reset;
        let mut got_fg = false;
        for dx in 0..w {
            let cell = &buf[(x + dx, y)];
            let sym = cell.symbol();
            if !got_fg && sym != " " {
                fg = cell.fg;
                got_fg = true;
            }
            s.push_str(sym);
        }
        (s.trim_end().to_string(), fg)
    }

    #[test]
    fn group_header_shows_selected_key_or_needs_selection_per_env() {
        let (_dir, ctx) = fixture_group_selected();
        let rows = build_rows(&ctx, None, &BTreeSet::new());
        let header_row = RowKind::GroupHeader {
            name: "creds".into(),
        };
        let y = row_y(&rows, &header_row);
        let (_content, term) = render(&ctx, None, BTreeSet::new(), 120, 0);

        let (qa_text, qa_fg) = cell_at(&term, env_col_x(&ctx, "qa"), y, ENV_W);
        assert_eq!(qa_text, "alice", "selected key shown bare on the header");
        assert_eq!(qa_fg, Theme::for_terminal().text);

        let (dev_text, dev_fg) = cell_at(&term, env_col_x(&ctx, "dev"), y, ENV_W);
        assert_eq!(dev_text, "\u{26a0} select");
        assert_eq!(dev_fg, Theme::for_terminal().warning);
    }

    #[test]
    fn group_members_show_key_dot_value_when_selected_needs_selection_otherwise() {
        let (_dir, ctx) = fixture_group_selected();
        let rows = build_rows(&ctx, None, &BTreeSet::new());
        let member_row = RowKind::GroupMember {
            group: "creds".into(),
            name: "customer_id".into(),
        };
        let y = row_y(&rows, &member_row);
        let (_content, term) = render(&ctx, None, BTreeSet::new(), 120, 0);

        let (qa_text, _) = cell_at(&term, env_col_x(&ctx, "qa"), y, ENV_W);
        assert_eq!(
            qa_text, "alice \u{b7} c-99",
            "qa's own entry value for customer_id must show through"
        );
        let (dev_text, _) = cell_at(&term, env_col_x(&ctx, "dev"), y, ENV_W);
        assert_eq!(dev_text, "\u{26a0} select");
    }

    #[test]
    fn group_entry_row_check_marks_only_the_env_that_selects_it() {
        let (_dir, ctx) = fixture_group_selected();
        let mut expanded = BTreeSet::new();
        expanded.insert("creds".to_string());
        let rows = build_rows(&ctx, None, &expanded);
        let entry_row = RowKind::EntryRow {
            group: "creds".into(),
            entry: "alice".into(),
        };
        let y = row_y(&rows, &entry_row);
        // wide enough that neither env column's values are clipped
        let (_content, term) = render(&ctx, None, expanded, 200, 0);

        // qa has this entry selected: check-marked, showing qa's values.
        let (qa_text, _qa_fg) = cell_at(&term, env_col_x(&ctx, "qa"), y, ENV_W);
        assert!(
            qa_text.starts_with('\u{2713}'),
            "qa's selected entry row must show the check mark: {qa_text:?}"
        );

        // dev declares the same entry but selects nothing: no check mark.
        let (dev_text, _dev_fg) = cell_at(&term, env_col_x(&ctx, "dev"), y, ENV_W);
        assert!(
            !dev_text.starts_with('\u{2713}'),
            "dev has no selection, must not be check-marked: {dev_text:?}"
        );
        assert!(dev_text.starts_with("user_id=1001"), "{dev_text:?}");
    }

    /// All occurrences of `needle` in the rendered buffer, as (row, fg) —
    /// actually just `fg`, since these tests only care about the color a
    /// given piece of text renders in, and a needle may legitimately
    /// repeat across environment columns.
    fn fgs_of(term: &ratatui::Terminal<ratatui::backend::TestBackend>, needle: &str) -> Vec<Color> {
        let buf = term.backend().buffer();
        let mut out = Vec::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            let mut start = 0;
            while let Some(pos) = line[start..].find(needle) {
                let byte_idx = start + pos;
                out.push(buf[(byte_idx as u16, y)].fg);
                start = byte_idx + needle.len();
            }
        }
        out
    }

    // -----------------------------------------------------------------
    // shell behaviors carried over
    // -----------------------------------------------------------------

    #[test]
    fn title_bar_shows_project_and_env() {
        let (_dir, ctx, _req) = fixture();
        let (content, _term) = render(&ctx, None, BTreeSet::new(), 120, 0);
        assert!(content.contains("Variables"));
        assert!(content.contains("demo"));
        assert!(content.contains("qa"));
    }

    #[test]
    fn footer_hint_mentions_esc() {
        let (_dir, ctx, _req) = fixture();
        let (content, _term) = render(&ctx, None, BTreeSet::new(), 120, 0);
        assert!(content.contains("esc"));
        assert!(content.contains("back"));
    }

    #[test]
    fn esc_asks_the_app_to_close_the_screen() {
        let (_dir, ctx, _req) = fixture();
        let mut vm = VarManager::default();
        let ev = KeyEvent::new(KeyCode::Esc, ratatui::crossterm::event::KeyModifiers::NONE);
        assert_eq!(vm.handle_key(ev, &ctx, None), Some(Action::CloseScreen));
    }

    #[test]
    fn unbound_plain_key_is_unhandled_here() {
        let (_dir, ctx, _req) = fixture();
        let mut vm = VarManager::default();
        let ev = KeyEvent::new(
            KeyCode::Char('q'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        );
        assert_eq!(vm.handle_key(ev, &ctx, None), None);
    }

    // -----------------------------------------------------------------
    // Task 11: navigation + in-place editing
    // -----------------------------------------------------------------

    fn idx(rows: &[RowKind], target: &RowKind) -> usize {
        rows.iter()
            .position(|r| r == target)
            .unwrap_or_else(|| panic!("row not found: {target:?}"))
    }

    fn env_col(ctx: &ProjectContext, env: &str) -> usize {
        2 + ctx
            .environments
            .iter()
            .position(|e| e == env)
            .unwrap_or_else(|| panic!("env not found: {env:?}"))
    }

    fn down(vm: &mut VarManager, ctx: &ProjectContext) {
        vm.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), ctx, None);
    }

    #[test]
    fn cursor_down_skips_section_headers_and_lands_on_the_first_data_row() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        assert!(matches!(vm.rows[0], RowKind::SectionHeader("This request")));
        assert_eq!(vm.cursor.0, 0);
        down(&mut vm, &ctx); // -> RequestVar trace_id (row 1), skipping nothing yet
        assert_eq!(vm.cursor.0, 1);
        assert!(
            matches!(&vm.rows[vm.cursor.0], RowKind::RequestVar { name } if name == "trace_id")
        );
        down(&mut vm, &ctx); // -> RequestVar disabled_flag
        assert_eq!(vm.cursor.0, 2);
        down(&mut vm, &ctx); // skips SectionHeader("Project") straight to base_url
        assert!(matches!(&vm.rows[vm.cursor.0], RowKind::Var { name } if name == "base_url"));
    }

    #[test]
    fn cursor_down_never_lands_on_the_trailing_ghost_rows() {
        let (_dir, ctx, _req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, None, &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor.0 = idx(
            &vm.rows,
            &RowKind::GroupMember {
                group: "creds".into(),
                name: "customer_id".into(),
            },
        );
        let last = vm.cursor.0;
        down(&mut vm, &ctx); // AddVar/AddGroup are not stops: cursor holds
        assert_eq!(
            vm.cursor.0, last,
            "no editable row past the last group member; cursor must not move onto a ghost row"
        );
    }

    #[test]
    fn member_env_cell_edits_the_selected_options_value() {
        let (_dir, ctx) = fixture_group_selected();
        let mut vm = VarManager {
            rows: build_rows(&ctx, None, &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::GroupMember {
                group: "creds".into(),
                name: "customer_id".into(),
            },
        );
        vm.cursor = (row, env_col(&ctx, "qa"));
        assert_eq!(vm.activate_cursor(&ctx, None), None);
        assert_eq!(
            vm.editing.as_ref().expect("edit began").input.text(),
            "c-99",
            "seeded with the selected option's value for this member+env"
        );
        vm.editing.as_mut().unwrap().input = LineInput::new("c-100");
        assert_eq!(
            vm.commit_edit(&ctx),
            Some(Action::VarEdit(VarEditOp::SetOptionValue {
                env: "qa".into(),
                owner: "creds".into(),
                key: "alice".into(),
                member: Some("customer_id".into()),
                value: "c-100".into(),
            }))
        );
    }

    #[test]
    fn unselected_cells_open_the_selection_dropdown() {
        let (_dir, ctx) = fixture_group_selected();
        let mut vm = VarManager {
            rows: build_rows(&ctx, None, &BTreeSet::new()),
            ..Default::default()
        };
        let header = idx(
            &vm.rows,
            &RowKind::GroupHeader {
                name: "creds".into(),
            },
        );
        let member = idx(
            &vm.rows,
            &RowKind::GroupMember {
                group: "creds".into(),
                name: "user_id".into(),
            },
        );
        // Enter on the header's env cell — selected or not — offers the
        // choice, and expand stays on the name column
        for (row, env) in [(header, "dev"), (header, "qa"), (member, "dev")] {
            vm.cursor = (row, env_col(&ctx, env));
            assert_eq!(
                vm.activate_cursor(&ctx, None),
                Some(Action::OpenSelectDropdown {
                    owner: "creds".into(),
                    env: env.to_string(),
                    row,
                    col: env_col(&ctx, env),
                }),
                "row {row} env {env}"
            );
            assert!(vm.expanded.is_empty(), "no expand from an env cell");
        }
        vm.cursor = (header, 0);
        assert_eq!(vm.activate_cursor(&ctx, None), None);
        assert!(vm.expanded.contains("creds"), "col 0 still expands");
    }

    #[test]
    fn single_click_on_a_group_headers_env_cell_opens_the_dropdown() {
        let (_dir, ctx) = fixture_group_selected();
        let mut vm = VarManager {
            rows: build_rows(&ctx, None, &BTreeSet::new()),
            ..Default::default()
        };
        let header = idx(
            &vm.rows,
            &RowKind::GroupHeader {
                name: "creds".into(),
            },
        );
        let col = env_col(&ctx, "dev");
        assert_eq!(
            vm.click_cell(header, col, false, &ctx, None),
            Some(Action::OpenSelectDropdown {
                owner: "creds".into(),
                env: "dev".into(),
                row: header,
                col,
            })
        );
    }

    #[test]
    fn e_on_an_entry_row_opens_the_edit_entry_prompt_prefilled() {
        let (_dir, ctx) = fixture_group_selected();
        let mut vm = VarManager {
            rows: build_rows(&ctx, None, &BTreeSet::from(["creds".to_string()])),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::EntryRow {
                    group: "creds".into(),
                    entry: "alice".into(),
                },
            ),
            0,
        );
        let action = vm.handle_key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            &ctx,
            None,
        );
        let Some(Action::OpenEditOptionPrompt {
            owner, key, values, ..
        }) = action
        else {
            panic!("expected the edit-option prompt, got {action:?}");
        };
        assert_eq!(owner, "creds");
        assert_eq!(key, "alice");
        // active env (qa) merge: shared user_id, qa-overridden customer_id
        assert_eq!(values.get("user_id").map(String::as_str), Some("1001"));
        assert_eq!(values.get("customer_id").map(String::as_str), Some("c-99"));
    }

    #[test]
    fn a_key_adds_a_member_from_any_of_the_groups_rows() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let expected = Some(Action::PromptAddGroupMember {
            group: "creds".into(),
        });
        for target in [
            RowKind::GroupHeader {
                name: "creds".into(),
            },
            RowKind::GroupMember {
                group: "creds".into(),
                name: "user_id".into(),
            },
        ] {
            vm.cursor = (idx(&vm.rows, &target), 0);
            let action = vm.handle_key(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                &ctx,
                Some(&req),
            );
            assert_eq!(action, expected, "from {target:?}");
        }
        let chips = action_chips(
            &ctx,
            vm.rows.get(idx(
                &vm.rows,
                &RowKind::GroupHeader {
                    name: "creds".into(),
                },
            )),
        );
        assert!(
            chips
                .iter()
                .any(|(key, label, _)| *key == "a" && *label == "member"),
        );
    }

    #[test]
    fn d_on_a_member_row_asks_to_remove_it_from_the_group() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::GroupMember {
                    group: "creds".into(),
                    name: "user_id".into(),
                },
            ),
            0,
        );
        let action = vm.handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &ctx,
            Some(&req),
        );
        assert_eq!(
            action,
            Some(Action::ConfirmRemoveGroupMember {
                group: "creds".into(),
                member: "user_id".into(),
            })
        );
    }

    #[test]
    fn e_key_renames_and_the_chip_advertises_it() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            0,
        );
        let action = vm.handle_key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            &ctx,
            Some(&req),
        );
        assert_eq!(
            action,
            Some(Action::PromptRenameVar {
                from: "base_url".into(),
            })
        );
        let chips = action_chips(&ctx, vm.rows.get(vm.cursor.0));
        assert!(
            chips
                .iter()
                .any(|(key, label, _)| *key == "e" && *label == "rename"),
            "the rename chip shows a character key like every other action: {chips:?}"
        );
    }

    #[test]
    fn click_name_double_on_a_var_row_opens_rename() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::Var {
                name: "base_url".into(),
            },
        );
        assert_eq!(vm.click_name(row, false, &ctx), None);
        assert_eq!(vm.cursor, (row, 0), "single click just moves the cursor");
        assert_eq!(
            vm.click_name(row, true, &ctx),
            Some(Action::PromptRenameVar {
                from: "base_url".into(),
            })
        );
    }

    #[test]
    fn click_name_double_on_an_expandable_row_toggles_expand() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        // `creds` has entries, so its name activation expands, not renames
        let row = idx(
            &vm.rows,
            &RowKind::GroupHeader {
                name: "creds".into(),
            },
        );
        assert_eq!(vm.click_name(row, true, &ctx), None);
        assert!(vm.expanded.contains("creds"));
    }

    #[test]
    fn click_anywhere_on_the_add_rows_dispatches_the_prompts() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let add_var = idx(&vm.rows, &RowKind::AddVar);
        let add_group = idx(&vm.rows, &RowKind::AddGroup);
        assert_eq!(
            vm.click_name(add_var, false, &ctx),
            Some(Action::PromptNewVar),
            "single click on the ghost row's label"
        );
        assert_eq!(
            vm.click_cell(add_var, 1, false, &ctx, Some(&req)),
            Some(Action::PromptNewVar),
            "single click on any of its cells"
        );
        assert_eq!(
            vm.click_cell(add_group, 0, false, &ctx, Some(&req)),
            Some(Action::PromptNewGroup)
        );
        assert_eq!(
            vm.click_row(add_group),
            Some(Action::PromptNewGroup),
            "single click on the row background"
        );
    }

    #[test]
    fn editing_the_description_renders_in_the_description_column() {
        let (_dir, ctx, req) = fixture();
        let theme = Theme::for_terminal();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::Var {
                name: "base_url".into(),
            },
        );
        vm.cursor = (row, 0);
        vm.activate_cursor(&ctx, Some(&req));
        assert_eq!(
            vm.editing.as_ref().expect("edit began").input.text(),
            "API root"
        );

        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, &ctx, Some(&req), &mut hits, None))
            .unwrap();
        let y = row_y(&vm.rows, &vm.rows[row].clone());
        let (name_text, _) = cell_at(&terminal, 0, y, NAME_W);
        assert!(
            name_text.contains("base_url"),
            "the name stays visible while its description is edited: {name_text:?}"
        );
        let (desc_text, _) = cell_at(&terminal, NAME_W, y, DESC_W);
        assert!(
            desc_text.contains("API root"),
            "the edit input lives in the description column: {desc_text:?}"
        );
    }

    #[test]
    fn draw_registers_separate_name_and_description_hits() {
        let (_dir, ctx, req) = fixture();
        let theme = Theme::for_terminal();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::Var {
                name: "base_url".into(),
            },
        );
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, &ctx, Some(&req), &mut hits, None))
            .unwrap();
        let name_rect = hits
            .rect_of(&crate::hit::Hit::VarName(row))
            .expect("name region registered");
        assert_eq!(name_rect.width, NAME_W);
        let desc_rect = hits
            .rect_of(&crate::hit::Hit::VarCell { row, col: 0 })
            .expect("description region registered");
        assert_eq!(desc_rect.x, name_rect.x + NAME_W);
        assert_eq!(desc_rect.width, DESC_W);
    }

    #[test]
    fn enter_on_the_default_cell_edits_and_commits_set_default() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            1,
        );
        let action = vm.activate_cursor(&ctx, Some(&req));
        assert_eq!(action, None);
        assert_eq!(
            vm.editing.as_ref().expect("edit began").input.text(),
            "http://localhost:8080",
            "seeded with the declared default"
        );
        vm.editing.as_mut().unwrap().input = LineInput::new("http://local:9999");
        assert_eq!(
            vm.commit_edit(&ctx),
            Some(Action::VarEdit(VarEditOp::SetDefault {
                name: "base_url".into(),
                value: "http://local:9999".into(),
            }))
        );
    }

    #[test]
    fn default_cell_on_a_secret_var_is_not_editable() {
        // secrets can't carry a default in variables.toml (parse rejects
        // it), so the Default cell must not offer an edit
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "api_key".into(),
                },
            ),
            1,
        );
        assert_eq!(vm.activate_cursor(&ctx, Some(&req)), None);
        assert!(vm.editing.is_none());
    }

    #[test]
    fn request_var_value_edits_in_the_default_column_not_env_columns() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::RequestVar {
                name: "trace_id".into(),
            },
        );
        vm.cursor = (row, 1);
        assert_eq!(vm.activate_cursor(&ctx, Some(&req)), None);
        assert_eq!(
            vm.editing.as_ref().expect("edit began").input.text(),
            "abc-123"
        );
        vm.editing.as_mut().unwrap().input = LineInput::new("xyz-999");
        assert_eq!(
            vm.commit_edit(&ctx),
            Some(Action::VarEdit(VarEditOp::SetRequestVar {
                name: "trace_id".into(),
                value: "xyz-999".into(),
            }))
        );

        vm.editing = None;
        vm.cursor = (row, env_col(&ctx, "qa"));
        assert_eq!(
            vm.activate_cursor(&ctx, Some(&req)),
            None,
            "request vars have no per-env values"
        );
        assert!(vm.editing.is_none());
    }

    #[test]
    fn header_strip_labels_the_default_and_env_columns() {
        let (_dir, ctx, req) = fixture();
        let (_content, term) = render(&ctx, Some(&req), BTreeSet::new(), 120, 0);
        let header_y = LIST_TOP - 1;
        let (default_label, _) = cell_at(&term, NAME_W + DESC_W, header_y, ENV_W);
        assert_eq!(default_label, "Default");
        let (dev_label, _) = cell_at(&term, env_col_x(&ctx, "dev"), header_y, ENV_W);
        assert_eq!(dev_label, "dev");
    }

    #[test]
    fn request_var_row_shows_its_value_in_the_default_column_only() {
        let (_dir, ctx, req) = fixture();
        let rows = build_rows(&ctx, Some(&req), &BTreeSet::new());
        let y = row_y(
            &rows,
            &RowKind::RequestVar {
                name: "trace_id".into(),
            },
        );
        let (_content, term) = render(&ctx, Some(&req), BTreeSet::new(), 120, 0);
        let (value, _) = cell_at(&term, NAME_W + DESC_W, y, ENV_W);
        assert_eq!(value, "abc-123");
        let theme = Theme::for_terminal();
        for env in ["dev", "qa"] {
            let (cell, fg) = cell_at(&term, env_col_x(&ctx, env), y, ENV_W);
            assert_eq!(
                cell, "\u{2014}",
                "a muted dash marks that a request var can't be set per env"
            );
            assert_eq!(fg, theme.text_muted);
        }
    }

    #[test]
    fn selecting_col_zero_highlights_only_the_description_region() {
        let (_dir, ctx, req) = fixture();
        let theme = Theme::for_terminal();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::Var {
                name: "base_url".into(),
            },
        );
        vm.cursor = (row, 0);
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, &ctx, Some(&req), &mut hits, None))
            .unwrap();
        let y = row_y(&vm.rows, &vm.rows[row].clone());
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(2u16, y)].bg,
            theme.control_hover,
            "name region carries only the row highlight"
        );
        assert_eq!(
            buf[(NAME_W + 2, y)].bg,
            theme.control_pressed,
            "description region carries the cell highlight"
        );
    }

    #[test]
    fn enter_on_a_simple_vars_env_cell_seeds_edit_with_the_shown_value() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            env_col(&ctx, "qa"),
        );
        let action = vm.activate_cursor(&ctx, Some(&req));
        assert_eq!(action, None, "beginning an edit dispatches nothing yet");
        let edit = vm.editing.as_ref().expect("edit began");
        assert!(!edit.masked);
        assert_eq!(edit.input.text(), "https://qa.example.com");
    }

    #[test]
    fn commit_on_a_simple_vars_env_cell_dispatches_set_env_value() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            env_col(&ctx, "dev"),
        );
        vm.activate_cursor(&ctx, Some(&req));
        vm.editing.as_mut().unwrap().input = LineInput::new("http://dev.local");
        let action = vm.commit_edit(&ctx);
        assert_eq!(
            action,
            Some(Action::VarEdit(VarEditOp::SetEnvValue {
                env: "dev".into(),
                name: "base_url".into(),
                value: "http://dev.local".into(),
            }))
        );
        assert!(
            vm.editing.is_some(),
            "commit itself never clears editing — App does, on success"
        );
    }

    #[test]
    fn enter_on_a_secret_vars_env_cell_begins_a_masked_edit_seeded_with_its_value() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "api_key".into(),
                },
            ),
            env_col(&ctx, "qa"),
        );
        vm.activate_cursor(&ctx, Some(&req));
        let edit = vm.editing.as_ref().expect("edit began");
        assert!(edit.masked, "a secret cell's edit must be masked");
        assert_eq!(
            edit.input.text(),
            "sk-qa-secret-value",
            "seeded with the actual value so retyping isn't required"
        );
    }

    #[test]
    fn commit_on_a_secret_vars_env_cell_dispatches_set_secret_value() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "api_key".into(),
                },
            ),
            env_col(&ctx, "dev"),
        );
        vm.activate_cursor(&ctx, Some(&req));
        vm.editing.as_mut().unwrap().input = LineInput::new("sk-dev-new");
        let action = vm.commit_edit(&ctx);
        assert_eq!(
            action,
            Some(Action::VarEdit(VarEditOp::SetSecretValue {
                env: "dev".into(),
                name: "api_key".into(),
                value: "sk-dev-new".into(),
            }))
        );
    }

    #[test]
    fn r_toggles_reveal_on_a_secret_cell_without_entering_edit() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "api_key".into(),
                },
            ),
            env_col(&ctx, "qa"),
        );
        let ev = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(vm.handle_key(ev, &ctx, Some(&req)), None);
        assert!(vm.editing.is_none(), "reveal must not begin an edit");
        assert_eq!(vm.revealed, Some(("api_key".to_string(), "qa".to_string())));
        vm.handle_key(ev, &ctx, Some(&req));
        assert_eq!(vm.revealed, None, "a second r hides it again");
    }

    #[test]
    fn enter_on_a_group_with_entries_toggles_expand_instead_of_editing() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::GroupHeader {
                    name: "creds".into(),
                },
            ),
            0,
        );
        let action = vm.activate_cursor(&ctx, Some(&req));
        assert_eq!(action, None);
        assert!(vm.editing.is_none());
        assert!(vm.expanded.contains("creds"));
        // and Enter again collapses it:
        vm.activate_cursor(&ctx, Some(&req));
        assert!(!vm.expanded.contains("creds"));
    }

    #[test]
    fn enter_on_a_group_header_with_options_toggles_expand() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::GroupHeader {
                    name: "creds".into(),
                },
            ),
            0,
        );
        vm.activate_cursor(&ctx, Some(&req));
        assert!(vm.expanded.contains("creds"));
        assert!(vm.editing.is_none());
    }

    #[test]
    fn enter_on_a_vars_col0_with_no_options_begins_a_description_edit() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            0,
        );
        vm.activate_cursor(&ctx, Some(&req));
        let edit = vm.editing.as_ref().expect("edit began");
        assert!(!edit.masked);
        assert_eq!(edit.input.text(), "API root");
        vm.editing.as_mut().unwrap().input = LineInput::new("the API's root URL");
        let action = vm.commit_edit(&ctx);
        assert_eq!(
            action,
            Some(Action::VarEdit(VarEditOp::SetDescription {
                owner: "base_url".into(),
                value: "the API's root URL".into(),
            }))
        );
    }

    #[test]
    fn space_on_an_entry_row_dispatches_select() {
        let (_dir, ctx, req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("creds".to_string());
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &expanded),
            expanded,
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::EntryRow {
                    group: "creds".into(),
                    entry: "bob".into(),
                },
            ),
            env_col(&ctx, "qa"),
        );
        let ev = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let action = vm.handle_key(ev, &ctx, Some(&req));
        assert_eq!(
            action,
            Some(Action::VarEdit(VarEditOp::Select {
                env: "qa".into(),
                name: "creds".into(),
                key: "bob".into(),
            }))
        );
        assert!(vm.editing.is_none(), "Select is not a text edit");
    }

    #[test]
    fn space_on_col0_or_a_non_entry_row_is_a_no_op() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            1,
        );
        let ev = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(vm.handle_key(ev, &ctx, Some(&req)), None);
    }

    #[test]
    fn first_esc_cancels_the_edit_second_esc_closes_the_screen() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            1,
        );
        vm.activate_cursor(&ctx, Some(&req));
        assert!(vm.editing.is_some());
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            vm.handle_key(esc, &ctx, Some(&req)),
            None,
            "first Esc eats the edit"
        );
        assert!(vm.editing.is_none());
        assert_eq!(
            vm.handle_key(esc, &ctx, Some(&req)),
            Some(Action::CloseScreen),
            "second Esc closes the screen"
        );
    }

    #[test]
    fn click_cell_on_an_entry_row_selects_immediately_on_a_single_click() {
        let (_dir, ctx, req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("creds".to_string());
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &expanded),
            expanded,
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::EntryRow {
                group: "creds".into(),
                entry: "alice".into(),
            },
        );
        let col = env_col(&ctx, "qa");
        let action = vm.click_cell(row, col, false, &ctx, Some(&req));
        assert_eq!(
            action,
            Some(Action::VarEdit(VarEditOp::Select {
                env: "qa".into(),
                name: "creds".into(),
                key: "alice".into(),
            }))
        );
        assert_eq!(vm.cursor, (row, col));
    }

    #[test]
    fn click_cell_single_click_on_a_simple_var_just_moves_the_cursor() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::Var {
                name: "base_url".into(),
            },
        );
        let col = env_col(&ctx, "qa");
        let action = vm.click_cell(row, col, false, &ctx, Some(&req));
        assert_eq!(action, None);
        assert!(vm.editing.is_none());
        assert_eq!(vm.cursor, (row, col));
    }

    #[test]
    fn click_cell_double_click_on_a_simple_var_begins_edit() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        let row = idx(
            &vm.rows,
            &RowKind::Var {
                name: "base_url".into(),
            },
        );
        let col = env_col(&ctx, "qa");
        let action = vm.click_cell(row, col, true, &ctx, Some(&req));
        assert_eq!(action, None, "begin-edit dispatches nothing yet");
        assert!(vm.editing.is_some(), "the double click began an edit");
    }

    #[test]
    fn click_row_moves_the_cursor_row_and_discards_any_edit() {
        let (_dir, ctx, req) = fixture();
        let mut vm = VarManager {
            rows: build_rows(&ctx, Some(&req), &BTreeSet::new()),
            ..Default::default()
        };
        vm.cursor = (
            idx(
                &vm.rows,
                &RowKind::Var {
                    name: "base_url".into(),
                },
            ),
            1,
        );
        vm.activate_cursor(&ctx, Some(&req));
        assert!(vm.editing.is_some());
        let target = idx(
            &vm.rows,
            &RowKind::Var {
                name: "api_key".into(),
            },
        );
        vm.click_row(target);
        assert_eq!(vm.cursor.0, target);
        assert!(vm.editing.is_none());
    }
}
