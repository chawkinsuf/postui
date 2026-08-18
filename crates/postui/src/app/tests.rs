use super::*;
use ratatui::crossterm::event::KeyCode;

#[test]
fn resolve_startup_fresh_install_picks_default_dir_to_init_with_no_prompt() {
    let registry = crate::config::ProjectsRegistry::default();
    let default_dir = PathBuf::from("/nonexistent/postui-default-xyz");
    let (root, disposition, stale_last) =
        resolve_startup(&registry, None, Some(default_dir.clone())).unwrap();
    assert_eq!(root, default_dir);
    assert_eq!(disposition, StartupDisposition::InitDefault);
    assert_eq!(stale_last, None);
}

#[test]
fn resolve_startup_cli_non_project_root_prompts_create() {
    let dir = tempfile::tempdir().unwrap();
    let registry = crate::config::ProjectsRegistry::default();
    let (root, disposition, _) =
        resolve_startup(&registry, Some(dir.path().to_path_buf()), None).unwrap();
    assert_eq!(root, dir.path());
    assert_eq!(disposition, StartupDisposition::PromptCreate);
}

#[test]
fn resolve_startup_cli_existing_project_is_registered() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), None).unwrap();
    let registry = crate::config::ProjectsRegistry::default();
    let (root, disposition, _) =
        resolve_startup(&registry, Some(dir.path().to_path_buf()), None).unwrap();
    assert_eq!(root, dir.path());
    assert_eq!(disposition, StartupDisposition::OpenAsIs { register: true });
}

#[test]
fn resolve_startup_registry_last_wins_over_known() {
    let last_dir = tempfile::tempdir().unwrap();
    let registry = crate::config::ProjectsRegistry {
        known: vec![PathBuf::from("/a"), PathBuf::from("/b")],
        last: Some(last_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (root, disposition, stale_last) = resolve_startup(&registry, None, None).unwrap();
    assert_eq!(root, last_dir.path());
    assert_eq!(
        disposition,
        StartupDisposition::OpenAsIs { register: false }
    );
    assert_eq!(stale_last, None);
}

#[test]
fn resolve_startup_cli_beats_registry_last() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), None).unwrap();
    let registry = crate::config::ProjectsRegistry {
        last: Some(PathBuf::from("/elsewhere")),
        ..Default::default()
    };
    let (root, disposition, _) =
        resolve_startup(&registry, Some(dir.path().to_path_buf()), None).unwrap();
    assert_eq!(root, dir.path());
    assert_eq!(disposition, StartupDisposition::OpenAsIs { register: true });
}

#[test]
fn resolve_startup_uses_first_existing_known_when_no_last() {
    let dir_a = tempfile::tempdir().unwrap();
    let registry = crate::config::ProjectsRegistry {
        known: vec![PathBuf::from("/nonexistent-a"), dir_a.path().to_path_buf()],
        ..Default::default()
    };
    let (root, disposition, _) = resolve_startup(&registry, None, None).unwrap();
    assert_eq!(root, dir_a.path());
    assert_eq!(
        disposition,
        StartupDisposition::OpenAsIs { register: false }
    );
}

#[test]
fn resolve_startup_stale_last_is_skipped_in_favor_of_first_existing_known() {
    let dir_a = tempfile::tempdir().unwrap();
    let missing = PathBuf::from("/nonexistent-last-xyz");
    let registry = crate::config::ProjectsRegistry {
        known: vec![PathBuf::from("/nonexistent-a"), dir_a.path().to_path_buf()],
        last: Some(missing.clone()),
        ..Default::default()
    };
    let (root, disposition, stale_last) = resolve_startup(&registry, None, None).unwrap();
    assert_eq!(root, dir_a.path());
    assert_eq!(
        disposition,
        StartupDisposition::OpenAsIs { register: false }
    );
    assert_eq!(stale_last, Some(missing));
}

#[test]
fn resolve_startup_stale_last_falls_through_to_default_when_no_known() {
    let missing = PathBuf::from("/nonexistent-last-xyz");
    let default_dir = PathBuf::from("/nonexistent/postui-default-xyz");
    let registry = crate::config::ProjectsRegistry {
        last: Some(missing.clone()),
        ..Default::default()
    };
    let (root, disposition, stale_last) =
        resolve_startup(&registry, None, Some(default_dir.clone())).unwrap();
    assert_eq!(root, default_dir);
    assert_eq!(disposition, StartupDisposition::InitDefault);
    assert_eq!(stale_last, Some(missing));
}

#[test]
fn resolve_startup_returns_none_when_nothing_available() {
    let registry = crate::config::ProjectsRegistry::default();
    assert!(resolve_startup(&registry, None, None).is_none());
}

#[test]
fn init_project_here_creates_project_toml_at_current_root() {
    let mut app = App::new_for_test();
    assert!(!postui_core::project::is_project(&app.project.root));
    app.update(Action::InitProjectHere);
    assert!(postui_core::project::is_project(&app.project.root));
}

#[test]
fn quit_action_sets_should_quit() {
    let mut app = App::new_for_test();
    assert!(!app.should_quit);
    app.update(Action::Quit);
    assert!(app.should_quit);
}

#[test]
fn tick_does_not_quit() {
    let mut app = App::new_for_test();
    app.update(Action::Tick);
    assert!(!app.should_quit);
}

#[test]
fn focus_next_moves_focus() {
    let mut app = App::new_for_test();
    let start = app.focus;
    app.update(Action::FocusNext);
    assert_ne!(app.focus, start);
    app.update(Action::FocusPrev);
    assert_eq!(app.focus, start);
}

#[test]
fn close_pops_modal_instead_of_quitting() {
    use crate::components::modal::Modal;
    let mut app = App::new_for_test();
    app.modals.push(Modal::Message {
        title: "t".into(),
        body: "b".into(),
    });
    app.update(Action::Close);
    assert!(app.modals.is_empty());
    assert!(!app.should_quit);
}

#[test]
fn open_palette_pushes_modal() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    assert!(!app.modals.is_empty());
}

#[test]
fn running_a_palette_command_via_enter_records_usage() {
    let mut app = App::new_for_test();
    assert_eq!(app.usage.score("quit", crate::usage::now()), 0.0);
    app.update(Action::OpenPalette);
    for c in "quit".chars() {
        app.handle_key(&Keymap::default_bindings(), plain(c));
    }
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(app.usage.score("quit", crate::usage::now()) > 0.0);
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

#[test]
fn ctrl_c_quits_even_with_modal_open() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    app.handle_key(&Keymap::default_bindings(), ctrl('c'));
    assert!(app.should_quit);
}

#[test]
fn plain_q_types_into_palette_instead_of_quitting() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    app.handle_key(&Keymap::default_bindings(), plain('q'));
    assert!(!app.should_quit);
    assert!(!app.modals.is_empty());
}

#[test]
fn ctrl_char_does_not_type_into_palette() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    app.handle_key(&Keymap::default_bindings(), ctrl('x')); // unbound ctrl combo
    // palette input must still be empty: filter list unchanged
    let crate::components::modal::Modal::Palette(p) = app.modals.top().unwrap() else {
        panic!()
    };
    assert_eq!(p.input(), "");
}

#[test]
fn plain_q_quits_when_no_modal_and_component_ignores_it() {
    let mut app = App::new_for_test();
    app.handle_key(&Keymap::default_bindings(), plain('q'));
    assert!(app.should_quit);
}

#[test]
fn tick_requests_no_redraw_when_idle() {
    let mut app = App::new_for_test();
    assert!(!app.update(Action::Tick), "idle tick must not redraw");
}

#[test]
fn tick_requests_redraw_while_toast_visible() {
    let mut app = App::new_for_test();
    app.update(Action::ShowToast(
        "hi".into(),
        crate::components::toast::ToastKind::Info,
    ));
    assert!(app.update(Action::Tick));
}

#[test]
fn render_action_requests_redraw() {
    let mut app = App::new_for_test();
    assert!(app.update(Action::Render));
}

#[test]
fn scroll_dispatches_without_changing_focus() {
    let mut app = App::new_for_test();
    let before = app.focus;
    assert!(app.update(Action::ScrollPane(PaneId::Response, 3)));
    assert_eq!(app.focus, before, "scrolling must not steal focus");
}

fn left_down(x: u16, y: u16) -> ratatui::crossterm::event::MouseEvent {
    ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(
            ratatui::crossterm::event::MouseButton::Left,
        ),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn moved(x: u16, y: u16) -> ratatui::crossterm::event::MouseEvent {
    ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Moved,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn scroll_down(x: u16, y: u16) -> ratatui::crossterm::event::MouseEvent {
    ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::ScrollDown,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

/// Renders `app` once at 120x40 so `app.hits` (and any component state
/// that records its own draw area, like the body editor) is populated.
fn render_once(app: &mut App) {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
}

#[test]
fn deleting_a_table_row_by_key_requires_confirmation() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Content;
    app.editor.table.selected = Some(0);
    let keymap = Keymap::default_bindings();

    app.handle_key(&keymap, plain('d'));
    match app.modals.top() {
        Some(Modal::Confirm { body, .. }) => {
            assert!(body.contains("page"), "confirm names the key: {body}")
        }
        _ => panic!("expected a Confirm modal"),
    }
    assert_eq!(app.editor.params.len(), 1, "row survives until confirmed");

    app.handle_key(&keymap, plain('y'));
    assert!(app.editor.params.is_empty(), "confirming deletes the row");
    assert!(app.modals.top().is_none(), "modal closed after the choice");
}

#[test]
fn declining_the_table_row_delete_keeps_the_row() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Content;
    app.editor.table.selected = Some(0);
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('d'));
    app.handle_key(&keymap, plain('n'));
    assert_eq!(app.editor.params.len(), 1, "declining keeps the row");
    assert!(app.modals.top().is_none());
}

#[test]
fn clicking_the_row_delete_affordance_opens_the_confirm_modal() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    app.editor.table.selected = Some(0);
    render_once(&mut app);
    let del = app
        .hits
        .rect_of(&Hit::TableDelete(0))
        .expect("delete affordance on the selected row");
    assert!(app.handle_mouse(left_down(del.x, del.y)));
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "clicking ✕ must confirm, not delete outright"
    );
    assert_eq!(app.editor.params.len(), 1);
}

#[test]
fn clicking_the_selected_row_again_deselects_it() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    render_once(&mut app);
    let row = app.hits.rect_of(&Hit::TableRow(0)).unwrap();
    assert!(app.handle_mouse(left_down(row.x + 4, row.y)));
    assert_eq!(app.editor.table.selected, Some(0), "first click selects");

    render_once(&mut app);
    let row = app.hits.rect_of(&Hit::TableRow(0)).unwrap();
    // Well past double-click time, so this registers as a fresh click.
    std::thread::sleep(std::time::Duration::from_millis(450));
    assert!(app.handle_mouse(left_down(row.x + 4, row.y)));
    assert_eq!(
        app.editor.table.selected, None,
        "clicking the selected row again deselects it"
    );
}

#[test]
fn clicking_elsewhere_in_the_app_clears_the_table_selection() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    app.editor.table.selected = Some(0);
    render_once(&mut app);
    // Click into the sidebar pane background.
    let sidebar = app.hits.rect_of(&Hit::Pane(PaneId::Sidebar)).unwrap();
    app.handle_mouse(left_down(sidebar.x + 1, sidebar.y + sidebar.height - 2));
    assert_eq!(
        app.editor.table.selected, None,
        "clicking another pane clears the selection"
    );

    // Same for the URL bar.
    app.editor.table.selected = Some(0);
    render_once(&mut app);
    let url = app.editor.last_url_text_area.unwrap();
    app.handle_mouse(left_down(url.x + 1, url.y));
    assert_eq!(app.editor.table.selected, None);
}

#[test]
fn clicking_the_url_bar_focuses_the_url_line_and_places_the_caret() {
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://x/y");
    app.editor.sub_focus = SubFocus::Content;
    app.focus = PaneId::Sidebar;
    render_once(&mut app);

    let text_area = app.editor.last_url_text_area.expect("url text area");
    // Click 4 columns into the text: caret lands on char index 4.
    assert!(app.handle_mouse(left_down(text_area.x + 4, text_area.y)));
    assert_eq!(app.focus, PaneId::Editor, "click focuses the editor pane");
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
    assert_eq!(app.editor.url.cursor(), 4);

    // A click past the end of the text clamps the caret to the end.
    render_once(&mut app);
    let text_area = app.editor.last_url_text_area.unwrap();
    assert!(app.handle_mouse(left_down(text_area.x + text_area.width - 1, text_area.y)));
    assert_eq!(app.editor.url.cursor(), "https://x/y".chars().count());

    // The padding columns just left of the text still land in the URL
    // bar hit (not the method selector) and focus the line.
    app.editor.sub_focus = SubFocus::Content;
    render_once(&mut app);
    let text_area = app.editor.last_url_text_area.unwrap();
    assert!(app.handle_mouse(left_down(text_area.x - 1, text_area.y)));
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
    assert_eq!(
        app.editor.url.cursor(),
        0,
        "clicks left of the text go to char 0"
    );
}

/// Button-held motion, the event kind real terminals send for a drag
/// (as opposed to the synthetic `Moved` events used elsewhere in these
/// tests) — see the `handle_mouse` doc comment on why both drive drags.
fn dragged(x: u16, y: u16) -> ratatui::crossterm::event::MouseEvent {
    ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Drag(
            ratatui::crossterm::event::MouseButton::Left,
        ),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn left_up(x: u16, y: u16) -> ratatui::crossterm::event::MouseEvent {
    ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Up(
            ratatui::crossterm::event::MouseButton::Left,
        ),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn dragging_the_sidebar_thumb_scrolls_and_release_ends_the_drag() {
    use crate::hit::{Hit, offset_for_thumb_top};
    let mut app = App::new_for_test();
    let slugs: Vec<postui_core::storage::RequestListing> = (0..60)
        .map(|i| postui_core::storage::RequestListing {
            slug: format!("r{i:02}"),
            broken: None,
            method: Some(postui_core::model::Method::Get),
        })
        .collect();
    app.sidebar.refresh(slugs, &Default::default());
    render_once(&mut app);

    let thumb = app
        .hits
        .rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar))
        .expect("sidebar thumb");
    let track = app.hits.track_of(PaneId::Sidebar).expect("sidebar track");
    let spec = app
        .scrollbar_spec(PaneId::Sidebar)
        .expect("sidebar scrollbar spec");
    assert_eq!(app.sidebar.scroll, 0);

    assert!(app.handle_mouse(left_down(thumb.x, thumb.y)));
    assert!(app.drag.is_some(), "pressing the thumb starts a drag");

    assert!(app.handle_mouse(moved(thumb.x, thumb.y + 3)));
    let after = app.sidebar.scroll;
    assert_eq!(
        after,
        offset_for_thumb_top(&spec, track.height, 3),
        "drag maps the thumb's new top back to a content offset"
    );
    assert!(after > 0);
    assert!(
        !app.sidebar.ensure_visible,
        "a free-scroll drag must not snap back to the selection"
    );

    app.handle_mouse(left_up(thumb.x, thumb.y + 3));
    assert!(app.drag.is_none());
    app.handle_mouse(moved(thumb.x, thumb.y + 6));
    assert_eq!(
        app.sidebar.scroll, after,
        "motion after release no longer scrolls"
    );
}

#[test]
fn dragging_the_sidebar_thumb_with_drag_events_scrolls_the_same_as_moved() {
    // Real terminals report button-held motion as `Drag(Left)`, not
    // `Moved` — the prior test only drove `Moved`. Same scenario, same
    // assertions, `Drag(Left)` motion instead.
    use crate::hit::{Hit, offset_for_thumb_top};
    let mut app = App::new_for_test();
    let slugs: Vec<postui_core::storage::RequestListing> = (0..60)
        .map(|i| postui_core::storage::RequestListing {
            slug: format!("r{i:02}"),
            broken: None,
            method: Some(postui_core::model::Method::Get),
        })
        .collect();
    app.sidebar.refresh(slugs, &Default::default());
    render_once(&mut app);

    let thumb = app
        .hits
        .rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar))
        .expect("sidebar thumb");
    let track = app.hits.track_of(PaneId::Sidebar).expect("sidebar track");
    let spec = app
        .scrollbar_spec(PaneId::Sidebar)
        .expect("sidebar scrollbar spec");
    assert_eq!(app.sidebar.scroll, 0);

    assert!(app.handle_mouse(left_down(thumb.x, thumb.y)));
    assert!(app.drag.is_some(), "pressing the thumb starts a drag");

    assert!(app.handle_mouse(dragged(thumb.x, thumb.y + 3)));
    let after = app.sidebar.scroll;
    assert_eq!(
        after,
        offset_for_thumb_top(&spec, track.height, 3),
        "Drag(Left) motion maps the thumb's new top back to a content offset"
    );
    assert!(after > 0);
    assert!(
        !app.sidebar.ensure_visible,
        "a free-scroll drag must not snap back to the selection"
    );

    app.handle_mouse(left_up(thumb.x, thumb.y + 3));
    assert!(app.drag.is_none());
    app.handle_mouse(dragged(thumb.x, thumb.y + 6));
    assert_eq!(
        app.sidebar.scroll, after,
        "motion after release no longer scrolls"
    );
}

#[test]
fn scrollbar_track_click_below_the_thumb_pages_the_response() {
    use crate::hit::Hit;
    let mut app = App::new_for_test();
    let body = (0..200)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.session
        .response
        .set_state(ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![],
            size: body.len(),
            body,
            elapsed: std::time::Duration::from_millis(1),
            content_type: Some("text/plain".into()),
        })));
    render_once(&mut app);

    let track = app.hits.track_of(PaneId::Response).expect("response track");
    let spec = app
        .scrollbar_spec(PaneId::Response)
        .expect("response scrollbar spec");
    assert_eq!(app.session.response.view().unwrap().scroll, 0);

    let below = track.y + track.height - 1;
    assert_eq!(
        app.hits.hit_at(track.x, below),
        Some(&Hit::ScrollbarTrack(PaneId::Response, spec.viewport as i16)),
        "the track under the thumb pages forward by a viewport"
    );
    assert!(app.handle_mouse(left_down(track.x, below)));
    assert_eq!(
        app.session.response.view().unwrap().scroll,
        (spec.viewport as i16).min(30) as usize,
        "a track click pages by a viewport (clamped)"
    );
}

#[test]
fn click_on_pane_hit_focuses_that_pane() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::Pane(PaneId::Response))
        .unwrap();
    app.handle_mouse(left_down(r.x + 2, r.y + 2));
    assert_eq!(app.focus, PaneId::Response);
}

#[test]
fn header_buffer_shows_dropdown_glyph_for_project_and_env() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    assert!(app.hits.rect_of(&crate::hit::Hit::HeaderProject).is_some());
    assert!(app.hits.rect_of(&crate::hit::Hit::HeaderEnv).is_some());
}

#[test]
fn click_header_env_opens_env_chooser() {
    // `App::new_for_test()`'s project has no environments configured, so
    // firing `OpenEnvChooser` toasts the "no environments" warning
    // rather than opening a chooser — proof enough that the click
    // dispatched the action.
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::HeaderEnv).unwrap();
    assert!(app.toasts.is_empty());
    app.handle_mouse(left_down(r.x, r.y));
    assert!(
        !app.toasts.is_empty(),
        "clicking the env name should fire OpenEnvChooser"
    );
}

#[test]
fn click_footer_palette_chip_opens_palette() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::FooterChip(Action::OpenPalette))
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
}

#[test]
fn hover_change_requests_redraw_and_same_hover_does_not() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::Pane(PaneId::Sidebar))
        .unwrap();
    assert!(
        app.handle_mouse(moved(r.x + 1, r.y + 1)),
        "first hover redraws"
    );
    assert!(
        !app.handle_mouse(moved(r.x + 1, r.y + 2)),
        "same hit: no redraw"
    );
}

#[test]
fn wheel_over_pane_routes_via_pane_at_to_scroll_pane() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::Pane(PaneId::Sidebar))
        .unwrap();
    let before = app.focus;
    assert!(app.handle_mouse(scroll_down(r.x + 1, r.y + 1)));
    assert_eq!(app.focus, before, "wheel must not steal focus");
}

#[test]
fn wheel_over_body_editor_forwards_to_the_editor() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello\nworld");
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");
    assert!(app.handle_mouse(scroll_down(area.x + 2, area.y + 1)));
}

#[test]
fn wheel_over_body_editor_with_modal_open_is_a_no_op() {
    // Regression test: the modal-open guard must be checked before the
    // Hit::BodyEditor short-circuit in the ScrollUp/ScrollDown arm, or a
    // wheel event over the editor body still reaches
    // `editor.handle_mouse` while a modal is open.
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello\nworld");
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");
    app.modals.push(crate::components::modal::Modal::Message {
        title: "About".into(),
        body: "hello".into(),
    });
    assert!(!app.handle_mouse(scroll_down(area.x + 2, area.y + 1)));
}

#[test]
fn click_in_body_area_places_cursor_and_focuses_content() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello\nworld");
    // render once so the view records its area
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");
    app.handle_mouse(left_down(area.x + 4, area.y + 1));
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
    assert_eq!(app.focus, PaneId::Editor);
    assert_eq!(app.editor.body.cursor.row, 1, "clicked the second line");
}

fn req(url: &str) -> postui_core::model::HttpRequest {
    postui_core::model::HttpRequest::from_toml_str(&format!(r#"url = "{url}""#)).unwrap()
}

#[test]
fn sidebar_lists_requests_grouped_and_enter_opens() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "auth/login", &req("https://x/login")).unwrap();
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    assert_eq!(
        app.sidebar.rows,
        vec![
            Row::Request {
                slug: "ping".into(),
                depth: 0,
                broken: None,
                method: Some(postui_core::model::Method::Get),
            },
            Row::Folder {
                path: "auth".into(),
                name: "auth".into(),
                depth: 0,
                expanded: false,
            },
        ]
    );

    // Nothing selected at startup: the first j lands on "ping" (index
    // 0), the second reaches the "auth" folder (index 1); Enter expands
    // it, then "auth/login" (index 2) becomes visible and Enter opens it.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('j'));
    app.handle_key(&keymap, plain('j'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(&keymap, plain('j'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.slug.as_deref(), Some("auth/login"));
}

#[test]
fn startup_restores_persisted_open_request() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: None,
            open_request: Some("ping".into()),
            expanded: vec![],
            ..Default::default()
        },
    )
    .unwrap();

    let app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("ping"),
        "the persisted open request loads into the editor at startup, \
         same as it does on a project switch"
    );
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("ping"));
}

#[test]
fn startup_restores_open_request_inside_a_collapsed_folder() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "auth/login", &req("https://x/l")).unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: None,
            open_request: Some("auth/login".into()),
            expanded: vec![],
            ..Default::default()
        },
    )
    .unwrap();

    let app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.editor.slug.as_deref(), Some("auth/login"));
    assert_eq!(
        app.sidebar.selected_slug().as_deref(),
        Some("auth/login"),
        "restoring expands the request's ancestor folders so the \
         selected row is actually visible"
    );
}

#[test]
fn startup_without_persisted_open_request_selects_nothing() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();

    let app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.editor.slug, None);
    assert_eq!(
        app.sidebar.selected, None,
        "no row wears the selected fill when nothing is open — a \
         highlighted row with an empty editor misstates what's loaded"
    );
}

#[test]
fn force_open_request_selects_its_sidebar_row() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "auth/login", &req("https://x/l")).unwrap();
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    // Opened by an out-of-band route (palette, dirty-gate confirm, …)
    // rather than a sidebar click: the sidebar must follow, expanding
    // ancestors as needed, so selection and open request can't diverge.
    app.update(Action::ForceOpenRequest("auth/login".into()));
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("auth/login"));
}

#[test]
fn opening_another_request_swaps_the_response_panel() {
    use crate::components::response::ResponseState;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "a", &req("https://x/a")).unwrap();
    postui_core::storage::save_request(dir.path(), "b", &req("https://x/b")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::ForceOpenRequest("a".into()));
    app.session
        .response
        .set_state(ResponseState::Failed("a's result".into()));

    app.update(Action::ForceOpenRequest("b".into()));
    assert!(
        matches!(app.session.response.state(), ResponseState::Empty),
        "b never sent anything; showing a's response would mislabel it"
    );

    app.update(Action::ForceOpenRequest("a".into()));
    assert!(
        matches!(app.session.response.state(), ResponseState::Failed(e) if e == "a's result"),
        "a's response comes back from the cache"
    );
}

#[test]
fn opening_over_dirty_editor_prompts_save_discard_cancel() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "a", &req("https://x/a")).unwrap();
    postui_core::storage::save_request(dir.path(), "b", &req("https://x/b")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    // Open "a", then edit its URL so the editor becomes dirty.
    app.update(Action::ForceOpenRequest("a".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('/'));
    assert!(app.editor.is_dirty());

    // Requesting to open "b" while dirty must prompt instead of opening.
    app.update(Action::OpenRequest("b".into()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("a"),
        "still on the original request"
    );

    // 'd' discards the edit and opens "b".
    app.handle_key(&keymap, plain('d'));
    assert_eq!(app.editor.slug.as_deref(), Some("b"));
    assert!(!app.editor.is_dirty());

    // Back to "a", dirty it again, this time choose 's' to save & open.
    let mut app = App::with_root(app.tx.clone(), dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("a".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('/'));
    assert!(app.editor.is_dirty());
    app.update(Action::OpenRequest("b".into()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&keymap, plain('s'));
    assert_eq!(app.editor.slug.as_deref(), Some("b"));
    let saved = postui_core::storage::load_request(dir.path(), "a").unwrap();
    assert_eq!(
        saved.url, "https://x/a/",
        "the edit was persisted before opening b"
    );
}

fn sidebar_test_app() -> (App, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "api/ping", &req("https://x/ping")).unwrap();
    postui_core::storage::save_request(dir.path(), "top", &req("https://x/top")).unwrap();
    let app = App::with_root(tx, dir.path().to_path_buf());
    (app, dir)
}

#[test]
fn click_sidebar_row_opens_that_request() {
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);
    assert_eq!(
        app.sidebar.rows[0],
        Row::Request {
            slug: "top".into(),
            depth: 0,
            broken: None,
            method: Some(postui_core::model::Method::Get),
        }
    );
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(0)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.editor.slug.as_deref(), Some("top"));
}

#[test]
fn click_folder_arrow_expands_the_folder() {
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);
    assert!(matches!(app.sidebar.rows[1], Row::Folder { .. }));
    let before = app.sidebar.rows.len();
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::SidebarFolderArrow(1))
        .expect("folder arrow hit registered");
    app.handle_mouse(left_down(r.x, r.y));
    assert!(
        app.sidebar.rows.len() > before,
        "expanding the folder reveals its child row"
    );
}

#[test]
fn single_click_folder_name_selects_only_double_click_expands() {
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);
    let before = app.sidebar.rows.len();
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(1)).unwrap();

    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(
        app.sidebar.selected,
        Some(1),
        "single click selects the folder"
    );
    assert_eq!(
        app.sidebar.rows.len(),
        before,
        "single click must not expand the folder"
    );

    // Second Down on the same hit within 400ms is a double click.
    app.handle_mouse(left_down(r.x, r.y));
    assert!(
        app.sidebar.rows.len() > before,
        "double click expands the folder"
    );
}

#[test]
fn triple_click_toggles_the_folder_exactly_once() {
    // Regression: `last_click` used to survive a double, so a third
    // click within the 400ms window paired with the second and counted
    // as another double — a fast triple-click toggled the folder twice
    // (expand then immediately collapse again), netting no change.
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);
    let before = app.sidebar.rows.len();
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(1)).unwrap();

    app.handle_mouse(left_down(r.x, r.y)); // 1st: select
    app.handle_mouse(left_down(r.x, r.y)); // 2nd: double -> expand
    assert!(app.sidebar.rows.len() > before, "double click expands");
    let expanded = app.sidebar.rows.len();

    app.handle_mouse(left_down(r.x, r.y)); // 3rd: fresh single, not another double
    assert_eq!(
        app.sidebar.rows.len(),
        expanded,
        "a third rapid click must not re-toggle the folder"
    );
}

#[test]
fn click_new_request_button_opens_prompt_modal() {
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::SidebarNewRequest)
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::NewRequest,
            ..
        })
    ));
}

#[test]
fn clicking_a_prompts_own_body_does_not_close_it_or_touch_the_input() {
    // Regression for the merge blocker: `ModalOutside` used to cover
    // the whole screen with nothing swallowing clicks on the modal's
    // own box, so clicking the input line (or any other point inside
    // the border) resolved to `ModalOutside` and closed the modal,
    // discarding typed input.
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::SidebarNewRequest)
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })));

    let keymap = Keymap::default_bindings();
    for c in "ping".chars() {
        app.handle_key(&keymap, plain(c));
    }
    render_once(&mut app);

    let body = app.hits.rect_of(&crate::hit::Hit::ModalBody).unwrap();
    let inside = (body.x + body.width / 2, body.y + body.height / 2);
    app.handle_mouse(left_down(inside.0, inside.1));

    assert!(
        matches!(
            app.modals.top(),
            Some(Modal::Prompt {
                kind: PromptKind::NewRequest,
                ..
            })
        ),
        "clicking the modal's own chrome must not close it"
    );
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        unreachable!()
    };
    assert_eq!(input.text(), "ping", "typed input must be untouched");
}

#[test]
fn clicking_another_row_over_dirty_editor_is_gated_by_confirm() {
    let (mut app, _dir) = sidebar_test_app();
    app.project.expanded.insert("api".into());
    app.refresh_sidebar();
    let keymap = Keymap::default_bindings();
    app.update(Action::ForceOpenRequest("top".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('/'));
    assert!(app.editor.is_dirty());

    render_once(&mut app);
    assert_eq!(
        app.sidebar.rows[2],
        Row::Request {
            slug: "api/ping".into(),
            depth: 1,
            broken: None,
            method: Some(postui_core::model::Method::Get),
        },
        "folder pre-expanded so api/ping is the third row"
    );
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(2)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "clicking a different request row while dirty must gate through the Confirm modal, not open silently"
    );
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("top"),
        "editor content unchanged until the modal is resolved"
    );
}

#[test]
fn broken_file_shows_marker_and_error_modal() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    std::fs::write(
        dir.path().join("requests/bad.toml"),
        "url = \"x\"\nurl = \"dup\"\n",
    )
    .unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    let Row::Request { broken, .. } = &app.sidebar.rows[0] else {
        panic!("expected a request row")
    };
    assert!(broken.is_some());

    // Nothing starts selected; the first Down puts the cursor on row 0.
    app.handle_key(&Keymap::default_bindings(), plain('j'));
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    match app.modals.top() {
        Some(Modal::Message { body, .. }) => {
            assert!(body.contains('2') || body.to_lowercase().contains("duplicate"));
        }
        _ => panic!("expected a Message modal"),
    }
}

#[test]
fn dirty_dot_renders_in_sidebar() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "a", &req("https://x/a")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("a".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    assert!(app.editor.is_dirty());

    let backend = TestBackend::new(60, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let content = format!("{:?}", terminal.backend().buffer());
    assert!(
        content.contains('\u{25cf}'),
        "expected a dirty dot in the sidebar: {content}"
    );
}

#[test]
fn new_request_prompt_flow_creates_file_and_opens_it() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Sidebar;
    app.handle_key(&keymap, plain('n'));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::NewRequest,
            ..
        })
    ));
    for c in "api/ping".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.slug.as_deref(), Some("api/ping"));
    assert!(postui_core::storage::load_request(&app.project.root, "api/ping").is_ok());
    assert!(
        app.sidebar
            .rows
            .iter()
            .any(|r| matches!(r, Row::Request { slug, .. } if slug == "api/ping")),
        "sidebar should list the new request: {:?}",
        app.sidebar.rows
    );
}

#[test]
fn new_request_invalid_name_toasts_and_creates_nothing() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "Bad Name".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.modals.is_empty(),
        "modal closes even though the save is rejected"
    );
    assert!(!app.toasts.is_empty(), "an invalid name must toast");
    assert!(
        postui_core::storage::list_requests(&app.project.root)
            .0
            .is_empty()
    );
}

#[test]
fn new_request_duplicate_name_toasts_and_leaves_existing_file_alone() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "api/ping", &req("https://x/existing"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "api/ping".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.modals.is_empty(),
        "modal closes even though the save is rejected"
    );
    assert!(!app.toasts.is_empty(), "a duplicate name must toast");
    let existing = postui_core::storage::load_request(&app.project.root, "api/ping").unwrap();
    assert_eq!(
        existing.url, "https://x/existing",
        "existing file must not be overwritten"
    );
}

#[test]
fn rename_request_updates_disk_and_open_slug() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "old", &req("https://x/old")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("old".into()));
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Sidebar;
    app.handle_key(&keymap, plain('r'));
    match app.modals.top() {
        Some(Modal::Prompt {
            kind: PromptKind::RenameRequest { from },
            ..
        }) => {
            assert_eq!(from, "old");
        }
        _ => panic!("expected a RenameRequest prompt"),
    }
    for _ in 0.."old".len() {
        app.handle_key(
            &keymap,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
    }
    for c in "new".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert!(postui_core::storage::load_request(&app.project.root, "old").is_err());
    assert!(postui_core::storage::load_request(&app.project.root, "new").is_ok());
    assert_eq!(app.editor.slug.as_deref(), Some("new"));
    assert_eq!(app.sidebar.open_slug.as_deref(), Some("new"));
}

#[test]
fn delete_open_request_clears_editor_and_removes_file() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "gone", &req("https://x/gone")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("gone".into()));
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Sidebar;
    app.handle_key(&keymap, plain('d'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&keymap, plain('y'));
    assert!(app.modals.is_empty());
    assert!(
        app.editor.slug.is_none(),
        "editor must reset once its open request is deleted"
    );
    assert!(postui_core::storage::load_request(&app.project.root, "gone").is_err());
}

#[test]
fn save_with_no_slug_opens_save_as_prompt() {
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://x/new");
    let keymap = Keymap::default_bindings();
    app.update(Action::SaveRequest);
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::SaveAs,
            ..
        })
    ));
    for c in "fresh".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.slug.as_deref(), Some("fresh"));
    let saved = postui_core::storage::load_request(&app.project.root, "fresh").unwrap();
    assert_eq!(saved.url, "https://x/new");
}

#[test]
fn rename_and_delete_on_empty_sidebar_do_nothing() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Sidebar;
    app.handle_key(&keymap, plain('r'));
    assert!(app.modals.is_empty());
    app.handle_key(&keymap, plain('d'));
    assert!(app.modals.is_empty());
}

#[tokio::test]
async fn send_with_invalid_body_prompts_first() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9"); // unroutable, never actually hit
    app.editor.set_body_text("{oops");
    app.update(Action::Send);
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert!(app.session.in_flight.is_none());
}

#[tokio::test]
async fn stale_generation_results_are_ignored() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.session.send_generation = 5;
    app.update(Action::RequestFailed {
        generation: 4,
        error: "old".into(),
    });
    assert!(
        matches!(app.session.response.state(), ResponseState::Empty),
        "stale result dropped"
    );
}

#[tokio::test]
async fn empty_url_toasts_instead_of_sending() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.update(Action::Send);
    assert!(app.session.in_flight.is_none());
    assert!(
        !app.toasts.is_empty(),
        "empty URL must toast rather than send"
    );
}

#[tokio::test]
async fn force_send_with_empty_url_toasts_and_does_not_spawn() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.update(Action::ForceSend);
    assert!(
        app.session.in_flight.is_none(),
        "no task should be spawned for an empty URL"
    );
    assert!(
        !app.toasts.is_empty(),
        "empty URL must toast even via ForceSend directly"
    );
    assert_eq!(
        app.session.send_generation, 0,
        "generation must not advance without a send"
    );
}

#[tokio::test]
async fn force_send_spawns_a_task_and_marks_response_in_flight() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9"); // unroutable, never actually hit
    app.update(Action::ForceSend);
    assert!(app.session.in_flight.is_some());
    assert!(matches!(
        app.session.response.state(),
        ResponseState::InFlight { .. }
    ));
    assert_eq!(app.session.send_generation, 1);
}

#[tokio::test]
async fn cancel_send_aborts_task_and_marks_cancelled() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::ForceSend);
    assert!(app.session.in_flight.is_some());
    app.update(Action::CancelSend);
    assert!(app.session.in_flight.is_none());
    assert!(matches!(
        app.session.response.state(),
        ResponseState::Cancelled
    ));
    // no-op when nothing is in flight
    assert!(!app.update(Action::CancelSend));
}

#[tokio::test]
async fn cancelled_send_ignores_a_result_that_was_already_queued() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::ForceSend);
    let generation = app.session.send_generation;
    app.update(Action::CancelSend);
    assert!(matches!(
        app.session.response.state(),
        ResponseState::Cancelled
    ));

    // Simulate the in-flight task's result landing after cancellation,
    // still tagged with the generation it was spawned under.
    let data = crate::http::ResponseData {
        status: 200,
        headers: vec![],
        body: "late".into(),
        elapsed: std::time::Duration::from_millis(1),
        size: 4,
        content_type: None,
    };
    app.update(Action::ResponseArrived {
        generation,
        data: Box::new(data),
    });
    assert!(
        matches!(app.session.response.state(), ResponseState::Cancelled),
        "a result racing the cancel must not overwrite it"
    );
}

#[tokio::test]
async fn response_arrived_with_current_generation_clears_in_flight() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.session.send_generation = 1;
    let data = crate::http::ResponseData {
        status: 200,
        headers: vec![],
        body: "ok".into(),
        elapsed: std::time::Duration::from_millis(1),
        size: 2,
        content_type: None,
    };
    app.update(Action::ResponseArrived {
        generation: 1,
        data: Box::new(data.clone()),
    });
    assert!(app.session.in_flight.is_none());
    assert!(matches!(app.session.response.state(), ResponseState::Ready(d) if **d == data));
}

#[test]
fn esc_on_in_flight_response_pane_requests_cancel() {
    let mut app = App::new_for_test();
    app.session.response.set_state(ResponseState::InFlight {
        started: std::time::Instant::now(),
    });
    let action = app
        .session
        .response
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(action, Some(Action::CancelSend));
}

#[test]
fn plain_keys_reach_the_focused_response_pane() {
    let mut app = App::new_for_test();
    app.session
        .response
        .set_state(ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![],
            body: r#"{"a": 1}"#.into(),
            elapsed: std::time::Duration::from_millis(5),
            size: 8,
            content_type: None,
        })));
    app.focus = PaneId::Response;
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('j'));
    assert_eq!(
        app.session.response.view().unwrap().cursor,
        1,
        "j moved the response cursor"
    );
    // 'q' quits globally, but the pane's search input takes it first.
    app.handle_key(&keymap, plain('/'));
    app.handle_key(&keymap, plain('q'));
    assert!(
        !app.should_quit,
        "a key the pane consumed must not fall through"
    );
    assert_eq!(
        app.session
            .response
            .view()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .input
            .text(),
        "q"
    );
}

#[test]
fn esc_on_idle_response_pane_does_nothing() {
    let mut app = App::new_for_test();
    let action = app
        .session
        .response
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(action, None);
}

fn two_projects() -> (App, tempfile::TempDir, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    postui_core::project::init_project(a.path(), Some("alpha")).unwrap();
    postui_core::project::init_project(b.path(), Some("beta")).unwrap();
    postui_core::storage::ensure_project(b.path()).unwrap();
    postui_core::storage::save_request(b.path(), "pong", &req("https://x/pong")).unwrap();
    let mut app = App::with_root(tx, a.path().to_path_buf());
    app.registry.register(a.path().to_path_buf());
    app.registry.register(b.path().to_path_buf());
    (app, a, b)
}

/// Renders the app and returns the terminal buffer's debug text, so
/// tests can assert on toast wording (`Toasts` exposes no message
/// accessor beyond `is_empty`).
fn rendered_text(app: &mut App) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

#[test]
fn cycle_switches_to_next_project_and_lists_its_requests() {
    let (mut app, _a, b) = two_projects();
    app.update(Action::CycleProject);
    assert_eq!(app.project.root, b.path());
    assert!(
        app.sidebar
            .rows
            .iter()
            .any(|r| matches!(r, Row::Request { slug, .. } if slug == "pong"))
    );
    assert_eq!(app.project.display_name(), "beta");
    assert!(
        rendered_text(&mut app).contains("Switched to beta"),
        "a clean cycle must confirm the switch with a toast"
    );
}

#[test]
fn cycle_with_dirty_editor_shows_no_switch_toast_until_discard() {
    let (mut app, _a, b) = two_projects();
    postui_core::storage::save_request(&app.project.root, "r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("r".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    assert!(app.editor.is_dirty());

    app.update(Action::CycleProject);
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert_ne!(app.project.root, b.path(), "not switched yet");
    assert!(
        !rendered_text(&mut app).contains("Switched to"),
        "no switch toast before the dirty gate is resolved"
    );

    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert_eq!(app.project.root, b.path());
    assert!(
        rendered_text(&mut app).contains("Switched to beta"),
        "the switch toast appears once the discard actually switches"
    );
}

#[test]
fn switch_with_dirty_editor_prompts_and_discard_proceeds() {
    let (mut app, _a, b) = two_projects();
    postui_core::storage::save_request(&app.project.root, "r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("r".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    assert!(app.editor.is_dirty());
    app.update(Action::SwitchProject(b.path().to_path_buf()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert_ne!(app.project.root, b.path(), "not switched yet");
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert_eq!(app.project.root, b.path());
}

#[test]
fn switch_restores_target_projects_open_request_and_saves_state() {
    let (mut app, a, b) = two_projects();
    postui_core::project::save_local_state(
        b.path(),
        &postui_core::project::LocalState {
            open_request: Some("pong".into()),
            ..Default::default()
        },
    )
    .unwrap();
    app.update(Action::SwitchProject(b.path().to_path_buf()));
    assert_eq!(app.editor.slug.as_deref(), Some("pong"));
    // and the old project's state got written on the way out
    let old = postui_core::project::load_local_state(a.path()).unwrap();
    assert_eq!(old.open_request, None);
}

#[test]
fn project_chooser_lists_known_and_open_by_path_creates() {
    let (mut app, _a, _b) = two_projects();
    app.update(Action::OpenProjectChooser);
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        panic!("expected chooser")
    };
    assert!(
        format!("{:?}", (c.input(), c.selected_label())).contains("alpha")
            || c.selected_label().is_some()
    );
    app.update(Action::Close);
    let fresh = tempfile::tempdir().unwrap();
    let target = fresh.path().join("newproj");
    app.update(Action::OpenProjectByPath(
        target.to_string_lossy().into_owned(),
    ));
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "non-project path asks to create"
    );
    app.handle_key(&Keymap::default_bindings(), plain('y'));
    assert!(postui_core::project::is_project(&target));
    assert_eq!(app.project.root, target);
}

#[test]
fn new_project_modal_prefills_path_from_name_and_creates() {
    let mut app = App::new_for_test();
    let root = tempfile::tempdir().unwrap();
    app.registry.root = Some(root.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewProject);
    for c in "My Svc".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let Some(Modal::NewProject { path, .. }) = app.modals.top() else {
        panic!()
    };
    assert!(
        path.text().ends_with("/my-svc"),
        "slugified prefill: {}",
        path.text()
    );
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let expected = root.path().join("my-svc");
    assert!(postui_core::project::is_project(&expected));
    assert_eq!(app.project.root, expected);
    assert_eq!(app.project.display_name(), "My Svc");
    assert!(app.registry.known.contains(&expected));
}

#[test]
fn create_project_with_dirty_editor_defers_last_until_dirty_gate_resolves() {
    let (mut app, dir, _b) = two_projects();
    // Dirty the editor on the current (old) project.
    postui_core::storage::save_request(&app.project.root, "r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("r".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    assert!(app.editor.is_dirty());

    let old_last = app.registry.last.clone();
    let fresh = tempfile::tempdir().unwrap();
    let new_path = fresh.path().join("newproj");
    app.update(Action::CreateProject {
        name: "New Proj".into(),
        path: new_path.to_string_lossy().into_owned(),
    });
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert_eq!(
        app.registry.last, old_last,
        "last must not change before the dirty gate resolves"
    );
    assert!(
        app.registry.known.contains(&new_path),
        "new path is known even though not yet current"
    );
    assert_eq!(app.project.root, dir.path(), "not switched yet");

    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert_eq!(app.project.root, new_path);
    assert_eq!(app.registry.last, Some(new_path));
}

#[test]
fn cycle_env_reloads_project_files_before_switching() {
    let (mut app, dir) = app_with_envs();
    // Rewrite variables.toml on disk with a bumped mtime so
    // reload_if_changed picks it up.
    std::fs::write(
        dir.path().join("variables.toml"),
        "[greeting]\ndefault = \"hi\"\n",
    )
    .unwrap();
    let t = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let f = std::fs::File::options()
        .append(true)
        .open(dir.path().join("variables.toml"))
        .unwrap();
    f.set_modified(t).unwrap();

    app.update(Action::CycleEnv);
    assert_eq!(
        app.project.model.vars["greeting"].default.as_deref(),
        Some("hi"),
        "CycleEnv must reload project files (spec sec7 symmetry with OpenEnvChooser)"
    );
}

#[test]
fn force_open_request_persists_open_request_to_local_state() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "a", &req("https://x/a")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("a".into()));
    let st = postui_core::project::load_local_state(&app.project.root).unwrap();
    assert_eq!(st.open_request.as_deref(), Some("a"));
}

#[test]
fn switch_env_failure_shows_warning_without_stale_success_toast() {
    let (mut app, dir) = app_with_envs();
    std::fs::write(dir.path().join("environments/broken.toml"), "not toml [").unwrap();
    app.update(Action::SwitchEnv(Some("broken".into())));
    let text = rendered_text(&mut app);
    assert!(
        text.contains("could not load environment"),
        "warning shown: {text}"
    );
    assert!(
        !text.contains("env:"),
        "no stale success toast on failure: {text}"
    );
}

#[test]
fn new_project_empty_name_swallows_enter_and_esc_cancels() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewProject);
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.modals.is_empty(), "empty name: modal stays");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
}

#[test]
fn new_project_tab_prefill_noop_when_slugify_is_empty() {
    let mut app = App::new_for_test();
    let root = tempfile::tempdir().unwrap();
    app.registry.root = Some(root.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewProject);
    for c in "日本語".chars() {
        app.handle_key(&keymap, plain(c));
    }
    let before = {
        let Some(Modal::NewProject { path, .. }) = app.modals.top() else {
            panic!()
        };
        path.text().to_string()
    };
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let Some(Modal::NewProject { path, .. }) = app.modals.top() else {
        panic!()
    };
    assert_eq!(
        path.text(),
        before,
        "empty slugify must not append to the path prefill"
    );
}

#[test]
fn create_project_with_empty_path_toasts_and_creates_nothing() {
    let mut app = App::new_for_test();
    let before_root = app.project.root.clone();
    let before_known = app.registry.known.clone();
    app.update(Action::CreateProject {
        name: "x".into(),
        path: "".into(),
    });
    let text = rendered_text(&mut app);
    assert!(
        text.contains("project path is empty — enter a path"),
        "error toast shown: {text}"
    );
    assert_eq!(app.project.root, before_root, "no project switch");
    assert_eq!(app.registry.known, before_known, "no project registered");
}

#[test]
fn stale_table_edit_does_not_capture_insert_var_text_after_focus_moves() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Params;
    app.editor.sub_focus = SubFocus::Content;
    app.editor.table.selected = Some(0);
    app.editor
        .table
        .begin_edit_selected(&app.editor.params.clone());
    let pending_before = app
        .editor
        .table
        .editing
        .as_ref()
        .unwrap()
        .input
        .text()
        .to_string();

    app.update(Action::FocusPane(PaneId::Response));
    app.update(Action::InsertVarText("x".into()));

    let text = rendered_text(&mut app);
    assert!(
        text.contains("nowhere to insert"),
        "toast shown when focus has moved off the table: {text}"
    );
    assert_eq!(
        app.editor.table.editing.as_ref().unwrap().input.text(),
        pending_before,
        "stale pending edit input must be unchanged"
    );
}

fn app_with_envs() -> (App, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    std::fs::write(dir.path().join("environments/prod.toml"), "tok = \"p\"\n").unwrap();
    std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"q\"\n").unwrap();
    (App::with_root(tx, dir.path().to_path_buf()), dir)
}

#[test]
fn cycle_env_wraps_and_skips_no_env() {
    let (mut app, dir) = app_with_envs();
    assert_eq!(app.project.env_label(), "no env");
    app.update(Action::CycleEnv);
    assert_eq!(app.project.env_label(), "prod");
    app.update(Action::CycleEnv);
    assert_eq!(app.project.env_label(), "qa");
    app.update(Action::CycleEnv);
    assert_eq!(
        app.project.env_label(),
        "prod",
        "wraps directly, never through no-env"
    );
    assert_eq!(app.project.env_data.values["tok"], "p");
    let st = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(st.environment.as_deref(), Some("prod"), "persisted");
}

#[test]
fn env_chooser_includes_no_environment_entry() {
    let (mut app, _dir) = app_with_envs();
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.update(Action::OpenEnvChooser);
    let Some(Modal::Chooser(_)) = app.modals.top() else {
        panic!("expected chooser")
    };
    app.update(Action::Close);
    app.update(Action::SwitchEnv(None));
    assert_eq!(app.project.env_label(), "no env");
}

#[test]
fn cycle_env_with_no_environments_toasts() {
    let mut app = App::new_for_test();
    app.update(Action::CycleEnv);
    assert!(!app.toasts.is_empty());
    assert_eq!(app.project.env_label(), "no env");
}

#[tokio::test]
async fn unresolved_variable_blocks_send_with_toast() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://x/{{gone}}");
    app.update(Action::ForceSend);
    assert!(app.session.in_flight.is_none());
    assert!(!app.toasts.is_empty());
}

#[test]
fn toggle_body_vars_flips_flag_and_shows_badge() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new_for_test();
    app.update(Action::ToggleBodyVars);
    assert!(app.editor.substitute_body);

    app.editor.active_tab = EditorTab::Body;
    let backend = TestBackend::new(60, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let content = format!("{:?}", terminal.backend().buffer());
    assert!(content.contains("vars"), "expected a vars badge: {content}");
}

fn app_with_vars() -> App {
    let mut app = App::new_for_test();
    std::fs::write(
        app.project.root.join("variables.toml"),
        "[base]\ndefault = \"http://x\"\n[tok]\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    app
}

#[test]
fn typing_double_brace_in_url_opens_completing_picker_and_insert_lands_in_url() {
    let mut app = app_with_vars();
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('{'));
    assert!(app.modals.is_empty(), "one brace: no picker");
    app.handle_key(&keymap, plain('{'));
    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("expected picker")
    };
    assert!(p.completing);
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.url.text(), "{{base}}");
}

#[test]
fn body_insert_autoenables_substitution() {
    let mut app = app_with_vars();
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;
    app.update(Action::OpenVarPicker { completing: false });
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.body_text(), "{{base}}");
    assert!(app.editor.substitute_body, "auto-enabled");
    assert!(!app.toasts.is_empty());
}

#[test]
fn picker_with_no_declared_vars_toasts() {
    let mut app = App::new_for_test();
    app.update(Action::OpenVarPicker { completing: false });
    assert!(app.modals.is_empty());
    assert!(!app.toasts.is_empty());
}

#[test]
fn click_editor_tab_selects_it() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app.hits.rect_of(&Hit::EditorTab(2)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    assert_eq!(app.focus, PaneId::Editor);
}

fn ready_response(app: &mut App, body: &str) {
    app.session
        .response
        .set_state(ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
            elapsed: std::time::Duration::from_millis(1),
            size: body.len(),
            content_type: Some("application/json".into()),
        })));
}

#[test]
fn click_response_tab_switches_to_headers() {
    use crate::components::response::ViewMode;
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": 1}"#);
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&Hit::ResponseTab(ViewMode::Headers))
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Headers);
    assert_eq!(app.focus, PaneId::Response);
}

#[test]
fn click_json_arrow_collapses_the_container_row() {
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": {"b": 1, "c": 2}}"#);
    render_once(&mut app);
    let before = app.session.response.view().unwrap().visible_len();
    let r = app.hits.rect_of(&Hit::JsonArrow(1)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(
        app.session.response.view().unwrap().visible_len() < before,
        "clicking the arrow collapsed the container"
    );
}

#[test]
fn click_json_row_moves_the_cursor_without_collapsing() {
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": 1, "b": 2}"#);
    render_once(&mut app);
    let before = app.session.response.view().unwrap().visible_len();
    let r = app.hits.rect_of(&Hit::JsonRow(2)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.session.response.view().unwrap().cursor, 2);
    assert_eq!(app.session.response.view().unwrap().visible_len(), before);
}

#[test]
fn oversize_response_does_not_register_the_tree_tab() {
    use crate::components::response::{MAX_PRETTY_BYTES, ViewMode};
    let mut app = App::new_for_test();
    let body = format!("{{\"a\": \"{}\"}}", "x".repeat(MAX_PRETTY_BYTES));
    ready_response(&mut app, &body);
    render_once(&mut app);
    assert_eq!(app.hits.rect_of(&Hit::ResponseTab(ViewMode::Pretty)), None);
}

#[test]
fn click_table_checkbox_toggles_enabled() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    render_once(&mut app);
    let r = app.hits.rect_of(&Hit::TableCheckbox(0)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(!app.editor.params["page"].enabled);
    assert_eq!(app.editor.table.selected, Some(0));
    assert_eq!(app.focus, PaneId::Editor);
}

#[test]
fn double_click_table_row_begins_editing_the_key_cell() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    render_once(&mut app);
    let r = app.hits.rect_of(&Hit::TableRow(0)).unwrap();
    // Clicks past the leading checkbox cell so the row hit (not the
    // checkbox registered on top of it) wins.
    let click_x = r.x + r.width - 1;
    app.handle_mouse(left_down(click_x, r.y));
    assert!(
        app.editor.table.editing.is_none(),
        "single click only selects"
    );
    assert_eq!(
        app.editor.table.selected,
        Some(0),
        "single click selects the row"
    );
    app.handle_mouse(left_down(click_x, r.y));
    let edit = app
        .editor
        .table
        .editing
        .as_ref()
        .expect("double click begins editing");
    assert_eq!(edit.input.text(), "page", "key cell seeded");
}

fn three_params(app: &mut App) {
    for (k, v) in [("a", "1"), ("b", "2"), ("c", "3")] {
        app.editor.params.insert(
            k.into(),
            postui_core::model::Entry {
                value: v.into(),
                enabled: true,
            },
        );
    }
}

#[test]
fn collapse_hides_body_and_keeps_tab_count_visible() {
    let mut app = App::new_for_test();
    three_params(&mut app);
    app.table_collapsed = true;
    app.editor.table_collapsed = true;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    let buf = terminal.backend().buffer();
    let content = format!("{buf:?}");
    assert!(
        !content.contains("NAME"),
        "table header must not be drawn while collapsed: {content}"
    );

    // The param count still shows while collapsed — inside the Params
    // tab's own label.
    assert!(
        content.contains("Params · 3"),
        "param count must stay visible in the tab label: {content}"
    );
}

#[test]
fn collapse_toggle_click_and_key() {
    let mut app = App::new_for_test();
    three_params(&mut app);
    render_once(&mut app);
    assert!(!app.table_collapsed);

    let r = app.hits.rect_of(&Hit::TableCollapse).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(app.table_collapsed, "click toggles collapse on");

    app.handle_key(&Keymap::default_bindings(), alt('p'));
    assert!(!app.table_collapsed, "alt+p toggles it back off");
}

#[test]
fn collapse_on_a_table_tab_shrinks_editor_to_chrome_and_grows_response() {
    let mut app = App::new_for_test();
    three_params(&mut app);
    render_once(&mut app);
    let expanded_response = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();

    app.table_collapsed = true;
    render_once(&mut app);
    let editor = app.hits.rect_of(&Hit::Pane(PaneId::Editor)).unwrap();
    let response = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    assert_eq!(
        editor.height,
        crate::components::editor::CHROME_HEIGHT,
        "editor pane shrinks to exactly its chrome"
    );
    assert!(
        response.height > expanded_response.height,
        "response pane reclaims the freed rows"
    );
}

#[test]
fn collapse_on_the_body_tab_leaves_the_split_unchanged() {
    let mut app = App::new_for_test();
    three_params(&mut app);
    app.editor.active_tab = EditorTab::Body;
    render_once(&mut app);
    let expanded_editor = app.hits.rect_of(&Hit::Pane(PaneId::Editor)).unwrap();
    let expanded_response = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();

    app.table_collapsed = true;
    render_once(&mut app);
    let editor = app.hits.rect_of(&Hit::Pane(PaneId::Editor)).unwrap();
    let response = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    assert_eq!(
        editor, expanded_editor,
        "Body tab active: split unchanged by collapse"
    );
    assert_eq!(response, expanded_response);
}

#[test]
fn open_method_dropdown_has_all_seven_methods_selected_at_current() {
    let mut app = App::new_for_test();
    app.editor.method = postui_core::model::Method::Put;
    app.update(Action::OpenMethodDropdown);
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("expected a Dropdown modal on top");
    };
    assert_eq!(state.items.len(), 7);
    assert_eq!(state.selected, 2, "Put is index 2 in Method::ALL");
    assert_eq!(
        state.items[2].1,
        Action::SetMethod(postui_core::model::Method::Put)
    );
}

#[test]
fn dropdown_down_down_enter_changes_method_and_closes() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::OpenMethodDropdown);
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.method, postui_core::model::Method::Put); // 3rd entry
    assert!(app.modals.is_empty());
}

#[test]
fn dropdown_esc_closes_without_change_and_keys_dont_leak() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    let original = app.editor.method;
    app.update(Action::OpenMethodDropdown);
    // A key with no dropdown binding (and no global binding either)
    // must not leak through to the app — proven here by 'q', which
    // would otherwise quit.
    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
    );
    assert!(!app.should_quit, "'q' must not leak through the dropdown");
    assert!(!app.modals.is_empty(), "dropdown must still be open");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.method, original, "Esc makes no change");
}

#[test]
fn click_method_selector_opens_dropdown_then_click_row_sets_method() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let badge = app.hits.rect_of(&Hit::MethodSelector).unwrap();
    app.handle_mouse(left_down(badge.x, badge.y));
    assert!(matches!(app.modals.top(), Some(Modal::Dropdown(_))));

    render_once(&mut app);
    let row3 = app.hits.rect_of(&Hit::DropdownRow(3)).unwrap();
    app.handle_mouse(left_down(row3.x, row3.y));
    assert_eq!(app.editor.method, postui_core::model::Method::Patch);
    assert!(app.modals.is_empty());
}

#[test]
fn click_palette_row_runs_immediately() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    for c in "quit".chars() {
        app.handle_key(&Keymap::default_bindings(), plain(c));
    }
    render_once(&mut app);
    let row = app.hits.rect_of(&Hit::PaletteRow(0)).unwrap();
    assert!(app.handle_mouse(left_down(row.x, row.y)));
    assert!(app.should_quit, "single click on the Quit row runs it");
    assert!(app.modals.is_empty());
}

#[test]
fn click_chooser_row_selects_then_click_again_confirms() {
    let (mut app, _a, b) = two_projects();
    app.update(Action::OpenProjectChooser);
    render_once(&mut app);
    // Row 0 is alpha (the currently open project); row 1 is beta.
    let row1 = app.hits.rect_of(&Hit::ChooserRow(1)).unwrap();
    assert!(app.handle_mouse(left_down(row1.x, row1.y)));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
        "first click only selects: modal stays open"
    );
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        unreachable!()
    };
    assert_eq!(c.selected(), 1, "selection moved to the clicked row");
    assert_ne!(app.project.root, b.path(), "not switched yet");

    render_once(&mut app);
    let row1 = app.hits.rect_of(&Hit::ChooserRow(1)).unwrap();
    assert!(app.handle_mouse(left_down(row1.x, row1.y)));
    assert_eq!(
        app.project.root,
        b.path(),
        "second click on the already-selected row confirms"
    );
    assert!(app.modals.is_empty());
}

#[test]
fn click_outside_the_palette_closes_it_with_no_action() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    render_once(&mut app);
    let palette_row = app.hits.rect_of(&Hit::PaletteRow(0)).unwrap();
    // A point in the screen's top-left corner, clear of the centered
    // palette rect.
    assert!(
        palette_row.y > 0,
        "sanity: the palette isn't flush against the top edge"
    );
    assert!(app.handle_mouse(left_down(0, 0)));
    assert!(app.modals.is_empty());
    assert!(!app.should_quit);
}

#[test]
fn click_confirm_choice_chip_deletes_the_request() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.sidebar.selected = Some(0);
    app.update(Action::ConfirmDeleteRequest);
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));

    render_once(&mut app);
    let chip = app.hits.rect_of(&Hit::ConfirmChoice('y')).unwrap();
    assert!(app.handle_mouse(left_down(chip.x, chip.y)));
    assert!(app.modals.is_empty());
    assert!(
        !postui_core::storage::request_exists(dir.path(), "ping"),
        "clicking the [y] chip must delete the request"
    );
}

#[test]
fn click_message_ok_button_closes_it_same_as_enter() {
    let mut app = App::new_for_test();
    app.update(Action::ShowAbout);
    assert!(matches!(app.modals.top(), Some(Modal::Message { .. })));

    render_once(&mut app);
    let ok = app.hits.rect_of(&Hit::ModalConfirm).unwrap();
    assert!(app.handle_mouse(left_down(ok.x, ok.y)));
    assert!(
        app.modals.is_empty(),
        "clicking OK must close the modal, exactly like Enter/Esc"
    );
}

#[test]
fn click_prompt_cancel_button_closes_without_creating_a_request() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "api/ping".chars() {
        app.handle_key(&keymap, plain(c));
    }
    render_once(&mut app);
    let cancel = app.hits.rect_of(&Hit::ModalCancel).unwrap();
    assert!(app.handle_mouse(left_down(cancel.x, cancel.y)));
    assert!(
        app.modals.is_empty(),
        "clicking Cancel must close the modal, exactly like Esc"
    );
    assert!(
        postui_core::storage::list_requests(&app.project.root)
            .0
            .is_empty(),
        "Cancel must not create anything, matching Esc's no-op"
    );
}

#[test]
fn click_prompt_confirm_button_creates_the_request_like_enter() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "api/ping".chars() {
        app.handle_key(&keymap, plain(c));
    }
    render_once(&mut app);
    let confirm = app.hits.rect_of(&Hit::ModalConfirm).unwrap();
    assert!(app.handle_mouse(left_down(confirm.x, confirm.y)));
    assert!(app.modals.is_empty());
    assert!(
        postui_core::storage::load_request(&app.project.root, "api/ping").is_ok(),
        "clicking Confirm must create the request, exactly like Enter"
    );
}

#[test]
fn click_new_project_cancel_button_closes_without_creating() {
    let mut app = App::new_for_test();
    let root = tempfile::tempdir().unwrap();
    app.registry.root = Some(root.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewProject);
    for c in "My Svc".chars() {
        app.handle_key(&keymap, plain(c));
    }
    render_once(&mut app);
    let cancel = app.hits.rect_of(&Hit::ModalCancel).unwrap();
    assert!(app.handle_mouse(left_down(cancel.x, cancel.y)));
    assert!(
        app.modals.is_empty(),
        "clicking Cancel must close the modal, exactly like Esc"
    );
    assert!(
        !postui_core::project::is_project(&root.path().join("my-svc")),
        "Cancel must not create anything, matching Esc's no-op"
    );
}

#[test]
fn click_new_project_confirm_button_creates_the_project_like_enter() {
    let mut app = App::new_for_test();
    let root = tempfile::tempdir().unwrap();
    app.registry.root = Some(root.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewProject);
    for c in "My Svc".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    render_once(&mut app);
    let confirm = app.hits.rect_of(&Hit::ModalConfirm).unwrap();
    assert!(app.handle_mouse(left_down(confirm.x, confirm.y)));
    let expected = root.path().join("my-svc");
    assert!(app.modals.is_empty());
    assert!(
        postui_core::project::is_project(&expected),
        "clicking Confirm must create the project, exactly like Enter"
    );
    assert_eq!(app.project.root, expected);
}

#[test]
fn chooser_keys_and_wheel_keep_a_long_list_scrolling_correctly() {
    use crate::components::chooser::{ChooserItem, ChooserState};
    let mut app = App::new_for_test();
    let items: Vec<ChooserItem> = (0..25)
        .map(|i| ChooserItem {
            label: format!("item{i:02}"),
            detail: None,
            actions: vec![Action::Render],
        })
        .collect();
    app.modals
        .push(Modal::Chooser(ChooserState::new("Many", items)));
    // A tall-enough terminal that the modal clamps to its 16-row cap.
    {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    }

    let keymap = Keymap::default_bindings();
    for _ in 0..20 {
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    render_once(&mut app);
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        panic!("expected a Chooser modal on top");
    };
    assert_eq!(c.selected(), 20);
    assert!(
        app.hits.rect_of(&Hit::ChooserRow(20)).is_some(),
        "row 20 must be drawn (and hit-registered) once scroll caught up: {}",
        c.selected()
    );

    // Wheel scrolling must move the viewport without moving selection.
    let area = app.hits.rect_of(&Hit::ChooserRow(20)).unwrap();
    app.handle_mouse(scroll_down(area.x, area.y));
    render_once(&mut app);
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        panic!("expected a Chooser modal on top");
    };
    assert_eq!(c.selected(), 20, "wheel must not move the selection");
}

/// Mouse-first ruling (post-stage-5-review): in flight is a distinct
/// state from disabled. The painted Send cap keeps `Hit::SendButton`
/// registered while sending -- it shows a spinner + "Sending" (or
/// "Cancel" on hover) instead of the old `[ Cancel ]` bracket text, but
/// a second click on the same rect still cancels, routed by `App`'s
/// `Hit::SendButton` handler checking `in_flight.is_some()`.
#[tokio::test]
async fn click_send_button_sends_then_click_again_cancels() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("https://example.com");
    render_once(&mut app);
    let before = app.hits.rect_of(&Hit::SendButton).unwrap();

    app.handle_mouse(left_down(before.x, before.y));
    assert!(app.session.in_flight.is_some(), "click dispatches Send");
    assert!(app.editor.sending, "editor.sending mirrors in_flight");

    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let after = app.hits.rect_of(&Hit::SendButton).unwrap();
    assert_eq!(
        before, after,
        "Send cap occupies the same rect while sending"
    );
    let content = format!("{:?}", terminal.backend().buffer());
    assert!(
        content.contains("Sending"),
        "cap now reads Sending: {content}"
    );

    app.handle_mouse(left_down(after.x, after.y));
    assert!(matches!(
        app.session.response.state(),
        ResponseState::Cancelled
    ));
}

#[test]
fn copy_body_writes_via_clipboard_cmd_and_toasts_copied() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    ready_response(&mut app, r#"{"a": 1}"#);

    app.update(Action::CopyToClipboard(CopyTarget::ResponseBody));

    assert_eq!(std::fs::read_to_string(&out).unwrap(), r#"{"a": 1}"#);
    assert!(
        rendered_text(&mut app).contains("Copied response body"),
        "toast confirms the copy"
    );
}

#[test]
fn copy_body_over_osc52_threshold_toasts_too_large() {
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(None, 8, false));
    ready_response(&mut app, "12345678"); // 8 bytes, at the threshold

    app.update(Action::CopyToClipboard(CopyTarget::ResponseBody));

    assert!(
        rendered_text(&mut app).contains("Too large for the terminal clipboard"),
        "toast explains the size limit"
    );
}

#[test]
fn prompt_save_body_prefills_json_extension_and_enter_writes_the_file() {
    let mut app = App::new_for_test();
    app.editor.slug = Some("pingpong".into());
    // Sync the session to the slug before seeding the response, or the
    // next update would stash it in the scratch request's cache slot.
    app.update(Action::Render);
    ready_response(&mut app, r#"{"a": 1}"#);

    app.update(Action::PromptSaveBody);

    let Some(Modal::Prompt {
        kind: PromptKind::SaveBodyAs,
        input,
        ..
    }) = app.modals.top()
    else {
        panic!("expected a SaveBodyAs prompt");
    };
    assert!(
        input.text().ends_with("-response.json"),
        "prefill: {}",
        input.text()
    );

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("body.json");
    app.update(Action::SaveBodyToFile(out.to_string_lossy().to_string()));

    assert_eq!(std::fs::read_to_string(&out).unwrap(), r#"{"a": 1}"#);
    assert!(rendered_text(&mut app).contains("Saved body to"));
}

#[test]
fn header_copy_click_and_key_parity_both_copy_the_header() {
    let mut app = App::new_for_test();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    ready_response(&mut app, r#"{"a": 1}"#);
    app.update(Action::ResponseViewMode(
        crate::components::response::ViewMode::Headers,
    ));
    render_once(&mut app);

    let r = app.hits.rect_of(&Hit::HeaderCopy(0)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "application/json",
        "clicking HeaderCopy(0) copies the first header's value"
    );
    assert!(rendered_text(&mut app).contains("Copied content-type"));

    // `c` key parity in Headers view produces the same action.
    let action = app
        .session
        .response
        .handle_key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        ));
    assert_eq!(
        action,
        Some(Action::CopyToClipboard(CopyTarget::ResponseHeader(0)))
    );
}

#[test]
fn copy_body_with_no_response_toasts_nothing_to_copy_and_leaves_clipboard_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));

    app.update(Action::CopyToClipboard(CopyTarget::ResponseBody));

    assert!(!out.exists(), "clipboard must not be touched");
    assert!(rendered_text(&mut app).contains("nothing to copy — send a request first"));
}

// --- Task 9: Screen enum + Variable Manager shell (spec §5) ---------------

#[test]
fn alt_v_opens_the_manager_and_renders_its_title() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);
    let content = rendered_text(&mut app);
    assert!(content.contains("Variables"));
}

#[test]
fn palette_variable_manager_command_opens_the_manager() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::OpenPalette);
    for c in "Variable Manager".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.screen, crate::app::Screen::VarManager);
    assert!(
        app.modals.is_empty(),
        "palette closes after running the command"
    );
}

#[test]
fn esc_returns_to_main_with_prior_focus_restored() {
    let mut app = App::new_for_test();
    app.focus = PaneId::Response;
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);

    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.screen, crate::app::Screen::Main);
    assert_eq!(app.focus, PaneId::Response, "prior focus is restored");
}

#[test]
fn modals_still_open_and_close_on_top_of_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);

    // ctrl+p still opens the palette on top of the Manager screen.
    app.handle_key(&keymap, ctrl('p'));
    assert!(!app.modals.is_empty());
    assert_eq!(
        app.screen,
        crate::app::Screen::VarManager,
        "opening a modal must not leave the screen"
    );

    // Esc closes the modal first, without leaving the Manager screen.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert_eq!(
        app.screen,
        crate::app::Screen::VarManager,
        "closing the modal must not also leave the screen"
    );
}

#[test]
fn plain_q_does_not_quit_from_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);

    app.handle_key(&keymap, plain('q'));
    assert!(!app.should_quit, "q is not the palette and must not quit");
    assert_eq!(app.screen, crate::app::Screen::VarManager);
}

#[test]
fn manager_screen_replaces_the_three_panes_but_keeps_header_and_footer() {
    let mut app = App::new_for_test();
    app.update(Action::OpenVarManager);
    let content = rendered_text(&mut app);
    assert!(content.contains("postui"), "header wordmark stays");
    assert!(
        content.contains("esc"),
        "footer hint stays / manager hint shows"
    );
    assert!(
        !content.contains("REQUESTS"),
        "the sidebar pane is replaced by the full-frame Manager"
    );
}
