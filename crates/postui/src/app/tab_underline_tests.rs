//! The tab strips' underline, aimed by `TabStrip::paint` itself (#119,
//! spec 2026-11-05-tabstrip-owns-underline): another tab glides, the same
//! tab's new columns are followed at once, and a strip not painted last
//! frame is placed. Every test reads the underline off a drawn frame.

use super::tests::ready_response;
use super::*;
use crate::components::manage::ManageTab;
use crate::components::response::ViewMode;
use crate::hit::Hit;
use ratatui::crossterm::event::KeyCode;

/// A body whose filters can land output (`.data.total`) or nothing.
const BODY: &str =
    r#"{"data":{"items":[{"id":1,"status":"active"},{"id":2,"status":"off"}],"total":2}}"#;

fn editor_tabs() -> Vec<Hit> {
    (0..4).map(Hit::EditorTab).collect()
}

fn response_tabs() -> Vec<Hit> {
    [ViewMode::Pretty, ViewMode::Raw, ViewMode::Headers].map(Hit::ResponseTab).to_vec()
}

fn manage_tabs() -> Vec<Hit> {
    (0..ManageTab::ALL.len()).map(Hit::ManageTab).collect()
}

/// A test app with animations on and a glide so long that one drawn
/// mid-flight is still at its start.
fn app() -> App {
    let mut app = App::new_for_test_with_anims(true);
    app.ui_settings.anim_ms.tab_slide = Duration::from_secs(3600);
    app
}

/// Draws `app` at `w`×`h`, then answers the accent run on the underline
/// row of the strip `tabs` names, across the strip's tabs, as `(first,
/// last)` columns, both inclusive.
fn draw_at(app: &mut App, w: u16, h: u16, tabs: &[Hit]) -> Option<(u16, u16)> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    let rects: Vec<_> = tabs.iter().filter_map(|t| app.hits.rect_of(t)).collect();
    let y = rects.first().expect("the strip is drawn").y + 1;
    let left = rects.iter().map(|r| r.x).min().unwrap();
    let right = rects.iter().map(|r| r.right()).max().unwrap();
    let buf = terminal.backend().buffer();
    let accent: Vec<u16> = (left..right)
        .filter(|&x| {
            let c = buf.cell((x, y)).unwrap();
            c.fg == app.theme.accent && matches!(c.symbol(), "━" | "╸" | "╺")
        })
        .collect();
    Some((*accent.first()?, *accent.last()?))
}

fn draw(app: &mut App, tabs: &[Hit]) -> Option<(u16, u16)> {
    draw_at(app, 120, 40, tabs)
}

/// `tab`'s columns as of the last draw, as [`draw_at`] answers them.
fn cols(app: &App, tab: &Hit) -> Option<(u16, u16)> {
    app.hits.rect_of(tab).map(|r| (r.x, r.right() - 1))
}

/// Answers whether `tab`'s columns moved between two draws, so a test
/// can show the change it makes really moves the label.
fn moved(before: Option<(u16, u16)>, app: &App, tab: &Hit) -> bool {
    before != cols(app, tab)
}

// --- a switch glides, on each strip (R1, R5) ---

#[test]
fn an_editor_tab_switch_glides() {
    let mut app = app();
    let tabs = editor_tabs();
    let headers = Hit::EditorTab(EditorTab::Headers.draw_position());
    let params = Hit::EditorTab(EditorTab::Params.draw_position());
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &headers), "the first frame places it");

    app.update(Action::EditorTabSelect(EditorTab::Params.index()));
    assert_eq!(draw(&mut app, &tabs), cols(&app, &headers), "the glide starts where it was");
    assert!(app.animating(), "and keeps the loop ticking");
    app.anims.finish_all();
    assert_eq!(draw(&mut app, &tabs), cols(&app, &params), "it lands on Params");
}

#[test]
fn a_response_tab_switch_glides() {
    let mut app = app();
    let tabs = response_tabs();
    ready_response(&mut app, BODY);
    let tree = Hit::ResponseTab(ViewMode::Pretty);
    let raw = Hit::ResponseTab(ViewMode::Raw);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &tree));

    app.update(Action::ResponseViewMode(ViewMode::Raw));
    assert_eq!(draw(&mut app, &tabs), cols(&app, &tree), "the glide starts on Tree");
    assert!(app.animating());
    app.anims.finish_all();
    assert_eq!(draw(&mut app, &tabs), cols(&app, &raw), "it lands on Raw");
}

/// To the right-anchored Settings tab, so the glide must land where the
/// label paints, not where a contiguous strip would put it.
#[test]
fn a_manage_tab_switch_glides() {
    let mut app = app();
    let tabs = manage_tabs();
    app.update(Action::OpenManage { tab: Some(ManageTab::Variables) });
    let variables = Hit::ManageTab(ManageTab::Variables.index());
    let settings = Hit::ManageTab(ManageTab::Settings.index());
    assert_eq!(draw(&mut app, &tabs), cols(&app, &variables));

    app.update(Action::SelectManageTab(ManageTab::Settings));
    assert_eq!(draw(&mut app, &tabs), cols(&app, &variables), "the glide starts on Variables");
    assert!(app.animating());
    app.anims.finish_all();
    assert_eq!(draw(&mut app, &tabs), cols(&app, &settings), "it lands on Settings");
}

/// #117: a method change that moves the tab off Body is a move to another
/// tab, so it glides.
#[test]
fn a_method_change_off_body_glides_to_the_first_tab() {
    let mut app = app();
    let tabs = editor_tabs();
    let body = Hit::EditorTab(EditorTab::Body.draw_position());
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.update(Action::EditorTabSelect(EditorTab::Body.index()));
    assert_eq!(draw(&mut app, &tabs), cols(&app, &body));

    app.update(Action::SetMethod(postui_core::model::Method::Get));
    let first = EditorTab::from_draw_position(0);
    assert_eq!(app.editor.active_tab, first);
    let was = cols(&app, &body);
    assert_eq!(draw(&mut app, &tabs), was, "the glide starts on Body");
    app.anims.finish_all();
    assert_eq!(draw(&mut app, &tabs), cols(&app, &Hit::EditorTab(first.draw_position())));
}

/// #117: a switch between two projects with a POST open on Body starts no
/// glide. The empty request the switch holds between the two (a GET, which
/// cannot show Body) moves the tab off Body and the landing moves it back
/// within one event, so no frame ever paints the tab it passed through.
#[test]
fn a_post_to_post_project_switch_starts_no_glide() {
    let (mut app, a, b) = super::tests::two_projects();
    app.anims.set_enabled(true);
    app.ui_settings.anim_ms.tab_slide = Duration::from_secs(3600);
    let post = super::tests::post_req;
    postui_core::fixtures::save_request(a.path(), "main/send", &post("https://a/send")).unwrap();
    postui_core::fixtures::save_request(b.path(), "main/send", &post("https://b/send")).unwrap();
    postui_core::fixtures::save_local_state(
        b.path(),
        &postui_core::project::LocalState {
            open_request: Some("main/send".into()),
            ..Default::default()
        },
    )
    .unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/send".into()));
    app.update(Action::EditorTabSelect(EditorTab::Body.index()));
    let tabs = editor_tabs();
    let body = Hit::EditorTab(EditorTab::Body.draw_position());
    assert_eq!(draw(&mut app, &tabs), cols(&app, &body));

    app.update(Action::ForceSwitchProject(b.path().to_path_buf()));
    assert_eq!(app.editor.url.text(), "https://b/send");
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &body), "still on Body, with no glide");
}

/// A request whose Headers carry a count shifts every later tab: opening
/// it with Vars active moves Vars, and the underline follows at once.
#[test]
fn a_request_switch_with_other_counts_follows_at_once() {
    let mut app = app();
    let tabs = editor_tabs();
    app.update(Action::CreateRequest("with-header".into()));
    app.editor
        .headers
        .insert("X-One".into(), postui_core::model::Entry { value: "1".into(), enabled: true });
    app.update(Action::SaveRequest);
    app.update(Action::CreateRequest("plain".into()));
    // The tab after Headers in the strip, so the count shifts it.
    let after = EditorTab::from_draw_position(EditorTab::Headers.draw_position() + 1);
    app.update(Action::EditorTabSelect(after.index()));
    let hit = Hit::EditorTab(after.draw_position());
    assert_eq!(draw(&mut app, &tabs), cols(&app, &hit));

    let before = cols(&app, &hit);
    app.update(Action::OpenRequest("main/with-header".into()));
    let drawn = draw(&mut app, &tabs);
    assert!(moved(before, &app, &hit), "the count shifts {after:?}");
    assert_eq!(drawn, cols(&app, &hit), "followed at once");
}

// --- the active tab's own move is followed at once (R3, R4, R6) ---

/// The first attempt's regression (R4): with Tree active, the badge
/// coming and going glided the underline away and back. Each toggle moves
/// Tree's columns (the strip is right-aligned, so Tree's left edge moves),
/// and the underline must be on them in the same frame, every time.
#[test]
fn a_jq_badge_toggle_keeps_the_underline_on_tree() {
    let mut app = app();
    let tabs = response_tabs();
    let tree = Hit::ResponseTab(ViewMode::Pretty);
    ready_response(&mut app, BODY);
    app.update(Action::JqApply(".data.total".into()));
    assert_eq!(draw(&mut app, &tabs), cols(&app, &tree));
    for i in 0..4 {
        let before = cols(&app, &tree);
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT | KeyModifiers::SHIFT));
        let drawn = draw(&mut app, &tabs);
        assert!(moved(before, &app, &tree), "toggle {i} moves Tree's label");
        assert_eq!(drawn, cols(&app, &tree), "toggle {i}: on Tree in the same frame");
    }
}

/// The same through typing, the way the regression was met: each key
/// reruns the filter, and a run that lands nothing (`.d`) drops the badge
/// that `.` had.
#[test]
fn typing_a_jq_filter_keeps_the_underline_on_tree() {
    let mut app = app();
    let tabs = response_tabs();
    let tree = Hit::ResponseTab(ViewMode::Pretty);
    ready_response(&mut app, BODY);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &tree));
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT));
    let mut widths = std::collections::BTreeSet::new();
    for c in ".data".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        let drawn = draw(&mut app, &tabs);
        assert_eq!(drawn, cols(&app, &tree), "after {c:?}: on Tree in the same frame");
        widths.insert(app.hits.rect_of(&tree).unwrap().width);
    }
    assert!(widths.len() > 1, "the badge came and went while typing: {widths:?}");
}

#[test]
fn a_jq_badge_toggle_leaves_raws_underline_still() {
    let mut app = app();
    let tabs = response_tabs();
    let raw = Hit::ResponseTab(ViewMode::Raw);
    let tree = Hit::ResponseTab(ViewMode::Pretty);
    ready_response(&mut app, BODY);
    app.update(Action::JqApply(".data.total".into()));
    app.update(Action::ResponseViewMode(ViewMode::Raw));
    app.anims.finish_all();
    let still = draw(&mut app, &tabs);
    assert_eq!(still, cols(&app, &raw));
    // The clear keeps Raw on screen, and so does the filter coming back.
    for (i, toggle) in
        [Action::ClearJqFilter, Action::JqApply(".data.total".into()), Action::ClearJqFilter]
            .into_iter()
            .enumerate()
    {
        let before = cols(&app, &tree);
        app.update(toggle);
        assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Raw);
        assert_eq!(draw(&mut app, &tabs), still, "toggle {i}: Raw's underline did not move");
        assert!(moved(before, &app, &tree), "toggle {i} moved Tree's label");
    }
}

/// The right-anchored Settings tab moves with the window's width; the
/// underline stays under it in the frame the resize draws.
#[test]
fn a_resize_keeps_the_underline_under_settings() {
    let mut app = app();
    let tabs = manage_tabs();
    let settings = Hit::ManageTab(ManageTab::Settings.index());
    app.update(Action::OpenManage { tab: Some(ManageTab::Settings) });
    assert_eq!(draw_at(&mut app, 120, 40, &tabs), cols(&app, &settings));
    for w in [100, 140, 90] {
        let before = cols(&app, &settings);
        let drawn = draw_at(&mut app, w, 40, &tabs);
        assert!(moved(before, &app, &settings), "width {w} moves Settings");
        assert_eq!(drawn, cols(&app, &settings), "width {w}: under Settings in the same frame");
    }
}

/// With Params active, a count in Headers' label (the tab before it)
/// shifts Params, and one in its own label widens it. Both are followed at
/// once.
#[test]
fn an_editor_count_change_follows_at_once() {
    let mut app = app();
    let tabs = editor_tabs();
    let params = Hit::EditorTab(EditorTab::Params.draw_position());
    assert!(EditorTab::Headers.draw_position() < EditorTab::Params.draw_position());
    app.update(Action::EditorTabSelect(EditorTab::Params.index()));
    app.anims.finish_all();
    assert_eq!(draw(&mut app, &tabs), cols(&app, &params));
    let entry = || postui_core::model::Entry { value: "1".into(), enabled: true };

    let before = cols(&app, &params);
    app.editor.headers.insert("X-One".into(), entry());
    let drawn = draw(&mut app, &tabs);
    assert!(moved(before, &app, &params), "the Headers count shifts Params");
    assert_eq!(drawn, cols(&app, &params), "followed at once");

    let before = cols(&app, &params);
    app.editor.params.insert("page".into(), entry());
    let drawn = draw(&mut app, &tabs);
    assert!(moved(before, &app, &params), "the Params count widens it");
    assert_eq!(drawn, cols(&app, &params), "followed at once");
}

// --- placing (R7) ---

/// Opening Manage places the underline on the tab it opens on, even when
/// the screen was last shown on another tab.
#[test]
fn opening_manage_places_the_underline() {
    let mut app = app();
    let tabs = manage_tabs();
    app.update(Action::OpenManage { tab: Some(ManageTab::Variables) });
    draw(&mut app, &tabs);
    app.update(Action::CloseScreen);
    draw(&mut app, &editor_tabs());

    app.update(Action::OpenManage { tab: Some(ManageTab::Settings) });
    let settings = Hit::ManageTab(ManageTab::Settings.index());
    assert_eq!(draw(&mut app, &tabs), cols(&app, &settings), "placed, not glided from Variables");
}

/// The Response strip is not painted while the pane is hidden, so showing
/// it again places the underline: the tab it shows could have changed.
#[test]
fn showing_the_response_pane_places_the_underline() {
    let mut app = app();
    let tabs = response_tabs();
    ready_response(&mut app, BODY);
    draw(&mut app, &tabs);
    app.update(Action::ToggleResponseCollapse);
    app.anims.finish_all();
    draw(&mut app, &editor_tabs());
    app.update(Action::ResponseViewMode(ViewMode::Headers));
    app.update(Action::ToggleResponseCollapse);
    app.anims.finish_all();
    let headers = Hit::ResponseTab(ViewMode::Headers);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &headers));
}

// --- another tab, with no switch action behind it (R5) ---

/// The strip is right-aligned, so dropping Tree narrows it from the left:
/// the glide starts where Tree was, now outside the strip, and the strip
/// shows it as it enters (TabStrip clips the segment to its own area). So
/// the first frame shows nothing of it rather than Raw's own underline.
fn assert_glides_in_to_raw(app: &mut App, tabs: &[Hit]) {
    let raw = Hit::ResponseTab(ViewMode::Raw);
    assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Raw);
    assert_eq!(draw(app, tabs), None, "the glide starts where Tree was, left of the strip");
    assert!(app.animating(), "and keeps the loop ticking");
    app.anims.finish_all();
    assert_eq!(draw(app, tabs), cols(app, &raw), "it lands on Raw");
}

/// A new response with no tree drops the Tree tab and lands on Raw: a
/// move to another tab, so it glides there.
#[test]
fn a_new_response_that_drops_tree_glides_to_raw() {
    let mut app = app();
    let tabs = response_tabs();
    ready_response(&mut app, BODY);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &Hit::ResponseTab(ViewMode::Pretty)));

    ready_response(&mut app, "not json");
    assert_glides_in_to_raw(&mut app, &tabs);
}

/// A background parse concluding "not JSON" removes the Tree tab under
/// the underline and forces Raw: the same move, the same glide.
#[test]
fn a_parse_concluding_not_json_glides_to_raw() {
    use crate::components::response::SYNC_PRETTY_BYTES;
    let mut app = app();
    let tabs = response_tabs();
    app.session.send_generation = 7;
    ready_response(&mut app, &"x".repeat(SYNC_PRETTY_BYTES + 1));
    app.update(Action::ResponseViewMode(ViewMode::Pretty));
    app.anims.finish_all();
    assert_eq!(draw(&mut app, &tabs), cols(&app, &Hit::ResponseTab(ViewMode::Pretty)));

    app.update(Action::PrettyParsed { generation: 7, tree: None });
    assert_glides_in_to_raw(&mut app, &tabs);
}

/// A request switch swaps in that request's response, on whatever tab it
/// was left on: another tab, so the underline glides there.
#[test]
fn a_request_switch_to_a_response_on_another_tab_glides() {
    let mut app = app();
    let tabs = response_tabs();
    app.update(Action::CreateRequest("a".into()));
    ready_response(&mut app, BODY);
    app.update(Action::ResponseViewMode(ViewMode::Headers));
    app.update(Action::CreateRequest("b".into()));
    ready_response(&mut app, BODY);
    app.anims.finish_all();
    let tree = Hit::ResponseTab(ViewMode::Pretty);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &tree));

    app.update(Action::OpenRequest("main/a".into()));
    assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Headers);
    assert_eq!(draw(&mut app, &tabs), cols(&app, &tree), "the glide starts on Tree");
    app.anims.finish_all();
    assert_eq!(draw(&mut app, &tabs), cols(&app, &Hit::ResponseTab(ViewMode::Headers)));
}

// --- animations off (R8) ---

#[test]
fn with_animations_off_a_switch_places_the_underline() {
    let mut app = App::new_for_test_with_anims(false);
    app.ui_settings.anim_ms.tab_slide = Duration::from_secs(3600);
    let tabs = editor_tabs();
    draw(&mut app, &tabs);
    app.update(Action::EditorTabSelect(EditorTab::Params.index()));
    let params = Hit::EditorTab(EditorTab::Params.draw_position());
    assert_eq!(draw(&mut app, &tabs), cols(&app, &params), "no glide");
}

#[test]
fn turning_animations_off_lands_a_glide_in_flight() {
    let mut app = app();
    let tabs = editor_tabs();
    draw(&mut app, &tabs);
    app.update(Action::EditorTabSelect(EditorTab::Params.index()));
    draw(&mut app, &tabs);
    app.anims.set_enabled(false);
    let params = Hit::EditorTab(EditorTab::Params.draw_position());
    assert_eq!(draw(&mut app, &tabs), cols(&app, &params), "landed at once");
    assert!(!app.anims.active(Instant::now()), "nothing left in flight");
}
