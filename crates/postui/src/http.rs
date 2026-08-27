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
    let mut header_map = reqwest::header::HeaderMap::new();
    for (k, v) in &req.headers {
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| format!("invalid header name {k:?}: {e}"))?;
        let value = reqwest::header::HeaderValue::from_str(v)
            .map_err(|e| format!("invalid header value for {k:?}: {e}"))?;
        header_map.insert(name, value);
    }
    builder = builder.headers(header_map);
    if let Some(body) = &req.body {
        builder = builder.body(body.clone());
    }

    let started = Instant::now();
    let result = builder.send().await;
    // `send` resolves once the response headers are in — the closest thing
    // reqwest exposes to "first byte".
    let ttfb = started.elapsed();
    let response = result.map_err(|e| error_chain(&e))?;

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
    let bytes = response.bytes().await.map_err(|e| error_chain(&e))?;
    let elapsed = started.elapsed();
    let size = bytes.len();
    let body = String::from_utf8_lossy(&bytes).into_owned();

    Ok(ResponseData {
        status,
        headers,
        body,
        ttfb,
        elapsed,
        size,
        content_type,
    })
}

/// Joins a reqwest error and its `source()` chain with ": ", so e.g. a
/// connection refused shows the useful underlying I/O message rather than
/// just reqwest's generic wrapper text.
fn error_chain(err: &reqwest::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_builds_without_a_tokio_runtime() {
        // No #[tokio::test] here on purpose: this is the load-bearing check
        // for App staying constructible in plain sync tests.
        let _c = client();
    }

    /// A server that sends its headers immediately but stalls before the
    /// body separates the two measures: `ttfb` stops at the headers,
    /// `elapsed` keeps counting until the body completes.
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
            headers: vec![],
            body: None,
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
