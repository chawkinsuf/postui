# An edit made outside is its own undo step — Design

Date: 2026-11-11
Status: approved by the user 2026-11-11 (task #283, comment 1296)
Task: #283 "Undo writes a file's old text over a change made outside since the op"

This is the second rework on #283. It is built on the branch
`undo-outside-change` on top of `e9383aa`, after `main` was merged in
(`9770512`, which brought #239).

## The question

After three review rounds found the same kind of defect in the
refuse-on-mismatch design, the user wrote on the ticket (comment 1284):

> Should we scrap all this and just treat loading external edits as its own undo step?

Short answer: yes to the idea, in a nearby form. An edit made outside
becomes a step in the undo order instead of a reason to refuse. But the
step is made when an undo or redo reaches the changed file, not when the
app loads it, and the text comparison stays, because it is how the app
finds the edit.

## Findings

Traced in the code at `e9383aa`. The probes were throwaway code and tests
in the worktree, all reverted.

1. **Loading is not one moment, and undo does not load first.**
   `Project::poll` runs only from `Action::ReloadProjectFiles`: focus
   gained, Send, the choosers and pickers, curl replace and copy,
   `resync_project`, the space ops. There is no timer. `App::step_history`
   (undo and redo) does not reload. The ticket's own steps (rename, append
   a line outside, undo) contain no load at all. Every op also takes in
   outside text by itself: `edit_project_toml`, `edit_variables` and
   `edit_env` read the file as it is on disk and write from that.
2. **A reload notices only some files.** `Project::watched` is
   `project.toml`, `variables.toml`, the `environments/` listing and the
   active environment's file. Another environment's file, a folder's
   `.folder.toml` and `secrets.toml` are rewritten by undo steps, but a
   change to one of them alone starts no reload.
3. **At a load the app holds no old text.** `reload_documents` reads each
   file, parses it into `ProjectMeta` / `VarModel` / `EnvData` and drops
   the text. The reload arm calls `invalidate_stamps()` before `poll()`, so
   the stamps cannot say which file changed either. The only old text in
   the app is the `after` of the journal entries that wrote the file.
4. **So a step made at load time cannot replace the check at undo time.**
   It would need a kept copy of every such file's text; undo, redo and
   every journaled op reloading first; and the watch set widened to every
   file a step can rewrite. Any hole (a file not watched, a change the
   stamp misses, an op that read the file before the reload) is the
   ticket's silent overwrite again, because nothing would check at the
   write.
5. **Made at undo time, it needs none of that.** Probe: in `preflight`, a
   text op whose file does not hold the op's `after` no longer refuses. It
   yields a text op `{file, before: the step's after, after: the file's
   text now}`. `replay` puts the popped entry back, wraps those ops in a new
   entry and replays that instead. About 70 lines of code, core and app.
   Results:
   - The ticket's steps, with or without a reload before the undo: the
     first undo puts `project.toml` back to the text the rename left and
     says so; the second undoes the rename; two redos bring the rename and
     then the outside line back. The two stacks stay aligned through undo,
     undo, redo, redo, undo, undo and a new op.
   - Comment 1283, first case (a folder renamed three times, undo, redo
     refused by #286, three undos): all three undos work.
   - Comment 1283, second case (the file put back outside to an older
     step's text): the first undo takes the outside text back, the next two
     undo the two steps. Nothing is refused.
   - Delete a space, change `project.toml` outside, undo twice: the space
     is back. On this branch that undo is refused and dropped and the space
     stays in `.postui/trash/`, which the next open empties
     (`open_with` calls `empty_trash`).
   - With the `Journal::follow` call in `fs_write_text` switched off, an
     undo after the sidebar's order completion never gets past it: each
     press takes the completion back and the refresh writes it again. That
     call stays.
   - No test from main fails with the probe in. The 15 that fail are tests
     this branch added to pin a refusal.
6. **One more path lost an outside edit: a keyboard burst. Closed by
   #239, not by this task.** At `e9383aa`, `Entry::try_merge` folded a
   keyboard burst (2 s) into one entry without checking that the new op
   started from the text the burst left, so an outside write between two
   key presses of a burst was merged into the entry and undone with it
   (probe on `e9383aa`: `move_environment`, outside append,
   `move_environment`, undo: the file is the text from before the burst).
   #239 added that condition to `try_merge` on `main`, with a unit test
   (`a_burst_step_that_starts_from_another_text_is_its_own_entry`) and an
   app test (`a_move_after_an_outside_change_does_not_join_the_burst_before_it`).
   It reached this branch with the merge of `main`.

## Where the information is lost (what the designs had in common)

The first design and rework 1 both compare the file with the copy of "the
text this step left" and treat a difference as proof of an outside change.
Rework 1 moved where the compare runs. It did not change what a difference
means. The app cannot know who wrote the text it finds: it keeps no record
of its own writes that survives a reload. So every way the app's own text
reached a file without that copy being updated was refused as "changed
outside", and each fix taught the copies about one more writer.

This design does not recover that information. It stops needing it:
whoever wrote the difference, it is kept as a step and nothing is refused.
A copy that is out of date costs one extra press with a label that is not
quite true, instead of a refusal that strands the steps beneath.

Two things about who writes what remain (R9, R10). R9's list has been on
the branch since its first commit, and comment 1283 records that R10's
call has drawn no verified finding. Under this design a mistake in R9
costs an extra press. R10 is needed for a different reason (finding 5:
without it an undo cannot get past an order completion).

## Rules

| # | Rule | From |
|---|---|---|
| R1 | An edit made outside the app is a step in the undo order, not a reason to refuse an undo or redo. | you, as a question (comment 1284): "Should we scrap all this and just treat loading external edits as its own undo step?" |
| R2 | The step is made when an undo or redo is about to rewrite a file that does not hold the text its step left there, not when the app loads the file. A reload does not touch the history, as on main. Undo and redo do not reload first. | mine (approved): question 1, "1: A" (comment 1296) |
| R3 | The step holds, for each such file, the text the replayed step left and the text the file holds now. Only the files that differ are in it. | mine |
| R4 | The press that finds it takes the outside text back: those files get the text the step left, the outside step goes on the other stack, and the step that was about to be replayed stays where it was. The next press replays it. | mine (approved): question 2, "2: A" (comment 1296) |
| R5 | Redo does the same in its direction: the outside step goes on the undo stack, and undo brings the outside text back. | mine |
| R6 | The toast is a warning that names the files and says whether undo or redo brings the text back: "Took back changes made outside the app to project.toml. Redo brings them back." (found by a redo: "Undo brings them back."). Replayed later as an ordinary step it says "Put back changes made outside the app to project.toml" or "Took back changes made outside the app to project.toml", as an info toast. Several files read "a.toml and b.toml" or "a, b and c". | mine |
| R7 | Nothing is refused over text. The path refusals stay as on main (a file gone, a path already taken, a made directory that holds something added since, the trash), and a refused step is dropped as on main. | mine |
| R8 | The step beneath a dropped one meets the dropped step's text as a difference and takes it back the same way, then undoes. The toast then says "outside the app" although the text was the app's. | mine |
| R9 | `.postui/state.toml` and request files are not compared, as on this branch: the app writes both outside the history. #285 stays its own task. | mine |
| R10 | The sidebar's order completion stays followed by the journal (`Journal::follow` from `fs_write_text`). | mine |
| R12 | A file that does not parse is taken back like any other: the app writes the step's text over it and keeps its text in the outside step. | mine |
| R13 | A file the check cannot read (unreadable, not UTF-8) is not compared. The undo or redo then fails where it reads that file to write it, as on main: nothing is written to the file, what the replay had already done is put back, and the step stays on its stack for a retry. *Changed in review round 1 (2026-11-11):* the rule was "refuses the step and drops it, as on this branch", which also dropped the step that restores a deleted request or space. | mine |

The reason the user gave with the answers to questions 1 and 2 (you,
comment 1296):

> Again, this is talking about outside edits, which should be a rare case. This at least lets the user know what happened, without messing up what they were doing. And with these files often in git anyway, likely they could recover the edits later if they wanted to.

There is no R11. It was "a keyboard burst merges only when the new op
started from the text the burst left", listed here as something this task
would add. It came with #239 (finding 6) and is not built again.

## Standing rules it touches

- "if the state after an undo is different than the state before the
  operation being undone, that is a bug that needs to be fixed" (user,
  2026-09-04). Holds per step: after the first press the files hold the
  text the step left, after the second the text from before the op, byte
  for byte.
- Deletes never confirm; undo is the safety net. Kept: a delete still
  undoes after an outside change, in two presses. Any design that refuses
  breaks it for that case (finding 5).
- "we should not use defaults if there is a parse error, only if there is
  no file at all" (user, 2026-09-05). No default is written. R12 does write
  over a broken file, with its text kept in the step. Main's undo writes
  over it too and keeps nothing.
- A note in the project memory records a ruling of 2026-09-06, during the
  file-access refactor, that an undo writes its text over edits made
  outside since the op, with no content check. It is a note, not a quote.
  The ticket and the ranking of 2026-11-09 ("data loss would be the most
  important") reverse it, and this design follows the newer one. Comment
  1289 put that to the user ("If the older ruling still stands, say so
  and the task is scrapped"). The answer (comment 1296) picks 1: A and
  2: A and does not say so.

## Design

**Core, `project/undo.rs`.** `preflight` returns the differences it found
instead of an error for them: `Result<Vec<Op>, Error>`. `replay`, when the
list is not empty, pushes the popped entry back on the stack it came from,
builds an entry from the list with a new id and no merge key, and replays
that entry in the same direction through the same code. It is not
pre-flighted again: it is made of the files as the pre-flight just read
them. If one of its writes fails, what it wrote is put back and it is
discarded, since the popped entry is already back on its stack. The loop
that calls `Journal::follow` for a dropped entry's ops is removed.

*As built (2026-11-11):* the probe replayed the new entry by calling
`replay` again, pre-flight included. The build branches inside the one
`replay` instead, so there is no second pre-flight and no recursion.

**Core, `journal.rs`.** A way to take a new id. `Journal::follow` and its
call in `fs_write_text` stay (R10). `try_merge` is not touched: its
condition that the new op starts from the text the burst left came with
#239.

**Contract, core to app.** Before: `Project::undo` / `redo` return the
`Undone` of the entry the app's marker named, or an error. After: they may
return the `Undone` of a step the app has no marker for. `Undone` gains
`outside: Vec<String>`, the files taken back. It is not empty only for the
press that found them; a later undo or redo of the outside step returns
it empty, like any step.

**App, `apply_undo_step`.** When `outside` is not empty: refresh as after
any replay (`after_replay` with the present view on both sides), toast R6,
put the popped marker back on its own stack, and put a marker for the
outside step on the other one. A new `Verb` (`Outside`) words its toasts.
The press reports a step taken, so a counted undo (vim's `3u`) counts it
as one of its steps and goes on.

**Removed from this branch:** the "has been edited since" refusal in both
directions; the second `follow` call (in `replay`); the expectations of
the 15 tests that pin a refusal. **Kept:** the compare in the pre-flight
(now the detector), `written_outside_journal`, `Journal::follow` with its
first call, and the tests for the order completion, a save after a rename
and a UI change after a space rename. Main's path checks are untouched.

**Specs.** The branch's amendments to `2026-08-24-undo-redo-design.md`
(Failure handling) and `2026-09-05-project-file-access-design.md` (Undo
journal) are replaced by this rule.

**Where it is built.** On top of this branch, as one commit: most of the
branch's tests and both kept pieces carry over. The review count starts
again at round 1.

**Costs.**
- No new stored copy, no reads on a timer, nothing at reload. Each undo or
  redo reads the small files its step would rewrite, as on this branch.
- One more journal entry and marker per outside edit met, holding two
  texts of each file in it. With a history at its cap of 200, it pushes
  the oldest step out, as a new step does.
- The first press writes over the outside edit on disk. After that it is
  kept only in the history: a new op or edit clears redo, and quitting or
  switching projects clears everything. The toast is the only warning.
- The outside edit cannot be kept while the older step is undone. History
  is linear.
- R8, and a file put back outside to an older text: one extra press, and
  for R8 a label that is not true.

## The two questions, answered (comment 1296)

Comment 1289 asked two questions with options. The user answered "1: A"
and "2: A", so both rules are mine (approved).

1. **When the step is made:** when an undo or redo reaches the file (R2).
   Its cost, as put to the user: an outside edit that no undo ever crosses
   never shows up as a step, so undo cannot take back a `git pull` by
   itself.
2. **What the press that finds it does:** it takes the outside text back
   at once, with the warning toast (R4). Its cost, as put to the user: the
   file is written before the user has seen a warning, and redo is the
   only way back.

## Tests

Core, `undo.rs`: the ticket's steps with and without a `poll` first (R1 to
R4); the redo side (R5); a delete, an outside change, two undos, the space
is back (R4, R7); a cascade where one file changed: only that file is in
the step (R3); a step made on the outside text undoes to it and the step
before takes it back first (R2); a folder's file (R3); a step beneath a
dropped one (R8); both cases of comment 1283; an outside edit between two
undos, then the redos; an unparsable file (R12); a file that is not text,
under an edit and under a space delete (R13); state and request files
never compared, the order completion followed (R9, R10, kept tests); the
path refusals (R7, main's tests).
Core, `journal.rs`: nothing new. The burst rule's tests came with #239.
App, `tests.rs`: the ticket's steps with the toasts and both stacks walked
down, up, down and past a new op (R4, R6); the redo side (R5); a delete
restored after an outside change, and after a file that was not text is
readable again (R13); a refused step, then the one beneath
(R8); the steps of comment 1290 (a move, an outside rewrite, a move up and
down that net to nothing, undo); rename, save, undo and the refresh tests
(kept).

## Mine, for veto

R2 and R4 are mine (approved). R3 and R5 to R13 are mine, not put to the
user as questions; comment 1289 pointed at the first three of this list
and they drew no objection, which does not make them the user's. They are:
the redo side mirrors undo (R5); a broken file is taken back like any
other (R12); the wording (R6); a refused step is still dropped (R7); the
untrue label in R8 is left as it is; state and request files are still not
compared (R9); the order completion is still followed (R10); a file the
check cannot read fails the undo or redo and keeps the step, as on main
(R13, changed in review round 1); the press that finds an outside edit
counts as one step of a counted undo; the work goes on top of this branch.

Not in this design: #285 (request files) and #286 (a folder rename's redo
refused on main) are unchanged. #286's fix has to keep the compare from
running on a path under a directory the same replay moves.
