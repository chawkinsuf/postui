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
use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::action::{Action, ConfigFile};
use crate::components::line_input::LineInput;
use crate::config::{JqTab, UiSettings};
use crate::glyph;
use crate::hit::{Hit, HitMap};
use crate::paint::{ListRow, RowHighlight, fill, text};
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

    /// Steps the cursor by `delta`, clamped at both ends -- the list
    /// does not wrap, so holding a key never rolls off one end onto the
    /// other.
    pub fn move_cursor(&mut self, delta: i32) {
        let n = Self::rows().len() as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(0, n - 1) as usize;
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
            chips.push(("←→", "edit / reset", None));
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

/// The label column's width: every control starts here, so the rows read
/// as one list rather than as ragged label/control pairs.
const LABEL_W: u16 = 28;
/// The widest a settings row is painted, however wide the body is: a
/// text field stretched across a 200-column terminal is unreadable.
const MAX_W: u16 = 76;

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
/// refused by `Config::edit` anyway). The two file rows' Edit… stays
/// live, since fixing the file is the way out; Reset does not, since it
/// too would be refused.
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
    let row_w = (body.width - 2).min(MAX_W);
    let cx = x0 + LABEL_W;
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
            x: body.x + 1,
            y,
            width: row_w,
            height: 1,
        };
        // File rows are not governed by `editable` at all: Edit… and
        // Reset are the two ways *out* of a broken config.toml (Task
        // 14's `ForceResetConfigFile` exists precisely for this case),
        // so they stay exactly as live as always. Only a setting row —
        // whose write genuinely would be refused by `Config::edit` — is
        // disabled.
        let row_disabled = !editable && matches!(row, SettingsRow::Setting(_));
        // A live edit is its own "you are here", so the row band steps
        // back to let the field carry the focus. A disabled setting row
        // never highlights: there is nothing for the cursor or the
        // pointer to land on.
        let highlight = if row_disabled {
            RowHighlight::None
        } else if tab.cursor == i && tab.editing.is_none() {
            RowHighlight::Selected
        } else if hovered == Some(&Hit::SettingsRow(i)) {
            RowHighlight::Hover
        } else {
            RowHighlight::None
        };
        ListRow {
            highlight,
            zebra: None,
        }
        .paint(buf, y, rect.x, rect.width, theme.page, 1.0, theme);
        let bg = ListRow::resolve_fill(theme, highlight, theme.page, 1.0);
        let label_fg = if row_disabled {
            theme.text_disabled
        } else {
            theme.text
        };
        text(buf, x0, y, row.label(), label_fg, bg, false);
        // A disabled setting row registers no hit at all: a control that
        // looks live and silently refuses every write is the defect this
        // banner exists to remove. A File row's own hit is unaffected.
        if !row_disabled {
            hits.register(rect, Hit::SettingsRow(i));
        }

        let right = rect.x + rect.width;
        match row {
            SettingsRow::Setting(field) => draw_setting_control(
                buf, hits, hovered, theme, tab, ui, *field, cx, y, right, bg, editable,
            ),
            SettingsRow::File(file) => {
                draw_file_buttons(buf, hits, hovered, theme, tab, i, *file, cx, y, right)
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
        "config.toml has a syntax error -- showing the settings this session started with:",
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

/// A one-row pill: the label in `face`, returning the width painted.
fn pill(buf: &mut Buffer, x: u16, y: u16, label: &str, face: Color, fg: Color) -> u16 {
    let padded = format!(" {label} ");
    let w = padded.chars().count() as u16;
    fill(
        buf,
        Rect {
            x,
            y,
            width: w,
            height: 1,
        },
        face,
    );
    text(buf, x, y, &padded, fg, face, false);
    w
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
    cx: u16,
    y: u16,
    right: u16,
    bg: Color,
    editable: bool,
) {
    if cx + 6 > right {
        return;
    }
    let hit = Hit::SettingsControl(field);
    let hot = editable && hovered == Some(&hit);

    if field == SettingsField::JqTab {
        let mut x = cx;
        for (label, mode) in JQ_SEGMENTS {
            let seg = Hit::SettingsJqTab(mode);
            let face = if !editable {
                theme.control
            } else if ui.jq_tab == mode {
                theme.accent
            } else if hovered == Some(&seg) {
                theme.control_hover
            } else {
                theme.control
            };
            let fg = if !editable {
                theme.text_disabled
            } else if ui.jq_tab == mode {
                theme.on_accent
            } else {
                theme.text
            };
            let w = label.chars().count() as u16 + 2;
            if x + w > right {
                return;
            }
            pill(buf, x, y, label, face, fg);
            if editable {
                hits.register(
                    Rect {
                        x,
                        y,
                        width: w,
                        height: 1,
                    },
                    seg,
                );
            }
            x += w + 1;
        }
        return;
    }

    if let Some(on) = field.checkbox(ui) {
        let glyph = if on {
            glyph::CHECKBOX
        } else {
            glyph::CHECKBOX_OFF
        };
        let fg = if !editable {
            theme.text_disabled
        } else if on {
            theme.accent
        } else {
            theme.text_muted
        };
        let face = if hot { theme.control_hover } else { bg };
        let rect = Rect {
            x: cx,
            y,
            width: 3,
            height: 1,
        };
        fill(buf, rect, face);
        text(buf, cx + 1, y, glyph, fg, face, false);
        if editable {
            hits.register(rect, hit);
        }
        return;
    }

    // The three text rows: a one-row well holding the live `LineInput`
    // while this field is under edit, else the value on disk.
    let editing = tab.editing == Some(field);
    let width = right.saturating_sub(cx).min(40);
    if width < 6 {
        return;
    }
    let rect = Rect {
        x: cx,
        y,
        width,
        height: 1,
    };
    let face = if !editable {
        theme.control
    } else if editing {
        theme.control_pressed
    } else if hot {
        theme.control_hover
    } else {
        theme.control
    };
    fill(buf, rect, face);
    let inner = width.saturating_sub(2);
    let line: Line<'static> = if editing {
        tab.input.draw_line_windowed(true, theme, inner)
    } else {
        match field.text_value(ui).unwrap_or_default() {
            v if v.is_empty() => Line::styled(
                "(not set)",
                Style::default().fg(if editable {
                    theme.text_muted
                } else {
                    theme.text_disabled
                }),
            ),
            v => Line::styled(
                v,
                Style::default().fg(if editable {
                    theme.text
                } else {
                    theme.text_disabled
                }),
            ),
        }
    };
    buf.set_line(rect.x + 1, y, &line, inner);
    if editable {
        hits.register(rect, hit);
    }
}

/// A File row's Edit… and Reset are never gated on `editable`: Edit… is
/// always the way to fix `config.toml` by hand, and `ForceResetConfigFile`
/// (Task 14) exists precisely so Reset still works when `config.toml`
/// will not parse -- the confirm it raises already warns, in that
/// branch, that the project list will be lost. Disabling either would
/// remove one of the two ways out.
#[allow(clippy::too_many_arguments)]
fn draw_file_buttons(
    buf: &mut Buffer,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
    theme: &Theme,
    tab: &SettingsTab,
    index: usize,
    file: ConfigFile,
    cx: u16,
    y: u16,
    right: u16,
) {
    let mut x = cx;
    for (slot, label) in [(0usize, "Edit…"), (1, "Reset")] {
        let hit = Hit::SettingsFile {
            file,
            reset: slot == 1,
        };
        // The keyboard's chosen button reads exactly like the hovered
        // one: left/right are how a File row is aimed.
        let chosen = tab.cursor == index && tab.file_button == slot;
        let face = if chosen {
            theme.accent
        } else if hovered == Some(&hit) {
            theme.control_hover
        } else {
            theme.control
        };
        let fg = if chosen { theme.on_accent } else { theme.text };
        let w = label.chars().count() as u16 + 2;
        if x + w > right {
            return;
        }
        pill(buf, x, y, label, face, fg);
        hits.register(
            Rect {
                x,
                y,
                width: w,
                height: 1,
            },
            hit,
        );
        x += w + 1;
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
            tab.move_cursor(1);
        }
        assert_eq!(tab.cursor, n - 1, "the cursor clamps at the bottom");
        for _ in 0..n * 2 {
            tab.move_cursor(-1);
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
    #[test]
    fn moving_off_a_file_row_re_aims_at_edit() {
        let mut tab = SettingsTab {
            cursor: SettingsTab::rows().len() - 2,
            file_button: 1,
            ..Default::default()
        };
        tab.move_cursor(1);
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
}
