# Checks tabs in the app (#309)

2026-11-16. Scope: board task #309, whose "What the user sees" list is
the whole of it. This spec restates those rules with their sources, says
how they are built, and lists the tests.

## Added beyond the ticket

None.

## 1. The problem

A request file can carry `[checks]` (suite checks spec 2026-11-14): the
statuses it expects, a first-byte limit, a total-time limit, and jq
filters that must each give `true`. `postui run` and `postui suite`
judge a response by them. The app carries them through every save
(`Editor::checks`) and shows none, so in the app a check can't be
written and a response's verdict can't be seen. This amends that spec's
R10 ("shows none").

## 2. Rules

`you` is the user's own words, `mine (approved)` a proposal the user
picked, `mine` my call. Sources are copied from the ticket.

### The request's Checks tab

| # | Rule | Source |
|---|---|---|
| Q1 | A fifth tab, Checks, last in the request's tab row, with a count as the other tabs have. Reached as they are: a click, or each profile's tab keys. No new default key. | the tab: you; its place, the count, no new key: mine |
| Q2 | Three one-line fields at the top: **status** (one code or several, typed `200, 201`), **first byte** and **total** (each a whole number of milliseconds). An empty field means no such check. | mine (approved) |
| Q3 | Under them the jq checks, one row each, with `+ Add check`, and copy and delete on each row as a Headers row has. | you: "a proper add check"; the list: mine (approved) |
| Q4 | A check is one line, typed and edited as the jq bar's filter is. | you |
| Q5 | A value the file can't hold (a code outside 100 to 599, a limit that isn't a whole number from 1 up) is refused when the field closes, with a warning toast, and the field keeps its old value, as a blanked row name is refused on the other tabs. | mine |
| Q6 | A jq check that doesn't compile is kept and drawn in the error colour. | mine |
| Q7 | Each change is one undo step, and a delete never asks. | existing rules |
| Q8 | An edit takes effect at the next send. Editing a check re-judges nothing. | mine (approved); you: "who ever asked for automatic runs?" |

### The response's Checks tab

| # | Rule | Source |
|---|---|---|
| P1 | A tab after Headers, shown when the request, as it was sent, had at least one check. | the tab: you; only with checks: mine (approved) |
| P2 | One line per check: `✓` or `✗`, then the text `postui run -v` lists for it. Order: status, first byte, total, then the jq checks in their order. | mine |
| P3 | The tab's label carries `✓` when the response passed and `✗` when it didn't, by the verdict `run` and `suite` give. So a request with no status check fails on a status of 400 or more, and the tab's first line then says why: `✗ status 500, expected below 400`. | you: "The checks badge shows the result of the checks, the same as suite would."; that line: mine |
| P4 | The status chip goes on showing the status in its usual colour, whatever the checks say. | you |
| P5 | The results are that response's. They are judged once, with the checks as they were at the send. They don't change when the checks are edited afterwards, and they are still there after a switch to another request and back. | judged once: mine (approved); the rest: mine |
| P6 | A failed check switches no tab and shows no toast. | mine |
| P7 | Search, copy, save and open in the editor act on the tab's lines as they do on Headers. | mine |

### The tab row's width

| # | Rule | Source |
|---|---|---|
| W1 | The main column's minimum stays 65. When the request's tab row doesn't fit before the split chips, every tab drops its count. | you: "yes, go back to 65 and compact the tab row"; dropping the counts as the one compact form: mine |
| W2 | The form follows the room only, never the selected tab, so switching tabs never moves the row. | the no-layout-shift rule |

"Not in this task" on the ticket is binding: no judging but at a send,
no multi-line checks, no check from a view or the jq bar, no variables
in a check, no on/off switch, names or sidebar marks, no file format
change, no second compact form, no unit in a time field, nothing for
#308 or #282.

## 3. The design

The ticket's nine "How to build it" choices (all `mine`) hold against
the code. How each lands:

**Core (`postui-core`).** `checks.rs` gains what the CLI's
`cli/checks.rs` holds today and both sides need: the below-400 rule
(`FIRST_FAILING_STATUS`, `passes`, `passed`), a check's line
(`Outcome::summary`), and the app's lines (`lines`: the outcomes'
summaries, led by `status N, expected below 400` when the 400 rule
failed the response). The CLI's `ShownCheck` calls them. `model.rs`:
the read drops a repeated status code or jq filter, keeping the first;
`Checks::parse_status` and `parse_limit` read a field's text by the
read's own limits, so the app's refusal and the file's are one rule.

**The send.** `http::ResponseData` gains `checks: Vec<Outcome>`. The
send's task takes the editor's checks at the send; after `http::send`
it runs `checks::evaluate` on the blocking pool (when there are any)
and the response arrives with its outcomes. Nothing is pending, nothing
is matched back, a cancel drops the task as before.

**The response tab.** `ViewMode::Checks`, offered when the response's
`checks` is not empty. Its lines are built once in `ReadyView::new`
from `checks::lines`, and join every place `ViewMode::Headers` supplies
plain lines (line count, search corpus, copy, save, `$EDITOR`, width).
It has none of the Headers tab's header copy pills or keys. The tab's
badge is `checks::passed`. The response side reads nothing from the
editor.

**The shared table (`table_editor.rs`).** Contract changes:

- `Stop::Head(k)`: a stop above the rows that the caller draws. The
  walk's `head: bool` becomes a head count. The Project tab's Name row
  is `Stop::Head(0)` (before: nothing selected), so the walk has one
  rule and "nothing selected" means the same on every table. Heads are
  not content lines: with the cursor on one the view shows the table
  from its top (`settle_view_under_head` and `view_scrolled_under_head`
  fold into `settle_view` and `view_scrolled`).
- A head can be open in the table's one edit slot (`CellEdit` with
  `Col::Head`, its `row` the head's index). The table never writes a
  head: closing one hands its text back (`TableOutcome::head_commit`),
  and a Tab walk that reaches a head asks the caller to open it
  (`TableOutcome::open_head`). The Project tab keeps its own Name edit
  and ignores both. So every reader of "a cell is open" (commit on
  click-away, save, send, tab switch, the undo bracket, paste, vim's
  field session, the mouse) meets a head as it meets a cell.
- `TableSpec`: what a table lists, handed to the walk, the commit, the
  click and the draw: its row noun for messages, its head count,
  whether a row is one column (no value cell, no toggle), and whether
  its inputs take variables. `EditorTab::table_spec` is the one spot the
  Checks tab's words and shape come from.

**The editor.** `EditorTab::Checks`. `Editor::checks` keeps the status
codes and the limits; the jq checks live in `Editor::check_rows`, an
`IndexMap` keyed by the check's text, so every table path (add, delete,
copy, put, undo's caret) is the one the other tabs use.
`current_request` puts them back in order. A check's text is the row's
name: a blank one is refused and a repeat merges, by the table's own
rules, worded for a check. A check that spans lines in a hand-written
file is the row's name as read; unedited, it is written back as read.
The fields draw above the table and don't scroll with it. A field's
close parses its text with core's `parse_status` / `parse_limit`; a
refusal is a warning toast and the old value stays (Q5).

**Variables.** `Editor::caret_takes_vars` is the one question; it
answers no for the Checks tab's content. Its askers are listed in
section 4.

**The tab row.** `draw_tab_bar` lays the row out with counts; when its
end (the `vars` mark included) reaches the split chips it lays it out
again without them.

Removed: `cli/checks.rs`'s own `FIRST_FAILING_STATUS`, `passed` body
and `summary` body; `settle_view_under_head`,
`view_scrolled_under_head`.

## 4. Consumer audit

Every reader the three fields and the one-column list meet.

**The table's cursor (`TableEditorState::selected`, `Stop`).**
`Stop::ordinal/line/at/clamp`, the walk, `settle_view`,
`cursor_into_view`, `draw_row` (table); `Editor::enter_content` (first
stop is the status field), `caret` / `restore_caret` / `recorded_stop`
(undo puts the cursor back on its field), `table_cursor`,
`table_cursor_live`, `client_owned_row`, `table_verb_action` (on a
field: add and put as on `+ Add`, copy and delete nothing),
`cursor_row`, `section_cursor`, the unconsumed-Up climb to the URL line
(from the status field), `draw_table_tab`; `App`: the footer's row
chips (`pane_chip_args`), `focused_extract_text`, the row click and
right-click paths in `mouse.rs` (`resolve_table_row_across_commit`, the
row hit, the hover-follow at `mouse.rs:701`), `repeat_cap`; the Project
tab's `on_name`, `begin_name`, its draw and its wheel.

**The open cell (`TableEditorState::editing`).** By presence only, so a
head needs nothing: `Editor::plain_keys_type`, `load`,
`apply_snapshot`, `table_cursor_live`; `App::commit_table_edit`, the
dirty mark (`ui.rs`), `capture_undo`, `pane_field_open`,
`OverrideAutoHeader`, `landing.rs`, `repeat_cap`, `line_buf`. By row,
each told a head from a row: `screen_field` (the field's id),
`App::editing`, `resolve_table_row_across_commit`, the row-hit click
(`mouse.rs:2251`), `Editor::adding_row`, the table's `editing_ghost`,
`clamp_view`, `draw_row`, `commit_cell`, the editing keys.

**Variables in a text input**, each asking `caret_takes_vars` (or, for
drawing, the table's spec): the `{{` picker (`Editor::handle_key`, vim's
`after_typed`), the picker by key, chip and palette
(`Action::OpenVarPicker`), a picked token's insert
(`Action::InsertVarText`, `vim_insert`), the selector picker on a token
under the caret, Extract to variable / selector (palette, row menu,
text menu), token colour and hover tips (`paint_cell_tokens`), the
caret's tooltip and its keys (`Editor::caret_token`), the footer's
`vars` chip.

**Words** (`TableSpec::noun`): the table's three warnings, the delete
toasts, the copy toast, the row menu, the footer's add chip, the pill
hints.

## 5. Tests

- Core: a repeated status or filter reads once; `parse_status` /
  `parse_limit` accept and refuse what the read does; `passed`, `lines`
  and `summary` over each kind of outcome, the 400 line included; the
  CLI's existing checks tests keep passing unchanged.
- Response: the tab shows only with outcomes; its lines and badge for a
  jq-only request that gets a 500, a non-JSON body, no checks, a failed
  send; results survive a switch away and back and an edit of the
  checks; search, copy and save read the lines; the lines equal `run
  -v`'s for the same request.
- Table: the head walk test, over three heads; a head edit's keys
  (Enter, Esc, ↑↓, Tab, Shift-Tab) and what they hand back; one-column
  rows (no value cell, no toggle, Tab walks rows).
- Request tab, in both profiles: the walk across fields, checks and
  `+ Add check`, open and closed; clicks and pills; copy, delete, put,
  add on a check and on a field; paste; undo, redo and the unsaved mark;
  a refused field value; every message says "check"; nothing about
  variables is offered or drawn; a check that doesn't compile draws in
  the error colour.
- Width: `the_tab_row_fits_beside_the_split_control_at_the_main_minimum`
  with five tabs; the row is the same whichever tab is selected.

## 6. `mine` choices, for veto

1. The tab's count is the number of checks: one for a status field
   that is set, one for each limit, one for each jq check.
2. On a field, the copy and delete keys do nothing; add and put act as
   they do on `+ Add`. Enter opens the field.
3. Tab in an open field goes to the next field, then the first check;
   Shift-Tab goes back the same way.
4. The fields stay put above the list while the list scrolls. In a
   pane too short for all three, the ones that fit are drawn.
5. A scratch request that holds only checks counts as unsaved work.
6. The response tab's copy toast says "Copied check results".
7. In the app the lines are not masked: they quote the response body,
   which the app shows in the clear.
