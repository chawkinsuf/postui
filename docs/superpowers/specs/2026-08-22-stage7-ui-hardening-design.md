# postui — Stage 7: UI Hardening

*2026-08-22 · Status: approved design. No new capabilities — this stage makes the existing basics rock solid before any feature work. The competition/feature-gap backlog is deliberately out of scope.*

## 1. Goals

Fix every usability defect the user reported, plus the closely related ones found while investigating them:

1. Requests can't be duplicated.
2. Essential actions (saving foremost) aren't mouse-accessible.
3. Clicking a line in the body editor lands on the last character of the document, not the end of the clicked line.
4. The headers actually sent (defaults, auto `Content-Type`, client headers) are invisible.
5. A variable's current value can't be seen from within a request.
6. Header/param row editing is a select-then-edit trap (click selects, click deselects, double-click falls into add-row with no mouse exit).
7. The Variable Manager and group management are unusable — hidden single-key commands, a confusing mega-grid, a confusing group model, and awkward editing flow (all four confirmed by the user).
8. The 2 MiB pretty-print cap is a workaround, not a solution.

**Out of scope:** everything in the competition-research backlog — auth helpers, non-JSON bodies, cookies, curl/Postman/OpenAPI import, history, console, TLS/network settings, scripting. Those resume after this stage.

## 2. Interaction principles (apply everywhere)

- **"Both" style:** primary actions get visible buttons/toolbars next to what they act on; secondary actions (rename, duplicate, delete) live in right-click context menus. Every existing keyboard shortcut survives, but **nothing is reachable only by keyboard**.
- **No trap states:** any click anywhere always does something sensible; `Esc` and click-away always exit an editing state by committing or reverting, never by ignoring the input.

## 3. Variable model & manager redesign (the core of the stage)

### 3.1 Conceptual model

A **group is a set of linked fields with named entries** — records, not an environment-like switcher. Example: a *user* consists of `user_id` + `customer_id`; the *entries* are `user 1`, `user 2`, …; picking an entry fills all its fields at once. Entries **belong to a specific environment**: staging's `user 2` doesn't exist in prod. Single-variable enumerated options are unified into this model as a one-field group — one concept instead of two.

### 3.2 On-disk format (breaking change, with migration)

`variables.toml` — declarations only, no values for groups:

```toml
[base_url]
description = "API root"
default = "https://api.example.com"   # simple vars keep a declaration default

[api_key]
secret = true                          # unchanged: values only in .local/secrets.toml

[groups.user]
description = "Linked user/customer pair"
fields = ["user_id", "customer_id"]   # renamed from `members`; ordered
```

`environments/<env>.toml` — flat values for simple vars (as today) plus that environment's group entries:

```toml
base_url = "https://stg.api.example.com"

[entries.user."user 1"]
user_id = "1001"
customer_id = "cust-77"

[entries.user."user 2"]
user_id = "1002"
customer_id = "cust-91"
```

- Entry names are free-form strings (unlike variable names); each entry must supply every field of its group — missing/extra fields are load-time validation errors with the existing friendly-error style.
- `entries` joins `options`/`groups` as a reserved name.
- Selections (`name → entry name`, per environment) stay in `.local/state.toml`; a selection naming a nonexistent entry degrades to "needs selection" exactly as stale selections do today.
- With no environment active, group fields are unresolved ("need a selection" at send time); simple vars fall back to their declaration default as today. Groups have no declaration-level entries — that's the point of the model.
- Removed syntax: `[options.<key>]` on variables, `[groups.<g>.options.<key>]`, per-env keyed option overrides (`EnvData.options`). `prepare`/resolution keep their public behavior (same `VarMeta` classification, same send-time error classes) with `Enumerated` collapsing into `GroupMember`.

### 3.3 Migration

On loading a project whose files use stage-6 syntax, offer a one-shot migration (confirm modal; writes atomically; a `.bak` copy of each rewritten file is left beside it):

- Variable `options` → a one-field group of the same name (the variable becomes the group's single field).
- Group `members` → `fields`.
- Declaration-level option values → `[entries.…]` in **every** environment file, with per-env keyed overrides applied on top. If the project has no environments, create `environments/default.toml` to hold the entries and say so in the confirm modal.
- Existing selections in `.local/state.toml` carry over unchanged (option keys become entry names).
- Declining the migration leaves files untouched and variables inert (sidebar-error style, project still loads); there is no dual-format support in the model code.
- Migration lives in `postui-core` with unit tests, including a fixture matching the user's real project layout.

### 3.4 Manager UI (master-detail)

Full-screen manager (`alt+v` / header chip, as today), replacing the grid:

```
┌ Variables ──────────┬────────────────────────────────────────┐
│ Environment: staging ▾            [+ Variable] [+ Group]     │
├─────────────────────┼────────────────────────────────────────┤
│ VARIABLES           │  Group: user            [+ Entry] [Edit fields]
│  base_url           │  entry     user_id   customer_id       │
│  api_key      🔒    │  ● user 1   1001      cust-77          │
│ GROUPS              │  ○ user 2   1002      cust-91          │
│ ▶ user (user 2)     │  ● = selected for staging              │
└─────────────────────┴────────────────────────────────────────┘
```

- **Left:** flat list — variables (secret badge, unresolved marker), then groups (current selection shown inline). Click to open in the detail pane; right-click for Rename / Duplicate / Delete.
- **Right, variable selected:** a small form — description, secret toggle, declaration default, and this environment's value (masked + reveal for secrets). Buttons for the operations that exist today as `p`/`P` (promote/demote between request/project/env scope) where applicable.
- **Right, group selected:** the entries table — rows = entries, columns = fields, leading radio column sets this environment's selection. `[+ Entry]` appends an empty in-place row; `[Edit fields]` opens a small field-list editor (add/rename/reorder/remove a field; removing warns that the column's values are deleted). Right-click an entry row: Duplicate entry, Delete entry.
- Environment switcher at the top swaps the whole value/entry layer.
- All current single-letter commands remain as shortcuts of the corresponding visible controls.
- Writes stay per-cell atomic-on-commit as today; a failed write keeps the text in the editor with a toast.
- The **var picker** (`ctrl+v` / `{{`) keeps its role; group fields show `grp` badges and previews resolve through the active entry selection. "New variable…" ghost-row flow is unchanged.
- The request-level `[variables]` tab is unaffected by the format change (simple name/value overlay, highest precedence).

## 4. In-place table editing

Applies to params/headers/request-vars tables and the new entries tables. The select-then-edit model is removed.

- Click a cell → caret in that cell, type immediately. Click another cell → commit, edit there. Click outside the table → commit and leave.
- `Enter` commits the row; `Esc` reverts the active cell to its pre-edit value (and exits editing); `Tab`/`Shift-Tab` move between cells, wrapping to the next/previous row.
- The "+ Add" ghost row is an always-present empty row: clicking into it starts editing; committing with any non-empty key creates the row; leaving it empty discards it silently.
- Checkbox and per-row `✕` keep single-click behavior; double-click is no longer meaningful (no distinct action to reach).
- Keyboard navigation (arrows between rows, `Enter` to start editing the focused cell) is preserved for parity.

## 5. Mouse-accessibility pass

- **New shared component: context menu** — a popup action list anchored at the pointer, painted above everything, closed by selection, `Esc`, or click-away. A click-away only closes the menu and is swallowed — it never also activates what was under it (one click, one effect). Items can be disabled with a reason. Right-click events are routed through `HitMap` like left-clicks.
- **Editor pane toolbar** (always visible while a request is open): `[Save]` (dirty-state indicator), `[Format]` `[Minify]` (body tab), `[{{vars}} on/off]` (body substitution toggle), `[$EDITOR]`. Save is no longer footer-only.
- **Response pane:** clickable search icon opening the search input (in addition to `/`), ▲/▼ next/previous match buttons beside it. View tabs already click.
- **Context menus:** sidebar request row (Open, Duplicate, Rename, Delete), folder row (New request here, Expand/Collapse), broken-file row (Show error), table row (Duplicate row, Delete row, Extract value to variable), manager left-list and entry rows (§3.4).
- **Sweep of remaining keyboard-only actions:** var picker gets a click affordance (the `{{ }}` toolbar toggle area / palette), the Vars editor tab gets its missing `alt+4` binding, extract-to-variable is reachable from the table context menu. Acceptance check: every `Action` variant reachable by keybinding is also reachable by mouse (button, menu, or palette-with-clickable-launcher), verified against the `Hit` enum during review.

## 6. Request headers — computed view

The editor's Headers tab gains a read-only **auto section** below the editable rows, separated by a divider, showing exactly what `prepare` + the HTTP client will send: project `[default_headers]` (struck through when suppressed by an override), the auto-added `Content-Type: application/json`, and client-generated headers (`Host`, `Content-Length`). Values render with `{{vars}}` resolved against the current environment (secrets masked, existing reveal toggle applies); unresolved tokens show red as in §7. Rows are greyed/dimmed and uneditable; a per-row copy icon matches the response headers tab. The section recomputes live as the request or environment changes.

## 7. Inline variable value preview

- Every `{{token}}` in the URL bar, table cells, and body editor is highlighted: tinted when it resolves, red when unresolved/needs-selection/missing-secret.
- Hovering a token (mouse-motion events; already captured for hover states) or resting the caret inside one for a tick pops a small tooltip: `name = value` (masked for secrets), plus the source scope (request / env / default / group "user → user 2").
- Clicking a token still opens the var picker prefiltered to it.

## 8. Small fixes

- **Body editor click** (§goal 3): clicking past the end of a line places the caret at that line's end; clicking below the last line goes to the end of the last line. Fixed in our edtui forwarding layer with a regression test.
- **Duplicate request** (§goal 1): sidebar context menu, palette entry ("Request: duplicate"), and a rebindable action. Creates `<slug>-copy` (then `-copy-2`, …) beside the original via the existing atomic save path and opens it.

## 9. Async pretty-print (remove the 2 MiB cap)

`MAX_PRETTY_BYTES` and the forced-Raw gate are removed. `JsonTree::parse` runs on a background task (same pattern as sends: generation-tagged, reported over the mpsc channel; stale results dropped). While parsing, the Pretty tab shows the braille spinner; Raw and Headers are available immediately. Small bodies (< ~256 KiB) parse synchronously to avoid a one-frame flicker. A parse superseded by a new response or request switch is abandoned. The existing 2 MiB integration test flips from "asserts capped" to "asserts Pretty eventually renders".

## 10. Testing

- `postui-core`: unit tests for the new model parsing/validation errors, entry resolution, selection staleness, and the migration (including the no-environments case and a real-layout fixture).
- `postui`: component tests for in-place table editing state machine (commit/revert/ghost-row), context-menu routing, computed-header section, tooltip resolution, duplicate-slug generation, async-parse lifecycle.
- Manual tmux-driven sweep (per the established recipe) of every mouse path before review, checked against the §5 acceptance list.

## 11. Risks / notes

- The variables format change touches `varmodel`/`vars`/`varedit`/`varmanager` (~9k lines) — the largest single piece; the manager rewrite should shed code, not grow it.
- Mouse-motion hover tooltips depend on motion events reaching us in all supported terminals; the caret-resting fallback covers terminals where they don't.
- `to_toml_string()` ordering rule (model.rs) applies to all new writers.
- Existing single-letter manager keys are kept, but any that conflict with in-place editing (typing into a cell) yield to text input — commands act only when no cell edit is active.
