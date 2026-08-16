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

#[tokio::test]
async fn connection_refused_yields_readable_error() {
    let (prepared, _) = prepare(
        &HttpRequest {
            method: Method::Get,
            url: "http://127.0.0.1:1/".into(),
            substitute_body: false,
            params: Default::default(),
            headers: Default::default(),
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
