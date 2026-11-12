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
    // Write-through op: before/after file states captured around the op
    FileStates {
        // (path, text) pairs; None = file absent. Undo writes `before`,
        // redo writes `after` — one apply function, no per-op variants,
        // multi-file ops (rename = two paths, group reshape = vars +
        // every env file) handled uniformly.
        before: Vec<(PathBuf, Option<String>)>,
        after: Vec<(PathBuf, Option<String>)>,
        // create-environment also switches the active env; undo/redo
        // restores it alongside the files when set.
        active_env: Option<(Option<String>, Option<String>)>, // (before, after)
    },
    // Undoes a request save: memory only — restores the previous
    // `editor.saved`, so `is_dirty()` flips back on and the file keeps
    // its saved content.
    SaveRequest { slug: String, prev_saved: Option<HttpRequest> },
}
```

Disk steps store *content*, not descriptions: the touched files' full text
is read just before and just after the operation. Undo works even though
history is in-memory, at the cost of recoverability ending at app exit.

`Context` records the request slug so undo can jump back to the step's
request. It records no caret (ruling 2026-09-18, superseding the earlier
stored-caret design and the 2026-09-17 "only a changed field" refinement):
undo/redo never move focus or the caret, the same as the Manage screen.
The editor's own caret is captured across the snapshot swap and re-placed
against the new contents — URL caret clamped, body row/col clamped with
the viewport seeded near it, table row re-found by key (row indices shift
under undo) with the old index, clamped to the ghost row, as the fallback
when the key is gone. The cell cursor column persists on its own.

*Amended 2026-10-26 (task #121, mine (approved)):* an `EditorDelta` step
also records the table cursor (tab and stop) from just before its op and
just after it. "Before" is the cursor at the end of the event before the
step's first edit, taken with the shadow. A swap that removes the row
under the table cursor (undoing a row add, a duplicate, an auto-header
take-over) puts the cursor on the step's recorded spot for that direction,
when it was recorded on the same tab, in place of the clamped index; the
swap restores exactly the request the spot was taken in, so its index
names the same row. Focus never moves, a cursor whose row survives stays
put, and a cursor on `+ Add` stays on `+ Add` unless the swap brings a row
back (R8's reselect of the restored row). An add through `+ Add` (the add
verb, a click on it) moves the cursor there to open the cell before the
row exists, so its undo lands on `+ Add`, wherever the verb was pressed
(user, 2026-10-26: "Just leave the cursor on + new").

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

Performance note: compare before cloning — `PartialEq` short-circuits on
the first differing field, and a clone happens only when a change is
detected. Large bodies make the equality check a memcmp-speed string
compare, not a per-keystroke clone.

The hook sits in `main.rs` after `handle_key` / `handle_mouse` return, so
it catches both the Action path and the direct-mutation path with no
changes to input code. Opening or creating a request resets the shadow
(the open itself is not a delta). `$EDITOR` body edits return via
`pending_terminal_action`; the hook runs after completion, so an external
edit is one step.

*Amended 2026-10-19 (task #66, split from #62; the commit as its own step is mine, the
gated case and the open rule below are `mine (approved)`):* a table cell
committed on the way
to something else (`App::commit_table_edit`, the one path that commits a
cell outside the table's own keys) is captured right there, as a step of
its own, before the rest of the event runs. A click on another row's
toggle or trash, or a click away whose action edits (the TLS lock), would
otherwise land in the same step as the commit, and one undo would take
the typing back with it. Whatever was pending before the commit is
captured first, so the commit is bracketed on both sides: a method change
whose tab sync commits the cell is its own step before the commit.

*Amended 2026-10-19 (tasks #63, #67, #65; spec
2026-10-19-commit-cell-first, which replaces the rest of the #66
amendment above, its record-beneath step and its load drop):* a cell
is open only while the caret is in it. `capture_undo` commits a cell the
caret has left (alt+u, a pane jump, the jq bar, search, Manage) as its
own step, as the user ruled: "when the cursor leaves the cell, the input
should close and the undo immediately recorded". A modal over the cell
is not leaving it (`mine (approved)`). Every dirty gate commits an open
cell before it asks (`App::leaving_unsaved`), and every path that
replaces the editor or records an op's before-view commits it first
(`App::flush_field_session`), so the cell's typing is asked about,
saved, and brought back by undo like any other edit (`mine
(approved)`). Cancel on the prompt therefore finds the cell closed with
its typing committed (`mine (approved)`). Nothing outside the table
reads an uncommitted draft. `Editor::load` still drops a stray cell as a
backstop; the row selection stays, so Save As keeps it.

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

Captured inside `App::apply` in each mutating arm: the touched files are
read into `before` ahead of the storage call and into `after` once it
succeeds; the step is pushed only on success:

| Operation | Files captured |
|---|---|
| Delete request | the request file (`after` = absent) |
| Create / duplicate request | the new request file (`before` = absent) |
| Rename / move request | both paths (old: content→absent, new: absent→content) |
| Var / env / secret edits | `variables.toml`, `environments/*.toml`, `.local/secrets.toml` — whichever the op touches (a shared snapshot helper reads them all; identical before/after entries are dropped) |
| Create environment | the new env file (`before` = absent), plus the active-env transition |
| Save request | none — `SaveRequest { prev_saved }` step, memory only |

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
  of `current_request()`, largely the existing load path), keep focus and
  the caret where they are, sync the shadow to the applied state so the
  capture hook doesn't record the undo as a new edit.
- *Jump-back:* switch to the step's request **bypassing the dirty gate**.
  Safe by construction: the departing request's unsaved state is fully
  reconstructible from the stack — every change to it was captured, and
  the newest delta's `after` is exactly its latest state, so redo walks
  back to it. Toast: "Undid edit in <name>".

  Belt-and-braces guard: the bypass is only valid if capture never missed
  a change. Before jumping, if the departing editor `is_dirty()` and its
  current state does not equal the newest history delta's `after` for that
  slug (or no such delta exists), fall back to the normal dirty-gate
  prompt instead of bypassing. A capture bug then degrades to a UX
  annoyance, never silent data loss.

**`FileStates`**: undo writes every `before` entry (content via the
existing atomic writer; `None` deletes the file), redo writes every
`after` entry — the same apply function both ways, nothing re-derived at
undo time. Then the existing refresh paths run: sidebar rebuild for
request-file steps, project-file reload + varmanager refresh for
variable steps, and the active-env transition is restored when the step
carries one. `SaveRequest` undo touches no files: it restores the
previous `editor.saved`, so `is_dirty()` flips back on.

**Failure handling.** A file may have changed or vanished outside the app
between capture and undo. Every apply is fallible; on failure the step is
discarded, an error toast explains, and the rest of history stays usable.
Each file write within a step is individually atomic, but a multi-file
`FileStates` step can still fail partway through: on a mid-step failure,
the writes that already succeeded stand, the UI (sidebar, project reload,
Variable Manager) is refreshed to match what's now on disk, and the step
itself is dropped rather than retried. Editor steps are pure memory and
either fully apply or not at all.

*Amended 2026-11-11 (task #283; design and rules in
`2026-11-11-outside-edit-undo-step-design.md`):* text changed outside the
app is not a reason to refuse an undo or redo. It becomes a step of its
own (you, as a question on the task: "Should we scrap all this and just
treat loading external edits as its own undo step?").

- A step that wrote a file's text expects the file to still hold that
  text. When an undo or redo finds other text there (a `git pull`, a hand
  edit, an agent), that press does not replay the step. It writes the
  step's text back into those files, and the text it found goes onto the
  other stack as a step. A warning toast names the files: "Took back
  changes made outside the app to project.toml. Redo brings them back."
  (found by a redo: "Undo brings them back."). The next press replays the
  step as usual. When the step is made and what the press does are mine
  (approved): the user answered "1: A" and "2: A" to the two questions on
  the task.
- The step for the outside text is undone and redone like any other. Its
  toasts are "Put back changes made outside the app to …" and "Took back
  changes made outside the app to …" (mine).
- It makes no difference whether the app has re-read the changed file,
  and a re-read does not touch the history.
- Nothing is refused over text. A file that does not parse is taken back
  like any other. The refusals over paths stay as they were (a file that
  vanished, a name taken), and a refused step is still dropped. A file
  that cannot be read as text fails the undo or redo, which writes
  nothing to it and keeps its step for a retry (mine).
- A dropped step leaves its text in the files it wrote. The step beneath
  that wrote one of those files finds that text the same way: its first
  press takes it back, with the same toast although the text was the
  app's, and the next press replays the step (mine).
- Two kinds of file are not compared, because the app itself writes them
  outside the history and their text since the step can be the app's own:
  `.postui/state.toml` and request files (a save). Undoing a request
  rename over an outside edit of that request is task #285 (mine).
  *Amended 2026-11-12 (task #285, mine):* a request rename sets only the
  `name` key in the request's file, the rest of its text as it is, and its
  undo and redo do the same with the name from before and after the
  rename, in whatever text the file holds then: a save or an outside edit
  made since stays, and no toast says so. A file that is no longer TOML
  moves back as it is.
- The sidebar also writes `project.toml` outside the history, when it
  finds requests or folders its order lists don't name (drag into folders
  D5a). The history follows that write: the step that would next rewrite
  the file is taken to have left the text the sidebar wrote, so that
  write is not met as a change made outside. A sidebar write over text
  that was changed outside is not followed, and both are taken back
  together (mine).

The user's reason, with the answers (task #283, comment 1296): "Again,
this is talking about outside edits, which should be a rare case. This at
least lets the user know what happened, without messing up what they were
doing. And with these files often in git anyway, likely they could recover
the edits later if they wanted to."

What it costs: the first press writes over the outside edit on disk. After
that the edit is kept only in the history: a new change clears the redo
stack, and quitting or switching projects clears everything.

**Empty stack**: quiet "Nothing to undo" / "Nothing to redo" toast.

## Testing

Unit tests in `undo.rs`: push/undo/redo ordering, redo cleared on new
edit, cap eviction, coalescing rules (merge on same field, break on field
switch / timeout / no-coalesce flag). Injectable time source; no sleeps.

App-level tests (existing harness in `app/tests.rs`, temp-dir projects,
synthetic events):

- type in URL → undo restores prior text; focus and caret stay put
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
| `crates/postui/src/project_ctx.rs` | var-file snapshot helper (reads `variables.toml` + env files + secrets as `(PathBuf, Option<String>)` pairs) |
| `crates/postui/src/app/tests.rs` | new test module |

`postui-core` is unchanged except possibly a small storage helper. Input
handling (`line_input.rs`, `table_editor.rs`, `mouse.rs`, edtui wiring) is
untouched — the payoff of the shadow-diff approach.
