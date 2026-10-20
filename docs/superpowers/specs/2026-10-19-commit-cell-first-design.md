# A table cell commits before anything reads or replaces the editor

Tasks #63, #67 and #65. Status: approved 2026-10-19 (walked through
section by section; the choices in §5 are recorded there).

## 1. Problem

A table cell holds its typing in its own `LineInput` until it commits.
The map, and so `Editor::current_request`, `is_dirty` and every undo
view, sees none of it until then. Today the cell commits on a click away,
a tab switch, Enter, Save, Send and Copy as curl. It stays open, typed
but uncommitted, when:

- **the caret leaves it by keyboard**: `alt+u` to the URL line, a pane
  jump, the jq bar, the search box, the Manage screen (#67);
- **a dirty gate runs with the caret still in it**: `ctrl+c`/`ctrl+q`,
  vim `:q`/`:wq`, or a palette row (open, new request, switch space or
  project, curl import…). `App::editor_holds_unsaved` reads committed
  fields only, so the gate doesn't ask, and the typing is lost: quit
  exits with it, and an open drops it in `Editor::load` (#63, verified
  with a throwaway test: `click_hit(TableCell)`, type, `Action::Quit` →
  `should_quit`, no modal);
- **a journaled op runs over it** from the palette, e.g. deleting the
  open request: the op's before-view is built without the typing, so
  undoing the op can't bring it back (#65).

### What went wrong last time (#62, scrapped)

#62's rework built this design (d589fdc). Its first review flagged that
Cancel on the unsaved-changes prompt found the cell already closed. The
fix chosen then (`mine (approved)`) made the gate count the cell's
*uncommitted* draft. That created a third state ("typed, not
committed") that every reader of "unsaved" or of the editor had to
learn: the gates, Discard, curl import's discard, the op before-views,
landing, undo jumps, the sidebar dot and vim. Each of eight review
rounds found one more reader.

This design goes back to commit-first, which needs no reader to know
about drafts. It accepts the Cancel behaviour that round objected to
(rule R4).

## 2. Rules

| # | Behaviour | Source |
|---|---|---|
| R1 | When the caret leaves an open cell, the cell commits and its undo step is recorded at once: a click away (already), `alt+u`, a pane jump, the jq bar, the search box, the Manage screen. | you (#62): "when the cursor leaves the cell, the input should close and the undo immediately recorded" |
| R2 | A modal over the cell is not leaving it (palette, var picker, extract, paste, theme picker). A shortcut that keeps the caret in the cell (`alt+i`, the TLS lock) leaves it open. | mine (approved) (#62 rework) |
| R3 | Every dirty gate commits an open cell first, as its own step, then asks. Typing in a cell therefore counts as unsaved at quit, open, new request, duplicate/put of a request, move of the open request, space or project switch, curl import and replace, and vim `:q`/`:wq`. Not a value typed on the "+ Add" row with no key: its commit only stashes it until a key arrives, and a save could not keep it, so it is lost as on main (task #68, found in review round 2). | mine (approved) |
| R4 | Cancel on that prompt leaves the cell closed, with its typing committed and undoable (one ctrl+z takes it back). This is what a click on another request's sidebar row already does today. | mine (approved); replaces #62's "prompt sees the cell" |
| R5 | Every path that leaves or replaces the open request commits an open cell first, as its own step: opening another request (the landing), Discard, Reload, curl import's scratch discard, curl replace, and an op that removes the open request without asking (delete request, delete space). Undoing the delete or the discard therefore brings the typing back. Other journaled ops leave the cell open, as on main: the ones run over a cell act on it (the var picker's new variable, a selector pick, extract). *Amended after review round 4:* the first R5 committed in every op (`begin_op`), which closed the cell under those dialogs. | mine (approved, narrowed after round 4 on the user's "just fix it"); fixes #65 |
| R6 | Extract (to variable or selector, whole cell or a selection in it) with a cell open keeps the cell open through its op, as on main: the variable or selector is the op's step, and the typing commits with the token as one step on top. *Amended after review round 1:* the first version committed the typed text before the op ("three steps"), which commits it as if Enter were pressed, so a key typed on the "+ Add" row that names an existing row, or a key renamed onto another, merged rows before the token arrived. | mine (approved), replacing "go with three steps" |
| R7 | A cell commit is its own undo step, bracketed by a capture before and after (#66, unchanged). A method change that moves the tab is two steps: the method, then the cell. | you (#66: "id rather have the logic in one place"), mine (approved) |
| R8 | `Editor::load` still drops a cell left open, as a backstop. The row selection stays, and so does the add-row stash (a value typed on the "+ Add" row with no key), as on main. *Amended after review round 3:* clearing the stash on load (the first R8) also lost it on Save As, rename and move-all, which load the same content; the stash's leak into another request moved to task #68 with its other problems. | mine (the user had approved clearing it; this reversal is proposed, not yet confirmed) |

## 3. Design

### Readers (the #67 checklist)

Every path that reads the editor's content or replaces it, and whether
the cell can still be open when it runs:

| Reader | Can a cell be open? | Covered by |
|---|---|---|
| 14 dirty gates (`editor_holds_unsaved`): Quit, GatedQuit, OpenRequest, Duplicate, put request, MoveRequestToSpace, CreateRequest, SwitchProject, create project, SwitchSpace, MoveAllRequests, curl import, curl replace into another row, vim WriteQuit | yes, through a key or a palette row from the cell | R3: `leaving_unsaved` |
| `view_on` (op before/after views) | yes, a palette op | R5: a delete of the open request or its space calls `leave_open_request` first; other ops never reload the open request |
| `land` → `open_in_editor` / scratch, `Editor::load` | only if no gate or op ran first | R5: `flush_field_session` (already called there); R8 backstop |
| DiscardChanges, ReloadOpenRequest, curl import's scratch discard | yes | R5: `flush_field_session` (already called there) |
| curl replace (in place) | yes | already commits; collapses into `flush_field_session` |
| Undo/redo (`apply_snapshot`, `jump_to_request_for_undo`) | only untouched: ctrl+z goes to an edited cell's own history | no change; `land` commits a no-op cell |
| Save, Send, Copy as curl, tab switch, click away | yes | already commit (`commit_table_edit`) |
| Extract | yes, it acts on the cell | R6 |
| Sidebar dirty dot, header Save/Discard group | yes (display) | no change (the group already shows while a cell is open) |
| Field gate (`capture_table_commit`) | no longer: R1 closes the cell before another field takes keys | removed |

### Changes

- **`App::capture_undo`** splits in two. A new `capture_step` holds
  today's gate check and shadow diff. `capture_undo` first closes the
  fields the caret has left: the URL line, as today, and now an open cell
  when the caret is not in the editor's content on the main screen
  (`commit_table_edit`). Then it calls `capture_step`. A pane jump
  (`Action::FocusPane` away from the editor) commits at the jump itself
  (review round 2): the jq tree's tee-up jumps to the bar and fills it in
  one event, and a commit left for the event's end would wait behind the
  bar's session and join its step.
- **`commit_table_edit`** brackets the commit with `capture_step`
  (before and after), not `capture_undo`, so the two can't recurse.
- **`capture_table_commit` is removed**, along with the 2026-09-15 and
  2026-08-24 amendments that describe it. Under R1 a cell is never open
  beside a field session, so its gated path is unreachable.
- **`flush_field_session`** commits an open cell first, then flushes the
  URL/jq/vim session (`flush_text_session`, the old body). Landings,
  Discard, Reload and curl already call it. Curl replace's three lines
  (commit, capture, flush) become one. `begin_op` flushes only the text
  session, as on main, so ops run over a cell (the var picker, a
  selector pick, extract) still act on it.
- **`App::leave_open_request`** commits an open cell before the open
  request is left. `leaving_unsaved` (the 14 gates) calls it and then
  `editor_holds_unsaved`, which stays read-only for `view_on` and the
  undo jump guard; delete request(s) and delete space call it before
  their op when they remove the open request. Quit checks
  `modals.is_empty()` first, so `ctrl+c` over a modal still exits at once.
- **Extract** (R6) is unchanged from main.
- **`commit_table_edit`** asserts (debug builds) that no field session
  holds the gate when a cell commits: the invariant R1 rests on. Added
  after review round 3 (every round raised it); `mine`, reversing the
  approved "no new debug assertions".

### Contract before → after

- Before: an open cell was a draft that nothing outside the table could
  see, with the click-away paths, tab switch, Save and Send each
  committing it.
- After: a cell is open only while the caret is in it (or a modal is over
  it). Anything that asks about or replaces the editor commits it first,
  through `commit_table_edit`, the one path. Nothing reads drafts.

## 4. Tests

- R1: for each of `alt+u`, a pane jump, the jq bar, search and Manage:
  type in a cell, leave, and the map holds the typing with one undo step
  that takes it back. `alt+u` then URL typing gives two steps (URL, then
  cell).
- R2: `alt+i` with a cell open leaves the cell open; so does opening the
  palette.
- R3: typed cell + `Action::Quit` → the Unsaved changes prompt, and
  Save & quit writes the cell. The same for OpenRequest from the palette,
  and vim `:q`. An untouched open cell doesn't ask.
- R4: Cancel → cell closed, map holds the typing, one ctrl+z restores.
- R5: palette delete of the open request over a typed cell, then undo:
  the request is back with the typing (#65's repro). Discard over a typed
  cell, then undo: the typing is back.
- R6: extract from an open cell to the project: two undo steps (token
  with typing, then the variable); a key typed on the "+ Add" row that
  names an existing row leaves that row alone; a selection in the cell
  keeps the text around it; "This request" stays one editor step.
- R8: no test of its own after the round-3 amendment (load keeps the
  stash, as on main; #68).
- Tests that pin the removed record-beneath step get the new rule (the
  cell commits when `alt+u` leaves it), not deletion.

## 5. Choices (resolved 2026-10-19)

All four were proposed by me. 1, 3 and 4 were approved by picking the
recommended option; 2 was approved in chat ("go with three steps") after
the user asked why Extract commits the cell first. So all are
`mine (approved)`. Review round 1 then showed the three-step shape
merging rows from a key cell; the user picked E1 instead (`mine
(approved)`), and R6 reads so.

1. **R4, Cancel closes the cell.** The alternative, commit and reopen the
   same cell, keeps the caret in it. It costs a reopen that has to
   re-resolve the row after the commit (a key rename can collapse rows,
   and the add row may have become a real one), and the cell's keystroke
   undo is gone either way. Recommended: close.
2. **R6, Extract.** The alternative, E1, lets Extract keep the cell open
   through its op, as today. That is one named exception in `begin_op`,
   and it keeps the typing and token as one step. Its before-view
   misses the typing, which is harmless because undoing an extract never
   reloads the editor. Recommended: R6 as written (no exception, and it
   matches the URL line).
3. **Scope:** fold #65 into this branch. It's the same rule (R5), and
   `flush_field_session` fixes it at no extra cost. #64 (a click closing
   the URL line merges its typing) stays separate: it is about the URL
   line, not a cell.
4. **R8:** clearing the add-row stash on load is a pre-existing leak of
   the same kind. One line.
5. Unchanged on purpose: the sidebar dirty dot doesn't show an open
   cell's typing until it commits (as on main), and there are no new
   debug assertions.
