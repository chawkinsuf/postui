# postui — Stage 2: HTTP Core Design

*2026-08-15 · Status: approved design. Parent: [2026-08-15-postui-design.md](2026-08-15-postui-design.md) (binding high-level authority). Exit criterion: usable as a basic daily HTTP client.*

## 1. Scope

**In:**

- Request TOML files stored in a **default project** at `~/.config/postui/default/requests/**/*.toml`.
- Sidebar listing those requests: open, create, rename, delete.
- Request editor: method, URL, query params, headers, JSON body (edtui editor widget + `$EDITOR` escape hatch).
- Async send via reqwest with cancellation and live elapsed-time spinner.
- Response viewer: summary line, syntect-highlighted collapsible JSON tree, raw view, headers view, search.
- The parked stage-1 review fixes (§7).

**Out (per roadmap):** variables/`{{}}` substitution, environments, project switching/multiple projects, clipboard/copy actions, curl import/export, scripting, history, multi-request tabs (the sidebar selection *is* the open request — "vertical tabs"). Non-JSON bodies: any text body can be *sent* and any text response *viewed*, but no dedicated affordances (no form-data editor, no non-JSON highlighting).

**JSON-first:** the user works almost exclusively with JSON REST APIs. JSON gets full support (highlighting, formatting, validation, tree view); other content types get graceful degradation, not features.

## 2. Storage: default project

- Fixed path `~/.config/postui/default/` (via the `directories` crate config dir), laid out exactly like the parent spec's §3 project directory. Stage 3 introduces opening other directories; this one keeps working unchanged — no migration.
- The path is a stage-2 constant; making it configurable is future work (acknowledged: XDG purists would use `~/.local/share`, but these files are hand-editable git-friendly TOML — config-dir placement is deliberate).
- Created on first launch (including `requests/`) if absent.
- File identity = path; a request has no separate name field. Filenames are slugs validated on create/rename (`[a-z0-9-_]`, subdirectories allowed via `/` in the new-request prompt).
- All writes are atomic (write temp file, rename).

## 3. Request file format

```toml
method = "POST"
url = "https://api.example.com/users"

[params]
page = "2"
verbose = { value = "1", enabled = false }

[headers]
Authorization = "Bearer abc123"
X-Request-Id = "test-run-7"

[body]
type = "json"
text = """
{ "name": "alice" }
"""
```

- `[params]` and `[headers]` are TOML **tables** (not arrays of tables). A plain string value means an enabled entry; the `{ value = "…", enabled = false }` table form is used for disabled entries (`enabled = true` in table form is legal but normalized to the string form on save).
- **Order is preserved** file → editor → file via an order-preserving map (`indexmap`). No alphabetization behind the user's back.
- **Duplicate keys within a table are a TOML parse error** (spec-level; the `toml` crate rejects the file). Surfaced as a friendly load error naming file and line; the sidebar marks the file broken (§5). Last-wins-with-warning applies only where we control merging (§3.1).
- Array values (`id = ["1", "2"]`) are **reserved for a future schema widening** to support repeated params; stage 2 rejects them with a clear load error.
- `[body]` is tagged: `type = "json"` with raw `text` (invalid-but-intentional JSON survives round-trips). Absent `[body]` = no body. Future body types add new `type` tags.
- Unknown TOML keys **error loudly on load** (`deny_unknown_fields`) rather than being silently dropped on the next save. Strictness protects hand-edited files from data loss; later stages widen the schema.

### 3.1 Duplicate-key policy (warn, last wins)

- **URL query string vs `[params]`:** if the URL literally contains `?id=1` and `[params]` also defines `id`, warn via toast at send time; the `[params]` entry wins (last-defined).
- **In-editor:** typing a key in the params/headers table that already exists warns and replaces the existing row's value.

## 4. Core crate (`postui-core`) additions

The crate stays terminal-free **and** IO-runtime-free (no reqwest, no tokio). New modules:

- **`model`** — `HttpRequest { method: Method, url: String, params: IndexMap<String, Entry>, headers: IndexMap<String, Entry>, body: Body }`; `Entry { value: String, enabled: bool }` with custom serde for the string-or-table form; `Body::None | Body::Json(String)`.
- **`storage`** — load/save/list/delete request files; atomic write; slug validation; recursive listing of `requests/**/*.toml`. Load errors are typed (parse error with location, IO error) so the UI can render them.
- **`prepare`** — `HttpRequest` → `PreparedRequest { method, url, headers, body_bytes }`: merges enabled `[params]` into the URL query (last-wins per §3.1, emitting structured warnings for the UI to toast), filters enabled headers, auto-adds `Content-Type: application/json` for JSON bodies unless a header already sets it (case-insensitive).
- **`json`** — validation (`valid | invalid(position, message)`) and pretty-print used by the Format action and the body validity indicator.

The TUI crate owns the actual reqwest send, taking `PreparedRequest` from core.

**New dependencies** (all verified actively maintained against crates.io, 2026-08-15): core — `indexmap` 2 (serde feature), `thiserror` 2, `serde_json` 1; TUI — `reqwest` 0.13 (rustls-tls), `edtui` 0.11 (`syntax-highlighting` feature); dev — `wiremock` 0.6, `tempfile` 3. Workspace `toml` bumps 0.8 → 1.

## 5. UI: sidebar and request editor

**Sidebar** (replaces the stage-1 placeholder):

- Indented flat list of request files; subdirectory names render as non-selectable group rows. Full tree UX is stage 3.
- Arrows/`j`/`k` move; Enter opens the highlighted request into the editor.
- Switching away from a dirty editor prompts save / discard / cancel (modal).
- Palette commands + direct keybindings: **New request** (name prompt; `/` creates subdirectories), **Rename**, **Delete** (confirm modal).
- Broken files (parse errors) stay listed with an error marker; opening one shows the load error (file, line, message) in place of the editor.
- A dot marks the dirty open request.

**Request editor** (replaces the placeholder), a vertical stack:

- **Method + URL bar.** Method is a cycling colored badge (Space/Enter or `m` cycles GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS, colors from stage-1 tokens). URL is a single-line input; a literal `?query` in the URL is permitted and simply stays there (§3.1 arbitrates conflicts with `[params]`).
- **Sub-tabs: Params / Headers / Body.** No Scripts placeholder tab (arrives in stage 5).
  - **Params & Headers** share one table-editor component: key/value rows with an enabled-checkbox column; `a` add row, `d`/Delete remove, Space toggle enabled, Enter edit focused cell inline, Tab key→value. Disabled rows render muted.
  - **Body:** `edtui` editor widget (chosen over the design's original `tui-textarea`, which is dormant — last release Oct 2024, ratatui 0.29 only, incompatible with our ratatui 0.30; edtui is actively maintained and built on ratatui-core 0.1/ratatui-widgets 0.3, i.e. ratatui 0.30's own components). edtui runs in **modeless "emacs mode"** (`EditorEventHandler::emacs_mode()`) — plain type-to-insert editing, no vim modes (a vim-mode config toggle is possible future work, not stage 2). JSON syntect highlighting via edtui's `syntax-highlighting` feature; the `$EDITOR` escape hatch uses edtui's `system-editor` feature; the URL bar may reuse the widget via `single_line(true)`; **Format** and **Minify** actions (serde_json pretty-print / compact; on parse failure, toast with error position, buffer unchanged — these are the *only* operations that ever rewrite body text; save never reformats); `Ctrl+E` (rebindable) suspends the TUI and opens the body in `$EDITOR`, resuming on exit. Validity indicator in the tab bar (`Body ✓` / `Body ✗`).
- **Save:** `Ctrl+S`, atomic. Saving works with invalid JSON — work-in-progress bodies are first-class; the body is written verbatim, never normalized or reformatted on save. The validity confirm modal guards *send* only. **Send does not auto-save** — what is sent is the live editor state, so experimentation never dirties the file implicitly.
- Sub-tab switching bound to `[`/`]` or `1`–`3` while the editor pane is focused (exact combos settled in the implementation plan against existing keymap conflicts).

Focus model unchanged from stage 1: Tab/Shift+Tab and mouse click move between sidebar / editor / response panes.

## 6. Send pipeline and response viewer

**Send** (`Ctrl+Enter` anywhere, plus palette command):

- Editor state → `PreparedRequest` (core `prepare`, §4). Sending with invalid JSON opens a confirm modal ("Send anyway?") — sometimes broken JSON is the test.
- A spawned tokio task runs reqwest — 30 s fixed timeout (per-request override is future work), redirects followed to reqwest's default 10, rustls TLS, no proxy/HTTP-version surface — and reports over the existing mpsc channel as actions (`ResponseArrived`, `RequestFailed`).
- **In-flight:** response pane shows spinner + live elapsed time. `Esc` in the response pane (or re-sending) cancels via the task's abort handle → "cancelled" state. One in-flight request at a time; a new send cancels the previous.
- **Errors** (DNS, refused, TLS, timeout) render as a styled error state in the response pane with the reqwest error chain, plus a toast. Never a crash.

**Response viewer** (replaces the placeholder):

- **Summary line:** status pill colored by class, elapsed time, body size, response `Content-Type`.
- **Pretty view** (default when the body parses as JSON): pretty-printed, syntect-highlighted, **collapsible** — cursor on an object/array, Space/Enter toggles, collapsed nodes render `{…} 12 keys` / `[…] 40 items`. `j`/`k`/arrows move by visible line; wheel scrolls (§7 scroll routing).
- **Raw view:** `r` toggles exact body text; automatic fallback for non-JSON/unparseable bodies. **Headers view:** `h` lists response headers.
- **Search:** `/` opens an input in the pane footer; matches highlight in the current view, `n`/`N` navigate, count shown ("3/17"). Search operates on the rendered text of the current view.
- **Large-body guard:** bodies over 2 MiB (constant, not configurable in stage 2) skip highlighting and tree parsing, opening in raw view with a hint. Streaming/virtualization is out of scope; the threshold is the stage-2 answer to "a huge payload must not freeze the UI."

## 7. Parked stage-1 fixes (in scope)

1. **Global quit escape hatch:** quit bindings are checked before modal/palette delegation — Ctrl+C always quits, modal open or not. The palette's char handling becomes modifier-aware (Ctrl+letter no longer inserts a literal character).
2. **Multi-combo bindings:** `Keymap` maps action → list of combos. A TOML override sets the full list explicitly (`quit = ["ctrl+q", "ctrl+c"]`; a bare string still means a one-element list). Ctrl+C remains reserved for quit regardless of overrides.
3. **Modal close policy:** modal updates return `ModalResult { action, close: bool }`; the event loop stops inferring closure from "any non-Close action." Stage 2's confirm/prompt modals are the first real consumers.
4. **`Action::Render` / redraw policy:** the mpsc arm carries real traffic (responses, spinner ticks); rendering becomes event-driven — redraw on state change or spinner tick, not a constant 10 fps repaint.
5. **Scroll routing:** mouse wheel hit-tests the hovered pane (reusing click-to-focus `hit_test`) and delivers scroll there **without changing focus**; the response viewer is the first consumer.
6. Deferred test minors from stage 1 are picked up where they touch code being edited anyway (keymap parse edges, removing the empty `[dev-dependencies]` from `crates/postui/Cargo.toml`); the rest stay parked.

## 8. Testing

- **`postui-core` unit tests:** TOML round-trip (table form, string-vs-table entries, `enabled` normalization, order preservation), duplicate URL-vs-params merge (last wins + warning emitted), array-value rejection, unknown-key rejection, slug/path validation, atomic save, `PreparedRequest` assembly (param merge, header filtering, `Content-Type` auto-add and case-insensitive override), JSON validation/format.
- **HTTP integration tests (wiremock):** success, non-2xx statuses, timeout, cancellation, redirect following, large-body threshold behavior, `Content-Type` handling.
- **TUI (`TestBackend`):** table-editor flows (add/remove/toggle/edit), body validity indicator, dirty-prompt on request switch, broken-file marker and error display, response tree collapse and search rendering, modal-result close semantics, Ctrl+C-quits-with-modal-open, scroll routing to hovered pane.
- TDD throughout; clippy `-D warnings` stays green; CI on Linux and macOS.
