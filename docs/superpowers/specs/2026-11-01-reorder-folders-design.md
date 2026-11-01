# Reorder request folders — design

Date: 2026-11-01
Task: #224 ("We should be able to drag request groups to reorder them
the same as we can requests.")
Status: approved 2026-11-01 (the user went through it part by part:
"Good", "Good", "Go"). The `mine (approved)` rows are proposals of mine
the user approved; only the quoted rows are the user's own words.

## 1. Problem

The sidebar draws each level of a space (its root, or one folder) as
that level's requests, then its subfolders. The requests follow the
space's `[space.<slug>] order = [...]` list in `project.toml` (spec
2026-09-04 request reordering). The folders don't: `Sidebar::build_rows`
emits them from a `BTreeMap` keyed by path, so they are always sorted by
name, and the 2026-09-04 spec lists "Reordering folders, or
interleaving folders with requests" as out of scope. A press on a
folder row arms no drag (`mouse.rs`, the `Hit::SidebarRow` arm), and
`Sidebar::begin_drag`, `group_bounds` and `level_group` all refuse a
folder row. alt+↑/↓ on a folder row does nothing, and the folder's
right-click menu has no Move items.

## 2. Rules

| # | Behaviour | Who |
|---|---|---|
| R1 | Folders move only among their sibling folders: the folders of the same level. They stay below that level's requests; folders and requests never interleave. | you ("maybe we can keep them separate if we can at least move the folders around") |
| R2 | A folder row drags with the same gesture as a request row. Press arms. Moving onto another row starts the drag, and the folders shift live. Releasing inside the sidebar writes the order. Releasing outside, Esc or a right click cancels. While the pointer is outside, the rows preview the cancel. The edges auto-scroll, the `⋮` grip shows in column 0, the pointer is `grabbing`, and a live drag owns the keyboard. | you (task text: "the same as we can requests"); the details are the request drag's, unchanged |
| R3 | An expanded folder moves as a block: its open subtree travels with its row. | mine (approved) |
| R4 | A drag's target follows a rule that can't oscillate between tall rows. The dragged folder moves up past the previous sibling once the pointer is on or above that sibling's first row. It moves down past the next sibling once the pointer is on or below that sibling's last row. It repeats while the rule still holds. For one-row siblings this is exactly today's request rule, so one rule serves both kinds. | mine (approved) |
| R5 | alt+↑/↓ on a folder row moves it one sibling up or down, clamped at the ends. The cursor stays on it. The folder's right-click menu gains "Move up" and "Move down" after "Import curl here…", drawn disabled at the group's ends. The footer's existing `alt+↑↓ reorder` chip covers it. | mine (approved; the 2026-09-04 rule "a keyboard route exists for every move the mouse can make") |
| R6 | The order is stored in the same `[space.<slug>] order` list. A folder entry is the folder's space-relative path with a trailing slash: `"auth/"`, `"api/v2/"`. The slash keeps it apart from a request slug, which never ends in `/`. | mine (approved) |
| R7 | The display rule for a level's folders matches the one for its requests. The folders named in the list come first, in list order. The rest follow by name, in today's byte order. A level with no folder entries draws exactly as today. Stale entries (no such folder) are ignored for display and preserved on write. | mine (approved; mirrors the request rule) |
| R8 | A folder reorder is an undo step, recorded like a request reorder. A drop is its own step. Quick repeated alt+↑/↓ presses on the same folder roll into one step, and a burst that ends where it started records nothing. The toast reads "Undid reorder of folder auth". Undo puts the cursor back on the folder row. | mine (approved; follows 2026-09-04 §Decisions "An undo step") |
| R9 | When a folder's last request leaves (delete, a rename into another folder, a move to another space, "Move all requests…"), the folder's entry is removed, along with every ancestor folder the same op emptied. A folder made again later is a new folder with no remembered slot. Undo of the op brings the entry back with the request, since the op's journal entry already covers `project.toml`. | you ("if its deleted, it will be a whole new folder if its made again. so it should not remember te position") |
| R10 | A folder that newly appears through postui (a create, a rename into a new folder path, a move, a curl import into a new folder) follows the request arrival rule. If its level already has listed folder entries, it is appended, so it lands last. Otherwise nothing is written and it sorts by name. Each new ancestor folder of the arriving request goes through the rule, outermost first. "New" means no request other than the arriving ones is under it. | mine (approved; mirrors `order_arrive`) |
| R10a | A new folder never takes a leftover slot. A folder can be emptied outside postui: a `git pull`, a file manager, an agent's edit, or a teammate on 0.2.0, which has no folder cascade. Its entry then stays in the list. When a folder with that name newly appears through postui (R10), the leftover entry is removed before the arrival rule runs. A folder that reappears outside postui (files written by hand or by an agent) can't be caught, so it comes back in the leftover slot. | mine (approved), to carry R9's ruling ("it will be a whole new folder") to deletes made outside postui |
| R11 | `order::displayed_slugs` walks the tree in the sidebar's order: a level's requests, then its folders in folder order, recursing into each. It feeds the CLI's `postui requests` listing, the order `postui suite` runs in (suite.rs: "in the order `postui requests` lists them"), "Move all requests…" and `level_shown`. So a folder drag changes the suite's run order, as a request drag already does. It also fixes a mismatch that exists today: levels are walked in slug order, so folder `a-b` comes before `a/x` (`-` sorts before `/`), while the sidebar draws `a/` first. `cli/read.rs`'s `displayed` already promises "the order the sidebar draws them". | mine (approved) |
| R12 | The Manage screen's request list stays name-sorted, as it is today. | mine (approved; 2026-09-04 Out of scope, unchanged) |

## 3. Design

### Core: `postui_core::order`

- **Entries have two kinds.** `folder_entry(rel_path) -> String` appends
  the slash. `entry_level(e)` is the level an entry belongs to: it strips
  a trailing `/`, then applies `level_of`. So `"api/v2/"` has level
  `"api"` and `"auth/login"` has level `"auth"`. Every list-scanning
  helper that today calls `level_of` on a list entry (`merge_level`,
  `level_is_listed`) calls `entry_level`. Without this, a folder-level
  write would be misfiled: `level_of("a/")` is `"a"`.
- **`order_folders(paths, order, space)`** is `order_level`'s twin for one
  level's folder paths (full paths, as `build_rows` holds them). Listed
  paths come first in list order, the rest by path, in byte order.
- **`level_is_listed` gains a kind**, so a request arrival asks "does this
  level have listed requests?" and a folder arrival asks "listed
  folders?". A folder entry counts as existing when some listing slug
  starts with `space/path/`.
- **`merge_level` is unchanged in shape.** It replaces entries of `level`
  that appear in `slugs` in place, then appends. Fed folder entries, it
  writes a folder level, and request entries of the same level are
  untouched, because they aren't in `slugs`. `Project::set_request_order`
  and `move_request_shown` therefore serve folders as they are. Their
  docs say "entries" rather than "requests". `move_request_shown`'s
  `MergeKey::RequestOrder { space, slug }` takes the folder's full path
  with the slash as its `slug` (`main/auth/`), so a folder burst and a
  request burst never merge.
- **`displayed_slugs`** becomes the tree walk of R11, using
  `order_level` and `order_folders`.

### Core: cascades (`project/requests.rs`)

- `order_remove(space, rel)` removes `rel`, then the entry of each
  ancestor folder of `rel` that no listed request is under any more.
  The listing has already been `relist`ed by every caller. The removal
  goes innermost first.
- `order_arrive(space, rel)` runs R10 and R10a for each ancestor folder
  of `rel`, outermost first, then the request's own rule. A folder is
  new when no listed request other than `rel` is under it.
- `order_move_all` does the same per moved pair, in its one
  read-modify-write per space. A destination folder is new when every
  request under it is one of the arriving ones.
- All of this runs inside the op's existing transaction, so undo and
  redo restore `project.toml` with the files. No new journal kind.

### Sidebar (`components/sidebar.rs`)

- `build_rows` orders `folder_children` with `order_folders`, and the
  drag overlay applies to the folders of the overlay's level as it does
  to the requests. The overlay becomes the level's order list with
  `working` merged in (`merge_level`). Then a request drag leaves that
  level's folder order alone, and a folder drag leaves its requests
  alone.
- `RowDrag.slug: String` becomes `RowDrag.key: RowKey`. `original` and
  `working` hold entries: request rels, or folder entries.
- `group_bounds(i)` becomes `siblings(i) -> Vec<(RowKey, Range<usize>)>`:
  the sibling group of row `i`, each with its row span. A request spans
  one row. A folder spans its row plus its visible subtree. A folder
  row's siblings are the folder rows at its depth under the same parent.
  `level_group`, `begin_drag`, the menu's first/last test and the Move
  handler all read it.
- `drag_to_row(i)` applies R4 over the spans. For requests (all spans of
  one row) it gives today's results, and the existing tests pin that.
- alt+↑/↓ on a folder row emits `Action::MoveFolder { path, delta }`.

### App

- `sidebar_press: Option<(usize, String)>` becomes `Option<(usize,
  RowKey)>`. A press on a folder row arms it. A single click still only
  selects, and a double click still toggles. Promotion re-resolves the
  row by key, as it does by slug today.
- `Action::MoveFolder { path, delta }` joins `Action::MoveRequest` in
  one handler that takes the `RowKey`. It reads the level and entries
  from `Sidebar::level_group`, calls `move_request_shown`, rebuilds,
  reselects by key and records the step.
- `finish_sidebar_drag` aims the cursor at `drag.key` and labels the step
  from it: `request_display` for a request, `folder <name>` for a folder.
- `context_menu_for`'s folder arm gains the two Move items (R5).

### Docs

- `skills/postui/reference.md`, the guide `postui skill install` puts in
  front of agents, says that `order` holds request slugs and that "a
  level's folders follow its requests, by name". It gains the `"x/"`
  entry form and the folder display rule (R6, R7).

### Removed

- The "Folder rows stay below the requests and stay alphabetical" half of
  the 2026-09-04 display rule. The row "Reordering folders" leaves its
  Out of scope list. That spec gets a dated note pointing here.

## 4. Tests

- **Core `order`**: `entry_level` for request and folder entries;
  `order_folders` with listed-first, unlisted by name, stale and
  duplicate entries ignored, and no entries meaning today's order;
  `merge_level` writing a folder level without touching the same
  level's request entries, and the reverse; `displayed_slugs` following
  folder order, and `a/x` before `a-b` with no folder entries at all.
- **CLI**: `postui requests` and the `postui suite` run order follow a
  folder entry.
- **Core cascades**: deleting a folder's last request drops its entry
  (and an emptied ancestor's), and undo restores it byte-identical.
  Deleting a request whose folder keeps others leaves the entry. A
  rename out of a folder drops it, and a rename into a new folder of a
  listed level appends it. A move to another space and "Move all…" do
  both sides. An arrival into an unlisted folder level writes nothing.
  A leftover `"x/"` entry (its folder emptied by hand) is dropped when a
  create makes `x/` again, and the new folder lands last (R10a).
- **Sidebar**: folders follow the list, and requests stay above them;
  `siblings` spans include an open subtree; the R4 rule moves a
  one-row folder past a tall open neighbour and back without
  oscillating (a fixed pointer over several motion events gives a
  fixed order); the existing request drag tests unchanged; alt+↑/↓ on
  a folder row emits `MoveFolder`.
- **App** (`app/tests.rs`): press–move–release on a folder row writes
  `"x/"` entries and the rows match on the next frame. Release outside,
  Esc and a right click cancel. An open folder drags with its subtree.
  A drag over a request row or another level clamps to the folder
  group. alt+↑/↓ and the menu items move a folder and keep the cursor
  on it, and the items are disabled at the ends. Undo and redo of a
  folder drag and of an alt burst, with the toast text and the cursor.
  A plain click on a folder still only selects, and a double click
  still toggles.
- **Manual (tmux)**: drag a folder past an open neighbour and back, drag
  an open folder, alt+↑/↓, the right-click menu, delete the last
  request then undo.

## 5. Build checklist

One sequence, each step test-first:

1. Core `order`: `entry_level`, `folder_entry`, `order_folders`, a kind
   for `level_is_listed`, `displayed_slugs` as the tree walk.
2. Core cascades: folder removal in `order_remove` and
   `order_move_all`'s source side; folder arrival and the leftover drop
   (R10, R10a) in `order_arrive` and the destination side.
3. Sidebar: folder order in `build_rows` with the merged overlay;
   `RowDrag.key`; `siblings` spans; R4 in `drag_to_row`; alt+↑/↓ on a
   folder row.
4. App: `sidebar_press` by `RowKey`; `Action::MoveFolder` sharing the
   move handler; `finish_sidebar_drag` by key; the folder menu items.
5. Docs: `skills/postui/reference.md`, plus the dated note in the
   2026-09-04 spec.
6. Full suite, clippy, fmt, tmux pass.

## 6. `mine` choices to veto

- R3: an open folder drags as a block, rather than collapsing during the
  drag.
- R4: the drag rule that can't oscillate.
- R5: alt+↑/↓ and the menu items on folder rows.
- R6: folder entries share the `order` list with a trailing slash,
  rather than going in a separate `folders = [...]` key.
- R8: undo, as for requests, with the toast "reorder of folder auth".
- R10: a new folder lands last when its level is arranged, and sorts by
  name otherwise.
- R10a: a folder made again through postui drops a leftover entry. A
  folder that reappears outside postui keeps the old slot.
- R11: the CLI listing, the suite's run order and "Move all…" follow the
  folder order. With no folder entries, this also moves `a-b` after
  `a/x`'s requests, to match the sidebar.
- R12: the Manage screen stays name-sorted.
