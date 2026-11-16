//! The Manage screen's Project tab: the open project's own settings in
//! `project.toml`, its name and its default headers (spec 2026-11-15
//! project settings).
//!
//! The tab holds only where its cursor is and what is being typed. Every
//! write is `App`'s: a committed name or a change to the header table
//! writes `project.toml` at once, as one undo step (R7, R9), so there is
//! no save step and nothing here goes stale behind the file.

use crate::action::{Action, Verb};
use crate::components::DrawCtx;
use crate::components::line_input::LineInput;
use crate::components::table_editor::{Stop, TableEditorState, TableOwner, table_height};
use crate::components::var_tokens::VarView;
use crate::hit::{Hit, HitMap};
use crate::keyroute::FieldId;
use crate::keyroute::spell::{Key, Nav};
use crate::paint::{ControlState, PROPERTY_MAX_W, PropertyRow, WELL_PAD, Well, fill, text};
use indexmap::IndexMap;
use postui_core::model::Entry;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;

/// The Name row's label, and the widest label the tab has.
const NAME_LABEL: &str = "Name";

/// The ghost row's label in the header table.
pub const ADD_HEADER: &str = "+ Add header";

/// Where the tab's keyboard cursor is: the Name row, or the header table
/// (whose own stop says which row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectStop {
    #[default]
    Name,
    Table,
}

#[derive(Debug, Default)]
pub struct ProjectTab {
    /// The stop goes to the table only with a row selected there, so the
    /// table stop always has a lit row.
    pub stop: ProjectStop,
    /// Whether the keyboard cursor is drawn, as the Settings tab's
    /// `focused`: a click away clears it, and a key or click on the tab,
    /// an added header, or the tab coming up, sets it.
    pub focused: bool,
    /// The Name row's live edit, while it is open.
    name: Option<LineInput>,
    /// The default headers' table: its cursor, its open cell and its view.
    pub table: TableEditorState,
}

impl ProjectTab {
    /// Starts over, as at launch: a project switch, whose rows are another
    /// project's. Whether the tab is focused stays.
    pub fn reset(&mut self) {
        *self = Self { focused: self.focused, ..Self::default() };
    }

    /// The field with the caret on this tab, if one is open.
    pub fn field(&self) -> Option<FieldId> {
        if self.name.is_some() {
            return Some(FieldId::ProjectName);
        }
        self.table
            .editing
            .as_ref()
            .map(|e| FieldId::ProjectCell { row: e.row, col: usize::from(e.col.index()) })
    }

    /// Opens the Name row's edit, seeded with `seed`, the caret at its end.
    pub fn begin_name(&mut self, seed: &str) {
        self.stop = ProjectStop::Name;
        self.table.set_selected(None);
        self.name = Some(LineInput::new(seed));
    }

    pub fn name_open(&self) -> bool {
        self.name.is_some()
    }

    /// The Name row's live edit, for typing and for the vim engine.
    pub fn name_input_mut(&mut self) -> Option<&mut LineInput> {
        self.name.as_mut()
    }

    /// Closes the Name row's edit, handing back what it held.
    pub fn take_name(&mut self) -> Option<LineInput> {
        self.name.take()
    }

    /// Puts a closed edit back, as it was: a write that was refused keeps
    /// the typing (R14).
    pub fn restore_name(&mut self, input: LineInput) {
        self.name = Some(input);
    }

    /// Drops the open name or cell and its typing, unsaved.
    pub fn end_edits(&mut self) {
        self.name = None;
        self.table.editing = None;
    }

    /// The open field's selected text, for ctrl+c.
    pub fn selected_text(&self) -> Option<String> {
        match &self.name {
            Some(input) => input.selected_text(),
            None => self.table.editing.as_ref()?.input.selected_text(),
        }
    }

    /// The cursor onto the Name row.
    pub fn to_name(&mut self) {
        self.stop = ProjectStop::Name;
        self.table.set_selected(None);
    }

    /// The cursor onto the table's first stop.
    pub fn to_table(&mut self, len: usize) {
        self.stop = ProjectStop::Table;
        self.table.set_selected(Some(Stop::of_row(0, len)));
    }

    /// The header row under the table's cursor, when it is on one: the
    /// stop drawn lit ([`Self::settle`]).
    pub fn selected_row(&self, len: usize) -> Option<usize> {
        match (self.stop, self.table.selected().map(|s| s.clamp(len, 0))) {
            (ProjectStop::Table, Some(Stop::Row(i))) => Some(i),
            _ => None,
        }
    }

    /// Settles the table for a frame whose view is `viewport` lines tall,
    /// by the request table's rules ([`TableEditorState::settle_view`]): a
    /// cursor the rows went from under (an undo, a reload) goes on the
    /// last row still there (R16), and the view keeps the cursor and an
    /// open cell in sight. On the Name row the table has no cursor.
    fn settle(&mut self, len: usize, viewport: usize) {
        self.table.settle_view(len, 0, viewport);
    }

    /// What a list verb means here (`App::selection_action`): add a header
    /// from anywhere on the tab, the Name row included (R12), and delete
    /// the header under the cursor. Nothing else applies.
    pub fn verb_action(&self, len: usize, verb: Verb) -> Option<Action> {
        match verb {
            Verb::Add => Some(Action::AddDefaultHeader),
            Verb::Delete { .. } => self.selected_row(len).map(Action::DeleteDefaultHeader),
            _ => None,
        }
    }

    /// The tab's footer chips. A live edit advertises its own pair.
    pub fn footer_chips(&self, len: usize) -> Vec<(Key, &'static str, Option<Action>)> {
        if self.field().is_some() {
            return vec![
                (Key::Named("enter"), "save", None),
                (Key::Named("esc"), "done", Some(Action::CloseField)),
            ];
        }
        let chip = |verb: Verb, label: &'static str| {
            (Key::Of(Action::OnSelection(verb)), label, self.verb_action(len, verb))
        };
        let mut chips = vec![
            (Key::Nav(Nav::UpDown), "move", None),
            (Key::Named("enter"), "edit", None),
            chip(Verb::Add, "add"),
        ];
        if self.selected_row(len).is_some() {
            chips.push((Key::Named("space"), "toggle", None));
            chips.push(chip(Verb::Delete { count: 1 }, "delete"));
        }
        chips.push((Key::Named("esc"), "back", Some(Action::CloseScreen)));
        chips
    }

    /// Paints the tab: the Name row, then the default headers' table,
    /// settled first ([`Self::settle`]). `folder` is the name the project
    /// shows with no `name` set. `ctx.focused` is ignored: the tab's own
    /// [`Self::focused`] says whether its cursor is drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        body: Rect,
        ctx: &DrawCtx,
        name: Option<&str>,
        folder: &str,
        headers: &IndexMap<String, Entry>,
        vars: &VarView,
        hits: &mut HitMap,
    ) {
        let theme = ctx.theme;
        fill(frame.buffer_mut(), body, theme.page);
        if body.width < 24 || body.height < 6 {
            self.settle(headers.len(), 0);
            return;
        }
        let x0 = body.x + 2;
        let row_w = body.width.saturating_sub(4).clamp(1, PROPERTY_MAX_W);
        let bottom = body.y + body.height;
        let mut y = body.y + 1;

        let buf = frame.buffer_mut();
        heading(buf, x0, y, "Project", ctx);
        y += 2;
        let rect = Rect { x: x0, y, width: row_w, height: 1 };
        let slot = PropertyRow {
            label: NAME_LABEL,
            label_w: crate::paint::label_column(&[NAME_LABEL]),
            hovered: ctx.hovered == Some(&Hit::ProjectName),
            disabled: false,
            trailing: &[],
        }
        .paint(buf, hits, rect, theme);
        let width = slot.rect.width.min(40);
        if width >= 6 {
            let well = Rect { width, ..slot.rect };
            let inner = width.saturating_sub(WELL_PAD * 2);
            let content: Line<'static> = match (&self.name, name) {
                (Some(input), _) => input.draw_line_windowed(true, theme, inner),
                (None, Some(n)) => LineInput::new(n).draw_line_at_rest(theme, theme.text, inner),
                // The folder name is what the project shows as (R5).
                (None, None) => {
                    Line::styled(folder.to_string(), Style::default().fg(theme.text_muted))
                }
            };
            let state = if self.name.is_some() || self.focused && self.stop == ProjectStop::Name {
                ControlState::Focused
            } else if ctx.hovered == Some(&Hit::ProjectName) {
                ControlState::Hover
            } else {
                ControlState::Normal
            };
            Well { content, state }.paint(buf, well, theme);
            hits.register(well, Hit::ProjectName);
        }
        y += 2;
        if y + 3 >= bottom {
            self.settle(headers.len(), 0);
            return;
        }
        heading(frame.buffer_mut(), x0, y, "Default headers", ctx);
        y += 2;

        let height = table_height(headers.len()).min(bottom.saturating_sub(y));
        let area = Rect { x: x0, y, width: row_w, height };
        self.settle(headers.len(), usize::from(height));
        let table_ctx = DrawCtx {
            theme,
            focused: self.focused && self.stop == ProjectStop::Table,
            hovered: ctx.hovered,
            pointer: ctx.pointer,
            dragging: false,
            anims: ctx.anims,
            now: ctx.now,
        };
        let (skip, owner) = (self.table.scroll, TableOwner::Project);
        self.table.draw(frame, area, headers, &table_ctx, ADD_HEADER, hits, &[], vars, skip, owner);
    }
}

fn heading(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, label: &str, ctx: &DrawCtx) {
    text(buf, x, y, label, ctx.theme.accent, ctx.theme.page, true);
}
