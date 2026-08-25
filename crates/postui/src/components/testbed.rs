//! The hidden `POSTUI_TESTBED=1` screen (stage 8, Task 8): a static grid
//! showing every painted primitive introduced so far, in every state, each
//! labeled. This is the permanent "looking-glass" screen the visual
//! language gets judged against — it never tears down, and it's only ever
//! reached at startup via the env var (see `App::new`).
//!
//! Nothing here is interactive beyond quitting the app (handled by
//! `App::handle_key`'s `Screen::Testbed` branch) — every specimen is
//! painted once, in a fixed state, with no hit registration.

use ratatui::{Frame, buffer::Buffer, layout::Rect, text::Line};

use crate::components::DrawCtx;
use crate::paint::{
    self, BUTTON_HEIGHT, Button, ButtonKind, Chip, ControlState, FIELD_HEIGHT, ListRow,
    RowHighlight, TabStrip, TextField, floating_panel, frac_vspan,
};

/// One column each side of `area`, and a left column reserved for the
/// per-row label (`"Primary"`, `"zebra off"`, …) before a row of specimens
/// starts.
const MARGIN: u16 = 1;
const LABEL_COL: u16 = 12;
const BUTTON_COL: u16 = 16;
const FIELD_COL: u16 = 18;

const STATES: [ControlState; 5] = [
    ControlState::Normal,
    ControlState::Hover,
    ControlState::Pressed,
    ControlState::Focused,
    ControlState::Disabled,
];
const STATE_NAMES: [&str; 5] = ["Normal", "Hover", "Pressed", "Focused", "Disabled"];

/// Draws the testbed screen into `area`: a static grid of every painted
/// primitive (`Button`, `TextField`, `ListRow`, `TabStrip`, `Chip`,
/// `frac_vspan`, `floating_panel`) across every state it has, on
/// `theme.page`, with a `theme.text_muted` label over every section and
/// every specimen column.
///
/// Every write is bounds-checked against `area` and the whole function
/// stops (rather than panicking) the moment a section would run past
/// `area`'s bottom edge — this is a fixed, non-scrolling grid on a fixed
/// terminal, so on a short terminal the tail of the grid is simply not
/// drawn instead of corrupting the buffer.
pub fn draw_testbed(frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
    let theme = ctx.theme;
    let buf = frame.buffer_mut();
    paint::fill(buf, area, theme.page);

    let x0 = area.x + MARGIN;
    let mut y = area.y + MARGIN;

    // --- Buttons: 5 states × 2 kinds -----------------------------------
    if !section_label(buf, area, x0, &mut y, "BUTTONS", theme) {
        return;
    }
    for (i, name) in STATE_NAMES.iter().enumerate() {
        let x = x0 + LABEL_COL + i as u16 * BUTTON_COL;
        safe_text(buf, area, x, y, name, theme.text_muted, theme.page, false);
    }
    y += 1;

    if !fits(area, y, BUTTON_HEIGHT) {
        return;
    }
    safe_text(
        buf,
        area,
        x0,
        y + 1,
        "Primary",
        theme.text_muted,
        theme.page,
        false,
    );
    for (i, state) in STATES.iter().enumerate() {
        let x = x0 + LABEL_COL + i as u16 * BUTTON_COL;
        let rect = Rect::new(x, y, BUTTON_COL.saturating_sub(2), BUTTON_HEIGHT);
        Button {
            label: "Send",
            kind: ButtonKind::Primary,
            state: *state,
        }
        .paint(buf, rect, theme.page, theme);
    }
    y += BUTTON_HEIGHT;

    if !fits(area, y, BUTTON_HEIGHT) {
        return;
    }
    safe_text(
        buf,
        area,
        x0,
        y + 1,
        "Secondary",
        theme.text_muted,
        theme.page,
        false,
    );
    for (i, state) in STATES.iter().enumerate() {
        let x = x0 + LABEL_COL + i as u16 * BUTTON_COL;
        let rect = Rect::new(x, y, BUTTON_COL.saturating_sub(2), BUTTON_HEIGHT);
        Button {
            label: "Cancel",
            kind: ButtonKind::Secondary,
            state: *state,
        }
        .paint(buf, rect, theme.page, theme);
    }
    y += BUTTON_HEIGHT + 1;

    // --- Text field: every state ----------------------------------------
    if !section_label(buf, area, x0, &mut y, "TEXT FIELD", theme) {
        return;
    }
    for (i, name) in STATE_NAMES.iter().enumerate() {
        let x = x0 + i as u16 * FIELD_COL;
        safe_text(buf, area, x, y, name, theme.text_muted, theme.page, false);
    }
    y += 1;
    if !fits(area, y, FIELD_HEIGHT) {
        return;
    }
    for (i, state) in STATES.iter().enumerate() {
        let x = x0 + i as u16 * FIELD_COL;
        let rect = Rect::new(x, y, FIELD_COL.saturating_sub(2), FIELD_HEIGHT);
        TextField {
            content: Line::raw("content"),
            state: *state,
        }
        .paint(buf, rect, theme);
    }
    y += FIELD_HEIGHT + 1;

    // --- List rows: zebra on/off, mid hover blend, selected --------------
    if !section_label(buf, area, x0, &mut y, "LIST ROWS (dense)", theme) {
        return;
    }
    let row_w = 30;
    let list_specimens: [(&str, RowHighlight, Option<bool>, f32); 4] = [
        ("zebra off", RowHighlight::None, Some(false), 1.0),
        ("zebra on", RowHighlight::None, Some(true), 1.0),
        ("hover 0.5", RowHighlight::Hover, None, 0.5),
        ("selected", RowHighlight::Selected, None, 1.0),
    ];
    for (label, highlight, zebra, hover_t) in list_specimens {
        if !fits(area, y, 1) {
            return;
        }
        safe_text(buf, area, x0, y, label, theme.text_muted, theme.page, false);
        ListRow { highlight, zebra }.paint(
            buf,
            y,
            x0 + LABEL_COL,
            row_w,
            theme.page,
            hover_t,
            theme,
        );
        y += 1;
    }
    y += 1;

    // --- Tab strip: static, and a deliberately mid-slide underline -------
    if !section_label(buf, area, x0, &mut y, "TAB STRIP", theme) {
        return;
    }
    let tabs = vec![
        ("Params".to_string(), None),
        ("Headers".to_string(), None),
        ("Body".to_string(), Some(('✓', theme.success))),
    ];
    let spans = TabStrip::spans(&tabs);
    let strip_w = 60;

    if !fits(area, y, 1) {
        return;
    }
    safe_text(
        buf,
        area,
        x0 + LABEL_COL,
        y,
        "static",
        theme.text_muted,
        theme.page,
        false,
    );
    y += 1;
    if !fits(area, y, 2) {
        return;
    }
    TabStrip {
        tabs: &tabs,
        active: 0,
        hovered: None,
        focused: false,
        underline: (spans[0].0 as f32, spans[0].1 as f32),
    }
    .paint(
        buf,
        Rect::new(x0 + LABEL_COL, y, strip_w, 2),
        theme.page,
        theme,
    );
    y += 2;

    if !fits(area, y, 1) {
        return;
    }
    safe_text(
        buf,
        area,
        x0 + LABEL_COL,
        y,
        "mid-slide",
        theme.text_muted,
        theme.page,
        false,
    );
    y += 1;
    if !fits(area, y, 2) {
        return;
    }
    // Straddles tab 0 and tab 1: an offset halfway between their spans'
    // left edges, at tab 0's own width — neither tab's own span.
    let mid_left = spans[0].0 as f32 + (spans[1].0 - spans[0].0) as f32 * 0.5;
    TabStrip {
        tabs: &tabs,
        active: 0,
        hovered: None,
        focused: false,
        underline: (mid_left, spans[0].1 as f32),
    }
    .paint(
        buf,
        Rect::new(x0 + LABEL_COL, y, strip_w, 2),
        theme.page,
        theme,
    );
    y += 2 + 1;

    // --- Chips -------------------------------------------------------------
    if !section_label(buf, area, x0, &mut y, "CHIPS", theme) {
        return;
    }
    if fits(area, y, 1) {
        let chips: [(&str, ratatui::style::Color); 4] = [
            ("GET", theme.success),
            ("POST", theme.accent),
            ("DELETE", theme.error),
            ("3", theme.warning),
        ];
        let mut cx = x0;
        for (label, color) in chips {
            let w = Chip { label, color }.paint(buf, cx, y, theme.page, theme);
            cx += w + 1;
        }
    }
    y += 2;

    // --- frac_vspan demo band -----------------------------------------
    if !section_label(buf, area, x0, &mut y, "FRAC_VSPAN", theme) {
        return;
    }
    if !fits(area, y, 4) {
        return;
    }
    // `frac_vspan` (unlike the direct-text calls above) is already
    // bounds-safe — it paints purely through `paint::fill`/`cell_mut` — so
    // no `safe_*` wrapper is needed here, only the same fits-check every
    // other multi-row block uses to decide whether to draw it at all.
    frac_vspan(
        buf,
        x0,
        x0 + 40,
        y as f32 + 0.5,
        y as f32 + 3.5,
        theme.accent,
        theme.page,
    );
    y += 4 + 1;

    // --- Floating panel --------------------------------------------------
    if !section_label(buf, area, x0, &mut y, "FLOATING PANEL", theme) {
        return;
    }
    if !fits(area, y, 5) {
        return;
    }
    let panel = Rect::new(x0, y, 30, 5);
    floating_panel(buf, panel, area, theme);
    safe_text(
        buf,
        area,
        panel.x + 2,
        panel.y + 2,
        "floating panel",
        theme.text,
        theme.panel,
        false,
    );
}

/// Whether a block `height` rows tall, starting at row `y`, fits entirely
/// inside `area` without crossing its bottom edge.
fn fits(area: Rect, y: u16, height: u16) -> bool {
    y >= area.top() && y.saturating_add(height) <= area.bottom()
}

/// Paints `s` at `(x, y)` — but only if `y` is inside `area`. The
/// lower-level `paint::text` (like the buffer methods it wraps) panics on
/// an out-of-bounds row rather than clipping, unlike this crate's own
/// `cell_mut`-based helpers (`paint::fill`, `bevel_top`, …); every direct
/// text write in this module goes through here instead so a short terminal
/// truncates the grid rather than crashing it.
#[allow(clippy::too_many_arguments)]
fn safe_text(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    y: u16,
    s: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    bold: bool,
) {
    if y < area.top() || y >= area.bottom() {
        return;
    }
    paint::text(buf, x, y, s, fg, bg, bold);
}

/// Paints `label` in `theme.text_muted`, bold, at `(x, *y)`, then advances
/// `*y` past it — the shared "section heading" look every specimen group
/// starts with. Returns whether there was room to paint it at all (`y` was
/// inside `area`); callers stop drawing entirely once this reports `false`,
/// since every section from here on would overflow too.
fn section_label(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    y: &mut u16,
    label: &str,
    theme: &crate::theme::Theme,
) -> bool {
    let room = fits(area, *y, 1);
    if room {
        paint::text(buf, x, *y, label, theme.text_muted, theme.page, true);
    }
    *y += 1;
    room
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Anims;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};

    /// The bevel-glyph (`▔`) and tab-underline-glyph (`▂`) assertions live
    /// once, at the app level, in `app::tests::testbed_renders_a_bevel_and_an_underline`
    /// (rendered through `ui::draw`, the real entry point) — this test
    /// covers what that one doesn't: every section is present and labeled
    /// when `draw_testbed` is called directly.
    #[test]
    fn every_section_is_labeled() {
        let theme = Theme::dark();
        let anims = Anims::new(false);
        let mut term = Terminal::new(TestBackend::new(160, 60)).unwrap();
        term.draw(|f| {
            let area = f.area();
            let ctx = DrawCtx {
                theme: &theme,
                focused: false,
                hovered: None,
                dragging: false,
                anims: &anims,
                now: std::time::Instant::now(),
            };
            draw_testbed(f, area, &ctx);
        })
        .unwrap();
        let content = format!("{:?}", term.backend().buffer());
        assert!(content.contains("BUTTONS"), "section label missing");
        assert!(content.contains("TEXT FIELD"), "section label missing");
        assert!(content.contains("LIST ROWS"), "section label missing");
        assert!(content.contains("TAB STRIP"), "section label missing");
        assert!(content.contains("CHIPS"), "section label missing");
        assert!(content.contains("FRAC_VSPAN"), "section label missing");
        assert!(content.contains("FLOATING PANEL"), "section label missing");
    }
}
