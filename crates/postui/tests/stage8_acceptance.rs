//! Stage-8 acceptance: a whole-app render walk asserting the new visual
//! language's landmarks are actually painted where the spec says they
//! should be, plus the two checkpoint-1 amendments (dimmer disabled labels,
//! `[animation_ms]` config parsing). This is not a pixel-diff against a
//! reference image — it is targeted glyph/color assertions at the exact
//! cells the spec calls out, the same style `stage7_acceptance.rs` uses.

use postui::action::Action;
use postui::app::App;
use postui::components::modal::Modal;
use postui::hit::Hit;
use postui::layout::PaneId;
use postui::paint::{ButtonKind, ControlState};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn render(app: &mut App) -> ratatui::buffer::Buffer {
    app.anims.finish_all();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

/// Like `render`, but WITHOUT settling every in-flight animation first --
/// captures whatever the frame actually looks like right now (mid-flight,
/// if anything just retargeted). `render` itself can't double as this: its
/// `finish_all` is deliberate everywhere else `render` is used (those
/// assertions want the settled landmark, not a timing-dependent one), so a
/// separate helper keeps that at-rest convenience without smuggling in a
/// settle where the test explicitly wants to catch motion in progress.
fn render_no_settle(app: &mut App) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn left_down(x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn click(app: &mut App, hit: Hit) {
    render(app);
    let r = app
        .hits
        .rect_of(&hit)
        .unwrap_or_else(|| panic!("no rect registered for {hit:?}"));
    app.handle_mouse(left_down(r.x + r.width / 2, r.y + r.height / 2));
}

fn seed(app: &mut App, slugs: &[&str]) {
    let req = postui_core::model::HttpRequest {
        name: None,
        method: postui_core::model::Method::Get,
        url: "https://example.test/x".into(),
        substitute_body: false,
        insecure: false,
        jq: None,
        jq_enabled: true,
        params: Default::default(),
        headers: Default::default(),
        variables: Default::default(),
        body: None,
    };
    // Every request lives in a space; these all go in the default one.
    for slug in slugs {
        postui_core::storage::save_request(&app.project.root, &format!("main/{slug}"), &req)
            .unwrap();
    }
    app.update(Action::RefreshSidebar);
}

/// One full render walk over the stage-8 landmarks: the address bar's
/// bevel, the tab strip's accent underline (`━` family, distinguished by
/// fg color from the hairline rest of the rule), the sidebar's flat
/// (unstriped) row fill, the dropdown's ring edge geometry (full-width top edge owning
/// the corner, shortened verticals), and the absence of the deleted
/// `PillRow`-era pad glyphs (`█`/`▀`) in a dense list.
#[test]
fn stage8_landmarks_render_walk() {
    let mut app = App::new_for_test();
    seed(&mut app, &["a", "b", "c"]);
    let (t_panel, t_accent, t_hairline) = (app.theme.panel, app.theme.accent, app.theme.hairline);

    // --- address bar: a "▔" bevel cap on the method/URL row -------------
    let buf = render(&mut app);
    let method = app.hits.rect_of(&Hit::MethodSelector).unwrap();
    let cap = buf.cell((method.x, method.y)).unwrap();
    assert_eq!(
        cap.symbol(),
        "▔",
        "address bar top row carries the bevel cap: {cap:?}"
    );

    // --- tab strip: accent-colored "━" under the active tab, hairline
    // "━" (same glyph, different fg) under the rest of the strip --------
    let active_tab = app.hits.rect_of(&Hit::EditorTab(0)).unwrap(); // Params, the default
    let other_tab = app.hits.rect_of(&Hit::EditorTab(1)).unwrap(); // Headers
    let under_active = buf.cell((active_tab.x + 1, active_tab.y + 1)).unwrap();
    assert_eq!(
        under_active.symbol(),
        "━",
        "underline family: {under_active:?}"
    );
    assert_eq!(
        under_active.fg, t_accent,
        "the active tab's segment is accent-colored, not the hairline: {under_active:?}"
    );
    let under_other = buf.cell((other_tab.x + 1, other_tab.y + 1)).unwrap();
    assert_eq!(under_other.symbol(), "━");
    assert_eq!(
        under_other.fg, t_hairline,
        "elsewhere the rule stays the hairline color: {under_other:?}"
    );

    // --- sidebar rows: the request list keeps a flat panel fill (no
    // zebra striping — dropped in the stage-8 feedback rounds) ----------
    let row0 = app.hits.rect_of(&Hit::SidebarRow(0)).unwrap();
    let row1 = app.hits.rect_of(&Hit::SidebarRow(1)).unwrap();
    let row2 = app.hits.rect_of(&Hit::SidebarRow(2)).unwrap();
    let bg0 = buf.cell((row0.x + 1, row0.y)).unwrap().bg;
    let bg1 = buf.cell((row1.x + 1, row1.y)).unwrap().bg;
    let bg2 = buf.cell((row2.x + 1, row2.y)).unwrap().bg;
    assert_eq!(bg0, t_panel, "row 0 sits on the flat panel fill");
    assert_eq!(bg1, t_panel, "row 1 matches — no zebra stripe");
    assert_eq!(bg2, t_panel, "row 2 matches — no zebra stripe");

    // --- no PillRow-era pad glyphs in the dense sidebar list ------------
    let sidebar_pane = app.hits.rect_of(&Hit::Pane(PaneId::Sidebar)).unwrap();
    for y in sidebar_pane.y..sidebar_pane.bottom() {
        for x in sidebar_pane.x..sidebar_pane.right() {
            if let Some(cell) = buf.cell((x, y)) {
                let s = cell.symbol();
                assert!(
                    s != "█" && s != "▀",
                    "found a PillRow-era pad glyph {s:?} at ({x}, {y}) in the sidebar"
                );
            }
        }
    }

    // --- dropdown ring: opening the method selector paints the accent
    // ring with square box-drawing corners — a `┌` corner glyph at the
    // popup's top-left cell, and a `│` vertical stroke down the row below
    // it. ------------------------------------------------------------
    click(&mut app, Hit::MethodSelector);
    assert!(
        matches!(app.modals.top(), Some(Modal::Dropdown(_))),
        "the method chip opens a dropdown"
    );
    let buf = render(&mut app);
    let popup = app.hits.rect_of(&Hit::ModalBody).unwrap();
    let corner = buf.cell((popup.x, popup.y)).unwrap();
    assert_eq!(
        corner.symbol(),
        "┌",
        "the popup's top-left cell is a square box-drawing corner: {corner:?}"
    );
    assert_eq!(corner.fg, t_accent);
    let left_edge_start = buf.cell((popup.x, popup.y + 1)).unwrap();
    assert_eq!(
        left_edge_start.symbol(),
        "│",
        "the left edge is a vertical stroke on the row below the corner: {left_edge_start:?}"
    );
}

/// `animations = false` (`Anims::new(false)`) renders identically to the
/// default (animated) construction at rest — no in-flight transitions are
/// running immediately after construction, so both should paint the same
/// frame with nothing to settle.
#[test]
fn animations_disabled_renders_identically_at_rest() {
    let mut animated = App::new_for_test_with_anims(true);
    let mut still = App::new_for_test_with_anims(false);
    seed(&mut animated, &["a", "b"]);
    seed(&mut still, &["a", "b"]);

    // Skip the header row: each app's `new_for_test` gets its own tempdir,
    // so the project-name chip differs by construction, not by animation
    // state. Everything else — including every animated surface (bevels,
    // tab underline, sidebar rows) — must match exactly.
    let rows = |buf: &ratatui::buffer::Buffer| -> Vec<String> {
        (0..buf.area.height)
            .filter(|y| *y != 1)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| format!("{:?}", buf.cell((x, y)).unwrap()))
                    .collect::<String>()
            })
            .collect()
    };
    let a = render(&mut animated);
    let b = render(&mut still);
    assert_eq!(
        rows(&a),
        rows(&b),
        "animations=false must not change the at-rest frame"
    );

    // At-rest equality alone doesn't prove the kill switch actually kills
    // motion -- `render`'s own `finish_all()` settles the animated side
    // too, so the comparison above never observes it mid-flight. Drive a
    // real transition (switching the active editor tab, which retargets
    // `AnimKey::TabUnderline`/`TabUnderlineWidth`) on each app, then
    // compare the very next frame WITHOUT settling first.
    use postui::components::editor::EditorTab;
    animated.update(Action::EditorTabSelect(EditorTab::Params.index()));
    still.update(Action::EditorTabSelect(EditorTab::Params.index()));
    let a_mid = render_no_settle(&mut animated);
    let b_mid = render_no_settle(&mut still);
    assert_ne!(
        rows(&a_mid),
        rows(&b_mid),
        "animations=true must actually be mid-motion right after a tab \
         switch, differing from the animations=false frame which jumps \
         straight to the settled position"
    );

    // The disabled app itself must be motion-free: its own frame right
    // after the switch already matches its fully-settled frame (nothing
    // left to finish).
    let b_settled = render(&mut still);
    assert_eq!(
        rows(&b_mid),
        rows(&b_settled),
        "animations=false must render the settled frame immediately, with \
         no in-between motion to catch"
    );
}

/// Checkpoint-1 amendment: disabled button and field labels are dimmed by
/// blending the control fill toward `text_muted` at `DISABLED_LABEL_MIX`
/// (0.55), not a flat disabled-text token.
#[test]
fn disabled_labels_are_dimmed_by_the_control_mix_formula() {
    use postui::paint::{Button, TextField};
    use postui::theme::Theme;
    use ratatui::text::Line;

    assert_eq!(postui::paint::DISABLED_LABEL_MIX, 0.55);
    let theme = Theme::dark();
    let expected = postui::theme::mix(theme.control, theme.text_muted, 0.55);

    let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
    term.draw(|f| {
        Button {
            label: "Send",
            kind: ButtonKind::Secondary,
            state: ControlState::Disabled,
        }
        .paint(
            f.buffer_mut(),
            ratatui::layout::Rect::new(0, 0, 20, 3),
            &theme,
        );
    })
    .unwrap();
    let cell = term.backend().buffer().cell((8, 1)).unwrap();
    assert_eq!(cell.fg, expected, "disabled button label: {cell:?}");

    let mut term2 = Terminal::new(TestBackend::new(20, 3)).unwrap();
    term2
        .draw(|f| {
            TextField {
                content: Line::raw("x"),
                state: ControlState::Disabled,
            }
            .paint(
                f.buffer_mut(),
                ratatui::layout::Rect::new(0, 0, 20, 3),
                &theme,
            );
        })
        .unwrap();
    let cell2 = term2.backend().buffer().cell((2, 1)).unwrap();
    assert_eq!(cell2.fg, expected, "disabled field content: {cell2:?}");
}

/// Checkpoint-1 amendment: `[animation_ms]` in `config.toml` is parsed —
/// `tab_slide = 400` overrides that one key, and every key defaults when
/// no table is present at all.
#[test]
fn animation_ms_table_parses() {
    use postui::config::load_ui_settings;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let (s, warnings) = load_ui_settings(&path);
    assert_eq!(s.anim_ms.tab_slide, Duration::from_millis(250));
    assert!(warnings.is_empty());

    std::fs::write(&path, "[animation_ms]\ntab_slide = 400\n").unwrap();
    let (s, warnings) = load_ui_settings(&path);
    assert_eq!(s.anim_ms.tab_slide, Duration::from_millis(400));
    assert_eq!(
        s.anim_ms.hover,
        Duration::from_millis(70),
        "unset keys still default"
    );
    assert!(warnings.is_empty());
}
