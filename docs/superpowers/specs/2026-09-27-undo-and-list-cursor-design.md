# Undo correctness and list cursor rules (piece 2)

Status: approved by the user 2026-09-28, after a section-by-section
review (drafted 2026-09-27). Base: `main` at 743d1bb. Profile-agnostic: no vim code
is involved, and everything here lands on `main` before piece 4. The user
decided all five open questions on 2026-09-27, each by accepting the
recommendation (§7).

Every decision carries a label:

- `you`: one of the user's own rules. Rule 1 "undo restores exactly":
  "if the state after an undo is different than the state before the
  operation being undone, that is a bug that needs to be fixed." Rule 2
  "delete never confirms": "`dd` (and the delete verb generally) should
  not pop a confirm dialog. Undo negates the need to confirm." Also
  the 2026-09-27 decision that groups A and B "need to be implemented
  better".
- `mine`: Claude's recommendation. Accept or reject it.
- `earlier`: decided during the vim-mode rounds and not provably the
  user's. Treat it as a recommendation.

## 1 Background

`vim-mode` fixed undo and cursor defects one at a time. The result was
an `after_undone` built from layered special cases (`delete_reopen`,
`closed_open`, a scratch branch), a `pending_reopen_buffer` side channel
touched at 16 sites, and 16 places that set the sidebar cursor, each by
its own rule. Sweep 7 still found two cursor bugs:

- (a) After undoing a cross-space move, `enter_space` clears the cursor
  while the open row still paints selected.
- (b) A rename that re-sorts the list leaves the cursor on the old
  index. On `main` the same rename clears the cursor, because
  `Sidebar::rebuild` only re-finds a row by identity.

This spec rebuilds the behaviour on `main` around two ideas:

1. A step records the view before and after its op. Undo puts back
   what the op changed.
2. One landing path is the only code that moves the editor, the space
   and the sidebar cursor.

The vim-mode code is used as reference only (§5).

## 2 Goals and success criteria

- G1 (`you`, rule 1): after any undo, the project files, the local
  state, the open request (with its unsaved edits) and the list
  cursors are what they were before the undone op. After any redo,
  they are what the op left.
- G2 (`you`, rule 2): no delete on any list shows a dialog. This covers
  requests, variables, options, environments and spaces.
- G3 (`mine`): one function, `App::land`, changes what is open, which
  space is active and where the sidebar cursor sits. `enter_space`,
  `ForceOpenRequest`, `follow_active_space`, the delete/move/rename
  arms and the undo tail all go through it. No other code assigns
  `sidebar.selected`, apart from keyboard navigation, clicks and the
  context-menu revert.
- G4 (`mine`): no side channels. The view that an undo restores is
  stored on the step itself.
- G5 (`mine`): while the sidebar has at least one row, its cursor is
  never `None`.
- Success means:
  - every test in §6 passes;
  - both sweep-7 bugs have regression tests;
  - `grep -n "sidebar.selected = " crates/postui/src/app.rs` finds only
    `land`, `set_sidebar_selected` and the menu revert;
  - `pending_reopen_buffer`, `Reselect` and `delete_reopen` do not
    exist.

## 3 The rules

### R1 A transaction never journals `state.toml` as absent (core)

- **Who:** `you` (rule 1). The fix is vim-mode 414f2af.
- **Rule:** undoing a journaled op never deletes `.local/state.toml`.
  Redoing that op is never refused with "already exists".
- **Why:** on a fresh project the file does not exist. The first
  journaled write records `before: None`, so undo deletes the file and
  resets the active space to its default. The app's unjournaled
  `persist_local` then recreates the file before redo runs, and redo's
  preflight refuses.
- **Change:** in `crates/postui-core/src/project/mod.rs`,
  `run_transaction` (l.454). Before the snapshot, if `STATE_TOML` does
  not exist, call `self.persist_local()` and ignore any error. Update
  the doc comment on `apply_meta_active_env` in `project/undo.rs` as
  414f2af did.
- **Tests:** copy 414f2af's tests unchanged (§6.1).

### R2 Undo and redo toasts name the thing the step changed

- **Who:** "same noun as the delete toast" is `earlier` (d982511). The
  exact wording below is `mine`.
- **Rule:** a toast never names the request that merely happened to be
  open. A delete's undo reads `Restored {what}`, and its redo reads
  `Deleted {what} again`, where `{what}` is exactly the text the delete
  toast used. Every other step reads `{Undid|Redid} {phrase}`:

  | Step | `{phrase}` |
  |---|---|
  | create, save-as, duplicate of a request | `create of {display}` |
  | rename of a request | `rename to {new display}` |
  | move to another space | `move of {display} to {space}` |
  | move all | `move of {n} requests to {space}` |
  | reorder (request / space / env) | as today: `reorder of …` |
  | request-file change (secret, promote, extract) | `change to {display}` |
  | variable, selector or option write | `change to variable {name}` |
  | env create / rename / TLS | `change to environment {display}` |
  | space create / rename | `change to space {display}` |
  | whole-project change (migration) | `project change` |

  The `{what}` text for deletes is: a request's display name,
  `environment {display}`, `space {display}`, `"{name}"` for a variable,
  and `option "{name}" in {env}` for an option.
- **Change:**
  - `crates/postui/src/undo.rs`: replace `ProjectNoun` (l.76) with
    `StepLabel { verb: Verb, subject: Subject }`. `Subject` stores the
    display text captured when the op ran, so the undo of a create or
    duplicate names the display name and not a slug leaf read after
    the file is gone. That slug-leaf toast was a sweep-7 cosmetic.
  - `app.rs`: `apply_undo_step` (l.10958) builds the toast from the
    label alone.
  - Copy `VarEditOp::subject` / `VarStructOp::subject` from d3f5535
    into `components/varmanager.rs`.
- **Tests:** §6.2.

### R3 Deletes never confirm, on every list

- **Who:** `you` (rule 2). The code is from a87378e (`earlier`).
- **Rule:** request, variable and option deletes already go straight
  through on `main`. These change:
  - `Action::DeleteEnv` (l.6126) calls `ForceDeleteEnv` directly. Its
    toast becomes `Deleted environment {display}: its values and
    secrets went with it{undo hint}` (`earlier`).
  - `Action::PromptDeleteSpace` (l.6245) goes away. `DeleteSpace` calls
    `ForceDeleteSpace`, which counts the space's requests before the
    delete and toasts `Deleted space {display} and its {n} requests
    {undo hint}`, or `Deleted space {display}{hint}` when it was empty
    (`earlier`).
  - `DeleteSpace`'s unsaved-changes gate (l.6238) goes too (`mine`,
    accepted by the user 2026-09-27; OQ4). The step carries the open request's unsaved buffer (R4), so undo
    brings the edits back and the gate is a confirm by another name.
  - The move and move-all dirty gates stay. A move is not a delete.
- **Tests:** §6.3.

### R4 Undo restores the parts of the view the op changed

- **Who:** `mine`, derived from rule 1. Two sub-points were open
  questions (OQ1, OQ2). The user accepted both recommendations on
  2026-09-27.
- **Rule:**
  - Each project step records the `View` just before its op and just
    after it. `View` is the active space, what the editor holds (a slug
    or a scratch, plus its unsaved buffer when it has one), the sidebar
    cursor row, and the Manage / Variables row the op targeted.
  - Undo lands on the parts where `before` differs from `after`, taken
    from `before`. Redo lands on the same parts, taken from `after`.
  - A part the op did not change is left as the user now has it. For
    example, undoing a variable edit never touches the editor, and
    undoing the delete of a request that was not open does not reopen
    the request that was open back then.
- **What this covers:**
  - Undoing a create, save-as or duplicate reopens the request that was
    open before. Undoing a save-as from a scratch brings the scratch
    back with its content (vim-mode 7d98380 and 00dc5fa).
  - Undoing the delete of the open request reopens it (as `main` does
    today). The unsaved edits it held come back with it (f19a2ef).
  - A reopen that fails (broken file) seats nothing and leaves the
    editor where it is (05a3b4d).
  - Undo of a rename or move puts the cursor back on the row. This
    fixes sweep-7 bugs (a) and (b).
- **Change:** §4. The core field `EntryMeta::reopen` (`journal.rs`
  l.58, filled in `requests.rs` l.351 and `spaces.rs` l.186) is removed
  (`mine`). The app's own `before.open` replaces it, and the
  "persist first so the entry records the right slug" hack
  (`DeleteRequest` l.3665 and vim-mode 7d98380) goes with it.
- **Tests:** §6.4.

### R5 Undo of a delete reselects the restored row, on every list

- **Who:** `earlier` (a87378e, 85ed897). It is also what R4 gives for a
  keyboard delete, because the cursor sat on the row when it was
  deleted.
- **Rule:** after undoing a delete made from a list, the cursor of the
  list that lost the row is back on that row: sidebar, Environments,
  Spaces or Variables. A delete made elsewhere (the editor's own delete
  while the sidebar cursor sat on another row) puts the cursor back
  where it was, as rule 1 requires. The row's own list is updated even when it is not on
  screen (`mine`). Undo never switches screen or tab to show the row
  (`earlier`).
- **Change:** each list's row lives in `View` (`sidebar`, `row`). The
  vim-mode `Reselect` enum is not ported.

### R6 The sidebar cursor is separate from the open request, and always somewhere

- **Who:** separating the two is `earlier`. The never-`None` invariant
  and the landing table are `mine`.
- **Rule:**
  - The open request is `editor.slug`. `sidebar.open_slug` is its
    mirror, kept by `dispatch` (l.1901).
  - The cursor is `sidebar.selected`.
  - They coincide after anything that changes what is open. They
    diverge only through navigation or a right-click.
  - While rows exist the cursor is `Some`. This replaces the `earlier`
    rules "a cleared cursor stays cleared" (dd3e138) and "a project
    switch clears the cursor" (2137262). Both existed to keep a stale
    index off an unrelated row, and landing on the open request does
    that better. It replaces a round-6 recommendation of mine that the
    user accepted (sweep-6 spec §9: with nothing open at startup or
    after a project switch, no row is selected). On 2026-09-27 the user
    accepted this rule in its place (OQ5). The cursor is drawn only
    while the sidebar is focused, so an unfocused sidebar at startup
    still shows no selection.
- **Change:** §4.2. `enter_space` (l.8273) stops clearing
  `sidebar.selected` (l.8295). `Sidebar::rebuild` (sidebar.rs l.156)
  keeps its identity rule, but falls back to the nearest request row
  at the old index instead of `None`. That fallback only serves
  outside refreshes: every in-app op lands explicitly.

### R7 Manage lists keep a cursor per tab

- **Who:** `earlier` (302dca2 and 458d663).
- **Rule:**
  - Environments and Spaces each keep their own cursor, scroll and pane
    focus across tab switches. A parked cursor clamps if its list
    shrank while it was away.
  - Deleting a row lands on the neighbour: the same index, clamped.
  - In the Variables list, deleting the open declaration opens the row
    now under the cursor. If that row is a header or spacer, it opens
    the nearest declaration above instead.
  - Creating a space does not switch into it. `CreateSpace` (l.6036)
    drops its trailing `SwitchSpace` (l.6050).
  - Creating a space or an environment puts that list's cursor on the
    new row. (Core still activates a new environment.)
- **Change:**
  - `components/manage.rs`: copy the `live`/`parked` pair and
    `switch_list` from 302dca2, so `manage.list` keeps meaning "the
    open tab's list" (≈30 non-test uses, no churn). Add
    `Manage::list_for_mut(tab)` so R5 can reselect in a parked list.
  - `manage_list.rs`: replace `reset` (l.169) with `clamp(len)`.
  - `varmanager.rs`: `sync` (l.1161) gets `nearest_stop` from 302dca2.
    A declaration named while its rows are stale becomes
    `detail = Var(name)` and is resolved on the next `sync`.

### R8 Table rows: undo reselects the restored row, never the ghost row

- **Who:** `earlier` (0e5fc8a, 766527d, and the text-caret ruling
  6772cd9).
- **Rule:** undoing a key/value row delete puts the row cursor on the
  row that came back (the first one, when several return). When the
  row under the cursor is gone, the cursor falls back to the last real
  row, never the ghost "+ Add" row at index `len`. Focus and cell text
  carets never move.
- **Change:** `components/editor.rs`:
  - `apply_snapshot` (l.416) takes the `keys_before` and reselect
    lines from 766527d;
  - the `restore_caret` `Caret::Cell` arm (l.498–502) clamps to
    `len.saturating_sub(1)`.

### R9 A scratch editor says it is unsaved

- **Who:** `earlier` (eb8bf59).
- **Rule:** while `editor.slug` is `None`, the URL well shows a muted
  ` unsaved ` marker on its right. Deleting the open request lands the
  editor on a scratch, and today that looks exactly like a saved
  request.
- **Change:** copy the `UNSAVED_MARKER` block from eb8bf59 into
  `Editor::draw_url_bar`.

## 4 The cursor model

### 4.1 State

```rust
// undo.rs
pub struct View {
    pub space: String,
    pub open: Open,
    pub cursor: Option<RowKey>,   // sidebar cursor (request slug or folder path)
    pub row: Option<ListRow>,     // Manage/Variables row the op targeted
}
pub enum Open {
    Request { slug: String, buffer: Option<Box<HttpRequest>> },
    Scratch { buffer: Option<Box<HttpRequest>> },
}
pub enum ListRow { Env(String), Space(String), Var(String) }
```

- `buffer` is `Some` only when `editor_holds_unsaved()` is true: dirty
  against disk, or a scratch with content.
- `RowKey` is today's private `sidebar::RowId`, made public.
- `StepKind::Project` becomes
  `{ id, label: StepLabel, before: View, after: View }`.
- `EditorDelta` and `Context` are unchanged.

### 4.2 The one landing path

```rust
pub struct Landing {
    pub space: Option<String>,      // enter this space
    pub open: Option<Open>,         // replace what the editor holds
    pub cursor: Option<CursorAim>,  // place the sidebar cursor
    pub row: Option<ListRow>,       // place a Manage/Variables cursor
}
pub enum CursorAim { OnOpen, On(RowKey), Neighbour(usize) }
fn land(&mut self, l: Landing) -> bool
```

`land` runs these steps, in order:

1. Cancel live drags. `enter_space` does this today; the step moves
   here.
2. Work out the target space: the space of `open`'s slug, else
   `l.space`, else the active one. If that differs from the active
   space, record the outgoing space's open request, call
   `set_active_space`, and toast `space: {name}`, at most once per
   landing. Nothing here touches the cursor.
3. `Open::Request`:
   - flush the field session, close the previously held request,
     `open_request`, and `editor.load`;
   - if `buffer` is `Some`, seat it (`apply_snapshot`, then set
     `shadow` to it; vim-mode `seat_editor_snapshot`);
   - if the open fails, toast, seat nothing, and continue with the
     editor unchanged (05a3b4d);
   - `land` itself never refuses. The unsaved-changes check belongs to
     the callers that land before anything is written: `ForceOpenRequest`,
     the space switches, and `jump_to_request_for_undo`, which keeps its
     guard (l.10915). They check first, show the dirty gate if the
     editor holds uncaptured unsaved content, and call `land` only once
     it is clear. A journaled arm calls `land` after its `Project` write,
     when a refusal would leave the files changed and the view not. Its
     own dirty gate (or the Discard that reached it) has already run, so
     it lands unconditionally. A `debug_assert!` in `land` pins that the
     editor holds nothing uncaptured on entry, apart from a `buffer`
     that the landing itself seats.

   `Open::Scratch`: set `Editor::default()`, then load the buffer as a
   scratch when it has one.
4. Call `refresh_sidebar` once, with `pending_expand` holding the
   ancestors of the cursor row, so rows inside collapsed folders
   appear.
5. Place the cursor. An explicit aim wins. `OnOpen` is the default
   whenever `open` changed. `On(key)` falls back to `OnOpen` when the
   row is not in the active space, and `OnOpen` falls back to the first
   request row, then to the first row. `Neighbour(i)` picks the request
   row at `i`, else the nearest below, else the nearest above
   (`select_nearest_request`). With no aim, identity is kept.
6. Place `row` in its own Manage list, or in the Variables list (R5,
   R7).
7. `retarget_sidebar_travel`, then `persist_open_request`.

These become thin callers of `land`:

- `ForceOpenRequest` (l.3248): `land(open: Request(slug))`.
- `ForceSwitchSpace` (l.5905) and `follow_active_space` (l.8309):
  `land(space, open: remembered | first visible | first in space |
  Scratch)`. "First in space" is `Sidebar::first_request_in_space`,
  copied from 624274f, which finds requests inside collapsed folders.
- `create_or_save_as` (l.9078) tail: `land(open: Request(new))`.
- `ForceSwitchProject` (l.4108) and the startup restore (l.910):
  `land(open: restored | Scratch, cursor: OnOpen)`.
- `jump_to_request_for_undo`: `land(open: Request(slug))`.

### 4.3 Recording a step

- `fn begin_op(&self) -> OpToken` captures `before = self.view()`.
- `fn record_project_step(&mut self, t: OpToken, label: StepLabel)`
  captures `after = self.view()` and pushes the marker. The
  journal-top logic of today's `record_project_step_as` (l.8028) is
  unchanged.
- The token makes "no before-view" a compile error (`mine`).
- Every journaled arm follows the same order: `begin_op` first, then
  the `Project` call, then `land(...)`, then `record_project_step`.
  `land` performs no journaled write (`refresh_sidebar`'s
  `set_expanded` and `persist_open_request` are unjournaled), so
  recording after the landing still pairs the marker with the op's
  own entry. `land` cannot refuse here (§4.2 step 3): the arm's dirty
  gate ran before `begin_op`.
- The dirty gate's Discard reaches the `Force*` arm while the editor
  still holds the edit, so `before.buffer` is the edit the op found.
  This is the OQ1 behaviour.

### 4.4 After a replay

The `StepKind::Project` arm of `apply_undo_step` (l.11001) does this,
replacing `after_undone` (l.8132):

1. Replay (`p.undo()` / `p.redo()`), then toast the warnings, rename
   session entries along `u.meta.moves`, and `sync_varmanager` on the
   Manage screen. Toast `env: …` if the active env moved. These steps
   are kept from today's code.
2. If the editor's slug is one of the moved slugs, retarget
   `editor.slug`, `editor.name` and `saved.name` without reloading, as
   today (l.8188–8215, minus its `enter_space`).
3. If the editor's slug no longer exists, reset the editor to a
   scratch. This is the case the `Open` part below then corrects.
4. Build the landing: `target = if redo { after } else { before }`.
   Each part where `before != after` is taken from `target`. `open`
   compares the slug and the presence of a buffer. If the editor now
   holds a request in a space other than the active one (step 2
   moved it), the landing gets that `space`.
5. `land(landing)`, then `sync_marked_entry`, then toast from the
   label.

### 4.5 Event table: sidebar and editor

"Neighbour" means the request row that now holds the old row's index,
else the nearest one below, else the nearest one above.

| Event | Open afterwards | Sidebar cursor afterwards |
|---|---|---|
| Open R (click, Enter, palette, cross-space) | R; switches space if needed | R (folders expanded) |
| Arrow / page / Home / End | unchanged | moved |
| Right-click a row, then dismiss the menu | unchanged | back where it was |
| Create N / save-as N | N | N |
| Undo create / save-as | `before.open` (+ its buffer; OQ1) | `before.cursor` |
| Redo create / save-as | N | N |
| Duplicate R | the copy (the dirty gate may intervene) | the copy |
| Undo duplicate | `before.open` (+ buffer) | `before.cursor` (R) |
| Delete R, R open | scratch (R9 marker) | neighbour of R |
| Delete R, R not open | unchanged | neighbour if the cursor was on R, else kept |
| Undo delete R | R reopened (+ buffer) if it was open, else unchanged | `before.cursor`: R when the delete came from the list (the cursor was on R); wherever it was otherwise (rule 1) |
| Redo delete R | as the forward delete | as the forward delete |
| Rename R to R2 (re-sort or not) | R2 if R was open | R2 if it was on R (bug b) |
| Undo / redo rename | follows R2 → R / R → R2 | R / R2 |
| Move R to space T (no follow) | scratch if R was open, else unchanged | neighbour of R |
| Undo move R to T | R back in its space; reopened if it was open | R, in R's space (bug a) |
| Move all S to T | the open request follows into T | the open row |
| Undo / redo move all | follows back / forth | the open row |
| Reorder R (alt+↑/↓, drag) | unchanged | R |
| Undo / redo reorder | unchanged | R |
| Switch space (ctrl+N, alt+c, chooser) | remembered, else first visible, else first in folders, else scratch | the open row, else the first row (a space with folders but no requests), else none (a space with no rows) |
| Create space | unchanged | unchanged |
| Delete the active space | fallback space's remembered or first request | the open row |
| Undo delete of the active space | `before.open` in the restored space | `before.cursor` |
| Delete an inactive space; its undo | unchanged | unchanged |
| Rename the active space | same request, re-prefixed | same row, re-prefixed |
| Switch project, startup | restored open request, else scratch | the open row, else the first row (R6) |
| Outside change (reload, poll) | unchanged | by identity, else neighbour |
| Undo / redo an edit (`EditorDelta`) of another request | jumps to that request | the open row |
| Undo / redo a variable / env / space edit | unchanged | unchanged |

### 4.6 Event table: Manage, Variables, key/value tables

| Event | Cursor afterwards |
|---|---|
| Switch Manage tab | that tab's own cursor, clamped |
| Create env / space | the new row in its list |
| Delete env / space | the same index, clamped (the neighbour) |
| Delete a variable | the row now under the cursor, else the nearest declaration above; the detail pane follows |
| Undo delete (any of the three) | the restored row, even in a parked list |
| Rename env / space; its undo | the renamed row; the old name |
| Reorder env / space; its undo | the moved row |
| Table row delete | the row that slid up, or the last real row |
| Undo table row delete | the restored row (R8) |
| Undo that removes the row under the table cursor | the last real row, never the ghost row |

### 4.7 Telling "open" from "cursor here"

- **Who:** `mine`, accepted by the user 2026-09-27 (OQ3).
- **Today:** the open row gets a `selection` fill and a `▌` bar. A
  cursor on another row gets a steady `control_hover` fill, drawn only
  while the sidebar is focused. When the cursor sits on the open row,
  the band wins and nothing marks the cursor (sidebar.rs l.904–948).
  Before R6, a cleared cursor looked identical to a cursor on the open
  row, which is how sweep 7 moved the wrong request.
- **Proposal:** R6 makes the cursor always present, so no new state
  needs showing. Add one cue: while the sidebar is focused and the
  cursor is on the open row, that row's fill is
  `mix(selection, control_hover, 0.5)` instead of plain `selection`.
  With the sidebar unfocused, it paints as today.
- **Tests:**
  - update `cursor_on_the_open_row_shows_plain_selection_not_accent_name`;
  - add `the_open_row_lifts_while_the_focused_cursor_is_on_it`.

## 5 What is copied from vim-mode, and what is rewritten

**Copied**, reviewed but close to verbatim:

- 414f2af: the core `run_transaction` materialise block and all seven
  of its tests.
- d3f5535: `VarEditOp::subject` and `VarStructOp::subject`.
- a87378e: the no-confirm arm bodies and toast wording for the env and
  space deletes.
- 624274f: `Sidebar::first_request_in_space`.
- 302dca2:
  - `Manage::{live, parked, switch_list}`;
  - `ManageList::clamp`;
  - `VarManager::nearest_stop` and the `sync` neighbour block.
- 458d663: dropping `SwitchSpace` from `CreateSpace`, and the Manage
  cursor on the created row.
- 0e5fc8a and 766527d: the two `editor.rs` hunks.
- eb8bf59: the ` unsaved ` marker.

**Rewritten.** These are the redesign that groups A and B call for
(`you`):

- `after_undone`, including `delete_reopen`, `closed_open` and the
  scratch branch. It becomes §4.4.
- `pending_reopen_buffer` and `dirty_reopen_buffer`: replaced by
  `View.open.buffer`, captured through `OpToken`.
- `Reselect` and `reselect_restored_row`: replaced by `View.cursor` and
  `View.row`.
- `ProjectNoun` and its `Trash*`, `is_delete` and `trash_subject`:
  replaced by `StepLabel`.
- `record_project_step_for` / `_as` / `record_var_step` /
  `record_env_step` / `record_space_step`: replaced by one
  `record_project_step(token, label)`.
- The cursor repairs in the `DeleteRequest` arm (eb8bf59), the
  `Sidebar::rebuild` index clamp (dd3e138) and the project-switch clear
  (2137262): replaced by `land`, plus the rebuild fallback in R6.

**Not ported:**

- ada2e90 (`delete_requests` batch) belongs with counted deletes in
  piece 4.
- `YankSelectedRequest` / `PutRequest` and their tests (piece 4). The
  put tests that exercise the reopen are re-expressed through
  `DuplicateRequest`.

## 6 Test plan

These are behavioural App-level tests in `crates/postui/src/app/tests.rs`
unless a module is named. They use the existing helpers `spaced_app`,
`scratch_app`, `app_with_envs`, `var_project` and `ordered_app`. Tests
marked (p) or listed as "Ported" take their name and body from vim-mode,
with vim keys replaced by actions. The rest are new.

**6.1 Core `state.toml` (R1).**
- In `project/spaces.rs`:
  `redo_survives_a_delete_space_whose_entry_first_created_state_toml` (p),
  `undo_of_a_first_state_write_restores_the_pre_op_active_space` (p).
- App-level: `redo_after_{delete_space, rename_space, delete_inactive_env,
  rename_inactive_env}_survives_a_project_with_no_local_state_yet` (p, four
  tests), `undo_of_delete_active_space_restores_the_active_space` (p).

**6.2 Toasts (R2).**
- Ported: `request_delete_undo_and_redo_toasts_use_the_display_name`,
  `environment_delete_undo_and_redo_toasts_name_the_environment`,
  `space_delete_undo_and_redo_toasts_name_the_space`,
  `undoing_an_environment_rename_names_the_environment`,
  `undoing_a_space_rename_names_the_space`,
  `undoing_the_migration_names_no_subject`.
- New: `variable_delete_undo_and_redo_toasts_quote_the_variable`,
  `option_delete_undo_toast_names_the_option_and_env`,
  `undoing_a_rename_says_rename_and_never_names_the_open_request`,
  `undoing_a_duplicate_names_the_copy_by_its_display_name`,
  `undoing_a_create_names_the_created_request`.

**6.3 Deletes never confirm (R3, R5).**
- Ported: `delete_space_says_the_count_then_trashes_and_undoes`,
  `delete_space_refuses_the_last_space_and_says_nothing_of_requests_for_an_empty_one`,
  `deleting_a_variable_undo_reselects_it`,
  `deleting_a_request_undo_reselects_it`.
- New: `delete_env_never_confirms_and_its_toast_says_values_and_secrets_went`,
  `deleting_a_space_holding_the_unsaved_open_request_does_not_gate_and_undo_restores_the_edit`,
  `deleting_an_environment_from_the_manage_list_undo_reselects_it`,
  `deleting_a_space_from_the_manage_list_undo_reselects_it`,
  `undoing_an_env_delete_off_screen_reselects_it_when_the_tab_opens`.

**6.4 The view comes back (R4).**
- Ported: `undoing_a_{create, duplicate}_reopens_the_request_open_before_it`,
  `undoing_a_save_as_reopens_the_original`,
  `undoing_a_save_as_from_a_scratch_brings_the_scratch_back`,
  `undoing_two_creates_walks_back_through_both`,
  `undoing_a_delete_of_the_dirty_open_request_restores_its_unsaved_edit`,
  `undoing_through_an_unrelated_step_still_finds_every_step`,
  `undoing_a_create_after_a_discarded_edit_reopens_clean`,
  `a_reopen_that_fails_seats_nothing`,
  `undoing_a_create_through_a_scratchs_discard_brings_the_scratch_back`
  (OQ1).
- New: `undoing_a_create_after_renaming_the_open_request_reopens_the_new_name`,
  `undoing_a_create_through_the_dirty_gates_discard_brings_the_edit_back`
  (OQ1), `undoing_a_create_after_moving_on_reopens_the_request_it_left`
  (OQ2),
  `undoing_a_delete_of_a_closed_request_after_moving_on_does_not_reopen_anything`,
  `undoing_a_variable_edit_never_touches_the_editor`,
  `redo_of_a_create_lands_on_the_created_request_from_anywhere`.
- Kept as they are: `undo_of_a_request_delete_reopens_it_in_the_editor`,
  `undo_of_a_request_delete_leaves_another_open_request_alone`.

**6.5 Sidebar cursor (R6, §4.5).**
- Ported: `deleting_the_open_request_skips_a_folder_header_neighbour`,
  `deleting_the_open_request_selects_the_neighbour_row_without_opening_it`,
  `switching_into_a_space_whose_request_is_foldered_opens_it`,
  `switching_into_a_space_opens_a_request_two_folders_deep`,
  `a_space_switch_prefers_a_top_level_request`,
  `a_space_switch_still_prefers_the_remembered_request`,
  `a_switch_into_an_empty_space_opens_nothing_and_selects_nothing`.
- New:
  - `opening_a_request_puts_the_cursor_on_it_even_in_a_collapsed_folder`;
  - `deleting_the_cursor_row_lands_on_the_neighbour`,
    `deleting_the_last_row_lands_on_the_row_above`;
  - bug (b): `renaming_a_request_that_resorts_keeps_the_cursor_on_it`,
    `undoing_a_resorting_rename_puts_the_cursor_back_on_the_old_name`;
  - bug (a):
    `undoing_a_cross_space_move_lands_the_cursor_on_the_moved_back_request`.
    It replays the sweep-7 sequence (move, space switch, undo), then
    checks that `PromptMoveSelectedRequestToSpace` targets the
    moved-back request;
  - `moving_a_request_to_another_space_lands_on_the_neighbour`,
    `reordering_keeps_the_cursor_on_the_moved_row_and_so_does_its_undo`;
  - `switching_projects_lands_the_cursor_on_the_restored_request`
    (replaces 2137262's test),
    `switching_to_a_project_with_nothing_open_puts_the_cursor_on_the_first_request`;
  - OQ5: `startup_without_persisted_open_request_selects_nothing` and
    `switching_projects_clears_the_sidebar_cursor` are rewritten as
    `startup_with_nothing_open_puts_the_cursor_on_the_first_row` plus
    `an_unfocused_sidebar_draws_no_cursor_fill_at_startup`;
  - `an_outside_delete_of_the_cursor_row_lands_on_the_neighbour`,
    `enter_space_never_clears_the_cursor`;
  - `undoing_a_delete_made_from_the_editor_puts_the_sidebar_cursor_back_where_it_was`,
    `switching_into_a_space_with_only_folders_puts_the_cursor_on_the_first_row`;
  - `the_sidebar_cursor_is_never_none_while_rows_exist`, which walks
    every row of §4.5 in turn on one fixture and asserts the invariant
    after each.

**6.6 Manage, Variables, tables (R7–R9).**
- Ported:
  - `each_manage_list_tab_keeps_its_own_cursor`,
    `a_parked_manage_cursor_clamps_to_a_shrunken_list`,
    `creating_a_space_does_not_switch_to_it`,
    `new_space_prompt_creates_without_switching`,
    `creating_a_{space, environment}_puts_the_manage_cursor_on_the_new_row`,
    `undoing_a_table_row_delete_selects_the_restored_row`;
  - in `varmanager.rs`:
    `deleting_the_open_variable_opens_the_neighbour_row`,
    `deleting_the_last_variable_of_a_section_lands_on_the_one_above`,
    `sync_clamps_the_cursor_and_reselects_after_a_deleted_detail`;
  - in `editor.rs`: `a_restored_cell_cursor_never_parks_on_the_ghost_row`,
    `a_restored_cell_cursor_on_an_emptied_table_sits_at_zero`,
    `a_scratch_editor_shows_the_unsaved_marker_and_a_saved_one_does_not`.
- New: `deleting_an_environment_lands_the_manage_cursor_on_the_neighbour`.

Existing `main` tests whose assertions change: the three
`delete_*_confirms_*` tests, `new_space_prompt_creates_and_switches`, and
every test that reads a `Restored x.toml` toast.

## 7 Open questions (decided 2026-09-27)

The user accepted every recommendation below. Each one is still labelled
`mine`: a decision on that question, not a standing rule. The options
and considerations are kept as the record.

**OQ1. Does undo bring back unsaved edits?** This covers undo of a
create, duplicate or save-as, and of a delete, run while the open
request had unsaved edits, including a create reached through the dirty
gate's Discard.

- Options:
  - (A) Yes: `before.buffer` is always seated (vim-mode f19a2ef).
  - (B) Yes, except after Discard. Discard records its own "discard"
    step, so the first `u` reopens the request clean and a second `u`
    brings the edits back.
  - (C) No: reopen the disk copy.
- Considerations:
  - (C) breaks rule 1, and it also scrambles history. The next `u`
    applies the last edit's `before` to a clean buffer, so undo makes
    the buffer *more* edited.
  - (B) matches what the user did step for step, but it adds a step
    kind, and a scratch's discard cannot be replayed once the scratch
    is gone.
- **Decided: (A)** (`mine`, accepted). Treat "Discard & create" as one
  op whose pre-op state still held the edits. For the delete of the
  open request and a save-as from a scratch, nothing was discarded, so
  (A) is simply rule 1.

**OQ2. What if the user has moved on?** An op opened or closed a
request (create, duplicate, save-as, delete of the open request, move
of the open request), and the user has since opened something else.
Should its undo still switch the editor back?

- Options:
  - Yes: this spec's §4.4 as written.
  - No: vim-mode's `earlier` "never yank them off it" (7d98380).
- Considerations:
  - An edit's undo already jumps back to its request
    (`jump_to_request_for_undo`), and `main`'s delete undo already
    reopens.
  - A project step can only be on top of the undo stack when the
    editor holds nothing uncaptured, so the jump loses nothing.
- **Decided: yes** (`mine`, accepted). Undo takes you to where the undone
  thing happened, and ops that did not touch the editor (R4) never
  move it.

**OQ3. Should the open row look different when the focused cursor is
on it (§4.7)?**

- **Decided: yes, the half-step fill lift** (`mine`, accepted). It is
  cheap, and sweep 7 listed the identical fill as a finding. Rejecting
  it leaves R6's invariant as the only fix, which already removes the
  harmful case (a cursor that is not there).

**OQ4. Should deleting a space still stop for unsaved changes?** Today
deleting the space that holds the open, unsaved request raises the
unsaved-changes dialog. R3 removes it because the step now carries the
unsaved buffer and undo brings it back.

- **Decided: remove it** (`mine`, accepted), for the same reason
  deletes never confirm (rule 2): undo restores everything.

**OQ5. With nothing open (startup, project switch), is a row
selected?** In round 6 the user accepted "no row wears the selected
fill when nothing is open" (tests
`startup_without_persisted_open_request_selects_nothing`,
`switching_projects_clears_the_sidebar_cursor`). R6 instead puts the
cursor on the first row, so `j`/`k`, `Enter` and `dd` always have a
target and a cleared-but-invisible cursor can't happen.

- **Decided: R6's rule** (`mine`, accepted), with the cursor drawn
  only while the sidebar is focused, so an unfocused sidebar still
  shows no selection at startup. The two round-6 tests are rewritten
  to pin this (§6).

## 8 Out of scope

- Everything vim-only: `dd`/`yy`/`p`, counts, `.`, ex verbs, and the
  prompt model (piece 4).
- Counted deletes and `delete_requests` batches (ada2e90, piece 4).
- Copy and paste of requests (`YankSelectedRequest`, `PutRequest`;
  piece 4).
- List `/` search (parked by the user).
- Undo of Settings (`Config` / `ConfigFile` steps): unchanged.
- Dirty gates on move and move-all: unchanged.
- The sweep-7 cosmetics not covered by R2/R9: the create toast lacking
  the space, and the two toasts of a table `2dd`.
- `History` coalescing, caps, and field-local undo (`undo_in_open_field`):
  unchanged.
