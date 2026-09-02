# Spaces and the Manage Screen — Design

Date: 2026-09-01
Status: implemented

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
  previous, wrapping; `alt+shift+s` opens the space dropdown (mirroring
  `alt+shift+e` for environments). All rebindable. **The editor tabs give
  up `alt+1`..`alt+4`** (decided 2026-09-01): tabs keep `alt+left` /
  `alt+right` and clicks, and the `editor_tab_N` names stay bindable in
  `keys.toml`. Consequence: with the caret in the body editor, where
  alt+arrows are word motion, switching tabs is by mouse or by leaving
  the body first.
- **Header, not sidebar,** hosts the space dropdown, beside the env
  dropdown. The sidebar keeps only the New request button.
- **Variable Manager → Manage screen** with Variables, Environments and
  Spaces tabs. The header chip reads `Manage` (`alt+v` unchanged).
- **Deletes go to a trash directory, not `remove`.** Request, environment
  and space deletes rename the path into `.local/trash/`; undo renames it
  back. One rename regardless of size, so undo cost is independent of how
  much was deleted. The trash is emptied at project open, matching the
  per-session undo history.
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
- `delete_space(root, name) -> Result<Option<Trashed>>` — trashes the
  directory (if it exists on disk) and removes the list entry. Refuses when
  it is the only space (`ProjectError::LastSpace`). The caller owns the
  confirmation.
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
- `move_all_requests(root, from, to) -> Result<Vec<(String, String)>>` —
  `move_request_to_space` over every request in `from` (nested folders
  keep their sub-paths). Stops at the first failure and reports how far it
  got; already-moved files stay moved.

`ensure_project` grows to create `requests/main/` and the `spaces` list
when the project has no spaces at all. `list_requests`' walk skips
`.local/` (it lives outside `requests/` already, so no change is needed
there; noted so the trash never leaks into the sidebar).

### Core: environments (`project.rs`)

- `rename_environment(root, from, to)` — validates, refuses if the target
  file exists, renames `environments/<from>.toml`. Does not touch
  `.local/secrets.toml` or local selections; the caller re-keys those.
- `delete_environment(root, name) -> Result<Trashed>` — trashes the file;
  `NotFound` if absent.

### Core: trash (`postui-core/src/trash.rs`)

```rust
pub struct Trashed { pub original: PathBuf, pub trashed: PathBuf }
pub fn trash(root: &Path, path: &Path) -> io::Result<Trashed>;
pub fn restore(t: &Trashed) -> io::Result<()>;
pub fn retrash(t: &Trashed) -> io::Result<()>;
pub fn empty(root: &Path) -> io::Result<()>;
```

`trash` renames `path` (file or directory) to
`root/.local/trash/<n>/<path relative to root>`, where `<n>` is a
per-call counter (max existing + 1) so two deletes of the same path never
collide. Parent directories under the trash slot are created as needed.
The rename stays within the project directory, so it is a single
same-filesystem `rename` whatever the size. `restore` renames it back and
fails with `AlreadyExists` if the original path is now occupied (never
clobbers). `retrash` renames it back into its recorded trash slot (redo).
`empty` removes `root/.local/trash/` recursively; `ProjectContext::open`
calls it, so the trash never outlives a session. `storage::delete_request`
returns a `Trashed` too.

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

### Undo: `StepKind::Trashed` (`undo.rs`, `app.rs`)

```rust
Trashed {
    items: Vec<postui_core::trash::Trashed>,
    /// Small companion files the delete also rewrote (`project.toml`'s
    /// `spaces` list, `.local/secrets.toml`), as FileStates-style
    /// before/after contents — bounded, unlike the trashed payload.
    files_before: Vec<(PathBuf, Option<String>)>,
    files_after: Vec<(PathBuf, Option<String>)>,
    active_env: Option<(Option<String>, Option<String>)>,
}
```

There is no step grouping in `History`, so a delete's side effects ride
inside the one `Trashed` step. Undo calls `restore` on each item in
reverse order, then writes `files_before`; redo calls `retrash` in order,
then writes `files_after`. A failure toasts, leaves earlier renames in the
step standing, drops the step, and reloads project files + sidebar — the
same policy as `FileStates`. After either direction the app reloads project files,
refreshes the sidebar and spaces, and re-syncs the Manage screen.
`DeleteRequest` moves from `FileStates` to `Trashed`; its toast and undo
hint are unchanged. `FileStates` stays for edits.

`Action::JumpSpace(usize)` (1-based; out of range is a silent no-op) and
`Action::CycleSpace(i32)` (wrapping) both resolve to `SwitchSpace`.

`OpenRequest(slug)` for a slug outside the active space first switches to
that space (still one dirty gate, not two) and then opens it — this is the
rule every open route (undo's jump-to-request, startup restore, a future
request search) inherits. The command palette has no request search today
and this spec does not add one.

### Sidebar (`components/sidebar.rs`)

`refresh` takes the active space and filters the listing to slugs with
that prefix, then builds rows with `prefix = "<space>/"`, so depth 0 is the
space's own top level. Row slugs stay full slugs (`auth/login`) — every
existing action, `expanded` entry and hit keeps working.

New request, rename and duplicate all resolve inside the active space:
`split_display_path` output is prefixed with `<space>/` at the call site.
A typed leading `/` is not special-cased; a user cannot address another
space from the name prompt.

The request row's right-click menu gains one flat **Move to <space>** row
per other space (the dropdown has no submenus); selecting one calls
`move_request_to_space`, records a `FileStates` step exactly like a rename
(one file), and if the moved request was open, follows it (switches space
and opens the new slug).

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
  and on the Spaces tab **Move up** / **Move down** and **Move all
  requests to ▸** (a dropdown of the other spaces). A muted line beneath
  shows the space's request count, or the environment's file path.
- Keys within the list: `enter` edit name, `n` new, `d` delete, and on
  Spaces `alt+up` / `alt+down` move. Footer chips advertise them. Tabs
  switch by click or `alt+left` / `alt+right`.
- Undo coverage on these tabs: deletes are `Trashed` steps; an
  environment rename is a `FileStates` step; **a space rename, a reorder,
  and Move all requests are not undo steps** (a directory rename or a bulk
  move has no bounded content capture; rename or move back by hand).

Delete semantics:

- Environment: confirm modal ("Delete environment `staging`? Its values
  and secrets are removed."). Trashes the file, drops its
  `selections[name]` from local state and its section from
  `.local/secrets.toml`; if it was active, the project switches to no
  environment. Recorded as one `Trashed` step: the env file as the item,
  `.local/secrets.toml` in the companion files, and `active_env` set so
  undo restores the active environment. `.local/state.toml` rides in the
  companion files too, so undo also restores that environment's
  per-environment selections. Rename is a `FileStates` step as env edits
  are today, with the same `state.toml` companion.
- Space: confirm modal. Title: "Delete space `auth`?". Body: "Its 7
  requests will be deleted." The confirm button reads "Delete 7 requests"
  (not "OK"/"Yes"); an empty space gets "Delete space" and drops the body
  line. The directory is trashed and recorded as a `Trashed` step; the
  toast carries the undo hint like a request delete. Undo restores the
  directory and re-adds the name to `spaces` at its old position
  (`project.toml` rides in the step's companion files). The intended path for keeping requests is still **Move all
  requests to ▸** first. Refused with a toast when it is the last space. If
  it was the active space, switch to the first remaining space before
  deleting. If the open request lived in it, the editor is cleared
  (through the dirty gate first).

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
- `trash`: file and directory; two deletes of the same path get distinct
  slots; `restore` refuses an occupied original; `retrash` round-trips;
  `empty` removes everything and tolerates a missing directory.
- App: request, environment and space deletes each push a `Trashed` step;
  undo restores the file/directory (and the space's list entry, and the
  active env); redo re-trashes; an occupied original path fails the undo
  with a toast and drops the step; project open empties the trash.
- The space delete modal's confirm label carries the count.
- `LocalState` round-trips `space` and `space_open`.

App (`crates/postui/src/app/tests.rs`):

- Switching restores the per-space open request; falls back to first /
  empty; records the outgoing request.
- Dirty gate fires on switch; cancel leaves space unchanged.
- `JumpSpace` out of range is a no-op; `CycleSpace` wraps both ways.
- Sidebar rows never include another space's slug; new/rename/duplicate
  land inside the active space.
- Cross-space `OpenRequest` switches then opens with one gate.
- Move to space follows an open request; move-all moves nested paths
  intact and follows the open request.
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
- A trash that survives the session (a "restore deleted" browser). The
  trash exists only to back this session's undo.

## Implementation notes (2026-09-01)

Rulings taken during implementation that amend the text above.

- E/E': `ensure_project` writes the `spaces` list only for a real project
  (`project.toml` present); a bare directory gets `requests/main/` but
  never a `project.toml` behind the "create a project here?" consent gate.
  On open, an existing project with no `spaces` key has its on-disk
  directories materialised into the list.
- F: the loose-file warning (and the sibling "not in a valid space"
  warning) toasts as `Warning`, once per change, not per refresh; walk
  errors keep the `Error` channel. An invalid name listed in `spaces` is
  reported the same way and is never rewritten away.
- G: in the Manage bar the tab strip has priority; right-aligned buttons
  are dropped (+ Selector, then + Variable, then Close) to make room.
- H: in the header, the space-cycle and env-cycle keycap pills yield
  first at narrow widths, then the Manage keycap; chip labels never
  shorten. The space chip reads `Space: <name> ▾`.
- I/I': move-all and move-to-space are dirty-gated when the open request
  is affected, via `ForceMoveAllRequests` / `ForceMoveRequestToSpace`;
  `ForceOpenRequest` owns the follow.
- J: env rename/delete steps carry `.local/state.toml` as a companion
  file, and a file-level undo/redo reloads `selections` from it.
- K: the Manage right-pane button row wraps to a second row rather than
  dropping buttons on overflow.
