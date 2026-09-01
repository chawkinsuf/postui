use postui::action::Action;
use postui::app::App;
use postui::components::editor::SubFocus;
use postui::components::line_input::LineInput;
use postui::components::modal::Modal;
use postui::components::sidebar::Row;
use postui::keys::Keymap;
use postui::layout::PaneId;
use postui_core::model::HttpRequest;
use postui_core::project::{self, LocalState};
use postui_core::storage;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn render(app: &mut App) -> String {
    app.anims.finish_all();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}
fn enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}

fn dummy_request(url: &str) -> HttpRequest {
    HttpRequest::from_toml_str(&format!("url = \"{url}\"")).unwrap()
}

/// Writes a project with declared variables `base`/`tok` (no defaults) and
/// two environments, `qa` and `prod`, both pointing `base` at the mock
/// server and giving `tok` distinct values.
fn write_alpha(root: &Path, server_uri: &str) {
    std::fs::create_dir_all(root.join("environments")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "name = \"alpha\"\n\n[default_headers]\nx-api-key = \"shared-secret\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("variables.toml"),
        "[base]\ndescription = \"API base URL\"\n\n[tok]\ndescription = \"auth token\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("environments/qa.toml"),
        format!("base = \"{server_uri}\"\ntok = \"qa-tok\"\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("environments/prod.toml"),
        format!("base = \"{server_uri}\"\ntok = \"prod-tok\"\n"),
    )
    .unwrap();
}

/// Writes a second project with one saved request (`pong`) and local state
/// that already points the editor at it, so switching to this project
/// exercises the "restore from local state" path.
fn write_beta(root: &Path) {
    std::fs::write(root.join("project.toml"), "name = \"beta\"\n").unwrap();
    storage::save_request(
        root,
        "main/pong",
        &dummy_request("https://example.test/pong"),
    )
    .unwrap();
    // A second request, not the one seeded as "open": switching to it while
    // beta is active (see the flow below) is what makes the final
    // persisted-state assertion discriminating rather than trivially true.
    storage::save_request(
        root,
        "main/ping",
        &dummy_request("https://example.test/ping"),
    )
    .unwrap();
    project::save_local_state(
        root,
        &LocalState {
            environment: None,
            open_request: Some("main/pong".into()),
            expanded: vec![],
            ..Default::default()
        },
    )
    .unwrap();
}

/// Drains `rx`, applying every action through `app.update` as the main loop
/// would, until the `ResponseArrived`/`RequestFailed` tagged `generation`
/// lands.
async fn drain_until(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>,
    app: &mut App,
    generation: u64,
) {
    loop {
        let action = rx.recv().await.expect("a background task result");
        let done = matches!(
            &action,
            Action::ResponseArrived { generation: g, .. } | Action::RequestFailed { generation: g, .. }
                if *g == generation
        );
        app.update(action);
        if done {
            break;
        }
    }
}

/// End-to-end sweep of the stage-3 surface: two registered projects, two
/// environments with distinct variable values, sidebar tree expand/collapse
/// and free-scroll, project/env cycling through real key bindings, the
/// `{{` variable picker, and local-state persistence per project.
#[tokio::test]
async fn stage3_acceptance_flow() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .and(query_param("tok", "qa-tok"))
        .and(header("x-api-key", "shared-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"env": "qa"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .and(query_param("tok", "prod-tok"))
        .and(header("x-api-key", "shared-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"env": "prod"})))
        .mount(&server)
        .await;

    let alpha_dir = tempfile::tempdir().unwrap();
    let beta_dir = tempfile::tempdir().unwrap();
    write_alpha(alpha_dir.path(), &server.uri());
    write_beta(beta_dir.path());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, alpha_dir.path().to_path_buf());
    app.registry.register(alpha_dir.path().to_path_buf());
    app.registry.register(beta_dir.path().to_path_buf());

    let keymap = Keymap::default_bindings();

    // --- create "users/list" via the `n` prompt flow -----------------
    app.handle_key(&keymap, plain('n'));
    assert!(
        matches!(app.modals.top(), Some(Modal::Prompt { .. })),
        "'n' opens the new-request prompt"
    );
    for c in "users/list".chars() {
        app.handle_key(&keymap, plain(c));
    }
    app.handle_key(&keymap, enter());
    assert_eq!(app.editor.slug.as_deref(), Some("main/users/list"));

    app.editor.url = LineInput::new("{{base}}/users?tok={{tok}}");
    app.update(Action::SaveRequest);
    assert!(!app.editor.is_dirty(), "saved: editor is clean again");

    // --- send with no environment active: unresolved, nothing sent ---
    app.update(Action::Send);
    assert!(
        app.session.in_flight.is_empty(),
        "unresolved variables must not send anything"
    );
    let frame = render(&mut app);
    assert!(
        frame.contains("unresolved"),
        "unresolved-variable toast shown: {frame}"
    );

    // --- qa environment: header renders, send resolves against it ----
    app.update(Action::SwitchEnv(Some("qa".into())));
    let frame = render(&mut app);
    assert!(
        frame.contains("alpha \u{25be}") && frame.contains("qa \u{25be}"),
        "header bar shows project and env chips: {frame}"
    );

    app.update(Action::Send);
    let generation = app.session.send_generation;
    assert!(
        !app.session.in_flight.is_empty(),
        "qa resolves: request goes out"
    );
    drain_until(&mut rx, &mut app, generation).await;
    let frame = render(&mut app);
    assert!(frame.contains("200"), "qa response is Ready: {frame}");
    assert!(frame.contains("qa"), "qa response body visible: {frame}");

    // --- cycle to prod via alt+c (real binding), send again ----------
    app.handle_key(&keymap, alt('c'));
    assert_eq!(app.project.active_env.as_deref(), Some("prod"));
    app.update(Action::Send);
    let generation = app.session.send_generation;
    drain_until(&mut rx, &mut app, generation).await;
    let frame = render(&mut app);
    assert!(frame.contains("200"), "prod response is Ready: {frame}");
    assert!(
        frame.contains("prod"),
        "prod response body visible: {frame}"
    );

    // --- sidebar: collapse/expand `users`, free-scroll survives draw -
    assert!(matches!(
        app.sidebar.rows.as_slice(),
        [Row::Folder { expanded: true, .. }, Row::Request { .. }]
    ));
    app.handle_key(&keymap, plain('k')); // move selection up to the folder row
    app.handle_key(&keymap, enter()); // collapse
    assert!(matches!(
        app.sidebar.rows.as_slice(),
        [Row::Folder {
            expanded: false,
            ..
        }]
    ));
    app.handle_key(&keymap, enter()); // expand again
    assert!(matches!(
        app.sidebar.rows.as_slice(),
        [Row::Folder { expanded: true, .. }, Row::Request { .. }]
    ));

    app.update(Action::ScrollPane(PaneId::Sidebar, 5));
    let scroll_before = app.sidebar.scroll;
    render(&mut app);
    assert_eq!(
        app.sidebar.scroll, scroll_before,
        "wheel scroll must not snap back after a draw"
    );

    // --- alt+o cycles projects (real binding): alpha -> beta -> alpha
    app.handle_key(&keymap, alt('o'));
    assert!(
        app.modals.is_empty(),
        "editor is clean: no dirty-gate prompt"
    );
    assert_eq!(app.project.root, beta_dir.path().to_path_buf());
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("main/pong"),
        "beta's local state restores the previously-open request"
    );
    let frame = render(&mut app);
    assert!(frame.contains("pong"), "beta's request is listed: {frame}");

    // Mutate beta's observable state while it's active — switching the open
    // request away from the pre-seeded "pong" — so the on-disk assertion
    // after cycling away actually exercises ForceSwitchProject's persist
    // path, rather than trivially matching the pre-seeded value.
    app.update(Action::ForceOpenRequest("main/ping".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("main/ping"));

    app.handle_key(&keymap, alt('o'));
    assert_eq!(app.project.root, alpha_dir.path().to_path_buf());
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("main/users/list"),
        "alpha's open request is restored"
    );
    assert_eq!(
        app.project.active_env.as_deref(),
        Some("prod"),
        "alpha's active environment is restored"
    );
    assert!(
        app.project.expanded.contains("main/users"),
        "alpha's sidebar expansion is restored"
    );

    // --- `{{` in the URL pops the picker; picking inserts the token --
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url = LineInput::new("");
    app.handle_key(&keymap, plain('{'));
    app.handle_key(&keymap, plain('{'));
    assert!(
        matches!(app.modals.top(), Some(Modal::VarPicker(_))),
        "typing {{{{ opens the variable picker"
    );
    app.handle_key(&keymap, enter()); // first option: declared order is base, then tok
    assert_eq!(
        app.editor.url.text(),
        "{{base}}",
        "picked variable inserted at the cursor"
    );

    // --- quit path: PersistLocalState, then check both projects' state
    app.update(Action::PersistLocalState);
    let alpha_state = project::load_local_state(alpha_dir.path()).unwrap();
    assert_eq!(alpha_state.environment.as_deref(), Some("prod"));
    assert_eq!(alpha_state.open_request.as_deref(), Some("main/users/list"));
    assert!(alpha_state.expanded.contains(&"main/users".to_string()));

    let beta_state = project::load_local_state(beta_dir.path()).unwrap();
    assert_eq!(
        beta_state.open_request.as_deref(),
        Some("main/ping"),
        "beta's state reflects the switch to \"ping\" made while beta was \
         active, not the \"pong\" it was pre-seeded with — proving \
         ForceSwitchProject actually persisted on the way out"
    );
}
