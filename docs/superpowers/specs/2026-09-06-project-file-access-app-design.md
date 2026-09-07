# Project File Access (app migration) Design

Stages 3–5 of `2026-09-05-project-file-access-design.md`: the TUI moves
onto `Project`, gains a `Config` object, and a lint proves that nothing
else touches the filesystem. Everything in the parent spec still holds;
this document records the decisions the parent left to this stage.

## Scope

One plan covering all three stages. The branch `project-file-access`
cannot merge until the lint lands, because until then `main` would have
two owners of project files. No user-visible behaviour change; a
difference discovered while migrating an arm stops the task and is
raised.

## App shape

- `App.project` is `Option<Project>`. `None` is the no-project state and
  replaces `ProjectContext::empty()` and the `refuse_without_project`
  gate. `App::project()` and `App::project_mut()` return options so every
  arm uses an early return. Tests use `App::proj()`, which unwraps.
- `ProjectContext` is deleted at the end of the migration, not the start.
  Each area task moves its arms; the legacy free functions keep the rest
  compiling until the last migration task removes `project_ctx.rs` and
  the free functions together.
- `open_error` becomes core's `OpenError`. A refused startup project
  still yields the empty state with the error shown in the sidebar.

## Undo

- `StepKind` keeps `EditorDelta` and gains `Project { label, slug_hint }`,
  a marker meaning "undo one journal entry". `FileStates`, `Trashed` and
  `Reorder` are removed with `write_file_states`, `read_file_states`,
  `replay_order_edits` and `History::rename_space`.
- The undo arm: pop a step; an editor delta applies as today; a marker
  calls `Project::undo` (or `redo`), then the sidebar, session and editor
  refresh from the returned `Undone`, whose request moves re-key the
  editor slug and session. `Undone.warnings` are shown as toasts.
- Core applies the active-environment transition inside `undo`; the app's
  `SwitchEnv` on undo is dropped.
- Keyboard reorder bursts merge in both stacks under the same 2 s rule:
  core's `transaction_merging` merges the journal entry, `History::try_merge`
  merges the marker, so the stacks stay one-to-one.
- A failed replay rolls back in core and keeps its entry; the app shows
  the error and leaves the marker in place.

## Local state

The core setters that are memory-only today (`set_expanded`,
`set_main_split`, `set_active_space`, `set_open_request`,
`record_space_open`) write `.local/state.toml` immediately, as the legacy
code does. `Action::PersistLocalState` is removed. This closes the
hand-off gap where `reload_all` after an unrelated undo could revert an
unpersisted split or expanded set.

## Migration order

0. **Residual fix in core.** The transaction snapshot keeps
   `open_request_keys`; restore rebuilds the held requests from those keys
   and re-reads them from disk. Test: a failed `rename_space` leaves the
   held request open and unchanged.
1. **Open and switch.** `App::new`, switch/new project, the Projects
   chooser, the file picker's is-project check, and `manage_list`'s
   create-space and ensure-project calls go through `Project::open`,
   `init`, `is_project`, `create_space`.
2. **Requests.** Open, save, create, rename, move, move-all, delete,
   duplicate, request and space reorder. Each arm is one `Project` call
   plus one marker step. The editor keeps the draft and the dirty gate.
3. **Spaces and environments.** Create, rename, delete, TLS, set-active,
   selection and secret cascades. `varmanager.sync` takes `&Project`.
4. **Variables.** `VarEdit` enum in core, one variant per operation the
   varmanager performs (upsert, rename, delete, selector edits, option
   fields, env values, secret flag, promote). `Project::apply_var_edit`
   runs the journaled primitive and its cascade as one entry with one
   label. The usage scan calls `Project::scan_usage`.
5. **Undo and poll.** The undo rewrite above. `Action::Tick` calls
   `Project::poll`, which also reloads held requests (today's reload
   re-reads the open request). Then `project_ctx.rs`, the removed app
   helpers, and the legacy free functions are deleted; test references
   move to `App::proj()` and `Project` accessors.

## Config

`Config` in the TUI crate holds `Settings`, `KeyMap`, `ThemeRegistry` and
`UiSettings`, loaded once at startup through its own `Disk` rooted at the
XDG config dir. `Usage` and the two `ui.toml` writers become `Config`
methods; the theme reload action re-reads through `Config`. A config file
that will not parse is reported at startup and never written over, as
today.

### Reload from disk

`Action::ReloadFromDisk` — `alt+r`, the palette's "Reload from disk", and
the Manage bar's Reload button — is the user's way to pick up edits made
outside the app without restarting it. It does both halves at once: a
forced (mtime-gated checks skipped) re-read of the open project's files
with the sidebar rebuild `ReloadProjectFiles` performs — or, when startup
refused to open that project, a retry of the open through
`ForceSwitchProject` — and a `Config::reload` of the user-editable XDG
config files: `config.toml` (the projects registry and the UI settings,
theme included), `keys.toml` and a full rescan of `themes/` — `ui.toml`
is app-owned state and is not re-read. Unlike startup, a config file that
exists but will not read or parse yields `None` rather than its defaults:
the app keeps whatever it already had for that file and warns, so a
syntax error (or a permission problem) in one file never silently resets
settings the user is relying on. A new keymap is assigned straight to
`app.keymap`; `handle_key` reads it there, once, before dispatching, so a
reload can only change the meaning of the *next* key. The editor's buffer
is never touched — a reload re-reads what is on disk
around the user's unsaved edits, it is not a discard.

## Host filesystem and the lint

A `hostfs` module in the TUI crate owns the three sites that touch files
outside any project: writing an export to a user-chosen path, listing a
directory for the file picker, and the external-editor temp-file
round-trip. Nothing else in either crate names `std::fs`.

The lint is one test per crate. It reads every source file under `src/`
and fails, naming the file and line, if `std::fs` (or `use std::fs`)
appears outside `crates/postui-core/src/disk.rs`,
`crates/postui/src/hostfs.rs`, and `#[cfg(test)]` blocks. It is the last
task so it proves the migration is complete.

## Verification

The existing app and acceptance tests pass with their assertions
unchanged. A manual tmux pass covers create, rename, move, move-all,
delete, duplicate, space and environment rename, a variable cascade, and
undo/redo of each.
