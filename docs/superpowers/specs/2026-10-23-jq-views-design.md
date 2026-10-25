# jq Views — Design

Date: 2026-10-23
Status: approved 2026-10-23; amended 2026-10-24 (the scratch is file
content, below), amendment approved 2026-10-24; amended again 2026-10-24
(#101, the bar's layout, copy to scratch and clear), approved in chat;
amended again 2026-10-24 (#102, no view, one clear, no copy), approved
in chat
Task: #96, #101, #102
Builds on: `2026-09-03-jq-response-filter-design.md`

## Amendment 2026-10-24 (#102): "no view", one ✕ to clear, no copy

**Supersedes** B4's `⧉`, B7, B9's copy button, `c` key and chip, B10's
copy entry, and every place the UI says "scratch". The model is
unchanged: the scratch is still the request's unnamed filter, and the
code still calls it that.

### Why

From the user after #101: "i dont like the copy view to scratch. the copy
icon doesnt make sense. that makes me think something is going to the
clipboard." "'clear the scratch' sounds strange too." "calling it scratch
is a little strange in general i think. the app thinks of it as scratch.
but the user doesnt really. its just, no view more than its scratch." On
what they reach for: "i really just want to clear the filter and start
from the beginning." And: "maybe an 'x' to clear the current view? copy
to scratch doesnt seem necessary now that i think about it, i can just
save as a new view for that use case." (Save as view from an edited view
already moves the edits to the new view and reverts the source,
decision 6.)

### Rules

| # | Rule | Source |
|---|---|---|
| C1 | The UI never says "scratch". The chip reads **no view**; tooltips, palette entries and toasts say "filter". | you (the idea, "no view"); the wording mine (approved) |
| C2 | Copy to scratch is gone: the bar's `⧉`, a view row's `⧉` and its `c` key (and the footer's `c` chip), and the palette entry. | you |
| C3 | The bar's slot shows `✕` **Clear the filter** on any view and on no view with text, and stays blank on an empty no view. Its width never changes (B4's reservation stands). | you (the ✕); when it shows mine (approved) |
| C4 | **Clear the filter** (the bar's `✕`, the palette) leaves the current view for no view with an empty filter. The view is untouched, unsaved edits included. On no view it empties the filter. Never asks; one undo step, which brings back the old no-view text and the view current. The switch stays as it is; from the palette focus stays put, and the bar's `✕` gives the bar the caret (B8's click rule). An empty no view refuses: "Nothing to clear: the filter is empty". | you ("an x to clear the current view", "start from the beginning"); the rest mine (approved) |
| C5 | The views dropdown has no row for no view: it lists the views, then the divider and `+ save as view…` (no divider when there are no views). The bar's `✕` (or the palette's clear) is the only way to no view. On no view no row is marked and the first view is highlighted. *Supersedes* spec 16's scratch row and B9's scratch-row clear. | you ("im not sure if i want no view in the dropdown. just leave it to the x for that"); the divider and highlight mine |
| C6 | The clear says "Filter cleared" with the undo hint. | mine (approved) |
| C7 | The palette's clear is "Response: clear jq filter" (C4). | mine |
| C8 | The no-view chip keeps the accent colour: muted already means a stale result. | mine (approved) |
| C9 | No view name is reserved: "scratch" and "no view" are ordinary names, and the file reader takes them. A view named "no view" makes the chip read as if none is current; that is the name's owner's choice. (First built as a reserved "no view" with lookalike spacing folded; each review round then found another invisible lookalike, and with C5 there is no dropdown row for a view to be mistaken for.) | you ("Drop it", after review round 3) |

| C10 | `+ save as view…` is enabled on an empty bar too, from the dropdown and the palette: it saves an empty view, current, for the typing to go into. *Supersedes* spec 18's "disabled while the bar is empty". (Review round 3: with no views and an empty bar, C5 left the dropdown with only a disabled row.) | you ("Can't we just enable the save as view even when there's empty filters?") |

### Design

- `JqView::SCRATCH` becomes `JqView::NO_VIEW` = `"no view"`, the chip's
  label only; `ViewNameRefusal::Scratch` goes (C9).
- `JqChip`'s `on_view` and `scratch_has_text` become one `clearable`
  (C3); `Hit::ResponseJqCopy`, `Action::CopyJqViewToScratch`,
  `Verb::CopyToScratch`, its `c` binding, `Hit::row_copy` and
  `Editor::jq_copy_to_scratch` go.
- `Action::ClearJqScratch` becomes `ClearJqFilter` (C4), sent by the
  bar's `✕` and the palette. `PickJqView` takes a view's name, no longer
  an `Option` (C5: nothing picks no view).

### Tests

- The slot shows `✕` on a view and on no view with text, nothing on an
  empty no view; the chip's column does not move.
- The bar's `✕` on an edited view: no view, empty, the view keeps its
  edits, the caret in the bar; one undo brings back the old text and the
  view current.
- The palette's clear on a view: no view, empty, focus and switch stay.
- The dropdown lists only views and the save; nothing marked on no view.
- The chip reads "no view"; no hint, toast or palette entry says
  "scratch"; views named "no view" and "scratch" save and read.

## Amendment 2026-10-24 (#101): the chip moves right, copy to scratch, clear

**Supersedes** decision 14's chip placement and `●` mark, and amendment
A8's mark. The ↺ stays.

### Why

From the user's manual testing after #96: "i dont like the drop down and
dirty marker to the left of the text. it means the text moves around
sometimes … move that over to the right side where there are already
buttons and such." And: "im not sure we need a dirty marker since the
revert button will appear." And the missing piece: "i need a way to take
the current view, and just copy it to scratch so i can play without
editing it", "and maybe a way to clear the scratch entry." Mockups:
https://claude.ai/artifact/3m9qXasb5A6nhgPHH4zvw2 (option A there is this
amendment; option B, views that never change from typing, was turned
down).

### Rules

| # | Rule | Source |
|---|---|---|
| B1 | The filter text starts in one column whatever is current: after a fixed `jq ` label at the bar's left, the one `main` had before views. Nothing left of the text changes width. | you (move it right); the label mine (approved) |
| B2 | The right side reads `[↺] [name ▾] [slot] [AI]`, the AI button fixed at the far right as today. The chip opens the views dropdown, as before. | you (right side); the order mine (approved) |
| B3 | No `●` mark. The ↺, shown while the current filter differs from its saved text, is the only sign. The ↺ takes its cells from the end of the text window. | you |
| B4 | The slot (a button's three-cell pill and a gap, as the ↺ takes) is always reserved: `⧉` copy to scratch on any view, `✕` clear on a scratch with text, blank on an empty scratch. Its width never changes, so the chip never moves when it switches. | mine (approved) |
| B5 | A slow run's spinner takes the chip's `▾`, as it does now (one cell either way). | decision 14 amendment, unchanged |
| B6 | On a narrow bar the text keeps at least half the row: the chip's label is cut first (as now), then the ↺ and the slot drop, then the AI button, as the ↺ drops today. | mine |
| B7 | **Copy to scratch** puts a view's filter as it stands (unsaved edits included) into the scratch, makes the scratch current and switches the filter on. The view is untouched. One undo step: undo brings the old scratch back and the view current again, as a menu action's landing does (decision 8). *Round 6, mine:* a copy says `Copied "<name>" to the scratch` with the undo hint, as a clear does. | you (copy); the details mine (approved) |
| B8 | **Clear** empties the scratch. One undo step, never asks (standing rule). What is current does not change; clearing the current scratch puts the caret in the bar, so an empty bar with no views stays up to type in. An already empty scratch refuses with a toast. *Amended (#101 after review round 3), mine (approved):* the clear itself only empties the scratch, from anywhere: focus, the switch and what is current stay put. The bar's ✕ is a press on the bar, so its click also gives the bar the caret, as a click on the text does, and the emptied bar stays up. (Rounds 1–3 each found a case where the clear moving focus from the palette or menu went wrong: a bar that could not take the caret, a switched-off filter, a collapsed pane.) *Round 4, mine:* a clear says "Scratch cleared" with the undo hint, as a deleted view does, since from the palette or the menu the scratch may not be on screen. | you (clear); never asks the standing rule; the rest mine (approved), "current unchanged" and the empty refusal mine |
| B9 | In the dropdown, a view row's own buttons are `⧉` `✎` and the delete icon, with keys `c`, `r` and `d` (`dd` in vim), the same letters in both profiles. The scratch row has one, `✕` clear, on `d`/`dd`: the scratch cannot be deleted, so its delete clears it. The footer gains a `copy` chip beside rename and delete. *Amended (#101 review round 1), mine:* a key's chip names every word its buttons go by, in row order: `d clear/delete` with views and a scratch with text, `d clear` for the scratch alone. | mine (approved) |
| B10 | The palette gets "Response: copy jq view to scratch" (the current view; on the scratch it refuses with a toast) and "Response: clear jq scratch". | mine (approved) |
| B11 | The focused bar gets no new keys: typing goes into the filter. | mine (approved) |

### Design

- `JqChip` keeps `modified` (it places the ↺ and the footer's revert
  chip) and gains whether a view is current, for the slot. Its text loses
  the mark.
- `jq_bar_layout` lays out `jq `, the text window, the ↺, the chip, the
  slot and the AI button, in that order; `Response::jq_chip_rect` still
  predicts the dropdown's anchor from it. New hits `ResponseJqCopy` and
  `ResponseJqClear` for the slot.
- New actions `CopyJqViewToScratch(Option<String>)` (`None` = the current
  view) and `ClearJqScratch`, each one undo step, closing open inputs
  first as the other view actions do (#75). New editor methods do the
  edit.
- A dropdown row's own buttons become a list (`MenuItem`'s rename and
  delete fields are replaced by row buttons, each a verb, a glyph, a tip
  and an action), and `Hit::DropdownRowRename`/`DropdownRowDelete` become
  one `Hit::DropdownRowButton { row, verb }`. The keyboard (`r`, `c`,
  `d`) and a click both find a row's button by its verb, as they do now.
- New `Verb::CopyToScratch`, bound to `c` on `Surface::Menu` in both
  profiles.

### Tests

- The text's start column is the same on the scratch, on a view, on an
  edited view and with a long view name; nothing paints a `●`.
- The slot shows `⧉` on a view, `✕` on a scratch with text, nothing on an
  empty scratch, and the chip's column does not change between them.
- Copy from the bar's `⧉`, from a dropdown row (`c` and a click) and from
  the palette: the scratch holds the view's filter, the scratch is current
  and on, the view is unchanged; one undo restores the old scratch and the
  view current.
- Clear from the bar's `✕`, the scratch row (`d`/`dd` and a click) and the
  palette: one undo step, current unchanged; the empty refusal; the caret
  in the bar after clearing the current scratch.
- The dropdown's footer advertises copy; the tooltips name the buttons.

## Amendment 2026-10-24: the scratch is file content

**Supersedes** decisions 2, 9, 10, 12, 19, 20, 24 (for the scratch), 29
and the "Undo and dirty" section where they say the scratch, the current
view or the switch are local. Every amendment paragraph dated 2026-10-24
below that is about `.local/jq.toml` is history now.

### Why

Five review rounds on the local `.local/jq.toml` kept finding new holes in
the same two mechanisms. The entry has to follow its request through
rename, move, duplicate, delete and undo, and also survive deletes made
outside the app, which the app cannot see. Its write timing also raced
whenever a second `Project` opened on the same folder. The user, asked
whether the scratch could go in the request file: "I'm not actually that
concerned with it being dirty or being saved. We should treat it sort of
like view that can't be renamed or deleted" **(user)**. In the file, the
scratch follows its request for free, because the request's own file
operations and undo already move it.

### Rules

| # | Rule | Source |
|---|---|---|
| A1 | The scratch is the request's unnamed view: it lives in the request file, cannot be renamed or deleted, and is otherwise a view. | you |
| A2 | Editing the scratch is a request edit: dirty, Save, undo, discard, as for a view (decision 4). | you |
| A3 | The scratch is the existing `jq = "…"` line, omitted when empty. Old files open with it as they are; the legacy path (decision 12) goes. | mine (approved) |
| A4 | The current view and the switch are in the file too, as plain edits: picking a view or flipping the switch dirties the request, is an undo step, and Save writes it. Decision 9's "never dirty" and the rule that "picking a view on its own is not an undo step" go. | mine (approved) |
| A5 | The switch is the existing `jq_enabled = false`, written whenever it is off, so the file holds exactly what the editor does. A hand-written `jq = ""` reads as no scratch. *Amended 2026-10-24 (round 3 after the rework):* first written as "only beside a scratch or a view", as on `main`; that left a state the file could not hold, and each boundary translated it (the writer, the editor's build, and the reader that did not, so such a file opened dirty). Cost: a request switched off with nothing to filter keeps the line, and flipping it is an edit with nothing visible. | mine (approved) |
| A6 | The current view is `jq_current = "<name>"`, written after `jq_enabled`, omitted for the scratch. A name no view has opens on the scratch (decision 27 stands); the next save drops the line. | mine |
| A7 | No local state means nothing now: a file with no `jq_enabled` line is on, as on `main`. Decision 10 ("no local state means off") goes. A teammate opening a request with views and no `jq_enabled` sees the bar on, with the scratch current. | mine |
| A8 | The scratch gets the modified mark and the ↺ revert to its saved text, as a view does (decision 5). | mine (approved) |
| A9 | Save as view from an edited scratch moves the edits to the new view and puts the scratch back to its saved text, as from a view (decision 6). An unedited or never-saved scratch is copied and keeps its text. | mine (approved) |
| A10 | Right-click menu actions and the AI reply still land in the scratch and make it current (decision 8). That is now a request edit, one undo step. | you (decision 8) + A2 |
| A11 | Curl replace keeps the scratch, the switch, the current view and the views (decision 30), as it keeps `jq` today. | decision 30 |
| A12 | An existing `.local/jq.toml` from earlier builds of this branch is ignored and left on disk. There are no pre-release migrations. | standing rule |

### What goes

- Core: `project/jq_local.rs`, `Local::jq`, `JQ_TOML`, `JqLocal`,
  `EntryMeta::jq`, the re-key, copy, carry and prune hooks, the deferred
  write with its `Drop`, the idle tick, the retry, the flush before
  `InitProjectHere`.
- App: `persist_jq_local`, `with_jq_local`, `Editor::carry_jq_local` and
  `Editor::jq_local` (discard and reload take the file's state like any
  field), `create_request_from`'s `jq` parameter, the `is_dirty` mask, and
  `undo::view_choice_only` and `undo::switch_without_filter` (picks and
  flips are ordinary steps).
- Round 4's directory compare in `SwitchProject` (`Project::is_root`): it
  was there for the deferred write. Opening the same folder twice is a
  general question that `main` has for `state.toml` too, and it is not
  this feature's.

### What stays

Views and their ids, the dropdown with its row rename and delete, revert,
Save as, the chip and spinner, the per-view and scratch undo coalescing,
the view-name rules (unique, not blank, not `scratch`, enforced by the
file reader too), menu actions landing in the scratch, and the
click-away exemption for the chip and ↺.

### Tests

- Scratch edits dirty the request, Save writes `jq = "…"`, reopening
  shows it (replaces `the_scratch_survives_a_restart_and_never_dirties`).
- A pick and a switch flip each dirty the request and are one undo step
  (replaces `picking_a_view_records_no_undo_step` and the empty-bar
  no-step test).
- A file with `jq_enabled = false` and `jq_current = "names"` round-trips
  exactly. A dangling `jq_current` opens on the scratch without going
  dirty.
- The scratch's mark, ↺ and Save as move (A8, A9).
- Rename, move, duplicate, delete and their undo keep the scratch, the
  switch and the current view, through the file alone.
- The jq.toml tests go with the file.

### Review count

This replaces the mechanism rounds 1–5 reviewed, so the next review is
round 1 of a new block (work-task §5). The old rounds' history stays on
#96.

## Why

Today a request carries one jq filter (`jq = "…"` plus `jq_enabled` in the
request file), and a filter change never marks the request dirty. So a
filter edit on its own is never saved: the Save button never shows up for
it. Worse, there is only one slot: the user wants to look at one aspect of
the data, changes the filter to look at another, and has to choose which of
the two to keep.

This adds **views**: several named jq filters per request, saved in the
request file and shared through it, plus one **scratch** filter per request
that is the user's own space to explore, remembered on their machine and
never dirtying anything.

The app is unreleased and the immediate use is saving a couple of filters
in the user's own testing setup, so the design favours simple over
bulletproof where the two differ.

This is phase 1. Views that belong to a space or the whole project are a
later phase. Nothing here should rule them out, but nothing here builds
them.

## Goals

- Keep several named filters on a request and switch between them from a
  dropdown on the jq bar.
- Exploring never costs a saved view: the scratch is free to change and is
  remembered without a Save.
- Changing a saved view is an ordinary request edit (dirty, Save, undo,
  discard), with a per-view revert.
- Rename and delete views from the dropdown itself.

## Non-goals

- Space- or project-level views (phase 2).
- A manage screen or dialog for views. The user wants to avoid one.
- Reordering views. They list in the order they were saved.
- Views in curl import/export. The filter never affected what is sent and
  still doesn't.

## Anti-goals

- A view changing without the user editing it. A right-click menu action or
  an AI reply never rewrites a saved view.
- Losing exploring work inside a session: anything that replaces the
  scratch is undoable.
- An existing request file that stops opening because it has a `jq` line.

## Decisions

Marked **(user)** where the user decided, **(mine)** where I made the call
and the user can overrule it while reviewing.

### The model

1. **Saved views live in the request file** and are shared with it
   **(user)**. Each has a name and a filter.
2. **The scratch is local** **(user)**: one per request, in `.local/`, never
   in the request file, never in git. It is remembered across restarts. It
   never marks the request dirty.
3. **The bar always shows the current filter**: the scratch or one saved
   view. The response shows that filter's output, as today.
4. **Picking a saved view makes it current, and editing the bar edits that
   view in place** **(user)**. Those edits are ordinary request edits: the
   request goes dirty, the request's Save keeps them, and undo and discard
   work as for any field **(user)**. There is no filter-only save.
5. **Revert** **(user)**: a saved view whose filter differs from its last
   saved text shows a modified mark and a ↺ revert. Revert puts just that
   view's filter back to its saved text and leaves the rest of the request
   alone. A revert is an undoable edit.
6. **Save as view** saves whatever is in the bar as a new saved view with
   a name the user types, whichever filter is current, and the new view
   becomes current **(user)**. It is a request edit. From a saved view
   with unsaved filter edits, the edits **move** to the new view and the
   original goes back to its saved text, like an editor's Save As
   **(user)**; the whole thing is one undo step. From an unedited view it
   is a copy. From the scratch, the scratch keeps its text **(mine)**.

   *Amended 2026-10-24 (round 6 after the rework), **(mine, approved)**:*
   the new view is switched on as it becomes current, as a pick makes a
   view (decision 16), so saving one from the palette with the bar closed
   shows what was saved.
7. **Rename and delete** are request edits. Delete never asks; undo is the
   safety net (standing rule).
8. **Right-click menu actions and the AI reply switch to the scratch**
   **(user)**: "it's not like an edit of the existing thing." When the
   current filter is a saved view, the action builds on that view's filter,
   because the tree the user clicked is that filter's output. The result
   replaces the scratch and the scratch becomes current. The saved view is
   untouched. The old scratch is gone, but one undo brings it back and
   makes the saved view current again **(user)**. This covers
   `Action::JqApply`, `Action::JqTeeUp` and `Action::JqAiFinished`.
9. **What is current is saved** **(user)**, with the on/off switch, per
   request in `.local/`, not the request file **(user)**, never
   dirty. Picking a view is a choice about what you look at, not an edit
   to the request.
10. **No local state for a request means the filter is off** **(user)**: a
    teammate's first open, or a fresh clone, shows the full body with the
    scratch current and empty. They find the shared views by opening the
    bar (alt+q, the header 󰈲 button), where the chip and dropdown are.

### The file

11. **Format** **(mine)**: an array of tables, written after the request's
    plain keys and before its sub-tables, wherever toml_edit puts arrays of
    tables cleanly:

    ```toml
    [[jq_view]]
    name = "names"
    filter = ".items[].name"

    [[jq_view]]
    name = "active users"
    filter = ".items[] | select(.active)"
    ```

    Omitted when there are none. `jq_view` rather than a bare `view` so
    someone reading a diff next to `params` and `headers` can tell what it
    is.
12. **Old `jq` lines are read into the scratch** **(user)**. The model still
    accepts `jq` and `jq_enabled` but never writes them. When a request
    opens with a `jq` line and there is no local scratch for that request
    yet, the old filter becomes the scratch, `jq_enabled = false` becomes
    the switch, and the scratch is current. If a local scratch already
    exists, it wins and the old line is ignored. The next save of the
    request writes no `jq` line, so the old line goes away then. Opening
    alone writes nothing to the request file, and must not make the
    request dirty. Anything else that rewrites the file before the request
    is opened (a rename does) drops the line; the user accepts that
    **(user)**: "not worried about the rename case."

### The UI

13. **"View" is the word everywhere the user sees it** **(user)**: the
    dropdown, palette entries, footer hints, toasts. The response-to-file
    picker titled "Save response view" (`app.rs` `open_save_picker`) is
    retitled **"Save response to file"** **(user)** so the two can't be
    confused.
14. **A chip on the left of the jq bar names the current filter**: `scratch ▾`
    or `<view name> ▾`. A saved view with unsaved filter edits shows a
    modified mark, `names ● ▾`, and a ↺ revert button beside the AI button.
    Clicking the chip opens the dropdown. The chip takes the current `jq `
    chip's place; the spinner that takes over the `jq ` chip during a long
    run (`draw_jq_bar`) still needs a home (builder's call, but the filter
    text must not shift when it appears).

    *Amended 2026-10-24 (round 6 after the rework), **(mine, approved)**:*
    one layout (`jq_bar_layout`) places the chip, the text window, the ↺,
    the AI button and the row under the text; the chip takes at most half
    the row, its label cut to fit. A click that keeps a focused bar's
    caret is any press on the bar's own row or on any footer chip (the
    focused bar's chip set), not a list of controls. That changes `main`
    for the AI button and the footer's `describe…`, `commands` and `quit`
    chips: they no longer blur the bar first, as their keys never did.
15. **When the bar shows** **(mine, approved)**: as today (`JqBar::is_open`: focused,
    switched on with a non-empty filter, or a run or AI reply pending),
    plus whenever the filter is switched on and the request has views,
    even if the current filter is empty. That keeps the chip, and so the
    dropdown, reachable on an empty view or an empty scratch.
16. **The dropdown** (`Modal::Dropdown`) lists:
    - `scratch`
    - the saved views in saved order
    - a divider
    - `＋ save as view…`

    `✓` marks the current one (the dropdown's existing `current`). Choosing
    a row makes it current and switches the filter on **(mine, approved)**.
17. **Rename and delete live in the dropdown rows** **(user, option A)**: the
    highlighted or hovered view row shows ✎ rename and 󰅖 delete. On the
    keyboard, `r` renames and `d` deletes the highlighted view (`dd` in the
    vim profile), matching the list-verb letters the panes already use.
    The scratch row has neither. This is new for `Modal::Dropdown`, whose
    rows are plain today.
18. **Save as view** and **rename** use the app's one-field prompt
    **(mine)**. A name is trimmed. An empty name, or one that matches
    another view on the same request ignoring case, keeps the prompt open
    with a toast, the way taken request names do
    (`a_name_taken_on_enter_keeps_the_prompt`) **(mine)**. "Save as view"
    is disabled while the bar is empty **(mine)**.

    *Amended 2026-10-24 (#96 review round 1), **(mine, approved)**:* case
    is compared the Unicode way request names are (`JqView::name_key`),
    not ASCII only. `scratch` in any case is refused as a view name, since
    the chip and the menu's first row already say it. A request file with
    two views of one name refuses to open with a parse error, as a
    duplicate header key does: the app addresses a view by its name, and
    two of one name made pick and rename reach the first while delete
    removed both.

    *Amended 2026-10-24 (#96 review round 3), **(mine, approved)**:* a file
    view with a blank name or named `scratch` refuses the open the same
    way: the file holds only names the app itself would accept.

    *Amended 2026-10-24 (round 5 after the rework), **(mine)**:* nor may a
    name hold a control character (a tab, an escape): the chip and the
    menu paint names as they are. One rule, `JqView::name_refusal`, serves
    the file reader and the editor's save-as and rename.

### Local storage

19. **The scratch, what is current and the switch go in their own file**
    under `.local/` (for example `.local/jq.toml`, keyed by request slug),
    not in `state.toml` **(mine, approved)**. `state.toml` is journaled whole, so a journal
    undo of any space, environment or request op would otherwise put back
    every request's scratch as of that op. The new file is not journaled.
    Two windows on one project: the last to write wins, like
    `layout.toml`. That is accepted given the app's stage.

    *Amended 2026-10-24 (#96 review round 1), **(mine, approved)**:* the
    file is not written at each change (every scratch keystroke was a full
    fsync'd rewrite on the UI thread). Memory is the truth while the
    project is open; a change marks the file stale, and it is written once
    changes stop for a second (the app's tick) and when the project is
    dropped (quit, project switch). The cost: a crash or kill loses up to
    the last second of scratch changes, never anything in a request file.
20. **The local entry follows its request** **(user)**: "a user would expect
    it to carry over … just remap the scratch to the new name."
    - **Rename and move** (one request, a folder, a space, move-all)
      remap the entry from the old slug to the new one. The journal
      already records exactly these pairs for the open request to follow
      (`EntryMeta::moves`, `crates/postui-core/src/journal.rs`), so the
      remap uses the same pairs: forward when the op runs or is redone,
      reversed when it is undone **(mine)**. Undo and redo already walk
      those pairs to re-key held state (`rekey_held_after_own_op` in
      `crates/postui-core/src/project/undo.rs`), which is the natural
      place to hook it.
    - **Duplicate** copies the entry to the copy's slug **(mine)**.
    - **Delete** leaves the entry alone, so undoing the delete finds it
      again. Entries whose request no longer exists are dropped when the
      project opens **(mine)**.

      *Amended 2026-10-24 (#96 review round 1), **(mine, approved)**:* a
      delete takes the entry out with the request, into its journal entry
      (`EntryMeta::jq`, in memory); replaying that entry restores the
      request and puts the entry back, and the reverse step takes it out
      again. A request file trashed alone, or under a trashed space, takes
      its entry either way. Left behind, a deleted request's entry had to
      be cleared by every create, rename and duplicate landing on its
      slug, and that clear lost it for good: delete, create the same name,
      undo both, and the scratch was gone. The open's prune now only
      covers requests deleted outside the app.

      *Amended 2026-10-24 (#96 review round 3), **(mine, approved)**:* one
      rule keeps entries honest without per-op clears: only a listed
      request has an entry. A slug the listing lacks takes none, and every
      poll that re-lists after an outside change prunes as the open does,
      so a request deleted outside the app leaves nothing for the next
      request created at its slug. A project switch writes the current
      project's pending entries before opening the target.

    Nothing re-keys per request today (`LocalState` only re-keys on a
    *space* rename, `LocalState::rename_space`), so this is new work in
    each of those paths.

### Smaller rules (all mine, approved)

21. (Folded into decision 6.)
22. Deleting the current view makes the scratch current; the scratch is
    unchanged.
23. Renaming keeps the view current and keeps its link to its saved text,
    so revert still works after a rename. Revert restores the filter only,
    never the name.
24. A view saved since the last save of the request has no saved text, so
    it has no modified mark and no revert. Undo covers it.
25. The modified mark tracks the filter only. A renamed view is covered by
    the request's own dirty mark.
26. A view's filter may be empty. It then shows the full body, like an
    empty scratch.
27. If the remembered current view no longer exists when a request opens
    (deleted, renamed, or changed by a teammate), the scratch is current.
28. The on/off switch is one switch for the bar, applying to whatever is
    current, as today (closing the bar switches the filter off and keeps
    its text).
29. A never-saved request (no slug) keeps its scratch in memory, and it is
    stored once the request is saved. Views or a non-empty scratch count as
    content for `is_scratch_dirty`, the way `jq` does today.
30. Replace-with-curl keeps the views, the scratch, what is current and the
    switch, the way it keeps `jq` today.
31. The AI prompt's "current filter" is the current filter, scratch or view.

## Undo and dirty

- `Editor::is_dirty` stops masking jq. The views are part of the request and
  compare like any field. The scratch, what is current and the switch are
  not in the request file, so they never affect dirty.
- **Undo restores exactly** (standing rule). Every step in the app's undo
  history records the scratch, what is current and the switch alongside
  the request, so undoing a step restores all of them as they were before
  it, including the "menu action replaced the scratch" case in decision 8.
  (This is the in-app editor history in `crates/postui/src/undo.rs`, not
  the core journal; decision 19 keeps the scratch out of the journal.)
- Picking a view on its own is not an undo step **(mine)**. But a step taken
  on a view records that view as current, so undoing it lands back on it.
- Scratch edits stay undoable as filter edits are today (they coalesce under
  the existing `CoalesceKey::Jq`). Edits to a view coalesce per view.

## Where things are now (for the builder)

- Model: `HttpRequest::jq`, `jq_enabled` and the hand-built writer in
  `crates/postui-core/src/model.rs`.
- Editor: `jq`/`jq_enabled` fields, `load`, `apply_snapshot`,
  `current_request`, `is_dirty` (the jq mask), `is_scratch_dirty` in
  `crates/postui/src/components/editor.rs`.
- Bar ↔ editor sync: `App::sync_jq` in `crates/postui/src/app.rs`.
- Bar drawing: `draw_jq_bar` in `crates/postui/src/components/response.rs`;
  bar state `JqBar`; open rule `JqBar::is_open`.
- Menu writes: `Action::JqApply`, `Action::JqTeeUp`; AI reply:
  `Action::JqAiFinished`.
- Undo coalescing: `coalesce_key` in `crates/postui/src/undo.rs`.
- Local files: `crates/postui-core/src/project/local.rs` (`state.toml`,
  journaled whole; `layout.toml`, not journaled, last writer wins) and
  `LocalState` in `crates/postui-core/src/project/meta.rs`. File access
  goes through `Project` (see the project `CLAUDE.md`).
- Dropdown: `DropdownState`, `MenuItem`, `draw_dropdown` in
  `crates/postui/src/components/modal.rs`.
- Curl replace: `crates/postui/src/app/curl_flow.rs` (`jq: current.jq.clone()`).

## Left to the builder

- The exact name and shape of the new `.local/` file, and how its entries
  reach the editor's undo snapshot.
- How views are identified in memory so revert and the modified mark follow
  a view through renames.
- The keyboard route to the dropdown: a palette entry at least, plus a key if
  one fits. Any new single-letter key gets the same letter in both
  profiles; footer hints advertise what the focused bar offers. Nothing may
  need the vim profile.
- Chip, mark and button glyphs (one-cell icons), widths and clipping of long
  names on a narrow pane.
- Where the run spinner goes now that the chip names the filter.

## Acceptance

- Editing a saved view marks the request dirty, Save writes it, and reopening
  shows it. Editing the scratch never marks the request dirty, and the
  scratch is back after a restart.
- A request with views but no local state opens with the filter off.
  Opening the bar shows the scratch chip, and the dropdown lists the views.
- Revert restores just the current view's filter and is undoable.
- A right-click menu action or an AI reply on a saved view leaves that view
  unchanged, puts the result in the scratch and makes it current. One undo
  restores the old scratch and the view as current.
- "Save as view" from an edited view moves the edits to the new view and
  puts the original back to its saved text, in one undo step; from an
  unedited view it copies; from the scratch the scratch keeps its text.
- Renaming or moving a request (alone, in a folder, a space rename, move
  all) keeps its scratch, current view and switch; undo and redo of that
  op keep them too. A duplicate starts with the same ones. Undoing a
  delete gets them back.
- Rename and delete work from the dropdown by mouse and keyboard in both
  profiles; delete never asks and undo restores the view.
- A request file with an old `jq` line opens without going dirty, shows that
  filter as the scratch, and loses the line on the next save. A file with
  `[[jq_view]]` round-trips exactly.
- The export picker reads "Save response to file".
