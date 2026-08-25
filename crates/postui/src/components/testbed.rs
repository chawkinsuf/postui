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
pub fn draw_testbed(frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
    let theme = ctx.theme;
    let buf = frame.buffer_mut();
    paint::fill(buf, area, theme.page);

    let x0 = area.x + MARGIN;
    let mut y = area.y + MARGIN;

    // --- Buttons: 5 states × 2 kinds -----------------------------------
    section_label(buf, x0, &mut y, "BUTTONS", theme);
    for (i, name) in STATE_NAMES.iter().enumerate() {
        let x = x0 + LABEL_COL + i as u16 * BUTTON_COL;
        paint::text(buf, x, y, name, theme.text_muted, theme.page, false);
    }
    y += 1;

    paint::text(
        buf,
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

    paint::text(
        buf,
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
    section_label(buf, x0, &mut y, "TEXT FIELD", theme);
    for (i, name) in STATE_NAMES.iter().enumerate() {
        let x = x0 + i as u16 * FIELD_COL;
        paint::text(buf, x, y, name, theme.text_muted, theme.page, false);
    }
    y += 1;
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
    section_label(buf, x0, &mut y, "LIST ROWS (dense)", theme);
    let row_w = 30;
    let list_specimens: [(&str, RowHighlight, Option<bool>, f32); 4] = [
        ("zebra off", RowHighlight::None, Some(false), 1.0),
        ("zebra on", RowHighlight::None, Some(true), 1.0),
        ("hover 0.5", RowHighlight::Hover, None, 0.5),
        ("selected", RowHighlight::Selected, None, 1.0),
    ];
    for (label, highlight, zebra, hover_t) in list_specimens {
        paint::text(buf, x0, y, label, theme.text_muted, theme.page, false);
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
    section_label(buf, x0, &mut y, "TAB STRIP", theme);
    let tabs = vec![
        ("Params".to_string(), None),
        ("Headers".to_string(), None),
        ("Body".to_string(), Some(('✓', theme.success))),
    ];
    let spans = TabStrip::spans(&tabs);
    let strip_w = 60;

    paint::text(
        buf,
        x0 + LABEL_COL,
        y,
        "static",
        theme.text_muted,
        theme.page,
        false,
    );
    y += 1;
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

    paint::text(
        buf,
        x0 + LABEL_COL,
        y,
        "mid-slide",
        theme.text_muted,
        theme.page,
        false,
    );
    y += 1;
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
    section_label(buf, x0, &mut y, "CHIPS", theme);
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
    y += 2;

    // --- frac_vspan demo band -----------------------------------------
    section_label(buf, x0, &mut y, "FRAC_VSPAN", theme);
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
    section_label(buf, x0, &mut y, "FLOATING PANEL", theme);
    let panel = Rect::new(x0, y, 30, 5);
    floating_panel(buf, panel, area, theme);
    paint::text(
        buf,
        panel.x + 2,
        panel.y + 2,
        "floating panel",
        theme.text,
        theme.panel,
        false,
    );
}

/// Paints `label` in `theme.text_muted`, bold, at `(x, *y)`, then advances
/// `*y` past it — the shared "section heading" look every specimen group
/// starts with.
fn section_label(buf: &mut Buffer, x: u16, y: &mut u16, label: &str, theme: &crate::theme::Theme) {
    paint::text(buf, x, *y, label, theme.text_muted, theme.page, true);
    *y += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Anims;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn testbed_paints_a_bevel_and_a_tab_underline() {
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
        assert!(content.contains('▔'), "no button bevel painted");
        assert!(content.contains('▂'), "no tab-strip underline painted");
        assert!(content.contains("BUTTONS"), "section label missing");
        assert!(content.contains("TEXT FIELD"), "section label missing");
        assert!(content.contains("LIST ROWS"), "section label missing");
        assert!(content.contains("TAB STRIP"), "section label missing");
        assert!(content.contains("CHIPS"), "section label missing");
        assert!(content.contains("FRAC_VSPAN"), "section label missing");
        assert!(content.contains("FLOATING PANEL"), "section label missing");
    }
}
