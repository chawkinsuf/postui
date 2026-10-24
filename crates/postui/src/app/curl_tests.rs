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
    postui_core::fixtures::save_request(app.proj().root(), "main/top", &req("https://x/top"))
        .unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/api/ping", &req("https://x/ping"))
        .unwrap();
    app.update(Action::RefreshSidebar);
    let mut c = crate::clipboard::Clipboard::new_for_test(None, 65536);
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
    app.proj().requests().iter().find(|l| l.name.as_deref() == Some(name)).map(|l| l.slug.clone())
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
fn a_name_comment_prefills_the_prompt_and_its_folder_is_a_folder() {
    let mut app = app_with_clip("# @name api/List users\ncurl https://x/users");
    import(&mut app);
    assert_eq!(prompt(&app).1, "api/List users");
    app.update(Action::ConfirmTopModal);
    assert!(slug_named(&app, "List users").unwrap().starts_with("main/api/"));
    // R8 numbers the leaf, in the folder the name points into.
    let mut app = app_with_clip("# @name api/ping\ncurl https://x/p");
    app.update(Action::ImportCurl { folder: Some("main".into()) });
    assert_eq!(prompt(&app).1, "api/ping 2");
    // The folder is read as the create reads it (slugged, the leaf
    // trimmed), and the name keeps its spelling.
    let mut app = app_with_clip("# @name API// ping\ncurl https://x/p");
    import(&mut app);
    assert_eq!(prompt(&app).1, "API// ping 2");
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
    let mut app = app_with_clip(
        "curl https://x/a\nexport T=1\ncurl -H \"A: $T\" https://x/b\ncurl https://x/c",
    );
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
    // `export T=1` is read, not skipped; only `b` used T, and it was skipped.
    assert_eq!(app.toasts.last_message(), Some("imported 2 of 3"));
    app.update(Action::Undo);
    assert!(!app.proj().request_exists(&c));
    assert!(app.proj().request_exists(&a));
}

#[test]
fn an_exported_value_is_saved_as_a_secret_and_one_undo_takes_it_back() {
    let mut app =
        app_with_clip("export TOKEN=abc123\ncurl -H \"Authorization: Bearer $TOKEN\" https://x/me");
    let env = app.proj().active_env().map(|e| app.proj().env_name(e));
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    let p = app.proj();
    assert!(p.variables().vars["TOKEN"].secret);
    assert_eq!(
        p.resolved().values.get("TOKEN").map(String::as_str),
        Some("abc123"),
        "Send resolves it"
    );
    let mut expected = "converted $TOKEN \u{2192} {{TOKEN}}; saved TOKEN as a secret".to_string();
    if let Some(env) = env {
        expected.push_str(&format!(" in {env}"));
    }
    assert_eq!(app.toasts.last_message(), Some(expected.as_str()));
    app.update(Action::Undo);
    assert!(slug_named(&app, "GET me").is_none());
    assert!(
        !app.proj().variables().vars.contains_key("TOKEN"),
        "the one undo took the variable too"
    );
    assert!(!app.proj().resolved().values.contains_key("TOKEN"));
}

#[test]
fn a_value_the_paste_cannot_know_is_declared_with_none() {
    let mut app = app_with_clip(
        "export TOKEN=$(get-token)\ncurl -H \"Authorization: Bearer $TOKEN\" https://x/me",
    );
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    let p = app.proj();
    assert!(p.variables().vars["TOKEN"].secret);
    assert_eq!(
        p.resolved().meta.get("TOKEN"),
        Some(&postui_core::varmodel::VarMeta::MissingSecret),
        "Send asks for it"
    );
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert_eq!(
        msg,
        "converted $TOKEN \u{2192} {{TOKEN}}; declared TOKEN as a secret with no value"
    );
    assert_eq!(*kind, crate::components::toast::ToastKind::Warning);
    app.update(Action::Undo);
    assert!(slug_named(&app, "GET me").is_none());
    assert!(
        !app.proj().variables().vars.contains_key("TOKEN"),
        "the one undo took the variable too"
    );
}

#[test]
fn a_variable_the_paste_never_assigns_is_declared_with_none() {
    let mut app = app_with_clip("curl -H \"Authorization: Bearer $TOKEN\" https://x/me");
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    let p = app.proj();
    assert!(p.variables().vars["TOKEN"].secret);
    assert_eq!(
        p.resolved().meta.get("TOKEN"),
        Some(&postui_core::varmodel::VarMeta::MissingSecret)
    );
    assert_eq!(
        app.toasts.last_message(),
        Some("converted $TOKEN \u{2192} {{TOKEN}}; declared TOKEN as a secret with no value")
    );
}

#[test]
fn a_later_value_for_a_name_declared_with_none_is_reported() {
    let mut app = app_with_clip(
        "export T=$(x)\ncurl -H \"A: $T\" https://x/a\nexport T=abc\ncurl -H \"A: $T\" https://x/b",
    );
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    app.update(Action::ConfirmTopModal);
    assert_eq!(
        app.proj().resolved().meta.get("T"),
        Some(&postui_core::varmodel::VarMeta::MissingSecret)
    );
    let msg = app.toasts.last_message().expect("a summary");
    assert!(
        msg.ends_with("; declared T as a secret with no value; did not save a later value of T"),
        "{msg}"
    );
}

#[test]
fn a_variable_the_project_has_is_left_alone() {
    let mut app = app_with_clip("export base=https://pasted\ncurl \"$base/me\"");
    std::fs::write(
        app.proj().root().join("variables.toml"),
        "[base]\ndefault = \"https://mine\"\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    assert!(slug_named(&app, "GET me").is_some());
    assert_eq!(app.proj().resolved().values.get("base").map(String::as_str), Some("https://mine"));
    assert!(!app.proj().variables().vars["base"].secret);
    assert_eq!(
        app.toasts.last_message(),
        Some("converted $base \u{2192} {{base}}; kept the existing base, not the paste's value"),
        "the request sends the project's value, not the curl's"
    );
    // The same value is no surprise.
    let mut app = app_with_clip("export base=https://mine\ncurl \"$base/me\"");
    std::fs::write(
        app.proj().root().join("variables.toml"),
        "[base]\ndefault = \"https://mine\"\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    assert_eq!(
        app.toasts.last_message(),
        Some("converted $base \u{2192} {{base}}; kept the existing base")
    );
}

#[test]
fn a_selector_field_is_kept_and_the_import_still_lands() {
    let mut app = app_with_clip("export user_id=5\ncurl \"https://x/users/$user_id\"");
    std::fs::write(
        app.proj().root().join("variables.toml"),
        "[selectors.creds]\nfields = [\"user_id\"]\n",
    )
    .unwrap();
    app.update(Action::ReloadProjectFiles);
    let before = app.proj().requests().len();
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    assert!(app.modals.is_empty(), "the create was not refused");
    assert_eq!(app.proj().requests().len(), before + 1);
    assert!(!app.proj().variables().vars.contains_key("user_id"));
    let msg = app.toasts.last_message().expect("a summary");
    assert!(msg.ends_with("; kept the existing user_id, not the paste's value"), "{msg}");
}

#[test]
fn a_value_only_an_environment_file_sets_is_kept() {
    // Declared as a secret, TOKEN would hide `other`'s own value.
    let mut app = app_with_clip("export TOKEN=old\ncurl -H \"A: $TOKEN\" https://x/me");
    std::fs::create_dir_all(app.proj().root().join("environments")).unwrap();
    std::fs::write(app.proj().root().join("environments/other.toml"), "TOKEN = \"real\"\n")
        .unwrap();
    app.update(Action::ReloadProjectFiles);
    assert!(app.proj().environments().iter().any(|e| e == "other"));
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    assert!(slug_named(&app, "GET me").is_some());
    assert!(!app.proj().variables().vars.contains_key("TOKEN"));
    let msg = app.toasts.last_message().expect("a summary");
    assert!(msg.contains("kept the existing TOKEN"), "{msg}");
    assert!(!msg.contains("saved TOKEN"), "{msg}");
}

#[test]
fn a_reserved_name_is_not_saved_and_not_called_existing() {
    let mut app = app_with_clip("export entries=5 T=1\ncurl \"https://x/me?n=$entries&t=$T\"");
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    assert!(slug_named(&app, "GET me").is_some(), "the other secret still lands");
    assert!(!app.proj().variables().vars.contains_key("entries"));
    assert_eq!(app.proj().resolved().values.get("T").map(String::as_str), Some("1"));
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert!(msg.ends_with("; did not save entries (a reserved name)"), "{msg}");
    assert!(!msg.contains("kept"), "{msg}");
    assert_eq!(*kind, crate::components::toast::ToastKind::Warning);
}

#[test]
fn a_later_command_given_another_value_is_told_it_kept_the_first() {
    let mut app = app_with_clip(
        "export T=1\ncurl -H \"A: $T\" https://x/a\nexport T=2\ncurl -H \"A: $T\" https://x/b",
    );
    import(&mut app);
    app.update(Action::ConfirmTopModal);
    app.update(Action::ConfirmTopModal);
    assert_eq!(app.proj().resolved().values.get("T").map(String::as_str), Some("1"));
    let msg = app.toasts.last_message().expect("a summary");
    assert!(msg.ends_with("; kept the first value of T"), "{msg}");
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
fn import_over_unsaved_edits_asks_first_and_discard_discards_even_if_every_prompt_is_skipped() {
    let mut app = app_with_clip("curl https://x/a");
    app.update(Action::ForceOpenRequest("main/top".into()));
    app.editor.url = LineInput::new("https://x/edited");
    app.capture_undo();
    import(&mut app);
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { title, .. }) if title == "Unsaved changes")
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    app.capture_undo();
    assert_eq!(prompt(&app).0, "Import curl");
    assert_eq!(app.editor.url.text(), "https://x/top", "Discard discarded");
    app.update(Action::CancelTopModal);
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.slug.as_deref(), Some("main/top"));
    assert!(!app.editor_holds_unsaved());
    app.update(Action::Undo);
    assert_eq!(app.editor.url.text(), "https://x/edited", "undo brings the edit back");
}

#[test]
fn discarding_a_scratch_request_for_an_import_clears_it() {
    let mut app = app_with_clip("curl https://x/a");
    app.editor.url = LineInput::new("https://x/scratch");
    app.capture_undo();
    assert!(app.editor_holds_unsaved());
    import(&mut app);
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { title, .. }) if title == "Unsaved request")
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    app.capture_undo();
    assert!(
        app.toasts.messages().iter().any(|m| m.starts_with("Request discarded")),
        "{:?}",
        app.toasts.messages()
    );
    app.update(Action::CancelTopModal);
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.url.text(), "");
    assert!(!app.editor_holds_unsaved());
    assert!(app.editor.saved.is_none(), "a scratch again, with nothing saved to compare to");
    app.update(Action::Undo);
    assert_eq!(app.editor.url.text(), "https://x/scratch", "undo brings the scratch back");
    assert!(app.editor.is_scratch_dirty());
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
    assert_eq!(
        items[new_here + 1].action,
        Some(Action::ImportCurl { folder: Some("main/api".into()) })
    );
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
fn vims_command_line_reaches_import_by_verb_and_by_the_first_ctrl_n() {
    for keys in [":import<CR>", ":imp<CR>", ":curl<C-n><CR>"] {
        let mut app = app_with_clip("curl https://x/users");
        app.use_profile(KeyProfile::Vim);
        draw(&mut app);
        feed(&mut app, keys);
        assert_eq!(prompt(&app).0, "Import curl", "{keys}");
    }
}

#[test]
fn vims_copycurl_verb_copies_the_open_request() {
    let (mut app, _dir, out) = copy_app();
    app.use_profile(KeyProfile::Vim);
    app.update(Action::ForceOpenRequest("main/top".into()));
    draw(&mut app);
    feed(&mut app, ":copycurl<CR>");
    assert_eq!(copied(&out).as_deref(), Some("curl 'https://x/top'"));
}

fn footer_labels(app: &App) -> Vec<String> {
    app.modals.footer_chips().unwrap().into_iter().map(|(_, l, _)| l).collect()
}

#[test]
fn a_batch_prompt_says_skip_and_a_single_one_says_cancel() {
    let mut app = app_with_clip("curl https://x/a\ncurl https://x/b");
    import(&mut app);
    let screen = super::tests::rendered_text_at(&mut app, 160, 45);
    assert!(screen.contains("Skip") && !screen.contains("Cancel"), "{screen}");
    assert!(footer_labels(&app).contains(&"skip".to_string()), "{:?}", footer_labels(&app));
    let mut one = app_with_clip("curl https://x/a");
    import(&mut one);
    let screen = super::tests::rendered_text_at(&mut one, 160, 45);
    assert!(screen.contains("Cancel") && !screen.contains("Skip"), "{screen}");
    assert!(footer_labels(&one).contains(&"cancel".to_string()));
}

#[test]
fn the_aimed_skip_button_is_named_skip() {
    let mut app = app_with_clip("curl https://x/a\ncurl https://x/b");
    import(&mut app);
    // ↓ closes the field onto the button row (Confirm aimed); ← aims Skip.
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    let labels = footer_labels(&app);
    assert!(
        labels.iter().filter(|l| *l == "skip").count() == 2,
        "enter and esc both skip: {labels:?}"
    );
}

#[test]
fn the_prompt_shows_the_method_url_and_ignored_flags() {
    let mut app = app_with_clip("curl --compressed -X POST https://x/items -d a=1");
    import(&mut app);
    let screen = super::tests::rendered_text_at(&mut app, 160, 45);
    assert!(screen.contains("POST https://x/items"), "{screen}");
    assert!(screen.contains("ignores --compressed"), "{screen}");
    let mut clean = app_with_clip("curl https://x/items");
    import(&mut clean);
    assert!(!super::tests::rendered_text_at(&mut clean, 160, 45).contains("ignores"));
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
    postui_core::fixtures::save_request(app.proj().root(), "main/top", &req("https://x/top"))
        .unwrap();
    let other = postui_core::model::HttpRequest::from_toml_str(
        "method = \"POST\"\nurl = \"https://x/other\"\n[headers]\nX-A = \"1\"\n",
    )
    .unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/other", &other).unwrap();
    app.update(Action::RefreshSidebar);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("clip.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(Some(cmd), 65536));
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
    assert_eq!(copied(&out).as_deref(), Some("curl -X POST 'https://x/other' \\\n  -H 'X-A: 1'"));
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
    std::fs::write(
        dir.path().join("variables.toml"),
        "[api_key]\ndescription = \"service key\"\nsecret = true\n",
    )
    .unwrap();
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
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(Some(cmd), 65536));
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

// Replace a request with curl (spec
// `docs/superpowers/specs/2026-10-14-replace-with-curl-design.md`).

fn replace(app: &mut App, slug: Option<&str>) {
    app.update(Action::ReplaceWithCurl(slug.map(str::to_string)));
    app.capture_undo();
}

/// `app_with_clip`, with `main/kept` open: a named request with params,
/// headers, a body, a jq filter and a request variable.
fn kept_app(clip: &str) -> App {
    let mut app = app_with_clip(clip);
    let kept = postui_core::model::HttpRequest::from_toml_str(
        "name = \"Kept\"\nurl = \"https://x/old\"\njq = \".a\"\n\
         [params]\np = \"1\"\n[headers]\nX-Old = \"1\"\n[variables]\nv = \"1\"\n",
    )
    .unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/kept", &kept).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/kept".into()));
    app.capture_undo();
    app
}

#[test]
fn replace_puts_the_curl_in_the_editor_and_keeps_name_jq_and_request_variables() {
    let mut app = kept_app(r#"curl -X POST -k 'https://x/new?q=1' -H 'X-New: 2' -d '{"b":2}'"#);
    let before = app.editor.current_request();
    replace(&mut app, None);
    let now = app.editor.current_request();
    assert_eq!(now.method, postui_core::model::Method::Post);
    assert_eq!(now.url, "https://x/new?q=1");
    assert!(now.params.is_empty(), "the query rides in the URL");
    assert_eq!(now.headers.keys().collect::<Vec<_>>(), ["X-New"]);
    assert_eq!(now.body, Some(postui_core::model::Body::Json { text: r#"{"b":2}"#.into() }));
    assert!(now.insecure);
    assert_eq!(now.name.as_deref(), Some("Kept"));
    assert_eq!(now.jq.as_deref(), Some(".a"));
    assert_eq!(now.variables.keys().collect::<Vec<_>>(), ["v"]);
    assert!(app.editor_holds_unsaved(), "unsaved until Save");
    let saved = postui_core::fixtures::load_request(app.proj().root(), "main/kept").unwrap();
    assert_eq!(saved.url, "https://x/old", "the file is untouched");
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert!(msg.starts_with("Replaced Kept from curl"), "{msg}");
    assert!(msg.contains("undoes"), "{msg}");
    assert_eq!(*kind, crate::components::toast::ToastKind::Success);
    app.update(Action::Undo);
    assert_eq!(app.editor.current_request(), before, "one undo restores it");
    assert!(!app.editor_holds_unsaved());
}

/// Spec 30, amendment A11: the views, the scratch, what is current and
/// the switch all ride through a replace.
#[test]
fn replace_with_curl_keeps_views_scratch_and_current() {
    let mut app = app_with_clip("curl https://x/new");
    let kept = postui_core::model::HttpRequest::from_toml_str(
        "url = \"https://x/old\"\n\n[[jq_view]]\nname = \"names\"\nfilter = \".items[].name\"\n",
    )
    .unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/kept", &kept).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/kept".into()));
    app.editor.jq = ".s".into();
    app.editor.jq_pick(Some("names"));
    app.capture_undo();
    let before = app.editor.current_request();
    replace(&mut app, None);
    let now = app.editor.current_request();
    assert_eq!(now.url, "https://x/new");
    assert_eq!(now.jq_views, before.jq_views);
    assert_eq!(now.jq.as_deref(), Some(".s"));
    assert_eq!(now.jq_current.as_deref(), Some("names"));
    assert!(now.jq_enabled);
}

#[test]
fn undo_restores_the_edits_first_then_the_declarations() {
    let mut app = kept_app("export TOKEN=abc\ncurl -H \"A: $TOKEN\" https://x/new");
    app.editor.url = LineInput::new("https://x/edited");
    app.capture_undo();
    let edited = app.editor.current_request();
    replace(&mut app, None);
    assert_eq!(app.editor.url.text(), "https://x/new");
    assert!(app.proj().variables().vars["TOKEN"].secret);
    assert_eq!(app.proj().resolved().values.get("TOKEN").map(String::as_str), Some("abc"));
    let msg = app.toasts.last_message().unwrap();
    assert!(
        msg.contains("; converted $TOKEN \u{2192} {{TOKEN}}; saved TOKEN as a secret"),
        "{msg}"
    );
    app.update(Action::Undo);
    assert_eq!(app.editor.current_request(), edited, "the unsaved edit is back");
    assert!(app.proj().variables().vars.contains_key("TOKEN"), "declarations are the next step");
    app.update(Action::Undo);
    assert!(!app.proj().variables().vars.contains_key("TOKEN"));
    assert_eq!(app.editor.current_request(), edited, "the editor stays as it was");
    let msg = app.toasts.last_message().unwrap();
    assert!(msg.contains("variable TOKEN"), "the toast names what it undid: {msg}");
}

#[test]
fn several_commands_refuse_and_change_nothing() {
    let mut app = kept_app("curl https://x/a\ncurl https://x/b");
    let before = app.editor.current_request();
    replace(&mut app, None);
    assert_eq!(app.editor.current_request(), before);
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert_eq!(msg, "clipboard holds 2 curl commands; replace takes one");
    assert_eq!(*kind, crate::components::toast::ToastKind::Error);
    let mut app = kept_app("hello there");
    replace(&mut app, None);
    assert_eq!(app.editor.current_request(), before);
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
}

#[test]
fn a_row_that_is_not_open_is_opened_through_the_dirty_gate_then_replaced() {
    let mut app = kept_app("curl https://x/new");
    app.editor.url = LineInput::new("https://x/edited");
    app.capture_undo();
    replace(&mut app, Some("main/top"));
    assert!(
        matches!(app.modals.top(), Some(Modal::Confirm { title, .. }) if title == "Unsaved changes")
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    app.capture_undo();
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.slug.as_deref(), Some("main/top"));
    assert_eq!(app.editor.url.text(), "https://x/new");
    assert!(app.toasts.last_message().unwrap().starts_with("Replaced top from curl"));
    // A clean editor goes straight through.
    app.update(Action::DiscardChanges);
    app.capture_undo();
    replace(&mut app, Some("main/api/ping"));
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.slug.as_deref(), Some("main/api/ping"));
    assert_eq!(app.editor.url.text(), "https://x/new");
    // The open and the replace share one input event; the replace is
    // still its own undo step.
    app.update(Action::Undo);
    assert_eq!(app.editor.slug.as_deref(), Some("main/api/ping"));
    assert_eq!(app.editor.url.text(), "https://x/ping");
    assert!(!app.editor_holds_unsaved());
}

#[test]
fn undo_after_replacing_a_row_that_was_not_open_restores_it_before_the_declarations() {
    let mut app = kept_app("export TOKEN=abc\ncurl -H \"A: $TOKEN\" https://x/new");
    replace(&mut app, Some("main/top"));
    assert_eq!(app.editor.slug.as_deref(), Some("main/top"));
    assert_eq!(app.editor.url.text(), "https://x/new");
    app.update(Action::Undo);
    assert_eq!(app.editor.url.text(), "https://x/top", "the editor change first");
    assert!(app.proj().variables().vars.contains_key("TOKEN"));
    app.update(Action::Undo);
    assert!(!app.proj().variables().vars.contains_key("TOKEN"));
    assert_eq!(app.editor.url.text(), "https://x/top");
}

#[test]
fn a_clipboard_without_curl_toasts_before_the_dirty_gate() {
    let mut app = kept_app("hello there");
    app.editor.url = LineInput::new("https://x/edited");
    app.capture_undo();
    replace(&mut app, Some("main/top"));
    assert!(app.modals.is_empty());
    assert_eq!(app.editor.slug.as_deref(), Some("main/kept"));
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
}

#[test]
fn a_request_variable_counts_as_kept() {
    let mut app = kept_app("export v=2\ncurl -H \"A: $v\" https://x/new");
    replace(&mut app, None);
    assert!(!app.proj().variables().vars.contains_key("v"), "not declared over it");
    let msg = app.toasts.last_message().unwrap();
    assert!(msg.contains("kept the existing v"), "{msg}");
}

#[test]
fn ignored_flags_make_the_replace_toast_a_warning() {
    let mut app = kept_app("curl --compressed https://x/new");
    replace(&mut app, None);
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert!(msg.starts_with("Replaced Kept from curl"), "{msg}");
    assert!(msg.contains("from curl; ignored: --compressed \u{2014} "), "{msg}");
    assert!(msg.ends_with(" undoes"), "{msg}");
    assert_eq!(*kind, crate::components::toast::ToastKind::Warning);
}

#[test]
fn a_scratch_request_is_replaced_in_place() {
    let mut app = app_with_clip("curl https://x/new");
    replace(&mut app, None);
    assert_eq!(app.editor.slug, None);
    assert_eq!(app.editor.url.text(), "https://x/new");
    assert!(
        app.toasts.last_message().unwrap().starts_with("Replaced the unsaved request from curl")
    );
}

#[test]
fn an_unreadable_environment_refuses_the_replace() {
    let mut app = kept_app("curl -H \"A: $OTHER\" https://x/new");
    let before = app.editor.current_request();
    std::fs::create_dir_all(app.proj().root().join("environments")).unwrap();
    std::fs::write(app.proj().root().join("environments/qa.toml"), "").unwrap();
    app.update(Action::ReloadProjectFiles);
    std::fs::write(app.proj().root().join("environments/qa.toml"), "TOKEN = [").unwrap();
    replace(&mut app, None);
    assert_eq!(app.editor.current_request(), before, "nothing changed");
    assert!(!app.proj().variables().vars.contains_key("OTHER"));
    let (_, kind) = *app.toasts.entries().last().unwrap();
    assert_eq!(*kind, crate::components::toast::ToastKind::Error);
}

#[test]
fn the_request_menu_offers_replace_after_copy_as_curl_and_the_palette_lists_it() {
    let (mut app, _dir, _out) = copy_app();
    let i = app.sidebar.row_of("main/other").expect("row");
    let items = app.context_menu_for(&Hit::SidebarRow(i)).expect("menu");
    let labels: Vec<&str> = items.iter().map(|m| m.label.as_str()).collect();
    let copy = labels.iter().position(|l| *l == "Copy as curl").unwrap();
    assert_eq!(labels[copy + 1], "Replace with curl");
    assert_eq!(items[copy + 1].action, Some(Action::ReplaceWithCurl(Some("main/other".into()))));
    let cmd = crate::components::palette::all_commands()
        .into_iter()
        .find(|c| c.id == "request-replace-curl")
        .expect("palette command");
    assert_eq!(cmd.name, "Request: replace with curl from clipboard");
    assert_eq!(cmd.action, Action::ReplaceWithCurl(None));
}

#[test]
fn a_broken_row_offers_no_replace() {
    let (mut app, _dir, _out) = copy_app();
    std::fs::write(app.proj().root().join("requests/main/broken.toml"), "url = [").unwrap();
    app.update(Action::RefreshSidebar);
    let i = app.sidebar.row_of("main/broken").expect("row");
    let items = app.context_menu_for(&Hit::SidebarRow(i)).expect("menu");
    assert!(items.iter().all(|m| m.label != "Replace with curl"));
}

#[test]
fn a_disabled_request_variable_is_declared_as_a_secret() {
    let mut app = kept_app("export v=2\ncurl -H \"A: $v\" https://x/new");
    app.editor.variables.get_mut("v").unwrap().enabled = false;
    app.capture_undo();
    replace(&mut app, None);
    assert!(app.proj().variables().vars["v"].secret, "a disabled one sends nothing");
    assert_eq!(app.proj().resolved().values.get("v").map(String::as_str), Some("2"));
}

#[test]
fn what_the_gate_answers_is_the_paste_read_before_it() {
    let mut app = kept_app("curl https://x/new");
    app.editor.url = LineInput::new("https://x/edited");
    app.capture_undo();
    replace(&mut app, Some("main/top"));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    let mut c = crate::clipboard::Clipboard::new_for_test(None, 65536);
    c.set_read_for_test("not curl any more");
    app.set_clipboard_for_test(c);
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    app.capture_undo();
    assert_eq!(app.editor.slug.as_deref(), Some("main/top"));
    assert_eq!(app.editor.url.text(), "https://x/new");
}

#[test]
fn an_unreadable_environment_refuses_before_the_gate_or_the_open() {
    let mut app = kept_app("curl -H \"A: $OTHER\" https://x/new");
    std::fs::create_dir_all(app.proj().root().join("environments")).unwrap();
    std::fs::write(app.proj().root().join("environments/qa.toml"), "").unwrap();
    app.update(Action::ReloadProjectFiles);
    app.editor.url = LineInput::new("https://x/edited");
    app.capture_undo();
    std::fs::write(app.proj().root().join("environments/qa.toml"), "TOKEN = [").unwrap();
    replace(&mut app, Some("main/top"));
    assert!(app.modals.is_empty(), "no gate for a replace that can't happen");
    assert_eq!(app.editor.slug.as_deref(), Some("main/kept"));
    assert_eq!(app.editor.url.text(), "https://x/edited");
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert!(msg.starts_with("cannot replace: "), "{msg}");
    assert_eq!(*kind, crate::components::toast::ToastKind::Error);
}

#[test]
fn an_open_cell_lands_before_the_variables_are_sorted() {
    use crate::components::table_editor::{CellEdit, Col};
    let mut app = kept_app("export v=9\ncurl -H \"A: $v\" https://x/new");
    app.focus = PaneId::Editor;
    app.editor.active_tab = crate::components::editor::EditorTab::Vars;
    app.editor.sub_focus = crate::components::editor::SubFocus::Content;
    app.editor.table.selected = Some(0);
    let input = LineInput::new("9");
    app.editor.table.editing = Some(CellEdit { row: 0, col: Col::Value, input });
    replace(&mut app, None);
    assert_eq!(app.editor.variables["v"].value, "9", "the typed value is kept");
    let msg = app.toasts.last_message().unwrap();
    assert!(msg.contains("; kept the existing v \u{2014} "), "the paste's value: {msg}");
}

#[test]
fn an_enabled_request_variable_needs_no_environment_read() {
    let mut app = kept_app("export v=1\ncurl -H \"A: $v\" https://x/new");
    std::fs::create_dir_all(app.proj().root().join("environments")).unwrap();
    std::fs::write(app.proj().root().join("environments/qa.toml"), "").unwrap();
    app.update(Action::ReloadProjectFiles);
    std::fs::write(app.proj().root().join("environments/qa.toml"), "TOKEN = [").unwrap();
    replace(&mut app, None);
    assert_eq!(app.editor.url.text(), "https://x/new", "v is the request's own");
}

#[test]
fn a_curl_the_request_already_matches_offers_no_undo() {
    let mut app = kept_app("curl https://x/new");
    replace(&mut app, None);
    replace(&mut app, None);
    let (msg, kind) = *app.toasts.entries().last().unwrap();
    assert_eq!(msg, "Nothing to replace: Kept already matches the curl");
    assert_eq!(*kind, crate::components::toast::ToastKind::Success);
    assert!(!app.no_coalesce, "the event's capture clears the flag the no-op set");
    app.update(Action::Undo);
    assert_eq!(app.editor.url.text(), "https://x/old", "the one undo is the first replace's");
}

#[test]
fn a_variable_declared_outside_the_app_is_kept() {
    let mut app = kept_app("export T=1\ncurl -H \"A: $T\" https://x/new");
    std::fs::write(app.proj().root().join("variables.toml"), "[T]\ndefault = \"1\"\n").unwrap();
    replace(&mut app, None);
    assert!(!app.proj().variables().vars["T"].secret, "not declared over the file's");
    let msg = app.toasts.last_message().unwrap();
    assert!(msg.contains("; kept the existing T"), "{msg}");
}

/// The shadow is seated by the replace, not by every open: reopening the
/// open request through the gate's Discard is still an undo step.
#[test]
fn reopening_the_open_request_through_discard_is_undoable() {
    let mut app = kept_app("");
    // Two fields, so the Discard's step is not merged into the typing's
    // as an instant same-field edit would be.
    app.editor.url = LineInput::new("https://x/edited");
    app.editor.method = postui_core::model::Method::Put;
    app.capture_undo();
    app.update(Action::OpenRequest("main/kept".into()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    app.capture_undo();
    assert_eq!(app.editor.url.text(), "https://x/old");
    app.update(Action::Undo);
    assert_eq!(app.editor.url.text(), "https://x/edited", "undo brings the edit back");
}

// Curl import visibility (spec 2026-10-15-curl-import-visibility-design.md).

/// `app_with_clip` with the sidebar focused and its cursor on `key`.
fn on_sidebar(clip: &str, key: &crate::components::sidebar::RowKey) -> App {
    use crate::components::sidebar::RowKey;
    let mut app = app_with_clip(clip);
    app.update(Action::FocusPane(PaneId::Sidebar));
    if let RowKey::Request(slug) = key {
        app.sidebar.select_slug(slug);
        app.update(Action::RefreshSidebar);
    }
    assert!(app.sidebar.select_key(key), "{key:?} is a row");
    app
}

#[test]
fn a_sidebar_paste_imports_into_the_cursor_rows_folder() {
    use crate::components::sidebar::RowKey;
    for (key, prefill) in [
        (RowKey::Folder("main/api".into()), "api/GET users"),
        (RowKey::Request("main/api/ping".into()), "api/GET users"),
        (RowKey::Request("main/top".into()), "GET users"),
    ] {
        let mut app = on_sidebar("curl https://x/users", &key);
        app.update(Action::Paste);
        assert_eq!(prompt(&app).1, prefill, "{key:?}");
    }
}

#[test]
fn a_bracketed_paste_into_the_sidebar_imports_the_pasted_text() {
    let top = crate::components::sidebar::RowKey::Request("main/top".into());
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = on_sidebar("not curl", &top);
        app.use_profile(profile);
        draw(&mut app);
        // As the event loop runs an `Event::Paste`.
        assert!(app.paste_text("curl https://x/users"));
        app.vim_catch_up();
        draw(&mut app);
        assert_eq!(prompt(&app).1, "GET users", "{profile:?}");
        if profile == KeyProfile::Vim {
            assert_eq!(app.mode_indicator(), Some("INSERT"), "the prompt opens in Insert");
        }
    }
}

#[test]
fn ctrl_v_on_the_sidebar_imports_in_both_profiles() {
    let top = crate::components::sidebar::RowKey::Request("main/top".into());
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = on_sidebar("curl https://x/users", &top);
        app.use_profile(profile);
        draw(&mut app);
        feed(&mut app, "<C-v>");
        assert_eq!(prompt(&app).1, "GET users", "{profile:?}");
    }
}

#[test]
fn a_curl_pasted_into_a_text_field_is_text() {
    let mut app = app_with_clip("curl https://x/users");
    app.update(Action::FocusUrl);
    app.update(Action::Paste);
    assert!(app.modals.is_empty());
    assert!(app.editor.url.text().contains("curl https://x/users"), "{}", app.editor.url.text());
}

#[test]
fn a_sidebar_paste_without_curl_says_so() {
    let top = crate::components::sidebar::RowKey::Request("main/top".into());
    let mut app = on_sidebar("hello", &top);
    app.update(Action::Paste);
    assert!(app.modals.is_empty());
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
    app.paste_text("curl 'https://x");
    assert!(app.modals.is_empty());
    assert_eq!(app.toasts.last_message(), Some("unclosed quote"));
    // A terminal's paste need not be the clipboard's: it says "paste",
    // an empty one too.
    for text in ["hello", ""] {
        app.toasts = Default::default();
        app.paste_text(text);
        assert_eq!(app.toasts.last_message(), Some("the paste holds no curl command"));
    }
}

#[test]
fn a_sidebar_ctrl_v_is_the_import_button() {
    let top = crate::components::sidebar::RowKey::Request("main/top".into());
    let mut app = on_sidebar("curl https://x/users", &top);
    app.ui_settings.curl_button = true;
    app.check_clipboard();
    // Its read is the button's: a stale button goes, as on a click.
    app.clipboard.set_read_for_test("hello");
    app.update(Action::Paste);
    assert!(!app.sidebar.curl_ready);
    // An empty clipboard and one that can't be read say what the button
    // says, not nothing and not the text field's read failure.
    app.clipboard.set_read_for_test("");
    app.update(Action::Paste);
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
    app.toasts = Default::default();
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(None, 65536));
    app.update(Action::Paste);
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
}

/// `app_with_clip` with the `curl_button` setting on (V13).
fn watching(clip: &str) -> App {
    let mut app = app_with_clip(clip);
    app.ui_settings.curl_button = true;
    app
}

#[test]
fn the_import_button_shows_only_while_the_clipboard_parses_as_curl() {
    let mut app = watching("");
    for (clip, ready) in [
        ("curl https://x/u", true),
        ("hello", false),
        ("curl 'https://x", false),
        ("# @name A\ncurl https://x/a\ncurl https://x/b", true),
    ] {
        app.clipboard.set_read_for_test(clip);
        app.check_clipboard();
        assert_eq!(app.sidebar.curl_ready, ready, "{clip:?}");
    }
    // A clipboard that can't be read holds no curl.
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(None, 65536));
    app.check_clipboard();
    assert!(!app.sidebar.curl_ready);
}

#[test]
fn the_terminal_regaining_focus_rechecks_the_clipboard() {
    let mut app = watching("hello");
    app.check_clipboard();
    app.clipboard.set_read_for_test("curl https://x/u");
    assert!(app.on_focus_gained(), "the button appearing is a repaint");
    assert!(app.sidebar.curl_ready);
}

#[test]
fn with_the_setting_off_nothing_reads_the_clipboard_for_the_button() {
    let mut app = app_with_clip("curl https://x/u");
    assert!(!app.check_clipboard());
    assert!(!app.on_focus_gained());
    assert!(!app.sidebar.curl_ready);
    // The import's own read found curl, but the setting is off.
    import(&mut app);
    assert!(app.modals.top().is_some(), "the import ran");
    assert!(!app.sidebar.curl_ready);
}

#[test]
fn turning_the_setting_on_checks_and_off_hides() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_clip("curl https://x/u");
    app.config = crate::config::Config::at(dir.path().to_path_buf());
    app.update(Action::SetUiFlag { key: "curl_button", value: true });
    assert!(app.sidebar.curl_ready, "on: checked at once");
    app.update(Action::SetUiFlag { key: "curl_button", value: false });
    assert!(!app.sidebar.curl_ready, "off: the button goes");
    app.update(Action::Undo);
    assert!(app.ui_settings.curl_button);
    assert!(app.sidebar.curl_ready, "undo turns it back on, and checks");
}

#[test]
fn with_no_project_an_import_never_reads_the_clipboard() {
    let mut app = watching("curl https://x/u");
    app.project = None;
    app.update(Action::FocusPane(PaneId::Sidebar));
    for run in [Action::ImportCurl { folder: None }, Action::Paste] {
        app.update(run.clone());
        assert!(!app.sidebar.curl_ready, "{run:?}: a read would have found the curl");
    }
}

#[test]
fn a_copy_does_not_check_the_clipboard() {
    let (mut app, _dir, _out) = copy_app();
    app.ui_settings.curl_button = true;
    let mut c = crate::clipboard::Clipboard::new_for_test(Some("cat > /dev/null".into()), 65536);
    c.set_read_for_test("curl https://x/u");
    app.set_clipboard_for_test(c);
    app.update(Action::CopyAsCurl(Some("main/top".into())));
    assert!(app.toasts.last_message().unwrap().starts_with("Copied"));
    assert!(!app.sidebar.curl_ready);
}

#[test]
fn an_unreadable_clipboard_holds_no_curl() {
    // Empty and image clipboards are read errors to arboard; the import
    // says what main always said (spec V14, reverted by the user).
    let mut app = app_with_clip("");
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(None, 65536));
    import(&mut app);
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
}

#[test]
fn the_import_button_imports_into_the_cursor_rows_folder() {
    let api = crate::components::sidebar::RowKey::Folder("main/api".into());
    let mut app = on_sidebar("curl https://x/users", &api);
    app.ui_settings.curl_button = true;
    app.check_clipboard();
    click(&mut app, Hit::SidebarImportCurl);
    assert_eq!(prompt(&app).1, "api/GET users");
}

#[test]
fn a_stale_import_button_says_why_and_goes_away() {
    let mut app = watching("curl https://x/users");
    app.check_clipboard();
    draw(&mut app);
    app.clipboard.set_read_for_test("hello");
    click(&mut app, Hit::SidebarImportCurl);
    assert!(app.modals.is_empty());
    assert_eq!(app.toasts.last_message(), Some("clipboard doesn't hold a curl command"));
    assert!(!app.sidebar.curl_ready);
    draw(&mut app);
    assert!(app.hits.rect_of(&Hit::SidebarImportCurl).is_none());
}

#[test]
fn a_sidebar_paste_leaves_the_button_to_its_checks() {
    // A terminal's paste (shift+Insert's PRIMARY, a paste history) need
    // not be the clipboard: what it holds says nothing about the button.
    let top = crate::components::sidebar::RowKey::Request("main/top".into());
    let mut app = on_sidebar("hello", &top);
    app.ui_settings.curl_button = true;
    app.check_clipboard();
    app.paste_text("curl https://x/users");
    assert_eq!(prompt(&app).1, "GET users", "the paste imports");
    assert!(!app.sidebar.curl_ready, "but shows no button for the clipboard");
}

#[test]
fn the_sidebar_footer_always_advertises_the_paste() {
    let top = crate::components::sidebar::RowKey::Request("main/top".into());
    let mut app = on_sidebar("hello", &top);
    let has_chip = |app: &App| {
        crate::ui::footer_chips_of(app)
            .iter()
            .any(|(_, l, a)| l == "import curl" && *a == Some(Action::Paste))
    };
    assert!(has_chip(&app), "whatever the clipboard holds, unchecked");
    app.update(Action::FocusPane(PaneId::Editor));
    assert!(!has_chip(&app), "the sidebar's chip only");
}
