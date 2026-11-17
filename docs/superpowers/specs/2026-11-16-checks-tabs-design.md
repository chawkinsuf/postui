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
| W3 | The mark that a body's variables are on is part of the Body tab's label, as a count is part of its tab's: `Body · vars`, inside the tab's width, click target and underline. | you: "the vars should be part of the body tab like the 2 is part of the checks tab"; the details (section 6, 15): mine |

"Not in this task" on the ticket is binding: no judging but at a send,
no multi-line checks, no check from a view or the jq bar, no variables
in a check, no on/off switch, names or sidebar marks, no file format
change, no second compact form, no unit in a time field, nothing for
#308, #310 or #282.

**Answers to review block 1 (2026-11-16, the ticket's comment 1400).**

1. A check that never ends keeps the response from showing. `you`:
   "This doesn't sound like an issue with this ticket. Why would we let
   checks run forever even in the cli? Each jq check should have a max
   runtime or it fails." Nothing is built for it here. The limit is its
   own task, #310, for `run`, `suite` and the app alike. (Since #312,
   section 7, such a check keeps only the Checks tab waiting.)
2. The `vars` mark sat after the last tab, so beside Checks. `you`: "the
   vars should be part of the body tab like the 2 is part of the checks
   tab." That is W3. The user plans to change this display later.
3. The branch's size. No cut: `mine (approved)`. `you`: "The limit is a
   target."

## 3. The design

The ticket's nine "How to build it" choices (all `mine`) hold against
the code. How each lands:

**Core (`postui-core`).** `checks.rs` gains what the CLI's
`cli/checks.rs` holds today and both sides need: the below-400 rule
(`FIRST_FAILING_STATUS`, `passes`), a check's line
(`Outcome::summary`), and the app's lines (`lines`: the outcomes'
summaries, led by `status N, expected below 400` when the 400 rule
failed the response; `passed`: every line passed). The CLI's
`ShownCheck` calls them. `model.rs`:
the read takes a jq filter without the whitespace around it and drops a
repeated status code or filter, keeping the first;
`Checks::parse_status` and `parse_limit` read a field's text by the
read's own limits, so the app's refusal and the file's are one rule.

**The send** (rewritten 2026-11-16 for #312, section 7; #309 held the
response until its checks were judged). The send's task takes the
editor's checks at the send and hands the response over as it arrives,
with those checks (`Action::ResponseArrived`). The app delivers it with
how its checks stand (`Session::arrived` takes a
`response::CheckState`, from `CheckState::on_arrival`): none, the
judged lines when no jq check has to read the body
(`ResponseData::check_lines`), or being judged. For the last the app
then starts the one job (`ResponseData::check_run`, through
`http::judge` and `http::run_checks`, the suite's hop too): it runs the
checks once on the blocking pool and sends the lines as
`Action::ChecksJudged` with the send's generation.
`Session::checks_judged` gives them to the
response that send made, on screen or in the session's cache, found by
its view's generation as a background parse finds it (`tree_arrived`);
when that response is gone they are dropped. The response keeps the
lines and not the outcomes, which hold every output of a jq check whole
(#309 review round 1). The job shares the response's body
(`ResponseData::body`, an `Arc`), so the checks copy no body, before
the response shows or after (#312 review rounds 1 and 2). The cost: the
body is read again when the jq bar is used on it. A jq check has no
time limit yet, so one that never ends leaves the tab on `checking…`
and keeps its thread and its read of the body: that limit is #310.

**The response tab.** `ViewMode::Checks`, offered unless the view's
`checks` (`CheckState`) is `None`. That one field is all the view keeps
of the checks; `CheckState::row` reads it as the tab's rows, each a
verdict and a text: the judged lines, or the one line `checking…`, which
has no verdict. The tab's text is each row led by its mark
(`ReadyView::check_text`, one line made per read; `check_texts` for all
of them): that joins every place `ViewMode::Headers`
supplies plain lines (line count, search corpus, copy, save, `$EDITOR`,
width). The paint draws the same rows with the mark in the verdict's
colour, and the tab's badge is `checks::passed` over the same verdicts,
or a dim `…` of the same width while they are judged; the mark and its
colour are `paint::verdict`, the Body tab's badge too. So the tab's
text is made on each read, a copy of its few lines, where Headers' is
borrowed (#309 review rounds 3 to 5). It has none of the Headers tab's
header copy pills or keys. The response side reads nothing from the
editor.

**The shared table (`table_editor.rs`).** Contract changes:

- `Stop::Head(k)`: a stop above the rows that the caller draws. The
  walk's `head: bool` becomes a head count. The Project tab's Name row
  is `Stop::Head(0)` (before: nothing selected), so the walk has one
  rule and "nothing selected" means the same on every table. ←/→ on a
  head are left to the caller, as they are with nothing selected, so
  the Name row hands them on as before (#309 review round 1). Heads are
  not content lines: with the cursor on one the view shows the table
  from its top (`settle_view_under_head` and `view_scrolled_under_head`
  fold into `settle_view` and `view_scrolled`).
- A head can be open in the table's one edit slot (`CellEdit`, whose
  `EditAt` is a row's cell or a head). The table never writes a
  head: closing one hands its text back (`TableOutcome::head_commit`),
  and a Tab walk that reaches a head asks the caller to open it
  (`TableOutcome::open_head`). The Project tab keeps its own Name edit
  and ignores both. So every reader of "a cell is open" (commit on
  click-away, save, send, tab switch, the undo bracket, paste, vim's
  field session, the mouse) meets a head as it meets a cell.
- `TableSpec`: what a table lists, handed to the walk, the commit, the
  click and the draw. Each thing the checks list differs in has one
  home there (#309 review round 5):
  - its words, written out as data: the noun (`check`), the header
    (`CHECK`), what a yank holds (`a check`), the two warnings whose
    sentence differs (a blanked name, a name merged into another row),
    and the add chip's hint;
  - `heads`, the three fields;
  - `name_only`, what a row is: its name alone, one line of text with
    no value and no on/off switch. What follows from that reads this
    one fact: one column drawn and walked, the name kept without the
    whitespace around it and quoted cut short, a copy that takes the
    name, and no second row made of one (a put of a check still in the
    list, a row menu without Duplicate or Extract value);
  - `vars`, whether its inputs take variables.

  `EditorTab::table_spec` gives a tab its spec. No caller asks which
  tab is up to pick a check's words or rules; the words the app sets by
  the tab are the other tabs' own, listed in section 4.

**The editor.** `EditorTab::Checks`. The status codes and the limits
are `Editor::check_status` and `check_limits`; the jq checks live in
`Editor::check_rows`, an `IndexMap` keyed by the check's text, so every
table path (add, delete, copy, put, undo's caret) is the one the other
tabs use. `Editor::checks` puts them together for a save and a send. A
check's text is the row's name: a blank one is refused and a repeat
merges, by the table's own rules, worded for a check. A check that
spans lines in a hand-written file is the row's name as read; unedited,
it is written back as read. The fields draw above the table and don't
scroll with it. An open field is `FieldId::CheckField` under
`Hit::CheckField`. A field's close parses its text with core's
`parse_status` / `parse_limit`; a refusal is a warning toast and the
old value stays (Q5). The table's keys, its commit and its click end
in `Editor::finish_table`, which writes a closed field and opens the
one a Tab walk reached. A delete drops an open input unwritten, on
every tab, as on `main`.

**Variables.** `Editor::caret_takes_vars` is the one question; it
answers no for the Checks tab's content. Its askers are listed in
section 4.

**The tab row.** `draw_tab_bar` lays the row out with counts; when its
end reaches the split chips it lays it out again without them. The Body
tab's label carries ` · vars` in both forms while the body's variables
are on (W3), and the row is measured with it, and with the Body tab's
badge, whether or not the tab has them (#309 review round 4).

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
chips (`PaneChipArgs`, `TableCursor::Checks`), `focused_extract_text`,
the row click and right-click paths in `mouse.rs`
(`resolve_table_row_across_commit`, the row hit, the right click's
cursor move, which a field takes too), `repeat_cap`; the Project tab's
`on_name`, `begin_name`, its draw and its wheel.

**The open cell (`TableEditorState::editing`).** By presence only, so a
head needs nothing: `Editor::plain_keys_type`, `load`,
`apply_snapshot`, `table_cursor_live`; `App::commit_table_edit`, the
dirty mark (`ui.rs`), `capture_undo`, `pane_field_open`,
`OverrideAutoHeader`, `landing.rs`, `repeat_cap`, `line_buf`, the
selection's copy, the paste. By what is open, which `EditAt` tells
apart (a row's cell or a head; #309 review round 2): the field's id
(`Editor::table_field`, asked by `screen_field`, `App::editing` and
`text_surface_menu`), and a cell's row (`CellEdit::cell`).

**Variables in a text input**, each asking `caret_takes_vars` (or, for
drawing, the table's spec): the `{{` picker (`Editor::handle_key`, vim's
`after_typed`), the picker by key, chip and palette and the selector
picker on a token under the caret (`Action::OpenVarPicker`, which a
picked token's insert can only follow), Extract to variable / selector
(palette and key through `focused_extract_text`; the row menu and the
text menu don't offer it), token colour and hover tips
(`paint_cell_tokens`), the caret's tooltip and its keys
(`Editor::caret_token`), the footer's `vars` chip.

**Words** (`TableSpec`): every message and hint that names or quotes a
row (#309 review rounds 4 and 5): the table's three warnings, the
delete toasts, the copy toast, the put's three warnings (a repeat, a
row yanked on another tab, a row put in the sidebar), and the hints of
a row's pills and chips, which read the spec through `HintCtx::table`
(`Copy this check`, `Delete this check`, `Add a check`). On the Params,
Headers and Vars tabs the three hints read as on `main`; their add
hint is `main`'s palette line, written out in the spec, since the
palette's own line now names Checks too. A field's refusal quotes what was typed cut short, as a
check is quoted, and a field's label and hint are one table
(`CHECK_FIELDS`, as long as the spec's `heads`). Beside it, by the tab,
where the other tabs' own fixed labels already are: the row menu's
noun (`param`, `header`, `variable`; the spec's for a check),
`+ Add check`, the footer's add chip (`EditorTab::add_row_label`) and
row chips (`TableCursor::Checks`). The response's toolbar hints name
the check results by `HintCtx::checks`, the Checks view being up.

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
4. The fields stay put above the list while the list scrolls. The
   split's floor (#129) leaves the list two lines; a window under 21
   rows has no floor and may leave it none, as the Body tab no text.
5. A scratch request that holds only checks counts as unsaved work.
6. The response tab's copy toast says "Copied check results", and its
   toolbar's hints name the check results.
7. In the app the lines are not masked: they quote the response body,
   which the app shows in the clear.
8. A put of a check that is still in the list adds none, with the
   warning a repeat typed into `+ Add check` gets; a count puts one.
9. A check's right-click menu is `Delete check` alone: no Duplicate (a
   list holds a check once) and no Extract. A right click on a field
   puts the cursor on it and opens no menu.
10. The palette lists its variable commands everywhere, as it lists
    every command; run with the caret in the Checks tab's content, each
    is refused with "a check takes no variables".
11. The read drops a repeated status code as well as a repeated jq
    filter, and a status typed twice in the field counts once.
12. The tab can be bound as `editor_tab_5`, unbound by default like the
    other four.
13. Tab or Shift-Tab out of a check that drew a warning (a repeat, a
    blank) ends on the check the warning names, as Enter does (round 2).
14. Checks that panic leave the response, with one failed line (round 2).
15. W3's details. The label reads `Body · vars`, with the body's `✓` or
    `✗` after it, in the label's own colour (the mark was in the accent
    colour). It is not a count, so the compact form keeps it. The row
    is measured with it whether or not it is on, so switching the
    body's variables never changes the form; the Checks tab moves by
    its 7 columns, as a tab moves when a count gains a digit.
16. The response is handed on after its checks, with no time limit on a
    jq check until #310 sets one (answer 1). Replaced by section 7
    (#312): the response is handed on as it arrives.
17. The tab row is measured with the Body tab's badge whether or not
    the body has text, as with `vars` (15), so typing in the body never
    changes the form. The counts are still measured as they are: they
    are what the compact form drops (W1). So at a width the row only
    just fits, a row added or a count gaining a digit drops every
    count. Measuring a fixed count for each tab instead would hide
    counts that fit on every main column narrower than the widest row
    (round 4).
18. A check's text is kept without the whitespace around it, typed (the
    table's commit) or read from the file, so `.ok` typed with a space
    after it is the check `.ok` and adds no second one. The jq bar runs
    its filter trimmed in the same way. A hand-written filter with
    whitespace around it is written back without it at the next save
    (round 5).
19. The read's dropped repeat (choice 6) and 18 change the file at the
    app's next save of that request. A save already writes the whole
    file from what the app holds, so it writes no comment back either,
    and neither a repeat nor the whitespace changed a verdict (round 5).

## 7. The response first, its checks after (#312, 2026-11-16)

Scope: board task #312, whose "What the user sees" list is the whole of
it. #309 held a response back until its jq checks were judged (choice
16), so a slow check delayed the response and a stuck one hid it. `you`:
"Why not show the response as soon as it's ready and show the checks
when they are ready".

### Added beyond the ticket

None.

### Rules

Sources are copied from the ticket: `you` is the user's own words,
`mine` the ticket's call, open to veto.

| # | Rule | Source |
|---|---|---|
| A1 | A response shows as soon as it arrives, whatever checks its request has. | you |
| A2 | When the request sent had a jq check, the response's Checks tab is there at once. Until the checks are judged its label carries a dim `…` where the `✓` or `✗` will go, so the label keeps its width and nothing on the row moves, and the tab shows one line: `checking…`. | mine |
| A3 | When the checks finish, the lines and the verdict take their place, all the lines together. No tab switches and no toast shows (P6). | mine |
| A4 | A request with only a status check or time limits is judged as it arrives and never shows `checking…`. | mine |
| A5 | The result belongs to its response. It lands there when another request is on screen and is there on the way back. A new send replaces the response, and the earlier send's result is dropped. | mine |
| A6 | Still judged once, with the checks as they were at the send. Editing a check re-judges nothing. | P5, Q8 |
| A7 | While the tab says `checking…`, search, copy, save and open in the editor act on that one line, as they act on whatever lines the tab shows (P7). | mine |
| A8 | A check that never finishes leaves the Checks tab on `checking…` and the rest of the response usable. #310 gives each check a time limit. | mine |
| A9 | Checks that crash give the one failed line they gave: `✗ the checks did not run: …`. | choice 14 |

The ticket's "Not in this task" is binding: no time limit or cancel for
a check (#310), no judging but at a send, no job per check or lines
that fill in one at a time, no sharing of the body's read with the jq
bar, no change to the request's Checks tab, `postui run` or `postui
suite`.

### The design

The ticket's five "How to build it" choices (all `mine`) hold against
the code. "The send" and "The response tab" in section 3 are rewritten
to them. What changes:

- **Removed:** `http::send_checked` and `ResponseData::checks`. The
  send's task calls `http::send` and hands the response over, as before
  #309.
- **One job per response.** Only a request with a jq check has one. The
  app starts it when the response is delivered, through
  `http::run_checks`, over the response's own body, which is shared
  (`ResponseData::check_run`; before review round 1 the send's task
  ran it over its own copy of the response, made before the response
  was handed over, and before round 2 the job rebuilt the body from
  the view's lines). Nothing tracks the job, so nothing cancels it: a
  new send of the request leaves the earlier job running, and its
  lines are dropped (detail 4 below). A send cancelled or replaced
  before its response is delivered starts none.
- **One action back,** `Action::ChecksJudged { generation, lines }`,
  matched as a background parse is: `Session::checks_judged` finds the
  response whose view was made for that generation, on screen or in the
  cache. A new send, a cancel and a project switch each leave no such
  view, so the lines are dropped with no guard beyond the generation. A
  deleted request's response stays in the session's cache, as on
  `main`, so its lines land on it there, and an undo of the delete
  shows them.
- **One field,** `ReadyView::checks`, a `CheckState`: `None`,
  `Judging`, `Judged(lines)`. `Session::arrived` takes the state the
  response starts in and sets it on the response it delivers, so the
  tab is there in the response's first frame. Nothing else on the
  response side holds the checks.

### Consumer audit

Every reader of `ReadyView::checks`, and what it does in each state.
All of them read it through `CheckState::row` and `len` (`None`: no
rows; `Judging`: the one row `checking…` with no verdict; `Judged`: a
row a line) or `CheckState::verdict`. The app starts the job when
`CheckState::on_arrival` answers `Judging`.

- **Presence:** `ReadyView::tab_modes` (the strip, the tab keys through
  `Response::tab_modes`, `set_view_mode`'s refusal of a tab not
  offered). Offered unless `None`, so it never appears or leaves after
  the response arrived.
- **Label:** `response_tab_defs`: the verdict's mark, or a dim `…`
  while judging. Each is one cell by the app's measure (`ui::cells`),
  so the tab's width, and with it the strip's and the toolbar's room,
  is the same in both. A terminal set to draw East Asian ambiguous
  characters two cells wide draws `…` wider than that measure, here as
  wherever the app cuts a text with `…` (review round 1; not handled).
- **Text:** `check_text`, behind `buffer_line` (Vim's cursor column
  and `%`), `display_line_text` (the mouse's selection and its copy)
  and `buffer_lines_of` (a yank); `check_texts`, behind `search_corpus`
  (search, `*`, and `view_text`: copy, save, `$EDITOR`) and
  `content_width` (the sideways scroll). While judging the one line is
  `checking…`, with no mark.
- **Count:** `visible_len` (the cursor's clamp, paging, the scrollbar):
  one while judging.
- **Paint:** `body_lines`: `checking…` in the muted colour, as the
  Headers tab's notes are.
- **When the lines arrive** (`Response::set_checks`): the tab's lines
  are other lines, so with the Checks tab up the view starts over on
  them as a tab switch does (`ReadyView::lines_replaced`, which
  `set_mode` now calls too): the cursor on the first line, the scroll
  at the top, the selection and the remembered column dropped, the
  search's matches found again. On another tab nothing visible changes
  but the label's mark. A cached width of the Checks tab is forgotten:
  its key (mode, line count) is the same for one judged line as for
  `checking…`. Another tab's is kept, and lines that land on a response
  that is not on screen ask for no draw (review round 2).
- **Words:** unchanged. The toolbar's hints and the copy toast say
  "check results" whatever the state (`HintCtx::checks`,
  `App::resolve_copy`).

A response shed or stashed while its checks run keeps its view, so the
lines find it by its generation as above.

### Tests

- The send (`tests/http_integration.rs`): a request with a jq check
  sent to a local server arrives judging, then its lines land; one with
  only a status check arrives judged. `http.rs`: the lines a sent
  response is judged into by a run that shares its body, and the one
  failed line of a run that panicked.
- Response: the three states' tab, label, text and paint; the tab's
  cells are the same judging and judged; lines landing with the Checks
  tab up reset the cursor, the selection, the search's matches and the
  width, and on another tab leave the cursor, the selection and that
  tab's width.
- Session: lines land on a cached response, a shed one included, and
  are dropped after a new send and a reset; only a landing on the
  response on screen asks for a draw.
- App, in both profiles: the response is ready with its tab on
  `checking…`; copy and search act on that line; `ChecksJudged` fills
  the tab with no toast and no tab switched. Lines for a request
  navigated away from and for a send since replaced (in flight,
  arrived, failed); a deleted request's are there when the delete is
  undone.

### `mine` details, for veto

1. `checking…` is drawn in the muted colour and has no mark.
2. When the lines arrive with the Checks tab up, the cursor goes to the
   first line and a selection made on `checking…` is dropped, as on a
   tab switch.
3. The check job reads the response's own body. `ResponseData::body`
   is an `Arc` for that, and the background Tree parse of a large body
   shares it too, where on `main` it had a copy. So while a large
   response's Tree parse and checks run, its body is held twice (the
   response's and the view's lines), where `main` held it three times
   during the parse and round 1's job a fourth. The job lets go of the
   body once jq has
   read it and keeps jq's document until its last check ends (review
   round 2).
4. Nothing ends a running check job. A new send of a request whose
   checks still run starts another job and leaves the earlier one
   running; its lines are dropped. With checks that end, the leftover
   jobs end too. With a check that never ends, every send of that
   request leaves one more thread busy and one more read of the body
   in memory until postui quits, up to the blocking pool's 512 threads,
   after which Tree parses and jq bar runs wait. A quit gives the
   terminal back at once, and the process ends when the running jobs
   do (seen in tmux in round 2 with a check that ends). `main` leaks
   the same way through Esc and a new send, since a cancelled send's
   checks run on. Nothing short of ending the run bounds this without
   a wrong result: a new job refused or skipped while an old one runs leaves a
   corrected check on `checking…`, and a job that gives up between its
   checks is still stuck in the one that never ends. Ending a run is
   #310's (review rounds 1 and 2).
