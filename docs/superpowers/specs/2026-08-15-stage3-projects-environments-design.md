# Stage 3 — Projects & Environments (design)

Date: 2026-08-15. Parent spec: `2026-08-15-postui-design.md` (binding authority; this
document refines its stage-3 row). Builds on stage 2 as merged at `70c1e16`.

## Goal

Multi-project, multi-environment workflows: several project directories the user can
switch between without friction, environment files with an always-visible selector,
simple `{{var}}` substitution with a picker, and project-level default headers.

Exit criterion (from the roadmap): a user can keep separate projects (including ones
embedded in git repos), switch between them and their environments with single
keypresses, and parameterize requests with variables resolved per environment.

## Scope decisions made during brainstorming

- **Variable editing is TOML-only this stage.** The app substitutes, lists, and
  inserts variables; defining or changing them means editing `variables.toml` /
  environment files by hand. The Variable Manager screen, *extract to variable*,
  enumerated options, groups, secrets, and request-scoped variables all stay in
  stage 4.
- **File changes are picked up on demand** (mtime-checked re-read at well-defined
  moments), not via a file watcher. No `notify` dependency.
- **Switching is effortless**: a New Project action in the TUI, a single keypress to
  cycle projects, and a palette-style fuzzy chooser for direct jumps. Environments
  get the same treatment.

## 1. Project model (`postui-core`)

A project is a plain directory:

```
my-project/
  project.toml            # name, default headers
  requests/               # unchanged from stage 2 (nested slugs = subdirectories)
  environments/
    qa.toml               # flat name = "value" pairs
    prod.toml
  variables.toml          # declarations: description + optional default
  .gitignore              # written on creation, ignores /.local/
  .local/
    state.toml            # active environment, open request, sidebar expansion
```

- `project.toml`:

  ```toml
  name = "My API"

  [default_headers]
  accept = "application/json"
  authorization = { value = "Bearer {{token}}", enabled = true }
  ```

  `default_headers` uses the same entry form as request tables (string or
  `{ value, enabled }`, indexmap order-preserving, duplicate keys = friendly parse
  error). `name` is the display name shown in the header bar and choosers; it
  defaults to the directory name if absent.

- `variables.toml` — one top-level table per variable:

  ```toml
  [base_url]
  description = "API root for this service"
  default = "http://localhost:8080"

  [token]
  description = "bearer token"
  ```

  Both fields optional. Variable names: ASCII alphanumeric plus `_` and `-`,
  case-sensitive, non-empty. Unknown fields in a variable table are a friendly
  parse error (they'd silently vanish on stage-4 rewrite otherwise).

- `environments/<env>.toml` — flat string pairs, `token = "abc123"`. The
  environment's name is the file stem; it must satisfy the single-segment slug
  rules (lowercase/digit/`-`/`_`). Values for names not declared in
  `variables.toml` are still usable in resolution (lenient), but only declared
  variables appear in the picker.

- `.local/state.toml` — `environment = "qa"`, `open_request = "users/list"`,
  `expanded = ["users", "admin/reports"]`. Written when the values change and on
  project switch/quit. Missing or invalid local state degrades to defaults with a
  toast, never an error.

- Project creation writes `project.toml`, `requests/`, `environments/`, an empty
  `variables.toml`, and a `.gitignore` containing `/.local/` (only if no
  `.gitignore` exists).

### Global registry

The global config (`~/.config/postui/config.toml`, alongside existing
theme/keymap settings) gains:

```toml
[projects]
known = ["/home/user/.config/postui/default", "/home/user/repos/svc/api-project"]
root = "~/postui-projects"     # default parent for New Project; created lazily
last = "/home/user/repos/svc/api-project"
```

- Paths are stored absolute; `~` is expanded on read. Order of `known` is the
  cycle order (most recently added last). A path that no longer exists is skipped
  with a toast, not removed automatically.
- On startup: open `last` if set and valid, else the first valid known project,
  else the migrated default project.

### Migration from stage 2

`~/.config/postui/default/` already has `requests/`. On first stage-3 launch it is
upgraded in place: write `project.toml` (`name = "default"`), `environments/`,
`variables.toml`, `.gitignore` if missing, and register it in `known`. Idempotent;
never touches existing files.

### CLI

`postui <dir>` opens `<dir>` as a project (creating/upgrading it after a
confirmation prompt if it lacks `project.toml`) and registers it in `known`.

## 2. Switching UX

- **Header bar** shows `project-name · environment-name` (environment omitted or
  shown as `no env` when the project has no environment files). This replaces the
  static app-name slot as the primary header content; exact layout follows the
  existing header component.
- **Project chooser** (new keybinding): a palette-style fuzzy modal listing known
  projects by name with their paths dimmed; typing filters, enter switches. A
  final entry **"open by path…"** prompts for a directory (line input; `~`
  expanded) and behaves like `postui <dir>`.
- **New Project** (palette entry + keybinding): modal with name and path inputs;
  path pre-filled with `<projects.root>/<slugified-name>` and editable. Creates,
  registers, and switches to it.
- **Quick cycle** (keybinding): instantly switches to the next project in `known`
  order, wrapping; a toast confirms the switch. No modal.
- **Environment chooser + cycle**: same pair of interactions over the project's
  environment files, plus a **"no environment"** entry (defaults-only
  resolution). Active environment is per-project local state.
- Switching projects with a dirty editor prompts with the existing dirty-prompt
  (save / discard / cancel) before proceeding; it never auto-saves. After the switch, the target project's
  `.local/state.toml` restores its active environment, open request, and sidebar
  expansion.
- Default keybindings are chosen at implementation time within the existing
  keymap (TOML-overridable, `keys.rs`); required actions: `project.choose`,
  `project.cycle`, `project.new`, `env.choose`, `env.cycle`. All also available
  through the command palette.

## 3. Sidebar tree

Replaces the stage-2 single-level grouping with a real tree derived from slugs:

- Folder nodes for each `requests/` subdirectory, collapsible to arbitrary depth.
  Expansion state persists in `.local/state.toml`.
- Keyboard: up/down move through visible rows; right/enter expands a collapsed
  folder (enter on a request opens it); left collapses the current folder or
  jumps to the parent.
- **Wheel scrolling is free** — it moves the viewport without moving the
  selection, and `draw` no longer snaps the viewport back to the selection
  (fixes the stage-2 snap-back finding). Keyboard navigation still scrolls to
  keep the selection visible when it moves.
- Existing CRUD is preserved; `/` in names still creates folders. Broken files
  render as before. Empty folders are not represented (folders exist only as
  slug prefixes), so no folder CRUD is needed this stage.

## 4. Variables & substitution (`postui-core`)

- **Token syntax:** `{{name}}` where `name` matches the variable-name rules, with
  optional surrounding whitespace (`{{ base_url }}`). Anything else — unmatched
  `{{`, invalid characters — is left literal. No escape mechanism this stage
  (documented limitation: a literal `{{name}}` cannot be sent in URLs, params,
  or headers; bodies can carry literal braces by leaving `substitute_body` off).
- **Resolution order (stage 3):** active environment's value → `default` in
  `variables.toml`. No script layer, no request scope yet, but the resolver's
  signature anticipates them (layered lookup).
- **Where substitution applies:** in `prepare()`, over the URL, query-param names
  and values, and header names and values (after the default-header merge) —
  always. The **body is opt-in per request**: a `substitute_body` boolean in the
  request TOML (default `false`, omitted when false). With the flag off, body
  braces are sent literally and body tokens are ignored by unresolved-variable
  checking. Toggled from the Body tab (keybinding + palette entry) with a
  visible indicator; inserting a variable via the body picker (§5) auto-enables
  the flag with a toast. Request files always keep raw `{{var}}` text;
  substitution is send-time-only.
- **Unresolved variables block the send:** if any referenced variable has no
  value after resolution, the send is aborted with a toast listing the missing
  names (and the active environment, to hint at the fix). No partial sends, no
  y/N modal.
- A `PrepareContext` (resolved variable map + default headers) is passed into
  `prepare()`; core stays IO-free — loading TOML into the context is the storage
  layer's job.

## 5. Variable picker & autocomplete

- **Line inputs** (URL, table-editor cells, and other single-line fields): typing
  `{{` opens an inline dropdown of declared variables — name, description, and
  the active environment's value (dimmed) — fuzzy-filtered by further typing.
  Enter completes the token (`{{name}}`); Esc closes the dropdown and leaves the
  literal text. A keybinding (`var.pick`) opens the same dropdown explicitly.
- **Body editor (edtui):** no on-type trigger this stage; `var.pick` opens the
  picker as a modal and inserts `{{name}}` at the cursor.
- The picker lists declared variables only (union of `variables.toml`), marking
  ones with no value in the active environment (they'd block a send if used
  without a default).

## 6. Default headers

- At prepare time, `project.toml` `default_headers` merge **under** the request's
  headers: a request header with the same name (case-insensitive, matching
  reqwest/HeaderMap semantics) overrides the inherited one; a **disabled**
  request row with that name suppresses the inherited header entirely.
- The editor's Headers tab shows inherited defaults as read-only annotated rows
  (dimmed, e.g. `(project)` marker) above the request's own rows, so the user
  sees the effective set. Editing an inherited row is not possible in-place this
  stage (project.toml is hand-edited); a request row with the same name is the
  override mechanism.

## 7. On-demand reload

Project-level files (`project.toml`, `variables.toml`, `environments/*`) and the
global registry are re-read, mtime-checked, at these moments:

- terminal focus regained (crossterm `FocusGained`; enable focus-change events in
  the event loop if not already),
- immediately before prepare/send,
- on project or environment switch,
- when opening the variable picker or either chooser.

A changed file re-parses; parse errors surface as toasts and the previous good
state is retained. The requests listing already re-reads on sidebar actions;
that behavior is unchanged.

## 8. Stage-2 deferred fixes folded in

From the stage-2 review ledger:

- Sidebar wheel-scroll snap-back — fixed by the tree rework (§3).
- Single-level dir grouping — replaced by the tree (§3).
- **edtui mouse**: forward clicks into the body editor (click-to-place-cursor)
  and implement wheel scroll (`Editor::handle_scroll`); the `mouse-support`
  feature is already enabled.
- Cheap cleanups: `create_or_save_as` double `list_requests`; rename-onto-itself
  AlreadyExists; `http::client()` `.expect` → error path; `list_requests`
  surfacing mid-walk IO errors instead of silently truncating.

Still deferred (stage 4+/6): table-editor `DrawCtx.focused` trap and non-windowed
cell drawing, horizontal scroll for long raw response lines, HeaderMap
case-duplicate collapsing, query re-encode on merge, response-viewer
`visible_indices` cost, syntax-theme matching, kitty keyboard protocol.

## 9. Out of scope (stage 4+)

Enumerated options, variable groups, secrets (`.local/secrets.toml`), request-
scoped variables, the Variable Manager screen, extract-to-variable, add-option-
from-picker, scripting (`pt.vars`), history, curl/Postman interop.

## 10. Testing

- **Core unit tests:** token parsing (valid/invalid/whitespace/unmatched),
  resolution precedence (env over default, lenient undeclared-env values),
  unresolved detection, `substitute_body` on/off behavior (literal braces
  preserved and ignored by unresolved checks when off; TOML round-trip omits
  the field when false), default-header merge/override/suppress semantics,
  `variables.toml` and environment parsing (friendly errors), registry
  round-trip with `~` expansion, migration idempotence, `.local/state.toml`
  degrade-to-defaults.
- **TUI (`TestBackend`):** sidebar tree render/expand/collapse/free-scroll,
  project & environment choosers, New Project modal, `{{` picker in line inputs,
  inherited header rows, header-bar project·env display.
- **wiremock integration:** send with substituted URL/params/headers/body against
  the active environment; default header inherited, overridden, and suppressed;
  unresolved variable blocks the send.
- **Stage-3 acceptance test:** script a two-project, two-environment workflow
  end to end (create project, define vars by writing TOML, switch env, send,
  switch project, state restored).
- Manual TTY sweep checklist to be written in the implementation plan (mirrors
  stages 1–2).
