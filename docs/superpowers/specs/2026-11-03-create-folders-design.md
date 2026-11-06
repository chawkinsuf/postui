# Create, rename and delete request folders — design

Date: 2026-11-03
Task: #172 ("Right now in order to group requests, you have to edit the
name and add a path to the front of the name. We should make a more user
friendly way to create a new group, both in the gui and with the
keyboard. Maybe a a create group (folder?) button next to the create
request button.")
Status: approved 2026-11-03. Rules come from an interactive design
session on 2026-11-03, and the user then reviewed the spec part by part
("That's fine", "Good", "Good", the decoupling revision, "good", "good",
"two pieces"). `you` rows quote the user's own words; `mine (approved)`
rows are my proposals the user approved; `mine` rows are my calls,
listed in §6, which the review saw. It is built in two pieces (§7).

## 1. Problem

A request folder is not a thing of its own today. `list()`
(`crates/postui-core/src/project/requests.rs:71`) walks `requests/` for
`.toml` files, and every folder the app knows about is derived from the
request slugs found there:

- `order::split_level` (order.rs:109) and `walk_space`/`walk_level`
  (order.rs:403) drive `complete_order`;
- `order::exists_in` (order.rs:61) is the existence test for
  `prune_folders` at its four call sites (requests.rs:455, 672, 987,
  1049);
- `arrive_with_folders`, `shows_folder_ignoring_case`, `move_to_level`
  and `level_entries` in requests.rs;
- the sidebar's `build_rows`, `folders_in` and `drag_draw`
  (sidebar.rs:379, 348, 451).

So a folder shows only while it holds a request. When its last request
leaves, it disappears and its order entry is pruned (2026-11-01 R9), even
though its directory stays on disk (requests.rs never removes a
directory; drag spec D18 deals with those leftovers by refusing drops
onto them). Nothing creates, renames or deletes a folder. The only way to
make one is to type `folder/name` in the New request or Rename prompt;
the folder row menu's "New request here…" prefills that path.

Two more facts shape the design:

- **Undo is a journal of path ops** (`journal::Op`: `Created`, `Renamed`,
  `Trashed`, `Restored`, `Text`). An op's path may already be a
  directory: `create_space` journals `Created{dir}` (spaces.rs:79),
  `move_folder_to` renames a directory (requests.rs:1031), `delete_space`
  trashes one (spaces.rs:180). But a request created at a new folder path
  journals only `Created{file}`: `Disk::write_new` makes the parent
  directories silently (disk.rs:388, `create_dirs`), and undo trashes the
  file and leaves the directories.
- **The dot rule is documented but not applied inside a space.** The
  bundled skill says "A file or folder whose name starts with `.` is
  ignored" (`skills/postui/reference.md`), but the code applies it to
  space directories only (`is_hidden`, mod.rs:254). `walk_files` lists
  `requests/main/.drafts/x.toml` as the slug `main/.drafts/x`.

## 2. Rules

| # | Behaviour | Who |
|---|---|---|
| F1 | A folder is its directory. Every directory under `requests/<space>/` is a folder and shows in the sidebar, empty or not. | you ("it should basically act like a folder on the file system") |
| F2 | Emptying a folder leaves it where it is. A request deleted, renamed or moved out of a folder (or a subfolder moved out) leaves the folder in place with its order entry. Replaces 2026-11-01 R9 and the "A drop elsewhere removes the folder" half of drag spec D15. | you ("it should stay where it is") |
| F3 | Directories left empty earlier (the drag spec's D18 leftovers) and directories made outside postui show as folders. | mine (approved: "that all sounds right") |
| F4 | A folder's order entry (`"auth/"`) lives as long as its directory. It is removed when the folder is deleted or moved, and by the complete-list write (drag spec D7) when the directory is gone. A folder made again later is a new folder (2026-11-01 R9's ruling, kept for deletes). | mine (approved) |
| F5 | Create, rename and delete folder are each one undo step, and undo restores the files, the directories and the order exactly. | mine (approved; the user's undo-restores-exactly rule) |
| F6 | A folder created or removed outside postui shows or goes on the next reload, as request files do. | mine (approved) |
| F7 | Empty folders add nothing to `postui requests` or `postui suite`. | mine (approved) |
| F8 | A drop or rename onto a name already on disk is refused, as drag spec D18 does today. The name it clashes with is now a folder the user can see. | mine (approved) |
| F9 | Create, rename and delete folder all come in this task. | you ("all three in this task") |
| F10 | The sidebar's top row gets a `+ Folder` button beside the request button, the way the Variable Manager pairs `+ Variable` and `+ Selector`. The request button's label becomes `+ Request` when both show. `+ Folder` is the secondary button and drops when the row is too narrow, staying reachable by key, as `+ Selector` does. | you ("i think like the ticket suggested. it follows new var and selector") |
| F11 | While the opt-in curl import button is on, it stays in the same row after `+ Folder`. Buttons drop right to left when there is no room, so the curl button drops first. #240 owns moving it. | mine; #240 is you ("maybe that could be done a different way? like a notification button that slides in. we can make a new task for that.") |
| F12 | Keys, in both profiles: `A` on the sidebar opens New folder (`a` stays New request; the variables list pairs `a`/`A` the same way). On a folder row, `r` renames the folder and `d` (arrows) / `dd` (vim) deletes it. A count on a folder row's delete is refused, as the row menu refuses one (`NO_COUNTED_DELETE`). The palette gains "New folder…". | mine (approved: "yes") |
| F13 | The folder row's right-click menu gains "New folder here…", "Rename…" and "Delete", alongside "New request here…" and "Import curl here…". | mine (approved) |
| F14 | The `+ Folder` button, `A` and the palette make a folder where the cursor is, by F30's rule. "New folder here…" makes one inside the folder it was opened on, last in it. *Revised in the review:* this replaced "a top-level folder", following F30. | mine (approved for "here"); the cursor rule for folders is mine, following F30 |
| F15 | A folder has a display name, stored as `name = "…"` in a `.folder.toml` inside its directory. A folder without one is named by its directory. The sidebar shows the display name, followed by `/` as today. | you ("what if we had a .folder.toml file that stored the display name in each folder?") |
| F16 | The request walk skips every file and directory whose name starts with `.` inside a space, so `.folder.toml` is never a request and a stray `.cache/` is never a folder. Slugs can't start with `.`, so no request can be skipped by mistake. This also makes the code match the skill's documented dot rule. | files: you ("maybe it should skip all .*.toml?"); directories: mine (approved) |
| F17 | postui writes a `.folder.toml` for every folder it creates, even when the name is already a plain slug. That includes the folders a request create, rename, move, duplicate, paste or curl import makes along its path. This keeps postui-made empty folders in git, which keeps no empty directories. | you ("yes always write it") |
| F18 | The New folder prompt takes a display name only. `User Accounts` creates `user-accounts/` holding `name = "User Accounts"`. A folder name can't hold `/`: create and rename refuse it, so every typed path reads one way and a shown name survives the prompts' prefills. An empty name is refused, and an empty `name` in a folder file counts as none. *Revised in the review:* `/` nesting is removed. *Revised again after review block 1* (round 3 found `/` in a name breaking the prefill round trip), replacing "a `/` is part of the name". | mine (approved: "good"); the `/` refusal is mine (approved: "No") |
| F18a | A folder segment in a typed path is a name, as a request's leaf is: it goes into the sibling folder that shows it, ignoring case, and is never read as a directory. So `Customer Accounts/x` goes into the folder shown as "Customer Accounts" even when its directory is `accounts/`. Any other segment makes a new folder named by the typed text, its directory the segment's slug, numbered when something on disk has it (F19), so `Users/x` beside a `users/` shown as "Accounts" makes `users-2/` named "Users". The curl import's prompt prefills its folder path by shown names. Only the curl import reads a typed path (F29); every other prompt takes a name (F28). *Revised 2026-11-03 after review round 2*, which found the first form (shown name, else directory name) landing prefilled directory paths in the wrong folder: the directory-name match is removed. | you ("There's no reason to search the name other than the display name", "It should work just like the requests do", "The tui uses the display name"); first proposed as mine at the builder check |
| F19 | A new folder follows the new-request clash rule exactly. A sibling folder already showing that name (ignoring case) refuses the create with a toast, the way `create_request`'s `sibling_name_taken` refuses a request. A slug that clashes while the shown names differ (`Ping!` next to `Ping`) gets a number (`ping-2`). A rename onto a shown name is refused. *Revised after review block 1:* one naming rule serves the create, the rename, a typed path and a move's copy. The directory is the name's slug as one segment (`folder` when nothing safe survives), numbered while taken. A rename keeps the directory when the name's slug is the one it has, numbered or not (`f-2` renamed "F" stays `f-2`), as a request rename keeps its slug (#209), and numbers rather than refuses when the slug is taken. A move's copy whose name a target sibling shows is named by its directory. A directory whose name isn't a slug follows F33. | mine (approved: "as a new request does"; corrected after the builder check found that requests refuse a duplicate name and number only a slug clash); the one rule is mine (approved: "Yes") |
| F20 | A folder that exists without a `.folder.toml` gets one the first time postui renames it. Nothing else writes into existing folders. | mine (approved) |
| F21 | Delete removes the folder and everything in it, with no confirmation; undo is the safety net. Rename behaves like a folder move: its contents and order go with it, and its place in its level is kept. | mine (approved; the user's delete-never-confirms rule) |
| F22 | The README and the bundled skill's layout section (`skills/postui/reference.md`) each say what `.folder.toml` is. | mine (approved) |
| F23 | The Rename prompt on a folder shows and edits its display name only, and a rename never moves the folder (F28). It keeps the folder's slot (F21). A new slug that clashes on disk is refused (F8). *Revised in the review:* this replaced a prompt prefilled with the parent path. | you (F28) |
| F24 | When a folder's `.folder.toml` doesn't parse, the folder shows under its directory name with a listing warning naming the file, and a rename is refused with that error rather than overwriting the file. Keys other than `name` are kept when postui rewrites the file. | mine (the user's parse-errors-never-default rule) |
| F25 | Deleting a folder that holds the open request does what deleting the open request does today, once for the whole folder: an open table cell commits first, the folder goes with no confirm, and the neighbour opens; undo brings the request back with its edits. *Corrected in the build:* the parenthetical "the dirty gate if it has unsaved edits" was wrong about today's request delete, which has no gate (delete never confirms). | mine |
| F27 | "Move all requests…" (Manage, Spaces tab) moves the folders too, empty ones included, so the source space ends up empty. A folder the target already has at the same path is merged into: its contents join it and the target's `.folder.toml` is kept. A folder the target lacks is made there with the source's `.folder.toml` (§3). The emptied source folders are removed. One op, one undo step, restored exactly. *Revised after review round 6:* every folder with a valid name is carried, and a source folder is removed only when nothing but its `.folder.toml` is left in it, so a dot file the user keeps there stays with its folder (F16). A folder holding only what is left behind (F33) therefore gets an empty twin in the target; choosing folders by what they hold took a patch per case in rounds 5 and 6. *Amended 2026-11-05 (#244), mine:* a folder the target lists only in another case (a hand-made `Users/` where the move makes `users/`) refuses the move, Move to space too, with nothing written and the toast naming its directory (`requests/auth/Users/ is already there in another case; rename it first`). On a case-insensitive disk the copy would land in it, a name nothing can change (F33); on a case-sensitive one it would be a case-only twin (F19, D18). | you ("Yes also the folders"); merging by path and keeping the target's file are mine |
| F28 | A request's name is decoupled from its folder. The Rename prompt on a request shows and edits its display name only: no folder path in the prefill, and a `/` typed there is part of the name (drag spec D18 already reads it that way). The New request prompt is the same. Moving between folders is done by drag or the keyboard move (F31). Replaces drag spec D21's "The Rename prompt stays the keyboard way to change a request's folder". | you ("The request name should be decoupled from the folder name. So after this, editing the request name should not show or be able to edit the folder name.") |
| F29 | The curl import is the exception: a name it is given (from the prompt or the curl) may hold a folder path, and the request is filed there. Each folder segment uses the folder that shows that name, or makes it when there is none (F17, F18a); the last part is the request's name, a conflict when a request there shows it, else created. It is the only input that reads a folder path: Replace with curl's `# @name` names the request by the part after its last `/`, the request staying in its folder (replacing curl replace spec N3's "a name with a `/` reads from the space's top"), so replaying the paste that imported a request changes nothing. *Revised after review round 2* (it found that taking the whole text renamed an imported request to "api/List users"), mine (approved: "C"). | you ("The import can be an exception as a way to input it.", "The one place we should still read a folder name is on import of a request from curl", "Either the folder exists so it uses it or the folder doesn't exist so it creates it and either the request exists so it is a conflict or the request doesn't exist so it creates it"); `# @name` as a name only is mine |
| F33 | A folder whose directory name isn't a valid slug (made in a file manager: `My Stuff/`) is handled as a request file of such a name is: it shows, and nothing changes it. Rename, delete, a drop into it and a create in it are refused as an invalid name; Move all leaves it behind with a warning; `postui check` reports it as an error on the directory, once, where its own name is invalid. *Added after review round 5*, replacing review round 2's "its rename gives it a slug". | you ("We should handle invalid folder names the same way we handle invalid requests names"); `check` reporting it is mine (approved) |
| F30 | A new request goes where the cursor is: right after the cursor's row, in that row's level. On a folder row it goes beside the folder, not into it; "New request here…" puts it inside, last. With no cursor it goes last at the top level (drag spec D4). Replaces D4's "a create lands at the end of its level" for the prompt; the curl import and the other arrivals keep D4. | you ("A new request should go where the cursor is and then we can put it into a folder."); beside-not-into on a folder row is mine |
| F31 | The keyboard move. `m` on a request or folder row picks it up: the row takes the drag's look (the `⋮` grip, an open folder as a block) and the footer shows the move keys. ↑/↓ (`k`/`j`) step it one place through the places a mouse drag passes: onto an open folder's row it becomes the folder's first item (D10), it steps over a closed folder at that folder's level, and past a folder's end it steps out a level. → (`l`) on a closed folder opens it and puts it inside first (D9's dwell). Where several levels end on one row, ← / → (`h` / `l`) pick the level (D12's column). Enter drops, as a mouse drop does (D17–D19: one undo step, the cursor on what moved, an open request stays open). Esc, a click, or a space switch cancels and puts everything back. Other keys are ignored while moving. Counts pass in vim (`3j`). | you ("Maybe trigger move and a ghost row appears that can be moved with the arrows and then enter places it."); the key mapping is mine (approved: "good") |
| F32 | `m` starts the keyboard move in both profiles. Move to space moves to `M`, and stays in the row menu and the palette. | mine (approved: "m and M") |
| F26 | After a create, the cursor is on the new folder row; its parent opens if it was collapsed. After a rename, the cursor stays on the folder. After a delete, it goes where a request delete puts it. Undo and redo put it back as for request ops. Toasts follow the request ops' wording: "Created folder User Accounts", "Renamed folder to Customers", "Deleted folder Customers", and undo's own ("Undid create of folder User Accounts", "Restored folder Customers" for an undone delete, as every undone delete reads). | mine |

## 3. Design

### Core: the listing knows folders

- `list()` returns folders alongside requests: one `FolderListing { path,
  name: Option<String>, broken: Option<String> }` for each non-dot
  directory under a valid space (path is the full path, `"main/auth"`,
  no trailing slash). `name` comes from `.folder.toml`; `broken` holds
  its parse error (F24). `Disk::walk_files` gets a sibling, or a mode,
  that also reports directories, and both skip dot entries (F16).
- `Project::folders()` exposes them next to `requests()`. `relist`
  refreshes both.

### Core: folder existence comes from the listing, not the slugs

Every place in §1 that derives folders from request slugs takes the
folder listing instead, or in addition:

- `order::split_level`, `walk_space`/`walk_level` and `complete_order`
  get the folder paths, so an empty folder has a level and an entry.
- `order::exists_in` for a folder entry checks the folder listing, not
  "some slug starts with it".
- `prune_folders` no longer runs after request ops (F2). Its call sites
  in `order_remove`, `order_move_all` and `move_request_to` go. It stays
  only where a folder itself goes: folder delete and the source side of
  `move_folder_to`, which drop the moved or deleted folder's own entry
  (and, for a move, rename the entries under it as today).
- `arrive_with_folders`, `shows_folder_ignoring_case`, `move_to_level`
  and `level_entries` read the folder listing.
- D18's on-disk check in `move_folder_to` stays as written: it already
  refuses any existing name.

### Core: every folder postui makes is journaled and gets its file

A new `Project` helper, used inside the op's transaction before the file
op that needs the folder, makes each missing folder on a path from the
outside in: `fs_create_dir(dir)` (journals `Created{dir}`), then
`fs_create_file(dir/.folder.toml, name = "<typed segment>")` (journals
`Created{file}`). The typed text of the segment is its display name, so
`User Accounts/login` gives `user-accounts/` named "User Accounts".

Every op that can make a folder calls it: `create_request`,
`rename_request`, `move_request` (Move to space), `move_request_to`,
`duplicate_one`, the put path, `move_all_requests`, and the curl import's
create. An op with no typed segment (Move to space, Move all requests)
copies the source folder's `.folder.toml` as it is, other keys included,
or writes the directory name when the source has none.

Only the curl import splits a name into a path (F29), through
`create_request(path)` and `resolve_display_path` (F18a): a segment
matching a sibling folder's shown name, ignoring case, takes that
folder's path. The prompts (New request, Save as, New folder, Rename)
and Replace with curl's `# @name` pass the typed text as a name only
(`create_request_in(level, name, after)`, `rename_request(slug, name)`)
and slugify it as one segment (F28), so a `/` in it never reaches a
path. Undo inverts the journal in reverse, so it trashes the file, then
the `.folder.toml`, then the directory, and nothing is left behind (F5).
The silent `create_dirs` in `Disk::write_new`/`write_bytes`/`rename`
stays as a backstop for directories the journal must not own (`.postui/`,
trash), but no request op relies on it any more.

### Core: the three folder ops

- `create_folder(level, name, at) -> Result<String>`: makes one folder
  in an existing level (a space root or a folder). It refuses a name a
  sibling folder already shows (F19, a folder counterpart of
  `sibling_name_taken`), numbers a slug that clashes otherwise
  (`unique_slug`'s rule for directories), makes the folder with the
  helper above, and puts its entry at `at`: after a given item (F30's
  cursor rule, through F14) or last (D4, for "New folder here…").
- `create_request_in(level, name, after)` (F28, F30): a name in a level,
  after a given item, or last. `create_request(path)` stays for the curl
  import, which lands last (F29, D4).
- `rename_folder(path, name) -> Result<String>`: never moves (F23). A
  slug change renames the directory with `fs_rename`, refusing a name on
  disk (F8). Then it writes `.folder.toml` with the new display name
  (`fs_write_text`, keeping other keys, F24), creating it if it is
  missing (F20). The folder's entry and the entries under it are renamed
  in place, as `move_folder_to` does. A rename that changes only the
  display name (same slug) is just the file write.
- `rename_request` takes a display name only (F28): the request stays
  in its folder, and a `/` is part of the name.
- `delete_folder(path)`: `fs_trash(dir)` as `delete_space` does, and
  removes the folder's entry and every entry under it from the order.
- `move_all_requests` (F27) walks the source space's folders as well as
  its requests: it makes each folder the target lacks (outside in,
  copying `.folder.toml`), renames each request file as today, then
  trashes the source folders, now empty, inside out. Folder entries
  arrive in the target's order like request entries (`order_move_all`),
  and the source space's order is left empty.

Each op runs in one `order_transaction` and returns what the app needs to
land the cursor and follow held requests (as `move_folder_to` does for
the open request).

### TUI

- **Sidebar.** `Sidebar::refresh` gets the folder listing. `build_rows`
  emits a folder row for each listed folder at its level, empty ones
  included. `folder_name` (sidebar.rs:915) becomes a lookup of the
  display name. `drag_draw` no longer needs a first item inside to draw a
  dragged folder. `folders_in` (for `zR`) reads the listing.
- **Buttons (F10, F11).** The top row lays out `+ Request` (primary),
  `+ Folder` (secondary) and, when on, the curl button, dropping from the
  right while they don't fit. With only the request button, its label
  stays `+ New request`. A new `Hit::SidebarNewFolder` opens the prompt.
- **Actions.** `PromptNewFolder`, `PromptNewFolderIn(path)`,
  `PromptRenameFolder`, `DeleteSelectedFolder`, and the matching
  `PromptKind::NewFolder { parent }` / `RenameFolder { from }`. The
  prompts are `Modal::Prompt` like the request ones. `Sidebar::verb_action`
  maps `Verb::AddAlt` to `PromptNewFolder` on any row, and `Verb::Rename`
  / `Verb::Delete` on a folder row to the folder actions (F12).
- **Keys.** `A` gains `Surfaces::SIDEBAR` for `Verb::AddAlt` in
  `keyroute/table.rs`. `r` and `d`/`dd` already reach the sidebar. `m`
  on the sidebar starts the keyboard move (F32) and `M` takes Move to
  space; `m` on the Manage list keeps its meaning.
- **Prompts (F28, F30).** `PromptNewRequest`, `PromptNewRequestIn` and
  `PromptRenameRequest` prefill no folder path (the rename prefills the
  display name). The create carries the cursor's item as its `at`.
- **Keyboard move (F31).** A sidebar state that holds the picked-up
  item and its current place, drawn with the drag's paint (grip, block,
  shifted rows). It reuses the drag's place model: the places a step
  can reach are the ones `move_to_level`'s drop targets cover (D9–D14),
  walked in screen order, with the level choice at shared end rows. A
  live keyboard move owns the keyboard as a live drag does; Enter calls
  the same drop path as a mouse release. Footer chips while moving:
  `↑↓ move`, `←→ level`, `⏎ drop`, `esc cancel`.
  *Built 2026-11-05 (#241):* the state is the drag's own `RowDrag`, with
  `keys` saying the keyboard drives it. The places are every gap among
  the rows outside the block, top to bottom, each with D12's levels,
  innermost first, from the same math the pointer's column uses
  (`levels_at`, `level_in`); so ↓ past a folder's end steps out one level
  per press, and ← / → walk the same levels. While it is live the router's
  surface is `Surface::SidebarMove`, which binds only vim's `j` `k` `h`
  `l` (counted); every other key reaches nothing. The pointer leaves it
  alone (motion, a lost-release recovery, a focus loss); a press cancels
  it. The chips read `enter drop`, as every other chip spells Enter, and
  run nothing, since a click cancels. Mine: → at the innermost level
  opens the closed folder beside the block in its level, on the side its
  last ↑/↓ step went toward, else on the other side (D9's "side it came
  from"); a click cancels, then acts as the click it is, as a press that
  ends a lost mouse drag does (it selects and arms nothing); the
  sidebar's `m move` chip starts the keyboard move and Move to space has
  no chip (row menu, palette, `M`); vim's `:move` on the sidebar starts
  the keyboard move; `m` closes an open input first.
  *Review round 1 (#241), mine:* a dialog that opens during a move (a
  reload's question) cancels the move first, as a click does, so the
  dialog gets the keys; `App::push_modal` does it, for a mouse drag too.
  A terminal paste during a keyboard move does nothing, as ctrl+v there
  does.
- **Menu and palette (F12, F13).** The folder row menu and the palette
  ("Folder: new", the palette's naming) get their entries. *Changed in
  the build:* no `A` footer chip. The sidebar's chip row has no room to
  spare: the chip pushed the hover hints out at 150–160 columns. The
  `+ Folder` button's hint names the key.
- **Undo views.** `RowKey::Folder(path)` already exists for the cursor;
  the step labels use the folder's display name.

### What is removed

- The request-slug test for folder existence (`exists_in`'s prefix branch
  for folders) and `prune_folders`' calls after request ops.
- 2026-11-01 R9 and the folder-removal half of drag spec D15 (amended in
  place with a pointer here).

### Out of scope

- Display names for spaces' folders in the Manage screen (it lists
  requests by name, 2026-11-01 R12).
- Noticing changes below `requests/<space>/` on focus: `changed_on_disk`
  stamps only the top of `requests/` (mod.rs:796), so a file or
  directory added deeper is picked up on the next op or forced reload,
  not on focus. That gap exists for request files today; it is checked
  and filed separately if real.
- The curl button's new home (#240).

## 4. Tests

- Core listing: a directory with no requests lists as a folder; a
  `.folder.toml` name is read; a dot file and a dot directory inside a
  space are skipped; a broken `.folder.toml` lists the folder with a
  warning (F1, F15, F16, F24).
- Core order: deleting a folder's last request keeps the folder and its
  entry; moving a request or a subfolder out keeps the source folder;
  `complete_order` writes an entry for an empty folder (F2, F4).
- Core create: `create_folder("main", "User Accounts")` makes
  `user-accounts/.folder.toml` with the name and appends the entry; a
  nested path makes each missing ancestor with its own file; a taken slug
  is numbered; undo leaves the tree and `project.toml` byte-identical
  (F5, F17, F18, F19).
- Core implicit folders: `create_request` at `auth/login` in a new
  folder journals the directory and its `.folder.toml`, and undo leaves
  no `auth/` behind; the same for rename into a new folder, Move to
  space, duplicate, put and curl import (F5, F17).
- Core rename: the directory and file change, entries rename in place,
  the slot is kept; a clash on disk is refused; a folder without the file
  gets one; a broken file refuses; a same-slug rename writes only the
  file; a parent change moves; undo is exact (F8, F20, F21, F23, F24).
- Core delete: the folder and its contents go to the trash with their
  entries; undo restores both exactly (F4, F5, F21).
- Core Move all: a space with nested and empty folders ends empty; the
  target gains the folders with their `.folder.toml`; a same-path folder
  merges and keeps the target's file; undo is exact (F27).
- CLI: `postui requests` and `postui suite` are unchanged by an empty
  folder (F7).
- Prompts: Rename of a request in a folder prefills only its display
  name, and `a/b` typed there renames it to "a/b" in the same folder; New
  request lands after the cursor's row, and beside a folder row; "New
  request here…" lands last inside; the curl import with `auth/x` files
  it in `auth` (F28, F29, F30). Existing tests that rename a request
  into another folder get the new rule (the move is now a drag or `m`).
- Keyboard move: `m` then ↓ into an open folder, → into a closed one,
  ← out at a shared end row, Enter drops with one undo step and the same
  result as the equivalent mouse drop; Esc restores; `3j` steps three;
  `M` opens Move to space (F31, F32).
- TUI: the button row at the widths where 3, 2 and 1 buttons fit, with
  and without the curl button (F10, F11); `A`, `r`, `d`/`dd` on folder
  rows in both profiles, with a count refused (F12); the folder menu's
  items (F13); a created folder gets the cursor, nested under an
  expanded parent (F14, F26); deleting a folder holding the open dirty
  request goes through the dirty gate (F25); an empty folder drags
  (D1 with F1).
- The key matrix (`key_matrix_tests.rs` `CASES`) gains the new key rows.

## 5. Spec amendments

- 2026-11-01 reorder folders R9: replaced by F2 (emptying keeps the
  folder) and F4 (deleting forgets the slot).
- 2026-11-02 drag into folders D15: the emptied folder stays after the
  drop. D18: unchanged, with a note that a leftover is now a visible
  folder. D21: the Rename prompt is no longer the keyboard way to change
  a request's folder; the keyboard move (F31) is. D4: a create from the
  prompt lands at the cursor (F30).
- 2026-09-04 request reordering: the keyboard route for every mouse move
  is now F31 for moves into and out of folders.

## 6. My calls, for veto

- F11: the curl button stays in the row and drops first.
- F24: a broken `.folder.toml` shows the directory name, warns, and
  blocks a rename; other keys are kept on rewrite.
- F25: deleting a folder holding the open request behaves like deleting
  that request.
- F26: cursor placement and toast wording.
- F29: Replace with curl's `# @name` takes the part after its last `/`
  as the name (approved: "C").
- F5: a create that makes a missing space directory journals it, so
  undo takes it back (approved: "record it"); every journaled write
  journals the parents it makes (review round 4).
- F5: undo of a directory an op made removes it when it holds nothing
  postui shows (dot files and other unshown files go to the trash with
  it; redo brings them back), and refuses only when it holds a request
  or folder added since (approved: "A", after review rounds 5 and 6).
- F30: a new request on a folder row goes beside the folder, not into
  it; F14: New folder follows the same cursor rule.
- §3: Move to space and Move all requests copy the source folder's
  `.folder.toml` into the folder they make.
- F27: Move all merges into a same-path folder and keeps the target's
  `.folder.toml`.
- §3: every request op that makes a folder along its path gives it a
  `.folder.toml` named by the typed segment, so a folder made from the
  name `User Accounts/login` (the curl import) shows "User Accounts".
- Out of scope: the focus-reload gap below `requests/<space>/`.

## 7. Delivery: two pieces

Built as two branches, merged in order (mine (approved): "two pieces").

1. **Real folders** (branch `create-folders`, #172): F1–F30 and F33. The
   core listing, folder existence from directories, the journaled folder
   helper with `.folder.toml`, `create_folder` / `rename_folder` /
   `delete_folder`, Move all carrying folders, the button row, `A`, `r`
   and `d`/`dd` on folder rows, the menu, palette and footer entries, the
   docs. New folder lands at the cursor (F14) and its prompt and the
   folder Rename prompt take a name only (F18, F23). *Revised
   2026-11-03 after review round 5:* the name-only request prompts and a
   new request at the cursor (F28, F30) moved here from piece 2, and the
   curl import is already the one input that reads a folder path (F29).
   The user: "make sure that folders were no longer part of request
   names. So if we're still processing folder names out of a request
   name, we need to stop it"; the choice to move them here, knowing the
   keyboard has no way into a folder until piece 2, is mine (approved).
2. **The keyboard move** (#241, a subtask of #172, its own branch from
   main after piece 1 merges): F31–F32. The keyboard move on `m` with
   Move to space on `M`. Until it lands, only a mouse drag moves a
   request into a folder.
