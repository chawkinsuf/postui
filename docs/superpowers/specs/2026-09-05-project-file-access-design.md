# Project File Access — Design

Date: 2026-09-05
Status: approved

## Goal

One official view of the open project. A single object owns every read
and write of project files, holds the parsed documents in memory, and
records its own undo journal. Individual components and the app never
read or write project files themselves.

This is an architectural refactor with **no user-visible behaviour
change**. The ask-before-reload and refuse-unasked-overwrite rules for
files changed outside the app are a later feature; this design gives them
one place to live.

## Why

Before this design, three layers each touched disk independently:

- `postui-core` had about 40 free functions across `project.rs`,
  `storage.rs`, `order.rs`, `trash.rs` and `varedit.rs`, each taking a
  root path and reading or writing files itself, with no shared state.
- `ProjectContext` in the TUI crate held the parsed documents and
  reloaded them by mtime, with its own atomic writer.
- `app.rs` bypassed both with 140 direct calls into core persistence
  plus raw filesystem calls for undo snapshots, undo writes, move
  snapshots and export. A few components did the same.

A write to `project.toml` could originate from any of the three, and none
knew about the others. Three review rounds of a disk-change guard each
found new bypasses, because the guard had no single choke point. The
guard work was reverted (branch `disk-guard-attempt` keeps it) and this
refactor replaces the foundation it needed.

## Decisions

- **Owner in `postui-core`.** A stateful `Project` type owns the root,
  the parsed documents, and every read and write. Core stays usable
  without the TUI. `ProjectContext` is deleted.
- **Three file families, three objects.** `Project` (shared files),
  `Local` reached as `project.local()` (per-project `.local/`), and
  `Config` in the TUI crate (XDG config dir). All three read and write
  through one `Disk` unit in core.
- **Save timing unchanged.** The request editor saves on demand with the
  existing dirty gate. Every other edit (variables, environments, spaces,
  order, selections) writes immediately, as today. Explicit save for
  variables and environments is left open as a later change; nothing
  here blocks it.
- **Undo lives in `Project`.** Mutating methods append inverse entries to
  a journal; undo and redo replay through the same methods. The app's
  history keeps editor text deltas plus markers. No raw file snapshots
  anywhere in the app.
- **Inverse operations, not content snapshots, for requests.** A move is
  undone by moving back; a delete by restoring from trash; a create by
  trashing. Move-all with thousands of requests stores slug pairs, never
  bodies, so no size limit is needed. Variable, environment and secret
  edits keep the prior text of the one to three small files they touch,
  which is what makes "undo restores exactly" hold for TOML with
  comments.
- **Requests loaded on demand.** The listing and order lists are always
  in memory; a request is parsed when opened and held while open.
  Nothing unsaved is ever unloaded (the dirty gate guarantees it), and
  editor undo steps are whole-request snapshots, so a reloaded request
  always matches what the step expects.
- **Editor holds the draft.** `Project` holds the saved request; the
  editor widgets are the working copy. Dirty means the editor's current
  request differs from what `Project` holds. Save hands the editor's
  request back to `Project`.
- **Outside changes: today's behaviour.** `Project::poll` compares Disk
  stamps and silently re-reads what changed, exactly as
  `reload_if_changed` does now. Disk records a stamp on every read and
  write so the future guard is a change to `Disk::write` and
  `Project::poll` only.
- **Out of scope.** Exporting a response or request to a user-chosen
  path is not a project file; it stays in one function in `app.rs`
  using Disk's atomic write helper. Pure slug and order helpers with no
  disk access stay as free functions.

## Architecture

### Public API (core)

```rust
pub struct Project { /* root, disk, meta, variables, environments, requests, local, journal */ }

impl Project {
    pub fn open(root: PathBuf) -> Result<(Project, Vec<Warning>), OpenError>;
    pub fn init(root: &Path, name: Option<&str>) -> Result<Project, OpenError>;

    // Reading: borrowed views of the in-memory truth
    pub fn meta(&self) -> &ProjectMeta;
    pub fn variables(&self) -> &VarModel;
    pub fn environments(&self) -> &[EnvName];
    pub fn environment(&self, name: &str) -> Option<&EnvData>; // active env held; others on demand
    pub fn spaces(&self) -> &[SpaceName];
    pub fn requests(&self) -> &Listing;                          // names, paths, order applied
    pub fn resolved(&self) -> &Resolved;                         // recomputed when inputs change
    pub fn local(&self) -> &Local;
    pub fn local_mut(&mut self) -> &mut Local;

    // Requests: loaded on demand, held while open
    pub fn open_request(&mut self, slug: &str) -> Result<&HttpRequest, ProjectError>;
    pub fn close_request(&mut self, slug: &str);
    pub fn save_request(&mut self, slug: &str, req: &HttpRequest) -> Result<(), ProjectError>;
    pub fn create_request(&mut self, ...) -> Result<Slug, ProjectError>;
    pub fn rename_request(&mut self, from: &str, to: &str) -> Result<(), ProjectError>;
    pub fn move_request(&mut self, slug: &str, space: &str) -> Result<Slug, ProjectError>;
    pub fn move_all_requests(&mut self, from: &str, to: &str) -> Result<Vec<(Slug, Slug)>, ProjectError>;
    pub fn delete_request(&mut self, slug: &str) -> Result<(), ProjectError>;
    pub fn duplicate_request(&mut self, slug: &str) -> Result<Slug, ProjectError>;

    // Spaces, environments, variables: mutate memory and write, one call each
    pub fn create_space / rename_space / delete_space / set_space_order / set_request_order;
    pub fn create_environment / rename_environment / delete_environment / set_env_tls;
    pub fn edit_variables(&mut self, edit: VarEdit) -> Result<(), ProjectError>;

    // Undo journal, replayed through the same methods
    pub fn undo(&mut self) -> Result<Option<Undone>, ProjectError>;
    pub fn redo(&mut self) -> Result<Option<Undone>, ProjectError>;
    pub fn journal_len(&self) -> usize;
    pub fn clear_journal(&mut self);

    // Outside changes: today's silent reload, stamps compared
    pub fn poll(&mut self) -> (Changed, Vec<Warning>);

    // Migration (stage 6 → 7) runs inside the object
    pub fn pending_migration(&self) -> Option<&MigrationOutcome>;
    pub fn apply_migration(&mut self) -> Result<Vec<Warning>, ProjectError>;
    pub fn decline_migration(&mut self);
}

pub struct Local { /* selections, shared selections, expanded, open request per space, secrets */ }
impl Local {
    pub fn set_selection_for / clear_selection_for / set_expanded / record_open_request
        / set_secret_for / remove_secret_for ...   // each writes .local/state.toml or .local/secrets.toml
}
```

`VarEdit` is one enum covering every `varedit` operation (upsert, rename,
delete, selector and option edits, promote, env value set) including its
cascades across environment files. `Undone` tells the app what to refresh
(sidebar, variable manager, active environment transition, request to
reopen).

`ProjectError` carries the relative path and operation for disk failures
and the file name for parse failures. The existing rule stands: a file
that exists but will not parse is fatal at open, reported, and never
written over.

### Public API (TUI crate)

```rust
pub struct Config { /* config.toml, keys.toml, themes/, ui.toml */ }
impl Config {
    pub fn load() -> Result<(Config, Vec<Warning>), ConfigError>;
    pub fn settings(&self) -> &Settings;
    pub fn keys(&self) -> &KeyMap;
    pub fn themes(&self) -> &ThemeRegistry;
    pub fn record_usage(&mut self, ...) -> Result<(), ConfigError>;   // the only writer of ui.toml
}
```

Behaviour is unchanged, including the rule that a config file that will
not parse is reported at startup and never saved over.

### Internal structure of `Project`

A facade over document units, each owning one kind of document and
testable alone: meta and order lists, variables, environments, secrets,
local state, requests, trash. Every unit reads and writes through the
single `Disk`. No unit constructs an absolute path or calls `std::fs`.

### Disk

The only code in the workspace that touches the filesystem for project,
local and config files.

```rust
pub struct Disk { root: PathBuf, stamps: HashMap<RelPath, Stamp> }

impl Disk {
    pub fn read(&mut self, rel: &RelPath) -> Result<Option<String>, DiskError>;  // None = absent
    pub fn write(&mut self, rel: &RelPath, text: &str) -> Result<(), DiskError>; // atomic, keeps mode
    pub fn remove(&mut self, rel: &RelPath) -> Result<(), DiskError>;
    pub fn rename(&mut self, from: &RelPath, to: &RelPath) -> Result<(), DiskError>;
    pub fn list(&mut self, rel: &RelPath) -> Result<Vec<Entry>, DiskError>;
    pub fn changed(&self, rel: &RelPath) -> bool;   // current mtime vs recorded; used by poll
}
```

- **Relative paths only.** `RelPath` is built by document units from
  slugs and validated to stay under the root.
- **Atomic writes** use the temp-file-and-rename approach from commit
  `ab5da59` (kept on `disk-guard-attempt`), preserving the file mode and
  writing through symlinks. That commit's tests return with it.
- **Stamps** are recorded on every read, write, list and rename. In this
  refactor they serve only `poll`.
- **Trash** is a set of Disk renames into `.local/trash/N/` and back,
  not a separate module with its own filesystem calls.
- **Errors** carry the relative path and the operation. Parse errors are
  the document units' concern.
- **Config** uses its own `Disk` rooted at the config dir.

### Undo journal

```rust
enum Entry {
    Renamed { from: Slug, to: Slug },                        // request rename or move
    MovedAll { pairs: Vec<(Slug, Slug)>, orders: OrderEdits },
    Created { slug: Slug },                                   // inverse: trash it
    Trashed { ticket: TrashTicket },                          // inverse: restore
    SpaceRenamed { from: String, to: String },
    SpaceCreated { name: String },
    SpaceTrashed { ticket: TrashTicket },
    EnvRenamed { from: String, to: String },
    EnvCreated { name: String },
    EnvTrashed { ticket: TrashTicket },
    TextReplaced { files: Vec<(RelPath, Option<String>)> },   // variables, env, secrets: prior text
    OrderChanged { edits: OrderEdits },
}
```

- Every entry records what it needs to undo and nothing more.
- Undo replays through the normal methods, which append the redo entry.
  There is no second write path.
- All or nothing per entry: before touching disk, undo checks every path
  the entry affects exists or is absent as expected, and refuses the
  whole entry naming the path on a conflict. The app toasts and drops the
  step, as today.
- Cascades are part of the entry: a request move carries its order-list
  edits, a space rename carries the local-state re-keying, an environment
  rename carries its secrets rename.
- Local-state writes (selections, expanded folders, active environment)
  are not journaled, as today.
- Cap of 200 entries, oldest dropped. Trash tickets of dropped entries
  stay in the trash until the next open empties it, as today.

### App history

`undo::StepKind` shrinks to `EditorDelta` (unchanged) and `ProjectEntry`,
a marker with no payload. Every `Project` mutation the app performs pushes
a marker at the moment it happens, so one Ctrl+Z walks editor edits and
project operations in chronological order. On a marker the app calls
`project.undo()` and refreshes the UI from the returned `Undone`.

Journal and history are cleared together on project switch, through one
call, so a future user-facing reload has one place to clear them too.
(A reload that did not clear history was a gap found during the guard
reviews.)

### What changes in the app and components

- `ProjectContext` is deleted. Its parsed fields become `Project`'s, its
  selections and expanded folders become `Local`'s, its `resolved` a
  method on `Project`. `App` holds `Option<Project>`; the no-project
  state is `None`, which replaces the `refuse_without_project` gate.
- The 140 direct core calls in `app.rs` become `Project` methods. The
  file-operation arms (create, rename, move, move-all, delete, duplicate,
  environment and variable cascades) each become one method call plus
  one history marker.
- `write_file_states`, `read_file_states`, and the trash and reorder
  replay leave `app.rs`. The undo arm becomes: editor delta, or
  `project.undo()` then refresh.
- Components stop calling core persistence: `manage_list` create-space
  and ensure-project, `varmanager` usage scan, `file_picker` is-project.
  Pure helpers (`space_of`, `relative`, `level_of`) stay.
- The timer reload action calls `project.poll()` and refreshes the
  sidebar on change, as today.
- `config.rs`, `keys.rs`, `theme/registry.rs` and `usage.rs` file access
  is replaced by `Config`, loaded once at startup.
- Tests that set up projects by writing files directly are unchanged:
  they simulate the outside world. Tests reaching into `ProjectContext`
  fields move to `Project` accessors. Core persistence tests move to the
  unit that owns the document.

## Sequencing

On a branch, in stages that each compile and pass the full suite:

1. **Disk** in core, with tests and the atomic writer from `ab5da59`.
   Nothing uses it yet.
2. **Project, Local and the journal** in core, built on Disk, alongside
   the existing free functions, which become thin wrappers so the app
   keeps working. Core tests move across.
3. **App migration**, split by area: requests; spaces; environments and
   variables; undo; timer reload. `ProjectContext` and the wrappers are
   deleted at the end.
4. **Config** object.
5. **A lint test** in each crate that fails if `std::fs` appears outside
   Disk, the export function, and test modules.

## Verification

Behavioural: the existing acceptance and app tests pass unchanged, plus a
manual pass in tmux of the file operations with undo (create, rename,
move, move-all, delete, duplicate, space and environment rename, variable
cascade). Any behaviour change discovered during migration is stopped and
raised, not folded in.

## Follow-ups this design enables (not part of it)

- Files changed outside the app: ask before reloading, never overwrite
  unasked. A change to `Disk::write` and `Project::poll`.
- Explicit save for variables and environments.
