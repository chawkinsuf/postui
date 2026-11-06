# Drag requests and folders into and out of folders — design

Date: 2026-11-02
Task: #232 ("Right now requests cannot be easily moved between groups /
folders. It make the drag and drop feature seem unfinished.")
Status: draft, in review with the user. This replaces the design on
branch `drag-into-folders`, which went through eight review rounds and
was set aside. `you` rows quote the user's own words. `mine (approved)`
rows are proposals of mine the user agreed to. `mine` rows are mine,
open to veto.

*Amended 2026-11-03 (#172, spec 2026-11-03 create folders):* a folder is
its directory and shows even when empty (its F1, F2). So D15's "A drop
elsewhere removes the folder and its order entry" is replaced: the
emptied folder stays, with its entry. D18 is unchanged, but a directory
left behind is now a folder the user can see, so a drop onto one clashes
as a shown folder; only a plain file of that name is refused naming the
path. D21's "#172 owns making folders friendlier" points to that spec.

## 1. Problem

A request's folder is the directory its file sits in:
`requests/main/users/login.toml` is request `login` in folder `users`.
Folders nest: `requests/main/users/v1/get.toml` is in `v1`, inside
`users`. The only way to change a request's folder is the Rename prompt,
by typing a path in front of the name, and a folder can't be moved at
all. The sidebar drag (2026-09-04 request reordering, 2026-11-01 reorder
folders) moves a row only among its siblings.

Two older rules shape the sidebar, and both are replaced here:

- Each level draws its requests first, then its folders (2026-11-01 R1).
- A level whose order was never written draws by name, and something new
  in such a level sorts by name (2026-09-04 arrival rule, 2026-11-01 R10).

### What went wrong last time

Two implementation choices on the first branch were patched round after
round instead of being backed out. This design avoids both:

- The order was written one level at a time, just before an op changed
  that level, so a level with no saved order could still draw by name
  everywhere else. Every review round found another case where that
  write disagreed with the screen. Here, every list always names every
  item (D5), so an op only appends, removes or renames entries.
- The drag preview gave the request a made-up slug in the folder it was
  shown in, then swapped the real slug back. Everything keyed by slug
  (the cursor, the open request's band, the unsaved mark) kept losing
  the row. Here, the dragged row keeps its real identity and is placed by
  position (§3, Sidebar).

## 2. Rules

### Order

| # | Behaviour | Who |
|---|---|---|
| D1 | Requests and folders drag into and out of folders in the sidebar, the way a folder tree does in other apps. | you ("Need to be able to drag and drop requests in and out of groups"; "Yeah just like a normal app with folders. Like postman has"; for folders: "if i knew we could have nested folders i would have insisted that dragging them works") |
| D2 | A folder can't go inside itself or inside one of its own folders. | mine (approved: "yes") |
| D3 | Folders and requests mix freely within a level. The order list already holds both kinds; a level draws in list order, whatever the kind. Replaces 2026-11-01 R1. | you ("maybe its better to just let the folders and requests be put in any order, mixed together") |
| D4 | Something new lands at the end of its level: a create, a curl import, a rename or move into a folder, Move to space, Move all requests, and a folder that appears through any of these. Replaces the "sorts by name" half of the 2026-09-04 arrival rule and of 2026-11-01 R10. Duplicate and paste keep their place beside the original. | you ("they expect a new request to be at the end of the list"; "for new spaces they are just in the order they are created by default"); duplicate and paste is mine |
| D5 | Every op that changes a space's order starts from a complete list: before it changes anything, it re-lists the requests and writes every item its list misses in where it draws (D6), so nothing on screen moves. The op then works on a complete list: an arrival is a plain append (D4), a rename renames in place, a put finds its anchor, a drop finds its slot. | you ("if we encounter a set without an order we can just write them in alpha order when we see it") |
| D5a | The same write also happens when the TUI sees missing items: when it opens a project and whenever it re-lists (after its own ops, an outside change, undo or redo). So a request or folder added from outside (by hand, by an agent, by a `git pull`) lands at the end of its folder in the order it appeared, not by name; only items that appear at the same moment sort by name among themselves. That write is not an undo step; the op's write (D5) is part of the op's transaction and undo step. Both writes build from `project.toml` as it is on disk at that moment, never from postui's last read of it, so an outside edit not yet read (a `git checkout`) is kept. Opening a project can therefore write `project.toml`. CLI commands never write it: `postui requests` and `postui suite` draw the same order without saving it. *Amended 2026-11-03, review round 2:* the op's own write is added, because an op could not count on the refresh write having happened (a file could appear between refreshes, the write could fail, a partial walk skipped it), and ops on an incomplete list misplaced arrivals, puts and drops; and the write reads the file on disk, because building from the remembered copy overwrote outside edits. | write when seen: you ("its a one time exception for the upgrade"; "they'll show up in alphabetical order and stay in alphabetical order if people keep adding folders to the disk, which seems odd"; "Yes" to doing both); the op's write and reading the file on disk are mine (approved: "Yes") |
| D6 | How a list missing items draws, and so where D5 writes them: the listed items in list order, then the unlisted requests by display name (slug as the tiebreak), then the unlisted folders by path. No special case for 0.3.0: a 0.3.0 level whose list names only its folders shows those folders above its requests after the upgrade, and D5 writes it that way. | mine (today's rule, kept); no 0.3.0 case is you ("for 0.3.0 just do whatever is simpliest") |
| D7 | D5's write drops entries naming nothing (a request or folder removed outside postui). This replaces "stale entries are preserved on write" (2026-09-04, 2026-11-01 R7) and makes 2026-11-01 R10a's leftover handling unnecessary. Only a whole listing drops anything: when the walk failed part way (an unreadable folder), the write still adds what is missing but keeps every entry, so an unreadable folder keeps its arrangement. | you ("drop them"); the partial-walk rule is mine |

### Dragging

A dragged row is a request, or a folder together with its open contents
(it moves as one block, 2026-11-01 R3). Where it goes is decided by the
rows as they show at the moment the pointer moves. Folders and requests
follow the same rules.

| # | Behaviour | Who |
|---|---|---|
| D8 | The pointer on the dragged block changes nothing, except for D12's column. A pointer that doesn't move never moves anything. A folder's own contents are part of its block, so a folder can never be dropped inside itself (D2). | you ("does this not always keep the cursor on the row") |
| D9 | The pointer on a closed folder's row: the dragged block waits beside it, in the folder's level, on the side it came from (above the folder coming down, below it coming up). It doesn't pass the folder while the pointer is on it. Resting there 800 ms opens the folder, and the block goes in as its first item. Coming from above, the folder row then shifts up one and the pointer is on the block. A drop on the folder before it opens lands beside it, where the block shows. | you ("you rest on a folder to open it and your pointer stays on the folder row. it opens. its still on the folder row. that position just drops the request at the top of the folder"; "up to a closed folder would hover, open it, and then be at the top of the folder"); 800 ms is mine |
| D10 | The pointer on an open folder's row: the block is that folder's first item. | you ("that position just drops the request at the top of the folder") |
| D11 | The pointer on any other row: the block moves to that row, in that row's folder, and lands under the pointer, as a reorder does today. Within one folder this is 2026-11-01 R4. | you ("so you drag down and the request moves down in the folder") |
| D12 | Where folders end, several places share one screen row. Under `users/ { login, v1/ { list, get } }` followed by `health`, the row after `get` is the end of `v1/`, the end of `users/` after `v1/`, and the top level right before `health`. While the block is on such a row, the pointer's column picks the level: each level's two indent columns pick that level, and anything right of the innermost level's indent (over the names, where a drag usually holds) picks the innermost folder. The column is read against the fixed indent columns, not against where the block's own name is drawn, so the block moving in or out never changes what the column picks. So a straight drag defaults to the inner folder, and moving the pointer left takes the block out, one level per indent step. The direction the block came from doesn't matter. This is how outliner and tree drags commonly pick a depth when dropping at the end of a nested list. | the inner default and the column are you ("if two folders end at the same spot, it should default to the inner folder"; "The pointer's column as long as that doesnt add too much complexity and is an accepted pattern"); the mapping is mine |
| D13 | Below the last row: the block goes last, after the last row, and D12's column picks the level, up to the top level. | mine |
| D14 | Above the first row, with the list scrolled to its top: the first place in the top level. It's the one place D9–D12 can't reach when row 0 is a folder. Scrolled down, that area keeps scrolling the list up. | mine (approved: "good") |
| D15 | A folder the drag empties (the block was its only item) stays drawn, open and empty, until the drop, so the rows don't jump and the block can go back in: by D10, its row puts the block back inside. A drop elsewhere removes the folder and its order entry (2026-11-01 R9). A cancel puts everything back. | mine |
| D16 | Cancel is unchanged: releasing outside the sidebar, Esc or a right click. Folders opened by D9 during the drag stay open after a drop or a cancel. The drag only works in the space it started in: switching spaces cancels it, and no folder springs open in another space. | mine (approved: "D17 is good", its number then) |

### The drop

| # | Behaviour | Who |
|---|---|---|
| D17 | A drop in another folder moves what was dragged there unchanged. A request's file keeps its file name (`-2`, `-3`, … when the folder already has that file name) and its text, as Move to space moves one. A folder's directory moves whole, every file inside untouched. Everything inside a moved folder keeps its order: its entries are renamed to the new path. The order gets the block at the place the drop showed. One op, one undo step. | request: mine (approved on the first branch: "Plain move"); folder: mine (approved: "yes") |
| D18 | A drop is refused when it would clash: for a request, the folder already shows a request with the name it will show there (its name, or, for a request with no `name`, its file name after any `-2`); for a folder, the destination already shows a folder of that name, ignoring case, or anything of that name already exists on disk there (a directory left behind when a folder's requests were moved out one by one, a plain file), with the toast naming the path. *Amended 2026-11-03, review block 1 after the rework (mine (approved): "yes continue"):* this replaces trashing an empty leftover directory, whose check grew a case per review round (nested empty directories, a plain file, case folding, `.DS_Store`). Cost: a leftover directory blocks a same-named folder drop until it is removed outside postui. The rows snap back and a warning toast names the clash. A display name holding `/` is only a name: the move never reads it as a path. *Amended 2026-11-05 (#245):* a refused folder drop's toast names the clashing folder by the name it shows and leaves out the destination (`"Accounts" already exists here`, you: "“Accounts” already exists here."). A request drop's toast is unchanged. | mine (request: approved on the first branch; folder: approved: "yes") |
| D19 | After a drop the cursor is on what was dragged. An open request that moved, alone or inside a moved folder, stays open, unsaved edits included, as after a Rename. Undo puts the files and the order back, and the toast names where it went: "Undid move of login to users", "Undid move of folder v1 to the top level". | mine; folder: mine (approved: "yes") |
| D20 | A drop in the block's own folder is a reorder, as today. | mine |

### Elsewhere

| # | Behaviour | Who |
|---|---|---|
| D21 | alt+↑/↓ and the row menu's Move up/down step past the next sibling of either kind. The Rename prompt stays the keyboard way to change a request's folder; no new keys (#172 owns making folders friendlier). *Amended 2026-11-05 (#258):* the sidebar's alt+↑/↓ are removed, so the rule now covers the row menu's Move up/down alone; `m`, the keyboard move, is the keyboard route ("Now that we have m for move for requests and folders in the request panel, i don't think we need the alt-arrows to move the requests as well", you). | mine; the removal you (#258) |
| D22 | `postui requests`, the `postui suite` run order, and "Move all requests…" follow the mixed order: the tree walk recurses into a folder at its place in its level (2026-11-01 R11, mixed). | mine |
| D23 | The Manage screen's request list stays name-sorted (2026-11-01 R12). | mine |

### Known limits

- The open request can't be dragged while it has unsaved edits: a press
  opens the request, and the Unsaved changes dialog comes up instead of a
  drag. This is on main for a plain reorder too, filed as #236. D19's
  "unsaved edits included" holds for the code path.
  2026-11-04, #236 (`mine`): a press on the open request's own row over
  unsaved edits now opens nothing and asks nothing, so it drags like any
  row. A press on another row still raises the dialog, so another row
  can't be dragged while the open one has unsaved edits.
  2026-11-04, #247 (`mine (approved)`): a press on a request row no
  longer opens it; the release does, unless the press became a drag
  (request reordering spec, "Click behaviour"). Over unsaved edits the
  dialog comes at a click's release, and any row drags over them. This
  limit is gone.
- An op or refresh made while request files are missing (a git branch
  switch in progress) drops their entries (D7); when they come back they
  land at the end of their levels.
- When an undo puts back a `project.toml` from before a refresh write,
  the items that write added are unlisted again, and the next re-list
  writes them back at the end of their levels.
- Items appear only as fast as postui re-reads the folders (its file
  watcher), which this task doesn't change.

## 3. Design

### Core: `postui_core::order`

- **`order_entries(requests, folders, order, space)`** replaces
  `order_level` and `order_folders`: one level's items of both kinds in
  D6's order. On a complete list it is plain list order.
- **`complete_order(listing, order, space, drop_dead) -> Vec<String>`**:
  the space's list as it draws, every level walked through
  `order_entries`: missing items are written in, and with `drop_dead`
  (a whole listing, D7) entries naming nothing drop out. Drawing and D5
  both use `order_entries`, so what is written is what was drawn.
- **`split_level`**: the one partition of a level into its requests and
  its folders, read by the sidebar's rows, the tree walk and
  `complete_order`.
- **`displayed_slugs`** recurses into a folder at its place in its level
  (D22).
- **`arrive`** appends, after removing an entry that already names the
  arrival so the list never holds it twice. `level_is_listed` and the
  `exists` parameters go.
- **`rename_prefix(order, from, to)`**: every entry under folder `from`
  (its own entry and everything inside) renamed to `to`, in place. Used
  by a folder move (D17).
- **`merge_level`** keeps its shape; its doc stops saying "one kind".

### Core: `Project`

- **`Project::complete_order(&mut self) -> Result<bool, Error>`** relists,
  then writes `complete_order` for each space whose list it changes
  (`drop_dead` when the walk had no error about no one file), through
  `edit_project_toml`, building each list from the order in the
  `project.toml` text it re-reads, not from `self.meta` (D5a). It is
  called as the first step inside the transaction of every op that adds,
  moves or removes an item (create, rename, Move to space, Move all,
  delete, duplicate and put, `move_to_level`; a batch once at its top),
  before the op's file change, so it is journaled with the op (D5); and
  by the app's `refresh_sidebar`, outside any transaction (D5a). The two
  reorders (`set_request_order`, `move_request_shown`) don't call it:
  they write their whole level as the sidebar drew it, unlisted items
  included, so that level ends complete and nothing arrives; completing
  there cost a full re-walk per held alt+↓ step (new round 1). In a
  directory with no `project.toml` (a registered project whose
  `project.toml` was deleted, which opens as-is with a warning) nothing
  writes an order: `order_edit` writes nothing there, so no op ever
  creates a `project.toml`. Creates, renames, moves, puts and drops still
  do their file part, and the items draw by D6. A drag reorder or alt+↑/↓
  there is refused with a warning toast saying the order can't be saved
  without a `project.toml`, rather than snapping back silently. This
  replaces main's reorders and puts creating one (mine (approved): "1 i
  think", new round 2). Inside an op the completion is best effort: a
  write that fails doesn't fail the op, and the op's own order writes
  report their own failures. A space whose directory is missing is left alone. A
  read-only project writes nothing, and a directory with no
  `project.toml` gains none from it.
- **The request ops stay as they are on main**, minus what an incomplete
  list needed: `arrive` appends, `order_rename` renames in place within a
  level, `order_remove` prunes emptied folders. The duplicate/paste
  `seed` flag and `level_shown` go: the anchor is always listed, so
  `order_insert_after`/`_before` find it. A copy whose anchor is in
  another space arrives (D4).
- **`Project::move_to_level(item: &TreeItem, level, at) -> Result<(String, Moves), Error>`**
  (D17–D19), `item` a request slug or a folder path: `at` is the index in
  `level`'s items, without the item, as they draw before the move (the
  app's rows show exactly that: a folder the drag emptied still exists
  until the move). It checks D2 and D18, and in one transaction moves the
  file or directory, re-keys every held request it carried, relists, and
  writes the order: the item's entry (and, for a folder, everything under
  it via `rename_prefix`) out of the old level and in at `at`, emptied
  folders pruned. A request keeps its file name, numbered as Move to space
  numbers it. A folder moves the way `rename_space` moves a space's
  directory: one `fs_rename` of the directory, with every request under
  it in the entry's `moves`, so undo and the held requests follow as
  they do for a space rename. Returns the new slug or path, and the
  (old, new) slug of every request it moved, which the app follows (the
  editor, its shadow, the open request) instead of working them out a
  second time (mine, review round 2 before the rework). `level`
  equal to the item's own is refused as a bug (the app sends D20 as
  `set_request_order`).

### Sidebar (`components/sidebar.rs`)

- **`build_rows`** draws each level through `order_entries`, recursing
  into an open folder at its place.
- **`siblings(i)`**: every row at row `i`'s depth under the same parent,
  of either kind. A folder covers its row and its open subtree.
- **A drag records a place, not a slug.** `RowDrag` holds the dragged
  row's real key (request slug or folder path), its origin level,
  `target: (level, index)` (the index among the level's other items) and
  `dwell: Option<(folder, Instant)>`. `build_rows` leaves the block out
  of its own level and draws it, with its real keys and names, at
  `target`, its contents one level deeper as usual. The folders on its
  original path are drawn even when it emptied them (D15). Everything
  keyed by slug keeps working, with no mapping. The `working` order and
  the folder-only path of main's drag go: a reorder is a `target` in the
  origin level.
- **`drag_to(row, column)`** applies D8–D12 by the rows as they show: on
  the block, only D12's column; a closed folder row, D9 (start or keep
  the dwell); an open folder row, D10; any other row, D11, then D12's
  column when the new place shares its row. Any row that isn't the dwelt
  folder clears the dwell. A target is never inside the block (D2).
  `drag_below(column)` applies D13, `drag_above()` D14.
- **D12's places**: the levels sharing the block's row run from the level
  of the row after it (the outermost; the top level at the end of the
  list) to the innermost folder that ends just above it. The column maps
  to a level by the rows' two-column indent and is clamped to that range.
- **Painting**: the dwelt folder row takes the hover fill and its glyph
  and name the accent.

### App

- **`refresh_sidebar`** calls `Project::complete_order` after `relist`
  (D5a) and before reading the order. A failed write toasts once per
  distinct error; the rows draw the same either way.
- **`sidebar_drag_to`** maps the pointer: below the last row,
  `drag_below`; above the first row at scroll 0, `drag_above`; else
  `drag_to`. It then checks the spring too, because the tick's sleep
  restarts with every event.
- **The tick** springs the dwelt folder after 800 ms (D9): adds it to the
  open folders, puts the block first inside, rebuilds. Only while the
  drag's space is the active one (D16).
- **`finish_sidebar_drag`**: same level, `set_request_order` (D20);
  another level, `move_to_level`, then `follow_move` for each request it
  carried (the slug half of what a rename does to the editor, its shadow
  and the open request), cursor on the new key, the moved folder kept
  open if it was, one step labelled with the destination. On a refusal:
  toast, clear the op-in-flight mark, snap back.

### Docs

- `skills/postui/reference.md`: the `order` paragraph says each level
  mixes both kinds in list order, postui keeps every item listed and
  appends new ones, and a file added by hand is written in at the end of
  its level the next time the TUI sees it.
- Dated notes in the 2026-09-04 and 2026-11-01 specs pointing here for
  D3, D4, D5 and D7.

## 4. Tests

- **Core `order`**: `order_entries` mixed, unlisted after listed in D6's
  order. `complete_order` on an empty list, a partial list, a 0.3.0
  folder-only level, and with entries naming nothing (dropped).
  `displayed_slugs` recursing at a mixed place. `arrive` appending, and
  never listing an entry twice. `rename_prefix` on a nested folder.
- **Core `Project`**:
  - Each order-changing op completes the list first, inside its undo
    step: on an incomplete list (a 0.3.0 list, a file added outside) a
    create lands last, a same-level rename keeps its place, a put lands
    beside its anchor, and a drop lands at its slot; undo restores
    `project.toml` byte for byte. A partial walk adds but drops nothing.
  - The refresh write is not journaled, and keeps an outside edit to
    `order` that postui hasn't read yet.
  - Create, rename into a folder, Move to space, Move all and curl import
    land last; a same-level rename keeps its place; duplicate and paste
    land beside the original.
  - `move_to_level` for a request: into a folder at a slot, to the top
    level, the emptied folder pruned, the file name kept and numbered
    when taken, the text untouched, both D18 clashes refused with nothing
    written, a name holding `/`.
  - `move_to_level` for a folder: into another folder and to the top
    level, nested contents moved with their order, a folder-name clash
    refused, into itself or its own folder refused, held requests
    re-keyed.
  - Undo restores files and `project.toml` byte for byte; redo.
- **Sidebar**: mixed levels draw in list order; `siblings` spans both
  kinds; each of D8–D15 for a request and for an open and a closed
  folder, a still pointer giving a fixed result; D12 at a single and a
  double folder end, by column, from above and from below; D13 by
  column; D14; the dragged row keeps its real key (cursor, band, marks).
- **App**: press, drag into a folder, release: file moved, order written,
  cursor on the new slug, the open request still open and dirty. The
  same for a folder holding the open request. A closed folder springs
  after 800 ms of ticks and of motion events. A clash toasts and snaps
  back. Undo and redo toasts. Cancels leave sprung folders open. A drag
  across a space switch cancels. `refresh_sidebar` writes the missing
  items once (toasting a failure once), and files added outside land at
  the end in the order they appeared, without a reload loop.
- **CLI**: `postui requests` lists a mixed level in mixed order and never
  writes `project.toml`.
- **Manual (tmux)**: every D8–D15 case by mouse for a request and a
  folder, the column at a double folder end, a clash, undo, and a new
  request landing last in a project from 0.3.0.

## 5. Build order

Each step test-first:

1. Core `order`: `order_entries`, `split_level`, `complete_order`, mixed
   `displayed_slugs`, `arrive`, `rename_prefix`.
2. Core `Project`: `complete_order`, the ops without the incomplete-list
   cases, `move_to_level` for requests and folders.
3. App: `refresh_sidebar` completing the order.
4. Sidebar: mixed rows and siblings, the drag's place and dwell, D8–D15,
   the dwell paint.
5. App: drag mapping, spring, the drop.
6. Docs.
7. Full suite, clippy, fmt, tmux pass.
