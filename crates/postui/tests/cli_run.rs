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
    root
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
