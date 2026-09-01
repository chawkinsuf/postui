# Spaces and the Manage Screen — Design

Date: 2026-09-01
Status: approved

## Goal

Add **spaces**: a mandatory, top-level partition of a project's requests
that the user swaps between with a key press. Exactly one space is visible
at a time; each space remembers which request it had open. This is the
i3/tmux-window model, not Postman's folders — nested folders inside a
space keep working as they do today.

Alongside it, generalise the Variable Manager into a tabbed **Manage**
screen (Variables / Environments / Spaces) so that spaces and environments
have a real editing surface. Deleting an environment is not exposed in the
UI today; this closes that gap too.

The name "space" is provisional. It appears in user-facing labels, the
`spaces` key in `project.toml`, and identifiers; a rename before
implementation is a find-and-replace.

## Decisions

- **Space = top-level directory under `requests/`.** A request's space is
  the first segment of its slug: `auth/login` is request `login` in space
  `auth`; `auth/tokens/refresh` is a nested folder inside `auth`. Every
  request is in exactly one space. No new per-request metadata.
- **Files directly under `requests/` are ignored**, reported in the same
  warning channel as today's walk errors ("`requests/foo.toml` is not in a
  space; move it into a space directory"). **No migration**: the app is
  pre-release and the user reorganises the config directory by hand.
- **Order lives in `project.toml`:** `spaces = ["main", "auth", "billing"]`.
  Listed names come first in that order; existing directories not listed
  are appended alphabetically; a listed name with no directory still counts
  as a (empty) space, which is how an empty space survives git. The UI's
  create/rename/delete/reorder operations rewrite the list.
- **A fresh project starts with one space, `main`.** `ensure_project`
  creates `requests/main/` and writes `spaces = ["main"]`.
- **One visible space; the sidebar is rooted at it.** The space name is
  never a row.
- **Per-space last-open request** in `.local/state.toml`; the `expanded`
  set is unchanged (its paths already carry the space prefix).
- **Switching spaces goes through the existing dirty gate**, same as
  opening another request.
- **Keys:** `alt+1`..`alt+9` jump by position; `alt+]` / `alt+[` next and
  previous, wrapping. All rebindable.
- **Header, not sidebar,** hosts the space dropdown, beside the env
  dropdown. The sidebar keeps only the New request button.
- **Variable Manager → Manage screen** with Variables, Environments and
  Spaces tabs. The header chip reads `Manage` (`alt+v` unchanged).
- **Request reordering is out of scope** (its own follow-on brainstorm).

## Architecture

### Core: spaces (`postui-core/src/project.rs`, `storage.rs`)

`ProjectMeta` gains `spaces: Vec<String>` (serde default empty; the struct
keeps `deny_unknown_fields`).

New in `project.rs`:

- `list_spaces(root, meta) -> Vec<String>` — the ordering rule above.
  Directory names that fail `validate_slug` (or contain `/`) are skipped.
- `create_space(root, name)` — validates the name, `create_dir_all`s
  `requests/<name>/`, appends to `spaces`. Errors on an existing space (in
  the list or on disk).
- `rename_space(root, from, to)` — validates, refuses if `to` exists,
  renames the directory (creating it first if `from` was list-only),
  rewrites the list entry. Callers translate `open_request` /
  per-space state / `expanded` prefixes from `from/` to `to/`.
- `delete_space(root, name)` — removes the directory recursively and the
  list entry. Refuses when it is the only space (`ProjectError::LastSpace`).
  The caller owns the confirmation.
- `move_space(root, name, delta: i32)` — swaps position in the list,
  clamped at the ends; materialises unlisted directories into the list
  first so the written order is the displayed order.
- `write_spaces(root, spaces)` — `toml_edit`-based edit of `project.toml`
  that touches only the `spaces` key, preserving the rest of the file and
  its comments (same discipline as `varedit`). Creates the file when
  missing.

New in `storage.rs`:

- `space_of(slug) -> Option<&str>` — first segment when the slug has at
  least two segments.
- `list_requests` is unchanged in shape but now skips top-level files and
  reports each as a warning in the returned error string (joined, as the
  walk error is today).
- `move_request_to_space(root, slug, space) -> Result<String>` — a
  `rename_request` to `<space>/<rest>` keeping the sub-path, with the
  existing unique-slug collision rule. Returns the new slug.

`ensure_project` grows to create `requests/main/` and the `spaces` list
when the project has no spaces at all.

### Core: environments (`project.rs`)

- `rename_environment(root, from, to)` — validates, refuses if the target
  file exists, renames `environments/<from>.toml`. Does not touch
  `.local/secrets.toml` or local selections; the caller re-keys those.
- `delete_environment(root, name)` — removes the file; `NotFound` if
  absent.

### Local state (`project.rs::LocalState`, `project_ctx.rs`)

`LocalState` gains:

```toml
space = "auth"                 # active space
[space_open]                   # space → last-open request slug
main = "main/health"
auth = "auth/login"
```

`open_request` stays as the currently open slug (the main-screen
restore on launch is unchanged). `ProjectContext` holds `spaces:
Vec<String>`, `active_space: String`, `space_open: IndexMap<String,
String>`, exposes `set_active_space`, `record_space_open(slug)`, and the
space/environment CRUD wrappers that reload `spaces` / `environments`
after each write and persist local state. `reload_if_changed` watches
`project.toml` and the `requests/` directory listing so an external
`mkdir` shows up.

Startup resolves `active_space` as: the stored one if it still exists,
else the space of `open_request`, else the first space. If a stored
`open_request` is not in the active space, the active space wins and the
request restored is that space's `space_open` entry.

### Switching (`app.rs`)

`Action::SwitchSpace(String)` → dirty gate → `Action::ForceSwitchSpace`:

1. Record the currently open request under the outgoing space.
2. Set the active space, persist.
3. Rebuild the sidebar rooted at the space.
4. Open, in order: the space's `space_open` entry if the file still
   exists; else the first request in sidebar order; else the empty editor.

`Action::JumpSpace(usize)` (1-based; out of range is a silent no-op) and
`Action::CycleSpace(i32)` (wrapping) both resolve to `SwitchSpace`.

`OpenRequest(slug)` for a slug outside the active space first switches to
that space (still one dirty gate, not two) and then opens it. The palette's
request search covers every space and displays the space as its detail
column, so a cross-space pick reads as such before Enter.

### Sidebar (`components/sidebar.rs`)

`refresh` takes the active space and filters the listing to slugs with
that prefix, then builds rows with `prefix = "<space>/"`, so depth 0 is the
space's own top level. Row slugs stay full slugs (`auth/login`) — every
existing action, `expanded` entry and hit keeps working.

New request, rename and duplicate all resolve inside the active space:
`split_display_path` output is prefixed with `<space>/` at the call site.
A typed leading `/` is not special-cased; a user cannot address another
space from the name prompt.

The request row's right-click menu gains **Move to space ▸** with one
entry per other space; selecting one calls `move_request_to_space`, and if
the moved request was open, follows it (switches space and opens the new
slug).

Footer chips when the sidebar is focused: existing new/rename/delete plus
`alt+] space`.

### Header (`components/header_bar.rs`, `hit.rs`)

Left cluster order: wordmark, project chip, **space chip**, env chip
(with its cycle pill), Manage chip. Theme stays on the right.

The space chip mirrors the env chip's idiom: `▾ auth` with an `alt+]`
cycle pill (`Hit::HeaderSpace`, `Hit::HeaderSpaceCycle`). Its dropdown
lists every space as `1 main`, `2 auth`, … with ✓ on the active one, then
`new space…` and `manage spaces…`. The env dropdown gains a `manage
environments…` row after `new environment…`.

The Variable Manager chip is relabelled `Manage`; `Hit::HeaderVars` and
`Action::OpenVarManager` are renamed to match (`HeaderManage`,
`OpenManage { tab }`).

### Manage screen (`components/varmanager.rs` → `manage.rs`)

`Screen::VarManager` becomes `Screen::Manage` and `ManageState` wraps the
existing `VarManagerState` plus a `tab: ManageTab` (Variables,
Environments, Spaces) and one `ListEditState` per simple tab. The top bar
gains a tab strip at its left (painted with the editor's existing tab
idiom); the Variables tab's environment switcher and `+ Variable` /
`+ Selector` buttons only render on that tab. `alt+v` opens on the last
used tab; the header dropdowns' manage rows open on their tab.

Environments and Spaces tabs share one **ListEditState** face:

- Left: the item list (environments in file order; spaces in `spaces`
  order, numbered). Top of the left list: a `+ New` button that opens the
  existing name prompt.
- Right, for the selected item: a name field (edit in place, commit =
  rename), then a button row: **Rename** (focuses the field), **Delete**,
  and on the Spaces tab **Move up** / **Move down**. A muted line beneath
  shows the space's request count, or the environment's variable-value
  count.
- Keys within the list: `enter` edit name, `d` delete, and on Spaces
  `alt+up` / `alt+down` move. Footer chips advertise them.

Delete semantics:

- Environment: confirm modal ("Delete environment `staging`? Its values
  and secrets are removed."). Removes the file, drops its
  `selections[name]` from local state and its section from
  `.local/secrets.toml`; if it was active, the project switches to no
  environment.
- Space: confirm modal with the count ("Delete space `auth` and its 7
  requests?"). Refused with a toast when it is the last space. If it was
  the active space, switch to the first remaining space before deleting.
  If the open request lived in it, the editor is cleared (through the
  dirty gate first).

Rename semantics: environment rename re-keys `selections`, the secrets
section, and `environment` in local state. Space rename re-keys
`space_open`, `active_space`, `open_request`, every `expanded` entry with
the old prefix, and the editor's open slug.

### Keys (`config.rs`, `keys.rs`)

Default bindings: `alt+1`..`alt+9` → `JumpSpace(n)`, `alt+]` →
`CycleSpace(1)`, `alt+[` → `CycleSpace(-1)`. Active on the Main and
Manage screens when no modal is open. Configurable under the same
`[keys]` table as existing bindings.

## Error handling

- Invalid space or environment names (not `a-z 0-9 - _`, or containing
  `/`) are rejected at the prompt with the prompt's inline error, as
  `NewEnvironment` does today.
- Directory operations that fail (permissions, non-empty rename target)
  toast the error and leave state unchanged; `project.toml` is written
  only after the filesystem operation succeeds.
- A `project.toml` that lists a space whose name fails validation is
  reported as a warning and the entry skipped, never rewritten silently.
- `JumpSpace` beyond the last space is a no-op, no toast.

## Testing

Core (`postui-core`):

- `list_spaces`: listed order wins; unlisted dirs append alphabetically;
  list-only names appear; invalid dir names skipped.
- `create`/`rename`/`delete`/`move_space` each update both disk and the
  list; `delete_space` refuses the last space; `write_spaces` preserves
  unrelated `project.toml` content and comments.
- `list_requests` skips top-level files and reports them.
- `space_of` and `move_request_to_space` (keeps sub-path, applies the
  collision rule).
- `rename_environment` / `delete_environment` happy paths and refusals.
- `LocalState` round-trips `space` and `space_open`.

App (`crates/postui/src/app/tests.rs`):

- Switching restores the per-space open request; falls back to first /
  empty; records the outgoing request.
- Dirty gate fires on switch; cancel leaves space unchanged.
- `JumpSpace` out of range is a no-op; `CycleSpace` wraps both ways.
- Sidebar rows never include another space's slug; new/rename/duplicate
  land inside the active space.
- Cross-space `OpenRequest` switches then opens with one gate.
- Move to space follows an open request.
- Startup resolution of `active_space` in the three cases above.
- Header space dropdown items and ✓ position; manage rows route to the
  right tab.
- Manage screen: tab switching; environment delete clears the active env
  and its selections; space delete of the active space switches first;
  space rename re-keys `expanded` and `space_open`.

## Out of scope

- Request reordering within a space or folder.
- Migration of pre-space projects.
- Per-space environment or selector state (both stay project-global).
- Drag-and-drop between spaces in the sidebar.
