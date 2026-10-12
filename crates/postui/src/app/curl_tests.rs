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
