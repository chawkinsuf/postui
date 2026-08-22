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
    select_palette_command(&mut app, "quit");
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(app.usage.score("quit", crate::usage::now()) > 0.0);
}

/// The filtered-list index of command `id` in the open palette. A typed
/// query is a subsequence match, so several commands can survive it (e.g.
/// "quit" also matches "Request: duplicate"); tests that mean one specific
/// command name it by id rather than assuming it lands on row 0.
fn palette_row_of(app: &App, id: &str) -> usize {
    let Some(Modal::Palette(p)) = app.modals.top() else {
        panic!("expected the palette to be open");
    };
    p.filtered()
        .iter()
        .position(|c| c.id == id)
        .unwrap_or_else(|| panic!("{id} was filtered out"))
}

/// Moves the open palette's cursor onto command `id`.
fn select_palette_command(app: &mut App, id: &str) {
    let i = palette_row_of(app, id);
    let Some(Modal::Palette(p)) = app.modals.top_mut() else {
        unreachable!()
    };
    p.select(i);
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

fn right_down(x: u16, y: u16) -> ratatui::crossterm::event::MouseEvent {
    ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(
            ratatui::crossterm::event::MouseButton::Right,
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
fn deleting_a_vars_row_by_key_requires_confirmation() {
    // Same delete-confirm plumbing as Params, pointed at the Vars tab's
    // request-scoped `[variables]` table.
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Vars;
    app.editor.variables.insert(
        "token".into(),
        postui_core::model::Entry {
            value: "abc".into(),
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
            assert!(body.contains("token"), "confirm names the key: {body}")
        }
        _ => panic!("expected a Confirm modal"),
    }
    assert_eq!(
        app.editor.variables.len(),
        1,
        "row survives until confirmed"
    );

    app.handle_key(&keymap, plain('y'));
    assert!(
        app.editor.variables.is_empty(),
        "confirming deletes the row"
    );
    assert!(app.modals.top().is_none(), "modal closed after the choice");
}

#[test]
fn editor_tab_cycle_order_is_params_headers_vars_body() {
    let mut app = App::new_for_test();
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(app.editor.active_tab, EditorTab::Vars);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(
        app.editor.active_tab,
        EditorTab::Params,
        "cycle wraps back to Params"
    );
}

#[test]
fn alt_1_2_3_still_select_params_headers_body_with_vars_inserted() {
    // Task 13: "alt+1/2/3 aliases unaffected" — Vars is reachable by click
    // or EditorTabCycle only, not by these three shortcuts.
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.editor.active_tab = EditorTab::Body;
    app.handle_key(&keymap, alt('1'));
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    app.handle_key(&keymap, alt('2'));
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    app.handle_key(&keymap, alt('3'));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
}

#[test]
fn shadowed_var_shows_masked_hint_when_project_var_is_secret() {
    use postui_core::model::Entry;

    let mut app = App::new_for_test();
    std::fs::write(
        app.project.root.join("variables.toml"),
        "[token]\nsecret = true\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    app.project
        .secrets
        .entry(String::new())
        .or_default()
        .insert("token".into(), "s3cr3t".into());
    app.project.refresh_resolved();

    app.editor.variables.insert(
        "token".into(),
        Entry {
            value: "override".into(),
            enabled: true,
        },
    );
    app.update(Action::Render);

    let hint = app
        .editor
        .shadowed
        .get("token")
        .expect("token shadows the project secret");
    assert!(
        !hint.contains("s3cr3t"),
        "the secret's real value must never appear: {hint}"
    );
    assert!(
        hint.contains("\u{25cf}\u{25cf}\u{25cf}\u{25cf}"),
        "expected a masked secret hint, got: {hint}"
    );
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

/// Seeds the Params tab with `page = 1` and puts the editor in front.
fn app_with_one_param() -> App {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "1".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Params;
    app.focus = PaneId::Editor;
    app
}

/// Re-renders and clicks just inside `hit`'s rect.
fn click_hit(app: &mut App, hit: Hit) {
    render_once(app);
    let r = app
        .hits
        .rect_of(&hit)
        .unwrap_or_else(|| panic!("no rect registered for {hit:?}"));
    let x = if r.width > 1 { r.x + 1 } else { r.x };
    app.handle_mouse(left_down(x, r.y));
}

fn type_chars(app: &mut App, s: &str) {
    let keymap = Keymap::default_bindings();
    for c in s.chars() {
        app.handle_key(&keymap, plain(c));
    }
}

#[test]
fn click_cell_edits_in_place_and_click_away_commits() {
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    assert!(
        app.editor.table.editing.is_some(),
        "one click into a cell edits it — no select-then-edit dance"
    );
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
    type_chars(&mut app, "2");

    // Clicking the URL bar is a click away: it commits, it doesn't discard.
    render_once(&mut app);
    let url = app.editor.last_url_text_area.unwrap();
    app.handle_mouse(left_down(url.x + 1, url.y));
    assert!(app.editor.table.editing.is_none());
    assert_eq!(app.editor.params["page"].value, "12");
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
}

#[test]
fn two_fast_clicks_on_a_cell_leave_exactly_one_edit_session() {
    let mut app = app_with_one_param();
    render_once(&mut app);
    let cell = app
        .hits
        .rect_of(&Hit::TableCell { row: 0, col: 1 })
        .unwrap();
    app.handle_mouse(left_down(cell.x + 1, cell.y));
    type_chars(&mut app, "2");
    // The second click of a double click lands on the same cell; it must
    // do nothing beyond what the first did.
    render_once(&mut app);
    let cell = app
        .hits
        .rect_of(&Hit::TableCell { row: 0, col: 1 })
        .unwrap();
    app.handle_mouse(left_down(cell.x + 1, cell.y));
    let edit = app.editor.table.editing.as_ref().expect("still editing");
    assert_eq!(edit.input.text(), "12", "the typing survives");
    assert_eq!(app.editor.params["page"].value, "1", "not committed yet");

    // The first click expands the row, so the second click of a real
    // double click often lands on one of the pad lines the expansion added
    // (the row background) rather than the cell. That must be inert too.
    render_once(&mut app);
    let row = app.hits.rect_of(&Hit::TableRow(0)).unwrap();
    assert_eq!(row.height, 3, "the edited row is expanded");
    app.handle_mouse(left_down(row.x, row.y));
    let edit = app
        .editor
        .table
        .editing
        .as_ref()
        .expect("a click on the edited row's own chrome keeps the edit");
    assert_eq!(edit.input.text(), "12");
}

#[test]
fn clicking_the_ghost_row_and_typing_creates_the_row_when_it_commits() {
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 1, col: 0 });
    type_chars(&mut app, "limit");
    assert_eq!(app.editor.params.len(), 1, "nothing inserted while typing");
    render_once(&mut app);
    let url = app.editor.last_url_text_area.unwrap();
    app.handle_mouse(left_down(url.x + 1, url.y));
    assert_eq!(
        app.editor.params.get("limit").map(|e| e.value.as_str()),
        Some(""),
        "the ghost row became a real row on commit"
    );
}

#[test]
fn a_ghost_row_left_empty_creates_nothing() {
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 1, col: 0 });
    render_once(&mut app);
    let url = app.editor.last_url_text_area.unwrap();
    app.handle_mouse(left_down(url.x + 1, url.y));
    assert_eq!(app.editor.params.len(), 1, "no empty row was added");
    assert!(app.editor.table.editing.is_none());
}

#[test]
fn esc_mid_edit_puts_the_original_cell_text_back() {
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_chars(&mut app, "999");
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(app.editor.table.editing.is_none());
    assert_eq!(app.editor.params["page"].value, "1", "the edit reverted");
    assert_eq!(app.editor.params.len(), 1, "the row survives");
}

#[test]
fn checkbox_and_delete_clicks_during_an_edit_commit_it_first() {
    let mut app = App::new_for_test();
    for (k, v) in [("a", "1"), ("b", "2")] {
        app.editor.params.insert(
            k.into(),
            postui_core::model::Entry {
                value: v.into(),
                enabled: true,
            },
        );
    }
    app.focus = PaneId::Editor;
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_chars(&mut app, "9");
    click_hit(&mut app, Hit::TableCheckbox(1));
    assert_eq!(app.editor.params["a"].value, "19", "the edit committed");
    assert!(!app.editor.params["b"].enabled, "and the toggle landed");
    assert!(app.editor.table.editing.is_none());

    // Same for the ✕ affordance on another row.
    click_hit(&mut app, Hit::TableCell { row: 0, col: 0 });
    type_chars(&mut app, "x");
    render_once(&mut app);
    let del = app.hits.rect_of(&Hit::TableDelete(0)).unwrap();
    app.handle_mouse(left_down(del.x, del.y));
    assert_eq!(
        app.editor.params.get_index(0).unwrap().0,
        "ax",
        "the rename committed before the delete confirm opened"
    );
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
}

/// Params `a=1, b=2, c=3`, editor focused.
fn app_with_three_params() -> App {
    let mut app = App::new_for_test();
    for (k, v) in [("a", "1"), ("b", "2"), ("c", "3")] {
        app.editor.params.insert(
            k.into(),
            postui_core::model::Entry {
                value: v.into(),
                enabled: true,
            },
        );
    }
    app.focus = PaneId::Editor;
    app
}

/// Puts row 0's key cell under edit with "c" typed into it — committing it
/// collapses row "a" into row "c" and shifts every later row down one.
fn stage_a_collapsing_rename(app: &mut App) {
    let keymap = Keymap::default_bindings();
    click_hit(app, Hit::TableCell { row: 0, col: 0 });
    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
    );
    type_chars(app, "c");
}

#[test]
fn a_collapsing_commit_reresolves_the_row_a_checkbox_click_named() {
    let mut app = app_with_three_params();
    stage_a_collapsing_rename(&mut app);
    // Row 1 is "b" in the frame the user clicked; after the commit collapses
    // "a" into "c" it is row 0. The toggle must follow the row, not the
    // index.
    click_hit(&mut app, Hit::TableCheckbox(1));
    assert_eq!(app.editor.params.len(), 2, "a collapsed into c");
    assert!(!app.editor.params["b"].enabled, "the clicked row toggled");
    assert!(app.editor.params["c"].enabled, "no neighbour was toggled");
}

#[test]
fn a_collapsing_commit_reresolves_the_row_a_delete_click_named() {
    let mut app = app_with_three_params();
    stage_a_collapsing_rename(&mut app);
    // The ✕ sits on the row being edited; after the collapse that row *is*
    // the surviving "c", so the confirm must name "c" — not whatever now
    // occupies index 0 ("b").
    render_once(&mut app);
    let del = app.hits.rect_of(&Hit::TableDelete(0)).unwrap();
    app.handle_mouse(left_down(del.x, del.y));
    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("expected the delete confirm");
    };
    assert!(
        body.contains('c'),
        "the confirm names the clicked row: {body}"
    );
    assert!(
        !body.contains('b'),
        "and never the row that shifted into its index: {body}"
    );
}

#[test]
fn ctrl_s_commits_the_cell_under_edit_into_the_saved_file() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "ping", &req("https://x/ping")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("ping".into()));
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Params;

    click_hit(&mut app, Hit::TableCell { row: 0, col: 0 }); // the ghost row
    type_chars(&mut app, "page");
    app.handle_key(&Keymap::default_bindings(), ctrl('s'));

    let saved = postui_core::storage::load_request(&app.project.root, "ping").unwrap();
    assert!(
        saved.params.contains_key("page"),
        "the cell under the caret is part of what ctrl+s saves: {:?}",
        saved.params
    );
    assert!(app.editor.table.editing.is_none());
}

#[test]
fn clicking_the_toolbar_save_chip_commits_the_cell_under_edit_and_saves() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "ping", &req("https://x/ping")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("ping".into()));
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Params;

    click_hit(&mut app, Hit::TableCell { row: 0, col: 0 }); // the ghost row
    type_chars(&mut app, "page");
    click_hit(&mut app, Hit::FooterChip(Action::SaveRequest));

    let saved = postui_core::storage::load_request(&app.project.root, "ping").unwrap();
    assert!(
        saved.params.contains_key("page"),
        "the in-progress cell rides along with a mouse-only save: {:?}",
        saved.params
    );
    assert!(
        !app.editor.is_dirty(),
        "a successful save clears the dirty flag"
    );
}

#[test]
fn clicking_the_toolbar_format_chip_formats_the_body() {
    let mut app = App::new_for_test();
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("{\"a\":1}");

    click_hit(&mut app, Hit::FooterChip(Action::FormatBody));

    assert!(
        app.editor.body_text().contains('\n'),
        "the format chip pretty-prints the body: {:?}",
        app.editor.body_text()
    );
}

#[tokio::test]
async fn sending_commits_the_cell_under_edit_into_the_request() {
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_chars(&mut app, "2");
    app.editor.url = crate::components::line_input::LineInput::new("https://x/y");
    app.update(Action::Send);
    assert_eq!(
        app.editor.params["page"].value, "12",
        "the typed cell rides along with the request"
    );
}

#[test]
fn switching_editor_tabs_commits_the_cell_under_edit() {
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_chars(&mut app, "2");
    app.handle_key(&Keymap::default_bindings(), alt('2'));
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    assert_eq!(
        app.editor.params["page"].value, "12",
        "the tab switch commits instead of resetting the edit away"
    );
    assert!(app.editor.table.editing.is_none());
}

#[test]
fn up_from_a_cell_under_edit_commits_and_never_desyncs_the_focus() {
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_chars(&mut app, "2");
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert!(app.editor.table.editing.is_none(), "the edit committed");
    assert_eq!(app.editor.params["page"].value, "12");
    assert_eq!(
        app.editor.sub_focus,
        SubFocus::Content,
        "the first Up stays in the table"
    );
    // Only then does Up climb out — with no edit left open behind it.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.editor.sub_focus, SubFocus::Tabs);
    assert!(app.editor.table.editing.is_none());
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
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![],
            size: body.len(),
            body,
            elapsed: std::time::Duration::from_millis(1),
            content_type: Some("text/plain".into()),
        })),
        0,
    );
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
fn clicking_the_manager_env_switcher_opens_the_environment_chooser() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenVarManager);
    render_once(&mut app);
    assert!(rendered_text(&mut app).contains("Environment: qa"));

    let r = app
        .hits
        .rect_of(&crate::hit::Hit::VmEnvSwitch)
        .expect("env switcher registered");
    app.handle_mouse(left_down(r.x + 2, r.y + 1));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
        "the switcher opens the same chooser the header env chip does"
    );

    // Switching relabels the bar and the group's inline selection with it.
    app.update(Action::Close);
    app.update(Action::SwitchEnv(Some("dev".into())));
    let content = rendered_text(&mut app);
    assert!(content.contains("Environment: dev"), "{content}");
    assert!(
        content.contains("user (needs selection)"),
        "dev has no entries for the group: {content}"
    );
}

#[test]
fn the_manager_left_list_lists_variables_then_groups() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenVarManager);
    let content = rendered_text(&mut app);
    assert!(
        content.contains("VARIABLES") && content.contains("GROUPS"),
        "{content}"
    );
    assert!(content.contains("base_url"), "{content}");

    use crate::components::varmanager::{VmDetail, VmRow};
    let group_row = app
        .varmanager
        .left_rows
        .iter()
        .position(|r| r == &VmRow::Group("user".into()))
        .expect("the group has a row");
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::VmLeftRow(group_row))
        .expect("left row registered");
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    assert_eq!(app.varmanager.detail, VmDetail::Group("user".into()));
}

#[test]
fn right_clicking_a_left_row_opens_its_rename_duplicate_delete_menu() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenVarManager);
    rendered_text(&mut app);

    use crate::components::varmanager::VmRow;
    let row = app
        .varmanager
        .left_rows
        .iter()
        .position(|r| r == &VmRow::Var("base_url".into()))
        .unwrap();
    let r = app.hits.rect_of(&crate::hit::Hit::VmLeftRow(row)).unwrap();
    app.handle_mouse(right_down(r.x + 1, r.y + 1));

    let Some(Modal::Dropdown(menu)) = app.modals.top() else {
        panic!("expected a context menu");
    };
    let labels: Vec<String> = menu.items.iter().map(|i| i.label.clone()).collect();
    assert_eq!(
        labels,
        vec!["Rename\u{2026}", "Duplicate", "Delete\u{2026}"]
    );
    assert_eq!(
        menu.items[2].action,
        Some(Action::ConfirmDeleteVar {
            name: "base_url".into()
        })
    );
}

#[test]
fn duplicating_a_variable_copies_its_description_and_default() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::DuplicateVar {
        name: "base_url".into(),
    });
    let copy = app
        .project
        .model
        .vars
        .get("base_url-copy")
        .expect("copy declared");
    assert_eq!(copy.default.as_deref(), Some("http://localhost:8080"));
    assert_eq!(copy.description.as_deref(), Some("API root"));

    // A second duplicate steps the suffix rather than colliding.
    app.update(Action::DuplicateVar {
        name: "base_url".into(),
    });
    assert!(app.project.model.vars.contains_key("base_url-copy-2"));
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
}

/// A group's fields belong to exactly one group (`ModelError::
/// FieldInTwoGroups`), so a duplicate carrying the same field list could
/// never load: the menu shows "Duplicate" disabled for a group instead of
/// writing a `variables.toml` that no longer parses.
#[test]
fn duplicating_a_group_is_offered_but_disabled() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Group("user".into())
    });

    let menu = app
        .varmanager
        .context_menu(app.varmanager.left_cursor)
        .expect("menu for a group row");
    assert_eq!(menu[1].label, "Duplicate");
    assert_eq!(menu[1].action, None);
}

#[test]
fn header_vars_button_toggles_the_variable_manager() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::HeaderVars)
        .expect("vars button registered in the header");
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.screen, crate::app::Screen::VarManager);
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::HeaderVars)
        .expect("vars button still present on the manager screen");
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(
        app.screen,
        crate::app::Screen::Main,
        "second click toggles back"
    );
}

#[test]
fn click_header_env_opens_env_chooser() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::HeaderEnv).unwrap();
    assert!(app.modals.is_empty());
    app.handle_mouse(left_down(r.x, r.y));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
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
        .set_state(ResponseState::Failed("a's result".into()), 0);

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
    app.session.response.set_state(
        ResponseState::InFlight {
            started: std::time::Instant::now(),
        },
        0,
    );
    let action = app
        .session
        .response
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(action, Some(Action::CancelSend));
}

#[test]
fn plain_keys_reach_the_focused_response_pane() {
    let mut app = App::new_for_test();
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![],
            body: r#"{"a": 1}"#.into(),
            elapsed: std::time::Duration::from_millis(5),
            size: 8,
            content_type: None,
        })),
        0,
    );
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
fn env_chooser_new_environment_row_opens_prompt() {
    let (mut app, _dir) = app_with_envs();
    let keymap = Keymap::default_bindings();
    app.update(Action::OpenEnvChooser);
    assert!(matches!(app.modals.top(), Some(Modal::Chooser(_))));
    // "new" filters to the "new environment…" row alone (prod/qa/"no
    // environment" don't match), so Enter confirms it
    for c in "new".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        matches!(
            app.modals.top(),
            Some(Modal::Prompt {
                kind: PromptKind::NewEnvironment,
                ..
            })
        ),
        "confirming the create row should open the name prompt"
    );
}

#[test]
fn create_env_prompt_flow_creates_empty_file_and_switches() {
    let (mut app, dir) = app_with_envs();
    let keymap = Keymap::default_bindings();
    app.update(Action::OpenNewEnvPrompt);
    for c in "dev".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    let path = dir.path().join("environments/dev.toml");
    assert!(path.is_file());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    assert_eq!(app.project.env_label(), "dev", "switches to the new env");
    assert!(app.project.environments.contains(&"dev".to_string()));
    let st = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(st.environment.as_deref(), Some("dev"), "persisted");
}

#[test]
fn create_env_invalid_or_duplicate_name_toasts_and_keeps_active_env() {
    let (mut app, dir) = app_with_envs();
    app.update(Action::SwitchEnv(Some("qa".into())));
    let toasts_before = app.toasts.messages().len();

    app.update(Action::CreateEnv("Bad Name".into()));
    assert!(
        app.toasts.messages().len() > toasts_before,
        "invalid name must toast"
    );
    assert_eq!(app.project.env_label(), "qa");
    assert!(!dir.path().join("environments/Bad Name.toml").exists());

    let toasts_before = app.toasts.messages().len();
    app.update(Action::CreateEnv("prod".into()));
    assert!(
        app.toasts.messages().len() > toasts_before,
        "duplicate name must toast"
    );
    assert_eq!(app.project.env_label(), "qa", "no switch on failure");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("environments/prod.toml")).unwrap(),
        "tok = \"p\"\n",
        "existing file untouched"
    );
}

#[test]
fn env_chooser_with_no_environments_still_opens_with_create_row() {
    let mut app = App::new_for_test();
    app.update(Action::OpenEnvChooser);
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
        "empty project opens the chooser (no-env + create rows), not a toast"
    );
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
    // Wide enough for all 4 tabs (Params, Headers, Vars, Body) plus the
    // "vars" substitution badge that follows the strip.
    let backend = TestBackend::new(80, 20);
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
fn picker_with_no_declared_vars_still_offers_the_new_variable_row() {
    // Task 15: the picker no longer needs anything declared — the "new
    // variable…" row makes it a creation flow too, so opening it with an
    // empty project stays open on that one row instead of toasting.
    let mut app = App::new_for_test();
    app.update(Action::OpenVarPicker { completing: false });
    assert!(app.modals.top().is_some());
    let content = rendered_text(&mut app);
    assert!(content.contains("new variable"), "{content}");
}

#[test]
fn insert_picker_lists_project_group_and_request_vars_with_badges_and_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    std::fs::write(
        dir.path().join("variables.toml"),
        r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[groups.identity]
description = "identity"
fields = ["user_id", "customer_id"]
"#,
    )
    .unwrap();
    let mut req = req("https://x/ping");
    req.variables.insert(
        "trace_id".into(),
        postui_core::model::Entry {
            value: "abc-123".into(),
            enabled: true,
        },
    );
    postui_core::storage::save_request(dir.path(), "r", &req).unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("r".into()));
    app.update(Action::OpenVarPicker { completing: false });

    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("expected the insert picker to open")
    };
    assert_eq!(p.mode, crate::components::var_picker::PickerMode::Insert);

    let content = rendered_text(&mut app);
    assert!(content.contains("base_url"), "{content}");
    assert!(content.contains("API root"), "{content}");
    assert!(content.contains("user_id"), "{content}");
    assert!(content.contains("trace_id"), "{content}");
    assert!(content.contains("proj"), "{content}");
    assert!(content.contains("grp"), "{content}");
    assert!(content.contains("req"), "{content}");
    assert!(content.contains("new variable"), "{content}");
}

#[test]
fn insert_picker_marks_secret_vars_with_the_lock_badge_and_never_shows_the_value() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::project::save_secrets(dir.path(), &{
        let mut secrets = indexmap::IndexMap::new();
        let mut qa = indexmap::IndexMap::new();
        qa.insert("api_key".to_string(), "sk-super-secret".to_string());
        secrets.insert("qa".to_string(), qa);
        secrets
    })
    .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenVarPicker { completing: false });

    let content = rendered_text(&mut app);
    assert!(content.contains("api_key"), "{content}");
    assert!(content.contains("\u{1f512}"), "secret badge: {content}");
    assert!(
        !content.contains("sk-super-secret"),
        "secret value must never render: {content}"
    );
}

#[test]
fn insert_picker_new_variable_row_opens_prompt_prefilled_with_typed_filter() {
    let mut app = app_with_vars();
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.update(Action::OpenVarPicker { completing: false });
    let keymap = Keymap::default_bindings();
    for c in "brand_new".chars() {
        app.handle_key(&keymap, plain(c));
    }
    // With nothing named "brand_new" declared, the filtered list is empty —
    // the ghost row is still there and still selectable.
    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("expected the picker to still be open")
    };
    assert_eq!(p.selected(), 0, "the only row left is the ghost row");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let Some(Modal::Prompt { input, kind, .. }) = app.modals.top() else {
        panic!("expected the new-variable prompt to open")
    };
    assert_eq!(input.text(), "brand_new");
    assert_eq!(
        *kind,
        crate::components::modal::PromptKind::NewVariableAndInsert { completing: false }
    );
}

#[test]
fn insert_picker_new_variable_confirm_creates_the_var_and_inserts_at_the_original_cursor() {
    let mut app = app_with_vars();
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url = crate::components::line_input::LineInput::new("https://x/?a=1");
    app.editor.url.set_cursor(10);
    app.update(Action::OpenVarPicker { completing: false });
    let keymap = Keymap::default_bindings();
    for c in "token".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // Confirming the ghost row swaps the picker for the prompt — same
    // focus, no separate stacked modal to dismiss.
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.modals.is_empty(), "both modals closed");
    assert_eq!(
        app.editor.url.text(),
        "https://x/{{token}}?a=1",
        "inserted at the original cursor, focus returned exactly where it was"
    );
    assert!(
        app.project.model.vars.contains_key("token"),
        "the new variable was declared"
    );
    let saved = postui_core::project::load_variables(&app.project.root).unwrap();
    assert!(saved.vars.contains_key("token"), "written to disk too");
}

#[test]
fn insert_picker_new_variable_confirm_with_a_reserved_name_toasts_and_inserts_nothing() {
    // Review finding: apply_modal_result used to dispatch every action in a
    // ModalResult unconditionally, so a failed NewVar (reserved/invalid/
    // colliding name — LineInput doesn't restrict characters) still ran the
    // InsertVarText that followed it, leaving a `{{options}}` token
    // referencing a variable that was never declared.
    let mut app = app_with_vars();
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url = crate::components::line_input::LineInput::new("https://x/?a=1");
    app.editor.url.set_cursor(10);
    app.update(Action::OpenVarPicker { completing: false });
    let keymap = Keymap::default_bindings();
    for c in "options".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.modals.is_empty(), "the prompt still closes on Enter");
    assert!(!app.toasts.is_empty(), "the reserved-name error toasts");
    assert_eq!(
        app.editor.url.text(),
        "https://x/?a=1",
        "no {{options}} token — the insert must not run after NewVar failed"
    );
    assert!(
        !app.project.model.vars.contains_key("options"),
        "\"options\" must not be declared — it's a reserved name"
    );
    let saved = postui_core::project::load_variables(&app.project.root).unwrap();
    assert!(
        !saved.vars.contains_key("options"),
        "variables.toml on disk must be unchanged"
    );
}

#[test]
fn click_editor_tab_selects_it() {
    // Draw order is Params, Headers, Vars, Body — position 2 is Vars.
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app.hits.rect_of(&Hit::EditorTab(2)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.editor.active_tab, EditorTab::Vars);
    assert_eq!(app.focus, PaneId::Editor);

    // Position 3 (Body) still maps correctly too.
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app.hits.rect_of(&Hit::EditorTab(3)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    assert_eq!(app.focus, PaneId::Editor);
}

fn ready_response(app: &mut App, body: &str) {
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
            elapsed: std::time::Duration::from_millis(1),
            size: body.len(),
            content_type: Some("application/json".into()),
        })),
        app.session.send_generation,
    );
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
fn a_big_body_offers_the_tree_tab_while_its_parse_runs() {
    use crate::components::response::{SYNC_PRETTY_BYTES, ViewMode};
    let mut app = App::new_for_test();
    let body = format!("{{\"a\": \"{}\"}}", "x".repeat(SYNC_PRETTY_BYTES));
    ready_response(&mut app, &body);
    render_once(&mut app);
    assert!(app.session.response.view().unwrap().parsing);
    let tab = app
        .hits
        .rect_of(&Hit::ResponseTab(ViewMode::Pretty))
        .expect("the Tree tab is clickable while parsing");
    app.handle_mouse(left_down(tab.x, tab.y));
    assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Pretty);
}

#[test]
fn a_non_json_body_never_offers_the_tree_tab() {
    use crate::components::response::ViewMode;
    let mut app = App::new_for_test();
    ready_response(&mut app, "<html>hi</html>");
    render_once(&mut app);
    assert_eq!(app.hits.rect_of(&Hit::ResponseTab(ViewMode::Pretty)), None);
}

#[test]
fn the_search_button_opens_the_response_search() {
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": 1}"#);
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&Hit::ResponseSearchButton)
        .expect("⌕ button");
    app.handle_mouse(left_down(r.x, r.y));
    let search = app
        .session
        .response
        .view()
        .unwrap()
        .search
        .as_ref()
        .expect("search opened");
    assert!(search.active, "and it is taking typing, exactly as / does");
    assert_eq!(app.focus, PaneId::Response);
}

#[test]
fn the_search_step_buttons_cycle_the_matches() {
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": 1, "b": 1, "c": 1}"#);
    app.focus = PaneId::Response;
    let keymap = Keymap::default_bindings();
    for k in ['/', '1'] {
        app.handle_key(&keymap, plain(k));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    render_once(&mut app);
    let matches = app
        .session
        .response
        .view()
        .unwrap()
        .search
        .as_ref()
        .unwrap();
    assert_eq!(matches.matches.len(), 3, "one per value");
    assert_eq!(matches.current, 0);

    let next = app.hits.rect_of(&Hit::ResponseSearchNext).expect("▼");
    app.handle_mouse(left_down(next.x, next.y));
    assert_eq!(
        app.session
            .response
            .view()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .current,
        1
    );

    let prev = app.hits.rect_of(&Hit::ResponseSearchPrev).expect("▲");
    app.handle_mouse(left_down(prev.x, prev.y));
    app.handle_mouse(left_down(prev.x, prev.y));
    assert_eq!(
        app.session
            .response
            .view()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .current,
        2,
        "▲ wraps backwards past the first match"
    );
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
        state.items[2].action,
        Some(Action::SetMethod(postui_core::model::Method::Put))
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
    let i = palette_row_of(&app, "quit");
    render_once(&mut app);
    let row = app.hits.rect_of(&Hit::PaletteRow(i)).unwrap();
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
    assert!(content.contains("VARIABLES"), "the left list's own heading");
    assert!(content.contains("Environment:"), "{content}");
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

/// Regression test for the Task 9 review finding: the screen-routing
/// carve-out let ANY modified global shortcut through, not just
/// modal-opening ones — so ctrl+enter/ctrl+r sent the loaded request
/// invisibly while the Manager screen was open (its panes aren't even
/// drawn). Only `screen_escape_whitelist`'s small set of actions may
/// escape; `Send` must not be one of them.
#[test]
fn ctrl_r_and_ctrl_enter_do_not_send_from_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);
    assert!(app.toasts.is_empty());

    app.handle_key(&keymap, ctrl('r'));
    assert!(
        app.toasts.is_empty(),
        "ctrl+r must not reach Action::Send (an empty-URL send would toast)"
    );
    assert!(app.session.in_flight.is_none());
    assert_eq!(app.screen, crate::app::Screen::VarManager);

    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
    );
    assert!(
        app.toasts.is_empty(),
        "ctrl+enter must not reach Action::Send either"
    );
    assert!(app.session.in_flight.is_none());
    assert_eq!(app.screen, crate::app::Screen::VarManager);
}

/// Same finding: alt+u (`Action::FocusUrl`) must not silently reassign
/// `App::focus` while the Manager screen is open — that would corrupt the
/// focus `Action::CloseScreen` is supposed to restore.
#[test]
fn alt_u_does_not_move_focus_from_the_manager_screen() {
    let mut app = App::new_for_test();
    app.focus = PaneId::Response;
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);

    app.handle_key(&keymap, alt('u'));
    assert_eq!(
        app.focus,
        PaneId::Response,
        "alt+u must not reach Action::FocusUrl while the screen is open"
    );

    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.screen, crate::app::Screen::Main);
    assert_eq!(
        app.focus,
        PaneId::Response,
        "CloseScreen restores the untouched prior focus"
    );
}

/// Same finding: alt+o (`Action::CycleProject`) and alt+c
/// (`Action::CycleEnv`) — arbitrary other global shortcuts — must not
/// reach their actions from the Manager screen either.
#[test]
fn other_unwhitelisted_global_shortcuts_are_swallowed_by_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);
    assert!(app.toasts.is_empty());

    app.handle_key(&keymap, alt('o')); // CycleProject: would toast "only one project registered"
    assert!(
        app.toasts.is_empty(),
        "alt+o must not reach Action::CycleProject"
    );

    app.handle_key(&keymap, alt('c')); // CycleEnv: would toast "no environments — ..."
    assert!(
        app.toasts.is_empty(),
        "alt+c must not reach Action::CycleEnv"
    );
    assert_eq!(app.screen, crate::app::Screen::VarManager);
}

/// The whitelist's whole point: opening the palette on top of the Manager
/// screen must keep working via the same ctrl+p combo used on `Main`.
#[test]
fn ctrl_p_still_opens_the_palette_on_top_of_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::VarManager);

    app.handle_key(&keymap, ctrl('p'));
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    assert_eq!(
        app.screen,
        crate::app::Screen::VarManager,
        "opening the palette must not leave the screen"
    );
}

// --- Task 11: Manager navigation + in-place value editing (spec §5) -------

fn var_project(dir: &std::path::Path) {
    postui_core::project::init_project(dir, Some("demo")).unwrap();
    std::fs::write(
        dir.join("variables.toml"),
        r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[groups.user]
description = "acting user"
fields = ["user"]

[api_key]
description = "service key"
secret = true
"#,
    )
    .unwrap();
    std::fs::write(dir.join("environments/dev.toml"), "").unwrap();
    std::fs::write(
        dir.join("environments/qa.toml"),
        "base_url = \"https://qa.example.com\"\n\n[entries.user.alice]\nuser = \"1001\"\n\n[entries.user.bob]\nuser = \"2002\"\n",
    )
    .unwrap();
    postui_core::project::save_local_state(
        dir,
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn var_edit_set_env_value_writes_the_env_file_and_re_resolves() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    let redrew = app.update(Action::VarEdit(VarEditOp::SetEnvValue {
        env: "qa".into(),
        name: "base_url".into(),
        value: "https://qa2.example.com".into(),
    }));
    assert!(redrew);
    assert!(app.toasts.is_empty(), "a successful write must not toast");

    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("https://qa2.example.com"), "{on_disk}");
    assert_eq!(
        app.project.resolved.values["base_url"],
        "https://qa2.example.com"
    );
}

#[test]
fn var_edit_set_env_value_on_a_non_active_env_does_not_disturb_the_active_resolution() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.project.env_label(), "qa");

    app.update(Action::VarEdit(VarEditOp::SetEnvValue {
        env: "dev".into(),
        name: "base_url".into(),
        value: "http://dev.local".into(),
    }));

    let on_disk = std::fs::read_to_string(dir.path().join("environments/dev.toml")).unwrap();
    assert!(on_disk.contains("http://dev.local"), "{on_disk}");
    assert_eq!(
        app.project.resolved.values["base_url"], "https://qa.example.com",
        "qa is still active; its own resolution must be untouched"
    );
}

#[test]
fn var_edit_set_default_writes_variables_toml() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarEdit(VarEditOp::SetDefault {
        name: "base_url".into(),
        value: "http://localhost:9090".into(),
    }));

    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(on_disk.contains("http://localhost:9090"), "{on_disk}");
    assert_eq!(
        app.project.model.vars["base_url"].default.as_deref(),
        Some("http://localhost:9090")
    );
}

#[test]
fn var_edit_set_secret_value_lands_only_in_secrets_toml() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarEdit(VarEditOp::SetSecretValue {
        env: "qa".into(),
        name: "api_key".into(),
        value: "sk-live-abc123".into(),
    }));

    let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
    assert_eq!(secrets["qa"]["api_key"], "sk-live-abc123");
    assert_eq!(app.project.resolved.values["api_key"], "sk-live-abc123");

    let vars_on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(!vars_on_disk.contains("sk-live-abc123"));
    let env_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!env_on_disk.contains("sk-live-abc123"));
}

#[test]
fn var_edit_set_option_value_writes_one_field_of_the_entry_in_that_env() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarEdit(VarEditOp::SetOptionValue {
        env: "qa".into(),
        owner: "user".into(),
        key: "alice".into(),
        member: Some("user".into()),
        value: "9999".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let env_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_on_disk.contains("9999"), "{env_on_disk}");
    let vars_on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(
        !vars_on_disk.contains("9999"),
        "an entry value must never land in variables.toml: {vars_on_disk}"
    );
}

#[test]
fn var_edit_set_request_var_mutates_the_open_editor_and_marks_it_dirty_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let mut req = postui_core::model::HttpRequest::from_toml_str(
        "url = \"https://x/ping\"\n[variables]\ntrace_id = \"abc-123\"\n",
    )
    .unwrap();
    req.url = "https://x/ping".into();
    postui_core::storage::save_request(dir.path(), "ping", &req).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));
    assert!(!app.editor.is_dirty());

    app.update(Action::VarEdit(VarEditOp::SetRequestVar {
        name: "trace_id".into(),
        value: "trace-xyz".into(),
    }));

    assert_eq!(app.editor.variables["trace_id"].value, "trace-xyz");
    assert!(
        app.editor.is_dirty(),
        "the existing dirty/save path owns persistence"
    );
    let on_disk = std::fs::read_to_string(dir.path().join("requests/ping.toml")).unwrap();
    assert!(
        on_disk.contains("abc-123"),
        "no immediate write — still the saved value on disk: {on_disk}"
    );
}

#[test]
fn var_edit_select_records_the_choice_for_the_targeted_env_even_when_not_active() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.project.env_label(), "qa");

    app.update(Action::VarEdit(VarEditOp::Select {
        env: "dev".into(),
        name: "user".into(),
        key: "bob".into(),
    }));

    assert_eq!(app.project.selections_for("dev")["user"], "bob");
    assert!(
        !app.project.resolved.values.contains_key("user"),
        "qa (active) has no selection of its own; must be unaffected"
    );

    let state = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(state.selections["dev"]["user"], "bob");
}

#[test]
fn var_edit_a_failed_write_toasts_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    use std::os::unix::fs::PermissionsExt;
    let env_dir = dir.path().join("environments");
    let original_mode = std::fs::metadata(&env_dir).unwrap().permissions().mode();
    std::fs::set_permissions(&env_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.update(Action::VarEdit(VarEditOp::SetEnvValue {
            env: "qa".into(),
            name: "base_url".into(),
            value: "https://blocked.example.com".into(),
        }));
    }));

    // Always restore write permission before any assertion/panic unwinding,
    // so a failing assertion doesn't leave a read-only dir behind for the
    // TempDir's own Drop cleanup to choke on.
    std::fs::set_permissions(&env_dir, std::fs::Permissions::from_mode(original_mode)).unwrap();
    result.unwrap();

    assert!(
        !app.toasts.is_empty(),
        "a failed write must toast, not silently drop the edit"
    );
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!on_disk.contains("blocked"), "{on_disk}");
}

// --- Task 12: Manager structural actions (spec §5 action list; §4
// promote/demote; §3 secret-flag transitions) --------------------------

/// Opens the Manager and selects whichever left-list row matches `pred`,
/// panicking if none does — `rendered_text` first so `left_rows` is
/// populated (the list only rebuilds inside `draw`).
fn goto_row(app: &mut App, pred: impl Fn(&crate::components::varmanager::VmRow) -> bool) {
    app.update(Action::OpenVarManager);
    rendered_text(app);
    let i = app
        .varmanager
        .left_rows
        .iter()
        .position(pred)
        .expect("no row matched");
    app.varmanager.select_row(i);
}

#[test]
fn var_struct_new_var_creates_a_bare_declaration() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::NewVar {
        name: "widget_id".into(),
        description: None,
    }));

    assert!(app.toasts.is_empty());
    assert!(app.project.model.vars.contains_key("widget_id"));
    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(on_disk.contains("[widget_id]"), "{on_disk}");
}

#[test]
fn var_struct_new_var_rejects_a_name_already_taken() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::NewVar {
        name: "base_url".into(),
        description: None,
    }));

    assert!(!app.toasts.is_empty(), "must toast on collision");
}

#[test]
fn var_struct_new_group_creates_group_with_members() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::NewGroup {
        name: "creds".into(),
        fields: vec!["user_id".into(), "customer_id".into()],
    }));

    assert!(app.toasts.is_empty());
    let g = app
        .project
        .model
        .groups
        .get("creds")
        .expect("group created");
    assert_eq!(
        g.fields,
        vec!["user_id".to_string(), "customer_id".to_string()]
    );
}

#[test]
fn var_struct_new_entry_writes_every_field_into_the_active_env() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarStruct(VarStructOp::NewGroup {
        name: "creds".into(),
        fields: vec!["user_id".into(), "customer_id".into()],
    }));

    let mut values = indexmap::IndexMap::new();
    values.insert("user_id".to_string(), "1001".to_string());
    values.insert("customer_id".to_string(), "c-77".to_string());
    app.update(Action::VarStruct(VarStructOp::NewEntry {
        env: "qa".into(),
        group: "creds".into(),
        name: "alice".into(),
        description: None,
        values,
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let entry = postui_core::varmodel::group_entries(&app.project.env_data, "creds")
        .and_then(|entries| entries.get("alice"))
        .expect("entry created in the active env");
    assert_eq!(entry.values["user_id"], "1001");
    assert_eq!(entry.values["customer_id"], "c-77");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("[entries.creds.alice]"), "{on_disk}");
}

#[test]
fn var_struct_rename_updates_the_declaration() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::Rename {
        from: "base_url".into(),
        to: "root_url".into(),
    }));

    assert!(app.toasts.is_empty());
    assert!(!app.project.model.vars.contains_key("base_url"));
    assert!(app.project.model.vars.contains_key("root_url"));
}

/// Review finding: `rename_var` only ever touches `variables.toml`, so a
/// rename used to leave every environment's flat value/`[options.*]`
/// table under the OLD name — silently degrading to the declaration's
/// default post-rename, with no error and no warning. `shard` is simple
/// (a flat value) in `dev` and unset in `qa`, where an unrelated group's
/// entries table lives — the rename must follow the flat value across and
/// leave everything else alone.
#[test]
fn var_struct_rename_cascades_into_every_environments_flat_value() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("demo")).unwrap();
    std::fs::write(
        dir.path().join("variables.toml"),
        r#"
[shard]
description = "shard id"

[groups.tier]
fields = ["tier"]
"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/dev.toml"),
        "shard = \"d-1\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "[entries.tier.gold]\ntier = \"g-1\"\n",
    )
    .unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: Some("dev".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.project.resolved.values["shard"], "d-1");

    app.update(Action::VarStruct(VarStructOp::Rename {
        from: "shard".into(),
        to: "node".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(app.project.model.vars.contains_key("node"));
    assert!(!app.project.model.vars.contains_key("shard"));
    // Resolution follows the rename — still "d-1" in dev, now under "node".
    assert_eq!(app.project.resolved.values["node"], "d-1");
    assert!(!app.project.resolved.values.contains_key("shard"));

    let dev_on_disk = std::fs::read_to_string(dir.path().join("environments/dev.toml")).unwrap();
    assert!(dev_on_disk.contains("node = \"d-1\""), "{dev_on_disk}");
    assert!(!dev_on_disk.contains("shard"), "{dev_on_disk}");

    let qa_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        qa_on_disk.contains("[entries.tier.gold]"),
        "an unrelated group's entries stay untouched: {qa_on_disk}"
    );
}

#[test]
fn var_struct_delete_var_removes_the_declaration_and_clamps_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenVarManager);
    rendered_text(&mut app);
    app.varmanager.left_cursor = app.varmanager.left_rows.len() + 5;

    app.update(Action::VarStruct(VarStructOp::Delete {
        name: "base_url".into(),
    }));

    assert!(app.toasts.is_empty());
    assert!(!app.project.model.vars.contains_key("base_url"));
    assert!(
        app.varmanager.left_cursor < app.varmanager.left_rows.len(),
        "cursor must clamp back inside the (now shorter) row list"
    );
}

/// Finding 1: `VarStructOp::Delete` used to only edit `variables.toml`,
/// stranding any environment's entries table for the deleted name. This
/// drives the cascade for a group whose ACTIVE env has entries for it
/// (the confusing-parse-toast case) and a NON-active env too (the
/// silently-stranded-file case).
#[test]
fn var_struct_delete_cascades_into_every_environments_entries_table() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    std::fs::write(
        dir.path().join("variables.toml"),
        std::fs::read_to_string(dir.path().join("variables.toml")).unwrap()
            + "\n[groups.region]\ndescription = \"deploy region\"\nfields = [\"region\"]\n",
    )
    .unwrap();
    // qa is the active env (see var_project); entries for "region" there,
    // plus the same shape in the non-active "dev" env.
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "base_url = \"https://qa.example.com\"\n[entries.region.east]\nregion = \"us-east-1\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/dev.toml"),
        "[entries.region.west]\nregion = \"us-west-1\"\n",
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(
        app.project.resolved.meta["region"],
        postui_core::varmodel::VarMeta::NeedsSelection
    );

    app.update(Action::VarStruct(VarStructOp::Delete {
        name: "region".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(!app.project.model.groups.contains_key("region"));
    let qa_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        !qa_on_disk.contains("region"),
        "active env must be stripped: {qa_on_disk}"
    );
    let dev_on_disk = std::fs::read_to_string(dir.path().join("environments/dev.toml")).unwrap();
    assert!(
        !dev_on_disk.contains("region"),
        "non-active env must be stripped too: {dev_on_disk}"
    );
    // The project must still load clean after switching to the
    // previously-stranded env.
    let warns = app.project.set_env(Some("dev".into()));
    assert!(warns.is_empty(), "{warns:?}");
}

/// Finding 1, the group half: deleting a group must also strip every
/// environment's `[entries.<group>]` table.
#[test]
fn var_struct_delete_group_cascades_into_every_environments_entries_table() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    app_new_group(dir.path(), "creds", &["user_id", "customer_id"]);
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "base_url = \"https://qa.example.com\"\n\n[entries.user.alice]\nuser = \"1001\"\n\n[entries.creds.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-1\"\n",
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::Delete {
        name: "creds".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(!app.project.model.groups.contains_key("creds"));
    let qa_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!qa_on_disk.contains("creds"), "{qa_on_disk}");
}

/// Declares a variable-less group directly in `variables.toml` — a thin
/// helper so the delete-cascade test above doesn't need a full
/// `VarStructOp::NewGroup` round trip through a running `App`.
fn app_new_group(dir: &std::path::Path, name: &str, members: &[&str]) {
    let existing = std::fs::read_to_string(dir.join("variables.toml")).unwrap();
    let members_list = members
        .iter()
        .map(|m| format!("\"{m}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("variables.toml"),
        format!("{existing}\n[groups.{name}]\nfields = [{members_list}]\n"),
    )
    .unwrap();
}

#[test]
fn var_struct_set_fields_replaces_the_group_list() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarStruct(VarStructOp::NewGroup {
        name: "creds".into(),
        fields: vec!["user_id".into()],
    }));

    app.update(Action::VarStruct(VarStructOp::SetFields {
        group: "creds".into(),
        fields: vec!["user_id".into(), "customer_id".into()],
    }));

    assert_eq!(
        app.project.model.groups["creds"].fields,
        vec!["user_id".to_string(), "customer_id".to_string()]
    );
}

fn request_with_var(dir: &std::path::Path, slug: &str, name: &str, value: &str) {
    let mut r = postui_core::model::HttpRequest::from_toml_str(&format!(
        "url = \"https://x/{slug}\"\n[variables]\n{name} = \"{value}\"\n"
    ))
    .unwrap();
    r.url = format!("https://x/{slug}");
    postui_core::storage::save_request(dir, slug, &r).unwrap();
}

#[test]
fn var_struct_promote_to_default_writes_the_declaration_and_removes_the_request_entry() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    request_with_var(dir.path(), "ping", "trace_id", "abc-123");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));

    app.update(Action::VarStruct(VarStructOp::Promote {
        name: "trace_id".into(),
        target: postui_core::varedit::PromoteTarget::Default,
    }));

    assert!(app.toasts.is_empty());
    assert_eq!(
        app.project.model.vars["trace_id"].default.as_deref(),
        Some("abc-123")
    );
    assert!(!app.editor.variables.contains_key("trace_id"));
}

#[test]
fn var_struct_promote_to_env_writes_the_env_value_and_a_bare_declaration() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    request_with_var(dir.path(), "ping", "trace_id", "abc-123");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.project.env_label(), "qa");
    app.update(Action::ForceOpenRequest("ping".into()));

    app.update(Action::VarStruct(VarStructOp::Promote {
        name: "trace_id".into(),
        target: postui_core::varedit::PromoteTarget::Env,
    }));

    assert!(app.toasts.is_empty());
    assert!(app.project.model.vars.contains_key("trace_id"));
    assert!(app.project.model.vars["trace_id"].default.is_none());
    let env_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_on_disk.contains("abc-123"), "{env_on_disk}");
    assert!(!app.editor.variables.contains_key("trace_id"));
}

#[test]
fn var_struct_demote_writes_the_resolved_value_into_the_request_and_strips_the_project() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));
    assert_eq!(
        app.project.resolved.values["base_url"],
        "https://qa.example.com"
    );

    app.update(Action::VarStruct(VarStructOp::Demote {
        name: "base_url".into(),
    }));

    assert!(app.toasts.is_empty());
    assert_eq!(
        app.editor.variables["base_url"].value,
        "https://qa.example.com"
    );
    assert!(!app.project.model.vars.contains_key("base_url"));
    let env_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!env_on_disk.contains("qa.example.com"), "{env_on_disk}");
}

/// Finding 2: `apply_demote` used to leave the compensating request entry
/// only in the dirty editor buffer — demote, then quit without a manual
/// save, lost the value everywhere. The request file on disk must carry
/// it immediately, as part of the op itself.
#[test]
fn var_struct_demote_writes_the_request_file_to_disk_immediately() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));

    app.update(Action::VarStruct(VarStructOp::Demote {
        name: "base_url".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(
        !app.editor.is_dirty(),
        "the demote op must save, not just dirty, the editor"
    );
    let on_disk = postui_core::storage::load_request(dir.path(), "ping").unwrap();
    assert_eq!(
        on_disk.variables["base_url"].value, "https://qa.example.com",
        "the request file on disk must already carry the demoted value"
    );
}

/// Finding 2: `apply_promote`'s request-entry removal used to only exist
/// in the dirty editor buffer. The request file on disk must lose the
/// promoted entry immediately.
#[test]
fn var_struct_promote_removes_the_entry_from_the_request_file_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    request_with_var(dir.path(), "ping", "trace_id", "abc-123");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));

    app.update(Action::VarStruct(VarStructOp::Promote {
        name: "trace_id".into(),
        target: postui_core::varedit::PromoteTarget::Default,
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(!app.editor.is_dirty());
    let on_disk = postui_core::storage::load_request(dir.path(), "ping").unwrap();
    assert!(
        !on_disk.variables.contains_key("trace_id"),
        "the on-disk request file must no longer carry the promoted entry"
    );
}

/// Finding 2: the `Request` destination of extract-to-variable used to
/// only dirty-save the field it replaced, leaving the new `[variables]`
/// entry (and the field edit itself) unsaved to disk.
#[test]
fn extract_to_request_saves_the_request_file_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping/abc-123")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));
    app.editor.url = crate::components::line_input::LineInput::new("https://x/ping/abc-123");
    app.focus = crate::layout::PaneId::Editor;
    app.editor.sub_focus = crate::components::editor::SubFocus::Url;

    app.update(Action::ConfirmExtractVariable {
        name: "trace_id".into(),
        destination: crate::action::ExtractDestination::Request,
    });

    assert!(
        app.toasts
            .messages()
            .iter()
            .all(|m| !m.contains("could not")),
        "{:?}",
        app.toasts.messages()
    );
    assert!(!app.editor.is_dirty());
    let on_disk = postui_core::storage::load_request(dir.path(), "ping").unwrap();
    assert_eq!(
        on_disk.variables["trace_id"].value,
        "https://x/ping/abc-123"
    );
    assert_eq!(on_disk.url, "{{trace_id}}");
}

/// Review finding: `apply_demote` used to insert the demoted entry into
/// `editor.variables` BEFORE the fallible `delete_var` write, so a
/// `delete_var` failure left a demoted entry live in the editor while the
/// project still held the declaration, violating `apply_var_struct`'s
/// documented "Err leaves everything unchanged" contract. This drives
/// exactly that failure path, with a name that resolves (an undeclared
/// environment value passes through — spec §3.2's leniency) but has no
/// declaration for `delete_var` to remove.
#[test]
fn demote_leaves_the_editor_untouched_when_the_project_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("demo")).unwrap();
    std::fs::write(dir.path().join("variables.toml"), "").unwrap();
    std::fs::write(
        dir.path().join("environments/dev.toml"),
        "shard = \"picked-value\"\n",
    )
    .unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: Some("dev".into()),
            ..Default::default()
        },
    )
    .unwrap();
    postui_core::storage::save_request(dir.path(), "r", &req("https://x/r")).unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("r".into()));
    assert_eq!(
        app.project.resolved.values.get("shard"),
        Some(&"picked-value".to_string()),
        "shard must resolve so apply_demote reaches delete_var"
    );

    app.update(Action::VarStruct(VarStructOp::Demote {
        name: "shard".into(),
    }));

    assert!(!app.toasts.is_empty(), "delete_var's failure must toast");
    assert!(
        !app.editor.variables.contains_key("shard"),
        "the editor must NOT gain a demoted entry when the project write failed"
    );
    let dev_on_disk = std::fs::read_to_string(dir.path().join("environments/dev.toml")).unwrap();
    assert!(
        dev_on_disk.contains("picked-value"),
        "the env value must be untouched: {dev_on_disk}"
    );
}

// -------------------------------------------------------------
// Finding 3: option delete
// -------------------------------------------------------------

#[test]
fn delete_entry_removes_it_from_the_active_env() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::DeleteEntry {
        env: "qa".into(),
        group: "user".into(),
        name: "bob".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let entries = postui_core::varmodel::group_entries(&app.project.env_data, "user")
        .expect("the group still has entries here");
    assert!(!entries.contains_key("bob"));
    assert!(entries.contains_key("alice"), "the others are untouched");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!on_disk.contains("[entries.user.bob]"), "{on_disk}");
}

#[test]
fn delete_entry_that_is_already_gone_is_a_quiet_no_op() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::DeleteEntry {
        env: "qa".into(),
        group: "user".into(),
        name: "carol".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
}

#[test]
fn delete_entry_clears_the_selection_when_the_deleted_entry_was_selected() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.project.set_selection("user", "alice");
    assert_eq!(app.project.resolved.values["user"], "1001");

    app.update(Action::VarStruct(VarStructOp::DeleteEntry {
        env: "qa".into(),
        group: "user".into(),
        name: "alice".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(!app.project.selections_for("qa").contains_key("user"));
    assert!(!app.project.resolved.values.contains_key("user"));
}

// -------------------------------------------------------------
// Finding 7: rename usage-scan parity
// -------------------------------------------------------------

#[test]
fn prompt_rename_var_surfaces_scan_usage_count_like_delete_does() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let mut r = req("https://x/uses-it/{{base_url}}");
    r.url = "https://x/uses-it/{{base_url}}".into();
    postui_core::storage::save_request(dir.path(), "uses-it", &r).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::PromptRenameVar {
        from: "base_url".into(),
    });

    let Some(Modal::Prompt { title, .. }) = app.modals.top() else {
        panic!("expected a Prompt modal");
    };
    assert!(
        title.contains("uses-it") && title.contains('1'),
        "the rename prompt must name the referencing request: {title}"
    );
}

#[test]
fn prompt_rename_var_with_no_usage_has_a_plain_title() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::PromptRenameVar {
        from: "base_url".into(),
    });

    let Some(Modal::Prompt { title, .. }) = app.modals.top() else {
        panic!("expected a Prompt modal");
    };
    assert_eq!(title, "Rename base_url");
}

// -------------------------------------------------------------
// Finding 8: promote-onto-secret refusal
// -------------------------------------------------------------

#[test]
fn prompt_promote_var_onto_an_existing_secret_name_refuses_with_a_message_modal() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    // `api_key` (var_project) is already `secret = true`; a request-scope
    // entry of the SAME name is exactly the promote-onto-secret case.
    request_with_var(dir.path(), "ping", "api_key", "sk-oops");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));

    app.update(Action::PromptPromoteVar {
        name: "api_key".into(),
    });

    assert!(
        matches!(app.modals.top(), Some(Modal::Message { .. })),
        "promoting onto a secret name must be refused with a message modal, not a raw parse error"
    );
    assert!(
        app.project.model.vars["api_key"].secret,
        "the declaration must be untouched"
    );
    assert!(app.editor.variables.contains_key("api_key"));
}

#[test]
fn confirm_delete_var_lists_referencing_requests_from_scan_usage() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let mut r = req("https://x/uses-it/{{base_url}}");
    r.url = "https://x/uses-it/{{base_url}}".into();
    postui_core::storage::save_request(dir.path(), "uses-it", &r).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::ConfirmDeleteVar {
        name: "base_url".into(),
    });

    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("expected a Confirm modal");
    };
    assert!(
        body.contains("uses-it") && body.contains('1'),
        "body must name the referencing request: {body}"
    );
}

#[test]
fn confirm_demote_var_on_a_group_refuses_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));

    app.update(Action::ConfirmDemoteVar {
        name: "user".into(),
    });

    assert!(
        matches!(app.modals.top(), Some(Modal::Message { .. })),
        "a group must be refused with a message modal"
    );
    assert!(
        app.project.model.groups.contains_key("user"),
        "the declaration must be untouched"
    );
    assert!(!app.editor.variables.contains_key("user"));
}

#[test]
fn confirm_demote_var_on_a_secret_variable_refuses() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));

    app.update(Action::ConfirmDemoteVar {
        name: "api_key".into(),
    });

    assert!(
        matches!(app.modals.top(), Some(Modal::Message { .. })),
        "a secret variable must be refused, never written into a request file"
    );
    assert!(app.project.model.vars.contains_key("api_key"));
}

#[test]
fn toggle_secret_var_secret_to_nonsecret_leaves_secrets_toml_untouched_and_shows_copy_offer() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarEdit(VarEditOp::SetSecretValue {
        env: "qa".into(),
        name: "api_key".into(),
        value: "sk-live-abc123".into(),
    }));
    let secrets_before = std::fs::read_to_string(dir.path().join(".local/secrets.toml")).unwrap();

    app.update(Action::ToggleSecretVar {
        name: "api_key".into(),
    });

    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("expected a Confirm modal");
    };
    assert!(
        body.contains("sk-live-abc123"),
        "the copy-offer modal must show the value: {body}"
    );
    let secrets_after = std::fs::read_to_string(dir.path().join(".local/secrets.toml")).unwrap();
    assert_eq!(
        secrets_before, secrets_after,
        "opening the confirm must not touch secrets.toml yet"
    );

    // Confirming flips the flag but still moves nothing (spec §3).
    app.update(Action::VarStruct(VarStructOp::ToggleSecret {
        name: "api_key".into(),
    }));
    assert!(!app.project.model.vars["api_key"].secret);
    let secrets_final = std::fs::read_to_string(dir.path().join(".local/secrets.toml")).unwrap();
    assert_eq!(
        secrets_before, secrets_final,
        "the value stays where it was"
    );
}

#[test]
fn toggle_secret_var_nonsecret_to_secret_moves_env_values_and_strips_env_files() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    // dev has no value for base_url; qa does — only qa's should move.
    app.update(Action::ToggleSecretVar {
        name: "base_url".into(),
    });
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));

    app.update(Action::VarStruct(VarStructOp::ToggleSecret {
        name: "base_url".into(),
    }));

    assert!(app.toasts.is_empty());
    assert!(app.project.model.vars["base_url"].secret);
    let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
    assert_eq!(secrets["qa"]["base_url"], "https://qa.example.com");
    let qa_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        !qa_on_disk.contains("qa.example.com"),
        "the env file must be stripped: {qa_on_disk}"
    );
}

#[test]
fn toggle_secret_is_refused_for_a_group() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::ToggleSecretVar {
        name: "user".into(),
    });

    assert!(app.modals.is_empty(), "no modal for an invalid target");
    assert!(!app.toasts.is_empty(), "must toast why it's refused");
}

// -- every structural op is reachable both by key and by a painted chip --

#[test]
fn keyboard_n_and_g_open_the_new_var_and_new_group_prompts() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    rendered_text(&mut app);

    app.handle_key(&keymap, plain('n'));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::NewVariable,
            ..
        })
    ));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    app.handle_key(&keymap, plain('g'));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::MultiPrompt {
            kind: PromptKind::NewGroup,
            ..
        })
    ));
}

#[test]
fn keyboard_f2_d_s_open_the_matching_var_row_actions() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    app.handle_key(&keymap, KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::RenameVariable { .. },
            ..
        })
    ));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    app.handle_key(&keymap, plain('d'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.project.model.vars.contains_key("base_url"), "cancelled");

    app.handle_key(&keymap, plain('s'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

/// Mouse/keyboard parity (spec §5: "every mutation ... has a keyboard
/// action and a painted button"): the left list's context-menu "Delete…"
/// opens the exact same confirm the `d` key does.
#[test]
fn the_context_menu_delete_opens_the_same_confirm_as_the_d_key() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    let via_menu = app
        .varmanager
        .context_menu(app.varmanager.left_cursor)
        .expect("menu for a variable row");
    app.update(via_menu[2].action.clone().unwrap());
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.update(Action::Close);

    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('d'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
}

#[test]
fn clicking_the_new_variable_button_opens_the_new_variable_prompt() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenVarManager);
    rendered_text(&mut app);

    let rect = app
        .hits
        .rect_of(&crate::hit::Hit::VmNewVar)
        .expect("+ Variable button must be painted");
    assert!(app.handle_mouse(left_down(rect.x + 1, rect.y + 1)));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::NewVariable,
            ..
        })
    ));

    // …and the `+ Group` button opens the group prompt.
    app.update(Action::Close);
    rendered_text(&mut app);
    let rect = app
        .hits
        .rect_of(&crate::hit::Hit::VmNewGroup)
        .expect("+ Group button must be painted");
    assert!(app.handle_mouse(left_down(rect.x + 1, rect.y + 1)));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::MultiPrompt {
            kind: PromptKind::NewGroup,
            ..
        })
    ));
}

#[test]
fn prompt_new_group_takes_a_name_and_a_field_list() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewGroup);

    for c in "creds".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for c in "user_id, customer_id".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.modals.is_empty());
    let g = app
        .project
        .model
        .groups
        .get("creds")
        .expect("group created");
    assert_eq!(
        g.fields,
        vec!["user_id".to_string(), "customer_id".to_string()]
    );
    // and the empty group survives a reload (parse accepts fields = [])
    app.update(Action::ReloadProjectFiles);
    assert!(app.project.model.groups.contains_key("creds"));
}

#[test]
fn add_and_remove_group_members_one_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::VarStruct(VarStructOp::NewGroup {
        name: "creds".into(),
        fields: vec![],
    }));

    // `a` flow: one member name per prompt, appended in order
    for member in ["user_id", "customer_id"] {
        app.update(Action::PromptAddGroupMember {
            group: "creds".into(),
        });
        for c in member.chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }
    assert_eq!(
        app.project.model.groups.get("creds").unwrap().fields,
        vec!["user_id".to_string(), "customer_id".to_string()]
    );

    // duplicate append toasts and changes nothing
    let toasts_before = app.toasts.messages().len();
    app.update(Action::PromptAddGroupMember {
        group: "creds".into(),
    });
    for c in "user_id".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.toasts.messages().len() > toasts_before);
    assert_eq!(
        app.project.model.groups.get("creds").unwrap().fields.len(),
        2
    );

    // `d` flow: confirm-remove one member
    app.update(Action::ConfirmRemoveGroupMember {
        group: "creds".into(),
        member: "user_id".into(),
    });
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "removal asks first"
    );
    app.handle_key(&keymap, plain('y'));
    assert_eq!(
        app.project.model.groups.get("creds").unwrap().fields,
        vec!["customer_id".to_string()]
    );
}

// --- Task 14: selection-context picker ---------------------------------

fn group_project(dir: &std::path::Path) {
    postui_core::project::init_project(dir, Some("demo")).unwrap();
    std::fs::write(
        dir.join("variables.toml"),
        r#"
[groups.identity]
description = "identity"
fields = ["user_id", "customer_id"]
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("environments/qa.toml"),
        r#"
[entries.identity.alice]
description = "admin"
user_id = "1001"
customer_id = "c-77"

[entries.identity.bob]
description = "reader"
user_id = "1002"
customer_id = "c-78"
"#,
    )
    .unwrap();
    postui_core::project::save_local_state(
        dir,
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            ..Default::default()
        },
    )
    .unwrap();
}

/// Places `app.editor.url` at `url` with the caret inside the first
/// occurrence of `token` (used to land the caret on a `{{name}}`) and
/// focuses it, as if the user had clicked/typed their way there.
fn focus_url_with_cursor_on(app: &mut App, url: &str, token: &str) {
    let mid = url.find(token).unwrap() + token.len() / 2;
    let mut input = crate::components::line_input::LineInput::new(url);
    input.set_cursor(mid);
    app.editor.url = input;
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
}

#[test]
fn ctrl_v_on_a_one_field_groups_token_opens_select_option_with_checkmark() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.project.set_selection_for("qa", "user", "alice");

    focus_url_with_cursor_on(&mut app, "https://x/{{user}}", "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });

    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("expected the selection-context picker to open")
    };
    assert_eq!(
        p.mode,
        crate::components::var_picker::PickerMode::SelectOption {
            name: "user".into(),
            group: "user".into(),
        }
    );

    let content = rendered_text(&mut app);
    assert!(content.contains("alice"), "{content}");
    assert!(content.contains("bob"), "{content}");
    assert!(content.contains("1001"), "{content}");
    assert!(
        content.contains("\u{2713}"),
        "current pick is checked: {content}"
    );
}

#[test]
fn ctrl_v_on_group_member_token_shows_the_group_s_options_with_full_preview() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    focus_url_with_cursor_on(&mut app, "https://x/{{user_id}}", "{{user_id}}");
    app.update(Action::OpenVarPicker { completing: false });

    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("expected the selection-context picker to open")
    };
    assert_eq!(
        p.mode,
        crate::components::var_picker::PickerMode::SelectOption {
            name: "user_id".into(),
            group: "identity".into(),
        }
    );

    let content = rendered_text(&mut app);
    assert!(content.contains("alice"), "{content}");
    assert!(content.contains("admin"), "{content}");
    assert!(content.contains("user_id 1001"), "{content}");
    assert!(content.contains("customer_id c-77"), "{content}");
}

#[test]
fn select_option_enter_writes_selection_to_state_toml_and_leaves_url_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    let url = "https://x/{{user}}";
    focus_url_with_cursor_on(&mut app, url, "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.modals.is_empty());
    assert_eq!(app.editor.url.text(), url, "token text must be untouched");
    assert_eq!(app.project.selections_for("qa")["user"], "alice");

    let state = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(state.selections["qa"]["user"], "alice");

    assert!(
        rendered_text(&mut app).contains("user \u{2192} alice (qa)"),
        "confirm toasts the selection"
    );
}

#[test]
fn select_option_typing_filters_rows() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    focus_url_with_cursor_on(&mut app, "https://x/{{user}}", "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });
    for c in "bob".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.project.selections_for("qa")["user"], "bob");
}

#[test]
fn blocked_send_toast_names_first_needs_selection_var_with_a_ctrl_v_hint() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "r", &req("https://x/{{user}}")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("r".into()));

    app.update(Action::ForceSend);

    assert!(app.session.in_flight.is_none());
    let content = rendered_text(&mut app);
    assert!(content.contains("need a selection"), "{content}");
    assert!(content.contains("press ctrl+v to select user"), "{content}");
}

// -- Task 16: send-time secret prompt chain (spec §3) --

/// A project with two secrets (`api_key` < `api_secret` alphabetically —
/// `BTreeMap` iteration order in `PrepareError::Unresolved`) wired into
/// default headers, so a real send exercises the substituted value.
fn two_secret_project(dir: &std::path::Path) {
    postui_core::project::init_project(dir, Some("svc")).unwrap();
    std::fs::write(
        dir.join("project.toml"),
        "name = \"svc\"\n[default_headers]\nx-api-key = \"{{api_key}}\"\nx-api-secret = \"{{api_secret}}\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("variables.toml"),
        "[api_key]\nsecret = true\n[api_secret]\nsecret = true\n",
    )
    .unwrap();
    std::fs::write(dir.join("environments/qa.toml"), "").unwrap();
}

async fn drain_until_settled(app: &mut App, rx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>) {
    let generation = app.session.send_generation;
    loop {
        let action = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for a send result")
            .expect("channel closed before a result arrived");
        let settled = matches!(
            &action,
            Action::ResponseArrived { generation: g, .. } | Action::RequestFailed { generation: g, .. }
            if *g == generation
        );
        app.update(action);
        if settled {
            break;
        }
    }
}

fn type_and_confirm(app: &mut App, keymap: &Keymap, text: &str) {
    for c in text.chars() {
        app.handle_key(keymap, plain(c));
    }
    app.handle_key(keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

#[tokio::test]
async fn missing_secrets_prompt_sequentially_then_the_request_sends() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/x"))
        .and(wiremock::matchers::header("x-api-key", "key-val"))
        .and(wiremock::matchers::header("x-api-secret", "secret-val"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    two_secret_project(dir.path());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.editor.url = LineInput::new(&format!("{}/x", server.uri()));
    let keymap = Keymap::default_bindings();

    // First send attempt: blocked, prompting for the alphabetically first
    // missing secret — never api_secret first.
    app.update(Action::ForceSend);
    assert!(app.session.in_flight.is_none());
    let Some(Modal::Prompt { title, kind, .. }) = app.modals.top() else {
        panic!("expected a secret prompt");
    };
    assert!(title.contains("api_key"), "title: {title}");
    assert!(title.contains("qa"), "title: {title}");
    assert!(
        !title.contains("key-val"),
        "title must never carry a value: {title}"
    );
    assert!(
        matches!(kind, PromptKind::SecretValue { name, env } if name == "api_key" && env == "qa")
    );

    type_and_confirm(&mut app, &keymap, "key-val");

    // Still not sent — the second secret is missing too.
    assert!(app.session.in_flight.is_none());
    let Some(Modal::Prompt { title, kind, .. }) = app.modals.top() else {
        panic!("expected the second secret prompt");
    };
    assert!(title.contains("api_secret"), "title: {title}");
    assert!(matches!(kind, PromptKind::SecretValue { name, .. } if name == "api_secret"));

    type_and_confirm(&mut app, &keymap, "secret-val");

    // Both secrets resolved: the send actually goes out this time.
    assert!(app.modals.is_empty());
    assert!(app.session.in_flight.is_some());

    drain_until_settled(&mut app, &mut rx).await;
    match app.session.response.state() {
        ResponseState::Ready(data) => assert_eq!(data.status, 200),
        _ => panic!("expected a ready response"),
    }

    let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
    assert_eq!(secrets["qa"]["api_key"], "key-val");
    assert_eq!(secrets["qa"]["api_secret"], "secret-val");
}

#[tokio::test]
async fn esc_mid_chain_cancels_the_send_and_keeps_only_confirmed_secrets() {
    let dir = tempfile::tempdir().unwrap();
    two_secret_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.editor.url = LineInput::new("http://example.invalid/x");
    let keymap = Keymap::default_bindings();

    app.update(Action::ForceSend);
    type_and_confirm(&mut app, &keymap, "key-val");

    // Second prompt (api_secret) is open now; cancel it.
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::SecretValue { name, .. },
            ..
        }) if name == "api_secret"
    ));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(app.modals.is_empty(), "esc closes the prompt");
    assert!(app.session.in_flight.is_none(), "nothing was sent");
    let content = rendered_text(&mut app);
    assert!(content.contains("send canceled"), "{content}");

    let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
    assert_eq!(secrets["qa"]["api_key"], "key-val");
    assert!(
        !secrets["qa"].contains_key("api_secret"),
        "the cancelled secret must not be persisted: {:?}",
        secrets["qa"]
    );
}

#[test]
fn secret_prompt_input_renders_masked_dots_not_the_typed_text() {
    let dir = tempfile::tempdir().unwrap();
    two_secret_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.editor.url = LineInput::new("http://example.invalid/x");
    let keymap = Keymap::default_bindings();

    app.update(Action::ForceSend);
    let typed = "zqxvw9";
    for c in typed.chars() {
        app.handle_key(&keymap, plain(c));
    }

    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("expected a secret prompt");
    };
    assert_eq!(
        input.text(),
        typed,
        "the buffer itself holds the typed text"
    );

    let content = rendered_text(&mut app);
    assert!(
        !content.contains(typed),
        "typed text must never render: {content}"
    );
    assert!(
        content.contains('\u{25cf}'),
        "masked dots must render: {content}"
    );
}

// -- Task 17: in-context flows (spec §6) --------------------------------

fn tab_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
}

fn enter_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}

/// Types `text` into whichever `Modal::MultiPrompt` field currently has
/// focus, without confirming.
fn type_into_field(app: &mut App, keymap: &Keymap, text: &str) {
    for c in text.chars() {
        app.handle_key(keymap, plain(c));
    }
}

#[test]
fn add_new_entry_writes_to_the_active_envs_entries_table_selects_it_and_restores_focus() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    let url = "https://x/{{user}}";
    focus_url_with_cursor_on(&mut app, url, "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });

    // "user" has two entries (alice, bob); the ghost "add new option…" row
    // sits one past them.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, enter_key());

    let Some(Modal::MultiPrompt { kind, .. }) = app.modals.top() else {
        panic!("expected the new-option-inline multi-prompt");
    };
    assert!(matches!(kind, PromptKind::NewOptionInline { owner } if owner == "user"));

    type_into_field(&mut app, &keymap, "carol");
    app.handle_key(&keymap, tab_key());
    type_into_field(&mut app, &keymap, "3003");
    app.handle_key(&keymap, tab_key());
    type_into_field(&mut app, &keymap, "temp hire");
    app.handle_key(&keymap, enter_key());

    assert!(app.modals.is_empty(), "closes back to the field");
    assert_eq!(app.focus, PaneId::Editor, "focus restored to where it was");
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
    assert_eq!(app.editor.url.text(), url, "the token text is untouched");

    // Written to the ACTIVE ENV's entries table — entries only ever live
    // in an environment file (spec §3.1).
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("[entries.user.carol]"), "{env_doc}");
    assert!(env_doc.contains("3003"), "{env_doc}");
    assert!(env_doc.contains("temp hire"), "{env_doc}");

    let shared_doc = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(
        !shared_doc.contains("carol"),
        "must not land in variables.toml: {shared_doc}"
    );

    assert_eq!(app.project.selections_for("qa")["user"], "carol");
}

#[test]
fn e_on_an_entry_edits_it_in_the_environment_that_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    focus_url_with_cursor_on(&mut app, "https://x/{{user}}", "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });
    // Row 0 is "alice" (first entry, file order).
    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
    );

    let Some(Modal::MultiPrompt { kind, fields, .. }) = app.modals.top() else {
        panic!("expected the edit-option multi-prompt");
    };
    assert!(
        matches!(kind, PromptKind::EditOption { owner, key } if owner == "user" && key == "alice")
    );
    assert_eq!(
        fields[0].input.text(),
        "1001",
        "prefilled with the current value"
    );

    for _ in 0..4 {
        app.handle_key(
            &keymap,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
    }
    type_into_field(&mut app, &keymap, "9999");
    app.handle_key(&keymap, enter_key());

    assert!(app.modals.is_empty());
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("9999"), "{env_doc}");
    let shared_doc = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(
        !shared_doc.contains("9999"),
        "an entry's edit must never touch variables.toml: {shared_doc}"
    );
}

/// Selects header row 0, begins editing its key, and Tabs into the value
/// cell — the state `focused_field_text`/extract requires (a table cell
/// under edit).
fn focus_header_value_cell(app: &mut App) {
    app.editor.active_tab = EditorTab::Headers;
    app.editor.sub_focus = SubFocus::Content;
    app.focus = PaneId::Editor;
    app.editor.table.selected = Some(0);
    let map = app.editor.headers.clone();
    app.editor.table.begin_edit_selected(&map);
    app.editor
        .table
        .handle_key(tab_key(), &mut app.editor.headers);
}

#[test]
fn extract_to_variable_prompts_writes_and_replaces_field_text_dirty_saved() {
    let mut app = App::new_for_test();
    app.editor.headers.insert(
        "x-api-key".into(),
        postui_core::model::Entry {
            value: "abc123".into(),
            enabled: true,
        },
    );
    app.editor.mark_saved();
    let keymap = Keymap::default_bindings();
    focus_header_value_cell(&mut app);
    assert_eq!(
        app.editor.table.editing.as_ref().unwrap().input.text(),
        "abc123"
    );

    app.update(Action::ExtractToVariable);
    let Some(Modal::MultiPrompt { kind, .. }) = app.modals.top() else {
        panic!("expected the extract-variable multi-prompt");
    };
    assert!(matches!(kind, PromptKind::ExtractVariable));

    type_into_field(&mut app, &keymap, "api_key");
    app.handle_key(&keymap, enter_key());

    assert!(app.modals.is_empty());
    let content = rendered_text(&mut app);
    assert!(content.contains("extracted to {{api_key}}"), "{content}");

    assert_eq!(
        app.editor.headers["x-api-key"].value, "{{api_key}}",
        "the field is committed into the map, not left as a pending edit"
    );
    assert!(
        app.editor.table.editing.is_none(),
        "the cell edit is committed, not left open"
    );
    assert!(app.editor.is_dirty(), "the field is dirty-saved");

    let doc = std::fs::read_to_string(app.project.root.join("variables.toml")).unwrap();
    assert!(doc.contains("[api_key]"), "{doc}");
    assert!(doc.contains("abc123"), "{doc}");
}

/// MINOR 9 (Task 18 review, sweep finding left inconclusive): a table cell
/// genuinely in editing state, then the palette opened and "Extract to
/// variable" run through it (not `Action::ExtractToVariable` dispatched
/// directly) — the palette modal stacks on top of the still-open cell
/// edit, and confirming it must reach `focused_field_text`'s table-cell
/// branch and open the extract prompt, exactly like the direct-action test
/// above.
#[test]
fn palette_extract_to_variable_with_a_table_cell_genuinely_in_edit_opens_the_prompt() {
    let mut app = App::new_for_test();
    app.editor.headers.insert(
        "x-api-key".into(),
        postui_core::model::Entry {
            value: "abc123".into(),
            enabled: true,
        },
    );
    app.editor.mark_saved();
    let keymap = Keymap::default_bindings();
    focus_header_value_cell(&mut app);
    assert!(
        app.editor.table.editing.is_some(),
        "the cell must genuinely be in edit before the palette opens"
    );

    app.update(Action::OpenPalette);
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    for c in "Extract to variable".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, enter_key());

    let Some(Modal::MultiPrompt { kind, .. }) = app.modals.top() else {
        panic!("expected the extract-variable multi-prompt to open");
    };
    assert!(matches!(kind, PromptKind::ExtractVariable));
    // The table cell edit must still be intact underneath — the palette
    // command didn't disturb it, only read through it.
    assert_eq!(
        app.editor.table.editing.as_ref().unwrap().input.text(),
        "abc123"
    );
}

#[test]
fn extract_to_variable_with_cursor_in_the_body_is_refused_with_a_toast() {
    let mut app = App::new_for_test();
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;

    app.update(Action::ExtractToVariable);

    assert!(app.modals.is_empty());
    let content = rendered_text(&mut app);
    assert!(content.contains("body"), "{content}");
}

#[test]
fn extract_to_variable_with_no_focused_field_is_refused_with_a_toast() {
    let mut app = App::new_for_test();
    app.focus = PaneId::Sidebar;

    app.update(Action::ExtractToVariable);

    assert!(app.modals.is_empty());
    let content = rendered_text(&mut app);
    assert!(content.contains("focus a text field"), "{content}");
}

// -- Task 17 review fix: ActiveEnv/Request destinations + guards --------

fn right_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)
}

/// Drives the extract flow through the palette-equivalent `Action` and the
/// live `Modal::MultiPrompt`: focuses `url` (with the whole text as the
/// literal to extract), opens the prompt, types `name`, cycles the
/// destination choice field right `rights` times (0 = Project default, 1 =
/// Active env value, 2 = This request), and confirms.
fn extract_url(app: &mut App, keymap: &Keymap, url: &str, name: &str, rights: u8) {
    app.editor.url = LineInput::new(url);
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;

    app.update(Action::ExtractToVariable);
    assert!(
        matches!(app.modals.top(), Some(Modal::MultiPrompt { .. })),
        "expected the extract-variable multi-prompt to open"
    );
    type_into_field(app, keymap, name);
    app.handle_key(keymap, tab_key());
    for _ in 0..rights {
        app.handle_key(keymap, right_key());
    }
    app.handle_key(keymap, enter_key());
}

#[test]
fn extract_to_active_env_writes_the_flat_pair_and_a_bare_declaration_and_replaces_the_field() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    extract_url(
        &mut app,
        &keymap,
        "https://x/token-abc123",
        "session_token",
        1, // Active env value
    );

    assert!(app.modals.is_empty());
    assert_eq!(app.editor.url.text(), "{{session_token}}");

    let shared_doc = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    let session_token_block = shared_doc
        .split_once("[session_token]")
        .expect("session_token declared")
        .1;
    assert!(
        !session_token_block.contains("default"),
        "ActiveEnv must declare bare (no shared default): {shared_doc}"
    );

    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        env_doc.contains("session_token = \"https://x/token-abc123\""),
        "{env_doc}"
    );

    assert_eq!(
        app.project.resolved.values["session_token"],
        "https://x/token-abc123"
    );
}

#[test]
fn extract_to_request_writes_editor_variables_and_dirty_saves() {
    let mut app = App::new_for_test();
    app.editor.mark_saved();
    let keymap = Keymap::default_bindings();

    extract_url(
        &mut app,
        &keymap,
        "https://x/inline-value",
        "inline_var",
        2, // This request
    );

    assert!(app.modals.is_empty());
    assert_eq!(app.editor.url.text(), "{{inline_var}}");
    assert_eq!(
        app.editor.variables["inline_var"].value,
        "https://x/inline-value"
    );
    assert!(app.editor.variables["inline_var"].enabled);
    assert!(app.editor.is_dirty(), "the field is dirty-saved");
}

#[test]
fn extract_to_active_env_refuses_a_name_colliding_with_an_existing_group() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    let shared_before = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    let env_before = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();

    extract_url(
        &mut app,
        &keymap,
        "https://x/whatever",
        "identity", // collides with the declared group
        1,          // Active env value
    );

    assert!(app.modals.is_empty());
    let content = rendered_text(&mut app);
    assert!(content.contains("already exists"), "{content}");
    assert_eq!(
        app.editor.url.text(),
        "https://x/whatever",
        "a refused extract must not touch the field"
    );

    let shared_after = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    let env_after = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert_eq!(shared_before, shared_after, "no file write on refusal");
    assert_eq!(env_before, env_after, "no file write on refusal");
}

#[test]
fn extract_to_active_env_refuses_a_name_colliding_with_an_existing_secret() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    let shared_before = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    let env_before = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();

    extract_url(
        &mut app,
        &keymap,
        "https://x/whatever",
        "api_key", // declared secret in var_project
        1,         // Active env value
    );

    assert!(app.modals.is_empty());
    let content = rendered_text(&mut app);
    assert!(content.contains("secret"), "{content}");
    assert_eq!(
        app.editor.url.text(),
        "https://x/whatever",
        "a refused extract must not touch the field"
    );

    let shared_after = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    let env_after = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert_eq!(
        shared_before, shared_after,
        "no file write on refusal (must never persist a flat value for a secret)"
    );
    assert_eq!(env_before, env_after, "no file write on refusal");
}

#[test]
fn confirm_edit_option_writes_every_field_into_the_active_envs_entry() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    let mut values = indexmap::IndexMap::new();
    values.insert("user_id".to_string(), "9001".to_string());
    values.insert("customer_id".to_string(), "c-101".to_string());
    app.update(Action::ConfirmEditOption {
        owner: "identity".into(),
        key: "alice".into(),
        values,
        description: Some("admin updated".into()),
    });

    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("9001"), "{env_doc}");
    assert!(env_doc.contains("c-101"), "{env_doc}");
    assert!(env_doc.contains("admin updated"), "{env_doc}");

    let shared_doc = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(
        !shared_doc.contains("9001"),
        "an entry edit must never touch variables.toml: {shared_doc}"
    );
}

// -- Stage 7: the variable-format migration prompt (spec §3.3) ----------

/// A project written with stage-6 syntax: an enumerated variable, a group
/// with `members`, and one environment carrying a keyed `[options.*]`
/// override.
fn legacy_project(dir: &std::path::Path) {
    postui_core::project::init_project(dir, Some("legacy")).unwrap();
    std::fs::write(
        dir.join("variables.toml"),
        r#"
[base_url]
default = "http://localhost:8080"

[tier]
description = "pricing tier"
[tier.options.gold]
value = "g-1"

[groups.user]
members = ["user_id", "customer_id"]
[groups.user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("environments/qa.toml"),
        "[options.tier.gold]\nvalue = \"g-qa\"\n",
    )
    .unwrap();
    postui_core::project::save_local_state(
        dir,
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn opening_a_legacy_project_offers_the_migration_and_lists_its_notes() {
    let dir = tempfile::tempdir().unwrap();
    legacy_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let app = App::with_root(tx, dir.path().to_path_buf());

    let Some(Modal::Confirm {
        title,
        body,
        choices,
    }) = app.modals.top()
    else {
        panic!("a legacy project must offer the migration");
    };
    assert_eq!(title, "Migrate variables");
    assert!(
        body.contains("tier"),
        "the conversion's notes are listed: {body}"
    );
    assert!(body.contains(".bak"), "the safety copy is promised: {body}");
    let keys: Vec<char> = choices.iter().map(|(k, _, _)| *k).collect();
    assert_eq!(keys, vec!['n', 'y']);

    // Until the user answers, the variables are inert rather than
    // half-parsed from a format the model doesn't speak.
    assert!(app.project.model.vars.is_empty());
    assert!(app.project.model.groups.is_empty());
}

#[test]
fn confirming_the_migration_rewrites_the_files_leaves_baks_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    legacy_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    let vars_before = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    let qa_before = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();

    app.handle_key(&keymap, plain('y'));

    assert!(app.modals.is_empty(), "answering closes the prompt");
    assert!(
        app.project.pending_migration().is_none(),
        "nothing left to migrate"
    );

    // The safety copies hold exactly what was there before.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("variables.toml.bak")).unwrap(),
        vars_before
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("environments/qa.toml.bak")).unwrap(),
        qa_before
    );

    // ...and the live files are the new format, loaded into the model.
    let vars_after = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(!vars_after.contains("options"), "{vars_after}");
    let parsed = postui_core::varmodel::parse_variables(&vars_after).expect("new text parses");
    assert_eq!(parsed.groups["tier"].fields, ["tier"]);
    assert_eq!(parsed.groups["user"].fields, ["user_id", "customer_id"]);
    assert_eq!(
        app.project.model.groups["user"].fields,
        ["user_id", "customer_id"]
    );
    assert_eq!(
        app.project.model.vars["base_url"].default.as_deref(),
        Some("http://localhost:8080"),
        "the plain variable came through untouched"
    );

    let qa_after = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    let env = postui_core::varmodel::parse_environment(&qa_after).expect("new env text parses");
    assert_eq!(env.entries["tier"]["gold"].values["tier"], "g-qa");
    assert_eq!(
        app.project.env_data.entries["user"]["alice"].values["customer_id"],
        "c-77"
    );

    let content = rendered_text(&mut app);
    assert!(
        content.contains("migrated"),
        "the result is toasted: {content}"
    );
}

#[test]
fn declining_the_migration_leaves_the_files_alone_and_the_project_open() {
    let dir = tempfile::tempdir().unwrap();
    legacy_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    let vars_before = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();

    app.handle_key(&keymap, plain('n'));

    assert!(app.modals.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("variables.toml")).unwrap(),
        vars_before,
        "declining must not touch a single file"
    );
    assert!(!dir.path().join("variables.toml.bak").exists());
    assert!(app.project.model.vars.is_empty(), "variables stay inert");
    assert!(app.project.resolved.values.is_empty());

    // The project itself is still perfectly usable, and the prompt does
    // not come back on the next reload.
    app.update(Action::ReloadProjectFiles);
    assert!(app.modals.is_empty(), "declined once, not re-offered");
    assert!(app.project.pending_migration().is_none());
    app.update(Action::CreateRequest("ping".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("ping"));
}

#[test]
fn migrating_a_project_with_no_environments_creates_default_toml_for_the_entries() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("legacy")).unwrap();
    std::fs::write(
        dir.path().join("variables.toml"),
        "[tier]\n[tier.options.gold]\nvalue = \"g-1\"\n",
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert!(app.project.environments.is_empty());

    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("a legacy project must offer the migration");
    };
    assert!(
        body.contains("environments/default.toml"),
        "the new environment is announced up front: {body}"
    );

    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('y'));

    let default_toml =
        std::fs::read_to_string(dir.path().join("environments/default.toml")).unwrap();
    let env = postui_core::varmodel::parse_environment(&default_toml).unwrap();
    assert_eq!(env.entries["tier"]["gold"].values["tier"], "g-1");
    assert_eq!(app.project.environments, vec!["default".to_string()]);
    assert!(
        !dir.path().join("environments/default.toml.bak").exists(),
        "a brand-new file has nothing to back up"
    );
}

/// Review finding: `open` used to prune "stale" selections against the
/// *empty* model a legacy project leaves behind, wiping (and persisting
/// the loss of) every selection before the user had even answered the
/// prompt — so declining lost local state despite the untouched promise,
/// and applying came up all-needs-selection even though the migration
/// keeps the group names.
#[test]
fn a_legacy_projects_saved_selections_survive_the_prompt_and_resolve_after_applying() {
    let dir = tempfile::tempdir().unwrap();
    legacy_project(dir.path());
    let mut selections = indexmap::IndexMap::new();
    let mut qa = indexmap::IndexMap::new();
    qa.insert("tier".to_string(), "gold".to_string());
    qa.insert("user".to_string(), "alice".to_string());
    selections.insert("qa".to_string(), qa);
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            selections,
            ..Default::default()
        },
    )
    .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    // Nothing was cleared while the prompt is still up...
    assert!(
        !app.toasts
            .messages()
            .iter()
            .any(|m| m.contains("no longer exists")),
        "no bogus stale-selection warnings: {:?}",
        app.toasts.messages()
    );
    assert_eq!(app.project.selections_for("qa")["tier"], "gold");
    assert_eq!(app.project.selections_for("qa")["user"], "alice");
    let on_disk = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(on_disk.selections["qa"]["tier"], "gold");
    assert_eq!(on_disk.selections["qa"]["user"], "alice");

    // ...and once migrated, they select the migrated entries.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('y'));

    assert_eq!(app.project.selections_for("qa")["tier"], "gold");
    assert_eq!(
        app.project.resolved.values["tier"], "g-qa",
        "the carried-over selection resolves: {:?}",
        app.project.resolved.values
    );
    assert_eq!(app.project.resolved.values["user_id"], "1001");
    assert_eq!(app.project.resolved.values["customer_id"], "c-77");
}

/// The decline half of the same finding: refusing the migration must leave
/// `.local/state.toml` exactly as it was, not just the shareable files.
#[test]
fn declining_the_migration_leaves_saved_selections_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    legacy_project(dir.path());
    let mut selections = indexmap::IndexMap::new();
    let mut qa = indexmap::IndexMap::new();
    qa.insert("tier".to_string(), "gold".to_string());
    selections.insert("qa".to_string(), qa);
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            selections,
            ..Default::default()
        },
    )
    .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('n'));

    let on_disk = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(
        on_disk.selections["qa"]["tier"], "gold",
        "declining must not touch local state either"
    );
}

// --- computed request-headers section: copy/reveal/env-switch (Task 10) ---

#[test]
fn auto_header_copy_icon_puts_the_resolved_value_on_the_clipboard() {
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        None, 65536, false,
    ));
    app.editor.active_tab = EditorTab::Headers;
    app.editor.url = LineInput::new("https://example.com/foo");
    app.update(Action::Render);

    let backend = ratatui::backend::TestBackend::new(100, 70);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    // The scratch request has no default headers and no body, so the Host
    // row (from the URL) is the only computed row, at index 0.
    let rect = app
        .hits
        .rect_of(&Hit::AutoHeaderCopy(0))
        .expect("the Host row's copy icon is registered");

    app.handle_mouse(left_down(rect.x, rect.y));

    assert!(
        rendered_text(&mut app).contains("Copied Host"),
        "toast confirms the copy"
    );
}

#[test]
fn computed_headers_mask_a_secret_by_default_and_the_reveal_toggle_unmasks() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarEdit(VarEditOp::SetSecretValue {
        env: "qa".into(),
        name: "api_key".into(),
        value: "sk-live-abc123".into(),
    }));
    // A project default header, not a request-table row: the computed
    // section only shows non-`Request`-origin rows, so this is what
    // exercises it (see `computed_headers_recompute_reflects_an_env_switch`
    // for the same reasoning).
    app.project.meta.default_headers.insert(
        "Authorization".into(),
        postui_core::model::Entry {
            value: "Bearer {{api_key}}".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Headers;
    app.update(Action::Render);

    let backend = ratatui::backend::TestBackend::new(100, 70);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let masked = format!("{:?}", terminal.backend().buffer());
    assert!(
        !masked.contains("sk-live-abc123"),
        "the secret must not render in the clear by default: {masked}"
    );
    assert!(
        masked.contains("\u{25cf}"),
        "the masked value renders as the dot mask: {masked}"
    );
    let reveal_rect = app
        .hits
        .rect_of(&Hit::AutoHeaderReveal)
        .expect("the reveal toggle shows because a secret is in play");

    app.handle_mouse(left_down(reveal_rect.x, reveal_rect.y));
    assert!(app.editor.computed.revealed);

    let mut terminal2 =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 70)).unwrap();
    terminal2.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let revealed = format!("{:?}", terminal2.backend().buffer());
    assert!(
        revealed.contains("sk-live-abc123"),
        "revealing shows the real value: {revealed}"
    );

    // The toggle itself must survive the round trip (still present, now
    // reading "hide") rather than vanishing once nothing is masked.
    assert!(app.hits.rect_of(&Hit::AutoHeaderReveal).is_some());
}

#[test]
fn computed_headers_recompute_reflects_an_env_switch() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    // A project default header, not a request-table row: the computed
    // section only shows non-`Request`-origin rows (the editable table
    // above already shows the request's own rows, literal text and all),
    // so this is what actually exercises the env-driven recompute.
    app.project.meta.default_headers.insert(
        "X-Base".into(),
        postui_core::model::Entry {
            value: "{{base_url}}".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Headers;
    app.update(Action::Render);

    let backend = ratatui::backend::TestBackend::new(100, 70);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let qa_view = format!("{:?}", terminal.backend().buffer());
    assert!(
        qa_view.contains("https://qa.example.com"),
        "qa's own value resolves: {qa_view}"
    );

    app.update(Action::SwitchEnv(Some("dev".into())));
    let mut terminal2 =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 70)).unwrap();
    terminal2.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let dev_view = format!("{:?}", terminal2.backend().buffer());
    assert!(
        dev_view.contains("http://localhost:8080"),
        "dev has no override, so the declared default resolves instead: {dev_view}"
    );
    assert!(
        !dev_view.contains("https://qa.example.com"),
        "the stale qa value must not linger: {dev_view}"
    );
}

#[test]
fn computed_headers_reveal_resets_when_switching_to_a_different_request() {
    // Reveal is a per-request gesture (spec §3: secrets masked by default);
    // it must not leak from request A into request B just because both
    // happen to render the same project default header.
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarEdit(VarEditOp::SetSecretValue {
        env: "qa".into(),
        name: "api_key".into(),
        value: "sk-live-abc123".into(),
    }));
    app.project.meta.default_headers.insert(
        "Authorization".into(),
        postui_core::model::Entry {
            value: "Bearer {{api_key}}".into(),
            enabled: true,
        },
    );
    postui_core::storage::save_request(&app.project.root, "a", &req("https://x/a")).unwrap();
    postui_core::storage::save_request(&app.project.root, "b", &req("https://x/b")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("a".into()));
    app.editor.active_tab = EditorTab::Headers;
    app.update(Action::Render);

    let backend = ratatui::backend::TestBackend::new(100, 70);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let reveal_rect = app
        .hits
        .rect_of(&Hit::AutoHeaderReveal)
        .expect("A shows the reveal toggle");
    app.handle_mouse(left_down(reveal_rect.x, reveal_rect.y));
    assert!(app.editor.computed.revealed, "A is now revealed");

    let mut terminal_a =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 70)).unwrap();
    terminal_a.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let a_view = format!("{:?}", terminal_a.backend().buffer());
    assert!(
        a_view.contains("sk-live-abc123"),
        "sanity: A really is showing the secret in the clear: {a_view}"
    );

    app.update(Action::OpenRequest("b".into()));
    assert!(
        !app.editor.computed.revealed,
        "opening a different request must re-mask"
    );

    let mut terminal_b =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 70)).unwrap();
    terminal_b.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let b_view = format!("{:?}", terminal_b.backend().buffer());
    assert!(
        !b_view.contains("sk-live-abc123"),
        "B must render masked, not inherit A's reveal: {b_view}"
    );
    assert!(
        b_view.contains("\u{25cf}"),
        "B shows the dot mask: {b_view}"
    );
    assert!(
        b_view.contains("\u{1F441} reveal") && !b_view.contains("\u{1F441} hide"),
        "B's own toggle reads \"reveal\", not \"hide\" (the collapse toggle's unrelated \
         \"⌄ hide\" label is a substring trap here, so this checks the 👁 glyph too): {b_view}"
    );
}
