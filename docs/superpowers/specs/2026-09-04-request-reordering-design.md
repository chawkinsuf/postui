# Request Reordering — Design

Date: 2026-09-04
Status: approved, not yet implemented

## Goal

Let the user put the requests in a space in any order, not just
alphabetical, and let them do it by dragging rows in the sidebar with
the mouse. A keyboard route (alt+up / alt+down, context-menu items)
exists for every move the mouse can make.

The spaces spec (2026-09-01) parked this as "its own follow-on
brainstorm"; this is that brainstorm.

## Decisions

- **Order lives in `project.toml`, shared.** `[space.<slug>] order =
  [...]` — the same home as the space order, so teammates on the same
  repo see the same order and a fresh clone keeps it. Per-user
  `.local/state.toml` and an `order` number inside each request file
  were rejected (order lost on clone; every reorder rewriting several
  request files).
- **One flat list per space.** Entries are slugs relative to the space:
  `"login"`, `"auth/refresh"`. The list only carries *relative* order
  among siblings; where an `auth/…` entry sits relative to a top-level
  one has no meaning. This keeps one key per space, one toml_edit write,
  and makes rename cascades a string replace.
- **The display rule at every level** (a space's root, or a folder): the
  level's requests that appear in the list come first, in list order;
  the rest follow, alphabetically by display name with slug as tiebreak
  (today's order). Folder rows stay below the requests and stay
  alphabetical — folders exist only implicitly (a `/` in a request
  name), and ordering them is a later addition on the same list if it
  is ever wanted.
- **Listed entries that match no file are ignored for display and never
  rewritten away.** Same policy as invalid space names: a hand-written
  or stale entry is the user's; an app write preserves it in place.
  Entries the app itself makes stale (delete, rename, move) are
  cascaded, so this only applies to ones the app didn't cause.
- **Drop-on-folder does nothing.** A drag only moves a request among its
  siblings. Moving a request into a folder stays a rename (put a `/` in
  the name). Drag never performs a file move by accident.
- **Live reorder, no insertion marker.** While dragging, the dragged row
  follows the pointer and its siblings shift, so the rows on screen are
  always the order that a release would write. A between-rows marker
  needs half-row precision terminals don't have.
- **Click behaviour is unchanged.** A press on a request row still
  focuses, selects and opens it. The press also arms a *possible* drag;
  nothing further happens unless the pointer moves onto another row
  while the button is held.
- **Not an undo step**, matching space reorder (undo-redo spec). A
  reorder is cheap to redo by hand and putting it in the undo stack
  would interleave with content edits confusingly.

## Architecture

### Core (`postui-core`)

**Model.** `project::ItemSettings` gains

```rust
/// Spaces only: request order, slugs relative to the space
/// (`"login"`, `"auth/refresh"`). See `order_level`.
#[serde(default)]
pub order: Vec<String>,
```

**Pure sort.** `project::order_level(entries: &[&RequestListing], order:
&[String], space: &str, prefix: &str) -> Vec<&RequestListing>` applies
the display rule to one level. `prefix` is the level's path *within the
space* (`""` for the root, `"auth/"` for folder `auth`). It matches each
entry's space-relative slug against `order`, sorts listed ones by list
position, then appends the unlisted ones by `(display.to_lowercase(),
slug)`. No filesystem; the sidebar and the Manage list both call it.

**Writes.** All go through the existing `edit_project_toml` (toml_edit;
comments and unrelated keys survive). `set_order(doc, space, &[String])`
writes `[space.<slug>] order`, creating the table if missing and
removing the key when the list is empty.

- `move_request(root, slug, delta) -> Result<(), ProjectError>` —
  keyboard step. Computes the level's *displayed* order (via
  `list_requests` + `order_level`), materialises it into the list so
  the order on disk is exactly what was shown (the way `move_space`
  does), swaps the entry `delta` places clamped to the level's ends,
  and writes. Entries for other levels keep their positions; the
  materialised level's entries replace that level's existing entries
  in place (first occurrence's slot), any new ones appended.
- `set_level_order(root, space, prefix, slugs: &[String])` — drag
  drop. Replaces the level's entries (those whose relative slug is
  directly under `prefix`) with `slugs` in the given order, using the
  same in-place-then-append rule. Other levels' entries and unmatched
  entries are untouched.
- `order_rename(root, space, from, to)` — rewrites the entry `from` to
  `to` in place (no-op if absent). Called after `rename_request`
  when the slug changed *within* the space (a rename that changes
  the folder part moves the entry to a new level; that's fine — it
  keeps its list slot, which is meaningless across levels, and shows
  first in its new level, which is a reasonable "you just put it
  here").
- `order_remove(root, space, slug)` — removes the entry. Called after
  `delete_request`, and for the source side of `move_request_to_space`.
- `order_insert_after(root, space, anchor, slug)` — inserts `slug`
  directly after `anchor` (appends if `anchor` is unlisted, which
  means the level has no list yet and the copy will sort
  alphabetically next to its source anyway). Called after
  `duplicate_request`.
- `move_request_to_space` cascades: remove from the old space; append
  to the new space's list only if that level already has one (else it
  sorts alphabetically like any unlisted request).

All cascades are applied by the app after the file operation
succeeded; a failed cascade toasts a warning and leaves the file op in
place (the file is the truth; a stale order entry is harmless and is
ignored for display).

### Sidebar (`components/sidebar.rs`)

- `refresh(listing, space, expanded, order: &[String])` passes `order`
  down; `build_rows` sorts each level's requests with `order_level`
  and takes the space slug so relative slugs line up. `space_requests()`
  (the Manage screen's list) uses the same call.
- New state: `drag: Option<RowDrag>`

  ```rust
  pub struct RowDrag {
      pub slug: String,          // full slug of the dragged request
      pub prefix: String,        // level path within the space
      pub original: Vec<String>, // sibling order at drag start (relative slugs)
      pub working: Vec<String>,  // current on-screen order
  }
  ```

  While `drag` is `Some`, `build_rows` uses `working` in place of the
  order list for that one level. Every other level is unaffected.
  `refresh` is called after each working-order change so `rows` is
  always the order on screen (selection survives by identity as it
  already does).
- **Draw.** The dragged row uses the selected style and shows a `⋮`
  grip glyph (U+22EE, one cell — see terminal-glyph-widths) in its
  left gutter in place of the method chip's leading space. No ghost
  row, no marker.
- `HitMap` registration is unchanged: one `Hit::SidebarRow(i)` per
  visible row.

### App: mouse (`app/mouse.rs`, `hit.rs`)

New `App` fields, alongside `drag` and `text_drag`:

```rust
sidebar_press: Option<(usize, String)>, // row index + slug of the armed press
```

`sidebar.drag` (above) is the active drag.

- **Press** (`Down(Left)` on `Hit::SidebarRow(i)` that is a
  `Row::Request`): existing behaviour (focus, select, open), then
  `sidebar_press = Some((i, slug))`. A press on a folder row, or a
  right-click, arms nothing.
- **Move** (`Moved | Drag(Left)`, same arm as the other drags):
  - if `sidebar.drag` is `None` and `sidebar_press` is `Some((i, _))`
    and the pointer's row ≠ `i` (and the button is still held: `Drag`
    kind, or `Moved` while `sidebar_press` is armed — mirror the
    scrollbar drag's handling of terminals that report either): promote
    to an active drag. `original`/`working` are the level's current
    on-screen sibling order.
  - if active: map the pointer's y to a row index; clamp to the first
    and last row of the sibling group (so a pointer over a folder row,
    another level, the header, or outside the pane pins to the nearest
    end); compute the target index within the group; if it differs,
    move the slug in `working` and `refresh`. Hover is suppressed as
    for the existing drags (`resync_hover`). Pointer shape:
    `PointerShape::Grabbing` (OSC 22 `grabbing`), two-line addition to
    `hit.rs`.
  - **Auto-scroll:** while active, a pointer on the sidebar's first or
    last visible row scrolls the list one row on each `Action::Tick`
    (the 100 ms tick the spinner already uses), then re-applies the
    pointer mapping.
- **Release** (`Up(Left)`): clear `sidebar_press`. If active: if the
  pointer is inside the sidebar pane and `working != original`, call
  `set_level_order`, then `project.reload_meta()` + refresh directly
  (the `ReloadProjectFiles` path is mtime-gated and a second move in
  the same tick would undo itself on screen — same fix as
  `MoveSpace`). Otherwise discard `working` and refresh. Either way
  clear `sidebar.drag`, restore the pointer shape, resync hover.
- **Escape** while active: cancel exactly as a release-outside does.
  Any other key during a drag is handled normally (the drag survives).
- A write error toasts `cannot reorder: <e>` and the sidebar refreshes
  to the on-disk order.

### App: keyboard and menu (`keys.rs`, `action.rs`, `app.rs`)

- `Action::MoveRequest { slug: String, delta: i32 }` → `move_request`,
  then `reload_meta` + refresh, then reselect `slug` by identity so the
  cursor follows the moved row. Error → toast `cannot move request`.
- Bindings, sidebar pane focused, selection on a request row:
  `alt+up` → delta −1, `alt+down` → delta +1. (Verify in the plan that
  neither is bound in that context; `keys.rs` currently has no
  alt+arrow bindings.) The footer's context-key strip for the sidebar
  advertises them (keyboard-context-actions).
- Request context menu (`context_menu_for`): "Move up" and "Move down"
  after "Duplicate", dispatching the same action. Both are disabled
  (drawn dim, no-op) when the row is already first / last in its
  group.
- Manage screen: no change this round (its list stays a name-sorted
  view; the sidebar is where order is authored).

## Error handling

- Unparseable `project.toml`: today's behaviour (warning, defaults);
  order writes then fail with the parse error and toast.
- A listed entry whose file is missing: ignored for display, preserved
  on write. No warning — unlike an invalid space name, a stale request
  entry is expected churn.
- Duplicate entries in `order`: first occurrence wins for display;
  writes dedupe.
- Concurrent edit of `project.toml` by another process between read
  and write: last writer wins, as for every other project.toml write
  today.

## Testing

**Core (`project.rs`, no app):**
- `order_level`: listed-before-unlisted; list order honoured; unlisted
  alphabetical by display name; stale and duplicate entries ignored;
  entries from other levels ignored; empty list reproduces today's
  order exactly.
- `move_request`: first move materialises the level; clamps at both
  ends; other levels' entries untouched; a stale entry keeps its slot;
  comments and other keys in `project.toml` survive.
- `set_level_order`: replaces only the level; in-place-then-append.
- Cascades: rename rewrites in place; delete removes; duplicate lands
  after source (and appends/no-ops when the level is unlisted);
  move-to-space removes from old, appends to new only if listed.

**Sidebar:**
- Rows honour the order per level, folders still alphabetical below.
- A `RowDrag.working` overrides exactly one level.
- Selection identity survives a reorder refresh.

**App (existing `app/tests.rs` harness):**
- Press–move–release reorders, writes `project.toml`, and the rows
  match on the next frame; a second move in the same tick sticks.
- Release outside the sidebar cancels; Escape cancels; rows revert.
- A drag over a folder row / another level / the header pins to the
  group's end and never leaves the group.
- Press without movement still just opens the request.
- Auto-scroll at the edges on `Tick`.
- `alt+up` / `alt+down` and the menu items move and keep the selection
  on the moved slug; disabled at the ends.
- Pointer shape is `Grabbing` during a drag, restored after.

**Manual (tmux, Ghostty):** grip glyph is one cell; `grabbing` cursor
shows; drag feels right at the edges.

## Out of scope

- Reordering folders, or interleaving folders with requests.
- Drag to reparent (into a folder) or across spaces.
- A "sort alphabetically" reset.
- Reorder as an undo step.
- Ordering on the Manage screen.
