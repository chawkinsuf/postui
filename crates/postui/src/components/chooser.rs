use super::line_input::LineInput;
use super::palette::fuzzy_match;
use crate::action::Action;
use crate::paint::{self, ControlState, FIELD_HEIGHT, ListRow, RowHighlight, TextField};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// One selectable entry in a `ChooserState`: a label, an optional detail
/// string shown dimmed after the label, and the actions dispatched on
/// selection.
#[derive(Clone, Default)]
pub struct ChooserItem {
    pub label: String,
    pub detail: Option<String>,
    pub actions: Vec<Action>,
    /// A stable identifier the app can read back from the selection
    /// (`ChooserState::selected_id`) without parsing the display label —
    /// the theme picker's live-preview hook.
    pub id: Option<String>,
}

/// An optional two-state switch on a chooser (the theme picker's
/// dark/light filter): `label` renders right-aligned on the title row,
/// and Tab/BackTab (Left/Right too without a filter box, or a click on the
/// label) dispatch `action` without closing the modal — the action's
/// handler is expected to swap the chooser's items via
/// [`ChooserState::set_items`]. An empty label is an inert toggle: not
/// painted, not advertised, and no key or click fires it.
pub struct ChooserToggle {
    pub label: String,
    pub action: Action,
}

/// An optional stepped level on a chooser (the theme picker's Contrast row,
/// spec 2026-10-16): painted as a row under the list — `label`, a `-`
/// button, a meter with one cell per level, a `+` button and the level as
/// text. On a chooser without a filter box `-` fires `down` and `+`/`=`
/// fire `up`, without closing; the buttons fire the same. The actions'
/// handler owns the level: it refuses a step past either end, and reports
/// the level back through [`ChooserState::set_stepper_level`], which only
/// paints it — the meter, and the end's button disabled.
#[derive(Clone)]
pub struct ChooserStepper {
    pub label: String,
    pub level: i8,
    pub min: i8,
    pub max: i8,
    pub down: Action,
    pub up: Action,
}

/// Where the stepper row's pieces sit, as columns from the row's start,
/// and the width the whole row needs. The panel's height and the paint
/// both read it, so a row that does not fit is neither drawn nor given
/// its line.
struct StepperLayout {
    minus: u16,
    meter: u16,
    cells: usize,
    plus: u16,
    level: u16,
    width: u16,
}

impl ChooserStepper {
    /// The row laid out from its own label: the `-` button three cells
    /// after it, the meter, the `+` button and then the level, read at
    /// its widest end.
    fn layout(&self) -> StepperLayout {
        use unicode_width::UnicodeWidthStr;
        // In i16: an i8 range's span can overflow i8 (-100..=100 is 200).
        let cells = (i16::from(self.max) - i16::from(self.min)).max(0) as usize + 1;
        let minus = self.label.width() as u16 + 3;
        let meter = minus + 4;
        let plus = meter + cells as u16 + 1;
        let level = plus + 5;
        let widest = [self.min, self.max].map(|n| level_text(n).chars().count()).into_iter().max();
        StepperLayout {
            minus,
            meter,
            cells,
            plus,
            level,
            width: level + widest.unwrap_or(1) as u16,
        }
    }
}

/// A stepper level as the row shows it: `+1`, `0`, `-2`. Plain ASCII, as
/// the `-` button is: U+2212 is East-Asian-ambiguous width, two cells in a
/// terminal set for CJK, which would run the row past its own layout.
fn level_text(n: i8) -> String {
    match n {
        n if n > 0 => format!("+{n}"),
        n => n.to_string(),
    }
}

/// The stepper row's meter glyphs, lowest to highest; the meter has one
/// cell per level and spreads these across them.
const METER: [&str; 7] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇"];

/// A generic fuzzy-filterable chooser modal. Structure mirrors
/// `PaletteState`: typed input filters `items` by fuzzy-matching against
/// `label + " " + detail`; arrows move the selection; `Enter` dispatches the
/// selected item's actions and closes; `Esc` closes with no actions.
/// [`ChooserState::without_filter`] drops the filter box.
pub struct ChooserState {
    pub(crate) title: String,
    /// The filter box, whose typed text filters `items`. `None` for the
    /// theme picker (task #6), whose few rows need no search; its arrows
    /// are then free for the toggle.
    input: Option<LineInput>,
    selected: usize,
    pub(crate) items: Vec<ChooserItem>,
    filtered: Vec<usize>,
    toggle: Option<ChooserToggle>,
    /// Boxed: its two actions would otherwise make every `Modal` larger.
    stepper: Option<Box<ChooserStepper>>,
    /// First visible row's index into `filtered`. Kept in view of `selected`
    /// on the next `draw` whenever `ensure_visible` is set; free to roam
    /// otherwise (wheel scrolling).
    scroll: usize,
    /// Set whenever `selected` changes via keys (or on refilter) so the next
    /// `draw` scrolls it back into view; wheel scrolling clears it so a free
    /// scroll survives the following draw untouched.
    ensure_visible: bool,
    /// The screen behind is the chooser's live preview (the theme picker,
    /// task #16): it stays undimmed, and the panel stands off it with the
    /// accent ring instead. See [`ChooserState::previewing`].
    previewing: bool,
}

impl ChooserState {
    pub fn new(title: &str, items: Vec<ChooserItem>) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            title: title.to_string(),
            input: Some(LineInput::new("")),
            selected: 0,
            items,
            filtered,
            scroll: 0,
            ensure_visible: true,
            toggle: None,
            stepper: None,
            previewing: false,
        }
    }

    /// Attaches a [`ChooserStepper`] (builder-style, for construction).
    pub fn with_stepper(mut self, stepper: ChooserStepper) -> Self {
        self.stepper = Some(Box::new(stepper));
        self
    }

    /// The attached stepper row's label, which the footer names its keys
    /// by; `None` without a stepper.
    pub fn stepper_label(&self) -> Option<&str> {
        self.stepper.as_ref().map(|s| s.label.as_str())
    }

    /// Updates the stepper's level after its action moved it. A no-op when
    /// no stepper is attached.
    pub fn set_stepper_level(&mut self, level: i8) {
        if let Some(s) = &mut self.stepper {
            s.level = level;
        }
    }

    /// The stepper's action one step up (or down), or `None` without a
    /// stepper. Refusing a step past an end is the action's handler's
    /// business; the row only shows it, by disabling that button.
    pub fn step_action(&self, up: bool) -> Option<&Action> {
        let s = self.stepper.as_ref()?;
        Some(if up { &s.up } else { &s.down })
    }

    /// The toggle while it is live: attached with a label showing.
    fn live_toggle(&self) -> Option<&ChooserToggle> {
        self.toggle.as_ref().filter(|t| !t.label.is_empty())
    }

    /// Whether a live title-row toggle is attached — the footer advertises
    /// its keys only then.
    pub fn has_toggle(&self) -> bool {
        self.live_toggle().is_some()
    }

    /// Attaches a [`ChooserToggle`] (builder-style, for construction).
    pub fn with_toggle(mut self, label: impl Into<String>, action: Action) -> Self {
        self.toggle = Some(ChooserToggle { label: label.into(), action });
        self
    }

    /// Drops the filter box (builder-style): every row stays listed, typed
    /// keys and pastes do nothing, and the keys a caret would take are the
    /// list's (Home, End) and the toggle's (Left, Right).
    pub fn without_filter(mut self) -> Self {
        self.input = None;
        self
    }

    /// Whether the chooser has a filter box — a field with the caret.
    pub fn has_filter(&self) -> bool {
        self.input.is_some()
    }

    /// Marks the screen behind as the chooser's live preview
    /// (builder-style): dimming it would misrepresent what is being
    /// previewed, so the backdrop stays as it is and the panel is ringed
    /// in accent to stand off it, as the undimmed dropdown is.
    pub fn previewing(mut self) -> Self {
        self.previewing = true;
        self
    }

    /// Whether the screen behind is this chooser's live preview.
    pub fn is_previewing(&self) -> bool {
        self.previewing
    }

    /// Updates the toggle's displayed label (e.g. after its action flipped
    /// the state it names). A no-op when no toggle is attached.
    pub fn set_toggle_label(&mut self, label: impl Into<String>) {
        if let Some(t) = &mut self.toggle {
            t.label = label.into();
        }
    }

    /// The live toggle's action — what a click on the toggle label, or a
    /// key that flips it, dispatches.
    pub fn toggle_action(&self) -> Option<&Action> {
        self.live_toggle().map(|t| &t.action)
    }

    /// Replaces the item list wholesale (the toggle's handler swapping in
    /// the other set), keeping the typed filter and re-running it. The
    /// selection resets to the first match; callers that want a specific
    /// row follow up with [`Self::select_id`].
    pub fn set_items(&mut self, items: Vec<ChooserItem>) {
        self.items = items;
        self.refilter();
    }

    /// Moves the selection to the filtered row whose item id is `id`, if
    /// one is visible under the current filter; otherwise leaves the
    /// selection where it is.
    pub fn select_id(&mut self, id: &str) {
        if let Some(pos) =
            self.filtered.iter().position(|&i| self.items[i].id.as_deref() == Some(id))
        {
            self.select(pos);
        }
    }

    /// Pastes into the filter query (the bracketed-paste/ctrl+v path),
    /// flattened to one line like every single-line surface. Returns
    /// whether there was a filter box to take it.
    pub fn paste(&mut self, text: &str) -> bool {
        let Some(input) = &mut self.input else {
            return false;
        };
        input.paste(text);
        self.refilter();
        true
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The filter text; "" without a filter box.
    pub fn input(&self) -> &str {
        self.input.as_ref().map_or("", LineInput::text)
    }

    /// The query, for the vim engine (plan M3b): it edits it in place.
    /// `None` without a filter box.
    pub(crate) fn input_mut(&mut self) -> Option<&mut LineInput> {
        self.input.as_mut()
    }

    /// The query changed outside this picker's own keys (the vim engine
    /// typed into it): the rows follow, as after a typed key.
    pub(crate) fn filter_changed(&mut self) {
        self.refilter();
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_label(&self) -> Option<&str> {
        self.filtered.get(self.selected).map(|&i| self.items[i].label.as_str())
    }

    /// The `id` of the highlighted item, mapped through the filter — `None`
    /// when nothing is selected or the item carries no id.
    pub fn selected_id(&self) -> Option<&str> {
        self.filtered.get(self.selected).and_then(|&i| self.items[i].id.as_deref())
    }

    /// Moves the keyboard/mouse cursor to filtered row `i` (clamped in
    /// range) and asks the next draw to scroll it into view.
    pub fn select(&mut self, i: usize) {
        if i < self.filtered.len() {
            self.selected = i;
            self.ensure_visible = true;
        }
    }

    /// The `ModalResult` an `Enter` (or a confirming click) on the current
    /// selection produces — `None` when nothing is selected (empty filter).
    pub fn confirm(&self) -> Option<super::modal::ModalResult> {
        let &idx = self.filtered.get(self.selected)?;
        Some(super::modal::ModalResult {
            actions: self.items[idx].actions.clone(),
            close: true,
            ..Default::default()
        })
    }

    /// Adjusts `scroll` by `delta` lines, clamped to the filtered list's
    /// bounds, without moving `selected`. A no-op on an empty list.
    pub fn scroll_by(&mut self, delta: i16) {
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len().saturating_sub(1);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
        self.ensure_visible = false;
    }

    fn refilter(&mut self) {
        // Without a filter box every row stays listed.
        self.filtered = match &self.input {
            None => (0..self.items.len()).collect(),
            Some(input) => self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    let haystack = match &item.detail {
                        Some(detail) => format!("{} {}", item.label, detail),
                        None => item.label.clone(),
                    };
                    fuzzy_match(input.text(), &haystack)
                })
                .map(|(i, _)| i)
                .collect(),
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
            // Tab fires the toggle (when one is attached) without closing.
            // With a filter box the arrows move its caret; without one the
            // bare arrows fire the toggle too (the label's `◂ ▸`), and a
            // modified one (a word move or selection chord) does nothing.
            KeyCode::Tab | KeyCode::BackTab => return self.fire_toggle(),
            KeyCode::Left | KeyCode::Right if !self.has_filter() => {
                return if key.modifiers.is_empty() { self.fire_toggle() } else { None };
            }
            // The stepper's keys, on a chooser with no box to type them
            // into. A shifted `+` is how most layouts deliver it; `=` is
            // the same key unshifted.
            // No other modifier: a chord like cmd+- (a terminal's font zoom,
            // which the kitty protocol can deliver as SUPER) is not a step.
            KeyCode::Char(c @ ('-' | '+' | '='))
                if !self.has_filter()
                    && key.modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
                return self.step_action(c != '-').map(keep_open);
            }
            KeyCode::Home if !self.has_filter() => self.select(0),
            KeyCode::End if !self.has_filter() => {
                self.select(self.filtered.len().saturating_sub(1))
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.ensure_visible = true;
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected = self.selected.saturating_sub(1);
                self.ensure_visible = true;
            }
            KeyCode::Down => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                self.ensure_visible = true;
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                self.ensure_visible = true;
            }
            _ => {
                if let Some(input) = &mut self.input {
                    let before = input.text().to_string();
                    input.handle_key(key);
                    if input.text() != before {
                        self.refilter();
                    }
                }
            }
        }
        None
    }

    /// The live toggle's action as a result that keeps the chooser open —
    /// `None` when no toggle is live.
    fn fire_toggle(&self) -> Option<super::modal::ModalResult> {
        self.toggle_action().map(keep_open)
    }

    /// Whether the filter box has keystrokes of its own to step — what
    /// the app's undo routing asks before handing it the key.
    pub(crate) fn filter_edited(&self) -> bool {
        self.input.as_ref().is_some_and(LineInput::edited)
    }

    /// Steps the filter's edit history one place (back, or forward when
    /// `redo`) and re-runs the filter, mirroring what typing that step
    /// forward did. Returns whether there was a step to take.
    pub(crate) fn undo_filter(&mut self, redo: bool) -> bool {
        let Some(input) = &mut self.input else {
            return false;
        };
        let stepped = if redo { input.redo() } else { input.undo() };
        if stepped {
            self.refilter();
        }
        stepped
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
        // Chrome (everything but the list): above it, 1 pad + 1 title + 1
        // gap, then with a filter box its 3-row field + 2 gap rows; below
        // it, 2 bottom pad rows — the stepper row, when there is one and it
        // fits inside the panel's two-column margins, taking the second of
        // them over a third. A panel too narrow for the row has neither the
        // row nor its line, so no button lands (or takes a click) outside
        // it; the keys still step.
        let above: u16 = if self.has_filter() { 3 + FIELD_HEIGHT + 2 } else { 3 };
        // A screen no taller than that chrome has no line to give it either
        // (review round 5).
        let stepper_fits = self.stepper.as_ref().is_some_and(|s| {
            s.layout().width <= width.saturating_sub(4) && screen.height > above + 3
        });
        let below: u16 = if stepper_fits { 3 } else { 2 };
        let chrome = above + below;
        let content_rows = (self.filtered.len() as u16).clamp(1, 10);
        let height = (chrome + content_rows).max(chrome + 3).min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        hits.register(area, crate::hit::Hit::ModalBody);
        if self.previewing {
            // Over its own undimmed preview there is no dim to fade in, so
            // no settle: the panel opens whole, ringed to stand off it.
            paint::floating_panel_accented(frame.buffer_mut(), area, screen, theme);
        } else {
            paint::floating_panel_settling(frame.buffer_mut(), area, screen, theme, t);
            if t < 1.0 {
                return;
            }
        }

        let title_y = area.y + 1;
        paint::text(
            frame.buffer_mut(),
            area.x + 2,
            title_y,
            &self.title,
            theme.text,
            theme.panel,
            true,
        );
        // An empty toggle label means the control is currently inert (the
        // theme picker hides it while an unpaired theme is highlighted):
        // paint nothing and register no hit.
        if let Some(t) = self.live_toggle() {
            // Right-aligned on the title row, clickable and flippable with
            // the keys `handle_key` gives it.
            let w = t.label.chars().count() as u16;
            let x = (area.x + area.width).saturating_sub(w + 2);
            paint::text(frame.buffer_mut(), x, title_y, &t.label, theme.accent, theme.panel, false);
            hits.register(
                Rect { x, y: title_y, width: w, height: 1 },
                crate::hit::Hit::ChooserToggle,
            );
        }

        if let Some(input) = &self.input {
            let field_area = Rect {
                x: area.x + 1,
                y: title_y + 2,
                width: area.width.saturating_sub(2),
                height: FIELD_HEIGHT,
            };
            let content = input.draw_line_windowed(true, theme, field_area.width.saturating_sub(2));
            TextField { content, state: ControlState::Focused }.paint(
                frame.buffer_mut(),
                field_area,
                theme,
            );
        }

        let list_area = Rect {
            x: area.x + 1,
            y: area.y + above,
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
                let max_scroll = self.filtered.len().saturating_sub(list_h);
                self.scroll = self.scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        // No hover-fade animation is wired for popup lists (transient
        // surfaces — see the task report); a hovered row shows its full
        // hover fill immediately, same convention as `DrawCtx::hover_t`'s
        // own documented default when no fade is in flight.
        let hover_t = 1.0;
        for (i, &idx) in self.filtered.iter().enumerate().skip(self.scroll).take(list_h.max(1)) {
            let item = &self.items[idx];
            let text_row = list_area.y + (i - self.scroll) as u16;
            let selected = i == self.selected;
            let row_hovered = hovered == Some(&crate::hit::Hit::ChooserRow(i));
            let highlight = if selected {
                RowHighlight::Selected
            } else if row_hovered {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            ListRow { highlight, zebra: None }.paint(
                frame.buffer_mut(),
                text_row,
                list_area.x,
                list_area.width,
                theme.panel,
                hover_t,
                theme,
            );
            let row_fill = ListRow::resolve_fill(theme, highlight, theme.panel, hover_t);

            let text_x = list_area.x + 1;
            let mut x = text_x;
            let right = list_area.x + list_area.width;
            let label = clip(item.label.as_str(), right.saturating_sub(x));
            let label_w = (label.chars().count() as u16).min(right.saturating_sub(x));
            paint::text(frame.buffer_mut(), x, text_row, label, theme.text, row_fill, selected);
            x += label_w;
            if let Some(detail) = &item.detail {
                let detail = format!(" {detail}");
                let w = right.saturating_sub(x);
                paint::text(
                    frame.buffer_mut(),
                    x,
                    text_row,
                    clip(&detail, w),
                    theme.text_muted,
                    row_fill,
                    false,
                );
            }

            let row_rect = Rect { x: list_area.x, y: text_row, width: list_area.width, height: 1 };
            hits.register(row_rect, crate::hit::Hit::ChooserRow(i));
        }

        // The stepper row sits above the bottom pad, a gap under the list,
        // whenever the panel was given its line (`stepper_fits`).
        if stepper_fits {
            // Inside the panel's two-column margins, as the title row is.
            let row = Rect {
                x: area.x + 2,
                y: area.y + area.height - 2,
                width: area.width.saturating_sub(4),
                height: 1,
            };
            self.draw_stepper(frame.buffer_mut(), row, theme, hits, hovered);
        }
    }

    /// Paints the stepper row into `row`, if one is attached:
    /// `Contrast   -  ▁▂▃▄▅▆▇  +   +1`. Each button is three cells (its
    /// glyph padded) so the hover fill reads as a key; one at its end paints
    /// in the disabled tone and registers no hit.
    fn draw_stepper(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        row: Rect,
        theme: &Theme,
        hits: &mut crate::hit::HitMap,
        hovered: Option<&crate::hit::Hit>,
    ) {
        let Some(s) = &self.stepper else {
            return;
        };
        let (x, y) = (row.x, row.y);
        let at = s.layout();
        let (minus_x, meter_x, plus_x, level_x) =
            (x + at.minus, x + at.meter, x + at.plus, x + at.level);
        let cells = at.cells;
        paint::text(buf, x, y, &s.label, theme.text, theme.panel, false);
        let mut button = |bx: u16, glyph: &str, hit: crate::hit::Hit, live: bool| {
            let rect = Rect { x: bx, y, width: 3, height: 1 };
            if live {
                paint::action(
                    buf,
                    rect,
                    &format!(" {glyph} "),
                    hit.clone(),
                    hovered,
                    theme.panel,
                    theme,
                );
                hits.register(rect, hit);
            } else {
                paint::text(
                    buf,
                    bx,
                    y,
                    &format!(" {glyph} "),
                    theme.text_disabled,
                    theme.panel,
                    false,
                );
            }
        };
        button(minus_x, "-", crate::hit::Hit::ChooserStepDown, s.level > s.min);
        button(plus_x, "+", crate::hit::Hit::ChooserStepUp, s.level < s.max);
        for i in 0..cells {
            // The glyphs rise across the meter whatever its length.
            let glyph = METER[i * (METER.len() - 1) / (cells - 1).max(1)];
            let lit = i16::from(s.min) + i as i16 <= i16::from(s.level);
            let fg = if lit { theme.accent } else { theme.text_disabled };
            paint::text(buf, meter_x + i as u16, y, glyph, fg, theme.panel, false);
        }
        let level = level_text(s.level);
        paint::text(buf, level_x, y, &level, theme.text_muted, theme.panel, false);
    }
}

/// `action` as a result that keeps the chooser open: what the toggle's and
/// the stepper's keys hand back.
fn keep_open(action: &Action) -> super::modal::ModalResult {
    super::modal::ModalResult { actions: vec![action.clone()], close: false, ..Default::default() }
}

/// Clips `s` to at most `width` columns on a char boundary.
pub(super) fn clip(s: &str, width: u16) -> &str {
    match s.char_indices().nth(width as usize) {
        Some((byte, _)) => &s[..byte],
        None => s,
    }
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

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_n_and_ctrl_p_are_down_and_up() {
        let mut a = ChooserState::new("t", items(&["a", "b"]));
        let mut b = ChooserState::new("t", items(&["a", "b"]));
        a.handle_key(key(KeyCode::Down));
        b.handle_key(ctrl('n'));
        assert_eq!(a.selected(), b.selected());
        a.handle_key(key(KeyCode::Up));
        b.handle_key(ctrl('p'));
        assert_eq!(a.selected(), b.selected());
        assert_eq!(b.input(), "", "ctrl+n/p never type");
    }

    fn items(labels: &[&str]) -> Vec<ChooserItem> {
        labels
            .iter()
            .map(|l| ChooserItem {
                label: l.to_string(),
                detail: None,
                actions: vec![Action::Render],
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn no_key_hint_footer_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut c = ChooserState::new("Projects", items(&["a", "b"]));
        let theme = crate::theme::Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("enter select"), "{content}");
        assert!(!content.contains("esc cancel"), "{content}");
    }

    #[test]
    fn left_and_right_move_the_caret_and_tab_fires_the_toggle() {
        let mut c = ChooserState::new("Pick", vec![])
            .with_toggle("dark", Action::ToggleThemePickerPolarity);
        for ch in "ab".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert!(c.handle_key(key(KeyCode::Left)).is_none(), "Left moves the caret, fires nothing");
        c.handle_key(key(KeyCode::Char('X')));
        assert_eq!(c.input(), "aXb");
        let res = c.handle_key(key(KeyCode::Tab)).expect("tab fires the toggle");
        assert_eq!(res.actions, vec![Action::ToggleThemePickerPolarity]);
        assert!(!res.close);
    }

    /// Task #6 (you, 2026-10-14): a chooser without a filter box (the
    /// theme picker) types nothing; Left and Right fire its toggle beside
    /// Tab and BackTab, and Home and End move the highlight.
    #[test]
    fn a_filterless_chooser_gives_the_arrows_to_its_toggle() {
        let mut c = ChooserState::new("Theme", items(&["a", "b", "c"]))
            .without_filter()
            .with_toggle("◂ dark ▸", Action::ToggleThemePickerPolarity);
        assert!(!c.has_filter());
        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Tab, KeyCode::BackTab] {
            let res = c.handle_key(key(code)).expect("fires the toggle");
            assert_eq!(res.actions, vec![Action::ToggleThemePickerPolarity], "{code:?}");
            assert!(!res.close, "{code:?}");
        }
        for mods in [KeyModifiers::SHIFT, KeyModifiers::CONTROL, KeyModifiers::ALT] {
            let ev = KeyEvent::new(KeyCode::Left, mods);
            assert!(c.handle_key(ev).is_none(), "a modified arrow flips nothing: {mods:?}");
        }
        c.handle_key(key(KeyCode::Char('c')));
        assert!(!c.paste("c"), "no box to take a paste");
        assert_eq!(c.input(), "", "nothing types or pastes");
        assert_eq!(c.selected_label(), Some("a"), "every row still listed");
        c.handle_key(key(KeyCode::End));
        assert_eq!(c.selected_label(), Some("c"));
        c.handle_key(key(KeyCode::Home));
        assert_eq!(c.selected_label(), Some("a"));
        // An inert toggle (label hidden) fires on no key, as it takes no
        // click and shows no chip.
        c.set_toggle_label("");
        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Tab] {
            assert!(c.handle_key(key(code)).is_none(), "{code:?}");
        }
    }

    /// The filterless chooser paints no field: the list sits right under
    /// the title row, and the box is that much shorter.
    #[test]
    fn a_filterless_chooser_draws_its_list_under_the_title() {
        let theme = Theme::dark();
        let draw = |c: &mut ChooserState| {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
            let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
            let row0 = hits.rect_of(&crate::hit::Hit::ChooserRow(0)).unwrap();
            (body, row0)
        };
        let (body, row0) =
            draw(&mut ChooserState::new("Theme", items(&["a", "b"])).without_filter());
        assert_eq!(row0.y, body.y + 3, "pad, title, gap, then the list");
        let (filtered, _) = draw(&mut ChooserState::new("Theme", items(&["a", "b"])));
        assert!(body.height < filtered.height, "no field, a shorter box");
    }

    #[test]
    fn typing_filters_on_label_and_detail_and_enter_returns_actions() {
        let mut c = ChooserState::new(
            "Projects",
            vec![
                ChooserItem {
                    label: "svc".into(),
                    detail: Some("/tmp/svc".into()),
                    actions: vec![Action::Quit],
                    ..Default::default()
                },
                ChooserItem {
                    label: "web".into(),
                    detail: Some("/tmp/web".into()),
                    actions: vec![Action::Render],
                    ..Default::default()
                },
            ],
        );
        for ch in "tmp/w".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(c.selected_label(), Some("web"), "detail participates in the fuzzy match");
        let res = c.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(res.close);
        assert_eq!(res.actions, vec![Action::Render]);
    }

    #[test]
    fn esc_closes_empty_enter_swallowed_arrows_clamp() {
        let mut c = ChooserState::new("t", items(&["a", "b"]));
        c.handle_key(key(KeyCode::Up));
        c.handle_key(key(KeyCode::Down));
        c.handle_key(key(KeyCode::Down)); // clamped at 1
        assert_eq!(c.selected_label(), Some("b"));
        for ch in "zz".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert!(c.handle_key(key(KeyCode::Enter)).is_none(), "no match: Enter swallowed");
        let res = c.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(res.close && res.actions.is_empty());
    }

    #[test]
    fn draw_renders_title_labels_and_dim_details() {
        let mut c = ChooserState::new(
            "Projects",
            vec![
                ChooserItem {
                    label: "svc".into(),
                    detail: Some("/tmp/svc".into()),
                    actions: vec![Action::Quit],
                    ..Default::default()
                },
                ChooserItem {
                    label: "web".into(),
                    detail: Some("/tmp/web".into()),
                    actions: vec![Action::Render],
                    ..Default::default()
                },
            ],
        );
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Projects"), "title should render");
        assert!(content.contains("svc"), "first label should render");
        assert!(content.contains("web"), "second label should render");
        assert!(content.contains("/tmp/svc"), "detail should render");
    }

    #[test]
    fn selected_row_is_a_dense_selection_fill_with_an_accent_bar() {
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::ChooserRow(0)).unwrap();
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
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| {
                c.draw(f, f.area(), &theme, &mut hits, Some(&crate::hit::Hit::ChooserRow(1)), 1.0)
            })
            .unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::ChooserRow(1)).unwrap();
        let buffer = terminal.backend().buffer();
        let right_edge = buffer[(row1.x + row1.width - 1, row1.y)].clone();
        assert_eq!(
            right_edge.bg, theme.control,
            "the hovered (non-selected) row's pill fill must span the full row width"
        );
    }

    #[test]
    fn tab_fires_the_toggle_without_closing_and_is_inert_without_one() {
        let mut plain = ChooserState::new("t", items(&["a", "b"]));
        assert!(plain.handle_key(key(KeyCode::Tab)).is_none(), "no toggle: Tab is ignored");
        let mut c =
            ChooserState::new("t", items(&["a", "b"])).with_toggle("◂ dark ▸", Action::Quit);
        for code in [KeyCode::Tab, KeyCode::BackTab] {
            let res = c.handle_key(key(code)).unwrap();
            assert!(!res.close, "toggle must not close the modal");
            assert_eq!(res.actions, vec![Action::Quit]);
        }
    }

    #[test]
    fn set_items_keeps_the_typed_filter_and_select_id_finds_a_visible_row() {
        let mut c = ChooserState::new("t", items(&["alpha", "beta"]));
        for ch in "bet".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(c.selected_label(), Some("beta"));
        // Swapping in a new set re-runs the same typed filter over it.
        c.set_items(items(&["betamax", "gamma"]));
        assert_eq!(c.input(), "bet", "filter text survives the swap");
        assert_eq!(c.selected_label(), Some("betamax"));
        // select_id moves to a visible row; an id filtered out is a no-op.
        let mut c = ChooserState::new(
            "t",
            vec![
                ChooserItem {
                    label: "alpha".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("alpha-id".into()),
                },
                ChooserItem {
                    label: "beta".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("beta-id".into()),
                },
            ],
        );
        c.select_id("beta-id");
        assert_eq!(c.selected_id(), Some("beta-id"));
        c.select_id("no-such-id");
        assert_eq!(c.selected_id(), Some("beta-id"), "unknown id leaves selection");
    }

    #[test]
    fn toggle_label_paints_right_aligned_on_the_title_row_and_registers_a_hit() {
        let mut c =
            ChooserState::new("Theme", items(&["a", "b"])).with_toggle("◂ dark ▸", Action::Quit);
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        let toggle = hits
            .rect_of(&crate::hit::Hit::ChooserToggle)
            .expect("toggle label registers a clickable hit");
        let modal = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        assert_eq!(toggle.y, modal.y + 1, "sits on the title row");
        assert_eq!(
            toggle.x + toggle.width,
            modal.x + modal.width - 2,
            "right-aligned with the title row's margin"
        );
        let buffer = terminal.backend().buffer();
        let first = buffer[(toggle.x, toggle.y)].clone();
        assert_eq!(first.symbol(), "◂");
        assert_eq!(first.fg, theme.accent);
    }

    #[test]
    fn empty_toggle_label_paints_nothing_and_registers_no_hit() {
        let mut c = ChooserState::new("Theme", items(&["a"])).with_toggle("", Action::Quit);
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        assert!(
            hits.rect_of(&crate::hit::Hit::ChooserToggle).is_none(),
            "hidden toggle must not be clickable"
        );
    }

    fn stepper(level: i8) -> ChooserStepper {
        ChooserStepper {
            label: "Contrast".into(),
            level,
            min: -3,
            max: 3,
            down: Action::StepThemeContrast(-1),
            up: Action::StepThemeContrast(1),
        }
    }

    /// Spec 2026-10-16 C5, C6: on a filterless chooser `-` fires the
    /// stepper's down action and `+`/`=` its up action, keeping the modal
    /// open. At an end the key still fires: refusing the step is the
    /// handler's (App's `Contrast::step`, review round 2), so there is
    /// one gate.
    #[test]
    fn minus_plus_and_equals_fire_the_stepper() {
        let mut c =
            ChooserState::new("Theme", items(&["a"])).without_filter().with_stepper(stepper(0));
        let fired = |c: &mut ChooserState, ch: char| {
            c.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)).map(|r| {
                assert!(!r.close, "{ch}: stepping keeps the picker open");
                r.actions
            })
        };
        assert_eq!(fired(&mut c, '-'), Some(vec![Action::StepThemeContrast(-1)]));
        assert_eq!(fired(&mut c, '+'), Some(vec![Action::StepThemeContrast(1)]));
        assert_eq!(fired(&mut c, '='), Some(vec![Action::StepThemeContrast(1)]));
        // A shifted `+` (how most layouts deliver it) still steps.
        let shifted = KeyEvent::new(KeyCode::Char('+'), KeyModifiers::SHIFT);
        assert!(c.handle_key(shifted).is_some());
        // Review round 6: any other modifier is a chord, not a step.
        for mods in [
            KeyModifiers::SUPER,
            KeyModifiers::META,
            KeyModifiers::HYPER,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
        ] {
            for ch in ['-', '+', '='] {
                let chord = KeyEvent::new(KeyCode::Char(ch), mods);
                assert!(c.handle_key(chord).is_none(), "{mods:?}+{ch} stepped");
            }
        }
        c.set_stepper_level(3);
        assert_eq!(fired(&mut c, '+'), Some(vec![Action::StepThemeContrast(1)]), "the app gates");
        // No stepper: the keys do nothing on a filterless chooser...
        let mut plain = ChooserState::new("t", items(&["a"])).without_filter();
        assert!(fired(&mut plain, '+').is_none());
        // ...and type into a filter box as ever.
        let mut filtered = ChooserState::new("t", items(&["a"])).with_stepper(stepper(0));
        assert!(fired(&mut filtered, '-').is_none());
        assert_eq!(filtered.input(), "-");
    }

    /// Spec C4: the stepper row sits under the list, one row taller than
    /// the chooser without it, with clickable `-`/`+`, a meter lit up to the
    /// level and the level as text; an end's button paints disabled and
    /// takes no click (C9).
    #[test]
    fn the_stepper_row_paints_under_the_list_with_buttons_meter_and_level() {
        let theme = Theme::dark();
        let draw = |c: &mut ChooserState| {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
            (terminal.backend().buffer().clone(), hits)
        };
        let row_text = |buf: &ratatui::buffer::Buffer, y: u16| {
            (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()
        };

        let (_, plain_hits) =
            draw(&mut ChooserState::new("Theme", items(&["a", "b"])).without_filter());
        let mut c = ChooserState::new("Theme", items(&["a", "b"]))
            .without_filter()
            .with_stepper(stepper(1));
        let (buf, hits) = draw(&mut c);
        let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let plain = plain_hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        assert_eq!(body.height, plain.height + 1, "the row adds one line");
        let down = hits.rect_of(&crate::hit::Hit::ChooserStepDown).expect("- is clickable");
        let up = hits.rect_of(&crate::hit::Hit::ChooserStepUp).expect("+ is clickable");
        assert_eq!(down.y, up.y);
        assert_eq!(down.y, body.y + body.height - 2, "the row above the bottom pad");
        let last_row = hits.rect_of(&crate::hit::Hit::ChooserRow(1)).unwrap();
        assert!(down.y > last_row.y + 1, "a gap between the list and the row");
        let line = row_text(&buf, down.y);
        assert!(line.contains("Contrast"), "{line:?}");
        assert!(line.contains("+1"), "{line:?}");
        // ASCII, one cell wide everywhere (review round 7: U+2212 is
        // ambiguous width).
        assert!(line.contains(" - ") && line.contains(" + "), "{line:?}");
        assert!(!line.contains('−'), "{line:?}");
        // The meter: one cell per level, lit in accent up to the level.
        let meter: Vec<_> = (down.x + down.width..up.x)
            .filter(|&x| "▁▂▃▄▅▆▇".contains(buf[(x, down.y)].symbol()))
            .map(|x| buf[(x, down.y)].fg)
            .collect();
        assert_eq!(meter.len(), 7, "{line:?}");
        assert_eq!(meter.iter().filter(|&&fg| fg == theme.accent).count(), 5, "-3..=1 lit");

        c.set_stepper_level(3);
        let (buf, hits) = draw(&mut c);
        assert!(hits.rect_of(&crate::hit::Hit::ChooserStepUp).is_none(), "+ dead at the top");
        let down = hits.rect_of(&crate::hit::Hit::ChooserStepDown).unwrap();
        let plus_x =
            (down.x..body.x + body.width).find(|&x| buf[(x, down.y)].symbol() == "+").unwrap();
        assert_eq!(buf[(plus_x, down.y)].fg, theme.text_disabled);
    }

    /// Review round 1: a panel too narrow for the whole row paints none of
    /// it, so no button lands (or takes a click) outside the panel.
    #[test]
    fn a_stepper_row_too_wide_for_the_panel_is_left_out() {
        let theme = Theme::dark();
        let mut c =
            ChooserState::new("Theme", items(&["a"])).without_filter().with_stepper(stepper(0));
        let mut terminal = Terminal::new(TestBackend::new(30, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        assert!(hits.rect_of(&crate::hit::Hit::ChooserStepDown).is_none());
        assert!(hits.rect_of(&crate::hit::Hit::ChooserStepUp).is_none());
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains("Contrast"), "{content}");
        // Review round 3: nor is its line kept, blank.
        let narrow = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let mut plain = ChooserState::new("Theme", items(&["a"])).without_filter();
        let mut terminal = Terminal::new(TestBackend::new(30, 24)).unwrap();
        let mut plain_hits = crate::hit::HitMap::default();
        terminal.draw(|f| plain.draw(f, f.area(), &theme, &mut plain_hits, None, 1.0)).unwrap();
        let plain_body = plain_hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        assert_eq!(narrow.height, plain_body.height, "no blank line for a row left out");
        // At a width with room, the same row is whole.
        let mut terminal = Terminal::new(TestBackend::new(36, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
        let up = hits.rect_of(&crate::hit::Hit::ChooserStepUp).expect("fits at 36");
        assert!(up.x + up.width <= body.x + body.width, "inside the panel");
    }

    /// Review round 5: on a screen too short for the row, it takes no line
    /// either, so the list keeps every row the plain chooser would show.
    #[test]
    fn a_stepper_row_on_a_short_screen_is_left_out() {
        let theme = Theme::dark();
        let draw = |c: &mut ChooserState| {
            // 6 rows: the chrome with the row (3 + 3) and nothing left over.
            let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
            hits
        };
        let many = ["a", "b", "c", "d", "e", "f"];
        let hits = draw(
            &mut ChooserState::new("T", items(&many)).without_filter().with_stepper(stepper(0)),
        );
        let plain = draw(&mut ChooserState::new("T", items(&many)).without_filter());
        assert!(hits.rect_of(&crate::hit::Hit::ChooserStepUp).is_none(), "no room for the row");
        let rows = |h: &crate::hit::HitMap| {
            (0..6).filter(|&i| h.rect_of(&crate::hit::Hit::ChooserRow(i)).is_some()).count()
        };
        assert_eq!(rows(&hits), rows(&plain), "no list row lost to a row not drawn");
    }

    /// Review round 3: the row is laid out from its own label, so a longer
    /// one pushes the buttons along rather than being painted over.
    #[test]
    fn the_stepper_row_lays_out_from_its_label() {
        let theme = Theme::dark();
        let draw = |label: &str| {
            let mut c = ChooserState::new("T", items(&["a"]))
                .without_filter()
                .with_stepper(ChooserStepper { label: label.into(), ..stepper(0) });
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut hits = crate::hit::HitMap::default();
            terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
            let body = hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
            let down = hits.rect_of(&crate::hit::Hit::ChooserStepDown).unwrap();
            (down.x - (body.x + 2), terminal.backend().buffer().clone(), down)
        };
        let (short, _, _) = draw("Contrast");
        let (long, buf, down) = draw("Indent width");
        assert_eq!(long - short, 4, "four more label cells, four cells along");
        let row: String = (0..80).map(|x| buf[(x, down.y)].symbol().to_string()).collect();
        assert!(row.contains("Indent width"), "{row:?}");
        // Review round 4: measured in cells, not chars (a wide glyph is
        // two), and a wide range's span does not overflow i8.
        let (wide, _, _) = draw("对比度");
        assert_eq!(wide, 6 + 3, "three two-cell glyphs, then the gap");
        let span = ChooserStepper { min: -100, max: 100, ..stepper(0) }.layout();
        assert_eq!(span.cells, 201);
    }

    /// A chooser without a stepper keeps today's shape and no step hits.
    #[test]
    fn a_chooser_without_a_stepper_registers_no_step_hits() {
        let theme = Theme::dark();
        let mut c = ChooserState::new("Theme", items(&["a"])).without_filter();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        assert!(hits.rect_of(&crate::hit::Hit::ChooserStepDown).is_none());
        assert!(hits.rect_of(&crate::hit::Hit::ChooserStepUp).is_none());
    }

    #[test]
    fn selected_id_maps_through_the_filter() {
        let mut c = ChooserState::new(
            "Theme",
            vec![
                ChooserItem {
                    label: "alpha".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("alpha-id".into()),
                },
                ChooserItem {
                    label: "beta".into(),
                    detail: None,
                    actions: vec![Action::Render],
                    id: Some("beta-id".into()),
                },
            ],
        );
        assert_eq!(c.selected_id(), Some("alpha-id"));
        for ch in "bet".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(c.selected_id(), Some("beta-id"), "id follows the filtered selection");
    }

    #[test]
    fn field_fill_and_gap_row_survive_the_list_draw() {
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
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
    fn rows_sit_on_a_dense_one_line_pitch() {
        let mut c = ChooserState::new("Projects", items(&["svc", "web", "auth"]));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        let row0 = hits.rect_of(&crate::hit::Hit::ChooserRow(0)).unwrap();
        let row1 = hits.rect_of(&crate::hit::Hit::ChooserRow(1)).unwrap();
        let row2 = hits.rect_of(&crate::hit::Hit::ChooserRow(2)).unwrap();
        assert_eq!(row1.y - row0.y, 1, "rows sit on a dense 1-row pitch");
        assert_eq!(row2.y - row1.y, 1, "rows sit on a dense 1-row pitch");
    }

    /// A dense chooser should fit noticeably more rows in the same modal
    /// height than the old 2-line-pitch pill list did — the point of this
    /// task. 13 items at a plain 80×24 terminal must all be reachable via
    /// scroll without the modal's height cap swallowing the tail.
    #[test]
    fn thirteen_items_all_scroll_into_view() {
        let labels: Vec<String> = (0..13).map(|i| format!("item-{i}")).collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut c = ChooserState::new("Projects", items(&label_refs));
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        c.select(12);
        terminal.draw(|f| c.draw(f, f.area(), &theme, &mut hits, None, 1.0)).unwrap();
        assert!(
            hits.rect_of(&crate::hit::Hit::ChooserRow(12)).is_some(),
            "scrolling to the last of 13 items must bring it into view"
        );
    }
}
