//! `postui run`, run for real in a throwaway home: how it picks the
//! environment and selections, that none of it is written back, and what
//! `--dry-run` prints with secrets masked (spec 2026-10-28 §4, R9-R11).

mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::process::Output;

const SECRET: &str = "s3cr3t-value";
const MASK: &str = "●●●●●●";

/// Piece 1's `write_project` plus a request that puts the secret in its URL,
/// a header and its body, and one that reads a shared selector. Both
/// environments point `base` at the given string and both hold the secret.
fn write_run_project(home: &Home, base: &str) -> PathBuf {
    let root = write_project(home, "api");
    let put = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    put(
        "environments/dev.toml",
        &format!("base = \"{base}\"\n\n[options.user.alice]\nuser_id = \"42\"\n"),
    );
    put(
        "environments/qa.toml",
        &format!("base = \"{base}\"\n\n[options.user.bob]\nuser_id = \"7\"\n"),
    );
    put(
        ".postui/secrets.toml",
        &format!("[dev]\ntoken = \"{SECRET}\"\n\n[qa]\ntoken = \"{SECRET}\"\n"),
    );
    put(
        "variables.toml",
        "[base]\ndescription = \"API root\"\n\n[token]\nsecret = true\n\n[id]\n\n\
         [region]\ndefault = \"eu\"\n\n[selectors.user]\nfields = [\"user_id\"]\n\n\
         [selectors.org]\nshared = true\nfields = [\"org_id\"]\n\n\
         [options.org.acme]\norg_id = \"A1\"\n\n[options.org.other]\norg_id = \"O2\"\n",
    );
    put(
        "requests/main/me.toml",
        r#"name = "Me"
method = "POST"
url = "{{base}}/me?key={{token}}"
substitute_body = true

[headers]
Authorization = "Bearer {{token}}"

[body]
type = "json"
text = '{"t": "{{token}}", "u": "{{user_id}}"}'
"#,
    );
    put(
        "requests/main/org.toml",
        "name = \"Org\"\nmethod = \"GET\"\nurl = \"{{base}}/org/{{org_id}}\"\n",
    );
    for name in ["ok", "missing", "bytes", "hang", "escaped"] {
        put(
            &format!("requests/main/{name}.toml"),
            &format!("name = \"{name}\"\nmethod = \"GET\"\nurl = \"{{{{base}}}}/{name}\"\n"),
        );
    }
    put(
        "requests/main/echo.toml",
        r#"name = "Echo"
method = "POST"
url = "{{base}}/echo"
substitute_body = true

[headers]
Authorization = "Bearer {{token}}"

[body]
type = "json"
text = '{"t": "{{token}}", "u": "{{user_id}}"}'
"#,
    );
    put(
        "requests/main/refused.toml",
        "name = \"Refused\"\nmethod = \"GET\"\nurl = \"http://127.0.0.1:1/x?key={{token}}\"\n",
    );
    root
}

/// `run <slug> -p <root>` and then `extra`, against a server.
fn send(home: &Home, root: &Path, slug: &str, extra: &[&str]) -> Output {
    let mut args = vec!["run", slug, "-p", s(root)];
    args.extend_from_slice(extra);
    home.run(&args)
}

fn served() -> (Home, Server, PathBuf) {
    let home = Home::new();
    let server = Server::start();
    let root = write_run_project(&home, &server.base());
    (home, server, root)
}

/// `run <slug> --dry-run -p <root>` and then `extra`.
fn dry(home: &Home, root: &Path, slug: &str, extra: &[&str]) -> Output {
    let mut args = vec!["run", slug, "--dry-run", "-p", s(root)];
    args.extend_from_slice(extra);
    home.run(&args)
}

fn state(root: &Path, text: &str) {
    std::fs::write(root.join(".postui/state.toml"), text).unwrap();
}

#[test]
fn dry_run_prints_the_prepared_request_masked() {
    let home = Home::new();
    let root = write_run_project(&home, "https://dev.example.com");

    let out = dry(&home, &root, "main/me", &[]);
    assert_ok(&out, "run --dry-run");
    let v = json(&out);
    assert_eq!(v["request"], "main/me");
    assert_eq!(v["file"], s(&root.join("requests/main/me.toml")));
    assert_eq!(v["environment"], "dev");
    assert_eq!(v["method"], "POST");
    assert_eq!(v["dry_run"], true);
    for key in ["response", "jq", "output_file", "error"] {
        assert!(v[key].is_null(), "{key}: {}", v[key]);
    }
    assert_eq!(v["url"], format!("https://dev.example.com/me?key={MASK}"));
    let auth = v["request_headers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "authorization")
        .unwrap_or_else(|| panic!("no authorization header in {}", v["request_headers"]));
    assert_eq!(auth["value"], format!("Bearer {MASK}"));
    let body = v["request_body"].as_str().unwrap();
    assert!(body.contains(MASK) && body.contains("\"u\": \"42\""), "{body}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));

    let out = dry(&home, &root, "main/me", &["--text"]);
    assert_ok(&out, "run --dry-run --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("POST") && text.contains(MASK) && !text.contains(SECRET), "{text}");
}

#[test]
fn dry_run_show_secrets_reveals() {
    let home = Home::new();
    let root = write_run_project(&home, "https://dev.example.com");

    let out = dry(&home, &root, "main/me", &["--show-secrets"]);
    assert_ok(&out, "run --dry-run --show-secrets");
    let v = json(&out);
    assert_eq!(v["url"], format!("https://dev.example.com/me?key={SECRET}"));
    assert!(v["request_body"].as_str().unwrap().contains(SECRET));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(MASK));
}

#[test]
fn select_overrides_for_this_run_only() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");
    let before = tree(&root);

    let out = dry(&home, &root, "main/me", &["--select", "user=bob", "--env", "qa"]);
    assert_ok(&out, "--select user=bob --env qa");
    let v = json(&out);
    assert_eq!(v["environment"], "qa");
    assert!(v["request_body"].as_str().unwrap().contains("\"u\": \"7\""), "{}", v["request_body"]);
    assert_eq!(tree(&root), before, "an override is never written back");

    let out = dry(&home, &root, "main/me", &[]);
    assert_ok(&out, "no override");
    let v = json(&out);
    assert_eq!(v["environment"], "dev");
    assert!(v["request_body"].as_str().unwrap().contains("\"u\": \"42\""), "{}", v["request_body"]);
    assert_eq!(tree(&root), before);
}

#[test]
fn saved_env_and_selections_are_used() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");

    state(&root, "environment = \"qa\"\n\n[selections.qa]\nuser = \"bob\"\n");
    let before = tree(&root);
    let out = dry(&home, &root, "main/me", &[]);
    assert_ok(&out, "saved env and selection");
    let v = json(&out);
    assert_eq!(v["environment"], "qa");
    assert!(v["request_body"].as_str().unwrap().contains("\"u\": \"7\""), "{}", v["request_body"]);
    assert_eq!(tree(&root), before);

    // A shared selector's saved pick is used, and a --select beats it.
    state(&root, "environment = \"dev\"\n\n[shared_selections]\norg = \"acme\"\n");
    let before = tree(&root);
    let out = dry(&home, &root, "main/org", &[]);
    assert_ok(&out, "shared selection");
    assert_eq!(json(&out)["url"], "https://x.example.com/org/A1");
    let out = dry(&home, &root, "main/org", &["--select", "org=other"]);
    assert_ok(&out, "--select org=other");
    assert_eq!(json(&out)["url"], "https://x.example.com/org/O2");
    assert_eq!(tree(&root), before);

    // A saved pick that no longer exists leaves the request unresolved.
    state(&root, "environment = \"dev\"\n\n[shared_selections]\norg = \"gone\"\n");
    let before = tree(&root);
    let out = dry(&home, &root, "main/org", &[]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert!(v["error"].as_str().unwrap().contains("need a selection"), "{}", v["error"]);
    assert!(v["response"].is_null());
    assert_eq!(tree(&root), before);

    state(&root, "environment = \"dev\"\n\n[selections.dev]\nuser = \"carol\"\n");
    let before = tree(&root);
    let out = dry(&home, &root, "main/me", &[]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert!(v["error"].as_str().unwrap().contains("need a selection"), "{}", v["error"]);
    assert_eq!(tree(&root), before);
}

#[test]
fn select_mistakes_name_the_choices() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");

    let error = |select: &str| {
        let out = dry(&home, &root, "main/me", &["--select", select]);
        assert_eq!(out.status.code(), Some(2), "{select}");
        json(&out)["error"].as_str().unwrap().to_string()
    };
    assert!(error("nobody=x").contains("user"));
    assert!(error("user=nobody").contains("alice"));
    assert!(error("user").contains("SELECTOR=OPTION"));
}

#[test]
fn unknown_request_is_an_error() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");

    let out = dry(&home, &root, "main/nope", &[]);
    assert_eq!(out.status.code(), Some(2));
    let message = json(&out)["error"].as_str().unwrap().to_string();
    assert!(message.contains("main/nope") && message.contains("postui requests"), "{message}");
}

#[test]
fn unresolved_variable_is_a_failed_result() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");
    std::fs::write(
        root.join("requests/main/typo.toml"),
        "name = \"Typo\"\nmethod = \"GET\"\nurl = \"{{bsae}}/x\"\n",
    )
    .unwrap();

    let out = dry(&home, &root, "main/typo", &[]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert!(v["error"].as_str().unwrap().contains("bsae"), "{}", v["error"]);
    assert!(v["response"].is_null());
    assert_eq!(v["request"], "main/typo");
}

#[test]
fn run_finds_its_project_like_the_other_commands() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");
    let empty = home.project("empty", "Empty");
    home.register(&[&root, &empty]);
    let config = tree(&home.config_dir());

    let out = home.run_in(&root, &["run", "main/me", "--dry-run"], |_| {});
    assert_ok(&out, "from the project directory");
    assert_eq!(json(&out)["request"], "main/me");

    let out = home.run(&["run", "main/me", "--dry-run", "-p", "Empty"]);
    assert_eq!(out.status.code(), Some(2), "a registered project without that request");
    assert!(json(&out)["error"].as_str().unwrap().contains("postui requests"));

    assert_eq!(tree(&home.config_dir()), config);
}

#[test]
fn run_reports_the_response() {
    let (home, server, root) = served();
    let before = tree(&root);

    let out = send(&home, &root, "main/ok", &[]);
    assert_ok(&out, "run main/ok");
    let v = json(&out);
    let r = &v["response"];
    assert_eq!(r["status"], 200);
    assert_eq!(r["body"], "{\"id\":42}");
    assert_eq!(r["body_json"], serde_json::json!({"id": 42}));
    assert_eq!(r["size"], 9);
    assert!(r["ttfb_ms"].as_u64().unwrap() <= r["elapsed_ms"].as_u64().unwrap());
    let has_type = r["headers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h["name"] == "content-type" && h["value"] == "application/json");
    assert!(has_type, "{}", r["headers"]);
    assert!(v["error"].is_null() && v["jq"].is_null() && v["output_file"].is_null());
    assert_eq!(v["dry_run"], false);
    assert_eq!(server.seen(), ["GET /ok"]);
    assert_eq!(tree(&root), before, "a send writes nothing into the project");

    let out = send(&home, &root, "main/ok", &["--text"]);
    assert_ok(&out, "run --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("200") && text.contains("{\"id\":42}"), "{text}");
}

#[test]
fn run_a_404_exits_1() {
    let (home, _server, root) = served();
    let out = send(&home, &root, "main/missing", &[]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert_eq!(v["response"]["status"], 404);
    assert_eq!(v["response"]["body"], "nope");
    assert!(v["error"].is_null());
}

#[test]
fn run_masks_an_echoed_secret() {
    let (home, _server, root) = served();
    let header_value = |v: &serde_json::Value| {
        v["response"]["headers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == "x-echo")
            .unwrap_or_else(|| panic!("no x-echo in {}", v["response"]["headers"]))["value"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let out = send(&home, &root, "main/echo", &[]);
    assert_ok(&out, "run main/echo");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
    let v = json(&out);
    assert_eq!(header_value(&v), format!("Bearer {MASK}"));
    assert!(v["response"]["body"].as_str().unwrap().contains(MASK));
    assert!(v["response"]["body_json"]["t"] == MASK, "{}", v["response"]["body_json"]);

    let out = send(&home, &root, "main/echo", &["--text"]);
    assert_ok(&out, "run main/echo --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(MASK) && !text.contains(SECRET), "{text}");

    let out = send(&home, &root, "main/echo", &["--show-secrets"]);
    assert_ok(&out, "run main/echo --show-secrets");
    let v = json(&out);
    assert_eq!(header_value(&v), format!("Bearer {SECRET}"));
    assert!(v["response"]["body"].as_str().unwrap().contains(SECRET));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(MASK));
}

#[test]
fn run_output_file_gets_raw_bytes() {
    let (home, _server, root) = served();
    let file = home.path().join("out.bin");

    let out = send(&home, &root, "main/bytes", &["-o", s(&file)]);
    assert_ok(&out, "run -o");
    assert_eq!(std::fs::read(&file).unwrap(), b"ok\xff\xfe");
    let v = json(&out);
    assert_eq!(v["output_file"], s(&file));
    assert!(v["response"]["body"].is_null() && v["response"]["body_json"].is_null());
    assert_eq!(v["response"]["size"], 4);

    // A relative name lands in the working directory and is reported absolute.
    let out = home.run(&["run", "main/bytes", "-p", s(&root), "--output", "rel.bin"]);
    assert_ok(&out, "run --output rel.bin");
    assert_eq!(json(&out)["output_file"], s(&home.path().join("rel.bin")));
    assert_eq!(std::fs::read(home.path().join("rel.bin")).unwrap(), b"ok\xff\xfe");

    let out = send(&home, &root, "main/bytes", &[]);
    assert_ok(&out, "run without -o");
    let v = json(&out);
    assert_eq!(v["response"]["body"], "ok\u{fffd}\u{fffd}");
    assert_eq!(v["response"]["size"], 4);
    assert!(v["response"]["body_json"].is_null());
}

#[test]
fn run_output_file_failure_is_an_envelope() {
    let (home, _server, root) = served();
    // A directory can't be replaced by a file.
    let out = send(&home, &root, "main/bytes", &["-o", s(&root)]);
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    assert!(v["error"].as_str().unwrap().contains("can't write"), "{}", v["error"]);
    assert_eq!(v["file"], s(&root));
}

#[test]
fn run_jq_prints_the_filters_output() {
    let (home, _server, root) = served();

    let out = send(&home, &root, "main/ok", &["--jq", ".id"]);
    assert_ok(&out, "run --jq .id");
    let v = json(&out);
    assert_eq!(v["jq"]["filter"], ".id");
    assert_eq!(v["jq"]["output"], serde_json::json!([42]));
    assert!(v["jq"]["error"].is_null());
    assert!(v["response"]["body"].is_null() && v["response"]["body_json"].is_null());
    assert_eq!(v["response"]["status"], 200);

    let out = send(&home, &root, "main/bytes", &["--jq", "."]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert!(v["jq"]["error"].as_str().is_some_and(|e| !e.is_empty()), "{}", v["jq"]);
    assert!(v["jq"]["output"].is_null());

    let file = home.path().join("id.txt");
    let out = send(&home, &root, "main/ok", &["--jq", ".id", "-o", s(&file)]);
    assert_ok(&out, "run --jq -o");
    assert_eq!(std::fs::read(&file).unwrap(), b"42\n");
    let v = json(&out);
    assert_eq!(v["output_file"], s(&file));
    assert!(v["jq"]["output"].is_null());

    // A filter that fails writes no file.
    let none = home.path().join("none.txt");
    let out = send(&home, &root, "main/ok", &["--jq", ".id.x", "-o", s(&none)]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!none.exists());
    assert!(json(&out)["output_file"].is_null());
}

#[test]
fn run_jq_output_is_masked() {
    let (home, _server, root) = served();
    let out = send(&home, &root, "main/echo", &["--jq", ".t"]);
    assert_ok(&out, "run --jq .t");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
    assert_eq!(json(&out)["jq"]["output"], serde_json::json!([MASK]));
}

/// A secret the server wrote with a JSON escape is plain text once parsed:
/// `body_json`, `--jq` output and the `-o` lines must not show it.
#[test]
fn run_masks_a_json_escaped_secret_in_parsed_output() {
    let (home, _server, root) = served();

    let out = send(&home, &root, "main/escaped", &[]);
    assert_ok(&out, "run main/escaped");
    assert_eq!(json(&out)["response"]["body_json"], serde_json::json!({"t": MASK}));

    let out = send(&home, &root, "main/escaped", &["--jq", ".t"]);
    assert_ok(&out, "run main/escaped --jq .t");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
    assert_eq!(json(&out)["jq"]["output"], serde_json::json!([MASK]));

    let file = home.path().join("t.txt");
    let out = send(&home, &root, "main/escaped", &["--jq", ".t", "-o", s(&file)]);
    assert_ok(&out, "run main/escaped --jq .t -o");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("\"{MASK}\"\n"));
}

/// A jq error that quotes the secret, and a filter that holds it, are
/// masked; with --show-secrets the same error does name it.
#[test]
fn run_jq_error_and_filter_are_masked() {
    let (home, _server, root) = served();

    let out = send(&home, &root, "main/echo", &["--jq", ".t | tonumber"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
    let error = json(&out)["jq"]["error"].as_str().unwrap().to_string();
    assert!(error.contains(MASK), "{error}");

    let out = send(&home, &root, "main/echo", &["--jq", ".t | tonumber", "--show-secrets"]);
    assert!(json(&out)["jq"]["error"].as_str().unwrap().contains(SECRET));

    let filter = format!("\"{SECRET}\" | tonumber");
    let out = send(&home, &root, "main/ok", &["--jq", &filter]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
    assert_eq!(json(&out)["jq"]["filter"], format!("\"{MASK}\" | tonumber"));
    let out = send(&home, &root, "main/ok", &["--jq", &filter, "--text"]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
}

#[test]
fn run_timeout_is_a_send_error() {
    let (home, _server, root) = served();
    let started = std::time::Instant::now();
    let out = send(&home, &root, "main/hang", &["--timeout", "1"]);
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert!(v["response"].is_null());
    let error = v["error"].as_str().unwrap().to_lowercase();
    assert!(error.contains("timed out") || error.contains("timeout"), "{error}");
}

#[test]
fn run_send_error_is_masked() {
    let (home, _server, root) = served();
    let out = send(&home, &root, "main/refused", &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
    let v = json(&out);
    assert!(v["response"].is_null());
    assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()));
}

#[test]
fn run_dry_run_sends_nothing() {
    let (home, server, root) = served();
    let out = dry(&home, &root, "main/ok", &[]);
    assert_ok(&out, "run --dry-run");
    assert!(server.seen().is_empty());
}
