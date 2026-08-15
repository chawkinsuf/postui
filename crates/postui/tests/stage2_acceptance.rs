use postui::action::Action;
use postui::app::App;
use postui::components::line_input::LineInput;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

/// End-to-end: create a request, point it at a mock HTTP server, send it,
/// and confirm the full frame shows everything the spec's exit criterion
/// ("usable as a basic daily HTTP client") calls for — the sidebar entry,
/// the method badge, the response status pill, and a key from the JSON
/// body.
#[tokio::test]
async fn stage2_acceptance_flow() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": true, "items": [1, 2, 3]})),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());

    // Create a request via actions (rather than reaching into storage
    // directly) so this exercises the same path the UI drives.
    app.update(Action::CreateRequest("smoke/ping".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("smoke/ping"));

    // Point it at the mock server.
    app.editor.url = LineInput::new(&format!("{}/ping", server.uri()));

    // Send, then drive the background task's result back through `update`
    // the same way the main loop does.
    app.update(Action::ForceSend);
    let result = rx.recv().await.expect("the send task reports a result");
    app.update(result);

    let frame = render(&mut app);
    assert!(frame.contains("ping"), "sidebar shows the request slug: {frame}");
    assert!(frame.contains("GET"), "method badge: {frame}");
    assert!(frame.contains("200"), "status pill: {frame}");
    assert!(
        frame.contains("ok") || frame.contains("items"),
        "response body JSON key visible in the tree: {frame}"
    );
}
