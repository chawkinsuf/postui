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
    for name in ["ok", "missing", "bytes", "hang"] {
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

/// `run <slug> -p <root> --json` and then `extra`, against a server.
fn send(home: &Home, root: &Path, slug: &str, extra: &[&str]) -> Output {
    let mut args = vec!["run", slug, "-p", s(root)];
    args.extend_from_slice(extra);
    home.run_json(&args)
}

/// [`send`] printing text.
fn send_text(home: &Home, root: &Path, slug: &str, extra: &[&str]) -> String {
    let mut args = vec!["run", slug, "-p", s(root)];
    args.extend_from_slice(extra);
    String::from_utf8_lossy(&home.run(&args).stdout).into_owned()
}

/// `run <slug> -p <root>` and then `extra`, printing text: the exit code,
/// stdout and stderr.
fn streams(home: &Home, root: &Path, slug: &str, extra: &[&str]) -> (Option<i32>, String, String) {
    let mut args = vec!["run", slug, "-p", s(root)];
    args.extend_from_slice(extra);
    let out = home.run(&args);
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (out.status.code(), text(&out.stdout), text(&out.stderr))
}

fn served() -> (Home, Server, PathBuf) {
    let home = Home::new();
    let server = Server::start();
    let root = write_run_project(&home, &server.base());
    (home, server, root)
}

/// `run <slug> --dry-run -p <root> --json` and then `extra`.
fn dry(home: &Home, root: &Path, slug: &str, extra: &[&str]) -> Output {
    let mut args = vec!["run", slug, "--dry-run", "-p", s(root)];
    args.extend_from_slice(extra);
    home.run_json(&args)
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

    let out = home.run(&["run", "main/me", "--dry-run", "-p", s(&root)]);
    assert_ok(&out, "run --dry-run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("POST") && text.contains(MASK) && !text.contains(SECRET), "{text}");
    // R20: the file relative to the root the text names, in the head every
    // report starts with.
    assert!(
        text.starts_with(&format!(
            "POST {}\nrequest      main/me\nroot         {}\nfile         requests/main/me.toml\n\
             environment  dev\ndry run      nothing was sent\n",
            "https://dev.example.com/me?key=●●●●●●",
            s(&root)
        )),
        "{text}"
    );
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
    // No `=`: a usage error, caught when the line is read.
    let out = dry(&home, &root, "main/me", &["--select", "user"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(json(&out)["error"].as_str().unwrap().contains("--select takes SELECTOR=OPTION"));
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
    // Refused, it still shows the method and the URL as written.
    assert_eq!(v["method"], "GET");
    assert_eq!(v["url"], "{{bsae}}/x");
}

#[test]
fn run_finds_its_project_like_the_other_commands() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");
    let empty = home.project("empty", "Empty");
    home.register(&[&root, &empty]);
    let config = tree(&home.config_dir());

    let out = home.run_in(&root, &["run", "main/me", "--dry-run", "--json"], |_| {});
    assert_ok(&out, "from the project directory");
    assert_eq!(json(&out)["request"], "main/me");

    let out = home.run_json(&["run", "main/me", "--dry-run", "-p", "Empty"]);
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
    // A JSON body is given once, parsed.
    assert!(r["body"].is_null(), "{}", r["body"]);
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

    // `-v`'s text prints the body exactly as it came, once.
    let text = send_text(&home, &root, "main/ok", &["-v"]);
    assert!(text.contains("\nresponse\n  200 in "), "{text}");
    assert!(text.ends_with("\nresponse body\n{\"id\":42}\n"), "{text}");
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
    assert!(v["response"]["body"].is_null());
    assert!(v["response"]["body_json"]["t"] == MASK, "{}", v["response"]["body_json"]);

    let text = send_text(&home, &root, "main/echo", &[]);
    assert!(text.contains(MASK) && !text.contains(SECRET), "{text}");

    let out = send(&home, &root, "main/echo", &["--show-secrets"]);
    assert_ok(&out, "run main/echo --show-secrets");
    let v = json(&out);
    assert_eq!(header_value(&v), format!("Bearer {SECRET}"));
    assert_eq!(v["response"]["body_json"]["t"], SECRET);
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

    // R20: the text names the file absolute, even one inside the project:
    // it is the user's, named from their working directory.
    let inside = root.join("out.bin");
    let text = send_text(&home, &root, "main/bytes", &["-o", s(&inside), "-v"]);
    assert!(text.contains(&format!("\nwritten to {}\n", s(&inside))), "{text}");
    // Without `-v`, as curl: nothing printed, the body is in the file.
    assert_eq!(send_text(&home, &root, "main/bytes", &["-o", s(&inside)]), "");

    // A relative name lands in the working directory and is reported absolute.
    let out = home.run_json(&["run", "main/bytes", "-p", s(&root), "--output", "rel.bin"]);
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
fn run_output_file_failure_keeps_the_response() {
    let (home, _server, root) = served();
    // A directory can't be replaced by a file.
    let out = send(&home, &root, "main/ok", &["-o", s(&root)]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert!(v["error"].as_str().unwrap().starts_with("could not write"), "{}", v["error"]);
    assert!(v["output_file"].is_null());
    // The request was sent, so what came back is in the report.
    assert_eq!(v["response"]["status"], 200);
    assert_eq!(v["response"]["body_json"], serde_json::json!({"id": 42}));

    // Under a file, with --jq: the results show.
    let blocker = home.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let missing = blocker.join("out.txt");
    let out = send(&home, &root, "main/ok", &["--jq", ".id", "-o", s(&missing)]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert!(v["error"].as_str().unwrap().contains("out.txt"), "{}", v["error"]);
    assert_eq!(v["jq"]["output"], serde_json::json!([42]));
    assert!(v["response"]["body"].is_null());
}

/// `-o` writes in place, like `curl -o`: `/dev/null` works, and a symlink is
/// written through, not replaced.
#[cfg(unix)]
#[test]
fn run_output_file_writes_in_place() {
    let (home, _server, root) = served();
    let out = send(&home, &root, "main/ok", &["-o", "/dev/null"]);
    assert_ok(&out, "run -o /dev/null");

    let target = home.path().join("target.txt");
    std::fs::write(&target, "old old old old").unwrap();
    let link = home.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let out = send(&home, &root, "main/ok", &["-o", s(&link)]);
    assert_ok(&out, "run -o symlink");
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), r#"{"id":42}"#);
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
    // The response is not lost: the caller needn't send the request again.
    assert_eq!(v["response"]["status"], 200);
    assert_eq!(v["response"]["body"], "ok\u{fffd}\u{fffd}");

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
    let v = json(&out);
    assert!(v["output_file"].is_null());
    assert!(v["response"]["body"].is_null());
    assert_eq!(v["response"]["body_json"], serde_json::json!({"id": 42}));
}

/// A failed run whose reader has gone (stdout a closed pipe) still exits 1.
#[test]
fn a_failed_run_to_a_closed_stdout_exits_1() {
    let home = Home::new();
    let root = write_run_project(&home, "https://x.example.com");
    std::fs::write(
        root.join("requests/main/typo.toml"),
        "name = \"Typo\"\nmethod = \"GET\"\nurl = \"{{bsae}}/x\"\n",
    )
    .unwrap();
    let mut child = home
        .command(home.path(), &["run", "main/typo", "--dry-run", "-p", s(&root)])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    assert_eq!(child.wait().unwrap().code(), Some(1));
}

#[test]
fn run_jq_output_is_masked() {
    let (home, _server, root) = served();
    let out = send(&home, &root, "main/echo", &["--jq", ".t"]);
    assert_ok(&out, "run --jq .t");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(SECRET));
    assert_eq!(json(&out)["jq"]["output"], serde_json::json!([MASK]));
}

/// `--jq -o` writes the masked results.
#[test]
fn run_jq_output_file_is_masked() {
    let (home, _server, root) = served();
    let file = home.path().join("t.txt");
    let out = send(&home, &root, "main/echo", &["--jq", ".t", "-o", s(&file)]);
    assert_ok(&out, "run main/echo --jq .t -o");
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
    let text = send_text(&home, &root, "main/ok", &["--jq", &filter, "-v"]);
    assert!(!text.contains(SECRET) && text.contains(MASK), "{text}");
    let (_, _, err) = streams(&home, &root, "main/echo", &["--jq", ".t | tonumber"]);
    assert!(!err.contains(SECRET) && err.contains(MASK), "{err}");
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

/// Spec 2026-10-30 D5: the body alone, as it came, as curl prints it.
#[test]
fn run_prints_the_body_alone_by_default() {
    let (home, _server, root) = served();
    assert_eq!(streams(&home, &root, "main/ok", &[]), (Some(0), "{\"id\":42}\n".into(), "".into()));
    let (_, out, _) = streams(&home, &root, "main/ok", &["--jq", ".id"]);
    assert_eq!(out, "42\n");
}

/// D5: `-i` puts the status line and the response headers, as `name:
/// value`, and a blank line before the body; `-v` the whole report.
#[test]
fn run_include_and_verbose_show_more() {
    let (home, _server, root) = served();
    let (code, out, err) = streams(&home, &root, "main/ok", &["-i"]);
    assert_eq!((code, err.as_str()), (Some(0), ""));
    let (head, body) = out.split_once("\n\n").unwrap_or_else(|| panic!("{out}"));
    assert!(head.starts_with("200 OK  ") && head.contains(" ms (first byte "), "{head}");
    assert!(head.lines().next().unwrap().ends_with(", 9 bytes"), "{head}");
    assert!(head.lines().any(|l| l == "content-type: application/json"), "{head}");
    assert_eq!(body, "{\"id\":42}\n");

    let (_, out, _) = streams(&home, &root, "main/ok", &["-v"]);
    assert!(out.starts_with("GET http://127.0.0.1:"), "{out}");
    assert!(out.contains("\nrequest headers\n") && out.contains("\nresponse body\n"), "{out}");
    // A dry run prints the report too: the request is what it is for.
    let (_, out, _) = streams(&home, &root, "main/ok", &["--dry-run"]);
    assert!(out.contains("dry run      nothing was sent"), "{out}");
}

/// D6: what matters and isn't the body goes to stderr, a `postui:` line
/// each, and the exit codes don't change.
#[test]
fn run_says_on_stderr_what_failed() {
    let (home, _server, root) = served();
    assert_eq!(
        streams(&home, &root, "main/missing", &[]),
        (Some(1), "nope\n".into(), "postui: 404 Not Found\n".into())
    );
    let (code, out, err) = streams(&home, &root, "main/refused", &[]);
    assert_eq!((code, out.as_str()), (Some(1), ""));
    assert!(err.starts_with("postui: ") && !err.contains(SECRET), "{err}");
    let (code, out, err) = streams(&home, &root, "main/bytes", &["--jq", "."]);
    assert_eq!(code, Some(1));
    assert_eq!(out, "ok\u{fffd}\u{fffd}\n", "the body shows when the filter fails");
    assert!(err.starts_with("postui: jq: "), "{err}");

    std::fs::write(
        root.join("requests/main/typo.toml"),
        "name = \"Typo\"\nmethod = \"GET\"\nurl = \"{{bsae}}/x\"\n",
    )
    .unwrap();
    assert_eq!(
        streams(&home, &root, "main/typo", &[]),
        (Some(1), "".into(), "postui: unresolved: bsae\n".into())
    );
    std::fs::write(
        root.join("requests/main/tagged.toml"),
        "name = \"Tagged\"\nmethod = \"GET\"\nurl = \"{{base}}/ok\"\n\n\
         [headers]\nX-Tag = \"{{tag}}\"\n\n[variables]\ntag = \"{{region}}\"\n",
    )
    .unwrap();
    assert_eq!(
        streams(&home, &root, "main/tagged", &[]),
        (
            Some(0),
            "{\"id\":42}\n".into(),
            "postui: warning: the X-Tag header: tag's value holds {{region}}, which is sent \
             as written\n"
                .into()
        )
    );
}

/// D3: `--view` filters with what the request's response pane applies, the
/// current view's filter, else the scratch; `--view=NAME` with a saved view.
#[test]
fn run_view_uses_the_requests_own_filter() {
    let (home, _server, root) = served();
    let put = |name: &str, extra: &str| {
        std::fs::write(
            root.join(format!("requests/main/{name}.toml")),
            format!("name = \"{name}\"\nmethod = \"GET\"\nurl = \"{{{{base}}}}/ok\"\n{extra}"),
        )
        .unwrap();
    };
    put(
        "viewed",
        "jq_current = \"Id\"\njq_enabled = false\n\n\
         [[jq_view]]\nname = \"Id\"\nfilter = \".id\"\n\n\
         [[jq_view]]\nname = \"Keys\"\nfilter = \"keys\"\n",
    );
    put("scratch", "jq = \".id + 1\"\n");

    assert_eq!(streams(&home, &root, "main/viewed", &["--view"]).1, "42\n");
    assert_eq!(streams(&home, &root, "main/viewed", &["--view=keys"]).1, "[\"id\"]\n");
    assert_eq!(streams(&home, &root, "main/scratch", &["--view"]).1, "43\n");

    let (code, out, err) = streams(&home, &root, "main/viewed", &["--view=nope"]);
    assert_eq!((code, out.as_str()), (Some(2), ""));
    assert!(
        err.ends_with(
            "viewed.toml: request main/viewed has no view \"nope\"; its views are Id, Keys\n"
        ),
        "{err}"
    );
    let (code, _, err) = streams(&home, &root, "main/ok", &["--view"]);
    assert_eq!(code, Some(2));
    assert!(err.ends_with("request main/ok has no jq filter; it has no saved views\n"), "{err}");
}

/// D4: `--var` beats every other source, the request's own `[variables]`
/// included; a name nothing declares or holds is refused.
#[test]
fn run_var_gives_a_value_for_this_run() {
    let (home, _server, root) = served();
    std::fs::write(
        root.join("requests/main/local.toml"),
        "name = \"Local\"\nmethod = \"GET\"\nurl = \"{{base}}/r/{{region}}/{{here}}\"\n\n\
         [variables]\nregion = \"local\"\nhere = \"h\"\n",
    )
    .unwrap();
    let url = |extra: &[&str]| {
        let mut args = vec!["--var", "base=http://x.test"];
        args.extend_from_slice(extra);
        json(&dry(&home, &root, "main/local", &args))["url"].as_str().unwrap().to_string()
    };
    assert_eq!(url(&[]), "http://x.test/r/local/h");
    assert_eq!(url(&["--var", "region=cli", "--var", "here=there"]), "http://x.test/r/cli/there");

    // A selector's field, over the selection.
    let out = send(&home, &root, "main/echo", &["--var", "user_id=99"]);
    assert_eq!(json(&out)["response"]["body_json"]["u"], "99");

    let (code, _, err) = streams(&home, &root, "main/ok", &["--var", "regoin=x"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        err,
        "postui: --var regoin: variables.toml doesn't declare it, and the request's \
         [variables] don't have it\n"
    );
}

/// D4: a secret given with `--var` is masked like the secret it stands for.
#[test]
fn run_var_for_a_secret_is_masked() {
    let (home, _server, root) = served();
    let other = "another-s3cret";
    let out = send(&home, &root, "main/echo", &["--var", &format!("token={other}")]);
    assert_ok(&out, "run --var token");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(!printed.contains(other), "{printed}");
    assert_eq!(json(&out)["response"]["body_json"]["t"], MASK);
}

/// D4: `suite --var` reaches every request, and is checked before any is
/// sent.
#[test]
fn suite_var_reaches_every_request() {
    let home = Home::new();
    let server = Server::start();
    let root = write_suite_project(
        &home,
        "http://127.0.0.1:1",
        &[("main/a", &get("/ok")), ("admin/b", &get("/ok"))],
    );
    let out =
        home.run_json(&["suite", "-p", s(&root), "--var", &format!("base={}", server.base())]);
    assert_ok(&out, "suite --var");
    assert_eq!(server.seen(), ["GET /ok", "GET /ok"]);

    let out = home.run(&["suite", "-p", s(&root), "--var", "nope=1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no request's [variables] has it"));
    assert_eq!(server.seen().len(), 2, "nothing more was sent");
}

#[test]
fn run_dry_run_sends_nothing() {
    let (home, server, root) = served();
    let out = dry(&home, &root, "main/ok", &[]);
    assert_ok(&out, "run --dry-run");
    assert!(server.seen().is_empty());
}

/// A project with exactly the given request files (`space/name`, text), three
/// spaces and the `dev` environment pointing `base` at `base`.
fn write_suite_project(home: &Home, base: &str, requests: &[(&str, &str)]) -> PathBuf {
    let root = home.path().join("suite");
    let put = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    put(
        "project.toml",
        "name = \"Suite\"\nspaces = [\"main\", \"admin\", \"echoes\"]\n\n\
         [environment.dev]\nname = \"Dev\"\n",
    );
    put("variables.toml", "[base]\n\n[token]\nsecret = true\n");
    put("environments/dev.toml", &format!("base = \"{base}\"\n"));
    put(".postui/secrets.toml", &format!("[dev]\ntoken = \"{SECRET}\"\n"));
    for (name, text) in requests {
        put(&format!("requests/{name}.toml"), text);
    }
    root
}

fn get(path: &str) -> String {
    format!("method = \"GET\"\nurl = \"{{{{base}}}}{path}\"\n")
}

/// main: a-ok, b-missing, c-unresolved, d-broken, e-ok; admin: x.
fn suite_project(home: &Home, base: &str) -> PathBuf {
    write_suite_project(
        home,
        base,
        &[
            ("main/a-ok", &get("/ok")),
            ("main/b-missing", &get("/missing")),
            ("main/c-unresolved", "method = \"GET\"\nurl = \"{{bsae}}/x\"\n"),
            ("main/d-broken", "nope = ["),
            ("main/e-ok", &get("/ok")),
            ("admin/x", &get("/ok")),
        ],
    )
}

#[test]
fn suite_runs_in_sidebar_order_and_keeps_going() {
    let home = Home::new();
    let server = Server::start();
    let root = suite_project(&home, &server.base());

    let listed = json(&home.run_json(&["requests", "-p", s(&root)]));
    let order: Vec<&str> = listed["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["slug"].as_str().unwrap())
        .collect();

    let out = home.run_json(&["suite", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert_eq!(v["environment"], "dev");
    assert!(v["space"].is_null() && v["output_file"].is_null());
    let results = v["results"].as_array().unwrap();
    let slugs: Vec<&str> = results.iter().map(|r| r["request"].as_str().unwrap()).collect();
    assert_eq!(slugs, order);
    assert_eq!(slugs.len(), 6);
    assert_eq!(server.seen(), ["GET /ok", "GET /missing", "GET /ok", "GET /ok"]);

    let by = |slug: &str| results.iter().find(|r| r["request"] == slug).unwrap();
    assert_eq!(by("main/a-ok")["status"], 200);
    assert_eq!(by("main/a-ok")["size"], 9);
    assert!(by("main/a-ok")["elapsed_ms"].is_u64());
    assert_eq!(by("main/b-missing")["status"], 404);
    let unresolved = by("main/c-unresolved");
    assert!(unresolved["error"].as_str().unwrap().contains("bsae"), "{unresolved}");
    for key in ["status", "elapsed_ms", "size"] {
        assert!(unresolved[key].is_null(), "{key}");
    }
    // Only the variables failed: the request keeps its method and URL.
    assert_eq!(unresolved["method"], "GET");
    assert!(unresolved["url"].as_str().unwrap().contains("{{bsae}}"), "{unresolved}");
    let broken = by("main/d-broken");
    assert!(broken["method"].is_null() && broken["url"].is_null() && broken["status"].is_null());
    assert!(broken["error"].as_str().unwrap().contains("d-broken.toml"), "{broken}");
    assert_eq!(v["totals"]["requests"], 6);
    assert_eq!(v["totals"]["passed"], 3);
    assert_eq!(v["totals"]["failed"], 3);
    let sum: u64 = results.iter().filter_map(|r| r["elapsed_ms"].as_u64()).sum();
    assert_eq!(v["totals"]["elapsed_ms"], sum);
}

#[test]
fn suite_one_space() {
    let home = Home::new();
    let server = Server::start();
    let root = suite_project(&home, &server.base());

    let out = home.run_json(&["suite", "--space", "admin", "-p", s(&root)]);
    assert_ok(&out, "suite --space admin");
    let v = json(&out);
    assert_eq!(v["space"], "admin");
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    assert_eq!(server.seen(), ["GET /ok"]);

    let out = home.run_json(&["suite", "--space", "nope", "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(2));
    assert!(json(&out)["error"].as_str().unwrap().contains("nope"));
}

#[test]
fn suite_output_file_matches_stdout() {
    let home = Home::new();
    let server = Server::start();
    let root = suite_project(&home, &server.base());
    let file = home.path().join("r.json");

    let out = home.run_json(&["suite", "--space", "admin", "-o", s(&file), "-p", s(&root)]);
    assert_ok(&out, "suite -o");
    assert_eq!(std::fs::read(&file).unwrap(), out.stdout);
    assert_eq!(json(&out)["output_file"], s(&file));

    let out = home.run(&["suite", "--space", "admin", "-o", s(&file), "-p", s(&root)]);
    assert_ok(&out, "suite -o, text");
    assert_eq!(std::fs::read(&file).unwrap(), out.stdout);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("admin/x") && text.contains("200") && text.contains("1 passed"),
        "{text}"
    );
    // R19: the text says where the report went, as the JSON's output_file.
    assert!(text.contains(&format!("\nwritten to {}\n", s(&file))), "{text}");

    // A file where a directory must be.
    let bad = file.join("r.json");
    let out = home.run_json(&["suite", "--space", "admin", "-o", s(&bad), "-p", s(&root)]);
    // The requests were sent: one document, the report, with a warning.
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    assert!(v["output_file"].is_null());
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w.as_str().unwrap().starts_with("could not write")
            && w.as_str().unwrap().contains("r.json")),
        "{v}"
    );
    assert!(v["error"].is_null());

    // The write warning is masked like the rest: a path holding a secret.
    let secret_path = file.join(SECRET).join("r.json");
    let out = home.run_json(&["suite", "--space", "admin", "-o", s(&secret_path), "-p", s(&root)]);
    assert_eq!(out.status.code(), Some(1));
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(!printed.contains(SECRET) && printed.contains(MASK), "{printed}");
}

/// R19, R21: a row a request with its method, URL and size, and an error
/// whole under its row; a broken file's error names it first, absolute.
#[test]
fn suite_text_puts_each_error_under_its_row() {
    let home = Home::new();
    let server = Server::start();
    let root = suite_project(&home, &server.base());

    let v = json(&home.run_json(&["suite", "-p", s(&root)]));
    let broken =
        v["results"].as_array().unwrap().iter().find(|r| r["request"] == "main/d-broken").unwrap()
            ["error"]
            .as_str()
            .unwrap()
            .to_string();
    let file = s(&root.join("requests/main/d-broken.toml")).to_string();
    assert!(broken.starts_with(&format!("{file}: ")), "{broken}");
    assert!(broken.lines().nth(1).is_some(), "a multi-line error: {broken}");

    let out = home.run(&["suite", "-p", s(&root)]);
    let text = String::from_utf8_lossy(&out.stdout);
    let under: String =
        broken.lines().map(|l| format!("    {l}").trim_end().to_string() + "\n").collect();
    assert!(text.contains(&format!("main/d-broken\n{under}")), "{text}");
    let base = server.base();
    assert!(text.contains(&format!("GET  main/a-ok          {base}/ok       9 bytes\n")), "{text}");
    assert!(text.contains("main/c-unresolved  {{bsae}}/x\n    unresolved: bsae\n"), "{text}");
}

#[test]
fn suite_masks_a_secret_written_in_a_refused_request() {
    let home = Home::new();
    let leaky = format!("method = \"GET\"\nurl = \"http://h/{SECRET}/{{{{bsae}}}}\"\n");
    let root = write_suite_project(&home, "http://h", &[("main/leaky", &leaky)]);
    for extra in [&[][..], &["--json"][..]] {
        let mut args = vec!["suite", "-p", s(&root)];
        args.extend_from_slice(extra);
        let out = home.run(&args);
        assert_eq!(out.status.code(), Some(1));
        let printed = String::from_utf8_lossy(&out.stdout);
        assert!(!printed.contains(SECRET), "{printed}");
    }
    let v = json(&home.run_json(&["suite", "-p", s(&root)]));
    let url = v["results"][0]["url"].as_str().unwrap();
    assert!(url.contains(MASK) && url.contains("{{bsae}}"), "{url}");
}

#[test]
fn suite_lists_each_requests_prepare_warnings() {
    let home = Home::new();
    let server = Server::start();
    let warned = "method = \"GET\"\nurl = \"{{base}}/ok?a=1\"\n\n[params]\na = \"2\"\n";
    let root = write_suite_project(
        &home,
        &server.base(),
        &[("main/a-ok", &get("/ok")), ("main/w", warned)],
    );
    let out = home.run_json(&["suite", "-p", s(&root)]);
    assert_ok(&out, "suite");
    let warnings = json(&out)["warnings"].clone();
    let warnings = warnings.as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let w = warnings[0].as_str().unwrap();
    assert!(w.starts_with("main/w: ") && w.contains("`a`"), "{w}");
}

#[test]
fn suite_masks_and_writes_nothing() {
    let home = Home::new();
    let server = Server::start();
    let echo = "method = \"POST\"\nurl = \"{{base}}/echo?key={{token}}\"\n\
                substitute_body = true\n\n[headers]\n\
                Authorization = \"Bearer {{token}}\"\n\n[body]\ntype = \"json\"\n\
                text = '{\"t\": \"{{token}}\"}'\n";
    let root = write_suite_project(
        &home,
        &server.base(),
        &[("main/a-ok", &get("/ok")), ("echoes/echo", echo)],
    );
    let before = tree(&root);

    for extra in [&[][..], &["--json"][..]] {
        let mut args = vec!["suite", "-p", s(&root)];
        args.extend_from_slice(extra);
        let out = home.run(&args);
        assert_ok(&out, "suite");
        let printed = String::from_utf8_lossy(&out.stdout);
        assert!(!printed.contains(SECRET), "{printed}");
        assert!(printed.contains("echoes/echo"));
    }
    let v = json(&home.run_json(&["suite", "-p", s(&root)]));
    assert!(v["results"][1]["url"].as_str().unwrap().contains(MASK));
    assert_eq!(server.seen().len(), 6);
    assert_eq!(tree(&root), before);
}
