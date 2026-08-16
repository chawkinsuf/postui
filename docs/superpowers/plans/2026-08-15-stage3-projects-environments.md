# Stage 3 — Projects & Environments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Multi-project, multi-environment workflows: project directories anywhere on disk with effortless switching, environment files with an always-visible selector, `{{var}}` substitution with a picker, and project-level default headers.

**Architecture:** `postui-core` gains a `vars` module (token parsing + substitution), a `project` module (project.toml / variables.toml / environments / .local state), and a context-taking `prepare()`. The `postui` crate gains a global project registry (`config.rs`), a `ProjectContext` held by `App` (replacing the bare `project_root`), a real sidebar tree, a reusable fuzzy chooser modal, a variable-picker modal, and switch/cycle actions. All file pickup is on-demand (mtime-checked), no watcher.

**Tech Stack:** Rust (edition 2024), ratatui 0.30 + crossterm 0.29 (via `ratatui::crossterm`), tokio, edtui 0.11, reqwest 0.13, toml/toml_edit, indexmap, wiremock (tests). **No new dependencies.**

**Spec:** `docs/superpowers/specs/2026-08-15-stage3-projects-environments-design.md` (and parent `2026-08-15-postui-design.md`). The spec travels with this plan; executors read both.

## Global Constraints

- Cargo needs the PATH prefix: run all cargo commands as `export PATH="$HOME/.cargo/bin:$PATH" && cargo ...`.
- Before every commit: `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` must pass, and `cargo test --workspace` must be green.
- Import crossterm types via `ratatui::crossterm::...`, never the `crossterm` crate directly (version-skew rule).
- No TTY is available to agents: all TUI tests use `ratatui::backend::TestBackend`; never run the app interactively.
- Request files keep raw `{{var}}` text; substitution is send-time-only.
- Variable names: ASCII alphanumeric plus `_` and `-`, case-sensitive, non-empty.
- Environment names: single-segment slug (lowercase/digit/`-`/`_`).
- `substitute_body` defaults to `false` and is omitted from request TOML when false.
- New keybinding defaults (all rebindable, may be adjusted after the manual TTY sweep): `ctrl+o` project chooser, `alt+o` project cycle, `alt+n` new project, `alt+e` environment chooser, `alt+c` environment cycle, `ctrl+v` variable picker, `alt+b` toggle body substitution.
- Commit messages: imperative, no Co-Authored-By, no Claude-Session trailer.

## File Structure

| File | Responsibility |
|---|---|
| `crates/postui-core/src/vars.rs` (new) | `{{var}}` token scanning, substitution, name validation |
| `crates/postui-core/src/project.rs` (new) | project.toml / variables.toml / environments / `.local/state.toml` types + IO, project init/upgrade, resolution |
| `crates/postui-core/src/model.rs` | `HttpRequest.substitute_body` flag |
| `crates/postui-core/src/prepare.rs` | `PrepareContext` (vars + default headers), unresolved error |
| `crates/postui-core/src/storage.rs` | rename no-op fix, checked listing |
| `crates/postui/src/config.rs` (new) | global project registry in `config.toml` (known/root/last), `~` expansion |
| `crates/postui/src/project_ctx.rs` (new) | `ProjectContext`: loaded project files, active env, resolved vars, mtime-checked reload |
| `crates/postui/src/components/chooser.rs` (new) | generic fuzzy chooser modal (projects, environments) |
| `crates/postui/src/components/var_picker.rs` (new) | variable picker modal |
| `crates/postui/src/components/sidebar.rs` | real tree, free wheel scroll |
| `crates/postui/src/components/modal.rs` | new modal variants (Chooser, NewProject, VarPicker) |
| `crates/postui/src/components/editor.rs` | substitute_body flag + indicator, inherited header rows, `{{` trigger, body mouse |
| `crates/postui/src/components/header_bar.rs` | `project · environment` display |
| `crates/postui/src/app.rs` | new actions, ProjectContext wiring, switch gates |
| `crates/postui/src/action.rs`, `keys.rs`, `components/palette.rs` | new actions/bindings/commands |
| `crates/postui/src/main.rs` | CLI arg, focus-change events, mouse forwarding |
| `crates/postui/tests/stage3_acceptance.rs` (new) | end-to-end two-project/two-env flow |

Execute in a worktree branch named `stage3-projects-envs` (superpowers:using-git-worktrees).

---

### Task 1: Core variable tokenizer + substitution (`vars.rs`)

**Files:**
- Create: `crates/postui-core/src/vars.rs`
- Modify: `crates/postui-core/src/lib.rs` (add `pub mod vars;`)

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces:
  - `pub fn is_valid_var_name(name: &str) -> bool`
  - `pub struct Token { pub name: String, pub start: usize, pub end: usize }` (byte offsets, braces included)
  - `pub fn find_tokens(text: &str) -> Vec<Token>`
  - `pub fn substitute(text: &str, values: &indexmap::IndexMap<String, String>, missing: &mut std::collections::BTreeSet<String>) -> String` — replaces resolvable tokens, leaves unresolvable token text verbatim and records the name in `missing`.

- [ ] **Step 1: Write the failing tests** (in `vars.rs` `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use std::collections::BTreeSet;

    fn vals(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn valid_names_are_alnum_dash_underscore() {
        assert!(is_valid_var_name("base_url"));
        assert!(is_valid_var_name("Token-2"));
        assert!(!is_valid_var_name(""));
        assert!(!is_valid_var_name("has space"));
        assert!(!is_valid_var_name("dotted.name"));
    }

    #[test]
    fn finds_simple_and_whitespace_padded_tokens() {
        let t = find_tokens("{{base_url}}/x/{{ id }}");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].name, "base_url");
        assert_eq!((t[0].start, t[0].end), (0, 12));
        assert_eq!(t[1].name, "id");
    }

    #[test]
    fn malformed_tokens_stay_literal() {
        assert!(find_tokens("{{unclosed").is_empty());
        assert!(find_tokens("{{bad name}}").is_empty());
        assert!(find_tokens("{ {x} }").is_empty());
        assert!(find_tokens("{{}}").is_empty());
        // a stray '{' immediately before a real token must not hide it
        let t = find_tokens("{{{a}}");
        assert_eq!(t.len(), 1, "{{ + {{a}} : the trailing {{a}} parses");
        assert_eq!(t[0].name, "a");
        assert_eq!((t[0].start, t[0].end), (1, 6));
    }

    #[test]
    fn substitute_replaces_known_and_records_missing() {
        let mut missing = BTreeSet::new();
        let out = substitute(
            "{{base}}/u/{{id}}?q={{gone}}",
            &vals(&[("base", "http://x"), ("id", "7")]),
            &mut missing,
        );
        assert_eq!(out, "http://x/u/7?q={{gone}}");
        assert_eq!(missing.into_iter().collect::<Vec<_>>(), vec!["gone".to_string()]);
    }

    #[test]
    fn substitute_without_tokens_is_identity() {
        let mut missing = BTreeSet::new();
        assert_eq!(substitute("plain { braces }", &vals(&[]), &mut missing), "plain { braces }");
        assert!(missing.is_empty());
    }

    #[test]
    fn substitute_is_single_pass_not_recursive() {
        let mut missing = BTreeSet::new();
        let out = substitute("{{a}}", &vals(&[("a", "{{b}}"), ("b", "boom")]), &mut missing);
        assert_eq!(out, "{{b}}", "a substituted value must not be re-scanned");
        assert!(missing.is_empty());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p postui-core vars`
Expected: compile error, `vars` module missing.

- [ ] **Step 3: Implement**

```rust
use indexmap::IndexMap;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub name: String,
    /// Byte offset of the opening `{{`.
    pub start: usize,
    /// Byte offset one past the closing `}}`.
    pub end: usize,
}

pub fn is_valid_var_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Scans for well-formed `{{ name }}` tokens (optional inner whitespace).
/// Anything malformed is left for the caller to treat as literal text; a
/// failed match advances by one byte so `{{{a}}` still finds `{{a}}`.
pub fn find_tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'{' || bytes[i + 1] != b'{' {
            i += 1;
            continue;
        }
        // try to parse `{{ \s* name \s* }}` starting at i
        let inner_start = i + 2;
        let Some(close) = text[inner_start..].find("}}").map(|p| inner_start + p) else {
            break; // no closing braces anywhere: nothing further can match
        };
        let name = text[inner_start..close].trim();
        if is_valid_var_name(name) {
            out.push(Token { name: name.to_string(), start: i, end: close + 2 });
            i = close + 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Replaces every token whose name is in `values`; tokens with no value stay
/// verbatim and their names are collected into `missing` (a set: each name
/// reported once, sorted).
pub fn substitute(
    text: &str,
    values: &IndexMap<String, String>,
    missing: &mut BTreeSet<String>,
) -> String {
    let tokens = find_tokens(text);
    if tokens.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for t in tokens {
        out.push_str(&text[last..t.start]);
        match values.get(&t.name) {
            Some(v) => out.push_str(v),
            None => {
                missing.insert(t.name.clone());
                out.push_str(&text[t.start..t.end]);
            }
        }
        last = t.end;
    }
    out.push_str(&text[last..]);
    out
}
```

Add `pub mod vars;` to `crates/postui-core/src/lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p postui-core vars`
Expected: all new tests PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/postui-core/src/vars.rs crates/postui-core/src/lib.rs
git commit -m "Add {{var}} tokenizer and single-pass substitution"
```

---

### Task 2: `substitute_body` flag on `HttpRequest`

**Files:**
- Modify: `crates/postui-core/src/model.rs`
- Modify: `crates/postui/src/components/editor.rs` (field + struct literals)
- Modify: `crates/postui/src/app.rs` (CreateRequest struct literal)

**Interfaces:**
- Produces: `HttpRequest.substitute_body: bool` (serde default false, omitted when false by `to_toml_string`); `Editor.substitute_body: bool` (public field, loaded/saved with the request; toggle UI arrives in Task 13).

- [ ] **Step 1: Write the failing tests** (model.rs test module)

```rust
#[test]
fn substitute_body_round_trips_and_is_omitted_when_false() {
    let mut req = sample();
    assert!(!req.substitute_body, "default off");
    let out = req.to_toml_string();
    assert!(!out.contains("substitute_body"), "false is omitted: {out}");

    req.substitute_body = true;
    let out = req.to_toml_string();
    assert!(out.contains("substitute_body = true"), "{out}");
    let back = HttpRequest::from_toml_str(&out).unwrap();
    assert!(back.substitute_body);
    assert_eq!(back, req);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p postui-core substitute_body`
Expected: compile error, no such field.

- [ ] **Step 3: Implement**

In `HttpRequest` (keep field order stable; place after `url`):

```rust
    /// Whether `{{var}}` tokens in the body are substituted at send time.
    /// Opt-in per request; `false` is the default and is omitted from the
    /// TOML so untouched requests don't churn in diffs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub substitute_body: bool,
```

In `to_toml_string`, after `doc["url"] = value(&self.url);`:

```rust
        if self.substitute_body {
            doc["substitute_body"] = value(true);
        }
```

Then fix every `HttpRequest { ... }` struct literal in the workspace to include `substitute_body: false` (model.rs test `sample()`, prepare.rs test `base()`, `app.rs` `CreateRequest` arm), and in `editor.rs`:
- add `pub substitute_body: bool,` to `Editor` (Default: `false`),
- `load()`: `self.substitute_body = req.substitute_body;`
- `current_request()`: `substitute_body: self.substitute_body,`

- [ ] **Step 4: Run the whole workspace**

Run: `cargo test --workspace`
Expected: PASS (round-trip test included).

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Add per-request substitute_body flag (default off, omitted from TOML)"
```

---

### Task 3: Core project files module (`project.rs`)

**Files:**
- Create: `crates/postui-core/src/project.rs`
- Modify: `crates/postui-core/src/lib.rs` (add `pub mod project;`)

**Interfaces:**
- Consumes: `model::Entry`, `vars::is_valid_var_name`, `storage::validate_slug` (single-segment check via `!name.contains('/') && validate_slug(name).is_ok()`).
- Produces:

```rust
pub struct ProjectMeta { pub name: Option<String>, pub default_headers: IndexMap<String, Entry> }
pub struct VarDecl { pub description: Option<String>, pub default: Option<String> }
pub type Variables = IndexMap<String, VarDecl>;
pub struct LocalState { pub environment: Option<String>, pub open_request: Option<String>, pub expanded: Vec<String> }
#[derive(thiserror::Error, Debug)] pub enum ProjectError { Io(#[from] std::io::Error), Parse(String), BadName(String) }

pub fn is_project(root: &Path) -> bool                         // project.toml exists
pub fn display_name(root: &Path, meta: &ProjectMeta) -> String // meta.name or dir basename
pub fn load_meta(root: &Path) -> Result<ProjectMeta, ProjectError>          // missing file => default
pub fn load_variables(root: &Path) -> Result<Variables, ProjectError>       // missing file => empty; bad names/fields => Parse/BadName
pub fn list_environments(root: &Path) -> Vec<String>                        // sorted valid stems; missing dir => empty
pub fn load_environment(root: &Path, name: &str) -> Result<IndexMap<String, String>, ProjectError>
pub fn resolve(vars: &Variables, env: Option<&IndexMap<String, String>>) -> IndexMap<String, String>
pub fn load_local_state(root: &Path) -> Result<LocalState, ProjectError>    // missing file => Ok(default)
pub fn save_local_state(root: &Path, state: &LocalState) -> std::io::Result<()>
pub fn init_project(root: &Path, name: Option<&str>) -> std::io::Result<()> // idempotent create/upgrade
```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn init_project_is_idempotent_and_never_overwrites() {
        let dir = tempdir().unwrap();
        init_project(dir.path(), Some("My API")).unwrap();
        assert!(dir.path().join("project.toml").is_file());
        assert!(dir.path().join("requests").is_dir());
        assert!(dir.path().join("environments").is_dir());
        assert!(dir.path().join("variables.toml").is_file());
        let gi = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gi.contains("/.local/"));

        // user edits survive a second init
        std::fs::write(dir.path().join("project.toml"), "name = \"edited\"\n").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "custom\n").unwrap();
        init_project(dir.path(), Some("My API")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("project.toml")).unwrap(), "name = \"edited\"\n");
        assert_eq!(std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(), "custom\n");
        assert!(is_project(dir.path()));
    }

    #[test]
    fn meta_defaults_and_display_name_fall_back_to_dir_basename() {
        let dir = tempdir().unwrap();
        let meta = load_meta(dir.path()).unwrap(); // no project.toml at all
        assert!(meta.name.is_none() && meta.default_headers.is_empty());
        let base = dir.path().file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(display_name(dir.path(), &meta), base);
        std::fs::write(
            dir.path().join("project.toml"),
            "name = \"svc\"\n[default_headers]\naccept = \"application/json\"\nx = { value = \"1\", enabled = false }\n",
        )
        .unwrap();
        let meta = load_meta(dir.path()).unwrap();
        assert_eq!(display_name(dir.path(), &meta), "svc");
        assert_eq!(meta.default_headers["accept"].value, "application/json");
        assert!(!meta.default_headers["x"].enabled);
    }

    #[test]
    fn variables_parse_validate_names_and_reject_unknown_fields() {
        let dir = tempdir().unwrap();
        assert!(load_variables(dir.path()).unwrap().is_empty(), "missing file is empty");
        std::fs::write(
            dir.path().join("variables.toml"),
            "[base_url]\ndescription = \"root\"\ndefault = \"http://l\"\n\n[token]\n",
        )
        .unwrap();
        let vars = load_variables(dir.path()).unwrap();
        assert_eq!(vars["base_url"].default.as_deref(), Some("http://l"));
        assert!(vars["token"].default.is_none());

        std::fs::write(dir.path().join("variables.toml"), "[\"bad name\"]\n").unwrap();
        assert!(matches!(load_variables(dir.path()), Err(ProjectError::BadName(_))));
        std::fs::write(dir.path().join("variables.toml"), "[a]\nbogus = 1\n").unwrap();
        assert!(matches!(load_variables(dir.path()), Err(ProjectError::Parse(_))));
    }

    #[test]
    fn environments_list_load_and_resolve_with_env_over_default() {
        let dir = tempdir().unwrap();
        assert!(list_environments(dir.path()).is_empty());
        std::fs::create_dir_all(dir.path().join("environments")).unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "token = \"qa-tok\"\nextra = \"e\"\n").unwrap();
        std::fs::write(dir.path().join("environments/prod.toml"), "token = \"prod-tok\"\n").unwrap();
        std::fs::write(dir.path().join("environments/Bad Name.toml"), "").unwrap();
        assert_eq!(list_environments(dir.path()), vec!["prod".to_string(), "qa".to_string()]);

        let mut vars: Variables = Variables::new();
        vars.insert("base".into(), VarDecl { description: None, default: Some("http://l".into()) });
        vars.insert("token".into(), VarDecl { description: None, default: None });
        let env = load_environment(dir.path(), "qa").unwrap();
        let r = resolve(&vars, Some(&env));
        assert_eq!(r["base"], "http://l", "default used when env has no value");
        assert_eq!(r["token"], "qa-tok", "env value wins");
        assert_eq!(r["extra"], "e", "undeclared env value still resolves (lenient)");
        let r = resolve(&vars, None);
        assert_eq!(r.get("token"), None, "no env: only defaults resolve");
    }

    #[test]
    fn local_state_round_trips_and_missing_is_default() {
        let dir = tempdir().unwrap();
        let s = load_local_state(dir.path()).unwrap();
        assert!(s.environment.is_none() && s.open_request.is_none() && s.expanded.is_empty());
        let state = LocalState {
            environment: Some("qa".into()),
            open_request: Some("users/list".into()),
            expanded: vec!["users".into()],
        };
        save_local_state(dir.path(), &state).unwrap();
        assert_eq!(load_local_state(dir.path()).unwrap(), state);
        std::fs::write(dir.path().join(".local/state.toml"), "environment = 3\n").unwrap();
        assert!(load_local_state(dir.path()).is_err(), "corrupt state is an Err the app degrades from");
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p postui-core project` → compile error.

- [ ] **Step 3: Implement**

Notes for the implementation (write real code, this is the shape):

```rust
use crate::model::Entry;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMeta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub default_headers: IndexMap<String, Entry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarDecl {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

pub type Variables = IndexMap<String, VarDecl>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalState {
    pub environment: Option<String>,
    pub open_request: Option<String>,
    pub expanded: Vec<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum ProjectError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Parse(String),
    #[error("invalid name: {0}")]
    BadName(String),
}
```

- `load_meta`: read `root/project.toml`; `ErrorKind::NotFound` → `Ok(ProjectMeta::default())`; parse error → `ProjectError::Parse(e.to_string())`.
- `load_variables`: same missing-file rule; parse as `IndexMap<String, VarDecl>` (via `toml::from_str`); then every key must pass `crate::vars::is_valid_var_name`, else `BadName(key)`.
- `list_environments`: read `root/environments`, collect `.toml` stems where the stem is a valid single-segment slug (`!s.contains('/') && crate::storage::validate_slug(s).is_ok()`), sort. Missing dir → empty.
- `load_environment`: parse the file as `IndexMap<String, String>`; parse error → `Parse`.
- `resolve`: start empty; insert every decl's `default` (if any); then overlay every env pair (declared or not).
- `load_local_state` / `save_local_state`: `root/.local/state.toml` via `toml::from_str` / `toml::to_string` (create `.local/` on save). This file is machine-owned, so derive-serde formatting is fine here (the model.rs hand-writer warning applies to *user*-owned files).
- `init_project`: `create_dir_all` for `requests/` and `environments/`; write `project.toml` (`name = "<name>"\n` when given, empty comment header otherwise), `variables.toml` (`# Declare variables: [name] with optional description/default\n`), `.gitignore` (`/.local/\n`) — **each only if the file does not already exist**.
- `is_project`: `root.join("project.toml").is_file()`.
- `display_name`: `meta.name` else `root.file_name()` lossy else `"project"`.

- [ ] **Step 4: Run** — `cargo test -p postui-core` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Add core project module: meta, variables, environments, local state, init"
```

---

### Task 4: `prepare()` with context (default headers + substitution + unresolved error)

**Files:**
- Modify: `crates/postui-core/src/prepare.rs`
- Modify: `crates/postui/src/app.rs` (ForceSend call site only, minimal)

**Interfaces:**
- Consumes: `vars::substitute`, `model::Entry`.
- Produces:

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrepareContext {
    pub vars: IndexMap<String, String>,          // fully resolved (env over default)
    pub default_headers: IndexMap<String, Entry>, // project.toml defaults
}
#[derive(Debug, Clone, PartialEq)]
pub enum PrepareError { Unresolved(std::collections::BTreeSet<String>) }
// Display: "unresolved variables: a, b"
pub fn prepare(req: &HttpRequest, ctx: &PrepareContext)
    -> Result<(PreparedRequest, Vec<PrepareWarning>), PrepareError>
```

- [ ] **Step 1: Write the failing tests** (extend prepare.rs tests; keep every existing test, updated to call `prepare(&r, &PrepareContext::default())` and unwrap the Ok)

```rust
fn ctx(vars: &[(&str, &str)], defaults: &[(&str, &str, bool)]) -> PrepareContext {
    PrepareContext {
        vars: vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        default_headers: defaults
            .iter()
            .map(|(k, v, en)| (k.to_string(), Entry { value: v.to_string(), enabled: *en }))
            .collect(),
    }
}

#[test]
fn substitutes_url_params_and_headers() {
    let mut r = base("{{base}}/users");
    r.params.insert("{{pkey}}".into(), on("{{pval}}"));
    r.headers.insert("x-{{h}}".into(), on("{{hv}}"));
    let c = ctx(&[("base", "http://x.test"), ("pkey", "id"), ("pval", "7"), ("h", "trace"), ("hv", "on")], &[]);
    let (p, _) = prepare(&r, &c).unwrap();
    assert_eq!(p.url, "http://x.test/users?id=7");
    assert!(p.headers.contains(&("x-trace".into(), "on".into())));
}

#[test]
fn body_substitution_is_opt_in() {
    let mut r = base("http://x.test");
    r.body = Some(Body::Json { text: r#"{"t": "{{tok}}"}"#.into() });
    let c = ctx(&[("tok", "abc")], &[]);
    let (p, _) = prepare(&r, &c).unwrap();
    assert_eq!(p.body.as_deref(), Some(r#"{"t": "{{tok}}"}"#), "flag off: literal braces");
    r.substitute_body = true;
    let (p, _) = prepare(&r, &c).unwrap();
    assert_eq!(p.body.as_deref(), Some(r#"{"t": "abc"}"#));
}

#[test]
fn unresolved_variables_error_and_body_tokens_only_count_when_opted_in() {
    let mut r = base("http://x.test/{{gone}}");
    r.body = Some(Body::Json { text: "{{also_gone}}".into() });
    let err = prepare(&r, &PrepareContext::default()).unwrap_err();
    let PrepareError::Unresolved(names) = err;
    assert_eq!(names.into_iter().collect::<Vec<_>>(), vec!["gone".to_string()], "body ignored while flag off");
    r.substitute_body = true;
    let PrepareError::Unresolved(names) = prepare(&r, &PrepareContext::default()).unwrap_err();
    assert_eq!(names.len(), 2);
}

#[test]
fn default_headers_merge_override_and_suppress() {
    let mut r = base("http://x.test");
    r.headers.insert("Accept".into(), on("text/plain"));          // overrides (case-insensitive)
    r.headers.insert("X-Trace".into(), off("ignored"));           // disabled row suppresses inherited
    let c = ctx(&[], &[("accept", "application/json", true), ("x-trace", "1", true), ("x-off", "0", false)]);
    let (p, _) = prepare(&r, &c).unwrap();
    assert_eq!(p.headers, vec![("Accept".to_string(), "text/plain".to_string())],
        "override kept in request position; suppressed + disabled defaults dropped");
}

#[test]
fn default_header_values_are_substituted_too() {
    let r = base("http://x.test");
    let c = ctx(&[("tok", "abc")], &[("authorization", "Bearer {{tok}}", true)]);
    let (p, _) = prepare(&r, &c).unwrap();
    assert!(p.headers.contains(&("authorization".into(), "Bearer abc".into())));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p postui-core prepare` → compile errors.

- [ ] **Step 3: Implement**

Order of operations inside `prepare`:

1. `let mut missing = BTreeSet::new();` and a closure `let mut sub = |s: &str| crate::vars::substitute(s, &ctx.vars, &mut missing);`
2. Substitute the URL, then run the existing param merge using **substituted** param keys and values (enabled entries only).
3. Merge headers: start from `ctx.default_headers` entries that are `enabled` **and** have no request row (any enabled state) with a case-insensitively equal name; then append the request's enabled rows (existing behavior). Substitute every resulting name and value.
4. Body: clone as today; if `req.substitute_body`, substitute it.
5. `if !missing.is_empty() { return Err(PrepareError::Unresolved(missing)); }`
6. Content-Type auto-add unchanged (runs on the merged, substituted header list).
7. `Ok((PreparedRequest { .. }, warnings))`

`Display for PrepareError`: `write!(f, "unresolved variables: {}", names.iter().cloned().collect::<Vec<_>>().join(", "))` plus `impl std::error::Error`.

In `app.rs` `ForceSend`, minimally:

```rust
let (prepared, warnings) =
    match postui_core::prepare::prepare(&self.editor.current_request(), &Default::default()) {
        Ok(x) => x,
        Err(e) => {
            self.toasts.push(e.to_string(), ToastKind::Error);
            return true;
        }
    };
```

(The real `PrepareContext` is wired in Task 13.)

- [ ] **Step 4: Run workspace** — `cargo test --workspace` → PASS (fix any straggler call sites).

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "prepare(): context with default headers, {{var}} substitution, unresolved error"
```

---

### Task 5: Global project registry (`postui/src/config.rs`)

**Files:**
- Create: `crates/postui/src/config.rs`
- Modify: `crates/postui/src/lib.rs` (add `pub mod config;`)

**Interfaces:**
- Produces:

```rust
pub struct ProjectsRegistry { pub known: Vec<PathBuf>, pub root: Option<PathBuf>, pub last: Option<PathBuf> }
impl ProjectsRegistry {
    pub fn load_from(path: &Path) -> Self;                 // missing/corrupt file => default (never errors)
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()>; // toml_edit round-trip: only [projects] is touched
    pub fn register(&mut self, path: PathBuf);             // dedup, push to end, set last
    pub fn default_root(&self) -> PathBuf;                 // self.root or ~/postui-projects
    pub fn next_after(&self, current: &Path) -> Option<PathBuf>; // cycle order, wrapping; None if <2 known
}
pub fn config_file_path() -> Option<PathBuf>;              // <config dir>/config.toml via directories
pub fn expand_tilde(s: &str) -> PathBuf;                   // leading ~/ -> home dir
```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn load_missing_or_corrupt_is_default() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let r = ProjectsRegistry::load_from(&p);
        assert!(r.known.is_empty() && r.last.is_none());
        std::fs::write(&p, "projects = 5\n").unwrap();
        let r = ProjectsRegistry::load_from(&p);
        assert!(r.known.is_empty(), "corrupt config degrades to default");
    }

    #[test]
    fn save_round_trips_and_preserves_unrelated_keys() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"dark\"\n").unwrap();
        let mut r = ProjectsRegistry::load_from(&p);
        r.register(PathBuf::from("/tmp/a"));
        r.register(PathBuf::from("/tmp/b"));
        r.register(PathBuf::from("/tmp/a")); // dedup, but last updates
        r.root = Some(PathBuf::from("/tmp/root"));
        r.save_to(&p).unwrap();

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("theme = \"dark\""), "unrelated key preserved: {text}");
        let r2 = ProjectsRegistry::load_from(&p);
        assert_eq!(r2.known, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
        assert_eq!(r2.last, Some(PathBuf::from("/tmp/a")));
        assert_eq!(r2.root, Some(PathBuf::from("/tmp/root")));
    }

    #[test]
    fn next_after_cycles_and_wraps() {
        let mut r = ProjectsRegistry::load_from(&PathBuf::from("/nonexistent"));
        assert!(r.next_after(&PathBuf::from("/tmp/a")).is_none(), "fewer than two projects");
        r.register(PathBuf::from("/tmp/a"));
        r.register(PathBuf::from("/tmp/b"));
        r.register(PathBuf::from("/tmp/c"));
        assert_eq!(r.next_after(&PathBuf::from("/tmp/b")), Some(PathBuf::from("/tmp/c")));
        assert_eq!(r.next_after(&PathBuf::from("/tmp/c")), Some(PathBuf::from("/tmp/a")), "wraps");
        assert_eq!(r.next_after(&PathBuf::from("/elsewhere")), Some(PathBuf::from("/tmp/a")),
            "unknown current starts from the top");
    }

    #[test]
    fn tilde_expansion() {
        let home = directories::BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(expand_tilde("~/x/y"), home.join("x/y"));
        assert_eq!(expand_tilde("/abs/x"), PathBuf::from("/abs/x"));
        assert_eq!(expand_tilde("rel"), PathBuf::from("rel"));
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p postui config` → compile error.

- [ ] **Step 3: Implement**

- `load_from`: read + `toml::from_str::<toml::Value>`; pull `["projects"]["known"/"root"/"last"]` string values through `expand_tilde`; any missing/mistyped piece just yields defaults for that piece.
- `save_to`: read existing file into `toml_edit::DocumentMut` (empty doc if missing/corrupt), rewrite only the `projects` table (`known` as an array of strings, `root`/`last` as strings when Some, removed when None), create parent dir, write.
- `register`: if not already in `known`, push; always `self.last = Some(path)`.
- `default_root`: `self.root.clone()` else home dir join `"postui-projects"` (fall back to `.` if no home).
- `next_after`: position of `current` in `known` (else index 0 minus one so the first entry comes next); `None` when `known.len() < 2`; otherwise `known[(i + 1) % len]`.
- `config_file_path`: `directories::ProjectDirs::from("", "", postui_core::APP_NAME).map(|d| d.config_dir().join("config.toml"))`.

- [ ] **Step 4: Run** — `cargo test -p postui config` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Add global project registry (config.toml [projects] table)"
```

---

### Task 6: `ProjectContext`, startup/migration, CLI arg, header display

**Files:**
- Create: `crates/postui/src/project_ctx.rs`
- Modify: `crates/postui/src/lib.rs` (add module), `crates/postui/src/app.rs`, `crates/postui/src/main.rs`, `crates/postui/src/components/header_bar.rs`, `crates/postui/src/ui.rs`

**Interfaces:**
- Consumes: Task 3 (`postui_core::project::*`), Task 5 (`config::*`).
- Produces:

```rust
pub struct ProjectContext {
    pub root: PathBuf,
    pub meta: ProjectMeta,
    pub variables: Variables,
    pub environments: Vec<String>,
    pub active_env: Option<String>,
    pub env_values: IndexMap<String, String>,
    pub expanded: std::collections::BTreeSet<String>,
    stamps: Vec<(PathBuf, Option<std::time::SystemTime>)>, // for Task 12
}
impl ProjectContext {
    pub fn open(root: PathBuf) -> (Self, Vec<String>);   // warnings, never fails; restores env/expanded from local state
    pub fn display_name(&self) -> String;
    pub fn env_label(&self) -> String;                    // active env name or "no env"
    pub fn prepare_context(&self) -> postui_core::prepare::PrepareContext;
    pub fn set_env(&mut self, env: Option<String>) -> Vec<String>; // loads values, persists local state
    pub fn persist_local_state(&self, open_request: Option<&str>); // best-effort save
    pub fn local_open_request(&self) -> Option<String>;   // from the state loaded at open()
}
```
- `App.project: ProjectContext` replaces `App.project_root: PathBuf`; `App::with_root(tx, root)` keeps its signature (tests depend on it). `App::new(tx, cli_root: Option<PathBuf>)` gains the arg.
- Header: `draw_header(frame, area, theme, project: &str, env: &str)`.

- [ ] **Step 1: Write the failing tests** (project_ctx.rs + app.rs)

```rust
// project_ctx.rs
#[test]
fn open_bare_dir_defaults_and_open_project_restores_state() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
    assert!(warns.is_empty());
    assert_eq!(ctx.env_label(), "no env");
    assert!(ctx.environments.is_empty());

    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"t\"\n").unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState {
            environment: Some("qa".into()),
            open_request: Some("ping".into()),
            expanded: vec!["users".into()],
        },
    )
    .unwrap();
    let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
    assert!(warns.is_empty());
    assert_eq!(ctx.display_name(), "svc");
    assert_eq!(ctx.env_label(), "qa");
    assert_eq!(ctx.env_values["tok"], "t");
    assert!(ctx.expanded.contains("users"));
    assert_eq!(ctx.local_open_request().as_deref(), Some("ping"));
}

#[test]
fn stale_env_in_local_state_degrades_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), None).unwrap();
    postui_core::project::save_local_state(
        dir.path(),
        &postui_core::project::LocalState { environment: Some("gone".into()), ..Default::default() },
    )
    .unwrap();
    let (ctx, warns) = ProjectContext::open(dir.path().to_path_buf());
    assert_eq!(ctx.env_label(), "no env");
    assert!(!warns.is_empty(), "stale env must be surfaced");
}

// ui.rs full-frame test: update the header assertions
assert!(content.contains("no env"));   // replaces "No environment"
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p postui` → compile errors.

- [ ] **Step 3: Implement**

- `ProjectContext::open`: `load_meta`/`load_variables`/`list_environments`, `load_local_state` (Err → default + warning). If the stored env is not in `environments`, drop it with a warning. `set_env`-style value loading inline for the initial env (env load Err → warning + `no env`). `stamps` recorded but only consumed in Task 12 (populate with project.toml, variables.toml, environments dir, active env file mtimes).
- `prepare_context()`: `PrepareContext { vars: postui_core::project::resolve(&self.variables, self.active_env.as_ref().map(|_| &self.env_values).map(|v| v as _)), default_headers: self.meta.default_headers.clone() }` — careful: `resolve` takes `Option<&IndexMap<..>>`; pass `self.active_env.is_some().then_some(&self.env_values)`.
- `persist_local_state`: build `LocalState { environment, open_request, expanded: sorted vec }`, `let _ = save_local_state(..)` (best-effort; a failed save must never break interaction).
- `App`: replace `project_root: PathBuf` with `project: ProjectContext`. `App::bare` takes root and builds `ProjectContext::open` (push its warnings as toasts). Every `self.project_root` becomes `self.project.root` (mechanical; `&app.project.root` in tests). `with_root` behavior otherwise unchanged (still `ensure_project` + sidebar refresh).
- **Startup** (`App::new(tx, cli_root)`): load registry from `config_file_path()`. Migration: if `default_project_dir()` exists on disk, `init_project(&it, Some("default"))` and `register` it (save registry). Target root = `cli_root` (expanded) `.or(registry.last)` `.or(first existing known)` `.or(default_project_dir())`. If the chosen CLI root is not a project (`!is_project`), open it anyway and push a Confirm modal: title "Not a postui project", body "<path> has no project.toml — create one here?", choices `('y', "Create project", vec![Action::InitProjectHere])` / `('n', "Open default project", vec![Action::SwitchProject(<fallback root>)])`. Add `Action::InitProjectHere` (runs `init_project(&self.project.root, None)`, registers + saves, refreshes). `Action::SwitchProject` arrives in Task 9 — for THIS task, have the 'n' choice carry `vec![]` and a TODO-free comment noting Task 9 replaces it; the modal still closes and the bare dir stays open (harmless: bare dirs work).
- `App` keeps a `pub registry: crate::config::ProjectsRegistry` field and `registry_path: Option<PathBuf>` (None in tests → saves skipped) so later tasks can save it. `new_for_test()` keeps a default registry with `registry_path: None`.
- `main.rs`: `let cli_root = std::env::args().nth(1).map(|s| postui::config::expand_tilde(&s));` and `App::new(tx, cli_root)`.
- Header: `draw_header(frame, area, theme, project, env)` renders `" postui "` badge, then `Span::styled(project, ..text)` + `Span::styled(" · ", muted)` + env (muted italic when "no env", normal otherwise). `ui.rs` passes `&app.project.display_name()`, `&app.project.env_label()`; update the full-frame test.

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "App holds ProjectContext; registry startup, default-project migration, CLI arg, header shows project - env"
```

---

### Task 7: Sidebar tree (expand/collapse, free wheel scroll)

**Files:**
- Modify: `crates/postui/src/components/sidebar.rs`, `crates/postui/src/app.rs`, `crates/postui/src/action.rs`

**Interfaces:**
- Consumes: `App.project.expanded` (BTreeSet), Task 6.
- Produces:

```rust
pub enum Row {
    Folder { path: String, name: String, depth: usize, expanded: bool },
    Request { slug: String, depth: usize, broken: Option<String> },
}
impl Sidebar {
    pub fn refresh(&mut self, listing: Vec<RequestListing>, expanded: &BTreeSet<String>); // rebuilds visible rows
    pub fn toggle_selected_folder(&mut self) -> Option<(String, bool)>; // (path, now_expanded)
    // selected_slug/select_slug keep their signatures; select_slug auto-expands ancestors
}
```
- New `Action::PersistLocalState` (App writes `.local/state.toml` from current expanded/env/open request).
- App helper `fn refresh_sidebar(&mut self)` replacing every `list_requests` + `sidebar.refresh` pair.

- [ ] **Step 1: Write the failing tests**

```rust
fn listing(slugs: &[&str]) -> Vec<RequestListing> { /* as before */ }
fn expanded(paths: &[&str]) -> std::collections::BTreeSet<String> {
    paths.iter().map(|s| s.to_string()).collect()
}

#[test]
fn tree_builds_nested_folders_and_hides_collapsed_children() {
    let mut s = Sidebar::default();
    s.refresh(listing(&["api/users/list", "api/users/create", "api/ping", "top"]), &expanded(&[]));
    // collapsed: only top-level rows visible
    assert_eq!(
        s.rows,
        vec![
            Row::Request { slug: "top".into(), depth: 0, broken: None },
            Row::Folder { path: "api".into(), name: "api".into(), depth: 0, expanded: false },
        ]
    );
    s.refresh(listing(&["api/users/list", "api/users/create", "api/ping", "top"]), &expanded(&["api"]));
    assert_eq!(
        s.rows,
        vec![
            Row::Request { slug: "top".into(), depth: 0, broken: None },
            Row::Folder { path: "api".into(), name: "api".into(), depth: 0, expanded: true },
            Row::Request { slug: "api/ping".into(), depth: 1, broken: None },
            Row::Folder { path: "api/users".into(), name: "users".into(), depth: 1, expanded: false },
        ]
    );
}

#[test]
fn enter_and_arrows_toggle_folders_and_navigate_all_rows() {
    let mut s = Sidebar::default();
    s.refresh(listing(&["api/ping", "top"]), &expanded(&[]));
    s.handle_key(key(KeyCode::Char('j'))); // now on the "api" folder row
    assert!(matches!(s.rows[s.selected], Row::Folder { .. }));
    assert_eq!(s.selected_slug(), None, "folder rows have no slug");
    // Enter on a folder emits PersistLocalState after toggling
    assert_eq!(s.handle_key(key(KeyCode::Enter)), Some(Action::ToggleSelectedFolder));
}

#[test]
fn wheel_scroll_is_free_and_keyboard_still_tracks_selection() {
    let mut s = Sidebar::default();
    let slugs: Vec<String> = (0..30).map(|i| format!("r{i:02}")).collect();
    let refs: Vec<&str> = slugs.iter().map(|s| s.as_str()).collect();
    s.refresh(listing(&refs), &expanded(&[]));
    s.handle_scroll(10);
    assert_eq!(s.scroll, 10);
    // drawing must NOT snap back to the selection
    let theme = Theme::dark();
    let ctx = DrawCtx { theme: &theme, focused: true };
    let backend = ratatui::backend::TestBackend::new(30, 10);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| s.draw(f, f.area(), &ctx)).unwrap();
    assert_eq!(s.scroll, 10, "free scroll survives draw");
    // moving the selection scrolls it back into view
    s.handle_key(key(KeyCode::Char('j')));
    terminal.draw(|f| s.draw(f, f.area(), &ctx)).unwrap();
    assert!(s.scroll <= 1, "keyboard nav brings the selection into view: {}", s.scroll);
}

#[test]
fn select_slug_expands_ancestor_folders() {
    let mut s = Sidebar::default();
    s.refresh(listing(&["a/b/c"]), &expanded(&[]));
    s.select_slug("a/b/c");
    assert!(s.pending_expand.contains("a") && s.pending_expand.contains("a/b"));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p postui sidebar` → compile errors.

- [ ] **Step 3: Implement**

- `Sidebar` keeps `listing: Vec<RequestListing>` + `pub pending_expand: BTreeSet<String>` (paths select_slug needs opened; App merges them into `project.expanded` and re-refreshes). `refresh(listing, expanded)` stores the listing and rebuilds `rows`:
  - Derive the folder set from slugs (`a/b/c` contributes `a`, `a/b`).
  - Walk sorted slugs recursively per directory level: at each level list this level's requests first, then subfolders (matches the existing top-before-dirs ordering); a folder row's children are emitted only when `expanded.contains(path)`.
  - Preserve selection by slug as today, else clamp; `selected` may land on folder rows now.
- Navigation: `move_selection` moves over ALL rows (folders selectable); drop `request_indices`. `'r'`/`'d'`/Enter guards already pattern-match `Row::Request`. Enter on `Row::Folder` returns new `Action::ToggleSelectedFolder`; Right on collapsed folder → same; Left on expanded folder → same; Left elsewhere → jump selection to the parent folder row if any (pure state change, `Action::Render`).
- New actions in `action.rs`: `ToggleSelectedFolder`, `PersistLocalState`. In `app.rs`:

```rust
Action::ToggleSelectedFolder => {
    if let Some((path, now_open)) = self.sidebar.toggle_selected_folder() {
        if now_open { self.project.expanded.insert(path); } else { self.project.expanded.remove(&path); }
        self.refresh_sidebar();
        self.apply(Action::PersistLocalState);
    }
    true
}
Action::PersistLocalState => {
    self.project.persist_local_state(self.editor.slug.as_deref());
    true
}
```

Also make `Action::Quit` persist on the way out (spec: state written on change **and on quit**):

```rust
Action::Quit => {
    self.project.persist_local_state(self.editor.slug.as_deref());
    self.should_quit = true;
    true
}
```

  `toggle_selected_folder` flips nothing itself — it reports the selected folder path and the desired new state (`!expanded`); App owns the set. After `refresh_sidebar`, re-`select` the folder path row (store selection by row identity: keep it simple — remember the folder path and reselect its row).
- **Free scroll:** `handle_scroll` unchanged; in `draw`, wrap the old ensure-visible block in `if self.ensure_visible { .. ; self.ensure_visible = false; }` where `ensure_visible: bool` is set by `move_selection`, `select_slug`, and `refresh` — never by `handle_scroll`.
- `refresh_sidebar` App helper: `let listing = postui_core::storage::list_requests(&self.project.root); let mut expanded = self.project.expanded.clone(); /* merge sidebar.pending_expand into project.expanded first */ self.sidebar.refresh(listing, &expanded);` — replace all five existing call sites (`with_root`, SaveRequest, RefreshSidebar, RenameRequest, DeleteRequest, create_or_save_as).
- `select_slug(slug)`: record needed ancestor paths in `pending_expand`; App's `create_or_save_as` (which calls it) follows with `refresh_sidebar()` + `PersistLocalState` so new `a/b/x` requests are visible immediately.
- Rendering: folder rows `▸ name/` collapsed, `▾ name/` expanded, indented two spaces per depth, muted; selected folder row gets the accent marker treatment requests have. Request rows indent by depth (replaces the old fixed two-space nested indent).
- Update the two app.rs tests that assert the old flat `rows` shape (`sidebar_lists_requests_grouped_and_enter_opens`, `broken_file_shows_marker_and_error_modal`) to the tree shape (folders start collapsed, so `auth/login` needs `expanded` seeded or an Enter on the folder row first — drive it through keys: `j` to folder, Enter to expand, `j`, Enter to open).

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Sidebar: real collapsible tree, free wheel scroll, expansion persisted"
```

---

### Task 8: Generic fuzzy chooser modal

**Files:**
- Create: `crates/postui/src/components/chooser.rs`
- Modify: `crates/postui/src/components/mod.rs`, `crates/postui/src/components/modal.rs`

**Interfaces:**
- Consumes: `palette::fuzzy_match`, `modal::ModalResult`.
- Produces:

```rust
pub struct ChooserItem { pub label: String, pub detail: Option<String>, pub actions: Vec<Action> }
pub struct ChooserState { /* title, input, selected, items, filtered indices */ }
impl ChooserState {
    pub fn new(title: &str, items: Vec<ChooserItem>) -> Self;
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ModalResult>;  // palette-identical semantics
    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme);
    pub fn input(&self) -> &str;
    pub fn selected_label(&self) -> Option<&str>;
}
```
- `Modal::Chooser(ChooserState)` variant; `ModalStack::handle_key`/`draw` delegate exactly like `Modal::Palette`.

- [ ] **Step 1: Write the failing tests**

```rust
fn items(labels: &[&str]) -> Vec<ChooserItem> {
    labels.iter()
        .map(|l| ChooserItem { label: l.to_string(), detail: None, actions: vec![Action::Render] })
        .collect()
}

#[test]
fn typing_filters_on_label_and_detail_and_enter_returns_actions() {
    let mut c = ChooserState::new("Projects", vec![
        ChooserItem { label: "svc".into(), detail: Some("/tmp/svc".into()), actions: vec![Action::Quit] },
        ChooserItem { label: "web".into(), detail: Some("/tmp/web".into()), actions: vec![Action::Render] },
    ]);
    for ch in "tmp/w".chars() { c.handle_key(key(KeyCode::Char(ch))); }
    assert_eq!(c.selected_label(), Some("web"), "detail participates in the fuzzy match");
    let res = c.handle_key(key(KeyCode::Enter)).unwrap();
    assert!(res.close);
    assert_eq!(res.actions, vec![Action::Render]);
}

#[test]
fn esc_closes_empty_enter_swallowed_arrows_clamp() {
    let mut c = ChooserState::new("t", items(&["a", "b"]));
    c.handle_key(key(KeyCode::Up));
    c.handle_key(key(KeyCode::Down));
    c.handle_key(key(KeyCode::Down)); // clamped at 1
    assert_eq!(c.selected_label(), Some("b"));
    for ch in "zz".chars() { c.handle_key(key(KeyCode::Char(ch))); }
    assert!(c.handle_key(key(KeyCode::Enter)).is_none(), "no match: Enter swallowed");
    let res = c.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(res.close && res.actions.is_empty());
}

#[test]
fn draw_renders_title_labels_and_dim_details() {
    // TestBackend render; assert title, both labels, and a detail string appear.
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** — copy `PaletteState`'s structure (input/selected/refilter/draw) with `String` labels, matching against `label + " " + detail`, rendering `label` normally and `detail` as a muted span after it. Modal plumbing mirrors `Modal::Palette` in both `handle_key` and `draw` (including the modal-swallows-keys `None` returns). Sizing: width 60, height clamp like palette.

- [ ] **Step 4: Run** — `cargo test -p postui chooser` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Add generic fuzzy chooser modal"
```

---

### Task 9: Project switching (chooser, cycle, open-by-path, dirty gate)

**Files:**
- Modify: `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/keys.rs`, `crates/postui/src/components/palette.rs`, `crates/postui/src/components/modal.rs` (PromptKind)

**Interfaces:**
- Consumes: Tasks 5–8.
- Produces actions:
  - `OpenProjectChooser` — builds `ChooserState` from `registry.known` (label = each project's display name via `load_meta` best-effort, detail = path string; skip-with-toast paths that no longer exist) + final item "open by path…" → `vec![Action::PromptOpenProjectPath]`. Each project item → `vec![Action::SwitchProject(path)]`.
  - `CycleProject` — `registry.next_after(&self.project.root)`; None → toast "only one project registered"; Some → `SwitchProject` + success toast with the target name.
  - `SwitchProject(PathBuf)` — dirty gate (same Confirm shape as `OpenRequest`: "Save & switch" `[SaveRequest, ForceSwitchProject(p)]` / "Discard changes" `[ForceSwitchProject(p)]`), else applies `ForceSwitchProject` directly. No-op (`false`) when the target equals the current root.
  - `ForceSwitchProject(PathBuf)` — persist old local state; `ProjectContext::open(target)` (+ warning toasts); `ensure_project`; `refresh_sidebar`; restore `local_open_request()` via `ForceOpenRequest` if the file still loads, else `Editor::default()`; `registry.register(target)`; save registry (when `registry_path` is Some).
  - `PromptOpenProjectPath` — `Modal::Prompt { kind: PromptKind::OpenProjectPath, .. }`; new `PromptKind::OpenProjectPath` maps Enter-text to `Action::OpenProjectByPath(text)`.
  - `OpenProjectByPath(String)` — expand tilde; if `is_project` → `SwitchProject`; else Confirm "create project at <path>?" → `('y', "Create", vec![Action::CreateProjectAt(path)])`; `CreateProjectAt(PathBuf)` runs `init_project(path, None)` (toast on Err) then `ForceSwitchProject`. Replace Task 6's placeholder 'n' choice for the CLI bare-dir modal with `Action::SwitchProject(fallback)` now that it exists.
- Keymap: `("ctrl+o", OpenProjectChooser)`, `("alt+o", CycleProject)` defaults; named actions `project_choose`, `project_cycle`. Palette commands: "Project: choose…", "Project: next", "Project: open by path…".

- [ ] **Step 1: Write the failing tests** (app.rs)

```rust
fn two_projects() -> (App, tempfile::TempDir, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    postui_core::project::init_project(a.path(), Some("alpha")).unwrap();
    postui_core::project::init_project(b.path(), Some("beta")).unwrap();
    postui_core::storage::ensure_project(b.path()).unwrap();
    postui_core::storage::save_request(b.path(), "pong", &req("https://x/pong")).unwrap();
    let mut app = App::with_root(tx, a.path().to_path_buf());
    app.registry.register(a.path().to_path_buf());
    app.registry.register(b.path().to_path_buf());
    (app, a, b)
}

#[test]
fn cycle_switches_to_next_project_and_lists_its_requests() {
    let (mut app, _a, b) = two_projects();
    app.update(Action::CycleProject);
    assert_eq!(app.project.root, b.path());
    assert!(app.sidebar.rows.iter().any(|r| matches!(r, Row::Request { slug, .. } if slug == "pong")));
    assert_eq!(app.project.display_name(), "beta");
}

#[test]
fn switch_with_dirty_editor_prompts_and_discard_proceeds() {
    let (mut app, _a, b) = two_projects();
    postui_core::storage::save_request(&app.project.root, "r", &req("https://x/r")).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("r".into()));
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&Keymap::default_bindings(), plain('/'));
    assert!(app.editor.is_dirty());
    app.update(Action::SwitchProject(b.path().to_path_buf()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert_ne!(app.project.root, b.path(), "not switched yet");
    app.handle_key(&Keymap::default_bindings(), plain('d'));
    assert_eq!(app.project.root, b.path());
}

#[test]
fn switch_restores_target_projects_open_request_and_saves_state() {
    let (mut app, a, b) = two_projects();
    postui_core::project::save_local_state(
        b.path(),
        &postui_core::project::LocalState { open_request: Some("pong".into()), ..Default::default() },
    ).unwrap();
    app.update(Action::SwitchProject(b.path().to_path_buf()));
    assert_eq!(app.editor.slug.as_deref(), Some("pong"));
    // and the old project's state got written on the way out
    let old = postui_core::project::load_local_state(a.path()).unwrap();
    assert_eq!(old.open_request, None);
}

#[test]
fn project_chooser_lists_known_and_open_by_path_creates() {
    let (mut app, _a, _b) = two_projects();
    app.update(Action::OpenProjectChooser);
    let Some(Modal::Chooser(c)) = app.modals.top() else { panic!("expected chooser") };
    assert!(format!("{:?}", (c.input(), c.selected_label())).contains("alpha") || c.selected_label().is_some());
    app.update(Action::Close);
    let fresh = tempfile::tempdir().unwrap();
    let target = fresh.path().join("newproj");
    app.update(Action::OpenProjectByPath(target.to_string_lossy().into_owned()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "non-project path asks to create");
    app.handle_key(&Keymap::default_bindings(), plain('y'));
    assert!(postui_core::project::is_project(&target));
    assert_eq!(app.project.root, target);
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** — as specified in Interfaces. Extract the dirty gate into a helper and reuse it from `OpenRequest`:

```rust
/// Push the standard unsaved-changes confirm whose "save" path relies on
/// SaveRequest completing synchronously (dirty implies a slugged request).
fn dirty_gate(&mut self, verb: &str, then: Action) {
    let current = self.editor.slug.clone().unwrap_or_default();
    self.modals.push(Modal::Confirm {
        title: "Unsaved changes".into(),
        body: format!("\"{current}\" has unsaved changes."),
        choices: vec![
            ('s', format!("Save & {verb}"), vec![Action::SaveRequest, then.clone()]),
            ('d', "Discard changes".into(), vec![then]),
        ],
    });
}
```

`OpenRequest` calls `self.dirty_gate("open", Action::ForceOpenRequest(slug))`; `SwitchProject`/`CreateProjectAt` call it with "switch". Keep the existing invariant comment on the helper.

`keys.rs`: add to `named_actions()` and `default_bindings()`; extend `default_bindings_cover_core_actions` test. `palette.rs`: add the three commands; palette test counts adjust if any assert totals.

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Project switching: fuzzy chooser, cycle key, open-by-path, dirty gate"
```

---

### Task 10: New Project modal

**Files:**
- Modify: `crates/postui/src/components/modal.rs`, `crates/postui/src/app.rs`, `crates/postui/src/action.rs`, `crates/postui/src/keys.rs`, `crates/postui/src/components/palette.rs`

**Interfaces:**
- Produces: `Modal::NewProject { name: LineInput, path: LineInput, on_path: bool }`; actions `PromptNewProject` (opens it, path prefilled `registry.default_root().display() + "/"`), `CreateProject { name: String, path: String }` (expand tilde; `init_project(&path, Some(&name))`; toast on Err; register + save; `ForceSwitchProject` — via the dirty gate when dirty). Keybinding `alt+n` = `project_new`; palette "Project: new…".
- Modal behavior: Tab/Down switches name→path (BackTab/Up back); on the FIRST hop off the name field, if the path text still ends with `/`, append `slugify(name)` (`lowercase; spaces→'-'; keep [a-z0-9_-]`). Enter anywhere: name empty → swallow; else emit `CreateProject`. Esc closes.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn new_project_modal_prefills_path_from_name_and_creates() {
    let mut app = App::new_for_test();
    let root = tempfile::tempdir().unwrap();
    app.registry.root = Some(root.path().to_path_buf());
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewProject);
    for c in "My Svc".chars() { app.handle_key(&keymap, plain(c)); }
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let Some(Modal::NewProject { path, .. }) = app.modals.top() else { panic!() };
    assert!(path.text().ends_with("/my-svc"), "slugified prefill: {}", path.text());
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let expected = root.path().join("my-svc");
    assert!(postui_core::project::is_project(&expected));
    assert_eq!(app.project.root, expected);
    assert_eq!(app.project.display_name(), "My Svc");
    assert!(app.registry.known.contains(&expected));
}

#[test]
fn new_project_empty_name_swallows_enter_and_esc_cancels() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.update(Action::PromptNewProject);
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.modals.is_empty(), "empty name: modal stays");
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** — modal variant with two `LineInput`s and its own `handle_key` arm in `ModalStack::handle_key` (Tab/BackTab/Up/Down field switching with the one-shot prefill; other keys go to the focused input). Draw: 60×8 centered block titled " New project ", two labelled input lines (name focused ⇒ its caret shown), hint line `[tab] switch  [enter] create  [esc] cancel`. `slugify` as a free fn in app.rs (or modal.rs) with a unit test (`"My Svc" → "my-svc"`).

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "New Project modal: name + prefilled path, creates/registers/switches"
```

---

### Task 11: Environment switching (chooser + cycle + persistence)

**Files:**
- Modify: `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/keys.rs`, `crates/postui/src/components/palette.rs`, `crates/postui/src/project_ctx.rs`

**Interfaces:**
- Produces actions:
  - `OpenEnvChooser` — `ChooserState` "Environments": one item per `project.environments` (detail: none) → `vec![Action::SwitchEnv(Some(name))]`, plus final "no environment" item → `vec![Action::SwitchEnv(None)]`. Empty environments list → toast `no environments — create environments/<name>.toml in the project` instead of the modal.
  - `CycleEnv` — next in `project.environments` after the active one (wrapping; from `None` start at the first; **skips the no-env state**); empty list → same toast.
  - `SwitchEnv(Option<String>)` — `project.set_env(env)` (loads values; returns warnings to toast), `PersistLocalState`, success toast `env: <label>`.
- `ProjectContext::set_env` re-reads the env file, updates `active_env`/`env_values`/stamps, returns warnings (missing/corrupt file → keep previous env, warn).
- Keymap: `alt+e` = `env_choose`, `alt+c` = `env_cycle`; palette "Environment: choose…", "Environment: next".

- [ ] **Step 1: Write the failing tests**

```rust
fn app_with_envs() -> (App, tempfile::TempDir) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    std::fs::write(dir.path().join("environments/prod.toml"), "tok = \"p\"\n").unwrap();
    std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"q\"\n").unwrap();
    (App::with_root(tx, dir.path().to_path_buf()), dir)
}

#[test]
fn cycle_env_wraps_and_skips_no_env() {
    let (mut app, dir) = app_with_envs();
    assert_eq!(app.project.env_label(), "no env");
    app.update(Action::CycleEnv);
    assert_eq!(app.project.env_label(), "prod");
    app.update(Action::CycleEnv);
    assert_eq!(app.project.env_label(), "qa");
    app.update(Action::CycleEnv);
    assert_eq!(app.project.env_label(), "prod", "wraps directly, never through no-env");
    assert_eq!(app.project.env_values["tok"], "p");
    let st = postui_core::project::load_local_state(dir.path()).unwrap();
    assert_eq!(st.environment.as_deref(), Some("prod"), "persisted");
}

#[test]
fn env_chooser_includes_no_environment_entry() {
    let (mut app, _dir) = app_with_envs();
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.update(Action::OpenEnvChooser);
    let Some(Modal::Chooser(_)) = app.modals.top() else { panic!("expected chooser") };
    app.update(Action::Close);
    app.update(Action::SwitchEnv(None));
    assert_eq!(app.project.env_label(), "no env");
}

#[test]
fn cycle_env_with_no_environments_toasts() {
    let mut app = App::new_for_test();
    app.update(Action::CycleEnv);
    assert!(!app.toasts.is_empty());
    assert_eq!(app.project.env_label(), "no env");
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** as specified. `set_env(None)` clears `env_values`. Re-list `environments` at the top of `OpenEnvChooser`/`CycleEnv` (cheap `list_environments` call — this is part of on-demand pickup).

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Environment chooser + cycle with per-project persisted active env"
```

---

### Task 12: On-demand reload (mtimes + FocusGained + hooks)

**Files:**
- Modify: `crates/postui/src/project_ctx.rs`, `crates/postui/src/app.rs`, `crates/postui/src/action.rs`, `crates/postui/src/main.rs`

**Interfaces:**
- Produces: `ProjectContext::reload_if_changed(&mut self) -> (bool, Vec<String>)` — compares stored stamps (mtimes of `project.toml`, `variables.toml`, the `environments/` dir, and the active env file; a missing file stamps as `None`); on any difference re-runs the Task-6 load path **keeping the current `active_env` if it still exists** (else warn + no env) and re-stamps. Parse failures: warn and keep the previous good value for that file.
- `Action::ReloadProjectFiles` — `reload_if_changed`; on change also `refresh_sidebar()`; toast warnings. Dispatched from: `Event::FocusGained` in main.rs, the top of `ForceSend` (Task 13 wires the context; add the call here), and the top of `OpenEnvChooser`/`OpenProjectChooser`/`OpenVarPicker` (picker exists in Task 15 — wire the two choosers now, note the third lands with Task 15).
- main.rs: `EnableFocusChange`/`DisableFocusChange` (from `ratatui::crossterm::event`) executed everywhere `EnableMouseCapture`/`DisableMouseCapture` are (init, exit, panic hook, `$EDITOR` round-trip), and an `Event::FocusGained => { redraw |= app.update(Action::ReloadProjectFiles); }` arm.

- [ ] **Step 1: Write the failing tests** (project_ctx.rs — set mtimes explicitly rather than sleeping)

```rust
fn bump_mtime(p: &std::path::Path) {
    let t = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let f = std::fs::File::options().append(true).open(p).unwrap();
    f.set_modified(t).unwrap();
}

#[test]
fn reload_picks_up_changed_variables_and_keeps_active_env() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), None).unwrap();
    std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"1\"\n").unwrap();
    let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
    ctx.set_env(Some("qa".into()));
    let (changed, _) = ctx.reload_if_changed();
    assert!(!changed, "nothing changed yet");
    std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"2\"\n").unwrap();
    bump_mtime(&dir.path().join("environments/qa.toml"));
    let (changed, warns) = ctx.reload_if_changed();
    assert!(changed && warns.is_empty());
    assert_eq!(ctx.env_values["tok"], "2");
    assert_eq!(ctx.env_label(), "qa");
}

#[test]
fn reload_with_broken_file_warns_and_keeps_previous_values() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), None).unwrap();
    std::fs::write(dir.path().join("variables.toml"), "[a]\ndefault = \"1\"\n").unwrap();
    let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
    assert_eq!(ctx.variables["a"].default.as_deref(), Some("1"));
    std::fs::write(dir.path().join("variables.toml"), "not toml [").unwrap();
    bump_mtime(&dir.path().join("variables.toml"));
    let (_, warns) = ctx.reload_if_changed();
    assert!(!warns.is_empty(), "parse failure surfaced");
    assert_eq!(ctx.variables["a"].default.as_deref(), Some("1"), "previous good state kept");
}

#[test]
fn deleted_active_env_degrades_to_no_env_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), None).unwrap();
    std::fs::write(dir.path().join("environments/qa.toml"), "tok = \"1\"\n").unwrap();
    let (mut ctx, _) = ProjectContext::open(dir.path().to_path_buf());
    ctx.set_env(Some("qa".into()));
    std::fs::remove_file(dir.path().join("environments/qa.toml")).unwrap();
    // no mtime bump needed: the active env file's stamp goes Some -> None,
    // which is itself a difference (bump_mtime can't open a directory anyway)
    let (changed, warns) = ctx.reload_if_changed();
    assert!(changed && !warns.is_empty());
    assert_eq!(ctx.env_label(), "no env");
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.** `stamps` becomes a first-class struct built by one `fn stamp(root, active_env) -> Vec<(PathBuf, Option<SystemTime>)>`; `reload_if_changed` compares fresh stamps against stored, and on mismatch reloads each piece with keep-previous-on-parse-error semantics. Wire the action + main.rs event/capture changes.

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "On-demand mtime reload of project files; refresh on terminal focus regain"
```

---

### Task 13: Send pipeline integration + substitute_body toggle

**Files:**
- Modify: `crates/postui/src/app.rs`, `crates/postui/src/action.rs`, `crates/postui/src/keys.rs`, `crates/postui/src/components/palette.rs`, `crates/postui/src/components/editor.rs`
- Test: `crates/postui/tests/http_integration.rs` (extend)

**Interfaces:**
- `ForceSend` becomes: `self.apply(Action::ReloadProjectFiles);` then `prepare(&req, &self.project.prepare_context())`; `Err(Unresolved)` → error toast `unresolved variables ({env_label}): a, b` and no spawn.
- `Action::ToggleBodyVars` flips `editor.substitute_body` (marks dirty naturally via `current_request`); keybinding `alt+b` = `toggle_body_vars`; palette "Body: toggle {{var}} substitution".
- Body tab indicator: when `substitute_body`, the tab bar renders a `vars` badge span (accent) after the validity glyph.

- [ ] **Step 1: Write the failing tests**

App-level:

```rust
#[tokio::test]
async fn unresolved_variable_blocks_send_with_toast() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, tempfile::tempdir().unwrap().path().into());
    app.editor.url = crate::components::line_input::LineInput::new("http://x/{{gone}}");
    app.update(Action::ForceSend);
    assert!(app.in_flight.is_none());
    assert!(!app.toasts.is_empty());
}

#[test]
fn toggle_body_vars_flips_flag_and_shows_badge() {
    let mut app = App::new_for_test();
    app.update(Action::ToggleBodyVars);
    assert!(app.editor.substitute_body);
    // render the editor with Body tab active; buffer must contain "vars"
}
```

wiremock (extend `tests/http_integration.rs`; follow its existing setup style):

```rust
#[tokio::test]
async fn send_substitutes_vars_and_applies_default_headers() {
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/users/7"))
        .and(matchers::header("authorization", "Bearer tok-qa"))
        .and(matchers::header("accept", "application/json"))
        .and(matchers::body_string(r#"{"id": "7"}"#))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    std::fs::write(
        dir.path().join("project.toml"),
        "name = \"svc\"\n[default_headers]\naccept = \"application/json\"\nauthorization = \"Bearer {{tok}}\"\n",
    ).unwrap();
    std::fs::write(dir.path().join("variables.toml"), "[base]\n[tok]\n[uid]\ndefault = \"7\"\n").unwrap();
    std::fs::write(
        dir.path().join("environments/qa.toml"),
        format!("base = \"{}\"\ntok = \"tok-qa\"\n", server.uri()),
    ).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = postui::app::App::with_root(tx, dir.path().to_path_buf());
    app.update(Action::SwitchEnv(Some("qa".into())));
    app.editor.method = postui_core::model::Method::Post;
    app.editor.url = postui::components::line_input::LineInput::new("{{base}}/users/{{uid}}");
    app.editor.set_body_text(r#"{"id": "{{uid}}"}"#);
    app.editor.substitute_body = true;
    app.update(Action::ForceSend);
    assert!(app.in_flight.is_some());
    // drain until the tagged result arrives, then assert 200 (copy the drain
    // loop pattern already used in this test file).
}
```

Also: a case where a disabled request header row suppresses a default (mock `.and(matchers::header_is_missing("x-default"))`... wiremock 0.6 exposes `matchers::header` only — instead assert via a handler that echoes headers, or match on the exact request and let a strict mock 404 a wrong one; follow whichever pattern the existing file uses for header assertions).

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.** Editor tab-bar badge: in `draw_tab_bar`, after the validity glyph for the Body tab:

```rust
if self.substitute_body {
    spans.push(Span::styled("vars ", Style::default().fg(theme.accent)));
}
```

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Sends resolve vars + default headers; unresolved blocks; body substitution toggle"
```

---

### Task 14: Inherited default-header rows in the Headers tab

**Files:**
- Modify: `crates/postui/src/components/editor.rs`, `crates/postui/src/app.rs`

**Interfaces:**
- `Editor.inherited_headers: IndexMap<String, Entry>` (public; App assigns `self.editor.inherited_headers = self.project.meta.default_headers.clone();` in the `update()` sync block alongside `open_slug`).
- Draw-only: the Headers tab renders inherited rows (enabled defaults only) ABOVE the request table, dimmed, marked `(project)`; a row whose name is case-insensitively present in `self.headers` gets `(overridden)` when that request row is enabled or `(disabled)` when it is disabled. Table-editor interaction (selection, `a`, `d`, space) is untouched and applies only to request rows: pass the table a sub-`Rect` below the inherited lines.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn headers_tab_shows_inherited_rows_with_status() {
    let mut e = Editor { active_tab: EditorTab::Headers, ..Editor::default() };
    e.inherited_headers.insert("accept".into(), Entry { value: "application/json".into(), enabled: true });
    e.inherited_headers.insert("x-a".into(), Entry { value: "1".into(), enabled: true });
    e.inherited_headers.insert("x-b".into(), Entry { value: "2".into(), enabled: true });
    e.headers.insert("X-A".into(), Entry { value: "9".into(), enabled: true });
    e.headers.insert("X-B".into(), Entry { value: "n".into(), enabled: false });
    let theme = Theme::dark();
    let ctx = DrawCtx { theme: &theme, focused: true };
    let backend = TestBackend::new(70, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| e.draw(f, f.area(), &ctx)).unwrap();
    let content = format!("{:?}", terminal.backend().buffer());
    assert!(content.contains("(project)"), "{content}");
    assert!(content.contains("(overridden)"), "{content}");
    assert!(content.contains("(disabled)"), "{content}");
    assert!(content.contains("application/json"));
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** in `draw_tab_content`'s Headers arm: build the inherited `Line`s (muted style, `  ✓ name  value  (project|overridden|disabled)`), render as a `Paragraph` in the top `inherited.len()` rows of the area, then call `self.table.draw` on the remaining sub-rect. Zero inherited headers ⇒ identical to today.

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Headers tab shows inherited project defaults with override/disable status"
```

---

### Task 15: Variable picker (`{{` trigger + keybinding + insertion)

**Files:**
- Create: `crates/postui/src/components/var_picker.rs`
- Modify: `crates/postui/src/components/mod.rs`, `modal.rs`, `editor.rs`, `line_input.rs`, `app.rs`, `action.rs`, `keys.rs`, `palette.rs`

**Interfaces:**
- `line_input.rs`: `pub fn insert_str(&mut self, s: &str)` (insert at cursor, advance cursor by `s.chars().count()`); `pub fn ends_with_at_cursor(&self, suffix: &str) -> bool` (text before the cursor ends with `suffix`).
- `var_picker.rs`:

```rust
pub struct VarEntry { pub name: String, pub description: Option<String>, pub value: Option<String> }
pub struct VarPickerState { /* input, selected, entries, filtered */ pub completing: bool }
impl VarPickerState {
    pub fn new(entries: Vec<VarEntry>, completing: bool) -> Self;
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ModalResult>;
    // Enter => ModalResult { actions: vec![Action::InsertVarText(text)], close: true }
    //   completing: text = format!("{}}}}}", name)        // "name}}" — caller already typed "{{"
    //   else:       text = format!("{{{{{}}}}}", name)    // "{{name}}"
    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme);
}
```
- `Modal::VarPicker(VarPickerState)` (handle_key/draw delegation like Palette/Chooser).
- Actions: `OpenVarPicker { completing: bool }` — reload project files; build entries from **declared** variables (`project.variables` order; `value` = resolved value from `prepare_context().vars` when present); zero declared → toast `no variables declared — edit variables.toml`. `InsertVarText(String)` — route by focus:
  - `PaneId::Editor` + `SubFocus::Url` → `self.editor.url.insert_str(&text)`
  - table cell editing (`self.editor.table.editing` is Some) → `edit.input.insert_str(&text)`
  - Body tab + `SubFocus::Content` → feed each char of `text` through the body handler as a synthesized plain `KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)` (add `pub fn body_insert_str(&mut self, s: &str)` on Editor doing exactly that); if `!substitute_body`, set it true and toast `body {{var}} substitution enabled`.
  - anything else → toast `nowhere to insert — focus a text field first`.
- `{{` trigger: in `Editor::handle_key`, after the URL input consumes a `Char('{')`, `if self.url.ends_with_at_cursor("{{") { return Some(Action::OpenVarPicker { completing: true }); }`. Same check after a table edit consumes `Char('{')` (peek `self.table.editing.as_ref().map(|e| &e.input)`).
- Keybinding `ctrl+v` = `pick_variable` → `OpenVarPicker { completing: false }`; palette "Variables: insert…".
- Picker rows render `name  description(dim)  = value(dim)`; entries with `value: None` get a warning-colored `unset` tag instead of a value.

- [ ] **Step 1: Write the failing tests**

```rust
// line_input.rs
#[test]
fn insert_str_at_cursor_and_suffix_probe() {
    let mut i = LineInput::new("ab");
    i.handle_key(code(KeyCode::Left));
    i.insert_str("{{x}}");
    assert_eq!(i.text(), "a{{x}}b");
    assert_eq!(i.cursor(), 6);
    let mut j = LineInput::new("http://{{");
    assert!(j.ends_with_at_cursor("{{"));
    j.handle_key(code(KeyCode::Left));
    assert!(!j.ends_with_at_cursor("{{"), "cursor moved off the braces");
}

// var_picker.rs
#[test]
fn enter_emits_completion_or_full_token() {
    let entries = vec![VarEntry { name: "base_url".into(), description: None, value: Some("x".into()) }];
    let mut p = VarPickerState::new(entries.clone(), true);
    let res = p.handle_key(key(KeyCode::Enter)).unwrap();
    assert_eq!(res.actions, vec![Action::InsertVarText("base_url}}".into())]);
    let mut p = VarPickerState::new(entries, false);
    let res = p.handle_key(key(KeyCode::Enter)).unwrap();
    assert_eq!(res.actions, vec![Action::InsertVarText("{{base_url}}".into())]);
}

// app.rs
fn app_with_vars() -> App {
    let mut app = App::new_for_test();
    std::fs::write(app.project.root.join("variables.toml"), "[base]\ndefault = \"http://x\"\n[tok]\n").unwrap();
    app.update(Action::ReloadProjectFiles);
    app
}

#[test]
fn typing_double_brace_in_url_opens_completing_picker_and_insert_lands_in_url() {
    let mut app = app_with_vars();
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Url;
    app.handle_key(&keymap, plain('{'));
    assert!(app.modals.is_empty(), "one brace: no picker");
    app.handle_key(&keymap, plain('{'));
    let Some(Modal::VarPicker(p)) = app.modals.top() else { panic!("expected picker") };
    assert!(p.completing);
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.url.text(), "{{base}}");
}

#[test]
fn body_insert_autoenables_substitution() {
    let mut app = app_with_vars();
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;
    app.update(Action::OpenVarPicker { completing: false });
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.editor.body_text(), "{{base}}");
    assert!(app.editor.substitute_body, "auto-enabled");
    assert!(!app.toasts.is_empty());
}

#[test]
fn picker_with_no_declared_vars_toasts() {
    let mut app = App::new_for_test();
    app.update(Action::OpenVarPicker { completing: false });
    assert!(app.modals.is_empty());
    assert!(!app.toasts.is_empty());
}
```

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** — picker modeled on ChooserState (fuzzy over name + description). Esc while `completing` simply closes; the typed `{{` stays as literal text (spec: Esc leaves the literal). Filter typing happens in the modal, not the underlying field.

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Variable picker: {{ trigger in line inputs, ctrl+v anywhere, insertion routing"
```

---

### Task 16: edtui mouse forwarding (click-to-place + wheel)

**Files:**
- Modify: `crates/postui/src/components/editor.rs`, `crates/postui/src/app.rs`, `crates/postui/src/main.rs`

Docs to check first: docs.rs `edtui` 0.11 — `EditorEventHandler::on_mouse_event` (signature and whether it consumes absolute terminal coordinates; the `EditorView` records the rendered area/offset into `EditorState`, which is why `draw` takes `&mut self`).

**Interfaces:**
- `Editor` gains `last_body_area: Option<Rect>` (set in `draw_tab_content`'s Body arm each frame, `None` on other tabs) and

```rust
/// Returns true when the event was consumed by the body editor.
pub fn handle_mouse(&mut self, m: ratatui::crossterm::event::MouseEvent) -> bool
```
which forwards to `self.body_handler.on_mouse_event(m, &mut self.body)` when `active_tab == Body` and the event position is inside `last_body_area`; a left-down also sets `sub_focus = Content`.
- `App` gains `pub fn handle_mouse(&mut self, m: MouseEvent, layout: &AppLayout) -> bool` — the logic currently inline in main.rs, extended: for a left-down or scroll inside the editor pane, try `self.editor.handle_mouse(m)` first (returning `self.update(Action::Render)` when consumed); otherwise fall back to the existing FocusPane/ScrollPane behavior (a body click focuses the pane too: apply `FocusPane(Editor)` before forwarding). main.rs shrinks to `redraw |= app.handle_mouse(m, &layout)`.
- `Editor::handle_scroll` (the `ScrollPane` path, wheel over an unfocused editor pane): when on the Body tab with a recorded area, synthesize `MouseEventKind::ScrollUp/ScrollDown` events at the area origin, `|delta|` times, through `handle_mouse`; other tabs remain a no-op.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn click_in_body_area_places_cursor_and_focuses_content() {
    let mut app = App::new_for_test();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("hello\nworld");
    // render once so the view records its area
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let area = app.editor.last_body_area.expect("body area recorded");
    let m = ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: area.x + 4, row: area.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    let layout = crate::layout::compute_layout(Rect::new(0, 0, 120, 40));
    app.handle_mouse(m, &layout);
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
    assert_eq!(app.focus, PaneId::Editor);
    assert_eq!(app.editor.body.cursor.row, 1, "clicked the second line");
}
```

(If edtui's coordinate handling makes the cursor-row assertion unreliable under TestBackend, keep the focus/sub_focus assertions, drop the cursor one, and note the cursor behavior for the manual TTY sweep — do not fake it.)

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** as specified. Watch the line-numbers gutter: edtui accounts for it internally when the event is inside the recorded view.

- [ ] **Step 4: Run** — `cargo test --workspace` → PASS.

- [ ] **Step 5: fmt, clippy, commit**

```bash
git add -A && git commit -m "Forward mouse clicks and wheel into the edtui body editor"
```

---

### Task 17: Deferred cheap cleanups

**Files:**
- Modify: `crates/postui-core/src/storage.rs`, `crates/postui/src/http.rs`, `crates/postui/src/app.rs`

Four independent fixes, one commit:

- [ ] **Step 1: rename-onto-itself is a no-op.** Test: `rename_request(root, "a", "a")` returns `Ok(())` and the file survives. Implement: early `if from == to { return validate_slug(from).map(|_| ()); }` in `rename_request`.

- [ ] **Step 2: `list_requests` surfaces mid-walk IO errors.** Change signature to `pub fn list_requests(root: &Path) -> (Vec<RequestListing>, Option<String>)` where the `Option<String>` is the first walk error's message (listing still returns everything walked). Update `App::refresh_sidebar` (single call site funnel from Task 7) to toast the error; update remaining direct callers (core tests, `create_or_save_as`). Test: a `requests/sub` directory with `0o000` perms yields `Some(err)` on Unix (guard with `#[cfg(unix)]`, restore perms after so tempdir cleanup works).

- [ ] **Step 3: `create_or_save_as` calls `list_requests` once.** Replace the exists-scan with `load_request(&self.project.root, name).is_ok()` → "request already exists" toast (also true for broken-but-present files? `load_request` errors on parse — use `request_path` existence instead: add `pub fn request_exists(root: &Path, slug: &str) -> bool` to storage.rs). Refresh after save via `refresh_sidebar()` as today. Test: existing app test `new_request_prompt_flow_creates_file_and_opens_it` still passes + a duplicate-name toast test if not already present.

- [ ] **Step 4: `http::client()` drops `.expect`.** `reqwest::Client::builder().build().unwrap_or_else(|_| reqwest::Client::new())` with a comment that builder failure is practically unreachable and the default client is the graceful fallback. Existing `client_builds_without_a_tokio_runtime` test still covers it.

- [ ] **Step 5: Run workspace, fmt, clippy, commit**

```bash
cargo test --workspace
git add -A && git commit -m "Cleanups: rename no-op, listing IO errors surfaced, single exists-check, no client expect"
```

---

### Task 18: Stage-3 acceptance test + manual sweep checklist

**Files:**
- Create: `crates/postui/tests/stage3_acceptance.rs`
- Modify: this plan file (check off the sweep list at the end)

- [ ] **Step 1: Write the acceptance test** — one scripted flow through the public `App` API + `Keymap::default_bindings()` + `TestBackend` renders (mirror `stage2_acceptance.rs`'s style):

1. Build two temp projects (alpha: vars `base`/`tok`, envs `qa`/`prod` pointing at a wiremock server URI; beta: one request `pong`). Register both in `app.registry`.
2. In alpha: create request `users/list` via the `n` prompt flow, URL `{{base}}/users?tok={{tok}}`.
3. Send with no env active → unresolved toast, nothing in flight.
4. `SwitchEnv(Some("qa"))` (assert header bar renders `alpha · qa`), send → drain the rx channel until the generation-tagged result lands → response `Ready`, wiremock got `?tok=qa-tok` and the default header.
5. Cycle to prod (`alt+c` through `handle_key`), send again → prod values hit.
6. Sidebar: collapse/expand the `users` folder via keys; wheel-scroll (`ScrollPane`) and assert no snap-back after a draw.
7. `alt+o` cycles to beta; assert `pong` listed, editor restored from beta's local state; cycle back; alpha's open request and expansion restored.
8. `{{` picker: type `{{` in the URL, pick, assert insertion.
9. Quit path: `PersistLocalState` then assert both projects' `.local/state.toml` contents.

- [ ] **Step 2: Run** — `cargo test --workspace` → all green.

- [ ] **Step 3: fmt, clippy, full suite, commit**

```bash
git add -A && git commit -m "Stage-3 acceptance test: two projects, two envs, vars end to end"
```

- [ ] **Step 4: Present the manual TTY sweep checklist to the user** (not automatable in this environment):

```
cargo run -p postui  (and: cargo run -p postui -- <dir>)
- header shows project · env; alt+e / alt+c switch envs; ctrl+o / alt+o switch projects
- alt+n creates a project at the prefilled path; open-by-path with ~ works
- editing variables.toml / env files in another terminal is picked up on focus return
- {{ in the URL pops the picker; ctrl+v works in table cells and the body
- unresolved var blocks send with a clear toast; alt+b toggles the body vars badge
- default headers render dimmed in the Headers tab; disabled row suppresses on the wire
- sidebar tree expand/collapse with enter/arrows; wheel scroll doesn't snap back
- mouse click into the body places the cursor; wheel scrolls the body
- project switch with dirty editor prompts; state (env, open request, expansion) restored per project
- keybinding defaults survive real terminals (note any that don't for rebinding)
```

---

## Self-Review Checklist (run after writing, fixed inline)

- Spec coverage: §1 → Tasks 3/5/6, §2 → 9/10/11, §3 → 7, §4 → 1/2/4/13, §5 → 15, §6 → 4/14, §7 → 12, §8 → 7/16/17, §10 → per-task tests + 18.
- Type consistency spot-checks: `PrepareContext`/`prepare` signature (Tasks 4, 6, 13, 15), `Row` variants (Tasks 7, 9, 18), `ChooserState` (Tasks 8, 9, 11), `ProjectContext` methods (Tasks 6, 11, 12, 13), `InsertVarText`/`OpenVarPicker` (Task 15), `list_requests` tuple return (Tasks 7→17 ordering: Task 7's helper is the single site Task 17 changes).
