# Stage 2: HTTP Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the stage-1 shell into a usable daily HTTP client: request TOML files in a default project, a real editor (method/URL/params/headers/JSON body), async send via reqwest, and a JSON response viewer.

**Architecture:** All request modeling, storage, and request preparation goes into `postui-core` (no tokio/reqwest/terminal deps). The TUI crate owns the reqwest send in a spawned tokio task reporting back over an mpsc channel of `Action`s. Stage-1's component pattern is extended with per-pane key routing, an explicit modal-close protocol, event-driven redraw, and scroll routing — the parked stage-1 fixes.

**Tech Stack:** Rust 2024 workspace; ratatui 0.30 + crossterm (via `ratatui::crossterm` re-export ONLY — never a direct crossterm dep); tokio; reqwest 0.13 (rustls); edtui 0.11 in modeless emacs mode (body editor); toml 1 + toml_edit (save formatting); indexmap 2; serde_json 1 (`preserve_order` + `arbitrary_precision`); thiserror 2; wiremock + tempfile (dev).

**Spec:** `docs/superpowers/specs/2026-08-15-stage2-http-core-design.md` (and parent `2026-08-15-postui-design.md`). The spec is binding; if implementation contradicts it, STOP and ask the user.

## Global Constraints

- Workspace edition 2024; `cargo clippy --workspace --all-targets -- -D warnings` must stay clean after every task.
- Run tests with `PATH="$HOME/.cargo/bin:$PATH"` prefix (cargo is not on the default PATH — see dev-environment memory).
- `postui-core` must never depend on tokio, reqwest, ratatui, or crossterm.
- All file writes atomic: write temp file in the same directory, then rename.
- `ctrl+c` must always quit — reserved; overrides cannot rebind or shadow it.
- Request body text is NEVER modified except by explicit Format/Minify actions. Save writes it verbatim; invalid JSON must save fine.
- Serialized request files use the table form: plain string = enabled entry, inline table `{ value = "…", enabled = false }` = disabled entry. `[params]`/`[headers]` key order preserved exactly.
- Fixed constants: default project dir = platform config dir + `postui/default` (e.g. `~/.config/postui/default`); HTTP timeout 30 s; large-body guard 2 MiB (`2 * 1024 * 1024` bytes).
- Commit after every task (no Co-Authored-By, no Claude-Session trailer).
- Work happens on branch `stage2-http-core` (create via superpowers:using-git-worktrees at execution start).

**Interaction model decided for this stage** (referenced by several tasks):

Key routing order (no modal open): (1) a *modified* combo bound to Quit (e.g. ctrl+c) always quits; (2) combos with CTRL or ALT go to the global keymap first, then fall through to the focused component; (3) plain keys go to the focused component first, then fall back to the global keymap. With a modal open: modified-quit first, then the modal gets everything. ALT-modified keys are never delegated to edtui.

Default new bindings (all rebindable; names in Task 5/12): send = `ctrl+r` AND `ctrl+enter` (ctrl+enter is indistinguishable from enter in many terminals — ctrl+r is the reliable default, ctrl+enter works where the terminal supports it); save = `ctrl+s`; tabs = `alt+1/2/3` + `alt+left`/`alt+right` cycle; cycle method = `alt+m`; focus URL row = `alt+u`; format body = `alt+f`; minify body = `alt+g`; open body in `$EDITOR` = `ctrl+e`.

---

### Task 1: Core request model (`postui-core::model`)

**Files:**
- Modify: `Cargo.toml` (workspace deps), `crates/postui-core/Cargo.toml`, `crates/postui-core/src/lib.rs`
- Create: `crates/postui-core/src/model.rs` (tests inline in module)

**Interfaces:**
- Consumes: nothing (first task).
- Produces: `postui_core::model::{Method, Entry, Body, HttpRequest}` and `HttpRequest::{from_toml_str(&str) -> Result<Self, toml::de::Error>, to_toml_string(&self) -> String}`. `Method` has `as_str() -> &'static str`, `cycle() -> Method`, `ALL: [Method; 7]`, `Default = Get`. `Entry { value: String, enabled: bool }`. `Body::Json { text: String }`; `HttpRequest { method: Method, url: String, params: IndexMap<String, Entry>, headers: IndexMap<String, Entry>, body: Option<Body> }` (all types `Debug + Clone + PartialEq`).

- [ ] **Step 1: Add dependencies**

In root `Cargo.toml` `[workspace.dependencies]`: change `toml = "0.8"` to `toml = "1"`, add:

```toml
indexmap = { version = "2", features = ["serde"] }
thiserror = "2"
serde_json = { version = "1", features = ["preserve_order", "arbitrary_precision"] }
toml_edit = "0.23"
```

In `crates/postui-core/Cargo.toml` add under `[dependencies]`: `indexmap.workspace = true`, `thiserror.workspace = true`, `serde_json.workspace = true`, `toml.workspace = true`, `toml_edit.workspace = true`. Run `cargo build --workspace` — if `toml_edit = "0.23"` fails to resolve or conflicts with `toml = "1"`, run `cargo add toml_edit -p postui-core` to get the version matching the toml 1.x family, and update the workspace pin to what it picks. Expected: builds green (toml 1 is serde-compatible with the 0.8 usage in `keys.rs`; if `keys.rs` fails to compile, fix the minimal API difference there and note it in the commit).

- [ ] **Step 2: Write failing round-trip tests**

Create `crates/postui-core/src/model.rs` with a `#[cfg(test)] mod tests` containing (module body itself just `// impl below` for now — declare `pub mod model;` in `lib.rs`):

```rust
use super::*;
use indexmap::IndexMap;

fn sample() -> HttpRequest {
    let mut params = IndexMap::new();
    params.insert("page".into(), Entry { value: "2".into(), enabled: true });
    params.insert("verbose".into(), Entry { value: "1".into(), enabled: false });
    let mut headers = IndexMap::new();
    headers.insert("Authorization".into(), Entry { value: "Bearer abc123".into(), enabled: true });
    HttpRequest {
        method: Method::Post,
        url: "https://api.example.com/users".into(),
        params,
        headers,
        body: Some(Body::Json { text: "{ \"broken\": ".into() }), // invalid JSON must round-trip
    }
}

#[test]
fn round_trips_preserving_content_and_order() {
    let req = sample();
    let toml_str = req.to_toml_string();
    let back = HttpRequest::from_toml_str(&toml_str).unwrap();
    assert_eq!(back, req);
    let keys: Vec<&String> = back.params.keys().collect();
    assert_eq!(keys, ["page", "verbose"], "insertion order preserved");
}

#[test]
fn enabled_entries_serialize_as_plain_strings_disabled_as_inline_tables() {
    let out = sample().to_toml_string();
    assert!(out.contains(r#"page = "2""#), "enabled entry is a plain string:\n{out}");
    assert!(out.contains("verbose = {"), "disabled entry is an inline table:\n{out}");
    assert!(!out.contains("[params.verbose]"), "no sub-table sections (they break ordering):\n{out}");
}

#[test]
fn parses_string_and_table_entry_forms() {
    let req = HttpRequest::from_toml_str(r#"
        method = "GET"
        url = "https://x.test"
        [headers]
        A = "1"
        B = { value = "2", enabled = false }
        C = { value = "3", enabled = true }
    "#).unwrap();
    assert_eq!(req.headers["A"], Entry { value: "1".into(), enabled: true });
    assert_eq!(req.headers["B"], Entry { value: "2".into(), enabled: false });
    assert_eq!(req.headers["C"], Entry { value: "3".into(), enabled: true });
}

#[test]
fn missing_method_defaults_to_get_missing_sections_default_empty() {
    let req = HttpRequest::from_toml_str(r#"url = "https://x.test""#).unwrap();
    assert_eq!(req.method, Method::Get);
    assert!(req.params.is_empty() && req.headers.is_empty() && req.body.is_none());
}

#[test]
fn rejects_unknown_keys_arrays_and_bad_entries() {
    assert!(HttpRequest::from_toml_str(r#"url = "u"
        bogus = 1"#).is_err(), "unknown top-level key");
    let arr = HttpRequest::from_toml_str(r#"url = "u"
        [params]
        id = ["1", "2"]"#);
    let msg = arr.unwrap_err().to_string();
    assert!(msg.contains("reserved"), "array rejection names the reservation: {msg}");
    assert!(HttpRequest::from_toml_str(r#"url = "u"
        [headers]
        A = { value = "1", typo = true }"#).is_err(), "unknown entry field");
    assert!(HttpRequest::from_toml_str(r#"url = "u"
        [body]
        type = "yaml"
        text = "x""#).is_err(), "unknown body type");
}

#[test]
fn method_cycles_through_all_and_wraps() {
    let mut m = Method::Get;
    for _ in 0..Method::ALL.len() { m = m.cycle(); }
    assert_eq!(m, Method::Get);
    assert_eq!(Method::Delete.as_str(), "DELETE");
}
```

- [ ] **Step 3: Run to verify failure** — `cargo test -p postui-core` → compile errors (types undefined).

- [ ] **Step 4: Implement**

In `model.rs` above the tests:

```rust
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get, Post, Put, Patch, Delete, Head, Options,
}

impl Method {
    pub const ALL: [Method; 7] = [Method::Get, Method::Post, Method::Put, Method::Patch, Method::Delete, Method::Head, Method::Options];
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET", Method::Post => "POST", Method::Put => "PUT",
            Method::Patch => "PATCH", Method::Delete => "DELETE",
            Method::Head => "HEAD", Method::Options => "OPTIONS",
        }
    }
    pub fn cycle(self) -> Method {
        let i = Method::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Method::ALL[(i + 1) % Method::ALL.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry { pub value: String, pub enabled: bool }
```

`Entry` gets hand-written serde. Deserialize via `deserialize_any` with a visitor: `visit_str` → enabled entry; `visit_map` → accept exactly `value` (required) and `enabled` (default true), `unknown_field` error otherwise; `visit_seq` → `Error::custom("array values are reserved for a future version; use a single string or { value = \"…\", enabled = false }")`. Serialize: enabled → `serialize_str(&self.value)`; disabled → `serialize_struct("Entry", 2)` with both fields. (The struct-serialize path is only exercised through `to_toml_string`, which controls inline-vs-section formatting itself — see below.)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Body {
    Json { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    #[serde(default)]
    pub method: Method,
    pub url: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub params: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Body>,
}

impl HttpRequest {
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Hand-built with toml_edit for exact formatting control: enabled entries
    /// as plain strings, disabled as *inline* tables (sub-table sections would
    /// force reordering), body text verbatim.
    pub fn to_toml_string(&self) -> String {
        use toml_edit::{DocumentMut, Item, Table, Value, value};
        let mut doc = DocumentMut::new();
        doc["method"] = value(self.method.as_str());
        doc["url"] = value(&self.url);
        let kv_table = |map: &IndexMap<String, Entry>| {
            let mut t = Table::new();
            for (k, e) in map {
                if e.enabled {
                    t[k] = value(&e.value);
                } else {
                    let mut inline = toml_edit::InlineTable::new();
                    inline.insert("value", Value::from(e.value.as_str()));
                    inline.insert("enabled", Value::from(false));
                    t[k] = Item::Value(Value::InlineTable(inline));
                }
            }
            Item::Table(t)
        };
        if !self.params.is_empty() { doc["params"] = kv_table(&self.params); }
        if !self.headers.is_empty() { doc["headers"] = kv_table(&self.headers); }
        if let Some(Body::Json { text }) = &self.body {
            let mut t = Table::new();
            t["type"] = value("json");
            t["text"] = value(text.as_str());
            doc["body"] = Item::Table(t);
        }
        doc.to_string()
    }
}
```

If toml_edit's exact API differs (e.g. `Table` indexing), adapt to the current toml_edit 0.23 API — the tests define the required output shape.

- [ ] **Step 5: Run tests** — `cargo test -p postui-core` → all pass. Then `cargo clippy --workspace --all-targets -- -D warnings`.

- [ ] **Step 6: Commit** — `git add -A && git commit -m "Add core request model with order-preserving TOML round-trip"`

---

### Task 2: Core storage (`postui-core::storage`)

**Files:**
- Create: `crates/postui-core/src/storage.rs`
- Modify: `crates/postui-core/src/lib.rs`, `crates/postui-core/Cargo.toml` (add `[dev-dependencies] tempfile = "3"`; add `directories = "6"` to workspace deps and core deps, and REMOVE `directories` from `crates/postui/Cargo.toml` direct deps if the TUI crate stops needing it directly — it still uses it in `keys.rs`, so keep both pointing at the workspace entry)

**Interfaces:**
- Consumes: Task 1 `HttpRequest`.
- Produces:
  - `storage::default_project_dir() -> Option<PathBuf>` (config dir + `default`, e.g. `~/.config/postui/default`)
  - `storage::ensure_project(root: &Path) -> std::io::Result<()>` (creates `root/requests/`)
  - `storage::validate_slug(slug: &str) -> Result<(), StorageError>`
  - `storage::list_requests(root: &Path) -> Vec<RequestListing>` where `RequestListing { slug: String, broken: Option<String> }`, sorted by slug; slug is the path relative to `requests/` without `.toml` (e.g. `auth/login`)
  - `storage::load_request(root: &Path, slug: &str) -> Result<HttpRequest, StorageError>`
  - `storage::save_request(root: &Path, slug: &str, req: &HttpRequest) -> Result<(), StorageError>` (atomic, creates parent dirs)
  - `storage::rename_request(root, from: &str, to: &str) -> Result<(), StorageError>`
  - `storage::delete_request(root, slug: &str) -> Result<(), StorageError>`
  - `enum StorageError` (thiserror): `Io(#[from] std::io::Error)`, `Parse(String)` (the toml error's Display — includes line/col), `InvalidSlug(String)`, `NotFound(String)`, `AlreadyExists(String)`

- [ ] **Step 1: Write failing tests** (in `storage.rs` `#[cfg(test)]`, using `tempfile::tempdir()`):

```rust
use super::*;
use crate::model::*;

fn req() -> HttpRequest {
    HttpRequest { method: Method::Get, url: "https://x.test".into(),
        params: Default::default(), headers: Default::default(), body: None }
}

#[test]
fn save_load_list_roundtrip_with_subdirectories() {
    let dir = tempfile::tempdir().unwrap();
    ensure_project(dir.path()).unwrap();
    save_request(dir.path(), "auth/login", &req()).unwrap();
    save_request(dir.path(), "get-user", &req()).unwrap();
    let listing = list_requests(dir.path());
    let slugs: Vec<&str> = listing.iter().map(|l| l.slug.as_str()).collect();
    assert_eq!(slugs, ["auth/login", "get-user"], "sorted, subdir path as slug");
    assert!(listing.iter().all(|l| l.broken.is_none()));
    assert_eq!(load_request(dir.path(), "auth/login").unwrap(), req());
}

#[test]
fn broken_file_is_listed_with_error_and_load_reports_line() {
    let dir = tempfile::tempdir().unwrap();
    ensure_project(dir.path()).unwrap();
    std::fs::write(dir.path().join("requests/bad.toml"), "url = \"x\"\nurl = \"dup\"\n").unwrap();
    let listing = list_requests(dir.path());
    assert_eq!(listing[0].slug, "bad");
    assert!(listing[0].broken.is_some());
    let err = load_request(dir.path(), "bad").unwrap_err().to_string();
    assert!(err.contains('2') || err.to_lowercase().contains("duplicate"),
        "error should locate/describe the duplicate key: {err}");
}

#[test]
fn slug_validation_rejects_traversal_and_bad_chars() {
    for bad in ["", "../etc", "a//b", "/abs", "trailing/", "Has Space", "UPPER", "dot.dot"] {
        assert!(validate_slug(bad).is_err(), "{bad:?} should be invalid");
    }
    for good in ["login", "auth/login", "a-b_c/d0"] {
        assert!(validate_slug(good).is_ok(), "{good:?} should be valid");
    }
}

#[test]
fn save_is_atomic_no_temp_left_and_rename_delete_work() {
    let dir = tempfile::tempdir().unwrap();
    ensure_project(dir.path()).unwrap();
    save_request(dir.path(), "a", &req()).unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("requests")).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_none_or(|x| x != "toml"))
        .collect();
    assert!(leftovers.is_empty(), "no temp files left behind");
    rename_request(dir.path(), "a", "sub/b").unwrap();
    assert!(load_request(dir.path(), "a").is_err());
    assert_eq!(load_request(dir.path(), "sub/b").unwrap(), req());
    assert!(rename_request(dir.path(), "sub/b", "sub/b").is_err(), "rename onto itself");
    delete_request(dir.path(), "sub/b").unwrap();
    assert!(list_requests(dir.path()).is_empty());
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p postui-core storage` → compile errors.

- [ ] **Step 3: Implement**

Key points (write the obvious code for the rest):

```rust
pub fn validate_slug(slug: &str) -> Result<(), StorageError> {
    let ok = !slug.is_empty()
        && !slug.starts_with('/') && !slug.ends_with('/')
        && slug.split('/').all(|seg| {
            !seg.is_empty() && seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        });
    if ok { Ok(()) } else { Err(StorageError::InvalidSlug(slug.to_string())) }
}

fn request_path(root: &Path, slug: &str) -> PathBuf {
    root.join("requests").join(format!("{slug}.toml"))
}

pub fn save_request(root: &Path, slug: &str, req: &HttpRequest) -> Result<(), StorageError> {
    validate_slug(slug)?;
    let path = request_path(root, slug);
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    use std::io::Write;
    tmp.write_all(req.to_toml_string().as_bytes())?;
    tmp.persist(&path).map_err(|e| StorageError::Io(e.error))?;
    Ok(())
}
```

`tempfile` therefore moves from dev-dependencies to real dependencies of postui-core (`tempfile.workspace = true`; add `tempfile = "3"` to workspace deps). `list_requests` walks `root/requests` recursively (plain `std::fs::read_dir` recursion — no walkdir dep), collects `.toml` files, sorts slugs, and attempts `HttpRequest::from_toml_str` on each to fill `broken`. `rename_request` validates both slugs, errors `AlreadyExists` if target exists or equals source, `create_dir_all` for the target parent, `fs::rename`. `default_project_dir()` uses `directories::ProjectDirs::from("", "", postui_core::APP_NAME)` → `config_dir().join("default")` (matching the existing `keys.rs` lookup convention).

- [ ] **Step 4: Run tests pass** — `cargo test -p postui-core` and clippy.

- [ ] **Step 5: Commit** — `git commit -am "Add request storage: atomic saves, slug validation, recursive listing"`

---

### Task 3: Request preparation (`postui-core::prepare`)

**Files:**
- Create: `crates/postui-core/src/prepare.rs`
- Modify: `crates/postui-core/src/lib.rs`, `crates/postui-core/Cargo.toml` (+ workspace): add `form_urlencoded = "1"` (maintained, servo/url family)

**Interfaces:**
- Consumes: Task 1 model types.
- Produces: `prepare::{PreparedRequest, PrepareWarning, prepare}`:
  - `PreparedRequest { method: Method, url: String, headers: Vec<(String, String)>, body: Option<String> }` (`Debug + Clone + PartialEq`)
  - `enum PrepareWarning { ParamOverridesUrl { key: String } }` with a `Display` impl ("query param `key` in [params] overrides the one in the URL")
  - `fn prepare(req: &HttpRequest) -> (PreparedRequest, Vec<PrepareWarning>)`

- [ ] **Step 1: Failing tests:**

```rust
use super::*;
use crate::model::*;
use indexmap::IndexMap;

fn base(url: &str) -> HttpRequest {
    HttpRequest { method: Method::Get, url: url.into(),
        params: IndexMap::new(), headers: IndexMap::new(), body: None }
}
fn on(v: &str) -> Entry { Entry { value: v.into(), enabled: true } }
fn off(v: &str) -> Entry { Entry { value: v.into(), enabled: false } }

#[test]
fn merges_enabled_params_into_query_encoding_values() {
    let mut r = base("https://x.test/path");
    r.params.insert("q".into(), on("a b&c"));
    r.params.insert("skip".into(), off("nope"));
    let (p, warns) = prepare(&r);
    assert_eq!(p.url, "https://x.test/path?q=a+b%26c");
    assert!(warns.is_empty());
}

#[test]
fn params_table_wins_over_url_query_with_warning() {
    let mut r = base("https://x.test/p?id=1&keep=y");
    r.params.insert("id".into(), on("2"));
    let (p, warns) = prepare(&r);
    assert_eq!(p.url, "https://x.test/p?id=2&keep=y");
    assert_eq!(warns, vec![PrepareWarning::ParamOverridesUrl { key: "id".into() }]);
}

#[test]
fn url_literal_duplicates_are_kept_verbatim() {
    let (p, warns) = prepare(&base("https://x.test/p?id=1&id=2"));
    assert_eq!(p.url, "https://x.test/p?id=1&id=2");
    assert!(warns.is_empty(), "user-typed duplicates pass through untouched");
}

#[test]
fn url_without_params_table_is_untouched() {
    let (p, _) = prepare(&base("https://x.test/p?a=%20weird&b"));
    assert_eq!(p.url, "https://x.test/p?a=%20weird&b", "no table entries: never rewrite the query");
}

#[test]
fn headers_filter_disabled_and_json_body_auto_adds_content_type() {
    let mut r = base("https://x.test");
    r.headers.insert("A".into(), on("1"));
    r.headers.insert("B".into(), off("2"));
    r.body = Some(Body::Json { text: "{}".into() });
    let (p, _) = prepare(&r);
    assert_eq!(p.headers, vec![("A".into(), "1".into()), ("Content-Type".into(), "application/json".into())]);
    assert_eq!(p.body.as_deref(), Some("{}"));
}

#[test]
fn explicit_content_type_wins_case_insensitively() {
    let mut r = base("https://x.test");
    r.headers.insert("content-TYPE".into(), on("application/vnd.x+json"));
    r.body = Some(Body::Json { text: "{}".into() });
    let (p, _) = prepare(&r);
    assert_eq!(p.headers.len(), 1);
    assert_eq!(p.headers[0].1, "application/vnd.x+json");
}
```

- [ ] **Step 2: Run to fail**, then **Step 3: Implement:**

Merge algorithm — important subtlety proven by the tests: if the params table is empty, return the URL byte-for-byte. Only when at least one enabled param exists do we parse and re-serialize the query:

```rust
pub fn prepare(req: &HttpRequest) -> (PreparedRequest, Vec<PrepareWarning>) {
    let mut warnings = Vec::new();
    let enabled: Vec<(&String, &Entry)> = req.params.iter().filter(|(_, e)| e.enabled).collect();
    let url = if enabled.is_empty() {
        req.url.clone()
    } else {
        let (base, query) = match req.url.split_once('?') {
            Some((b, q)) => (b, q),
            None => (req.url.as_str(), ""),
        };
        // (key, value, from_table) triples; URL pairs first, in order.
        let mut pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes()).into_owned().collect();
        for (k, e) in &enabled {
            let existing = pairs.iter().position(|(pk, _)| pk == *k);
            if let Some(i) = existing {
                warnings.push(PrepareWarning::ParamOverridesUrl { key: (*k).clone() });
                pairs.retain(|(pk, _)| pk != *k);
                pairs.insert(i.min(pairs.len()), ((*k).clone(), e.value.clone()));
            } else {
                pairs.push(((*k).clone(), e.value.clone()));
            }
        }
        let qs = form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        format!("{base}?{qs}")
    };
    let mut headers: Vec<(String, String)> = req.headers.iter()
        .filter(|(_, e)| e.enabled)
        .map(|(k, e)| (k.clone(), e.value.clone()))
        .collect();
    let body = req.body.as_ref().map(|Body::Json { text }| text.clone());
    if body.is_some() && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
        headers.push(("Content-Type".into(), "application/json".into()));
    }
    (PreparedRequest { method: req.method, url, headers, body }, warnings)
}
```

- [ ] **Step 4: Tests + clippy pass.** **Step 5: Commit** — `git commit -am "Add request preparation: query merge with last-wins warnings, content-type auto-add"`

---

### Task 4: JSON helpers (`postui-core::json`)

**Files:**
- Create: `crates/postui-core/src/json.rs`; modify `lib.rs`.

**Interfaces:**
- Produces: `json::validate(text: &str) -> Result<(), JsonError>` with `JsonError { line: usize, column: usize, message: String }` (`Display`: `"line {line}, column {column}: {message}"`); `json::format(text: &str) -> Result<String, JsonError>` (pretty, 2-space); `json::minify(text: &str) -> Result<String, JsonError>`.

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn validate_reports_position() {
    assert!(validate("{\"a\": 1}").is_ok());
    let e = validate("{\n  \"a\": oops\n}").unwrap_err();
    assert_eq!(e.line, 2);
    assert!(e.column > 0);
}

#[test]
fn format_pretty_prints_preserving_key_order_and_number_text() {
    let out = format("{\"z\":1,\"a\":{\"n\":1e3}}").unwrap();
    let z = out.find("\"z\"").unwrap();
    let a = out.find("\"a\"").unwrap();
    assert!(z < a, "preserve_order: keys must not be alphabetized");
    assert!(out.contains("1e3"), "arbitrary_precision: number text preserved verbatim");
    assert!(out.contains("\n"), "actually pretty");
}

#[test]
fn minify_round_trips() {
    let min = minify("{\n  \"a\": [ 1, 2 ]\n}").unwrap();
    assert_eq!(min, "{\"a\":[1,2]}");
    assert!(format("{oops").is_err() && minify("{oops").is_err());
}
```

- [ ] **Step 2: fail. Step 3: Implement** — `serde_json::from_str::<serde_json::Value>` for all three (`serde_json::Error` exposes `line()`/`column()`); format via `serde_json::to_string_pretty`, minify via `to_string`. **Step 4: pass + clippy. Step 5: Commit** — `git commit -am "Add JSON validate/format/minify helpers"`

---

### Task 5: Multi-combo keymap with reserved ctrl+c

**Files:**
- Modify: `crates/postui/src/keys.rs`

**Interfaces:**
- Consumes: existing `KeyCombo`, `Keymap`, `Action`.
- Produces: `Keymap::lookup` unchanged in signature. `apply_overrides` accepts `action = "combo"` or `action = ["combo", "combo"]`; an override REPLACES that action's combo list; binding any non-quit action to `ctrl+c` is an error; after overrides `ctrl+c → Quit` is unconditionally restored. `named_actions()` is the extension point later tasks add to. Also produces `Keymap::bind(&mut self, combo: KeyCombo, action: Action)` (used by defaults and overrides internally).

- [ ] **Step 1: Failing tests** (replace `toml_overrides_rebind_and_reject_unknown`, keep the rest):

```rust
#[test]
fn override_accepts_string_or_list_and_replaces_all_defaults() {
    let mut m = Keymap::default_bindings();
    m.apply_overrides(r#"quit = ["ctrl+q", "f10"]"#).unwrap();
    let get = |s: &str| m.lookup(&KeyCombo::parse(s).unwrap());
    assert_eq!(get("ctrl+q"), Some(Action::Quit));
    assert_eq!(get("q"), None, "default 'q' replaced by explicit list");
    m.apply_overrides(r#"open_palette = "ctrl+k""#).unwrap();
    assert_eq!(get("ctrl+k"), Some(Action::OpenPalette));
    assert_eq!(get("ctrl+p"), None);
}

#[test]
fn ctrl_c_is_always_quit_and_cannot_be_taken() {
    let mut m = Keymap::default_bindings();
    m.apply_overrides(r#"quit = "ctrl+q""#).unwrap();
    assert_eq!(m.lookup(&KeyCombo::parse("ctrl+c").unwrap()), Some(Action::Quit),
        "ctrl+c survives a quit rebind");
    assert!(m.apply_overrides(r#"open_palette = "ctrl+c""#).is_err(),
        "ctrl+c cannot be bound to another action");
}

#[test]
fn unknown_action_and_bad_combo_still_error() {
    let mut m = Keymap::default_bindings();
    assert!(m.apply_overrides(r#"unknown_action = "x""#).is_err());
    assert!(m.apply_overrides(r#"quit = "not+a+key""#).is_err());
    assert!(m.apply_overrides(r#"quit = ["q", "not+a+key"]"#).is_err());
}
```

Add `f10`-style function-key parsing to `KeyCombo::parse` (`"f1"`..`"f12"` → `KeyCode::F(n)`) — the test uses it, and it also covers a deferred stage-1 parse-edge gap. Also add parse-edge tests parked from stage 1: `KeyCombo::parse("ctrl+")`, `"q+"`, `"qq"` all `None`.

- [ ] **Step 2: fail. Step 3: Implement:**

```rust
fn parse_overrides(toml_str: &str) -> anyhow::Result<Vec<(String, Vec<String>)>> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OneOrMany { One(String), Many(Vec<String>) }
    let table: std::collections::HashMap<String, OneOrMany> = toml::from_str(toml_str)?;
    Ok(table.into_iter().map(|(k, v)| (k, match v {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })).collect())
}
```

`apply_overrides`: for each (name, combos): resolve action from `named_actions()` (error if unknown); parse every combo up front (error on any failure); if action != Quit and any combo == ctrl+c → `anyhow::bail!("ctrl+c is reserved for quit")`; `self.bindings.retain(|_, a| *a != action)`; insert each. At the end of `apply_overrides` (always): `self.bindings.insert(KeyCombo::parse("ctrl+c").unwrap(), Action::Quit)`.

- [ ] **Step 4: `cargo test -p postui` + clippy pass. Step 5: Commit** — `git commit -am "Support multi-combo keybindings; reserve ctrl+c for quit"`

---

### Task 6: Modal close protocol (`ModalResult`)

**Files:**
- Modify: `crates/postui/src/components/modal.rs`, `crates/postui/src/main.rs`

**Interfaces:**
- Produces: `pub struct ModalResult { pub actions: Vec<Action>, pub close: bool }` and `ModalStack::handle_key(&mut self, key: KeyEvent) -> Option<ModalResult>` (`None` = swallowed, modal stays). Palette Enter → `ModalResult { actions: vec![chosen], close: true }`; palette/message Esc → `{ actions: vec![], close: true }`. Later tasks add `Modal::Confirm`/`Modal::Prompt` returning multi-action results.

- [ ] **Step 1: Update tests in modal.rs** (adjust existing ones to the new type) and add:

```rust
#[test]
fn palette_enter_returns_action_and_closes() {
    let mut m = ModalStack::default();
    m.push(Modal::Palette(crate::components::palette::PaletteState::new()));
    for c in "quit".chars() { assert!(m.handle_key(key(KeyCode::Char(c))).is_none()); }
    let res = m.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(res.close);
    assert_eq!(res.actions, vec![Action::Quit]);
    // note: the STACK does not pop itself — the caller pops on close.
    assert!(!m.is_empty());
}

#[test]
fn message_modal_closes_without_action() {
    let mut m = ModalStack::default();
    m.push(Modal::Message { title: "t".into(), body: "b".into() });
    let res = m.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(res.close && res.actions.is_empty());
}
```

- [ ] **Step 2: fail. Step 3: Implement** — `PaletteState::handle_key` changes return type to `Option<ModalResult>`: Esc → close/no actions; Enter with a selection → close + action; Enter with empty results → `None`; everything else edits state and returns `None`. `Modal::Message` Esc/Enter → close. In `main.rs`, temporarily inline: on `Some(res)`, `if res.close { app.modals.pop(); }` then `for a in res.actions { app.update(a); }` (this event-loop block is replaced wholesale in Task 7). Update palette tests to the new return type. `Action::Close`'s `App::update` arm stays (Esc with no modal is still a no-op close).

- [ ] **Step 4: tests + clippy. Step 5: Commit** — `git commit -am "Make modal closing explicit via ModalResult"`

---

### Task 7: Central key routing (`App::action_for_key`) + modifier-aware palette

**Files:**
- Modify: `crates/postui/src/app.rs`, `crates/postui/src/main.rs`, `crates/postui/src/components/palette.rs`, `crates/postui/src/components/mod.rs`

**Interfaces:**
- Produces: `App::handle_key(&mut self, keymap: &Keymap, ev: KeyEvent) -> bool` (returns "state changed → redraw"; internally routes and applies actions). Routing order (documented in a comment, tested): (1) modified combo bound to Quit → quit, even with modals open; (2) modal stack; (3) CTRL/ALT combos → global keymap, falling through to component if unbound; (4) focused component `handle_key`; (5) global keymap. `Component::handle_key` (existing trait method) becomes the per-pane hook; placeholder panes return `None`.
- Consumes: Task 5 keymap, Task 6 `ModalResult`.

- [ ] **Step 1: Failing tests in app.rs:**

```rust
fn ctrl(c: char) -> KeyEvent { KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL) }
fn plain(c: char) -> KeyEvent { KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE) }

#[test]
fn ctrl_c_quits_even_with_modal_open() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    app.handle_key(&Keymap::default_bindings(), ctrl('c'));
    assert!(app.should_quit);
}

#[test]
fn plain_q_types_into_palette_instead_of_quitting() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    app.handle_key(&Keymap::default_bindings(), plain('q'));
    assert!(!app.should_quit);
    assert!(!app.modals.is_empty());
}

#[test]
fn ctrl_char_does_not_type_into_palette() {
    let mut app = App::new_for_test();
    app.update(Action::OpenPalette);
    app.handle_key(&Keymap::default_bindings(), ctrl('x')); // unbound ctrl combo
    // palette input must still be empty: filter list unchanged
    let crate::components::modal::Modal::Palette(p) = app.modals.top().unwrap() else { panic!() };
    assert_eq!(p.input(), "");
}

#[test]
fn plain_q_quits_when_no_modal_and_component_ignores_it() {
    let mut app = App::new_for_test();
    app.handle_key(&Keymap::default_bindings(), plain('q'));
    assert!(app.should_quit);
}
```

`App::new_for_test()` — added in this task: constructs `App` (Task 10 later extends it with a channel + temp project root; keep it compiling forward from here). `ModalStack` needs `pub fn top(&self) -> Option<&Modal>`.

- [ ] **Step 2: fail. Step 3: Implement:**

```rust
pub fn handle_key(&mut self, keymap: &Keymap, ev: KeyEvent) -> bool {
    let combo = KeyCombo::from_event(&ev);
    let global = keymap.lookup(&combo);
    let modified = ev.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    // 1. A modified quit combo is the escape hatch: it pre-empts everything.
    if modified && global == Some(Action::Quit) {
        return self.apply(Action::Quit);
    }
    // 2. Modals capture all remaining input.
    if !self.modals.is_empty() {
        let Some(res) = self.modals.handle_key(ev) else { return true }; // typed into modal
        if res.close { self.modals.pop(); }
        let mut changed = true;
        for a in res.actions { changed |= self.apply(a); }
        return changed;
    }
    // 3. Modified combos prefer the global keymap (app shortcuts beat editors).
    if modified && let Some(a) = global {
        return self.apply(a);
    }
    // 4. The focused component gets plain keys (and unbound modified ones) next.
    if let Some(a) = self.focused_component_key(ev) {
        return self.apply(a);
    }
    // 5. Global fallback for plain keys the component ignored.
    if let Some(a) = global { return self.apply(a); }
    false
}

fn focused_component_key(&mut self, ev: KeyEvent) -> Option<Action> {
    match self.focus {
        PaneId::Sidebar => self.sidebar.handle_key(ev),
        PaneId::Editor => self.editor.handle_key(ev),
        PaneId::Response => self.response.handle_key(ev),
    }
}
```

`apply` is the rename of `update` in Task 8; in THIS task keep `self.update(a); true` and let Task 8 introduce the bool. Palette `handle_key` `KeyCode::Char` arm gains the guard `if key.modifiers.difference(KeyModifiers::SHIFT).is_empty()` (modified chars are swallowed without inserting). `main.rs` key handling collapses to `app.handle_key(&keymap, ev);`.

- [ ] **Step 4: tests + clippy (includes updated stage-1 tests). Step 5: Commit** — `git commit -am "Centralize key routing: quit escape hatch, modal capture, component-first plain keys"`

---

### Task 8: Event-driven redraw + background action channel

**Files:**
- Modify: `crates/postui/src/app.rs`, `crates/postui/src/main.rs`, `crates/postui/src/components/toast.rs`

**Interfaces:**
- Produces: `App::update(&mut self, action: Action) -> bool` (true = state changed, redraw). `App` gains `pub tx: tokio::sync::mpsc::UnboundedSender<Action>` and `App::new(tx)` / `App::new_for_test()` (creates its own channel, leaks the receiver). `Toasts::on_tick(&mut self) -> bool` (true while any toast is visible/animating). `Action::Render` documented as "no state change, force redraw" for background tasks; `update(Render)` returns true. Main loop only calls `terminal.draw` when the previous iteration reported a change.
- Consumes: Task 7 `handle_key` (its `apply` now returns `update`'s bool).

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn tick_requests_no_redraw_when_idle() {
    let mut app = App::new_for_test();
    assert!(!app.update(Action::Tick), "idle tick must not redraw");
}

#[test]
fn tick_requests_redraw_while_toast_visible() {
    let mut app = App::new_for_test();
    app.update(Action::ShowToast("hi".into(), ToastKind::Info));
    assert!(app.update(Action::Tick));
}

#[test]
fn render_action_requests_redraw() {
    let mut app = App::new_for_test();
    assert!(app.update(Action::Render));
}
```

- [ ] **Step 2: fail. Step 3: Implement** — `update` returns `true` from every arm except: `Tick` → `self.toasts.on_tick() || self.in_flight_ticking()` (the latter is `false` until Task 15 introduces in-flight state — write it as a private `fn in_flight_ticking(&self) -> bool { false }` now with a comment that Task 15 replaces it), and `Close` with an empty modal stack → `false`. Main loop:

```rust
let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Action>();
let mut app = App::new(tx);
let mut redraw = true;
while !app.should_quit {
    if redraw {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        redraw = false;
    }
    tokio::select! {
        maybe_event = events.next() => { /* existing match; every branch sets redraw |= ... */ }
        Some(action) = rx.recv() => { redraw |= app.update(action); }
        _ = tick.tick() => { redraw |= app.update(Action::Tick); }
    }
}
```

Resize events (`Event::Resize`) must set `redraw = true` — add that arm explicitly.

- [ ] **Step 4: tests + clippy; also run the app manually is NOT possible here (no TTY for agents) — rely on tests. Step 5: Commit** — `git commit -am "Make rendering event-driven; add background action channel"`

---

### Task 9: Mouse scroll routing

**Files:**
- Modify: `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/main.rs`, `crates/postui/src/components/mod.rs`

**Interfaces:**
- Produces: `Action::ScrollPane(PaneId, i16)` (negative = up, in rows; wheel step = ±3). `Component` trait gains `fn handle_scroll(&mut self, _delta: i16) {}` default no-op. `App::update(ScrollPane(pane, d))` dispatches to that pane's component **without changing focus**. Main loop maps `MouseEventKind::ScrollUp/ScrollDown` through `hit_test` at the event's coordinates.
- Consumes: Task 8 update-returns-bool.

- [ ] **Step 1: Failing test in app.rs:**

```rust
#[test]
fn scroll_dispatches_without_changing_focus() {
    let mut app = App::new_for_test();
    let before = app.focus;
    assert!(app.update(Action::ScrollPane(PaneId::Response, 3)));
    assert_eq!(app.focus, before, "scrolling must not steal focus");
}
```

(The real scroll-consumer assertions land with the response viewer in Task 16; this pins the routing contract.)

- [ ] **Step 2: fail. Step 3: Implement** — trait method, `update` arm dispatching to `self.sidebar/editor/response.handle_scroll(d)`, and in `main.rs` extend the mouse match:

```rust
Event::Mouse(m) if app.modals.is_empty() => {
    let size = terminal.size()?;
    let layout = compute_layout(Rect::new(0, 0, size.width, size.height));
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(pane) = hit_test(&layout, m.column, m.row) {
                redraw |= app.update(Action::FocusPane(pane));
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if let Some(pane) = hit_test(&layout, m.column, m.row) {
                let d = if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                redraw |= app.update(Action::ScrollPane(pane, d));
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 4: tests + clippy. Step 5: Commit** — `git commit -am "Route mouse wheel to hovered pane without focus change"`

---

### Task 10: Shared line input + editor skeleton (method, URL, tabs, dirty)

**Files:**
- Create: `crates/postui/src/components/line_input.rs`
- Modify: `crates/postui/src/components/editor.rs` (full rewrite), `crates/postui/src/components/mod.rs`, `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/keys.rs`, `crates/postui/src/ui.rs`

**Interfaces:**
- Produces:
  - `LineInput { }` with `new(text: &str)`, `text(&self) -> &str`, `cursor(&self) -> usize` (char index), `handle_key(&mut self, KeyEvent) -> bool` (true = consumed: chars insert at cursor, Backspace/Delete, Left/Right/Home/End; returns false for everything else), `draw_line(&self, focused: bool, theme) -> ratatui::text::Line` (cursor rendered as a REVERSED-style cell when focused). Reused by prompt modals (Task 14) and response search (Task 16).
  - `Editor` (replaces unit struct): fields `slug: Option<String>`, `saved: Option<HttpRequest>`, `method: Method`, `url: LineInput`, `params: IndexMap<String, Entry>`, `headers: IndexMap<String, Entry>`, `body_text: String` (placeholder until Task 12 swaps in edtui state), `active_tab: EditorTab`, `sub_focus: SubFocus` (`Url` | `Content`), `table: TableState2` placeholder unit until Task 11. Methods: `load(&mut self, slug: Option<String>, req: HttpRequest)` (also resets `saved`), `current_request(&self) -> HttpRequest`, `is_dirty(&self) -> bool` (`saved.as_ref() != Some(&self.current_request())`; a never-loaded empty editor is not dirty), `mark_saved(&mut self)`.
  - `pub enum EditorTab { Params, Headers, Body }` with `index()`/`from_index(usize)`.
  - New actions + default bindings + `named_actions` names: `Action::EditorTabSelect(usize)` (`editor_tab_1..3` = alt+1/2/3), `Action::EditorTabCycle(i8)` (`editor_tab_next`/`prev` = alt+right/alt+left), `Action::CycleMethod` (`cycle_method` = alt+m), `Action::FocusUrl` (`focus_url` = alt+u). `KeyCombo::parse` must accept named keys used here (already does for arrows; add none).
- Consumes: Tasks 1 (model), 7 (routing), 8 (update bool).

- [ ] **Step 1: Failing tests** (editor.rs test module):

```rust
use postui_core::model::*;

fn key(c: KeyCode) -> KeyEvent { KeyEvent::new(c, KeyModifiers::NONE) }

#[test]
fn typing_into_url_marks_dirty_and_updates_request() {
    let mut e = Editor::default();
    e.load(Some("a".into()), HttpRequest::from_toml_str(r#"url = "https://x""#).unwrap());
    assert!(!e.is_dirty());
    e.sub_focus = SubFocus::Url;
    e.handle_key(key(KeyCode::Char('/')));
    assert_eq!(e.current_request().url, "https://x/");
    assert!(e.is_dirty());
}

#[test]
fn method_cycles_via_action_and_tabs_select() {
    let mut app = App::new_for_test();
    app.update(Action::CycleMethod);
    assert_eq!(app.editor.method, Method::Post);
    app.update(Action::EditorTabSelect(2));
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    app.update(Action::EditorTabCycle(1));
    assert_eq!(app.editor.active_tab, EditorTab::Params, "cycle wraps");
}

#[test]
fn up_down_moves_between_url_and_content() {
    let mut e = Editor::default();
    e.sub_focus = SubFocus::Url;
    e.handle_key(key(KeyCode::Down));
    assert_eq!(e.sub_focus, SubFocus::Content);
    e.handle_key(key(KeyCode::Up));
    assert_eq!(e.sub_focus, SubFocus::Url);
}

#[test]
fn draw_shows_method_badge_url_and_tab_bar() {
    // TestBackend render of Editor with a loaded request; assert buffer contains
    // "POST", the url, "Params", "Headers", "Body" tab labels.
}
```

Write that last test fully (mirror the TestBackend pattern in `components/mod.rs`). Also a `line_input.rs` test module: insert at cursor, backspace, home/end, arrow clamping at both ends — four short tests asserting `text()`/`cursor()`.

- [ ] **Step 2: fail. Step 3: Implement.** Notes: `Editor::handle_key` routing — `sub_focus == Url` → `self.url.handle_key(ev)` consumed→`Some(Action::Render)`? NO: components signal "consumed, state changed" by returning `Some` of a cheap action; introduce nothing new — return `Some(Action::Render)` when a key mutated internal state and no app-level action is needed (its update arm already just redraws). Down/Up switch sub_focus (Body's line-aware Up comes in Task 12). Draw: row 1 = method badge (method colors from stage-1 theme: reuse the token mapping used by the palette/theme for method colors; if none exists yet add `Theme::method_color(Method) -> Color`) + URL line; row 2 = tab bar with `Body`'s validity indicator placeholder (plain label until Task 12); rest = active-tab content (placeholder paragraphs until Tasks 11/12). Keep `ui.rs` full-frame test updated (editor placeholder assertions change: now expects "Params" etc.).

- [ ] **Step 4: `cargo test -p postui` + clippy. Step 5: Commit** — `git commit -am "Rebuild editor skeleton: method badge, URL input, tab bar, dirty tracking"`

---

### Task 11: Key/value table editor (Params & Headers tabs)

**Files:**
- Create: `crates/postui/src/components/table_editor.rs`
- Modify: `crates/postui/src/components/editor.rs`, `crates/postui/src/components/mod.rs`

**Interfaces:**
- Produces: `TableEditorState` operating on a `&mut IndexMap<String, Entry>` passed into each call (state holds only cursor/edit info, never the data):
  - `selected: usize`, `editing: Option<CellEdit>` where `CellEdit { col: Col (Key|Value), input: LineInput, original_key: Option<String> }`
  - `handle_key(&mut self, ev, map: &mut IndexMap<String, Entry>) -> TableOutcome` where `TableOutcome { consumed: bool, warning: Option<String> }`
  - Keys: `j`/`k`/Up/Down move; `a` append row (starts key edit, `original_key: None`); Enter edit selected cell; while editing: chars/arrows via LineInput, Tab commits key edit and opens value edit, Enter commits, Esc cancels; Space toggles `enabled`; `d`/Delete removes row.
  - Commit semantics: committing a key that equals another existing key → the OTHER row keeps its position and takes this row's value; this row is removed; `warning: Some("duplicate key '<k>' replaced the existing value")`.
  - `draw(&self, frame, area, map, ctx)`: columns `[✓/✗] key  value`, selected row accent-highlighted, disabled rows `text_muted`, editing cell shows LineInput cursor, empty state "No params yet — press a to add".
- Consumes: Task 10 `LineInput`, `Editor`.

- [ ] **Step 1: Failing tests** (pure state tests, no rendering except one TestBackend smoke test):

```rust
#[test]
fn add_edit_commit_creates_entry() {
    let mut map = IndexMap::new();
    let mut t = TableEditorState::default();
    t.handle_key(key(KeyCode::Char('a')), &mut map);
    for c in "page".chars() { t.handle_key(key(KeyCode::Char(c)), &mut map); }
    t.handle_key(key(KeyCode::Tab), &mut map); // key → value
    t.handle_key(key(KeyCode::Char('2')), &mut map);
    t.handle_key(key(KeyCode::Enter), &mut map);
    assert_eq!(map["page"], Entry { value: "2".into(), enabled: true });
    assert!(t.editing.is_none());
}

#[test]
fn duplicate_key_commit_replaces_and_warns() {
    let mut map = IndexMap::new();
    map.insert("a".into(), Entry { value: "1".into(), enabled: true });
    map.insert("b".into(), Entry { value: "2".into(), enabled: true });
    let mut t = TableEditorState::default();
    // add new row keyed "a" with value "9"
    t.handle_key(key(KeyCode::Char('a')), &mut map);
    t.handle_key(key(KeyCode::Char('a')), &mut map);
    t.handle_key(key(KeyCode::Tab), &mut map);
    t.handle_key(key(KeyCode::Char('9')), &mut map);
    let out = t.handle_key(key(KeyCode::Enter), &mut map);
    assert!(out.warning.is_some());
    assert_eq!(map.len(), 2);
    assert_eq!(map["a"].value, "9");
    assert_eq!(map.get_index(0).unwrap().0, "a", "original position kept");
}

#[test]
fn space_toggles_d_deletes_esc_cancels() {
    let mut map = IndexMap::new();
    map.insert("a".into(), Entry { value: "1".into(), enabled: true });
    let mut t = TableEditorState::default();
    t.handle_key(key(KeyCode::Char(' ')), &mut map);
    assert!(!map["a"].enabled);
    t.handle_key(key(KeyCode::Enter), &mut map); // start editing
    t.handle_key(key(KeyCode::Char('x')), &mut map);
    t.handle_key(key(KeyCode::Esc), &mut map);   // cancel
    assert_eq!(map["a"].value, "1", "esc discards the edit");
    t.handle_key(key(KeyCode::Char('d')), &mut map);
    assert!(map.is_empty());
}
```

- [ ] **Step 2: fail. Step 3: Implement**, then wire into `Editor::handle_key` for `active_tab ∈ {Params, Headers}` with the matching map; a `warning` becomes `Some(Action::ShowToast(w, ToastKind::Warning))` (add `ToastKind::Warning` if stage 1 only has Info/Error — check `toast.rs` and add the variant + theme color if missing). Editing a key/value marks dirty automatically because `current_request()` reads the maps. Renaming an existing key: Enter on key cell seeds LineInput with the key and `original_key: Some(k)`; commit removes `original_key` and inserts at ITS former index via `IndexMap::shift_remove_index` + `shift_insert` (check exact indexmap API: `shift_insert(index, k, v)` exists on 2.x).

- [ ] **Step 4: tests + clippy. Step 5: Commit** — `git commit -am "Add key/value table editor for params and headers tabs"`

---

### Task 12: Body tab — edtui, validity, format/minify, $EDITOR

**Files:**
- Modify: `crates/postui/Cargo.toml` (add `edtui = { version = "0.11", default-features = false, features = ["syntax-highlighting", "mouse-support"] }`), `crates/postui/src/components/editor.rs`, `crates/postui/src/action.rs`, `crates/postui/src/keys.rs`, `crates/postui/src/app.rs`, `crates/postui/src/main.rs`

**Interfaces:**
- Produces: `Editor.body: edtui::EditorState` + `Editor.body_handler: edtui::EditorEventHandler` (constructed with `EditorEventHandler::emacs_mode()`), replacing `body_text`. `Editor::body_text(&self) -> String` (from `edtui` Lines) and `Editor::set_body_text(&mut self, s: &str)`. `current_request()` maps empty body text → `body: None`, non-empty → `Some(Body::Json { text })`. New actions/bindings/names: `Action::FormatBody` (`format_body` = alt+f), `Action::MinifyBody` (`minify_body` = alt+g), `Action::OpenBodyInEditor` (`open_body_editor` = ctrl+e). Tab bar renders `Body ✓`/`Body ✗` from `postui_core::json::validate` (empty body counts as ✓). `App::update(FormatBody)`: on parse error toast `"line L, column C: msg"` and leave the buffer untouched.
- `Action::OpenBodyInEditor` is NOT applied inside `App::update` — `main.rs` intercepts it before `app.update` (it must suspend the terminal): write body to a `NamedTempFile` with `.json` suffix, `ratatui::restore()` + disable mouse capture, run `$EDITOR` (fall back to `vi`) via `std::process::Command` inherit-stdio, on success read the file back into the body state, then `terminal = ratatui::init()` + re-enable mouse capture + force redraw. On editor exit-failure: toast, body unchanged. Route it as an action so it stays rebindable: `App::handle_key` returns actions normally; give `App` a `pub pending_terminal_action: Option<Action>` — `update(OpenBodyInEditor)` stores it there and returns true; the main loop checks and executes it after `handle_key`. This keeps `App::update` terminal-free and testable.
- Consumes: Tasks 4 (json), 10 (editor skeleton).

- [ ] **Step 1: API spike (throwaway)** — before tests, confirm edtui's exact API compiles: create `crates/postui/examples/edtui_spike.rs` constructing `EditorState` from `Lines::from("{}")`, extracting text back to `String`, pushing a key event through `EditorEventHandler::emacs_mode()`, and rendering `EditorView` into a TestBackend frame. `cargo build --example edtui_spike`. Adapt the interface names in this task to what actually compiles (the crate README shows `EditorState`, `EditorView`, `EditorEventHandler`, `Lines`; text extraction is likely `String::from(&state.lines)` or an iterator — pin it here). DELETE the example file once the real code compiles.

- [ ] **Step 2: Failing tests:**

```rust
#[test]
fn body_text_roundtrip_and_empty_means_no_body() {
    let mut e = Editor::default();
    e.set_body_text("{\n  \"a\": 1\n}");
    assert_eq!(e.body_text(), "{\n  \"a\": 1\n}");
    assert!(matches!(e.current_request().body, Some(Body::Json { .. })));
    e.set_body_text("");
    assert!(e.current_request().body.is_none());
}

#[test]
fn typing_in_body_tab_inserts_text_modelessly() {
    let mut e = Editor::default();
    e.active_tab = EditorTab::Body;
    e.sub_focus = SubFocus::Content;
    e.handle_key(key(KeyCode::Char('{')));
    assert_eq!(e.body_text(), "{", "emacs mode: chars insert without entering a vim insert mode");
}

#[test]
fn format_body_pretty_prints_only_valid_json() {
    let mut app = App::new_for_test();
    app.editor.set_body_text("{\"a\":1}");
    app.update(Action::FormatBody);
    assert!(app.editor.body_text().contains('\n'));
    app.editor.set_body_text("{oops");
    app.update(Action::FormatBody);
    assert_eq!(app.editor.body_text(), "{oops", "invalid body untouched");
    app.editor.set_body_text("{ \"a\": 1 }");
    app.update(Action::MinifyBody);
    assert_eq!(app.editor.body_text(), "{\"a\":1}");
}

#[test]
fn save_preserves_invalid_body_verbatim() {
    let mut e = Editor::default();
    e.set_body_text("{ \"in-progress\": ");
    let req = e.current_request();
    let back = HttpRequest::from_toml_str(&req.to_toml_string()).unwrap();
    assert_eq!(back.body, Some(Body::Json { text: "{ \"in-progress\": ".into() }));
}
```

- [ ] **Step 3: fail, implement.** Editor body key handling: in `handle_key` with `active_tab == Body && sub_focus == Content`, if the key is Up and the body cursor is on row 0, move `sub_focus` to Url instead of delegating; Esc → `sub_focus = Url`; otherwise delegate to `self.body_handler.on_key_event(ev, &mut self.body)` and return `Some(Action::Render)`. ALT-modified events never reach here (routing order). Draw the body tab with `EditorView::new(&mut …)` themed from stage-1 tokens (`EditorTheme::default().base(...)` — background `surface`, cursor style REVERSED) plus JSON `SyntaxHighlighter` (theme name: pick the closest bundled syntect theme to the app theme — `"base16-ocean.dark"` for dark, `"base16-ocean.light"` for light — and note that matching custom themes is a stage-6 polish item). NOTE: `EditorView` requires `&mut EditorState`; `Component::draw` takes `&self` — give `Editor` interior mutability for the body state via `std::cell::RefCell<edtui::EditorState>` OR change the `Component::draw` signature to `&mut self` across all components (pick the trait change — it's honest, mechanical, and all components are ours; update the trait, all impls, and `ui::draw`).
- [ ] **Step 4: implement $EDITOR flow in main.rs as specified in Interfaces.** No automated test (needs a TTY); the manual sweep covers it. Keep the logic in a `fn edit_body_externally(terminal, app) -> anyhow::Result<()>` so it's reviewable.
- [ ] **Step 5: tests + clippy. Step 6: Commit** — `git commit -am "Add JSON body editing: edtui emacs mode, validity badge, format/minify, external editor"`

---

### Task 13: Real sidebar — listing, navigation, open with dirty prompt

**Files:**
- Modify: `crates/postui/src/components/sidebar.rs` (rewrite), `crates/postui/src/components/modal.rs` (add `Modal::Confirm`), `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/ui.rs`

**Interfaces:**
- Produces:
  - `Sidebar` state: `rows: Vec<Row>` (`enum Row { Dir(String), Request { slug: String, broken: Option<String> } }`), `selected: usize` (always on a Request row; Dir rows are skipped in navigation), `scroll: usize`, `open_slug: Option<String>`, `open_dirty: bool`. `Sidebar::refresh(&mut self, listing: Vec<postui_core::storage::RequestListing>)` builds rows: requests sorted by slug; a Dir row is inserted before the first request of each directory prefix (top-level requests come first, no Dir row). `handle_key`: j/k/Up/Down move (skipping Dir rows, clamped), Enter → `Some(Action::OpenRequest(slug))` (`None` on broken → instead `Some(Action::ShowRequestError(slug))`), `handle_scroll` adjusts `scroll`.
  - `App` gains `project_root: PathBuf` and `App::new(tx)` resolves `storage::default_project_dir()` + `ensure_project` (on failure: toast + empty sidebar); `App::new_for_test()` uses a `tempfile::TempDir` kept alive in a test-only field (`#[cfg(test)] _test_dir: Option<tempfile::TempDir>`; add `tempfile` to postui dev-deps... it must be a real dep for cfg(test) fields — instead store it in the test itself: `App::with_root(tx, PathBuf)` public constructor, `new_for_test` uses `std::env::temp_dir().join(unique)` — simplest: `with_root` + tests create tempdirs and pass paths).
  - Actions: `Action::OpenRequest(String)` (dirty-checks), `Action::ForceOpenRequest(String)` (loads + `editor.load` + `sidebar.open_slug` update), `Action::SaveRequest` (Task 14 completes it; here: saves when `slug.is_some()`, toast "Saved <slug>"), `Action::ShowRequestError(String)` (Message modal with the stored parse error), `Action::RefreshSidebar`.
  - `Modal::Confirm { title: String, body: String, choices: Vec<(char, String, Vec<Action>)> }` — handle_key: a choice char (case-insensitive) → `ModalResult { actions: that vec, close: true }`; Esc → close, no actions; all else swallowed. Draw: centered modal listing "[s] Save  [d] Discard  [esc] Cancel"-style hints.
  - Dirty flow in `update(OpenRequest(slug))`: if `editor.is_dirty()` push `Modal::Confirm` with choices `[('s', "Save & open", vec![SaveRequest, ForceOpenRequest(slug)]), ('d', "Discard changes", vec![ForceOpenRequest(slug)])]`; else apply `ForceOpenRequest` inline.
- Consumes: Tasks 2 (storage), 6 (ModalResult), 7 (routing), 10 (editor.load).

- [ ] **Step 1: Failing tests** (app-level, with `App::with_root` + tempdir + files created via `storage::save_request`):

```rust
#[test]
fn sidebar_lists_requests_grouped_and_enter_opens() { /* create auth/login + ping; refresh;
    assert rows = [Request ping, Dir auth, Request auth/login] (top-level first);
    navigate to auth/login, Enter → editor.slug == Some("auth/login") */ }

#[test]
fn opening_over_dirty_editor_prompts_save_discard_cancel() { /* open a, edit url, OpenRequest(b):
    modal Confirm present, editor still on a; press 'd' via app.handle_key → editor on b, not dirty;
    repeat with 's' → file a re-read from disk contains the edit */ }

#[test]
fn broken_file_shows_marker_and_error_modal() { /* write bad TOML; refresh; row broken.is_some();
    Enter → Modal::Message containing the parse error */ }

#[test]
fn dirty_dot_renders_in_sidebar() { /* TestBackend: open request, type into url, draw, buffer contains "●" on the open row */ }
```

Write these fully in the implementation (the comments above specify exact behavior; expand each into real code following the patterns of earlier tasks).

- [ ] **Step 2: fail. Step 3: Implement.** Draw details: Dir rows muted + `▸ name/`; Request rows show basename indented under their Dir, `✗` in error color when broken, `●` accent when `open_slug == slug && open_dirty`, selection = accent bold + `› ` marker (match palette's selection style); respect `scroll` with simple windowing (`rows[scroll..]` into available height, keep selected visible by adjusting scroll in handle_key). `open_dirty` is refreshed by `App::update` after every applied action: `self.sidebar.open_dirty = self.editor.is_dirty()` (single line at the end of `update`).

- [ ] **Step 4: tests + clippy. Step 5: Commit** — `git commit -am "Real sidebar: request listing, open with dirty prompt, broken-file surfacing"`

---

### Task 14: Request CRUD — new, rename, delete, save-as; palette commands

**Files:**
- Modify: `crates/postui/src/components/modal.rs` (add `Modal::Prompt`), `crates/postui/src/components/sidebar.rs`, `crates/postui/src/components/palette.rs`, `crates/postui/src/action.rs`, `crates/postui/src/app.rs`

**Interfaces:**
- Produces:
  - `Modal::Prompt { title: String, input: LineInput, kind: PromptKind }`, `enum PromptKind { NewRequest, RenameRequest { from: String }, SaveAs }`. handle_key: LineInput consumes editing keys; Enter with non-empty text → `ModalResult { close: true, actions: vec![matching action] }`; Esc → close.
  - Actions: `Action::PromptNewRequest`, `Action::CreateRequest(String)`, `Action::PromptRenameRequest` (prefills input with selected slug), `Action::RenameRequest { from: String, to: String }`, `Action::ConfirmDeleteRequest`, `Action::DeleteRequest(String)`. Sidebar keys: `n` → PromptNewRequest, `r` → PromptRenameRequest, `d` → ConfirmDeleteRequest (all only when a request row is selected for r/d).
  - `CreateRequest(name)`: `validate_slug` (invalid → error toast naming the rule "lowercase letters, digits, - _ and / only"), reject existing slug, save a fresh `HttpRequest { method: Get, url: "" , .. }`, refresh sidebar, open it. `RenameRequest`: storage rename, refresh, follow `open_slug` if it was the renamed one. `DeleteRequest`: storage delete, refresh, if it was open reset editor to `Editor::default()`. `SaveRequest` completed: `slug: None` → open `Modal::Prompt` SaveAs instead; SaveAs enter → validate/save/refresh/set slug.
  - Palette commands appended to `all_commands()`: "Request: new", "Request: save", "Request: rename", "Request: delete", "Method: cycle", "Body: format JSON", "Body: minify JSON", "Body: open in $EDITOR", "Send request" (action added Task 15 — add the palette entry there).
- Consumes: Tasks 10 (LineInput), 13 (sidebar/actions).

- [ ] **Step 1: Failing tests** — app-level with tempdir root: create via prompt flow (`PromptNewRequest` → type name via `handle_key` → Enter → file exists on disk, editor.slug set); invalid name → toast + no file + modal closed; rename updates disk and `open_slug`; delete of open request empties editor; `SaveRequest` with no slug opens SaveAs prompt. Five tests, written out fully following Task 13's pattern.

- [ ] **Step 2: fail. Step 3: Implement.** All storage errors surface as `ShowToast(err.to_string(), ToastKind::Error)` — never panic. **Step 4: tests + clippy. Step 5: Commit** — `git commit -am "Request CRUD: create, rename, delete, save-as prompts and palette commands"`

---

### Task 15: Send pipeline — reqwest task, cancel, spinner state

**Files:**
- Create: `crates/postui/src/http.rs`
- Modify: `crates/postui/Cargo.toml` (add `reqwest = { version = "0.13", default-features = false, features = ["rustls-tls"] }`), `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/keys.rs`, `crates/postui/src/lib.rs`, `crates/postui/src/components/response.rs` (state only; rendering in Task 16)

**Interfaces:**
- Produces:
  - `http::ResponseData { status: u16, headers: Vec<(String, String)>, body: String, elapsed: std::time::Duration, size: usize, content_type: Option<String> }` (`Debug + Clone + PartialEq`)
  - `http::client() -> reqwest::Client` (30 s total timeout via `Client::builder().timeout(...)`; built once, stored on `App`)
  - `async fn http::send(client: &reqwest::Client, req: &PreparedRequest) -> Result<ResponseData, String>` — maps `Method` via `reqwest::Method::from_bytes`, sets headers, body if any, reads full body via `bytes()` (lossy UTF-8 into `String`), `size` = raw byte len, elapsed measured inside. Error string = the reqwest error chain joined with ": " (walk `std::error::Error::source`).
  - `App` fields: `client: reqwest::Client`, `in_flight: Option<InFlight>` where `InFlight { started: std::time::Instant, generation: u64, task: tokio::task::JoinHandle<()> }`, `send_generation: u64`.
  - Actions: `Action::Send` (`send` = `ctrl+r` AND `ctrl+enter`), `Action::ForceSend`, `Action::CancelSend`, `Action::ResponseArrived { generation: u64, data: Box<http::ResponseData> }`, `Action::RequestFailed { generation: u64, error: String }`.
  - `update(Send)`: body invalid (non-empty and `json::validate` fails) → `Modal::Confirm` "Body is not valid JSON — send anyway?" `[('y', "Send anyway", vec![ForceSend])]`; else same as ForceSend. `ForceSend`: `prepare()` → each warning `ShowToast(w.to_string(), Warning)`; empty URL → error toast, stop; abort any existing in_flight task; `generation += 1`; spawn (tx clone) — on completion sends `ResponseArrived`/`RequestFailed` tagged with generation; store InFlight; set response pane state `InFlight`. `ResponseArrived`/`RequestFailed`/`CancelSend`: ignore if `generation != send_generation`; clear `in_flight`; set response state. `CancelSend` aborts the task, state `Cancelled`. `in_flight_ticking()` (Task 8 stub) now returns `self.in_flight.is_some()`.
  - `Response` component gains `pub state: ResponseState` — `enum ResponseState { Empty, InFlight { started: Instant }, Ready(Box<ResponseData>), Failed(String), Cancelled }`; Esc key in response pane while InFlight → `Some(Action::CancelSend)`.
- Consumes: Tasks 3 (prepare), 8 (tx channel), 13 (editor current_request).

- [ ] **Step 1: Failing unit tests** (tokio tests, no network — wiremock arrives next task; these cover state wiring):

```rust
#[tokio::test]
async fn send_with_invalid_body_prompts_first() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = LineInput::new("http://127.0.0.1:9"); // unroutable, never actually hit
    app.editor.set_body_text("{oops");
    app.update(Action::Send);
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert!(app.in_flight.is_none());
}

#[tokio::test]
async fn stale_generation_results_are_ignored() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.send_generation = 5;
    app.update(Action::RequestFailed { generation: 4, error: "old".into() });
    assert!(matches!(app.response.state, ResponseState::Empty), "stale result dropped");
}

#[tokio::test]
async fn empty_url_toasts_instead_of_sending() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.update(Action::Send);
    assert!(app.in_flight.is_none());
}
```

(`App::with_root` construction needs a tokio runtime because `reqwest::Client` — hence `#[tokio::test]`; if Client construction is actually runtime-free, plain `#[test]` is fine — keep whichever compiles.) Existing non-async App tests keep working: if `Client::new` panics without a runtime, make `client` lazy (`OnceCell<reqwest::Client>` built on first send inside the spawned context) — decide by testing, prefer the simplest that keeps `App::new_for_test()` synchronous.

- [ ] **Step 2: fail. Step 3: Implement** (including the `send` palette entry "Send request"). **Step 4: tests + clippy. Step 5: Commit** — `git commit -am "Async send pipeline: reqwest task, generation-tagged results, cancel"`

---

### Task 16: Response viewer — summary, JSON tree, raw/headers, search

**Files:**
- Create: `crates/postui/src/components/json_tree.rs`
- Modify: `crates/postui/src/components/response.rs` (full rewrite of rendering + keys)

**Interfaces:**
- Produces:
  - `json_tree::JsonTree`: `parse(text: &str) -> Option<JsonTree>` (None = not JSON or > 2 MiB handled by caller); internal flat `Vec<TreeLine>` from a recursive walk of `serde_json::Value` — each line: `indent: usize`, `text spans` split into (key, punctuation, value) for theme coloring, `container: Option<Container { id: usize, children: usize, is_array: bool, end_line: usize }>`, `parent_ids: Vec<usize>`. API: `visible_lines(&self) -> Vec<&TreeLine>` (skips lines inside collapsed containers; a collapsed container's opening line renders `{…} N keys` / `[…] N items`), `toggle(&mut self, visible_index: usize)`, `expand_ancestors(&mut self, line_index: usize)`, `line_count(&self)`, `full_text_lines(&self) -> Vec<String>` (fully-expanded plain text, for search).
  - `Response` rendering by state: `Empty` → stage-1-style empty hint; `InFlight` → spinner glyph cycle (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` indexed by `started.elapsed().subsec_millis()/100`) + "1.2s" elapsed + "esc to cancel"; `Failed(e)` → error-styled paragraph; `Cancelled` → muted "Request cancelled"; `Ready` → summary line (status pill colored: 2xx success, 3xx accent, 4xx warning, 5xx error tokens; elapsed; human size "1.4 KB"; content-type) above the active view.
  - Views & keys (plain keys in response pane): `r` toggle Pretty↔Raw, `h` Headers view, `j/k`/arrows move cursor line, Space/Enter toggle collapse at cursor (Pretty only), `g`/`G` top/bottom, `/` open search (LineInput in the pane footer; Enter commits, Esc closes), `n`/`N` next/prev match with counter "3/17" and match highlighting in Pretty and Raw views (search matches on `full_text_lines`; jumping in Pretty calls `expand_ancestors` then recomputes the visible cursor), `handle_scroll(delta)` moves the viewport.
  - Pretty is the default view when `JsonTree::parse` succeeds AND `body.len() <= 2 * 1024 * 1024`; otherwise Raw with a one-line hint ("body exceeds 2 MiB — raw view only" when over the guard). Body text is NEVER reformatted for Raw — verbatim lines.
- Consumes: Task 15 `ResponseState`/`ResponseData`, Task 10 `LineInput`, Task 9 scroll.

- [ ] **Step 1: Failing tests** — json_tree pure tests first:

```rust
#[test]
fn tree_flattens_and_collapses() {
    let mut t = JsonTree::parse(r#"{"a": {"b": 1, "c": [1, 2]}, "d": null}"#).unwrap();
    let total = t.visible_lines().len();
    // line 0 = "{", line 1 = "a": { ... find the container line for "a"
    t.toggle(1);
    let collapsed = t.visible_lines().len();
    assert!(collapsed < total);
    let line_text = t.visible_lines()[1].plain_text();
    assert!(line_text.contains("2 keys"), "collapsed summary shows child count: {line_text}");
    t.toggle(1);
    assert_eq!(t.visible_lines().len(), total, "re-expand restores");
}

#[test]
fn search_lines_cover_collapsed_content() {
    let mut t = JsonTree::parse(r#"{"outer": {"needle": "x"}}"#).unwrap();
    t.toggle(1); // collapse outer
    let text = t.full_text_lines().join("\n");
    assert!(text.contains("needle"), "search text ignores collapse state");
}
```

Then component tests: TestBackend render of `Ready` asserts status pill text ("200"), elapsed, size, and a JSON key from the body; `r` toggles to raw (assert a line rendered verbatim); guard test with a >2 MiB synthetic body asserts Raw + hint; scroll/cursor clamp test.

- [ ] **Step 2: fail. Step 3: Implement.** Rendering colors come from theme tokens directly (keys = accent, strings = success, numbers = warning-or-a-new `literal` token — reuse existing tokens, do NOT invent new theme fields unless stage 1 lacks any usable one). This intentionally implements the spec's "highlighted pretty view" from the parsed tree rather than running syntect over generated text — same visual outcome, single source of truth (syntect still runs in the body editor via edtui). **Step 4: tests + clippy. Step 5: Commit** — `git commit -am "Response viewer: summary line, collapsible JSON tree, raw/headers views, search"`

---

### Task 17: HTTP integration tests (wiremock)

**Files:**
- Create: `crates/postui/tests/http_integration.rs`
- Modify: `crates/postui/Cargo.toml` (`[dev-dependencies] wiremock = "0.6"`, `tempfile = "3"` if not already there from earlier tasks)

**Interfaces:** consumes `http::send`, `http::client`, `prepare`, App actions. To test timeout quickly, `http::client` gains a sibling `http::client_with_timeout(d: Duration)` used by tests (app keeps 30 s).

- [ ] **Step 1: Write the tests** (these are the deliverable — they should pass immediately if Tasks 3/15 are correct; any failure is a real bug to fix now):

```rust
use postui::http;
use postui_core::model::*;
use postui_core::prepare::prepare;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path, header, query_param};

fn req_to(url: String) -> HttpRequest {
    HttpRequest { method: Method::Post, url, params: Default::default(),
        headers: Default::default(), body: Some(Body::Json { text: "{\"a\":1}".into() }) }
}

#[tokio::test]
async fn sends_json_with_auto_content_type_and_reads_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/x"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(201).set_body_string("{\"ok\":true}"))
        .mount(&server).await;
    let (prepared, _) = prepare(&req_to(format!("{}/x", server.uri())));
    let data = http::send(&http::client(), &prepared).await.unwrap();
    assert_eq!(data.status, 201);
    assert_eq!(data.body, "{\"ok\":true}");
    assert_eq!(data.size, 11);
}

#[tokio::test]
async fn merged_query_params_reach_the_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(query_param("id", "2"))
        .respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let mut r = req_to(format!("{}/x?id=1", server.uri()));
    r.method = Method::Get; r.body = None;
    r.params.insert("id".into(), Entry { value: "2".into(), enabled: true });
    let (prepared, warns) = prepare(&r);
    assert_eq!(warns.len(), 1);
    assert_eq!(http::send(&http::client(), &prepared).await.unwrap().status, 200);
}

#[tokio::test]
async fn non_2xx_is_a_response_not_an_error() { /* 500 template → Ok(data), data.status == 500 */ }

#[tokio::test]
async fn timeout_produces_err() {
    // ResponseTemplate::new(200).set_delay(Duration::from_secs(2)); client_with_timeout(200ms)
    // assert send(...).await.is_err()
}

#[tokio::test]
async fn redirects_are_followed() {
    // Mock /a → 302 Location /b; /b → 200 "done"; assert body "done"
}

#[tokio::test]
async fn connection_refused_yields_readable_error() {
    let (prepared, _) = prepare(&HttpRequest { method: Method::Get,
        url: "http://127.0.0.1:1/".into(), params: Default::default(),
        headers: Default::default(), body: None });
    let err = http::send(&http::client(), &prepared).await.unwrap_err();
    assert!(!err.is_empty() && !err.contains("Error {"), "human string, not Debug dump: {err}");
}
```

Fill in the three sketched bodies completely. 

- [ ] **Step 2: Run** — `cargo test -p postui --test http_integration`. Fix any real bugs surfaced (e.g. redirect defaults, error formatting). **Step 3: clippy. Step 4: Commit** — `git commit -am "Add wiremock integration tests for the send pipeline"`

---

### Task 18: Polish & stage-2 acceptance

**Files:**
- Modify: `crates/postui/src/components/footer.rs`, `crates/postui/src/ui.rs`, `crates/postui/tests/stage1_acceptance.rs`, create `crates/postui/tests/stage2_acceptance.rs`

**Steps:**

- [ ] **Step 1: Context-sensitive footer hints** — footer shows hints for the focused pane (Sidebar: "enter open · n new · r rename · d delete"; Editor: "ctrl+r send · ctrl+s save · alt+1/2/3 tabs"; Response: "r raw · h headers · / search"), plus the global "ctrl+p commands · q quit". Test: TestBackend render per focus asserts a distinguishing hint substring.
- [ ] **Step 2: Stage-2 acceptance test** — end-to-end with TestBackend + tempdir + wiremock: create a request via actions, type a URL pointing at a mock server, send (drive the channel: after `update(ForceSend)`, `rx.recv().await` the result action and feed it back through `update`), then render and assert the full frame shows: sidebar entry, method badge, status pill "200", and a body key from the JSON tree. This is the spec's exit criterion in test form.
- [ ] **Step 3: Sweep** — update the stage-1 acceptance test if chrome text changed; delete any dead code flagged by clippy; confirm `Action::Render` is genuinely used (it is: component-consumed redraws + background); `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` both green.
- [ ] **Step 4: Commit** — `git commit -am "Stage-2 polish: contextual footer hints, acceptance test"`

---

### Task 19: Manual TTY sweep (user) and wrap-up

- [ ] Ask the user to run `cargo run -p postui` and walk: create/save/reopen a request (including a name with `/`), edit params/headers/body (format, minify, `$EDITOR` round-trip), send against a real API, watch the spinner, cancel one, collapse/search the response, wheel-scroll each pane, verify ctrl+c quits from inside every modal, and that `~/.config/postui/default/requests/*.toml` files look like the spec's example and diff cleanly after edits.
- [ ] Fix what the sweep surfaces, then invoke superpowers:finishing-a-development-branch.
