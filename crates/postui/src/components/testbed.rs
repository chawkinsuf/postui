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

use crate::anim::{AnimKey, ListId};
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

    // --- MOTION: looping demos (Task 8b) ---------------------------------
    // Drawn first, ahead of every static specimen grid below: this is a
    // fixed, non-scrolling screen (see the module doc), and the static
    // sections below already run to ~46 rows on their own — on anything
    // short of a very tall terminal, content placed after them would never
    // be reachable at all. The MOTION section is what a checkpoint review
    // is actually here to judge, so it gets the top of the screen.
    //
    // Everything below is live, not a frozen freeze-frame: `draw_testbed`
    // is called every frame while `Screen::Testbed` is up, and
    // `App::tick_testbed_demos` keeps retargeting each demo's `AnimKey`(s)
    // to their opposite pole once it arrives, so they loop for as long as
    // the screen is showing.
    draw_motion_section(buf, area, x0, &mut y, ctx);
    y += 1;

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

/// The "MOTION" section: looping animated demos judged live, not as a
/// static freeze-frame — the tab-underline slide in both shipping and
/// alternative form, a hover fade, the in-flight Send breathe, and a
/// list-selection travel. Every demo reads its value straight from
/// `ctx.anims`/`ctx.now`; nothing here mutates state — driving the loop is
/// `App::tick_testbed_demos`'s job (called from `Action::Tick`).
fn draw_motion_section(buf: &mut Buffer, area: Rect, x0: u16, y: &mut u16, ctx: &DrawCtx) {
    let theme = ctx.theme;
    if !section_label(buf, area, x0, y, "MOTION", theme) {
        return;
    }
    // Wider than the file's shared `LABEL_COL` (12): this section's inline
    // duration labels (e.g. `"500ms (current demo)"`, 21 columns) are
    // longer than any specimen label elsewhere in the grid, and sit on the
    // very same line as the row they label (see the compactness note
    // below), so they need more room before the row starts or they'd run
    // straight into it.
    const MOTION_LABEL_COL: u16 = 22;

    // Shared two-tab geometry every underline duration copy slides across.
    let motion_tabs = vec![
        ("Params".to_string(), None),
        ("Headers".to_string(), None),
        ("Body".to_string(), Some(('✓', theme.success))),
    ];
    let spans = TabStrip::spans(&motion_tabs);
    let strip_w = 40;
    let lerp = |a: u16, b: u16, t: f32| a as f32 + (b as f32 - a as f32) * t;

    // Underline slide: a duration COMPARISON — the plan (140ms) alongside
    // three longer alternatives, so the actual question ("is 140ms too
    // fast against a reference app with longer transitions?") can be
    // judged directly rather than guessed at from a single sample. Every
    // row shares one cycle clock (`App::drive_testbed_group`): all four
    // start moving at the same instant, each easing over its own labeled
    // duration, so nothing but the duration itself differs between them.
    // Each row is the real, shipping `TabStrip::paint`/`underline` field —
    // just fed a different-duration animated value. The duration label
    // sits at `x0` on the strip's own top row (not a separate line above
    // it, the way the earlier TAB STRIP section's "static"/"mid-slide"
    // labels do) — this section is meant to fit near the top of a normal
    // terminal without scrolling, and a checkpoint review needs to see
    // every comparison row at once, so every line saved here matters more
    // than it would in the static reference grid below.
    if !section_label(buf, area, x0, y, "underline slide (duration)", theme) {
        return;
    }
    let underline_specs: [(&str, AnimKey); 4] = [
        ("140ms (plan)", AnimKey::ToastFade(140)),
        ("250ms", AnimKey::ToastFade(250)),
        ("400ms", AnimKey::ToastFade(400)),
        ("500ms (current demo)", AnimKey::ToastFade(500)),
    ];
    for (label, key) in underline_specs {
        if !fits(area, *y, 2) {
            return;
        }
        safe_text(
            buf,
            area,
            x0,
            *y,
            label,
            theme.text_muted,
            theme.page,
            false,
        );
        let t = ctx.anims.value_or(key, ctx.now, 0.0);
        TabStrip {
            tabs: &motion_tabs,
            active: 0,
            hovered: None,
            focused: false,
            underline: (
                lerp(spans[0].0, spans[1].0, t),
                lerp(spans[0].1, spans[1].1, t),
            ),
        }
        .paint(
            buf,
            Rect::new(x0 + MOTION_LABEL_COL, *y, strip_w, 2),
            theme.page,
            theme,
        );
        *y += 2;
    }

    // Hover fade: the same duration-comparison treatment — the plan
    // (70ms) alongside two longer alternatives, each a dense `ListRow`
    // ping-ponging Normal↔Hover through `theme::mix` (via `ListRow`'s own
    // `hover_t` blend), label and row sharing one line for the same
    // compactness reason as above.
    if !section_label(buf, area, x0, y, "hover fade (duration)", theme) {
        return;
    }
    let hover_specs: [(&str, AnimKey); 3] = [
        ("70ms (plan)", AnimKey::ToastFade(70)),
        ("150ms", AnimKey::ToastFade(150)),
        ("300ms", AnimKey::ToastFade(300)),
    ];
    for (label, key) in hover_specs {
        if !fits(area, *y, 1) {
            return;
        }
        safe_text(
            buf,
            area,
            x0,
            *y,
            label,
            theme.text_muted,
            theme.page,
            false,
        );
        let hover_t = ctx.anims.value_or(key, ctx.now, 0.0);
        ListRow {
            highlight: RowHighlight::Hover,
            zebra: None,
        }
        .paint(
            buf,
            *y,
            x0 + MOTION_LABEL_COL,
            24,
            theme.page,
            hover_t,
            theme,
        );
        *y += 1;
    }

    // Send breathe: the in-flight catalog motion at its plan duration —
    // fill ping-pongs `mix(accent, accent_edge_dark, t)` at 700ms per pole.
    // A single copy: unlike the two demos above, this duration isn't in
    // question, so there's nothing to compare it against.
    if !fits(area, *y, 1) {
        return;
    }
    safe_text(
        buf,
        area,
        x0,
        *y,
        "send breathe — 700ms/pole (plan)",
        theme.text_muted,
        theme.page,
        false,
    );
    if fits(area, *y + 1, BUTTON_HEIGHT) {
        let breathe_t = ctx.anims.value_or(AnimKey::SendBreathe, ctx.now, 0.0);
        let fill = crate::theme::mix(theme.accent, theme.accent_edge_dark, breathe_t);
        let rect = Rect::new(x0 + MOTION_LABEL_COL, *y + 1, 16, BUTTON_HEIGHT);
        paint::fill(buf, rect, fill);
        let (light, dark) = paint::face_edges(fill, theme);
        paint::bevel_top(buf, Rect::new(rect.x, rect.y, rect.width, 1), light, fill);
        paint::bevel_bottom(
            buf,
            Rect::new(rect.x, rect.y + rect.height - 1, rect.width, 1),
            dark,
            fill,
        );
        let label = "Send";
        let lw = label.chars().count() as u16;
        let sx = rect.x + rect.width.saturating_sub(lw) / 2;
        paint::text(buf, sx, rect.y + 1, label, theme.on_accent, fill, true);
    }
    *y += 1 + BUTTON_HEIGHT;

    // List travel: the plan (100ms/row) alongside one longer alternative
    // (250ms/row) — two independent 5-row dense lists, each with its own
    // `frac_vspan`-painted selection band sliding row-to-row. Not synced
    // to a shared clock (unlike the two ease comparisons above): each is
    // a discrete dwell-then-step cadence, not a continuous ease, so lining
    // up their start instants doesn't carry the same comparison value.
    // Label shares the first row's line, same compactness reason as above.
    if !section_label(buf, area, x0, y, "list travel (duration)", theme) {
        return;
    }
    let list_specs: [(&str, AnimKey); 2] = [
        ("100ms/row (plan)", AnimKey::ListTravel(ListId::Sidebar)),
        ("250ms/row", AnimKey::ListTravel(ListId::Palette)),
    ];
    const ROWS: usize = 5;
    let row_w = 24;
    for (label, key) in list_specs {
        if !fits(area, *y, ROWS as u16) {
            return;
        }
        safe_text(
            buf,
            area,
            x0,
            *y,
            label,
            theme.text_muted,
            theme.page,
            false,
        );
        for i in 0..ROWS {
            ListRow {
                highlight: RowHighlight::None,
                zebra: Some(i % 2 == 1),
            }
            .paint(
                buf,
                *y + i as u16,
                x0 + MOTION_LABEL_COL,
                row_w,
                theme.page,
                1.0,
                theme,
            );
        }
        let travel_t = ctx.anims.value_or(key, ctx.now, 0.0);
        let band_y0 = *y as f32 + travel_t;
        frac_vspan(
            buf,
            x0 + MOTION_LABEL_COL,
            x0 + MOTION_LABEL_COL + row_w,
            band_y0,
            band_y0 + 1.0,
            theme.selection,
            theme.page,
        );
        *y += ROWS as u16;
    }
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
    /// when `draw_testbed` is called directly. Backend height is 90, not
    /// 60: since Task 8b, MOTION is drawn *first* (see `draw_testbed`'s own
    /// doc comment on why) and needs real room of its own before any of
    /// the static sections below it get a chance to render at all.
    #[test]
    fn every_section_is_labeled() {
        let theme = Theme::dark();
        let anims = Anims::new(false);
        let mut term = Terminal::new(TestBackend::new(160, 90)).unwrap();
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

    /// The MOTION section (Task 8b) needs a much taller backend than
    /// `every_section_is_labeled`'s 160×60 to avoid being clipped by
    /// `fits` — it's the last section, appended after everything that test
    /// already covers, and its duration-comparison rows (4 underline + 3
    /// hover + 2 list-travel copies) run well past 60 rows on their own.
    #[test]
    fn motion_section_is_labeled_and_shows_every_duration_row() {
        let theme = Theme::dark();
        let anims = Anims::new(false);
        let mut term = Terminal::new(TestBackend::new(160, 140)).unwrap();
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
        assert!(content.contains("MOTION"), "MOTION section label missing");
        assert!(
            content.contains("underline slide (duration)"),
            "underline comparison header missing"
        );
        for label in ["140ms (plan)", "250ms", "400ms", "500ms (current demo)"] {
            assert!(
                content.contains(label),
                "underline duration label {label:?} missing"
            );
        }
        assert!(
            content.contains("hover fade (duration)"),
            "hover comparison header missing"
        );
        for label in ["70ms (plan)", "150ms", "300ms"] {
            assert!(
                content.contains(label),
                "hover duration label {label:?} missing"
            );
        }
        assert!(
            content.contains("send breathe — 700ms/pole (plan)"),
            "send breathe label missing"
        );
        assert!(
            content.contains("list travel (duration)"),
            "list travel comparison header missing"
        );
        for label in ["100ms/row (plan)", "250ms/row"] {
            assert!(
                content.contains(label),
                "list travel duration label {label:?} missing"
            );
        }
    }

    /// At `t = 0` every underline duration row must actually be painting
    /// its thin `▂` bar — confirming each row's own `AnimKey` (borrowed
    /// `ToastFade` ids) is read and painted, not just the shared strip
    /// geometry underneath.
    #[test]
    fn every_underline_duration_row_paints_at_rest() {
        let theme = Theme::dark();
        let mut anims = Anims::new(false);
        let now = std::time::Instant::now();
        for ms in [140, 250, 400, 500] {
            anims.snap(crate::anim::AnimKey::ToastFade(ms), 0.0);
        }
        let mut term = Terminal::new(TestBackend::new(160, 140)).unwrap();
        term.draw(|f| {
            let area = f.area();
            let ctx = DrawCtx {
                theme: &theme,
                focused: false,
                hovered: None,
                dragging: false,
                anims: &anims,
                now,
            };
            draw_testbed(f, area, &ctx);
        })
        .unwrap();
        let content = format!("{:?}", term.backend().buffer());
        // Each buffer row is its own quoted line in the Debug output (see
        // `ratatui_core::buffer::Buffer`'s `Debug` impl), so counting
        // *lines* containing the glyph — not raw character occurrences,
        // which would overcount a single row's multi-column bar — gives
        // the number of distinct rows painting one. Restricted to the
        // MOTION section itself: the earlier TAB STRIP section also
        // paints `▂` bars of its own (on 2 rows), and this assertion is
        // specifically about the 4 duration-comparison rows.
        let motion_start = content
            .find("MOTION")
            .expect("MOTION section label missing");
        let motion_content = &content[motion_start..];
        let rows_with_bar = motion_content.lines().filter(|l| l.contains('▂')).count();
        assert!(
            rows_with_bar >= 4,
            "expected a `▂` bar on each of the 4 underline duration rows, found {rows_with_bar}: {motion_content}"
        );
    }
}
