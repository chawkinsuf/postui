# Project settings on the Manage screen — Design

Task #303. Status: **approved 2026-11-15**, **restarted 2026-11-16** on the
simpler model (§2, R17). The six questions on the ticket (comment 1353)
were answered "A for each" (comment 1357), so each recommended option
below is `mine (approved)`. Choices made while building are `mine` and
listed in §5 for a second look.

**Restart (2026-11-16).** The first build (kept as the branch
`project-settings-v1`) wrote the headers onto `project.toml` header by
header, read them back after each write, and found the cursor's and the
clicked row again by name. Nine review rounds later it still had
findings, nearly all about rows moving under a click after the read-back.
The user: "Just start this branch over with the simpler version" (you).
The simpler version, proposed by main and chosen by the user: the tab
edits the default headers the way a request's headers are edited, the
same table on a list in memory, and each change writes the whole
`[default_headers]` table as one undo step. No merge onto the file, no
read-back, no finding rows again.

## 1. The problem

Every project-level setting lives in `project.toml` (`ProjectMeta`,
`crates/postui-core/src/project/meta.rs`, `deny_unknown_fields`, so that
struct is the full list). Space order, environment order, space and
environment names, request order and the environment TLS force already have
editors on the Manage screen or the sidebar. Two have none:

- **`name`**, the project's display name (absent = the folder name). Set
  only by the new-project dialog; shown on the header's project chip and in
  the project picker.
- **`[default_headers]`**, sent with every request (`X-Client = "postui"`,
  or `X-Off = { value = "x", enabled = false }`; values take
  `{{variables}}`). A request's Headers tab lists the enabled ones as auto
  rows and its pencil copies one into that request; nothing edits the
  defaults themselves.

Useful pieces already there: `TableEditorState` (the request Headers and
Params table) edits an `IndexMap<String, Entry>`, which is the type of
`default_headers`; and the Manage edits write `project.toml` at once as
journaled, undoable steps (`Project::set_env_tls`, the renames), through
`Project::edit_project_toml`.

## 2. Rules

| # | Rule | Who |
|---|---|---|
| R1 | Both the project name and the default headers get TUI editing; no new `project.toml` keys | mine (approved), Q1 and Q6 |
| R2 | A new Manage tab **Project**, last of the left group: Variables, Environments, Spaces, Project … Settings. Variables stays the tab Manage first opens on | mine (approved), Q2 |
| R3 | With no project open the tab shows the "no project is open" message the other project tabs show | mine (approved), Q2 |
| R4 | A palette entry opens the tab ("Manage: project", named as the other tabs' are) | mine (approved), Q2 |
| R5 | A **Name** row at the top: the `name` value, or the folder name as a muted placeholder when unset or blank (a blank `name` is no name, for the project's shown name too). Enter (vim `i`, a click) opens it seeded with `name` | mine (approved), Q4 |
| R6 | The name commits on Enter and on Esc, keeping the typing (the field rule, spec 2026-09-15, which the Settings fields follow); discard is undo | mine, see §5 |
| R7 | The committed name is trimmed; empty removes `name` (the project shows its folder name again); nothing else is refused; one undo step per commit. One rule says when a commit writes: when what it would write (the trimmed typing, or no name when that is empty) differs from the `name` the file holds. So typing that trims to the file's name writes nothing, and a commit over a name written by hand with spaces round it, or blank, trims or removes it | mine (approved), Q4; the one write rule mine, 2026-11-16 (comment 1381) |
| R8 | Under it, **Default headers**: the request Headers table reused (`TableEditorState`): a checkbox per row (off writes `{ value, enabled = false }`), name and value cells with `{{variable}}` tinting, a `+ Add header` ghost row, the delete pill and the delete keys on a row. A delete toasts what went with the undo key, as a request table's does | mine (approved), Q3; the toast mine, 2026-11-16 |
| R9 | Every change the table makes to the headers (a committed cell, a toggle, a delete) writes `project.toml` at once as one undo step; no save step | mine (approved), Q3 |
| R10 | Rows keep file order; a new row goes last; no reordering | mine (approved), Q3 |
| R11 | Renaming onto a name already there, blanking a name, and a `+ Add` name already there do what a request's Headers table does on `main` (merge with a warning; refused with a warning; no row added) | mine (approved), restart |
| R12 | Keys: ↑↓ (vim `j k`, `gg G`) walk the Name row, the header rows and `+ Add`; the page keys go to the first or last stop, and ctrl+d/ctrl+u step half the table, crossing to and from the Name row as ↓↑ do; ←→ (`h l`) pick the name or value cell; Enter (`i`) opens; Space toggles; Delete or `d` (vim `dd`) deletes; Tab and Shift-Tab walk the cells while one is open; Esc with nothing open leaves the screen. `a` adds a header from anywhere on the tab, the Name row included, in both profiles, and the footer shows the add chip on both rows | mine; `a` from anywhere mine (approved), comment 1382 |
| R13 | A row's copy pill copies its value to the clipboard; `y` (vim `yy`) on the row does the same, in both profiles, since the tab has no put (as a request table's auto row), and the footer shows the copy chip on a row | mine |
| R14 | A tab switch, closing the screen, an undo, an environment switch, a reload, a journaled op, or a click away commits an open name or cell first (close-input-first, task #75). A write `project.toml` refuses (it no longer parses) toasts and keeps the field open with the typing; a reload of changed files cancels it instead, as it does the Variable Manager's | mine; the journaled op and the reload's cancel mine (approved), comment 1367 |
| R15 | No link from a request's default row to this tab in this task; filed as its own task (#305) | mine (approved), Q5 |
| R16 | Undo of a project step redraws the tab from the restored file; the tab's cursor is clamped, not restored | mine |
| R17 | Each change writes the whole `[default_headers]` table as the tab shows it. A hand edit made to `[default_headers]` while the tab is open, not yet re-read, is overwritten by the next change made on the tab; undo puts it back | you, 2026-11-16: "Just start this branch over with the simpler version" (the simpler version as main proposed it, overwrite and undo named) |
| R18 | A click on a header row whose own commit of an open name or cell changed how many headers there are (an add, or a rename that merged two) does nothing; the next click acts | mine |

**R17 is decided.** A review that finds "a header added to `project.toml`
outside the app is lost by the next change on the tab" has found R17: the
user accepted that cost (2026-11-16) in exchange for dropping the merge
and read-back machinery of the first build. It also follows the standing
rule that edge cases get the simple behaviour: the case needs a hand edit
of `project.toml` in the moment before the app re-reads it, nothing is
lost for good (undo restores it), and a re-read (focus gained, Send, a
chooser) brings the hand edit into the tab before any change there.

## 3. Design

**Core** (`project/project_toml.rs`, a new pair of `Project` methods):

- `Project::set_project_name(&mut self, name: Option<&str>)`: trims, writes
  or removes the top-level `name`, journaled (`transaction` +
  `edit_project_toml`). `None` or blank removes the key. A new name keeps
  the old one's comments.
- `Project::set_default_headers(&mut self, headers: &IndexMap<String, Entry>)`:
  writes `headers` as the whole `[default_headers]` table, in its order
  (R17). Enabled entries are plain strings and disabled ones
  `{ value, enabled = false }`, as a request file's table rows. As a
  cheap courtesy, a key the file's table already holds keeps its
  comments (its item, when unchanged; its decor, when changed); this is
  not a merge, since every header not in `headers` goes. A hand-written
  inline `default_headers = { … }` is written back as a table. An empty
  map removes the table. Journaled; no change records nothing.

**App** (`postui`):

- `ManageTab::Project`, inserted before `Settings` in `ALL`.
- `components/project_tab.rs`: `ProjectTab { stop, focused, name, table }`
  — which stop has the cursor (the Name row or the table), whether the
  cursor is drawn (as the Settings tab's `focused`), the name's live
  `LineInput` while open, and a `TableEditorState`. It draws the two
  sections and publishes its footer chips; `App` owns the writes.
- Routing: a new `Surface::Project` (`Surfaces::PROJECT`, one of the
  navigation surfaces, so vim's list rows, `i`, the add and delete verb
  rows and the window rows reach it) and two field ids,
  `FieldId::ProjectName` and `FieldId::ProjectCell { row, col }`, which
  `line_buf` maps to the tab's inputs so the vim engine edits them like
  any one-line field. The `{{` variable picker is not wired to these
  cells.
- Writes (`app/project_settings.rs`, `run_header_table`): every table
  operation runs the table's own call (`handle_key`, `commit`,
  `click_cell`, `begin_add`, `delete_row`) on a copy of
  `meta().default_headers`, the list the app holds in memory; when the
  copy differs afterwards, `App` writes it whole (`set_default_headers`)
  between `begin_op` and `record_project_step`. A refused write puts the
  table's state back (its open cell and typing) and toasts. The name
  commit writes through `set_project_name`. Nothing is read back after a
  write, and no row is looked up again: the table's cursor and open cell
  stay where the table's call left them, as in a request's table. A key
  that only edits an open cell's text goes straight into the cell
  (`type_into_cell`), since it cannot change the headers; a key neither
  the cell nor the table takes is swallowed, as on the Name row, the
  Settings fields and the Variables grid. The name commit follows R7's
  one rule (`commit_project_name`): it starts a write only when the
  trimmed typing, or no name, differs from the file's `name`.
- Mouse: the table's draw takes the owner its hits register for
  (`TableOwner`): a request's table registers `Hit::TableCell`,
  `TableCheckbox` and the rest, and the Project tab's registers each as
  `Hit::ProjectTable`, so no request-table handler takes them. The Name
  well registers `Hit::ProjectName`. A click on a row first commits what
  is open; if that commit changed the header count, the click does
  nothing (R18). A cell or row click takes the tab's cursor to that row;
  the checkbox, copy and delete pills act on their header and leave the
  cursor on what it was on (the Name row, `+ Add`, or the same header,
  which a delete above it moves up one), as a request table's checkbox
  does; a delete of the header under the cursor lands it on the row that
  moved up, as the delete keys do. The cursor goes to the table only with
  a row selected there. The wheel and the table's view follow a request
  table's rules, shared on `TableEditorState` (spec 2026-10-25 S22–S24):
  the arrows wheel moves the view and leaves the cursor, the vim wheel
  moves the cursor into the view, and a notch that moves nothing marks
  nothing. An open name or cell offers the Copy/Paste menu on a right
  click. A click away drops the tab's cursor, as on the Settings tab; a
  key the tab hands on leaves it dropped.
- Close-input-first (R14): the tab's input holds project state, as the
  Variable Manager's does, so it closes wherever the Manager's closes,
  through one helper (`close_project_inputs`): `close_open_inputs`, a
  journaled op's `begin_op`, a reload of files changed on disk
  (`ReloadProjectFiles`, which cancels one whose write is refused, with
  the Manager's "Edit discarded" toast), Reload from disk (which stops on
  a refusal); vim's `ZQ`/`:q!` abandon it. The tab's own writes take
  their open cell out of the table across the write (a Tab walk has
  already opened the next cell), so `begin_op`'s close finds nothing
  open.

## 4. Tests

- Core: `set_project_name` writes, trims, removes on blank, journals one
  entry, undo restores, keeps comments; `set_default_headers` writes
  plain and inline forms in order, keeps an existing key's comments,
  keeps a renamed header's place, removes the table when emptied, records
  nothing for no change.
- Tab: the strip order and anchoring; the palette entry; the hits.
- App, both profiles (`app/project_tests.rs`): open the name, type,
  commit with Enter and Esc (written, trimmed, cleared removes; a padded
  or blank name in the file tidied by a commit); add a header through
  `+ Add`, from the table and from the Name row; toggle; delete; each one
  undo step that
  restores the file; a rename onto a taken name merges with the warning
  (R11); a hand edit overwritten by a change and put back by undo (R17);
  a refused write keeps the typing; a tab switch, a reload (both arms), a
  journaled op commit an open cell first; `ZQ`/`:q!` abandon an open
  name; clicks on a cell, the checkbox, the delete pill and the name; the
  checkbox, copy and delete pills leaving the cursor where it was, from
  the Name row and from a table row; a delete pill above, below and on
  the cursor's header; the delete toast; `y`/`yy` copying a value; the
  half-page keys to and from the Name row; a blank name drawn as the
  folder name; the wheel; the footer chips.
- Key matrix: a `ManageProject` fixture in the sweeps (verb chips,
  keycaps, the way back) and cases for move, open, add and delete in both
  profiles.

## 5. `mine` choices, for veto

- **R6, Esc keeps the name.** The approved answer read "Enter commits, Esc
  cancels (the Settings text field contract)". The Settings fields as built
  commit on Esc too: the field rule (spec 2026-09-15, "Nothing discards on
  Esc any more") replaced the Settings spec's earlier "esc cancels", and the
  comment quoted the older wording. The tab follows the rule every field
  follows; discarding a typed name is undo.
- R11 and R12: the request table's own behaviour and keys, reused as they
  are. This includes what a request's table does on `main` when a cell
  click's own commit merges two rows (comment 1377): the click opens the
  cell by its drawn row number. The fix for that in the shared
  `click_cell` (comment 1374, built on `project-settings-v1`) is not
  carried over; the restart keeps `table_editor.rs` as it is.
- R13: the copy pill is part of the reused row; it copies the value, as on
  a request row.
- R14: refusal keeps the typing, as the Variables grid and the Settings
  fields do.
- R16: the cursor is clamped onto the rows after an undo, not put back
  where the step left it.
- R18: a dropped click (do it again) rather than a lookup of the clicked
  row after the commit.
- R7's one write rule (2026-11-16, comment 1381): the commit compares
  what it would write with the file's `name`, so Enter on a name written
  by hand with spaces round it trims it, as one undo step.
- The pills leave the cursor where it was (2026-11-16, comment 1381),
  the delete pill included: on a table row the cursor stays on its
  header (round 4 of the restart: keeping its place put it on another
  header after a delete above it), where a request table's delete pill
  moves it to the deleted row's place. A delete of the header under the
  cursor lands on the row that moved up either way.
- R13: `y` and vim's `yy` copy the value to the clipboard, where on a
  request table's rows vim's `yy` fills the register for `p`: the tab has
  no put, so a register-only yank would do nothing visible. A request
  table's auto rows do the same (spec 2026-10-25 S14).
- The ctrl+d/ctrl+u half-page keys cross onto and off the Name row as
  ↓↑ do: ctrl+d off the Name row lands on the first header, and ctrl+u
  goes to the Name row only from the table's first stop.
- A right click opens no menu on the tab, except the Copy/Paste menu on
  an open name or cell. Only the Name row's well is clickable, not its
  label.
- Tokens in the values are tinted as they resolve for the open request
  (the editor's variable snapshot), the same colours its Headers tab gives
  the auto rows.
