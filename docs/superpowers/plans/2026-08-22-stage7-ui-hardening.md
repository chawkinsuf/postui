# Stage 7 — UI Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing basics rock solid — the linked-records variable model with a master-detail manager, in-place table editing, full mouse accessibility (toolbar + right-click context menus), computed request headers, inline variable previews, duplicate request, the body-click fix, and background pretty-print — no new capabilities.

**Architecture:** `postui-core` gets a rewritten variable model (groups = declared `fields` + per-environment `entries`), a `migrate` module for one-shot legacy conversion, a `computed_headers` helper beside `prepare`, and `duplicate_request` in storage. The TUI gains right-click routing through the existing `HitMap`/`Dropdown` machinery, a rewritten `table_editor` (click-cell-to-edit, commit-on-click-away), an editor toolbar row, a `Hit::VarToken` hover/tooltip layer, a background JSON-parse task copied from the send pattern, and a master-detail `varmanager` replacing the grid.

**Tech Stack:** Rust, ratatui + crossterm (via `ratatui::crossterm`), tokio, indexmap, toml / toml_edit, serde, edtui 0.11.6.

**Spec:** `docs/superpowers/specs/2026-08-22-stage7-ui-hardening-design.md` — binding. Read it before any task; sections cited per task.

## Global Constraints

- Cargo needs `export PATH="$HOME/.cargo/bin:$PATH"` in every shell (subagents too).
- Import crossterm types via `ratatui::crossterm::...` only.
- Before every commit: `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` clean, full `cargo test --workspace` green.
- All edits to shareable TOML (`variables.toml`, `environments/*.toml`, request files) go through `toml_edit` document mutation; request files persist via `HttpRequest::to_toml_string()` only (never `toml::to_string`). `.local/` files may serialize fresh.
- Painted-UI conventions for every new surface: `paint::` Button/Chip/TabStrip/TextField/floating_panel/PillRow, hover via `HitMap` (`ctx.hovered == Some(&Hit::X)`), keyboard parity for every mouse action and mouse parity for every keyboard action (spec §2, §5).
- Interaction principles (spec §2): no trap states — any click always does something sensible; `Esc`/click-away exit edit states by committing or reverting, never by ignoring input. A click that closes a popup is swallowed (one click, one effect).
- Secret values never appear in toasts, errors, logs, or unmasked tooltips — names only.
- Reserved names in `variables.toml`/env files: `options`, `groups`, `entries`.
- New `Hit` variants that must not blur table/editor state MUST be added to the `keeps_table_selection` / `keeps_editor_input` allow-lists in `app/mouse.rs:171-199` — check this for every new variant.
- **tmux visual + usability verification is REQUIRED for every UI task** (Tasks 7–17) before it is called done: hold a server with a `run_in_background` Bash call (`tmux new-session -d ... && sleep 3600`, `TMUX_TMPDIR=/tmp/claude-1000/tmux`), drive with `send-keys`, read with `capture-pane -p`, mouse via SGR sequences (right-click press is `\e[<2;COL;ROWM`, release `\e[<2;COL;ROWm`; motion/hover is `\e[<35;COL;ROWM`). Exercise the task's flows end to end; judge friction, not just rendering.
- Commit messages: plain, no Co-Authored-By, no Claude-Session trailer.

## File Structure

- `crates/postui-core/src/varmodel.rs` — REWRITE in place. `VarDecl` (no options), `GroupDecl { description, fields }`, `EntryDecl { description, values }`, `EnvData { values, entries }`, new `ModelError` variants, `parse_variables`, `parse_environment`, `validate_env`, `resolve_env` (no `VarMeta::Enumerated`). `merged_var_options`/`merged_group_options`/`OptionDecl`/`GroupOption` deleted.
- `crates/postui-core/src/varedit.rs` — REWRITE option verbs into entry verbs; group verbs use `fields`.
- `crates/postui-core/src/migrate.rs` — NEW. Legacy detection + text-level migration.
- `crates/postui-core/src/prepare.rs` — MODIFY. `computed_headers` + `ComputedHeader`/`HeaderOrigin`; `Enumerated` arm removal.
- `crates/postui-core/src/vars.rs` — MODIFY. `pub fn find_tokens`.
- `crates/postui-core/src/storage.rs` — MODIFY. `duplicate_request`.
- `crates/postui/src/project_ctx.rs` — MODIFY. New model types, migration entry points.
- `crates/postui/src/hit.rs` — MODIFY. New variants (listed per task).
- `crates/postui/src/app/mouse.rs` — MODIFY. Right-click arm, new hit dispatch, revised blanket-deselect rules.
- `crates/postui/src/components/modal.rs` — MODIFY. `MenuItem` (disabled items) in `DropdownState`.
- `crates/postui/src/components/table_editor.rs` — REWRITE interaction model (in-place editing).
- `crates/postui/src/components/editor.rs` — MODIFY. Toolbar row, computed-headers section, body click fix, token rects.
- `crates/postui/src/components/response.rs` — MODIFY. Async pretty, search buttons.
- `crates/postui/src/session.rs` — MODIFY. `tree_arrived`.
- `crates/postui/src/components/varmanager.rs` — REWRITE. Master-detail.
- `crates/postui/src/components/var_picker.rs` — MODIFY. Entries instead of options.
- `crates/postui/src/action.rs`, `src/keys.rs`, `src/components/footer.rs`, `src/components/palette.rs`, `src/components/sidebar.rs`, `src/ui.rs`, `src/app.rs` — MODIFY per task.
- `crates/postui/tests/stage7_acceptance.rs` — NEW.

Order: Tasks 1→2→3 sequential (core model). 4, 5 parallel after 1. Task 6 (compile ripple) after 2+3+4. Tasks 7–13 after 6, mostly independent. Tasks 14→15→16 (manager) after 6+8. Task 17 after 7–16. Task 18 last.

---

### Task 1: Core variable model rewrite (`varmodel.rs`)

Spec §3.1, §3.2. Groups become declared `fields` + per-env `entries`. `VarMeta::Enumerated` is deleted (one-field groups replace enumerated vars).

**Files:**
- Modify: `crates/postui-core/src/varmodel.rs` (rewrite types/parse/validate/resolve; keep module position in `lib.rs`)

**Interfaces (Produces):**
```rust
pub struct VarDecl { pub description: Option<String>, pub default: Option<String>, pub secret: bool } // Debug, Clone, Default, PartialEq
pub struct GroupDecl { pub description: Option<String>, pub fields: Vec<String> }
pub struct EntryDecl { pub description: Option<String>, pub values: IndexMap<String, String> }        // field -> value
pub struct VarModel { pub vars: IndexMap<String, VarDecl>, pub groups: IndexMap<String, GroupDecl> }  // Default
pub struct EnvData {                                                                                  // Default
    pub values: IndexMap<String, String>,
    pub entries: IndexMap<String, IndexMap<String, EntryDecl>>,   // group -> entry name -> EntryDecl
}
pub enum VarMeta { Simple, GroupMember { group: String, selected: String }, Secret, NeedsSelection, MissingSecret }
pub type Selections = IndexMap<String, String>;   // group name (or legacy var name) -> entry name — unchanged
pub type SecretValues = IndexMap<String, String>; // unchanged
pub struct Resolved { pub values: IndexMap<String, String>, pub meta: IndexMap<String, VarMeta> }     // unchanged shape

pub fn parse_variables(s: &str) -> Result<VarModel, ModelError>;
pub fn parse_environment(s: &str) -> Result<EnvData, ModelError>;
pub fn validate_env(model: &VarModel, env: &EnvData) -> Result<(), ModelError>;
pub fn resolve_env(model: &VarModel, env: &EnvData, selections: &Selections, secrets: &SecretValues) -> Resolved;
pub fn group_entries<'a>(env: &'a EnvData, group: &str) -> Option<&'a IndexMap<String, EntryDecl>>;
```
`ModelError`: keep `Toml`, `ReservedName`, `InvalidName`, `NameCollision`, `NotATable`, `UnknownField`, `NotAString`, `NotABool`, `SecretWithDefault`, `EnvValueForSecret`, `EnvValueForGroup` (flat env value naming a group), and rename `MembersNotArray`→`FieldsNotArray{group}`, `MissingMembers`→`MissingFields{group}`, `MemberIsSecret`→`FieldIsSecret{group,field}` (a field name may not be a declared secret var), `EnvValueForGroupMember`→`EnvValueForField{name,group}`. Delete every options-related variant. Add: `EntriesNotTable(String)`, `EntryNotTable{group,entry}`, `EntryFieldNotString{group,entry,field}`, `EntryForUndeclaredGroup(String)`, `EntryMissingField{group,entry,field}`, `EntryUnknownField{group,entry,field}`, `EntryEmptyName{group}`, `FieldCollidesWithVar{group,field}` (a field may not also be a plain declared variable), `FieldInMultipleGroups{field,first,second}`. Each keeps the friendly-message-with-fix style. `RESERVED_NAMES` becomes `["options", "groups", "entries"]`.

Namespace rules (spec §3.1/§3.2 + migration need): **field names and var names share one namespace** (tokens `{{...}}` resolve vars ∪ fields); **group names share the selections namespace with nothing else** and may NOT collide with a var name or another group, but a group MAY share its name with one of its own fields (the one-field-group migration case). Field names obey `vars::is_valid_var_name`; entry names are free-form non-empty strings (quoted TOML keys), no length/charset limit, but not `description`.

- [ ] **Step 1: Rewrite the test module first** (replace the 41 old tests; old option/merge tests are deleted, not ported). Core round-trip test:

```rust
#[test]
fn parses_vars_and_groups_with_fields() {
    let m = parse_variables(r#"
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
secret = true

[groups.user]
description = "Linked user/customer pair"
fields = ["user_id", "customer_id"]
"#).unwrap();
    assert_eq!(m.groups["user"].fields, ["user_id", "customer_id"]);
    assert!(m.vars["api_key"].secret);
    assert!(m.vars["base_url"].default.is_some());
}

#[test]
fn parses_env_entries_and_resolves_selection() {
    let m = parse_variables("[groups.user]\nfields = [\"user_id\", \"customer_id\"]\n").unwrap();
    let e = parse_environment(r#"
base_url = "https://stg.example.com"

[entries.user."user 1"]
user_id = "1001"
customer_id = "cust-77"

[entries.user."user 2"]
description = "the premium one"
user_id = "1002"
customer_id = "cust-91"
"#).unwrap();
    assert_eq!(e.entries["user"]["user 2"].values["customer_id"], "cust-91");
    assert_eq!(e.entries["user"]["user 2"].description.as_deref(), Some("the premium one"));
    let mut sel = Selections::new();
    sel.insert("user".into(), "user 2".into());
    let r = resolve_env(&m, &e, &sel, &SecretValues::new());
    assert_eq!(r.values["user_id"], "1002");
    assert_eq!(r.meta["customer_id"], VarMeta::GroupMember { group: "user".into(), selected: "user 2".into() });
}
```

Plus one test per error/validation rule (exact-message `contains` assertions): reserved name `entries` as a var; group colliding with a var; field in two groups; field that is itself a declared var (`FieldCollidesWithVar`); field declared `secret = true` elsewhere (`FieldIsSecret`); `fields` missing / not an array; entry table for undeclared group; entry missing a field / with an extra field; entry field non-string; empty entry name; flat env value naming a group or a field; flat env value for a secret; `description` inside an entry does NOT count as a field. Resolve tests: no selection → every field `NeedsSelection` and absent from `values`; stale selection (entry deleted) → `NeedsSelection`; secret precedence over flat value; decl `default` fallback; undeclared env values pass through into `values` with no meta (preserved behavior); a group sharing its name with its own single field resolves that field via the selection.

- [ ] **Step 2: Run** `cargo test -p postui-core varmodel` — expect FAIL (types changed).
- [ ] **Step 3: Rewrite the module.** Keep the manual `toml::Table` walk. `groups` table → `GroupDecl` (`fields` key); `entries` in `parse_environment` → nested walk pulling out `description` per entry. `validate_env` enforces the entry rules; `resolve_env` precedence per name: secret value → (field) selected entry's value → flat env value → decl default; a group's selection missing/stale → all its fields get `NeedsSelection`. `group_entries` is a trivial lookup returning `None` when absent.
- [ ] **Step 4: Run tests** — PASS. (The workspace will NOT fully build yet — dependents break. `cargo test -p postui-core --lib varmodel` may itself fail to compile because varedit/prepare are in the same crate; if so, comment nothing out — proceed to Task 2/4 in the same working state and commit only when `cargo test -p postui-core` compiles at the end of Task 4. Tasks 1–4 may share commits per the checkpoints below.)
- [ ] **Step 5:** Commit checkpoint happens at end of Task 4 (core crate green): this task contributes `feat(core): linked-records variable model`.

---

### Task 2: `varedit.rs` entry verbs

Spec §3.2, §3.4 operations. Option verbs become entry verbs; group verbs speak `fields`.

**Files:**
- Modify: `crates/postui-core/src/varedit.rs`

**Interfaces (Produces):** unchanged: `EditError`, `upsert_var`, `set_secret_flag`, `rename_var`, `delete_var`, `set_env_value`, `rename_env_var`, `delete_env_var`, `scan_usage`, `PromoteTarget`, `promote_var`. `set_secret_flag(true)` drops its `Conflict`-on-options case (no options exist). Deleted: `upsert_shared_option`, `delete_shared_option`, `upsert_env_option`, `delete_env_option`, `remove_group_member`, `strip_env_group_member`. Changed/new:
```rust
pub fn upsert_group(doc: &str, name: &str, description: Option<&str>, fields: &[String]) -> Result<String, EditError>; // writes `fields = [...]`
pub fn delete_group(doc: &str, name: &str) -> Result<String, EditError>;                    // unchanged signature
// environments/<env>.toml
pub fn upsert_entry(doc: &str, group: &str, entry: &str, description: Option<&str>,
                    values: &IndexMap<String, String>) -> Result<String, EditError>;        // creates [entries.<group>."<entry>"]
pub fn rename_entry(doc: &str, group: &str, from: &str, to: &str) -> Result<String, EditError>;   // Conflict if `to` exists
pub fn delete_entry(doc: &str, group: &str, entry: &str) -> Result<String, EditError>;      // NotFound if absent
pub fn delete_group_entries(doc: &str, group: &str) -> Result<String, EditError>;           // no-op if absent (for delete_group cascade)
pub fn rename_entry_field(doc: &str, group: &str, from: &str, to: &str) -> Result<String, EditError>; // renames the key inside every entry of the group; no-op per entry if absent
pub fn strip_entry_field(doc: &str, group: &str, field: &str) -> Result<String, EditError>; // removes the key from every entry; no-op if absent
```

- [ ] **Step 1: Rewrite affected tests** (keep the var-verb tests; replace option-verb tests):

```rust
#[test]
fn upsert_entry_creates_and_updates_preserving_other_entries() {
    let doc = "base_url = \"x\"\n";
    let mut vals = IndexMap::new();
    vals.insert("user_id".to_string(), "1001".to_string());
    vals.insert("customer_id".to_string(), "cust-77".to_string());
    let out = upsert_entry(doc, "user", "user 1", None, &vals).unwrap();
    assert!(out.contains("[entries.user.\"user 1\"]"));
    let mut vals2 = vals.clone();
    vals2.insert("user_id".to_string(), "9999".to_string());
    let out2 = upsert_entry(&out, "user", "user 1", Some("admin"), &vals2).unwrap();
    assert!(out2.contains("9999") && out2.contains("description = \"admin\""));
    assert_eq!(out2.matches("[entries.user.").count(), 1);
}
```

Plus: `rename_entry` conflict + success (preserves value order and comments elsewhere in the doc); `delete_entry` NotFound; `delete_group_entries` removes the whole `[entries.<group>]` subtree and is a no-op when absent; `rename_entry_field`/`strip_entry_field` touch every entry; `upsert_group` writes `fields` and round-trips through `parse_variables`; every verb's output re-parses via `parse_variables`/`parse_environment` (add a helper `fn reparses_env(s: &str)` used in each env-verb test).

- [ ] **Step 2:** `cargo test -p postui-core varedit` — FAIL.
- [ ] **Step 3: Implement** with `toml_edit::DocumentMut`; entry names need `toml_edit::Key` quoting (use `doc["entries"][group][entry]` indexing which quotes automatically; verify a name with spaces and a `"` in a test). `description` is written first in an entry table when present.
- [ ] **Step 4:** tests PASS (crate may still not fully compile until Task 4 — same checkpoint rule as Task 1).

---

### Task 3: Legacy migration (`migrate.rs`)

Spec §3.3.

**Files:**
- Create: `crates/postui-core/src/migrate.rs`
- Modify: `crates/postui-core/src/lib.rs` (add `pub mod migrate;`)

**Interfaces (Produces):**
```rust
pub struct MigrationOutcome {
    pub variables: Option<String>,            // new variables.toml text, None if unchanged
    pub envs: Vec<(String, String)>,          // (env name, new text) — only changed envs
    pub new_default_env: Option<String>,      // Some(text) => write environments/default.toml
    pub notes: Vec<String>,                   // human-readable, shown in the confirm modal
}
/// True if variables.toml uses stage-6 syntax ([<var>.options], [groups.<g>] with `members`, [groups.<g>.options]),
/// or any env doc has a top-level [options] table.
pub fn needs_migration(vars_doc: &str, env_docs: &[(String, String)]) -> bool;
pub fn migrate(vars_doc: &str, env_docs: &[(String, String)]) -> Result<MigrationOutcome, varedit::EditError>;
```

Rules (from spec §3.3): var with `[<var>.options]` → one-field group of the same name (`fields = [<var>]`), the var table itself is deleted (its `description` moves to the group; a `default` on an enumerated var was already illegal). Group `members` → `fields`. Declaration-level option values → entries in **every** env doc (env `[options.<name>.<key>]` overrides merged on top, per-field); option `description`s become entry descriptions. If `env_docs` is empty and any entries were produced, they go to `new_default_env`. Selections need no rewrite (keys/names unchanged). Env-only option keys (keys present in an env's `[options]` but not declared) become entries of that env only. `notes` records: each var converted to a group, dropped constructs (none expected — assert nothing is silently lost: any unrecognized legacy shape returns `EditError::Parse` with the offending path), and "created environments/default.toml" when applicable.

- [ ] **Step 1: Write failing tests.** A full fixture mirroring the user's real shapes:

```rust
#[test]
fn migrates_enumerated_var_group_and_env_overrides() {
    let vars = r#"
[tier]
description = "pricing tier"
[tier.options.gold]
description = "the good one"
value = "g-1"
[tier.options.free]
value = "f-1"

[groups.user]
members = ["user_id", "customer_id"]
[groups.user.options.alice]
user_id = "1001"
customer_id = "c-77"
"#;
    let qa_env = ("qa".to_string(), "[options.tier.gold]\nvalue = \"g-qa\"\n".to_string());
    let out = migrate(vars, &[qa_env]).unwrap();
    let new_vars = out.variables.unwrap();
    let m = crate::varmodel::parse_variables(&new_vars).unwrap();
    assert_eq!(m.groups["tier"].fields, ["tier"]);
    assert_eq!(m.groups["user"].fields, ["user_id", "customer_id"]);
    assert!(m.vars.is_empty());
    let (_, qa_text) = &out.envs[0];
    let e = crate::varmodel::parse_environment(qa_text).unwrap();
    assert_eq!(e.entries["tier"]["gold"].values["tier"], "g-qa");     // env override won
    assert_eq!(e.entries["tier"]["free"].values["tier"], "f-1");
    assert_eq!(e.entries["tier"]["gold"].description.as_deref(), Some("the good one"));
    assert_eq!(e.entries["user"]["alice"].values["customer_id"], "c-77");
    crate::varmodel::validate_env(&m, &e).unwrap();
}
```

Plus: no-env project → `new_default_env` populated + note; already-new-format input → `needs_migration` false and `migrate` returns all-None/empty; plain vars and secrets pass through untouched; comments elsewhere in env docs survive (toml_edit); a `[groups.g]` with both `members` and `fields` errors.

- [ ] **Step 2:** run — FAIL. **Step 3: Implement** (parse legacy shapes with raw `toml::Table` walking — the old parser is gone; write entries via Task-2 verbs). **Step 4:** run — PASS.

---

### Task 4: `prepare.rs` + `vars.rs` + core ripple; commit checkpoint

Spec §3.2 (resolution), §6 (computed headers), §7 (token spans).

**Files:**
- Modify: `crates/postui-core/src/prepare.rs`, `crates/postui-core/src/vars.rs`, `crates/postui-core/src/project.rs` (only if it references removed types — `load_variables`/`load_environment` signatures are unchanged)

**Interfaces (Produces):**
```rust
// vars.rs
pub struct TokenSpan { pub start: usize, pub end: usize, pub name: String }   // byte range incl. braces
pub fn find_tokens(text: &str) -> Vec<TokenSpan>;   // same token grammar as substitution (optional inner ws); malformed tokens skipped

// prepare.rs
pub struct ComputedHeader { pub name: String, pub value: String, pub origin: HeaderOrigin, pub unresolved: Vec<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderOrigin { Request, DefaultHeader { suppressed: bool }, AutoContentType, Client }
/// Everything the client will send, in send order. Never errors: unresolved tokens stay literal
/// and are listed in `unresolved`. When mask_secrets, values of vars whose ctx.meta is Secret
/// substitute as "●●●●●●".
pub fn computed_headers(req: &HttpRequest, ctx: &PrepareContext, mask_secrets: bool) -> Vec<ComputedHeader>;
```
`prepare()` itself: only the `VarMeta::Enumerated` match arm disappears (it fell to `Undefined` anyway — verify no behavior test breaks). Client rows in `computed_headers`: `Host` (substitute the URL, then strip `scheme://`, take up to the first `/`, `?`, or `#`; omit the row if the remaining host still contains `{{`), `Content-Length` (byte length of the body after substitution when `req.substitute_body`, else of the raw body; omit when no body). Suppression logic mirrors `prepare`'s case-insensitive default-header suppression.

- [ ] **Step 1: Failing tests.** `find_tokens`: plain, `{{ spaced }}`, malformed `{{a b}}` skipped, adjacent tokens, byte offsets exact. `computed_headers`: request header overrides default (default row present with `suppressed: true`); auto Content-Type appears only with a body and no explicit content-type; explicit content-type wins case-insensitively; secret masked when `mask_secrets` and not when false; unresolved `{{ghost}}` stays literal and is listed; Host/Content-Length rows; Host omitted for unresolved host.

```rust
#[test]
fn computed_headers_shows_suppressed_default_and_masked_secret() {
    let mut req = HttpRequest::default();
    req.url = "https://api.example.com/v1/users".into();
    req.headers.insert("X-Auth".into(), Entry { value: "{{api_key}}".into(), enabled: true });
    let mut ctx = PrepareContext::default();
    ctx.vars.insert("api_key".into(), "s3cret".into());
    ctx.meta.insert("api_key".into(), varmodel::VarMeta::Secret);
    ctx.default_headers.insert("X-Auth".into(), Entry { value: "default".into(), enabled: true });
    let rows = computed_headers(&req, &ctx, true);
    let auth: Vec<_> = rows.iter().filter(|r| r.name.eq_ignore_ascii_case("x-auth")).collect();
    assert!(auth.iter().any(|r| r.origin == HeaderOrigin::Request && r.value == "●●●●●●"));
    assert!(auth.iter().any(|r| r.origin == (HeaderOrigin::DefaultHeader { suppressed: true })));
    assert!(rows.iter().any(|r| r.name == "Host" && r.value == "api.example.com"));
}
```

- [ ] **Step 2:** FAIL. **Step 3: Implement** — factor `prepare`'s substitution into a shared private helper both paths call, with a `mask: &dyn Fn(&str) -> bool` (or a simple `&BTreeSet<String>` of secret names) so masking substitutes the mask string.
- [ ] **Step 4:** `cargo test -p postui-core` — the WHOLE core crate must now compile and pass (Tasks 1–4 land together).
- [ ] **Step 5:** fmt, clippy (core), commit: `feat(core): linked-records variable model, migration, computed headers, token spans`.

---

### Task 5: `storage::duplicate_request`

Spec §8.

**Files:**
- Modify: `crates/postui-core/src/storage.rs`

**Interfaces (Produces):**
```rust
/// Copies <slug>.toml to <slug>-copy.toml (then -copy-2, -copy-3, … on collision), byte-identical
/// content (raw file copy — round-tripping through parse would reorder). Returns the new slug.
pub fn duplicate_request(root: &Path, slug: &str) -> Result<String, StorageError>;
```

- [ ] **Step 1: Failing tests:** duplicate produces `users/list-copy` next to `users/list` with identical bytes; duplicating again yields `users/list-copy-2`; duplicating a broken-TOML file still copies it (no parse involved); `NotFound` for a missing slug; the generated slug passes `validate_slug`.
- [ ] **Step 2:** FAIL. **Step 3:** implement with `std::fs::read` + the existing NamedTempFile-persist pattern from `save_request`. **Step 4:** PASS. **Step 5:** fmt/clippy/test, commit `feat(core): duplicate_request`.

---

### Task 6: TUI compile ripple + migration flow

Bring `crates/postui` back to green against the new core, and wire the migration confirm (spec §3.3).

**Files:**
- Modify: `crates/postui/src/project_ctx.rs`, `src/components/var_picker.rs`, `src/components/varmanager.rs` (minimal stub-level fixes only — full rewrite is Tasks 14–16), `src/app.rs`, `src/action.rs`

**Interfaces (Produces):**
```rust
// project_ctx.rs
impl ProjectContext {
    /// Some(outcome) when the on-disk files are legacy; computed during open()/reload_if_changed().
    pub fn pending_migration(&self) -> Option<&postui_core::migrate::MigrationOutcome>;
    /// Writes outcome files atomically, leaving `<file>.bak` beside each rewritten file
    /// (and creating environments/default.toml when new_default_env is Some), then reloads.
    pub fn apply_migration(&mut self) -> Result<Vec<String>, String>;   // Ok(notes-for-toast)
    pub fn decline_migration(&mut self);   // clears pending; model stays Default (vars inert), project loads
}
// action.rs
Action::ApplyMigration,
Action::DeclineMigration,
```

- [ ] **Step 1:** Mechanical ripple: replace `members`→`fields`, `merged_*` call sites with `group_entries`, delete `Enumerated` arms (`app.rs:2122` keeps only the `GroupMember` case → `(name, Some(group))` picker opening; the picker's `SelectOption { group: Option<String> }` mode simplifies — `group` is now always known, change it to `group: String` and fix `new_select` callers). `varmanager.rs`: patch just enough to compile (`OptionRow`→ temporary `EntryRow { group, entry }`, option ops → entry ops mapped to Task-2 verbs; the component is replaced in Task 14 — do not polish). `var_picker.rs`: `insert_entries` gains group fields as `VarScope::Group` rows; `SelectEntry.values` comes from `EntryDecl.values`, `description` from `EntryDecl.description`.
- [ ] **Step 2: Migration flow test** (in `app/tests.rs`): open a temp project written with stage-6 syntax → a `Modal::Confirm` titled "Migrate variables" is on the stack listing the notes; choosing `y` runs `Action::ApplyMigration` → files rewritten (assert `variables.toml.bak` exists, new text parses), toast shown; choosing `n` → project open, variables empty, no crash; legacy project with zero envs → `environments/default.toml` created on apply.
- [ ] **Step 3: Implement**: `ProjectContext::open`/`reload_if_changed` call `migrate::needs_migration` on the raw texts before parsing; when legacy, skip parsing (model stays `Default`) and stash the computed `MigrationOutcome`. `App` pushes the confirm modal when `pending_migration().is_some()` after open/reload. `.bak` files: plain `fs::write` (they're the safety copy, not the live file).
- [ ] **Step 4:** `cargo test --workspace` — everything compiles; pre-existing UI tests that exercised option rows are updated to entries in the minimal way (full manager tests rewritten in Tasks 14–16).
- [ ] **Step 5:** fmt/clippy/test, commit: `feat: new variable model wired through the TUI + migration prompt`.

---

### Task 7: Right-click context menus (infrastructure + sidebar + duplicate request)

Spec §5 (context-menu component), §8 (duplicate). Reuses the Dropdown machinery (anchor/flip/click-away already work — modal.rs:1079).

**Files:**
- Modify: `crates/postui/src/components/modal.rs`, `src/app/mouse.rs`, `src/app.rs`, `src/action.rs`, `src/keys.rs`, `src/components/palette.rs`, `src/hit.rs`

**Interfaces (Produces):**
```rust
// modal.rs
pub struct MenuItem { pub label: String, pub action: Option<Action> }   // None = disabled (dimmed, click/Enter ignored)
pub struct DropdownState { pub anchor: Rect, pub items: Vec<MenuItem>, pub selected: usize, pub current: Option<usize> }
// app.rs
impl App {
    /// Push a Dropdown anchored at the pointer (1x1 rect at (x,y)); flip/clamp logic is draw_dropdown's.
    pub fn open_context_menu(&mut self, x: u16, y: u16, items: Vec<MenuItem>) -> bool;
    /// Builds the item list for a right-clicked hit; None = no menu for this hit.
    fn context_menu_for(&mut self, hit: &Hit) -> Option<Vec<MenuItem>>;   // private
}
// action.rs
Action::DuplicateRequest,          // unit variant; acts on sidebar.selected_slug()
```

- [ ] **Step 1: Failing tests** (stage4-style, in `app/tests.rs` or `tests/stage7_acceptance.rs` started here):

```rust
fn right_down(x: u16, y: u16) -> MouseEvent {
    MouseEvent { kind: MouseEventKind::Down(MouseButton::Right), column: x, row: y, modifiers: KeyModifiers::NONE }
}

#[test]
fn right_click_sidebar_row_opens_menu_and_duplicate_creates_copy() {
    let mut app = App::new_for_test();                    // seed one request "users/list"
    render(&mut app);
    let r = app.hits.rect_of(&Hit::SidebarRow(0)).unwrap();
    app.handle_mouse(right_down(r.x + 2, r.y));
    assert!(matches!(app.modals.top(), Some(Modal::Dropdown(_))));   // items: Open, Duplicate, Rename…, Delete…
    render(&mut app);
    // click the "Duplicate" row
    let dup = app.hits.rect_of(&Hit::DropdownRow(1)).unwrap();
    app.handle_mouse(left_down(dup.x + 1, dup.y));
    assert!(postui_core::storage::request_exists(app.project.root(), "users/list-copy"));
    assert_eq!(app.editor.slug.as_deref(), Some("users/list-copy"));  // opened
}
```

Plus: right-click on empty space → no modal; disabled `MenuItem { action: None }` click leaves the menu open and runs nothing; click outside the menu closes it and does NOT activate what's underneath (assert the underlying request did not open); Esc closes; right-click a folder row → menu with "New request here" (prefills the new-request prompt with `<folder>/`) and Expand/Collapse; keyboard path `Action::DuplicateRequest` from the palette works on the selected row.
- [ ] **Step 2:** FAIL. **Step 3: Implement.** `handle_mouse` gains a `MouseEventKind::Down(MouseButton::Right)` arm before the `_ => false`: resolve `hit_at`, run the same select-first side effects as left-click for `SidebarRow`/`TableRow` (set `selected`), then `context_menu_for` → `open_context_menu`. Menu tables (this task): `SidebarRow` request → Open / Duplicate / Rename… / Delete…; `SidebarRow` folder → New request here / Expand-Collapse; broken row → Show error. `draw_dropdown` renders `action: None` items in `theme` muted with no hover fill; `on_hit` `DropdownRow(i)` arm ignores disabled items (don't pop). Method-dropdown call sites wrap their `(String, Action)` pairs into `MenuItem`s. `DuplicateRequest` handler: `selected_slug()` → `storage::duplicate_request` → refresh listing → `Action::OpenRequest(new_slug)`; toast on error. Registered as `("request_duplicate", Action::DuplicateRequest)` in BOTH keys.rs tables (default combo `ctrl+shift+d`) and as palette `Command { id: "request-duplicate", name: "Request: duplicate" }`.
- [ ] **Step 4:** PASS. tmux sweep: right-click via SGR `\e[<2;COL;ROWM` on a row; verify menu paints at pointer, flips near bottom edge, click-away swallows. **Step 5:** fmt/clippy/test, commit `feat: right-click context menus + duplicate request`.

---

### Task 8: In-place table editing (`table_editor.rs` rewrite)

Spec §4. The select-then-edit trap dies here.

**Files:**
- Modify: `crates/postui/src/components/table_editor.rs`, `src/hit.rs`, `src/app/mouse.rs`, `src/components/editor.rs` (draw call), `src/app.rs`

**Interfaces (Produces):**
```rust
// hit.rs — replaces TableRow/TableAdd click semantics; TableRow stays for the row rect (context menu, hover)
Hit::TableCell { row: usize, col: u8 },   // col 0 = key, 1 = value; row == map.len() is the ghost row
// table_editor.rs
pub struct CellEdit { pub row: usize, pub col: Col, pub input: LineInput, pub original: String }  // original = pre-edit cell text for Esc-revert
pub struct TableEditorState { pub selected: Option<usize>, pub editing: Option<CellEdit> }
impl TableEditorState {
    /// Click entry point. Commits any in-progress edit first (returning its warning), then begins
    /// editing (row, col) with the caret at the end. row == map.len() targets the ghost row.
    pub fn click_cell(&mut self, row: usize, col: Col, map: &mut IndexMap<String, Entry>) -> TableOutcome;
    /// Commit whatever is being edited (used by click-away and focus loss). Empty-key new rows are discarded.
    pub fn commit(&mut self, map: &mut IndexMap<String, Entry>) -> TableOutcome;
    /// Esc: revert the active cell to `original` and leave editing; row survives if it existed.
    pub fn revert(&mut self, map: &mut IndexMap<String, Entry>);
    pub fn handle_key(&mut self, ev: KeyEvent, map: &mut IndexMap<String, Entry>) -> TableOutcome;  // reworked, below
    // reset/delete_row/active_index/draw/table_height keep their signatures (draw now renders the ghost row
    // as a normal empty row labeled by add_label when not editing, and registers Hit::TableCell per cell)
}
```
Keyboard model: nav (no edit): arrows/`j`/`k` move `selected` (ghost row included), `Enter` edits the key cell of the selected row, `Space` toggles enabled, `d`/`Delete` requests delete, `Esc` deselects. Editing: `Tab`/`Shift-Tab` commit the cell and move right/left, wrapping to the next/previous row's first/last cell (wrapping past the ghost row commits and exits); `Enter` commits the row and exits; `Esc` reverts the cell and exits; other keys → `LineInput`. Duplicate-key collapse keeps its warning path.

- [ ] **Step 1: Rewrite the test module** — key sequences per the model above plus mouse-level tests in `app/tests.rs`:

```rust
#[test]
fn click_cell_edits_in_place_and_click_away_commits() {
    let mut app = App::new_for_test();     // open request, Params tab, one row page=1
    render(&mut app);
    let cell = app.hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).unwrap();
    app.handle_mouse(left_down(cell.x + 1, cell.y));
    assert!(app.editor.table.editing.is_some());
    app.update(Action::Key(key(KeyCode::Char('2'))));        // types into the value cell — adapt to the app's key path
    // click on the URL bar = click-away
    let url = app.hits.rect_of(&Hit::UrlBar).unwrap();
    app.handle_mouse(left_down(url.x + 1, url.y));
    assert!(app.editor.table.editing.is_none());
    assert_eq!(app.editor.params["page"].value, "12");        // committed, not discarded
}
```

Plus: ghost-row click → typing creates the row; ghost left empty + click away → no row; Esc mid-edit → original value back; double-click is inert beyond what single click did (no distinct action — regression test that two fast clicks on a cell leave exactly one edit session); checkbox/delete clicks still work during someone else's row edit (they commit first via the blanket rule).
- [ ] **Step 2:** FAIL. **Step 3: Implement.** `editor.rs` draw registers `Hit::TableCell` for the key/value halves of each row including the ghost. `app/mouse.rs`: `Hit::TableCell` arm calls `click_cell`; the blanket `keeps_table_selection` rule changes meaning — a click on anything NOT in the allow-list first calls `table.commit(map)` (not a silent deselect) and surfaces the outcome warning as a toast; `TableCell`/`TableCheckbox`/`TableDelete`/`TableCollapse` join the allow-list. Delete `Hit::TableAdd` (ghost row is cells now) and its `on_hit` arm.
- [ ] **Step 4:** PASS. tmux sweep: click into cells, type, click away, Esc, Tab across rows, ghost row, checkbox mid-edit. **Step 5:** commit `feat: in-place table editing`.

---

### Task 9: Editor toolbar

Spec §5 (toolbar row). Save gets a visible button next to what it saves.

**Files:**
- Modify: `crates/postui/src/components/editor.rs`, `src/layout.rs` (CHROME_HEIGHT consumer), `src/app/mouse.rs` (allow-list only)

**Interfaces (Produces):**
```rust
pub const TOOLBAR_HEIGHT: u16 = 1;                    // editor.rs, added into CHROME_HEIGHT
// Toolbar chips reuse Hit::FooterChip(Action) — on_hit already dispatches the action; no new variant.
```
Chip set, left to right (chips are `paint::Chip` with `theme.control` fill, hover per HitMap): `⭳ save` (label `⭳ save •` when dirty; `Action::SaveRequest`) — always; `{{ }} vars` (`Action::OpenVarPicker { completing: false }`) — always; Body tab only: `align format` (`Action::FormatBody` — use the existing action name bound to alt+f; verify in keys.rs and use that exact variant), `min minify` (alt+g's action), `sub {{on}}`/`sub {{off}}` (alt+b's toggle action, label reflects `req.substitute_body`), `ed $EDITOR` (ctrl+e's action).

- [ ] **Step 1: Failing tests:** toolbar row renders between tab bar and content; `Hit::FooterChip(Action::SaveRequest)` rect exists while the editor pane shows a request, and clicking it saves (assert file mtime/content change with a dirty editor); dirty dot appears/disappears; Body-only chips absent on Params tab; clicking `format` on Body formats. Layout: collapsed-table case still computes (CHROME_HEIGHT users at editor.rs:622-645 + `layout::compute_layout`).
- [ ] **Step 2:** FAIL. **Step 3: Implement** — 4th vertical constraint in `Component::draw for Editor` (editor.rs:557), `CHROME_HEIGHT = ADDRESS_BAR_HEIGHT + TAB_BAR_HEIGHT + TOOLBAR_HEIGHT`, subtract in the `available` math. Chips paint like `draw_footer`'s (footer.rs:48) — reuse its chip-painting helper if extractable, else mirror it.
- [ ] **Step 4:** PASS + tmux (hover fills, click save with mouse only — the original complaint). **Step 5:** commit `feat: editor toolbar with mouse-accessible save`.

---

### Task 10: Computed request headers view

Spec §6.

**Files:**
- Modify: `crates/postui/src/components/editor.rs`, `src/hit.rs`, `src/action.rs`, `src/app/mouse.rs`, `src/clipboard.rs` call path (`CopyTarget`)

**Interfaces (Produces):**
```rust
// hit.rs
Hit::AutoHeaderCopy(usize),        // index into the last-drawn computed rows
Hit::AutoHeaderReveal,             // the masked-values reveal toggle
// action.rs
CopyTarget::ComputedHeader(usize), // new variant on the existing enum
// editor.rs
pub struct ComputedHeadersView { pub rows: Vec<postui_core::prepare::ComputedHeader>, pub revealed: bool }
impl Editor { fn draw_computed_headers(&mut self, frame, area: Rect, ctx: &DrawCtx, hits: &mut HitMap); }
```
Rendering (Headers tab only, below the editable table): a dim divider line `── auto ──────`, then one dim row per `ComputedHeader` that is not `Request`-origin (request rows are already the table above): `DefaultHeader { suppressed: true }` struck through (crossed-out modifier), value column shows resolved text; rows with `unresolved` names tint those tokens `theme.error` (Task 12's span painter — until then, whole-value tint is acceptable and revised in Task 12); per-row `⧉` copy icon; one `👁 reveal`/`hide` toggle when any masked secret is present. Recomputed every draw from `computed_headers(req, &ctx.prepare_context(), !self.computed.revealed)` — cheap (small N).

- [ ] **Step 1: Failing tests:** Headers tab draw shows a default header both as editable row (when overridden: table row) and struck-through auto row; auto Content-Type appears with a body; Host row present; copy icon click puts the resolved value on the test clipboard (`set_clipboard_for_test`); reveal toggle unmasks; secrets masked by default; recompute reflects an env switch.
- [ ] **Step 2:** FAIL. **Step 3: Implement** (extend the Headers-tab arm of `draw_tab_content`; content height math adds `computed_rows + 1` lines, clamped like the table). **Step 4:** PASS + tmux (the user's #4 flow: open request with default headers + body, SEE everything that will be sent). **Step 5:** commit `feat: computed request-headers section`.

---

### Task 11: Body-editor click fix

Spec §8. Root cause: edtui 0.11.6 `mouse_position_to_cursor_position` (edtui `src/events/mouse.rs:146`) — clicks below the last line fall through to a buffer-end snap, and past-EOL clicks land ON the last char instead of after it.

**Files:**
- Modify: `crates/postui/src/components/editor.rs` (the `handle_mouse` fn at editor.rs:325)

**Interfaces (Produces):**
```rust
impl Editor {
    /// Maps a screen click inside last_body_area to a buffer cursor, honoring wrap(true),
    /// line numbers gutter, and the viewport offset. col clamps to line_len (caret AFTER the
    /// last char); rows below the last line clamp to the end of the LAST line.
    fn body_cursor_for_click(&self, x: u16, y: u16) -> Option<edtui::Index2>;
}
```

- [ ] **Step 1: Failing tests** (unit, on `Editor` with seeded `body` text and a synthetic `last_body_area` — drive `handle_mouse` with `MouseEvent`s):

```rust
#[test]
fn click_past_line_end_places_caret_at_line_end() {
    let mut ed = editor_with_body("{\n  \"a\": 1,\n  \"bb\": 2\n}\n");   // helper: sets body + last_body_area 40x10 at (0,0), draws once to set edtui screen_area
    ed.handle_mouse(left_down(35, 1));                                   // far right of line 1 ("  \"a\": 1,")
    assert_eq!(ed.body.cursor, edtui::Index2::new(1, 10));               // AFTER the trailing comma, not on it
}
#[test]
fn click_below_last_line_goes_to_end_of_last_line_not_buffer_scan_result() {
    let mut ed = editor_with_body("{\n  \"a\": 1\n}\n");
    ed.handle_mouse(left_down(5, 8));                                    // blank space below content
    assert_eq!(ed.body.cursor.row, 3);                                   // last line
}
```
(Exact expected columns: verify against edtui's `Index2` semantics — insert-mode caret may sit at `len`. Adjust the assertions to the real coordinate system when writing the helper; the invariant under test is "end of clicked/last line, never elsewhere".)
- [ ] **Step 2:** FAIL (current behavior snaps to buffer end / last char). **Step 3: Implement:** in `handle_mouse`, after forwarding `Down(Left)` to `on_mouse_event`, overwrite `self.body.cursor` with `body_cursor_for_click`'s result when `Some`. Port the wrapped-line walk from edtui 0.11.6 `mouse.rs:146` with the two fixes (clamp col to `line_len`, clamp row to last line's end); account for the line-number gutter via `self.body.view.screen_area` (edtui sets it post-gutter at its `view.rs:262`) rather than `last_body_area`. Drag-selection still forwards untouched (only `Down` is corrected).
- [ ] **Step 4:** PASS + tmux: click around a multi-line JSON body — every click lands where a desktop editor would put it. **Step 5:** commit `fix: body-editor click places caret at end of clicked line`.

---

### Task 12: Inline variable highlighting + hover tooltip

Spec §7.

**Files:**
- Modify: `crates/postui/src/components/editor.rs`, `src/components/table_editor.rs` (cell painter), `src/hit.rs`, `src/ui.rs`, `src/app.rs`, `src/app/mouse.rs`

**Interfaces (Produces):**
```rust
// hit.rs
Hit::VarToken(String),   // the variable name; registered OVER UrlBar/TableCell/BodyEditor rects (last wins)
// app.rs
pub struct TokenTip { pub name: String, pub anchor: Rect }
pub fn var_token_tip(&self) -> Option<TokenTip>;   // from hovered VarToken, or caret-resting (below)
// ui.rs (drawn last, above everything except modals)
fn draw_var_tooltip(frame: &mut Frame, screen: Rect, theme: &Theme, tip: &TokenTip, ctx: &ProjectContext);
// editor.rs / table_editor.rs — span painter used by URL bar, table cells, computed rows:
pub fn paint_var_tokens(buf: &mut Buffer, row_area: Rect, text: &str, text_origin_col: u16,
                        resolved: &postui_core::varmodel::Resolved, theme: &Theme,
                        hits: &mut HitMap) ;   // tints resolved tokens theme.accent-dim, unresolved theme.error; registers Hit::VarToken per span
```
Tooltip content: line 1 `name = value` (value masked `●●●●●●` for secrets — always, tooltip has no reveal), line 2 source: `request var` / `env <name>` / `default` / `group user → "user 2"` / `needs selection` / `missing secret` — derived from `resolved.meta` + whether `env_data.values`/request vars contain the name. Body coverage: after `frame.render_widget(view, area)` for edtui, scan the rendered buffer rows within `area` for `{{name}}` occurrences (cell-text reconstruction per row) and run the same tint+register; tokens wrapped across visual lines are skipped (documented limitation, spec risk note). Caret-resting: on `Action::Tick`, if `sub_focus == Url` and the URL caret byte-offset falls inside a `find_tokens` span for ≥ 2 consecutive ticks (~200 ms), synthesize the tip anchored at that span's rect from the last draw; same for the body editor (`sub_focus == Content`): run `find_tokens` on the body line under `self.body.cursor` and anchor at that token's registered `VarToken` rect when one exists in the current frame.

- [ ] **Step 1: Failing tests:** URL `{{base_url}}/x` draw registers `Hit::VarToken("base_url")` with a rect inside the URL bar; unresolved token renders `theme.error` fg in the buffer; hover (Moved event over the rect) → `var_token_tip()` is Some → `ui::draw` output contains `base_url = ` and the source line; secret var tooltip shows `●` not the value; clicking the token opens the var picker prefiltered (input seeded with the name); needs-selection group field shows `needs selection`; table value cells and computed-header rows get the same treatment.
- [ ] **Step 2:** FAIL. **Step 3: Implement.** `Moved` handling already updates `hovered` — no new routing needed; add `VarToken` to BOTH blanket allow-lists (hovering/clicking a token must not blur the table/URL). `on_hit` `VarToken(name)` → open picker with input pre-seeded (reuse the existing picker-prefilter path used by `app.rs:2122`).
- [ ] **Step 4:** PASS + tmux with SGR motion events (`\e[<35;x;yM`): hover over tokens in URL, params, body; verify tooltip placement and that it never lingers after the pointer leaves. **Step 5:** commit `feat: inline variable highlighting + value tooltips`.

---

### Task 13: Response — async pretty-print + mouse search

Spec §9, §5 (response pane).

**Files:**
- Modify: `crates/postui/src/components/response.rs`, `src/components/json_tree.rs` (none expected — `parse` stays sync, just called off-thread), `src/session.rs`, `src/app.rs`, `src/action.rs`, `src/hit.rs`, `src/app/mouse.rs`

**Interfaces (Produces):**
```rust
// response.rs
pub const SYNC_PRETTY_BYTES: usize = 256 * 1024;      // ≤ this parses synchronously; MAX_PRETTY_BYTES + OVERSIZE_HINT deleted
pub struct ReadyView { /* existing fields, plus: */ pub generation: u64, pub parsing: bool }
impl Response {
    /// Deliver a background parse. Matches on ReadyView.generation; stale/mismatched = false.
    pub fn attach_tree(&mut self, generation: u64, tree: Option<crate::components::json_tree::JsonTree>) -> bool;
}
// session.rs
impl Session { pub fn tree_arrived(&mut self, generation: u64, tree: Option<JsonTree>) -> bool; }  // tries current, then every cache slot
// action.rs
Action::PrettyParsed { generation: u64, tree: Option<Box<crate::components::json_tree::JsonTree>> },
// hit.rs
Hit::ResponseSearchButton, Hit::ResponseSearchNext, Hit::ResponseSearchPrev,
```
`ReadyView::new(data, generation)`: body ≤ `SYNC_PRETTY_BYTES` → parse inline (today's path); larger → `tree: None, parsing: true, mode: Raw`. `App` spawn site (right after `session.arrived` returns true, app.rs:985): clone the body `String` into `tokio::task::spawn_blocking(move || JsonTree::parse(&body))`, send `Action::PrettyParsed` over `self.tx` (mirror the send-task shape at app.rs:950-971; no `InFlight` registration — a stale parse is dropped by generation matching, and navigating away is fine because `tree_arrived` finds the cache slot). `attach_tree`: sets `parsing = false`; `Some(tree)` → store + if the user is on (or asked for) Pretty, switch to it; `None` (not JSON) → stays Raw, Pretty permanently unavailable for this response (current non-JSON behavior). While `parsing`, the Pretty tab is clickable and shows the existing `SPINNER` + "parsing…" in the body area; `set_view_mode(Pretty)` is allowed (no more toast-refusal). Search buttons: a `⌕` button on the header strip (opens `SearchState { active: true }` exactly as `/` does) and, while `search.is_some()`, `▲`/`▼` beside the search footer dispatching the existing next/prev cycling.

- [ ] **Step 1: Failing tests:** big-body flow — `set_state(Ready)` with a 3 MiB JSON body → `view().unwrap().parsing == true`, no tree; `attach_tree(gen, Some(tree))` → Pretty renders (flip the existing `http_integration.rs:138` test from asserts-capped to asserts-Pretty-after-attach, pumping the action channel with the stage-4 `drain_until` helper); stale generation → `attach_tree` false; navigate to another request then deliver → cached slot got the tree (switch back and see Pretty); non-JSON large body → Raw stays, no spinner after `attach_tree(gen, None)`; `⌕` click opens search; `▼`/`▲` clicks cycle matches.
- [ ] **Step 2:** FAIL. **Step 3–4: Implement, PASS** + tmux with a local big-JSON fixture served via `python3 -m http.server` from the scratchpad (spinner visible, UI stays responsive while parsing — scroll Raw during the parse). **Step 5:** commit `feat: background pretty-print, no size cap; mouse-accessible response search`.

---

### Task 14: Variable Manager rewrite — skeleton, left list, env switcher

Spec §3.4. Tasks 14–16 replace `varmanager.rs`; old grid code (rows/cells/env_scroll machinery) is deleted as its replacement lands, and old manager tests are rewritten per task.

**Files:**
- Modify: `crates/postui/src/components/varmanager.rs`, `src/hit.rs`, `src/action.rs`, `src/app.rs`, `src/app/mouse.rs`

**Interfaces (Produces):**
```rust
// varmanager.rs
pub enum VmDetail { None, Var(String), Group(String) }
pub enum VmRow { SectionVars, Var(String), SectionGroups, Group(String) }   // left list, rebuilt per draw
pub struct VarManager {
    pub detail: VmDetail,
    pub left_rows: Vec<VmRow>,
    pub left_cursor: usize, pub left_scroll: usize,
    pub form: VarFormState,          // Task 15 (Default-able placeholder struct this task)
    pub grid: EntryGridState,        // Task 16 (Default-able placeholder struct this task)
}
impl VarManager {
    pub fn handle_key(&mut self, ev: KeyEvent, ctx: &ProjectContext) -> Option<Action>;
    pub fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, ctx: &ProjectContext,
                open_request: Option<&HttpRequest>, hits: &mut HitMap, hovered: Option<&Hit>);
    pub fn select_row(&mut self, i: usize);      // click/keyboard both land here; sets detail from left_rows[i]
}
// hit.rs
Hit::VmLeftRow(usize), Hit::VmEnvSwitch, Hit::VmNewVar, Hit::VmNewGroup,
// action.rs — VarStructOp changes (VarEditOp gains SetEntryValue/entry Select in Task 16):
pub enum VarStructOp {
    NewVar { name: String, description: Option<String> },
    NewGroup { name: String, fields: Vec<String> },
    Rename { from: String, to: String },
    Delete { name: String },
    ToggleSecret { name: String },
    SetFields { group: String, fields: Vec<String> },
    Promote { name: String, target: PromoteTarget },
    Demote { name: String },
    NewEntry { env: String, group: String, name: String, description: Option<String>, values: IndexMap<String, String> },
    RenameEntry { env: String, group: String, from: String, to: String },
    DeleteEntry { env: String, group: String, name: String },
    DuplicateEntry { env: String, group: String, name: String },
}
```
Layout: top bar (3 rows painted like the header): `Environment: <name> ▾` (Hit::VmEnvSwitch → the existing env Chooser modal), right-aligned `[+ Variable] [+ Group]` paint::Buttons (existing `PromptNewVar`/`PromptNewGroup` prompts, with the group prompt reworked to ask name + fields via `Modal::MultiPrompt`). Left column (fixed 28 cols, PillRow rows, scrollbar): `VARIABLES` section then each var (name + `🔒` for secrets + red dot when unresolved), `GROUPS` section then each group (`▶ user (user 2)` — current selection inline, `(needs selection)` when none). Right pane: placeholder text this task ("select a variable or group"); detail panes are Tasks 15–16. Keyboard: up/down move `left_cursor` + `select_row`, existing single-letter keys keep working where their target is the left selection (`n`→NewVar prompt, `g`→NewGroup, `e`/`F2`→Rename prompt, `d`→Delete confirm, `s`→ToggleSecret confirm) — commands are ignored while any cell edit is active (Task 16's grid).

- [ ] **Step 1: Failing tests:** open manager → left list shows seeded vars then groups with selection labels; click `Hit::VmLeftRow(i)` on a group row → `detail == VmDetail::Group(..)`; env switcher click opens the chooser and switching env relabels selections; `[+ Variable]` click opens the new-var prompt; `n` still does too; right-click a left row → context menu (Rename…/Duplicate/Delete…) wired through Task 7's `context_menu_for`.
- [ ] **Step 2:** FAIL. **Step 3: Implement** (delete `RowKind`, `build_rows`, `EnvColumn`, `Cell`, cursor/env_scroll fields and their draw code; keep `VarEditOp` plumbing that survives). `App::update` arms for `VarStruct` map to Task-2 verbs via `ctx.edit_variables`/`edit_env` (entry ops → `upsert_entry`/`rename_entry`/`delete_entry`; `DuplicateEntry` = read current values from `env_data`, `upsert_entry` under `"<name> copy"` with `-2` suffixes on collision; `Delete { name }` of a group cascades `delete_group_entries` across every env file + clears its selections via `clear_selection_for`).
- [ ] **Step 4:** PASS + tmux (navigate list, switch env, open prompts by mouse only). **Step 5:** commit `feat: variable manager master-detail skeleton`.

---

### Task 15: Manager — variable detail form

Spec §3.4 (variable selected).

**Files:**
- Modify: `crates/postui/src/components/varmanager.rs`, `src/hit.rs`

**Interfaces (Produces):**
```rust
pub struct VarFormState { pub editing: Option<(VmField, LineInput)>, pub revealed: bool }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmField { Description, Default, EnvValue }
// hit.rs
Hit::VmFormField(VmField), Hit::VmSecretToggle, Hit::VmRevealToggle, Hit::VmRename, Hit::VmDelete, Hit::VmPromoteBtn,
```
Right pane for `VmDetail::Var(name)`: title row `name  🔒?  [rename] [delete]` (paint::Buttons); fields as label + TextField rows — Description (writes `VarEditOp::SetDescription`), `secret [on/off]` toggle (existing `ToggleSecretVar` confirm flow), Default (SetDefault; hidden for secrets), `Value in <env>` (SetEnvValue / SetSecretValue; masked + `👁` when secret; shows `(no environment)` hint when `active_env` is None and targets the declaration default instead), usage line `used by: <scan_usage list>` (dim). Editing a field follows Task 8's in-place rules exactly: click → edit, click-away → commit, Esc → revert (reuse `LineInput`; commit dispatches the `VarEditOp`). Promote/demote buttons appear when the legacy `p`/`P` preconditions hold (non-secret simple var; same `promote_var` conflicts).

- [ ] **Step 1: Failing tests:** select a var → form renders description/default/env value; click env-value field, type, click-away → env file updated on disk (through `edit_env`), toast on write failure keeps the text; secret var shows mask + reveal toggle and no Default row; rename button opens the existing rename prompt; delete button opens the existing confirm (with usage list in the body); keyboard: `e` renames, `s` toggles secret — unchanged.
- [ ] **Step 2–4:** FAIL → implement → PASS + tmux. **Step 5:** commit `feat: variable detail form`.

---

### Task 16: Manager — group entries grid

Spec §3.4 (group selected), §4 (same editing rules).

**Files:**
- Modify: `crates/postui/src/components/varmanager.rs`, `src/hit.rs`, `src/action.rs` (VarEditOp)

**Interfaces (Produces):**
```rust
pub struct EntryGridState { pub cursor: (usize, usize), pub editing: Option<GridEdit>, pub scroll: usize }
pub struct GridEdit { pub row: usize, pub col: usize, pub input: LineInput, pub original: String }
// hit.rs
Hit::VmEntryRadio(usize), Hit::VmEntryCell { row: usize, col: usize },   // row == entries.len() is the ghost entry row
Hit::VmNewEntry, Hit::VmEditFields,
// action.rs — VarEditOp gains:
SetEntryValue { env: String, group: String, entry: String, field: String, value: String },
SelectEntry   { env: String, group: String, entry: String },              // replaces stage-6 Select
```
Right pane for `VmDetail::Group(name)`: title `Group: user   [+ Entry] [Edit fields] [rename] [delete]`; column headers = field names; rows = entries: `◉/○` radio (click → `SelectEntry`; `● = selected for <env>` legend line under the table), entry-name cell, one cell per field. Cell editing = Task 8 rules (click-to-edit, click-away commit → `SetEntryValue`; name-cell commit → `RenameEntry`). Ghost row: clicking its name cell starts a new entry; committing a non-empty name creates it via `NewEntry` with empty field values, then continues editing left-to-right. `[Edit fields]` opens a `Modal::MultiPrompt` listing current fields (one text field each + a trailing empty "add field" slot); confirm computes renames (`rename_entry_field` for changed names — matched by position), additions, and removals — a removal pushes a confirm modal warning "values in this column will be deleted from every entry in <env list>" before dispatching `SetFields` + `strip_entry_field` per env. Right-click an entry row → Duplicate entry / Rename… / Delete… (Task 7 menu). With `active_env` None: the pane shows "entries live in environments — pick or create one" + the env switcher; no grid. Old keys: `o` → new entry, `space` on a row → select entry, `m` → edit fields; ignored while `editing.is_some()`.

- [ ] **Step 1: Failing tests:** grid renders entries × fields for the active env; radio click writes the selection to `.local/state.toml` and updates resolution (`{{user_id}}` resolves to the new entry's value); cell edit commit rewrites the env file (assert on-disk TOML); ghost-row flow creates an entry; field add/rename/remove round-trips through `variables.toml` + every env; delete-group cascade (Task 14's confirm) removes entries in all envs; no-env state renders the hint.
- [ ] **Step 2–4:** FAIL → implement → PASS + tmux (the user's core scenario: create group `user` with `user_id`/`customer_id`, add `user 1`/`user 2` in staging, flip radios, watch two request headers change together via the Task-12 tooltips). **Step 5:** commit `feat: group entries grid`.

---

### Task 17: Mouse-parity sweep + remaining keyboard gaps

Spec §5 acceptance check: every keybound `Action` is mouse-reachable (button, menu, or palette row); plus the small named gaps.

**Files:**
- Modify: `crates/postui/src/keys.rs`, `src/components/palette.rs`, `src/components/footer.rs`, `src/app/mouse.rs`, `src/components/sidebar.rs`

- [ ] **Step 1:** `alt+4` → the Vars editor tab: add the default binding parallel to the existing `alt+1`..`alt+3` rows in `Keymap::default_bindings` (same action constructor, index `EditorTab::Vars.index()` = 3) and the matching `named_actions` row (`"tab_vars"`). Test: alt+4 switches to Vars tab.
- [ ] **Step 2:** Table-row context menu (right-click a params/headers/vars row): Duplicate row (insert `key-copy` below with same value), Delete row (existing confirm), Extract value to variable (`Action::ExtractToVariable` after selecting the row). Test each menu item end to end.
- [ ] **Step 3:** Footer chips audit: chips whose action is `None` but which have a real action now (Response pane `r`/`h`/`/` → `Action::ResponseViewMode(..)` ×2 and the Task-13 search-open action) become `Some(..)` clickable chips.
- [ ] **Step 4:** Palette audit: add commands for every new action without one (`Request: duplicate` exists from Task 7; add `Body: format`, `Body: minify`, `Body: toggle {{vars}}`, `Body: open in $EDITOR`, `Response: search`, `Variables: new variable`, `Variables: new group` if missing).
- [ ] **Step 5: The parity check itself** — a test that enumerates `keys.rs::named_actions()` and asserts each action appears in at least one of: a `Hit` dispatch arm in `on_hit`/`FooterChip`/toolbar chip const list, a context-menu builder, or `palette::all_commands()`. Implement as a plain unit test over a hand-maintained allow-list of intentionally-keyboard-only actions (navigation like `FocusPane`; the list must be empty of anything user-facing — reviewer checks the list against spec §5).
- [ ] **Step 6:** fmt/clippy/test + tmux spot checks, commit `feat: mouse-parity sweep (alt+4, table menus, footer/palette gaps)`.

---

### Task 18: Acceptance — `stage7_acceptance.rs` + scripted tmux sweep

**Files:**
- Create: `crates/postui/tests/stage7_acceptance.rs` (stage-4 harness style: `render`, `click`, `right_click`, `drain_until`)

- [ ] **Step 1: Write the acceptance tests** — one scenario per spec goal, exercised through the public event surface (mouse + keys + action channel), no component internals:
  1. duplicate a request via right-click and see it open (goal 1);
  2. save a dirty request with only mouse clicks (goal 2);
  3. body click lands at end of clicked line (goal 3);
  4. Headers tab shows default header + auto Content-Type + Host with resolved values (goal 4);
  5. hover a `{{token}}` → tooltip with value and scope (goal 5);
  6. click-edit-clickaway a param cell commits; Esc reverts; ghost row creates (goal 6);
  7. full variables flow: migrate a legacy project fixture, create a group + entries, flip selection, resolution changes (goal 7);
  8. 3 MiB JSON body: Raw immediately, Pretty after the parse action arrives, no cap toast (goal 8).
- [ ] **Step 2:** Run the full suite: `cargo test --workspace` green, fmt + clippy clean.
- [ ] **Step 3: Scripted tmux whole-workflow sweep** (mandatory, mouse-first): fresh project → migration prompt on a legacy fixture → build a request end to end using ONLY the mouse (method dropdown, URL, params in-place, body, toolbar save, send, response tabs/search, computed headers, var manager group + entries + radios) — record any friction found as follow-up notes in the final report; fix in-scope paper cuts before calling the stage done.
- [ ] **Step 4:** Commit `test: stage 7 acceptance`, then run the finishing-a-development-branch skill.
