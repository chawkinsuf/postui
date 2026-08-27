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

/// Regression for the idle-tick redraw bug: an animation's very last tick
/// always sees `animating() == false` (its duration has just elapsed by the
/// time that tick runs), so a redraw decision based only on "is anything
/// animating right now" misses exactly the tick that needs to redraw one
/// more time to reveal whatever was gated on the animation reaching t==1.0
/// (a modal's contents, a dropdown's shadow). `Action::Tick` must still
/// report a redraw on that active→finished transition, then fall quiet on
/// the next (truly idle) tick.
#[test]
fn tick_redraws_once_more_when_an_animation_finishes_between_ticks() {
    let mut app = App::new_for_test();
    app.ui_settings.anim_ms.modal_open = std::time::Duration::from_millis(1);
    app.update(Action::OpenPalette);

    // Still mid-flight: this tick is expected to redraw.
    assert!(app.update(Action::Tick), "mid-flight tick should redraw");

    // Let the 1ms open-settle duration fully elapse before the next tick
    // samples `Instant::now()` — `animating()` now reads false, exactly
    // like the very last real tick of any settle animation.
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(
        app.update(Action::Tick),
        "the tick that observes the animation just finished must still \
         redraw once more, or the settled frame (e.g. a modal's gated \
         contents) never gets painted without further input"
    );

    // Now genuinely idle: no further redraw is owed.
    assert!(
        !app.update(Action::Tick),
        "a subsequent tick with nothing animating shouldn't force a redraw"
    );
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
fn ctrl_c_copies_the_url_selection_instead_of_quitting() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    app.editor.url = crate::components::line_input::LineInput::new("https://example.com");
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url.select_all();

    app.handle_key(&Keymap::default_bindings(), ctrl('c'));

    assert!(!app.should_quit, "copy pre-empts quit");
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "https://example.com"
    );
    assert!(
        app.editor.url.selection().is_some(),
        "copy keeps the selection"
    );
    // With the selection gone, ctrl+c means quit again (here gated on the
    // unsaved URL edit, so it surfaces as the confirm modal).
    app.editor.url.clear_selection();
    app.handle_key(&Keymap::default_bindings(), ctrl('c'));
    assert!(
        app.should_quit || !app.modals.is_empty(),
        "quit (or its unsaved-changes gate) fires once nothing is selected"
    );
}

#[test]
fn ctrl_c_copies_a_modal_prompt_selection_and_keeps_the_modal_open() {
    use crate::components::modal::{Modal, PromptKind};
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    let mut input = crate::components::line_input::LineInput::new("my-request");
    input.select_all();
    app.modals.push(Modal::Prompt {
        title: "Name".into(),
        input,
        kind: PromptKind::NewRequest,
        revealed: false,
    });

    app.handle_key(&Keymap::default_bindings(), ctrl('c'));

    assert!(!app.should_quit);
    assert!(!app.modals.is_empty(), "the modal stays open");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "my-request");
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
fn apply_ui_settings_wires_animations_into_anims() {
    // Regression: `App::new`'s two branches (resolved root / no-root
    // fallback) must both route a loaded `UiSettings.animations` into
    // `App.anims` — easy to forget when a new UiSettings-derived field is
    // added to one branch's block of assignments but not the other's.
    // `App::new` itself reads the real user config file, so it isn't
    // unit-testable here; `apply_ui_settings` is the single place both
    // branches delegate to, so exercising it directly covers the wiring.
    let mut app = App::new_for_test();
    assert!(app.anims.enabled, "new_for_test defaults to animations on");

    let settings = crate::config::UiSettings {
        animations: false,
        ..crate::config::UiSettings::default()
    };
    app.apply_ui_settings(settings, crate::theme::Theme::dark());
    assert!(
        !app.anims.enabled,
        "animations = false in UiSettings must disable App.anims"
    );

    let settings = crate::config::UiSettings {
        animations: true,
        ..crate::config::UiSettings::default()
    };
    app.apply_ui_settings(settings, crate::theme::Theme::dark());
    assert!(app.anims.enabled, "animations = true re-enables App.anims");
}

#[test]
fn hover_change_starts_a_hover_fade_and_tick_redraws_while_animating() {
    let mut app = App::new_for_test();
    app.begin_hover_fade();
    assert!(app.animating(), "hover fade counts as a live animation");
    assert!(
        app.update(Action::Tick),
        "ticks redraw while an animation is live"
    );
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

fn scroll_right(x: u16, y: u16) -> ratatui::crossterm::event::MouseEvent {
    ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::ScrollRight,
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
    app.editor.active_tab = EditorTab::Params;
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

/// Task 17, spec §5: right-clicking a params row opens Duplicate/Delete/
/// Extract, and each one works end to end.
#[test]
fn table_row_context_menu_duplicate_delete_extract_end_to_end() {
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Params;
    app.editor.sub_focus = SubFocus::Content;
    app.editor.table.selected = Some(0);
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::TableRow(0))
        .expect("the row's background is registered");

    app.handle_mouse(right_down(r.x, r.y));
    let Some(Modal::Dropdown(menu)) = app.modals.top() else {
        panic!("expected the row's context menu");
    };
    let labels: Vec<String> = menu.items.iter().map(|i| i.label.clone()).collect();
    assert_eq!(
        labels,
        vec![
            "Duplicate row",
            "Delete param\u{2026}",
            "Extract value to variable\u{2026}"
        ]
    );
    let duplicate = menu.items[0].action.clone().unwrap();
    let delete = menu.items[1].action.clone().unwrap();
    let extract = menu.items[2].action.clone().unwrap();
    app.update(Action::Close);

    // "Duplicate row": inserts "page-copy" right below with the same value.
    app.update(duplicate);
    assert_eq!(app.editor.params.len(), 2);
    assert_eq!(
        app.editor.params.get_index(1),
        Some((
            &"page-copy".to_string(),
            &postui_core::model::Entry {
                value: "2".into(),
                enabled: true,
            }
        ))
    );

    // "Extract value to variable…": the row is only *selected*, not under
    // edit, yet the prompt still opens against its Value cell's text.
    app.editor.table.selected = Some(0);
    app.editor.table.editing = None;
    app.update(extract);
    let Some(Modal::MultiPrompt { kind, .. }) = app.modals.top() else {
        panic!("expected the extract-variable multi-prompt");
    };
    assert!(matches!(kind, PromptKind::ExtractVariable));
    let keymap = Keymap::default_bindings();
    type_into_field(&mut app, &keymap, "page_num");
    app.handle_key(&keymap, enter_key());
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.params["page"].value, "{{page_num}}");

    // "Delete param…": same confirm the `d` key opens.
    app.editor.table.selected = Some(0);
    app.update(delete);
    match app.modals.top() {
        Some(Modal::Confirm { body, .. }) => assert!(body.contains("page"), "{body}"),
        _ => panic!("expected a delete confirm"),
    }
    app.handle_key(&keymap, plain('y'));
    assert!(!app.editor.params.contains_key("page"));
    assert!(app.editor.params.contains_key("page-copy"));
}

/// A second right-click-duplicate collides with an existing `-copy` row and
/// falls back to `-copy-2`, matching `DuplicateRequest`/`DuplicateVar`.
#[test]
fn duplicate_table_row_resolves_collisions_like_duplicate_request() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Params;
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    app.editor.params.insert(
        "page-copy".into(),
        postui_core::model::Entry {
            value: "9".into(),
            enabled: true,
        },
    );
    app.update(Action::DuplicateTableRow(0));
    assert!(app.editor.params.contains_key("page-copy-2"));
    assert_eq!(app.editor.params["page-copy-2"].value, "2");
}

#[test]
fn editor_tab_cycle_order_is_headers_params_vars_body() {
    let mut app = App::new_for_test();
    // Body is only reachable for a method that sends one.
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(app.editor.active_tab, EditorTab::Vars);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(
        app.editor.active_tab,
        EditorTab::Headers,
        "cycle wraps back to Headers"
    );
}

#[test]
fn alt_number_shortcuts_follow_the_screen_order() {
    // alt+1..4 select the tabs in the order they appear on screen:
    // Headers, Params, Vars, Body.
    let mut app = App::new_for_test();
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    let keymap = Keymap::default_bindings();
    app.editor.active_tab = EditorTab::Body;
    app.handle_key(&keymap, alt('1'));
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    app.handle_key(&keymap, alt('2'));
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    app.handle_key(&keymap, alt('3'));
    assert_eq!(app.editor.active_tab, EditorTab::Vars);
    app.handle_key(&keymap, alt('4'));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
}

#[test]
fn body_tab_is_unreachable_for_get_and_head() {
    // GET/HEAD send no body, so the Body tab is disabled: direct select is
    // a no-op and cycling skips over it in both directions.
    let mut app = App::new_for_test();
    assert_eq!(app.editor.method, postui_core::model::Method::Get);
    app.update(Action::EditorTabSelect(3));
    assert_eq!(
        app.editor.active_tab,
        EditorTab::Headers,
        "select is a no-op"
    );
    app.update(Action::EditorTabSelect(2)); // Vars, the tab before Body
    app.update(Action::EditorTabCycle(1));
    assert_eq!(
        app.editor.active_tab,
        EditorTab::Headers,
        "forward skips Body"
    );
    app.update(Action::EditorTabCycle(-1));
    assert_eq!(
        app.editor.active_tab,
        EditorTab::Vars,
        "backward skips Body"
    );

    app.update(Action::SetMethod(postui_core::model::Method::Head));
    app.update(Action::EditorTabSelect(3));
    assert_eq!(
        app.editor.active_tab,
        EditorTab::Vars,
        "HEAD: still a no-op"
    );

    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.update(Action::EditorTabSelect(3));
    assert_eq!(app.editor.active_tab, EditorTab::Body, "POST re-enables it");
}

#[test]
fn switching_to_a_bodyless_method_hops_off_the_body_tab() {
    let mut app = App::new_for_test();
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.update(Action::EditorTabSelect(3));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    app.update(Action::SetMethod(postui_core::model::Method::Get));
    assert_eq!(
        app.editor.active_tab,
        EditorTab::Headers,
        "GET can't sit on a disabled tab"
    );
    // The body text itself survives the round trip.
    app.update(Action::SetMethod(postui_core::model::Method::Put));
    app.update(Action::EditorTabSelect(3));
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
    app.editor.active_tab = EditorTab::Params;
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

/// The alt+a footer chip names what it will actually add on the active
/// tab — "add header" on Headers, "add param" on Params, "add variable"
/// on Vars — and disappears on the Body tab, where alt+a is inert.
#[test]
fn add_row_chip_label_follows_the_active_tab() {
    let mut app = App::new_for_test();
    app.focus = PaneId::Editor;
    let frame = |app: &mut App| {
        render_once(app);
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    };
    assert!(frame(&mut app).contains("add header"), "Headers tab");
    app.update(Action::EditorTabSelect(1));
    assert!(frame(&mut app).contains("add param"), "Params tab");
    app.update(Action::EditorTabSelect(2));
    assert!(frame(&mut app).contains("add variable"), "Vars tab");
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.update(Action::EditorTabSelect(3));
    let body = frame(&mut app);
    assert!(
        !body.contains("add header") && !body.contains("add param"),
        "no add chip on the Body tab"
    );
}

/// alt+a starts a new row on whichever table tab is active — from any
/// focus, so adding a header never needs a mouse trip to the ghost row.
#[test]
fn alt_a_starts_a_new_row_on_the_active_table_tab() {
    use crate::components::table_editor::Col;
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Sidebar; // works from anywhere
    app.handle_key(&keymap, alt('a'));
    assert_eq!(app.focus, PaneId::Editor);
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
    let edit = app
        .editor
        .table
        .editing
        .as_ref()
        .expect("a new-row edit began on the Headers tab");
    assert_eq!((edit.row, edit.col), (0, Col::Key));
    type_chars(&mut app, "X-Trace");
    app.handle_key(&keymap, enter_key());
    assert!(app.editor.headers.contains_key("X-Trace"));

    // On the Body tab there is no table to add to: inert.
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.update(Action::EditorTabSelect(3));
    app.handle_key(&keymap, alt('a'));
    assert!(app.editor.table.editing.is_none());
    assert_eq!(app.editor.active_tab, EditorTab::Body);
}

/// A control that appears under a stationary pointer (here: the row's
/// hover-revealed toggle button) must pick up hover styling from the
/// post-frame resync, without the mouse having to move again.
#[test]
fn hover_resyncs_to_controls_revealed_under_a_stationary_pointer() {
    let mut app = app_with_one_param();
    render_once(&mut app);
    let row = app.hits.rect_of(&Hit::TableRow(0)).unwrap();
    // Land exactly where the toggle button will appear (3 cells starting 8
    // from the row's right edge — see `draw_row_buttons`).
    let x = row.right() - 7;
    app.handle_mouse(moved(x, row.y));
    assert_ne!(
        app.hovered,
        Some(Hit::TableCheckbox(0)),
        "frame N has no button registered yet"
    );
    render_once(&mut app); // frame N+1 draws + registers the buttons
    assert!(
        app.resync_hover(),
        "the resync notices the new control under the pointer"
    );
    assert_eq!(app.hovered, Some(Hit::TableCheckbox(0)));
    assert!(
        !app.resync_hover(),
        "a second resync with nothing changed is quiet"
    );
}

/// Moves the pointer onto `row_hit`'s rect so hover-revealed affordances
/// (the row's toggle/trash buttons) get registered, then clicks `hit`.
fn hover_row_then_click(app: &mut App, row_hit: Hit, hit: Hit) {
    render_once(app);
    let r = app
        .hits
        .rect_of(&row_hit)
        .unwrap_or_else(|| panic!("no rect registered for {row_hit:?}"));
    app.handle_mouse(moved(r.x + 1, r.y));
    click_hit(app, hit);
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
    app.editor.active_tab = EditorTab::Params;
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
    hover_row_then_click(&mut app, Hit::TableRow(1), Hit::TableCheckbox(1));
    assert_eq!(app.editor.params["a"].value, "19", "the edit committed");
    assert!(!app.editor.params["b"].enabled, "and the toggle landed");
    assert!(app.editor.table.editing.is_none());

    // Same for the trash button on another row.
    click_hit(&mut app, Hit::TableCell { row: 0, col: 0 });
    type_chars(&mut app, "x");
    hover_row_then_click(&mut app, Hit::TableRow(1), Hit::TableDelete(1));
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
    app.editor.active_tab = EditorTab::Params;
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
    hover_row_then_click(&mut app, Hit::TableRow(1), Hit::TableCheckbox(1));
    assert_eq!(app.editor.params.len(), 2, "a collapsed into c");
    assert!(!app.editor.params["b"].enabled, "the clicked row toggled");
    assert!(app.editor.params["c"].enabled, "no neighbour was toggled");
}

#[test]
fn a_collapsing_commit_reresolves_the_row_a_delete_click_named() {
    let mut app = app_with_three_params();
    stage_a_collapsing_rename(&mut app);
    // The trash clicked belongs to "b" (row 1 in the frame the user saw);
    // after the commit collapses "a" into "c", "b" is row 0 — the confirm
    // must name "b", not whatever now occupies index 1 ("c"). (The edited
    // row itself shows no buttons while its cell edit is live, so a stale
    // click on it can no longer happen at all.)
    hover_row_then_click(&mut app, Hit::TableRow(1), Hit::TableDelete(1));
    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("expected the delete confirm");
    };
    assert!(
        body.contains('b'),
        "the confirm names the clicked row: {body}"
    );
    assert!(
        !body.contains('c'),
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
    app.handle_key(&Keymap::default_bindings(), alt('1'));
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    assert_eq!(
        app.editor.params["page"].value, "12",
        "the tab switch commits instead of resetting the edit away"
    );
    assert!(app.editor.table.editing.is_none());
}

/// Task 10: switching editor tabs (Params -> Headers) retargets the
/// underline slide's independent left/right edges toward the new tab's
/// span, easing over `ui_settings.anim_ms.tab_slide`.
#[test]
fn switching_editor_tabs_retargets_the_underline_slide() {
    let mut app = App::new_for_test_with_anims(true);
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    let left_key = AnimKey::TabUnderline(StripId::EditorTabs);
    let right_key = AnimKey::TabUnderlineWidth(StripId::EditorTabs);
    let before = Instant::now();
    assert!(
        app.anims.value(left_key, before).is_none(),
        "untouched before any switch"
    );

    app.update(Action::EditorTabSelect(EditorTab::Params.index()));
    assert_eq!(app.editor.active_tab, EditorTab::Params);

    let now = Instant::now();
    assert!(
        app.anims.active(now),
        "the underline is easing right after the switch"
    );

    let spans = app.editor.tab_strip_spans();
    let (x, w) = spans[EditorTab::Params.draw_position()];
    let done_at = now + app.ui_settings.anim_ms.tab_slide + Duration::from_millis(5);
    assert_eq!(
        app.anims.value(left_key, done_at),
        Some(x as f32),
        "left edge settles on the Params span's left edge"
    );
    assert_eq!(
        app.anims.value(right_key, done_at),
        Some((x + w) as f32),
        "right edge settles on the Params span's right edge"
    );
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
            name: None,
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
            name: None,
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
    // 1-line pitch: the row's hit rect is exactly one row tall now, so the
    // click must land on its own `y`, not `y + 1` (the next row's).
    app.handle_mouse(left_down(r.x + 1, r.y));
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
    // 1-line pitch: click on the row's own `y`, not `y + 1`.
    app.handle_mouse(right_down(r.x + 1, r.y));

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
fn pointer_shape_update_emits_only_on_change() {
    let mut app = App::new_for_test();
    render_once(&mut app);

    // Nothing hovered yet: already `Default`, the shape it starts at.
    assert_eq!(app.pointer_shape_update(), None);

    let chip = app
        .hits
        .rect_of(&crate::hit::Hit::FooterChip(Action::OpenPalette))
        .unwrap();
    app.handle_mouse(moved(chip.x, chip.y));
    assert_eq!(
        app.pointer_shape_update(),
        Some(crate::hit::PointerShape::Pointer),
        "hovering a clickable hit emits the new shape"
    );
    // Still hovering the same button: no repeat emission.
    assert_eq!(app.pointer_shape_update(), None);

    let sidebar = app
        .hits
        .rect_of(&crate::hit::Hit::Pane(PaneId::Sidebar))
        .unwrap();
    app.handle_mouse(moved(sidebar.x, sidebar.y));
    assert_eq!(
        app.pointer_shape_update(),
        Some(crate::hit::PointerShape::Default),
        "moving back over background emits the reset"
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
fn horizontal_wheel_over_the_response_pane_scrolls_it_sideways() {
    let mut app = App::new_for_test();
    // A non-JSON one-liner far wider than the response pane, so the raw
    // view has columns to scroll to.
    let body = "x".repeat(300);
    app.session.response.set_state(
        crate::components::response::ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![],
            body: body.clone(),
            elapsed: std::time::Duration::from_millis(1),
            size: body.len(),
            content_type: None,
        })),
        0,
    );
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::Pane(PaneId::Response))
        .unwrap();
    assert!(app.handle_mouse(scroll_right(r.x + 1, r.y + 1)));
    assert!(
        app.session.response.view().unwrap().h_scroll > 0,
        "a sideways wheel notch moves the response viewport right"
    );
}

/// An app with one saved request open and a dirtying edit typed into its
/// URL, so quit paths can exercise the unsaved-changes gate.
fn dirty_app() -> App {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("r".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    assert!(app.editor.is_dirty());
    app
}

#[test]
fn quitting_with_unsaved_changes_gates_on_the_confirm() {
    let mut app = dirty_app();
    app.update(Action::Quit);
    assert!(!app.should_quit, "quit must wait for the gate");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert!(app.should_quit, "Discard changes quits");
}

#[test]
fn quitting_with_unsaved_changes_can_save_first() {
    let mut app = dirty_app();
    app.update(Action::Quit);
    app.handle_key(&Keymap::default_bindings(), plain('s'));
    assert!(app.should_quit, "Save & quit quits");
    assert!(!app.editor.is_dirty(), "…after actually saving");
}

#[test]
fn discard_changes_confirms_then_reverts_to_the_saved_request() {
    let mut app = dirty_app(); // url edited from "https://x/r"
    app.update(Action::ConfirmDiscardChanges);
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "discard asks first"
    );
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert!(!app.editor.is_dirty(), "reverted to the saved snapshot");
    assert_eq!(app.editor.url.text(), "https://x/r");
    assert!(app.modals.is_empty());
}

#[test]
fn clicking_the_row_toggle_toggles_without_selecting() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Params;
    app.editor.params.insert(
        "a".into(),
        postui_core::model::Entry {
            value: "1".into(),
            enabled: true,
        },
    );
    render_once(&mut app);
    // The buttons are hover-revealed: move the pointer onto the row first,
    // redraw so the frame registers them, then click.
    let row = app.hits.rect_of(&crate::hit::Hit::TableRow(0)).unwrap();
    app.handle_mouse(moved(row.x + 2, row.y));
    render_once(&mut app);
    let toggle = app
        .hits
        .rect_of(&crate::hit::Hit::TableCheckbox(0))
        .expect("hovered row registers its toggle");
    app.handle_mouse(left_down(toggle.x + 1, toggle.y));
    assert!(!app.editor.params["a"].enabled, "the click toggled");
    assert_eq!(
        app.editor.table.selected, None,
        "a toggle click is not a row selection"
    );
}

/// An app whose editor holds typed-but-never-saved content: no slug, no
/// saved snapshot, a URL in the bar.
fn scratch_app() -> App {
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://x/scratch");
    assert!(!app.editor.is_dirty(), "no snapshot, so never 'dirty'");
    app
}

#[test]
fn quitting_a_never_saved_scratch_gates_too() {
    let mut app = scratch_app();
    app.update(Action::Quit);
    assert!(!app.should_quit, "typed content must not vanish silently");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert!(app.should_quit, "Discard quits");
}

#[test]
fn saving_a_scratch_through_the_gate_chains_the_quit() {
    let mut app = scratch_app();
    let keymap = Keymap::default_bindings();
    app.update(Action::Quit);
    app.handle_key(&keymap, plain('s')); // Save as… & quit
    assert!(
        matches!(app.modals.top(), Some(Modal::Prompt { .. })),
        "the scratch save path is the name prompt"
    );
    for c in "fresh".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let saved = postui_core::storage::load_request(&app.project.root, "fresh").unwrap();
    assert_eq!(saved.url, "https://x/scratch");
    assert!(app.should_quit, "the deferred quit ran after the save");
}

#[test]
fn a_failing_gate_save_does_not_run_the_deferred_action() {
    let mut app = scratch_app();
    let keymap = Keymap::default_bindings();
    app.update(Action::Quit);
    app.handle_key(&keymap, plain('s'));
    // A blank name: the save fails with a toast, so quitting now would
    // still lose the content.
    for c in "   ".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.should_quit, "no save, no quit");
}

#[test]
fn escaping_the_gates_save_prompt_cancels_everything() {
    let mut app = scratch_app();
    let keymap = Keymap::default_bindings();
    app.update(Action::Quit);
    app.handle_key(&keymap, plain('s'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert!(!app.should_quit, "Esc means stay, with everything intact");
    assert_eq!(app.editor.url.text(), "https://x/scratch");
}

#[test]
fn opening_a_request_over_a_scratch_gates_first() {
    let mut app = scratch_app();
    postui_core::storage::save_request(&app.project.root, "other", &req("https://x/other"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("other".into()));
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "the scratch content gates the open"
    );
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert_eq!(app.editor.slug.as_deref(), Some("other"));
}

#[test]
fn quitting_clean_needs_no_confirm() {
    let mut app = App::new_for_test();
    app.update(Action::Quit);
    assert!(app.should_quit);
    assert!(app.modals.is_empty());
}

#[test]
fn quit_with_a_modal_already_open_stays_immediate() {
    // ctrl+c must stay a reliable exit from inside any modal — including
    // the unsaved-changes gate itself (press it twice to leave without
    // saving), so a dirty editor never traps the user.
    let mut app = dirty_app();
    app.modals.push(crate::components::modal::Modal::Message {
        title: "About".into(),
        body: "hello".into(),
    });
    app.update(Action::Quit);
    assert!(app.should_quit);
}

#[test]
fn clicking_a_multi_prompt_field_moves_focus_there() {
    let mut app = App::new_for_test();
    app.modals.push(Modal::MultiPrompt {
        title: "New group".into(),
        fields: vec![
            crate::components::modal::PromptField::text("name", "Name", ""),
            crate::components::modal::PromptField::text("fields", "Fields", ""),
        ],
        focus: 0,
        kind: crate::components::modal::PromptKind::NewGroup,
    });
    render_once(&mut app);
    let second = app
        .hits
        .rect_of(&crate::hit::Hit::ModalField(1))
        .expect("each prompt field registers a click target");
    assert!(app.handle_mouse(left_down(second.x + 2, second.y + 1)));
    let Some(Modal::MultiPrompt { focus, .. }) = app.modals.top() else {
        panic!("modal must stay open");
    };
    assert_eq!(*focus, 1, "the click moved focus to the second field");
}

/// An app whose response pane holds a wide non-JSON one-liner, rendered
/// once so the horizontal scrollbar's hits exist.
fn app_with_wide_response() -> App {
    let mut app = App::new_for_test();
    let body = "x".repeat(300);
    app.session.response.set_state(
        crate::components::response::ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![],
            body: body.clone(),
            elapsed: std::time::Duration::from_millis(1),
            size: body.len(),
            content_type: None,
        })),
        0,
    );
    render_once(&mut app);
    app
}

#[test]
fn dragging_the_horizontal_thumb_scrolls_the_response_sideways() {
    let mut app = app_with_wide_response();
    let thumb = app
        .hits
        .rect_of(&crate::hit::Hit::HScrollThumb(PaneId::Response))
        .expect("horizontal thumb hit");
    assert!(app.handle_mouse(left_down(thumb.x + 1, thumb.y)));
    assert!(app.handle_mouse(moved(thumb.x + 20, thumb.y)));
    assert!(
        app.session.response.view().unwrap().h_scroll > 0,
        "the thumb drag moved the viewport right"
    );
    let offset = app.session.response.view().unwrap().h_scroll;
    // Vertical motion while a horizontal drag is live must not scroll.
    app.handle_mouse(moved(thumb.x + 20, thumb.y.saturating_sub(5)));
    assert_eq!(
        app.session.response.view().unwrap().scroll,
        0,
        "a horizontal drag never feeds the vertical axis"
    );
    assert_eq!(app.session.response.view().unwrap().h_scroll, offset);
}

#[test]
fn clicking_the_horizontal_track_pages_sideways() {
    let mut app = app_with_wide_response();
    let viewport = app.session.response.view().unwrap().width() as i16;
    let track = app
        .hits
        .rect_of(&crate::hit::Hit::HScrollTrack(PaneId::Response, viewport))
        .expect("page-right track segment");
    assert!(app.handle_mouse(left_down(track.x + 1, track.y)));
    assert!(
        app.session.response.view().unwrap().h_scroll > 0,
        "a track click pages the viewport toward the click"
    );
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

/// Clicking into the body content starts the same `AnimKey::FocusFade` the
/// URL well's own focus lift uses (Task 12 controller amendment) — one
/// mechanism, retargeted wherever keyboard focus actually lands on
/// something that fades in on focus.
#[test]
fn click_in_body_area_starts_the_focus_fade() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello\nworld");
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");
    // Settle any fade a prior focus move left in flight, so this click is
    // unambiguously what (re)starts it.
    app.anims.snap(AnimKey::FocusFade, 1.0);
    app.handle_mouse(left_down(area.x + 4, area.y + 1));
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
    let now = std::time::Instant::now();
    assert!(
        app.anims.value(AnimKey::FocusFade, now).unwrap() < 1.0,
        "clicking into the body content restarts the focus fade from 0"
    );
}

/// Tabbing keyboard focus into the body content (from the tab strip) starts
/// the same fade the mouse-click path does.
#[test]
fn tab_into_body_content_starts_the_focus_fade() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello");
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Tabs;
    app.anims.snap(AnimKey::FocusFade, 1.0);
    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    app.handle_key(&Keymap::default_bindings(), down);
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
    let now = std::time::Instant::now();
    assert!(
        app.anims.value(AnimKey::FocusFade, now).unwrap() < 1.0,
        "tabbing into the body content restarts the focus fade from 0"
    );
}

#[test]
fn dragging_in_the_body_selects_and_ctrl_c_copies_it() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello\nworld");
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");
    // 2 lines -> a 2-cell gutter; content column 0 is at area.x + 2.
    app.handle_mouse(left_down(area.x + 2, area.y));
    assert!(
        app.handle_mouse(dragged(area.x + 4, area.y)),
        "the sweep routes to the body editor"
    );
    app.handle_mouse(left_up(area.x + 4, area.y));
    assert_eq!(app.editor.body_selected_text().as_deref(), Some("hel"));
    assert!(app.text_drag.is_none(), "release ends the sweep");

    app.handle_key(&Keymap::default_bindings(), ctrl('c'));
    assert!(!app.should_quit, "copy pre-empts quit");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "hel");
}

#[test]
fn body_selection_paints_with_the_selection_color() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello\nworld");
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");
    app.handle_mouse(left_down(area.x + 2, area.y));
    app.handle_mouse(dragged(area.x + 4, area.y));

    // Same size as `render_once`, so the recorded body area still matches.
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer();
    let cell = buf.cell((area.x + 3, area.y)).unwrap();
    assert_eq!(
        cell.bg, app.theme.selection,
        "a selected body cell paints on the selection background"
    );
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
                name: "ping".into(),
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
            name: "top".into(),
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

/// A project with three flat top-level requests, sorted (by name, all
/// nameless so it falls back to slug) into rows `alpha`(0), `beta`(1),
/// `gamma`(2) — no folders, so row indices are unambiguous.
fn sidebar_test_app_three_flat_rows() -> (App, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    for slug in ["alpha", "beta", "gamma"] {
        postui_core::storage::save_request(dir.path(), slug, &req("https://x/1")).unwrap();
    }
    let app = App::with_root(tx, dir.path().to_path_buf());
    (app, dir)
}

/// Regression test for the mouse-click travel-desync bug: keyboard-nav to
/// one row, then click a *different* row, must SNAP the travel band to the
/// clicked row instantly rather than leaving it animating (or frozen) on
/// wherever the keyboard cursor last settled. Also exercises the
/// coincide-wins ruling: the clicked request becomes both the cursor row
/// and the open row, so it must show the plain `▌`/`theme.selection`
/// treatment with a normal-colored (not `theme.accent`) name.
#[test]
fn click_after_keyboard_nav_snaps_the_travel_band_to_the_clicked_row() {
    let (mut app, _dir) = sidebar_test_app_three_flat_rows();
    render_once(&mut app);

    // Keyboard-select row 0 ("alpha"): lands the cursor and its travel anim
    // there.
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
    );
    assert_eq!(app.sidebar.selected, Some(0));

    // Click row 2 ("gamma") — a different row from the keyboard cursor.
    render_once(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(2)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    assert_eq!(
        app.sidebar.selected,
        Some(2),
        "the click moves the selection"
    );
    let now = std::time::Instant::now();
    let key = crate::anim::AnimKey::ListTravel(crate::anim::ListId::Sidebar);
    assert_eq!(
        app.anims.value(key, now),
        Some(2.0),
        "the travel anim snapped straight to the clicked row, not left \
         animating (or frozen) on the keyboard cursor's old row"
    );

    // Drawn: row 2 carries the plain selection fill/bar (cursor ==
    // clicked == now-open row, so open's accent-name styling doesn't
    // layer on top — the fill simply wins); row 0 (the stale keyboard
    // position) carries neither.
    render_once(&mut app);
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer();
    let row0 = app.hits.rect_of(&crate::hit::Hit::SidebarRow(0)).unwrap();
    let row2 = app.hits.rect_of(&crate::hit::Hit::SidebarRow(2)).unwrap();
    assert_eq!(
        buf[(row2.x, row2.y)].symbol(),
        "\u{258c}",
        "accent bar on the clicked row"
    );
    assert_eq!(
        buf[(row2.x + row2.width - 2, row2.y)].bg,
        app.theme.selection
    );
    assert_ne!(
        buf[(row0.x, row0.y)].symbol(),
        "\u{258c}",
        "no bar left behind on the stale keyboard-cursor row"
    );
}

/// Same regression, for the folder-arrow click path (which sets
/// `sidebar.selected` on its own line, separate from `Hit::SidebarRow`).
#[test]
fn folder_arrow_click_moves_only_the_cursor_not_the_travel_band() {
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);

    // Keyboard-select row 0 ("top").
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
    );
    assert_eq!(app.sidebar.selected, Some(0));

    // Click the folder arrow on row 1 ("api").
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::SidebarFolderArrow(1))
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    assert_eq!(app.sidebar.selected, Some(1));
    // The selection band tracks the OPEN request, and nothing is open
    // here: neither the keyboard nav nor the folder-arrow click may seed
    // or move the travel anim.
    let now = std::time::Instant::now();
    let key = crate::anim::AnimKey::ListTravel(crate::anim::ListId::Sidebar);
    assert_eq!(
        app.anims.value(key, now),
        None,
        "cursor moves must leave the open-request travel band untouched"
    );
}

/// Re-focusing the URL bar while it is already the focused input (e.g.
/// clicking the well the caret is already in) must not restart the focus
/// fade — restarting snaps `FocusFade` to 0, dropping the well's lifted
/// fill to the unfocused color for a frame before easing back: a visible
/// blink.
#[test]
fn refocusing_the_already_focused_url_bar_does_not_restart_the_fade() {
    let mut app = App::new_for_test();
    app.update(Action::FocusUrl);
    // Let the first fade finish.
    let key = crate::anim::AnimKey::FocusFade;
    app.anims.snap(key, 1.0);

    app.update(Action::FocusUrl);
    assert_eq!(
        app.anims.value(key, std::time::Instant::now()),
        Some(1.0),
        "an already-focused URL bar must keep its settled fade"
    );
}

/// Right-clicking a different request moves the cursor onto it only while
/// its context menu is open: dismissing the menu without choosing anything
/// restores the previous selection (both the click-off `Action::Close`
/// route and the Esc key route), so a row the user never acted on isn't
/// left looking targeted.
#[test]
fn dismissed_sidebar_context_menu_restores_the_previous_selection() {
    let (mut app, _dir) = sidebar_test_app_three_flat_rows();
    render_once(&mut app);
    // Open "alpha" (row 0): cursor and band both land on it.
    app.update(Action::ForceOpenRequest("alpha".into()));
    assert_eq!(app.sidebar.selected, Some(0));
    render_once(&mut app);

    // Right-click "gamma" (row 2): its menu opens and the cursor moves.
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(2)).unwrap();
    app.handle_mouse(right_down(r.x, r.y));
    assert!(matches!(app.modals.top(), Some(Modal::Dropdown(_))));
    assert_eq!(app.sidebar.selected, Some(2));

    // Click off (Hit::ModalOutside dispatches Action::Close).
    app.update(Action::Close);
    assert_eq!(
        app.sidebar.selected,
        Some(0),
        "dismissing the menu must undo the right-click's pre-selection"
    );

    // Same restore through the Esc key path.
    render_once(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(2)).unwrap();
    app.handle_mouse(right_down(r.x, r.y));
    assert_eq!(app.sidebar.selected, Some(2));
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.sidebar.selected, Some(0), "Esc dismissal restores too");
}

/// Regression test for the ghost-travel-band bug: `refresh_sidebar` can
/// re-map the OPEN request's row to a different index (a row above it
/// disappearing) without the open request itself changing. If the
/// `ListTravel` anim isn't snapped to match, it keeps easing toward the
/// OLD index forever (`settled` never becomes true), painting a stale
/// selection band/accent bar alongside the real one.
#[test]
fn deleting_a_row_above_the_selection_snaps_the_travel_band_not_a_ghost() {
    let (mut app, _dir) = sidebar_test_app_three_flat_rows();
    render_once(&mut app);

    // Open "gamma" (row 2) — the selection band anchors to it.
    app.update(Action::ForceOpenRequest("gamma".into()));
    assert_eq!(app.sidebar.selected, Some(2));
    render_once(&mut app); // let the travel anim settle at row 2

    // Delete "alpha" (row 0, above it) -- "gamma" is still the open
    // request, but its row index shifts from 2 to 1.
    app.update(Action::DeleteRequest("alpha".into()));
    assert_eq!(
        app.sidebar.selected,
        Some(1),
        "gamma re-maps to row 1 once alpha is gone"
    );

    let now = std::time::Instant::now();
    let key = crate::anim::AnimKey::ListTravel(crate::anim::ListId::Sidebar);
    assert_eq!(
        app.anims.value(key, now),
        Some(1.0),
        "the travel anim must snap to the re-mapped index, not keep \
         easing toward the old (now out-of-range-meaning) index 2"
    );

    // Only the real selected row (now row 1, "gamma") paints the accent
    // bar -- no ghost band left behind at the old row 2's position.
    render_once(&mut app);
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer();
    let row1 = app.hits.rect_of(&crate::hit::Hit::SidebarRow(1)).unwrap();
    assert_eq!(
        buf[(row1.x, row1.y)].symbol(),
        "\u{258c}",
        "accent bar on gamma's new row"
    );
    let bar_count = (0..buf.area.height)
        .filter(|&y| buf[(row1.x, y)].symbol() == "\u{258c}")
        .count();
    assert_eq!(bar_count, 1, "exactly one row carries the accent bar");
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
    app.anims.enabled = false;
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
            name: "ping".into(),
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
fn new_request_accepts_free_form_names_and_derives_the_slug() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "My Request!".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.slug.as_deref(), Some("my-request"));
    assert_eq!(app.editor.name.as_deref(), Some("My Request!"));
    let loaded = postui_core::storage::load_request(&app.project.root, "my-request").unwrap();
    assert_eq!(loaded.name.as_deref(), Some("My Request!"));
    assert!(
        rendered_text(&mut app).contains("Saved My Request!"),
        "toast names the request, not the slug"
    );
}

#[test]
fn new_request_blank_name_toasts_and_creates_nothing() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "folder/   ".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.toasts.is_empty(), "a blank name must toast");
    assert!(
        postui_core::storage::list_requests(&app.project.root)
            .0
            .is_empty()
    );
}

#[test]
fn new_request_same_display_name_toasts_and_creates_nothing() {
    let mut app = App::new_for_test();
    app.update(Action::CreateRequest("My Request!".into()));
    assert_eq!(
        postui_core::storage::list_requests(&app.project.root)
            .0
            .len(),
        1
    );
    app.update(Action::CreateRequest("my request!".into()));
    assert_eq!(
        postui_core::storage::list_requests(&app.project.root)
            .0
            .len(),
        1,
        "case-insensitive duplicate display name is rejected"
    );
    // A different name that merely collides on slug is fine and dedupes.
    app.update(Action::CreateRequest("My Request?".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("my-request-2"));
}

#[test]
fn rename_flow_speaks_display_names_and_regenerates_the_slug() {
    let mut app = App::new_for_test();
    app.update(Action::CreateRequest("Get User".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("get-user"));

    // The prompt prefills the display name, not the slug.
    app.sidebar.select_slug("get-user");
    app.refresh_sidebar();
    app.sidebar.select_slug("get-user");
    app.update(Action::PromptRenameRequest);
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("expected the rename prompt");
    };
    assert_eq!(input.text(), "Get User");
    app.modals.pop();

    // Renaming (with a sloppy trailing space) regenerates the slug and
    // rewrites the name.
    app.update(Action::RenameRequest {
        from: "get-user".into(),
        to: "Get User v2 ".into(),
    });
    assert_eq!(app.editor.slug.as_deref(), Some("get-user-v2"));
    assert_eq!(app.editor.name.as_deref(), Some("Get User v2"));
    assert_eq!(app.sidebar.open_slug.as_deref(), Some("get-user-v2"));
    let loaded = postui_core::storage::load_request(&app.project.root, "get-user-v2").unwrap();
    assert_eq!(loaded.name.as_deref(), Some("Get User v2"));
}

#[test]
fn delete_confirm_and_duplicate_toast_show_display_names() {
    let mut app = App::new_for_test();
    app.update(Action::CreateRequest("Fancy Name!".into()));
    app.sidebar.select_slug("fancy-name");
    app.refresh_sidebar();
    app.sidebar.select_slug("fancy-name");

    app.update(Action::ConfirmDeleteRequest);
    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("expected the delete confirm");
    };
    assert!(
        body.contains("Fancy Name!"),
        "display name in confirm: {body}"
    );
    app.modals.pop();

    app.update(Action::DuplicateRequest);
    assert!(
        rendered_text(&mut app).contains("Duplicated to Fancy Name! copy"),
        "duplicate toast names the copy"
    );
}

#[test]
fn saving_a_legacy_request_does_not_invent_a_name() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "legacy", &req("https://x/a")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("legacy".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    app.update(Action::SaveRequest);
    let loaded = postui_core::storage::load_request(dir.path(), "legacy").unwrap();
    assert_eq!(loaded.name, None, "no name field appears uninvited");
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
    assert!(app.session.in_flight.is_empty());
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
    assert!(app.session.in_flight.is_empty());
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
        app.session.in_flight.is_empty(),
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

#[test]
fn modal_prompt_field_supports_click_to_place_drag_select_and_double_click() {
    // Modal text boxes share `LineInput` with the URL bar — the mouse
    // affordances (click places the caret, drag sweeps a selection,
    // double click selects all) must work there too.
    let mut app = App::new_for_test();
    // Animations off so the modal's settle-in doesn't hide its fields on
    // the single test frame.
    app.anims.enabled = false;
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "hello".chars() {
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalInput(0))
        .expect("prompt input hit");

    // Click lands the caret on the clicked char (inner text starts two
    // cells into the box, on its middle row).
    app.handle_mouse(left_down(r.x + 2 + 2, r.y + 1));
    let input = app.modals.focused_input().expect("prompt input");
    assert_eq!(input.cursor(), 2, "caret placed at the clicked column");

    // Dragging right extends a selection from the click's anchor.
    app.handle_mouse(dragged(r.x + 2 + 4, r.y + 1));
    let input = app.modals.focused_input().expect("prompt input");
    assert_eq!(input.selection(), Some((2, 4)), "drag swept a selection");

    // A second click within the double-click window selects the whole
    // text (the sweep's own press was click #1).
    app.handle_mouse(left_up(r.x + 2 + 2, r.y + 1));
    app.handle_mouse(left_down(r.x + 2 + 2, r.y + 1));
    let input = app.modals.focused_input().expect("prompt input");
    assert_eq!(input.selection(), Some((0, 5)), "double click selects all");
}

#[tokio::test]
async fn shift_enter_sends_even_while_the_body_editor_has_focus() {
    // Shift+Enter is a global Send shortcut that must win over the focused
    // component — plain Enter in the body editor still inserts a newline.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = crate::components::editor::SubFocus::Content;
    app.editor.set_body_text("{}");

    let keymap = Keymap::default_bindings();
    let ev = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    app.handle_key(&keymap, ev);
    assert!(!app.session.in_flight.is_empty(), "shift+enter sent");
    assert_eq!(
        app.editor.body_text(),
        "{}",
        "no newline leaked into the body"
    );
}

#[tokio::test]
async fn force_send_spawns_a_task_and_marks_response_in_flight() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9"); // unroutable, never actually hit
    app.update(Action::ForceSend);
    assert!(!app.session.in_flight.is_empty());
    assert!(matches!(
        app.session.response.state(),
        ResponseState::InFlight { .. }
    ));
    assert_eq!(app.session.send_generation, 1);
}

#[tokio::test]
async fn sidebar_mirrors_which_requests_are_in_flight() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.session.in_flight.push(crate::session::InFlight {
        started: std::time::Instant::now(),
        generation: 1,
        slug: Some("ping".into()),
        task: tokio::spawn(async {}),
    });
    app.update(Action::Render);
    assert!(app.sidebar.in_flight.contains("ping"));

    app.session.in_flight.clear();
    app.update(Action::Render);
    assert!(app.sidebar.in_flight.is_empty());
}

#[tokio::test]
async fn send_is_a_noop_while_the_open_request_is_in_flight() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::ForceSend);
    let generation = app.session.in_flight[0].generation;

    app.update(Action::Send);
    app.update(Action::ForceSend);
    assert_eq!(
        app.session.in_flight.len(),
        1,
        "a request already in flight cannot be sent again"
    );
    assert_eq!(
        app.session.in_flight[0].generation, generation,
        "the original send is untouched, not superseded"
    );
}

#[tokio::test]
async fn esc_nothing_else_consumed_cancels_the_open_requests_send() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::ForceSend);
    assert!(!app.session.in_flight.is_empty());

    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        app.session.in_flight.is_empty(),
        "a bare esc cancels the open request's send from any pane"
    );
    assert!(matches!(
        app.session.response.state(),
        ResponseState::Cancelled
    ));
}

#[tokio::test]
async fn cancel_send_aborts_task_and_marks_cancelled() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::ForceSend);
    assert!(!app.session.in_flight.is_empty());
    app.update(Action::CancelSend);
    assert!(app.session.in_flight.is_empty());
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
    // A delivery only counts while its send is still tracked — fabricate
    // the tracked entry the real send path would have pushed.
    app.session.in_flight.push(crate::session::InFlight {
        started: std::time::Instant::now(),
        generation: 1,
        slug: None,
        task: tokio::spawn(async {}),
    });
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
    assert!(app.session.in_flight.is_empty());
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
/// accessor beyond `is_empty`). Settles every in-flight animation first
/// (`Anims::finish_all`) — this helper's whole purpose is asserting
/// static content, which would otherwise land mid-flight of whatever
/// animation the action under test happened to also start (e.g. a
/// freshly pushed toast's own slide-in, off in its first frame).
fn rendered_text(app: &mut App) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    app.anims.finish_all();
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
    assert!(app.session.in_flight.is_empty());
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

/// The toolbar's `{{ }} vars` chip is the *mouse* route into the picker
/// (spec §5), so clicking it must not blur the field it is going to insert
/// into — found by the stage-7 tmux sweep, where the chip could only ever
/// answer "nowhere to insert".
#[test]
fn the_vars_chip_keeps_the_url_focused_so_the_picker_can_insert() {
    let mut app = app_with_vars();
    app.anims.enabled = false;
    app.update(Action::FocusUrl);
    assert_eq!(app.editor.sub_focus, SubFocus::Url);

    click_hit(
        &mut app,
        Hit::FooterChip(Action::OpenVarPicker { completing: false }),
    );
    assert!(matches!(app.modals.top(), Some(Modal::VarPicker(_))));
    assert_eq!(
        app.editor.sub_focus,
        SubFocus::Url,
        "the chip is not a click-away from the line it inserts into"
    );

    click_hit(&mut app, Hit::VarPickerRow(0));
    assert_eq!(app.editor.url.text(), "{{base}}");
    assert!(
        app.toasts.is_empty(),
        "no \"nowhere to insert\" — the caret was still there"
    );
}

/// Same rule for a live table cell: the chip inserts into what is being
/// typed rather than committing it away first.
#[test]
fn the_vars_chip_inserts_into_a_table_cell_under_edit() {
    let mut app = app_with_vars();
    app.anims.enabled = false;
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_chars(&mut app, "x");
    click_hit(
        &mut app,
        Hit::FooterChip(Action::OpenVarPicker { completing: false }),
    );
    click_hit(&mut app, Hit::VarPickerRow(0));
    let edit = app
        .editor
        .table
        .editing
        .as_ref()
        .expect("the cell is still under edit");
    assert_eq!(edit.input.text(), "x{{base}}");
}

#[test]
fn picker_with_no_declared_vars_still_offers_the_new_variable_row() {
    // Task 15: the picker no longer needs anything declared — the "new
    // variable…" row makes it a creation flow too, so opening it with an
    // empty project stays open on that one row instead of toasting.
    let mut app = App::new_for_test();
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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

    // Position 3 (Body) still maps correctly too (for a method that has one).
    let mut app = App::new_for_test();
    app.update(Action::SetMethod(postui_core::model::Method::Post));
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
fn ctrl_c_copies_a_table_cell_selection_and_keeps_the_edit_live() {
    use crate::components::table_editor::{CellEdit, Col};
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    let mut input = crate::components::line_input::LineInput::new("page");
    input.select_all();
    app.editor.table.editing = Some(CellEdit {
        row: 0,
        col: Col::Key,
        input,
        original: "page".into(),
    });

    app.handle_key(&Keymap::default_bindings(), ctrl('c'));

    assert!(!app.should_quit);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "page");
    assert!(
        app.editor.table.editing.is_some(),
        "the cell edit stays live"
    );
}

#[test]
fn ctrl_c_copies_a_var_form_selection_on_the_varmanager_screen() {
    use crate::components::varmanager::VmField;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    app.screen = Screen::VarManager;
    let mut input = crate::components::line_input::LineInput::new("token value");
    input.select_all();
    app.varmanager.form.editing = Some((VmField::Default, input));

    app.handle_key(&Keymap::default_bindings(), ctrl('c'));

    assert!(!app.should_quit);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "token value");
    assert!(app.varmanager.form.editing.is_some(), "the edit stays live");
}

#[test]
fn url_bar_drag_selects_and_double_click_selects_all() {
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://example.com");
    render_once(&mut app);
    let area = app.editor.last_url_text_area.expect("url area recorded");

    // Click at the start, sweep 5 cells right: "https" selected.
    app.handle_mouse(left_down(area.x, area.y));
    assert!(app.handle_mouse(dragged(area.x + 5, area.y)));
    app.handle_mouse(left_up(area.x + 5, area.y));
    assert_eq!(app.editor.url.selected_text().as_deref(), Some("https"));

    // A double click selects the whole URL. (Reset the click pairing so
    // the sweep's Down above can't count as this pair's first click.)
    app.last_click = None;
    app.handle_mouse(left_down(area.x + 2, area.y));
    app.handle_mouse(left_down(area.x + 2, area.y)); // within 400ms => clicks == 2
    assert_eq!(
        app.editor.url.selected_text().as_deref(),
        Some("https://example.com")
    );

    // A later plain click collapses the selection.
    app.handle_mouse(left_down(area.x + 1, area.y));
    assert_eq!(app.editor.url.selection(), None);
}

#[test]
fn dragging_in_the_response_selects_and_ctrl_c_copies_it() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    ready_response(&mut app, "plain text body"); // not JSON -> Raw view
    render_once(&mut app);
    let area = app
        .session
        .response
        .view()
        .unwrap()
        .last_area
        .expect("body area recorded");
    app.handle_mouse(left_down(area.x, area.y));
    assert!(app.handle_mouse(dragged(area.x + 4, area.y)));
    app.handle_mouse(left_up(area.x + 4, area.y));
    assert_eq!(
        app.session.response.selected_text().as_deref(),
        Some("plain")
    );

    app.handle_key(&Keymap::default_bindings(), ctrl('c'));
    assert!(!app.should_quit, "copy pre-empts quit");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "plain");
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

/// Task 17, spec §5: the Response pane's footer chips are clickable, and
/// clicking them does what the `r`/`/` keys do.
#[test]
fn click_footer_response_chips_toggle_view_and_open_search() {
    use crate::components::response::ViewMode;
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": 1}"#);
    app.focus = PaneId::Response;
    render_once(&mut app);

    let r = app
        .hits
        .rect_of(&Hit::FooterChip(Action::ResponseViewMode(ViewMode::Raw)))
        .expect("the 'r' chip is registered");
    app.handle_mouse(left_down(r.x + 1, r.y));
    assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Raw);

    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&Hit::FooterChip(Action::OpenResponseSearch))
        .expect("the '/' chip is registered");
    app.handle_mouse(left_down(r.x + 1, r.y));
    assert!(
        app.session.response.view().unwrap().search.is_some(),
        "clicking the search chip opens the in-pane search"
    );
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
        .expect("Find button");
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
    app.editor.active_tab = EditorTab::Params;
    app.editor.params.insert(
        "page".into(),
        postui_core::model::Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    hover_row_then_click(&mut app, Hit::TableRow(0), Hit::TableCheckbox(0));
    assert!(!app.editor.params["page"].enabled);
    assert_eq!(
        app.editor.table.selected, None,
        "a toggle click toggles — it never selects the row"
    );
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
fn collapse_hides_body_and_fades_the_tab_labels_out() {
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

    // Hiding hides the controls too: the tab labels (count included) fade
    // out entirely, leaving only the `› show` toggle on the row.
    assert!(
        !content.contains("Params · 3"),
        "tab labels are invisible while hidden: {content}"
    );
    assert!(content.contains("show"), "{content}");
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
fn collapse_on_a_table_tab_shrinks_editor_to_a_row_and_grows_response() {
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
        crate::components::editor::COLLAPSED_HEIGHT,
        "editor pane shrinks to exactly its one-row strip"
    );
    assert!(
        response.height > expanded_response.height,
        "response pane reclaims the freed rows"
    );
}

#[test]
fn collapse_on_the_body_tab_shrinks_editor_to_chrome_too() {
    let mut app = App::new_for_test();
    three_params(&mut app);
    app.editor.active_tab = EditorTab::Body;
    render_once(&mut app);
    let expanded_response = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();

    app.table_collapsed = true;
    app.editor.table_collapsed = true;
    render_once(&mut app);
    let editor = app.hits.rect_of(&Hit::Pane(PaneId::Editor)).unwrap();
    let response = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    assert_eq!(
        editor.height,
        crate::components::editor::COLLAPSED_HEIGHT,
        "Body tab active: hide collapses the editor all the same"
    );
    assert!(
        response.height > expanded_response.height,
        "response pane reclaims the freed rows"
    );
}

/// The `AnimKey::ResponseCollapse` target is per-request state
/// (`session.response.collapsed`), but the anim is global: switching to a
/// request whose response isn't collapsed must re-open the pane rather
/// than leave the layout squashed under a stale 1.0.
#[test]
fn response_collapse_reopens_when_switching_to_an_expanded_request() {
    // Disabled anims: each retarget lands instantly, so the layout can be
    // asserted right after the update that moves it.
    let mut app = App::new_for_test_with_anims(false);
    app.update(Action::ToggleResponseCollapse);
    render_once(&mut app);
    let hidden = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    assert_eq!(
        hidden.height,
        crate::components::response::COLLAPSED_HEIGHT,
        "toggle collapses the pane"
    );

    // Opening a different request swaps in that request's own response,
    // which is not collapsed.
    app.editor.slug = Some("other".into());
    app.update(Action::Render);
    assert!(!app.session.response.collapsed);
    render_once(&mut app);
    let reopened = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    assert!(
        reopened.height > hidden.height,
        "the pane re-opens with the swapped-in response: {reopened:?}"
    );
}

/// Hiding the only expanded panel would leave the screen blank: the
/// panels swap instead — the clicked one hides and the other expands.
#[test]
fn hiding_the_expanded_editor_swaps_the_panels() {
    let mut app = App::new_for_test_with_anims(false);
    app.update(Action::ToggleResponseCollapse);
    assert!(app.session.response.collapsed);

    app.update(Action::ToggleTableCollapse);
    assert!(app.table_collapsed, "the editor hides");
    assert!(
        !app.session.response.collapsed,
        "the response expands instead of leaving the screen blank"
    );
    render_once(&mut app);
    let response = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    assert!(
        response.height > crate::components::response::COLLAPSED_HEIGHT,
        "response pane takes the freed space: {response:?}"
    );
}

/// The same no-blank-screen rule holds when a hidden response arrives by
/// request switch rather than by toggle: the swapped-in response keeps its
/// hidden state, so the editor expands.
#[test]
fn switching_to_a_hidden_response_while_the_editor_is_hidden_expands_the_editor() {
    let mut app = App::new_for_test_with_anims(false);
    // Hide request A's response, then switch away — A caches as hidden
    // (a non-Empty state, so `sync_open` keeps its cache slot).
    app.editor.slug = Some("a".into());
    app.update(Action::Render);
    app.session
        .response
        .set_state(crate::components::response::ResponseState::Cancelled, 0);
    app.update(Action::ToggleResponseCollapse);
    app.editor.slug = Some("b".into());
    app.update(Action::Render);
    assert!(!app.session.response.collapsed);

    // Hide the editor on B, then switch back to A: both flags would be
    // set — the editor must re-open.
    app.update(Action::ToggleTableCollapse);
    app.editor.slug = Some("a".into());
    app.update(Action::Render);
    assert!(
        app.session.response.collapsed,
        "A's response is still hidden"
    );
    assert!(
        !app.table_collapsed,
        "the editor expands instead of leaving the screen blank"
    );
}

#[test]
fn hiding_the_expanded_response_swaps_the_panels() {
    let mut app = App::new_for_test_with_anims(false);
    app.update(Action::ToggleTableCollapse);
    assert!(app.table_collapsed);

    app.update(Action::ToggleResponseCollapse);
    assert!(app.session.response.collapsed, "the response hides");
    assert!(
        !app.table_collapsed,
        "the editor expands instead of leaving the screen blank"
    );
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

/// Opening the method dropdown retargets `AnimKey::DropdownOpen` 0→1 over
/// `ui_settings.anim_ms.dropdown_open` (90ms by default); every close path
/// then snaps it straight back to 1 — overlay close is always instant, no
/// exception for this popup's own open-settle motion.
#[test]
fn method_dropdown_open_settles_then_every_close_path_snaps_instantly() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let badge = app.hits.rect_of(&Hit::MethodSelector).unwrap();

    // Open: the animation starts short of 1 (still easing in).
    app.handle_mouse(left_down(badge.x, badge.y));
    let now = std::time::Instant::now();
    assert!(
        app.anims.value(AnimKey::DropdownOpen, now).unwrap() < 1.0,
        "opening starts the settle animation short of 1"
    );

    // Close via Esc (`Action::Close`): snaps instantly.
    app.update(Action::Close);
    assert_eq!(app.anims.value(AnimKey::DropdownOpen, now), Some(1.0));

    // Re-open, then close by clicking a row (`Hit::DropdownRow`'s own pop).
    // Freshly rendered first so the click resolves against this frame's
    // hits — a stale `HitMap` from the still-open popup still has
    // `ModalOutside` covering the badge, same as any real redraw cadence
    // would refresh before the next click lands.
    render_once(&mut app);
    app.handle_mouse(left_down(badge.x, badge.y));
    assert!(app.anims.value(AnimKey::DropdownOpen, now).unwrap() < 1.0);
    render_once(&mut app);
    let row0 = app.hits.rect_of(&Hit::DropdownRow(0)).unwrap();
    app.handle_mouse(left_down(row0.x, row0.y));
    assert_eq!(app.anims.value(AnimKey::DropdownOpen, now), Some(1.0));

    // Re-open, then close by clicking outside (`Hit::ModalOutside` →
    // `Action::Close`, via `apply_modal_result`'s Esc-key twin path is
    // covered above; this exercises the `on_hit` `ModalOutside` arm).
    render_once(&mut app);
    app.handle_mouse(left_down(badge.x, badge.y));
    assert!(app.anims.value(AnimKey::DropdownOpen, now).unwrap() < 1.0);
    render_once(&mut app);
    app.handle_mouse(left_down(0, 0));
    assert!(app.modals.is_empty());
    assert_eq!(app.anims.value(AnimKey::DropdownOpen, now), Some(1.0));
}

/// `AnimKey::ModalOpen` — the panel-style shell's own open-settle — only
/// retargets from 0 on an empty→non-empty push; pushing a second modal on
/// top of an already-open one snaps it straight to 1 instead (no
/// re-animation of an already-visible shell). Every close path then snaps
/// it back to 1, same convention as `AnimKey::DropdownOpen`.
#[test]
fn modal_open_retargets_only_on_empty_to_non_empty_push() {
    let mut app = App::new_for_test();
    let now = std::time::Instant::now();

    // First push (empty -> non-empty): starts the settle animation short
    // of 1 — still easing in.
    app.update(Action::OpenPalette);
    assert!(
        app.anims.value(AnimKey::ModalOpen, now).unwrap() < 1.0,
        "opening the first modal on an empty stack starts the settle animation short of 1"
    );

    // Second push onto the now non-empty stack: snaps straight to 1, no
    // re-animation.
    app.update(Action::ShowAbout);
    assert_eq!(
        app.anims.value(AnimKey::ModalOpen, now),
        Some(1.0),
        "a modal pushed on top of another must not re-trigger the settle"
    );

    // Close (pop back to the palette): instant, snaps to 1.
    app.update(Action::Close);
    assert!(!app.modals.is_empty(), "the palette is still on the stack");
    assert_eq!(app.anims.value(AnimKey::ModalOpen, now), Some(1.0));

    // Close again (stack now empty): still instant.
    app.update(Action::Close);
    assert!(app.modals.is_empty());
    assert_eq!(app.anims.value(AnimKey::ModalOpen, now), Some(1.0));
}

/// A `Modal::Dropdown` push must never touch `AnimKey::ModalOpen` — it
/// settles only via its own `AnimKey::DropdownOpen` (see
/// `method_dropdown_open_settles_then_every_close_path_snaps_instantly`).
/// Pushing a dropdown on top of an *empty* stack (no panel modal open) must
/// leave `ModalOpen` untouched, and pushing one on top of an *already open*
/// panel modal must not snap its still-animating settle to 1 early.
#[test]
fn dropdown_push_never_touches_modal_open() {
    let mut app = App::new_for_test();
    let now = std::time::Instant::now();

    // Dropdown pushed onto an empty stack: `ModalOpen` stays untouched
    // (never set at all).
    app.update(Action::OpenMethodDropdown);
    assert_eq!(app.anims.value(AnimKey::ModalOpen, now), None);
    app.update(Action::Close);

    // A panel modal opens and is still mid-settle...
    app.update(Action::OpenPalette);
    let mid_settle = app.anims.value(AnimKey::ModalOpen, now).unwrap();
    assert!(mid_settle < 1.0);

    // ...then a dropdown (e.g. a right-click context menu) pushes on top:
    // `ModalOpen`'s own in-flight settle must be left exactly as it was,
    // not snapped to 1.
    app.update(Action::OpenMethodDropdown);
    assert_eq!(
        app.anims.value(AnimKey::ModalOpen, now),
        Some(mid_settle),
        "a Dropdown push must not perturb ModalOpen's own in-flight settle"
    );
}

/// Regression for a review finding: `Action::OpenEnvChooser` originally
/// pushed straight onto `self.modals` (a two-line `self.modals\n.push(...)`
/// form a single-line grep for `self.modals.push(` missed) instead of going
/// through `push_modal`, so the Environments chooser never retargeted
/// `AnimKey::ModalOpen` on open. Mirrors
/// `modal_open_retargets_only_on_empty_to_non_empty_push`, but for this
/// specific push site, so a future modal-opening action that bypasses
/// `push_modal` fails a test rather than only a grep sweep.
#[test]
fn env_chooser_open_retargets_modal_open_on_empty_to_non_empty_push() {
    let (mut app, _dir) = app_with_envs();
    let now = std::time::Instant::now();

    app.update(Action::OpenEnvChooser);
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
        "sanity: the chooser actually opened"
    );
    assert!(
        app.anims.value(AnimKey::ModalOpen, now).unwrap() < 1.0,
        "opening the env chooser on an empty stack must start the settle animation short of 1"
    );
}

#[test]
fn click_palette_row_runs_immediately() {
    let mut app = App::new_for_test();
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;
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
/// `Hit::SendButton` handler checking `is_in_flight(&editor.slug)`.
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
    assert!(!app.session.in_flight.is_empty(), "click dispatches Send");
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

#[test]
fn address_bar_copy_chip_is_clickable_and_copies_url() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(
        Some(cmd),
        65536,
        false,
    ));
    app.editor.url = crate::components::line_input::LineInput::new("https://example.com/x");

    render_once(&mut app);
    assert!(
        app.hits.rect_of(&Hit::CopyUrl).is_some(),
        "the address bar must register a CopyUrl hit for its chip"
    );
    click_hit(&mut app, Hit::CopyUrl);

    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "https://example.com/x",
        "the chip must copy via the same clipboard path the palette's \
         \"Request: copy URL\" command uses"
    );
    assert!(
        rendered_text(&mut app).contains("Copied URL"),
        "toast confirms the copy, same as the palette command"
    );
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
        !content.contains("New request"),
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
    assert!(app.session.in_flight.is_empty());
    assert_eq!(app.screen, crate::app::Screen::VarManager);

    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
    );
    assert!(
        app.toasts.is_empty(),
        "ctrl+enter must not reach Action::Send either"
    );
    assert!(app.session.in_flight.is_empty());
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
fn var_edit_set_entry_value_writes_one_field_of_the_entry_in_that_env() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarEdit(VarEditOp::SetEntryValue {
        env: "qa".into(),
        group: "user".into(),
        entry: "alice".into(),
        field: "user".into(),
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

    app.update(Action::VarEdit(VarEditOp::SelectEntry {
        env: "dev".into(),
        group: "user".into(),
        entry: "bob".into(),
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
    app.anims.enabled = false;
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
    app.anims.enabled = false;

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

    assert!(app.session.in_flight.is_empty());
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
    assert!(app.session.in_flight.is_empty());
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
    assert!(app.session.in_flight.is_empty());
    let Some(Modal::Prompt { title, kind, .. }) = app.modals.top() else {
        panic!("expected the second secret prompt");
    };
    assert!(title.contains("api_secret"), "title: {title}");
    assert!(matches!(kind, PromptKind::SecretValue { name, .. } if name == "api_secret"));

    type_and_confirm(&mut app, &keymap, "secret-val");

    // Both secrets resolved: the send actually goes out this time.
    assert!(app.modals.is_empty());
    assert!(!app.session.in_flight.is_empty());

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
    assert!(app.session.in_flight.is_empty(), "nothing was sent");
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
    app.anims.enabled = false;
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

/// Review finding 1: entry names are free-form strings (spec §3.2), unlike
/// variable names — the inline-create path must not run them through
/// `is_valid_var_name`'s charset check.
#[test]
fn inline_create_accepts_a_free_form_entry_name_with_a_space() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    focus_url_with_cursor_on(&mut app, "https://x/{{user}}", "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, enter_key());

    type_into_field(&mut app, &keymap, "user 1");
    app.handle_key(&keymap, tab_key());
    type_into_field(&mut app, &keymap, "9009");
    app.handle_key(&keymap, enter_key());

    assert!(
        app.modals.is_empty(),
        "a free-form name must not be rejected: {:?}",
        app.toasts.messages()
    );
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("[entries.user.\"user 1\"]"), "{env_doc}");
    assert!(env_doc.contains("9009"), "{env_doc}");
}

/// Review finding 4: an inline-created entry on a multi-field group only
/// fills the first field — the rest start empty — so a hint toast must
/// point the user at the Manager to fill them in.
#[test]
fn inline_create_on_a_multi_field_group_hints_at_the_empty_fields() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    focus_url_with_cursor_on(&mut app, "https://x/{{user_id}}", "{{user_id}}");
    app.update(Action::OpenVarPicker { completing: false });
    // "identity" has two entries (alice, bob); the ghost row sits one past
    // them.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, enter_key());

    type_into_field(&mut app, &keymap, "carol");
    app.handle_key(&keymap, enter_key());

    assert!(app.modals.is_empty());
    let msgs = app.toasts.messages();
    assert!(
        msgs.iter().any(|m| m.contains("empty")),
        "expected a hint about the unfilled fields: {msgs:?}"
    );
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

// --- Task 15: variable detail form (spec §3.4) ---------------------------

use crate::components::varmanager::VmField;

/// The form's full column of rows doesn't fit `rendered_text`'s 80x24
/// screen (header + footer leave the right pane only a little taller than
/// its title + description + default fields) — plenty for a real terminal,
/// but these tests need the rest of the column too.
fn rendered_text_tall(app: &mut App) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    app.anims.finish_all();
    let backend = TestBackend::new(100, 46);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

fn field_rect(app: &mut App, field: VmField) -> ratatui::layout::Rect {
    rendered_text_tall(app);
    app.hits
        .rect_of(&crate::hit::Hit::VmFormField(field))
        .unwrap_or_else(|| panic!("no rect for {field:?}"))
}

#[test]
fn selecting_a_var_renders_its_form_fields() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    let content = rendered_text_tall(&mut app);
    assert!(content.contains("Description"), "{content}");
    assert!(content.contains("API root"), "{content}");
    assert!(content.contains("Default"), "{content}");
    assert!(content.contains("http://localhost:8080"), "{content}");
    assert!(content.contains("Value in qa"), "{content}");
    assert!(
        content.contains("https://qa.example.com"),
        "qa's own override, not the declaration default: {content}"
    );
}

#[test]
fn clicking_the_env_value_field_typing_and_clicking_away_writes_the_env_file() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    assert!(app.varmanager.form.editing.is_some(), "the field is live");

    let keymap = Keymap::default_bindings();
    for c in "9".chars() {
        app.handle_key(&keymap, plain(c));
    }

    // Click away — the left list row for the same variable is "elsewhere".
    let row = app.varmanager.left_cursor;
    let left_rect = app.hits.rect_of(&crate::hit::Hit::VmLeftRow(row)).unwrap();
    app.handle_mouse(left_down(left_rect.x + 1, left_rect.y + 1));

    assert!(app.varmanager.form.editing.is_none(), "the click committed");
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        on_disk.contains("https://qa.example.com9"),
        "the typed digit landed at the caret (end of qa's own override): {on_disk}"
    );
    assert_eq!(
        app.project.resolved.values["base_url"],
        "https://qa.example.com9"
    );
}

#[test]
fn enter_commits_a_field_edit_and_esc_reverts_it() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    let keymap = Keymap::default_bindings();

    // Esc reverts: the typed digit never reaches disk.
    let r = field_rect(&mut app, VmField::Description);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    app.handle_key(&keymap, plain('!'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.varmanager.form.editing.is_none());
    assert_eq!(
        app.project.model.vars["base_url"].description.as_deref(),
        Some("API root"),
        "Esc must not write anything"
    );

    // Enter commits.
    let r = field_rect(&mut app, VmField::Description);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    app.handle_key(&keymap, plain('!'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.varmanager.form.editing.is_none());
    assert_eq!(
        app.project.model.vars["base_url"].description.as_deref(),
        Some("API root!")
    );
}

/// Clicking straight from one form field into a *different* one (no
/// intervening click-away) must commit the first field rather than
/// silently discarding it — a regression the top-of-`on_hit` guard's
/// `VmFormField(_)` exemption briefly reintroduced.
#[test]
fn clicking_directly_from_one_field_into_another_commits_the_first() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    let r = field_rect(&mut app, VmField::Description);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let keymap = Keymap::default_bindings();
    for c in "!".chars() {
        app.handle_key(&keymap, plain(c));
    }

    // Straight into the env-value field — no click-away in between.
    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));

    assert_eq!(
        app.project.model.vars["base_url"].description.as_deref(),
        Some("API root!"),
        "the description field must have committed, not been discarded"
    );
    assert_eq!(
        app.varmanager.form.editing.as_ref().map(|(f, _)| *f),
        Some(VmField::EnvValue),
        "the click landed in the newly clicked field"
    );
}

/// The write-failure variant of the above: when the first field's commit
/// fails, the click into the second field must not clobber the restored
/// (still-live) edit with a fresh one on the field that was clicked.
#[test]
fn clicking_into_another_field_after_a_failed_commit_keeps_the_original_edit_live() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(None));
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("api_key".into())
    });

    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let keymap = Keymap::default_bindings();
    for c in "sk-typed-secret".chars() {
        app.handle_key(&keymap, plain(c));
    }

    // Click straight into Description — the env-value commit must fail
    // first (a secret has no active env to target and can't hold a
    // default), so this must not switch away from it.
    let r = field_rect(&mut app, VmField::Description);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));

    assert_eq!(
        app.varmanager.form.editing.as_ref().map(|(f, _)| *f),
        Some(VmField::EnvValue),
        "the failed commit's field stays live rather than switching to the click"
    );
    assert_eq!(
        app.varmanager
            .form
            .editing
            .as_ref()
            .map(|(_, i)| i.text().to_string()),
        Some("sk-typed-secret".to_string()),
        "its typed text is untouched"
    );
    assert!(!app.toasts.is_empty(), "the failed commit still toasts");
    for msg in app.toasts.messages() {
        assert!(!msg.contains("sk-typed-secret"), "{msg}");
    }
}

/// Review finding 2: clicking a DIFFERENT left-list row after a failed
/// form commit must not reset `form` (which would discard the typed text
/// the failure left live) — the click is absorbed instead.
#[test]
fn clicking_a_different_left_row_after_a_failed_commit_keeps_the_original_edit_live() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(None));
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("api_key".into())
    });
    rendered_text_tall(&mut app);

    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let keymap = Keymap::default_bindings();
    for c in "sk-typed-secret".chars() {
        app.handle_key(&keymap, plain(c));
    }

    let other = app
        .varmanager
        .left_rows
        .iter()
        .position(|r| r == &crate::components::varmanager::VmRow::Var("base_url".into()))
        .expect("base_url row present");
    let left_rect = app
        .hits
        .rect_of(&crate::hit::Hit::VmLeftRow(other))
        .unwrap();
    app.handle_mouse(left_down(left_rect.x + 1, left_rect.y + 1));

    assert_eq!(
        app.varmanager.detail,
        crate::components::varmanager::VmDetail::Var("api_key".into()),
        "the click must not move the detail pane off the failed edit"
    );
    assert_eq!(
        app.varmanager
            .form
            .editing
            .as_ref()
            .map(|(_, i)| i.text().to_string()),
        Some("sk-typed-secret".to_string()),
        "the typed text must survive the click on another row"
    );
}

/// A secret var with no active environment: the value field falls back to
/// targeting the declaration default (spec's stated fallback), which the
/// model rejects for a secret (`SecretWithDefault`) — the write fails, the
/// typed text stays in the live editor, and the failure toasts without the
/// secret value ever appearing in it.
#[test]
fn a_write_failure_keeps_the_typed_text_and_toasts_without_the_secret_value() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(None));
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("api_key".into())
    });

    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let keymap = Keymap::default_bindings();
    for c in "sk-typed-secret".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(
        app.varmanager.form.editing.is_some(),
        "a failed write must keep the field live with its typed text"
    );
    assert_eq!(
        app.varmanager
            .form
            .editing
            .as_ref()
            .map(|(_, i)| i.text().to_string()),
        Some("sk-typed-secret".to_string())
    );
    assert!(!app.toasts.is_empty(), "the failure must toast");
    for msg in app.toasts.messages() {
        assert!(
            !msg.contains("sk-typed-secret"),
            "a secret's value must never appear in a toast: {msg}"
        );
    }
}

#[test]
fn a_secret_var_masks_its_value_and_the_reveal_toggle_unmasks_it() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarEdit(VarEditOp::SetSecretValue {
        env: "qa".into(),
        name: "api_key".into(),
        value: "sk-live-secret".into(),
    }));
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("api_key".into())
    });

    let content = rendered_text_tall(&mut app);
    assert!(!content.contains("Default"), "{content}");
    assert!(!content.contains("sk-live-secret"), "{content}");
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::VmRevealToggle)
        .expect("reveal toggle registered for a secret");
    app.handle_mouse(left_down(r.x, r.y));
    let content = rendered_text_tall(&mut app);
    assert!(content.contains("sk-live-secret"), "{content}");
}

#[test]
fn the_rename_button_opens_the_same_prompt_as_the_e_key() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    rendered_text_tall(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::VmRename).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::RenameVariable { .. },
            ..
        })
    ));
}

#[test]
fn the_delete_button_opens_the_confirm_with_the_usage_list() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    request_with_var(dir.path(), "ping", "trace_id", "abc-123");
    postui_core::storage::save_request(
        dir.path(),
        "uses-base",
        &req("https://x/uses-base/{{base_url}}"),
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    rendered_text_tall(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::VmDelete).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("expected a delete confirm");
    };
    assert!(body.contains("uses-base"), "{body}");
}

#[test]
fn the_secret_toggle_button_opens_the_same_confirm_as_the_s_key() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    rendered_text_tall(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::VmSecretToggle).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
}

#[test]
fn the_promote_button_promotes_the_requests_override_up_into_the_project() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    request_with_var(dir.path(), "ping", "base_url", "http://from-request");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    let content = rendered_text_tall(&mut app);
    assert!(content.contains("Promote"), "{content}");

    let r = app.hits.rect_of(&crate::hit::Hit::VmPromoteBtn).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    // Confirm "Default value".
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert_eq!(
        app.project.model.vars["base_url"].default.as_deref(),
        Some("http://from-request")
    );
    assert!(!app.editor.variables.contains_key("base_url"));
}

#[test]
fn the_demote_button_opens_the_demote_confirm_when_no_override_exists() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "ping", &req("https://x/ping")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("ping".into()));
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    let content = rendered_text_tall(&mut app);
    assert!(content.contains("Demote"), "{content}");

    let r = app.hits.rect_of(&crate::hit::Hit::VmPromoteBtn).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
}

/// Keyboard parity: `e`/`F2` rename and `s` secret-toggle still work while
/// the variable form is on screen (unchanged from before this task).
#[test]
fn keyboard_e_and_s_still_work_with_the_form_on_screen() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    let keymap = Keymap::default_bindings();

    app.handle_key(&keymap, plain('e'));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::RenameVariable { .. },
            ..
        })
    ));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    app.handle_key(&keymap, plain('s'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
}

// --- Task 16: the group entries grid (spec §3.4) -------------------------

fn goto_group(app: &mut App, name: &str) {
    goto_row(app, |r| {
        r == &crate::components::varmanager::VmRow::Group(name.into())
    });
}

fn cell_rect(app: &mut App, row: usize, col: usize) -> ratatui::layout::Rect {
    rendered_text_tall(app);
    app.hits
        .rect_of(&crate::hit::Hit::VmEntryCell { row, col })
        .unwrap_or_else(|| panic!("no rect for cell {row}/{col}"))
}

#[test]
fn clicking_an_entrys_radio_records_the_selection_and_re_resolves_every_field() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    assert!(
        !app.project.resolved.values.contains_key("user"),
        "no selection yet: the group's field doesn't resolve"
    );

    rendered_text_tall(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::VmEntryRadio(1))
        .expect("bob's radio");
    app.handle_mouse(left_down(r.x, r.y));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(app.project.selections_for("qa")["user"], "bob");
    assert_eq!(
        app.project.resolved.values["user"], "2002",
        "{{user}} now resolves through the selected entry"
    );
    let state = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(state.selections["qa"]["user"], "bob");

    // …and clicking the other radio moves it, rather than adding a second.
    let r = app.hits.rect_of(&crate::hit::Hit::VmEntryRadio(0)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.project.selections_for("qa")["user"], "alice");
    assert_eq!(app.project.resolved.values["user"], "1001");
}

#[test]
fn editing_a_field_cell_and_clicking_away_rewrites_the_env_file() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    let r = cell_rect(&mut app, 0, 1);
    app.handle_mouse(left_down(r.x, r.y));
    assert!(app.varmanager.grid.editing.is_some(), "the cell is live");

    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('9'));

    // Clicking a *different* cell commits the first one (Task 8's
    // commit-first rule) and starts editing the one clicked.
    let other = cell_rect(&mut app, 1, 1);
    app.handle_mouse(left_down(other.x, other.y));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("10019"), "{on_disk}");
    let edit = app.varmanager.grid.editing.as_ref().expect("second cell");
    assert_eq!((edit.row, edit.col), (1, 1));
    assert_eq!(edit.input.text(), "2002");

    // Esc puts the second cell back with nothing written.
    app.handle_key(&keymap, plain('x'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.varmanager.grid.editing.is_none());
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!on_disk.contains("2002x"), "esc reverted: {on_disk}");
}

#[test]
fn the_ghost_row_creates_an_entry_and_keeps_going_into_its_first_field() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    let keymap = Keymap::default_bindings();

    // Row 2 is the ghost row (alice, bob, then the ghost).
    let r = cell_rect(&mut app, 2, 0);
    app.handle_mouse(left_down(r.x, r.y));
    for c in "carol".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("[entries.user.carol]"), "{on_disk}");
    // The new entry is created with an empty value for every field, so it
    // validates — and the edit walks on into that first field cell.
    let edit = app
        .varmanager
        .grid
        .editing
        .as_ref()
        .expect("editing continues left-to-right");
    assert_eq!((edit.row, edit.col), (2, 1));

    for c in "3003".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let env = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert_eq!(env.entries["user"]["carol"].values["user"], "3003");
}

#[test]
fn a_refused_entry_name_toasts_and_keeps_the_typed_text() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    let keymap = Keymap::default_bindings();

    let r = cell_rect(&mut app, 2, 0);
    app.handle_mouse(left_down(r.x, r.y));
    // `description` inside an entries table is an entry's own description,
    // so core refuses it as an entry name.
    for c in "description".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(!app.toasts.is_empty(), "the refusal is surfaced");
    let edit = app
        .varmanager
        .grid
        .editing
        .as_ref()
        .expect("the failed write left the edit in place");
    assert_eq!(edit.input.text(), "description");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!on_disk.contains("description"), "{on_disk}");
}

#[test]
fn the_field_editor_renames_adds_and_removes_across_variables_and_every_env() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    std::fs::write(
        dir.path().join("environments/dev.toml"),
        "[entries.user.dave]\nuser = \"7\"\n",
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    // --- rename: slot 0 is the group's current field, retyped -----------
    app.update(Action::ApplyGroupFields {
        group: "user".into(),
        slots: vec!["user_id".into()],
        confirmed: false,
    });
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(app.project.model.groups["user"].fields, vec!["user_id"]);
    let qa = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert_eq!(qa.entries["user"]["alice"].values["user_id"], "1001");
    let dev = postui_core::project::load_environment(dir.path(), "dev").unwrap();
    assert_eq!(
        dev.entries["user"]["dave"].values["user_id"], "7",
        "a non-active environment renames too"
    );

    // --- add: a slot past the current list -------------------------------
    app.update(Action::ApplyGroupFields {
        group: "user".into(),
        slots: vec!["user_id".into(), "customer_id".into()],
        confirmed: false,
    });
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(
        app.project.model.groups["user"].fields,
        vec!["user_id", "customer_id"]
    );
    let qa = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert_eq!(
        qa.entries["user"]["alice"].values["customer_id"], "",
        "every existing entry gains the column, empty"
    );

    // --- remove: a cleared slot warns before deleting the column ---------
    app.update(Action::ApplyGroupFields {
        group: "user".into(),
        slots: vec!["user_id".into(), String::new()],
        confirmed: false,
    });
    let Some(Modal::Confirm { body, .. }) = app.modals.top() else {
        panic!("a removal must confirm first");
    };
    assert!(body.contains("deleted from"), "{body}");
    assert!(body.contains("qa") && body.contains("dev"), "{body}");
    assert_eq!(
        app.project.model.groups["user"].fields,
        vec!["user_id", "customer_id"],
        "nothing has changed yet"
    );

    app.handle_key(&Keymap::default_bindings(), plain('y'));
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(app.project.model.groups["user"].fields, vec!["user_id"]);
    let qa = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert!(
        !qa.entries["user"]["alice"]
            .values
            .contains_key("customer_id"),
        "the column is gone from every entry"
    );
}

#[test]
fn renaming_a_group_moves_its_declaration_its_entries_and_its_selections() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    std::fs::write(
        dir.path().join("environments/dev.toml"),
        "[entries.user.dave]\nuser = \"7\"\n",
    )
    .unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            selections: [
                (
                    "qa".to_string(),
                    [("user".to_string(), "bob".to_string())].into(),
                ),
                (
                    "dev".to_string(),
                    [("user".to_string(), "dave".to_string())].into(),
                ),
            ]
            .into(),
            ..Default::default()
        },
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    assert_eq!(app.project.resolved.values["user"], "2002");

    app.update(Action::VarStruct(VarStructOp::Rename {
        from: "user".into(),
        to: "account".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let vars = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(vars.contains("[groups.account]"), "{vars}");
    assert!(!vars.contains("[groups.user]"), "{vars}");
    for env in ["qa", "dev"] {
        let text =
            std::fs::read_to_string(dir.path().join(format!("environments/{env}.toml"))).unwrap();
        assert!(
            text.contains("[entries.account."),
            "{env} entries moved: {text}"
        );
        assert!(!text.contains("[entries.user."), "{env}: {text}");
    }
    // The selection follows the name in every environment…
    assert_eq!(app.project.selections_for("qa")["account"], "bob");
    assert_eq!(app.project.selections_for("dev")["account"], "dave");
    assert!(!app.project.selections_for("qa").contains_key("user"));
    let state = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(state.selections["dev"]["account"], "dave");
    // …so the group's field still resolves to the same value it did.
    assert_eq!(app.project.resolved.values["user"], "2002");
    // …and the detail pane is still looking at the group it was on.
    assert_eq!(
        app.varmanager.detail,
        crate::components::varmanager::VmDetail::Group("account".into())
    );
}

#[test]
fn a_group_rename_onto_a_taken_name_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::Rename {
        from: "user".into(),
        to: "base_url".into(),
    }));

    assert!(!app.toasts.is_empty(), "the refusal is surfaced");
    assert!(app.project.model.groups.contains_key("user"));
    let qa = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(qa.contains("[entries.user.alice]"), "{qa}");
}

#[test]
fn right_clicking_an_entry_row_opens_its_own_menu() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    let r = cell_rect(&mut app, 0, 0);
    app.handle_mouse(right_down(r.x, r.y));
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("no entry menu");
    };
    let labels: Vec<&str> = state.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        vec!["Duplicate entry", "Rename\u{2026}", "Delete\u{2026}"]
    );
}

/// Review finding 1: the right-click path has to commit a live cell edit
/// *before* its menu can reshape the rows. Otherwise a menu action on
/// another row (Delete…) renumbers the entries under a `GridEdit` that
/// still holds the old index, and the next click-away writes the typed
/// text into a different record.
#[test]
fn right_clicking_another_row_commits_the_live_cell_to_the_entry_it_belongs_to() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    // Type into bob's (row 1) value cell…
    let r = cell_rect(&mut app, 1, 1);
    app.handle_mouse(left_down(r.x, r.y));
    app.handle_key(&Keymap::default_bindings(), plain('9'));
    assert!(app.varmanager.grid.editing.is_some());

    // …then right-click alice's row (row 0).
    let r = cell_rect(&mut app, 0, 0);
    app.handle_mouse(right_down(r.x, r.y));

    assert!(
        app.varmanager.grid.editing.is_none(),
        "the right click committed the live cell first"
    );
    let env = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert_eq!(
        env.entries["user"]["bob"].values["user"], "20029",
        "the text landed in the entry it was typed into"
    );
    assert_eq!(
        env.entries["user"]["alice"].values["user"], "1001",
        "the right-clicked entry is untouched"
    );
    // …and the menu is the one for the row that was right-clicked.
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("no entry menu");
    };
    assert_eq!(
        state.items[2].action,
        Some(Action::ConfirmDeleteEntry {
            env: "qa".into(),
            group: "user".into(),
            name: "alice".into(),
        })
    );
}

/// The same rule for the variable form's own field: a right click is a
/// click away from it too.
#[test]
fn right_clicking_commits_a_live_form_field() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    app.handle_key(&Keymap::default_bindings(), plain('9'));

    let row = app.varmanager.left_cursor;
    let left_rect = app.hits.rect_of(&crate::hit::Hit::VmLeftRow(row)).unwrap();
    app.handle_mouse(right_down(left_rect.x + 1, left_rect.y + 1));

    assert!(app.varmanager.form.editing.is_none(), "committed");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("https://qa.example.com9"), "{on_disk}");
}

/// Review finding 2: the grid is a keyboard focus stop of its own, so a
/// keyboard-only user can reach an entry other than the first one and
/// select it — and can start editing the focused cell.
#[test]
fn the_keyboard_reaches_the_grid_selects_a_row_and_edits_the_focused_cell() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    let keymap = Keymap::default_bindings();
    let arrow = |c| KeyEvent::new(c, KeyModifiers::NONE);
    assert_eq!(app.varmanager.focus, VmFocus::List);

    // Right steps into the grid; Down then moves the *grid's* cursor
    // rather than the left list's selection.
    app.handle_key(&keymap, arrow(KeyCode::Right));
    assert_eq!(app.varmanager.focus, VmFocus::Grid);
    app.handle_key(&keymap, arrow(KeyCode::Down));
    assert_eq!(app.varmanager.grid.cursor.0, 1);
    assert_eq!(
        app.varmanager.detail,
        crate::components::varmanager::VmDetail::Group("user".into()),
        "the left list kept its own selection"
    );

    // space selects the entry the cursor is on — row 1, not row 0.
    app.handle_key(&keymap, plain(' '));
    assert_eq!(app.project.selections_for("qa")["user"], "bob");
    assert_eq!(app.project.resolved.values["user"], "2002");

    // Enter edits the focused cell; Right first moves onto the value column.
    app.handle_key(&keymap, arrow(KeyCode::Right));
    app.handle_key(&keymap, arrow(KeyCode::Enter));
    let edit = app
        .varmanager
        .grid
        .editing
        .as_ref()
        .expect("Enter started an edit");
    assert_eq!((edit.row, edit.col), (1, 1));
    assert_eq!(edit.input.text(), "2002");

    // Esc leaves the edit; a second Esc hands the keyboard back to the
    // list; only a third closes the screen.
    app.handle_key(&keymap, arrow(KeyCode::Esc));
    assert!(app.varmanager.grid.editing.is_none());
    assert_eq!(app.varmanager.focus, VmFocus::Grid);
    app.handle_key(&keymap, arrow(KeyCode::Esc));
    assert_eq!(app.varmanager.focus, VmFocus::List);
    assert_eq!(app.screen, Screen::VarManager);
    app.handle_key(&keymap, arrow(KeyCode::Esc));
    assert_eq!(app.screen, Screen::Main);
}

/// Task 8 parity for the grid's cell walk: `Tab` runs on in reading order
/// (wrapping to the next row), `BackTab` runs back.
#[test]
fn tab_and_backtab_walk_the_grid_in_reading_order() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    let keymap = Keymap::default_bindings();
    let at = |app: &App| app.varmanager.grid.editing.as_ref().map(|e| (e.row, e.col));

    // Start on alice's value cell (the last column of row 0).
    let r = cell_rect(&mut app, 0, 1);
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(at(&app), Some((0, 1)));

    // Off the end of the row wraps to the next row's name cell…
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(at(&app), Some((1, 0)));
    // …and BackTab runs back the same way.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(at(&app), Some((0, 1)));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(at(&app), Some((0, 0)));
    // Nothing is before the first cell: the edit stays put.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(at(&app), Some((0, 0)));
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
}

#[test]
fn the_new_entry_button_starts_the_ghost_row_and_edit_fields_opens_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    rendered_text_tall(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::VmNewEntry).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let edit = app.varmanager.grid.editing.as_ref().expect("ghost is live");
    assert_eq!((edit.row, edit.col), (2, 0));

    rendered_text_tall(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::VmEditFields).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::MultiPrompt {
            kind: PromptKind::GroupFields { .. },
            ..
        })
    ));
    // One slot per current field, plus the empty "add field" slot.
    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        unreachable!()
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].input.text(), "user");
    assert_eq!(fields[1].input.text(), "");
}

// -- Task 17: the mouse-parity sweep (spec §5) --------------------------

/// THE PARITY CHECK. Every action `keys::named_actions()` can bind a key to
/// must also be reachable by mouse: a footer/toolbar chip, a direct `on_hit`
/// dispatch, a context-menu item, or a palette command. This walks the real
/// production lists/builders (not copies of them) so a future keybinding
/// added without a mouse path fails here rather than shipping silently.
///
/// The only exceptions are `keyboard_only_navigation` below — pure
/// navigation actions whose every target is *also* reachable by clicking it
/// directly, so no button for "next"/"previous" itself is missing any real
/// capability. That list must stay empty of anything else: if this test
/// fails, the fix is a mouse path, not a new entry there.
#[test]
fn every_named_action_is_mouse_reachable() {
    // Kept deliberately short, and each entry justified: these are the only
    // named actions with no mouse-dispatchable path anywhere, and both are
    // pure cycling over targets a click already reaches directly.
    let keyboard_only_navigation: Vec<Action> = vec![
        // tab/shift+tab cycles Sidebar → Editor → Response → Sidebar; each
        // pane is focused directly by clicking it (`Hit::Pane`).
        Action::FocusNext,
        Action::FocusPrev,
        // alt+right/left cycles the four editor tabs in draw order; each
        // tab is selected directly by clicking it (`Hit::EditorTab`, listed
        // in `App::mouse_dispatch_mirror`).
        Action::EditorTabCycle(1),
        Action::EditorTabCycle(-1),
    ];

    // Group A: footer/toolbar chips — the same function `draw_footer`
    // paints from. The always-present quit hint and palette chip are
    // registered separately in `draw_footer` itself (`QUIT_LABEL` /
    // `PALETTE_CHIP`, not part of `footer_chips`), so they're added by
    // hand here.
    let mut mouse_reachable: Vec<Action> = vec![Action::Quit, Action::OpenPalette];
    for pane in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
        mouse_reachable.extend(
            crate::components::footer::footer_chips(pane, false, false, Some("add header"))
                .into_iter()
                .filter_map(|(_, _, a)| a),
        );
    }

    // Group B: the command palette.
    mouse_reachable.extend(
        crate::components::palette::all_commands()
            .into_iter()
            .map(|c| c.action),
    );

    // Group C: `on_hit`'s own direct dispatches not already covered above —
    // the hand-maintained mirror kept beside `on_hit` in `app/mouse.rs`.
    mouse_reachable.extend(App::mouse_dispatch_mirror());

    // Group D: context menus, built with real state through the same
    // methods the mouse's right-click path calls.
    let mut app = App::new_for_test();
    app.editor.params.insert(
        "k".into(),
        postui_core::model::Entry {
            value: "v".into(),
            enabled: true,
        },
    );
    mouse_reachable.extend(
        app.table_row_context_menu(0)
            .into_iter()
            .flatten()
            .filter_map(|item| item.action),
    );
    postui_core::storage::save_request(&app.project.root, "req", &req("https://x/req")).unwrap();
    app.refresh_sidebar();
    let row = app
        .sidebar
        .rows
        .iter()
        .position(|r| matches!(r, Row::Request { slug, .. } if slug == "req"))
        .expect("the saved request is in the sidebar tree");
    mouse_reachable.extend(
        app.context_menu_for(&Hit::SidebarRow(row))
            .into_iter()
            .flatten()
            .filter_map(|item| item.action),
    );

    for (name, action) in crate::keys::named_actions() {
        assert!(
            keyboard_only_navigation.contains(&action) || mouse_reachable.contains(&action),
            "named action {name:?} ({action:?}) has a keybinding but no mouse path \
             (chip/menu/palette/on_hit) — add one, or justify a keyboard-only \
             exception in `keyboard_only_navigation`"
        );
    }
}

// --- Task 8 (stage 8): Testbed screen (POSTUI_TESTBED=1) -------------------

#[test]
fn explicit_testbed_flag_enters_the_testbed_screen() {
    let app = App::new_for_test_with_testbed(true);
    assert_eq!(app.screen, crate::app::Screen::Testbed);
}

#[test]
fn testbed_renders_a_bevel_and_an_underline() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new_for_test_with_testbed(true);
    let accent = app.theme.accent;
    let focus_ring = app.theme.focus_ring;
    let backend = TestBackend::new(160, 60);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buf = terminal.backend().buffer();
    let content = format!("{buf:?}");
    assert!(content.contains('▔'), "no bevel glyph found: {content}");
    // The tab-strip underline segment shares its glyph (`━`, box-drawing
    // heavy horizontal) with the plain hairline rule under it — distinguished
    // only by color — so this checks for an accent-colored `━` cell, not
    // just the glyph's presence, to avoid a false positive on the
    // hairline.
    let width = buf.area.width;
    let height = buf.area.height;
    let has_accent_underline = (0..height).any(|y| {
        (0..width).any(|x| {
            let cell = buf.cell((x, y)).unwrap();
            cell.symbol() == "━" && (cell.fg == accent || cell.fg == focus_ring)
        })
    });
    assert!(
        has_accent_underline,
        "no accent-colored tab-strip underline segment found"
    );
}

#[test]
fn q_quits_the_app_from_the_testbed_screen() {
    let mut app = App::new_for_test_with_testbed(true);
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('q'));
    assert!(app.should_quit, "q must quit from the testbed screen");
}

#[test]
fn esc_quits_the_app_from_the_testbed_screen() {
    let mut app = App::new_for_test_with_testbed(true);
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.should_quit, "Esc must quit from the testbed screen");
}

/// Review finding: the header/footer chrome is drawn (and hit-registered)
/// unconditionally on every screen, `Testbed` included — unlike the keyboard
/// path, mouse dispatch (`App::on_hit`, via `handle_mouse`) had no
/// `Screen::Testbed` guard, so a click on e.g. the project/env/vars chips or
/// a footer action chip could open a chooser, a modal, or navigate into
/// `Screen::VarManager` right out from under the showcase, with no way back
/// short of quitting. Every click but the footer's own quit chip must be a
/// pure no-op here.
#[test]
fn testbed_screen_ignores_every_mouse_click_except_the_quit_chip() {
    let mut app = App::new_for_test_with_testbed(true);

    click_hit(&mut app, Hit::HeaderProject);
    assert_eq!(app.screen, crate::app::Screen::Testbed);
    assert!(
        app.modals.is_empty(),
        "a header-chip click must not open the project chooser from the testbed"
    );

    click_hit(&mut app, Hit::HeaderVars);
    assert_eq!(
        app.screen,
        crate::app::Screen::Testbed,
        "a header-chip click must not navigate into the Variable Manager"
    );

    click_hit(&mut app, Hit::FooterChip(Action::OpenPalette));
    assert!(
        app.modals.is_empty(),
        "a footer-chip click (other than quit) must not open the palette"
    );

    assert!(
        !app.should_quit,
        "none of the inert clicks above should have quit the app"
    );
}

#[test]
fn testbed_screen_quit_chip_still_quits_via_mouse() {
    let mut app = App::new_for_test_with_testbed(true);
    click_hit(&mut app, Hit::FooterChip(Action::Quit));
    assert!(
        app.should_quit,
        "the footer's own quit chip must still work on the testbed"
    );
}

// --- Task 8b (stage 8): looping motion demos on the testbed screen ---------

/// The MOTION section's demos self-retarget to their opposite pole every
/// time they finish (see `App::tick_testbed_demos`), so — unlike every
/// other idle screen — a tick on `Screen::Testbed` never goes fully quiet:
/// `Anims::active` (and so `App::animating`) stays true across many
/// consecutive ticks, keeping the redraw loop alive with nothing else
/// (background sends, toasts, ...) driving it.
#[test]
fn testbed_tick_keeps_animating_true_across_many_ticks() {
    let mut app = App::new_for_test_with_testbed(true);
    for _ in 0..50 {
        app.update(Action::Tick);
        assert!(
            app.animating(),
            "a looping testbed demo should still be in flight or dwelling"
        );
    }
}

/// `tick_testbed_demos` must only ever run from `Screen::Testbed` — ticking
/// on `Screen::Main` (the overwhelmingly common case) must not start
/// tracking any of the reused `AnimKey`s (`SendBreathe`,
/// `ListTravel(Sidebar)`, ...), since a later real feature wiring one of
/// them up must not find it already animating from a screen that was never
/// showing.
#[test]
fn testbed_demo_drive_does_not_run_on_the_main_screen() {
    let mut app = App::new_for_test();
    assert_eq!(app.screen, crate::app::Screen::Main);
    let now = std::time::Instant::now();
    app.update(Action::Tick);
    assert!(
        app.anims
            .value(crate::anim::AnimKey::SendBreathe, now)
            .is_none(),
        "the testbed's Send-breathe demo must not start ticking on Screen::Main"
    );
    assert!(
        app.anims
            .value(
                crate::anim::AnimKey::ListTravel(crate::anim::ListId::Sidebar),
                now
            )
            .is_none(),
        "the testbed's list-travel demo must not start ticking on Screen::Main"
    );
}

// --- Task 14 (stage 8): toast motion, Send breathe, pane collapse --------

/// Pushing a toast (any `App::update` that ends up calling `Toasts::push`)
/// must start its slide-in ease immediately -- `App::update`'s trailing
/// `Toasts::start_pending_anims` call, not something a later `Action::Tick`
/// is needed for.
#[test]
fn pushing_a_toast_starts_an_active_toast_fade_anim() {
    let mut app = App::new_for_test();
    // No response yet: `CopyToClipboard(ResponseBody)` toasts "nothing to
    // copy" unconditionally, with no fixture setup needed.
    app.update(Action::CopyToClipboard(CopyTarget::ResponseBody));
    assert!(!app.toasts.is_empty(), "the copy must have toasted");
    assert!(
        app.animating(),
        "the freshly pushed toast's own slide-in must already be in flight"
    );
}

/// While a send is in flight, `App::animating()` must stay true across
/// ticks: the Send-cap breathe (`AnimKey::SendBreathe`) keeps retargeting
/// to the opposite pole every time it finishes, for as long as
/// `session.in_flight` is set -- see `App::tick_send_breathe`.
#[tokio::test]
async fn in_flight_send_keeps_animating_true_across_ticks() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::ForceSend);
    assert!(!app.session.in_flight.is_empty());
    for _ in 0..5 {
        app.update(Action::Tick);
        assert!(
            app.animating(),
            "the Send-cap breathe must still be easing or dwelling"
        );
    }
    // Cleanup: cancel so the spawned task doesn't outlive the test.
    app.update(Action::CancelSend);
}

/// Once nothing is in flight any more, the breathe's `AnimKey::SendBreathe`
/// entry is dropped -- so the *next* send starts a fresh breathe from 0
/// rather than resuming wherever the last one left off.
#[tokio::test]
async fn send_breathe_anim_is_cleared_once_nothing_is_in_flight() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::ForceSend);
    app.update(Action::Tick);
    assert!(
        app.anims
            .value(crate::anim::AnimKey::SendBreathe, std::time::Instant::now())
            .is_some(),
        "the breathe must have started while sending"
    );
    app.update(Action::CancelSend);
    app.update(Action::Tick);
    assert!(
        app.anims
            .value(crate::anim::AnimKey::SendBreathe, std::time::Instant::now())
            .is_none(),
        "the breathe must be cleared once nothing is in flight"
    );
}

/// `Action::ToggleTableCollapse` (the `⌄ hide`/`⌄ show` chip and alt+p)
/// must retarget `AnimKey::PaneCollapse` rather than snap it -- right after
/// the toggle, the anim is still in flight (not yet `is_done`), so the very
/// next `ui::draw` reads an eased value, not the settled endpoint.
#[test]
fn toggle_table_collapse_retargets_pane_collapse_instead_of_snapping() {
    let mut app = App::new_for_test_with_anims(true);
    app.editor.active_tab = EditorTab::Params;
    assert!(!app.table_collapsed);
    let now = std::time::Instant::now();
    assert!(
        app.anims
            .value(crate::anim::AnimKey::PaneCollapse, now)
            .is_none(),
        "untouched before the first toggle"
    );

    app.update(Action::ToggleTableCollapse);
    assert!(app.table_collapsed);
    let now = std::time::Instant::now();
    assert!(
        !app.anims.is_done(crate::anim::AnimKey::PaneCollapse, now),
        "collapsing must ease over ui_settings.anim_ms.pane_collapse, not snap"
    );

    // `layout::compute_layout` interpolates the row split from this same
    // eased value -- see the dedicated mid-anim coverage in
    // `layout::tests::mid_collapse_height_sits_strictly_between_both_endpoints`,
    // which drives `now` manually rather than depending on real elapsed
    // time between two `Instant::now()` calls here.
}

/// `Action::ToggleResponseCollapse` (the response header's `⌄ hide`/`› show`
/// toggle and the palette entry) flips the pane's collapsed flag and eases
/// `AnimKey::ResponseCollapse` toward the new pole rather than snapping.
#[test]
fn toggle_response_collapse_flips_the_flag_and_eases_the_anim() {
    let mut app = App::new_for_test_with_anims(true);
    assert!(!app.session.response.collapsed);
    app.update(Action::ToggleResponseCollapse);
    assert!(app.session.response.collapsed);
    let now = std::time::Instant::now();
    assert!(
        !app.anims
            .is_done(crate::anim::AnimKey::ResponseCollapse, now),
        "collapsing must ease, not snap"
    );
    app.update(Action::ToggleResponseCollapse);
    assert!(!app.session.response.collapsed, "toggles back open");
}

/// A tab switch that changes whether the table is actually showing (while
/// `table_collapsed` itself stays put) must also ease `PaneCollapse` --
/// switching onto the Body tab always keeps the normal 50/50 split
/// (`layout::compute_layout`'s own doc), so leaving a collapsed Params tab
/// for Body must animate the pane back open exactly like an explicit
/// toggle would.
#[test]
fn switching_to_the_body_tab_keeps_a_hidden_editor_hidden() {
    let mut app = App::new_for_test_with_anims(true);
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.editor.active_tab = EditorTab::Params;
    app.update(Action::ToggleTableCollapse);
    assert!(app.table_collapsed);
    // Let it settle (real time is fine here -- pane_collapse defaults to
    // 120ms, and `is_done`/`value` both tolerate an already-finished anim).
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(app.anims.is_done(
        crate::anim::AnimKey::PaneCollapse,
        std::time::Instant::now()
    ));

    // Hide applies on every tab now, the Body buffer included: a tab
    // switch no longer re-opens the pane.
    app.update(Action::EditorTabSelect(EditorTab::Body.index()));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    let now = std::time::Instant::now();
    assert!(
        app.anims.is_done(crate::anim::AnimKey::PaneCollapse, now),
        "switching tabs while hidden must not disturb the collapse"
    );
    assert!(app.table_collapsed, "still hidden on the Body tab");
}

/// The caret-resting variable tooltip must dwell for `CARET_TIP_DWELL`
/// (wall-clock) before it appears -- not a fixed tick count, since the
/// tick period is adaptive (16ms while anything else animates) and a
/// tick-counted dwell would otherwise fire up to ~6x fast.
#[test]
fn caret_resting_in_a_token_shows_its_tooltip_only_after_the_wall_clock_dwell() {
    let mut app = App::new_for_test();
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url = crate::components::line_input::LineInput::new("{{base}}");
    app.editor.url.set_cursor(3); // inside the token
    // The tooltip's anchor comes from `Hit::VarToken`, registered by a real
    // draw pass (`components::var_tokens`) -- not by `update` itself.
    render_once(&mut app);
    app.update(Action::Tick);
    assert!(
        app.var_token_tip().is_none(),
        "must not show immediately -- the dwell hasn't elapsed"
    );

    // Real time is fine here -- the dwell is 200ms and `var_token_tip`
    // tolerates being sampled well past it.
    std::thread::sleep(std::time::Duration::from_millis(250));
    app.update(Action::Tick);
    let tip = app.var_token_tip();
    assert_eq!(
        tip.map(|t| t.name),
        Some("base".to_string()),
        "must show once the caret has rested past the dwell"
    );
}

mod undo_tests {
    use super::*;
    use crate::undo::CursorPos;
    use postui_core::model::Entry;

    #[test]
    fn apply_snapshot_swaps_fields_but_not_saved() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("snap-test".into()));
        let saved_before = app.editor.saved.clone();
        let mut req = app.editor.current_request();
        req.url = "https://example.com".into();
        app.editor.apply_snapshot(&req);
        assert_eq!(app.editor.url.text(), "https://example.com");
        assert_eq!(app.editor.saved, saved_before, "saved snapshot untouched");
        assert!(app.editor.is_dirty());
    }

    #[test]
    fn cursor_roundtrip_url() {
        let mut app = App::new_for_test();
        app.editor.sub_focus = SubFocus::Url;
        app.editor.url = LineInput::new("hello");
        app.editor.url.set_cursor(3);
        let pos = app.editor.cursor_pos();
        app.editor.url = LineInput::new("hello world");
        app.editor.restore_cursor(&pos);
        assert_eq!(app.editor.cursor_pos(), pos);
    }

    #[test]
    fn restore_cursor_cell_key_survives_row_shift() {
        let mut app = App::new_for_test();
        app.editor.active_tab = EditorTab::Params;
        app.editor.sub_focus = SubFocus::Content;
        app.editor.params.insert(
            "a".into(),
            Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        app.editor.params.insert(
            "b".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        app.editor.table.selected = Some(1); // key "b"
        let pos = app.editor.cursor_pos();
        assert_eq!(
            pos,
            CursorPos::Cell {
                tab: EditorTab::Params,
                key: "b".into()
            }
        );
        app.editor.params.shift_remove("a"); // "b" is now index 0
        app.editor.restore_cursor(&pos);
        assert_eq!(app.editor.table.selected, Some(0));
    }

    #[test]
    fn url_typing_is_captured_and_coalesced() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("cap".into()));
        app.capture_undo(); // seed shadow
        app.editor.sub_focus = SubFocus::Url;
        for c in ['h', 't', 't', 'p'] {
            app.editor
                .handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            app.capture_undo();
        }
        let step = app.history.pop_undo().expect("typing recorded");
        let crate::undo::StepKind::EditorDelta { before, after, .. } = step.kind else {
            panic!()
        };
        assert_eq!(before.url, "");
        assert_eq!(after.url, "http");
        // create's own FileStates step (Task 6) may remain beneath it, but
        // the typing burst itself must be exactly one coalesced EditorDelta.
        while let Some(step) = app.history.pop_undo() {
            assert!(
                !matches!(step.kind, crate::undo::StepKind::EditorDelta { .. }),
                "typing burst produced more than one EditorDelta step"
            );
        }
    }

    #[test]
    fn opening_another_request_reseeds_without_recording() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("one".into()));
        app.capture_undo();
        app.update(Action::CreateRequest("two".into()));
        app.capture_undo();
        // create itself may record FileStates steps (Task 6), but must not
        // record an EditorDelta for the editor being swapped out.
        while let Some(step) = app.history.pop_undo() {
            assert!(
                !matches!(step.kind, crate::undo::StepKind::EditorDelta { .. }),
                "request switch must not record an EditorDelta"
            );
        }
    }

    #[test]
    fn format_body_stands_alone_mid_burst() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("fmt".into()));
        app.capture_undo();
        app.editor.set_body_text("{\"a\":1}");
        app.capture_undo();
        app.update(Action::FormatBody);
        app.capture_undo();
        let mut steps = 0;
        while app.history.pop_undo().is_some() {
            steps += 1;
        }
        assert!(steps >= 2, "format must not merge into the typing step");
    }

    #[test]
    fn body_select_all_switches_to_the_body_tab_and_selects_everything() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("sel".into()));
        app.update(Action::CycleMethod); // GET -> POST, so Body is enabled
        app.editor.set_body_text("{\"a\": 1}");
        app.editor.active_tab = EditorTab::Headers;
        app.update(Action::BodySelectAll);
        assert_eq!(app.editor.active_tab, EditorTab::Body);
        assert_eq!(app.editor.sub_focus, SubFocus::Content);
        assert_eq!(app.focus, PaneId::Editor);
        assert_eq!(
            app.editor.body_selected_text().as_deref(),
            Some("{\"a\": 1}")
        );
    }

    #[test]
    fn body_select_all_is_inert_while_the_method_sends_no_body() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("sel-get".into()));
        app.editor.set_body_text("held");
        app.update(Action::BodySelectAll);
        assert_ne!(app.editor.active_tab, EditorTab::Body);
        assert_eq!(app.editor.body_selected_text(), None);
    }

    #[test]
    fn body_clear_empties_the_body_and_undo_brings_it_back() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("clr".into()));
        app.update(Action::CycleMethod);
        app.capture_undo();
        app.editor.set_body_text("{\"a\": 1}");
        app.capture_undo();
        app.update(Action::BodyClear);
        app.capture_undo();
        assert_eq!(app.editor.body_text(), "");
        app.update(Action::Undo);
        assert_eq!(app.editor.body_text(), "{\"a\": 1}");
    }

    #[test]
    fn undo_reverts_url_typing_and_redo_restores() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("uz".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        for c in "abc".chars() {
            app.editor
                .handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            app.capture_undo();
        }
        app.update(Action::Undo);
        assert_eq!(app.editor.url.text(), "");
        app.update(Action::Redo);
        assert_eq!(app.editor.url.text(), "abc");
    }

    #[test]
    fn empty_stacks_toast_quietly() {
        let mut app = App::new_for_test();
        app.update(Action::Undo);
        let msgs = app.toasts.messages();
        assert!(
            msgs.iter().any(|m| m.contains("Nothing to undo")),
            "expected a 'Nothing to undo' toast: {msgs:?}"
        );
        app.update(Action::Redo);
        let msgs = app.toasts.messages();
        assert!(
            msgs.iter().any(|m| m.contains("Nothing to redo")),
            "expected a 'Nothing to redo' toast: {msgs:?}"
        );
    }

    #[test]
    fn undo_is_inert_while_a_modal_is_open() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("modal".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::PromptNewRequest); // opens a modal
        app.update(Action::Undo);
        assert_eq!(app.editor.url.text(), "x", "no undo under a modal");
    }

    #[test]
    fn edit_after_undo_clears_redo() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("lin".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::Undo);
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::Redo);
        assert_eq!(app.editor.url.text(), "z", "redo stack cleared by new edit");
    }

    #[test]
    fn undo_jumps_back_to_the_edited_request() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("aaa".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::SaveRequest); // save so switching away is clean
        app.capture_undo();
        app.update(Action::CreateRequest("bbb".into()));
        app.capture_undo();
        // History top is bbb's create (a FileStates step once Task 6 lands);
        // undo past it back to aaa's edit. Before Task 6, the top IS aaa's
        // save/edit — pop accordingly. Final state to assert:
        while app.editor.slug.as_deref() != Some("aaa") {
            app.update(Action::Undo);
        }
        assert_eq!(app.editor.slug.as_deref(), Some("aaa"));
    }

    #[test]
    fn jump_back_reverts_and_redo_returns() {
        // jb1's history: create (FileStates), url "" -> "x" (EditorDelta),
        // a save (SaveRequest — breaks coalescing), then url "x" -> "xq"
        // (a second, unmerged EditorDelta). "x" is distinct from both the
        // disk-original "" and the post-edit "xq", so asserting on it can't
        // be satisfied by undoing either too little or too much.
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("jb1".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::SaveRequest);
        app.capture_undo();
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        app.capture_undo();
        // open jb2 through the dirty gate's discard? No — undo's jump-back
        // must work even with jb1 dirty. Open jb2 by force:
        app.update(Action::CreateRequest("jb2".into()));
        // create_or_save_as loads the new request unconditionally, so jb1's
        // unsaved "xq" lives only in history now.
        while app.editor.slug.as_deref() != Some("jb1") {
            app.update(Action::Undo);
        }
        assert_eq!(
            app.editor.url.text(),
            "x",
            "jb1's edit reverted to its pre-'q' state, not past it to the disk-empty original"
        );
        app.update(Action::Redo);
        assert_eq!(app.editor.url.text(), "xq", "redo re-applies jb1's edit");
    }

    #[test]
    fn undo_restores_a_deleted_request_file_byte_identical() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("del-me".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::SaveRequest);
        app.capture_undo();
        let path = postui_core::storage::request_path(&app.project.root, "del-me");
        let original = std::fs::read_to_string(&path).unwrap();
        app.update(Action::DeleteRequest("del-me".into()));
        app.capture_undo();
        assert!(!path.exists());
        app.update(Action::Undo);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        app.update(Action::Redo);
        assert!(!path.exists(), "redo deletes again");
    }

    #[test]
    fn undo_reverts_a_rename_on_disk() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("old-name".into()));
        app.capture_undo();
        assert_eq!(app.editor.slug.as_deref(), Some("old-name"));
        app.update(Action::RenameRequest {
            from: "old-name".into(),
            to: "new-name".into(),
        });
        app.capture_undo();
        // The forward rename retitles the still-open editor in place
        // (doesn't close it) — undo/redo of the FileStates step it
        // records must do the same, not treat the moved-away path as a
        // delete and close the editor (reviewer finding).
        assert_eq!(app.editor.slug.as_deref(), Some("new-name"));
        app.update(Action::Undo);
        let root = app.project.root.clone();
        assert!(
            postui_core::storage::request_path(&root, "old-name").exists(),
            "old-name restored"
        );
        assert!(
            !postui_core::storage::request_path(&root, "new-name").exists(),
            "new-name gone"
        );
        assert_eq!(
            app.editor.slug.as_deref(),
            Some("old-name"),
            "undo retitles the open editor back, rather than closing it"
        );
        app.update(Action::Redo);
        assert!(
            postui_core::storage::request_path(&root, "new-name").exists(),
            "new-name restored"
        );
        assert!(
            !postui_core::storage::request_path(&root, "old-name").exists(),
            "old-name gone"
        );
        assert_eq!(
            app.editor.slug.as_deref(),
            Some("new-name"),
            "redo retitles the open editor forward again"
        );
    }

    #[test]
    fn undo_past_a_save_marks_dirty_but_keeps_the_file() {
        // A single pre-save edit would make `prev_saved` (captured before
        // that edit's own `mark_saved`) structurally identical to the
        // EditorDelta's `before` — undoing both would coincidentally land
        // `current == saved` and read as clean, which isn't what "undo
        // reverted a save" should mean. A second, post-save edit (kept
        // un-merged because SaveRequest breaks coalescing) avoids that:
        // undoing back to it still leaves a real edit ("ab") standing
        // against the reverted, pre-edit `saved` marker.
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("sv".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.capture_undo(); // one coalesced EditorDelta: "" -> "ab"
        app.update(Action::SaveRequest); // breaks coalescing; disk now holds "ab"
        app.capture_undo();
        let path = postui_core::storage::request_path(&app.project.root, "sv");
        let saved_file = std::fs::read_to_string(&path).unwrap();
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        app.capture_undo(); // a second, unmerged EditorDelta: "ab" -> "abc"
        app.update(Action::Undo); // undoes the "abc" edit
        assert_eq!(app.editor.url.text(), "ab");
        app.update(Action::Undo); // undoes the save (memory only)
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            saved_file,
            "undoing a save never touches disk"
        );
        assert!(
            app.editor.is_dirty(),
            "save's undo reverted editor.saved past the still-present 'ab' edit"
        );
    }

    #[test]
    fn undo_of_a_deleted_step_fails_gracefully_when_disk_changed() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("ext".into()));
        app.capture_undo();
        app.update(Action::DeleteRequest("ext".into()));
        app.capture_undo();
        app.update(Action::Undo); // restores file
        let path = postui_core::storage::request_path(&app.project.root, "ext");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap(); // remove_file on a dir errors
        app.update(Action::Undo); // tries to delete "created" file -> error
        // step dropped, no panic, history still usable:
        app.update(Action::Undo);
    }

    /// Final-review finding: `jump_to_request_for_undo`'s dirty-gate
    /// fallback ignored its `redo` parameter and always pushed the step
    /// back onto the undo stack with an `Action::Undo` retry, so a tripped
    /// guard on a *redo* silently turned Ctrl+Y into an undo. Forces the
    /// guard by editing the open request directly (bypassing
    /// `capture_undo`) so the editor is dirty and the shadow is stale,
    /// then hands `apply_undo_step` a step targeting a *different*,
    /// unopened request so `jump_to_request_for_undo` runs.
    #[test]
    fn jump_guard_trip_retries_in_the_direction_it_was_pushed() {
        for redo in [false, true] {
            let mut app = App::new_for_test();
            app.update(Action::CreateRequest("one".into()));
            app.capture_undo();
            app.update(Action::CreateRequest("two".into()));
            app.capture_undo(); // shadow now matches saved "two"

            // Dirty the open editor without recapturing: `is_dirty()` goes
            // true and the shadow (still "two"'s saved snapshot) no longer
            // matches `editor.current_request()`.
            app.editor.sub_focus = SubFocus::Url;
            app.editor
                .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
            assert!(app.editor.is_dirty());

            // A step targeting "one" (not the open "two") forces the jump
            // path, which must hit the guard given the state above.
            let step = crate::undo::Step {
                kind: crate::undo::StepKind::EditorDelta {
                    slug: Some("one".into()),
                    before: req("https://before"),
                    after: Box::new(req("https://after")),
                },
                context: crate::undo::Context {
                    slug: Some("one".into()),
                    cursor_before: CursorPos::None,
                    cursor_after: CursorPos::None,
                },
            };

            let before_undo_len = app.history.undo_len();
            let before_redo_len = app.history.redo_len();
            let applied = app.apply_undo_step(step, redo);
            assert!(!applied, "guard trip must report failure");

            if redo {
                assert_eq!(
                    app.history.redo_len(),
                    before_redo_len + 1,
                    "redo direction: step must land back on the redo stack"
                );
                assert_eq!(app.history.undo_len(), before_undo_len);
            } else {
                assert_eq!(
                    app.history.undo_len(),
                    before_undo_len + 1,
                    "undo direction: step must land back on the undo stack"
                );
                assert_eq!(app.history.redo_len(), before_redo_len);
            }

            match app.modals.top() {
                Some(Modal::Confirm { choices, .. }) => {
                    let retry = choices
                        .iter()
                        .find(|(key, _, _)| *key == 's')
                        .expect("dirty gate offers a save-and-retry choice");
                    let expects_redo = retry.2.contains(&Action::Redo);
                    let expects_undo = retry.2.contains(&Action::Undo);
                    assert_eq!(
                        expects_redo, redo,
                        "retry action must match the direction the step was pushed in"
                    );
                    assert_eq!(expects_undo, !redo);
                }
                Some(_) => panic!("expected the dirty-gate confirm modal, got a different one"),
                None => panic!("expected the dirty-gate confirm modal, got none"),
            }
        }
    }

    /// Final-review finding: a mid-loop failure applying a multi-file
    /// `FileStates` step returned early *before* the reload/refresh block,
    /// so a write that landed (earlier in the loop) before the one that
    /// failed never showed up in the sidebar. Builds a two-path step where
    /// the first write succeeds (creates a brand-new request file) and the
    /// second targets a path that's actually a directory, forcing a
    /// mid-loop failure, then asserts the successfully-written request is
    /// visible in the sidebar despite the step being dropped.
    #[test]
    fn failed_multi_file_step_still_refreshes_the_sidebar() {
        let mut app = App::new_for_test();
        let new_path = postui_core::storage::request_path(&app.project.root, "brand-new");
        let blocked_path = app.project.root.join("blocked.toml");
        std::fs::create_dir_all(&blocked_path).unwrap(); // a dir where a file write is expected

        let step = crate::undo::Step {
            kind: crate::undo::StepKind::FileStates {
                before: vec![
                    (new_path.clone(), None),
                    (blocked_path.clone(), Some("x".into())),
                ],
                after: vec![
                    (
                        new_path.clone(),
                        Some(req("https://brand-new").to_toml_string()),
                    ),
                    (blocked_path.clone(), Some("y".into())),
                ],
                active_env: None,
            },
            context: crate::undo::Context {
                slug: None,
                cursor_before: CursorPos::None,
                cursor_after: CursorPos::None,
            },
        };

        let applied = app.apply_undo_step(step, true); // redo direction
        assert!(!applied, "the blocked second write must fail the step");
        assert!(new_path.exists(), "the first write in the step must stand");
        assert!(
            app.sidebar
                .rows
                .iter()
                .any(|r| matches!(r, Row::Request { slug, .. } if slug == "brand-new")),
            "sidebar must be refreshed to reflect the write that landed before the failure: {:?}",
            app.sidebar.rows
        );
    }

    #[test]
    fn undo_reverts_a_variable_value_edit_on_disk() {
        let mut app = App::new_for_test();
        app.update(Action::VarStruct(VarStructOp::NewVar {
            name: "tok".into(),
            description: None,
        }));
        app.update(Action::VarEdit(VarEditOp::SetDefault {
            name: "tok".into(),
            value: "v1".into(),
        }));
        app.capture_undo();
        let vars_path = app.project.root.join("variables.toml");
        let with_v1 = std::fs::read_to_string(&vars_path).unwrap();
        assert!(with_v1.contains("v1"), "{with_v1}");

        app.update(Action::VarEdit(VarEditOp::SetDefault {
            name: "tok".into(),
            value: "v2".into(),
        }));
        app.capture_undo();
        assert!(std::fs::read_to_string(&vars_path).unwrap().contains("v2"));

        app.update(Action::Undo);
        assert_eq!(std::fs::read_to_string(&vars_path).unwrap(), with_v1);
        app.update(Action::Redo);
        assert!(std::fs::read_to_string(&vars_path).unwrap().contains("v2"));
    }

    /// Final-review finding: `Action::DuplicateVar` wrote variables.toml
    /// (twice, for a secret) with no capture wrap at all. Cloned from
    /// `undo_reverts_a_variable_value_edit_on_disk`.
    #[test]
    fn undo_reverts_a_duplicate_var_on_disk() {
        let mut app = App::new_for_test();
        app.update(Action::VarStruct(VarStructOp::NewVar {
            name: "tok".into(),
            description: None,
        }));
        app.capture_undo();
        let vars_path = app.project.root.join("variables.toml");
        let before_dup = std::fs::read_to_string(&vars_path).unwrap();

        app.update(Action::DuplicateVar { name: "tok".into() });
        app.capture_undo();
        let with_dup = std::fs::read_to_string(&vars_path).unwrap();
        assert_ne!(with_dup, before_dup, "duplicate must change variables.toml");
        assert!(with_dup.contains("tok-copy"));

        app.update(Action::Undo);
        assert_eq!(
            std::fs::read_to_string(&vars_path).unwrap(),
            before_dup,
            "undo must byte-for-byte revert the duplicate"
        );
        app.update(Action::Redo);
        assert_eq!(std::fs::read_to_string(&vars_path).unwrap(), with_dup);
    }

    #[test]
    fn undo_reverts_create_environment_and_active_env() {
        let mut app = App::new_for_test();
        app.update(Action::CreateEnv("staging".into()));
        app.capture_undo();
        assert_eq!(app.project.active_env.as_deref(), Some("staging"));
        let env_path = app.project.root.join("environments/staging.toml");
        assert!(env_path.exists());
        app.update(Action::Undo);
        assert!(!env_path.exists());
        assert_eq!(app.project.active_env, None);
        app.update(Action::Redo);
        assert!(env_path.exists());
        assert_eq!(app.project.active_env.as_deref(), Some("staging"));
    }

    #[test]
    fn failed_var_edit_records_no_step() {
        let mut app = App::new_for_test();
        app.update(Action::VarStruct(VarStructOp::NewVar {
            name: "a".into(),
            description: None,
        }));
        app.update(Action::VarStruct(VarStructOp::NewVar {
            name: "b".into(),
            description: None,
        }));
        app.capture_undo();
        let before = app.history.undo_len();

        // "b" already exists, so renaming "a" to "b" fails validation and
        // must record nothing.
        app.update(Action::VarStruct(VarStructOp::Rename {
            from: "a".into(),
            to: "b".into(),
        }));
        assert!(!app.toasts.is_empty(), "rename collision must toast");
        assert_eq!(app.history.undo_len(), before, "failed op recorded nothing");
    }

    /// Reviewer finding: `commit_var_form` (the Variable Manager detail
    /// form's click-away/Enter commit) is called directly from `handle_key`
    /// — it never routes through `Action::VarEdit`/`self.apply`, so the
    /// arm-level wrapping alone misses it. Mirrors
    /// `clicking_the_env_value_field_typing_and_clicking_away_writes_the_env_file`,
    /// then drives it through Undo/Redo.
    #[test]
    fn undo_reverts_a_var_form_commit_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        var_project(dir.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, dir.path().to_path_buf());
        goto_row(&mut app, |r| {
            r == &crate::components::varmanager::VmRow::Var("base_url".into())
        });

        let env_path = dir.path().join("environments/qa.toml");
        let before_text = std::fs::read_to_string(&env_path).unwrap();

        let r = field_rect(&mut app, VmField::EnvValue);
        app.handle_mouse(left_down(r.x + 1, r.y + 1));
        let keymap = Keymap::default_bindings();
        app.handle_key(&keymap, plain('9'));
        // Click away commits (Task 8's commit-first rule).
        let row = app.varmanager.left_cursor;
        let left_rect = app.hits.rect_of(&crate::hit::Hit::VmLeftRow(row)).unwrap();
        app.handle_mouse(left_down(left_rect.x + 1, left_rect.y + 1));

        assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
        let with_9 = std::fs::read_to_string(&env_path).unwrap();
        assert!(with_9.contains("https://qa.example.com9"), "{with_9}");
        assert_ne!(with_9, before_text);

        app.update(Action::Undo);
        assert_eq!(std::fs::read_to_string(&env_path).unwrap(), before_text);
        app.update(Action::Redo);
        assert_eq!(std::fs::read_to_string(&env_path).unwrap(), with_9);
    }

    /// Reviewer finding: `commit_grid_edit` (the group entries grid's
    /// click-away/Enter commit) has the same gap as `commit_var_form` —
    /// called directly from `handle_key`, never through
    /// `Action::VarStruct`/`Action::VarEdit`. Mirrors
    /// `editing_a_field_cell_and_clicking_away_rewrites_the_env_file`.
    #[test]
    fn undo_reverts_a_grid_cell_commit_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        var_project(dir.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, dir.path().to_path_buf());
        goto_group(&mut app, "user");

        let env_path = dir.path().join("environments/qa.toml");
        let before_text = std::fs::read_to_string(&env_path).unwrap();

        let r = cell_rect(&mut app, 0, 1);
        app.handle_mouse(left_down(r.x, r.y));
        let keymap = Keymap::default_bindings();
        app.handle_key(&keymap, plain('9'));
        // Clicking a different cell commits the first one.
        let other = cell_rect(&mut app, 1, 1);
        app.handle_mouse(left_down(other.x, other.y));

        assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
        let with_9 = std::fs::read_to_string(&env_path).unwrap();
        assert!(with_9.contains("10019"), "{with_9}");
        assert_ne!(with_9, before_text);

        app.update(Action::Undo);
        assert_eq!(std::fs::read_to_string(&env_path).unwrap(), before_text);
        app.update(Action::Redo);
        assert_eq!(std::fs::read_to_string(&env_path).unwrap(), with_9);
    }

    #[test]
    fn switching_requests_keeps_the_active_tab() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("a".into()));
        app.update(Action::CreateRequest("b".into()));
        app.update(Action::EditorTabSelect(EditorTab::Params.index()));
        app.update(Action::OpenRequest("a".into()));
        assert_eq!(app.editor.slug.as_deref(), Some("a"));
        assert_eq!(app.editor.active_tab, EditorTab::Params);
    }

    #[test]
    fn body_tab_survives_a_detour_through_a_bodyless_request() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("get-req".into()));
        app.update(Action::CreateRequest("post-req".into()));
        app.update(Action::CycleMethod); // post-req: GET -> POST
        app.update(Action::SaveRequest);
        app.update(Action::EditorTabSelect(EditorTab::Body.index()));
        assert_eq!(app.editor.active_tab, EditorTab::Body);
        // Opening the GET request hops off the disabled Body tab...
        app.update(Action::OpenRequest("get-req".into()));
        assert_ne!(app.editor.active_tab, EditorTab::Body);
        // ...but coming back to the POST restores the chosen tab.
        app.update(Action::OpenRequest("post-req".into()));
        assert_eq!(app.editor.active_tab, EditorTab::Body);
    }

    #[test]
    fn method_change_restores_the_body_tab_when_it_reenables() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("m".into()));
        app.update(Action::CycleMethod); // POST
        app.update(Action::EditorTabSelect(EditorTab::Body.index()));
        app.update(Action::SetMethod(postui_core::model::Method::Get));
        assert_ne!(app.editor.active_tab, EditorTab::Body, "hops off disabled Body");
        app.update(Action::SetMethod(postui_core::model::Method::Post));
        assert_eq!(app.editor.active_tab, EditorTab::Body, "returns when re-enabled");
    }

    #[test]
    fn choosing_a_tab_after_the_hop_replaces_the_body_preference() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("g".into()));
        app.update(Action::CreateRequest("p".into()));
        app.update(Action::CycleMethod); // p: POST
        app.update(Action::SaveRequest);
        app.update(Action::EditorTabSelect(EditorTab::Body.index()));
        app.update(Action::OpenRequest("g".into())); // hop off Body
        app.update(Action::EditorTabSelect(EditorTab::Vars.index()));
        app.update(Action::OpenRequest("p".into()));
        assert_eq!(
            app.editor.active_tab,
            EditorTab::Vars,
            "the explicit Vars choice replaced the old Body preference"
        );
    }
}
