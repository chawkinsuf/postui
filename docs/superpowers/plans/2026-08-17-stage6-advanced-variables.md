# Stage 6 — Advanced Variables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement master-spec §4 in full — enumerated options, groups, secrets, request scope, the Variable Manager screen, and the in-context picker flows — everything editable in the GUI.

**Architecture:** A pure resolver in `postui-core` (`varmodel.rs`: declarations + env merge + `resolve_env` → flat values + per-name metadata) feeds both send-time `prepare()` and every UI surface. A `varedit.rs` module performs surgical `toml_edit` mutations on shareable TOML. The TUI gains a `Screen { Main, VarManager }` enum, a full-screen Manager grid component, a Vars tab in the editor, and an upgraded two-context picker.

**Tech Stack:** Rust, ratatui + crossterm (via `ratatui::crossterm`), tokio, indexmap, toml / toml_edit, serde.

**Spec:** `docs/superpowers/specs/2026-08-17-stage6-advanced-variables-design.md` — binding. Read it before any task; sections cited per task.

## Global Constraints

- Cargo needs `export PATH="$HOME/.cargo/bin:$PATH"` in every shell (subagents too).
- Import crossterm types via `ratatui::crossterm::...` only.
- Before every commit: `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` clean, full `cargo test --workspace` green.
- All edits to shareable TOML (`variables.toml`, `environments/*.toml`, request files) go through `toml_edit` document mutation — `toml::to_string` on these files is forbidden (spec §7). `.local/` files may serialize fresh.
- Painted-UI conventions (stage 5 + post-stage-5 rounds) for every new surface: `paint::` Button/TextField/Chip/TabStrip/floating_panel, hover via HitMap, keyboard parity for every mouse action.
- Secret values never appear in toasts, errors, or logs — names only (spec §3).
- Reserved names: `options`, `groups` are not valid variable or group names; variable and group names share one namespace (spec §1.1).
- **tmux visual + usability verification is REQUIRED for every UI task** (Tasks 9–17) before it is called done: run the app under tmux (recipe: hold the server with a `run_in_background` Bash call `tmux new-session -d ... && sleep 3600`; `TMUX_TMPDIR=/tmp/claude-1000/tmux`; drive with `send-keys`, read with `capture-pane -p`; mouse via SGR sequences; answer terminal queries per the tty-driving notes), exercise the task's flows end to end, and judge friction, not just rendering. The stage ends with a scripted whole-workflow sweep (Task 18).
- Commit messages: plain, no Co-Authored-By, no Claude-Session trailer.

## File Structure

- `crates/postui-core/src/varmodel.rs` — NEW. `OptionDecl`, `VarDecl` (new form), `GroupOption`, `GroupDecl`, `VarModel`, `EnvData`, `ModelError`, `parse_variables`, `parse_environment`, `validate_env`, `merged_var_options`, `merged_group_options`, `Resolved`, `VarMeta`, `resolve_env`.
- `crates/postui-core/src/varedit.rs` — NEW. toml_edit mutation verbs for `variables.toml` / env files + `scan_usage`.
- `crates/postui-core/src/project.rs` — MODIFY. `load_variables` → `VarModel`, `load_environment` → `EnvData`, `LocalState.selections`, `load_secrets`/`save_secrets`. Old `VarDecl`/`Variables`/`resolve` deleted.
- `crates/postui-core/src/model.rs` — MODIFY. `HttpRequest.variables: IndexMap<String, Entry>` + save/load.
- `crates/postui-core/src/prepare.rs` — MODIFY. `PrepareContext { vars, meta, request‑overlay handling }`, `UnresolvedCause`, richer `PrepareError`.
- `crates/postui-core/src/lib.rs` — MODIFY. `pub mod varmodel; pub mod varedit;`.
- `crates/postui/src/project_ctx.rs` — MODIFY. Holds `VarModel`/`EnvData`/selections/secrets, exposes `resolved()`, write-through helpers.
- `crates/postui/src/components/varmanager.rs` — NEW. Manager grid component (state, draw, keys).
- `crates/postui/src/components/editor.rs` — MODIFY. Vars tab.
- `crates/postui/src/components/var_picker.rs` — MODIFY. Two contexts, options, group preview, inline flows.
- `crates/postui/src/app.rs`, `src/app/mouse.rs`, `src/ui.rs`, `src/action.rs`, `src/keys.rs`, `src/hit.rs`, `src/components/modal.rs` — MODIFY. Screen enum, actions, keybindings, hits, prompts, secret chain.
- `crates/postui/tests/stage6_acceptance.rs` — NEW.

Tasks 1–8 are core/plumbing (no UI); 9–17 are UI; 18 is acceptance. Sequential except where noted; 13 and 14–15 may run in parallel after 12.

---

### Task 1: Core variable model & parsing (`varmodel.rs`)

Spec §1.1, §1.2 (parse only; merge is Task 2).

**Files:**
- Create: `crates/postui-core/src/varmodel.rs`
- Modify: `crates/postui-core/src/lib.rs` (add `pub mod varmodel;`)

**Interfaces (Produces):**
```rust
pub struct OptionDecl { pub description: Option<String>, pub value: String }
#[derive(Default)]
pub struct VarDecl {
    pub description: Option<String>,
    pub default: Option<String>,
    pub secret: bool,
    pub options: IndexMap<String, OptionDecl>,
}
pub struct GroupOption { pub description: Option<String>, pub values: IndexMap<String, String> } // member → value
pub struct GroupDecl { pub description: Option<String>, pub members: Vec<String>, pub options: IndexMap<String, GroupOption> }
#[derive(Default)]
pub struct VarModel { pub vars: IndexMap<String, VarDecl>, pub groups: IndexMap<String, GroupDecl> }
pub enum ModelError { /* one variant per friendly error; Display gives the message incl. the fix */ }
pub fn parse_variables(s: &str) -> Result<VarModel, ModelError>;
// EnvData: flat values + raw option tables (interpreted against the model in Task 2)
pub struct EnvData {
    pub values: IndexMap<String, String>,
    pub options: IndexMap<String, IndexMap<String, IndexMap<String, String>>>, // name → key → field/member → string
}
pub fn parse_environment(s: &str) -> Result<EnvData, ModelError>;
```
All structs `Debug, Clone, PartialEq`; maps are `indexmap::IndexMap` (order-preserving).

- [ ] **Step 1: Write failing parse tests** in `varmodel.rs` `#[cfg(test)] mod tests`. Cover, each with an exact-message assertion (`assert!(err.to_string().contains(...))`) for the error cases:

```rust
#[test]
fn parses_simple_secret_enumerated_and_group() {
    let m = parse_variables(r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
secret = true

[user]
[user.options.alice]
description = "admin"
value = "1001"
[user.options.bob]
value = "2002"

[groups.test-user]
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#).unwrap();
    assert_eq!(m.vars["user"].options.keys().collect::<Vec<_>>(), ["alice", "bob"]);
    assert!(m.vars["api_key"].secret);
    assert_eq!(m.groups["test-user"].members, ["user_id", "customer_id"]);
    assert_eq!(m.groups["test-user"].options["alice"].values["customer_id"], "c-77");
}
```

Plus one test per friendly error: variable named `options` or `groups`; group name colliding with a variable name; member with its own `options`; member declared `secret = true`; variable in two groups; `secret = true` with `default` or with `options`; invalid variable/option-key name (reuse `crate::vars::is_valid_var_name`); unknown field in a variable table (stage-3 behavior preserved). Env parsing tests: flat pairs + `[options.x.k]` tables land in `EnvData.options`; a non-table under `[options]` errors; non-string flat value errors.

- [ ] **Step 2: Run** `cargo test -p postui-core varmodel` — expect FAIL (module missing).
- [ ] **Step 3: Implement.** Parse with `toml::from_str::<toml::Table>` and walk manually (serde derive can't express "any key except reserved ones is a variable table"). `groups` key → groups; every other top-level key → variable table. Validate per Step-1 rules; collect member→group map to detect duplicates. `parse_environment`: top-level string values → `values`; `options` table → nested maps (every leaf must be a string; `description`/`value`/member names all live in the same string map at this layer). Any other non-string top-level = error.
- [ ] **Step 4: Run tests** — expect PASS.
- [ ] **Step 5:** fmt, clippy, full test run, commit: `feat(core): stage-6 variable model parsing`.

---

### Task 2: Env merge rule + env validation

Spec §1.2 exactly — one merge rule, per-env enumerated-ness, conflict errors.

**Files:**
- Modify: `crates/postui-core/src/varmodel.rs`

**Interfaces (Produces):**
```rust
/// Env option tables merged by key onto the shared list; env-only lists
/// come through wholesale. Empty map = simple in this env.
pub fn merged_var_options(model: &VarModel, env: &EnvData, name: &str) -> IndexMap<String, OptionDecl>;
pub fn merged_group_options(model: &VarModel, env: &EnvData, group: &str) -> IndexMap<String, GroupOption>;
/// Friendly errors: flat value for a var enumerated *in this env*; flat
/// value for a secret var; [options.<name>] where <name> is undeclared or
/// secret; group option row naming a non-member.
pub fn validate_env(model: &VarModel, env: &EnvData) -> Result<(), ModelError>;
```

- [ ] **Step 1: Write failing tests:** shared+override (env `value` wins, shared `description` kept); env-added key appended after shared keys; env list wholesale when declaration has no options; group member-value override; `validate_env` errors for each §1.2 case — including the *per-env* twist: flat `user = "x"` errs in the env that gives `user` options, and passes in an env that doesn't (two `EnvData`s, same model).
- [ ] **Step 2: Run** — FAIL (functions missing).
- [ ] **Step 3: Implement.** Merge: clone shared options; for each env table for `name`, `entry(key)`: existing → override `value` (vars) / listed member values (groups) and `description` if given; new key → construct from env fields (`value` required for vars — missing = `ModelError`; validate this in `validate_env` so merge itself can stay infallible after validation). Interpret group env rows: keys other than `description` are member names.
- [ ] **Step 4: Run** — PASS. **Step 5:** fmt/clippy/test, commit `feat(core): env option merge + validation`.

---

### Task 3: `resolve_env` + `Resolved` metadata

Spec §2 — the heart of the stage.

**Files:**
- Modify: `crates/postui-core/src/varmodel.rs`

**Interfaces (Produces):**
```rust
pub type Selections = IndexMap<String, String>;      // name → option key (one env's section)
pub type SecretValues = IndexMap<String, String>;    // one env's section
#[derive(Debug, Clone, PartialEq)]
pub enum VarMeta {
    Simple,
    Enumerated { selected: String },
    GroupMember { group: String, selected: String },
    Secret,
    NeedsSelection,           // enumerated/group with no (or stale) selection
    MissingSecret,            // secret with no value for this env
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    pub values: IndexMap<String, String>,   // names needing selection/secret are OMITTED
    pub meta: IndexMap<String, VarMeta>,    // every declared name (vars + group members)
}
pub fn resolve_env(model: &VarModel, env: &EnvData, selections: &Selections, secrets: &SecretValues) -> Resolved;
```
Precedence inside (spec §2, layers 3–6): secret → selected option (env-merged) → simple env value → default. Request overlay (layer 1) is Task 5's job. Undeclared env values pass through into `values` with no meta entry (stage-3 leniency).

- [ ] **Step 1: Failing tests:** each precedence layer; group member resolves from group's selected option with `GroupMember` meta; stale selection key (not in merged list) → `NeedsSelection` and omitted; secret present → `Secret` + value, absent → `MissingSecret` + omitted; per-env enumerated var with a selection resolves, in the simple env resolves as `Simple` from flat value; undeclared env value passes through.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement** (iterate `model.vars`, then `model.groups` members, then leftover `env.values`). **Step 4: Run** — PASS. **Step 5:** commit `feat(core): resolve_env with per-name metadata`.

---

### Task 4: Selections + secrets local state (`project.rs`)

Spec §1.3. Also swaps `project.rs` onto the new model.

**Files:**
- Modify: `crates/postui-core/src/project.rs` (and callers that compile-break)

**Interfaces (Produces / Changes):**
```rust
// LocalState gains (serde default):
pub selections: IndexMap<String, IndexMap<String, String>>,  // env → name → option key
// New:
pub fn load_secrets(root: &Path) -> Result<IndexMap<String, IndexMap<String, String>>, ProjectError>; // env → name → value; missing file = empty
pub fn save_secrets(root: &Path, s: &IndexMap<String, IndexMap<String, String>>) -> std::io::Result<()>; // fresh serialize into .local/, atomic (write temp + rename)
// Changed signatures:
pub fn load_variables(root: &Path) -> Result<varmodel::VarModel, ProjectError>;   // via parse_variables
pub fn load_environment(root: &Path, name: &str) -> Result<varmodel::EnvData, ProjectError>; // via parse_environment; caller runs validate_env
// DELETED: old VarDecl, Variables alias, resolve(). Compile errors guide caller updates (project_ctx.rs, tests) — port them to resolve_env with empty selections/secrets for now.
```

- [ ] **Step 1: Failing tests:** `LocalState` with selections round-trips and old state.toml (no selections) still parses; secrets round-trip; missing secrets.toml = empty; stale-selection tolerance lives in resolve (already tested) — here test that a state.toml with unknown fields still errors (deny is NOT set on LocalState — keep `#[serde(default)]` permissive as today).
- [ ] **Step 2–4:** Run FAIL → implement → PASS, porting `project_ctx.rs` and existing tests to the new signatures (temporary: `resolved().values` where they used `resolve`).
- [ ] **Step 5:** commit `feat(core): selections + secrets local state; project loaders on VarModel`.

---

### Task 5: Request `[variables]` + prepare overlay + unresolved causes

Spec §1.4, §2 layers 1–2, §4 file form.

**Files:**
- Modify: `crates/postui-core/src/model.rs`, `crates/postui-core/src/prepare.rs`

**Interfaces (Produces / Changes):**
```rust
// HttpRequest gains (same serde/save treatment as params/headers — plain
// string or inline {value, enabled}; skip when empty; to_toml_string emits
// [variables] after [headers], before body):
pub variables: IndexMap<String, Entry>,
// prepare.rs:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedCause { Undefined, NeedsSelection, MissingSecret }
pub enum PrepareError { Unresolved(std::collections::BTreeMap<String, UnresolvedCause>) }
// PrepareContext gains:
pub meta: IndexMap<String, varmodel::VarMeta>,   // from Resolved
// prepare(): before substitution, overlay enabled request variables:
//   for (k, e) in &req.variables { if e.enabled { vars.insert(k, e.value) } }
// (on a clone of ctx.vars — highest precedence). Missing-name causes come
// from ctx.meta: NeedsSelection / MissingSecret map through; else Undefined.
// Display: group by cause — "unresolved: a, b · need a selection: user · missing secret: api_key".
```

- [ ] **Step 1: Failing tests:** round-trip a request with `[variables]` incl. a disabled inline entry, assert `to_toml_string` formatting (plain string for enabled, inline table for disabled) and section order; prepare: request var beats secret/selection/env/default; disabled request var does not resolve; causes mapped per meta; Display groups causes.
- [ ] **Step 2–4:** FAIL → implement → PASS (update `PrepareError` matchers across the workspace; the send-path message in `app.rs` compiles against the new Display).
- [ ] **Step 5:** commit `feat(core): request-scoped variables + prepare causes`.

---

### Task 6: Surgical TOML editing (`varedit.rs`)

Spec §5 persistence + §7 write fidelity. Pure text→text functions for testability.

**Files:**
- Create: `crates/postui-core/src/varedit.rs`
- Modify: `crates/postui-core/src/lib.rs`

**Interfaces (Produces):** all `fn(doc: &str, ...) -> Result<String, EditError>`; every function parses with `toml_edit::DocumentMut`, mutates only the addressed item, returns `doc.to_string()`.
```rust
pub enum EditError { Parse(String), NotFound(String), Conflict(String) }
// variables.toml verbs:
pub fn upsert_var(doc, name, description: Option<&str>, default: Option<&str>) -> ...;   // create or update fields given as Some
pub fn set_secret_flag(doc, name, secret: bool) -> ...;   // removes `default` when setting secret (spec §1.1) — Conflict if options exist
pub fn rename_var(doc, from, to) -> ...;                  // also rewrites group members + group option member keys
pub fn delete_var(doc, name) -> ...;                      // Conflict if a group member
pub fn upsert_shared_option(doc, owner, key, description: Option<&str>, value_or_members: &IndexMap<String,String>) -> ...; // vars: {"value": v}; groups: member map
pub fn delete_shared_option(doc, owner, key) -> ...;
pub fn upsert_group(doc, name, description: Option<&str>, members: &[String]) -> ...;
pub fn delete_group(doc, name) -> ...;
// env-file verbs (same doc-in/doc-out):
pub fn set_env_value(doc, name, value: Option<&str>) -> ...;             // None = remove the flat pair
pub fn upsert_env_option(doc, owner, key, fields: &IndexMap<String,String>) -> ...;
pub fn delete_env_option(doc, owner, key) -> ...;
```

- [ ] **Step 1: Failing tests.** The signature test of this task is **round-trip fidelity**: start each from a fixture doc with comments, blank lines, and unrelated entries; assert the output differs from the input *only* in the addressed lines (compare full strings, with the expected output written out in the test). Cover every verb, plus: rename cascades into `groups.*.members` and group option member keys; `set_secret_flag(true)` strips `default`; delete_var on a member → `Conflict`; `NotFound` on absent targets.
- [ ] **Step 2–4:** FAIL → implement with `DocumentMut` indexing (`doc["user"]["options"]["alice"]["value"] = value(...)`; create intermediate tables as non-inline `Table` with `set_implicit(true)` so output style matches the spec examples) → PASS.
- [ ] **Step 5:** commit `feat(core): toml_edit variable editing verbs`.

---

### Task 7: Usage scan + promote/demote composition

Spec §4 Manager integration.

**Files:**
- Modify: `crates/postui-core/src/varedit.rs`

**Interfaces (Produces):**
```rust
/// Slugs of saved requests whose url/params/headers/body/variables text
/// contains a well-formed {{name}} token (vars::find_tokens, exact name).
pub fn scan_usage(root: &Path, name: &str) -> Vec<String>;
/// Request→project: returns (new variables.toml doc, new env doc if the
/// value targets the env). Caller removes the request entry.
pub enum PromoteTarget { Default, Env }
pub fn promote_var(vars_doc: &str, env_doc: Option<&str>, name: &str, value: &str, target: PromoteTarget)
    -> Result<(String, Option<String>), EditError>;
/// Project→request is composition at the caller: delete_var(vars_doc) +
/// set_env_value(each env doc, None) — no new core verb needed. Conflict
/// (enumerated/group member) surfaces from delete_var/model checks.
```

- [ ] **Step 1: Failing tests:** scan over a temp project (`storage::save_request` fixtures) finds tokens in every field incl. `[variables]` values, ignores `{{other}}`; promote to Default writes declaration `default`, to Env writes flat env pair + bare declaration; promote onto an existing enumerated name → `Conflict`.
- [ ] **Step 2–4:** FAIL → implement (scan: `storage::list_requests` + read each file's raw text — tokens can live in any field; raw-text find_tokens is exact and cheap) → PASS.
- [ ] **Step 5:** commit `feat(core): usage scan + promote helpers`.

---

### Task 8: `ProjectContext` integration

Wires the model into the TUI state layer. No visible UI yet.

**Files:**
- Modify: `crates/postui/src/project_ctx.rs`

**Interfaces (Produces):**
```rust
pub struct ProjectContext {
    // replaces variables/env_values:
    pub model: varmodel::VarModel,
    pub env_data: varmodel::EnvData,            // empty when no env
    pub secrets: IndexMap<String, IndexMap<String, String>>,
    pub resolved: varmodel::Resolved,           // recomputed by refresh_resolved()
    /* root, meta, environments, active_env, expanded unchanged */
}
impl ProjectContext {
    pub fn refresh_resolved(&mut self);          // resolve_env for active_env (selections from local state field)
    pub fn selections_mut(&mut self) -> &mut IndexMap<String, String>;  // active env's section (empty-string key when no env)
    pub fn set_selection(&mut self, name: &str, key: &str);   // updates state, persists local state, refresh_resolved
    pub fn set_secret(&mut self, name: &str, value: String);  // writes save_secrets, refresh_resolved
    pub fn prepare_context(&self) -> PrepareContext;          // vars = resolved.values, meta = resolved.meta, default_headers as today
    // env-file/vars-file write-through used by Manager/picker tasks:
    pub fn edit_variables(&mut self, f: impl FnOnce(&str) -> Result<String, EditError>) -> Result<(), String>;
    pub fn edit_env(&mut self, env: &str, f: impl FnOnce(&str) -> Result<String, EditError>) -> Result<(), String>;
    // both: read file (missing = ""), apply f, atomic write, reload model/env, validate_env, refresh_resolved; Err(msg) for toasting
}
```
`open()`/`reload_if_changed()` load secrets + run `validate_env` (validation failure = warning toast + keep previous data, matching existing broken-file behavior). `set_env` refreshes resolved. `LocalState.selections` round-trips through `persist_local_state`.

- [ ] **Step 1: Failing tests** (existing `project_ctx` test style, tempdir fixtures): open loads options/secrets and `resolved` reflects a selection from state.toml; `set_selection` persists and re-resolves; `set_secret` writes `.local/secrets.toml` and value resolves; `edit_variables` applying `upsert_var` updates `model` and preserves a comment in the file; broken env option table on reload warns and keeps previous.
- [ ] **Step 2–4:** FAIL → implement → PASS (port `prepare_context` callers in `app.rs`).
- [ ] **Step 5:** commit `feat: ProjectContext on the stage-6 variable model`.

---

### Task 9: `Screen` enum + Manager shell (open/close)

Spec §5 entry/exit. First UI task — tmux verification applies from here on.

**Files:**
- Modify: `crates/postui/src/app.rs` (`pub screen: Screen`), `src/ui.rs` (draw branch), `src/action.rs`, `src/keys.rs`, `src/components/palette.rs` (command), `src/components/mod.rs`
- Create: `crates/postui/src/components/varmanager.rs` (shell only: title bar "Variables — <project> · <env>", empty grid area, footer hints)
- Test: `crates/postui/src/app/tests.rs`

**Interfaces (Produces):**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen { #[default] Main, VarManager }
// Actions: Action::OpenVarManager, Action::CloseScreen
// Keymap defaults: ("alt+v", Action::OpenVarManager); palette command id "var-manager" / "Variable Manager"
// Routing rules in App::handle_key: when screen == VarManager and no modal,
// keys go to self.varmanager (a VarManager component field on App);
// Esc with no modal/edit → Action::CloseScreen → Screen::Main, focus restored
// (store prior PaneId on open). ui::draw: match app.screen — VarManager draws
// full-frame instead of the three panes; header/footer stay.
```

- [ ] **Step 1: Failing tests:** `alt+v` (and the palette command) sets `screen == VarManager` and renders the Manager title via `render_once` + buffer assertion; Esc returns to `Main` with prior focus; modals still open/close on top without leaving the screen; `q` does not quit from Manager (it's not the palette) unless bound.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5 (tmux):** drive `alt+v` in/out, palette entry, resize; confirm chrome stays stable and hints read correctly.
- [ ] **Step 6:** commit `feat: Screen enum + Variable Manager shell`.

---

### Task 10: Manager grid rendering

Spec §5 layout — read-only truth display first.

**Files:**
- Modify: `crates/postui/src/components/varmanager.rs`, `src/hit.rs` (`Hit::VarCell { row, col }`, `Hit::VarRow(usize)`), `src/ui.rs`
- Test: component tests in `varmanager.rs` (TestBackend, style asserted via buffer cells like `editor.rs` tests)

**Interfaces (Produces):**
```rust
pub struct VarManager {
    pub rows: Vec<RowKind>,      // rebuilt from &ProjectContext each draw prep
    pub cursor: (usize, usize),  // (row, col); col 0 = name/desc block, 1.. = env columns
    pub env_scroll: usize,       // first visible env column
    pub scroll: usize, pub expanded: BTreeSet<String>,
}
pub enum RowKind {
    SectionHeader(&'static str),                  // "This request", "Project"
    RequestVar { name: String },
    Var { name: String },                         // simple/enumerated/secret alike
    GroupHeader { name: String },
    GroupMember { group: String, name: String },  // indented
    OptionRow { owner: String, key: String },     // indented under expanded Var/GroupHeader
    AddVar, AddGroup,                             // ghost action rows
}
pub fn build_rows(ctx: &ProjectContext, open_request: Option<&HttpRequest>, expanded: &BTreeSet<String>) -> Vec<RowKind>;
```
Cell content rules (assertable): env cell of a simple var = env value, `theme.muted` when falling back to default; enumerated/group cell = `key · value` of the selection; `●●●●` for secrets (never the value); `⚠ select` / `⚠ secret` markers for NeedsSelection/MissingSecret; option rows show key, description, per-env value with the env-override rendered in normal fg vs shared in muted; selected option's row shows `✓` in that env column.

- [ ] **Step 1: Failing tests:** fixture context (2 envs, simple+default-fallback, enumerated with selection in qa only, group of two, secret with value in one env) → assert row order (request section first when open request has variables; groups as header+indented members; expansion inserts option rows), the cell rules above by inspecting buffer cells, masked secret has no value text anywhere in the buffer, horizontal `env_scroll` hides the first env column.
- [ ] **Step 2–4:** FAIL → implement draw with `paint::` rows/pills, register `Hit::VarCell` per visible cell → PASS.
- [ ] **Step 5 (tmux):** eyeball alignment with many/long names, both themes, narrow terminal (env columns scroll), expansion glyphs.
- [ ] **Step 6:** commit `feat: Variable Manager grid rendering`.

---

### Task 11: Manager navigation + in-place value editing

Spec §5 editing (values only; structure ops are Task 12).

**Files:**
- Modify: `crates/postui/src/components/varmanager.rs`, `src/app.rs` + `src/app/mouse.rs` (route hits), `src/action.rs`
- Test: `varmanager.rs` + `app/tests.rs`

**Interfaces (Produces):**
```rust
// VarManager gains: pub editing: Option<CellEdit>
pub struct CellEdit { pub row: usize, pub col: usize, pub input: LineInput, pub masked: bool }
// Key model: arrows/Tab move cursor (skip headers/ghost rows vertically into
// nearest editable); Enter or click-on-cursor-cell begins edit (masked=true
// for secret cells; `r` toggles reveal on a secret cell without editing);
// Enter commits, Esc cancels (first Esc eats — screen close needs a second).
// Commit dispatch by cell kind → Action::VarEdit(VarEditOp) applied in App:
pub enum VarEditOp {
    SetEnvValue { env: String, name: String, value: String },          // varedit::set_env_value via ctx.edit_env
    SetDefault { name: String, value: String },                        // varedit::upsert_var
    SetDescription { owner: String, value: String },
    SetSecretValue { env: String, name: String, value: String },       // ctx.set_secret
    SetOptionValue { env: String, owner: String, key: String, member: Option<String>, value: String }, // env override via upsert_env_option; shared cell edits shared doc
    SetRequestVar { name: String, value: String },                     // editor.request mutation + save (existing dirty/save path)
    Select { env: String, name: String, key: String },                 // ctx.set_selection (the ✓ action, also used by picker)
}
```
Write failures (Err from edit_/set_ helpers) toast and keep `editing` open with the typed text (spec §5).

- [ ] **Step 1: Failing tests:** cursor movement skips headers; Enter on each cell kind produces the right `VarEditOp` and the file content changes accordingly (tempdir ctx; assert re-resolved value shows in the next frame); secret edit is masked and commit lands in secrets.toml; failed write (read-only dir) toasts and edit stays; click selects cell, click-again edits (mouse parity).
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5 (tmux):** edit flow feel — type/commit/cancel, reveal toggle, wrong-input recovery, wheel + drag over the grid.
- [ ] **Step 6:** commit `feat: Manager cell editing writes through`.

---

### Task 12: Manager structural actions

Spec §5 action list + §4 promote/demote + §3 flag transitions. Everything-GUI-editable lands here.

**Files:**
- Modify: `crates/postui/src/components/varmanager.rs`, `src/components/modal.rs` (`PromptKind::{NewVariable, NewGroup, NewOption { owner }, RenameVariable { from }}`), `src/action.rs`, `src/app.rs`
- Test: `app/tests.rs`

**Interfaces (Produces):** keyboard + painted-button per action (footer shows the keys):
```rust
// n=new variable, g=new group, o=new option (on an enumerated/group row),
// F2/`=`=rename, d/Delete=delete, s=toggle secret, m=edit group members,
// p=promote (request row), P=demote (project row), Space/✓-click=select option
pub enum VarStructOp {
    NewVar { name: String, description: Option<String> },
    NewGroup { name: String, members: Vec<String> },        // members prompt = comma-separated, validated
    NewOption { owner: String, key: String, description: Option<String>, values: IndexMap<String, String> },
    Rename { from: String, to: String },
    Delete { name: String },                                 // Confirm modal first, body includes scan_usage list
    ToggleSecret { name: String },                           // §3 transition modals
    SetMembers { group: String, members: Vec<String> },
    Promote { name: String, target: PromoteTarget },         // choice modal: default / env
    Demote { name: String },                                 // Confirm with usage count; refuses enumerated/group with message modal
}
```
All apply through `ctx.edit_variables`/`edit_env`/request save; delete of a selected/edited row clamps cursor (table_editor precedent).

- [ ] **Step 1: Failing tests** (app-level, tempdir project): each op end-to-end file assertion; delete shows Confirm listing "referenced by N requests" from `scan_usage`; demote on enumerated shows the refusal message and changes nothing; secret→non-secret leaves secrets.toml untouched and shows the copy-offer modal; non-secret→secret moves env values into secrets.toml and strips them from env files; every op reachable by click (Hit) and by key.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5 (tmux):** run the full create-organize story by hand: new vars → options → group them → rename → env values → select ✓ → promote/demote; judge prompt wording and step count.
- [ ] **Step 6:** commit `feat: Manager structural actions — GUI-complete variable editing`.

---

### Task 13: Editor Vars tab

Spec §4 Vars tab. Independent of 10–12 (after 9).

**Files:**
- Modify: `crates/postui/src/components/editor.rs` (`EditorTab::Vars` after Headers; table_editor reuse over `request.variables`; count in tab label; shadow hint), `src/app.rs` (tab actions/count plumbing)
- Test: `editor.rs` tests

- [ ] **Step 1: Failing tests:** Vars tab renders `[variables]` entries with the shared table editor (selection/expand/ghost-add/delete-confirm all function — reuse the existing table tests' patterns pointed at the new tab); tab label shows `Vars · N`; a row shadowing a resolved project var renders the dim `overrides <env>: <value>` hint in its expanded row (editor receives the shadowed value via a `pub shadowed: IndexMap<String, String>` set from `App` out of `ctx.resolved`); alt+1/2/3 aliases unaffected, `EditorTabCycle` order Params→Headers→Vars→Body.
- [ ] **Step 2–4:** FAIL → implement → PASS (dirty/save path identical to params — `[variables]` persists via Task 5's serializer).
- [ ] **Step 5 (tmux):** tab feel, add/override/disable a var, watch send behavior change.
- [ ] **Step 6:** commit `feat: request Vars tab`.

---

### Task 14: Picker — selection context + group preview

Spec §6 first context.

**Files:**
- Modify: `crates/postui/src/components/var_picker.rs`, `src/app.rs` (invoke rules: `ctrl+v`/click with cursor on a `{{name}}` token whose name is enumerated-or-group-member in the active env → selection mode; needs-selection send block gains a "press ctrl+v" hint naming the first such variable)
- Test: `var_picker.rs` + `app/tests.rs`

**Interfaces (Produces):**
```rust
pub enum PickerMode {
    Insert,                                   // existing behavior (Task 15 upgrades)
    SelectOption { name: String, group: Option<String> },
}
// SelectOption rows = merged options for the active env: "key  description  value",
// current selection marked ✓; group form previews every member:
// "alice — admin · user_id 1001 · customer_id c-77". Typing filters (existing
// fuzzy). Enter → Action::VarEdit(VarEditOp::Select { .. }) + toast
// "user → alice (qa)"; token text untouched. Last row "＋ add new option…" is
// Task 17 (render it disabled/hidden until then — no dead click).
```

- [ ] **Step 1: Failing tests:** cursor-on-token opens SelectOption with the right rows and ✓; group member shows group's options with full member preview text; Enter writes selection via `set_selection` (state.toml assertion) and does not modify the URL/field text; filter narrows; send-block toast names needs-selection vars distinctly (uses Task 5 causes).
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5 (tmux):** flow: blocked send → picker → select → resend; group preview legibility at narrow widths.
- [ ] **Step 6:** commit `feat: option-selection picker with group preview`.

---

### Task 15: Picker — insert context upgrade

Spec §6 second context.

**Files:**
- Modify: `crates/postui/src/components/var_picker.rs`, `src/components/modal.rs` (`PromptKind::NewVariable` reuse from Task 12)
- Test: `var_picker.rs`

- [ ] **Step 1: Failing tests:** insert list covers all defined names (project vars, group members, request vars of the open request) with scope badge chips (`req`/`proj`/`grp`/`🔒` — secret badge on secret vars) and descriptions; Enter inserts `{{name}}` (existing behavior retained); final row "new variable…" opens the NewVariable prompt pre-filled with the typed filter text, and on confirm creates the var (via `VarStructOp::NewVar`) *and* inserts `{{name}}` at the original cursor (focus-return assertion).
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5 (tmux):** autocomplete typing feel, badge alignment, create-and-insert round trip.
- [ ] **Step 6:** commit `feat: insert picker — badges, descriptions, create-and-insert`.

---

### Task 16: Secret prompt chain at send

Spec §3 send-time prompt.

**Files:**
- Modify: `crates/postui/src/app.rs` (send path), `src/components/modal.rs` (`PromptKind::SecretValue { name, env }`, masked `LineInput` rendering `●` per char with reveal toggle key)
- Test: `app/tests.rs`

**Interfaces (Produces):** in the send action, when `prepare` fails with only `MissingSecret` causes (mixed causes: secrets prompt first, then re-prepare surfaces the rest): push `SecretValue` prompt for the first missing name; confirm → `ctx.set_secret(name, value)` → re-run the send action (which prompts for the next, or proceeds). Esc → cancel send entirely, toast "send canceled". Empty input = invalid (re-prompt), not an empty secret.

- [ ] **Step 1: Failing tests:** two missing secrets prompt sequentially then the request actually sends (wiremock, assert the substituted header value arrived); Esc mid-chain sends nothing and secrets.toml keeps only already-confirmed values; prompt title contains name + env, body never contains any secret value; masked input shows `●●●` not the typed text (buffer assertion).
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5 (tmux):** clone-fresh scenario: delete `.local/secrets.toml`, send, answer prompts, confirm 200.
- [ ] **Step 6:** commit `feat: send-time secret prompt chain`.

---

### Task 17: In-context flows — add option, edit option, extract to variable

Spec §6 in-context editing.

**Files:**
- Modify: `crates/postui/src/components/var_picker.rs` (add/edit option), `src/components/modal.rs` (`PromptKind::{NewOptionInline { owner }, EditOption { owner, key }, ExtractVariable}` — multi-field prompts: reuse the existing prompt modal with one input per field, Tab between), `src/app.rs`, `src/action.rs` (`Action::ExtractToVariable`), `src/keys.rs` (`ctrl+shift+e` default, palette "Extract to variable")
- Test: `var_picker.rs` + `app/tests.rs`

- [ ] **Step 1: Failing tests:** picker "add new option…" prompts key/value/description, writes to the ACTIVE ENV's options table (env doc assertion, not variables.toml), selects it, closes back to the field with focus restored; `e` on a highlighted option edits value/description writing to where the option lives (shared fixture → variables.toml changes; env-override fixture → env file changes); ExtractToVariable from a focused line-input/table cell with literal text prompts name + destination (default/env/request), writes it, replaces field text with `{{name}}`, and the field is dirty-saved; extract with cursor in the body is refused with a toast (excluded this stage).
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5 (tmux):** the zero-friction story end to end: from a header value, extract → pick → add option → switch env → override — without ever opening the Manager.
- [ ] **Step 6:** commit `feat: in-context variable flows`.

---

### Task 18: Acceptance — rust test + scripted tmux workflow sweep

Spec §7 tier 3 + stage exit.

**Files:**
- Create: `crates/postui/tests/stage6_acceptance.rs`
- Create: `scripts/tmux_stage6_sweep.md` (the scripted sweep: exact send-keys sequences + expected capture-pane checks, so it is rerunnable)

- [ ] **Step 1: Rust acceptance test** (wiremock, tempdir project, TestBackend app — stage-3 acceptance style): build a project entirely through actions (no hand-written variables.toml): new vars, options, a group, secret; select via picker action; env switch flips resolved values; request var overrides; send hits wiremock with fully substituted URL/headers incl. secret; assert variables.toml/env files match expected canonical text (comments fixture preserved through one Manager edit).
- [ ] **Step 2: Run** — PASS required.
- [ ] **Step 3: Scripted tmux sweep** — execute the full workflow from the Global Constraints paragraph against the real binary, recording each check in `scripts/tmux_stage6_sweep.md` as done/issue. Any friction finding becomes a fix commit or a written deferral for the user.
- [ ] **Step 4:** fmt/clippy/full workspace tests; commit `test: stage-6 acceptance + tmux sweep script`.
- [ ] **Step 5:** report rulings/deferrals in chat; hand the user the manual real-terminal sweep checklist (spec Global Constraints — final gate).

---

## Self-review notes (already applied)

- Spec coverage: §1 → T1–T5; §2 → T3+T5; §3 → T4, T12 (transitions), T16; §4 → T5, T7, T12, T13; §5 → T9–T12; §6 → T14, T15, T17; §7 → T6 fidelity tests, T18, compat implicitly via additive parsing tests (T1 keeps stage-3 fixtures green).
- Type consistency: `VarEditOp`/`VarStructOp` defined T11/T12, consumed T14 (`Select`), T15 (`NewVar`); `PromoteTarget` defined T7, consumed T12; `Resolved`/`VarMeta` defined T3, consumed T5, T8, T10, T13, T14.
- The old `project::resolve`/`Variables` are deleted in T4 — no task may reference them after that point.
