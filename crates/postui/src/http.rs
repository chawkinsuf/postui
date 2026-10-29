//! The reqwest-backed send path: building a client, issuing a prepared
//! request, and shaping the result into something the UI can render without
//! knowing anything about reqwest itself.
use postui_core::prepare::{PreparedRequest, SentHeader};
use std::error::Error as _;
use std::time::{Duration, Instant};

/// A fully-resolved HTTP response, shaped for the UI. `body` is a lossy
/// UTF-8 decode of the raw bytes (never fails, may show replacement chars
/// for non-text bodies); `size` is the exact raw byte length regardless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseData {
    pub status: u16,
    /// The URL the request was actually sent to, with secret values masked
    /// (`PreparedRequest::display_url`) — shown on the response header
    /// strip so the rendered result of `{{vars}}` and merged params is
    /// visible after a send.
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The headers this request went out with, in send order — a snapshot
    /// taken at send time, so it still says what was sent after the
    /// request has been edited (`PreparedRequest::wire`). Shown
    /// under the response headers as the Headers tab's `sent` section.
    pub sent_headers: Vec<SentHeader>,
    pub body: String,
    /// Time to first byte: send → response headers received.
    pub ttfb: Duration,
    /// Total: send → body fully downloaded.
    pub elapsed: Duration,
    pub size: usize,
    pub content_type: Option<String>,
}

/// Builds the plain verifying client with no timeout (used by tests;
/// `Clients` builds its own the same way). Deliberately no timeout of any
/// kind: a slow request stays in flight until the server answers or the
/// user cancels it (Esc) — the UI warns when a request has been waiting a
/// long time instead of killing it.
pub fn client() -> reqwest::Client {
    // `Client::builder().build()` does not need a running Tokio reactor —
    // it only sets up connection pooling/TLS config, and does not touch I/O
    // until the first request is actually sent. Verified with a plain
    // `#[test]` (no `#[tokio::test]`) below.
    reqwest::Client::new()
}

/// The two clients a send can go out on: the normal verifying one and one
/// that skips TLS certificate verification, for requests flagged
/// `insecure = true`. Both are built once up front — a reqwest `Client` is
/// an Arc'd pool, and keeping the pair beats rebuilding per send.
pub struct Clients {
    verifying: reqwest::Client,
    insecure: reqwest::Client,
}

impl Default for Clients {
    fn default() -> Self {
        Self::new()
    }
}

impl Clients {
    /// No timeout, as the TUI wants it.
    pub fn new() -> Self {
        Self::with_timeout(None)
    }

    /// Both clients with a total timeout when given (the command line's
    /// `--timeout`).
    pub fn with_timeout(timeout: Option<Duration>) -> Self {
        // Same fallback story as `client_with_timeout`: `build()` only fails
        // on TLS backend init, and if the insecure variant somehow can't be
        // built, failing closed (a verifying client) is the safe default.
        let build = |insecure: bool| {
            let mut builder = reqwest::Client::builder();
            if insecure {
                builder = builder.danger_accept_invalid_certs(true);
            }
            if let Some(d) = timeout {
                builder = builder.timeout(d);
            }
            builder.build().unwrap_or_else(|_| reqwest::Client::new())
        };
        Self { verifying: build(false), insecure: build(true) }
    }

    /// Picks the client `req` should be sent on.
    pub fn for_request(&self, req: &PreparedRequest) -> &reqwest::Client {
        if req.insecure { &self.insecure } else { &self.verifying }
    }
}

/// Like [`client`] but with a total timeout, for tests that need a request
/// to die quickly on its own (the real client never times out).
pub fn client_with_timeout(timeout: Duration) -> reqwest::Client {
    // `build()` can only fail on TLS backend initialization; with the config
    // here (no custom certs/proxies) that's practically unreachable. Fall
    // back to the plain default client rather than panicking if it ever
    // does.
    reqwest::Client::builder().timeout(timeout).build().unwrap_or_else(|_| reqwest::Client::new())
}

/// A response with its body left as the bytes that arrived — what the
/// command line needs to write a body out exactly. [`send`] is this plus a
/// lossy decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawResponse {
    pub status: u16,
    /// As [`ResponseData::url`]: masked.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub sent_headers: Vec<SentHeader>,
    pub bytes: Vec<u8>,
    pub ttfb: Duration,
    pub elapsed: Duration,
    pub content_type: Option<String>,
}

/// Sends `req` and shapes the result (or the error chain) for the UI.
/// Never panics: header/method construction errors and transport errors
/// alike come back as `Err(String)`.
pub async fn send(client: &reqwest::Client, req: &PreparedRequest) -> Result<ResponseData, String> {
    let raw = send_raw(client, req).await?;
    Ok(ResponseData {
        status: raw.status,
        url: raw.url,
        headers: raw.headers,
        sent_headers: raw.sent_headers,
        body: String::from_utf8_lossy(&raw.bytes).into_owned(),
        ttfb: raw.ttfb,
        elapsed: raw.elapsed,
        size: raw.bytes.len(),
        content_type: raw.content_type,
    })
}

/// [`send`] without the decode: the body comes back as the raw bytes.
pub async fn send_raw(
    client: &reqwest::Client,
    req: &PreparedRequest,
) -> Result<RawResponse, String> {
    let method = reqwest::Method::from_bytes(req.method.as_str().as_bytes())
        .map_err(|e| format!("invalid method: {e}"))?;
    let mut builder = client.request(method, &req.url);
    builder = builder
        .headers(header_map(req.headers.iter().map(|h| (h.name.as_str(), h.value.as_str())))?);
    if let Some(body) = &req.body {
        builder = builder.body(body.clone());
    }

    let started = Instant::now();
    let result = builder.send().await;
    // `send` resolves once the response headers are in — the closest thing
    // reqwest exposes to "first byte".
    let ttfb = started.elapsed();
    let response = result.map_err(|e| error_chain(&e, req))?;

    let status = response.status().as_u16();
    // Lossy UTF-8, like the body: `to_str` refuses any byte past ASCII, and
    // a value shown with a U+FFFD beats one silently shown as empty.
    let text =
        |v: &reqwest::header::HeaderValue| String::from_utf8_lossy(v.as_bytes()).into_owned();
    let headers: Vec<(String, String)> =
        response.headers().iter().map(|(k, v)| (k.to_string(), text(v))).collect();
    let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).map(text);
    let bytes = response.bytes().await.map_err(|e| error_chain(&e, req))?;
    let elapsed = started.elapsed();

    Ok(RawResponse {
        status,
        url: req.display_url.clone(),
        headers,
        sent_headers: req.wire.clone(),
        bytes: bytes.to_vec(),
        ttfb,
        elapsed,
        content_type,
    })
}

/// Every prepared header, in order. `append`, not `insert`: two headers
/// that differ only in case (`Accept` / `accept`), or two `{{templates}}`
/// that resolve to the same name, are both what the user wrote, so both
/// go on the wire rather than the later one silently replacing the first.
fn header_map<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<reqwest::header::HeaderMap, String> {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| format!("invalid header name {k:?}: {e}"))?;
        let value = reqwest::header::HeaderValue::from_str(v)
            .map_err(|e| format!("invalid header value for {k:?}: {e}"))?;
        map.append(name, value);
    }
    Ok(map)
}

/// Joins a reqwest error and its `source()` chain with ": ", so e.g. a
/// connection refused shows the useful underlying I/O message rather than
/// just reqwest's generic wrapper text. reqwest's own text names the wire
/// URL — secret values substituted — so the URL is stripped and the
/// masked `display_url` shown instead, the same one the success path
/// shows; any other mention of the wire URL in the chain is masked too.
fn error_chain(err: &reqwest::Error, req: &PreparedRequest) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    mask_error_text(&parts.join(": "), err.url().map(|u| u.as_str()), &req.url, &req.display_url)
}

/// The masking behind [`error_chain`], on the joined chain text: `wire_url`
/// is the URL reqwest itself reports on the error (its parsed, normalised
/// form — `a%20b` for a sent `a b`, a lowercased host — so it need not be
/// byte-identical to `sent_url`). reqwest's own text names it as
/// ` for url (<wire_url>)`; that exact clause is removed and re-added
/// with `display_url` — matched on the URL itself, never on the first
/// `)` in the text, which a URL can contain. Every other mention of
/// either form of the URL is masked too.
fn mask_error_text(
    joined: &str,
    wire_url: Option<&str>,
    sent_url: &str,
    display_url: &str,
) -> String {
    let mut text = joined.to_string();
    if let Some(wire) = wire_url {
        text = text.replace(&format!(" for url ({wire})"), "");
        if wire != display_url {
            text = text.replace(wire, display_url);
        }
    }
    if !sent_url.is_empty() && sent_url != display_url {
        text = text.replace(sent_url, display_url);
    }
    match wire_url {
        Some(_) => format!("{text} for url ({display_url})"),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_strips_the_url_clause_on_the_url_itself_not_the_first_paren() {
        // The wire URL holds a `)`: cutting at the first `)` would leave
        // the secret-bearing tail of the URL in the message.
        let wire = "http://127.0.0.1:1/a%20b?f=(x)&key=sk-REAL";
        let sent = "http://127.0.0.1:1/a b?f=(x)&key=sk-REAL";
        let display = "http://127.0.0.1:1/a b?f=(x)&key=\u{2022}\u{2022}\u{2022}";
        let joined = format!(
            "error sending request for url ({wire}): client error (Connect): tcp connect error: Connection refused"
        );
        let out = mask_error_text(&joined, Some(wire), sent, display);
        assert!(!out.contains("sk-REAL"), "{out}");
        assert_eq!(
            out,
            format!(
                "error sending request: client error (Connect): tcp connect error: Connection refused for url ({display})"
            )
        );
    }

    #[test]
    fn error_text_masks_a_normalised_url_the_sent_form_would_miss() {
        let wire = "http://example.test/p?key=sk-REAL";
        let sent = "http://EXAMPLE.test/p?key=sk-REAL";
        let display = "http://EXAMPLE.test/p?key=\u{2022}";
        let joined = format!("error sending request for url ({wire}): boom ({wire})");
        let out = mask_error_text(&joined, Some(wire), sent, display);
        assert!(!out.contains("sk-REAL"), "{out}");
        assert_eq!(out, format!("error sending request: boom ({display}) for url ({display})"));
    }

    #[test]
    fn error_text_without_a_url_on_the_error_is_left_alone() {
        let out = mask_error_text("body read failed: reset", None, "http://x", "http://x");
        assert_eq!(out, "body read failed: reset");
    }

    #[test]
    fn client_builds_without_a_tokio_runtime() {
        // No #[tokio::test] here on purpose: this is the load-bearing check
        // for App staying constructible in plain sync tests.
        let _c = client();
        let _cs = Clients::new();
    }

    #[test]
    fn for_request_picks_the_insecure_client_only_when_flagged() {
        let clients = Clients::new();
        let mut req = PreparedRequest {
            method: postui_core::model::Method::Get,
            url: "https://x.test".into(),
            display_url: "https://x.test".into(),
            headers: vec![],
            wire: vec![],
            body: None,
            insecure: false,
        };
        assert!(
            std::ptr::eq(clients.for_request(&req), &clients.verifying),
            "unflagged request uses the verifying client"
        );
        req.insecure = true;
        assert!(
            std::ptr::eq(clients.for_request(&req), &clients.insecure),
            "flagged request uses the insecure client"
        );

        let timed = Clients::with_timeout(Some(Duration::from_secs(5)));
        req.insecure = false;
        assert!(
            std::ptr::eq(timed.for_request(&req), &timed.verifying),
            "unflagged request uses the verifying client"
        );
        req.insecure = true;
        assert!(
            std::ptr::eq(timed.for_request(&req), &timed.insecure),
            "flagged request uses the insecure client"
        );
    }

    #[tokio::test]
    async fn send_raw_keeps_the_bodys_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let _ = sock.read(&mut buf).await.unwrap();
                sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nok\xff\xfe")
                    .await
                    .unwrap();
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let req = PreparedRequest {
            method: postui_core::model::Method::Get,
            url: url.clone(),
            display_url: url,
            headers: vec![],
            wire: vec![],
            body: None,
            insecure: false,
        };
        let raw = send_raw(&client(), &req).await.unwrap();
        assert_eq!(raw.bytes, b"ok\xff\xfe");
        assert_eq!(raw.status, 200);
        let data = send(&client(), &req).await.unwrap();
        server.await.unwrap();
        assert_eq!(data.body, "ok\u{fffd}\u{fffd}");
        assert_eq!(data.size, 4);
    }

    #[tokio::test]
    async fn a_timeout_client_gives_up_on_a_silent_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop(sock);
        });
        let url = format!("http://127.0.0.1:{port}/");
        let req = PreparedRequest {
            method: postui_core::model::Method::Get,
            url: url.clone(),
            display_url: url,
            headers: vec![],
            wire: vec![],
            body: None,
            insecure: false,
        };
        let clients = Clients::with_timeout(Some(Duration::from_millis(200)));
        let started = Instant::now();
        let err = send_raw(clients.for_request(&req), &req).await.expect_err("never answered");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert!(err.contains("timed out"), "{err}");
        server.abort();
    }

    /// A server that sends its headers immediately but stalls before the
    /// body separates the two measures: `ttfb` stops at the headers,
    /// `elapsed` keeps counting until the body completes.
    #[test]
    fn same_named_headers_are_all_sent() {
        let map = header_map([("Accept", "a"), ("accept", "b"), ("X-One", "1")]).unwrap();
        let accepts: Vec<&str> =
            map.get_all("accept").iter().map(|v| v.to_str().unwrap()).collect();
        assert_eq!(accepts, ["a", "b"], "neither case-variant is dropped");
        assert_eq!(map.len(), 3);
        assert!(header_map([("bad name", "x")]).is_err());
    }

    #[tokio::test]
    async fn a_transport_error_shows_the_masked_url_never_the_secret() {
        let req = PreparedRequest {
            method: postui_core::model::Method::Get,
            url: "http://127.0.0.1:1/v1/items?key=sk-live-REAL".into(),
            display_url: "http://127.0.0.1:1/v1/items?key=•••••".into(),
            headers: vec![],
            wire: vec![],
            body: None,
            insecure: false,
        };
        let err = send(&client(), &req).await.expect_err("nothing listens on port 1");
        assert!(!err.contains("sk-live-REAL"), "{err}");
        assert!(err.contains("key=•••••"), "{err}");
    }

    #[tokio::test]
    async fn ttfb_stops_at_headers_while_elapsed_covers_the_body() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n").unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(150));
            stream.write_all(b"ok").unwrap();
        });

        let client = client();
        let req = PreparedRequest {
            method: postui_core::model::Method::Get,
            url: format!("http://{addr}/"),
            display_url: format!("http://{addr}/"),
            headers: vec![],
            wire: vec![],
            body: None,
            insecure: false,
        };
        let resp = send(&client, &req).await.unwrap();
        server.join().unwrap();

        assert!(
            resp.elapsed >= resp.ttfb + Duration::from_millis(100),
            "the stalled body must land in elapsed but not ttfb: \
             ttfb={:?} elapsed={:?}",
            resp.ttfb,
            resp.elapsed
        );
    }

    /// `sent_headers` is the wire truth, not the configured rows: what a
    /// raw server actually receives from a prepared send, name for name,
    /// value for value, in order — including what reqwest and hyper add on
    /// their own (`authorization` from the URL's userinfo, `host`,
    /// `content-length`, which core's `header_rows` predicts), with names
    /// in their wire case. postui's own `accept: application/json`
    /// goes in place of reqwest's `*/*` whatever the method (task #174). A typed
    /// `Host`, `Content-Length` or `Transfer-Encoding` is never sent (spec
    /// 2026-10-18 R3a): the client's own row goes out instead, if it sends
    /// one. The userinfo cases pin core's copy of reqwest's decoding.
    #[tokio::test]
    async fn sent_headers_match_the_wire() {
        use postui_core::model::{Body, Entry, HttpRequest, Method};
        use postui_core::prepare::{PrepareContext, prepare};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let plain: &[(&str, &str)] = &[];
        let typed: &[(&str, &str)] =
            &[("Host", "other.test"), ("content-LENGTH", "99"), ("Transfer-Encoding", "chunked")];
        let auth: &[(&str, &str)] = &[("Authorization", "Bearer typed")];
        let twice: &[(&str, &str)] = &[("X-B", "2"), ("x-one", "3")];
        let accept: &[(&str, &str)] = &[("accept", "text/plain")];
        let json = Some("{\"a\":1}");
        let empty = Some("");
        let (get, post) = (Method::Get, Method::Post);
        for (userinfo, method, body, extra) in [
            ("", get, None, plain),
            ("", post, json, plain),
            ("", get, None, typed),
            ("", post, json, typed),
            ("", post, empty, plain),
            ("", Method::Put, empty, plain),
            ("", Method::Patch, empty, plain),
            ("", Method::Delete, empty, plain),
            ("", Method::Options, empty, plain),
            ("", post, empty, typed),
            ("u:pw@", get, None, plain),
            ("u@", post, json, plain),
            (":pw@", get, None, plain),
            ("u%40x:p%20w@", get, None, plain),
            ("%C3%BC:%C3%A9@", get, None, plain),
            ("u:p%FF@", get, None, plain),
            ("u%FF:p@", get, None, plain),
            ("u:pw@", get, None, auth),
            ("", get, None, twice),
            ("u:pw@", get, None, twice),
            ("", get, None, accept),
            ("", post, json, accept),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap();
                sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await.unwrap();
                String::from_utf8_lossy(&buf[..n]).to_string()
            });
            let on = |v: &str| Entry { value: v.into(), enabled: true };
            let mut req = HttpRequest {
                url: format!("http://{userinfo}127.0.0.1:{port}/p?q=1"),
                ..Default::default()
            };
            req.headers.insert("X-One".into(), on("1"));
            for (name, value) in extra {
                req.headers.insert(name.to_string(), on(value));
            }
            req.method = method;
            req.body = body.map(|text| Body::Json { text: text.into() });
            let (prepared, _) = prepare(&req, &PrepareContext::default()).unwrap();
            let data = send(&client(), &prepared).await.unwrap();
            let raw = server.await.unwrap();
            let wire: Vec<(String, String)> = raw
                .split("\r\n")
                .skip(1)
                .take_while(|l| !l.is_empty())
                .map(|l| {
                    let (k, v) = l.split_once(": ").unwrap();
                    (k.to_string(), v.to_string())
                })
                .collect();
            assert!(wire.len() >= 3, "the probe saw the client's own rows: {raw}");
            let sent: Vec<(String, String)> =
                data.sent_headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect();
            assert_eq!(sent, wire, "the snapshot is what went on the wire:\n{raw}");
            assert!(
                wire.iter().any(|(k, v)| k == "host" && v == &format!("127.0.0.1:{port}")),
                "the client's own host went out:\n{raw}"
            );
            assert!(!raw.contains("other.test") && !raw.contains(": 99"), "{raw}");
            assert!(!raw.to_ascii_lowercase().contains("transfer-encoding"), "{raw}");
            let accepts: Vec<&str> =
                wire.iter().filter(|(k, _)| k == "accept").map(|(_, v)| v.as_str()).collect();
            // postui's own on every method; a typed one goes out instead of
            // it, never next to it.
            let want = if extra == accept { "text/plain" } else { "application/json" };
            assert_eq!(accepts, [want], "one Accept on the wire:\n{raw}");
            if !userinfo.is_empty() && userinfo != "u%FF:p@" {
                assert!(wire.iter().any(|(k, _)| k == "authorization"), "{userinfo}: {raw}");
            }
            for h in &data.sent_headers {
                assert_eq!(h.display, h.value, "no secrets here, so display = value");
            }
        }
    }

    /// A received header value with bytes past ASCII is shown, not
    /// dropped: UTF-8 decodes as text and an invalid byte (a Latin-1 `é`)
    /// as U+FFFD, the same lossy decode the body gets. Content-Type too.
    #[tokio::test]
    async fn received_non_ascii_header_values_decode_lossily() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await.unwrap();
            let mut head = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n".to_vec();
            head.extend_from_slice("x-title: 日本語\r\n".as_bytes());
            head.extend_from_slice(b"x-legacy: caf\xe9\r\n");
            head.extend_from_slice("content-type: text/plain; name=ü\r\n\r\n".as_bytes());
            sock.write_all(&head).await.unwrap();
        });
        let url = format!("http://127.0.0.1:{port}/");
        let req = PreparedRequest {
            method: postui_core::model::Method::Get,
            url: url.clone(),
            display_url: url,
            headers: vec![],
            wire: vec![],
            body: None,
            insecure: false,
        };
        let data = send(&client(), &req).await.unwrap();
        server.await.unwrap();
        let value =
            |name: &str| data.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
        assert_eq!(value("x-title"), Some("日本語"));
        assert_eq!(value("x-legacy"), Some("caf\u{fffd}"));
        assert_eq!(value("content-type"), Some("text/plain; name=ü"));
        assert_eq!(data.content_type.as_deref(), Some("text/plain; name=ü"));
    }

    /// The masked twins travel with the wire rows: a secret in a prepared
    /// header or in the URL's host shows as its mask, while `value` keeps
    /// the real text for the copy pill.
    #[test]
    fn wire_rows_carry_masked_displays_for_secrets() {
        use postui_core::model::{Entry, HttpRequest};
        use postui_core::prepare::{PrepareContext, prepare};
        use postui_core::varmodel::VarMeta;
        let mask = postui_core::prepare::SECRET_MASK;
        let mut req =
            HttpRequest { url: "https://{{key}}.example.com:8443/p".into(), ..Default::default() };
        req.headers.insert(
            "Authorization".into(),
            Entry { value: "Bearer {{key}}".into(), enabled: true },
        );
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("key".into(), "s3cret".into());
        ctx.meta.insert("key".into(), VarMeta::Secret);
        let (req, _) = prepare(&req, &ctx).unwrap();
        let rows = req.wire;
        let auth = rows.iter().find(|r| r.name == "authorization").unwrap();
        assert_eq!(auth.value, "Bearer s3cret");
        assert_eq!(auth.display, format!("Bearer {mask}"));
        let host = rows.iter().find(|r| r.name == "host").unwrap();
        assert_eq!(host.value, "s3cret.example.com:8443");
        assert_eq!(host.display, mask, "a secret shapes the Host: all of it masked");
    }

    /// The rows the editor lists as sent (`header_rows`) and the
    /// response's sent section (`PreparedRequest::wire`) describe the same
    /// send: same names, same values, same order while the names are
    /// distinct (the editor keeps table order where the wire groups a
    /// name's values, core's `wire_order`) — including the shapes
    /// where they once disagreed (spec 2026-10-18 §1 and task #22): a
    /// default port, userinfo, a typed `Host`, `Content-Length` or
    /// `Transfer-Encoding`, and an empty body.
    #[test]
    fn the_auto_section_and_the_sent_section_agree() {
        use postui_core::model::{Body, Entry, HttpRequest, Method};
        use postui_core::prepare::{PrepareContext, header_rows, prepare};
        let on = |v: &str| Entry { value: v.into(), enabled: true };
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("host".into(), "api.example.com".into());
        ctx.vars.insert("api_key".into(), "s3cret".into());
        ctx.default_headers.insert("X-Trace".into(), on("on"));

        let mut full =
            HttpRequest { url: "https://{{host}}:8443/v1/things?q=1".into(), ..Default::default() };
        full.method = Method::Post;
        full.body = Some(Body::Json { text: "{\"k\": \"{{api_key}}\"}".into() });
        full.substitute_body = true;
        full.headers.insert("Authorization".into(), on("Bearer {{api_key}}"));
        let at = |url: &str| HttpRequest { url: url.into(), ..Default::default() };
        let mut typed_host = at("http://example.com/p");
        typed_host.headers.insert("Host".into(), on("other.test"));
        let mut typed_length = at("http://example.com/p");
        typed_length.method = Method::Post;
        typed_length.body = Some(Body::Json { text: "{}".into() });
        typed_length.headers.insert("Content-Length".into(), on("2"));
        let mut typed_encoding = typed_length.clone();
        typed_encoding.headers.insert("Transfer-Encoding".into(), on("chunked"));
        let mut empty = at("http://example.com/p");
        empty.method = Method::Put;
        empty.body = Some(Body::Json { text: String::new() });

        for req in [
            full,
            at("https://Example.com:443/p"),
            at("https://u:pw@example.com/p"),
            typed_host,
            typed_length,
            typed_encoding,
            empty,
        ] {
            let auto: Vec<(String, String)> = header_rows(&req, &ctx)
                .into_iter()
                .filter(|r| r.sent())
                .map(|r| (r.header.name.to_ascii_lowercase(), r.header.value))
                .collect();
            let (prepared, _) = prepare(&req, &ctx).unwrap();
            let sent: Vec<(String, String)> =
                prepared.wire.into_iter().map(|h| (h.name, h.value)).collect();
            assert_eq!(auto, sent, "{}", req.url);
        }
    }
}
