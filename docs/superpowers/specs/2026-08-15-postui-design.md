# postui — High-Level Design

*2026-08-15 · Status: approved high-level design. Each roadmap stage gets its own detailed spec + implementation plan when work on it begins.*

*Naming: `postui` is a working name only (final name TBD before first release; `curlew`, `satchel`, `pigeon` are known-taken on crates.io; `postie`, `whimbrel`, `waybill`, `aerogram`, `dunlin` were verified available as of 2026-08-15).*

## 1. Product Summary

A keyboard-driven terminal HTTP client for Linux and macOS, focused on JSON APIs — the useful core of Postman without the bloat. Everything is local: plain files on disk, no accounts, no cloud sync, no telemetry, no metering.

**Design pillars:**

1. **Local-first, git-friendly.** Projects are directories of human-readable TOML. Sharing is copying a folder or pushing a repo.
2. **Best-in-class variable management** — the differentiator. Enumerated variable options with descriptions, tied to environments, and variable groups that update together.
3. **GUI-grade visual polish.** It should look like it could be a GUI that happens to run in a terminal, not a debug tool.
4. **Frictionless clipboard.** Copy actions everywhere; never select text by hand.

**Non-goals:** team/cloud features, mock servers, monitors, AI features, GraphQL/gRPC/WebSocket (HTTP only initially). **Deferred (revisit later):** cookie jar, collection runner, OAuth2 interactive flows (script-based token capture covers the main cases), `pt.sendRequest` from scripts.

## 2. Tech Stack

| Concern | Choice | Notes |
|---|---|---|
| TUI framework | **ratatui** + **crossterm** backend | The maintained successor to tui-rs; use `ratatui::crossterm` re-export to avoid version skew |
| Async runtime | **tokio** | Event loop, background HTTP, script execution |
| HTTP | **reqwest** | Async, rustls TLS |
| Serialization | **serde**, **serde_json**, **toml** | |
| Syntax highlighting | **syntect** (+ ratatui style conversion) | JSON in editor and response viewer |
| Scripting | **boa** (pure-Rust JS engine) | No C toolchain; swap for rquickjs later only if perf demands |
| Clipboard | **arboard**, with **OSC 52** fallback | OSC 52 covers SSH sessions and Wayland compositors without data-control |
| Paths/config | **directories** | XDG on Linux, `~/Library` on macOS |

Targets: Linux (x86_64, aarch64) and macOS (aarch64, x86_64) from one codebase. Windows is not a target but nothing chosen precludes it.

**Crate layout:** a Cargo workspace with `postui-core` (request model, storage, variable resolution, scripting, import/export — no terminal dependency) and `postui` (the TUI). Core is unit-testable headless and could later back a CLI runner.

## 3. On-Disk Data Model

A **project** is a directory:

```
my-project/
  project.toml            # name, default headers, settings
  variables.toml          # variable definitions (simple, enumerated, groups)
  environments/
    qa-milestone.toml     # per-environment values for variables/options
    qa-staging.toml
  requests/
    auth/login.toml       # subdirectories are the organization hierarchy
    users/get-user.toml
  .local/                 # gitignored (template .gitignore written on project creation)
    state.toml            # active environment, selected variable options, UI state
    secrets.toml          # values of secret-marked variables
    history/*.jsonl       # append-only send history, size-capped, rotated
```

- A request file holds: method, URL, query params (with enable/disable flags), headers (with enable/disable flags), body, and optional pre/post scripts (inline or path to a shared `.js` file).
- `project.toml` defines **default headers** merged into every request (request-level headers override; a request can explicitly disable an inherited header).
- Everything above `.local/` is shareable and diffable. Everything machine- or person-specific lives in `.local/`.
- History entries record the request name, the *resolved* request (variables substituted, secrets redacted), response status/time/size, and a body snippet.

Global app config lives in the platform config dir (`~/.config/postui/` on Linux): known project paths, theme, keybinding overrides.

## 4. Variable System

All variables are referenced as `{{name}}` in URLs, query params, headers, and bodies. Resolution order: script-set values (`.local/state.toml`) → environment value → default in `variables.toml`. Unresolved variables are flagged visibly before send.

Three kinds, defined in `variables.toml`:

1. **Simple** — one value per environment. `variables.toml` declares the name (and optional description, secret flag); each environment file supplies the value. Secret-flagged variables read their value from `.local/secrets.toml` instead.
2. **Enumerated** — a named list of options, each option having a value and a **description** (e.g. `user`: "alice — admin, active sub", "bob — expired trial"). Options can be defined per environment (qa-milestone's user list differs from qa-staging's) or shared with per-environment value overrides. The currently selected option is per-environment local state.
3. **Groups** — a group binds several variables that travel together (e.g. `user_id` + `customer_id`). Options are defined at group level: one option = one description + a value for every member variable. Selecting an option sets all members atomically. The picker for *any* member variable shows the group's options, including what the linked variables will become.

**Picker UX:** with the cursor on a `{{var}}` token (or via a keybinding in any editor field), a dropdown lists options with descriptions for the active environment; typing filters. Inserting a new variable reference offers autocomplete over defined names.

**Scripts and variables:** scripts read via `pt.vars.get(name)` and write via `pt.vars.set(name, value)`. Writes go to local state, never to shared files — a `/login` post-script saving a token can't dirty the git-tracked project.

## 5. UI Layout & Visual Design

**Layout** (Postman-familiar, three zones):

- **Header bar:** app name/project switcher, active **environment selector** (always visible), key hints.
- **Left sidebar:** tabs/panes for the request tree (folders from `requests/` subdirectories), environments, and history.
- **Main area:** request editor on top — method + URL bar, sub-tabs for Params / Headers / Body / Scripts — response pane below: status-code pill, time, size; pretty-printed syntax-highlighted JSON with collapsible nodes; search-within-response; raw view toggle.

**Interaction (hybrid):** arrows/Tab/Enter and mouse (click to focus, scroll) work everywhere; vim-style `hjkl` and a command palette (Ctrl+P) for speed; visible contextual key hints; keybindings configurable.

**Clipboard, first-class:** dedicated copy actions for whole response body, a single header value, a JSON field under the cursor, the request as curl, and a full request/response dump. Copy a header from one request and paste into another via the header editor. Every copy shows a toast confirmation.

**Visual design system** (built in stage 1, inherited by everything after — the "GUI in a TUI" pillar):

- **Design tokens, not hardcoded colors:** named roles (surface, surface-raised, accent, muted, success/error/warning, border, border-focused). Truecolor default, graceful 256-color fallback. One carefully designed default theme in light and dark variants.
- **Depth and hierarchy:** focused pane lifts (accent border + title), unfocused panes recede; rounded borders; consistent interior padding; deliberate whitespace.
- **GUI-grade chrome:** centered modals with dimmed backdrop, styled command palette, toast notifications, loading spinners on in-flight requests, colored method badges (GET/POST/PUT/DELETE), status-code pills.
- **Typography:** Unicode glyphs for tree lines and selection; optional Nerd Font icons with ASCII fallback; highlighting themes matched to the app theme.
- **Detail work:** dropdown pickers with aligned description columns, subtle table row striping, helpful empty states.

The concrete visual direction (palette values, spacing rules, component looks) is developed with the frontend-design skill during stage 1 and iterated against screenshots of the running app.

## 6. Scripting

**Pre-request** and **post-response** JavaScript, per request and optionally per project (project script runs first). Engine: boa, executed in a background task with a timeout; no filesystem or network access from scripts initially.

API surface (`pt.*`, Postman-inspired but smaller):

- `pt.request` — method, url, headers, body; **mutable in pre-request** (set a computed signature header, tweak the body).
- `pt.response` — `status`, `headers`, `text()`, `json()`; available in post-response.
- `pt.vars.get(name)` / `pt.vars.set(name, value)` — resolution-aware read; writes to local state.
- `pt.env` — name of the active environment (read-only).
- `console.log/warn/error` — lands in the app console pane.

Canonical use case: `POST /login` post-script does `pt.vars.set("auth_token", pt.response.json().token)`; other requests carry `Authorization: Bearer {{auth_token}}`.

Script errors surface as toasts + console entries; a failing pre-script aborts the send.

## 7. Architecture

Component pattern (gitui-style) over an async action loop (the documented ratatui pattern):

- A central **`Action` enum**; components translate key/mouse events into actions; an update step applies actions to state.
- **UI components** (sidebar, url bar, editor tabs, response viewer, modal stack, toasts, palette) each own their state, input handling, and draw.
- **`tokio::select!`** event loop over crossterm's `EventStream`, a render tick, and an mpsc channel carrying results from background tasks.
- **HTTP sends and script runs execute in spawned tasks**, reporting back as actions (`ResponseArrived`, `ScriptFailed`, …). The UI never blocks; in-flight requests are cancellable.
- Rendering reads a single app-state struct; no component reaches into another's internals.

Error handling: all user-facing failures (network, parse, script, storage) become toast + console entries — never a crash or a silently swallowed error. File writes are atomic (write-temp-then-rename).

## 8. Roadmap

| Stage | Scope | Exit criterion |
|---|---|---|
| **1. Foundation** | Workspace skeleton, event loop, component framework, keybindings, **theme/token system**, core chrome (panes, modals, toasts, palette), pane layout with placeholders | App runs, looks designed, navigation works |
| **2. HTTP core** | Request TOML files, editor (method/URL/params/headers/body), send via reqwest, response viewer (highlighting, collapse, search) | Usable as a basic daily HTTP client |
| **3. Projects & environments** | Sidebar tree, project switcher, environment files + selector, simple variables with `{{var}}` substitution + picker + autocomplete, default headers | Multi-project, multi-env workflows |
| **4. Advanced variables** | Enumerated options with descriptions, groups with linked updates, secrets | The differentiator works end-to-end |
| **5. Scripting** | boa integration, `pt.*` API, pre/post hooks, script-set variables, console pane for script output | Login-saves-token flow works |
| **6. Interop & polish** | Copy actions/clipboard, export to curl, paste-curl import, Postman v2.1 collection + environment import, project export | Migration from Postman is real |
| **7. Console & history** | Wire log (resolved request as actually sent), searchable history, promote history entry to saved request | Debugging parity with Postman's console |

Stages 1→2→3 are sequential. After stage 3, **stages 4, 5, and 6 are independent and can proceed in parallel**; stage 7 can start any time after stage 2.

## 9. Testing

- **`postui-core`:** plain unit tests — variable resolution and precedence, group selection semantics, `{{var}}` substitution, TOML round-tripping, curl generation, Postman v2.1 import against fixture files.
- **HTTP:** integration tests against a local **wiremock** server.
- **Scripting:** boa-executed script tests over the `pt.*` API (pure Rust, runs in CI).
- **TUI:** ratatui `TestBackend` snapshot/assertion tests for components (rendering, focus, modal stack).
- TDD throughout; CI runs the full suite on Linux and macOS.
