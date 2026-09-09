//! The Manage screen's Settings tab: the app's global settings, the one
//! tab that is not about the open project.
//!
//! Changes apply and persist immediately -- there is no save step. Save
//! and discard are unavailable anyway (the header slot they would use
//! holds Reload on this screen), and undo is project-scoped, so folding
//! config changes into it would interleave "undo my hover-hints change"
//! with "undo my request edit" in one stack. Neither is needed: a
//! checkbox toggled by accident is undone by clicking it again, and text
//! fields commit on enter and cancel on esc. The two Reset buttons are
//! the only hard-to-reverse actions, and both are behind confirms.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;

use crate::action::{Action, ConfigFile};
use crate::components::line_input::LineInput;
use crate::config::{JqTab, UiSettings};
use crate::hit::{Hit, HitMap};
use crate::paint::{
    ButtonKind, ControlSlot, ControlState, PROPERTY_MAX_W, Pill, PropertyRow, Toggle, WELL_PAD,
    Well, fill, label_column, pill_min_width, text,
};
use crate::theme::Theme;

/// Which control a row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Animations,
    HoverHints,
    JqTab,
    AiCmd,
    AiConfirmed,
    ClipboardCmd,
    Osc52Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    Setting(SettingsField),
    /// A file's Edit… and Reset pair.
    File(ConfigFile),
}

impl SettingsField {
    /// The `config.toml` key this row writes. `jq_tab` is a string key
    /// even though its control is a two-state one, so every row's key is
    /// exactly the name a user would hand-edit.
    pub fn key(self) -> &'static str {
        match self {
            SettingsField::Animations => "animations",
            SettingsField::HoverHints => "hover_hints",
            SettingsField::JqTab => "jq_tab",
            SettingsField::AiCmd => "ai_cmd",
            SettingsField::AiConfirmed => "ai_confirmed",
            SettingsField::ClipboardCmd => "clipboard_cmd",
            SettingsField::Osc52Limit => "osc52_limit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SettingsField::Animations => "Animations",
            SettingsField::HoverHints => "Hover hints",
            SettingsField::JqTab => "jq Tab behavior",
            SettingsField::AiCmd => "AI command",
            SettingsField::AiConfirmed => "Ask before sending to AI",
            SettingsField::ClipboardCmd => "Clipboard command",
            SettingsField::Osc52Limit => "OSC 52 limit",
        }
    }

    /// The checkbox rows' tick state, or `None` for a row that carries
    /// some other control. `ai_confirmed` is deliberately inverted: the
    /// row is worded as the consent question ("Ask before sending to
    /// AI"), so a *ticked* box means the flag is `false`.
    pub fn checkbox(self, ui: &UiSettings) -> Option<bool> {
        match self {
            SettingsField::Animations => Some(ui.animations),
            SettingsField::HoverHints => Some(ui.hover_hints),
            SettingsField::AiConfirmed => Some(!ui.ai_confirmed),
            _ => None,
        }
    }

    /// The text rows' current value, or `None` for a row that carries
    /// some other control. An unset `clipboard_cmd` reads as empty --
    /// the field paints "(not set)" for it.
    pub fn text_value(self, ui: &UiSettings) -> Option<String> {
        match self {
            SettingsField::AiCmd => Some(ui.ai_cmd.clone()),
            SettingsField::ClipboardCmd => Some(ui.clipboard_cmd.clone().unwrap_or_default()),
            SettingsField::Osc52Limit => Some(ui.osc52_limit.to_string()),
            _ => None,
        }
    }
}

impl SettingsRow {
    pub fn label(self) -> &'static str {
        match self {
            SettingsRow::Setting(f) => f.label(),
            SettingsRow::File(ConfigFile::Config) => "Settings (config.toml)",
            SettingsRow::File(ConfigFile::Keys) => "Key bindings (keys.toml)",
        }
    }
}

#[derive(Default)]
pub struct SettingsTab {
    pub cursor: usize,
    /// The field whose `TextField` is live, if any. A live edit owns the
    /// keyboard, exactly as the Manage grid's cell edit does.
    pub editing: Option<SettingsField>,
    /// Which of a File row's two buttons is chosen: 0 = Edit…, 1 = Reset.
    pub file_button: usize,
    /// Whether the keyboard cursor belongs to this tab right now.
    ///
    /// With no selection band on a property row, the cursor *is* the
    /// control lifting its own fill -- so a control left lifted after
    /// the user has clicked somewhere else says "type here" about a row
    /// that will no longer answer. A click away clears this (see
    /// `App::on_hit`); any click or key on the tab's own controls sets
    /// it, as does the tab becoming visible.
    pub focused: bool,
    /// What the live edit holds, and the `LineInput` that owns its
    /// caret, selection and word-nav. Both are private and always
    /// written together — [`SettingsTab::set_field_text`],
    /// [`SettingsTab::type_key`] and [`SettingsTab::paste`] are the only
    /// writers — because the commit reads the buffer while the paint
    /// reads the input: a caller that set one alone would commit text
    /// the user never saw, or see text the commit never reads.
    field_text: String,
    input: LineInput,
}

/// The two segments of the `jq_tab` control, in painted order.
const JQ_SEGMENTS: [(&str, JqTab); 2] = [("Menu", JqTab::Menu), ("Ghost", JqTab::Cycle)];

/// The `jq_tab` value written for each mode. `"ghost"` is the spelling
/// written going forward; `"cycle"` is still parsed as its older name.
pub fn jq_tab_spelling(mode: JqTab) -> &'static str {
    match mode {
        JqTab::Menu => "menu",
        JqTab::Cycle => "ghost",
    }
}

impl SettingsTab {
    pub fn rows() -> &'static [SettingsRow] {
        use ConfigFile::*;
        use SettingsField::*;
        &[
            SettingsRow::Setting(Animations),
            SettingsRow::Setting(HoverHints),
            SettingsRow::Setting(JqTab),
            SettingsRow::Setting(AiCmd),
            SettingsRow::Setting(AiConfirmed),
            SettingsRow::Setting(ClipboardCmd),
            SettingsRow::Setting(Osc52Limit),
            SettingsRow::File(Config),
            SettingsRow::File(Keys),
        ]
    }

    /// The first row the cursor may occupy. Normally `0`; while
    /// `config.toml` will not parse every *setting* row is disabled --
    /// it paints no highlight and registers no hit -- so a cursor above
    /// the Files rows would simply be invisible. The Files rows are the
    /// two ways out of a broken config and stay live, so the cursor
    /// lives among them until the file is fixed.
    pub fn first_live_row(editable: bool) -> usize {
        if editable {
            return 0;
        }
        Self::rows()
            .iter()
            .position(|r| matches!(r, SettingsRow::File(_)))
            .unwrap_or(0)
    }

    /// Lifts the cursor onto the first row that can actually show it --
    /// called wherever the tab becomes visible, so the very first paint
    /// after a broken-config open already has a visible cursor.
    pub fn clamp_to_live(&mut self, editable: bool) {
        let floor = Self::first_live_row(editable);
        if self.cursor < floor {
            self.cursor = floor;
            self.file_button = 0;
        }
    }

    /// Steps the cursor by `delta`, clamped at both ends -- the list
    /// does not wrap, so holding a key never rolls off one end onto the
    /// other. The bottom clamp is [`Self::first_live_row`], so with a
    /// broken config the cursor cannot walk up into the disabled rows
    /// and vanish.
    pub fn move_cursor(&mut self, delta: i32, editable: bool) {
        let n = Self::rows().len() as i32;
        let floor = Self::first_live_row(editable) as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(floor, n - 1) as usize;
        // Aim resets to Edit… on every row change: Reset is destructive,
        // and inheriting the previous row's aim would fire it on a file
        // the user never pointed at.
        self.file_button = 0;
    }

    /// What the live edit holds. The commit reads this.
    pub fn field_text(&self) -> &str {
        &self.field_text
    }

    /// Replaces the live edit's text, re-seeding the `LineInput` with it
    /// so the caret and the buffer cannot drift apart.
    pub fn set_field_text(&mut self, text: &str) {
        self.field_text = text.to_string();
        self.input = LineInput::new(text);
    }

    /// The row the cursor is on.
    pub fn row(&self) -> SettingsRow {
        Self::rows()[self.cursor.min(Self::rows().len() - 1)]
    }

    /// Puts the cursor on `field`'s row, for a click that landed on a
    /// control rather than on the row behind it.
    pub fn focus_field(&mut self, field: SettingsField) {
        self.focused = true;
        if let Some(i) = Self::rows()
            .iter()
            .position(|r| *r == SettingsRow::Setting(field))
        {
            self.cursor = i;
        }
    }

    /// Opens the live edit on `field`, seeded with `text`.
    pub fn begin_edit(&mut self, field: SettingsField, text: &str) {
        self.focus_field(field);
        self.editing = Some(field);
        self.set_field_text(text);
        self.input.select_all();
    }

    /// Drops the live edit, keeping nothing: the row goes back to
    /// painting the value on disk. Called on every way out of the tab
    /// too — a field left live behind a tab switch would go on owning
    /// ctrl+v and ctrl+c from a screen that no longer shows it.
    pub fn end_edit(&mut self) {
        self.editing = None;
        self.set_field_text("");
    }

    /// Forwards one key to the live edit's `LineInput` and re-syncs the
    /// buffer from it.
    pub fn type_key(&mut self, ev: KeyEvent) -> bool {
        let changed = self.input.handle_key(ev);
        self.field_text = self.input.text().to_string();
        changed
    }

    /// Pastes into the live edit, re-syncing the buffer from it.
    pub fn paste(&mut self, text: &str) {
        self.input.paste(text);
        self.field_text = self.input.text().to_string();
    }

    /// Where the live edit's caret sits, as a character index.
    pub fn caret(&self) -> usize {
        self.input.cursor()
    }

    /// The live edit's content line, windowed to `width` columns with
    /// its caret shown — what the well paints while this field is under
    /// edit. `LineInput::draw_line_windowed` styles the caret and the
    /// selection from the theme, so it needs one.
    pub fn input_line(&self, theme: &Theme, width: u16) -> Line<'static> {
        self.input.draw_line_windowed(true, theme, width)
    }

    /// Places the caret for a click inside the live edit's well.
    ///
    /// `col` is the column *within the well's content area* and
    /// `inner_w` that area's width, so the caller maps through
    /// [`WELL_PAD`] once and this maps through the input's own scroll
    /// window. `already_editing` says whether the well was live before
    /// this click: a freshly opened one draws its window from 0, so the
    /// two cases resolve different indices for the same column.
    /// `double` selects the word instead of placing a bare caret.
    ///
    /// Neither path changes the text, so `field_text` stays in sync
    /// with `input` without being rewritten.
    pub fn click_caret(&mut self, col: usize, inner_w: u16, already_editing: bool, double: bool) {
        let idx = self.input.window_start(already_editing, inner_w) + col;
        if double {
            self.input.select_word_at(idx);
        } else {
            self.input.set_cursor(idx);
            self.input.begin_mouse_selection();
        }
    }

    /// Extends the live edit's mouse selection to `col` within a well
    /// content area of width `inner_w`. The sweep's other end was
    /// anchored by [`Self::click_caret`].
    pub fn drag_caret_to(&mut self, col: usize, inner_w: u16) {
        let idx = self.input.window_start(true, inner_w) + col;
        self.input.extend_mouse_selection_to(idx);
    }

    /// The live edit's selected text, for ctrl+c.
    pub fn selected_text(&self) -> Option<String> {
        self.editing.and(self.input.selected_text())
    }

    /// The focused row's keys, for the footer. A live edit owns the
    /// keyboard, so its own pair is advertised instead -- every letter
    /// types into the field and the row chips would all be dead.
    pub fn footer_chips(&self) -> Vec<(&'static str, &'static str, Option<Action>)> {
        if self.editing.is_some() {
            return vec![("enter", "save", None), ("esc", "cancel", None)];
        }
        let mut chips = vec![("↑↓", "move", None), ("enter", "change", None)];
        if matches!(self.row(), SettingsRow::File(_)) {
            // Names what the key dispatches, not the two buttons it
            // aims between: ←→ moves the selection, enter is what
            // actually edits or resets.
            chips.push(("←→", "select button", None));
        }
        chips
    }
}

/// `osc52_limit`'s validator. Rejected rather than coerced: a silent 0
/// would disable OSC 52 copying without ever saying so.
pub fn parse_osc52_limit(text: &str) -> Result<usize, String> {
    text.trim()
        .parse::<usize>()
        .map_err(|_| "expected a size in bytes, e.g. 65536".to_string())
}

/// Paints the tab: the seven settings, then the Files section's two
/// Edit…/Reset rows. Every row registers `Hit::SettingsRow`, and every
/// control its own hit on top of it, so the mouse reaches exactly what
/// the keyboard does.
///
/// `config_error` is `Some` when a *reload* has found `config.toml`
/// broken since it was last read: startup can no longer begin in this
/// state (it blocks behind a modal instead), but a reload can enter it
/// while the tab is open. The values shown are then the ones the session
/// started with -- not what is on disk, which no longer parses -- so
/// every setting row is dimmed and registers no hit (a write would be
/// refused by `Config::edit` anyway). Both of a file row's buttons stay
/// live: Edit… is the way to fix the file by hand, and Reset goes
/// through `ForceResetConfigFile`, which is written for exactly this
/// state. Dimming either would remove one of the two ways out.
#[allow(clippy::too_many_arguments)]
pub fn draw_settings(
    frame: &mut Frame,
    body: Rect,
    theme: &Theme,
    tab: &SettingsTab,
    ui: &UiSettings,
    config_error: Option<&str>,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let editable = config_error.is_none();
    let buf = frame.buffer_mut();
    fill(buf, body, theme.page);
    if body.width < 32 || body.height < 6 {
        return;
    }
    let x0 = body.x + 2;
    // Two columns of inset each side, the same shape the Variables and
    // Environments panes use -- so a row's label starts exactly under
    // its section heading and the three panes line up with each other.
    let row_w = body.width.saturating_sub(4).clamp(1, PROPERTY_MAX_W);
    let labels: Vec<&str> = SettingsTab::rows().iter().map(|r| r.label()).collect();
    let label_w = label_column(&labels);
    let bottom = body.y + body.height;
    let mut y = body.y + 1;
    let mut files_started = false;

    if let Some(err) = config_error {
        y = draw_broken_config_banner(buf, x0, y, row_w, bottom, err, theme);
        if y >= bottom {
            return;
        }
    }

    heading(buf, x0, y, "Settings", theme);
    y += 2;

    for (i, row) in SettingsTab::rows().iter().enumerate() {
        if matches!(row, SettingsRow::File(_)) && !files_started {
            files_started = true;
            if y + 3 >= bottom {
                return;
            }
            y += 1;
            heading(buf, x0, y, "Files", theme);
            y += 2;
        }
        if y >= bottom {
            return;
        }
        let rect = Rect {
            x: x0,
            y,
            width: row_w,
            height: 1,
        };
        // File rows are not governed by `editable` at all: Edit… and
        // Reset are the two ways *out* of a broken config.toml, so they
        // stay exactly as live as always. Only a setting row — whose
        // write genuinely would be refused by `Config::edit` — is
        // disabled.
        let row_disabled = !editable && matches!(row, SettingsRow::Setting(_));
        // A row washes when the pointer is over anything that row will
        // respond to (see `paint::property`'s module doc). This row
        // activates from its label half *and* from its control, so the
        // wash has to follow both: washing only for the label would say
        // the pointer had left a row it is still going to act on.
        let hovered_row = match hovered {
            Some(Hit::SettingsRow(h)) => *h == i,
            Some(Hit::SettingsControl(f)) => *row == SettingsRow::Setting(*f),
            Some(Hit::SettingsJqTab(_)) => *row == SettingsRow::Setting(SettingsField::JqTab),
            Some(Hit::SettingsFile { file, .. }) => *row == SettingsRow::File(*file),
            _ => false,
        };
        // A disabled setting row registers no hit at all: a control that
        // looks live and silently refuses every write is the defect the
        // banner above exists to remove.
        //
        // Registered *before* the row's control, because `HitMap`
        // resolves the last registration containing the point: a
        // row-wide hit landing after the pills would swallow every click
        // on Edit… and Reset.
        if !row_disabled {
            hits.register(rect, Hit::SettingsRow(i));
        }
        // No trailing pills on this tab: a File row's Edit… and Reset
        // are one pair, and a pair split across the row -- one pill at
        // the control column, its partner pinned to the right edge --
        // reads as two unrelated buttons. They go in the control slot
        // together, where every other row's control lives.
        let slot = PropertyRow {
            label: row.label(),
            label_w,
            hovered: hovered_row,
            disabled: row_disabled,
            trailing: &[],
        }
        .paint(buf, hits, rect, theme);
        match row {
            SettingsRow::Setting(field) => draw_setting_control(
                buf, hits, hovered, theme, tab, ui, *field, &slot, i, editable,
            ),
            SettingsRow::File(file) => {
                draw_file_buttons(buf, hits, hovered, theme, tab, i, *file, &slot)
            }
        }
        y += 1;
    }
}

/// The parse-error banner painted when a reload has left `config.toml`
/// broken. Returns the `y` the caller should resume painting at.
fn draw_broken_config_banner(
    buf: &mut Buffer,
    x0: u16,
    mut y: u16,
    row_w: u16,
    bottom: u16,
    err: &str,
    theme: &Theme,
) -> u16 {
    let w = row_w.saturating_sub(2) as usize;
    text(
        buf,
        x0,
        y,
        // `config_error` covers an unreadable file as well as an
        // unparsable one, so the banner does not claim which; `err`
        // itself, painted just below, says.
        "config.toml could not be loaded -- showing the settings this session started with:",
        theme.error,
        theme.page,
        true,
    );
    y += 1;
    if y >= bottom {
        return y;
    }
    let shown: String = err.chars().take(w).collect();
    text(buf, x0, y, &shown, theme.text_muted, theme.page, false);
    y + 2
}

fn heading(buf: &mut Buffer, x: u16, y: u16, label: &str, theme: &Theme) {
    text(buf, x, y, label, theme.accent, theme.page, true);
}

/// The `(state)` a File row's button `which` (0 = Edit…, 1 = Reset)
/// paints with: the aimed one lifts when the cursor is on this row.
fn file_button_state(
    tab: &SettingsTab,
    row: usize,
    which: usize,
    hovered: Option<&Hit>,
    file: ConfigFile,
) -> ControlState {
    let hit = Hit::SettingsFile {
        file,
        reset: which == 1,
    };
    if tab.focused && tab.cursor == row && tab.file_button == which {
        ControlState::Focused
    } else if hovered == Some(&hit) {
        ControlState::Hover
    } else {
        ControlState::Normal
    }
}

/// A File row's Edit… and Reset, side by side from the control column.
/// Neither is gated on `editable`: they are the two ways *out* of a
/// broken `config.toml` -- Edit… fixes it by hand, and Reset works even
/// then through `ForceResetConfigFile`. Disabling either would remove
/// one of the two ways out.
///
/// A pill that will not fit is dropped rather than clipped or painted
/// over its neighbour; it stays reachable by key, exactly as
/// `PropertyRow` drops a trailing pill that would reach the label.
#[allow(clippy::too_many_arguments)]
fn draw_file_buttons(
    buf: &mut Buffer,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
    theme: &Theme,
    tab: &SettingsTab,
    row: usize,
    file: ConfigFile,
    slot: &ControlSlot,
) {
    let mut x = slot.rect.x;
    let right = slot.rect.x + slot.rect.width;
    for (which, label) in [(0usize, "Edit\u{2026}"), (1, "Reset")] {
        let w = pill_min_width(label);
        if x + w > right {
            return;
        }
        let rect = Rect {
            x,
            width: w,
            ..slot.rect
        };
        Pill {
            label,
            kind: ButtonKind::Secondary,
            state: file_button_state(tab, row, which, hovered, file),
        }
        .paint(buf, rect, theme);
        hits.register(
            rect,
            Hit::SettingsFile {
                file,
                reset: which == 1,
            },
        );
        x += w + 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_setting_control(
    buf: &mut Buffer,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
    theme: &Theme,
    tab: &SettingsTab,
    ui: &UiSettings,
    field: SettingsField,
    slot: &ControlSlot,
    row: usize,
    editable: bool,
) {
    if slot.rect.width < 4 {
        return;
    }
    let hit = Hit::SettingsControl(field);
    // The cursor is this control lifting its own fill — with no band,
    // it is the only thing that says where the keyboard is. A live edit
    // counts as focused too: it *is* the cursor.
    let state = || {
        if !editable {
            ControlState::Disabled
        } else if tab.focused && tab.cursor == row && tab.editing.is_none() {
            ControlState::Focused
        } else if hovered == Some(&hit) {
            ControlState::Hover
        } else {
            ControlState::Normal
        }
    };

    if field == SettingsField::JqTab {
        let mut x = slot.rect.x;
        for (label, mode) in JQ_SEGMENTS {
            let seg = Hit::SettingsJqTab(mode);
            let w = pill_min_width(label);
            if x + w > slot.rect.x + slot.rect.width {
                return;
            }
            // The active segment is Primary; the cursor still lifts
            // whichever segment the pointer or keyboard is on.
            let kind = if ui.jq_tab == mode {
                ButtonKind::Primary
            } else {
                ButtonKind::Secondary
            };
            // Focused before Hover, like every other ladder on this
            // screen: with the band gone the cursor is the only thing
            // saying where the keyboard is, and it must not vanish
            // under the pointer that happens to be resting on it.
            let seg_state = if !editable {
                ControlState::Disabled
            } else if tab.focused && tab.cursor == row && ui.jq_tab == mode {
                ControlState::Focused
            } else if hovered == Some(&seg) {
                ControlState::Hover
            } else {
                ControlState::Normal
            };
            let rect = Rect {
                x,
                width: w,
                ..slot.rect
            };
            Pill {
                label,
                kind,
                state: seg_state,
            }
            .paint(buf, rect, theme);
            if editable {
                hits.register(rect, seg);
            }
            x += w + 1;
        }
        return;
    }

    if let Some(on) = field.checkbox(ui) {
        // The toggle clamps itself to the slot and hands back what it
        // painted: the hit goes over that, never over a width this row
        // may not have.
        let rect = Toggle { on, state: state() }.paint(buf, slot.rect, theme);
        if editable {
            hits.register(rect, hit);
        }
        return;
    }

    // The three text rows: a one-row well holding the live `LineInput`
    // while this field is under edit, else the value on disk.
    let editing = tab.editing == Some(field);
    let width = slot.rect.width.min(40);
    if width < 6 {
        return;
    }
    let rect = Rect { width, ..slot.rect };
    let well_state = if !editable {
        ControlState::Disabled
    } else if editing || (tab.focused && tab.cursor == row && tab.editing.is_none()) {
        ControlState::Focused
    } else if hovered == Some(&hit) {
        ControlState::Hover
    } else {
        ControlState::Normal
    };
    let inner = width.saturating_sub(WELL_PAD * 2);
    let content: Line<'static> = if editing {
        tab.input_line(theme, inner)
    } else {
        match field.text_value(ui).unwrap_or_default() {
            // Muted unconditionally: a disabled well is
            // `ControlState::Disabled`, and `Well::paint` recolors every
            // span it is handed, so a second disabled colour here would
            // never reach the screen.
            v if v.is_empty() => Line::styled("(not set)", Style::default().fg(theme.text_muted)),
            v => Line::raw(v),
        }
    };
    Well {
        content,
        state: well_state,
    }
    .paint(buf, rect, theme);
    if editable {
        hits.register(rect, hit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Every row is reachable by keyboard, the two Files rows included --
    /// this tab is not a mouse-only surface.
    #[test]
    fn the_cursor_reaches_every_row_including_the_files_rows() {
        let mut tab = SettingsTab::default();
        let n = SettingsTab::rows().len();
        assert!(n >= 9, "7 settings + 2 file rows");
        for _ in 0..n * 2 {
            tab.move_cursor(1, true);
        }
        assert_eq!(tab.cursor, n - 1, "the cursor clamps at the bottom");
        for _ in 0..n * 2 {
            tab.move_cursor(-1, true);
        }
        assert_eq!(tab.cursor, 0, "and at the top -- the list never wraps");
        tab.cursor = n - 1;
        assert!(
            matches!(SettingsTab::rows()[tab.cursor], SettingsRow::File(_)),
            "the last row is a file row"
        );
    }

    /// osc52_limit is the one field whose text is not free-form. Bad input
    /// is rejected outright: never coerced, never silently zeroed.
    #[test]
    fn osc52_rejects_input_that_is_not_a_size() {
        assert_eq!(parse_osc52_limit("1024"), Ok(1024));
        assert_eq!(parse_osc52_limit(" 65536 "), Ok(65536));
        assert!(parse_osc52_limit("abc").is_err());
        assert!(parse_osc52_limit("-1").is_err());
        assert!(parse_osc52_limit("").is_err());
        assert!(parse_osc52_limit("99999999999999999999999999").is_err());
    }

    /// The tab publishes its own chips; inheriting the previous tab's would
    /// advertise keys that do nothing here.
    #[test]
    fn the_tab_publishes_its_own_footer_chips() {
        let chips = SettingsTab::default().footer_chips();
        assert!(!chips.is_empty(), "a focused area advertises its keys");
        assert!(chips.iter().any(|(k, _, _)| *k == "enter"));
        // The focused row is a settings row, so exactly the two row keys
        // are advertised: not the Files pair, and not a list tab's
        // new/rename/delete set carried over.
        let keys: Vec<_> = chips.iter().map(|(k, l, _)| (*k, *l)).collect();
        assert_eq!(keys, vec![("↑↓", "move"), ("enter", "change")]);
    }

    /// A live edit owns the keyboard, so the chips advertise its keys
    /// instead -- the same pair the Manage grid already shows.
    #[test]
    fn a_live_edit_advertises_commit_and_cancel() {
        let tab = SettingsTab {
            editing: Some(SettingsField::AiCmd),
            ..Default::default()
        };
        let chips = tab.footer_chips();
        assert!(chips.iter().any(|(k, l, _)| *k == "enter" && *l == "save"));
        assert!(chips.iter().any(|(k, l, _)| *k == "esc" && *l == "cancel"));
    }

    /// A File row's two buttons are keyboard-aimed, so the footer says so
    /// there and nowhere else.
    #[test]
    fn only_a_file_row_advertises_the_left_right_pair() {
        let mut tab = SettingsTab::default();
        assert!(!tab.footer_chips().iter().any(|(k, _, _)| *k == "←→"));
        tab.cursor = SettingsTab::rows().len() - 1;
        assert!(tab.footer_chips().iter().any(|(k, _, _)| *k == "←→"));
    }

    /// Reset is destructive, so aiming it on one file must not arm it on
    /// the next: moving off a Files row re-aims at Edit….
    /// With a broken config every *setting* row is disabled and paints
    /// no highlight, so a cursor sitting up there is simply invisible.
    /// It lives among the Files rows -- the two ways out -- instead.
    #[test]
    fn a_broken_config_keeps_the_cursor_on_the_rows_that_still_work() {
        let first_file = SettingsTab::rows()
            .iter()
            .position(|r| matches!(r, SettingsRow::File(_)))
            .unwrap();
        assert_eq!(SettingsTab::first_live_row(true), 0);
        assert_eq!(SettingsTab::first_live_row(false), first_file);

        let mut tab = SettingsTab::default();
        tab.clamp_to_live(false);
        assert_eq!(tab.cursor, first_file, "opening lands on a visible row");

        // And it cannot walk back up into the invisible ones.
        for _ in 0..SettingsTab::rows().len() * 2 {
            tab.move_cursor(-1, false);
        }
        assert_eq!(tab.cursor, first_file);

        // A working config is unchanged: row 0 is reachable.
        let mut tab = SettingsTab {
            cursor: SettingsTab::rows().len() - 1,
            ..Default::default()
        };
        tab.clamp_to_live(true);
        for _ in 0..SettingsTab::rows().len() * 2 {
            tab.move_cursor(-1, true);
        }
        assert_eq!(tab.cursor, 0);
    }

    #[test]
    fn moving_off_a_file_row_re_aims_at_edit() {
        let mut tab = SettingsTab {
            cursor: SettingsTab::rows().len() - 2,
            file_button: 1,
            ..Default::default()
        };
        tab.move_cursor(1, true);
        assert_eq!(tab.file_button, 0, "the next file row starts on Edit…");
    }

    /// The live edit's buffer and its `LineInput` never disagree: the
    /// commit reads `field_text`, so a key that only reached the input
    /// would be silently dropped.
    #[test]
    fn typing_keeps_the_buffer_and_the_input_in_step() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let mut tab = SettingsTab::default();
        tab.begin_edit(SettingsField::AiCmd, "");
        for c in "llm -p".chars() {
            tab.type_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(tab.field_text(), "llm -p");
        tab.paste(" --json");
        assert_eq!(tab.field_text(), "llm -p --json");
        tab.end_edit();
        assert_eq!(tab.field_text(), "");
    }

    fn render(tab: &SettingsTab, ui: &UiSettings) -> HitMap {
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                let area = f.area();
                draw_settings(f, area, &theme, tab, ui, None, &mut hits, None);
            })
            .unwrap();
        hits
    }

    /// Every control the keyboard reaches is clickable too, the two file
    /// rows' four buttons included.
    #[test]
    fn every_control_registers_a_hit() {
        let hits = render(&SettingsTab::default(), &UiSettings::default());
        for (i, row) in SettingsTab::rows().iter().enumerate() {
            assert!(
                hits.rect_of(&Hit::SettingsRow(i)).is_some(),
                "row {i} is not clickable"
            );
            match row {
                SettingsRow::Setting(SettingsField::JqTab) => {
                    for (_, mode) in JQ_SEGMENTS {
                        assert!(hits.rect_of(&Hit::SettingsJqTab(mode)).is_some());
                    }
                }
                SettingsRow::Setting(f) => assert!(
                    hits.rect_of(&Hit::SettingsControl(*f)).is_some(),
                    "{f:?} has no control hit"
                ),
                SettingsRow::File(file) => {
                    for reset in [false, true] {
                        assert!(
                            hits.rect_of(&Hit::SettingsFile { file: *file, reset })
                                .is_some(),
                            "{file:?} reset={reset} has no button"
                        );
                    }
                }
            }
        }
    }

    /// The tab's rows are properties, not list items, so none of them
    /// may paint the list band. This is the assertion that keeps the
    /// band meaning "selected item in a list" everywhere else.
    #[test]
    fn no_settings_row_paints_the_list_band() {
        use crate::hit::HitMap;
        let theme = Theme::dark();
        let ui = UiSettings::default();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let tab = SettingsTab {
            cursor: 3,
            ..Default::default()
        };
        let mut hits = HitMap::default();
        term.draw(|f| {
            let body = Rect::new(0, 0, 100, 30);
            draw_settings(f, body, &theme, &tab, &ui, None, &mut hits, None);
        })
        .unwrap();
        let buf = term.backend().buffer();
        for y in 0..30 {
            for x in 0..100 {
                let cell = buf.cell((x, y)).unwrap();
                assert_ne!(cell.bg, theme.selection, "band at {x},{y}");
                assert_ne!(cell.symbol(), "▌", "accent bar at {x},{y}");
            }
        }
    }

    /// The cursor is the control lifting its own fill. With the band
    /// gone this is the *only* thing that says where the keyboard is,
    /// so it has to be unmistakably brighter than a resting control.
    #[test]
    fn the_cursor_row_lifts_its_own_control() {
        use crate::hit::HitMap;
        let theme = Theme::dark();
        let ui = UiSettings::default();
        let ai_row = SettingsTab::rows()
            .iter()
            .position(|r| *r == SettingsRow::Setting(SettingsField::AiCmd))
            .unwrap();
        let mut hits = HitMap::default();
        let tab = SettingsTab {
            cursor: ai_row,
            focused: true,
            ..Default::default()
        };
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| {
            draw_settings(
                f,
                Rect::new(0, 0, 100, 30),
                &theme,
                &tab,
                &ui,
                None,
                &mut hits,
                None,
            );
        })
        .unwrap();
        let well = hits
            .rect_of(&Hit::SettingsControl(SettingsField::AiCmd))
            .unwrap();
        let painted = term.backend().buffer().cell((well.x, well.y)).unwrap().bg;
        assert_eq!(
            painted,
            crate::theme::lift_color(theme.control, 0.12),
            "the cursor's control is lifted, not merely hovered"
        );
        assert_ne!(painted, theme.control_hover);
    }

    /// Focused beats Hover on the jq row too. The active segment is the
    /// one place the cursor rides a `Primary` face, and it used to lose
    /// to the pointer resting on it -- the keyboard cursor vanishing on
    /// exactly one row of the app.
    #[test]
    fn the_jq_segments_cursor_outlives_the_pointer_resting_on_it() {
        use crate::hit::HitMap;
        let theme = Theme::dark();
        let ui = UiSettings::default();
        let jq_row = SettingsTab::rows()
            .iter()
            .position(|r| *r == SettingsRow::Setting(SettingsField::JqTab))
            .unwrap();
        let tab = SettingsTab {
            cursor: jq_row,
            focused: true,
            ..Default::default()
        };
        // The pointer is on the very segment the keyboard is on.
        let active = Hit::SettingsJqTab(ui.jq_tab);
        let mut hits = HitMap::default();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| {
            draw_settings(
                f,
                Rect::new(0, 0, 100, 30),
                &theme,
                &tab,
                &ui,
                None,
                &mut hits,
                Some(&active),
            );
        })
        .unwrap();
        let seg = hits
            .rect_of(&active)
            .expect("the active segment is painted");
        let painted = term.backend().buffer().cell((seg.x, seg.y)).unwrap().bg;
        assert_eq!(
            painted,
            crate::theme::lift_color(theme.accent, 0.20),
            "the cursor still lifts the active segment"
        );
        assert_ne!(
            painted, theme.accent_edge_light,
            "the pointer must not paint over the cursor"
        );
    }

    /// The same ordering in the one ladder that is a plain function: the
    /// aimed file button stays Focused with the pointer on it, and the
    /// button beside it still shows Hover.
    #[test]
    fn the_aimed_file_button_stays_focused_under_the_pointer() {
        let row = SettingsTab::rows()
            .iter()
            .position(|r| matches!(r, SettingsRow::File(_)))
            .unwrap();
        let SettingsRow::File(file) = SettingsTab::rows()[row] else {
            unreachable!("the row we just found is a file row")
        };
        let tab = SettingsTab {
            cursor: row,
            file_button: 1,
            focused: true,
            ..Default::default()
        };
        let reset = Hit::SettingsFile { file, reset: true };
        let edit = Hit::SettingsFile { file, reset: false };
        assert_eq!(
            file_button_state(&tab, row, 1, Some(&reset), file),
            ControlState::Focused,
            "the aimed button keeps its cursor under the pointer"
        );
        assert_eq!(
            file_button_state(&tab, row, 0, Some(&edit), file),
            ControlState::Hover,
            "and the one beside it still answers the mouse"
        );
    }

    /// The hover wash follows the rule `paint::property` states: a row
    /// washes when the pointer is over anything it will respond to. This
    /// row activates from its label *and* from its control, so both must
    /// wash -- and a row the pointer is nowhere near must not.
    #[test]
    fn a_settings_row_washes_from_its_control_as_well_as_its_label() {
        use crate::hit::HitMap;
        let theme = Theme::dark();
        let ui = UiSettings::default();
        let ai_row = SettingsTab::rows()
            .iter()
            .position(|r| *r == SettingsRow::Setting(SettingsField::AiCmd))
            .unwrap();
        let wash = crate::theme::mix(theme.page, theme.control, crate::paint::HOVER_WASH);
        let label_bg = |hovered: Option<&Hit>| {
            let mut hits = HitMap::default();
            let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
            term.draw(|f| {
                draw_settings(
                    f,
                    Rect::new(0, 0, 100, 30),
                    &theme,
                    &SettingsTab::default(),
                    &ui,
                    None,
                    &mut hits,
                    hovered,
                );
            })
            .unwrap();
            let row = hits
                .rect_of(&Hit::SettingsRow(ai_row))
                .expect("the row is clickable");
            // A column in the label half, clear of every control.
            (
                term.backend().buffer().cell((row.x, row.y)).unwrap().bg,
                row,
            )
        };
        let (resting, _) = label_bg(None);
        assert_eq!(resting, theme.page, "a row nobody points at stays flat");
        let (from_label, _) = label_bg(Some(&Hit::SettingsRow(ai_row)));
        assert_eq!(from_label, wash, "the label half washes the row");
        let (from_control, _) = label_bg(Some(&Hit::SettingsControl(SettingsField::AiCmd)));
        assert_eq!(
            from_control, wash,
            "and so does the control -- the click activates the same row either way"
        );
        let (other_row, _) = label_bg(Some(&Hit::SettingsControl(SettingsField::HoverHints)));
        assert_eq!(
            other_row, theme.page,
            "a different row's control washes that row, not this one"
        );
    }

    /// Edit… and Reset are one pair, so they are painted as one: both in
    /// the control column, side by side, Reset immediately after Edit….
    /// Split across the row -- one at the label's edge, the other pinned
    /// to the pane's right -- they read as two unrelated buttons, which
    /// is the complaint this layout answers.
    #[test]
    fn a_file_rows_two_buttons_sit_together_in_the_control_column() {
        let hits = render(&SettingsTab::default(), &UiSettings::default());
        let labels: Vec<&str> = SettingsTab::rows().iter().map(|r| r.label()).collect();
        // The pane insets by two, and the control column starts one label
        // column in from there -- the same arithmetic `draw_settings` does.
        let control_x = 2 + label_column(&labels);
        for file in [ConfigFile::Config, ConfigFile::Keys] {
            let edit = hits
                .rect_of(&Hit::SettingsFile { file, reset: false })
                .expect("Edit… is painted");
            let reset = hits
                .rect_of(&Hit::SettingsFile { file, reset: true })
                .expect("Reset is painted");
            assert_eq!(
                edit.x, control_x,
                "{file:?}: Edit… starts at the control column, like every other row's control"
            );
            assert_eq!(edit.y, reset.y, "{file:?}: one row, not two");
            assert_eq!(
                reset.x,
                edit.x + edit.width + 1,
                "{file:?}: Reset follows Edit… with a single column between them"
            );
        }
    }

    /// The keyboard cursor is a control lifting its own fill, so it has
    /// to be droppable: after a click away (`SettingsTab::focused` goes
    /// false) no control on the tab may still be painting Focused, or it
    /// says "type here" about a row that will no longer answer.
    #[test]
    fn an_unfocused_tab_lifts_nothing_at_all() {
        let theme = Theme::dark();
        let ui = UiSettings::default();
        let render_bgs = |focused: bool| -> Vec<ratatui::style::Color> {
            let tab = SettingsTab {
                cursor: 0,
                focused,
                ..Default::default()
            };
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            let mut hits = HitMap::default();
            terminal
                .draw(|f| {
                    let area = f.area();
                    draw_settings(f, area, &theme, &tab, &ui, None, &mut hits, None);
                })
                .unwrap();
            let buf = terminal.backend().buffer();
            let mut bgs = Vec::new();
            for row in 0..SettingsTab::rows().len() {
                let r = hits.rect_of(&Hit::SettingsRow(row)).expect("row painted");
                for x in r.x..r.x + r.width {
                    bgs.push(buf[(x, r.y)].bg);
                }
            }
            bgs
        };
        let focused = render_bgs(true);
        let blurred = render_bgs(false);
        // Something *did* lift while focused -- otherwise the assertion
        // below would pass on an empty premise.
        let lifted = crate::theme::lift_color(theme.control, 0.12);
        assert!(
            focused.contains(&lifted),
            "the focused cursor lifts its control"
        );
        assert!(
            !blurred.contains(&lifted),
            "and nothing lifts once the tab has lost the cursor"
        );
    }

    /// A File row registers its own row hit *and* the Reset pill sitting
    /// on it. `HitMap` resolves the last registration to contain the
    /// point, so if the row-wide hit were registered after the pill,
    /// Reset would stop being clickable -- the button still painted, and
    /// a click on it landing on the row behind. `rect_of` cannot catch
    /// that (both registrations exist); only the point lookup can.
    #[test]
    fn a_file_rows_reset_button_wins_the_click_over_the_row_behind_it() {
        let hits = render(&SettingsTab::default(), &UiSettings::default());
        for file in [ConfigFile::Config, ConfigFile::Keys] {
            let reset = Hit::SettingsFile { file, reset: true };
            let rect = hits.rect_of(&reset).expect("Reset is painted");
            let landed = hits.hit_at(rect.x + rect.width / 2, rect.y);
            assert_eq!(
                landed,
                Some(&reset),
                "{file:?}: a click inside Reset must reach Reset, not the row behind it"
            );
        }
    }
}
