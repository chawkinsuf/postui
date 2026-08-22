use postui::action::Action;
use postui::http;
use postui_core::model::*;
use postui_core::prepare::{PrepareContext, prepare};
use std::time::Duration;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req_to(url: String) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        url,
        substitute_body: false,
        params: Default::default(),
        headers: Default::default(),
        variables: Default::default(),
        body: Some(Body::Json {
            text: "{\"a\":1}".into(),
        }),
    }
}

#[tokio::test]
async fn sends_json_with_auto_content_type_and_reads_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/x"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(201).set_body_string("{\"ok\":true}"))
        .mount(&server)
        .await;
    let (prepared, _) = prepare(
        &req_to(format!("{}/x", server.uri())),
        &PrepareContext::default(),
    )
    .unwrap();
    let data = http::send(&http::client(), &prepared).await.unwrap();
    assert_eq!(data.status, 201);
    assert_eq!(data.body, "{\"ok\":true}");
    assert_eq!(data.size, 11);
}

#[tokio::test]
async fn merged_query_params_reach_the_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("id", "2"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut r = req_to(format!("{}/x?id=1", server.uri()));
    r.method = Method::Get;
    r.body = None;
    r.params.insert(
        "id".into(),
        Entry {
            value: "2".into(),
            enabled: true,
        },
    );
    let (prepared, warns) = prepare(&r, &PrepareContext::default()).unwrap();
    assert_eq!(warns.len(), 1);
    assert_eq!(
        http::send(&http::client(), &prepared).await.unwrap().status,
        200
    );
}

#[tokio::test]
async fn non_2xx_is_a_response_not_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let (prepared, _) = prepare(
        &req_to(format!("{}/x", server.uri())),
        &PrepareContext::default(),
    )
    .unwrap();
    let data = http::send(&http::client(), &prepared).await.unwrap();
    assert_eq!(data.status, 500);
    assert_eq!(data.body, "boom");
}

#[tokio::test]
async fn timeout_produces_err() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let (prepared, _) = prepare(
        &req_to(format!("{}/x", server.uri())),
        &PrepareContext::default(),
    )
    .unwrap();
    let client = http::client_with_timeout(Duration::from_millis(200));
    let started = std::time::Instant::now();
    let result = http::send(&client, &prepared).await;
    assert!(result.is_err(), "expected a timeout error, got {result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "should have timed out quickly, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn redirects_are_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/b", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .mount(&server)
        .await;
    let mut r = req_to(format!("{}/a", server.uri()));
    r.method = Method::Get;
    r.body = None;
    let (prepared, _) = prepare(&r, &PrepareContext::default()).unwrap();
    let data = http::send(&http::client(), &prepared).await.unwrap();
    assert_eq!(data.status, 200);
    assert_eq!(data.body, "done");
}

#[tokio::test]
async fn large_body_is_handled() {
    let server = MockServer::start().await;
    let big = "x".repeat(2 * 1024 * 1024 + 17);
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big.clone()))
        .mount(&server)
        .await;
    let mut r = req_to(format!("{}/big", server.uri()));
    r.method = Method::Get;
    r.body = None;
    let (prepared, _) = prepare(&r, &PrepareContext::default()).unwrap();
    let data = http::send(&http::client(), &prepared).await.unwrap();
    assert_eq!(data.status, 200);
    assert_eq!(data.size, big.len());
}

/// A body far past the inline-parse threshold is still pretty-printed —
/// the parse just happens off the UI thread and arrives as its own action.
#[tokio::test]
async fn a_multi_megabyte_json_body_is_pretty_printed_in_the_background() {
    use postui::components::response::ViewMode;

    let server = MockServer::start().await;
    let big = format!("{{\"a\": \"{}\"}}", "x".repeat(2 * 1024 * 1024 + 17));
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big.clone()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = postui::app::App::with_root(tx, dir.path().to_path_buf());
    app.editor.url =
        postui::components::line_input::LineInput::new(&format!("{}/big", server.uri()));
    app.update(Action::ForceSend);

    drain_until_settled(&mut app, &mut rx).await;
    let view = app.session.response.view().expect("a ready response");
    assert!(
        view.parsing && view.tree.is_none(),
        "a body this big is not parsed on the UI thread"
    );
    assert_eq!(view.mode, ViewMode::Raw, "the raw body is readable at once");

    // Pump on until the background parse reports in.
    loop {
        let action = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("timed out waiting for the background parse")
            .expect("channel closed before the parse arrived");
        let parsed = matches!(action, Action::PrettyParsed { .. });
        app.update(action);
        if parsed {
            break;
        }
    }

    let view = app.session.response.view().unwrap();
    assert!(!view.parsing, "the parse is done");
    assert!(view.tree.is_some(), "and it produced a tree");
    app.update(Action::ResponseViewMode(ViewMode::Pretty));
    let view = app.session.response.view().unwrap();
    assert_eq!(
        view.mode,
        ViewMode::Pretty,
        "the Tree view is now available"
    );
    assert!(view.visible_len() > 1, "and it has the parsed lines");
}

#[tokio::test]
async fn connection_refused_yields_readable_error() {
    let (prepared, _) = prepare(
        &HttpRequest {
            method: Method::Get,
            url: "http://127.0.0.1:1/".into(),
            substitute_body: false,
            params: Default::default(),
            headers: Default::default(),
            variables: Default::default(),
            body: None,
        },
        &PrepareContext::default(),
    )
    .unwrap();
    let err = http::send(&http::client(), &prepared).await.unwrap_err();
    assert!(
        !err.is_empty() && !err.contains("Error {"),
        "human string, not Debug dump: {err}"
    );
}

async fn drain_until_settled(
    app: &mut postui::app::App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>,
) {
    let generation = app.session.send_generation;
    loop {
        let action = tokio::time::timeout(Duration::from_secs(5), rx.recv())
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

#[tokio::test]
async fn send_substitutes_vars_and_applies_default_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users/7"))
        .and(header("authorization", "Bearer tok-qa"))
        .and(header("accept", "application/json"))
        .and(wiremock::matchers::body_string(r#"{"id": "7"}"#))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    std::fs::write(
        dir.path().join("project.toml"),
        "name = \"svc\"\n[default_headers]\naccept = \"application/json\"\nauthorization = \"Bearer {{tok}}\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("variables.toml"),
        "[base]\n[tok]\n[uid]\ndefault = \"7\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        format!("base = \"{}\"\ntok = \"tok-qa\"\n", server.uri()),
    )
    .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = postui::app::App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.editor.method = Method::Post;
    app.editor.url = postui::components::line_input::LineInput::new("{{base}}/users/{{uid}}");
    app.editor.set_body_text(r#"{"id": "{{uid}}"}"#);
    app.editor.substitute_body = true;
    app.update(Action::ForceSend);
    assert!(app.session.in_flight.is_some());

    drain_until_settled(&mut app, &mut rx).await;
    match app.session.response.state() {
        postui::components::response::ResponseState::Ready(data) => {
            assert_eq!(data.status, 200);
        }
        _ => panic!("expected a ready response"),
    }
}

#[tokio::test]
async fn disabled_request_header_row_suppresses_a_default_header() {
    let server = MockServer::start().await;
    // The request disables its own `accept` row, and the project default
    // for `accept` must not leak through. Strictly matching on reqwest's
    // own implicit `accept: */*` (rather than the project default
    // `application/json`) proves the default header was suppressed, not
    // merely overridden — a leaked default would match no mock and 404.
    Mock::given(method("GET"))
        .and(path("/x"))
        .and(header("accept", "*/*"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    std::fs::write(
        dir.path().join("project.toml"),
        "name = \"svc\"\n[default_headers]\naccept = \"application/json\"\n",
    )
    .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = postui::app::App::with_root(tx, dir.path().to_path_buf());
    app.editor.url = postui::components::line_input::LineInput::new(&format!("{}/x", server.uri()));
    app.editor.headers.insert(
        "accept".into(),
        Entry {
            value: "ignored".into(),
            enabled: false,
        },
    );
    app.update(Action::ForceSend);
    assert!(app.session.in_flight.is_some());

    drain_until_settled(&mut app, &mut rx).await;
    match app.session.response.state() {
        postui::components::response::ResponseState::Ready(data) => {
            assert_eq!(
                data.status, 200,
                "default `accept` header must be suppressed"
            );
        }
        _ => panic!("expected a ready response"),
    }
}
