//! The vim profile's response pane against real Vim 9.1: every case in
//! `tests/vim_conformance/response_golden.jsonl` (written by
//! `scripts/vim_oracle/response.py` from `response_corpus.toml`) is typed
//! on a fresh app and must leave the window as Vim's: the cursor (its
//! hidden column too), the top line, the V anchor, the closed folds, the
//! unnamed register, the last pattern and its direction, and the message
//! the message line ends on (the last toast; Vim's without its `E123:`).

use super::*;
use crate::config::KeyProfile;
use serde_json::Value;

const GOLDEN: &str = include_str!("../../tests/vim_conformance/response_golden.jsonl");

/// Cases that differ from Vim on purpose, with the reason. Empty: every
/// case must match.
const DIVERGENCES: &[(&str, &str)] = &[];

fn ready(app: &mut App, body: &str) {
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            url: "https://x.test/a".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            sent_headers: Vec::new(),
            body: body.to_string(),
            ttfb: std::time::Duration::from_millis(1),
            elapsed: std::time::Duration::from_millis(1),
            size: body.len(),
            content_type: Some("application/json".into()),
        })),
        app.session.send_generation,
    );
}

fn pair(v: &Value) -> (usize, usize) {
    let n = |i: usize| v[i].as_u64().expect("a number") as usize;
    (n(0), n(1))
}

/// What the case left, in the golden's terms, or the first mismatch.
fn run(head: &Value, case: &Value) -> Result<(), String> {
    let body = &head["bodies"][case["body"].as_str().unwrap()];
    let mut app = App::new_for_test();
    app.use_profile(KeyProfile::Vim);
    ready(&mut app, body["body"].as_str().unwrap());
    app.focus = PaneId::Response;
    // The app settles the new response (its jq pass) before anyone types,
    // as the event loop does when it lands.
    app.update(Action::Render);
    let height = head["winheight"].as_u64().unwrap() as usize;
    app.session.response.set_view_height(height);
    let want_lines: Vec<String> =
        body["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    let before = app.session.response.vim_window().ok_or("no ready view")?;
    if before.lines != want_lines {
        return Err(format!("the pane's lines are not Vim's: {:?}", before.lines));
    }
    for ev in crate::keyroute::notation::events(case["keys"].as_str().unwrap()) {
        app.handle_key(ev);
        app.session.response.set_view_height(height);
    }
    let win = app.session.response.vim_window().ok_or("no ready view")?;
    let want = &case["expect"];
    let mut diffs = Vec::new();
    let mut check = |what: &str, got: String, want: String| {
        if got != want {
            diffs.push(format!("{what}: got {got}, Vim {want}"));
        }
    };
    let mode = if app.session.response.line_selection() { "V" } else { "n" };
    check("mode", mode.into(), want["mode"].as_str().unwrap().into());
    check("cursor", format!("{:?}", win.cursor), format!("{:?}", pair(&want["cursor"])));
    check("top", win.top.to_string(), want["top"].to_string());
    let anchor = want.get("visual").map(pair);
    check("anchor", format!("{:?}", win.anchor), format!("{anchor:?}"));
    let closed: Vec<(usize, usize)> = want["closed"].as_array().unwrap().iter().map(pair).collect();
    check("closed", format!("{:?}", win.closed), format!("{closed:?}"));
    let reg = app.glue.unnamed();
    let kind = match reg.kind {
        crate::vim::engine::RegKind::Line => "V",
        crate::vim::engine::RegKind::Char => "v",
    };
    check("reg", format!("{:?}/{kind}", reg.text), format!("{:?}/{}", want["reg"].as_str().unwrap(), want["regtype"].as_str().unwrap()));
    let (pattern, fwd) = match app.session.response.search_memory().pattern() {
        Some((p, backward)) => (p.to_string(), if backward { 0 } else { 1 }),
        None => (String::new(), 1),
    };
    check("search", format!("{pattern:?}/{fwd}"), format!("{:?}/{}", want["search"].as_str().unwrap(), want["fwd"]));
    check("msg", format!("{:?}", app.toasts.last_message()), format!("{:?}", want["msg"].as_str()));
    if diffs.is_empty() { Ok(()) } else { Err(diffs.join("; ")) }
}

#[test]
fn the_response_matches_vim() {
    let mut rows = GOLDEN.lines().map(|l| serde_json::from_str::<Value>(l).expect("golden JSON"));
    let head = rows.next().expect("a header")["header"].clone();
    let mut failures = Vec::new();
    let mut total = 0;
    for case in rows {
        total += 1;
        let id = case["id"].as_str().unwrap().to_string();
        let diverges = DIVERGENCES.iter().any(|(d, _)| *d == id);
        match (run(&head, &case), diverges) {
            (Err(m), false) => failures.push(format!("{id}: {m}")),
            (Ok(()), true) => failures.push(format!("{id}: listed as a divergence but matches Vim")),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{} of {total} case(s) differ from Vim:\n{}", failures.len(), failures.join("\n"));
}
