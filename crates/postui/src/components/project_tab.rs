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

#[derive(Debug, Default)]
pub struct ProjectTab {
    /// Whether the keyboard cursor is drawn, as the Settings tab's
    /// `focused`: a click away clears it, and a key or click on the tab,
    /// an added header, or the tab coming up, sets it.
    pub focused: bool,
    /// The Name row's live edit, while it is open.
    name: Option<LineInput>,
    /// The default headers' table: its open cell, its view, and the tab's
    /// cursor. The Name row is the head of the table's walk
    /// ([`TableEditorState::handle_key_walk`]), so the cursor is on it when
    /// the table has nothing selected ([`Self::on_name`]).
    pub table: TableEditorState,
    /// The last frame had no room for the tab ([`Self::draw`]) and drew
    /// none of it, so nothing on it acts at rest (R19).
    pub hidden: bool,
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

    /// Whether the cursor is on the Name row: the table has nothing
    /// selected.
    pub fn on_name(&self) -> bool {
        self.table.selected().is_none()
    }

    /// Opens the Name row's edit, seeded with `seed`, the caret at its end.
    pub fn begin_name(&mut self, seed: &str) {
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

    /// The header row under the table's cursor, when it is on one: the
    /// stop drawn lit ([`TableEditorState::settle_view`]).
    pub fn selected_row(&self, len: usize) -> Option<usize> {
        match self.table.selected().map(|s| s.clamp(len, 0)) {
            Some(Stop::Row(i)) => Some(i),
            _ => None,
        }
    }

    /// What a list verb means here (`App::selection_action`): add a header
    /// from anywhere on the tab, the Name row included (R12), and delete or
    /// copy the header under the cursor (R13). Nothing else applies, and
    /// nothing at all on a tab the last frame didn't draw (R19).
    pub fn verb_action(&self, len: usize, verb: Verb) -> Option<Action> {
        if self.hidden {
            return None;
        }
        match verb {
            Verb::Add => Some(Action::AddDefaultHeader),
            Verb::Delete { .. } => self.selected_row(len).map(Action::DeleteDefaultHeader),
            Verb::Yank => self.selected_row(len).map(Action::CopyDefaultHeader),
            _ => None,
        }
    }

    /// The tab's footer chips. A live edit advertises its own pair, and a
    /// tab the last frame didn't draw only the way back (R19).
    pub fn footer_chips(&self, len: usize) -> Vec<(Key, &'static str, Option<Action>)> {
        if self.field().is_some() {
            return vec![
                (Key::Named("enter"), "save", None),
                (Key::Named("esc"), "done", Some(Action::CloseField)),
            ];
        }
        let back = (Key::Named("esc"), "back", Some(Action::CloseScreen));
        if self.hidden {
            return vec![back];
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
            chips.push(chip(Verb::Yank, "copy"));
            chips.push((Key::Named("space"), "toggle", None));
            chips.push(chip(Verb::Delete { count: 1 }, "delete"));
        }
        chips.push(back);
        chips
    }

    /// Paints the tab: the Name row, then the default headers' table,
    /// settled first by the request table's rules
    /// ([`TableEditorState::settle_view`]): a cursor the rows went from
    /// under (an undo, a reload) goes on the last row still there (R16),
    /// and the view keeps the cursor and an open cell in sight. `name` is
    /// the project's given name
    /// (`Project::given_name`: none when blank), and `folder` the name it
    /// shows without one. `ctx.focused` is ignored: the tab's own
    /// [`Self::focused`] says whether its cursor is drawn.
    ///
    /// The tab is drawn whole or not at all: a body with no room for the
    /// Name row and two lines of the table draws none of it, and records
    /// that in [`Self::hidden`] (R19).
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
        // Two heading lines, the Name row and the gaps take the first 7.
        self.hidden = body.width < 24 || body.height < 9;
        if self.hidden {
            self.table.settle_view(headers.len(), 0, 0);
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
            let state = if self.name.is_some() || self.focused && self.on_name() {
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
        heading(frame.buffer_mut(), x0, y, "Default headers", ctx);
        y += 2;

        let height = table_height(headers.len()).min(bottom.saturating_sub(y));
        let area = Rect { x: x0, y, width: row_w, height };
        self.table.settle_view(headers.len(), 0, usize::from(height));
        let table_ctx = DrawCtx {
            theme,
            focused: self.focused && !self.on_name(),
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
