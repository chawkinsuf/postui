//! The reqwest-backed send path: building a client, issuing a prepared
//! request, and shaping the result into something the UI can render without
//! knowing anything about reqwest itself.
use postui_core::prepare::PreparedRequest;
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
    pub body: String,
    /// Time to first byte: send → response headers received.
    pub ttfb: Duration,
    /// Total: send → body fully downloaded.
    pub elapsed: Duration,
    pub size: usize,
    pub content_type: Option<String>,
}

/// Builds the client used for all requests. Deliberately no timeout of any
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
    pub fn new() -> Self {
        // Same fallback story as `client_with_timeout`: `build()` only fails
        // on TLS backend init, and if the insecure variant somehow can't be
        // built, failing closed (a verifying client) is the safe default.
        Self {
            verifying: client(),
            insecure: reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Picks the client `req` should be sent on.
    pub fn for_request(&self, req: &PreparedRequest) -> &reqwest::Client {
        if req.insecure {
            &self.insecure
        } else {
            &self.verifying
        }
    }
}

/// Like [`client`] but with a total timeout, for tests that need a request
/// to die quickly on its own (the real client never times out).
pub fn client_with_timeout(timeout: Duration) -> reqwest::Client {
    // `build()` can only fail on TLS backend initialization; with the config
    // here (no custom certs/proxies) that's practically unreachable. Fall
    // back to the plain default client rather than panicking if it ever
    // does.
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Sends `req` and shapes the result (or the error chain) for the UI.
/// Never panics: header/method construction errors and transport errors
/// alike come back as `Err(String)`.
pub async fn send(client: &reqwest::Client, req: &PreparedRequest) -> Result<ResponseData, String> {
    let method = reqwest::Method::from_bytes(req.method.as_str().as_bytes())
        .map_err(|e| format!("invalid method: {e}"))?;
    let mut builder = client.request(method, &req.url);
    builder = builder.headers(header_map(&req.headers)?);
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
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = response.bytes().await.map_err(|e| error_chain(&e, req))?;
    let elapsed = started.elapsed();
    let size = bytes.len();
    let body = String::from_utf8_lossy(&bytes).into_owned();

    Ok(ResponseData {
        status,
        url: req.display_url.clone(),
        headers,
        body,
        ttfb,
        elapsed,
        size,
        content_type,
    })
}

/// Every prepared header, in order. `append`, not `insert`: two headers
/// that differ only in case (`Accept` / `accept`), or two `{{templates}}`
/// that resolve to the same name, are both what the user wrote, so both
/// go on the wire rather than the later one silently replacing the first.
fn header_map(headers: &[(String, String)]) -> Result<reqwest::header::HeaderMap, String> {
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
    mask_error_text(
        &parts.join(": "),
        err.url().map(|u| u.as_str()),
        &req.url,
        &req.display_url,
    )
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
        assert_eq!(
            out,
            format!("error sending request: boom ({display}) for url ({display})")
        );
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
    }

    /// A server that sends its headers immediately but stalls before the
    /// body separates the two measures: `ttfb` stops at the headers,
    /// `elapsed` keeps counting until the body completes.
    #[test]
    fn same_named_headers_are_all_sent() {
        let map = header_map(&[
            ("Accept".into(), "a".into()),
            ("accept".into(), "b".into()),
            ("X-One".into(), "1".into()),
        ])
        .unwrap();
        let accepts: Vec<&str> = map
            .get_all("accept")
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(accepts, ["a", "b"], "neither case-variant is dropped");
        assert_eq!(map.len(), 3);
        assert!(header_map(&[("bad name".into(), "x".into())]).is_err());
    }

    #[tokio::test]
    async fn a_transport_error_shows_the_masked_url_never_the_secret() {
        let req = PreparedRequest {
            method: postui_core::model::Method::Get,
            url: "http://127.0.0.1:1/v1/items?key=sk-live-REAL".into(),
            display_url: "http://127.0.0.1:1/v1/items?key=•••••".into(),
            headers: vec![],
            body: None,
            insecure: false,
        };
        let err = send(&client(), &req)
            .await
            .expect_err("nothing listens on port 1");
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
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n")
                .unwrap();
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
}
