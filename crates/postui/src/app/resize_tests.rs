//! The resize drag (task #90, spec S1, S3, E2, E4–E7, E9a, P2, P2a): the
//! sidebar gutter sets the sidebar width in columns, the response header
//! strip moves the editor/response line, and every way a drag can end keeps
//! and saves the size.

use super::*;
use crate::split::SplitState;
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
    postui_core::fixtures::load_local_state(app.proj().root()).unwrap().sidebar_width
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
    assert_eq!(app.sidebar_width, Some(79), "120 wide leaves the gutter and 40 for the main");
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

/// Review F2: on a window too narrow for the 28% default (S5's limits
/// allow less than it draws), a step or drag that asks to widen the
/// sidebar never narrows it to the limit, nor saves a width.
#[test]
fn widening_a_default_wider_than_the_limit_moves_nothing() {
    const WIDE: u16 = 50;
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Sidebar));
    render_at(&mut app, WIDE, 30);
    let drawn = sidebar_cols(&app);
    let body = drawn + 1 + rect(&app, &Hit::Pane(PaneId::Editor)).width;
    assert!(
        drawn > crate::layout::clamp_sidebar_cols(body, i32::MAX),
        "the default ({drawn}) is past the limit"
    );
    press(&mut app, "<A-w>");
    assert_eq!(app.sidebar_width, None);
    render_at(&mut app, WIDE, 30);
    assert_eq!(sidebar_cols(&app), drawn);

    let g = rect(&app, &Hit::SidebarGutter);
    let y = g.y + g.height / 2;
    app.handle_mouse(left_down(g.x, y));
    app.handle_mouse(left_drag(g.x + 6, y));
    app.handle_mouse(left_up(g.x + 6, y));
    assert_eq!(app.sidebar_width, None, "a rightward drag doesn't shrink it");
    render_at(&mut app, WIDE, 30);
    assert_eq!(sidebar_cols(&app), drawn);
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
    let width_in = |root: &std::path::Path| {
        postui_core::fixtures::load_local_state(root).unwrap().sidebar_width
    };
    assert_eq!(width_in(a.path()), Some(24), "saved into the project being left");
    let split = app.split_state();
    render(&mut app);
    app.handle_mouse(left_drag(40, y));
    app.handle_mouse(left_up(40, y));
    assert_eq!(app.sidebar_width, None, "the next project's width is untouched");
    assert_eq!(width_in(b.path()), None, "and nothing saved into it");
    assert_eq!(app.split_state(), split);
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
    app.editor.table.selected = Some(0);
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
    assert_eq!(app.editor.table.selected, Some(0), "and its row selected");
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
    assert_eq!(app.editor.table.selected, None, "its row deselected");
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
    let saved = postui_core::fixtures::load_local_state(app.proj().root()).unwrap().main_split;
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
    let saved = postui_core::fixtures::load_local_state(app.proj().root()).unwrap().main_split;
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
        let state = app.proj().root().join(".local").join("state.toml");
        let _ = std::fs::remove_file(&state);
        let (x, y) = handle_cell(&app);
        app.handle_mouse(left_down(x, y));
        if via_focus_loss {
            app.on_focus_lost();
        } else {
            let editor = rect(&app, &Hit::Pane(PaneId::Editor));
            app.handle_mouse(right_down(editor.x + 2, editor.y + 1));
        }
        assert!(app.resize.is_none(), "focus loss {via_focus_loss}");
        assert!(!state.exists(), "focus loss {via_focus_loss}: nothing saved");
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
    let state = app.proj().root().join(".local").join("state.toml");
    let _ = std::fs::remove_file(&state);
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
    let saved = postui_core::fixtures::load_local_state(app.proj().root()).unwrap().main_split;
    assert_eq!(saved, Some(before.to_token()), "saved where it ended");
}

/// Review F7: a press and release on either handle that moved nothing
/// (a strip click, a gutter click) writes no `.local/state.toml`.
#[test]
fn a_handle_click_writes_no_state() {
    let mut app = App::new_for_test();
    render(&mut app);
    let state = app.proj().root().join(".local").join("state.toml");
    let _ = std::fs::remove_file(&state);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_up(x, y));
    assert_eq!(app.focus, PaneId::Response, "the strip click still focuses");
    let (x, y) = gutter_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_up(x, y));
    assert!(!state.exists(), "nothing was saved");
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
    assert_eq!(app.sidebar_width, Some(79), "the limit at 120 wide");
    type_keys(&mut app, "<A-w>");
    assert_eq!(app.sidebar_width, Some(79), "no wrap");
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
    assert_eq!(sidebar_cols(&app), 79, "squeezed for the frame");
    press(&mut app, "<A-w>");
    assert_eq!(app.sidebar_width, Some(100));
    press(&mut app, "<A-W>");
    assert_eq!(app.sidebar_width, Some(75), "a real step starts from the screen");
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

/// Review round 4 I1, round 5 K1: a drag to the rows a stop draws stores
/// that stop, so the first `alt+w` after it always moves the line. With 25
/// content rows, a drag to 13 is Even (12.5 → 13), and the press goes on
/// to 25/75; the mirror, a drag to 6 is 25/75 (6.25 → 6), and
/// `shift+alt+w` goes on to 50/50. Each drag first leaves the row, so it
/// arrives there by the drag's own inversion.
#[test]
fn the_first_alt_w_after_a_drag_always_moves_the_line() {
    use crate::split::SplitStop;
    let height = height_for_content(25);
    for (rows, at, keys, lands) in [
        (13, SplitStop::Even, "<A-w>", SplitStop::ResponseBig),
        (6, SplitStop::ResponseBig, "<A-W>", SplitStop::Even),
    ] {
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
        assert_eq!(app.split_state().stop(), Some(at), "a drag to {rows} rows stores the stop");
        app.update(Action::FocusPane(PaneId::Editor));
        press(&mut app, keys);
        assert_eq!(app.split_state().stop(), Some(lands), "{keys} from {rows} rows");
        app.anims.finish_all();
        render_at(&mut app, 120, height);
        assert_ne!(pane_rows(&app, PaneId::Editor), before, "{keys}: the line moved");
    }
}

/// Review round 5 K1 (J1): on a column of 3 content rows, 75/25 and 50/50
/// both draw 2 rows. Each `alt+w` press still changes the split (some may
/// draw the same rows), and five presses visit every stop and wrap.
#[test]
fn alt_w_on_a_short_column_never_sticks() {
    use crate::split::SplitStop;
    let height = height_for_content(3);
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render_at(&mut app, 120, height);
    assert_eq!(app.split_state().stop(), Some(SplitStop::Even));
    let mut seen = Vec::new();
    for _ in 0..5 {
        let before = app.split_state();
        press(&mut app, "<A-w>");
        assert_ne!(app.split_state(), before, "the press after {seen:?} moved the split");
        seen.push(app.split_state().stop());
        app.anims.finish_all();
        render_at(&mut app, 120, height);
    }
    assert_eq!(
        seen,
        [
            Some(SplitStop::ResponseBig),
            Some(SplitStop::ResponseFull),
            Some(SplitStop::EditorFull),
            Some(SplitStop::EditorBig),
            Some(SplitStop::Even),
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
    assert_eq!(app.key_action(Action::CycleSplit), Action::StepSidebar(step));
    assert_eq!(app.key_action(Action::CycleSplitBack), Action::StepSidebar(-step));
    app.update(Action::FocusPane(PaneId::Editor));
    assert_eq!(app.key_action(Action::CycleSplit), Action::CycleSplit);
    assert_eq!(app.key_action(Action::CycleSplitBack), Action::CycleSplitBack);
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
        postui_core::fixtures::load_local_state(app.proj().root()).unwrap().main_split,
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
    assert_eq!(app.sidebar_width, Some(79));
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
