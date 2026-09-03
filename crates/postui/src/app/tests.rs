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
fn init_default_startup_writes_project_toml_with_the_main_space() {
    // The fresh-install path: `with_root` runs `ensure_project` on a bare
    // directory (which can only make `requests/main`, never a
    // `project.toml`), then the disposition writes one — and only a second
    // `ensure_project` after that seeds `spaces`.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert!(!dir.path().join("project.toml").exists());

    app.init_default_project();

    assert_eq!(
        postui_core::project::load_meta(dir.path()).unwrap().spaces,
        ["main"]
    );
    assert_eq!(app.project.spaces, ["main"]);
    assert!(dir.path().join("requests/main").is_dir());
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
    assert_eq!(
        postui_core::project::load_meta(&app.project.root)
            .unwrap()
            .spaces,
        ["main"]
    );
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
    app.apply_ui_settings(settings, "dark".into(), crate::theme::Theme::dark());
    assert!(
        !app.anims.enabled,
        "animations = false in UiSettings must disable App.anims"
    );

    let settings = crate::config::UiSettings {
        animations: true,
        ..crate::config::UiSettings::default()
    };
    app.apply_ui_settings(settings, "dark".into(), crate::theme::Theme::dark());
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
fn deleting_a_table_row_by_key_is_immediate() {
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
    assert!(app.modals.top().is_none(), "delete is undoable, no confirm");
    assert!(app.editor.params.is_empty(), "the row is gone at once");
}

#[test]
fn deleting_a_vars_row_by_key_is_immediate() {
    // Same delete plumbing as Params, pointed at the Vars tab's
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
    assert!(app.modals.top().is_none(), "delete is undoable, no confirm");
    assert!(app.editor.variables.is_empty(), "the row is gone at once");
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
            "Delete param",
            "Extract value to variable\u{2026}",
            "Extract value to selector\u{2026}"
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

    // "Delete param": the same immediate delete the `d` key runs.
    app.editor.table.selected = Some(0);
    app.update(delete);
    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
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
fn editor_tab_select_slot_numbers_follow_the_screen_order() {
    // `EditorTabSelect`'s slot numbers ([`EditorTab::index`], bindable as
    // `editor_tab_N`) select the tabs in the order they appear on screen:
    // Headers, Params, Vars, Body. alt+1..4 no longer drive this — they
    // jump spaces now (`Action::JumpSpace`) — so this drives the action
    // directly rather than through the old default alt bindings.
    let mut app = App::new_for_test();
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.editor.active_tab = EditorTab::Body;
    app.update(Action::EditorTabSelect(EditorTab::Headers.index()));
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    app.update(Action::EditorTabSelect(EditorTab::Params.index()));
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    app.update(Action::EditorTabSelect(EditorTab::Vars.index()));
    assert_eq!(app.editor.active_tab, EditorTab::Vars);
    app.update(Action::EditorTabSelect(EditorTab::Body.index()));
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
fn undo_restores_a_deleted_table_row() {
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
    app.capture_undo(); // seed the shadow before the delete
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('d'));
    assert!(app.editor.params.is_empty());
    app.capture_undo();
    app.update(Action::Undo);
    assert_eq!(app.editor.params.len(), 1, "undo brings the row back");
}

#[test]
fn clicking_the_row_delete_affordance_deletes_the_row() {
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
    assert!(app.modals.top().is_none(), "delete is undoable, no confirm");
    assert!(app.editor.params.is_empty(), "clicking ✕ deletes the row");
}

/// A test app whose clipboard read returns `text` (no OS clipboard).
fn app_with_clipboard_text(text: &str) -> App {
    let mut app = App::new_for_test();
    let mut clip = crate::clipboard::Clipboard::new_for_test(None, 65536, false);
    clip.set_read_for_test(text);
    app.set_clipboard_for_test(clip);
    app
}

/// ctrl+v is paste now (GUI muscle memory — the variable picker moved to
/// alt+shift+v): with the URL bar focused it reads the clipboard and
/// inserts at the caret, flattening any line break.
#[test]
fn ctrl_v_pastes_clipboard_text_into_the_url_bar() {
    let mut app = app_with_clipboard_text("https://example.com\n/x");
    app.update(Action::FocusUrl);
    app.handle_key(&Keymap::default_bindings(), ctrl('v'));
    assert_eq!(app.editor.url.text(), "https://example.com /x");
    assert!(
        app.modals.is_empty(),
        "ctrl+v must not open the variable picker any more"
    );
}

/// Modals capture all input, but paste digs through: ctrl+v while a
/// Prompt is open inserts into its focused text box instead of typing a
/// literal nothing.
#[test]
fn ctrl_v_pastes_into_an_open_modal_prompt_input() {
    let mut app = app_with_clipboard_text("pasted-name");
    app.modals.push(Modal::Prompt {
        title: "Name".into(),
        input: crate::components::line_input::LineInput::new(""),
        kind: PromptKind::NewRequest,
        revealed: false,
    });
    app.handle_key(&Keymap::default_bindings(), ctrl('v'));
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("the prompt stays open");
    };
    assert_eq!(input.text(), "pasted-name");
}

/// With the body caret live, ctrl+v pastes multi-line text verbatim.
#[test]
fn ctrl_v_pastes_multiline_text_into_the_body_editor() {
    let mut app = app_with_clipboard_text("{\n  \"a\": 1\n}");
    app.update(Action::SetMethod(postui_core::model::Method::Post));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = crate::components::editor::SubFocus::Content;
    app.editor.active_tab = EditorTab::Body;
    app.handle_key(&Keymap::default_bindings(), ctrl('v'));
    assert_eq!(app.editor.body_text(), "{\n  \"a\": 1\n}");
}

/// The variable picker's new home: alt+shift+v (ctrl+v now pastes).
#[test]
fn alt_shift_v_opens_the_variable_picker() {
    let mut app = App::new_for_test();
    app.update(Action::FocusUrl);
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT | KeyModifiers::SHIFT),
    );
    assert!(
        matches!(app.modals.top(), Some(Modal::VarPicker(_))),
        "alt+shift+v opens the picker"
    );
}

/// A terminal bracketed paste (cmd+V on macOS, ctrl+shift+V on Linux)
/// arrives as one `Event::Paste` and routes through the same insert
/// path — no clipboard read involved.
#[test]
fn bracketed_paste_routes_to_the_focused_input() {
    let mut app = App::new_for_test();
    app.update(Action::FocusUrl);
    assert!(app.paste_text("http://host/a b"));
    assert_eq!(app.editor.url.text(), "http://host/a b");

    // Nothing focused that takes text: the paste reports unhandled.
    app.focus = PaneId::Sidebar;
    app.editor.sub_focus = crate::components::editor::SubFocus::Method;
    assert!(!app.paste_text("ignored"));
}

/// cmd+c (SUPER+c, from terminals that report it) copies a live selection
/// and otherwise does nothing — it must never quit, unlike selectionless
/// ctrl+c.
#[test]
fn super_c_copies_the_selection_and_never_quits() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    let super_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER);
    app.handle_key(&keymap, super_c);
    assert!(!app.should_quit, "selectionless cmd+c is a no-op, not quit");

    app.update(Action::FocusUrl);
    app.paste_text("http://host/x");
    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Char('A'), KeyModifiers::CONTROL),
    );
    app.handle_key(&keymap, super_c);
    assert!(!app.should_quit);
    assert_eq!(
        app.editor.url.selected_text().as_deref(),
        Some("http://host/x"),
        "selection survives the copy"
    );
}

/// The SUPER fold end-to-end: cmd+a selects all in the focused input and
/// cmd+z undoes, both spelled exactly as a kitty-protocol terminal
/// reports the unbound cmd chords.
#[test]
fn super_a_selects_all_and_super_z_undoes() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::FocusUrl);
    app.paste_text("http://host/x");
    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER),
    );
    assert_eq!(
        app.editor.url.selected_text().as_deref(),
        Some("http://host/x"),
        "cmd+a folds to select-all"
    );
    // cmd+z folds onto the same global undo binding ctrl+z uses.
    use crate::keys::KeyCombo;
    let folded =
        crate::keys::normalize_super_keys(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER));
    assert_eq!(
        keymap.lookup(&KeyCombo::from_event(&folded)),
        Some(Action::Undo)
    );
}

/// The filter-query modals hold their query as a plain String rather than
/// a LineInput, so they sit outside `focused_input_index` — before the
/// bracketed-paste rework they received a paste as typed keystrokes, and
/// `paste_text` must keep them reachable.
#[test]
fn paste_reaches_the_palette_and_chooser_filter_queries() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    assert!(app.paste_text("send re\nquest"));
    let Some(Modal::Palette(p)) = app.modals.top() else {
        panic!("palette open");
    };
    assert_eq!(p.input(), "send re quest", "flattened like a LineInput");

    let mut app = App::new_for_test();
    app.update(Action::OpenThemeChooser);
    assert!(app.paste_text("gruv"));
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        panic!("chooser open");
    };
    assert_eq!(c.input(), "gruv");
}

#[test]
fn paste_reaches_the_var_picker_filter_in_insert_mode() {
    let mut app = App::new_for_test();
    app.update(Action::FocusUrl);
    app.update(Action::OpenVarPicker { completing: false });
    assert!(app.paste_text("base"));
    let Some(Modal::VarPicker(v)) = app.modals.top() else {
        panic!("picker open");
    };
    assert_eq!(v.input(), "base");
}

/// The response pane's live search input takes a paste; once the query is
/// committed (search inactive) the pane has no caret and the paste
/// reports unhandled.
#[test]
fn paste_reaches_the_response_search_only_while_its_input_is_live() {
    let mut app = App::new_for_test();
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            url: "https://x.test/a".into(),
            headers: vec![],
            body: r#"{"a": 1}"#.into(),
            ttfb: std::time::Duration::from_millis(5),
            elapsed: std::time::Duration::from_millis(5),
            size: 8,
            content_type: None,
        })),
        0,
    );
    app.focus = PaneId::Response;
    assert!(!app.paste_text("early"), "no search open yet");
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('/'));
    assert!(app.paste_text("a"));
    let text = |app: &App| {
        app.session
            .response
            .view()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .input
            .text()
            .to_string()
    };
    assert_eq!(text(&app), "a");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.paste_text("late"), "committed query: no live caret");
    assert_eq!(text(&app), "a");
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
    // On the address bar the add chip gives way to copy/tls; the label
    // sweep below is about content focus.
    app.editor.sub_focus = SubFocus::Content;
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

/// While a new row is already being added (the ghost-row edit), the
/// footer stops advertising "add param"/"add header" — the add is
/// already underway. Editing an EXISTING row keeps the chip: adding
/// another row is still a sensible next action there.
#[test]
fn add_row_chip_hides_only_while_a_new_row_is_being_added() {
    let mut app = app_with_one_param();
    app.editor.sub_focus = SubFocus::Content;
    render_once(&mut app);
    assert!(
        app.hits
            .rect_of(&Hit::FooterChip(Action::TableAddRow))
            .is_some(),
        "no edit live: the add chip shows"
    );

    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    assert!(app.editor.table.editing.is_some(), "a cell edit began");
    render_once(&mut app);
    assert!(
        app.hits
            .rect_of(&Hit::FooterChip(Action::TableAddRow))
            .is_some(),
        "editing an existing row: the add chip stays"
    );

    app.update(Action::TableAddRow);
    let edit = app.editor.table.editing.as_ref().expect("ghost-row edit");
    assert_eq!(edit.row, app.editor.params.len(), "on the ghost row");
    render_once(&mut app);
    assert!(
        app.hits
            .rect_of(&Hit::FooterChip(Action::TableAddRow))
            .is_none(),
        "already adding a row: the add chip hides"
    );
}

#[test]
fn toggle_table_row_action_flips_the_entry() {
    let mut app = app_with_one_param();
    assert!(app.editor.params["page"].enabled);
    app.update(Action::ToggleTableRow(0));
    assert!(!app.editor.params["page"].enabled);
    app.update(Action::ToggleTableRow(0));
    assert!(app.editor.params["page"].enabled);
    app.update(Action::ToggleTableRow(5)); // out of range: inert
    assert!(app.editor.params["page"].enabled);
}

/// With a data row selected, the footer advertises its toggle/delete keys;
/// they leave with the selection (and never appear for the ghost row or a
/// live cell edit, where space/d would type).
#[test]
fn selected_row_footer_chips_come_and_go_with_the_selection() {
    let mut app = app_with_one_param();
    app.editor.sub_focus = SubFocus::Content;
    render_once(&mut app);
    assert!(
        app.hits
            .rect_of(&Hit::FooterChip(Action::ToggleTableRow(0)))
            .is_none(),
        "nothing selected: no row chips"
    );

    app.editor.table.selected = Some(0);
    render_once(&mut app);
    assert!(
        app.hits
            .rect_of(&Hit::FooterChip(Action::ToggleTableRow(0)))
            .is_some()
    );
    assert!(
        app.hits
            .rect_of(&Hit::FooterChip(Action::DeleteTableRow(0)))
            .is_some()
    );

    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    render_once(&mut app);
    assert!(
        app.hits
            .rect_of(&Hit::FooterChip(Action::ToggleTableRow(0)))
            .is_none(),
        "cell edit live: space/d type, so the chips hide"
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
fn table_cell_click_places_the_caret_and_drag_sweeps_a_selection() {
    let mut app = app_with_one_param();
    render_once(&mut app);
    let cell = app
        .hits
        .rect_of(&Hit::TableCell { row: 0, col: 0 })
        .unwrap();
    // Click at the cell's left edge: the edit opens with the caret at the
    // clicked column, not at the end.
    app.handle_mouse(left_down(cell.x, cell.y));
    let edit = app.editor.table.editing.as_ref().expect("editing");
    assert_eq!(edit.input.text(), "page");
    assert_eq!(edit.input.cursor(), 0, "the caret follows the pointer");

    // Button-held motion sweeps a selection, and keeps sweeping past the
    // cell's edge (the drag clamps rather than dropping).
    assert!(app.handle_mouse(dragged(cell.x + 2, cell.y)));
    let edit = app.editor.table.editing.as_ref().unwrap();
    assert_eq!(edit.input.selected_text().as_deref(), Some("pa"));
    assert!(app.handle_mouse(dragged(cell.x + cell.width + 10, cell.y + 3)));
    let edit = app.editor.table.editing.as_ref().unwrap();
    assert_eq!(edit.input.selected_text().as_deref(), Some("page"));
    app.handle_mouse(left_up(cell.x + cell.width + 10, cell.y + 3));
    assert!(app.text_drag.is_none(), "release ends the sweep");
}

#[test]
fn table_cell_double_click_selects_the_word() {
    let mut app = app_with_one_param();
    render_once(&mut app);
    let cell = app
        .hits
        .rect_of(&Hit::TableCell { row: 0, col: 0 })
        .unwrap();
    app.handle_mouse(left_down(cell.x + 1, cell.y));
    render_once(&mut app);
    let cell = app
        .hits
        .rect_of(&Hit::TableCell { row: 0, col: 0 })
        .unwrap();
    app.handle_mouse(left_down(cell.x + 1, cell.y)); // within 400ms => clicks == 2
    let edit = app.editor.table.editing.as_ref().expect("editing");
    assert_eq!(
        edit.input.selected_text().as_deref(),
        Some("page"),
        "double click selects the word under the pointer"
    );
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
        "the rename committed before the delete applied"
    );
    assert!(
        app.editor.params.get_index(1).is_none(),
        "the clicked row is gone"
    );
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
    // after the commit collapses "a" into "c", "b" is row 0 — the delete
    // must remove "b", not whatever now occupies index 1 ("c"). (The
    // edited row itself shows no buttons while its cell edit is live, so a
    // stale click on it can no longer happen at all.)
    hover_row_then_click(&mut app, Hit::TableRow(1), Hit::TableDelete(1));
    assert!(
        !app.editor.params.contains_key("b"),
        "the clicked row is the one deleted"
    );
    assert!(
        app.editor.params.contains_key("c"),
        "and never the row that shifted into its index"
    );
}

#[test]
fn ctrl_s_commits_the_cell_under_edit_into_the_saved_file() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "main/ping", &req("https://x/ping"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("main/ping".into()));
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Params;

    click_hit(&mut app, Hit::TableCell { row: 0, col: 0 }); // the ghost row
    type_chars(&mut app, "page");
    app.handle_key(&Keymap::default_bindings(), ctrl('s'));

    let saved = postui_core::storage::load_request(&app.project.root, "main/ping").unwrap();
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
    postui_core::storage::save_request(&app.project.root, "main/ping", &req("https://x/ping"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("main/ping".into()));
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Params;

    click_hit(&mut app, Hit::TableCell { row: 0, col: 0 }); // the ghost row
    type_chars(&mut app, "page");
    click_hit(&mut app, Hit::FooterChip(Action::SaveRequest));

    let saved = postui_core::storage::load_request(&app.project.root, "main/ping").unwrap();
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
    // alt+1 no longer selects an editor tab (it jumps spaces now), so this
    // drives the tab switch directly via `EditorTabSelect`.
    let mut app = app_with_one_param();
    click_hit(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_chars(&mut app, "2");
    app.update(Action::EditorTabSelect(EditorTab::Headers.index()));
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

/// The Manage screen's tab strip glides like the editor's: selecting a
/// tab (by click, alt+arrows, or `Action::SelectManageTab`) retargets the
/// `StripId::ManageTabs` edges from the previous tab's span toward the new
/// one's; opening the screen from Main snaps straight to the active tab.
#[test]
fn switching_manage_tabs_retargets_the_underline_slide() {
    use crate::components::manage::ManageTab;
    let mut app = App::new_for_test_with_anims(true);
    let left_key = AnimKey::TabUnderline(StripId::ManageTabs);
    let right_key = AnimKey::TabUnderlineWidth(StripId::ManageTabs);
    app.update(Action::OpenManage { tab: None });
    render_once(&mut app);
    assert!(
        app.anims.value(left_key, Instant::now()).is_none(),
        "opening the screen snaps: nothing is in flight"
    );

    app.update(Action::SelectManageTab(ManageTab::Spaces));
    let now = Instant::now();
    assert!(
        app.anims.active(now),
        "the underline is easing after the switch"
    );
    let spans = ManageTab::strip_spans();
    let (x, w) = spans[ManageTab::Spaces.index()];
    let done_at = now + app.ui_settings.anim_ms.tab_slide + Duration::from_millis(5);
    assert_eq!(app.anims.value(left_key, done_at), Some(x as f32));
    assert_eq!(app.anims.value(right_key, done_at), Some((x + w) as f32));
    let (vx, _) = spans[ManageTab::Variables.index()];
    let mid = app.anims.value(left_key, now).unwrap();
    assert!(
        (vx as f32) <= mid && mid < x as f32,
        "the left edge is on its way from Variables ({vx}) to Spaces ({x}): {mid}"
    );

    // Closing and reopening forgets the glide so the reopened bar snaps.
    app.update(Action::CloseScreen);
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Variables),
    });
    assert!(app.anims.value(left_key, Instant::now()).is_none());
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
            slug: format!("main/r{i:02}"),
            broken: None,
            method: Some(postui_core::model::Method::Get),
        })
        .collect();
    app.sidebar.refresh(slugs, "main", &Default::default());
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
            slug: format!("main/r{i:02}"),
            broken: None,
            method: Some(postui_core::model::Method::Get),
        })
        .collect();
    app.sidebar.refresh(slugs, "main", &Default::default());
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
            url: "https://x.test/a".into(),
            headers: vec![],
            size: body.len(),
            body,
            ttfb: std::time::Duration::from_millis(1),
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
fn the_header_env_chip_still_switches_environments_on_the_manager_screen() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenManage { tab: None });
    render_once(&mut app);
    assert!(rendered_text(&mut app).contains("qa \u{25be}"));

    // The manager has no switcher of its own — the header's env chip is
    // the one env button, and it stays clickable on this screen.
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::HeaderEnv)
        .expect("header env chip registered on the manager screen");
    app.handle_mouse(left_down(r.x + 1, r.y));
    match app.modals.top() {
        Some(Modal::Dropdown(state)) => assert_eq!(
            state.anchor, r,
            "the dropdown anchors to the header env chip"
        ),
        _ => panic!("the header env chip opens the env dropdown from the manager screen too"),
    }

    // Switching relabels the chip and the group's inline selection with it.
    app.update(Action::Close);
    app.update(Action::SwitchEnv(Some("dev".into())));
    let content = rendered_text(&mut app);
    assert!(content.contains("dev \u{25be}"), "{content}");
    assert!(
        !content.contains("user (") && content.contains('\u{25cf}'),
        "dev has no entries for the selector, so its row wears the unresolved dot: {content}"
    );
}

#[test]
fn the_manager_left_list_lists_variables_then_selectors() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenManage { tab: None });
    let content = rendered_text(&mut app);
    assert!(
        content.contains("VARIABLES") && content.contains("SELECTORS"),
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
    app.update(Action::OpenManage { tab: None });
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
    assert_eq!(labels, vec!["Rename\u{2026}", "Duplicate", "Delete"]);
    assert_eq!(
        menu.items[2].action,
        Some(Action::DeleteVar {
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
        .rect_of(&crate::hit::Hit::HeaderManage)
        .expect("vars button registered in the header");
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.screen, crate::app::Screen::Manage);
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::HeaderManage)
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
    match app.modals.top() {
        Some(Modal::Dropdown(state)) => assert_eq!(
            state.anchor, r,
            "the dropdown anchors to the header env chip"
        ),
        _ => panic!("clicking the env name should open the env dropdown"),
    }
}

/// The keycap pill beside the env chip is a real cycle button: clicking
/// it steps the environment exactly as alt+c does, no chooser involved.
#[test]
fn click_header_env_cycle_pill_cycles_the_environment() {
    let (mut app, _dir) = app_with_envs();
    render_once(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::HeaderEnvCycle)
        .expect("env-cycle pill registered in the header");
    assert_eq!(app.project.env_label(), "no env");
    app.handle_mouse(left_down(r.x + 1, r.y));
    assert_eq!(app.project.env_label(), "prod");
    assert!(app.modals.is_empty(), "cycling opens no dropdown");
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
            url: "https://x.test/a".into(),
            headers: vec![],
            body: body.clone(),
            ttfb: std::time::Duration::from_millis(1),
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
    postui_core::storage::save_request(&app.project.root, "main/r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/r".into()));
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
fn discard_changes_reverts_immediately_with_an_undo_hint() {
    let mut app = dirty_app(); // url edited from "https://x/r"
    app.update(Action::DiscardChanges);
    assert!(
        app.modals.is_empty(),
        "discard is undoable, so no confirm gate"
    );
    assert!(!app.editor.is_dirty(), "reverted to the saved snapshot");
    assert_eq!(app.editor.url.text(), "https://x/r");
    assert!(
        rendered_text(&mut app).contains("^Z undoes"),
        "the toast advertises the escape hatch"
    );
}

#[test]
fn discard_on_a_clean_editor_is_a_no_op() {
    let mut app = dirty_app();
    app.update(Action::DiscardChanges);
    app.toasts = Default::default();
    app.update(Action::DiscardChanges);
    assert!(
        app.toasts.is_empty(),
        "a clean editor has nothing to discard, so no toast"
    );
}

#[test]
fn discard_is_itself_undoable() {
    let mut app = dirty_app();
    app.capture_undo(); // the dirtying edit becomes its own step
    let dirty_url = app.editor.url.text().to_string();
    app.update(Action::DiscardChanges);
    app.capture_undo(); // …and so does the discard
    assert_eq!(app.editor.url.text(), "https://x/r");
    app.update(Action::Undo);
    assert_eq!(
        app.editor.url.text(),
        dirty_url,
        "undo brings the discarded edit back"
    );
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
    let saved = postui_core::storage::load_request(&app.project.root, "main/fresh").unwrap();
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
    postui_core::storage::save_request(&app.project.root, "main/other", &req("https://x/other"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("main/other".into()));
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "the scratch content gates the open"
    );
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert_eq!(app.editor.slug.as_deref(), Some("main/other"));
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
        kind: crate::components::modal::PromptKind::NewSelector {
            shared: false,
            on_toggle: false,
        },
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
            url: "https://x.test/a".into(),
            headers: vec![],
            body: body.clone(),
            ttfb: std::time::Duration::from_millis(1),
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
fn double_click_in_body_selects_the_word_and_drag_extends_by_words() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("alpha beta gamma\nsecond");
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");
    // 2 lines -> a 2-cell gutter; content column 0 is at area.x + 2.
    app.handle_mouse(left_down(area.x + 2 + 1, area.y));
    app.handle_mouse(left_down(area.x + 2 + 1, area.y)); // within 400ms => clicks == 2
    assert_eq!(app.editor.body_selected_text().as_deref(), Some("alpha"));
    // The double click armed a word-wise sweep: dragging onto "beta"
    // extends the selection a whole word at a time.
    assert!(app.handle_mouse(dragged(area.x + 2 + 7, area.y)));
    app.handle_mouse(left_up(area.x + 2 + 7, area.y));
    assert_eq!(
        app.editor.body_selected_text().as_deref(),
        Some("alpha beta")
    );
}

#[test]
fn double_click_in_response_selects_the_word_and_drag_extends_by_words() {
    let mut app = App::new_for_test();
    ready_response(&mut app, "plain text body"); // not JSON -> Raw view
    render_once(&mut app);
    let area = app
        .session
        .response
        .view()
        .unwrap()
        .last_area
        .expect("body area recorded");
    app.handle_mouse(left_down(area.x + 1, area.y));
    app.handle_mouse(left_down(area.x + 1, area.y)); // within 400ms => clicks == 2
    assert_eq!(
        app.session.response.selected_text().as_deref(),
        Some("plain")
    );
    assert!(app.handle_mouse(dragged(area.x + 7, area.y)));
    app.handle_mouse(left_up(area.x + 7, area.y));
    assert_eq!(
        app.session.response.selected_text().as_deref(),
        Some("plain text")
    );
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
    // Both live in the active space `main`; `auth` is a folder *inside*
    // it, so the tree still groups (a second space would simply be
    // invisible here).
    postui_core::storage::save_request(dir.path(), "main/auth/login", &req("https://x/login"))
        .unwrap();
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    assert_eq!(
        app.sidebar.rows,
        vec![
            Row::Request {
                slug: "main/ping".into(),
                name: "ping".into(),
                depth: 0,
                broken: None,
                method: Some(postui_core::model::Method::Get),
            },
            Row::Folder {
                path: "main/auth".into(),
                name: "auth".into(),
                depth: 0,
                expanded: false,
            },
        ],
        "the space itself is never a row; its own top level is depth 0"
    );

    // Nothing selected at startup: the first j lands on "ping" (index
    // 0), the second reaches the "auth" folder (index 1); Enter expands
    // it, then "main/auth/login" (index 2) becomes visible and Enter
    // opens it.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('j'));
    app.handle_key(&keymap, plain('j'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(&keymap, plain('j'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.slug.as_deref(), Some("main/auth/login"));
}

#[test]
fn startup_restores_persisted_open_request() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping")).unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: None,
            open_request: Some("main/ping".into()),
            expanded: vec![],
            ..Default::default()
        },
    )
    .unwrap();

    let app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("main/ping"),
        "the persisted open request loads into the editor at startup, \
         same as it does on a project switch"
    );
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/ping"));
}

#[test]
fn startup_restores_open_request_inside_a_collapsed_folder() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    // `auth` is a folder *inside* the active space, not a space: the point
    // of this test is the ancestor-folder expansion, which only happens for
    // a slug nested under the space root.
    postui_core::storage::save_request(dir.path(), "main/auth/login", &req("https://x/l")).unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: None,
            open_request: Some("main/auth/login".into()),
            expanded: vec![],
            ..Default::default()
        },
    )
    .unwrap();

    let app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.editor.slug.as_deref(), Some("main/auth/login"));
    assert_eq!(
        app.sidebar.selected_slug().as_deref(),
        Some("main/auth/login"),
        "restoring expands the request's ancestor folders so the \
         selected row is actually visible"
    );
}

#[test]
fn startup_without_persisted_open_request_selects_nothing() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping")).unwrap();

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
    // `auth` is a folder inside the active space, so opening `login`
    // really does have an ancestor folder to expand.
    postui_core::storage::save_request(dir.path(), "main/auth/login", &req("https://x/l")).unwrap();
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    // Opened by an out-of-band route (palette, dirty-gate confirm, …)
    // rather than a sidebar click: the sidebar must follow, expanding
    // ancestors as needed, so selection and open request can't diverge.
    app.update(Action::ForceOpenRequest("main/auth/login".into()));
    assert_eq!(
        app.sidebar.selected_slug().as_deref(),
        Some("main/auth/login")
    );
}

#[test]
fn opening_another_request_swaps_the_response_panel() {
    use crate::components::response::ResponseState;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "main/a", &req("https://x/a")).unwrap();
    postui_core::storage::save_request(dir.path(), "main/b", &req("https://x/b")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::ForceOpenRequest("main/a".into()));
    app.session
        .response
        .set_state(ResponseState::Failed("a's result".into()), 0);

    app.update(Action::ForceOpenRequest("main/b".into()));
    assert!(
        matches!(app.session.response.state(), ResponseState::Empty),
        "b never sent anything; showing a's response would mislabel it"
    );

    app.update(Action::ForceOpenRequest("main/a".into()));
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
    postui_core::storage::save_request(dir.path(), "main/a", &req("https://x/a")).unwrap();
    postui_core::storage::save_request(dir.path(), "main/b", &req("https://x/b")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    // Open "a", then edit its URL so the editor becomes dirty.
    app.update(Action::ForceOpenRequest("main/a".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('/'));
    assert!(app.editor.is_dirty());

    // Requesting to open "b" while dirty must prompt instead of opening.
    app.update(Action::OpenRequest("main/b".into()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("main/a"),
        "still on the original request"
    );

    // 'd' discards the edit and opens "b".
    app.handle_key(&keymap, plain('d'));
    assert_eq!(app.editor.slug.as_deref(), Some("main/b"));
    assert!(!app.editor.is_dirty());

    // Back to "a", dirty it again, this time choose 's' to save & open.
    let mut app = App::with_root(app.tx.clone(), dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/a".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('/'));
    assert!(app.editor.is_dirty());
    app.update(Action::OpenRequest("main/b".into()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&keymap, plain('s'));
    assert_eq!(app.editor.slug.as_deref(), Some("main/b"));
    let saved = postui_core::storage::load_request(dir.path(), "main/a").unwrap();
    assert_eq!(
        saved.url, "https://x/a/",
        "the edit was persisted before opening b"
    );
}

fn sidebar_test_app() -> (App, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::storage::save_request(dir.path(), "main/api/ping", &req("https://x/ping"))
        .unwrap();
    postui_core::storage::save_request(dir.path(), "main/top", &req("https://x/top")).unwrap();
    let app = App::with_root(tx, dir.path().to_path_buf());
    (app, dir)
}

/// A two-space project: `main` holds `alpha` + `beta`, `auth` holds
/// `login`. The space list is materialised by `create_space`, so
/// `App::with_root` opens with `spaces == ["main", "auth"]`.
fn spaced_app() -> (App, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    postui_core::project::create_space(dir.path(), "auth").unwrap();
    for slug in ["main/alpha", "main/beta", "auth/login"] {
        postui_core::storage::save_request(dir.path(), slug, &req("https://x/1")).unwrap();
    }
    let app = App::with_root(tx, dir.path().to_path_buf());
    (app, dir)
}

#[test]
fn sidebar_shows_only_the_active_space_rooted_at_depth_zero() {
    let (mut app, _dir) = spaced_app();
    render_once(&mut app);
    let slugs: Vec<String> = app
        .sidebar
        .rows
        .iter()
        .filter_map(|r| match r {
            Row::Request { slug, depth, .. } => {
                assert_eq!(*depth, 0);
                Some(slug.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(slugs, ["main/alpha", "main/beta"]);
    assert!(
        !app.sidebar
            .rows
            .iter()
            .any(|r| matches!(r, Row::Folder { .. })),
        "the space is never a row"
    );
}

#[test]
fn switching_spaces_restores_each_spaces_open_request_and_persists() {
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/beta".into()));
    app.update(Action::SwitchSpace("auth".into()));
    assert_eq!(app.project.active_space, "auth");
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("auth/login"),
        "first request when nothing remembered"
    );
    app.update(Action::SwitchSpace("main".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("main/beta"), "remembered");
    let st = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(st.space.as_deref(), Some("main"));
    assert_eq!(st.space_open["auth"], "auth/login");
    assert_eq!(st.space_open["main"], "main/beta");
}

#[test]
fn switching_to_an_empty_space_clears_the_editor() {
    let (mut app, dir) = spaced_app();
    postui_core::project::create_space(dir.path(), "empty").unwrap();
    app.update(Action::ReloadProjectFiles);
    app.project.reload_spaces();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    app.update(Action::SwitchSpace("empty".into()));
    assert_eq!(app.project.active_space, "empty");
    assert!(app.editor.slug.is_none());
    assert!(app.sidebar.rows.is_empty());
}

/// Dirties the open request the way the dirty-gate tests do: a keystroke
/// into the URL field.
fn dirty_the_editor(app: &mut App) {
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    assert!(app.editor.is_dirty());
}

#[test]
fn switching_spaces_goes_through_the_dirty_gate() {
    let (mut app, _dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    dirty_the_editor(&mut app);
    app.update(Action::SwitchSpace("auth".into()));
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { .. })),
        "gate opened"
    );
    assert_eq!(app.project.active_space, "main", "not switched yet");
    app.update(Action::Close);
    assert_eq!(app.project.active_space, "main", "cancel keeps the space");
}

#[test]
fn jump_and_cycle_resolve_by_position_and_wrap() {
    let (mut app, _dir) = spaced_app();
    app.update(Action::JumpSpace(2));
    assert_eq!(app.project.active_space, "auth");
    app.update(Action::JumpSpace(9));
    assert_eq!(app.project.active_space, "auth", "out of range is a no-op");
    app.update(Action::CycleSpace(1));
    assert_eq!(app.project.active_space, "main", "wraps");
    app.update(Action::CycleSpace(-1));
    assert_eq!(app.project.active_space, "auth");
}

#[test]
fn opening_a_request_from_another_space_switches_first() {
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenRequest("auth/login".into()));
    assert_eq!(app.project.active_space, "auth");
    assert_eq!(app.editor.slug.as_deref(), Some("auth/login"));
    assert!(
        app.sidebar
            .rows
            .iter()
            .all(|r| matches!(r, Row::Request { slug, .. } if slug.starts_with("auth/")))
    );
}

#[test]
fn startup_restores_the_stored_space_and_its_request() {
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("auth/login".into()));
    drop(app);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.project.active_space, "auth");
    assert_eq!(app.editor.slug.as_deref(), Some("auth/login"));
}

#[test]
fn new_and_renamed_requests_land_inside_the_active_space() {
    let (mut app, dir) = spaced_app();
    app.update(Action::SwitchSpace("auth".into()));
    app.update(Action::CreateRequest("Tokens/Refresh".into()));
    assert!(
        dir.path()
            .join("requests/auth/tokens/refresh.toml")
            .is_file()
    );
    assert_eq!(app.editor.slug.as_deref(), Some("auth/tokens/refresh"));
    app.update(Action::PromptRenameRequest);
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("prompt")
    };
    assert_eq!(input.text(), "tokens/Refresh", "prefill hides the space");
    app.update(Action::Close);
    app.update(Action::RenameRequest {
        from: "auth/tokens/refresh".into(),
        to: "Renewed".into(),
    });
    assert!(dir.path().join("requests/auth/renewed.toml").is_file());
}

#[test]
fn a_loose_top_level_file_warns_once_and_never_enters_the_tree() {
    let (mut app, dir) = spaced_app();
    let rows_before = app.sidebar.rows.clone();
    std::fs::write(dir.path().join("requests/loose.toml"), "url = \"x\"\n").unwrap();

    app.toasts = Default::default();
    app.update(Action::RefreshSidebar);
    let warned: Vec<_> = app
        .toasts
        .entries()
        .into_iter()
        .filter(|(m, _)| m.contains("loose.toml"))
        .collect();
    assert_eq!(warned.len(), 1, "{:?}", app.toasts.messages());
    assert_eq!(
        warned[0].1,
        &crate::components::toast::ToastKind::Warning,
        "a file the app deliberately never migrates is a warning, not an error"
    );

    // A second refresh must not re-toast: a loose file is a chronic state,
    // and re-reporting it would paint a banner on every save/open/delete.
    app.toasts = Default::default();
    app.update(Action::RefreshSidebar);
    assert!(
        !app.toasts
            .messages()
            .iter()
            .any(|m| m.contains("loose.toml")),
        "{:?}",
        app.toasts.messages()
    );

    assert_eq!(
        app.sidebar.rows, rows_before,
        "the loose file is skipped, never listed"
    );
}

#[test]
fn a_request_under_an_invalid_space_dir_warns_once_and_never_enters_the_tree() {
    let (mut app, dir) = spaced_app();
    let rows_before = app.sidebar.rows.clone();
    std::fs::create_dir_all(dir.path().join("requests/Auth")).unwrap();
    std::fs::write(dir.path().join("requests/Auth/login.toml"), "url = \"x\"\n").unwrap();

    app.toasts = Default::default();
    app.update(Action::RefreshSidebar);
    let warned: Vec<_> = app
        .toasts
        .entries()
        .into_iter()
        .filter(|(m, _)| m.contains("Auth/login.toml"))
        .collect();
    assert_eq!(warned.len(), 1, "{:?}", app.toasts.messages());
    assert!(
        warned[0]
            .0
            .contains("is not in a valid space (space names are a-z 0-9 - _)"),
        "{}",
        warned[0].0
    );
    assert_eq!(
        warned[0].1,
        &crate::components::toast::ToastKind::Warning,
        "a chronic, never-migrated state is a warning, not an error"
    );

    // Chronic, so it rides the same warn-once channel as a loose file.
    app.toasts = Default::default();
    app.update(Action::RefreshSidebar);
    assert!(
        !app.toasts
            .messages()
            .iter()
            .any(|m| m.contains("Auth/login.toml")),
        "{:?}",
        app.toasts.messages()
    );

    assert!(
        !app.project.spaces.iter().any(|s| s == "Auth"),
        "{:?}",
        app.project.spaces
    );
    assert_eq!(
        app.sidebar.rows, rows_before,
        "the request under a non-space directory is skipped, never listed"
    );
}

#[test]
fn an_invalid_listed_space_name_warns_once_and_survives_the_next_space_op() {
    let (mut app, dir) = spaced_app();
    postui_core::project::write_spaces(
        dir.path(),
        &["main".into(), "Not Valid".into(), "auth".into()],
    )
    .unwrap();
    app.project.reload_meta();
    app.project.reload_spaces();

    app.toasts = Default::default();
    app.update(Action::RefreshSidebar);
    let warned: Vec<_> = app
        .toasts
        .entries()
        .into_iter()
        .filter(|(m, _)| m.contains("Not Valid"))
        .collect();
    assert_eq!(warned.len(), 1, "{:?}", app.toasts.messages());
    assert_eq!(warned[0].1, &crate::components::toast::ToastKind::Warning);

    app.toasts = Default::default();
    app.update(Action::RefreshSidebar);
    assert!(
        !app.toasts
            .messages()
            .iter()
            .any(|m| m.contains("Not Valid")),
        "{:?}",
        app.toasts.messages()
    );

    // The next space op must not quietly erase the user's hand-written
    // entry — it keeps its slot in `project.toml`.
    app.update(Action::CreateSpace("billing".into()));
    assert_eq!(
        postui_core::project::load_meta(dir.path()).unwrap().spaces,
        ["main", "Not Valid", "auth", "billing"]
    );
    assert_eq!(app.project.spaces, ["main", "auth", "billing"]);
}

#[test]
fn sidebar_footer_advertises_move_to_space_not_the_space_cycle() {
    let (mut app, _dir) = spaced_app();
    app.focus = PaneId::Sidebar;
    // Wide enough that the chip strip isn't truncated before the last
    // sidebar chip.
    let text = rendered_text_tall(&mut app);
    assert!(text.contains("m  move"), "{text}");
    // Switching spaces is the header's (space pill + cycle pill); the
    // footer no longer repeats it.
    assert!(!text.contains("alt+]"), "{text}");
}

/// `m` on a selected sidebar request opens the "Move to space" chooser —
/// the keyboard twin of the row menu's "Move to space…" — and the footer
/// chip / palette dispatch the same slug-less action.
#[test]
fn m_in_the_sidebar_opens_the_move_to_space_chooser_for_the_selection() {
    let (mut app, _dir) = spaced_app();
    app.focus = PaneId::Sidebar;
    app.sidebar.select_slug("main/alpha");
    app.handle_key(&Keymap::default_bindings(), plain('m'));
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        panic!("expected the Move to space chooser");
    };
    assert_eq!(c.title(), "Move to space");
}

// -- Task 11: space CRUD --------------------------------------------------

#[test]
fn new_space_prompt_creates_and_switches() {
    let (mut app, dir) = spaced_app();
    let keymap = Keymap::default_bindings();
    app.update(Action::OpenNewSpacePrompt);
    for c in "billing".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert!(dir.path().join("requests/billing").is_dir());
    assert_eq!(app.project.spaces, ["main", "auth", "billing"]);
    assert_eq!(app.project.active_space, "billing");
    assert!(app.editor.slug.is_none());
    let toasts = app.toasts.messages().len();
    app.update(Action::CreateSpace("auth".into()));
    assert!(app.toasts.messages().len() > toasts, "duplicate toasts");
    app.update(Action::CreateSpace("   ".into()));
    assert_eq!(app.project.spaces.len(), 3);
}

#[test]
fn creating_a_space_with_a_free_form_name_slugs_the_folder_and_shows_the_name() {
    let (mut app, dir) = spaced_app();
    app.update(Action::CreateSpace("Auth v2!".into()));
    assert!(dir.path().join("requests/auth-v2").is_dir());
    assert_eq!(app.project.active_space, "auth-v2");
    assert_eq!(app.project.space_name("auth-v2"), "Auth v2!");
    let text = rendered_text_wide(&mut app);
    assert!(text.contains("Space: Auth v2!"), "{text}");
    assert!(
        app.toasts.messages().contains(&"Created space Auth v2!"),
        "{:?}",
        app.toasts.messages()
    );
    // The chooser lists display names but switches by slug.
    app.update(Action::OpenSpaceChooser);
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("dropdown")
    };
    let row = state
        .items
        .iter()
        .find(|it| it.label == "3  Auth v2!")
        .expect("display name in the chooser");
    assert_eq!(row.action, Some(Action::SwitchSpace("auth-v2".into())));
}

#[test]
fn renaming_a_space_takes_a_display_name_and_reslugs() {
    let (mut app, dir) = spaced_app();
    app.update(Action::SwitchSpace("auth".into()));
    app.update(Action::RenameSpace {
        from: "auth".into(),
        to: "Identity & SSO".into(),
    });
    assert_eq!(app.project.spaces, ["main", "identity-sso"]);
    assert_eq!(app.project.active_space, "identity-sso");
    assert_eq!(app.editor.slug.as_deref(), Some("identity-sso/login"));
    assert!(
        dir.path()
            .join("requests/identity-sso/login.toml")
            .is_file()
    );
    assert_eq!(app.project.space_name("identity-sso"), "Identity & SSO");
    // The rename prompt opens prefilled with the display name, not the slug.
    app.update(Action::PromptRenameSpace("identity-sso".into()));
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("prompt")
    };
    assert_eq!(input.text(), "Identity & SSO");
}

#[test]
fn creating_an_environment_with_a_free_form_name_slugs_the_file_and_undoes_with_project_toml() {
    let (mut app, dir) = app_with_envs();
    app.update(Action::CreateEnv("Staging (EU)".into()));
    assert!(dir.path().join("environments/staging-eu.toml").is_file());
    assert_eq!(app.project.active_env.as_deref(), Some("staging-eu"));
    assert_eq!(app.project.env_name("staging-eu"), "Staging (EU)");
    let text = rendered_text_wide(&mut app);
    assert!(text.contains("Environment: Staging (EU)"), "{text}");

    app.update(Action::Undo);
    assert!(!dir.path().join("environments/staging-eu.toml").exists());
    let meta = postui_core::project::load_meta(dir.path()).unwrap();
    assert!(
        meta.environment.get("staging-eu").is_none(),
        "project.toml rides along with the step"
    );
    app.update(Action::Redo);
    assert!(dir.path().join("environments/staging-eu.toml").is_file());
    assert_eq!(app.project.env_name("staging-eu"), "Staging (EU)");
}

#[test]
fn renaming_an_environment_takes_a_display_name_and_reslugs() {
    let (mut app, dir) = app_with_envs();
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.update(Action::RenameEnv {
        from: "qa".into(),
        to: "QA / Staging".into(),
    });
    assert!(dir.path().join("environments/qa-staging.toml").is_file());
    assert!(!dir.path().join("environments/qa.toml").exists());
    assert_eq!(app.project.active_env.as_deref(), Some("qa-staging"));
    assert_eq!(app.project.env_name("qa-staging"), "QA / Staging");
    app.update(Action::PromptRenameEnv("qa-staging".into()));
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("prompt")
    };
    assert_eq!(input.text(), "QA / Staging");
    app.update(Action::Close);

    app.update(Action::OpenEnvChooser);
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("dropdown")
    };
    let row = state
        .items
        .iter()
        .find(|it| it.label == "QA / Staging")
        .expect("display name in the chooser");
    assert_eq!(
        row.action,
        Some(Action::SwitchEnv(Some("qa-staging".into())))
    );
    app.update(Action::Close);

    // Undo puts the file and its name back.
    app.update(Action::Undo);
    assert!(dir.path().join("environments/qa.toml").is_file());
    assert!(!dir.path().join("environments/qa-staging.toml").exists());
    let meta = postui_core::project::load_meta(dir.path()).unwrap();
    assert!(meta.environment.get("qa-staging").is_none());
}

#[test]
fn manage_lists_show_display_names_and_the_move_toast_uses_them() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = spaced_app();
    app.update(Action::CreateSpace("Auth v2".into()));
    app.update(Action::SwitchSpace("main".into()));
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Spaces),
    });
    let text = rendered_text_tall(&mut app);
    assert!(text.contains("3  Auth v2"), "{text}");
    app.update(Action::Close);
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "auth-v2".into(),
    });
    assert!(
        app.toasts.messages().contains(&"Moved alpha to Auth v2"),
        "{:?}",
        app.toasts.messages()
    );
}

#[test]
fn rename_space_cascades_editor_sidebar_and_state() {
    let (mut app, dir) = spaced_app();
    app.update(Action::SwitchSpace("auth".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("auth/login"));
    app.update(Action::RenameSpace {
        from: "auth".into(),
        to: "identity".into(),
    });
    assert_eq!(app.project.spaces, ["main", "identity"]);
    assert_eq!(app.project.active_space, "identity");
    assert_eq!(app.editor.slug.as_deref(), Some("identity/login"));
    assert!(!app.editor.is_dirty());
    assert!(dir.path().join("requests/identity/login.toml").is_file());
    let st = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(st.space.as_deref(), Some("identity"));
    assert_eq!(st.space_open["identity"], "identity/login");
}

#[test]
fn delete_space_confirms_with_the_count_then_trashes_and_undoes() {
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    app.update(Action::DeleteSpace("main".into()));
    let Some(Modal::Confirm {
        title,
        body,
        choices,
    }) = app.modals.top()
    else {
        panic!("confirm")
    };
    assert_eq!(title, "Delete space \"main\"?");
    assert_eq!(body, "Its 2 requests will be deleted.");
    assert_eq!(choices[0].1, "Delete 2 requests");
    let confirm = choices[0].0;
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain(confirm));
    assert!(app.modals.is_empty());
    assert!(!dir.path().join("requests/main").exists());
    assert_eq!(app.project.spaces, ["auth"]);
    assert_eq!(
        app.project.active_space, "auth",
        "switched away before deleting"
    );
    assert_eq!(app.editor.slug.as_deref(), Some("auth/login"));
    assert_eq!(
        postui_core::project::load_meta(dir.path()).unwrap().spaces,
        ["auth"]
    );

    app.update(Action::Undo);
    assert!(dir.path().join("requests/main/alpha.toml").is_file());
    assert_eq!(
        app.project.spaces,
        ["main", "auth"],
        "list entry restored at its old position"
    );
}

#[test]
fn delete_space_refuses_the_last_space_and_shows_a_plain_label_for_an_empty_one() {
    let (mut app, dir) = spaced_app();
    postui_core::project::create_space(dir.path(), "empty").unwrap();
    app.update(Action::ReloadProjectFiles);
    app.project.reload_spaces();
    app.update(Action::DeleteSpace("empty".into()));
    let Some(Modal::Confirm { body, choices, .. }) = app.modals.top() else {
        panic!("confirm")
    };
    assert_eq!(body, "");
    assert_eq!(choices[0].1, "Delete space");
    app.update(Action::Close);

    // One request is a *request*, not "1 requests".
    app.update(Action::DeleteSpace("auth".into()));
    let Some(Modal::Confirm { body, choices, .. }) = app.modals.top() else {
        panic!("confirm")
    };
    assert_eq!(body, "Its 1 request will be deleted.");
    assert_eq!(choices[0].1, "Delete 1 request");
    app.update(Action::Close);

    app.update(Action::ForceDeleteSpace("auth".into()));
    app.update(Action::ForceDeleteSpace("empty".into()));
    assert_eq!(app.project.spaces, ["main"]);
    let toasts = app.toasts.messages().len();
    app.update(Action::ForceDeleteSpace("main".into()));
    assert_eq!(app.project.spaces, ["main"]);
    assert!(app.toasts.messages().len() > toasts);
}

#[test]
fn delete_space_holding_a_dirty_open_request_gates_first() {
    let (mut app, _dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    dirty_the_editor(&mut app);
    app.update(Action::DeleteSpace("main".into()));
    let Some(Modal::Confirm { title, .. }) = app.modals.top() else {
        panic!("gate")
    };
    assert_eq!(title, "Unsaved changes");
}

#[test]
fn move_space_reorders_and_persists() {
    let (mut app, dir) = spaced_app();
    app.update(Action::MoveSpace {
        name: "auth".into(),
        delta: -1,
    });
    assert_eq!(app.project.spaces, ["auth", "main"]);
    assert_eq!(
        postui_core::project::load_meta(dir.path()).unwrap().spaces,
        ["auth", "main"]
    );
    app.update(Action::JumpSpace(1));
    assert_eq!(
        app.project.active_space, "auth",
        "alt+1 follows the new order"
    );
}

#[test]
fn two_move_space_steps_in_a_row_both_land_without_waiting_for_mtime() {
    // `ReloadProjectFiles` is mtime-gated; on a coarse-mtime filesystem two
    // writes in the same tick look unchanged. Nothing here sleeps. (The
    // mtime hazard itself is pinned deterministically by
    // `project_ctx::tests::reload_meta_sees_a_write_the_stamp_cannot`.)
    let (mut app, dir) = spaced_app();
    postui_core::project::create_space(dir.path(), "billing").unwrap();
    app.project.reload_meta();
    app.project.reload_spaces();
    assert_eq!(app.project.spaces, ["main", "auth", "billing"]);

    app.update(Action::MoveSpace {
        name: "billing".into(),
        delta: -1,
    });
    app.update(Action::MoveSpace {
        name: "billing".into(),
        delta: -1,
    });
    assert_eq!(app.project.spaces, ["billing", "main", "auth"]);
    assert_eq!(
        postui_core::project::load_meta(dir.path()).unwrap().spaces,
        ["billing", "main", "auth"]
    );
}

#[test]
fn move_all_requests_empties_the_source_and_follows_the_open_request() {
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    let steps_before = app.history.undo_len();
    app.update(Action::MoveAllRequests {
        from: "main".into(),
        to: "auth".into(),
    });
    assert!(dir.path().join("requests/auth/alpha.toml").is_file());
    assert!(dir.path().join("requests/auth/beta.toml").is_file());
    assert_eq!(app.project.active_space, "auth");
    assert_eq!(app.editor.slug.as_deref(), Some("auth/alpha"));
    assert_eq!(app.sidebar.space_counts().get("main"), None);

    // The follow re-seeds the shadow through `ForceOpenRequest`, so the
    // next input event finds nothing to diff: a move is not an edit, and
    // must never leave a phantom `EditorDelta` behind.
    assert!(!app.capture_undo(), "the move itself is not an edit");
    assert_eq!(app.history.undo_len(), steps_before);
    dirty_the_editor(&mut app);
    assert!(app.capture_undo());
    assert_eq!(
        app.history.undo_len(),
        steps_before + 1,
        "only the typed character, no phantom step for the move"
    );
}

#[test]
fn move_all_requests_holding_a_dirty_open_request_gates_first() {
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    dirty_the_editor(&mut app);
    app.update(Action::MoveAllRequests {
        from: "main".into(),
        to: "auth".into(),
    });
    let Some(Modal::Confirm { title, .. }) = app.modals.top() else {
        panic!("gate")
    };
    assert_eq!(title, "Unsaved changes");
    assert!(
        dir.path().join("requests/main/alpha.toml").is_file(),
        "nothing moved yet"
    );
    assert_eq!(app.project.active_space, "main");

    // Discarding runs the move: the editor is re-opened from disk by
    // `ForceOpenRequest`, which re-seeds the shadow — the discarded edit
    // must not come back as a phantom undo step.
    let steps_before = app.history.undo_len();
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert!(dir.path().join("requests/auth/alpha.toml").is_file());
    assert_eq!(app.editor.slug.as_deref(), Some("auth/alpha"));
    assert!(!app.editor.is_dirty(), "reloaded clean from disk");
    assert!(!app.capture_undo(), "the move is not an edit");
    assert_eq!(app.history.undo_len(), steps_before);
}

#[test]
fn request_context_menu_offers_one_move_row_that_opens_the_space_chooser() {
    let (mut app, _dir) = spaced_app();
    render_once(&mut app);
    let r = app.hits.rect_of(&Hit::SidebarRow(0)).unwrap();
    app.handle_mouse(right_down(r.x + 1, r.y));
    let Some(Modal::Dropdown(d)) = app.modals.top() else {
        panic!("menu")
    };
    let labels: Vec<&str> = d.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Open",
            "Duplicate",
            "Rename\u{2026}",
            "Move to space\u{2026}",
            "Delete"
        ]
    );
    let move_row = d.items[3].action.clone();
    assert_eq!(
        move_row,
        Some(Action::PromptMoveRequestToSpace("main/alpha".into()))
    );
    app.update(Action::Close);
    app.update(move_row.unwrap());
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        panic!("a chooser of the other spaces, not a flat row per space")
    };
    assert_eq!(c.title(), "Move to space");
    assert_eq!(c.selected_label(), Some("auth"));
    let result = c.confirm().unwrap();
    assert_eq!(
        result.actions,
        vec![Action::MoveRequestToSpace {
            slug: "main/alpha".into(),
            space: "auth".into()
        }]
    );
}

/// With no other space to move to, the row is left out rather than
/// opening an empty chooser.
#[test]
fn request_context_menu_has_no_move_row_in_a_single_space_project() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "main/ping", &req("https://x/ping"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    render_once(&mut app);
    let r = app.hits.rect_of(&Hit::SidebarRow(0)).unwrap();
    app.handle_mouse(right_down(r.x + 1, r.y));
    let Some(Modal::Dropdown(d)) = app.modals.top() else {
        panic!("menu")
    };
    assert!(
        d.items.iter().all(|i| !i.label.starts_with("Move to")),
        "{:?}",
        d.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn move_request_to_space_moves_the_file_stays_put_and_undoes() {
    // User feedback: following the request into its new space made moving
    // several in a row a chore (each move switched spaces). The move
    // leaves the sidebar where it was, on the neighbor, with the editor
    // cleared — the same shape as delete from this space's point of view.
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    render_once(&mut app);
    app.sidebar.select_slug("main/alpha");
    let steps_before = app.history.undo_len();
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "auth".into(),
    });
    assert!(dir.path().join("requests/auth/alpha.toml").is_file());
    assert_eq!(app.project.active_space, "main");
    assert_eq!(app.editor.slug, None, "the editor clears");
    assert_eq!(
        app.sidebar.selected_slug().as_deref(),
        Some("main/beta"),
        "the selection lands on the neighbor so the next `m` works"
    );

    assert!(!app.capture_undo(), "the move itself is not an edit");
    assert_eq!(app.history.undo_len(), steps_before + 1);

    app.update(Action::Undo);
    assert!(dir.path().join("requests/main/alpha.toml").is_file());
    assert!(!dir.path().join("requests/auth/alpha.toml").exists());
    assert_eq!(app.project.active_space, "main");
}

#[test]
fn moving_a_request_that_is_not_open_leaves_the_editor_alone() {
    let (mut app, _dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/beta".into()));
    render_once(&mut app);
    app.sidebar.select_slug("main/alpha");
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "auth".into(),
    });
    assert_eq!(app.project.active_space, "main");
    assert_eq!(app.editor.slug.as_deref(), Some("main/beta"));
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/beta"));
}

#[test]
fn moving_the_last_request_selects_the_one_above() {
    let (mut app, _dir) = spaced_app();
    render_once(&mut app);
    app.sidebar.select_slug("main/beta");
    app.update(Action::MoveRequestToSpace {
        slug: "main/beta".into(),
        space: "auth".into(),
    });
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/alpha"));
}

#[test]
fn undo_of_a_move_follows_the_file_back_and_keeps_the_outgoing_space_s_memory() {
    // `enter_space` records the outgoing space's open request. On the
    // undo-follow paths the editor has *already* been moved to the
    // incoming space's slug, so recording it would take the `_ =>` arm and
    // erase the space being left. `SpaceExit::Keep` is what stops that.
    let (mut app, _dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "auth".into(),
    });
    assert_eq!(app.project.active_space, "main");
    // Go look at it in auth, then undo from there: the file comes back to
    // main and the editor follows it, leaving auth on what it was on.
    app.update(Action::ForceOpenRequest("auth/alpha".into()));
    assert_eq!(app.project.active_space, "auth");
    app.update(Action::Undo);
    assert_eq!(app.project.active_space, "main");
    assert_eq!(app.editor.slug.as_deref(), Some("main/alpha"));
    assert_eq!(
        app.project.space_open_for("auth").as_deref(),
        Some("auth/alpha"),
        "the space being left keeps what it was left on"
    );
}

#[test]
fn renaming_an_environment_reports_a_secrets_write_failure() {
    let (mut app, dir) = spaced_app();
    postui_core::project::create_environment(dir.path(), "dev").unwrap();
    app.project.environments = postui_core::project::list_environments(dir.path());
    // Block `.local/secrets.toml` with a non-empty directory.
    std::fs::create_dir_all(dir.path().join(".local/secrets.toml/in-the-way")).unwrap();

    app.toasts = Default::default();
    app.update(Action::RenameEnv {
        from: "dev".into(),
        to: "staging".into(),
    });
    let msg = app.toasts.messages().join(" | ");
    assert!(msg.contains("could not save secrets:"), "{msg}");
    assert!(
        dir.path().join("environments/staging.toml").is_file(),
        "the rename itself still happened"
    );
}

#[test]
fn a_failed_redo_of_a_file_step_says_redo_not_undo() {
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "auth".into(),
    });
    app.update(Action::Undo);
    assert!(dir.path().join("requests/main/alpha.toml").is_file());

    // Block the redo's write with a non-empty directory in its place.
    let blocked = dir.path().join("requests/auth/alpha.toml");
    std::fs::create_dir_all(blocked.join("in-the-way")).unwrap();
    app.toasts = Default::default();
    app.update(Action::Redo);
    let msg = app.toasts.messages().join(" | ");
    assert!(msg.contains("redo failed at"), "{msg}");
}

#[test]
fn move_request_to_the_space_it_is_already_in_says_so() {
    let (mut app, _dir) = spaced_app();
    app.toasts = Default::default();
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "main".into(),
    });
    assert_eq!(app.toasts.messages(), ["already in main"]);

    app.toasts = Default::default();
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "nope".into(),
    });
    assert_eq!(app.toasts.messages(), ["no space named \"nope\""]);
}

#[test]
fn move_request_to_space_holding_a_dirty_open_request_gates_first() {
    let (mut app, dir) = spaced_app();
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    dirty_the_editor(&mut app);
    app.update(Action::MoveRequestToSpace {
        slug: "main/alpha".into(),
        space: "auth".into(),
    });
    let Some(Modal::Confirm { title, .. }) = app.modals.top() else {
        panic!("gate")
    };
    assert_eq!(title, "Unsaved changes");
    assert!(
        dir.path().join("requests/main/alpha.toml").is_file(),
        "nothing moved yet"
    );
    assert_eq!(app.project.active_space, "main");
}

#[test]
fn click_sidebar_row_opens_that_request() {
    let (mut app, _dir) = sidebar_test_app();
    render_once(&mut app);
    assert_eq!(
        app.sidebar.rows[0],
        Row::Request {
            slug: "main/top".into(),
            name: "top".into(),
            depth: 0,
            broken: None,
            method: Some(postui_core::model::Method::Get),
        }
    );
    let r = app.hits.rect_of(&crate::hit::Hit::SidebarRow(0)).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert_eq!(app.editor.slug.as_deref(), Some("main/top"));
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
    for slug in ["main/alpha", "main/beta", "main/gamma"] {
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
    app.update(Action::ForceOpenRequest("main/alpha".into()));
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

/// A right click while a context menu is already open re-targets in one
/// click: the open menu is dismissed (with the same selection revert any
/// click-away does) and the row now under the pointer gets its own menu,
/// instead of the click dying against the menu's full-screen
/// `ModalOutside` overlay.
#[test]
fn right_click_while_context_menu_open_retargets_to_the_new_row() {
    let (mut app, _dir) = sidebar_test_app_three_flat_rows();
    render_once(&mut app);
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    assert_eq!(app.sidebar.selected, Some(0));
    render_once(&mut app);

    // Open row 2's menu, then render so the hitmap carries the overlay.
    let r2 = app.hits.rect_of(&crate::hit::Hit::SidebarRow(2)).unwrap();
    app.handle_mouse(right_down(r2.x, r2.y));
    assert!(matches!(app.modals.top(), Some(Modal::Dropdown(_))));
    render_once(&mut app);

    // Right-click row 1 while row 2's menu is open: the selection moves
    // to row 1 and ITS menu is now the open one.
    let r1 = app.hits.rect_of(&crate::hit::Hit::SidebarRow(1)).unwrap();
    assert!(app.handle_mouse(right_down(r1.x, r1.y)));
    assert!(
        matches!(app.modals.top(), Some(Modal::Dropdown(_))),
        "the re-targeted row's menu must be open"
    );
    assert_eq!(app.sidebar.selected, Some(1));

    // Dismissing the re-targeted menu still restores the pre-menu
    // selection (row 0), not the intermediate right-clicked row.
    app.update(Action::Close);
    assert!(app.modals.is_empty(), "one Close empties the stack");
    assert_eq!(app.sidebar.selected, Some(0));
}

/// A right click over dead space while a context menu is open just closes
/// the menu (and reverts the pre-selection), exactly like a left click
/// away.
#[test]
fn right_click_on_dead_space_while_context_menu_open_closes_it() {
    let (mut app, _dir) = sidebar_test_app_three_flat_rows();
    render_once(&mut app);
    app.update(Action::ForceOpenRequest("main/alpha".into()));
    render_once(&mut app);

    let r2 = app.hits.rect_of(&crate::hit::Hit::SidebarRow(2)).unwrap();
    app.handle_mouse(right_down(r2.x, r2.y));
    assert!(matches!(app.modals.top(), Some(Modal::Dropdown(_))));
    render_once(&mut app);

    // The response pane's background offers no context menu while there
    // is no response text to copy.
    let dead = app
        .hits
        .rect_of(&crate::hit::Hit::Pane(crate::layout::PaneId::Response))
        .unwrap();
    assert!(app.handle_mouse(right_down(
        dead.x + dead.width / 2,
        dead.y + dead.height / 2
    )));
    assert!(app.modals.is_empty(), "the menu closes, nothing reopens");
    assert_eq!(app.sidebar.selected, Some(0), "pre-selection restored");
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
    app.update(Action::ForceOpenRequest("main/gamma".into()));
    assert_eq!(app.sidebar.selected, Some(2));
    render_once(&mut app); // let the travel anim settle at row 2

    // Delete "alpha" (row 0, above it) -- "gamma" is still the open
    // request, but its row index shifts from 2 to 1.
    app.update(Action::DeleteRequest("main/alpha".into()));
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
    app.project.expanded.insert("main/api".into());
    app.refresh_sidebar();
    let keymap = Keymap::default_bindings();
    app.update(Action::ForceOpenRequest("main/top".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('/'));
    assert!(app.editor.is_dirty());

    render_once(&mut app);
    assert_eq!(
        app.sidebar.rows[2],
        Row::Request {
            slug: "main/api/ping".into(),
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
        Some("main/top"),
        "editor content unchanged until the modal is resolved"
    );
}

#[test]
fn broken_file_shows_marker_and_error_modal() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::storage::ensure_project(dir.path()).unwrap();
    std::fs::write(
        dir.path().join("requests/main/bad.toml"),
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
    postui_core::storage::save_request(dir.path(), "main/a", &req("https://x/a")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/a".into()));
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
    assert_eq!(app.editor.slug.as_deref(), Some("main/api/ping"));
    assert!(postui_core::storage::load_request(&app.project.root, "main/api/ping").is_ok());
    assert!(
        app.sidebar
            .rows
            .iter()
            .any(|r| matches!(r, Row::Request { slug, .. } if slug == "main/api/ping")),
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
    assert_eq!(app.editor.slug.as_deref(), Some("main/my-request"));
    assert_eq!(app.editor.name.as_deref(), Some("My Request!"));
    let loaded = postui_core::storage::load_request(&app.project.root, "main/my-request").unwrap();
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
    assert_eq!(app.editor.slug.as_deref(), Some("main/my-request-2"));
}

#[test]
fn rename_flow_speaks_display_names_and_regenerates_the_slug() {
    let mut app = App::new_for_test();
    app.update(Action::CreateRequest("Get User".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("main/get-user"));

    // The prompt prefills the display name, not the slug.
    app.sidebar.select_slug("main/get-user");
    app.refresh_sidebar();
    app.sidebar.select_slug("main/get-user");
    app.update(Action::PromptRenameRequest);
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("expected the rename prompt");
    };
    assert_eq!(input.text(), "Get User");
    app.modals.pop();

    // Renaming (with a sloppy trailing space) regenerates the slug and
    // rewrites the name.
    app.update(Action::RenameRequest {
        from: "main/get-user".into(),
        to: "Get User v2 ".into(),
    });
    assert_eq!(app.editor.slug.as_deref(), Some("main/get-user-v2"));
    assert_eq!(app.editor.name.as_deref(), Some("Get User v2"));
    assert_eq!(app.sidebar.open_slug.as_deref(), Some("main/get-user-v2"));
    let loaded = postui_core::storage::load_request(&app.project.root, "main/get-user-v2").unwrap();
    assert_eq!(loaded.name.as_deref(), Some("Get User v2"));
}

#[test]
fn delete_and_duplicate_toasts_show_display_names() {
    let mut app = App::new_for_test();
    app.update(Action::CreateRequest("Fancy Name!".into()));
    app.sidebar.select_slug("main/fancy-name");
    app.refresh_sidebar();
    app.sidebar.select_slug("main/fancy-name");

    app.update(Action::DeleteSelectedRequest);
    assert!(
        app.modals.is_empty(),
        "delete is undoable, so no confirm gate"
    );
    assert!(!postui_core::storage::request_exists(
        &app.project.root,
        "main/fancy-name"
    ));
    let text = rendered_text(&mut app);
    assert!(
        text.contains("Deleted Fancy Name!"),
        "display name in the delete toast"
    );
    assert!(
        text.contains("^Z undoes"),
        "the toast advertises the escape hatch"
    );
    app.update(Action::Undo);
    assert!(
        postui_core::storage::request_exists(&app.project.root, "main/fancy-name"),
        "undo restores the deleted request"
    );

    app.refresh_sidebar();
    app.sidebar.select_slug("main/fancy-name");
    app.update(Action::ForceOpenRequest("main/fancy-name".into()));
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
    postui_core::storage::save_request(dir.path(), "main/legacy", &req("https://x/a")).unwrap();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/legacy".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    app.update(Action::SaveRequest);
    let loaded = postui_core::storage::load_request(dir.path(), "main/legacy").unwrap();
    assert_eq!(loaded.name, None, "no name field appears uninvited");
}

#[test]
fn new_request_duplicate_name_toasts_and_leaves_existing_file_alone() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(
        &app.project.root,
        "main/api/ping",
        &req("https://x/existing"),
    )
    .unwrap();
    app.update(Action::RefreshSidebar);
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewRequest);
    for c in "api/ping".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // The rejected name keeps the prompt open (typed text intact) so it
    // can be corrected instead of retyped.
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("the name prompt stays open after the rejection")
    };
    assert_eq!(input.text(), "api/ping");
    assert!(!app.toasts.is_empty(), "a duplicate name must toast");
    let existing = postui_core::storage::load_request(&app.project.root, "main/api/ping").unwrap();
    assert_eq!(
        existing.url, "https://x/existing",
        "existing file must not be overwritten"
    );
}

#[test]
fn rename_request_updates_disk_and_open_slug() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "main/old", &req("https://x/old"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/old".into()));
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Sidebar;
    app.handle_key(&keymap, plain('r'));
    match app.modals.top() {
        Some(Modal::Prompt {
            kind: PromptKind::RenameRequest { from },
            ..
        }) => {
            assert_eq!(from, "main/old");
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
    assert!(postui_core::storage::load_request(&app.project.root, "main/old").is_err());
    assert!(postui_core::storage::load_request(&app.project.root, "main/new").is_ok());
    assert_eq!(app.editor.slug.as_deref(), Some("main/new"));
    assert_eq!(app.sidebar.open_slug.as_deref(), Some("main/new"));
}

#[test]
fn delete_open_request_clears_editor_and_removes_file() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "main/gone", &req("https://x/gone"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/gone".into()));
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Sidebar;
    app.handle_key(&keymap, plain('d'));
    assert!(app.modals.is_empty(), "delete needs no confirm");
    assert!(
        app.editor.slug.is_none(),
        "editor must reset once its open request is deleted"
    );
    assert!(postui_core::storage::load_request(&app.project.root, "main/gone").is_err());
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
    assert_eq!(app.editor.slug.as_deref(), Some("main/fresh"));
    let saved = postui_core::storage::load_request(&app.project.root, "main/fresh").unwrap();
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

    // A second click within the double-click window selects the word
    // under the pointer (the sweep's own press was click #1) — here the
    // whole text, since "hello" is one word run.
    app.handle_mouse(left_up(r.x + 2 + 2, r.y + 1));
    app.handle_mouse(left_down(r.x + 2 + 2, r.y + 1));
    let input = app.modals.focused_input().expect("prompt input");
    assert_eq!(
        input.selection(),
        Some((0, 5)),
        "double click selects the word"
    );
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
        url: "https://x.test/a".into(),
        headers: vec![],
        body: "late".into(),
        ttfb: std::time::Duration::from_millis(1),
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
        url: "https://x.test/a".into(),
        headers: vec![],
        body: "ok".into(),
        ttfb: std::time::Duration::from_millis(1),
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
            url: "https://x.test/a".into(),
            headers: vec![],
            body: r#"{"a": 1}"#.into(),
            ttfb: std::time::Duration::from_millis(5),
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
    postui_core::storage::save_request(b.path(), "main/pong", &req("https://x/pong")).unwrap();
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
            .any(|r| matches!(r, Row::Request { slug, .. } if slug == "main/pong"))
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
    postui_core::storage::save_request(&app.project.root, "main/r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/r".into()));
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
    postui_core::storage::save_request(&app.project.root, "main/r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/r".into()));
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
            open_request: Some("main/pong".into()),
            ..Default::default()
        },
    )
    .unwrap();
    app.update(Action::SwitchProject(b.path().to_path_buf()));
    assert_eq!(app.editor.slug.as_deref(), Some("main/pong"));
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
    postui_core::storage::save_request(&app.project.root, "main/r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/r".into()));
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
    postui_core::storage::save_request(&app.project.root, "main/a", &req("https://x/a")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/a".into()));
    let st = postui_core::project::load_local_state(&app.project.root).unwrap();
    assert_eq!(st.open_request.as_deref(), Some("main/a"));
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
fn set_env_tls_writes_project_toml_and_toasts() {
    use postui_core::project::TlsPolicy;
    let (mut app, dir) = app_with_envs();
    app.update(Action::SetEnvTls {
        env: "prod".into(),
        policy: Some(TlsPolicy::Verify),
    });
    let meta = postui_core::project::load_meta(dir.path()).unwrap();
    assert_eq!(
        postui_core::project::env_tls(&meta, "prod"),
        Some(TlsPolicy::Verify)
    );
    assert_eq!(
        postui_core::project::env_tls(&app.project.meta, "prod"),
        Some(TlsPolicy::Verify),
        "meta reloaded in memory"
    );
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("prod forces TLS verification")
    );
    app.update(Action::SetEnvTls {
        env: "prod".into(),
        policy: Some(TlsPolicy::Insecure),
    });
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("prod skips TLS verification")
    );
    app.update(Action::SetEnvTls {
        env: "prod".into(),
        policy: None,
    });
    assert_eq!(
        postui_core::project::env_tls(&app.project.meta, "prod"),
        None
    );
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("prod leaves TLS verification to each request")
    );
}

#[test]
fn toggle_insecure_under_an_environment_force_saves_but_warns() {
    use postui_core::project::TlsPolicy;
    let (mut app, dir) = app_with_envs();
    // The toast names the environment the way the header does: by its
    // display name, not its slug.
    std::fs::write(
        dir.path().join("project.toml"),
        "[environment.prod]\nname = \"Prod\"\n",
    )
    .unwrap();
    app.project.reload_meta();
    app.update(Action::SetEnvTls {
        env: "prod".into(),
        policy: Some(TlsPolicy::Verify),
    });
    app.update(Action::SwitchEnv(Some("prod".into())));
    app.update(Action::ToggleInsecure);
    assert!(app.editor.insecure, "the request's own flag still flips");
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("Saved, but Prod forces TLS verification")
    );
    app.update(Action::SetEnvTls {
        env: "prod".into(),
        policy: Some(TlsPolicy::Insecure),
    });
    app.update(Action::ToggleInsecure);
    assert!(!app.editor.insecure);
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("Saved, but Prod skips TLS verification")
    );
    // No force: the plain toasts.
    app.update(Action::SetEnvTls {
        env: "prod".into(),
        policy: None,
    });
    app.update(Action::ToggleInsecure);
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("TLS verification disabled for this request")
    );
}

#[test]
fn manage_environments_pane_has_a_tls_control_that_writes_the_force() {
    use crate::components::manage::ManageTab;
    use postui_core::project::TlsPolicy;
    let (mut app, _dir) = app_with_envs();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Environments),
    });
    app.manage
        .list
        .select_name(ManageTab::Environments, &app.project, "prod");
    let text = rendered_text_tall(&mut app);
    assert!(text.contains("TLS"), "{text}");
    assert!(text.contains("Per request"), "{text}");
    assert!(text.contains("Verify"), "{text}");
    assert!(text.contains("Insecure"), "{text}");

    click_hit(&mut app, Hit::ManageEnvTls(Some(TlsPolicy::Verify)));
    assert_eq!(
        postui_core::project::env_tls(&app.project.meta, "prod"),
        Some(TlsPolicy::Verify)
    );
    click_hit(&mut app, Hit::ManageEnvTls(None));
    assert_eq!(
        postui_core::project::env_tls(&app.project.meta, "prod"),
        None
    );

    // `t` cycles per request → verify → insecure → per request.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('t'));
    assert_eq!(
        postui_core::project::env_tls(&app.project.meta, "prod"),
        Some(TlsPolicy::Verify)
    );
    app.handle_key(&keymap, plain('t'));
    assert_eq!(
        postui_core::project::env_tls(&app.project.meta, "prod"),
        Some(TlsPolicy::Insecure)
    );
    app.handle_key(&keymap, plain('t'));
    assert_eq!(
        postui_core::project::env_tls(&app.project.meta, "prod"),
        None
    );
    // Advertised in the footer.
    assert!(
        app.manage
            .list
            .footer_chips(ManageTab::Environments, &app.project)
            .iter()
            .any(|(k, l, _)| *k == "t" && *l == "tls"),
    );
}

#[test]
fn padlock_shows_the_effective_tls_state_under_an_environment_force() {
    use postui_core::project::TlsPolicy;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let (mut app, _dir) = app_with_envs();
    app.update(Action::SetEnvTls {
        env: "prod".into(),
        policy: Some(TlsPolicy::Insecure),
    });
    let lock_glyph = |app: &mut App| -> (String, ratatui::style::Color) {
        app.anims.finish_all();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let r = app
            .hits
            .rect_of(&Hit::FooterChip(Action::ToggleInsecure))
            .expect("padlock registered");
        let cell = &terminal.backend().buffer()[(r.x + 1, r.y)];
        (cell.symbol().to_string(), cell.fg)
    };
    // No env, request verifying: the filled lock.
    let (plain_glyph, plain_fg) = lock_glyph(&mut app);
    assert_eq!(plain_glyph, "\u{F033E}");
    // prod forces insecure: the outline lock even though the request is
    // unflagged, and muted further to say the request isn't in charge.
    app.update(Action::SwitchEnv(Some("prod".into())));
    let (forced_glyph, forced_fg) = lock_glyph(&mut app);
    assert_eq!(forced_glyph, "\u{F0340}");
    assert_ne!(forced_fg, plain_fg, "forced lock paints differently");
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
fn rename_env_moves_the_file_rekeys_secrets_and_follows_the_active_env() {
    let (mut app, dir) = app_with_envs();
    // A real selector (declared in variables.toml, options in the env
    // file) rather than a bare made-up key: `reload_if_changed` prunes
    // selections the model can't account for, so only a genuine one
    // survives the reload an undo triggers.
    std::fs::write(
        dir.path().join("variables.toml"),
        "[selectors.user]\nfields = [\"user\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "tok = \"q\"\n\n[options.user.alice]\nuser = \"1001\"\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.project.set_secret("tok", "s3cret".into()).unwrap();
    app.project.set_selection_for("qa", "user", "alice");
    app.update(Action::PersistLocalState);
    app.update(Action::RenameEnv {
        from: "qa".into(),
        to: "staging".into(),
    });
    assert!(dir.path().join("environments/staging.toml").is_file());
    assert!(!dir.path().join("environments/qa.toml").exists());
    assert_eq!(app.project.env_label(), "staging");
    let secrets = postui_core::project::load_secrets(dir.path()).unwrap();
    assert_eq!(secrets["staging"]["tok"], "s3cret");
    assert!(!secrets.contains_key("qa"));
    let st = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(st.environment.as_deref(), Some("staging"));
    assert_eq!(app.project.selections_for("staging")["user"], "alice");
    assert!(app.project.selections_for("qa").is_empty());
    assert_eq!(st.selections["staging"]["user"], "alice");
    assert!(!st.selections.contains_key("qa"));
    app.update(Action::Undo);
    assert!(dir.path().join("environments/qa.toml").is_file());
    assert_eq!(app.project.env_label(), "qa");
    assert_eq!(
        app.project.secrets["qa"]["tok"], "s3cret",
        "secrets re-keyed back"
    );
    assert_eq!(
        app.project.selections_for("qa")["user"],
        "alice",
        "selections re-keyed back"
    );
    assert!(app.project.selections_for("staging").is_empty());
    let st = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(st.selections["qa"]["user"], "alice");
    assert!(
        !st.selections.contains_key("staging"),
        "no phantom key left on disk"
    );
}

#[test]
fn delete_env_confirms_trashes_clears_the_active_env_and_undoes() {
    let (mut app, dir) = app_with_envs();
    // A real selector (declared in variables.toml, options in the env
    // file) rather than a bare made-up key: `reload_if_changed` prunes
    // selections the model can't account for, so only a genuine one
    // survives the reload an undo triggers.
    std::fs::write(
        dir.path().join("variables.toml"),
        "[selectors.user]\nfields = [\"user\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "tok = \"q\"\n\n[options.user.alice]\nuser = \"1001\"\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.project.set_secret("tok", "s3cret".into()).unwrap();
    app.project.set_selection_for("qa", "user", "alice");
    app.update(Action::PersistLocalState);
    app.update(Action::DeleteEnv("qa".into()));
    let Some(Modal::Confirm {
        title,
        body,
        choices,
    }) = app.modals.top()
    else {
        panic!("confirm")
    };
    assert_eq!(title, "Delete environment \"qa\"?");
    assert_eq!(body, "Its values and secrets are removed.");
    assert_eq!(choices[0].1, "Delete environment");
    let confirm = choices[0].0;
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain(confirm));
    assert!(!dir.path().join("environments/qa.toml").exists());
    assert_eq!(app.project.env_label(), "no env");
    assert!(
        !postui_core::project::load_secrets(dir.path())
            .unwrap()
            .contains_key("qa")
    );
    assert!(!app.project.environments.contains(&"qa".to_string()));
    assert!(app.project.selections_for("qa").is_empty());
    assert!(
        !postui_core::project::load_local_state(dir.path())
            .unwrap()
            .selections
            .contains_key("qa")
    );

    app.update(Action::Undo);
    assert!(dir.path().join("environments/qa.toml").is_file());
    assert_eq!(app.project.env_label(), "qa", "active env restored");
    assert_eq!(
        postui_core::project::load_secrets(dir.path()).unwrap()["qa"]["tok"],
        "s3cret"
    );
    assert_eq!(
        app.project.secrets["qa"]["tok"], "s3cret",
        "the restored secrets file is re-read into memory, not just to disk"
    );
    assert_eq!(
        app.project.selections_for("qa")["user"],
        "alice",
        "the restored selections come back into memory too"
    );
    assert_eq!(
        postui_core::project::load_local_state(dir.path())
            .unwrap()
            .selections["qa"]["user"],
        "alice"
    );
}

#[test]
fn env_chooser_includes_no_environment_entry() {
    let (mut app, _dir) = app_with_envs();
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.update(Action::OpenEnvChooser);
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("expected dropdown")
    };
    assert!(
        state.items.iter().any(|it| it.label == "no environment"),
        "the dropdown offers a way back to no-env"
    );
    app.update(Action::Close);
    app.update(Action::SwitchEnv(None));
    assert_eq!(app.project.env_label(), "no env");
}

#[test]
fn env_chooser_opens_on_the_active_environment() {
    let (mut app, _dir) = app_with_envs();
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.update(Action::OpenEnvChooser);
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("expected dropdown")
    };
    assert_eq!(
        state.items[state.selected].label, "qa",
        "the dropdown opens on the active env, not row 0"
    );
    assert_eq!(
        state.current,
        Some(state.selected),
        "the ✓ marker sits on the active env"
    );
    app.update(Action::Close);

    // With no env active it opens on the "no environment" row.
    app.update(Action::SwitchEnv(None));
    app.update(Action::OpenEnvChooser);
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("expected dropdown")
    };
    assert_eq!(state.items[state.selected].label, "no environment");
    assert_eq!(state.current, Some(state.selected));
}

#[test]
fn env_chooser_new_environment_row_opens_prompt() {
    let (mut app, _dir) = app_with_envs();
    let keymap = Keymap::default_bindings();
    app.update(Action::OpenEnvChooser);
    let rows = match app.modals.top() {
        Some(Modal::Dropdown(state)) => state.items.len(),
        _ => panic!("expected dropdown"),
    };
    // "new environment…" is the second-to-last row (Task 10 appended
    // "manage environments…" after it); Down past the end stays put, so
    // over-stepping then stepping back once lands on it regardless of
    // where the cursor opened.
    for _ in 0..rows {
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
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

    app.update(Action::CreateEnv("   ".into()));
    assert!(
        app.toasts.messages().len() > toasts_before,
        "invalid name must toast"
    );
    assert_eq!(app.project.env_label(), "qa");

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
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("empty project opens the dropdown (no-env + create rows), not a toast")
    };
    let labels: Vec<&str> = state.items.iter().map(|it| it.label.as_str()).collect();
    assert!(
        labels.contains(&"new environment…"),
        "the create row is there even with no environments: {labels:?}"
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

#[test]
fn toggle_insecure_flips_the_editor_flag() {
    let mut app = App::new_for_test();
    assert!(!app.editor.insecure);
    app.update(Action::ToggleInsecure);
    assert!(app.editor.insecure);
    app.update(Action::ToggleInsecure);
    assert!(!app.editor.insecure);
}

#[test]
fn toggle_insecure_toasts_the_new_state() {
    let mut app = App::new_for_test();
    app.update(Action::ToggleInsecure);
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("TLS verification disabled for this request")
    );
    app.update(Action::ToggleInsecure);
    assert_eq!(
        app.toasts.messages().last().copied(),
        Some("TLS verification enabled")
    );
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

[selectors.identity]
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
    postui_core::storage::save_request(dir.path(), "main/r", &req).unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.anims.enabled = false;
    app.update(Action::ForceOpenRequest("main/r".into()));
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

    // The refused name keeps the prompt open (typed text intact) so it
    // can be fixed rather than retyped.
    let Some(Modal::Prompt { input, .. }) = app.modals.top() else {
        panic!("the prompt stays open after the refusal")
    };
    assert_eq!(input.text(), "options");
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
            url: "https://x.test/a".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
            ttfb: std::time::Duration::from_millis(1),
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
    app.screen = Screen::Manage;
    let mut input = crate::components::line_input::LineInput::new("token value");
    input.select_all();
    app.varmanager.form.editing = Some((VmField::Default, input));

    app.handle_key(&Keymap::default_bindings(), ctrl('c'));

    assert!(!app.should_quit);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "token value");
    assert!(app.varmanager.form.editing.is_some(), "the edit stays live");
}

#[test]
fn url_bar_drag_selects_and_double_click_selects_the_word() {
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://example.com");
    render_once(&mut app);
    let area = app.editor.last_url_text_area.expect("url area recorded");

    // Click at the start, sweep 5 cells right: "https" selected.
    app.handle_mouse(left_down(area.x, area.y));
    assert!(app.handle_mouse(dragged(area.x + 5, area.y)));
    app.handle_mouse(left_up(area.x + 5, area.y));
    assert_eq!(app.editor.url.selected_text().as_deref(), Some("https"));

    // A double click selects the word under the pointer — the scheme run,
    // not the whole URL (punctuation like `://` bounds it). (Reset the
    // click pairing so the sweep's Down above can't count as this pair's
    // first click.)
    app.last_click = None;
    app.handle_mouse(left_down(area.x + 2, area.y));
    app.handle_mouse(left_down(area.x + 2, area.y)); // within 400ms => clicks == 2
    assert_eq!(app.editor.url.selected_text().as_deref(), Some("https"));

    // Dragging on from the double click extends word by word: onto
    // "example" the selection grows to the run between the two words.
    assert!(app.handle_mouse(dragged(area.x + 10, area.y)));
    assert_eq!(
        app.editor.url.selected_text().as_deref(),
        Some("https://example")
    );
    app.handle_mouse(left_up(area.x + 10, area.y));

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
    // out entirely, leaving only the split control on the row.
    assert!(
        !content.contains("Params · 3"),
        "tab labels are invisible while hidden: {content}"
    );
    assert!(
        content.contains('\u{2586}'),
        "the split control's chips stay: {content}"
    );
}

#[test]
fn collapse_toggle_click_and_key() {
    let mut app = App::new_for_test();
    three_params(&mut app);
    render_once(&mut app);
    assert!(!app.table_collapsed);

    let r = app
        .hits
        .rect_of(&Hit::SplitStop(crate::split::SplitStop::ResponseFull))
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    assert!(
        app.table_collapsed,
        "clicking the response-full chip collapses the editor"
    );

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

/// The response pane's collapse is a layout preference, not per-request
/// state: switching to another request keeps the pane hidden instead of
/// swapping the flag with the response.
#[test]
fn response_collapse_sticks_when_switching_requests() {
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

    // Opening a different request swaps in that request's own response;
    // the collapse rides along.
    app.editor.slug = Some("other".into());
    app.update(Action::Render);
    assert!(app.session.response.collapsed);
    render_once(&mut app);
    let still_hidden = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    assert_eq!(
        still_hidden.height,
        crate::components::response::COLLAPSED_HEIGHT,
        "the pane stays collapsed on the next request: {still_hidden:?}"
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

/// With collapse riding across request switches, the no-blank-screen rule
/// needs only the toggles: a switch never changes either flag, so the two
/// panes can't both arrive hidden.
#[test]
fn no_blank_screen_across_request_switches() {
    let mut app = App::new_for_test_with_anims(false);
    // Hide request A's response, then switch away — the collapse follows.
    app.editor.slug = Some("a".into());
    app.update(Action::Render);
    app.session
        .response
        .set_state(crate::components::response::ResponseState::Cancelled, 0);
    app.update(Action::ToggleResponseCollapse);
    app.editor.slug = Some("b".into());
    app.update(Action::Render);
    assert!(app.session.response.collapsed, "collapse sticks on B");

    // Hiding the editor on B swaps the panels (toggle-time rule), and a
    // switch back to A changes neither flag: no blank screen.
    app.update(Action::ToggleTableCollapse);
    assert!(!app.session.response.collapsed, "panels swapped");
    app.editor.slug = Some("a".into());
    app.update(Action::Render);
    assert!(app.table_collapsed, "the editor stays hidden");
    assert!(
        !app.session.response.collapsed,
        "the response stays expanded"
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

/// Successor to a review-finding regression test: when the env chooser was
/// a centered modal, its push site once bypassed `push_modal` and skipped
/// the `AnimKey::ModalOpen` retarget. Now that it opens as an anchored
/// `Modal::Dropdown` (which never touches `ModalOpen`), the equivalent
/// guarantee is that the open runs `begin_dropdown_open` — a push site
/// that skips it would pop the menu in with no settle.
#[test]
fn env_chooser_open_starts_the_dropdown_settle() {
    let (mut app, _dir) = app_with_envs();
    let now = std::time::Instant::now();

    app.update(Action::OpenEnvChooser);
    assert!(
        matches!(app.modals.top(), Some(Modal::Dropdown(_))),
        "sanity: the dropdown actually opened"
    );
    assert!(
        app.anims.value(AnimKey::DropdownOpen, now).unwrap() < 1.0,
        "opening the env dropdown must start the open settle short of 1"
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
fn click_confirm_choice_chip_fires_its_action() {
    let mut app = dirty_app();
    app.anims.enabled = false;
    app.update(Action::Quit);
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));

    render_once(&mut app);
    let chip = app.hits.rect_of(&Hit::ConfirmChoice('d')).unwrap();
    assert!(app.handle_mouse(left_down(chip.x, chip.y)));
    assert!(app.modals.is_empty());
    assert!(
        app.should_quit,
        "clicking the [d] chip must discard changes and quit"
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
        postui_core::storage::load_request(&app.project.root, "main/api/ping").is_ok(),
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
            ..Default::default()
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

/// The ❐ toolbar button follows the active tab like search does: on the
/// Headers tab it copies the header list, not the body.
#[test]
fn toolbar_copy_follows_the_headers_tab() {
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
    app.update(Action::ResponseViewMode(
        crate::components::response::ViewMode::Headers,
    ));

    click_hit(&mut app, Hit::CopyBodyButton);

    let copied = std::fs::read_to_string(&out).unwrap();
    assert!(
        copied.contains("content-type:") && copied.contains("application/json"),
        "the Headers tab copies the header list: {copied:?}"
    );
    assert!(!copied.contains("\"a\""), "not the body: {copied:?}");
    assert!(
        rendered_text(&mut app).contains("Copied response headers"),
        "toast names what was copied"
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

/// The ✎ toolbar button parks `OpenResponseInEditor` for the main loop
/// (which must suspend the terminal), exactly as the request body's
/// `$EDITOR` action does.
#[test]
fn toolbar_editor_button_parks_the_terminal_action() {
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": 1}"#);

    click_hit(&mut app, Hit::ResponseEditorButton);

    assert_eq!(
        app.pending_terminal_action,
        Some(Action::OpenResponseInEditor),
        "the click parks the terminal action for the main loop"
    );
}

/// The 💾 toolbar button follows the active tab like search does: on the
/// Headers tab it prompts with a `.txt` prefill and writes the header
/// list, not the body.
#[test]
fn toolbar_save_follows_the_headers_tab() {
    let mut app = App::new_for_test();
    ready_response(&mut app, r#"{"a": 1}"#);
    app.update(Action::ResponseViewMode(
        crate::components::response::ViewMode::Headers,
    ));

    click_hit(&mut app, Hit::SaveBodyButton);

    let Some(Modal::Prompt {
        kind: PromptKind::SaveViewAs,
        input,
        ..
    }) = app.modals.top()
    else {
        panic!("expected a SaveViewAs prompt");
    };
    assert!(
        input.text().ends_with("-response.txt"),
        "headers prefill is .txt: {}",
        input.text()
    );

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("view.txt");
    app.update(Action::SaveViewToFile(out.to_string_lossy().to_string()));

    let saved = std::fs::read_to_string(&out).unwrap();
    assert!(
        saved.contains("content-type:") && !saved.contains("\"a\""),
        "the Headers tab saves the header list: {saved:?}"
    );
    assert!(rendered_text(&mut app).contains("Saved"));
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
    assert_eq!(app.screen, crate::app::Screen::Manage);
    let content = rendered_text(&mut app);
    assert!(content.contains("VARIABLES"), "the left list's own heading");
    assert!(
        content.contains("no env \u{25be}"),
        "the header's env chip is the manager screen's env control: {content}"
    );
}

#[test]
fn palette_manage_command_opens_the_manage_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::OpenPalette);
    for c in "Manage: variables".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.screen, crate::app::Screen::Manage);
    assert_eq!(
        app.manage.tab,
        crate::components::manage::ManageTab::Variables
    );
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
    assert_eq!(app.screen, crate::app::Screen::Manage);

    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.screen, crate::app::Screen::Main);
    assert_eq!(app.focus, PaneId::Response, "prior focus is restored");
}

#[test]
fn modals_still_open_and_close_on_top_of_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::Manage);

    // ctrl+p still opens the palette on top of the Manager screen.
    app.handle_key(&keymap, ctrl('p'));
    assert!(!app.modals.is_empty());
    assert_eq!(
        app.screen,
        crate::app::Screen::Manage,
        "opening a modal must not leave the screen"
    );

    // Esc closes the modal first, without leaving the Manager screen.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert_eq!(
        app.screen,
        crate::app::Screen::Manage,
        "closing the modal must not also leave the screen"
    );
}

#[test]
fn plain_q_types_into_a_live_grid_edit_instead_of_quitting() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    goto_group(&mut app, "user");
    app.varmanager.start_cell_edit(&app.project, 0, 1);

    app.handle_key(&keymap, plain('q'));
    assert!(!app.should_quit, "a live edit owns the keyboard");
    let edit = app.varmanager.grid.editing.as_ref().unwrap();
    assert!(edit.input.text().ends_with('q'), "{:?}", edit.input.text());
    // ...and the quit chip's keycap goes back to the honest ^C while the
    // edit is live.
    let content = rendered_text_tall(&mut app);
    assert!(content.contains("^C  quit"), "{content}");
}

#[test]
fn manager_screen_replaces_the_three_panes_but_keeps_header_and_footer() {
    let mut app = App::new_for_test();
    app.update(Action::OpenManage { tab: None });
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
    assert_eq!(app.screen, crate::app::Screen::Manage);
    assert!(app.toasts.is_empty());

    app.handle_key(&keymap, ctrl('r'));
    assert!(
        app.toasts.is_empty(),
        "ctrl+r must not reach Action::Send (an empty-URL send would toast)"
    );
    assert!(app.session.in_flight.is_empty());
    assert_eq!(app.screen, crate::app::Screen::Manage);

    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
    );
    assert!(
        app.toasts.is_empty(),
        "ctrl+enter must not reach Action::Send either"
    );
    assert!(app.session.in_flight.is_empty());
    assert_eq!(app.screen, crate::app::Screen::Manage);
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
    assert_eq!(app.screen, crate::app::Screen::Manage);

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

/// Same finding: alt+o (`Action::CycleProject`) — an arbitrary other
/// global shortcut — must not reach its action from the Manager screen
/// either. (alt+c/`Action::CycleEnv` is deliberately whitelisted: the
/// active environment is meaningful inside the Manager — see
/// `alt_c_cycles_env_from_the_manager_screen`.)
#[test]
fn other_unwhitelisted_global_shortcuts_are_swallowed_by_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::Manage);
    assert!(app.toasts.is_empty());

    app.handle_key(&keymap, alt('o')); // CycleProject: would toast "only one project registered"
    assert!(
        app.toasts.is_empty(),
        "alt+o must not reach Action::CycleProject"
    );
    assert_eq!(app.screen, crate::app::Screen::Manage);
}

/// Unlike send/focus/cycle-project, the active environment IS meaningful
/// inside the Variable Manager (it shows per-env values), so alt+c
/// escapes the screen's input capture, switches the env, and re-syncs
/// the Manager without leaving it.
#[test]
fn alt_c_cycles_env_from_the_manager_screen() {
    let (mut app, _dir) = app_with_envs();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::Manage);
    assert_eq!(app.project.env_label(), "no env");

    app.handle_key(&keymap, alt('c'));
    assert_eq!(
        app.project.env_label(),
        "prod",
        "alt+c must reach Action::CycleEnv from the Manager screen"
    );
    assert_eq!(
        app.screen,
        crate::app::Screen::Manage,
        "cycling the env must not leave the screen"
    );
}

/// The whitelist's whole point: opening the palette on top of the Manager
/// screen must keep working via the same ctrl+p combo used on `Main`.
#[test]
fn ctrl_p_still_opens_the_palette_on_top_of_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::Manage);

    app.handle_key(&keymap, ctrl('p'));
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    assert_eq!(
        app.screen,
        crate::app::Screen::Manage,
        "opening the palette must not leave the screen"
    );
}

/// alt+t opens the theme chooser — from Main, and (whitelisted under the
/// same modal-on-top rule as the palette) from the Manager screen too.
/// (It moved off alt+b, which is reserved for word-left — ESC b.)
#[test]
fn alt_t_opens_the_theme_chooser_on_main_and_the_manager_screen() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('t'));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
        "alt+t opens the theme chooser"
    );
    app.update(Action::Close);

    app.handle_key(&keymap, alt('v'));
    assert_eq!(app.screen, crate::app::Screen::Manage);
    app.handle_key(&keymap, alt('t'));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
        "alt+t escapes the manager screen's input capture"
    );
    assert_eq!(app.screen, crate::app::Screen::Manage);
}

/// alt+t is a toggle: pressed again over the open theme picker it closes
/// it (reverting any preview, same as esc), rather than being swallowed.
/// Over any other chooser (e.g. projects) it keeps its hands off.
#[test]
fn alt_t_closes_the_open_theme_chooser() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('t'));
    assert!(matches!(app.modals.top(), Some(Modal::Chooser(_))));
    app.handle_key(&keymap, alt('t'));
    assert!(
        app.modals.is_empty(),
        "a second alt+t closes the theme chooser"
    );

    app.update(Action::OpenProjectChooser);
    assert!(matches!(app.modals.top(), Some(Modal::Chooser(_))));
    app.handle_key(&keymap, alt('t'));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(_))),
        "alt+t must not close a non-theme chooser"
    );
}

/// ToggleInsecure moved off alt+t to make room for the theme chooser.
#[test]
fn alt_i_toggles_insecure() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    assert!(!app.editor.insecure);
    app.handle_key(&keymap, alt('i'));
    assert!(app.editor.insecure, "alt+i toggles TLS verification off");
    app.handle_key(&keymap, alt('i'));
    assert!(!app.editor.insecure);
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

[selectors.user]
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
        "base_url = \"https://qa.example.com\"\n\n[options.user.alice]\nuser = \"1001\"\n\n[options.user.bob]\nuser = \"2002\"\n",
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

    app.update(Action::VarEdit(VarEditOp::SetOptionValue {
        env: "qa".into(),
        selector: "user".into(),
        option: "alice".into(),
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
    postui_core::storage::save_request(dir.path(), "main/ping", &req).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
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
    let on_disk = std::fs::read_to_string(dir.path().join("requests/main/ping.toml")).unwrap();
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

    app.update(Action::VarEdit(VarEditOp::SelectOption {
        env: "dev".into(),
        selector: "user".into(),
        option: "bob".into(),
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
// promote; §3 secret-flag transitions) ---------------------------------

/// Opens the Manager and selects whichever left-list row matches `pred`,
/// panicking if none does — `rendered_text` first so `left_rows` is
/// populated (the list only rebuilds inside `draw`).
fn goto_row(app: &mut App, pred: impl Fn(&crate::components::varmanager::VmRow) -> bool) {
    // OpenManage is a toggle now — only open when not already there.
    if app.screen != crate::app::Screen::Manage {
        app.update(Action::OpenManage { tab: None });
    }
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
fn a_new_declaration_selects_its_row_in_the_manager() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::NewVar {
        name: "zeta".into(),
        description: None,
    }));
    assert_eq!(app.varmanager.detail, VmDetail::Var("zeta".into()));
    assert_eq!(
        app.varmanager.left_rows[app.varmanager.left_cursor],
        crate::components::varmanager::VmRow::Var("zeta".into())
    );

    app.update(Action::VarStruct(VarStructOp::NewSelector {
        name: "creds".into(),
        fields: vec!["user_id".into()],
        shared: false,
    }));
    assert_eq!(app.varmanager.detail, VmDetail::Group("creds".into()));
    assert_eq!(
        app.varmanager.left_rows[app.varmanager.left_cursor],
        crate::components::varmanager::VmRow::Group("creds".into())
    );
}

/// A project whose `locale` selector is shared: options in variables.toml,
/// two envs (qa active) with empty files.
fn shared_locale_project(dir: &std::path::Path) {
    postui_core::project::init_project(dir, Some("demo")).unwrap();
    std::fs::write(
        dir.join("variables.toml"),
        "[selectors.locale]\nshared = true\nfields = [\"lang\"]\n\n[options.locale.en]\nlang = \"en\"\n\n[options.locale.fr]\nlang = \"fr\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("environments/dev.toml"), "").unwrap();
    std::fs::write(dir.join("environments/qa.toml"), "").unwrap();
    postui_core::project::save_local_state(
        dir,
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            ..Default::default()
        },
    )
    .unwrap();
}

fn shared_locale_app(dir: &std::path::Path) -> App {
    shared_locale_project(dir);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    App::with_root(tx, dir.to_path_buf())
}

#[test]
fn shared_selector_new_option_writes_variables_toml_not_the_env() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());

    let mut values = indexmap::IndexMap::new();
    values.insert("lang".to_string(), "de".to_string());
    app.update(Action::VarStruct(VarStructOp::NewOption {
        env: "qa".into(),
        selector: "locale".into(),
        name: "de".into(),
        description: None,
        values,
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(
        app.project.model.options["locale"]["de"].values["lang"],
        "de"
    );
    let vars = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(vars.contains("[options.locale.de]"), "{vars}");
    let qa = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!qa.contains("locale"), "{qa}");
}

#[test]
fn shared_selector_rename_option_carries_the_global_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());
    app.project.set_selection_for("qa", "locale", "fr");

    app.update(Action::VarStruct(VarStructOp::RenameOption {
        env: "qa".into(),
        selector: "locale".into(),
        from: "fr".into(),
        to: "fr-FR".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(app.project.model.options["locale"].contains_key("fr-FR"));
    assert_eq!(app.project.shared_selections()["locale"], "fr-FR");
}

#[test]
fn shared_selector_delete_option_clears_the_global_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());
    app.project.set_selection_for("qa", "locale", "fr");

    app.update(Action::VarStruct(VarStructOp::DeleteOption {
        env: "qa".into(),
        selector: "locale".into(),
        name: "fr".into(),
    }));

    assert!(!app.project.model.options["locale"].contains_key("fr"));
    assert!(!app.project.shared_selections().contains_key("locale"));
}

#[test]
fn shared_selector_duplicate_option_lands_in_variables_toml() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());

    app.update(Action::VarStruct(VarStructOp::DuplicateOption {
        env: "qa".into(),
        selector: "locale".into(),
        name: "en".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(
        app.project.model.options["locale"]["en copy"].values["lang"],
        "en"
    );
}

#[test]
fn shared_selector_option_edits_route_to_variables_toml() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());

    app.update(Action::VarEdit(VarEditOp::SetOptionValue {
        env: "qa".into(),
        selector: "locale".into(),
        option: "en".into(),
        field: "lang".into(),
        value: "en-GB".into(),
    }));
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(
        app.project.model.options["locale"]["en"].values["lang"],
        "en-GB"
    );

    app.update(Action::VarEdit(VarEditOp::SetOptionDescription {
        env: "qa".into(),
        selector: "locale".into(),
        option: "en".into(),
        description: Some("the King's".into()),
    }));
    assert_eq!(
        app.project.model.options["locale"]["en"]
            .description
            .as_deref(),
        Some("the King's")
    );
}

#[test]
fn shared_selector_inline_new_option_needs_no_active_env_and_selects_globally() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());
    app.update(Action::SwitchEnv(None));

    app.update(Action::ConfirmNewOptionInline {
        owner: "locale".into(),
        key: "de".into(),
        values: indexmap::IndexMap::from([("lang".to_string(), "de".to_string())]),
        description: None,
    });

    assert!(
        app.project.model.options["locale"].contains_key("de"),
        "{:?}",
        app.toasts.messages()
    );
    assert_eq!(app.project.shared_selections()["locale"], "de");
    assert_eq!(app.project.resolved.values["lang"], "de");
}

#[test]
fn deleting_a_shared_selector_removes_its_options_and_global_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());
    app.project.set_selection_for("qa", "locale", "fr");

    app.update(Action::VarStruct(VarStructOp::Delete {
        name: "locale".into(),
    }));

    assert!(!app.project.model.selectors.contains_key("locale"));
    let vars = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(!vars.contains("options.locale"), "{vars}");
    assert!(!app.project.shared_selections().contains_key("locale"));
}

#[test]
fn renaming_a_shared_selector_carries_options_and_the_global_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());
    app.project.set_selection_for("qa", "locale", "fr");

    app.update(Action::VarStruct(VarStructOp::Rename {
        from: "locale".into(),
        to: "lingo".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(app.project.model.selectors["lingo"].shared);
    assert_eq!(
        app.project.model.options["lingo"]["fr"].values["lang"],
        "fr"
    );
    assert_eq!(app.project.shared_selections()["lingo"], "fr");
    assert!(!app.project.shared_selections().contains_key("locale"));
}

#[test]
fn apply_group_fields_reshapes_a_shared_selectors_options_in_variables_toml() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());

    // Rename `lang` to `tongue` and add `fmt`: every option in
    // variables.toml must carry both changes in the same write.
    app.update(Action::ApplyGroupFields {
        selector: "locale".into(),
        slots: vec!["tongue".into(), "fmt".into()],
    });

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(
        app.project.model.selectors["locale"].fields,
        vec!["tongue".to_string(), "fmt".to_string()]
    );
    let en = &app.project.model.options["locale"]["en"];
    assert_eq!(en.values["tongue"], "en");
    assert_eq!(en.values["fmt"], "");
}

#[test]
fn add_and_remove_selector_field_reshape_a_shared_selectors_options() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());

    app.update(Action::AddSelectorField {
        selector: "locale".into(),
        field: "fmt".into(),
    });
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(app.project.model.options["locale"]["en"].values["fmt"], "");

    app.update(Action::RemoveSelectorField {
        selector: "locale".into(),
        field: "fmt".into(),
    });
    assert_eq!(
        app.project.model.selectors["locale"].fields,
        vec!["lang".to_string()]
    );
    assert!(
        !app.project.model.options["locale"]["en"]
            .values
            .contains_key("fmt")
    );
}

#[test]
fn shared_selector_grid_edit_and_ghost_row_work_without_an_env() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = shared_locale_app(dir.path());
    app.update(Action::SwitchEnv(None));
    app.varmanager.sync(&app.project);
    app.varmanager.select_name("locale");

    // The ghost row starts a new option with no environment active…
    app.update(Action::StartNewOptionEdit);
    let edit = app
        .varmanager
        .grid
        .editing
        .as_mut()
        .expect("ghost edit started without an env");
    assert_eq!((edit.row, edit.col), (2, 0));
    edit.input.insert_str("de");
    app.commit_grid_edit();
    assert!(
        app.project.model.options["locale"].contains_key("de"),
        "{:?}",
        app.toasts.messages()
    );

    // …and a field-cell edit commits to variables.toml the same way.
    app.varmanager.start_cell_edit(&app.project, 0, 1);
    let edit = app.varmanager.grid.editing.as_mut().unwrap();
    edit.input.select_all();
    edit.input.paste("en-GB");
    app.commit_grid_edit();
    assert_eq!(
        app.project.model.options["locale"]["en"].values["lang"],
        "en-GB"
    );
}

#[test]
fn new_selector_op_with_shared_writes_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::NewSelector {
        name: "locale".into(),
        fields: vec!["locale".into()],
        shared: true,
    }));

    assert!(app.project.model.selectors["locale"].shared);
    let vars = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(vars.contains("shared = true"), "{vars}");
}

#[test]
fn new_selector_prompt_arrows_focus_the_toggle_and_space_flips_shared() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    app.update(Action::PromptNewSelector);
    for c in "locale".chars() {
        app.handle_key(&keymap, plain(c));
    }
    // Space while the name field still has focus types a space, it does
    // not reach the toggle.
    app.handle_key(&keymap, plain(' '));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, plain(' '));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(
        app.project.model.selectors["locale"].shared,
        "{:?}",
        app.toasts.messages()
    );
}

/// Choosing the picker's "add new option…" row closes the picker and
/// immediately opens the new-option prompt. The stack is empty for that
/// instant, but the user must not see the backdrop un-dim and the panel
/// re-settle behind it — that reads as a flash mid-flow.
#[test]
fn chaining_from_the_picker_into_the_option_prompt_does_not_replay_the_open_settle() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    focus_url_with_cursor_on(&mut app, "https://x/{{user}}", "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });
    // "user" has two entries (alice, bob); the ghost "add new option…" row
    // sits one past them.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, enter_key());

    assert!(
        matches!(app.modals.top(), Some(Modal::MultiPrompt { .. })),
        "the new-option prompt takes over"
    );
    assert_eq!(
        app.anims
            .value_or(crate::anim::AnimKey::ModalOpen, Instant::now(), 1.0),
        1.0,
        "the handoff keeps the panel settled instead of re-opening it"
    );
}

#[test]
fn new_selector_prompt_tab_cycles_between_the_name_field_and_the_toggle() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

    app.update(Action::PromptNewSelector);
    for c in "locale".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, tab); // onto the toggle
    app.handle_key(&keymap, plain(' ')); // shared on
    app.handle_key(&keymap, tab); // back to the field
    app.handle_key(&keymap, plain(' ')); // a typed space, not a second flip
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(
        app.project.model.selectors["locale"].shared,
        "{:?}",
        app.toasts.messages()
    );
}

#[test]
fn new_selector_prompt_up_returns_focus_to_the_name_field() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    app.update(Action::PromptNewSelector);
    for c in "locale".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, plain(' ')); // shared on
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    app.handle_key(&keymap, plain(' ')); // back in the field: a typed space
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(
        app.project.model.selectors["locale"].shared,
        "{:?}",
        app.toasts.messages()
    );
}

#[test]
fn var_struct_new_group_creates_group_with_members() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::NewSelector {
        name: "creds".into(),
        fields: vec!["user_id".into(), "customer_id".into()],
        shared: false,
    }));

    assert!(app.toasts.is_empty());
    let g = app
        .project
        .model
        .selectors
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
    app.update(Action::VarStruct(VarStructOp::NewSelector {
        name: "creds".into(),
        fields: vec!["user_id".into(), "customer_id".into()],
        shared: false,
    }));

    let mut values = indexmap::IndexMap::new();
    values.insert("user_id".to_string(), "1001".to_string());
    values.insert("customer_id".to_string(), "c-77".to_string());
    app.update(Action::VarStruct(VarStructOp::NewOption {
        env: "qa".into(),
        selector: "creds".into(),
        name: "alice".into(),
        description: None,
        values,
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    let entry = postui_core::varmodel::selector_options(&app.project.env_data, "creds")
        .and_then(|entries| entries.get("alice"))
        .expect("entry created in the active env");
    assert_eq!(entry.values["user_id"], "1001");
    assert_eq!(entry.values["customer_id"], "c-77");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("[options.creds.alice]"), "{on_disk}");
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

[selectors.tier]
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
        "[options.tier.gold]\ntier = \"g-1\"\n",
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
        qa_on_disk.contains("[options.tier.gold]"),
        "an unrelated group's entries stay untouched: {qa_on_disk}"
    );
}

#[test]
fn var_struct_delete_var_removes_the_declaration_and_clamps_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenManage { tab: None });
    rendered_text(&mut app);
    app.varmanager.left_cursor = app.varmanager.left_rows.len() + 5;

    app.update(Action::VarStruct(VarStructOp::Delete {
        name: "base_url".into(),
    }));

    assert!(
        app.toasts.messages().join("\n").contains("^Z undoes"),
        "{:?}",
        app.toasts.messages()
    );
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
            + "\n[selectors.region]\ndescription = \"deploy region\"\nfields = [\"region\"]\n",
    )
    .unwrap();
    // qa is the active env (see var_project); entries for "region" there,
    // plus the same shape in the non-active "dev" env.
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "base_url = \"https://qa.example.com\"\n[options.region.east]\nregion = \"us-east-1\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/dev.toml"),
        "[options.region.west]\nregion = \"us-west-1\"\n",
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
    assert!(!app.project.model.selectors.contains_key("region"));
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
/// environment's `[options.<group>]` table.
#[test]
fn var_struct_delete_group_cascades_into_every_environments_entries_table() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    app_new_group(dir.path(), "creds", &["user_id", "customer_id"]);
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "base_url = \"https://qa.example.com\"\n\n[options.user.alice]\nuser = \"1001\"\n\n[options.creds.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-1\"\n",
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::Delete {
        name: "creds".into(),
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(!app.project.model.selectors.contains_key("creds"));
    let qa_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!qa_on_disk.contains("creds"), "{qa_on_disk}");
}

/// Declares a variable-less group directly in `variables.toml` — a thin
/// helper so the delete-cascade test above doesn't need a full
/// `VarStructOp::NewSelector` round trip through a running `App`.
fn app_new_group(dir: &std::path::Path, name: &str, members: &[&str]) {
    let existing = std::fs::read_to_string(dir.join("variables.toml")).unwrap();
    let members_list = members
        .iter()
        .map(|m| format!("\"{m}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("variables.toml"),
        format!("{existing}\n[selectors.{name}]\nfields = [{members_list}]\n"),
    )
    .unwrap();
}

#[test]
fn var_struct_set_fields_replaces_the_group_list() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarStruct(VarStructOp::NewSelector {
        name: "creds".into(),
        fields: vec!["user_id".into()],
        shared: false,
    }));

    app.update(Action::VarStruct(VarStructOp::SetFields {
        selector: "creds".into(),
        fields: vec!["user_id".into(), "customer_id".into()],
    }));

    assert_eq!(
        app.project.model.selectors["creds"].fields,
        vec!["user_id".to_string(), "customer_id".to_string()]
    );
}

fn request_with_var(dir: &std::path::Path, slug: &str, name: &str, value: &str) {
    let leaf = slug.rsplit('/').next().unwrap_or(slug);
    let mut r = postui_core::model::HttpRequest::from_toml_str(&format!(
        "url = \"https://x/{leaf}\"\n[variables]\n{name} = \"{value}\"\n"
    ))
    .unwrap();
    r.url = format!("https://x/{leaf}");
    postui_core::storage::save_request(dir, slug, &r).unwrap();
}

#[test]
fn var_struct_promote_to_default_writes_the_declaration_and_removes_the_request_entry() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    request_with_var(dir.path(), "main/ping", "trace_id", "abc-123");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));

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
    request_with_var(dir.path(), "main/ping", "trace_id", "abc-123");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    assert_eq!(app.project.env_label(), "qa");
    app.update(Action::ForceOpenRequest("main/ping".into()));

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

/// Finding 2: `apply_promote`'s request-entry removal used to only exist
/// in the dirty editor buffer. The request file on disk must lose the
/// promoted entry immediately.
#[test]
fn var_struct_promote_removes_the_entry_from_the_request_file_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    request_with_var(dir.path(), "main/ping", "trace_id", "abc-123");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));

    app.update(Action::VarStruct(VarStructOp::Promote {
        name: "trace_id".into(),
        target: postui_core::varedit::PromoteTarget::Default,
    }));

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(!app.editor.is_dirty());
    let on_disk = postui_core::storage::load_request(dir.path(), "main/ping").unwrap();
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
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping/abc-123"))
        .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
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
    let on_disk = postui_core::storage::load_request(dir.path(), "main/ping").unwrap();
    assert_eq!(
        on_disk.variables["trace_id"].value,
        "https://x/ping/abc-123"
    );
    assert_eq!(on_disk.url, "{{trace_id}}");
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

    app.update(Action::VarStruct(VarStructOp::DeleteOption {
        env: "qa".into(),
        selector: "user".into(),
        name: "bob".into(),
    }));

    assert!(
        app.toasts.messages().join("\n").contains("^Z undoes"),
        "{:?}",
        app.toasts.messages()
    );
    let entries = postui_core::varmodel::selector_options(&app.project.env_data, "user")
        .expect("the group still has entries here");
    assert!(!entries.contains_key("bob"));
    assert!(entries.contains_key("alice"), "the others are untouched");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!on_disk.contains("[options.user.bob]"), "{on_disk}");
}

#[test]
fn delete_entry_that_is_already_gone_is_a_quiet_no_op() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarStruct(VarStructOp::DeleteOption {
        env: "qa".into(),
        selector: "user".into(),
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

    app.update(Action::VarStruct(VarStructOp::DeleteOption {
        env: "qa".into(),
        selector: "user".into(),
        name: "alice".into(),
    }));

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
    postui_core::storage::save_request(dir.path(), "main/uses-it", &r).unwrap();
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
    request_with_var(dir.path(), "main/ping", "api_key", "sk-oops");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));

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
fn delete_var_warns_about_referencing_requests() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let mut r = req("https://x/uses-it/{{base_url}}");
    r.url = "https://x/uses-it/{{base_url}}".into();
    postui_core::storage::save_request(dir.path(), "main/uses-it", &r).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::DeleteVar {
        name: "base_url".into(),
    });

    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
    assert!(!app.project.model.vars.contains_key("base_url"));
    let msgs = app.toasts.messages().join("\n");
    assert!(
        msgs.contains("uses-it") && msgs.contains('1'),
        "a toast must name the referencing request: {msgs}"
    );
}

#[test]
fn delete_var_is_immediate_with_an_undo_hint_toast() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::DeleteVar {
        name: "base_url".into(),
    });

    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
    assert!(!app.project.model.vars.contains_key("base_url"));
    let msgs = app.toasts.messages().join("\n");
    assert!(
        msgs.contains("^Z undoes"),
        "the toast advertises the escape hatch: {msgs}"
    );
    app.update(Action::Undo);
    assert!(
        app.project.model.vars.contains_key("base_url"),
        "undo restores the declaration"
    );
}

#[test]
fn delete_entry_is_immediate_with_an_undo_hint_toast() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::DeleteEntry {
        env: "qa".into(),
        selector: "user".into(),
        name: "alice".into(),
    });

    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
    let env = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert!(!env.options["user"].contains_key("alice"));
    assert!(
        app.toasts.messages().join("\n").contains("^Z undoes"),
        "the toast advertises the escape hatch: {:?}",
        app.toasts.messages()
    );
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
        Some(Modal::Prompt {
            kind: PromptKind::NewSelector {
                shared: false,
                on_toggle: false,
            },
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
    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
    assert!(
        !app.project.model.vars.contains_key("base_url"),
        "the row is deleted at once"
    );
    app.update(Action::Undo);
    assert!(
        app.project.model.vars.contains_key("base_url"),
        "undo restores it"
    );
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    app.handle_key(&keymap, plain('s'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

/// Mouse/keyboard parity (spec §5: "every mutation ... has a keyboard
/// action and a painted button"): the left list's context-menu "Delete"
/// dispatches the exact same action the `d` key does.
#[test]
fn the_context_menu_delete_matches_the_d_key() {
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
    assert_eq!(
        via_menu[2].action,
        Some(Action::DeleteVar {
            name: "base_url".into()
        })
    );
    app.update(via_menu[2].action.clone().unwrap());
    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
    assert!(!app.project.model.vars.contains_key("base_url"));
}

#[test]
fn clicking_the_new_variable_button_opens_the_new_variable_prompt() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::OpenManage { tab: None });
    // Rendered wide: the Manage bar's tab strip has priority for its own
    // width, so `+ Selector` is dropped on an 80-column bar (it stays
    // reachable by key and footer chip). Both buttons paint from 120.
    rendered_text_wide(&mut app);

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
    rendered_text_wide(&mut app);
    let rect = app
        .hits
        .rect_of(&crate::hit::Hit::VmNewSelector)
        .expect("+ Group button must be painted");
    assert!(app.handle_mouse(left_down(rect.x + 1, rect.y + 1)));
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::NewSelector {
                shared: false,
                on_toggle: false,
            },
            ..
        })
    ));
}

#[test]
fn prompt_new_selector_takes_a_name_and_defaults_its_field() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewSelector);

    // Name only — the common case is a one-field selection set, so the
    // field defaults to the selector's own name.
    for c in "creds".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    // Creating the selector is the whole gesture: no follow-up prompt
    // opens, the new declaration is simply selected in the manager.
    assert!(app.modals.is_empty());
    let g = app
        .project
        .model
        .selectors
        .get("creds")
        .expect("selector created");
    assert_eq!(g.fields, vec!["creds".to_string()]);
    app.update(Action::ReloadProjectFiles);
    assert!(app.project.model.selectors.contains_key("creds"));
}

#[test]
fn add_and_remove_group_members_one_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::VarStruct(VarStructOp::NewSelector {
        name: "creds".into(),
        fields: vec![],
        shared: false,
    }));

    // `a` flow: one member name per prompt, appended in order
    for member in ["user_id", "customer_id"] {
        app.update(Action::PromptAddSelectorField {
            selector: "creds".into(),
        });
        for c in member.chars() {
            app.handle_key(&keymap, plain(c));
        }
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }
    assert_eq!(
        app.project.model.selectors.get("creds").unwrap().fields,
        vec!["user_id".to_string(), "customer_id".to_string()]
    );

    // duplicate append toasts and changes nothing
    let toasts_before = app.toasts.messages().len();
    app.update(Action::PromptAddSelectorField {
        selector: "creds".into(),
    });
    for c in "user_id".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.toasts.messages().len() > toasts_before);
    assert_eq!(
        app.project
            .model
            .selectors
            .get("creds")
            .unwrap()
            .fields
            .len(),
        2
    );

    // the failed duplicate keeps its prompt open for a retry; drop it
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // `d` flow: removal is immediate (undoable)
    app.update(Action::RemoveSelectorField {
        selector: "creds".into(),
        field: "user_id".into(),
    });
    assert!(app.modals.is_empty(), "removal is undoable, no confirm");
    assert_eq!(
        app.project.model.selectors.get("creds").unwrap().fields,
        vec!["customer_id".to_string()]
    );
}

// --- Task 14: selection-context picker ---------------------------------

fn group_project(dir: &std::path::Path) {
    postui_core::project::init_project(dir, Some("demo")).unwrap();
    std::fs::write(
        dir.join("variables.toml"),
        r#"
[selectors.identity]
description = "identity"
fields = ["user_id", "customer_id"]
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("environments/qa.toml"),
        r#"
[options.identity.alice]
description = "admin"
user_id = "1001"
customer_id = "c-77"

[options.identity.bob]
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
            selector: "user".into(),
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

// -- Clicking a token is a value control, not an insert picker ----------

fn token_popup_app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.anims.enabled = false;
    (app, dir)
}

#[test]
fn clicking_a_selector_field_token_opens_the_select_picker() {
    let (mut app, _dir) = token_popup_app();
    app.project.set_selection_for("qa", "user", "alice");
    app.editor.url = crate::components::line_input::LineInput::new("https://x/{{user}}");
    render_once(&mut app);

    let r = app
        .hits
        .rect_of(&crate::hit::Hit::VarToken("user".into()))
        .expect("the token registers a hit");
    app.handle_mouse(left_down(r.x, r.y));

    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("clicking a selector-field token must open the select picker")
    };
    assert_eq!(
        p.mode,
        crate::components::var_picker::PickerMode::SelectOption {
            name: "user".into(),
            selector: "user".into(),
        }
    );
}

#[test]
fn clicking_a_simple_var_token_opens_the_value_popup_on_its_supplying_scope() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));

    let Some(Modal::MultiPrompt {
        title,
        fields,
        kind,
        ..
    }) = app.modals.top()
    else {
        panic!("a simple variable token must open the value popup")
    };
    assert_eq!(title, "{{base_url}}");
    assert!(
        matches!(kind, PromptKind::EditVarValue { name, .. } if name == "base_url"),
        "unexpected kind"
    );
    let value = fields.iter().find(|f| f.key == "value").unwrap();
    assert_eq!(value.input.text(), "https://qa.example.com");
    let scope = fields.iter().find(|f| f.key == "destination").unwrap();
    assert_eq!(
        scope.input.text(),
        "Active env value",
        "the env supplies the value today, so the env scope is preselected"
    );
    assert_eq!(
        scope.choices,
        vec!["Active env value", "This request"],
        "a shadowed wider scope (the default) is not offered — editing it \
         here would change nothing visible"
    );
}

#[test]
fn the_value_popup_preselects_the_request_scope_when_a_request_var_supplies_it() {
    let (mut app, _dir) = token_popup_app();
    app.editor.variables.insert(
        "base_url".into(),
        postui_core::model::Entry {
            value: "http://req.local".into(),
            enabled: true,
        },
    );
    app.update(Action::OpenVarTokenPopup("base_url".into()));

    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("expected the value popup")
    };
    let value = fields.iter().find(|f| f.key == "value").unwrap();
    assert_eq!(value.input.text(), "http://req.local");
    let scope = fields.iter().find(|f| f.key == "destination").unwrap();
    assert_eq!(scope.input.text(), "This request");
    assert_eq!(
        scope.choices,
        vec!["This request"],
        "a request override shadows everything wider"
    );
}

#[test]
fn the_value_popup_preselects_default_when_only_the_default_supplies_it() {
    let (mut app, _dir) = token_popup_app();
    // dev has no flat values, so base_url falls back to its default there.
    app.update(Action::SwitchEnv(Some("dev".into())));
    app.update(Action::OpenVarTokenPopup("base_url".into()));

    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("expected the value popup")
    };
    let value = fields.iter().find(|f| f.key == "value").unwrap();
    assert_eq!(value.input.text(), "http://localhost:8080");
    let scope = fields.iter().find(|f| f.key == "destination").unwrap();
    assert_eq!(scope.input.text(), "Project default");
    assert_eq!(
        scope.choices,
        vec!["Project default", "Active env value", "This request"],
        "nothing shadows the default, so every scope is on offer"
    );
}

#[test]
fn confirming_the_value_popup_writes_the_env_scope_and_re_resolves() {
    let (mut app, dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    // Type a replacement value and confirm with the preselected env scope.
    let keymap = Keymap::default_bindings();
    for _ in 0.."https://qa.example.com".len() {
        app.handle_key(
            &keymap,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
    }
    for c in "https://qa2.example.com".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.modals.is_empty(), "confirm closes the popup");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        on_disk.contains("base_url = \"https://qa2.example.com\""),
        "{on_disk}"
    );
    assert_eq!(
        app.project.resolved.values["base_url"], "https://qa2.example.com",
        "linked tokens re-resolve immediately"
    );
}

#[test]
fn confirming_the_value_popup_on_the_default_scope_writes_variables_toml() {
    let (mut app, dir) = token_popup_app();
    app.update(Action::ConfirmEditVarValue {
        name: "base_url".into(),
        value: "http://new-default".into(),
        destination: crate::action::ExtractDestination::ProjectDefault,
    });
    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(
        on_disk.contains("default = \"http://new-default\""),
        "{on_disk}"
    );
}

#[test]
fn confirming_the_value_popup_on_the_request_scope_sets_a_request_var() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::ConfirmEditVarValue {
        name: "base_url".into(),
        value: "http://req.local".into(),
        destination: crate::action::ExtractDestination::Request,
    });
    assert_eq!(app.editor.variables["base_url"].value, "http://req.local");
    assert!(app.editor.variables["base_url"].enabled);
}

#[test]
fn clicking_the_write_to_field_cycles_the_scope() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);

    // The destination is field 1; a click advances to the next choice,
    // wrapping — no keyboard needed.
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalField(1))
        .expect("the choice field takes clicks");
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let scope_text = |app: &App| {
        let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
            panic!("popup still open")
        };
        fields[1].input.text().to_string()
    };
    assert_eq!(
        scope_text(&app),
        "This request",
        "from the preselected env scope, one click advances"
    );
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    assert_eq!(
        scope_text(&app),
        "This request",
        "no wrap: the last scope is an end stop"
    );
}

#[test]
fn a_taken_name_keeps_the_new_variable_prompt_open_with_the_typed_text() {
    let (mut app, _dir) = token_popup_app();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewVar);
    for c in "base_url".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(!app.toasts.is_empty(), "the refusal is surfaced");
    let Some(Modal::Prompt { input, kind, .. }) = app.modals.top() else {
        panic!("the prompt must stay open so the name can be fixed")
    };
    assert_eq!(*kind, PromptKind::NewVariable);
    assert_eq!(input.text(), "base_url");
}

#[test]
fn a_taken_name_keeps_the_new_selector_prompt_open() {
    let (mut app, _dir) = token_popup_app();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewSelector);
    for c in "user".chars() {
        // "user" is already a selector in the fixture.
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(!app.toasts.is_empty());
    let Some(Modal::Prompt { input, kind, .. }) = app.modals.top() else {
        panic!("the prompt must stay open")
    };
    assert!(matches!(kind, PromptKind::NewSelector { .. }));
    assert_eq!(input.text(), "user");
}

#[test]
fn a_refused_apply_keeps_the_fields_editor_open() {
    let (mut app, _dir) = fields_editor_app();
    let keymap = Keymap::default_bindings();
    // Retype row 0 (user_id) as customer_id — a duplicate within the list.
    for _ in 0.."user_id".len() {
        app.handle_key(
            &keymap,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
    }
    for c in "customer_id".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(!app.toasts.is_empty(), "the refusal is surfaced");
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("the fields editor must stay open so the clash can be fixed")
    };
    assert_eq!(fe.rows[0].input.text(), "customer_id", "typed text kept");
    assert_eq!(
        app.project.model.selectors["creds"].fields,
        vec!["user_id", "customer_id"],
        "nothing was written"
    );
}

#[test]
fn cycling_the_write_to_scope_shows_that_scopes_current_value() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    let keymap = Keymap::default_bindings();
    // Focus the destination field and cycle: the value field follows,
    // showing what is currently stored at each scope.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let field_texts = |app: &App| {
        let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
            panic!("popup open")
        };
        (
            fields[0].input.text().to_string(),
            fields[1].input.text().to_string(),
        )
    };
    assert_eq!(
        field_texts(&app),
        ("https://qa.example.com".into(), "Active env value".into())
    );
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(
        field_texts(&app),
        (String::new(), "This request".into()),
        "no request override yet, so the value box is empty"
    );
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(
        field_texts(&app),
        (String::new(), "This request".into()),
        "no wrap: stepping right at the last scope stays put"
    );
}

#[test]
fn toasts_paint_undimmed_above_an_open_modal() {
    let (mut app, _dir) = token_popup_app();
    app.toasts.push("\"user\" already exists", ToastKind::Error);
    let find_cell = |app: &mut App| {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        app.anims.finish_all();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width.saturating_sub(8) {
                let text: String = (0..8).map(|i| buf[(x + i, y)].symbol()).collect();
                if text.contains("already") {
                    return Some(buf[(x, y)].fg);
                }
            }
        }
        None
    };
    let plain_fg = find_cell(&mut app).expect("toast visible with no modal");
    app.update(Action::PromptNewVar);
    let modal_fg = find_cell(&mut app).expect("toast visible over the modal");
    assert_eq!(
        plain_fg, modal_fg,
        "the modal backdrop must not dim the toast"
    );
}

#[test]
fn the_value_popup_offers_remove_only_where_a_value_is_stored() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    let content = rendered_text(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalRemove)
        .expect("the env stores a value, so it can be removed");
    // Same affordance as the variable form's inline control: the
    // one-row "✕ remove" beside the value field's label, not a boxed
    // button in the confirm row.
    assert!(content.contains("\u{2715} remove"), "{content}");
    assert_eq!(r.height, 1, "inline control, not a boxed button");

    // Cycle to "This request", which stores nothing — nothing to remove.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    let content = rendered_text(&mut app);
    assert!(
        app.hits.rect_of(&crate::hit::Hit::ModalRemove).is_none(),
        "no stored value at this scope, no remove button"
    );
    assert!(
        content.contains("(not set)"),
        "an empty scope's value box reads (not set), not a copied-over value: {content}"
    );
}

/// The keyboard mirror of the popup's "✕ remove": `alt+d` marks the
/// chosen Write-to scope's stored value for removal and re-lands the
/// popup on the next supplier, exactly like the click. Where the chosen scope stores
/// nothing (no ✕ painted), the chord is inert.
#[test]
fn the_value_popup_alt_d_removes_the_chosen_scopes_value() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('d'));

    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("the popup rebuilds on the next supplier after a removal")
    };
    let scope = fields.iter().find(|f| f.key == "destination").unwrap();
    assert_eq!(
        scope.input.text(),
        "Project default",
        "the env value is gone, so the default supplies now"
    );
    let value = fields.iter().find(|f| f.key == "value").unwrap();
    assert_eq!(value.input.text(), "http://localhost:8080");

    // Cycle to "This request" (stores nothing): alt+d must be inert.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    app.handle_key(&keymap, alt('d'));
    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("an inert alt+d must not close the popup")
    };
    let scope = fields.iter().find(|f| f.key == "destination").unwrap();
    assert_eq!(scope.input.text(), "This request", "nothing was removed");
}

/// The value popup's footer chips teach its keys and name the scope the
/// remove chord would hit — and the remove chip only shows where the ✕
/// itself would (the chosen scope stores something).
#[test]
fn the_value_popup_advertises_its_chords_in_the_footer() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    let content = rendered_text(&mut app);
    assert!(content.contains("write to"), "{content}");
    assert!(
        content.contains("remove env value"),
        "the chip names the chosen scope: {content}"
    );

    // Cycle to "This request", which stores nothing — no remove chip.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    let content = rendered_text(&mut app);
    assert!(!content.contains("remove env value"), "{content}");
    assert!(!content.contains("remove request value"), "{content}");
}

/// User finding: only the keyboard cycle reseeded the value box — a
/// *click* on the Write-to field cycled the choice but kept the previous
/// scope's text, so an empty scope showed a value copied over from the
/// last one instead of "(not set)".
#[test]
fn clicking_the_write_to_field_cycles_the_scope_and_reseeds_the_value_box() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    // Fields are [value, destination]; a click on the choice field cycles
    // it from "Active env value" (stored) to "This request" (nothing).
    let r = app.hits.rect_of(&crate::hit::Hit::ModalField(1)).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let content = rendered_text(&mut app);
    assert!(content.contains("This request"), "{content}");
    assert!(
        content.contains("(not set)"),
        "the empty scope's value box must reseed on a click-cycle too: {content}"
    );
}

/// The Write-to control is a bounded stepper, not a loop: each arrow
/// steps one scope in its direction and greys out (unregistered) at its
/// end, so the two ends orient you in the list.
#[test]
fn the_write_to_arrows_step_and_disable_at_the_ends() {
    let left = crate::hit::Hit::ModalChoiceArrow { field: 1, dir: -1 };
    let right = crate::hit::Hit::ModalChoiceArrow { field: 1, dir: 1 };
    let (mut app, _dir) = token_popup_app();
    // dev stores nothing, so all three scopes are on offer, preselected
    // to "Project default" (the supplier, and the first choice).
    app.update(Action::SwitchEnv(Some("dev".into())));
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);

    assert!(
        app.hits.rect_of(&left).is_none(),
        "at the first scope the left arrow is disabled"
    );
    let r = app.hits.rect_of(&right).expect("right arrow live");
    app.handle_mouse(left_down(r.x, r.y));
    let content = rendered_text(&mut app);
    assert!(content.contains("Active env value"), "{content}");
    assert!(
        app.hits.rect_of(&left).is_some() && app.hits.rect_of(&right).is_some(),
        "mid-list, both arrows are live"
    );

    let r = app.hits.rect_of(&right).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    let content = rendered_text(&mut app);
    assert!(content.contains("This request"), "{content}");
    assert!(
        app.hits.rect_of(&right).is_none(),
        "at the last scope the right arrow is disabled"
    );

    let l = app.hits.rect_of(&left).expect("left arrow live at the end");
    app.handle_mouse(left_down(l.x, l.y));
    let content = rendered_text(&mut app);
    assert!(
        content.contains("Active env value"),
        "the left arrow steps back: {content}"
    );
}

/// Confirms the top modal by clicking its painted Confirm button.
fn click_modal_confirm(app: &mut App) {
    rendered_text(app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalConfirm)
        .expect("confirm button painted");
    app.handle_mouse(left_down(r.x, r.y));
}

/// User finding: Remove wrote to disk on the spot, so Cancel afterwards
/// had nothing to put back. It is staged now: the popup previews the
/// cleared state and the next supplier, and only Confirm applies it.
#[test]
fn remove_is_pending_until_confirm_and_cancel_puts_nothing_on_disk() {
    let (mut app, dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalRemove).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    assert!(
        !app.modals.is_empty(),
        "removal keeps the popup open to show the cleared state"
    );
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        on_disk.contains("base_url"),
        "nothing written yet: {on_disk}"
    );
    // The popup follows the pending removal to the scope that would
    // supply: the default, with its stored value ready to edit (or
    // remove in turn).
    let content = rendered_text(&mut app);
    assert!(content.contains("Project default"), "{content}");
    assert!(
        content.contains("http://localhost:8080"),
        "the next supplier's stored value is on show: {content}"
    );
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalRemove)
        .expect("the default stores a value, so it too can be marked");

    // Marking the default too previews "nothing supplies".
    app.handle_mouse(left_down(r.x, r.y));
    let content = rendered_text(&mut app);
    assert!(content.contains("(not set)"), "{content}");
    assert!(
        app.hits.rect_of(&crate::hit::Hit::ModalRemove).is_none(),
        "nothing left to mark"
    );

    // Cancel: both marks are forgotten, nothing was ever written.
    app.update(Action::Close);
    assert!(app.modals.is_empty());
    let env_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_on_disk.contains("base_url"), "{env_on_disk}");
    let vars_on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(vars_on_disk.contains("default"), "{vars_on_disk}");
    assert_eq!(
        app.project.resolved.values["base_url"], "https://qa.example.com",
        "the env value still supplies"
    );
    // Reopening starts clean: the env value is back on offer to remove.
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    assert!(app.hits.rect_of(&crate::hit::Hit::ModalRemove).is_some());
}

#[test]
fn confirming_applies_the_pending_removal_and_the_default_shows_through() {
    let (mut app, dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalRemove).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    // Confirm on the default's own (unchanged) value: the env value goes,
    // the default is rewritten as itself.
    click_modal_confirm(&mut app);
    assert!(app.modals.is_empty(), "confirm closes the popup");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!on_disk.contains("base_url"), "{on_disk}");
    assert_eq!(
        app.project.resolved.values["base_url"], "http://localhost:8080",
        "the default shows through once the env value is gone"
    );
}

#[test]
fn confirming_with_every_scope_marked_removes_them_all_and_writes_nothing_blank() {
    let (mut app, dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalRemove).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalRemove).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    click_modal_confirm(&mut app);
    assert!(app.modals.is_empty());
    let env_on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!env_on_disk.contains("base_url"), "{env_on_disk}");
    let vars_on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(!vars_on_disk.contains("default"), "{vars_on_disk}");
    assert!(
        app.project.model.vars["base_url"].default.is_none(),
        "an empty box on a removed scope means removed, not set to \"\""
    );
}

#[test]
fn typing_a_value_on_a_marked_scope_writes_it_instead_of_removing() {
    let (mut app, dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalRemove).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    // Cycle Write-to back onto the (marked) env scope and type a value.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("popup open")
    };
    let scope = fields.iter().find(|f| f.key == "destination").unwrap();
    assert_eq!(scope.input.text(), "Active env value");
    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
    );
    for c in "http://new.qa".chars() {
        app.handle_key(&keymap, plain(c));
    }

    click_modal_confirm(&mut app);
    assert!(app.modals.is_empty());
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("http://new.qa"), "{on_disk}");
    assert_eq!(app.project.resolved.values["base_url"], "http://new.qa");
}

#[test]
fn remove_marks_a_request_override_and_confirm_drops_it() {
    let (mut app, _dir) = token_popup_app();
    app.editor.variables.insert(
        "base_url".into(),
        postui_core::model::Entry {
            value: "http://req.local".into(),
            enabled: true,
        },
    );
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalRemove).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    assert!(!app.modals.is_empty(), "the popup stays open");
    assert!(
        app.editor.variables.contains_key("base_url"),
        "pending: the [variables] override is still there"
    );
    click_modal_confirm(&mut app);
    assert!(
        !app.editor.variables.contains_key("base_url"),
        "the [variables] override is gone on confirm"
    );
}

#[test]
fn remove_marks_the_default_when_it_is_the_supplier_and_confirm_clears_it() {
    let (mut app, dir) = token_popup_app();
    // dev has no flat values, so the default supplies base_url there.
    app.update(Action::SwitchEnv(Some("dev".into())));
    app.update(Action::OpenVarTokenPopup("base_url".into()));
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalRemove).unwrap();
    app.handle_mouse(left_down(r.x, r.y));

    assert!(!app.modals.is_empty(), "the popup stays open");
    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(on_disk.contains("default"), "pending: {on_disk}");
    click_modal_confirm(&mut app);
    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(!on_disk.contains("default"), "{on_disk}");
    assert!(
        app.project.model.vars["base_url"].default.is_none(),
        "the declaration keeps only its description"
    );
}

#[test]
fn clicking_a_secret_token_opens_the_masked_secret_prompt() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("api_key".into()));

    let Some(Modal::Prompt { kind, .. }) = app.modals.top() else {
        panic!("a secret token must open the masked secret prompt")
    };
    assert_eq!(
        *kind,
        PromptKind::SecretValue {
            name: "api_key".into(),
            env: "qa".into(),
        }
    );
}

#[test]
fn clicking_a_token_of_a_selector_with_no_options_opens_the_picker_on_its_ghost_row() {
    let (mut app, _dir) = token_popup_app();
    // dev has no options for the selector, so the picker opens with only
    // its "add new option…" ghost row — no prompt is forced on the user.
    app.update(Action::SwitchEnv(Some("dev".into())));
    app.update(Action::OpenVarTokenPopup("user".into()));

    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("expected the select picker")
    };
    assert!(matches!(
        p.mode,
        crate::components::var_picker::PickerMode::SelectOption { .. }
    ));
    assert_eq!(p.row_count(), 1, "the ghost row is the only row");
}

#[test]
fn clicking_an_undefined_token_still_opens_the_insert_picker_seeded() {
    let (mut app, _dir) = token_popup_app();
    app.update(Action::OpenVarTokenPopup("nope".into()));

    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("an undefined name keeps the insert/create picker")
    };
    assert!(matches!(
        p.mode,
        crate::components::var_picker::PickerMode::Insert
    ));
    assert_eq!(p.input(), "nope");
}

#[test]
fn ctrl_v_on_group_member_token_shows_the_group_s_options_with_a_detail_pane() {
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
            selector: "identity".into(),
        }
    );

    // alice (row 0) is highlighted: her description sits on the row and
    // her member values fill the detail pane below the list.
    let content = rendered_text(&mut app);
    assert!(content.contains("alice"), "{content}");
    assert!(content.contains("admin"), "{content}");
    assert!(content.contains("user_id"), "{content}");
    assert!(content.contains("1001"), "{content}");
    assert!(content.contains("c-77"), "{content}");
    assert!(
        !content.contains("1002"),
        "bob's values stay out of sight until he's highlighted: {content}"
    );
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
fn select_option_arrows_move_the_selection_and_typing_is_inert() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    focus_url_with_cursor_on(&mut app, "https://x/{{user}}", "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });
    // The picker has no filter: typed letters do nothing, arrows select.
    for c in "alice".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.project.selections_for("qa")["user"], "bob");
}

#[test]
fn blocked_send_toast_names_first_needs_selection_var_with_a_picker_hint() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "main/r", &req("https://x/{{user}}")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/r".into()));

    app.update(Action::ForceSend);

    assert!(app.session.in_flight.is_empty());
    let content = rendered_text(&mut app);
    assert!(content.contains("need a selection"), "{content}");
    assert!(
        content.contains(&format!(
            "press {}+shift+v to select user",
            crate::keys::alt_label()
        )),
        "{content}"
    );
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

    // The quick-create prompt is name + value only (no description field —
    // that lives in the option's edit prompt).
    type_into_field(&mut app, &keymap, "carol");
    app.handle_key(&keymap, tab_key());
    type_into_field(&mut app, &keymap, "3003");
    app.handle_key(&keymap, enter_key());

    assert!(app.modals.is_empty(), "closes back to the field");
    assert_eq!(app.focus, PaneId::Editor, "focus restored to where it was");
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
    assert_eq!(app.editor.url.text(), url, "the token text is untouched");

    // Written to the ACTIVE ENV's entries table — entries only ever live
    // in an environment file (spec §3.1).
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("[options.user.carol]"), "{env_doc}");
    assert!(env_doc.contains("3003"), "{env_doc}");

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
    assert!(env_doc.contains("[options.user.\"user 1\"]"), "{env_doc}");
    assert!(env_doc.contains("9009"), "{env_doc}");
}

/// A selector's fields are meant to be set together, so the inline
/// create prompt on a multi-field group takes one input per field —
/// labelled by field name, in declared order — and writes them all.
#[test]
fn inline_create_on_a_multi_field_group_takes_one_input_per_field() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    // Start from the second field's token: the prompt is about the whole
    // option, not the field clicked.
    focus_url_with_cursor_on(&mut app, "https://x/{{customer_id}}", "{{customer_id}}");
    app.update(Action::OpenVarPicker { completing: false });
    // "identity" has two entries (alice, bob); the ghost row sits one past
    // them.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, enter_key());

    let Some(Modal::MultiPrompt { title, fields, .. }) = app.modals.top() else {
        panic!("expected the new-option prompt")
    };
    assert_eq!(title, "Add option on identity");
    let labels: Vec<&str> = fields.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(labels, ["Name", "user_id", "customer_id"]);

    type_into_field(&mut app, &keymap, "carol");
    app.handle_key(&keymap, tab_key());
    type_into_field(&mut app, &keymap, "u-3");
    app.handle_key(&keymap, tab_key());
    type_into_field(&mut app, &keymap, "c-3");
    app.handle_key(&keymap, enter_key());

    assert!(app.modals.is_empty(), "{:?}", app.toasts.messages());
    let carol = &app.project.env_data.options["identity"]["carol"].values;
    assert_eq!(carol["user_id"], "u-3");
    assert_eq!(carol["customer_id"], "c-3");
    assert!(
        !app.toasts.messages().iter().any(|m| m.contains("empty")),
        "nothing is left empty: {:?}",
        app.toasts.messages()
    );
}

#[test]
fn typing_e_in_the_select_picker_is_inert() {
    // The select picker has no filter, and `e` must not hijack into the
    // option edit prompt either (regression guard for the old filter-era
    // behavior): it stays open, unchanged.
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    focus_url_with_cursor_on(&mut app, "https://x/{{user}}", "{{user}}");
    app.update(Action::OpenVarPicker { completing: false });
    app.handle_key(
        &keymap,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
    );

    let Some(Modal::VarPicker(p)) = app.modals.top() else {
        panic!("the picker stays open");
    };
    assert_eq!(p.input(), "", "no filter to type into");
    assert_eq!(p.selected(), 0, "the highlight is untouched");
}

#[test]
fn the_option_menus_edit_opens_the_prompt_in_the_environment_that_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    goto_group(&mut app, "user");

    // Row 0 is "alice" (first option, file order); its context menu's
    // "Edit…" opens the full edit prompt (values + description).
    let items = app
        .varmanager
        .entry_context_menu(&app.project, 0)
        .expect("option menu");
    let edit = items
        .iter()
        .find(|i| i.label.starts_with("Edit"))
        .expect("an Edit… item")
        .action
        .clone()
        .expect("enabled");
    app.update(edit);

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

    // The refusal keeps the prompt open with the typed name, so it can be
    // fixed instead of retyped.
    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("the extract prompt stays open after a refusal")
    };
    assert_eq!(fields[0].input.text(), "identity");
    let content = rendered_text(&mut app);
    assert!(content.contains("already exists"), "{content}");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
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

    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("the extract prompt stays open after a refusal")
    };
    assert_eq!(fields[0].input.text(), "api_key");
    let content = rendered_text(&mut app);
    assert!(content.contains("secret"), "{content}");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
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

#[test]
fn clicking_off_the_option_edit_prompt_saves_it() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    let mut values = indexmap::IndexMap::new();
    values.insert("user_id".to_string(), "1001".to_string());
    values.insert("customer_id".to_string(), "c-77".to_string());
    app.update(Action::OpenEditOptionPrompt {
        owner: "identity".into(),
        key: "alice".into(),
        description: Some("admin".into()),
        values,
    });
    // The focused first field is user_id, seeded "1001"; type a digit.
    type_chars(&mut app, "9");
    // A click outside the prompt is a save, like the grid's
    // commit-on-click-away — the prompt is an editing surface.
    click_hit(&mut app, Hit::ModalOutside);

    assert!(app.modals.is_empty(), "click-away confirms and closes");
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("10019"), "the typed edit saved: {env_doc}");
}

#[test]
fn clicking_off_the_quick_add_option_prompt_still_cancels() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::OpenNewOptionInlinePrompt {
        owner: "identity".into(),
    });
    type_chars(&mut app, "carol");
    click_hit(&mut app, Hit::ModalOutside);

    assert!(app.modals.is_empty());
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        !env_doc.contains("carol"),
        "creation prompts keep cancel-on-click-away: {env_doc}"
    );
}

#[test]
fn alt_v_toggles_the_variable_manager_closed_and_restores_focus() {
    let mut app = App::new_for_test();
    app.update(Action::FocusPane(PaneId::Response));
    let keymap = Keymap::default_bindings();
    let alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);

    app.handle_key(&keymap, alt_v);
    assert_eq!(app.screen, Screen::Manage);
    app.handle_key(&keymap, alt_v);
    assert_eq!(app.screen, Screen::Main, "alt+v closes the open manager");
    assert_eq!(app.focus, PaneId::Response, "prior focus restored");
}

#[test]
fn plain_q_quits_from_the_variable_manager() {
    let mut app = App::new_for_test();
    app.update(Action::OpenManage { tab: None });
    app.handle_key(&Keymap::default_bindings(), plain('q'));
    assert!(app.should_quit);
}

#[test]
fn variables_screen_footer_hides_the_dead_save_group_but_keeps_palette_and_quit() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    // `ctrl+s` is not on the non-Main screens' global whitelist — the key
    // is swallowed here, so the footer must not advertise it. Palette and
    // quit still work and stay.
    let content = rendered_text_tall(&mut app);
    assert!(!content.contains("^S"), "{content}");
    assert!(
        app.hits
            .rect_of(&crate::hit::Hit::FooterChip(Action::SaveRequest))
            .is_none(),
        "no clickable save chip on the Variables screen"
    );
    assert!(
        content.contains("^P"),
        "palette works here and stays: {content}"
    );
    assert!(
        !content.contains("^C"),
        "plain q quits here now, so the quit keycap is honest again: {content}"
    );
}

#[test]
fn var_edit_set_option_description_writes_and_clearing_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    app.update(Action::VarEdit(VarEditOp::SetOptionDescription {
        env: "qa".into(),
        selector: "identity".into(),
        option: "alice".into(),
        description: Some("boss".into()),
    }));
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("boss"), "{env_doc}");
    assert!(
        !env_doc.contains("admin"),
        "replaced, not appended: {env_doc}"
    );

    app.update(Action::VarEdit(VarEditOp::SetOptionDescription {
        env: "qa".into(),
        selector: "identity".into(),
        option: "alice".into(),
        description: None,
    }));
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!env_doc.contains("boss"), "{env_doc}");
    assert!(env_doc.contains("1001"), "values untouched: {env_doc}");
}

#[test]
fn committing_the_description_cell_writes_through_and_an_emptied_one_removes() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.varmanager.detail = crate::components::varmanager::VmDetail::Group("identity".into());
    app.varmanager.sync(&app.project);

    // Cols: 0 entry, 1 user_id, 2 customer_id, 3 description.
    app.varmanager.start_cell_edit(&app.project, 0, 3);
    let edit = app.varmanager.grid.editing.as_mut().unwrap();
    assert_eq!(edit.input.text(), "admin", "seeded from the stored text");
    edit.input = crate::components::line_input::LineInput::new("head admin");
    app.commit_grid_edit();
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env_doc.contains("head admin"), "{env_doc}");

    app.varmanager.start_cell_edit(&app.project, 0, 3);
    let edit = app.varmanager.grid.editing.as_mut().unwrap();
    edit.input = crate::components::line_input::LineInput::new("");
    app.commit_grid_edit();
    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        !env_doc.contains("head admin"),
        "an emptied description leaves the file: {env_doc}"
    );
    assert!(env_doc.contains("1001"), "values untouched: {env_doc}");
}

#[test]
fn confirm_edit_option_with_an_emptied_description_removes_it_from_the_env_file() {
    let dir = tempfile::tempdir().unwrap();
    group_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    let mut values = indexmap::IndexMap::new();
    values.insert("user_id".to_string(), "1001".to_string());
    values.insert("customer_id".to_string(), "c-77".to_string());
    // The Edit prompt maps a cleared Description field to `None`: that
    // means "remove the stored description", not "leave it alone".
    app.update(Action::ConfirmEditOption {
        owner: "identity".into(),
        key: "alice".into(),
        values,
        description: None,
    });

    let env_doc = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(
        !env_doc.contains("admin"),
        "alice's cleared description must leave the file: {env_doc}"
    );
    assert!(
        env_doc.contains("reader"),
        "bob's description is untouched: {env_doc}"
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
    assert!(app.project.model.selectors.is_empty());
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
    assert_eq!(parsed.selectors["tier"].fields, ["tier"]);
    assert_eq!(parsed.selectors["user"].fields, ["user_id", "customer_id"]);
    assert_eq!(
        app.project.model.selectors["user"].fields,
        ["user_id", "customer_id"]
    );
    assert_eq!(
        app.project.model.vars["base_url"].default.as_deref(),
        Some("http://localhost:8080"),
        "the plain variable came through untouched"
    );

    let qa_after = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    let env = postui_core::varmodel::parse_environment(&qa_after).expect("new env text parses");
    assert_eq!(env.options["tier"]["gold"].values["tier"], "g-qa");
    assert_eq!(
        app.project.env_data.options["user"]["alice"].values["customer_id"],
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
    assert_eq!(app.editor.slug.as_deref(), Some("main/ping"));
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
    assert_eq!(env.options["tier"]["gold"].values["tier"], "g-1");
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

/// Every copy affordance uses the same `❐` glyph — the computed-header
/// rows once carried `⧉`, which renders wrong in some terminals.
#[test]
fn auto_header_copy_icon_is_the_shared_copy_glyph() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Headers;
    app.editor.url = LineInput::new("https://example.com/foo");
    app.update(Action::Render);

    let text = rendered_text(&mut app);
    assert!(!text.contains('\u{29c9}'), "no ⧉ anywhere: {text}");
    let backend = ratatui::backend::TestBackend::new(100, 70);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let rect = app
        .hits
        .rect_of(&Hit::AutoHeaderCopy(0))
        .expect("the Host row's copy icon is registered");
    let row = terminal.backend().buffer().content[rect.y as usize * 100..]
        .iter()
        .take(100)
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(row.contains('❐'), "the Host row's copy icon is ❐: {row}");
}

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
    postui_core::storage::save_request(&app.project.root, "main/a", &req("https://x/a")).unwrap();
    postui_core::storage::save_request(&app.project.root, "main/b", &req("https://x/b")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::OpenRequest("main/a".into()));
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

    app.update(Action::OpenRequest("main/b".into()));
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
/// `rendered_text` on a wide terminal: some bars lay controls out
/// right-aligned and drop the lowest-priority ones when the width runs
/// short (see `components::manage::draw_manage_bar`), so a test about a
/// specific button needs a width that actually paints it.
/// 120 columns and tall: the Manage screen's Spaces pane needs the width
/// for its five title-row buttons and the height for its request list.
fn rendered_text_wide_tall(app: &mut App) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    app.anims.finish_all();
    let backend = TestBackend::new(120, 46);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

fn rendered_text_wide(app: &mut App) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    app.anims.finish_all();
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

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

    // Click at the field's right edge: a click places the caret at the
    // pointer, and these assertions want it at the end of the text.
    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + r.width - 2, r.y + 1));
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

    // Esc reverts: the typed digit never reaches disk. (Right-edge clicks
    // throughout: a click places the caret at the pointer, and the
    // assertions want the typed char at the end of the text.)
    let r = field_rect(&mut app, VmField::Description);
    app.handle_mouse(left_down(r.x + r.width - 2, r.y + 1));
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
    app.handle_mouse(left_down(r.x + r.width - 2, r.y + 1));
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

    // Right-edge click: the caret follows the pointer, and the '!' must
    // land at the end of the text.
    let r = field_rect(&mut app, VmField::Description);
    app.handle_mouse(left_down(r.x + r.width - 2, r.y + 1));
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
    request_with_var(dir.path(), "main/ping", "trace_id", "abc-123");
    postui_core::storage::save_request(
        dir.path(),
        "main/uses-base",
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
    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
    assert!(!app.project.model.vars.contains_key("base_url"));
    assert!(
        app.toasts.messages().join("\n").contains("uses-base"),
        "the usage warning names the referencing request: {:?}",
        app.toasts.messages()
    );
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
    request_with_var(dir.path(), "main/ping", "base_url", "http://from-request");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
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

// -- The fields editor modal (per-row remove / add buttons) --------------

fn fields_editor_app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    std::fs::write(
        dir.path().join("variables.toml"),
        "[selectors.creds]\nfields = [\"user_id\", \"customer_id\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        "[options.creds.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-77\"\n",
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.anims.enabled = false;
    app.update(Action::PromptGroupFields {
        selector: "creds".into(),
    });
    (app, dir)
}

#[test]
fn fields_editor_shows_a_row_per_field_with_remove_buttons_and_an_add_button() {
    let (mut app, _dir) = fields_editor_app();
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("expected the fields editor modal")
    };
    assert_eq!(fe.rows.len(), 2);
    assert_eq!(fe.rows[0].input.text(), "user_id");
    assert_eq!(fe.rows[1].input.text(), "customer_id");

    rendered_text(&mut app);
    assert!(
        app.hits
            .rect_of(&crate::hit::Hit::ModalRowToggle(0))
            .is_some(),
        "each row has a remove button"
    );
    assert!(
        app.hits
            .rect_of(&crate::hit::Hit::ModalRowToggle(1))
            .is_some()
    );
    assert!(
        app.hits.rect_of(&crate::hit::Hit::ModalAddRow).is_some(),
        "an add-field button is present"
    );
}

#[test]
fn fields_editor_remove_button_marks_the_row_and_confirm_deletes_the_field() {
    let (mut app, _dir) = fields_editor_app();
    rendered_text(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalRowToggle(1))
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("still open")
    };
    assert!(fe.rows[1].removed, "the ✕ marks the row for removal");

    // Clicking again restores it...
    rendered_text(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalRowToggle(1))
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("still open")
    };
    assert!(!fe.rows[1].removed);

    // ...remove it again and apply: the removal lands at once (undoable).
    rendered_text(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::ModalRowToggle(1))
        .unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty(), "removal is undoable, no confirm");
    assert_eq!(app.project.model.selectors["creds"].fields, vec!["user_id"]);
}

#[test]
fn fields_editor_rename_types_into_the_row() {
    let (mut app, _dir) = fields_editor_app();
    let keymap = Keymap::default_bindings();
    // Row 0 focused; retype it.
    for _ in 0.."user_id".len() {
        app.handle_key(
            &keymap,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
    }
    for c in "uid".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty(), "apply closes the editor");
    assert_eq!(
        app.project.model.selectors["creds"].fields,
        vec!["uid", "customer_id"]
    );
}

#[test]
fn fields_editor_add_button_appends_a_focused_row() {
    let (mut app, _dir) = fields_editor_app();
    rendered_text(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::ModalAddRow).unwrap();
    app.handle_mouse(left_down(r.x, r.y));
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("still open")
    };
    assert_eq!(fe.rows.len(), 3);
    assert_eq!(fe.focus, 2, "the new row takes focus");

    let keymap = Keymap::default_bindings();
    for c in "region".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert_eq!(
        app.project.model.selectors["creds"].fields,
        vec!["user_id", "customer_id", "region"]
    );
}

/// The keyboard mirror of the add button: `alt+a` appends a row and
/// focuses it, so a keyboard-only user can grow a selector's fields.
#[test]
fn fields_editor_alt_a_appends_a_focused_row() {
    let (mut app, _dir) = fields_editor_app();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('a'));
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("still open")
    };
    assert_eq!(fe.rows.len(), 3, "alt+a appends a row");
    assert_eq!(fe.focus, 2, "the new row takes focus");

    for c in "region".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert_eq!(
        app.project.model.selectors["creds"].fields,
        vec!["user_id", "customer_id", "region"]
    );
}

/// The keyboard mirror of a row's ✕/↩ toggle: `alt+d` marks the focused
/// row removed (focus stepping off it, as the click does), and pressing
/// it again on that row restores it.
#[test]
fn fields_editor_alt_d_toggles_removal_of_the_focused_row() {
    let (mut app, _dir) = fields_editor_app();
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('d'));
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("still open")
    };
    assert!(
        fe.rows[0].removed,
        "alt+d marks the focused row for removal"
    );
    assert_eq!(fe.focus, 1, "focus steps off the removed row");

    // Step back onto the removed row — it must be landable, or the
    // keyboard could never restore it — and flip it back.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("still open")
    };
    assert_eq!(fe.focus, 0, "focus can land on a removed row");
    app.handle_key(&keymap, alt('d'));
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("still open")
    };
    assert!(!fe.rows[0].removed, "alt+d on a removed row restores it");

    // Remove the second field and apply: the removal lands at once
    // (undoable).
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, alt('d'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty(), "removal is undoable, no confirm");
    assert_eq!(app.project.model.selectors["creds"].fields, vec!["user_id"]);
}

/// While the fields editor is open, the footer swaps to *its* context
/// actions — the discoverability layer for the alt chords — instead of
/// the screen's chips, whose verbs aren't reachable under a modal.
#[test]
fn fields_editor_advertises_its_chords_in_the_footer() {
    let (mut app, _dir) = fields_editor_app();
    let content = rendered_text(&mut app);
    assert!(content.contains("add field"), "{content}");
    assert!(content.contains("remove field"), "{content}");
    assert!(content.contains("apply"), "{content}");
    assert!(
        !content.contains("new selector"),
        "the screen's own chips give way while the modal is open: {content}"
    );

    // On a removed row the same chord restores — the chip says so.
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, alt('d'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    let content = rendered_text(&mut app);
    assert!(content.contains("restore field"), "{content}");
}

/// The quit chip's keycap tells the truth: `q` only where a plain `q`
/// actually quits (Main screen, non-typing pane, no modal); everywhere
/// typing owns plain keys — a modal open, the editor's text areas, the
/// Variable Manager — it shows the pre-empting `^C` combo instead.
#[test]
fn the_quit_chip_shows_ctrl_c_wherever_plain_q_would_type() {
    let (mut app, _dir) = sidebar_test_app();
    app.focus = PaneId::Sidebar;
    let content = rendered_text(&mut app);
    assert!(content.contains("q  quit"), "{content}");

    // The editor pane: only its text inputs eat plain keys. On the tab
    // strip (or the method badge, or a selected table row) `q` still
    // quits, so the chip keeps saying `q`.
    app.focus = PaneId::Editor;
    app.editor.sub_focus = crate::components::editor::SubFocus::Tabs;
    let content = rendered_text(&mut app);
    assert!(content.contains("q  quit"), "{content}");
    app.editor.sub_focus = crate::components::editor::SubFocus::Content;
    app.editor.active_tab = crate::components::editor::EditorTab::Headers;
    app.editor.table.selected = Some(0);
    let content = rendered_text(&mut app);
    assert!(content.contains("q  quit"), "{content}");
    // The URL line and the body editor type it.
    app.editor.sub_focus = crate::components::editor::SubFocus::Url;
    let content = rendered_text(&mut app);
    assert!(content.contains("^C  quit"), "{content}");
    assert!(!content.contains("q  quit"), "{content}");
    app.editor.sub_focus = crate::components::editor::SubFocus::Content;
    app.editor.active_tab = crate::components::editor::EditorTab::Body;
    let content = rendered_text(&mut app);
    assert!(content.contains("^C  quit"), "{content}");

    // A modal captures everything; only the modified combo quits.
    app.focus = PaneId::Sidebar;
    app.update(Action::PromptNewRequest);
    let content = rendered_text(&mut app);
    assert!(content.contains("^C  quit"), "{content}");
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );

    // The manager binds plain q to quit in every focus stop, so the chip
    // advertises it there.
    app.update(Action::OpenManage { tab: None });
    let content = rendered_text(&mut app);
    assert!(content.contains("q  quit"), "{content}");
}

/// While any screen-owning modal is open, the footer shows that modal's
/// live keys and the scrim stops above it — a dimmed toolbar reads as
/// inactive, and these chips are exactly the keys that DO work.
#[test]
fn the_scrim_leaves_the_footer_bright_under_every_modal() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let (mut app, _dir) = fields_editor_app();

    let footer_bg = |app: &mut App| {
        app.anims.finish_all();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        // (1, 22): the footer's content row; (40, 11): mid-screen backdrop.
        (
            buf.cell((1, 22)).unwrap().bg,
            buf.cell((40, 11)).unwrap().bg,
        )
    };

    let panel = app.theme.panel;
    let (footer, _) = footer_bg(&mut app);
    assert_eq!(footer, panel, "fields editor: the footer stays undimmed");

    // A plain message modal — no chords of its own — still keeps the
    // footer bright, showing its enter/esc keys.
    app.modals.pop();
    let (_, backdrop_before) = footer_bg(&mut app);
    app.push_modal(Modal::Message {
        title: "note".into(),
        body: "hello".into(),
    });
    let (footer, backdrop) = footer_bg(&mut app);
    assert_eq!(footer, panel, "message modal: the footer stays undimmed");
    assert_ne!(
        backdrop, backdrop_before,
        "the scrim still dims the screen above the footer"
    );
    let content = rendered_text(&mut app);
    assert!(content.contains("close"), "{content}");
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

/// Clicking bare background — the pane under the last option row registers
/// no hit of its own — is a click away like any other: it saves the cell,
/// rather than leaving typed text live with nothing to say it isn't stored.
#[test]
fn clicking_bare_background_commits_the_cell_under_edit() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    let r = cell_rect(&mut app, 0, 1);
    app.handle_mouse(left_down(r.x + 10, r.y)); // caret past the text, at its end
    assert!(app.varmanager.grid.editing.is_some(), "the cell is live");
    app.handle_key(&Keymap::default_bindings(), plain('9'));

    // The lowest row of the detail pane that no control claims.
    let (x, y) = (0..46)
        .rev()
        .flat_map(|y| (0..100).map(move |x| (x, y)))
        .find(|&(x, y)| app.hits.hit_at(x, y).is_none())
        .expect("some bare background");
    assert!(app.handle_mouse(left_down(x, y)), "the click redraws");

    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert!(app.varmanager.grid.editing.is_none(), "the edit is done");
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(on_disk.contains("10019"), "{on_disk}");
}

#[test]
fn editing_a_field_cell_and_clicking_away_rewrites_the_env_file() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    // Mid-cell clicks: the caret follows the pointer, and a click past the
    // short value text still lands the caret at its end.
    let r = cell_rect(&mut app, 0, 1);
    app.handle_mouse(left_down(r.x + 10, r.y));
    assert!(app.varmanager.grid.editing.is_some(), "the cell is live");

    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, plain('9'));

    // Clicking a *different* cell commits the first one (Task 8's
    // commit-first rule) and starts editing the one clicked.
    let other = cell_rect(&mut app, 1, 1);
    app.handle_mouse(left_down(other.x + 10, other.y));

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

/// User finding: there was no button for deleting an option — only the `d`
/// key and the right-click menu. Each option row gets an explicit `🗑`
/// (the table editor's row-trash twin) running the same immediate delete.
#[test]
fn clicking_an_option_rows_trash_deletes_the_option() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    rendered_text_tall(&mut app);
    let r = app
        .hits
        .rect_of(&crate::hit::Hit::VmEntryDelete(0))
        .expect("each option row registers a trash zone");
    app.handle_mouse(left_down(r.x + 1, r.y));
    assert!(app.modals.is_empty(), "delete is undoable, no confirm");
    let env = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert!(!env.options["user"].contains_key("alice"));
}

#[test]
fn grid_cell_click_places_the_caret_and_drag_sweeps_a_selection() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    let r = cell_rect(&mut app, 0, 1);
    app.handle_mouse(left_down(r.x, r.y));
    let edit = app.varmanager.grid.editing.as_ref().expect("editing");
    assert_eq!(edit.input.text(), "1001");
    assert_eq!(edit.input.cursor(), 0, "the caret follows the pointer");

    assert!(app.handle_mouse(dragged(r.x + 3, r.y)));
    let edit = app.varmanager.grid.editing.as_ref().unwrap();
    assert_eq!(edit.input.selected_text().as_deref(), Some("100"));
    // The sweep keeps going outside the cell's rect (drag-out-of-the-box).
    assert!(app.handle_mouse(dragged(r.x + r.width + 5, r.y + 2)));
    let edit = app.varmanager.grid.editing.as_ref().unwrap();
    assert_eq!(edit.input.selected_text().as_deref(), Some("1001"));
    app.handle_mouse(left_up(r.x + r.width + 5, r.y + 2));
    assert!(app.text_drag.is_none(), "release ends the sweep");
}

#[test]
fn form_field_double_click_selects_the_word_and_drag_sweeps() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    // Text starts 2 columns into the field ("API root"); a double click on
    // its first cell selects the word under it.
    let r = field_rect(&mut app, VmField::Description);
    app.handle_mouse(left_down(r.x + 2, r.y + 1));
    app.handle_mouse(left_down(r.x + 2, r.y + 1)); // within 400ms => clicks == 2
    let (_, input) = app.varmanager.form.editing.as_ref().expect("editing");
    assert_eq!(input.selected_text().as_deref(), Some("API"));

    // Dragging on from the double click extends the selection word by
    // word — onto "root" grows it to the whole phrase, back onto the
    // anchored word shrinks it again (the body editor's word sweep).
    assert!(app.handle_mouse(dragged(r.x + 2 + 6, r.y + 1)));
    let (_, input) = app.varmanager.form.editing.as_ref().unwrap();
    assert_eq!(input.selected_text().as_deref(), Some("API root"));
    assert!(app.handle_mouse(dragged(r.x + 2 + 1, r.y + 1)));
    let (_, input) = app.varmanager.form.editing.as_ref().unwrap();
    assert_eq!(input.selected_text().as_deref(), Some("API"));
    app.handle_mouse(left_up(r.x + 2 + 1, r.y + 1));

    // A fresh click collapses the selection; a drag sweeps a new one.
    app.last_click = None;
    app.handle_mouse(left_down(r.x + 2, r.y + 1));
    let (_, input) = app.varmanager.form.editing.as_ref().unwrap();
    assert_eq!(input.selection(), None);
    assert!(app.handle_mouse(dragged(r.x + 2 + 8, r.y + 1)));
    let (_, input) = app.varmanager.form.editing.as_ref().unwrap();
    assert_eq!(input.selected_text().as_deref(), Some("API root"));
}

/// The Variable Manager screen's footer advertises its own verbs (the main
/// screen's per-pane chips act on requests, which aren't on screen there);
/// with the grid focused, `d` is the clickable delete-option chip the user
/// asked for.
#[test]
fn vm_footer_advertises_the_option_verbs_while_the_grid_has_focus() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.varmanager.focus, VmFocus::Grid);

    let delete = Action::DeleteEntry {
        env: "qa".into(),
        selector: "user".into(),
        name: "alice".into(),
    };
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        chips
            .iter()
            .any(|(k, l, a)| *k == "d" && *l == "delete" && a.as_ref() == Some(&delete)),
        "{chips:?}"
    );
    // Every grid chip is clickable here — "new option" included (it drives
    // the same ghost-row edit the `o` key does).
    assert!(
        chips.iter().all(|(_, _, a)| a.is_some()),
        "no plain-hint chips with a real row under the cursor: {chips:?}"
    );
    // And the drawn footer registers it as a clickable chip.
    rendered_text_tall(&mut app);
    assert!(
        app.hits
            .rect_of(&crate::hit::Hit::FooterChip(delete))
            .is_some(),
        "the chip is click-registered"
    );
}

/// The variable form is a keyboard area like the options grid: `Right`
/// from the list enters it with a field cursor, arrows move over the
/// fields, `Enter` edits in place, and `Esc` steps back out to the list
/// — the form is no longer reachable only by clicking.
#[test]
fn keyboard_enters_the_variable_form_and_edits_its_fields() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });

    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.varmanager.focus, VmFocus::Form, "Right enters the form");

    // Description first; Down to the default; Enter starts the in-place
    // edit clicking the field would.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let (field, input) = app.varmanager.form.editing.as_ref().expect("editing");
    assert_eq!(*field, crate::components::varmanager::VmField::Default);
    assert_eq!(input.text(), "http://localhost:8080");

    app.handle_key(&keymap, plain('9'));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.varmanager.form.editing.is_none(), "Enter commits");
    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(on_disk.contains("http://localhost:80809"), "{on_disk}");

    // Esc leaves the form for the list; one more Esc would close the
    // screen, same leave-the-inner-thing-first rhythm as the grid.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.varmanager.focus, VmFocus::List);
    assert_eq!(app.screen, Screen::Manage, "the screen stays open");
}

/// With the form focused, the footer advertises the form's own quick
/// actions and the keys work: `s` flips the secret flag, and — with the
/// cursor on the env-value field, while the env stores one — `x` clears
/// the stored value, the inline "✕ remove" control's keyboard twin.
#[test]
fn form_focus_advertises_and_handles_the_field_verbs() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    // The open request overrides base_url, so promote applies too.
    request_with_var(dir.path(), "main/ping", "base_url", "http://req.local");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.varmanager.focus, VmFocus::Form);

    let secret = Action::ToggleSecretVar {
        name: "base_url".into(),
    };
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        chips
            .iter()
            .any(|(k, l, a)| *k == "s" && *l == "secret" && a.as_ref() == Some(&secret)),
        "{chips:?}"
    );
    assert!(
        !chips.iter().any(|(_, l, _)| *l == "clear env value"),
        "off the env-value field, no clear chip: {chips:?}"
    );

    // Down to the env-value field: qa stores one, so `x` clears it.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let clear = Action::RemoveVarValue {
        name: "base_url".into(),
        destination: crate::action::ExtractDestination::ActiveEnv,
    };
    assert_eq!(
        app.varmanager.form_cursor,
        crate::components::varmanager::VmField::EnvValue,
        "two downs land on the env-value field"
    );
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        chips
            .iter()
            .any(|(k, l, a)| *k == "x" && *l == "clear env value" && a.as_ref() == Some(&clear)),
        "{chips:?}"
    );
    app.handle_key(&keymap, plain('x'));
    let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!on_disk.contains("base_url"), "{on_disk}");
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        !chips.iter().any(|(_, l, _)| *l == "clear env value"),
        "nothing stored, nothing to clear: {chips:?}"
    );

    // The open request overrides this name, so promote is on offer, and
    // `p` opens the same promote prompt the button does.
    let promote = Action::PromptPromoteVar {
        name: "base_url".into(),
    };
    let open_request = app.editor.current_request();
    let chips = app
        .varmanager
        .footer_chips(&app.project, Some(&open_request));
    assert!(
        chips
            .iter()
            .any(|(k, l, a)| *k == "p" && *l == "promote" && a.as_ref() == Some(&promote)),
        "{chips:?}"
    );
    app.handle_key(&keymap, plain('p'));
    assert!(!app.modals.is_empty(), "p opens the promote prompt");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    // `s` works from the form area, not just the list — through the same
    // make-secret confirm the list's `s` opens.
    app.handle_key(&keymap, plain('s'));
    assert!(!app.modals.is_empty(), "s opens the secret confirm");
    app.handle_key(&keymap, plain('y'));
    assert!(
        app.project.model.vars["base_url"].secret,
        "s flips the secret flag from the form area"
    );
}

/// On a secret variable, `r` in the form area flips the reveal toggle —
/// the 👁 control's keyboard twin — and the footer hint tracks its state.
#[test]
fn form_focus_r_toggles_reveal_on_a_secret_variable() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("api_key".into())
    });
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        chips.iter().any(|(k, l, _)| *k == "r" && *l == "reveal"),
        "{chips:?}"
    );
    app.handle_key(&keymap, plain('r'));
    assert!(app.varmanager.form.revealed, "r reveals the secret");
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        chips.iter().any(|(k, l, _)| *k == "r" && *l == "hide"),
        "{chips:?}"
    );
    app.handle_key(&keymap, plain('r'));
    assert!(!app.varmanager.form.revealed);
}

/// With a selector open in the detail pane, the footer advertises `m`
/// "edit fields" — the key existed but nothing taught it, which left the
/// fields editor mouse-only in practice.
#[test]
fn vm_footer_advertises_edit_fields_on_a_selector() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    let edit_fields = Action::PromptGroupFields {
        selector: "user".into(),
    };
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        chips
            .iter()
            .any(|(k, l, a)| *k == "m" && *l == "edit fields" && a.as_ref() == Some(&edit_fields)),
        "{chips:?}"
    );

    // On a plain variable there is no fields editor to advertise.
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        !chips.iter().any(|(_, l, _)| *l == "edit fields"),
        "{chips:?}"
    );
}

/// User finding: with a variable's form open but the left cursor on a
/// section header, the rename/delete chips rendered as plain hints. They
/// fall back to the open detail's name so they stay clickable.
#[test]
fn vm_footer_rename_delete_act_on_the_open_form_from_a_header_row() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_row(&mut app, |r| {
        r == &crate::components::varmanager::VmRow::Var("base_url".into())
    });
    app.varmanager.left_cursor = 0; // the "VARIABLES" section header
    let chips = app.varmanager.footer_chips(&app.project, None);
    assert!(
        chips.iter().any(|(k, _, a)| *k == "e"
            && *a
                == Some(Action::PromptRenameVar {
                    from: "base_url".into()
                })),
        "{chips:?}"
    );
    assert!(
        chips.iter().any(|(k, _, a)| *k == "d"
            && *a
                == Some(Action::DeleteVar {
                    name: "base_url".into()
                })),
        "{chips:?}"
    );
}

/// User finding: chips with no target rendered as dead, unclickable text.
/// They're dropped instead — with nothing open, only new-variable/-selector
/// show; on the ghost row, only the new-option chip.
#[test]
fn vm_footer_drops_chips_with_no_target() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    // Fresh open: nothing selected, no detail — no rename/delete chips.
    app.update(Action::OpenManage { tab: None });
    let keys: Vec<&str> = app
        .varmanager
        .footer_chips(&app.project, None)
        .iter()
        .map(|(k, _, _)| *k)
        .collect();
    assert_eq!(keys, vec!["n", "g"]);

    // Grid focus with the cursor on the ghost row: only "new option".
    goto_group(&mut app, "user");
    app.varmanager.focus = VmFocus::Grid;
    app.varmanager.grid.cursor = (2, 0); // alice, bob, then the ghost
    let keys: Vec<&str> = app
        .varmanager
        .footer_chips(&app.project, None)
        .iter()
        .map(|(k, _, _)| *k)
        .collect();
    assert_eq!(keys, vec!["o"]);
}

/// The option-row "rename" is the inline name-cell edit — the `e` key and
/// the context menu's "Rename" both open it, and committing the changed
/// name renames on disk. No modal anywhere.
#[test]
fn option_rename_is_the_inline_name_cell_edit() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    let keymap = Keymap::default_bindings();

    // The context menu's "Rename" seeds the name cell in place.
    app.update(Action::StartOptionNameEdit { row: 1 });
    assert!(app.modals.is_empty(), "no rename modal");
    let edit = app.varmanager.grid.editing.as_ref().expect("inline edit");
    assert_eq!((edit.row, edit.col), (1, 0));
    assert_eq!(edit.input.text(), "bob");

    // Committing a changed name IS the rename.
    type_chars(&mut app, "by");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let qa = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(qa.contains("[options.user.bobby]"), "{qa}");
    assert!(!qa.contains("[options.user.bob]\n"), "{qa}");

    // The grid's `F2` opens the same inline edit on the cursor row (`e` is
    // the full Edit prompt now).
    app.varmanager.focus = VmFocus::Grid;
    app.varmanager.grid.cursor = (0, 1);
    app.handle_key(&keymap, KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert!(app.modals.is_empty(), "no rename modal");
    let edit = app.varmanager.grid.editing.as_ref().expect("inline edit");
    assert_eq!((edit.row, edit.col), (0, 0), "F2 targets the name cell");
    assert_eq!(edit.input.text(), "alice");
}

#[test]
fn start_new_option_edit_action_opens_the_ghost_rows_name_cell() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");
    app.update(Action::StartNewOptionEdit);
    let edit = app.varmanager.grid.editing.as_ref().expect("ghost edit");
    assert_eq!((edit.row, edit.col), (2, 0), "alice, bob, then the ghost");
    assert_eq!(edit.input.text(), "");
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
    assert!(on_disk.contains("[options.user.carol]"), "{on_disk}");
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
    assert_eq!(env.options["user"]["carol"].values["user"], "3003");
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
        "[options.user.dave]\nuser = \"7\"\n",
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    goto_group(&mut app, "user");

    // --- rename: slot 0 is the group's current field, retyped -----------
    app.update(Action::ApplyGroupFields {
        selector: "user".into(),
        slots: vec!["user_id".into()],
    });
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(app.project.model.selectors["user"].fields, vec!["user_id"]);
    let qa = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert_eq!(qa.options["user"]["alice"].values["user_id"], "1001");
    let dev = postui_core::project::load_environment(dir.path(), "dev").unwrap();
    assert_eq!(
        dev.options["user"]["dave"].values["user_id"], "7",
        "a non-active environment renames too"
    );

    // --- add: a slot past the current list -------------------------------
    app.update(Action::ApplyGroupFields {
        selector: "user".into(),
        slots: vec!["user_id".into(), "customer_id".into()],
    });
    assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(
        app.project.model.selectors["user"].fields,
        vec!["user_id", "customer_id"]
    );
    let qa = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert_eq!(
        qa.options["user"]["alice"].values["customer_id"], "",
        "every existing entry gains the column, empty"
    );

    // --- remove: a cleared slot deletes the column at once (undoable) ----
    app.update(Action::ApplyGroupFields {
        selector: "user".into(),
        slots: vec!["user_id".into(), String::new()],
    });
    assert!(app.modals.is_empty(), "removal is undoable, no confirm");
    assert_eq!(app.project.model.selectors["user"].fields, vec!["user_id"]);
    let qa = postui_core::project::load_environment(dir.path(), "qa").unwrap();
    assert!(
        !qa.options["user"]["alice"]
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
        "[options.user.dave]\nuser = \"7\"\n",
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
    assert!(vars.contains("[selectors.account]"), "{vars}");
    assert!(!vars.contains("[selectors.user]"), "{vars}");
    for env in ["qa", "dev"] {
        let text =
            std::fs::read_to_string(dir.path().join(format!("environments/{env}.toml"))).unwrap();
        assert!(
            text.contains("[options.account."),
            "{env} entries moved: {text}"
        );
        assert!(!text.contains("[options.user."), "{env}: {text}");
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
    assert!(app.project.model.selectors.contains_key("user"));
    let qa = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(qa.contains("[options.user.alice]"), "{qa}");
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
    // "Rename" has no ellipsis: it starts the inline name-cell edit.
    assert_eq!(
        labels,
        vec!["Edit\u{2026}", "Duplicate option", "Rename", "Delete"]
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

    // Type into bob's (row 1) value cell… (mid-cell click: the caret
    // follows the pointer, and the '9' must land at the end)
    let r = cell_rect(&mut app, 1, 1);
    app.handle_mouse(left_down(r.x + 10, r.y));
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
        env.options["user"]["bob"].values["user"], "20029",
        "the text landed in the entry it was typed into"
    );
    assert_eq!(
        env.options["user"]["alice"].values["user"], "1001",
        "the right-clicked entry is untouched"
    );
    // …and the menu is the one for the row that was right-clicked.
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("no entry menu");
    };
    assert_eq!(
        state.items[3].action,
        Some(Action::DeleteEntry {
            env: "qa".into(),
            selector: "user".into(),
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

    // Right-edge click: the caret follows the pointer, and the '9' must
    // land at the end of the text.
    let r = field_rect(&mut app, VmField::EnvValue);
    app.handle_mouse(left_down(r.x + r.width - 2, r.y + 1));
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
    assert_eq!(app.screen, Screen::Manage);
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
    let r = app.hits.rect_of(&crate::hit::Hit::VmNewOption).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    let edit = app.varmanager.grid.editing.as_ref().expect("ghost is live");
    assert_eq!((edit.row, edit.col), (2, 0));

    rendered_text_tall(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::VmEditFields).unwrap();
    app.handle_mouse(left_down(r.x + 1, r.y + 1));
    // One editable row per current field.
    let Some(Modal::FieldsEditor(fe)) = app.modals.top() else {
        panic!("expected the fields editor")
    };
    assert_eq!(fe.rows.len(), 1);
    assert_eq!(fe.rows[0].input.text(), "user");
}

/// Every button inside the request panel focuses it on click — send,
/// copy-URL, the TLS lock, the split control and its alt+w pill, and the
/// Body toolbar chips — not just its text surfaces (URL bar, tabs, tables).
#[tokio::test]
async fn request_panel_buttons_focus_the_editor_pane() {
    let mut app = App::new_for_test();
    app.editor.method = postui_core::model::Method::Post;
    app.editor.url = crate::components::line_input::LineInput::new("http://127.0.0.1:9");
    app.update(Action::EditorTabSelect(EditorTab::Body.index()));
    let buttons = [
        Hit::SendButton,
        Hit::CopyUrl,
        Hit::SplitStop(crate::split::SplitStop::Even),
        Hit::FooterChip(Action::ToggleInsecure),
        Hit::FooterChip(Action::FormatBody),
        Hit::FooterChip(Action::CycleSplit),
    ];
    for hit in buttons {
        app.update(Action::FocusPane(PaneId::Response));
        render_once(&mut app);
        let r = app
            .hits
            .rect_of(&hit)
            .unwrap_or_else(|| panic!("{hit:?} not on screen"));
        app.handle_mouse(left_down(r.x, r.y));
        assert_eq!(
            app.focus,
            PaneId::Editor,
            "clicking {hit:?} must focus the request panel"
        );
    }
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
/// fails, the fix is a mouse path, not a new entry there. The one other
/// carve-out is `space_actions_pending_mouse_path`, a deliberately
/// temporary list (see its own comment) for actions declared ahead of the
/// UI that will dispatch them by mouse.
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
        // shift+alt+w walks the split stops backward; every stop is also a
        // directly clickable chip on the split control (`Hit::SplitStop`),
        // and the forward cycle's alt+w pill is the strip's mouse path.
        Action::CycleSplitBack,
    ];

    // Task 7 (stage: spaces + Manage screen) declares the space actions
    // and their key bindings ahead of the UI that dispatches them by
    // mouse (the space chooser and switcher land in Task 8/10/11). Unlike
    // `keyboard_only_navigation` above, these have no click path at all
    // yet — this list exists only to bridge that gap and should empty out
    // as those tasks land their footer chip / chooser rows / on_hit
    // dispatch. What is left after Task 14 (the Manage screen's
    // Environments/Spaces face, whose `+ New` / `Delete` / `Move` buttons
    // and footer chips cover the manage-side space actions) is the
    // backward cycle and the numbered jumps: every space they reach is
    // also one click away in the header's numbered space dropdown, but
    // that dropdown's rows dispatch `SwitchSpace`, so these named actions
    // themselves still have no click path.
    let space_actions_pending_mouse_path: Vec<Action> = vec![
        // `OpenSpaceChooser` is off this list as of Task 10: the header's
        // space chip opens the chooser by click. `CycleSpace(1)` came off
        // in Task 8 (the sidebar footer's `alt+]` chip), and Task 10's
        // header cycle pill dispatches it too.
        Action::CycleSpace(-1),
        Action::JumpSpace(1),
        Action::JumpSpace(2),
        Action::JumpSpace(3),
        Action::JumpSpace(4),
        Action::JumpSpace(5),
        Action::JumpSpace(6),
        Action::JumpSpace(7),
        Action::JumpSpace(8),
        Action::JumpSpace(9),
    ];

    // Group A: footer/toolbar chips — the same function `draw_footer`
    // paints from. The always-present quit hint and palette chip are
    // registered separately in `draw_footer` itself (`QUIT_LABEL` /
    // `PALETTE_CHIP`, not part of `footer_chips`), so they're added by
    // hand here.
    let mut mouse_reachable: Vec<Action> = vec![Action::Quit, Action::OpenPalette];
    for pane in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
        mouse_reachable.extend(
            crate::components::footer::footer_chips(
                pane,
                false,
                false,
                Some("add header"),
                false,
                None,
                false,
            )
            .into_iter()
            .filter_map(|(_, _, a)| a),
        );
        // The address-bar variant swaps in chips of its own (copy url,
        // tls verify) — enumerate those too.
        mouse_reachable.extend(
            crate::components::footer::footer_chips(
                pane,
                false,
                false,
                Some("add header"),
                true,
                None,
                false,
            )
            .into_iter()
            .filter_map(|(_, _, a)| a),
        );
        // The response pane's jq-bar-focused chip set swaps in its own
        // actions (done/tree/describe) — enumerate those too.
        mouse_reachable.extend(
            crate::components::footer::footer_chips(
                pane,
                false,
                false,
                Some("add header"),
                false,
                None,
                true,
            )
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
    postui_core::storage::save_request(&app.project.root, "main/req", &req("https://x/req"))
        .unwrap();
    app.refresh_sidebar();
    let row = app
        .sidebar
        .rows
        .iter()
        .position(|r| matches!(r, Row::Request { slug, .. } if slug == "main/req"))
        .expect("the saved request is in the sidebar tree");
    mouse_reachable.extend(
        app.context_menu_for(&Hit::SidebarRow(row))
            .into_iter()
            .flatten()
            .filter_map(|item| item.action),
    );

    for (name, action) in crate::keys::named_actions() {
        assert!(
            keyboard_only_navigation.contains(&action)
                || space_actions_pending_mouse_path.contains(&action)
                || mouse_reachable.contains(&action),
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
/// `Screen::Manage` right out from under the showcase, with no way back
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

    click_hit(&mut app, Hit::HeaderManage);
    assert_eq!(
        app.screen,
        crate::app::Screen::Testbed,
        "a header-chip click must not navigate into the Manage screen"
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

/// `Action::SplitStop` (the split control's five chips) drives the
/// five-state split: a ratio chip moves the stop and eases
/// `AnimKey::SplitRatio`; the endpoint chips set the same flags the old
/// toggles did, keeping the ratio sticky underneath.
#[test]
fn split_stops_update_the_split_and_ease_the_ratio_anim() {
    use crate::split::{SplitRatio, SplitStop};
    let mut app = App::new_for_test_with_anims(true);
    assert_eq!(app.split_ratio, SplitRatio::Even);

    // 75/25 from 50/50 shrinks the response to its quarter, easing the
    // ratio rather than snapping.
    app.update(Action::SplitStop(SplitStop::EditorBig));
    assert_eq!(app.split_ratio, SplitRatio::EditorBig);
    assert!(!app.session.response.collapsed);
    assert!(!app.table_collapsed);
    let now = std::time::Instant::now();
    assert!(
        !app.anims.is_done(crate::anim::AnimKey::SplitRatio, now),
        "the ratio must ease between stops, not snap"
    );

    // The editor-full chip collapses the response exactly like the old
    // toggle; the ratio stop survives underneath.
    app.update(Action::SplitStop(SplitStop::EditorFull));
    assert!(app.session.response.collapsed);
    assert_eq!(app.split_ratio, SplitRatio::EditorBig, "ratio is sticky");

    // The response-full chip gives the response the whole column by
    // minimizing the editor — one click from the opposite endpoint.
    app.update(Action::SplitStop(SplitStop::ResponseFull));
    assert!(app.table_collapsed);
    assert!(!app.session.response.collapsed);

    // A ratio chip reopens both panes straight at its stop.
    app.update(Action::SplitStop(SplitStop::ResponseBig));
    assert!(!app.table_collapsed);
    assert_eq!(app.split_ratio, SplitRatio::ResponseBig);
}

/// `Action::CycleSplit` (the footer's `alt+s` context chip) steps the
/// split through the five stops in on-screen order and wraps — the
/// one-key keyboard route to every state the control's chips reach.
#[test]
fn cycle_split_steps_through_every_stop_and_wraps() {
    use crate::split::SplitStop::*;
    let mut app = App::new_for_test();
    assert_eq!(app.split_state().stop(), Even);
    let mut seen = vec![];
    for _ in 0..5 {
        app.update(Action::CycleSplit);
        seen.push(app.split_state().stop());
    }
    assert_eq!(
        seen,
        [ResponseBig, ResponseFull, EditorFull, EditorBig, Even]
    );
}

/// `Action::SplitStep` (the response header's ▲/▼ buttons) nudges the
/// split exactly one stop per press and stalls at the endpoints rather
/// than wrapping — the button at the edge is a no-op, not a jump across
/// the column.
#[test]
fn split_step_moves_one_stop_and_stalls_at_the_ends() {
    use crate::split::SplitStop::*;
    let mut app = App::new_for_test();
    assert_eq!(app.split_state().stop(), Even);
    app.update(Action::SplitStep(1));
    assert_eq!(
        app.split_state().stop(),
        ResponseBig,
        "▲ grows the response"
    );
    app.update(Action::SplitStep(1));
    assert_eq!(app.split_state().stop(), ResponseFull);
    assert!(app.table_collapsed);
    app.update(Action::SplitStep(1));
    assert_eq!(
        app.split_state().stop(),
        ResponseFull,
        "stalls at the bottom stop"
    );
    for _ in 0..4 {
        app.update(Action::SplitStep(-1));
    }
    assert_eq!(
        app.split_state().stop(),
        EditorFull,
        "▼ shrinks the response"
    );
    assert!(app.session.response.collapsed);
    app.update(Action::SplitStep(-1));
    assert_eq!(
        app.split_state().stop(),
        EditorFull,
        "stalls at the top stop"
    );
}

/// The split is a persisted layout preference: chip presses and the
/// keyboard toggles record it in the project's `.local/state.toml`, and
/// opening the project seeds the split back from it.
#[test]
fn split_persists_to_local_state_and_reseeds_on_open() {
    use crate::split::SplitStop;
    let mut app = App::new_for_test();
    app.update(Action::SplitStop(SplitStop::EditorBig));
    let root = app.project.root.clone();
    let saved = |root: &std::path::Path| {
        postui_core::project::load_local_state(root)
            .unwrap()
            .main_split
    };
    assert_eq!(saved(&root).as_deref(), Some("editor-big"));

    app.update(Action::ToggleResponseCollapse);
    assert_eq!(saved(&root).as_deref(), Some("editor-full"));

    // A fresh app on the same root wakes up with the saved split.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let reopened = App::with_root(tx, root.clone());
    assert!(
        reopened.session.response.collapsed,
        "the minimized response comes back minimized"
    );
    drop(app); // keeps the temp project alive until the reopened app is done
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

// --- Task 9: the Manage screen's tabbed shell --------------------------

#[test]
fn manage_opens_on_the_requested_tab_and_alt_arrows_cycle_tabs() {
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenManage {
        tab: Some(crate::components::manage::ManageTab::Spaces),
    });
    assert_eq!(app.screen, Screen::Manage);
    assert_eq!(app.manage.tab, crate::components::manage::ManageTab::Spaces);
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(
        app.manage.tab,
        crate::components::manage::ManageTab::Variables,
        "wraps"
    );
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.manage.tab, crate::components::manage::ManageTab::Spaces);
    app.update(Action::OpenManage { tab: None });
    assert_eq!(app.screen, Screen::Main, "alt+v toggles closed");
    app.update(Action::OpenManage { tab: None });
    assert_eq!(
        app.manage.tab,
        crate::components::manage::ManageTab::Spaces,
        "reopens on the last tab"
    );
}

#[test]
fn manage_bar_paints_three_tabs_and_clicking_one_selects_it() {
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenManage { tab: None });
    let text = rendered_text(&mut app);
    for label in ["Variables", "Environments", "Spaces"] {
        assert!(text.contains(label), "{label} missing: {text}");
    }
    click_hit(&mut app, Hit::ManageTab(1));
    assert_eq!(
        app.manage.tab,
        crate::components::manage::ManageTab::Environments
    );
}

#[test]
fn header_chip_reads_manage_and_toggles_the_screen() {
    let (mut app, _dir) = spaced_app();
    let text = rendered_text(&mut app);
    assert!(text.contains(" Manage "), "{text}");
    assert!(!text.contains("Variable Manager"));
    click_hit(&mut app, Hit::HeaderManage);
    assert_eq!(app.screen, Screen::Manage);
    click_hit(&mut app, Hit::HeaderManage);
    assert_eq!(app.screen, Screen::Main);
}

// --- Task 10: header space chip, cycle pill, space/env dropdown rows ----

#[test]
fn header_shows_the_space_chip_between_env_and_manage() {
    let (mut app, _dir) = spaced_app();
    render_once(&mut app);
    let space = app.hits.rect_of(&Hit::HeaderSpace).expect("space chip");
    let env = app.hits.rect_of(&Hit::HeaderEnv).expect("env chip");
    let manage = app.hits.rect_of(&Hit::HeaderManage).expect("manage chip");
    assert!(env.x < space.x && space.x < manage.x);
    let text = rendered_text(&mut app);
    assert!(text.contains("Space: main"), "{text}");
    click_hit(&mut app, Hit::HeaderSpaceCycle);
    assert_eq!(app.project.active_space, "auth");
}

/// At 80 columns — the common terminal width — the header's cycle pills
/// yield so the Manage chip (the only mouse path to the Manage screen)
/// still fits. The chips themselves keep their full labels.
#[test]
fn header_cycle_pills_yield_at_eighty_columns_so_the_manage_chip_fits() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let (mut app, _dir) = spaced_app();
    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    for hit in [Hit::HeaderProject, Hit::HeaderSpace, Hit::HeaderEnv] {
        assert!(
            app.hits.rect_of(&hit).is_some(),
            "{hit:?} must still be on the bar at 80 columns"
        );
    }
    let manage = app
        .hits
        .rect_of(&Hit::HeaderManage)
        .expect("the Manage chip must be on the bar at 80 columns");
    // Whatever of the chip survives the yield order (here the fixture's
    // 10-character tempdir project name also costs it the `alt+v` keycap),
    // the whole painted chip lies within the bar.
    assert!(
        manage.x + manage.width <= 80,
        "the Manage chip must not run off an 80-column bar: {manage:?}"
    );
    assert!(
        rendered_text(&mut app).contains(" Manage "),
        "and its name must actually be painted there"
    );
    assert!(
        app.hits.rect_of(&Hit::HeaderSpaceCycle).is_none(),
        "the space cycle pill yields first"
    );
    assert!(
        app.hits.rect_of(&Hit::HeaderEnvCycle).is_none(),
        "so does the env cycle pill"
    );

    // The 120-column path keeps both pills.
    render_once(&mut app);
    assert!(app.hits.rect_of(&Hit::HeaderSpaceCycle).is_some());
    assert!(app.hits.rect_of(&Hit::HeaderEnvCycle).is_some());
}

#[test]
fn space_dropdown_lists_numbered_spaces_with_new_and_manage_rows() {
    let (mut app, _dir) = spaced_app();
    render_once(&mut app);
    app.update(Action::OpenSpaceChooser);
    let Some(Modal::Dropdown(d)) = app.modals.top() else {
        panic!("dropdown")
    };
    let labels: Vec<&str> = d.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["1  main", "2  auth", "new space…", "manage spaces…"]
    );
    assert_eq!(d.current, Some(0));
    assert_eq!(d.items[1].action, Some(Action::SwitchSpace("auth".into())));
    assert_eq!(
        d.items[3].action,
        Some(Action::OpenManage {
            tab: Some(crate::components::manage::ManageTab::Spaces)
        })
    );
}

#[test]
fn env_dropdown_gains_a_manage_row() {
    let (mut app, _dir) = app_with_envs();
    render_once(&mut app);
    app.update(Action::OpenEnvChooser);
    let Some(Modal::Dropdown(d)) = app.modals.top() else {
        panic!("dropdown")
    };
    let last = d.items.last().unwrap();
    assert_eq!(last.label, "manage environments…");
    assert_eq!(
        last.action,
        Some(Action::OpenManage {
            tab: Some(crate::components::manage::ManageTab::Environments)
        })
    );
}

// --- Task 14: the Environments / Spaces tabs — list-edit face -------------

#[test]
fn spaces_tab_lists_numbered_spaces_with_request_names_and_buttons() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Spaces),
    });
    let text = rendered_text_wide_tall(&mut app);
    assert!(text.contains("1  main"), "{text}");
    assert!(text.contains("2  auth"), "{text}");
    assert!(text.contains("Space: main"), "{text}");
    assert!(
        !text.contains("\u{2713}"),
        "no check mark beside the active space: {text}"
    );
    // The request names themselves, not a count.
    assert!(!text.contains("2 requests"), "{text}");
    let alpha = text.find("alpha").expect("alpha listed");
    let beta = text.find("beta").expect("beta listed");
    assert!(alpha < beta, "in sidebar order");
    assert!(!text.contains("login"), "auth's request is not main's");
    for hit in [
        Hit::ManageNew,
        Hit::ManageRename,
        Hit::ManageDelete,
        Hit::ManageMoveUp,
        Hit::ManageMoveDown,
        Hit::ManageMoveAll,
    ] {
        assert!(app.hits.rect_of(&hit).is_some(), "{hit:?} missing");
    }
}

/// The Environments/Spaces panes share the Variables pane's title-row
/// layout: the item's name at the left, the buttons right-aligned on the
/// same row with Delete at the pane's edge, just like the selector grid.
#[test]
fn manage_pane_buttons_sit_right_aligned_on_the_title_row() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Spaces),
    });
    rendered_text_wide_tall(&mut app);
    let delete = app.hits.rect_of(&Hit::ManageDelete).unwrap();
    let rename = app.hits.rect_of(&Hit::ManageRename).unwrap();
    let move_all = app.hits.rect_of(&Hit::ManageMoveAll).unwrap();
    assert_eq!(
        rename.x + rename.width + 1,
        delete.x,
        "Rename just left of it"
    );
    assert!(move_all.x < rename.x);
    assert_eq!(delete.y, rename.y);
    // The Variables pane's title row sits at the same y.
    std::fs::write(
        app.project.root.join("variables.toml"),
        "[base_url]\ndefault = \"http://localhost\"\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    app.update(Action::SelectManageTab(ManageTab::Variables));
    rendered_text_wide_tall(&mut app); // builds the left rows
    app.varmanager.select_row(1); // row 0 is the "Variables" section header
    rendered_text_wide_tall(&mut app);
    let vm_delete = app.hits.rect_of(&Hit::VmDelete).expect("a var's Delete");
    assert_eq!(vm_delete.y, delete.y);
    assert_eq!(vm_delete.x + vm_delete.width, delete.x + delete.width);
}

#[test]
fn environments_tab_lists_envs_and_hides_the_space_only_buttons() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = app_with_envs();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Environments),
    });
    let text = rendered_text_tall(&mut app);
    assert!(text.contains("prod"), "{text}");
    assert!(text.contains("Environment: prod"), "{text}");
    assert!(text.contains("environments/prod.toml"), "{text}");
    assert!(app.hits.rect_of(&Hit::ManageMoveUp).is_none());
    assert!(app.hits.rect_of(&Hit::ManageMoveAll).is_none());
    assert!(app.hits.rect_of(&Hit::ManageRename).is_some());
}

#[test]
fn list_keys_move_delete_and_rename_through_the_prompt() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Spaces),
    });
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.manage.list.cursor, 1);
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(app.project.spaces, ["auth", "main"]);
    assert_eq!(app.manage.list.cursor, 0, "cursor follows the moved space");

    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.modals.is_empty(), "Enter is not rename");
    app.handle_key(&keymap, plain('r'));
    assert!(
        matches!(
            app.modals.top(),
            Some(Modal::Prompt {
                kind: PromptKind::RenameSpace { from },
                ..
            }) if from == "auth"
        ),
        "`r` opens the rename prompt for the selected space"
    );
    app.update(Action::Close);

    app.handle_key(&keymap, plain('m'));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(c)) if c.title() == "Move all requests to"),
        "`m` opens the move-all chooser (the button drops on a narrow pane)"
    );
    app.update(Action::Close);

    app.handle_key(&keymap, plain('d'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
}

#[test]
fn rename_button_opens_the_rename_prompt() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = app_with_envs();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Environments),
    });
    rendered_text_tall(&mut app);
    click_hit(&mut app, Hit::ManageRename);
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::RenameEnvironment { from },
            ..
        }) if from == "prod"
    ));
}

#[test]
fn move_all_button_opens_the_space_chooser() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Spaces),
    });
    rendered_text_tall(&mut app);
    click_hit(&mut app, Hit::ManageMoveAll);
    let Some(Modal::Chooser(c)) = app.modals.top() else {
        panic!("chooser")
    };
    assert_eq!(c.title(), "Move all requests to");
    assert_eq!(c.selected_label(), Some("auth"));
    assert_eq!(
        c.confirm().unwrap().actions,
        vec![Action::MoveAllRequests {
            from: "main".into(),
            to: "auth".into()
        }]
    );
}

#[test]
fn clicking_new_delete_and_a_row_dispatch_the_right_actions() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = app_with_envs();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Environments),
    });
    rendered_text_tall(&mut app);
    click_hit(&mut app, Hit::ManageRow(1));
    assert_eq!(app.manage.list.cursor, 1);
    click_hit(&mut app, Hit::ManageNew);
    assert!(matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: PromptKind::NewEnvironment,
            ..
        })
    ));
    app.update(Action::Close);
    click_hit(&mut app, Hit::ManageDelete);
    let Some(Modal::Confirm { title, .. }) = app.modals.top() else {
        panic!("confirm")
    };
    assert_eq!(title, "Delete environment \"qa\"?");
}

#[test]
fn manage_tabs_footer_chips_advertise_list_keys() {
    use crate::components::manage::ManageTab;
    let (mut app, _dir) = spaced_app();
    app.update(Action::OpenManage {
        tab: Some(ManageTab::Spaces),
    });
    let text = rendered_text_wide_tall(&mut app);
    for label in ["rename", "new", "delete", "move all", "move"] {
        assert!(text.contains(label), "{label}: {text}");
    }
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
        while app.editor.slug.as_deref() != Some("main/aaa") {
            app.update(Action::Undo);
        }
        assert_eq!(app.editor.slug.as_deref(), Some("main/aaa"));
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
        while app.editor.slug.as_deref() != Some("main/jb1") {
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
        let path = postui_core::storage::request_path(&app.project.root, "main/del-me");
        let original = std::fs::read_to_string(&path).unwrap();
        app.update(Action::DeleteRequest("main/del-me".into()));
        app.capture_undo();
        assert!(!path.exists());
        app.update(Action::Undo);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        app.update(Action::Redo);
        assert!(!path.exists(), "redo deletes again");
    }

    #[test]
    fn delete_request_trashes_the_file_and_undo_restores_it() {
        let (mut app, dir) = spaced_app();
        app.update(Action::ForceOpenRequest("main/alpha".into()));
        app.update(Action::DeleteRequest("main/alpha".into()));
        let path = dir.path().join("requests/main/alpha.toml");
        assert!(!path.exists());
        assert!(postui_core::trash::trash_dir(dir.path()).is_dir());
        assert!(
            app.editor.slug.is_none(),
            "deleting the open request clears the editor"
        );
        assert!(matches!(
            app.history_top_kind_for_test(),
            Some(crate::undo::StepKind::Trashed { .. })
        ));

        app.update(Action::Undo);
        assert!(path.is_file());
        assert!(
            app.sidebar
                .rows
                .iter()
                .any(|r| matches!(r, Row::Request { slug, .. } if slug == "main/alpha"))
        );

        app.update(Action::Redo);
        assert!(!path.exists());
    }

    #[test]
    fn undo_of_a_trashed_delete_fails_cleanly_when_the_path_is_occupied() {
        let (mut app, dir) = spaced_app();
        app.update(Action::DeleteRequest("main/alpha".into()));
        // Someone re-created the file meanwhile.
        postui_core::storage::save_request(dir.path(), "main/alpha", &req("https://x/2")).unwrap();
        let toasts_before = app.toasts.messages().len();
        app.update(Action::Undo);
        assert!(
            app.toasts.messages().len() > toasts_before,
            "failure toasts"
        );
        let on_disk = std::fs::read_to_string(dir.path().join("requests/main/alpha.toml")).unwrap();
        assert!(on_disk.contains("x/2"), "the newer file is never clobbered");
        assert_eq!(app.history.undo_len(), 0, "the failed step is dropped");
    }

    #[test]
    fn undo_reverts_a_rename_on_disk() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("old-name".into()));
        app.capture_undo();
        assert_eq!(app.editor.slug.as_deref(), Some("main/old-name"));
        app.update(Action::RenameRequest {
            from: "main/old-name".into(),
            to: "new-name".into(),
        });
        app.capture_undo();
        // The forward rename retitles the still-open editor in place
        // (doesn't close it) — undo/redo of the FileStates step it
        // records must do the same, not treat the moved-away path as a
        // delete and close the editor (reviewer finding).
        assert_eq!(app.editor.slug.as_deref(), Some("main/new-name"));
        app.update(Action::Undo);
        let root = app.project.root.clone();
        assert!(
            postui_core::storage::request_path(&root, "main/old-name").exists(),
            "old-name restored"
        );
        assert!(
            !postui_core::storage::request_path(&root, "main/new-name").exists(),
            "new-name gone"
        );
        assert_eq!(
            app.editor.slug.as_deref(),
            Some("main/old-name"),
            "undo retitles the open editor back, rather than closing it"
        );
        app.update(Action::Redo);
        assert!(
            postui_core::storage::request_path(&root, "main/new-name").exists(),
            "new-name restored"
        );
        assert!(
            !postui_core::storage::request_path(&root, "main/old-name").exists(),
            "old-name gone"
        );
        assert_eq!(
            app.editor.slug.as_deref(),
            Some("main/new-name"),
            "redo retitles the open editor forward again"
        );
    }

    #[test]
    fn save_is_not_an_undo_step_and_dirty_tracks_disk() {
        // Save updates the `saved` baseline but records nothing: the first
        // undo after a save reverts the last *edit*, never the save, and
        // the dirty flag stays an honest "buffer differs from disk".
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("sv".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.capture_undo(); // one coalesced EditorDelta: "" -> "ab"
        app.update(Action::SaveRequest); // disk now holds "ab"
        app.capture_undo();
        let path = postui_core::storage::request_path(&app.project.root, "main/sv");
        let saved_file = std::fs::read_to_string(&path).unwrap();
        assert!(!app.editor.is_dirty());
        app.update(Action::Undo); // reverts the "ab" edit, not the save
        assert_eq!(
            app.editor.url.text(),
            "",
            "the first undo after a save reverts the last edit"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            saved_file,
            "undo never touches disk"
        );
        assert!(
            app.editor.is_dirty(),
            "buffer ('') genuinely differs from disk ('ab')"
        );
        app.update(Action::Redo); // back to the saved content
        assert_eq!(app.editor.url.text(), "ab");
        assert!(
            !app.editor.is_dirty(),
            "buffer matches the saved baseline again"
        );
    }

    #[test]
    fn save_still_splits_a_typing_burst() {
        // Two keystrokes inside the coalesce window with a save between
        // them must stay two undo steps, so one undo lands exactly on the
        // saved snapshot.
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("sv2".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::SaveRequest); // disk holds "a"
        app.capture_undo();
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::Undo);
        assert_eq!(
            app.editor.url.text(),
            "a",
            "one undo lands on the saved snapshot, not past it"
        );
        assert!(!app.editor.is_dirty(), "buffer matches disk ('a')");
    }

    #[test]
    fn save_does_not_clear_redo() {
        // Saving is not an edit: an undone edit must stay redoable across
        // a save.
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("sv3".into()));
        app.capture_undo();
        app.editor.sub_focus = SubFocus::Url;
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.capture_undo();
        app.update(Action::Undo); // url back to ""
        assert_eq!(app.editor.url.text(), "");
        app.update(Action::SaveRequest); // disk holds ""
        app.capture_undo();
        app.update(Action::Redo);
        assert_eq!(
            app.editor.url.text(),
            "a",
            "the undone edit survives a save as a redo"
        );
        assert!(app.editor.is_dirty(), "buffer ('a') differs from disk ('')");
    }

    #[test]
    fn undo_of_a_deleted_step_fails_gracefully_when_disk_changed() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("ext".into()));
        app.capture_undo();
        app.update(Action::DeleteRequest("main/ext".into()));
        app.capture_undo();
        app.update(Action::Undo); // restores file
        let path = postui_core::storage::request_path(&app.project.root, "main/ext");
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
                    before: Box::new(req("https://before")),
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
        let new_path = postui_core::storage::request_path(&app.project.root, "main/brand-new");
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
                .any(|r| matches!(r, Row::Request { slug, .. } if slug == "main/brand-new")),
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

        // Right-edge click: the caret follows the pointer, and the '9'
        // must land at the end of the text.
        let r = field_rect(&mut app, VmField::EnvValue);
        app.handle_mouse(left_down(r.x + r.width - 2, r.y + 1));
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

    /// User finding: there was no way to remove an env value from the
    /// variable form. It gets an explicit `✕ remove` control beside the
    /// "Value in <env>" label (mirroring the value popup's Remove button)
    /// — an emptied field commit stays a verbatim write (`name = ""`),
    /// deliberately not overloaded to mean removal.
    #[test]
    fn clicking_the_env_value_remove_control_removes_the_stored_pair() {
        let dir = tempfile::tempdir().unwrap();
        var_project(dir.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, dir.path().to_path_buf());
        goto_row(&mut app, |r| {
            r == &crate::components::varmanager::VmRow::Var("base_url".into())
        });

        rendered_text_tall(&mut app);
        let r = app
            .hits
            .rect_of(&crate::hit::Hit::VmRemoveEnvValue)
            .expect("a stored env value offers the remove control");
        app.handle_mouse(left_down(r.x, r.y));

        let on_disk = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
        assert!(!on_disk.contains("base_url"), "pair removed: {on_disk}");
        assert_eq!(
            app.project.resolved.values["base_url"], "http://localhost:8080",
            "resolution falls back to the declaration default"
        );

        let with_value = "base_url = \"https://qa.example.com\"\n\n[options.user.alice]\nuser = \"1001\"\n\n[options.user.bob]\nuser = \"2002\"\n";
        app.update(Action::Undo);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap(),
            with_value,
            "undo restores the removed pair"
        );
    }

    /// With nothing stored there is nothing to remove, so the control
    /// isn't offered — and an emptied commit writes `name = ""` verbatim
    /// (an explicit empty value, not a removal).
    #[test]
    fn the_remove_control_is_absent_when_the_env_stores_nothing() {
        use crate::components::line_input::LineInput;
        use crate::components::varmanager::VmField;

        let dir = tempfile::tempdir().unwrap();
        var_project(dir.path());
        let qa_path = dir.path().join("environments/qa.toml");
        std::fs::write(&qa_path, "[options.user.alice]\nuser = \"1001\"\n").unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, dir.path().to_path_buf());
        goto_row(&mut app, |r| {
            r == &crate::components::varmanager::VmRow::Var("base_url".into())
        });

        rendered_text_tall(&mut app);
        assert!(
            app.hits
                .rect_of(&crate::hit::Hit::VmRemoveEnvValue)
                .is_none(),
            "no stored value, no remove control"
        );

        app.varmanager.form.editing = Some((VmField::EnvValue, LineInput::new("")));
        app.commit_var_form();
        assert!(app.toasts.is_empty(), "{:?}", app.toasts.messages());
        assert!(
            std::fs::read_to_string(&qa_path)
                .unwrap()
                .contains("base_url = \"\""),
            "an emptied commit writes the empty value verbatim"
        );
    }

    /// The secret twin: the remove control clears the stored secret from
    /// the secrets store (memory and `.local/secrets.toml` both).
    #[test]
    fn the_remove_control_clears_a_stored_secret_value() {
        let dir = tempfile::tempdir().unwrap();
        var_project(dir.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::with_root(tx, dir.path().to_path_buf());
        app.update(Action::VarEdit(VarEditOp::SetSecretValue {
            env: "qa".into(),
            name: "api_key".into(),
            value: "s3cret".into(),
        }));
        goto_row(&mut app, |r| {
            r == &crate::components::varmanager::VmRow::Var("api_key".into())
        });

        rendered_text_tall(&mut app);
        let r = app
            .hits
            .rect_of(&crate::hit::Hit::VmRemoveEnvValue)
            .expect("a stored secret offers the remove control");
        app.handle_mouse(left_down(r.x, r.y));

        assert_eq!(
            app.project.secrets.get("qa").and_then(|m| m.get("api_key")),
            None,
            "the stored secret is gone"
        );
        let on_disk = std::fs::read_to_string(dir.path().join(".local/secrets.toml")).unwrap();
        assert!(!on_disk.contains("api_key"), "{on_disk}");
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

        // Mid-cell clicks: the caret follows the pointer, and a click past
        // the short value text still lands the caret at its end.
        let r = cell_rect(&mut app, 0, 1);
        app.handle_mouse(left_down(r.x + 10, r.y));
        let keymap = Keymap::default_bindings();
        app.handle_key(&keymap, plain('9'));
        // Clicking a different cell commits the first one.
        let other = cell_rect(&mut app, 1, 1);
        app.handle_mouse(left_down(other.x + 10, other.y));

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
    fn alt_arrows_word_jump_in_the_body_instead_of_cycling_tabs() {
        let keymap = Keymap::default_bindings();
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("w".into()));
        app.update(Action::CycleMethod); // POST, so Body is enabled
        app.update(Action::EditorTabSelect(EditorTab::Body.index()));
        app.editor.set_body_text("foo bar");
        app.editor.sub_focus = SubFocus::Content;
        app.focus = PaneId::Editor;
        app.editor.body.cursor = edtui::Index2::new(0, 0);
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(
            app.editor.active_tab,
            EditorTab::Body,
            "alt+Right must not cycle tabs while the body caret is live"
        );
        assert_eq!(
            app.editor.body.cursor,
            edtui::Index2::new(0, 3),
            "alt+Right word-jumps instead"
        );
        // Anywhere else, alt+Right still cycles tabs.
        app.editor.sub_focus = SubFocus::Url;
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_ne!(
            app.editor.active_tab,
            EditorTab::Body,
            "outside the body caret, alt+Right cycles tabs as before"
        );
    }

    #[test]
    fn tab_underline_follows_span_shifts_on_request_switch() {
        use crate::anim::{AnimKey, StripId};
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("with-header".into()));
        app.editor.headers.insert(
            "X-One".into(),
            postui_core::model::Entry {
                value: "1".into(),
                enabled: true,
            },
        );
        app.update(Action::SaveRequest);
        app.update(Action::CreateRequest("plain".into()));
        // Choose Params on the plain request (its Headers label has no
        // count, so every span sits further left than with-header's).
        app.update(Action::EditorTabSelect(EditorTab::Params.index()));
        let plain_x = app.editor.tab_strip_spans()[EditorTab::Params.draw_position()].0;
        assert_eq!(
            app.anims.target(AnimKey::TabUnderline(StripId::EditorTabs)),
            Some(plain_x as f32)
        );
        // Opening the request whose Headers label carries a count shifts
        // every later span right; the underline must follow without a tab
        // switch.
        app.update(Action::OpenRequest("main/with-header".into()));
        let spans = app.editor.tab_strip_spans();
        let (x, w) = spans[EditorTab::Params.draw_position()];
        assert_ne!(x, plain_x, "the span must actually have moved");
        assert_eq!(
            app.anims.target(AnimKey::TabUnderline(StripId::EditorTabs)),
            Some(x as f32),
            "underline left edge follows the shifted span"
        );
        assert_eq!(
            app.anims
                .target(AnimKey::TabUnderlineWidth(StripId::EditorTabs)),
            Some((x + w) as f32),
            "underline right edge follows the shifted span"
        );
    }

    /// A request switch swaps in that request's response, whose active tab
    /// is whatever it was left on — no `ResponseViewMode` runs, so the
    /// outgoing response's underline glide must be forgotten (a stale
    /// tracked value would pin the underline under the wrong tab).
    #[test]
    fn request_switch_forgets_the_response_underline_anim() {
        use crate::anim::{AnimKey, StripId};
        use crate::components::response::ViewMode;
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("a".into()));
        ready_response(&mut app, "{}");
        app.update(Action::ResponseViewMode(ViewMode::Headers));
        let left_key = AnimKey::TabUnderline(StripId::ResponseTabs);
        assert!(
            app.anims.value(left_key, Instant::now()).is_some(),
            "the switch tracked the underline"
        );

        app.update(Action::CreateRequest("b".into()));
        assert!(
            app.anims.value(left_key, Instant::now()).is_none(),
            "the swapped-in response starts from its own static span"
        );
    }

    /// A background parse concluding "not JSON" removes the Tree tab and
    /// forces the mode to Raw — the tab set changed under the underline, so
    /// its animation must be forgotten too.
    #[test]
    fn parse_concluding_not_json_forgets_the_response_underline_anim() {
        use crate::anim::{AnimKey, StripId};
        use crate::components::response::{SYNC_PRETTY_BYTES, ViewMode};
        let mut app = App::new_for_test();
        app.session.send_generation = 7;
        let big = "x".repeat(SYNC_PRETTY_BYTES + 1);
        ready_response(&mut app, &big);
        // While the parse runs the Tree tab exists (spinner) and may be
        // switched to; that tracks the underline keys.
        app.update(Action::ResponseViewMode(ViewMode::Pretty));
        let left_key = AnimKey::TabUnderline(StripId::ResponseTabs);
        assert!(app.anims.value(left_key, Instant::now()).is_some());

        app.update(Action::PrettyParsed {
            generation: 7,
            tree: None,
        });
        assert_eq!(
            app.session.response.view().unwrap().mode,
            ViewMode::Raw,
            "not-JSON forces Raw"
        );
        assert!(
            app.anims.value(left_key, Instant::now()).is_none(),
            "the forced Raw tab snaps to its own span"
        );
    }

    #[test]
    fn switching_requests_keeps_the_active_tab() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("a".into()));
        app.update(Action::CreateRequest("b".into()));
        app.update(Action::EditorTabSelect(EditorTab::Params.index()));
        app.update(Action::OpenRequest("main/a".into()));
        assert_eq!(app.editor.slug.as_deref(), Some("main/a"));
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
        app.update(Action::OpenRequest("main/get-req".into()));
        assert_ne!(app.editor.active_tab, EditorTab::Body);
        // ...but coming back to the POST restores the chosen tab.
        app.update(Action::OpenRequest("main/post-req".into()));
        assert_eq!(app.editor.active_tab, EditorTab::Body);
    }

    #[test]
    fn method_change_restores_the_body_tab_when_it_reenables() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("m".into()));
        app.update(Action::CycleMethod); // POST
        app.update(Action::EditorTabSelect(EditorTab::Body.index()));
        app.update(Action::SetMethod(postui_core::model::Method::Get));
        assert_ne!(
            app.editor.active_tab,
            EditorTab::Body,
            "hops off disabled Body"
        );
        app.update(Action::SetMethod(postui_core::model::Method::Post));
        assert_eq!(
            app.editor.active_tab,
            EditorTab::Body,
            "returns when re-enabled"
        );
    }

    #[test]
    fn choosing_a_tab_after_the_hop_replaces_the_body_preference() {
        let mut app = App::new_for_test();
        app.update(Action::CreateRequest("g".into()));
        app.update(Action::CreateRequest("p".into()));
        app.update(Action::CycleMethod); // p: POST
        app.update(Action::SaveRequest);
        app.update(Action::EditorTabSelect(EditorTab::Body.index()));
        app.update(Action::OpenRequest("main/g".into())); // hop off Body
        app.update(Action::EditorTabSelect(EditorTab::Vars.index()));
        app.update(Action::OpenRequest("main/p".into()));
        assert_eq!(
            app.editor.active_tab,
            EditorTab::Vars,
            "the explicit Vars choice replaced the old Body preference"
        );
    }

    #[test]
    fn open_theme_chooser_lists_registry_entries_with_ids() {
        let mut app = App::new_for_test();
        app.update(Action::OpenThemeChooser);
        let Some(crate::components::modal::Modal::Chooser(c)) = app.modals.top() else {
            panic!("theme chooser modal expected");
        };
        assert_eq!(c.selected_id(), Some("terminal"), "terminal entry first");
    }

    /// The picker opens filtered to the applied theme's polarity: on the
    /// (dark) default, light themes are not reachable by browsing, so no
    /// bright flashes. Left/Right flips to the light set, the preview
    /// follows, and Esc still restores the original theme.
    #[test]
    fn theme_picker_polarity_toggle_flips_sets_and_esc_still_reverts() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new_for_test();
        let keymap = crate::keys::Keymap::load();
        let original = app.theme.page;
        let original_name = app.theme_name.clone();
        app.update(Action::OpenThemeChooser);
        {
            // Dark set only: fuzzy-typing a light theme's name matches
            // nothing.
            let Some(crate::components::modal::Modal::Chooser(c)) = app.modals.top() else {
                panic!("theme chooser modal expected");
            };
            assert_eq!(c.selected_id(), Some("terminal"));
        }
        for ch in "light".chars() {
            app.handle_key(
                &keymap,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            );
        }
        {
            let Some(crate::components::modal::Modal::Chooser(c)) = app.modals.top() else {
                panic!("theme chooser modal expected");
            };
            assert_eq!(c.selected_id(), None, "no light themes in the dark set");
        }
        for _ in 0..5 {
            app.handle_key(
                &keymap,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            );
        }
        // Terminal has no light/dark counterpart: the switch is inert
        // while it's highlighted.
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "terminal", "unpaired: flip does nothing");
        // Move to the paired "dark" builtin; Right now lands on its
        // counterpart in the light set, and the preview follows.
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "dark");
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "light", "flip follows the counterpart");
        assert_ne!(app.theme.page, original);
        // Flip back: counterpart again — the same family, dark side.
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "dark");
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.theme_name, original_name, "esc restores after toggling");
        assert_eq!(app.theme.page, original);
    }

    /// The switch keeps the selected family across polarity flips for
    /// non-adjacent rows too — gruvbox-dark lands on gruvbox-light, not
    /// on the light set's first row.
    #[test]
    fn theme_picker_flip_stays_on_the_selected_family() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new_for_test();
        let keymap = crate::keys::Keymap::load();
        app.update(Action::ApplyTheme("gruvbox-dark".into()));
        app.update(Action::OpenThemeChooser);
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "gruvbox-light");
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "gruvbox-dark");
        // Catppuccin pairs across its own names, not the stem convention.
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.update(Action::ApplyTheme("catppuccin-mocha".into()));
        app.update(Action::OpenThemeChooser);
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "catppuccin-latte");
    }

    /// The picker must open with the highlight on the currently-applied
    /// theme — the highlight drives the live preview, so opening on row 0
    /// would instantly re-theme the app before the user touches a key.
    #[test]
    fn open_theme_chooser_defaults_to_the_current_theme() {
        let mut app = App::new_for_test();
        app.update(Action::ApplyTheme("gruvbox-dark".into()));
        let gruvbox_page = app.theme.page;
        app.update(Action::OpenThemeChooser);
        let Some(crate::components::modal::Modal::Chooser(c)) = app.modals.top() else {
            panic!("theme chooser modal expected");
        };
        assert_eq!(c.selected_id(), Some("gruvbox-dark"));
        assert_eq!(
            app.theme.page, gruvbox_page,
            "opening the picker must not change the applied theme"
        );
    }

    #[test]
    fn apply_theme_switches_the_live_theme_and_records_the_name() {
        let mut app = App::new_for_test();
        let before = app.theme.page;
        app.update(Action::ApplyTheme("gruvbox-dark".into()));
        assert_eq!(app.theme_name, "gruvbox-dark");
        assert_eq!(app.ui_settings.theme, "gruvbox-dark");
        assert_ne!(
            app.theme.page, before,
            "gruvbox page differs from the default dark page"
        );
    }

    #[test]
    fn apply_theme_with_an_unknown_name_degrades_to_terminal() {
        let mut app = App::new_for_test();
        app.update(Action::ApplyTheme("no-such-theme".into()));
        assert_eq!(app.theme_name, "terminal");
    }

    #[test]
    fn theme_picker_previews_on_highlight_and_esc_reverts() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new_for_test();
        let keymap = crate::keys::Keymap::default_bindings();
        let original = app.theme.text;
        let original_name = app.theme_name.clone();
        app.update(Action::OpenThemeChooser);
        // The picker opens filtered to the current (dark) polarity: row 0
        // is "terminal"; Down moves to "dark", Down again to
        // "gruvbox-dark". The live-apply proof compares the text token
        // (the terminal fallback shares Dark's seeds, so page alone
        // wouldn't necessarily distinguish neighbors).
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "gruvbox-dark", "highlight applies live");
        assert_ne!(app.theme.text, original);
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.theme_name, original_name,
            "esc restores the prior theme"
        );
        assert_eq!(app.theme.text, original);
        assert!(app.modals.top().is_none());
    }

    #[test]
    fn theme_picker_click_away_reverts_the_live_preview() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new_for_test();
        let keymap = crate::keys::Keymap::default_bindings();
        let original = app.theme.page;
        let original_name = app.theme_name.clone();
        app.update(Action::OpenThemeChooser);
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)); // "dark"
        assert_eq!(app.theme_name, "dark", "highlight applies live");
        // Hit::ModalOutside routes here, not through apply_modal_result.
        app.update(Action::Close);
        assert_eq!(
            app.theme_name, original_name,
            "click-away reverts the prior theme"
        );
        assert_eq!(app.theme.page, original);
        assert!(
            app.theme_preview.is_none(),
            "preview state disarmed so a later project/env chooser is unaffected"
        );
    }

    #[test]
    fn theme_picker_enter_keeps_the_previewed_theme() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new_for_test();
        let keymap = crate::keys::Keymap::default_bindings();
        app.update(Action::OpenThemeChooser);
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)); // "dark"
        app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.theme_name, "dark");
        assert_eq!(app.ui_settings.theme, "dark");
        assert!(app.modals.top().is_none());
        assert!(
            app.theme_preview.is_none(),
            "preview state cleared on close"
        );
    }

    #[test]
    fn filter_typing_moves_the_live_preview_with_the_highlight() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new_for_test();
        let keymap = crate::keys::Keymap::default_bindings();
        app.update(Action::OpenThemeChooser);
        for ch in "mocha".chars() {
            app.handle_key(
                &keymap,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            );
        }
        assert_eq!(
            app.theme_name, "catppuccin-mocha",
            "refilter re-selects row 0 and previews it"
        );
    }
}

// --- right-click text menus (Copy / Paste on text surfaces) ---------------

/// A clipboard whose writes land in `out` and whose reads return `read`.
fn file_clipboard(out: &std::path::Path, read: &str) -> crate::clipboard::Clipboard {
    let cmd = format!("cat > {}", out.to_string_lossy());
    let mut clipboard = crate::clipboard::Clipboard::new_for_test(Some(cmd), 65536, false);
    clipboard.set_read_for_test(read);
    clipboard
}

fn menu_labels(app: &App) -> Vec<String> {
    let Some(Modal::Dropdown(menu)) = app.modals.top() else {
        panic!(
            "expected a context menu, got {:?}",
            app.modals.top().is_some()
        );
    };
    menu.items.iter().map(|i| i.label.clone()).collect()
}

fn menu_action(app: &App, label: &str) -> Option<Action> {
    let Some(Modal::Dropdown(menu)) = app.modals.top() else {
        panic!("expected a context menu");
    };
    menu.items
        .iter()
        .find(|i| i.label == label)
        .unwrap_or_else(|| panic!("no {label:?} item"))
        .action
        .clone()
}

#[test]
fn right_click_on_the_url_bar_offers_copy_and_paste_and_copy_copies_the_selection() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(file_clipboard(&out, ""));
    app.editor.url = crate::components::line_input::LineInput::new("https://example.com");
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url.select_all();
    render_once(&mut app);
    let area = app.editor.last_url_text_area.expect("url area recorded");

    assert!(app.handle_mouse(right_down(area.x + 3, area.y)));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Paste",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ]
    );
    assert!(
        app.editor.url.selection().is_some(),
        "opening the menu keeps the selection"
    );
    let copy = menu_action(&app, "Copy").expect("Copy is enabled with a selection");
    app.update(Action::Close);
    app.update(copy);

    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "https://example.com"
    );
    assert!(
        app.editor.url.selection().is_some(),
        "copy keeps the selection"
    );
}

#[test]
fn right_click_on_the_url_bar_without_a_selection_greys_copy_and_paste_lands_in_the_url() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(file_clipboard(&out, "/v2"));
    app.editor.url = crate::components::line_input::LineInput::new("https://example.com");
    app.editor
        .url
        .set_cursor(app.editor.url.text().chars().count());
    // Focus starts elsewhere: the right click has to claim the URL bar
    // itself, or the menu's Paste would have nowhere to land.
    app.focus = PaneId::Sidebar;
    app.editor.sub_focus = SubFocus::None;
    render_once(&mut app);
    let area = app.editor.last_url_text_area.expect("url area recorded");

    app.handle_mouse(right_down(area.x + 3, area.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Paste",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ]
    );
    assert!(
        menu_action(&app, "Copy").is_none(),
        "nothing selected: Copy is greyed"
    );
    let paste = menu_action(&app, "Paste").expect("Paste is enabled");
    app.update(Action::Close);
    app.update(paste);

    assert_eq!(app.editor.url.text(), "https://example.com/v2");
    assert_eq!(app.focus, PaneId::Editor);
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
}

#[test]
fn right_click_on_the_edited_table_cell_offers_the_text_menu_and_keeps_the_edit_live() {
    use crate::components::table_editor::{CellEdit, Col};
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(file_clipboard(&out, "-x"));
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
    let mut input = crate::components::line_input::LineInput::new("page");
    input.select_all();
    app.editor.table.editing = Some(CellEdit {
        row: 0,
        col: Col::Key,
        input,
        original: "page".into(),
    });
    render_once(&mut app);
    let cell = app
        .hits
        .rect_of(&crate::hit::Hit::TableCell { row: 0, col: 0 })
        .expect("the edited cell is registered");

    app.handle_mouse(right_down(cell.x, cell.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Paste",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ]
    );
    assert!(
        app.editor.table.editing.is_some(),
        "a right click on the cell under edit must not commit it"
    );
    let copy = menu_action(&app, "Copy").expect("Copy is enabled with a selection");
    let paste = menu_action(&app, "Paste").expect("Paste is enabled");
    app.update(Action::Close);
    app.update(copy);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "page");
    app.update(paste);
    let edit = app.editor.table.editing.as_ref().expect("still editing");
    assert_eq!(edit.input.text(), "-x", "paste replaces the selection");
}

#[test]
fn right_click_elsewhere_on_the_row_keeps_the_row_menu_and_commits_the_edit() {
    use crate::components::table_editor::{CellEdit, Col};
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
    app.editor.table.editing = Some(CellEdit {
        row: 0,
        col: Col::Key,
        input: crate::components::line_input::LineInput::new("pages"),
        original: "page".into(),
    });
    render_once(&mut app);
    // The value cell of the same row is not the cell under edit.
    let value = app
        .hits
        .rect_of(&crate::hit::Hit::TableCell { row: 0, col: 1 })
        .expect("the value cell is registered");

    app.handle_mouse(right_down(value.x, value.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Duplicate row",
            "Delete param",
            "Extract value to variable\u{2026}",
            "Extract value to selector\u{2026}"
        ]
    );
    assert!(
        app.editor.table.editing.is_none(),
        "a row-level right click commits the edit first, as before"
    );
    assert!(app.editor.params.contains_key("pages"));
}

#[test]
fn right_click_on_the_response_pane_offers_copy_only() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(file_clipboard(&out, ""));
    ready_response(&mut app, "plain text body"); // not JSON -> Raw view
    render_once(&mut app);
    let area = app
        .session
        .response
        .view()
        .unwrap()
        .last_area
        .expect("body area recorded");
    app.handle_mouse(left_down(area.x + 1, area.y));
    app.handle_mouse(left_down(area.x + 1, area.y)); // double click: "plain"
    app.handle_mouse(left_up(area.x + 1, area.y));
    assert_eq!(
        app.session.response.selected_text().as_deref(),
        Some("plain")
    );

    app.handle_mouse(right_down(area.x + 1, area.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ],
        "the response is read-only: no Paste"
    );
    let copy = menu_action(&app, "Copy").expect("Copy is enabled with a selection");
    app.update(Action::Close);
    app.update(copy);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "plain");
}

#[test]
fn right_click_on_the_response_pane_copies_the_response_selection_not_the_body_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(file_clipboard(&out, ""));
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("body text");
    app.editor.body_select_all();
    ready_response(&mut app, "plain text body");
    render_once(&mut app);
    let area = app
        .session
        .response
        .view()
        .unwrap()
        .last_area
        .expect("body area recorded");
    app.handle_mouse(left_down(area.x + 1, area.y));
    app.handle_mouse(left_down(area.x + 1, area.y));
    app.handle_mouse(left_up(area.x + 1, area.y));
    assert_eq!(
        app.session.response.selected_text().as_deref(),
        Some("plain")
    );

    app.handle_mouse(right_down(area.x + 1, area.y));
    let copy = menu_action(&app, "Copy").unwrap();
    app.update(Action::Close);
    app.update(copy);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "plain");
}

#[test]
fn right_click_on_the_body_editor_offers_copy_and_paste() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let mut app = App::new_for_test();
    app.set_clipboard_for_test(file_clipboard(&out, "replaced"));
    app.focus = PaneId::Sidebar;
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello world");
    app.editor.body_select_all();
    render_once(&mut app);
    let area = app.editor.last_body_area.expect("body area recorded");

    app.handle_mouse(right_down(area.x + 2, area.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Paste",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ]
    );
    assert!(
        app.editor.body_selected_text().is_some(),
        "opening the menu keeps the selection"
    );
    let copy = menu_action(&app, "Copy").expect("Copy is enabled with a selection");
    let paste = menu_action(&app, "Paste").expect("Paste is enabled");
    app.update(Action::Close);
    app.update(copy);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello world");
    app.update(paste);
    assert_eq!(app.editor.body_text(), "replaced");
    assert_eq!(app.focus, PaneId::Editor);
}

#[test]
fn right_click_on_the_edited_grid_cell_offers_the_text_menu_instead_of_the_row_menu() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let out = dir.path().join("out.txt");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.set_clipboard_for_test(file_clipboard(&out, ""));
    goto_group(&mut app, "user");
    app.varmanager.start_cell_edit(&app.project, 0, 1);
    app.varmanager
        .grid
        .editing
        .as_mut()
        .unwrap()
        .input
        .select_all();
    let expected = app
        .varmanager
        .grid
        .editing
        .as_ref()
        .unwrap()
        .input
        .text()
        .to_string();
    render_once(&mut app);
    let cell = app
        .hits
        .rect_of(&crate::hit::Hit::VmEntryCell { row: 0, col: 1 })
        .expect("the edited cell is registered");

    app.handle_mouse(right_down(cell.x, cell.y));
    assert_eq!(
        menu_labels(&app),
        vec!["Copy", "Paste"],
        "the manager's own cells are variables already: no extract"
    );
    assert!(
        app.varmanager.grid.editing.is_some(),
        "a right click on the cell under edit must not commit it"
    );
    let copy = menu_action(&app, "Copy").expect("Copy is enabled with a selection");
    app.update(Action::Close);
    app.update(copy);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), expected);

    // Any other cell of the row still opens the row's own menu.
    render_once(&mut app);
    let other = app
        .hits
        .rect_of(&crate::hit::Hit::VmEntryCell { row: 0, col: 0 })
        .expect("the other cell is registered");
    app.handle_mouse(right_down(other.x, other.y));
    assert!(
        menu_labels(&app).iter().any(|l| l == "Rename"),
        "{:?}",
        menu_labels(&app)
    );
}

// --- extract a selection to a variable (right-click text menu) ------------

/// Selects `needle` inside the URL bar (first occurrence) the way a mouse
/// sweep would.
fn select_in_url(app: &mut App, needle: &str) {
    let text = app.editor.url.text().to_string();
    let start = text.find(needle).expect("needle in url");
    let start_chars = text[..start].chars().count();
    let end_chars = start_chars + needle.chars().count();
    app.editor.url.set_cursor(start_chars);
    app.editor.url.begin_mouse_selection();
    app.editor.url.extend_mouse_selection_to(end_chars);
    assert_eq!(app.editor.url.selected_text().as_deref(), Some(needle));
}

#[test]
fn text_menu_offers_extract_to_variable_only_with_a_selection() {
    use crate::action::TextSurface;
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://x/ping/abc-123");
    render_once(&mut app);
    let area = app.editor.last_url_text_area.expect("url area recorded");

    app.handle_mouse(right_down(area.x + 3, area.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Paste",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ]
    );
    assert!(
        menu_action(&app, "Extract to variable\u{2026}").is_none(),
        "nothing selected: greyed"
    );
    app.update(Action::Close);

    select_in_url(&mut app, "abc-123");
    app.handle_mouse(right_down(area.x + 3, area.y));
    assert_eq!(
        menu_action(&app, "Extract to variable\u{2026}"),
        Some(Action::ExtractSelection(TextSurface::Url))
    );
    let open = menu_action(&app, "Extract to variable\u{2026}").unwrap();
    app.update(Action::Close);
    app.update(open);
    let Some(Modal::MultiPrompt { kind, .. }) = app.modals.top() else {
        panic!("expected the extract prompt");
    };
    assert!(matches!(
        kind,
        PromptKind::ExtractSelection(TextSurface::Url)
    ));
}

#[test]
fn extracting_a_url_selection_replaces_only_the_selected_part() {
    use crate::action::TextSurface;
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping/abc-123"))
        .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    select_in_url(&mut app, "abc-123");

    app.update(Action::ConfirmExtractSelection {
        name: "trace_id".into(),
        destination: crate::action::ExtractDestination::ProjectDefault,
        surface: TextSurface::Url,
    });

    assert_eq!(app.editor.url.text(), "https://x/ping/{{trace_id}}");
    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(on_disk.contains("default = \"abc-123\""), "{on_disk}");
    assert!(
        app.toasts
            .messages()
            .iter()
            .any(|m| m.contains("extracted to {{trace_id}}")),
        "{:?}",
        app.toasts.messages()
    );
}

#[test]
fn extracting_a_table_cell_selection_replaces_the_part_and_commits_the_cell() {
    use crate::action::TextSurface;
    use crate::components::table_editor::{CellEdit, Col};
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
    app.editor.headers.insert(
        "authorization".into(),
        postui_core::model::Entry {
            value: "Bearer abc".into(),
            enabled: true,
        },
    );
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Headers;
    app.editor.sub_focus = SubFocus::Content;
    app.editor.table.selected = Some(0);
    let mut input = crate::components::line_input::LineInput::new("Bearer abc");
    input.set_cursor(7);
    input.begin_mouse_selection();
    input.extend_mouse_selection_to(10);
    assert_eq!(input.selected_text().as_deref(), Some("abc"));
    app.editor.table.editing = Some(CellEdit {
        row: 0,
        col: Col::Value,
        input,
        original: "Bearer abc".into(),
    });

    app.update(Action::ConfirmExtractSelection {
        name: "token".into(),
        destination: crate::action::ExtractDestination::Request,
        surface: TextSurface::TableCell,
    });

    assert!(app.editor.table.editing.is_none(), "the cell commits");
    assert_eq!(
        app.editor.headers["authorization"].value,
        "Bearer {{token}}"
    );
    assert_eq!(app.editor.variables["token"].value, "abc");
}

#[test]
fn extracting_a_body_selection_replaces_it_with_the_token() {
    use crate::action::TextSurface;
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;
    app.editor.set_body_text("{\"id\": 1}");
    app.editor.body_select_all();

    app.update(Action::ConfirmExtractSelection {
        name: "payload".into(),
        destination: crate::action::ExtractDestination::ProjectDefault,
        surface: TextSurface::Body,
    });

    assert_eq!(app.editor.body_text(), "{{payload}}");
    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(
        on_disk.contains("{\\\"id\\\": 1}") || on_disk.contains("'{\"id\": 1}'"),
        "{on_disk}"
    );
}

#[test]
fn extracting_a_response_selection_creates_the_variable_without_touching_the_editor() {
    use crate::action::TextSurface;
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.editor.url = crate::components::line_input::LineInput::new("https://x/login");
    ready_response(&mut app, "session abc123 ok"); // not JSON -> Raw view
    render_once(&mut app);
    let area = app
        .session
        .response
        .view()
        .unwrap()
        .last_area
        .expect("body area recorded");
    app.handle_mouse(left_down(area.x + 9, area.y));
    app.handle_mouse(left_down(area.x + 9, area.y)); // double click: "abc123"
    app.handle_mouse(left_up(area.x + 9, area.y));
    assert_eq!(
        app.session.response.selected_text().as_deref(),
        Some("abc123")
    );

    app.handle_mouse(right_down(area.x + 9, area.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ]
    );
    let open = menu_action(&app, "Extract to variable\u{2026}").unwrap();
    app.update(Action::Close);
    app.update(open);
    assert!(matches!(
        app.modals.top(),
        Some(Modal::MultiPrompt {
            kind: PromptKind::ExtractSelection(TextSurface::Response),
            ..
        })
    ));
    app.update(Action::Close);

    app.update(Action::ConfirmExtractSelection {
        name: "session".into(),
        destination: crate::action::ExtractDestination::ProjectDefault,
        surface: TextSurface::Response,
    });

    let on_disk = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(on_disk.contains("default = \"abc123\""), "{on_disk}");
    assert_eq!(
        app.editor.url.text(),
        "https://x/login",
        "nothing to replace"
    );
    assert_eq!(
        app.session.response.selected_text().as_deref(),
        Some("abc123")
    );
}

#[test]
fn extract_selection_refuses_when_the_selection_is_gone() {
    use crate::action::TextSurface;
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://x/ping");
    app.update(Action::ExtractSelection(TextSurface::Url));
    assert!(app.modals.is_empty(), "no prompt without a selection");
    assert!(
        app.toasts.messages().iter().any(|m| m.contains("select")),
        "{:?}",
        app.toasts.messages()
    );
}

// -- Extract to selector: a new one-field selector whose only option holds
// the extracted value --

#[test]
fn text_menu_offers_extract_to_selector_under_extract_to_variable() {
    use crate::action::{ExtractSource, TextSurface};
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new("https://x/ping/abc-123");
    render_once(&mut app);
    let area = app.editor.last_url_text_area.expect("url area recorded");

    app.handle_mouse(right_down(area.x + 3, area.y));
    assert_eq!(
        menu_labels(&app),
        vec![
            "Copy",
            "Paste",
            "Extract to variable\u{2026}",
            "Extract to selector\u{2026}"
        ]
    );
    assert!(
        menu_action(&app, "Extract to selector\u{2026}").is_none(),
        "nothing selected: greyed"
    );
    app.update(Action::Close);

    select_in_url(&mut app, "abc-123");
    app.handle_mouse(right_down(area.x + 3, area.y));
    let open = menu_action(&app, "Extract to selector\u{2026}").unwrap();
    assert_eq!(open, Action::ExtractSelectionToSelector(TextSurface::Url));
    app.update(Action::Close);
    app.update(open);
    let Some(Modal::MultiPrompt { kind, fields, .. }) = app.modals.top() else {
        panic!("expected the extract-selector prompt");
    };
    assert!(matches!(
        kind,
        PromptKind::ExtractSelector(ExtractSource::Selection(TextSurface::Url))
    ));
    let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, vec!["name", "option", "scope"]);
    assert_eq!(
        fields[1].input.text(),
        "abc-123",
        "a short, name-safe value seeds the option name"
    );
    assert_eq!(fields[2].choices, vec!["Per environment", "Shared"]);
}

#[test]
fn extract_selector_option_seed_is_blank_for_a_long_or_unsafe_value() {
    use crate::action::TextSurface;
    let mut app = App::new_for_test();
    app.editor.url = crate::components::line_input::LineInput::new(
        "https://x/ping/3f2504e0-4f89-11d3-9a0c-0305e82c3301",
    );
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    select_in_url(&mut app, "3f2504e0-4f89-11d3-9a0c-0305e82c3301");
    app.update(Action::ExtractSelectionToSelector(TextSurface::Url));
    let Some(Modal::MultiPrompt { fields, .. }) = app.modals.top() else {
        panic!("expected the extract-selector prompt");
    };
    assert_eq!(fields[1].input.text(), "");
}

#[test]
fn extract_selector_from_a_url_selection_creates_the_selector_its_option_and_selects_it() {
    use crate::action::{ExtractSource, TextSurface};
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    postui_core::storage::save_request(dir.path(), "main/ping", &req("https://x/ping/east"))
        .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::ForceOpenRequest("main/ping".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    select_in_url(&mut app, "east");

    app.update(Action::ConfirmExtractToSelector {
        name: "region".into(),
        option: "us-east".into(),
        shared: false,
        source: ExtractSource::Selection(TextSurface::Url),
    });

    assert!(app.modals.is_empty());
    assert_eq!(app.editor.url.text(), "https://x/ping/{{region}}");
    let vars = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(vars.contains("[selectors.region]"), "{vars}");
    assert!(vars.contains("fields = [\"region\"]"), "{vars}");
    assert!(!vars.contains("[options.region"), "not shared: {vars}");
    let env = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(env.contains("[options.region.us-east]"), "{env}");
    assert!(env.contains("region = \"east\""), "{env}");
    assert_eq!(
        app.project
            .selections_for("qa")
            .get("region")
            .map(String::as_str),
        Some("us-east"),
        "the new option is selected so the token resolves"
    );
    assert_eq!(app.project.resolved.values["region"], "east");
    assert!(
        app.toasts
            .messages()
            .iter()
            .any(|m| m.contains("extracted to {{region}}")),
        "{:?}",
        app.toasts.messages()
    );
}

#[test]
fn extract_selector_shared_puts_the_option_in_variables_toml() {
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.editor.url = LineInput::new("v2");
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;

    app.update(Action::ExtractToSelector);
    assert!(matches!(
        app.modals.top(),
        Some(Modal::MultiPrompt {
            kind: PromptKind::ExtractSelector(crate::action::ExtractSource::FocusedField),
            ..
        })
    ));
    type_into_field(&mut app, &keymap, "api_version");
    app.handle_key(&keymap, tab_key()); // option, seeded "v2"
    app.handle_key(&keymap, tab_key()); // scope
    app.handle_key(&keymap, right_key()); // Shared
    app.handle_key(&keymap, enter_key());

    assert!(app.modals.is_empty(), "{:?}", app.toasts.messages());
    assert_eq!(app.editor.url.text(), "{{api_version}}");
    let vars = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    assert!(vars.contains("[selectors.api_version]"), "{vars}");
    assert!(vars.contains("shared = true"), "{vars}");
    assert!(vars.contains("[options.api_version.v2]"), "{vars}");
    assert!(vars.contains("api_version = \"v2\""), "{vars}");
    let env = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();
    assert!(!env.contains("api_version"), "{env}");
    assert_eq!(
        app.project
            .shared_selections()
            .get("api_version")
            .map(String::as_str),
        Some("v2")
    );
    assert_eq!(app.project.resolved.values["api_version"], "v2");
}

#[test]
fn extract_selector_refuses_a_taken_name_and_leaves_everything_alone() {
    use crate::action::{ExtractSource, TextSurface};
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.editor.url = LineInput::new("https://x/ping/east");
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    select_in_url(&mut app, "east");
    let vars_before = std::fs::read_to_string(dir.path().join("variables.toml")).unwrap();
    let env_before = std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap();

    for taken in ["user", "base_url"] {
        app.update(Action::ConfirmExtractToSelector {
            name: taken.into(),
            option: "x".into(),
            shared: false,
            source: ExtractSource::Selection(TextSurface::Url),
        });
    }

    assert_eq!(app.editor.url.text(), "https://x/ping/east");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("variables.toml")).unwrap(),
        vars_before
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("environments/qa.toml")).unwrap(),
        env_before
    );
    let errors = app
        .toasts
        .messages()
        .iter()
        .filter(|m| m.contains("already exists"))
        .count();
    assert_eq!(errors, 2, "{:?}", app.toasts.messages());
}

#[test]
fn row_menu_extract_value_to_selector_promotes_the_row_and_replaces_the_cell() {
    use crate::action::ExtractSource;
    let dir = tempfile::tempdir().unwrap();
    var_project(dir.path());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.editor.params.insert(
        "tenant".into(),
        postui_core::model::Entry {
            value: "acme".into(),
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
    assert_eq!(
        menu_labels(&app),
        vec![
            "Duplicate row",
            "Delete param",
            "Extract value to variable\u{2026}",
            "Extract value to selector\u{2026}"
        ]
    );
    let extract = menu_action(&app, "Extract value to selector\u{2026}").unwrap();
    assert_eq!(extract, Action::ExtractToSelector);
    app.update(Action::Close);
    app.update(extract);
    let Some(Modal::MultiPrompt { kind, fields, .. }) = app.modals.top() else {
        panic!("expected the extract-selector prompt");
    };
    assert!(matches!(
        kind,
        PromptKind::ExtractSelector(ExtractSource::FocusedField)
    ));
    assert_eq!(fields[1].input.text(), "acme");
    app.modals.pop();

    app.update(Action::ConfirmExtractToSelector {
        name: "tenant".into(),
        option: "acme".into(),
        shared: false,
        source: ExtractSource::FocusedField,
    });
    assert_eq!(app.editor.params["tenant"].value, "{{tenant}}");
    assert!(
        app.editor.table.editing.is_none(),
        "the cell edit is committed"
    );
    assert_eq!(app.project.resolved.values["tenant"], "acme");
}

// --- jq filter bar ---

const JQ_BODY: &str = r#"{"data":{"items":[{"id":1,"status":"active"},{"id":2,"status":"off"}],"total":2}}"#;

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.handle_key(&Keymap::default_bindings(), plain(c));
    }
}

#[test]
fn alt_q_focuses_the_jq_bar_and_typing_filters_the_tree_live() {
    let mut app = App::new_for_test();
    postui_core::storage::save_request(&app.project.root, "main/r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/r".into()));
    ready_response(&mut app, JQ_BODY);
    assert!(app.handle_key(&Keymap::default_bindings(), alt('q')));
    assert_eq!(app.focus, PaneId::Response);
    assert!(app.session.response.jq_focused());
    type_str(&mut app, ".data.total");
    assert_eq!(app.session.response.view().unwrap().view_text(), "2");
    assert_eq!(app.editor.jq, ".data.total", "the bar mirrors into the request");
    assert!(app.editor.is_dirty());
    app.handle_key(&Keymap::default_bindings(), alt('q'));
    assert!(!app.session.response.jq_focused(), "alt+q again blurs");
}

#[test]
fn a_saved_filter_is_applied_when_the_request_opens_and_when_a_response_lands() {
    let mut app = App::new_for_test();
    app.editor.jq = ".data.total".into();
    ready_response(&mut app, JQ_BODY);
    app.update(Action::Render); // any update runs the reconcile
    assert_eq!(app.session.response.jq_text(), ".data.total");
    assert_eq!(app.session.response.view().unwrap().view_text(), "2");
}

#[test]
fn undo_restores_the_previous_filter_text_in_the_bar() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.handle_key(&Keymap::default_bindings(), alt('q'));
    type_str(&mut app, ".data");
    app.capture_undo();
    app.no_coalesce = true;
    type_str(&mut app, ".total");
    app.capture_undo();
    app.update(Action::Undo);
    assert_eq!(app.editor.jq, ".data");
    assert_eq!(
        app.session.response.jq_text(),
        ".data",
        "the bar follows the editor after undo"
    );
}

#[test]
fn jq_apply_and_tee_up_drive_the_bar() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.update(Action::JqApply(".data.items | length".into()));
    assert_eq!(app.session.response.view().unwrap().view_text(), "2");
    assert!(!app.session.response.jq_focused(), "apply does not focus the bar");
    app.update(Action::JqTeeUp {
        text: ".data.items | map(select(.status == ))".into(),
        cursor: 37,
    });
    assert!(app.session.response.jq_focused());
    assert_eq!(app.session.response.jq_bar().input.cursor(), 37);
    assert!(
        app.session.response.jq_bar().error.is_some(),
        "an unfinished tee-up is a syntax error until typed into"
    );
    assert_eq!(
        app.session.response.view().unwrap().view_text(),
        "2",
        "…and the previous tree stays"
    );
}

#[test]
fn paste_goes_to_the_focused_jq_bar() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.handle_key(&Keymap::default_bindings(), alt('q'));
    assert!(app.paste_text(".data.total"));
    assert_eq!(app.session.response.jq_text(), ".data.total");
}

#[test]
fn a_big_body_runs_the_filter_in_the_background_and_lands_via_an_action() {
    let mut app = App::new_for_test();
    let big = format!(
        r#"{{"pad": "{}", "n": 7}}"#,
        "x".repeat(crate::components::response::SYNC_PRETTY_BYTES)
    );
    ready_response(&mut app, &big);
    // Big bodies parse in the background; feed the tree in as the app would.
    let tree = crate::components::json_tree::JsonTree::parse(&big);
    app.update(Action::PrettyParsed {
        generation: app.session.send_generation,
        tree: tree.map(Box::new),
    });
    app.update(Action::ResponseViewMode(
        crate::components::response::ViewMode::Pretty,
    ));
    // `App::new_for_test` builds its channel outside a tokio runtime, so
    // the background run happens inline (`jq_worker`, dispatched
    // immediately) rather than through `tokio::spawn` — the result lands
    // within this one `update` call.
    app.update(Action::JqApply(".n".into()));
    assert_eq!(app.session.response.view().unwrap().view_text(), "7");
    assert!(app.session.response.jq_bar().pending.is_none());

    // The cached document (handed back by the first run) means the next
    // run needs no body — checked directly against `apply_jq`, the
    // interface `sync_jq` drives.
    let req2 = app
        .session
        .response
        .apply_jq(".pad | length", crate::components::response::SYNC_PRETTY_BYTES)
        .expect("still a background-sized run");
    assert!(req2.doc.is_some(), "the doc from the first run is cached");
    assert!(req2.body.is_none(), "…so no body needs to be handed to the worker");

    // Direct coverage of the worker itself: parses the body when handed no
    // cached document, and runs the filter against it.
    let action = crate::app::jq_worker(
        app.session.send_generation,
        req2.run,
        ".n".into(),
        None,
        Some(big.clone()),
    );
    let Action::JqRunFinished {
        result: Ok((Some(_), outputs)),
        ..
    } = &action
    else {
        panic!("the worker parses the body and hands the document back: {action:?}");
    };
    assert_eq!(outputs, &["7".to_string()]);
}

#[test]
fn the_footer_and_palette_reach_the_jq_bar() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.focus = PaneId::Response;
    let chips = crate::components::footer::footer_chips(
        PaneId::Response,
        false,
        false,
        None,
        false,
        None,
        false,
    );
    assert!(
        chips
            .iter()
            .any(|(k, l, a)| *k == "alt+q" && *l == "jq" && *a == Some(Action::ToggleJqBar)),
        "{chips:?}"
    );
    app.update(Action::OpenPalette);
    type_str(&mut app, "jq filter");
    select_palette_command(&mut app, "response-jq");
    app.handle_key(
        &Keymap::default_bindings(),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(app.session.response.jq_focused());
}

#[test]
fn jq_is_a_no_op_on_a_non_json_response() {
    let mut app = App::new_for_test();
    ready_response(&mut app, "plain");
    assert!(app.handle_key(&Keymap::default_bindings(), alt('q')));
    assert!(!app.session.response.jq_focused());
    assert_eq!(app.toasts.messages().len(), 1, "a toast says the response is not JSON");
}

// --- structural (jq) right-click menu ---

fn row_containing(app: &App, needle: &str) -> usize {
    let tree = app.session.response.active_tree().expect("tree");
    tree.visible_lines()
        .iter()
        .position(|l| l.plain_text().contains(needle))
        .unwrap_or_else(|| panic!("no visible row containing {needle:?}"))
}

fn right_click_row(app: &mut App, needle: &str) {
    render_once(app);
    let row = row_containing(app, needle);
    // The response pane's default split may be too short to show every
    // row of a deep tree at once — scroll the row into view (a no-op when
    // it is already visible) before re-rendering to pick up its hit.
    if app.hits.rect_of(&Hit::JsonRow(row)).is_none() {
        app.session.response.set_scroll(row);
        render_once(app);
    }
    let rect = app.hits.rect_of(&Hit::JsonRow(row)).expect("row is on screen");
    app.handle_mouse(right_down(rect.x + 1, rect.y));
}

#[test]
fn right_click_on_an_array_line_offers_the_array_verbs_above_the_text_menu() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    right_click_row(&mut app, "\"items\"");
    let labels = menu_labels(&app);
    assert_eq!(
        labels,
        vec![
            "Filter to this", "Copy path", "Count", "Pluck field\u{2026}", "Where field\u{2026}",
            "Describe a filter\u{2026}", "\u{2500}\u{2500}", // a disabled rule between the sections
            "Copy", "Extract to variable\u{2026}", "Extract to selector\u{2026}",
        ],
        "{labels:?}"
    );
    assert_eq!(menu_action(&app, "Filter to this"), Some(Action::JqApply(".data.items".into())));
    assert_eq!(menu_action(&app, "Copy path"), Some(Action::CopyJqPath(".data.items".into())));
    assert_eq!(menu_action(&app, "Count"), Some(Action::JqApply(".data.items | length".into())));
    assert_eq!(
        menu_action(&app, "Pluck field\u{2026}"),
        Some(Action::JqPluckPrompt { path: ".data.items".into(), keys: vec!["id".into(), "status".into()] })
    );
}

#[test]
fn a_scalar_inside_an_array_element_offers_only_items_where() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    right_click_row(&mut app, "\"status\": \"active\"");
    let labels = menu_labels(&app);
    assert!(labels.contains(&"Only items where status == \"active\"".to_string()), "{labels:?}");
    assert!(!labels.contains(&"Count".to_string()));
    assert_eq!(
        menu_action(&app, "Only items where status == \"active\""),
        Some(Action::JqApply(r#".data.items | map(select(.status == "active"))"#.into()))
    );
    app.update(Action::Close);
    right_click_row(&mut app, "\"total\"");
    assert!(!menu_labels(&app).iter().any(|l| l.starts_with("Only items")), "no enclosing array");
}

#[test]
fn verbs_compose_onto_an_existing_filter_with_relative_paths() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.update(Action::JqApply(".data".into()));
    right_click_row(&mut app, "\"items\"");
    assert_eq!(menu_action(&app, "Count"), Some(Action::JqApply(".data | .items | length".into())));
    app.update(Action::Close);
    app.update(Action::JqApply(".data.items".into()));
    right_click_row(&mut app, "[");
    assert_eq!(menu_action(&app, "Count"), Some(Action::JqApply(".data.items | length".into())), "a root path is dropped");
}

#[test]
fn with_several_outputs_the_composing_verbs_are_disabled_and_collect_is_offered() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.update(Action::JqApply(".data.items[]".into()));
    right_click_row(&mut app, "\"id\": 1");
    let labels = menu_labels(&app);
    assert!(labels.contains(&"Collect into array".to_string()), "{labels:?}");
    assert_eq!(menu_action(&app, "Filter to this  (collect into array first)"), None, "disabled, with the hint in its label");
    let collect = menu_action(&app, "Collect into array").unwrap();
    app.update(Action::Close);
    app.update(collect);
    assert_eq!(app.session.response.jq_text(), "[ .data.items[] ]");
    assert_eq!(app.session.response.jq_output_count(), 1);
}

#[test]
fn pluck_and_where_open_a_key_chooser_that_writes_the_verb() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.update(Action::JqPluckPrompt { path: ".data.items".into(), keys: vec!["id".into(), "status".into()] });
    let Some(Modal::Chooser(c)) = app.modals.top() else { panic!("chooser") };
    assert_eq!(c.title, "Pluck field");
    let pick = c.items[1].actions.clone();
    app.update(Action::Close);
    for a in pick { app.update(a); }
    assert_eq!(app.session.response.jq_text(), ".data.items | map(.status)");
    assert_eq!(app.session.response.view().unwrap().view_text(), "[\n  \"active\",\n  \"off\"\n]");

    app.update(Action::JqApply("".into()));
    app.update(Action::JqWherePrompt { path: ".data.items".into(), keys: vec!["status".into()] });
    let Some(Modal::Chooser(c)) = app.modals.top() else { panic!("chooser") };
    let pick = c.items[0].actions.clone();
    app.update(Action::Close);
    for a in pick { app.update(a); }
    assert_eq!(app.session.response.jq_text(), ".data.items | map(select(.status == ))");
    assert!(app.session.response.jq_focused());
    assert_eq!(app.session.response.jq_bar().input.cursor(), ".data.items | map(select(.status == ".len());
}

#[test]
fn the_structural_menu_is_absent_on_raw_and_non_json_views() {
    let mut app = App::new_for_test();
    ready_response(&mut app, JQ_BODY);
    app.update(Action::ResponseViewMode(crate::components::response::ViewMode::Raw));
    render_once(&mut app);
    let area = app.session.response.view().unwrap().last_area.unwrap();
    app.handle_mouse(right_down(area.x + 1, area.y));
    assert_eq!(menu_labels(&app)[0], "Copy");
}
