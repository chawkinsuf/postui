# Free-Form Request Names Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Users type one free-form request name ("Get user by ID"); the filename is an auto-derived slug they never see, deduped on collision, regenerated on rename.

**Architecture:** `HttpRequest` gains an optional `name` field (the display name of the leaf). The filename slug is derived (`slugify`) and uniquified (`-2`, `-3`) at create/rename/save-as time — collisions are invisible because the slug is invisible. `/` in a typed name still means folders; folder segments are slugified and displayed in slug form (folder display metadata is out of scope). Display-name uniqueness is enforced case-insensitively among sibling requests. All identity plumbing (editor.slug, sidebar open_slug, persisted state, response cache keys) stays slug-based and unchanged. Files without `name` display as their slug leaf — no migration.

**Tech Stack:** Rust, serde + toml_edit (`to_toml_string` is hand-built), existing storage layer in `crates/postui-core/src/storage.rs`.

**Spec:** User decision in chat (2026-08-23): option 1 — display name in file + derived slug filename, dedupe on collision, regenerate slug on rename, no user-visible slugs.

## Global Constraints

- The user never types, sees, or is warned about slugs. Every user-facing string (prompts, toasts, confirm bodies, sidebar rows) uses display names.
- Slug identity stays the internal key everywhere it is today (editor.slug, open_slug, state.toml, session response cache).
- `validate_slug` keeps guarding every storage path exactly as now (it's the traversal/safety gate).
- Old files (no `name` field) keep working: display = slug leaf; nothing rewrites them until save/rename.
- Broken (unparsable) files: rename moves the file without rewriting `name`; duplicate falls back to today's byte-copy.
- `cargo test` green after every task.

---

### Task 1: `HttpRequest.name` round-trips

**Files:** Modify `crates/postui-core/src/model.rs`.

**Interfaces — Produces:** `pub name: Option<String>` on `HttpRequest` (serde `default` + `skip_serializing_if`), written first by `to_toml_string` when `Some`.

- [ ] **Step 1: Failing test** — round-trip: `to_toml_string` of a request with `name: Some("Get user by ID")` starts with `name = "Get user by ID"` before `method`; `from_toml_str` parses it back; a nameless request emits no `name` line and parses as `name: None`; existing fixtures unaffected.
- [ ] **Step 2: Run, verify failure.**
- [ ] **Step 3: Implement** — field + `doc["name"] = value(...)` ahead of `method` in `to_toml_string`. Fix any struct-literal construction sites (`app.rs` CreateRequest arm, tests) with `name: None`.
- [ ] **Step 4: Full run + commit** — `feat(core): optional display name field on HttpRequest`.

---

### Task 2: slugify + unique_slug + display-path split

**Files:** Modify `crates/postui-core/src/storage.rs`.

**Interfaces — Produces:**
- `pub fn slugify(segment: &str) -> String` — lowercase; keep `[a-z0-9_-]`; every other char (spaces included) → `-`; collapse runs of `-`; trim leading/trailing `-`; empty result → `"request"`. Output always passes `validate_slug` as a single segment.
- `pub fn split_display_path(input: &str) -> Option<(String, String)>` — splits typed input on `/`: all but the last segment are slugified and joined as the folder prefix (`""` for none), the last segment (trimmed) is the leaf display name. `None` when the trimmed leaf is empty.
- `pub fn unique_slug(root: &Path, folder: &str, leaf_display: &str, exclude: Option<&str>) -> String` — `folder/slugify(leaf)`, appending `-2`, `-3`, … while `request_exists` (skipping `exclude`, the renaming request's own slug).

- [ ] **Step 1: Failing tests** — `slugify("Get user by ID!") == "get-user-by-id"`, `slugify("???") == "request"`, `slugify("Ünïcode") == "n-code"`-style (non-ascii dropped to `-`; assert only that output validates), collapse/trim; `split_display_path("API Auth/Get User")` → `("api-auth", "Get User")`; leading/trailing spaces trimmed; empty leaf → None; `unique_slug` returns base when free, `-2` on collision, and `exclude` doesn't count as a collision.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Full run + commit** — `feat(core): slug derivation from display names`.

---

### Task 3: name-aware create + listing names

**Files:** Modify `crates/postui-core/src/storage.rs`.

**Interfaces — Produces:**
- `RequestListing.name: Option<String>` — parsed in the same pass as `method` (None for broken files).
- `pub fn sibling_name_taken(root: &Path, folder: &str, leaf_display: &str, exclude_slug: Option<&str>) -> bool` — case-insensitive match against sibling requests' display names (falling back to slug leaf for nameless files).
- `pub fn create_request_named(root: &Path, display_path: &str, req: HttpRequest) -> Result<(String, String), StorageError>` — returns `(slug, leaf_display)`; rejects an empty leaf as `InvalidSlug` and a taken sibling name as `AlreadyExists(leaf_display)`; otherwise saves `req` with `name = Some(leaf_display)` at the deduped slug.

- [ ] **Step 1: Failing tests** — create "My Request!" → file `my-request.toml` containing `name = "My Request!"`; creating "My Request?" next to it lands at `my-request-2.toml` (different names may share a slug base); creating "my request!" (case-insensitive same name) → AlreadyExists; listing surfaces `name`; nameless legacy file → listing `name: None` and its slug leaf counts for `sibling_name_taken`.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Full run + commit** — `feat(core): name-aware request creation and listings`.

---

### Task 4: name-aware rename + duplicate

**Files:** Modify `crates/postui-core/src/storage.rs`.

**Interfaces — Produces:**
- `pub fn rename_request_named(root: &Path, from_slug: &str, display_path: &str) -> Result<(String, String), StorageError>` — returns the new `(slug, leaf_display)`. Validates like create (empty leaf, sibling name excluding `from_slug`); derives the new slug with `unique_slug(..., exclude: Some(from_slug))`; renames the file when the slug changed; rewrites `name` when the file parses (a broken file just moves).
- `duplicate_request` updated: when the source parses, the copy's display name is `"<name> copy"` / `"<name> copy 2"` … (checked via `sibling_name_taken`), slug derived from it, `name` field written; a broken source keeps today's byte-copy `-copy` behavior.

- [ ] **Step 1: Failing tests** — rename to a new display name regenerates the slug and rewrites `name`; rename that collides on slug with a *different* request dedupes to `-2`; rename onto its own current name is a no-op Ok; rename to a sibling's display name (case-insensitive) errors; renaming a broken file moves it without a parse error; duplicate of "Get User" creates display "Get User copy" at `get-user-copy.toml`; duplicating again yields "Get User copy 2".
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Full run + commit** — `feat(core): display-name rename and duplicate`.

---

### Task 5: App create/save-as flows speak display names

**Files:** Modify `crates/postui/src/app.rs` (`create_or_save_as` at ~3695, `CreateRequest` arm, `SaveRequestAs`/`SaveRequestAsThen`), `crates/postui/src/components/editor.rs` (name carry-through), app tests.

**Interfaces — Produces:** `Editor.name: Option<String>` (set by `load` from the request, cleared by `Editor::default`, emitted by `current_request()` so saves preserve it); `create_or_save_as` takes the typed display path, calls `create_request_named`, loads the editor with the returned slug + name; toasts say `Saved <leaf display>`; empty-name / name-taken toasts replace the old "lowercase letters, digits…" message (which disappears from the codebase).

- [ ] **Step 1: Failing tests** — app: `CreateRequest("My Request!")` creates `my-request.toml`, editor.slug == "my-request", editor.name == Some("My Request!"), toast contains `Saved My Request!`; creating the same display name again toasts "already exists" and creates nothing; `SaveRequestAs` on a scratch with free-form name works end-to-end; saving an opened legacy request does not invent a `name` field (name None round-trips).
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement.** Grep for the old invalid-name toast string — every remaining site routes through the new flow.
- [ ] **Step 4: Full run + commit** — `feat: free-form names in create and save-as flows`.

---

### Task 6: App rename/delete/duplicate speak display names

**Files:** Modify `crates/postui/src/app.rs` (`PromptRenameRequest`, `RenameRequest`, `ConfirmDeleteRequest`, `DuplicateRequest` arms), app tests.

**Interfaces — Produces:** rename prompt prefills `<folder-slug-path>/<leaf display>` (just the leaf display at root); `RenameRequest{from, to}` keeps `from` = slug, `to` = typed display path, routed through `rename_request_named` (editor.slug/open_slug updated to the returned slug, editor.name to the returned leaf); delete confirm body and duplicate toast show display names.

- [ ] **Step 1: Failing tests** — rename opened request "get-user" to "Get user v2 " (trailing space) → file `get-user-v2.toml`, `name = "Get user v2"`, editor.slug updated, prompt-prefill test asserts the display form; delete confirm body shows the display name for a named request and the slug leaf for a legacy one.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Full run + commit** — `feat: display names in rename, delete, duplicate flows`.

---

### Task 7: Sidebar shows display names

**Files:** Modify `crates/postui/src/components/sidebar.rs` (`Row::Request` gains `name: String`, `build_rows`, `paint_row` label at ~605), sidebar tests.

**Interfaces — Produces:** `Row::Request.name` = listing name or slug leaf; within a folder level requests sort by display name (case-insensitive, then slug for stability) — folder grouping still walks slug-sorted entries; label paint uses `name`.

- [ ] **Step 1: Failing tests** — a listing with `name: Some("Zeta but shown first")`/`Some("Alpha")` renders display names in name order regardless of slug order; a nameless legacy file shows its slug leaf; folder rows unchanged.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement** (requests collected per level, sorted by `(name.to_lowercase(), slug)`, then pushed).
- [ ] **Step 4: Full run + commit** — `feat: sidebar shows request display names`.

---

### Task 8: Verification sweep

- [ ] **Step 1:** Full `cargo test`, `cargo clippy --all-targets` clean.
- [ ] **Step 2:** tmux smoke ([[tmux-tui-driving]] recipe): create "My First Request!" via `n` prompt → sidebar shows it verbatim, file lands as `my-first-request.toml` with the `name` line; create "My First Request?" → dedupes silently; rename via `r`; legacy nameless file still opens and shows slug leaf.
- [ ] **Step 3:** Check plan boxes, final commit.
