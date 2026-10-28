# Manage cursors by name (task #83)

Branch `manage-cursors-by-name`, restarted from main on 2026-10-27. The
first attempt (cursors followed by index around the reload) is kept at
the tag `manage-cursors-by-name-history`; its two review rounds are on the
task. Review rounds start again at round 1.

**Approved 2026-10-27.** The user: "I meant the full fix. Spec is good."
They confirmed that option 1 meant this by-name design (the full fix),
not the board's earlier follow-at-reload recommendation, so N1 is `you`.
The `mine` choices below are now `mine (approved)`, choice 5 (N10)
included.

## 1. Findings

- **F1 The cursors are bare indices.** The Variable Manager's option grid
  keeps `OptionGridState::cursor: (usize, usize)` (`varmanager.rs`, row
  and column). The Manage Environments and Spaces lists keep
  `ManageList::cursor: usize` (`manage_list.rs`), one live and one parked
  (`Manage::list` / `parked`, `manage.rs`). Both fields are `pub`, and the
  keys, the mouse code, the draws and the tests write them directly.
- **F2 The symptom (task description, reproduced by a test on main):**
  with `var_project`, open the `user` selector, put the grid cursor on
  (1, 1) (bob), rewrite `environments/qa.toml` to hold bob then carol
  (alice gone), then `on_focus_gained()`. The cursor is still (1, 1),
  which is now carol, so the next Enter, space or `dd` (`dd` never
  confirms) acts on carol. The Environments and Spaces lists do the same
  when an outside edit adds an item above the cursor or reorders the
  list. The first attempt reproduced this with failing tests (tag, `tests.rs`).
- **F3 Where the information is lost.** A cursor is set at a moment
  that knows the item: a key steps onto a row the frame drew, or a click
  lands on a row's hit. Storing only the index throws the name away right
  there, so every later change to the list can only guess. The first
  attempt guessed at one place (the reload's `poll`). Round 1 found that
  the reload's close of an open cell writes, and re-reads, before that
  place. Round 2 found that the guess was wrong for the close's own
  writes, and that every Manager write re-reads the file it saves into,
  so outside edits reach the lists at every write, not only at the reload
  (task comment, "Paused after review round 2"). If the cursor keeps the
  name where it is set, no path has to guess.
- **F4 Behaviours today that work only because the index stays put**, which
  the design must keep with explicit writes:
  - Deleting an item leaves the cursor on its neighbour: the same index,
    clamped (`App::clamp_list` after `DeleteEnv` and `DeleteSpace`, and
    the grid draw's clamp). The ghost row counts as a row in the grid, so
    deleting the last option leaves the cursor on the ghost row.
  - Creating an option from the ghost row leaves the cursor on the new
    option. Options are appended, so the new one takes the ghost's index
    (`commit_grid_edit`).
  - Renaming an option, environment or space keeps the cursor on it,
    because its index does not change.
  - Undoing a grid `dd` lands on the restored option only because the
    index matches. `undo::View` records no grid row.
  - A parked list's cursor clamps when its list shrank while it was away
    (2026-09-27 spec, R7).
- **F5 Readers and writers today.**
  - Grid, writes: `handle_grid_focus_key` (Up/Down/Home/End/Tab/Left/
    Right), `start_cell_edit`, `select_row` and `sync` (reset),
    `mouse.rs` (right-press on a cell or radio, cell click, radio click,
    copy and trash clicks).
  - Grid, reads: `draw` (clamp, highlight, keep-in-view),
    `select_entry_action`, `grid_verb`, Enter, F2, and through
    `verb_action` the footer chips and the router's rows.
  - List, writes: `step` (every move key), `select_name`, `clamp`,
    `begin_drag`, `drag_to_row`, `drag_reset`, and `mouse.rs` (right-press
    and click on a row).
  - List, reads: `selected` (so `manage_selected`, `verb_action`, chips,
    `enter_detail`, `undo` views), `draw` (clamp, highlight,
    `ensure_visible`), `draw_right`.
  - Tests set the fields directly at about 60 sites (`app/tests.rs`,
    `key_matrix_tests.rs`, `key_invariant_tests.rs`, and the unit tests in
    `varmanager.rs` and `manage_list.rs`).

## 2. Rules

| # | Behaviour | Who |
|---|---|---|
| N1 | Each cursor holds the name of the item it is on: the list cursor holds the environment or space slug, and the grid cursor's row holds the option's name, or the ghost row. An outside edit that renumbers, adds or reorders items leaves the cursor on its item, with no code that follows it. | `you`: "Maybe restart the branch with option 1" (option 1 as it was put to you: cursors remember their item by name, not row number), confirmed: "I meant the full fix. Spec is good." |
| N2 | The cursor is state its owner keeps (`ManageList`, `VarManager`). Every read and write goes through the owner's methods and the existing paths (the op arms, `land`, the undo `View`). No copy is taken around the reload, and nothing writes to it on the side. | `you`: "reads and writes must follow the proper pattern" |
| N3 | Undo puts the cursor back exactly: undoing a grid `dd` lands on the restored option, and undoing a create, rename or delete in either list lands where the cursor was before the op. | `you` (the undo-restores-exactly rule) |
| N4 | The ghost row is its own place, not "past the last option". An option added on disk leaves a cursor on the ghost row on the ghost row. | `mine (approved)` |
| N5 | A cursor whose item is gone (deleted or renamed by another program) is on nothing. Nothing is highlighted, the detail pane shows its "Select an environment" / "Select a space" hint, and Enter, space, `dd` and the chips act on nothing. The next move key starts above the first row, so ↓ (or ↑) lands on the first row and End on the last. | `mine (approved)` |
| N6 | A list cursor that is on nothing, because it was never set or its item went while the list was off screen, settles on the first item when the list comes on screen: when the Manage screen opens and when the tab switches to it. The grid does the same when the keyboard enters it (`enter_detail`). This replaces "a parked cursor clamps" (2026-09-27 R7). | `mine (approved)` |
| N6a | The grid also settles when its selector opens in the detail pane (a list move or click onto a selector, and `sync` reopening the pane on a neighbour), which is where its state is reset today. Amendment 2026-10-27, found in implementation: space from the left list selects the grid cursor's option (`handle_key`), and with the grid settling only in `enter_detail`, a freshly opened selector's space would select nothing, where main selects the first option. Settling on open keeps main's behaviour, so this is no change the user sees. *Amended 2026-10-27, review round 2:* the open settles on the first *option*; a selector with none there (none yet, or no environment) opens on nothing, not on the ghost row, because a ghost the open stored would stay put once options arrived (N4), where main lands on the first option. Entering the grid still settles on its first row, the ghost when it is the only one (N6). The cost: space from the left list on a selector opened while it had no options selects nothing until the keyboard has been in its grid; main selected the first option there. | `mine` |
| N7 | An app write that removes the cursor's item moves the cursor to the item now at that index: the last item when it was the last one, nothing when the list is empty, and in the grid the ghost row when it was the last option. This is today's neighbour rule, done by the delete itself. A delete of another item leaves the cursor on its own item (today it can shift onto a neighbour). | `mine (approved)` (keeps main's behaviour) |
| N8 | An app write that renames the cursor's item moves the cursor to the new name, in its own list (live or parked) whether or not the Manage screen is open. The jump that `RenameEnv` and `RenameSpace` already make onto the renamed item while the screen is open is kept. | `mine (approved)` (keeps main's behaviour) |
| N9 | An app write that adds options while the grid cursor is on the ghost row (a ghost-row create, a paste) moves the cursor onto the first option it added. With the cursor on an option, an add leaves it there. *Amended 2026-10-27, review round 4:* an add with the cursor on nothing lands on the option it added too: a selector opened with no options (N6a) has its cursor on nothing, where main's index sat on the ghost row and a paste landed on the new option. | `mine (approved)` (keeps main's behaviour) |
| N10 | An environment switch keeps the grid cursor's option name. If the new environment has an option of that name, the cursor is on it. If not, the cursor is on nothing (N5). | `mine (approved)` |
| N11 | The grid's column stays an index (name, field 1..n, description). | `mine (approved)` |
| N12 | During a Manage row drag the cursor holds the dragged item's name, so it rides with the row. A drag cancelled by the reload leaves the cursor on that item in the order on disk. | `mine (approved)` |
| N13 | A cursor whose item moved off screen (an outside reorder) is not scrolled into view until a key moves it, as with the wheel today. | `mine (approved)` |
| N13a | The grid keeps its own scroll rule: its draw keeps the cursor's row (or the open cell's) in view on every frame, as on main, now from the row the name is at. Clarification 2026-10-27: N13 is the Manage lists' rule; the grid never had a "not until a key" scroll. | `mine` |

## 3. Design

### State

```rust
// components/varmanager.rs
pub enum GridRow { Named(String), Ghost }
pub struct GridCursor { row: Option<GridRow>, col: usize }   // fields private
pub struct OptionGridState { cursor: GridCursor, pub editing: Option<GridEdit>, pub scroll: usize }

// components/manage_list.rs
pub struct ManageList { cursor: Option<String>, /* the rest unchanged */ }
```

`None` means the cursor is on nothing (N5, N6). A name that the list does
not hold also means nothing. Neither case needs another state.

### The contract, before and after

| | Before | After |
|---|---|---|
| Grid read | `vm.grid.cursor` | `VarManager::grid_at(ctx) -> Option<(usize, usize)>`: the row the name is at now (the ghost is `options.len()`) and the column, or `None` when the cursor is on nothing. The draw, keys, verbs, mouse comparisons and tests all read this. |
| Grid write | `vm.grid.cursor = (r, c)` | `VarManager::point_grid(ctx, r, c)`: stores the option at row `r` of the list now (or `Ghost`) and column `c`. Keys, `start_cell_edit` and the mouse arms write through this. `settle_grid(ctx)` in `enter_detail` (N6), and where the pane opens a selector (N6a): `select_row` and `select_name` take `ctx` for it, and so does `sync`'s reopen. |
| Grid, app writes | none: the index stayed put | In `App::apply_var_struct`, the one path every option op takes (the `VarStruct` arm and `commit_grid_edit` both call it), after `Ok`. `NewOption` / `PasteOption` with the cursor on the ghost: the cursor goes to the option now at the ghost's old index (N9). `RenameOption` with the cursor on `from`: it goes to `to` (N8). `DeleteOption`: the option's index is read before the write, and if the cursor was on it, the cursor goes to that index (N7). Each one is a `VarManager` method. *Amended 2026-10-27, review round 1:* the write re-reads the file it saves into, so another program's edit can land with it and an index read before the write can name another option after it. `VarManager::grid_target` names the cursor's destination before the write instead (a delete's next option or the ghost row, a create's name, the name a paste takes by core's `pasted_option_name`, a rename's new name) and `land_grid` puts it there after. Same rules, by name. *Amended 2026-10-27, review round 3 (option 1, `you`: "Option 1 for 83"):* the two cell walks that go on after a commit's write read their row from the by-name cursor after the write: the ghost row's walk-on opens the created option's first field, and Tab/BackTab in a cell (`step_grid_edit`) steps from the committed option's row. |
| List read | `list.cursor`, `items.get(cursor)` | `selected(tab, ctx)` (same signature). `cursor_in(items) -> Option<usize>` for the draw (in the drag's working order or the project's list) and for `step`. |
| List write | `list.cursor = i` | `select_name` (unchanged) and `select_index(tab, ctx, i)` for clicks. `step` resolves, moves and stores the name. `settle(tab, ctx)` replaces `clamp`, and `App::settle_list` replaces `App::clamp_list` in `switch_manage_tab_state` and in `OpenManage` (N6). |
| List, app writes | `clamp_list` after a delete | `DeleteEnv` / `DeleteSpace` read the item's neighbour by name before the delete (`ManageList::neighbour`: the next item, else the one before) and call `removed(name, next)` on that tab's own list (N7). *Amended 2026-10-27, review round 1:* by name rather than by index, since an environment delete re-lists the files on disk. `RenameEnv` / `RenameSpace` call `renamed(from, to)` on the tab's own list (N8) and open with `begin_list_op(tab)`, as create and delete already do, so both undo views record the list's row (N3). Their jump onto the renamed item happens while that list's own tab is on screen, on that list (`select_list_row`). Main jumped in whichever tab was open (`manage_select_name`), which with the Variables tab up could put an environment's index into the Spaces cursor; with the item's own tab up, the jump is main's. |
| Drag | `begin_drag`, `drag_to_row` and `drag_reset` write the index | `begin_drag` stores the dragged item's name. `drag_to_row` and `drag_reset` stop touching the cursor (N12). |
| Undo | `View { space, open, cursor, row }` | `View` gains `grid: Option<GridSpot { selector, row: GridRow }>`, recorded by `view_on` while the Manager's detail shows a selector and its cursor is on something. *Amended 2026-10-27, review round 4:* `GridSpot` also records the environment whose options it was in (`None` for a shared selector), and a landing applies only while the grid shows that environment: an undo made after an environment switch restores an option of the recorded environment, and landing on a same-named option of the new one would be another option. A project switch also starts the Manage lists over (`Manage::reset_lists`), as #179 does for the Variable Manager, since their cursors now hold the old project's slugs. `Landing` gains `grid`. `land` step 6 puts it back through the `VarManager` when its detail shows that selector. `after_replay` lands it when `before.grid != after.grid`, as it does for `row` (N3). |
| Reload | (the first attempt captured and followed the cursors in `ReloadProjectFiles`) | `ReloadProjectFiles` does not change. Nothing reads the cursors around the `poll`. |

### Removed

`ManageList::clamp`, `App::clamp_list`, the row clamps in both draws and in
`handle_grid_focus_key` (the column clamp stays), the cursor writes in
`drag_to_row` / `drag_reset`, and the "same index, clamped" doc lines.

### Not changed

- The Variable Manager's left list. It follows its rows by `rebuild_rows`
  (#178).
- `GridEdit::row`. An open cell is never re-read under: inputs close
  before a reload and before every op (#75, #78), so its index cannot go
  stale while it is open.
- Actions and hits that carry a row index (`CopyOption { row }`,
  `StartOptionNameEdit { row }`, `Hit::VmEntryCell`, `Hit::ManageRow`).
  Each one is built from an index resolved in the same frame or event it
  is applied in.

  *Note 2026-10-27, the round-3 sweep:* one exception, left as on main. A
  click on a grid row while a cell is open closes the cell first (the top
  of `on_hit`), and that close writes, re-reading the env file. If another
  program edited the file meanwhile, the hit's row index can then name a
  different option than the one drawn under the pointer (a cell to open,
  a radio, the copy or trash button). Fixing it means the grid hits carry
  identity, which this choice keeps out; reported to the user. Review round
  4 found the same for the grid row's context menu: its `Copy option` and
  `Rename` items carry the row index from when the menu opened, and a
  reload while the menu is open can renumber the options under them
  (`Delete` carries the name). Same class, left as on main, reported.
- The sidebar cursor (its own R6 rules).

### Spec amendments when the code lands

In the 2026-09-27 spec, R7's "A parked cursor clamps if its list shrank"
and "Deleting a row lands on the neighbour: the same index, clamped" each
get a dated paragraph naming #83 and N6 / N7.

### Plan (one sequence)

- [x] Failing tests first (§4).
- [x] Grid: the `GridRow` / `GridCursor` state, `grid_at` / `point_grid` /
  `settle_grid`, readers and writers moved onto them, and the draw and key
  clamps removed.
- [x] Grid app writes in `apply_var_struct` (`VarManager::follow_option_op`).
- [x] List: `Option<String>` state, `cursor_in` / `select_index` / `settle` /
  `removed` / `renamed`, the drag, and `App::settle_list`.
- [x] Undo: `View::grid`, `Landing::grid`, `land` step 6, `after_replay`,
  and `begin_list_op` for the renames.
- [x] Tests that set the fields directly move to the owner's methods.
- [x] The R7 amendments.
- [ ] Full suite, clippy, fmt, and a tmux check: an outside reorder with the
  grid and with each list open, `dd` then undo, a ghost create.
- [ ] Review rounds.

## 4. Tests

| Rule | Test |
|---|---|
| N1 | The task's repro: an outside rewrite that drops alice leaves the grid cursor on bob (now row 0). An environment added above the selected one on disk leaves the list on it. An outside space reorder leaves the live list, and a parked one, on their spaces. A reload with a grid cell open (the close saves into the re-read file) leaves the cursor on its option. Ported from the tag. |
| N2 | Outside the owner's module, no code or test sets or reads the cursor fields directly. The fields are private, so the compiler enforces it. The owner's own unit tests use the same methods. |
| N3 | Grid `dd` then undo: the cursor is on the restored option. Redo: the cursor is on the neighbour. Undo of a ghost-row create lands on the ghost row. Undo of a grid rename lands on the old name. Undo of an environment rename made with the Manage screen closed: the parked or live list is back on the old name. |
| N4 | An option added on disk with the cursor on the ghost row leaves it on the ghost row (ported). |
| N5 | An outside delete of the selected environment while its tab is live: no row is highlighted, the pane shows the hint, and `dd` returns no action. Then ↓ lands on the first row. The same for a grid option, with the grid focused. |
| N6 | First open of Manage → Environments: the first item is selected. An item deleted outside while its tab was parked: the tab switch lands on the first item. Entering the grid on a fresh selector lands on the first row (the ghost row when there are no options). |
| N6a | A freshly opened selector's grid is on its first row, and space from the left list selects its first option; with no options, the ghost row (`varmanager.rs` unit test). |
| N7 | Deleting the selected env or space selects the item now at its index, and the last item when it was the last one. Deleting the last option puts the cursor on the ghost row. Deleting another env (from the header) leaves the cursor on its own env, including one below the deleted one. |
| N8 | Renaming the selected env or space from the header with the screen closed, then opening it: the cursor is on the renamed item. A grid rename: the cursor is on the new name. |
| N9 | Ghost-row create: the cursor is on the new option (the existing walk-on tests still pass). A paste with the cursor on the ghost row: the cursor is on the first pasted option. |
| N10 | An env switch where both environments have bob: the cursor stays on bob. An env switch to one without bob: the cursor is on nothing. |
| N11 | The existing column tests pass unchanged (Tab, Right, Left, a click on a cell). *Amended 2026-10-27 (task #188):* Tab's walk now takes in the description column (the stage 7 spec's §4), so the Tab tests step through it; the by-name walk test still checks the step is from the committed option's row. |
| N12 | The existing drag tests pass. A reload under a drag leaves the cursor on the dragged space in the order on disk (ported). |
| N13 | None: the existing scroll tests cover it. |

## 5. `mine (approved)` choices

Put to the user for veto; approved 2026-10-27 ("Spec is good").

1. **A gone item leaves the cursor on nothing (N5)**, with no row
   highlighted, rather than on a neighbour or the first row. Moving it to
   a neighbour needs the old list, which is the guess this design
   removes. Moving it to the first row means `dd` acts on an item nobody
   pointed at. Cost: one extra ↓ after another program deletes or
   renames the item you were on.
2. **A list or grid that comes on screen with its cursor on nothing
   settles on the first row (N6).** This keeps today's first open
   (the first item selected). It changes R7 for a parked list whose item
   went: today the cursor clamps to the end, after this it goes to the
   first item.
3. **The ghost row is its own place (N4)**: an option added on disk does
   not pull the cursor off it.
4. **App writes keep today's cursor moves (N7, N8, N9)**: a delete goes to
   the neighbour, a rename follows the new name, and an add from the ghost
   row lands on the new option. Each write now does this itself, where it
   knows what it did. Renames switch to `begin_list_op` so undo puts the
   list cursor back.
5. **An environment switch matches the option by name (N10).** The
   alternative is that a switch starts the grid on nothing (or the first
   row). Options with the same name in two environments usually mean the
   same thing, and the cursor is visible after the switch.
6. **The column stays an index (N11).** An outside reorder of a selector's
   `fields` moves the column cursor to another field. Only Enter and F2
   read the column, and they open the cell for editing, so nothing is
   written until you type. Making the column a field name too would add a
   rule for a renamed or removed field.
7. **Undo records the grid row in `View` (N3)**, as #121 did for the
   request table, rather than relying on the index matching.
8. **The open cell (`GridEdit::row`), the row-carrying actions and the
   hits stay indices.** Each is resolved and used within one event, and
   inputs close before any re-read.
9. **No scroll-into-view after an outside reorder (N13).**
