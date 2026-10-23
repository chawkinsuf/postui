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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
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

/// A resize press is not a click away: an open cell stays open.
#[test]
fn a_resize_press_leaves_an_open_cell_edit_open() {
    use crate::components::table_editor::{CellEdit, Col};
    for handle in [Hit::SidebarGutter, Hit::SplitHandle] {
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
        let (x, y) =
            if handle == Hit::SidebarGutter { gutter_cell(&app) } else { handle_cell(&app) };
        app.handle_mouse(left_down(x, y));
        assert!(app.resize.is_some(), "{handle:?}: a resize armed");
        assert!(app.editor.table.editing.is_some(), "{handle:?}: the cell is still open");
        assert_eq!(app.editor.table.selected, Some(0), "{handle:?}: and its row selected");
    }
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

/// Amendment (ruling T4-1): a click on the strip, a press and release
/// that never moved the line, focuses the response as it always has. A
/// drag that moved it leaves the focus where it was.
#[test]
fn a_click_on_the_strip_focuses_the_response() {
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    let before = app.split_state();
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    assert_eq!(app.focus, PaneId::Editor, "the press moves no focus");
    app.handle_mouse(left_up(x, y));
    assert_eq!(app.focus, PaneId::Response);
    assert_eq!(app.split_state(), before);

    app.update(Action::FocusPane(PaneId::Editor));
    render(&mut app);
    let (x, y) = handle_cell(&app);
    app.handle_mouse(left_down(x, y));
    app.handle_mouse(left_drag(x, y + 2));
    app.handle_mouse(left_up(x, y + 2));
    assert_ne!(app.split_state(), before, "the line moved");
    assert_eq!(app.focus, PaneId::Editor, "a drag keeps the focus");
}

/// A right click or focus loss is never a click on the strip: only the
/// release focuses the response.
#[test]
fn a_cancelled_strip_press_moves_no_focus() {
    for via_focus_loss in [false, true] {
        let mut app = App::new_for_test();
        app.update(Action::FocusPane(PaneId::Editor));
        render(&mut app);
        let (x, y) = handle_cell(&app);
        app.handle_mouse(left_down(x, y));
        if via_focus_loss {
            app.on_focus_lost();
        } else {
            let editor = rect(&app, &Hit::Pane(PaneId::Editor));
            app.handle_mouse(right_down(editor.x + 2, editor.y + 1));
        }
        assert!(app.resize.is_none());
        assert_eq!(app.focus, PaneId::Editor, "focus loss {via_focus_loss}");
    }
}
