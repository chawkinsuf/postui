# The table tabs scroll (task #56)

## 1. The problem

The request editor's Params, Headers and Vars tabs draw an editable table
(`components/table_editor.rs`) and, on Headers and Vars, a read-only section
under it: the **auto** headers (since #21 the only place project defaults
such as `Authorization` show) and the **referenced** variables. None of it
scrolls (`table_editor.rs`: "The table never scrolls").
`Editor::draw_tab_content` gives the table its full height first and the
section only what is left, so on a short pane, or with many rows, the
section is cut short or gone, and only dragging the pane split brings it
back. Rows past the pane's bottom can't be reached either: they are not
drawn, so they can't be clicked.

The user, on #21: "No change. The issue is it can't scroll." (`you`)

Every other scrolling surface already has the parts this needs:

- **Wheel:** `Action::ScrollPane(PaneId::Editor, ±3)` already reaches
  `Editor::handle_scroll`, which answers only for the Body tab.
- **Scrollbar:** `hit::draw_scrollbar` + `ScrollbarSpec`. `App::drag_to`'s
  Editor arm already feeds a thumb drag to `Editor::handle_scroll`, and a
  track click pages through `ScrollPane`. The sidebar draws its bar in the
  pane's right margin column, so no row changes width when the bar shows.
- **The cursor and the view:** the sidebar and the Variable Manager's left
  list scroll freely under the wheel, and bring the cursor into view
  (scrolling as little as they can) only when the cursor moves.

## 2. Rules

The draft's rules were shown to the user with question Q1. S1–S6 and
S8–S9 were not vetoed and stand as `mine` (shown, not vetoed). The user
answered Q1 with a fourth option, which replaces S7 and amends S10 (§2a).

| # | Behaviour | Who |
|---|---|---|
| S1 | A table tab's content scrolls as **one column of lines**: the table (its NAME/VALUE header, rows, `+ Add` row, closing edge), then the section under it. The section keeps every row; nothing gives way to the table any more. Params has no section. | the scroll: `you`; one scroll for both parts: `mine` (shown, not vetoed) |
| S2 | The header row scrolls with the rows, as part of the one painted table control. | `mine` (shown, not vetoed) |
| S3 | The wheel over the editor pane scrolls it 3 lines a notch, like every pane. The view stops with the last line on the bottom row. | `mine` (shown, not vetoed) |
| S4 | While the content overflows, a scrollbar shows in the pane's right margin column, where the sidebar puts its own, so no row changes width when it comes or goes. The thumb drags and the track pages, like every pane's bar. | `mine` (shown, not vetoed) |
| S5 | The wheel and the scrollbar move only the view. The cursor stays on its row, even off screen. *Amended after review round 2:* in the vim profile a cursor the view leaves moves onto the nearest stop in view, as Vim's window does and as the vim response pane already does (`cursor_into_view`), so a key never acts on a row out of sight. The arrows profile keeps the sidebar's rule. | `mine` (shown, not vetoed); the vim half is the user's rule "follow what Vim does" (`you`) |
| S6 | When the keyboard cursor moves to another stop, the next frame scrolls as little as it can to show it. That covers every way the cursor moves: keys, a click, undo, paste, add, the auto row's pencil. *Amended after review block 2:* the view keeps the cursor in sight every frame (S22) unless the arrows wheel scrolled it away (S23), so nothing is compared. *Amended after review block 3:* once the arrows wheel has scrolled the view away, only a cursor move brings the view back; `y`, `ctrl+s`, Space and toggle clicks leave it where the user scrolled. | `mine` (shown, not vetoed); the stateless rule `you` (§3); the cursor-move clear `mine (approved)` ("R is fine") |
| ~~S7~~ | ~~On the `+ Add` row the view also shows the section below.~~ Replaced by S11: the section's rows are stops of their own, so S6 shows them. | |
| S8 | An open cell never leaves the view. The wheel and the bar stop once its row reaches the top or bottom edge. | `mine` (shown, not vetoed) |
| S22–S25 | The view keeps the cursor in sight every frame; the arrows wheel's `scrolled_away` flag is the one exception, cleared by the cursor's setter when the stop changes (not where input enters: that classified inputs and was replaced, block 3); vim drags the cursor; a stale stop is clamped once where the table's keys come in. See §3. | the rule: `you` ("keep it in sight", "why do we need to store a copy at all"); the setter clear: `mine (approved)` ("R is fine"); the flag's name: `mine` |
| S26 | *Added after review block 4, rounds 4–5:* clamping a stale stop is not a cursor move: it writes the stop that is already drawn lit, so it leaves a scrolled-away view where it is. A wheel or bar scroll while the pane has no viewport (0 lines) scrolls nothing and marks nothing. Delete on a cursor the view was scrolled away from leaves the view where it is (the same class as Space in S6: the toast names the row, undo brings it back). | `mine` |
| S9 | A tab switch and opening a request start the view at the top. *Amended after review block 2, round 1:* a load keeps the row selection, as it always has (Save As lands on the content it was made in), so when a cursor is kept the next frame shows it (S6) rather than the top. | `mine` (shown, not vetoed); the amendment `mine` |
| S10 | The page keys move the cursor through every stop, section rows included (S11): Home/PageUp/ctrl+b/`gg` go to the first row, End/PageDown/ctrl+f/ctrl+End/`G` to the last stop, ctrl+d/u half the stops. | `mine` (shown, not vetoed), amended by S11 |

### 2a. The section's rows are cursor stops (amended 2026-10-25, Q1)

The user: "if the cursor can go to the lower rows it would be strange to
not let the down arrow go there too", and "the add isn't the last row if
we can go into the auto rows is it? Letting the cursor go to the auto rows
also means the keyboard can access the context buttons there" (`you`).

| # | Behaviour | Who |
|---|---|---|
| S11 | Each row of the section is a cursor stop after `+ Add`. ↓/`j` from `+ Add` goes to the first section row (the divider is not a stop), ↑/`k` comes back, and the last section row is the last stop. ↓ there does nothing, as ↓ on the last stop does today. | `you` |
| S12 | The Vars tab's referenced rows are stops by the same rule. They are the same kind of read-only list under the table, and the user's first reason holds for them. They have no buttons, so no keys act on them. | `mine` |
| S13 | A section row is read-only: no cell opens on it. Under the focused cursor it takes the table's cursor-row fill, so the cursor reads the same on every stop. | `mine` (read-only was the coordinator's brief, not the user's words) |
| S14 | **Copy:** on an auto row, the table's yank keys (arrows `y`, vim `yy`) copy the row's value to the clipboard, as its copy pill does (the real value, masked or not; spec 2026-10-18 R7). The yank register is untouched, since the pill doesn't fill it either. | `mine` (yank is the table's copy letter; the response Headers tab's arrows `c` was the other candidate) |
| S15 | **Take over:** on an auto row that has the `󰏫` pill, Enter does what the pill does (#20): it adds the header to the request and opens its value. Whatever reaches the table as Enter (vim's `o` and `i`) does the same. | `mine` (Enter opens a row everywhere else; the pill has no key of its own) |
| S16 | **Footer:** with the cursor on an auto row the footer offers `copy` (its yank key) always, and `edit` (enter) only on a row with a pencil. The editable row's toggle and delete chips don't show. The add chip stays, since `a` still adds a row. | `mine` (keyboard context actions; footer keys only) |
| S17 | The footer's centre line already speaks for the focused table's cursor row (spec 2026-10-18 §7). On a client-owned auto row it now shows `Host cannot be set manually`, as hovering the row does. | follows from S11 (spec 2026-10-18 §7, `mine`) |
| S18 | A click on an auto row is unchanged (the bare pane's, spec 2026-10-18 §7), and doesn't move the cursor there. The pills keep their clicks. | `mine` |
| S19 | The divider's `reveal` toggle stays mouse-only. The divider is not a stop and the toggle has no key today. | `mine` (out of scope) |
| ~~S20~~ | ~~An undo or redo that clamps the cursor to a table row clamps a section-row cursor the same way, to the last real row.~~ *Replaced after review block 1:* an undo or redo puts a cursor that was on a section row back on that section row (`Caret::Section`), clamped to the section's last row if it shrank. `+ Add` keeps today's clamp to the last real row. *Amended after review block 3 round 2:* on the Vars tab the section row is found by its variable's name, as `Caret::Cell` finds a row by its key (the referenced list is rebuilt in first-seen order, so an undo that removes an earlier `{{token}}` shifts the index); and an undo or redo that brings a row back puts the cursor on it from a section row too, as it does from a table row. *Amended 2026-10-26 (task #121):* `+ Add` stays on `+ Add` unless the swap brings a row back, and a cursor whose row or variable the swap removed goes to the spot the undo step recorded (the undo spec, R8 amended). | the rule: `you` ("undo restores exactly"); the old clamp was `mine`, raised by review round 3; the amendment `mine` (coordinator's brief) |
| S21 | A key whose action doesn't apply on a section row does nothing, as the same keys do on `+ Add` today: Delete/`d`/`dd`, Space, ←/→ (*amended after review block 1:* they used to move the remembered cell column there), alt+↑↓, Enter on an auto row without the pencil, and every key but the moves on a referenced row. The footer offers only the keys that apply (S16). `p`/`P` put the yanked row at the table's end, as on any stop. | `mine (approved)` (Q2, option A); ←/→ `mine` |

## 3. Design

- ~~`TableEditorState` gains `scroll` and `followed` (the cursor stop the
  last frame brought into view), and the draw compares the cursor with
  `followed` to notice that it moved.~~ *Replaced after review block 2 (a
  rework). The user: "Yes keep it in sight. Restart the review. Why do we
  need to store a copy at all that can get out of sync." (`you`).* Rounds
  kept finding cases the stored copy missed (rows moving under a kept
  stop, a pane resize, the vim wheel), one guard per round. Now
  **nothing derived is stored**:
  - `scroll` (where the user scrolled, which can't be derived) and
    `scrolled_away`, a plain flag, are the view's only state. `reset` (every
    tab switch) and `Editor::load` clear both (S9).
  - **S22. Rule 1:** every frame, unless `scrolled_away` is set, the view
    keeps the cursor in sight, recomputed from the cursor and the layout
    (the Variable Manager grid's stateless rule). That covers every way the
    cursor moves (S6), rows moving under it, a pane resize, and a stale
    stop. An open cell is always kept in sight, flag or not (S8).
  - **S23. Rule 2 (arrows):** a wheel or scrollbar scroll that moves the
    table's view sets `scrolled_away` (one that moves nothing, at a limit
    or on content that fits, doesn't), and the view keeps its offset while
    it is set (S5). **The flag clears in one place: the cursor's setter.**
    `TableEditorState::selected` is private, every write goes through
    `set_selected`, and a write that changes the stop clears the flag,
    comparing the field at the moment of the write. Whatever moves the
    cursor (a key, a click, undo, paste, the pencil, a footer chip)
    therefore brings the view back, and nothing that leaves it in place
    does. (`mine (approved)`: the user, "R is fine".) *Cost, which the user
    saw:* `y`, `ctrl+s`, Space and toggle clicks leave the view where the
    user scrolled it; only a cursor move brings it back. *Amended after
    review block 4 round 1:* a move key counts as a cursor move even when
    it lands on the stop it is on (End on the last stop, ↓ at the end), so
    the key pressed to find the cursor isn't dead (`mine`).
    ~~The flag cleared where input came in (keys while the table held the
    keyboard, presses on the table's rows).~~ *Replaced after review block
    3 round 2:* that boundary classified input events, and each round found
    another input it misjudged (URL-line keys and auto-pill clicks, then
    toggle and copy clicks, keys that bring the keyboard into the table,
    and keys typed into a dialog). The boundary was the coordinator's
    instruction, and it was the wrong shape: whether the view should come
    back depends only on whether the cursor moved, which only the cursor's
    writer knows.
  - **S24. Rule 3 (vim):** the vim profile never sets the flag. Its wheel
    and bar drag the cursor onto the nearest stop in view (S5), and rule 1
    keeps it in sight. *Cost:* a 1–2 line pane can't rest showing only the
    closing edge or the divider, because the view pulls back to the
    nearest stop. That is Vim's rule, and it reverses review block 2 round
    1's fix #3.
  - **S25. Rule 4:** a stale stop (a row or section row the rows went from
    under) is clamped once, at the top of the table's key handling, so
    every arm (the moves, Enter, Space, Delete, ←/→) sees the same stop. The
    draw clamps it too, so a frame without a key shows it in range.
- ~~**One index for every stop.** `table.selected` keeps its meaning for
  `0..=len`, and `len + 1 + j` is section row `j`.~~ *Replaced after review
  block 1 (a rework, `mine (approved)`: the user, "let's just do B now and
  get it over with since it sounds like the better path"):* each review
  round found another reader that assumed the index stops at `len` (the
  vim count cap, undo's caret). **`TableEditorState::selected` is an
  `Option<Stop>`**, `Stop::Row(i) | Stop::Add | Stop::Section(j)` (the
  shape `mine`), so every reader matches each kind and the compiler finds
  them. A stop's position among the stops (its ordinal: rows, `+ Add`, then
  the section) is used only inside the table's moves (↓, ↑, the page
  keys), never by a reader. A click resolves its hit's row index to a stop
  at the edge it comes in at (`Stop::of_row`). The cell under edit keeps
  its row index (`CellEdit::row`, `len` for `+ Add`): only the cursor
  changes type. Undo's caret gains `Caret::Section(j)` (S20).
- The vim wheel's cursor-into-view (S5) finds the first or last stop whose
  line (`stop_line`) is in view, by search, so the line layout has one
  source.
- The Vars tab's referenced names: see §5 Q3.
- `TableEditorState::draw` takes a `skip` (content lines above the view) and
  draws line `L` at `area.y + L - skip`. Lines it doesn't draw register no
  hits, so clicks and the caret math, which read hit rects back, need no
  change. The section painters (`draw_computed_headers`,
  `draw_referenced_vars`) take the same `skip`, less the table's height,
  and paint the cursor row (S13).
- The three table arms of `Editor::draw_tab_content` become one helper. It
  settles the offset (S6, S8, then clamps to the content), draws the table
  and the section, and draws the bar in the margin column one past the
  content area (`pane_inner`'s right inset).
- `Editor::scrollbar_spec` and `Editor::handle_scroll` answer for the table
  tabs too: offset `scroll`, content the table's height plus the section's,
  viewport the last drawn height. `App::drag_to` and `ScrollPane` need no
  change beyond comments.
- `table_verb_action`'s `Yank` on an auto row resolves to
  `CopyToClipboard(CopyTarget::ComputedHeader(j))` (S14). Enter on an auto
  row resolves to `OverrideAutoHeader(j)` when the row has a pencil (S15).
- Removed: the "table first, section gets what's left" height split in the
  Headers and Vars arms, and the "never scrolls" comment on the page keys.

## 4. Tests

- `table_editor`: a skipped draw puts row `i` at `y + 1 + i - skip` and
  registers nothing for the lines above (S1). ↓ from `+ Add` with a section
  of 2 goes to `len + 1`, then `len + 2`, then stays; End lands on
  `len + 2`; ↑ comes back (S10, S11).
- `editor`: on a short area with many headers and auto rows,
  `handle_scroll` brings the last auto row into view and stops at the end
  (S1, S3). `scrollbar_spec` reports the content and viewport, and the bar
  is drawn in the margin column with the rows' width unchanged (S4).
- `editor`: ↓ past the bottom scrolls one line, and a wheel scroll then
  survives the next draw (S5, S6). ↓ onto the last auto row shows it (S6,
  S11).
- `editor`: with a cell open, the wheel stops at its row (S8). A tab switch
  and a load start at the top (S9).
- `app`: `ScrollPane(Editor, n)` scrolls the Headers tab, and a thumb drag
  on the editor's bar moves it (S3, S4). On an auto row, the yank key
  copies the value through the test clipboard sink (S14). Enter on a row
  with a pencil adds the request row and opens its value; Enter on a
  section row never opens a cell there (S13, S15). The footer chips
  (S16) and the client-owned message (S17).
- tmux: a short editor pane with defaults and several header rows; the
  wheel, the bar and ↓/End reach the auto section; `y` and Enter on an auto
  row.

## 5. For the user to decide

**Q1 (answered 2026-10-25):** the user picked none of A–C. Their answer
is §2a.

**Q2 (answered 2026-10-25: A, silent; now S21):** what a key does on a
section row when its action doesn't apply.
That means Delete/`d`/`dd`, Space (toggle), alt+↑↓ (move), `p`/`P`, and
Enter on an auto row without a pencil (a struck row, `Host`, the URL's
`Authorization`, a header the request already names). It also means every
key but the moves on a referenced row.

- **A (recommended): silent,** as those same keys are on `+ Add` today
  (`table_verb_action` answers `None`, and `OnSelection` does nothing). The
  footer offers only the keys that apply (S16). A client-owned row already
  says why in the footer (S17). `p`/`P` stay what they are on any stop:
  they put the yanked row at the table's end. *Cost:* a key on a row
  without the pencil gives no feedback beyond the missing chip.
- **B: a toast saying why,** as the jq views menu does for a row without
  that button ("on a row without that button the key says so"). *Cost:*
  new wordings, one per reason a row has no pencil (overridden,
  client-owned, URL credentials, named by a request row) plus delete,
  toggle and move, and the + Add row's keys would then disagree with the
  section's unless they toast too.

**`mine` choices shown with Q2 and not vetoed:** S12 (referenced rows are
stops), S13's cursor fill, S14's keys (yank letters rather than `c`), S15
(Enter takes over), S18 (a click doesn't move the cursor onto an auto
row), S19 (reveal stays mouse-only). They stand as `mine` (shown, not
vetoed).

**Q3 (answered 2026-10-25: A, `mine`: the instruction was the
coordinator's, not the user's, and the user may still veto it): the Vars
tab's referenced-name scan.** As built: no cache. The draw scans once
(the layout pass once more, as on main). Every key on a closed
table scans twice, whether or not it moves: once for the table's moves (the
count of stops past `+ Add`) and once for the router's count cap. Typing in
an open cell scans for neither. A wheel notch scans once, or twice in vim (the
cursor into view), and a thumb-drag step scans twice, or three times in
vim. The cost is about 14 ms a scan on a 4.9 MB body, beside the ~20 ms of
body copies each event already makes, and under 0.3 ms below 100 kB.
The coordinator asked for the names to be worked out when the body
changes and kept with the body, so keys, the wheel and the thumb read a
value that is always current, rather than a count stored at draw time.
What the code shows:

- The body has no single place it changes. edtui's key and mouse handlers
  edit it in place, the editor writes its lines directly (paste, indent,
  format, minify, `$EDITOR`), `set_body_text` replaces it, and the vim
  seam edits it too: about thirty call sites between `editor.rs` and
  `vim_seam.rs`.
- Measured (release, a 4.9 MB body, `substitute_body` on): one scan is
  14 ms. One `current_request()`, which the app already runs several
  times per event (the dirty check per frame and per action, the auto
  header recompute, the footer), is 5 ms.

Options:

- **A (recommended): no cache.** Scan when asked, once per input event on
  each path. Cost: on that 4.9 MB body, about +14 ms a key, +14 ms a
  wheel notch and +28–56 ms a thumb-drag event, beside the ~20 ms of
  body copies each event already makes. Under 0.3 ms below 100 kB.
- **B: a revision counter kept with the body,** bumped at each of those
  thirty sites, and the names recomputed when it moves. Reads are O(1),
  but a missed site leaves the count stale, which is what the coordinator
  wanted to avoid.
- **C: a cache checked against a stored copy of the body.** Always exact,
  but the check is itself O(body) (about the cost of a copy), so it saves
  roughly two thirds of each scan, for a second copy of the body kept in
  memory.
