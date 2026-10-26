//! The resize drag (task #90, spec S1, S3, E2, E4–E7, E9a, P2, P2a): the
//! sidebar gutter sets the sidebar width in columns, the response header
//! strip moves the editor/response line, and every way a drag can end keeps
//! and saves the size.

use super::*;
use crate::split::SplitState;
use postui_core::project::LAYOUT_TOML;
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }
}

fn left_down(x: u16, y: u16) -> MouseEvent {
    mouse(MouseEventKind::Down(MouseButton::Left), x, y)
}

fn left_drag(x: u16, y: u16) -> MouseEvent {
    mouse(MouseEventKind::Drag(MouseButton::Left), x, y)
}

fn left_up(x: u16, y: u16) -> MouseEvent {
    mouse(MouseEventKind::Up(MouseButton::Left), x, y)
}

fn right_down(x: u16, y: u16) -> MouseEvent {
    mouse(MouseEventKind::Down(MouseButton::Right), x, y)
}

fn moved(x: u16, y: u16) -> MouseEvent {
    mouse(MouseEventKind::Moved, x, y)
}

/// Renders `app` once at 120x40, as `tests.rs::render_once` does.
fn render(app: &mut App) {
    render_at(app, 120, 40);
}

/// Renders `app` once at `width`x`height`.
fn render_at(app: &mut App, width: u16, height: u16) {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
}

fn rect(app: &App, hit: &Hit) -> ratatui::layout::Rect {
    app.hits.rect_of(hit).unwrap_or_else(|| panic!("no rect for {hit:?}"))
}

/// A cell of the response header strip that is the handle itself (no tab
/// or icon on top), on the strip's top row.
fn handle_cell(app: &App) -> (u16, u16) {
    let strip = rect(app, &Hit::SplitHandle);
    (strip.x..strip.right())
        .map(|x| (x, strip.y))
        .find(|&(x, y)| app.hits.hit_at(x, y) == Some(&Hit::SplitHandle))
        .expect("a bare handle cell on the strip")
}

/// The gutter column, at a row in the middle of the body.
fn gutter_cell(app: &App) -> (u16, u16) {
    let g = rect(app, &Hit::SidebarGutter);
    (g.x, g.y + g.height / 2)
}

fn saved_width(app: &App) -> Option<u16> {
    postui_core::fixtures::load_layout(app.proj().root()).unwrap().sidebar_width
}

#[test]
fn dragging_the_gutter_sets_the_sidebar_width_in_columns() {
    let mut app = App::new_for_test();
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(24, y));
    app.handle_mouse(left_up(24, y));
    assert_eq!(app.sidebar_width, Some(24));
    render(&mut app);
    assert_eq!(rect(&app, &Hit::Pane(PaneId::Sidebar)).width, 24);
}

#[test]
fn gutter_drag_clamps_to_the_limits() {
    let mut app = App::new_for_test();
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(2, y));
    assert_eq!(app.sidebar_width, Some(crate::layout::SIDEBAR_MIN_COLS));
    render(&mut app);
    app.handle_mouse(left_drag(119, y));
    assert_eq!(
        app.sidebar_width,
        Some(120 - 1 - crate::layout::MAIN_MIN_COLS),
        "120 wide leaves the gutter and the main column's minimum"
    );
    app.handle_mouse(left_up(119, y));
}

/// Review F1: a gutter press whose motion stays on the edge's column (a
/// one-row vertical jitter) moves nothing. The unset 28% default stays
/// unset, and a saved width the window squeezes stays saved.
#[test]
fn a_gutter_jitter_keeps_the_default_and_a_squeezed_saved_width() {
    for saved in [None, Some(100)] {
        let mut app = App::new_for_test();
        app.sidebar_width = saved;
        render(&mut app);
        let (x, y) = gutter_cell(&app);
        app.handle_mouse(left_down(x, y));
        assert!(!app.handle_mouse(left_drag(x, y + 1)), "{saved:?}: nothing to repaint");
        app.handle_mouse(left_up(x, y + 1));
        assert_eq!(app.sidebar_width, saved);
    }
}

/// Review F2, S8b: on a window narrow enough that 28% would leave the main
/// column under its minimum, the default draws the sidebar's upper limit,
/// so a step or drag that asks to widen it moves nothing and saves no
/// width. Asked to narrow, it moves.
#[test]
fn widening_a_default_at_the_limit_moves_nothing() {
    const WIDE: u16 = 85;
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Sidebar));
    render_at(&mut app, WIDE, 30);
    let drawn = sidebar_cols(&app);
    let widest = crate::layout::clamp_sidebar_cols(WIDE, i32::from(WIDE));
    assert_eq!(drawn, widest, "the default obeys the main column's minimum");
    assert!(drawn > crate::layout::SIDEBAR_MIN_COLS);
    press(&mut app, "<A-w>");
    assert_eq!(app.sidebar_width, None);
    render_at(&mut app, WIDE, 30);
    assert_eq!(sidebar_cols(&app), drawn);

    let g = rect(&app, &Hit::SidebarGutter);
    let y = g.y + g.height / 2;
    app.handle_mouse(left_down(g.x, y));
    app.handle_mouse(left_drag(g.x + 6, y));
    app.handle_mouse(left_up(g.x + 6, y));
    assert_eq!(app.sidebar_width, None, "a rightward drag saves nothing");
    render_at(&mut app, WIDE, 30);
    assert_eq!(sidebar_cols(&app), drawn);

    press(&mut app, "<A-W>");
    let narrower = crate::layout::clamp_sidebar_cols(WIDE, i32::from(drawn) - 4);
    assert!(narrower < drawn);
    assert_eq!(app.sidebar_width, Some(narrower), "narrowing moves");
}

/// The line follows the pointer row by row, un-eased (E5, E14): the frame
/// right after the motion already shows it, animations on.
#[test]
fn dragging_the_strip_moves_the_line_row_by_row() {
    let mut app = App::new_for_test_with_anims(true);
    render(&mut app);
    let editor = rect(&app, &Hit::Pane(PaneId::Editor));
    let response = rect(&app, &Hit::Pane(PaneId::Response));
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 3));
    render(&mut app);
    assert_eq!(rect(&app, &Hit::Pane(PaneId::Editor)).height, editor.height + 3);
    assert_eq!(rect(&app, &Hit::Pane(PaneId::Response)).height, response.height - 3);
    app.handle_mouse(left_drag(x, y + 1));
    render(&mut app);
    assert_eq!(rect(&app, &Hit::Pane(PaneId::Editor)).height, editor.height + 1);
}

/// Into the editor's chrome lands on "response only" (E6), and dragging
/// back out restores the split where the pointer is (E7).
#[test]
fn dragging_the_strip_to_the_top_minimizes_the_editor_and_back_restores() {
    let mut app = App::new_for_test();
    render(&mut app);
    let top = rect(&app, &Hit::Pane(PaneId::Editor)).y;
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, top));
    assert!(app.table_collapsed, "the editor is minimized");
    render(&mut app);
    app.handle_mouse(left_drag(x, y));
    assert!(!app.table_collapsed, "dragged back out, the editor shows");
    assert!(!app.session.response.collapsed, "and the response with it");
    render(&mut app);
    assert!(
        rect(&app, &Hit::Pane(PaneId::Response)).height
            > crate::components::response::HEADER_STRIP_HEIGHT,
        "the response has rows below its strip"
    );
    app.handle_mouse(left_up(x, y));
}

/// A drag that ends minimized keeps the share the split had before it
/// began, not one the pointer passed on its way into the snap (E9a).
#[test]
fn a_drag_that_ends_minimized_keeps_the_pre_drag_share() {
    let mut app = App::new_for_test();
    app.split_ratio = 0.6;
    app.update(Action::Render);
    render(&mut app);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    for row in y + 1..40 {
        app.handle_mouse(left_drag(x, row));
        render(&mut app);
    }
    app.handle_mouse(left_up(x, 39));
    assert!(app.session.response.collapsed, "the bottom is \"editor only\"");
    assert_eq!(app.split_ratio, 0.6);
}

/// Review Focus 3 (E2): a tab on the strip stays a button. A press on it
/// arms no resize, and a row of wobble before the release moves nothing.
#[test]
fn a_wobbling_click_on_a_response_tab_never_resizes() {
    use crate::components::response::ViewMode;
    let mut app = App::new_for_test();
    super::tests::ready_response(&mut app, r#"{"a": 1}"#);
    render(&mut app);
    let before = app.split_state();
    let tab = rect(&app, &Hit::ResponseTab(ViewMode::Headers));
    app.handle_mouse(left_down(tab.x, tab.y));
    assert!(app.resize.is_none());
    app.handle_mouse(left_drag(tab.x, tab.y + 1));
    assert!(app.resize.is_none());
    app.handle_mouse(left_up(tab.x, tab.y + 1));
    assert!(app.resize.is_none());
    assert_eq!(app.split_state(), before);
    assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Headers, "the tab selected");
}

/// Review Focus 4 (P2a): a press that arrives with a resize still live
/// proves the release was lost. The resize ends where it got to, saved.
#[test]
fn a_lost_release_keeps_the_size_and_ends_the_drag() {
    let mut app = App::new_for_test();
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(24, y));
    render(&mut app);
    let body = rect(&app, &Hit::Pane(PaneId::Response));
    app.handle_mouse(left_down(body.x + body.width / 2, body.bottom() - 2));
    assert!(app.resize.is_none(), "the press ended the resize");
    assert_eq!(app.sidebar_width, Some(24), "keeping its size");
    assert_eq!(saved_width(&app), Some(24), "and saving it");
}

/// Review round 7 M5 (P2a): a drag that has had `Drag(Left)` motion shows
/// the terminal tracks the button, so a later bare `Moved` means the button
/// is up and its release was lost. The resize ends there, kept and saved,
/// and the `Moved` is a hover: its row or column moves nothing.
#[test]
fn a_bare_move_after_drag_motion_ends_the_resize() {
    let mut app = App::new_for_test();
    render(&mut app);
    let editor = pane_rows(&app, PaneId::Editor);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 3));
    render(&mut app);
    assert!(app.handle_mouse(moved(x, y + 6)), "the end repaints");
    assert!(app.resize.is_none(), "the bare move ended the resize");
    render(&mut app);
    assert_eq!(pane_rows(&app, PaneId::Editor), editor + 3, "kept; the move's row moved nothing");
    let saved = postui_core::fixtures::load_layout(app.proj().root()).unwrap().main_split;
    assert_eq!(saved, Some(app.split_state().to_token()), "saved");

    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(24, y));
    app.handle_mouse(moved(40, y));
    assert!(app.resize.is_none(), "the gutter's too");
    assert_eq!((app.sidebar_width, saved_width(&app)), (Some(24), Some(24)), "kept and saved");
    assert_eq!(app.hovered, app.hits.hit_at_ignoring_var_tokens(40, y).cloned(), "a hover");
}

/// Review round 8 N4 (P2a): a key that takes the Main screen away while the
/// gutter is held (`alt+r` to Manage, `ctrl+p` to the palette) leaves the
/// resize live until the next motion, which ends it where it got to, saved,
/// and sizes nothing it can't show.
#[test]
fn a_motion_off_the_main_screen_ends_the_resize() {
    for open in [Action::OpenManage { tab: None }, Action::OpenPalette] {
        let mut app = App::new_for_test();
        render(&mut app);
        let (x, y) = gutter_cell(&app);
        app.handle_mouse(left_down(x, y));
        app.handle_mouse(left_drag(24, y));
        render(&mut app);
        app.update(open.clone());
        assert!(app.handle_mouse(left_drag(40, y)), "{open:?}: the end repaints");
        assert!(app.resize.is_none(), "{open:?}: the motion ended the resize");
        assert_eq!(app.sidebar_width, Some(24), "{open:?}: kept, the motion moved nothing");
        assert_eq!(saved_width(&app), Some(24), "{open:?}: saved");
    }
}

/// Review round 7 M5: a terminal that reports a held drag only as `Moved`
/// (it doesn't track buttons) still drives the resize with it.
#[test]
fn a_moved_only_drag_still_follows_the_pointer() {
    let mut app = App::new_for_test();
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(moved(24, y));
    render(&mut app);
    app.handle_mouse(moved(30, y));
    assert!(app.resize.is_some(), "still live");
    assert_eq!(app.sidebar_width, Some(30), "following the pointer");
    app.handle_mouse(left_up(30, y));
    assert_eq!(saved_width(&app), Some(30));
}

#[test]
fn right_click_and_focus_loss_end_a_resize_keeping_it() {
    for via_focus_loss in [false, true] {
        let mut app = App::new_for_test();
        render(&mut app);
        let (x, y) = gutter_cell(&app);
        app.handle_mouse(left_down(x, y));
        app.handle_mouse(left_drag(24, y));
        render(&mut app);
        if via_focus_loss {
            assert!(app.on_focus_lost(), "a live resize ending needs a repaint");
        } else {
            app.handle_mouse(right_down(60, 20));
        }
        assert!(app.resize.is_none(), "focus loss {via_focus_loss}: the resize ended");
        assert_eq!(app.sidebar_width, Some(24), "focus loss {via_focus_loss}: kept");
        assert_eq!(saved_width(&app), Some(24), "focus loss {via_focus_loss}: saved");
    }
}

/// Review round 4 I2: a key that switches project while the gutter is
/// held ends the resize into the project being left, saved there. Later
/// motion of the same held button resizes nothing in the next project.
#[test]
fn a_project_switch_by_key_ends_a_live_resize_into_the_old_project() {
    let (mut app, a, b) = super::tests::two_projects();
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(24, y));
    render(&mut app);
    press(&mut app, "<A-z>");
    assert_eq!(app.proj().root(), b.path(), "switched");
    assert!(app.resize.is_none(), "the switch ended the resize");
    let width_in =
        |root: &std::path::Path| postui_core::fixtures::load_layout(root).unwrap().sidebar_width;
    assert_eq!(width_in(a.path()), Some(24), "saved into the project being left");
    let split = app.split_state();
    render(&mut app);
    app.handle_mouse(left_drag(40, y));
    app.handle_mouse(left_up(40, y));
    assert_eq!(app.sidebar_width, None, "the next project's width is untouched");
    assert_eq!(width_in(b.path()), None, "and nothing saved into it");
    assert_eq!(app.split_state(), split);
}

/// Review round 8 N2 (task #92): a project with no saved split opens at the
/// default 50/50 with both panes showing, as a project with no saved width
/// opens at the default 28%. The previous project's split doesn't carry
/// over, and the switch writes nothing into the next project.
#[test]
fn a_project_with_no_saved_split_opens_at_the_default() {
    let (mut app, a, b) = super::tests::two_projects();
    let split_a = SplitState { editor_minimized: false, response_minimized: true, ratio: 0.8 };
    assert!(app.apply_split_state(split_a));
    let layout_in = |root: &std::path::Path| postui_core::fixtures::load_layout(root).unwrap();
    assert_eq!(layout_in(a.path()).main_split, Some(split_a.to_token()), "A saved its split");
    render(&mut app);
    press(&mut app, "<A-z>");
    assert_eq!(app.proj().root(), b.path(), "switched");
    assert_eq!(app.split_state(), SplitState::default(), "B opens at 50/50, both panes showing");
    assert!(!b.path().join(LAYOUT_TOML).exists(), "B's layout.toml unwritten");
}

/// Asserts the split's three anims draw `app`'s split settled: each one is
/// set, done, and sits at the split field's own value.
fn assert_split_drawn_settled(app: &App) {
    use crate::anim::AnimKey;
    let now = std::time::Instant::now();
    let s = app.split_state();
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    for (key, settled) in [
        (AnimKey::PaneCollapse, flag(s.editor_minimized)),
        (AnimKey::ResponseCollapse, flag(s.response_minimized)),
        (AnimKey::SplitRatio, s.ratio),
    ] {
        assert!(app.anims.is_done(key, now), "{key:?} eases instead of drawing at once");
        assert_eq!(app.anims.value(key, now), Some(settled), "{key:?}");
    }
}

/// A launch on `root` as `App::new` builds it once a project opened:
/// `with_root`, then `apply_ui_settings` with animations on. (`App::new`
/// itself reads the real user config, so it isn't unit-testable;
/// `apply_ui_settings` is the step both of its branches delegate to.)
fn launch(root: &std::path::Path) -> App {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, root.to_path_buf());
    let settings =
        crate::config::UiSettings { animations: true, ..crate::config::UiSettings::default() };
    app.apply_ui_settings(settings, "dark".into(), crate::theme::Theme::dark());
    app
}

/// Task #93 (P5): with animations on, a launch draws the project's saved
/// split at once, not easing out of the 50/50 default.
#[test]
fn a_launch_draws_the_saved_split_at_once() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::fixtures::init_project(dir.path(), Some("alpha")).unwrap();
    let saved = SplitState { editor_minimized: true, response_minimized: false, ratio: 0.25 };
    let mut layout = postui_core::fixtures::load_layout(dir.path()).unwrap();
    layout.main_split = Some(saved.to_token());
    postui_core::fixtures::save_layout(dir.path(), &layout).unwrap();
    let mut app = launch(dir.path());
    app.update(Action::Tick);
    assert_eq!(app.split_state(), saved);
    assert_split_drawn_settled(&app);
}

/// Task #93 review round 2: a launch that restores a request draws its
/// first frame with nothing easing. Whatever `with_root`'s restore started
/// (the open's tab underline, a fade) is settled before the first frame,
/// as on main.
#[test]
fn a_launch_restoring_a_request_starts_with_nothing_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::fixtures::init_project(dir.path(), Some("alpha")).unwrap();
    postui_core::fixtures::ensure_project(dir.path()).unwrap();
    let req = postui_core::model::HttpRequest {
        url: "https://x/pong".into(),
        ..postui_core::model::HttpRequest::default()
    };
    postui_core::fixtures::save_request(dir.path(), "main/pong", &req).unwrap();
    let mut state = postui_core::fixtures::load_local_state(dir.path()).unwrap();
    state.open_request = Some("main/pong".into());
    postui_core::fixtures::save_local_state(dir.path(), &state).unwrap();
    let app = launch(dir.path());
    assert_eq!(app.editor.slug.as_deref(), Some("main/pong"), "the request was restored");
    assert!(!app.anims.active(std::time::Instant::now()), "nothing eases into the first frame");
}

/// Task #93 (P5): with animations on, a project switch draws the next
/// project's saved split at once, not easing out of the previous one's.
#[test]
fn a_project_switch_draws_the_saved_split_at_once() {
    let (mut app, _a, b) = super::tests::two_projects();
    assert!(app.anims.enabled, "animations on, the default");
    let saved = SplitState { editor_minimized: false, response_minimized: true, ratio: 0.25 };
    let mut layout = postui_core::fixtures::load_layout(b.path()).unwrap();
    layout.main_split = Some(saved.to_token());
    postui_core::fixtures::save_layout(b.path(), &layout).unwrap();
    render(&mut app);
    press(&mut app, "<A-z>");
    assert_eq!(app.proj().root(), b.path(), "switched");
    assert_eq!(app.split_state(), saved);
    assert_split_drawn_settled(&app);
}

/// S3, E4: the handles show resize shapes, and a live drag keeps its
/// shape wherever the pointer wanders until the release.
#[test]
fn resize_handles_show_resize_pointers() {
    let mut app = App::new_for_test();
    render(&mut app);
    let (gx, gy) = gutter_cell(&app);
    app.handle_mouse(moved(gx, gy));
    assert_eq!(app.pointer_shape_update(), Some(PointerShape::ColResize));
    let (x, y) = handle_cell(&app);
    app.handle_mouse(moved(x, y));
    assert_eq!(app.pointer_shape_update(), Some(PointerShape::RowResize));

    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(5, y));
    render(&mut app);
    assert_eq!(app.pointer_shape_update(), None, "still the row shape over the sidebar");
    assert_eq!(app.hovered, Some(Hit::SplitHandle), "the strip stays lit through the drag");
    app.handle_mouse(left_up(5, y));
    let after = app.pointer_shape_update();
    assert!(
        after.is_some_and(|s| s != PointerShape::RowResize),
        "the release hands the shape back to what is under the pointer: {after:?}"
    );

    app.handle_mouse(left_down(gx, gy));
    app.handle_mouse(left_drag(90, gy));
    assert_eq!(app.pointer_shape_update(), Some(PointerShape::ColResize), "the gutter's drag");
    app.handle_mouse(left_up(90, gy));
}

/// The app with a Params cell edit open and the editor focused, after a
/// frame.
fn editing_a_cell() -> App {
    use crate::components::table_editor::{CellEdit, Col};
    let mut app = App::new_for_test();
    app.editor
        .params
        .insert("page".into(), postui_core::model::Entry { value: "1".into(), enabled: true });
    app.editor.active_tab = EditorTab::Params;
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Content;
    app.editor.table.set_selected(Some(Stop::Row(0)));
    app.editor.table.editing =
        Some(CellEdit { row: 0, col: Col::Value, input: LineInput::new("19") });
    render(&mut app);
    app
}

/// A gutter press is not a click away: a click there never did anything,
/// so an open cell stays open (E2a).
#[test]
fn a_gutter_press_leaves_an_open_cell_edit_open() {
    let mut app = editing_a_cell();
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    assert!(app.resize.is_some(), "a resize armed");
    assert!(app.editor.table.editing.is_some(), "the cell is still open");
    assert_eq!(app.editor.table.selected(), Some(Stop::Row(0)), "and its row selected");
    assert_eq!(app.focus, PaneId::Editor, "no focus moved");
}

/// E2a: a press on the strip first does what a click there always did —
/// focuses the response, clears its selection and lit bracket pair, and
/// clicks away from an open cell edit or the jq bar — then arms the drag.
#[test]
fn a_strip_press_clicks_like_the_response_pane() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // A lit bracket pair, then a selection: each cleared by a strip press.
    let mut app = App::new_for_test();
    super::tests::ready_response(&mut app, "x\n(a\nb)"); // not JSON -> Raw view
    render(&mut app);
    let area = app.session.response.view().unwrap().last_area.expect("body area recorded");
    let partner_lit = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let cell = terminal.backend().buffer()[(area.x + 1, area.y + 2)].clone();
        assert_eq!(cell.symbol(), ")");
        (cell.fg, cell.bg) == (app.theme.on_accent, app.theme.accent)
    };
    app.handle_mouse(left_down(area.x, area.y + 1));
    app.handle_mouse(left_up(area.x, area.y + 1));
    assert!(partner_lit(&mut app), "the click lit the pair");
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_up(x, y));
    assert!(!partner_lit(&mut app), "the strip press cleared the lit pair");

    app.handle_mouse(left_down(area.x, area.y));
    app.handle_mouse(left_down(area.x, area.y)); // double click: "x"
    app.handle_mouse(left_up(area.x, area.y));
    assert_eq!(app.session.response.selected_text().as_deref(), Some("x"));
    render(&mut app);
    app.handle_mouse(left_down(x, y));
    assert!(app.session.response.selected_text().is_none(), "the selection is cleared");
    assert!(app.text_drag.is_none(), "the press arms no text sweep");
    assert!(
        matches!(app.resize, Some(ResizeDrag { kind: ResizeHandle::Split { .. }, .. })),
        "it arms the resize"
    );
    app.handle_mouse(left_up(x, y));

    // An open cell edit: closed (committed), its row deselected, the
    // caret out of the editor, the focus on the response — at the press.
    let mut app = editing_a_cell();
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    assert_eq!(app.focus, PaneId::Response, "the press focuses the response");
    assert!(app.editor.table.editing.is_none(), "the cell edit is closed");
    assert_eq!(app.editor.params.get("page").map(|e| e.value.as_str()), Some("19"), "saved");
    assert_eq!(app.editor.table.selected(), None, "its row deselected");
    assert_eq!(app.editor.sub_focus, SubFocus::None, "the caret left the editor");
    assert!(matches!(app.resize, Some(ResizeDrag { kind: ResizeHandle::Split { .. }, .. })));
    app.handle_mouse(left_up(x, y));

    // The jq bar: blurred.
    let mut app = App::new_for_test();
    super::tests::ready_response(&mut app, r#"{"a": [1, 2]}"#);
    app.focus = PaneId::Response;
    assert!(app.session.response.set_jq_focus(true));
    render(&mut app);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    assert!(!app.session.response.jq_focused(), "the jq bar blurred");
    assert!(app.resize.is_some());
    app.handle_mouse(left_up(x, y));
}

/// E2a: the click comes first and the drag still follows: an open cell
/// closes at the press, and the line then moves with the pointer and is
/// saved at the release.
#[test]
fn a_strip_drag_still_resizes_after_the_click() {
    let mut app = editing_a_cell();
    let editor = rect(&app, &Hit::Pane(PaneId::Editor));
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    assert!(app.editor.table.editing.is_none(), "the click closed the cell");
    render(&mut app);
    app.handle_mouse(left_drag(x, y + 3));
    render(&mut app);
    assert_eq!(rect(&app, &Hit::Pane(PaneId::Editor)).height, editor.height + 3);
    app.handle_mouse(left_up(x, y + 3));
    assert!(app.resize.is_none());
    assert_eq!(app.focus, PaneId::Response);
    let saved = postui_core::fixtures::load_layout(app.proj().root()).unwrap().main_split;
    assert_eq!(saved, Some(app.split_state().to_token()));
}

/// P2: a finished drag is saved once, at its end.
#[test]
fn a_finished_resize_is_saved() {
    let mut app = App::new_for_test();
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(24, y));
    assert_eq!(saved_width(&app), None, "not on every motion");
    app.handle_mouse(left_up(24, y));
    assert_eq!(saved_width(&app), Some(24));

    render(&mut app);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 3));
    app.handle_mouse(left_up(x, y + 3));
    let saved = postui_core::fixtures::load_layout(app.proj().root()).unwrap().main_split;
    assert_eq!(saved, Some(app.split_state().to_token()));
    let loaded = SplitState::from_token(saved.as_deref().unwrap()).unwrap();
    assert!((loaded.ratio - app.split_ratio).abs() < 1e-4, "{loaded:?}");
    assert!(!loaded.editor_minimized && !loaded.response_minimized);
}

/// E2a: a click on the strip focuses the response at the press, as it
/// always has; the release that follows moves nothing.
#[test]
fn a_click_on_the_strip_focuses_the_response() {
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    let before = app.split_state();
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    assert_eq!(app.focus, PaneId::Response, "the press focuses the response");
    app.handle_mouse(left_up(x, y));
    assert_eq!(app.focus, PaneId::Response);
    assert_eq!(app.split_state(), before);
}

/// A click with a sideways jitter is still a click: a motion that leaves
/// the line on its row recomputes no share, so the split is untouched (and
/// the press focused the response).
#[test]
fn a_sideways_jitter_on_the_strip_moves_nothing() {
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    let before = app.split_state();
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    assert!(!app.handle_mouse(left_drag(x + 1, y)), "nothing to repaint");
    app.handle_mouse(left_up(x + 1, y));
    assert_eq!(app.split_state(), before, "the share is not rewritten");
    assert_eq!(app.focus, PaneId::Response, "the press focused the response");
}

/// A right click or focus loss ends a strip press's resize; an unmoved
/// one has nothing to save.
#[test]
fn a_cancelled_strip_press_ends_the_resize() {
    for via_focus_loss in [false, true] {
        let mut app = App::new_for_test();
        render(&mut app);
        let layout = app.proj().root().join(LAYOUT_TOML);
        let _ = std::fs::remove_file(&layout);
        let (x, y) = handle_cell(&app);
        app.handle_mouse(left_down(x, y));
        if via_focus_loss {
            app.on_focus_lost();
        } else {
            let editor = rect(&app, &Hit::Pane(PaneId::Editor));
            app.handle_mouse(right_down(editor.x + 2, editor.y + 1));
        }
        assert!(app.resize.is_none(), "focus loss {via_focus_loss}");
        assert!(!layout.exists(), "focus loss {via_focus_loss}: nothing saved");
    }
}

/// Review F3/F7: whether a drag moved, not where it ended, decides the
/// save: one that moves the line and brings it back is still saved.
#[test]
fn a_strip_drag_that_returns_to_its_start_is_still_saved() {
    let mut app = App::new_for_test();
    render(&mut app);
    // Start from the share the drawn rows invert to, so the return lands
    // on exactly the starting state.
    let editor = rect(&app, &Hit::Pane(PaneId::Editor));
    let column = editor.union(rect(&app, &Hit::Pane(PaneId::Response)));
    app.split_ratio = crate::layout::editor_share_for(column.height, editor.height);
    app.update(Action::Render);
    render(&mut app);
    let layout = app.proj().root().join(LAYOUT_TOML);
    let _ = std::fs::remove_file(&layout);
    let before = app.split_state();
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 3));
    render(&mut app);
    assert_ne!(app.split_state(), before, "the line moved");
    app.handle_mouse(left_drag(x, y));
    render(&mut app);
    assert_eq!(app.split_state(), before, "and came back");
    app.handle_mouse(left_up(x, y));
    let saved = postui_core::fixtures::load_layout(app.proj().root()).unwrap().main_split;
    assert_eq!(saved, Some(before.to_token()), "saved where it ended");
}

/// Review F7: a press and release on either handle that moved nothing
/// (a strip click, a gutter click) writes no `layout.toml`.
#[test]
fn a_handle_click_writes_no_state() {
    let mut app = App::new_for_test();
    render(&mut app);
    let layout = app.proj().root().join(LAYOUT_TOML);
    let _ = std::fs::remove_file(&layout);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_up(x, y));
    assert_eq!(app.focus, PaneId::Response, "the strip click still focuses");
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_up(x, y));
    assert!(!layout.exists(), "nothing was saved");
}

/// Review F5: the strip is still the response pane to a right click. A
/// blank cell of the header strip opens the response's Copy menu, as it
/// did before the strip became the split handle.
#[test]
fn right_click_on_the_strip_opens_the_response_text_menu() {
    let mut app = App::new_for_test();
    super::tests::ready_response(&mut app, "plain text body");
    render(&mut app);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(right_down(x, y));
    let Some(Modal::Dropdown(menu)) = app.modals.top() else {
        panic!("expected the response's context menu");
    };
    assert_eq!(menu.items.first().map(|i| i.label.as_str()), Some("Copy"));
}

/// Task #95: the cell between two icons and the cell between two tabs are
/// the strip's control band, not the handle. A press-and-drag there clicks
/// as the strip does (the response takes focus) but moves no line, the
/// pointer stays the plain arrow, and a right click is still the
/// response's menu.
#[test]
fn the_gaps_between_the_strips_controls_never_resize() {
    use crate::components::response::ViewMode;
    let mut app = App::new_for_test();
    super::tests::ready_response(&mut app, r#"{"a": 1}"#);
    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    let icon = rect(&app, &Hit::ResponseSearchButton);
    let tab = rect(&app, &Hit::ResponseTab(ViewMode::Pretty));
    for (x, y) in [(icon.right(), icon.y), (tab.right(), tab.y)] {
        assert_eq!(app.hits.hit_at(x, y), Some(&Hit::StripControls), "({x}, {y})");
        let before = app.split_state();
        let (hx, hy) = handle_cell(&app);
        app.handle_mouse(moved(hx, hy));
        assert_eq!(app.pointer_shape_update(), Some(PointerShape::RowResize), "the handle");
        app.handle_mouse(moved(x, y));
        assert_eq!(app.pointer_shape_update(), Some(PointerShape::Default), "({x}, {y})");
        app.handle_mouse(left_down(x, y));
        assert!(app.resize.is_none(), "({x}, {y}): no resize armed");
        assert_eq!(app.focus, PaneId::Response, "({x}, {y}): the press still focuses");
        app.handle_mouse(left_drag(x, y + 4));
        app.handle_mouse(left_up(x, y + 4));
        assert_eq!(app.split_state(), before, "({x}, {y}): the line stays");
        app.update(Action::FocusPane(PaneId::Editor));
        render(&mut app);

        app.handle_mouse(right_down(x, y));
        let Some(Modal::Dropdown(menu)) = app.modals.top() else {
            panic!("({x}, {y}): expected the response's context menu");
        };
        assert_eq!(menu.items.first().map(|i| i.label.as_str()), Some("Copy"));
        app.update(Action::CancelTopModal);
        render(&mut app);
    }
}

/// Task #95, vim: a press on the control band is the strip's status-line
/// press without the drag, so the current window, its Visual mode and its
/// selection all stay.
#[test]
fn a_vim_press_between_the_strips_controls_keeps_the_window() {
    use super::key_matrix_tests::{Fixture, build, draw, feed};
    let mut app = build(Fixture::Body);
    app.use_profile(crate::config::KeyProfile::Vim);
    super::tests::ready_response(&mut app, r#"{"a": 1}"#);
    draw(&mut app);
    feed(&mut app, "jwvll");
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    let selection = app.editor.body_visual;
    let icon = rect(&app, &Hit::ResponseSearchButton);
    assert_eq!(app.hits.hit_at(icon.right(), icon.y), Some(&Hit::StripControls));
    app.handle_mouse(left_down(icon.right(), icon.y));
    assert!(app.resize.is_none(), "no resize armed");
    app.handle_mouse(left_up(icon.right(), icon.y));
    assert_eq!(app.focus, PaneId::Editor, "the current window stays");
    assert_eq!(app.mode_indicator(), Some("VISUAL"), "still Visual");
    assert_eq!(app.editor.body_visual, selection, "the selection is unchanged");
}

// -- The keyboard (spec S6, S7, E11, E13) --------------------------------

/// The app in the vim profile with `focus`, after a frame.
fn vim_at(focus: PaneId) -> App {
    let mut app = App::new_for_test();
    app.use_profile(crate::config::KeyProfile::Vim);
    app.update(Action::FocusPane(focus));
    render(&mut app);
    app
}

/// Types `keys` (vim notation) through `handle_key`, then draws the frame
/// the next key measures against, as the event loop does.
fn type_keys(app: &mut App, keys: &str) {
    for ev in crate::keyroute::notation::events(keys) {
        app.handle_key(ev);
    }
    render(app);
}

fn sidebar_cols(app: &App) -> u16 {
    rect(app, &Hit::Pane(PaneId::Sidebar)).width
}

fn pane_rows(app: &App, pane: PaneId) -> u16 {
    rect(app, &Hit::Pane(pane)).height
}

/// Presses `keys` (vim notation, `<A-w>` for alt+w and `<A-W>` for
/// shift+alt+w) through `handle_key`, with no frame after.
fn press(app: &mut App, keys: &str) {
    for ev in crate::keyroute::notation::events(keys) {
        app.handle_key(ev);
    }
}

/// S6, S6a: with the sidebar focused, the `alt+w` key widens it and
/// `shift+alt+w` narrows it a step at a time from the width on screen,
/// stopping at the limit with no wrap. The split never moves. (The vim
/// profile binds no `alt+w`: its keymap drops the split chords.)
#[test]
fn alt_w_in_the_sidebar_steps_the_width() {
    let step = crate::layout::SIDEBAR_STEP_COLS.unsigned_abs();
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Sidebar));
    render(&mut app);
    let w = sidebar_cols(&app);
    let split = app.split_state();
    press(&mut app, "<A-w>");
    assert_eq!(app.sidebar_width, Some(w + step), "from the default's width on screen");
    assert_eq!(saved_width(&app), Some(w + step), "saved on the step");
    render(&mut app);
    press(&mut app, "<A-W>");
    render(&mut app);
    press(&mut app, "<A-W>");
    render(&mut app);
    assert_eq!(app.sidebar_width, Some(w - step));
    for _ in 0..40 {
        type_keys(&mut app, "<A-w>");
    }
    let limit = 120 - 1 - crate::layout::MAIN_MIN_COLS;
    assert_eq!(app.sidebar_width, Some(limit), "the limit at 120 wide");
    type_keys(&mut app, "<A-w>");
    assert_eq!(app.sidebar_width, Some(limit), "no wrap");
    assert_eq!(app.split_state(), split, "the split never moved");
}

/// S5: a step that can't move the sidebar (a saved width squeezed by a
/// narrow window, pushed further the same way) leaves the saved width as
/// it was.
#[test]
fn a_step_at_the_limit_keeps_a_squeezed_saved_width() {
    let mut app = App::new_for_test();
    app.sidebar_width = Some(100);
    app.update(Action::FocusPane(PaneId::Sidebar));
    render(&mut app);
    let limit = 120 - 1 - crate::layout::MAIN_MIN_COLS;
    assert_eq!(sidebar_cols(&app), limit, "squeezed for the frame");
    press(&mut app, "<A-w>");
    assert_eq!(app.sidebar_width, Some(100));
    press(&mut app, "<A-W>");
    assert_eq!(app.sidebar_width, Some(limit - 4), "a real step starts from the screen");
}

/// E11: anywhere but the sidebar, the `alt+w` key cycles the split as
/// before.
#[test]
fn alt_w_elsewhere_still_cycles_the_split() {
    use crate::split::SplitStop;
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    assert_eq!(app.split_state().stop(), Some(SplitStop::Even));
    press(&mut app, "<A-w>");
    assert_eq!(app.split_state().stop(), Some(SplitStop::ResponseBig));
    assert_eq!(app.sidebar_width, None, "the sidebar is untouched");
}

/// The screen height whose main column has `content` content rows.
fn height_for_content(content: u16) -> u16 {
    content
        + crate::components::editor::CHROME_HEIGHT
        + crate::components::response::HEADER_STRIP_HEIGHT
        + crate::components::header_bar::HEADER_HEIGHT
        + crate::components::footer::FOOTER_HEIGHT
}

/// Review round 4 I1, E11: a drag to the rows a stop draws keeps its exact
/// share, with no snapping (the user), and the first `alt+w` after it
/// still moves the line: the stop that draws those rows is skipped. With
/// 25 content rows, a drag to 13 draws what Even does (12.5 → 13), and the
/// press goes on to 25/75; the mirror, a drag to 6 draws 25/75's rows
/// (6.25 → 6), and `shift+alt+w` goes on to 50/50. Each drag first leaves
/// the row, so it arrives there by the drag's own inversion.
#[test]
fn the_first_alt_w_after_a_drag_always_moves_the_line() {
    use crate::split::SplitStop;
    let height = height_for_content(25);
    for (rows, keys, lands) in
        [(13, "<A-w>", SplitStop::ResponseBig), (6, "<A-W>", SplitStop::Even)]
    {
        let mut app = App::new_for_test();
        app.update(Action::FocusPane(PaneId::Editor));
        render_at(&mut app, 120, height);
        let column = rect(&app, &Hit::Pane(PaneId::Editor)).y;
        let (x, y) = handle_cell(&app);
        let grab = y - rect(&app, &Hit::SplitHandle).y;
        let to = |content: u16| column + crate::components::editor::CHROME_HEIGHT + content + grab;
        app.handle_mouse(left_down(x, y));
        app.handle_mouse(left_drag(x, to(rows + 2)));
        render_at(&mut app, 120, height);
        app.handle_mouse(left_drag(x, to(rows)));
        app.handle_mouse(left_up(x, to(rows)));
        render_at(&mut app, 120, height);
        let before = pane_rows(&app, PaneId::Editor);
        assert_eq!(before, crate::components::editor::CHROME_HEIGHT + rows, "dragged to {rows}");
        assert_eq!(app.split_ratio, f32::from(rows) / 25.0, "the exact share, no stop");
        app.update(Action::FocusPane(PaneId::Editor));
        press(&mut app, keys);
        assert_eq!(app.split_state().stop(), Some(lands), "{keys} from {rows} rows");
        app.anims.finish_all();
        render_at(&mut app, 120, height);
        assert_ne!(pane_rows(&app, PaneId::Editor), before, "{keys}: the line moved");
    }
}

/// New count, round 6 (E11): a share dragged at one height can draw a
/// stop's rows at another. Dragged to 12 of 25 content rows (0.48, no
/// stop's rows there), then the window shrinks to 20 content rows, where
/// 0.48 draws 10 rows, as 50/50 does. `shift+alt+w` skips 50/50 and the
/// line moves.
#[test]
fn alt_w_after_a_window_resize_still_moves_the_line() {
    use crate::split::SplitStop;
    let tall = height_for_content(25);
    let short = height_for_content(20);
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render_at(&mut app, 120, tall);
    let column = rect(&app, &Hit::Pane(PaneId::Editor)).y;
    let (x, y) = handle_cell(&app);
    let grab = y - rect(&app, &Hit::SplitHandle).y;
    let to = column + crate::components::editor::CHROME_HEIGHT + 12 + grab;
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, to));
    app.handle_mouse(left_up(x, to));
    assert_eq!(app.split_ratio, 12.0 / 25.0);
    render_at(&mut app, 120, short);
    let before = pane_rows(&app, PaneId::Editor);
    let short_column = 20
        + crate::components::editor::CHROME_HEIGHT
        + crate::components::response::HEADER_STRIP_HEIGHT;
    assert_eq!(before, crate::layout::editor_rows_for(short_column, 0.5), "draws 50/50's rows");
    press(&mut app, "<A-W>");
    assert_eq!(app.split_state().stop(), Some(SplitStop::EditorBig), "50/50 skipped");
    app.anims.finish_all();
    render_at(&mut app, 120, short);
    assert_ne!(pane_rows(&app, PaneId::Editor), before, "the line moved");
}

/// E11: a minimized editor is as tall as a shown one with no content rows,
/// but it is another screen. On a column of 1 content row, 25/75 gives the
/// editor none, and `alt+w` from there still goes to "editor minimized"
/// rather than skipping it.
#[test]
fn alt_w_reaches_the_minimized_editor_on_a_short_column() {
    use crate::split::SplitStop;
    let height = height_for_content(1);
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    app.update(Action::SplitStop(SplitStop::ResponseBig));
    render_at(&mut app, 120, height);
    assert_eq!(pane_rows(&app, PaneId::Editor), crate::components::editor::CHROME_HEIGHT);
    assert_eq!(
        crate::components::editor::COLLAPSED_HEIGHT,
        crate::components::editor::CHROME_HEIGHT,
        "the two draw the same rows"
    );
    press(&mut app, "<A-w>");
    assert_eq!(app.split_state().stop(), Some(SplitStop::ResponseFull));
}

/// Review round 5 K1 (J1), E11: on a column of 3 content rows, 75/25 and
/// 50/50 both draw 2 rows. Every `alt+w` press moves the line, so from
/// 75/25 the press skips 50/50 for 25/75.
#[test]
fn alt_w_on_a_short_column_always_moves_the_line() {
    use crate::split::SplitStop;
    let height = height_for_content(3);
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render_at(&mut app, 120, height);
    assert_eq!(app.split_state().stop(), Some(SplitStop::Even));
    let mut seen = Vec::new();
    for _ in 0..5 {
        let before = pane_rows(&app, PaneId::Editor);
        press(&mut app, "<A-w>");
        seen.push(app.split_state().stop());
        app.anims.finish_all();
        render_at(&mut app, 120, height);
        assert_ne!(pane_rows(&app, PaneId::Editor), before, "the press to {seen:?} moved it");
    }
    assert_eq!(
        seen,
        [
            Some(SplitStop::ResponseBig),
            Some(SplitStop::ResponseFull),
            Some(SplitStop::EditorFull),
            Some(SplitStop::EditorBig),
            Some(SplitStop::ResponseBig),
        ]
    );
}

/// Review round 5 K2: a strip click with a sideways jitter right after an
/// eased `alt+w` (the frame still partway through the ease) moves
/// nothing: the split stays the stop `alt+w` chose.
#[test]
fn a_strip_jitter_mid_ease_keeps_the_cycled_stop() {
    use crate::split::SplitStop;
    const TALL: u16 = 100;
    let mut target = App::new_for_test_with_anims(false);
    target.update(Action::SplitStop(SplitStop::ResponseBig));
    render_at(&mut target, 120, TALL);
    let target_rows = pane_rows(&target, PaneId::Editor);

    let mut app = App::new_for_test_with_anims(true);
    app.update(Action::FocusPane(PaneId::Editor));
    render_at(&mut app, 120, TALL);
    assert!(app.update(Action::CycleSplit));
    let cycled = app.split_state();
    assert_eq!(cycled.stop(), Some(SplitStop::ResponseBig));
    render_at(&mut app, 120, TALL);
    assert_ne!(pane_rows(&app, PaneId::Editor), target_rows, "the frame is mid-ease");
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x + 1, y));
    app.handle_mouse(left_up(x + 1, y));
    assert_eq!(app.split_state(), cycled);
}

/// Review round 5 K2 (J2): the event loop draws the ease's next frame
/// between a strip press made mid-ease and its sideways jitter. The jitter
/// is still a click: the split stays the stop `alt+w` chose, though no
/// frame now draws the line on the pressed row.
#[test]
fn a_strip_jitter_after_a_redraw_mid_ease_keeps_the_cycled_stop() {
    use crate::split::SplitStop;
    const TALL: u16 = 100;
    let mut app = App::new_for_test_with_anims(true);
    app.update(Action::FocusPane(PaneId::Editor));
    render_at(&mut app, 120, TALL);
    assert!(app.update(Action::CycleSplit));
    let cycled = app.split_state();
    assert_eq!(cycled.stop(), Some(SplitStop::ResponseBig));
    render_at(&mut app, 120, TALL);
    let pressed_rows = pane_rows(&app, PaneId::Editor);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    // The loop draws the next frame of the ease before the motion arrives.
    app.anims.finish_all();
    render_at(&mut app, 120, TALL);
    assert_ne!(pane_rows(&app, PaneId::Editor), pressed_rows, "the line moved under the press");
    assert!(!app.handle_mouse(left_drag(x + 1, y)), "nothing to repaint");
    app.handle_mouse(left_up(x + 1, y));
    assert_eq!(app.split_state(), cycled);
}

/// E5: a strip pressed mid-ease follows the pointer from where it was
/// drawn: dragged two rows down, the line is two rows below the drawn line
/// the user grabbed, not below where the ease would have settled.
#[test]
fn a_strip_drag_pressed_mid_ease_follows_the_pointer_from_the_drawn_line() {
    use crate::split::SplitStop;
    const TALL: u16 = 100;
    let mut target = App::new_for_test_with_anims(false);
    target.update(Action::SplitStop(SplitStop::ResponseBig));
    render_at(&mut target, 120, TALL);
    let target_rows = pane_rows(&target, PaneId::Editor);

    let mut app = App::new_for_test_with_anims(true);
    app.update(Action::FocusPane(PaneId::Editor));
    render_at(&mut app, 120, TALL);
    assert!(app.update(Action::CycleSplit));
    render_at(&mut app, 120, TALL);
    let pressed_rows = pane_rows(&app, PaneId::Editor);
    assert_ne!(pressed_rows, target_rows, "the frame is mid-ease");
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 2));
    render_at(&mut app, 120, TALL);
    assert_eq!(pane_rows(&app, PaneId::Editor), pressed_rows + 2, "under the pointer");
    app.handle_mouse(left_up(x, y + 2));
    app.anims.finish_all();
    render_at(&mut app, 120, TALL);
    assert_eq!(pane_rows(&app, PaneId::Editor), pressed_rows + 2, "and it stays there");
}

/// K2: once a drag has moved the line, the pressed row is an ordinary
/// row. A drag that comes back to it puts the line back there.
#[test]
fn a_strip_drag_back_to_the_pressed_row_moves_the_line_back() {
    let mut app = App::new_for_test();
    render(&mut app);
    let editor = pane_rows(&app, PaneId::Editor);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 3));
    render(&mut app);
    assert!(app.handle_mouse(left_drag(x + 1, y)), "the line moves back");
    render(&mut app);
    assert_eq!(pane_rows(&app, PaneId::Editor), editor);
    assert!(!app.handle_mouse(left_drag(x + 2, y)), "a jitter there after a move moves nothing");
    app.handle_mouse(left_up(x + 2, y));
}
/// S6a (review round 2 G1): the sidebar context belongs to the key. The
/// split action itself always moves the split, so Vim's `:split` with the
/// sidebar focused cycles the split and leaves the sidebar alone.
#[test]
fn ex_split_in_the_sidebar_cycles_the_split() {
    use crate::split::SplitStop;
    let mut app = vim_at(PaneId::Sidebar);
    let w = sidebar_cols(&app);
    assert_eq!(app.split_state().stop(), Some(SplitStop::Even));
    type_keys(&mut app, ":split<CR>");
    assert!(app.modals.is_empty(), "the line ran and closed");
    assert_eq!(app.split_state().stop(), Some(SplitStop::ResponseBig));
    assert_eq!((app.sidebar_width, sidebar_cols(&app)), (None, w), "the sidebar is untouched");
    assert_eq!(app.focus, PaneId::Sidebar);
}

/// S6a, S6b: in the sidebar, the split keys' bindings become the
/// sidebar-only step, not vim's `PaneResize` (which follows the current
/// window). Anywhere else they stay the split's.
#[test]
fn the_split_keys_become_the_sidebar_step_in_the_sidebar() {
    let step = crate::layout::SIDEBAR_STEP_COLS;
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Sidebar));
    let ctx = app.key_context();
    assert_eq!(ctx.action(Action::CycleSplit), Action::StepSidebar(step));
    assert_eq!(ctx.action(Action::CycleSplitBack), Action::StepSidebar(-step));
    app.update(Action::FocusPane(PaneId::Editor));
    let ctx = app.key_context();
    assert_eq!(ctx.action(Action::CycleSplit), Action::CycleSplit);
    assert_eq!(ctx.action(Action::CycleSplitBack), Action::CycleSplitBack);
}

/// Review round 8 N6: the one keymap read applies the key's context, and
/// leaves every other binding, the quit key's included, as bound.
#[test]
fn the_keymap_read_applies_the_keys_context() {
    use crate::keys::KeyCombo;
    let combo = |s: &str| KeyCombo::parse(s).unwrap();
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Sidebar));
    let step = crate::layout::SIDEBAR_STEP_COLS;
    assert_eq!(app.lookup_key(&combo("alt+w")), Some(Action::StepSidebar(step)));
    assert_eq!(app.lookup_key(&combo("ctrl+c")), Some(Action::Quit));
    app.update(Action::FocusPane(PaneId::Editor));
    assert_eq!(app.lookup_key(&combo("alt+w")), Some(Action::CycleSplit));
}

/// S6b: `StepSidebar` always means the sidebar, wherever focus is. From
/// the editor a positive step widens it (where vim's `ctrl+w >` from the
/// editor narrows it, S7), and the step is saved.
#[test]
fn step_sidebar_from_the_editor_widens_the_sidebar() {
    let step = crate::layout::SIDEBAR_STEP_COLS;
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    let (w, split) = (sidebar_cols(&app), app.split_state());
    assert!(app.update(Action::StepSidebar(step)));
    let widened = w + step.unsigned_abs();
    assert_eq!(app.sidebar_width, Some(widened));
    assert_eq!(saved_width(&app), Some(widened), "saved on the step");
    assert_eq!(app.split_state(), split, "the split never moved");
    assert_eq!(app.focus, PaneId::Editor);
}

/// E12a, E12b: `shift+alt+p` hides the response and shows it again in both
/// profiles, and the ▁▁ chip's hint names it, wherever focus is.
#[test]
fn shift_alt_p_toggles_the_response_and_the_chip_hint_names_it() {
    let alt = crate::keys::alt_label();
    let chip = Hit::SplitStop(crate::split::SplitStop::EditorFull);
    let hint = |app: &App| crate::hint::hint_for(&chip, &app.keymap, &app.hint_ctx(&chip));
    for profile in [crate::config::KeyProfile::Arrows, crate::config::KeyProfile::Vim] {
        for focus in [PaneId::Sidebar, PaneId::Editor] {
            let mut app = App::new_for_test();
            app.use_profile(profile);
            app.update(Action::FocusPane(focus));
            render(&mut app);
            let before = app.split_state();
            assert_eq!(
                hint(&app),
                Some(format!("Hide the response \u{b7} {alt}+shift+p")),
                "{profile:?} {focus:?}"
            );
            press(&mut app, "<A-P>");
            assert!(app.session.response.collapsed, "{profile:?} {focus:?}: hidden");
            assert_eq!(
                hint(&app),
                Some(format!("Show the response \u{b7} {alt}+shift+p")),
                "{profile:?} {focus:?}"
            );
            press(&mut app, "<A-P>");
            assert_eq!(app.split_state(), before, "{profile:?} {focus:?}: shown again");
        }
    }
}

/// Review round 6 L8 (E12a): only the two end stops are chips, each with
/// its toggle's hint. A middle stop has no chip and names no toggle.
#[test]
fn only_the_end_stops_have_a_chip_hint() {
    use crate::split::SplitStop;
    let app = App::new_for_test();
    let hint = |stop| {
        let chip = Hit::SplitStop(stop);
        crate::hint::hint_for(&chip, &app.keymap, &app.hint_ctx(&chip))
    };
    for stop in [SplitStop::EditorFull, SplitStop::ResponseFull] {
        assert!(hint(stop).is_some(), "{stop:?}");
    }
    for stop in [SplitStop::EditorBig, SplitStop::Even, SplitStop::ResponseBig] {
        assert_eq!(hint(stop), None, "{stop:?}");
    }
}

/// S6b: the palette's "Widen sidebar" / "Narrow sidebar" run from the
/// editor move the sidebar's edge a step the way they say.
#[test]
fn the_palette_widens_and_narrows_the_sidebar_from_the_editor() {
    let step = i32::from(crate::layout::SIDEBAR_STEP_COLS);
    for (id, delta) in [("sidebar-widen", step), ("sidebar-narrow", -step)] {
        let mut app = App::new_for_test();
        app.update(Action::FocusPane(PaneId::Editor));
        render(&mut app);
        let w = i32::from(sidebar_cols(&app));
        app.update(Action::OpenPalette);
        press(&mut app, "sidebar");
        super::tests::select_palette_command(&mut app, id);
        press(&mut app, "<CR>");
        assert!(app.modals.is_empty(), "{id}: the palette ran and closed");
        render(&mut app);
        assert_eq!(i32::from(sidebar_cols(&app)), w + delta, "{id}");
        assert_eq!(app.focus, PaneId::Editor, "{id}");
    }
}

/// S6a: the vim `:` palette's key column takes the same key context as
/// ctrl+p's. With a keys.toml that binds the split keys in vim (the arrows
/// defaults stand in for it), the split rows show no key while the sidebar
/// has focus, where that key widens the sidebar, and show it from the
/// editor.
#[test]
fn the_ex_palette_names_the_split_keys_only_where_they_move_the_split() {
    for (focus, shown) in [(PaneId::Sidebar, false), (PaneId::Editor, true)] {
        let mut app = App::new_for_test();
        app.use_profile(crate::config::KeyProfile::Vim);
        app.keymap = crate::keys::Keymap::default_bindings();
        app.update(Action::FocusPane(focus));
        render(&mut app);
        app.update(Action::OpenExPalette);
        let Some(Modal::Palette(palette)) = app.modals.top() else {
            panic!("{focus:?}: the palette is open");
        };
        assert!(palette.is_ex());
        let row = palette.filtered().iter().find(|c| c.id == "split-step").expect("split-step");
        assert_eq!(palette.key_column(row, &app.keymap).is_some(), shown, "{focus:?}");
    }
}

/// Review round 7 M4 (S6a): the palette's split rows name the split keys
/// only where those keys move the split. Opened from the sidebar, where
/// `alt+w` widens the sidebar, they show no key; from the editor they show
/// `alt+w` and `alt+shift+w`.
#[test]
fn the_palette_names_the_split_keys_only_where_they_move_the_split() {
    let alt = crate::keys::alt_label();
    for (focus, keys) in [
        (PaneId::Sidebar, [None, None]),
        (PaneId::Editor, [Some(format!("{alt}+w")), Some(format!("{alt}+shift+w"))]),
    ] {
        let mut app = App::new_for_test();
        app.update(Action::FocusPane(focus));
        render(&mut app);
        app.update(Action::OpenPalette);
        let Some(Modal::Palette(palette)) = app.modals.top() else {
            panic!("{focus:?}: the palette is open");
        };
        let shown = ["split-step", "split-step-back"].map(|id| {
            let row = palette.filtered().iter().find(|c| c.id == id).expect(id);
            palette.key_column(row, &app.keymap)
        });
        assert_eq!(shown, keys, "{focus:?}");
    }
}

/// Review round 7 M2, round 9 O1 (S8, S8a): a step from the Manage screen
/// moves the edge and is saved, and each further step moves it again: the
/// width comes from the saved width and the terminal, which every screen's
/// frame records, not from a Main frame. Main is never drawn here.
#[test]
fn step_sidebar_from_the_manage_screen_widens_the_sidebar_each_time() {
    let step = crate::layout::SIDEBAR_STEP_COLS;
    let mut app = App::new_for_test();
    app.update(Action::OpenManage { tab: Some(crate::components::manage::ManageTab::Spaces) });
    render(&mut app);
    assert_eq!(app.screen, Screen::Manage);
    let w = crate::layout::sidebar_cols(120, None);
    assert!(app.update(Action::StepSidebar(step)));
    render(&mut app);
    assert!(app.update(Action::StepSidebar(step)));
    let widened = w + 2 * step.unsigned_abs();
    assert_eq!(app.sidebar_width, Some(widened), "two steps, two moves");
    assert_eq!(saved_width(&app), Some(widened), "saved on the step");
    app.update(Action::CloseScreen);
    render(&mut app);
    assert_eq!(sidebar_cols(&app), widened, "Main draws it");
}

/// Review round 7 M2: with no frame ever drawn there is nothing to
/// measure, and a step changes nothing.
#[test]
fn step_sidebar_before_any_frame_is_a_no_op() {
    let mut app = App::new_for_test();
    assert!(!app.update(Action::StepSidebar(crate::layout::SIDEBAR_STEP_COLS)));
    assert_eq!(app.sidebar_width, None);
}

/// S7, E13: the plan's Vim 9.1 table, one assertion per row. `>`/`<` act
/// on the current window, so from the editor or response they move the
/// sidebar the other way; `+`/`-` move the editor/response line for the
/// pane that has focus, and do nothing on the full-height sidebar.
#[test]
fn vim_ctrl_w_resizes_follow_the_current_window() {
    // Sidebar: `N<C-w>>` / `<` → the sidebar ±N.
    let mut app = vim_at(PaneId::Sidebar);
    let w = sidebar_cols(&app);
    type_keys(&mut app, "3<C-w>>");
    assert_eq!(sidebar_cols(&app), w + 3);
    type_keys(&mut app, "2<C-w><lt>");
    assert_eq!(sidebar_cols(&app), w + 1);

    // Editor or response: `N<C-w>>` / `<` → that window ±N, the sidebar ∓N.
    for focus in [PaneId::Editor, PaneId::Response] {
        let mut app = vim_at(focus);
        let w = sidebar_cols(&app);
        type_keys(&mut app, "3<C-w>>");
        assert_eq!(sidebar_cols(&app), w - 3, "{focus:?}");
        type_keys(&mut app, "<C-w><lt>");
        assert_eq!(sidebar_cols(&app), w - 2, "{focus:?}");
        assert_eq!(app.focus, focus, "the focus stays");
    }

    // Sidebar: `N<C-w>+` / `-` → nothing.
    let mut app = vim_at(PaneId::Sidebar);
    let (w, split) = (sidebar_cols(&app), app.split_state());
    for keys in ["2<C-w>+", "2<C-w>-"] {
        type_keys(&mut app, keys);
        assert_eq!((sidebar_cols(&app), app.split_state()), (w, split), "{keys}");
    }

    // Editor: `N<C-w>+` / `-` → the editor ±N, the response ∓N.
    let mut app = vim_at(PaneId::Editor);
    let (e, r) = (pane_rows(&app, PaneId::Editor), pane_rows(&app, PaneId::Response));
    type_keys(&mut app, "2<C-w>+");
    assert_eq!(
        (pane_rows(&app, PaneId::Editor), pane_rows(&app, PaneId::Response)),
        (e + 2, r - 2)
    );
    type_keys(&mut app, "<C-w>-");
    assert_eq!(
        (pane_rows(&app, PaneId::Editor), pane_rows(&app, PaneId::Response)),
        (e + 1, r - 1)
    );

    // Response: `N<C-w>+` / `-` → the response ±N, the editor ∓N.
    let mut app = vim_at(PaneId::Response);
    let (e, r) = (pane_rows(&app, PaneId::Editor), pane_rows(&app, PaneId::Response));
    type_keys(&mut app, "2<C-w>+");
    assert_eq!(
        (pane_rows(&app, PaneId::Editor), pane_rows(&app, PaneId::Response)),
        (e - 2, r + 2)
    );
    type_keys(&mut app, "<C-w>-");
    assert_eq!(
        (pane_rows(&app, PaneId::Editor), pane_rows(&app, PaneId::Response)),
        (e - 1, r + 1)
    );
    assert_eq!(
        postui_core::fixtures::load_layout(app.proj().root()).unwrap().main_split,
        Some(app.split_state().to_token()),
        "each step is saved"
    );
}

/// E13 (review round 4): `<C-w>_` maximizes the current window: from the
/// editor it minimizes the response, from the response the editor, and on
/// the full-height sidebar it does nothing.
#[test]
fn ctrl_w_underscore_maximizes_the_current_window() {
    let app = |focus| {
        let mut app = vim_at(focus);
        type_keys(&mut app, "<C-w>_");
        app.split_state()
    };
    let editor = app(PaneId::Editor);
    assert!(editor.response_minimized && !editor.editor_minimized, "editor: {editor:?}");
    let response = app(PaneId::Response);
    assert!(response.editor_minimized && !response.response_minimized, "response: {response:?}");
    let mut sidebar = vim_at(PaneId::Sidebar);
    let (w, split) = (sidebar_cols(&sidebar), sidebar.split_state());
    type_keys(&mut sidebar, "<C-w>_");
    assert_eq!((sidebar_cols(&sidebar), sidebar.split_state()), (w, split), "sidebar: no-op");
}

/// E13: `N<C-w>_` sets the focused pane's content rows to N: the editor
/// below its chrome, the response below its header strip.
#[test]
fn ctrl_w_underscore_with_a_count_sets_the_content_rows() {
    let mut app = vim_at(PaneId::Editor);
    type_keys(&mut app, "5<C-w>_");
    assert_eq!(pane_rows(&app, PaneId::Editor), crate::components::editor::CHROME_HEIGHT + 5);
    let mut app = vim_at(PaneId::Response);
    type_keys(&mut app, "5<C-w>_");
    assert_eq!(
        pane_rows(&app, PaneId::Response),
        crate::components::response::HEADER_STRIP_HEIGHT + 5
    );
}

/// E13: `<C-w>=` evens a dragged share to 50/50.
#[test]
fn ctrl_w_equal_evens_a_dragged_share() {
    let mut app = vim_at(PaneId::Editor);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 3));
    app.handle_mouse(left_up(x, y + 3));
    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    assert_ne!(app.split_state(), SplitState::default(), "the drag moved the line");
    type_keys(&mut app, "<C-w>=");
    assert_eq!(app.split_state(), SplitState::default());
}

/// Review Focus 5: a huge count clamps to the sidebar's limits, or snaps
/// the split to its minimized end, with no overflow.
#[test]
fn huge_vim_counts_clamp() {
    let mut app = vim_at(PaneId::Sidebar);
    type_keys(&mut app, "500<C-w>>");
    assert_eq!(app.sidebar_width, Some(120 - 1 - crate::layout::MAIN_MIN_COLS));
    type_keys(&mut app, "500<C-w><lt>");
    assert_eq!(app.sidebar_width, Some(crate::layout::SIDEBAR_MIN_COLS));

    let mut app = vim_at(PaneId::Editor);
    let ratio = app.split_ratio;
    type_keys(&mut app, "500<C-w>+");
    assert!(app.session.response.collapsed, "editor only");
    assert_eq!(app.split_ratio, ratio, "the minimized end keeps the share");
    type_keys(&mut app, "500<C-w>>");
    assert_eq!(app.sidebar_width, Some(crate::layout::SIDEBAR_MIN_COLS));

    let mut app = vim_at(PaneId::Editor);
    type_keys(&mut app, "500<C-w>-");
    assert!(app.table_collapsed, "response only");
}

/// Review round 2 G6: `<C-w>+` right after an eased split change counts
/// from the split the state holds, not from the frame partway through the
/// ease: the editor ends one row taller than the stop `alt+w` went to. Tall,
/// so the two stops are far apart in rows.
#[test]
fn ctrl_w_plus_mid_ease_counts_from_the_target() {
    use crate::split::SplitStop;
    const TALL: u16 = 100;
    // The stop's settled height, from an app that does not ease.
    let mut target = App::new_for_test_with_anims(false);
    target.update(Action::SplitStop(SplitStop::ResponseBig));
    render_at(&mut target, 120, TALL);
    let target_rows = pane_rows(&target, PaneId::Editor);

    let mut app = App::new_for_test_with_anims(true);
    app.use_profile(crate::config::KeyProfile::Vim);
    app.update(Action::FocusPane(PaneId::Editor));
    render_at(&mut app, 120, TALL);
    assert_eq!(app.split_state().stop(), Some(SplitStop::Even));
    assert!(app.update(Action::CycleSplit));
    assert_eq!(app.split_state().stop(), Some(SplitStop::ResponseBig));
    render_at(&mut app, 120, TALL);
    assert_ne!(pane_rows(&app, PaneId::Editor), target_rows, "the frame is mid-ease");
    for ev in crate::keyroute::notation::events("<C-w>+") {
        app.handle_key(ev);
    }
    render_at(&mut app, 120, TALL);
    assert_eq!(pane_rows(&app, PaneId::Editor), target_rows + 1);
}

/// Review round 6 L2 (E13): `<C-w>=` evens the split and also sets the
/// sidebar to half the body, `(body − 1) / 2`, clamped (S5), as Vim's
/// CTRL-W = makes all windows equally wide too. The width is saved.
#[test]
fn ctrl_w_equal_also_halves_the_sidebar() {
    const WIDE: u16 = 160;
    let mut app = App::new_for_test();
    app.use_profile(crate::config::KeyProfile::Vim);
    app.sidebar_width = Some(30);
    app.update(Action::FocusPane(PaneId::Editor));
    app.update(Action::SplitStop(crate::split::SplitStop::EditorBig));
    render_at(&mut app, WIDE, 40);
    assert_eq!(sidebar_cols(&app), 30);
    press(&mut app, "<C-w>=");
    let half = crate::layout::clamp_sidebar_cols(WIDE, i32::from((WIDE - 1) / 2));
    assert_eq!(app.split_state(), SplitState::default(), "50/50");
    assert_eq!(app.sidebar_width, Some(half));
    assert_eq!(saved_width(&app), Some(half), "saved");
    render_at(&mut app, WIDE, 40);
    assert_eq!(sidebar_cols(&app), half);
}

/// Review round 6 L1 (S7): in vim a press on the sidebar gutter is no
/// click, as Vim's press on a window separator is not. The body keeps its
/// Visual selection through the press, the drag and the release, and a
/// pending operator through a press.
#[test]
fn a_vim_gutter_drag_keeps_the_bodys_visual_and_operator() {
    use super::key_matrix_tests::{Fixture, build, draw, feed};
    let mut app = build(Fixture::Body);
    app.use_profile(crate::config::KeyProfile::Vim);
    draw(&mut app);
    feed(&mut app, "jwvll");
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    let selection = app.editor.body_visual;
    assert!(selection.is_some());
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x + 4, y));
    app.handle_mouse(left_up(x + 4, y));
    assert!(app.sidebar_width.is_some(), "the drag resized");
    assert_eq!(app.mode_indicator(), Some("VISUAL"), "still Visual");
    assert_eq!(app.editor.body_visual, selection, "the selection is unchanged");

    feed(&mut app, "<Esc>d");
    draw(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_up(x, y));
    feed(&mut app, "d");
    assert_eq!(app.editor.body_text(), "{\n}", "the `d` before the press and the one after");
}

/// Review round 9 O2 (P5): a project created in place keeps the sizes on
/// screen and saves them as its own, over whatever its folder's
/// `layout.toml` held (here another postui's sizes, written from
/// outside before the init). Main kept them; rounds 6 and 8 had the init
/// seed from the file, and a size with nothing in it fell to the default.
#[test]
fn init_project_here_keeps_and_saves_the_sizes_on_screen() {
    let mut app = App::new_for_test();
    assert!(!postui_core::project::Project::is_project(app.proj().root()));
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(24, y));
    app.handle_mouse(left_up(24, y));
    render(&mut app);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 3));
    app.handle_mouse(left_up(x, y + 3));
    let split = app.split_state();
    assert_ne!(split, SplitState::default(), "the drag moved the line");
    let root = app.proj().root().to_path_buf();
    let mut outside = postui_core::fixtures::load_layout(&root).unwrap();
    outside.sidebar_width = Some(30);
    outside.main_split = Some(
        SplitState { editor_minimized: false, response_minimized: true, ratio: 0.6 }.to_token(),
    );
    postui_core::fixtures::save_layout(&root, &outside).unwrap();
    app.update(Action::InitProjectHere);
    assert!(postui_core::project::Project::is_project(app.proj().root()));
    assert_eq!(app.sidebar_width, Some(24), "the width on screen");
    assert_eq!(app.split_state(), split, "the split on screen");
    let saved = postui_core::fixtures::load_layout(app.proj().root()).unwrap();
    assert_eq!(saved.sidebar_width, Some(24), "saved as the project's");
    assert_eq!(saved.main_split.as_deref().and_then(SplitState::from_token), Some(split));
    render(&mut app);
    assert_eq!(sidebar_cols(&app), 24, "drawn");
}

/// P5: a launch takes the project's saved sizes even when its spaces can't
/// be made (here `requests` is a file), so the screen draws what the
/// project's local state holds, and the next size save can't write a
/// width that was never on screen.
#[test]
fn a_launch_seeds_the_sizes_when_the_spaces_fail() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("requests"), "").unwrap();
    let mut layout = postui_core::fixtures::load_layout(dir.path()).unwrap();
    layout.main_split = Some(
        SplitState { editor_minimized: false, response_minimized: false, ratio: 0.6 }.to_token(),
    );
    layout.sidebar_width = Some(34);
    postui_core::fixtures::save_layout(dir.path(), &layout).unwrap();
    let app = App::with_root(tx, dir.path().to_path_buf());
    assert!(
        app.toasts.messages().iter().any(|m| m.contains("could not open project")),
        "the spaces failed: {:?}",
        app.toasts.messages()
    );
    assert_eq!(app.sidebar_width, Some(34));
    assert_eq!(app.split_ratio, 0.6);
}

/// Rework round 8 (S7): in vim the response's header strip is the editor
/// window's status line, and Vim 9.1 keeps the current window and Visual
/// mode through a press there. A strip drag keeps the body's Visual
/// selection and the editor's focus, and a press keeps a pending operator,
/// as the gutter does.
#[test]
fn a_vim_strip_drag_keeps_the_bodys_visual_focus_and_operator() {
    use super::key_matrix_tests::{Fixture, build, draw, feed};
    let mut app = build(Fixture::Body);
    app.use_profile(crate::config::KeyProfile::Vim);
    draw(&mut app);
    feed(&mut app, "jwvll");
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    let selection = app.editor.body_visual;
    let split = app.split_state();
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 2));
    app.handle_mouse(left_up(x, y + 2));
    assert_ne!(app.split_state(), split, "the drag resized");
    assert_eq!(app.mode_indicator(), Some("VISUAL"), "still Visual");
    assert_eq!(app.editor.body_visual, selection, "the selection is unchanged");
    assert_eq!(app.focus, PaneId::Editor, "the editor keeps focus");

    feed(&mut app, "<Esc>d");
    draw(&mut app);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_up(x, y));
    feed(&mut app, "d");
    assert_eq!(app.editor.body_text(), "{\n}", "the `d` before the press and the one after");
}

/// Audit (spec P2a): a quit while a resize is live ends it where it got
/// to, saved, as every other end of a resize does.
#[test]
fn a_quit_mid_resize_saves_the_size() {
    let mut app = App::new_for_test();
    render(&mut app);
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(24, y));
    app.update(Action::ForceQuit);
    assert!(app.should_quit);
    assert!(app.resize.is_none(), "the quit ended the resize");
    assert_eq!(saved_width(&app), Some(24), "and saved it");
}

/// E7: from "editor only", `<C-w>+` on the response's strip grows it by
/// one row below the strip.
#[test]
fn ctrl_w_plus_grows_a_minimized_response_from_its_strip() {
    let mut app = vim_at(PaneId::Response);
    app.update(Action::SplitStop(crate::split::SplitStop::EditorFull));
    render(&mut app);
    assert!(app.session.response.collapsed);
    app.update(Action::FocusPane(PaneId::Response));
    render(&mut app);
    type_keys(&mut app, "<C-w>+");
    assert!(!app.session.response.collapsed);
    assert_eq!(
        pane_rows(&app, PaneId::Response),
        crate::components::response::HEADER_STRIP_HEIGHT + 1
    );
}
