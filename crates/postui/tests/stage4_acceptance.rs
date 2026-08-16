use postui::action::Action;
use postui::app::App;
use postui::clipboard::Clipboard;
use postui::components::editor::SubFocus;
use postui::components::modal::{Modal, PromptKind};
use postui::components::response::ViewMode;
use postui::hit::Hit;
use postui::keys::Keymap;
use postui_core::model::Method;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}
fn type_text(app: &mut App, keymap: &Keymap, text: &str) {
    for c in text.chars() {
        app.handle_key(keymap, plain(c));
    }
}

fn left_down(x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

/// Re-renders `app` (so `app.hits` is fresh), looks up `hit`'s rect, and
/// sends a left-button Down at its center — the mouse-only equivalent of a
/// click for every step of this flow. Panics with the missing hit's debug
/// form if it isn't on screen, since that's always a test bug (either a
/// wrong flow step or a real regression in hit registration) rather than
/// something a caller should recover from.
fn click(app: &mut App, hit: Hit) {
    render(app);
    let r = app
        .hits
        .rect_of(&hit)
        .unwrap_or_else(|| panic!("no rect registered for {hit:?}"));
    let cx = r.x + r.width / 2;
    let cy = r.y + r.height / 2;
    app.handle_mouse(left_down(cx, cy));
}

/// Drains `rx`, applying every action through `app.update` as the main loop
/// would, until the `ResponseArrived`/`RequestFailed` tagged `generation`
/// lands. Mirrors the stage-3 acceptance test's pumping pattern.
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

/// End-to-end sweep of the stage-4 mouse-first surface: every step of the
/// flow is driven by a synthesized click on a `Hit` looked up from the last
/// render, with keyboard used only where the brief calls it out as an
/// acceptable exception (naming a request, typing a URL, entering a table
/// cell) — never to substitute for a click that has a mouse affordance.
#[tokio::test]
async fn stage4_mouse_only_acceptance_flow() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/items"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"a": {"b": 1, "c": 2}})),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    let keymap = Keymap::default_bindings();

    let out = dir.path().join("clipboard-out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    app.set_clipboard_for_test(Clipboard::new_for_test(Some(cmd), 65536, false));

    // --- step 2: click SidebarNewRequest -> prompt opens; name via keys --
    click(&mut app, Hit::SidebarNewRequest);
    assert!(
        matches!(
            app.modals.top(),
            Some(Modal::Prompt {
                kind: PromptKind::NewRequest,
                ..
            })
        ),
        "clicking the new-request button opens the naming prompt"
    );
    type_text(&mut app, &keymap, "items/create");
    app.handle_key(&keymap, enter());
    assert_eq!(
        app.editor.slug.as_deref(),
        Some("items/create"),
        "naming prompt created and opened the request"
    );

    // --- step 3: click MethodSelector -> click DropdownRow(1) -> POST ----
    click(&mut app, Hit::MethodSelector);
    assert!(
        matches!(app.modals.top(), Some(Modal::Dropdown(_))),
        "clicking the method badge opens the method dropdown"
    );
    click(&mut app, Hit::DropdownRow(1)); // Method::ALL[1] == Post
    assert_eq!(app.editor.method, Method::Post);
    assert!(app.modals.is_empty(), "picking a row closes the dropdown");
    let frame = render(&mut app);
    assert!(frame.contains("POST"), "method badge reads POST: {frame}");

    // --- step 4: URL is keyboard-only (no mouse affordance) -------------
    app.focus = postui::layout::PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url = postui::components::line_input::LineInput::new("");
    type_text(&mut app, &keymap, &format!("{}/items", server.uri()));
    assert_eq!(app.editor.url.text(), format!("{}/items", server.uri()));

    // --- step 5: tab switches by click; add a param via keys; toggle its
    //     checkbox off then on via click ----------------------------------
    click(&mut app, Hit::EditorTab(1)); // Headers
    assert_eq!(
        app.editor.active_tab,
        postui::components::editor::EditorTab::Headers
    );
    click(&mut app, Hit::EditorTab(0)); // Params
    assert_eq!(
        app.editor.active_tab,
        postui::components::editor::EditorTab::Params
    );

    app.focus = postui::layout::PaneId::Editor;
    app.editor.sub_focus = SubFocus::Content;
    app.handle_key(&keymap, plain('a')); // start a new row
    type_text(&mut app, &keymap, "id");
    app.handle_key(&keymap, key(KeyCode::Tab)); // move to the value cell
    type_text(&mut app, &keymap, "42");
    app.handle_key(&keymap, enter()); // commit the row
    assert!(app.editor.params.contains_key("id"));
    assert!(app.editor.params["id"].enabled);

    click(&mut app, Hit::TableCheckbox(0));
    assert!(!app.editor.params["id"].enabled, "click turned it off");
    click(&mut app, Hit::TableCheckbox(0));
    assert!(app.editor.params["id"].enabled, "click turned it back on");

    // --- step 6: click SendButton; pump until the response arrives ------
    click(&mut app, Hit::SendButton);
    let generation = app.send_generation;
    assert!(app.in_flight.is_some(), "click dispatches Send");
    drain_until(&mut rx, &mut app, generation).await;
    let frame = render(&mut app);
    assert!(frame.contains("200"), "status 200 rendered: {frame}");

    // --- step 7: Headers tab; copy a header via the file-backed clipboard
    click(&mut app, Hit::ResponseTab(ViewMode::Headers));
    assert_eq!(app.response.view().unwrap().mode, ViewMode::Headers);
    let headers = match app.response.state() {
        postui::components::response::ResponseState::Ready(data) => data.headers.clone(),
        _ => panic!("expected a Ready response"),
    };
    let (idx, (_, content_type_value)) = headers
        .iter()
        .enumerate()
        .find(|(_, (k, _))| k.eq_ignore_ascii_case("content-type"))
        .expect("a content-type header on a JSON response");
    let content_type_value = content_type_value.clone();
    click(&mut app, Hit::HeaderCopy(idx));
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        content_type_value,
        "HeaderCopy click copied the header's value to the file-backed clipboard"
    );

    // --- step 8: Pretty tab; click a JsonArrow to collapse a node --------
    click(&mut app, Hit::ResponseTab(ViewMode::Pretty));
    assert_eq!(app.response.view().unwrap().mode, ViewMode::Pretty);
    let before = app.response.view().unwrap().visible_len();
    click(&mut app, Hit::JsonArrow(1)); // the "a": {...} container row
    assert!(
        app.response.view().unwrap().visible_len() < before,
        "clicking the arrow collapsed the container"
    );

    // --- step 9: open the palette by click, search "quit", click it -----
    click(&mut app, Hit::FooterChip(Action::OpenPalette));
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    type_text(&mut app, &keymap, "quit");
    click(&mut app, Hit::PaletteRow(0));
    assert!(app.should_quit, "clicking the Quit palette row quits");
    assert!(app.modals.is_empty());
}
