//! Curl import and export at the app level (spec
//! `docs/superpowers/specs/2026-10-11-curl-import-export-design.md`).

use super::key_matrix_tests::{draw, feed};
use super::*;
use crate::components::line_input::LineInput;
use crate::components::modal::{Modal, PromptKind};
use crate::config::KeyProfile;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn req(url: &str) -> postui_core::model::HttpRequest {
    postui_core::model::HttpRequest::from_toml_str(&format!(r#"url = "{url}""#)).unwrap()
}

/// A test app (space `main`) holding `main/top` and `main/api/ping`, whose
/// clipboard reads `clip`.
fn app_with_clip(clip: &str) -> App {
    let mut app = App::new_for_test();
    postui_core::fixtures::save_request(app.proj().root(), "main/top", &req("https://x/top")).unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/api/ping", &req("https://x/ping")).unwrap();
    app.update(Action::RefreshSidebar);
    let mut c = crate::clipboard::Clipboard::new_for_test(None, 65536, false);
    c.set_read_for_test(clip);
    app.set_clipboard_for_test(c);
    app
}

/// The open import prompt's (title, text).
fn prompt(app: &App) -> (String, String) {
    match app.modals.top() {
        Some(Modal::Prompt { title, input, kind: PromptKind::ImportCurl(_), .. }) => {
            (title.clone(), input.text().to_string())
        }
        _ => panic!("expected the import prompt"),
    }
}

fn set_prompt(app: &mut App, text: &str) {
    if let Some(Modal::Prompt { input, .. }) = app.modals.top_mut() {
        *input = LineInput::new(text);
    }
}

/// The slug of the request whose display name is `name`.
fn slug_named(app: &App, name: &str) -> Option<String> {
    app.proj()
        .requests()
        .iter()
        .find(|l| l.name.as_deref() == Some(name))
        .map(|l| l.slug.clone())
}

fn import(app: &mut App) {
    app.update(Action::ImportCurl { folder: None });
}

#[test]
fn palette_import_prompts_with_the_suggested_name_then_creates_and_opens() {
    let mut app = app_with_clip(r#"curl -X POST https://api.x/users -d '{"a":1}'"#);
    import(&mut app);
    assert_eq!(prompt(&app), ("Import curl".into(), "POST users".into()));
    app.update(Action::ConfirmTopModal);
    assert!(app.modals.is_empty());
    let slug = slug_named(&app, "POST users").expect("created");
    assert_eq!(slug, "main/post-users");
    assert_eq!(app.editor.slug.as_deref(), Some(slug.as_str()));
    let saved = postui_core::fixtures::load_request(app.proj().root(), &slug).unwrap();
    assert_eq!(saved.method, postui_core::model::Method::Post);
    assert_eq!(saved.body, Some(postui_core::model::Body::Json { text: r#"{"a":1}"#.into() }));
}

#[test]
fn the_palette_lists_import_curl() {
    let cmd = crate::components::palette::all_commands()
        .into_iter()
        .find(|c| c.id == "request-import-curl")
        .expect("palette command");
    assert_eq!(cmd.name, "Request: import curl from clipboard");
    assert_eq!(cmd.action, Action::ImportCurl { folder: None });
}

#[test]
fn a_folder_import_prefills_the_folder() {
    let mut app = app_with_clip("curl https://x/users");
    app.update(Action::ImportCurl { folder: Some("main/api".into()) });
    assert_eq!(prompt(&app).1, "api/GET users");
    app.update(Action::ConfirmTopModal);
    assert!(slug_named(&app, "GET users").unwrap().starts_with("main/api/"));
}

#[test]
fn a_taken_name_prefills_the_next_free_one() {
    let mut taken = req("https://x/u");
    taken.name = Some("GET users".into());
    let mut app = app_with_clip("curl https://x/users");
    postui_core::fixtures::save_request(app.proj().root(), "main/get-users", &taken).unwrap();
    app.update(Action::RefreshSidebar);
    import(&mut app);
    assert_eq!(prompt(&app).1, "GET users 2");
}

#[test]
fn one_undo_removes_the_import() {
    let mut app = app_with_clip("curl https://x/users");
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    assert!(slug_named(&app, "GET users").is_some());
    app.update(Action::Undo);
    assert!(slug_named(&app, "GET users").is_none());
    assert!(app.proj().request_exists("main/top"), "nothing else went");
}

#[test]
fn esc_creates_nothing() {
    let mut app = app_with_clip("curl https://x/users");
    let before = app.proj().requests().len();
    import(&mut app);
    app.update(Action::CancelTopModal);
    assert!(app.modals.is_empty());
    assert_eq!(app.proj().requests().len(), before);
}

#[test]
fn a_name_taken_on_enter_keeps_the_prompt() {
    let mut app = app_with_clip("curl https://x/users");
    import(&mut app);
    set_prompt(&mut app, "top");
    app.update(Action::ConfirmTopModal);
    assert_eq!(prompt(&app), ("Import curl".into(), "top".into()));
    assert!(app.toasts.last_message().unwrap().contains("already exists"));
}

#[test]
fn ignored_flags_show_in_the_summary() {
    let mut app = app_with_clip("curl --compressed -L https://x/a");
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert_eq!(msg, "ignored: --compressed, -L");
    assert_eq!(*kind, crate::components::toast::ToastKind::Warning);
}

#[test]
fn a_batch_runs_one_prompt_per_command_and_each_create_is_its_own_undo() {
    let mut app = app_with_clip("curl https://x/a\nexport T=1\ncurl -H \"A: $T\" https://x/b\ncurl https://x/c");
    import(&mut app);
    assert_eq!(prompt(&app), ("Import curl (1 of 3)".into(), "GET a".into()));
    app.update(Action::ConfirmTopModal);
    assert_eq!(prompt(&app).0, "Import curl (2 of 3)");
    app.update(Action::CancelTopModal);
    assert_eq!(prompt(&app).0, "Import curl (3 of 3)");
    app.update(Action::ConfirmTopModal);
    assert!(app.modals.is_empty());
    let a = slug_named(&app, "GET a").expect("a created");
    let c = slug_named(&app, "GET c").expect("c created");
    assert!(slug_named(&app, "GET b").is_none(), "b was skipped");
    assert_eq!(app.editor.slug.as_deref(), Some(c.as_str()), "the last created is open");
    assert_eq!(app.toasts.last_message(), Some("imported 2 of 3; skipped 1 non-curl line"));
    app.update(Action::Undo);
    assert!(!app.proj().request_exists(&c));
    assert!(app.proj().request_exists(&a));
}

#[test]
fn prefills_stay_unique_across_the_batch() {
    let mut app = app_with_clip("curl https://x/a\ncurl https://y/a");
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    assert_eq!(prompt(&app).1, "GET a 2");
}

#[test]
fn a_parse_error_imports_nothing() {
    let mut app = app_with_clip("curl https://x/a\ncurl -H");
    import(&mut app);
    assert!(app.modals.is_empty());
    assert_eq!(app.toasts.last_message(), Some("command 2: -H needs a value"));
}

#[test]
fn a_clipboard_without_curl_says_so() {
    let mut app = app_with_clip("hello there");
    import(&mut app);
    assert!(app.modals.is_empty());
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
}

#[test]
fn import_over_unsaved_edits_asks_first_and_esc_keeps_them() {
    let mut app = app_with_clip("curl https://x/a");
    app.update(Action::ForceOpenRequest("main/top".into()));
    app.editor.url = LineInput::new("https://x/edited");
    import(&mut app);
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { title, .. }) if title == "Unsaved changes"));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(prompt(&app).0, "Import curl");
    app.update(Action::CancelTopModal);
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.slug.as_deref(), Some("main/top"));
    assert_eq!(app.editor.url.text(), "https://x/edited", "nothing replaced the edit");
}

#[test]
fn the_folder_menu_offers_import_after_new_request_here() {
    let mut app = app_with_clip("");
    let i = app
        .sidebar
        .rows
        .iter()
        .position(|r| matches!(r, Row::Folder { path, .. } if path == "main/api"))
        .expect("folder row");
    let items = app.context_menu_for(&Hit::SidebarRow(i)).expect("menu");
    let labels: Vec<&str> = items.iter().map(|m| m.label.as_str()).collect();
    let new_here = labels.iter().position(|l| *l == "New request here\u{2026}").unwrap();
    assert_eq!(labels[new_here + 1], "Import curl here\u{2026}");
    assert_eq!(items[new_here + 1].action, Some(Action::ImportCurl { folder: Some("main/api".into()) }));
}

fn click(app: &mut App, hit: Hit) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    draw(app);
    let r = app.hits.rect_of(&hit).unwrap_or_else(|| panic!("no {hit:?}"));
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    });
}

#[test]
fn a_click_outside_skips_to_the_next_command() {
    let mut app = app_with_clip("curl https://x/a\ncurl https://x/b");
    import(&mut app);
    assert_eq!(prompt(&app).0, "Import curl (1 of 2)");
    click(&mut app, Hit::ModalOutside);
    assert_eq!(prompt(&app).0, "Import curl (2 of 2)");
    assert!(slug_named(&app, "GET a").is_none());
}

#[test]
fn a_click_outside_the_send_time_secret_prompt_cancels_the_send_like_esc() {
    let mut app = app_with_clip("");
    app.push_modal(Modal::Prompt {
        title: "Secret".into(),
        input: LineInput::new(""),
        kind: PromptKind::SecretValue { name: "token".into(), env: "dev".into(), then_send: true },
        revealed: false,
    });
    click(&mut app, Hit::ModalOutside);
    assert!(app.modals.is_empty());
    assert_eq!(app.toasts.last_message(), Some("send canceled"));
}

#[test]
fn the_cancel_button_skips_to_the_next_command() {
    let mut app = app_with_clip("curl https://x/a\ncurl https://x/b");
    import(&mut app);
    assert_eq!(prompt(&app).0, "Import curl (1 of 2)");
    click(&mut app, Hit::ModalCancel);
    assert_eq!(prompt(&app).0, "Import curl (2 of 2)");
    assert!(slug_named(&app, "GET a").is_none());
}

#[test]
fn esc_skips_to_the_next_command_in_both_profiles() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = app_with_clip("curl https://x/a\ncurl https://x/b");
        app.use_profile(profile);
        draw(&mut app);
        import(&mut app);
        draw(&mut app);
        assert_eq!(prompt(&app).0, "Import curl (1 of 2)", "{profile:?}");
        // Arrows may first close the field, then aim the buttons; vim goes
        // Insert → Normal → cancel. Press until the prompt changes.
        for _ in 0..3 {
            feed(&mut app, "<Esc>");
            draw(&mut app);
            if prompt(&app).0 != "Import curl (1 of 2)" {
                break;
            }
        }
        assert_eq!(prompt(&app).0, "Import curl (2 of 2)", "{profile:?}");
        assert!(slug_named(&app, "GET a").is_none(), "{profile:?}: a was skipped");
        if profile == KeyProfile::Vim {
            assert_eq!(app.mode_indicator(), Some("INSERT"), "the next prompt opens in Insert");
        }
    }
}

/// A test app whose clipboard writes land in the returned file.
fn copy_app() -> (App, tempfile::TempDir, std::path::PathBuf) {
    let mut app = App::new_for_test();
    postui_core::fixtures::save_request(app.proj().root(), "main/top", &req("https://x/top")).unwrap();
    let other = postui_core::model::HttpRequest::from_toml_str(
        "method = \"POST\"\nurl = \"https://x/other\"\n[headers]\nX-A = \"1\"\n",
    )
    .unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/other", &other).unwrap();
    app.update(Action::RefreshSidebar);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("clip.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(Some(cmd), 65536, false));
    (app, dir, out)
}

fn copied(out: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(out).ok()
}

#[test]
fn copy_as_curl_copies_the_open_request_with_unsaved_edits() {
    let (mut app, _dir, out) = copy_app();
    app.update(Action::ForceOpenRequest("main/top".into()));
    app.editor.url = LineInput::new("https://x/edited");
    app.update(Action::CopyAsCurl(None));
    assert_eq!(copied(&out).as_deref(), Some("curl 'https://x/edited'"));
    assert_eq!(app.toasts.last_message(), Some("Copied as curl"));
}

#[test]
fn copy_on_a_closed_request_reads_its_file_without_opening_it() {
    let (mut app, _dir, out) = copy_app();
    app.update(Action::ForceOpenRequest("main/top".into()));
    app.update(Action::CopyAsCurl(Some("main/other".into())));
    assert_eq!(
        copied(&out).as_deref(),
        Some("curl -X POST 'https://x/other' \\\n  -H 'X-A: 1'")
    );
    assert!(app.proj().held_request("main/other").is_none(), "not held");
    assert_eq!(app.editor.slug.as_deref(), Some("main/top"), "editor untouched");
}

#[test]
fn the_open_rows_copy_uses_the_live_editor() {
    let (mut app, _dir, out) = copy_app();
    app.update(Action::ForceOpenRequest("main/top".into()));
    app.editor.url = LineInput::new("https://x/edited");
    app.update(Action::CopyAsCurl(Some("main/top".into())));
    assert_eq!(copied(&out).as_deref(), Some("curl 'https://x/edited'"));
}

#[test]
fn an_empty_url_copies_nothing() {
    let (mut app, _dir, out) = copy_app();
    app.editor.url = LineInput::new("");
    app.update(Action::CopyAsCurl(None));
    assert_eq!(copied(&out), None);
    assert_eq!(app.toasts.last_message(), Some("cannot copy: URL is empty"));
}

#[test]
fn a_closed_request_with_an_empty_url_copies_nothing() {
    for url in ["", "   "] {
        let (mut app, _dir, out) = copy_app();
        postui_core::fixtures::save_request(app.proj().root(), "main/blank", &req(url)).unwrap();
        app.update(Action::RefreshSidebar);
        app.update(Action::ForceOpenRequest("main/top".into()));
        app.update(Action::CopyAsCurl(Some("main/blank".into())));
        assert_eq!(copied(&out), None, "{url:?}");
        let (msg, kind) = *app.toasts.entries().last().unwrap();
        assert_eq!(msg, "cannot copy: URL is empty");
        assert_eq!(*kind, crate::components::toast::ToastKind::Warning);
    }
}

#[test]
fn unresolved_variables_copy_nothing() {
    let (mut app, _dir, out) = copy_app();
    app.update(Action::ForceOpenRequest("main/top".into()));
    app.editor.url = LineInput::new("https://x/{{missing}}");
    app.update(Action::CopyAsCurl(None));
    assert_eq!(copied(&out), None);
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert!(msg.contains("missing"), "{msg}");
    assert_eq!(*kind, crate::components::toast::ToastKind::Error);
}

#[test]
fn a_resolved_secret_is_noted_on_the_toast() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::fixtures::init_project(dir.path(), Some("demo")).unwrap();
    postui_core::fixtures::ensure_project(dir.path()).unwrap(); // seeds the `main` space
    std::fs::write(dir.path().join("variables.toml"), "[api_key]\ndescription = \"service key\"\nsecret = true\n").unwrap();
    std::fs::write(dir.path().join("environments/qa.toml"), "").unwrap();
    postui_core::fixtures::save_local_state(
        dir.path(),
        &postui_core::project::LocalState { environment: Some("qa".into()), ..Default::default() },
    )
    .unwrap();
    let sec = postui_core::model::HttpRequest::from_toml_str(
        "url = \"https://x\"\n[headers]\nAuthorization = \"Bearer {{api_key}}\"\n",
    )
    .unwrap();
    postui_core::fixtures::save_request(dir.path(), "main/sec", &sec).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::VarEdit(crate::components::varmanager::VarEditOp::SetSecretValue {
        env: "qa".into(),
        name: "api_key".into(),
        value: "sk-1".into(),
    }));
    let out = dir.path().join("clip.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(Some(cmd), 65536, false));
    app.update(Action::CopyAsCurl(Some("main/sec".into())));
    assert!(copied(&out).unwrap().contains("Bearer sk-1"), "fully resolved (D2)");
    assert_eq!(app.toasts.last_message(), Some("Copied as curl \u{2014} includes secret values"));
}

#[test]
fn the_palette_lists_copy_as_curl() {
    let cmd = crate::components::palette::all_commands()
        .into_iter()
        .find(|c| c.id == "request-copy-curl")
        .expect("palette command");
    assert_eq!(cmd.name, "Request: copy as curl");
    assert_eq!(cmd.action, Action::CopyAsCurl(None));
}

#[test]
fn the_request_menu_offers_copy_as_curl_after_duplicate() {
    let (mut app, _dir, _out) = copy_app();
    let i = app.sidebar.row_of("main/other").expect("row");
    let items = app.context_menu_for(&Hit::SidebarRow(i)).expect("menu");
    let labels: Vec<&str> = items.iter().map(|m| m.label.as_str()).collect();
    let dup = labels.iter().position(|l| *l == "Duplicate").unwrap();
    assert_eq!(labels[dup + 1], "Copy as curl");
    assert_eq!(items[dup + 1].action, Some(Action::CopyAsCurl(Some("main/other".into()))));
}
