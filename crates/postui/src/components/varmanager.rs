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
use ratatui::style::Color;
use std::collections::BTreeSet;

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
    /// A declared variable: simple, enumerated, or secret alike.
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
    /// An option sub-row, indented under an expanded `Var` or `GroupHeader`.
    OptionRow {
        owner: String,
        key: String,
    },
    /// Ghost action rows at the end of the project section.
    AddVar,
    AddGroup,
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
    /// block, `col` 1.. are environment columns (relative to `env_scroll`).
    pub cursor: (usize, usize),
    /// Index into the project's environment list: the first visible env
    /// column.
    pub env_scroll: usize,
    /// First visible row's index into `rows`.
    pub scroll: usize,
    /// Names (variable or group) whose option sub-rows are shown.
    pub expanded: BTreeSet<String>,
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

    // Group members are rendered under their `GroupHeader`, not as their
    // own top-level `Var` row, even though a member may also have its own
    // (option-less) entry in `model.vars`.
    let group_members: std::collections::HashSet<&str> = ctx
        .model
        .groups
        .values()
        .flat_map(|g| g.members.iter().map(String::as_str))
        .collect();

    for name in ctx.model.vars.keys() {
        if group_members.contains(name.as_str()) {
            continue;
        }
        rows.push(RowKind::Var { name: name.clone() });
        if expanded.contains(name) {
            for key in ctx.model.vars[name].options.keys() {
                rows.push(RowKind::OptionRow {
                    owner: name.clone(),
                    key: key.clone(),
                });
            }
        }
    }

    for (group_name, decl) in &ctx.model.groups {
        rows.push(RowKind::GroupHeader {
            name: group_name.clone(),
        });
        if expanded.contains(group_name) {
            for key in decl.options.keys() {
                rows.push(RowKind::OptionRow {
                    owner: group_name.clone(),
                    key: key.clone(),
                });
            }
        }
        for member in &decl.members {
            rows.push(RowKind::GroupMember {
                group: group_name.clone(),
                name: member.clone(),
            });
        }
    }

    rows.push(RowKind::AddVar);
    rows.push(RowKind::AddGroup);

    rows
}

/// Fixed column widths (spec §5: "Fixed left columns: name, description;
/// then one column per environment side by side").
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
    let env_data = postui_core::project::load_environment(&ctx.root, name).unwrap_or_default();
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
/// `open_request` is only consulted for `RequestVar` rows.
fn env_cell(
    ctx: &ProjectContext,
    row: &RowKind,
    col: &EnvColumn,
    open_request: Option<&HttpRequest>,
    theme: &Theme,
) -> Cell {
    use postui_core::varmodel::VarMeta;

    match row {
        RowKind::SectionHeader(_) | RowKind::AddVar | RowKind::AddGroup => Cell::blank(),

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

        RowKind::Var { name } => {
            let Some(decl) = ctx.model.vars.get(name) else {
                return Cell::blank();
            };
            if decl.secret {
                return match col.resolved.meta.get(name) {
                    Some(VarMeta::Secret) => Cell {
                        text: "\u{25cf}\u{25cf}\u{25cf}\u{25cf}".into(),
                        fg: theme.text,
                    },
                    _ => Cell {
                        text: "\u{26a0} secret".into(),
                        fg: theme.warning,
                    },
                };
            }
            let merged = postui_core::varmodel::merged_var_options(&ctx.model, &col.env_data, name);
            if !merged.is_empty() {
                return match col.resolved.meta.get(name) {
                    Some(VarMeta::Enumerated { selected }) => {
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
            let merged =
                postui_core::varmodel::merged_group_options(&ctx.model, &col.env_data, name);
            if merged.is_empty() {
                return Cell::blank();
            }
            match col.selections.get(name) {
                Some(key) if merged.contains_key(key) => Cell {
                    text: key.clone(),
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

        RowKind::OptionRow { owner, key } => {
            if ctx.model.vars.contains_key(owner) {
                let merged =
                    postui_core::varmodel::merged_var_options(&ctx.model, &col.env_data, owner);
                let Some(opt) = merged.get(key) else {
                    return Cell::blank();
                };
                let overridden = col
                    .env_data
                    .options
                    .get(owner)
                    .and_then(|m| m.get(key))
                    .is_some_and(|fields| fields.contains_key("value"));
                let mut text = opt.value.clone();
                if col.selections.get(owner) == Some(key) {
                    text = format!("\u{2713} {text}");
                }
                Cell {
                    text,
                    fg: if overridden {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                }
            } else if ctx.model.groups.contains_key(owner) {
                let merged =
                    postui_core::varmodel::merged_group_options(&ctx.model, &col.env_data, owner);
                let Some(opt) = merged.get(key) else {
                    return Cell::blank();
                };
                let overridden = col
                    .env_data
                    .options
                    .get(owner)
                    .and_then(|m| m.get(key))
                    .is_some_and(|fields| fields.keys().any(|f| f != "description"));
                let mut text = opt
                    .values
                    .iter()
                    .map(|(m, v)| format!("{m}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                if col.selections.get(owner) == Some(key) {
                    text = format!("\u{2713} {text}");
                }
                Cell {
                    text,
                    fg: if overridden {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                }
            } else {
                Cell::blank()
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
    let name = match row {
        RowKind::Var { name } => name,
        RowKind::GroupHeader { name } => name,
        _ => return None,
    };
    let has_options = ctx
        .model
        .vars
        .get(name)
        .map(|d| !d.options.is_empty())
        .or_else(|| ctx.model.groups.get(name).map(|g| !g.options.is_empty()))
        .unwrap_or(false);
    if !has_options {
        return None;
    }
    Some(if expanded.contains(name) {
        GLYPH_EXPANDED
    } else {
        GLYPH_COLLAPSED
    })
}

/// The name column's indent (in cells) and label for `row`.
fn name_and_indent(row: &RowKind) -> (u16, String) {
    match row {
        RowKind::SectionHeader(s) => (0, s.to_string()),
        RowKind::RequestVar { name } => (0, name.clone()),
        RowKind::Var { name } => (0, name.clone()),
        RowKind::GroupHeader { name } => (0, name.clone()),
        RowKind::GroupMember { name, .. } => (2, name.clone()),
        RowKind::OptionRow { key, .. } => (4, key.clone()),
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
        RowKind::OptionRow { owner, key } => if let Some(decl) = ctx.model.vars.get(owner) {
            decl.options.get(key).and_then(|o| o.description.clone())
        } else {
            ctx.model
                .groups
                .get(owner)
                .and_then(|g| g.options.get(key))
                .and_then(|o| o.description.clone())
        }
        .unwrap_or_default(),
        _ => String::new(),
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
    /// `Esc` asks the app to leave the screen (`Action::CloseScreen`);
    /// nothing else is handled yet (grid navigation/editing is Task 11).
    pub fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match ev.code {
            KeyCode::Esc => Some(Action::CloseScreen),
            _ => None,
        }
    }

    /// Paints the full-screen grid into `area`: a `theme.panel` title bar
    /// reading "Variables — `<project>` · `<env>`", a column-header strip,
    /// the variable/environment grid itself (rebuilt from `ctx` +
    /// `open_request` + `self.expanded` at the top of every call), and a
    /// muted footer hint row.
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        ctx: &ProjectContext,
        open_request: Option<&HttpRequest>,
        hits: &mut HitMap,
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

            // How many env columns fit, and which ones (via env_scroll).
            let env_capacity = ((list_area.width.saturating_sub(NAME_W + DESC_W)) / ENV_W) as usize;
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
            for (i, col) in columns.iter().enumerate() {
                let x = header_row.x + NAME_W + DESC_W + (i as u16) * ENV_W;
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

                let is_header = matches!(row, RowKind::SectionHeader(_));
                let (indent, label) = name_and_indent(row);
                let mut x = list_area.x + 1 + indent;
                if let Some(glyph) = expand_glyph(ctx, row, &self.expanded) {
                    text(buf, x, y, glyph, theme.text_muted, row_fill, false);
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
                    row_fill,
                    is_header,
                );

                let desc = description_for(ctx, row);
                if !desc.is_empty() {
                    let desc_x = list_area.x + NAME_W;
                    let desc_w = DESC_W.saturating_sub(1);
                    text(
                        buf,
                        desc_x,
                        y,
                        super::chooser::clip(&desc, desc_w),
                        theme.text_muted,
                        row_fill,
                        false,
                    );
                }

                let name_col_rect = Rect {
                    x: list_area.x,
                    y,
                    width: NAME_W + DESC_W,
                    height: 1,
                };
                hits.register(name_col_rect, crate::hit::Hit::VarCell { row: i, col: 0 });

                for (ci, col) in columns.iter().enumerate() {
                    let cx = list_area.x + NAME_W + DESC_W + (ci as u16) * ENV_W;
                    if cx >= list_area.x + list_area.width {
                        break;
                    }
                    let cell = env_cell(ctx, row, col, open_request, theme);
                    if !cell.text.is_empty() {
                        let w = ENV_W.saturating_sub(1);
                        text(
                            buf,
                            cx,
                            y,
                            super::chooser::clip(&cell.text, w),
                            cell.fg,
                            row_fill,
                            false,
                        );
                    }
                    let cell_rect = Rect {
                        x: cx,
                        y,
                        width: ENV_W,
                        height: 1,
                    };
                    hits.register(
                        cell_rect,
                        crate::hit::Hit::VarCell {
                            row: i,
                            col: 1 + ci,
                        },
                    );
                }
            }
        }

        fill(buf, hint_row, theme.panel);
        if hint_row.height > 0 {
            text(
                buf,
                hint_row.x + 1,
                hint_row.y,
                " esc back ",
                theme.text_muted,
                theme.panel,
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use postui_core::model::Entry;
    use postui_core::project;

    /// A project with: two envs (dev, qa); `base_url` simple with a
    /// default (no env value in dev → falls back; qa sets its own);
    /// `user` enumerated, selected in qa only; `api_key` secret, with a
    /// value in qa only; group `creds` with two members, unselected in
    /// both envs. Plus a `trace_id` request-scope override on the
    /// returned `HttpRequest`.
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
[user.options.alice]
description = "admin"
value = "1001"
[user.options.bob]
value = "2002"

[api_key]
description = "service key"
secret = true

[groups.creds]
description = "paired ids"
members = ["user_id", "customer_id"]
[groups.creds.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "").unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "base_url = \"https://qa.example.com\"\n",
        )
        .unwrap();

        let mut selections = IndexMap::new();
        let mut qa_sel = IndexMap::new();
        qa_sel.insert("user".to_string(), "alice".to_string());
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
    fn expanding_a_variable_splices_its_option_rows_right_after_it() {
        let (_dir, ctx, _req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("user".to_string());
        let rows = build_rows(&ctx, None, &expanded);
        let user_idx = rows
            .iter()
            .position(|r| {
                r == &RowKind::Var {
                    name: "user".into(),
                }
            })
            .unwrap();
        assert_eq!(
            rows[user_idx + 1],
            RowKind::OptionRow {
                owner: "user".into(),
                key: "alice".into()
            }
        );
        assert_eq!(
            rows[user_idx + 2],
            RowKind::OptionRow {
                owner: "user".into(),
                key: "bob".into()
            }
        );
        // and NOT expanded when not in `expanded`:
        let rows2 = build_rows(&ctx, None, &BTreeSet::new());
        assert!(
            !rows2
                .iter()
                .any(|r| matches!(r, RowKind::OptionRow { owner, .. } if owner == "user"))
        );
    }

    #[test]
    fn expanding_a_group_splices_its_option_rows_after_the_header() {
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
            RowKind::OptionRow {
                owner: "creds".into(),
                key: "alice".into()
            }
        );
        // members still follow, after the option row(s):
        assert_eq!(
            rows[header_idx + 2],
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
            .draw(|f| vm.draw(f, f.area(), &theme, ctx, req, &mut hits))
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

        // Find the dev column cell holding the default-fallback value and
        // assert it's muted, and the qa column's explicit value is not.
        let buf = term.backend().buffer();
        let mut found_muted = false;
        let mut found_normal = false;
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            if line.contains("http://localhost:8080") {
                let start = line.find("http://localhost:8080").unwrap() as u16;
                let cell = &buf[(start, y)];
                assert_eq!(cell.fg, theme.text_muted, "default fallback must be muted");
                found_muted = true;
            }
            if line.contains("https://qa.example.com") {
                let start = line.find("https://qa.example.com").unwrap() as u16;
                let cell = &buf[(start, y)];
                assert_eq!(cell.fg, theme.text, "explicit env value is normal fg");
                found_normal = true;
            }
        }
        assert!(found_muted && found_normal);
    }

    #[test]
    fn enumerated_var_shows_key_dot_value_when_selected_and_warns_when_not() {
        let (_dir, ctx, req) = fixture();
        let (content, _term) = render(&ctx, Some(&req), BTreeSet::new(), 120, 0);
        assert!(content.contains("alice \u{b7} 1001"), "{content}");
        assert!(content.contains("\u{26a0} select"), "{content}");
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
    fn expanded_option_row_shows_key_and_value_never_the_secret() {
        let (_dir, ctx, req) = fixture();
        let mut expanded = BTreeSet::new();
        expanded.insert("user".to_string());
        let (content, _term) = render(&ctx, Some(&req), expanded, 120, 0);
        assert!(content.contains("alice"));
        assert!(content.contains("bob"));
        assert!(content.contains("1001"));
        assert!(content.contains("2002"));
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
        expanded.insert("user".to_string());
        expanded.insert("creds".to_string());
        let (content, _term) = render(&ctx, Some(&req), expanded, 200, 0);
        assert!(!content.contains("sk-qa-secret-value"));
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
        let mut vm = VarManager::default();
        let ev = KeyEvent::new(KeyCode::Esc, ratatui::crossterm::event::KeyModifiers::NONE);
        assert_eq!(vm.handle_key(ev), Some(Action::CloseScreen));
    }

    #[test]
    fn unbound_plain_key_is_unhandled_here() {
        let mut vm = VarManager::default();
        let ev = KeyEvent::new(
            KeyCode::Char('q'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        );
        assert_eq!(vm.handle_key(ev), None);
    }
}
