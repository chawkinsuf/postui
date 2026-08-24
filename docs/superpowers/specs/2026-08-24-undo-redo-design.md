# Undo/Redo System — Design

Date: 2026-08-24
Status: approved

## Goal

A single global undo/redo history covering every user-facing mutation in
postui: request edits (URL, method, headers, params, variables, body, name),
sidebar structural operations (create/delete/rename/move/duplicate request),
variable-manager edits, environment edits, secrets, and request saves.
Ctrl+Z undoes the last change wherever it happened; Ctrl+Shift+Z (alias
Ctrl+Y) redoes.

## Decisions

- **Scope: global.** One interleaved history across editor edits and
  disk-immediate operations. Ctrl+Z always undoes the chronologically-last
  change regardless of kind.
- **Persistence: in-memory only.** History lives for the session and is
  gone on quit. A deleted request is recoverable only until the app exits.
- **Granularity: coalesced bursts.** Consecutive typing in the same field
  merges into one step until the user pauses (~2s), switches fields, or a
  structural/disk action intervenes.
- **Save semantics: full symmetry.** Undoing a disk-immediate op rewrites
  disk (deleted request comes back, rename reverts). Undoing past a request
  save reverts only the editor and marks it dirty again; the file keeps the
  saved content until the next save (IDE-style).
- **Cross-request: jump back and revert.** If the last step belongs to a
  request that is no longer open, undo reopens that request (bypassing the
  dirty gate — see below), applies the revert, and shows a toast naming the
  request. On-screen editor undos are silent; jumps and disk ops toast.
- **Linear history.** Any new edit clears the redo stack. No undo tree.
- **Body editor:** edtui's internal undo stack is not wired up in postui
  and stays that way — this system is the only history. Any edtui-internal
  undo binding must remain unreachable.

## Architecture

New module `crates/postui/src/undo.rs` owns the system. The app calls
`record_*`, `undo`, and `redo`; nothing else knows how history works.

```rust
pub struct History {
    undo: Vec<Step>,      // capped at ~200 steps, oldest dropped
    redo: Vec<Step>,      // cleared whenever a new step is recorded
}

pub struct Step {
    kind: StepKind,
    context: Context,     // slug, focused pane/field, cursor position
}

pub enum StepKind {
    // Open-request edit: full before/after snapshots
    EditorDelta {
        slug: String,
        before: HttpRequest,
        after: HttpRequest,
        coalesce: CoalesceKey,   // field id + timestamp
    },
    // Write-through op: inverse captured at dispatch time
    DiskOp(DiskInverse),
}

pub enum DiskInverse {
    RestoreRequestFile { slug: String, content: String },  // undoes delete
    DeleteRequestFile { slug: String },                    // undoes create/duplicate
    RenameRequest { from: String, to: String },            // undoes rename/move
    RestoreVarFiles { files: Vec<(PathBuf, String)> },     // undoes var/env/secret edits
    SaveRequest { prev_saved: Option<HttpRequest> },       // undoes save (memory only)
}
```

Disk inverses store *content*, not descriptions: undoing a delete rewrites
the file from stored TOML text; undoing a variable edit restores the old
file text captured at the `ProjectContext::edit_*` chokepoints. Undo works
even though history is in-memory, at the cost of recoverability ending at
app exit.

`Context` records the request slug, focused pane/field, and cursor position
at capture time so undo can jump back and place the cursor.

## Capture

### Editor deltas — shadow diff

`App` holds `shadow: Option<(String, HttpRequest)>` — slug and last-known
state of the open request. After **every** processed input event (key or
mouse), the main loop calls `app.capture_undo()`:

1. Reassemble `editor.current_request()`; compare to the shadow
   (`PartialEq`, cheap at this size).
2. If different, push `EditorDelta { before: shadow, after: current }` and
   update the shadow.
3. If unchanged, do nothing.

The hook sits in `main.rs` after `handle_key` / `handle_mouse` return, so
it catches both the Action path and the direct-mutation path with no
changes to input code. Opening or creating a request resets the shadow
(the open itself is not a delta). `$EDITOR` body edits return via
`pending_terminal_action`; the hook runs after completion, so an external
edit is one step.

### Coalescing

A new delta merges into the top step (keeping the top's `before`, taking
the new `after`) when all of:

- top of stack is an `EditorDelta` for the same slug,
- same `CoalesceKey` (url / body / one specific table cell / name),
- less than ~2 seconds since the top step was recorded,
- no undo/redo has happened since.

Switching fields, pausing, or any disk op breaks the burst. Deliberate
wholesale changes — format/minify body, insert-variable, discard-changes,
method change — set a per-frame no-coalesce flag so they always stand as
their own step.

### Disk-op inverses

Captured inside `App::apply` in each mutating arm, before the storage
call; pushed only if the storage call succeeds:

| Operation | Inverse captured |
|---|---|
| Delete request | read file content first → `RestoreRequestFile` |
| Create / duplicate request | `DeleteRequestFile` |
| Rename / move request | `RenameRequest` reversed |
| Var / env / secret edits | old file text from `ProjectContext::edit_*` → `RestoreVarFiles` |
| Save request | previous `editor.saved` snapshot → `SaveRequest` |

**Not recorded** (deliberately out of history): `.local/state.toml` writes
(expanded folders, selections, active-env switch — navigation, not edits),
sending requests, response/session state, and the one-shot migration
rewrite (already makes `.bak` files).

## Applying undo/redo

New `Action::Undo` / `Action::Redo`, bound to Ctrl+Z and Ctrl+Shift+Z
(plus Ctrl+Y redo alias for terminals without the Kitty keyboard
protocol), with command-palette entries. Active on Main and VarManager
screens; inert while a modal is open.

**`EditorDelta`** (undo applies `before`, redo applies `after`; one code
path):

- *Same request open:* decompose the snapshot into editor fields (inverse
  of `current_request()`, largely the existing load path), restore
  cursor/focus from `Context`, sync the shadow to the applied state so the
  capture hook doesn't record the undo as a new edit.
- *Jump-back:* switch to the step's request **bypassing the dirty gate**.
  Safe by construction: the departing request's unsaved state is fully
  reconstructible from the stack — every change to it was captured, and
  the newest delta's `after` is exactly its latest state, so redo walks
  back to it. Toast: "Undid edit in <name>".

**`DiskOp`**: replay the stored inverse through the existing storage
functions (atomic writes, same error handling), then trigger the existing
refresh paths (sidebar rebuild for request-file ops, varmanager/resolved
refresh for variable ops). The forward inverse for the redo stack is
captured at undo time the same way the original was. `SaveRequest` undo
touches no files: it restores the previous `editor.saved`, so `is_dirty()`
flips back on.

**Failure handling.** A file may have changed or vanished outside the app
between capture and undo. Every apply is fallible; on failure the step is
discarded, an error toast explains, and the rest of history stays usable.
A failed undo never half-applies: disk steps are single-file atomic
writes, editor steps are pure memory.

**Empty stack**: quiet "Nothing to undo" / "Nothing to redo" toast.

## Testing

Unit tests in `undo.rs`: push/undo/redo ordering, redo cleared on new
edit, cap eviction, coalescing rules (merge on same field, break on field
switch / timeout / no-coalesce flag). Injectable time source; no sleeps.

App-level tests (existing harness in `app/tests.rs`, temp-dir projects,
synthetic events):

- type in URL → undo restores prior text and cursor
- edit two fields → two steps
- delete request → undo restores file on disk with identical content →
  redo deletes again
- rename → undo → file back at old path
- variable edit → undo restores old file text
- save → edit → undo ×2 → editor matches pre-save state, `is_dirty()` true
- edit A → open B → undo jumps to A, reverts, toast, no dirty-gate prompt;
  redo returns to B's latest state
- external deletion of a file → undo of its step fails with toast, history
  remains usable
- format-body stands as its own step mid-typing-burst

Manual tmux pass: Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y in Ghostty, body typing
bursts, mouse-driven table edits, `$EDITOR` round-trip.

## Files touched

| File | Change |
|---|---|
| `crates/postui/src/undo.rs` | **new** — `History`, `Step`, capture/apply logic |
| `crates/postui/src/app.rs` | `history` + `shadow` fields, `capture_undo()`, `Undo`/`Redo` arms, inverse capture in mutating arms, dirty-gate bypass for jump-back |
| `crates/postui/src/main.rs` | call `capture_undo()` after each input event |
| `crates/postui/src/action.rs` | `Undo`, `Redo` variants + palette metadata |
| `crates/postui/src/keys.rs` | Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y bindings |
| `crates/postui/src/components/editor.rs` | snapshot-apply (decompose into fields, restore cursor) |
| `crates/postui/src/project_ctx.rs` | `edit_*` methods return old file text |
| `crates/postui/src/app/tests.rs` | new test module |

`postui-core` is unchanged except possibly a small storage helper. Input
handling (`line_input.rs`, `table_editor.rs`, `mouse.rs`, edtui wiring) is
untouched — the payoff of the shadow-diff approach.
