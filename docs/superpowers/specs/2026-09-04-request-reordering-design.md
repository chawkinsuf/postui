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
- **An undo step** *(revised 2026-09-04; originally "not an undo step,
  matching space reorder")*: a dropped drag, an alt+↑/↓ press and the
  row menu's Move up/down each record one `StepKind::Reorder` carrying
  the `OrderEdit::Replaced` that `set_level_order` reports (the level's
  list before and after). Quick successive keyboard moves of the same
  request roll up into one step (the history's usual 2 s burst window,
  the same one typing uses), and a burst that ends where it started
  records nothing; a drag is always its own step. Undo/redo rewrites
  the list, reloads `meta`, refreshes the sidebar and puts the selection
  back on the moved request, with an "Undid/Redid reorder of X" toast. A
  move that writes nothing records nothing. The Manage screen's space
  reorder is unchanged (still not an undo step).

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
- `order_rename(root, space, from, to)` — a rename that stays in the
  same level rewrites the entry `from` to `to` in place (no-op if
  absent), so the request keeps its slot. A rename that changes the
  level (the folder part of the slug) is a removal from the old level
  plus an *arrival* in the new one (below).
- `order_arrive(root, space, slug)` — the one rule for every way a
  request can newly appear in a level: **if the level already has
  entries in the list, append `slug` after the last of them, so it
  lands last on screen; if the level has none, do nothing**, and the
  request sorts alphabetically with its unlisted siblings as today.
  Called after `create_request`, after a level-changing rename, and
  for the destination side of `move_request_to_space`.
- `order_remove(root, space, slug)` — removes the entry. Called after
  `delete_request`, for the source side of `move_request_to_space`,
  and for the old level of a level-changing rename.
- `order_insert_after(root, space, anchor, slug)` — inserts `slug`
  directly after `anchor`; if `anchor` is unlisted the level has no
  list, so this is a no-op and the copy sorts alphabetically next to
  its source. Called after `duplicate_request`.

All cascades are applied by the app after the file operation
succeeded; a failed cascade toasts a warning and leaves the file op in
place (the file is the truth; a stale order entry is harmless and is
ignored for display).

### Sidebar (`components/sidebar.rs`)

- `refresh(listing, space, expanded, order: &[String])` passes `order`
  down; `build_rows` sorts each level's requests with `order_level`
  and takes the space slug so relative slugs line up. `space_requests()`
  (the Manage screen's list) is *not* changed: it stays name-sorted, as
  Out of scope says.
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
  - if active and the pointer is *inside* the sidebar pane (the same
    test the release commits on): map the pointer's y to a row index;
    clamp to the first and last row of the sibling group (so a pointer
    over a folder row, another level or the header pins to the nearest
    end); compute the target index within the group; if it differs,
    move the slug in `working` and `refresh`.
  - if active and the pointer is *outside* the pane: `working` snaps
    back to `original` (`Sidebar::drag_reset`) without ending the drag,
    so the rows always preview what letting go right now would leave
    behind. Motion back onto a row maps it again as usual. The list's
    own edge rows are inside the pane, so auto-scroll is unaffected. Hover is suppressed as
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
  A right click while active cancels the same way — see "Implementation
  notes" below for why that alternative exists.
- **A live drag owns the keyboard.** Escape cancels and the modified
  quit combo (ctrl+c, or whatever `Action::Quit` is bound to with a
  modifier) still quits; *every other key is swallowed* — before
  modals, before the per-screen routes — so nothing opens a dialog,
  moves the selection or reorders by keyboard under a row in flight.
  The footer says the same: while a drag is live its chips are exactly
  `esc cancel drag` and `right-click cancel drag`, the palette chip
  hides (its key is dead, as under a modal) and the quit chip shows the
  modified combo.
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
- **Undo restores the order slot with the file.** *(Revised 2026-09-04:
  the state after an undo must equal the state before the op it undoes,
  so the earlier "file, not slot" acceptance is withdrawn.)* Every
  cascade (`order_arrive`, `order_remove`, `order_rename`,
  `order_insert_after`, `order_move_all`) reports what it did as a list
  of `OrderEdit`s — insert at index, remove from index, rename — and the
  request file op's undo step (`FileStates`/`Trashed` `orders`) carries
  them. Undo replays the inverses backwards, redo replays them forwards
  (`order::apply_edits`), so a deleted, renamed, moved or duplicated
  request goes back to exactly the slot it held. The replay is
  index-tolerant: a reorder made between the op and its undo (still not
  an undo step) survives — the re-inserted entry lands at its old index
  and its rearranged siblings stay put. Move-all is one undo step of
  its own (every file back, both lists replayed); the source space's
  local memory is left in place by the move, since a remembered open
  request is checked for existence before use.

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
- Cascades: same-level rename rewrites in place; level-changing rename
  removes from the old level and arrives in the new; create, and the
  destination of move-to-space, arrive; arrival appends when the level
  is listed and no-ops when it isn't; delete and the source of
  move-to-space remove; duplicate lands after source (no-op when the
  level is unlisted).

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
- Ordering on the Manage screen. *(Superseded — see §Space drag
  (Manage screen) below, which brings the same gesture to the Spaces
  tab. Environments still have no order to rearrange.)*

## Space drag (Manage screen)

The Manage screen's Spaces tab reorders with the same gesture the
sidebar does. A left press on a space row selects it and arms a press;
the moment the pointer reaches a *different* row of that list the press
becomes a live drag, and from there every motion event belongs to the
drag until the button comes up, whichever pane the pointer wanders
over. Release on the list commits; release anywhere else, Escape, or a
right click cancels and the rows snap back. While the pointer is
outside the list — the same containment test the release commits on —
the working order snaps back to the order the drag started from
(`ManageList::drag_reset`, the cursor riding back with it) without
ending the drag, so the rows preview the cancel a release there would
be; motion back onto a row rearranges them again. The Environments tab never
arms a press — environments have no order to rearrange.

While the drag is live the list paints the working order the pointer
has arranged rather than the project's spaces, the dragged row keeps
the selected fill and shows the `⋮` grip in its first cell (one cell,
no variation selector, in the column the label never uses, so nothing
shifts), and the list cursor rides along with the dragged row so the
detail pane keeps showing the space being moved. Hover is frozen for
the length of the drag and the pointer shape is `grabbing`, exactly as
in the sidebar.

A live space drag owns the keyboard exactly as the sidebar's does:
Escape cancels, the modified quit combo quits, every other key is
swallowed, and the footer carries only the two cancel chips.

A drop *shifts* the rows between source and target rather than swapping
two of them, so the writer takes the finished order rather than a
delta: `postui_core::project::set_space_order(root, displayed)` deals
the displayed names back out over the valid-name slots of the written
list, leaving an invalid listed entry in the slot it was written in
(the same slot arithmetic `move_space` does), and writes nothing when
the result equals the list already on disk. An order that is not
exactly the displayed set is refused with `NotFound`. On commit the app
reloads `meta` and the space list directly rather than through the
mtime-gated `ReloadProjectFiles`, for the same reason `Action::MoveSpace`
does, then puts the cursor back on the dragged space.

Like `Action::MoveSpace` and the sidebar's request drag, a space
reorder is **not an undo step**: it rewrites `project.toml`'s `spaces`
key and nothing else, and the alt+↑↓ keys and the row menu's
Move up/Move down remain the keyboard route to the same change.

The terminal quirks recorded for the sidebar drag apply here
unchanged: a right click is the in-band cancel because Ghostty (Linux)
swallows the first Escape pressed while a button is held; a `Down(Left)`
arriving during a live drag means the release was lost, so it cancels
the drag and then handles the press normally; and phantom `Drag(Left)`
motion after a cancel carries no drag meaning but must still drive
hover.

## Implementation notes (2026-09-04)

What the shipped code does differently from the text above. The
behaviour is as designed; these are placement and signature details a
later reader would otherwise have to rediscover.

**Module placement.** The order helpers live in their own module,
`crates/postui-core/src/order.rs` (`postui_core::order`), not in
`project.rs`. `project.rs` keeps only `ProjectMeta` and the space/
environment machinery; `order.rs` owns `space_order`, `order_level`,
`order_arrive` / `order_remove` / `order_rename`, `set_level_order` and
`move_request`, together with the `toml_edit` writer that creates
`[space.<slug>] order` and removes the key when the list empties.

**`order_level` takes no prefix.** Its signature is
`order_level(entries: &[&RequestListing], order: &[String], space: &str)`.
The sidebar has already partitioned the listing by folder level before
it calls in, so the function only applies the display rule (listed
entries in list order, then the rest by display name with the slug as
tiebreak) to the slice it is handed.

**App-side helpers.** Two private helpers on `App` carry the cascades:

- `App::split_rel(slug) -> Option<(space, rel)>` — splits a full slug
  into the space and the path relative to it, which is the shape every
  order-list call wants.
- `App::order_cascade(what, result)` — applied after a request file op
  has already succeeded. The file on disk is the truth and a stale
  order entry is ignored for display, so a failed cascade pushes a
  warning toast and reloads the meta; it never fails the file op.

**Sidebar rendering.** `Sidebar::build_rows` outgrew clippy's argument
limit once the space, order list and drag overlay joined it, so the
per-level constants travel as a `LevelCtx<'_>` (`expanded`, `space`,
`order`, `overlay`) instead of four positional arguments.

**`Sidebar::drag_edge` is inclusive.** It compares `y <= list_top` and
`y >= list_bottom` rather than testing strict equality, so a pointer
dragged past the list rather than exactly onto its last row still
auto-scrolls.

**`App.sidebar_press` is `(row index, slug)`, and the slug is the
authority.** The index only says *that* a row was pressed; a rebuild
between the press and the first motion event (a focus-gained
`ReloadProjectFiles`, a folder toggle, a background refresh) can put a
different request at that index. Promotion therefore re-resolves the row
by the stored slug and, when that slug is no longer on screen, disarms
the press rather than promoting.

**`RowDrag` carries its `space`.** A drag belongs to the space it
started in: `Sidebar::rebuild` applies the working-order overlay only
when the space it is drawing matches, and `finish_sidebar_drag` treats a
space mismatch as a cancel. `App::enter_space` also ends any live drag
before the root changes, so a space switch with the button still held
(ctrl+1..9, alt+z) cannot write the old space's slugs into the new
space's order list.

**Promotion yields to modals.** A press on a broken row opens its error
modal; the modal owns the pointer from then on, so the promotion branch
is gated on an empty modal stack.

**Tick edge-scroll checks x too.** `drag_edge` only sees a y, and the
response pane shares the sidebar list's bottom row, so the `Tick`
handler first requires `hits.pane_at(x, y) == Some(PaneId::Sidebar)`.

**`move_request` writes nothing when nothing moves.** The clamp can land
the request where it already is (alt+↑ on the first row); it returns
early there rather than materialising the level into `project.toml` for
a move that had no effect.

**Cancel disarms the press.** `finish_sidebar_drag` clears
`sidebar_press` on *every* drag end, commit or cancel. Review found
that Escape alone left the press armed, so the next stray motion
event — with no button held — restarted the drag. Confirmed fixed in
the manual pass.

**"Move all requests…" cascades too.** `ForceMoveAllRequests` runs
`order_remove` on the source and `order_arrive` on the destination for
every request it moved, so emptying a space leaves no stale entries
behind — the app made them stale, and entries the app makes stale are
cascaded.

**Tick auto-scroll casts.** The `Tick` handler calls
`self.sidebar.handle_scroll(delta as i16)`; `drag_edge` returns an
`i32` and `handle_scroll` takes an `i16`.

**A right click during a drag cancels it instead of opening the row
menu.** `Down(Right)` checks `sidebar.drag.is_some()` first and, if so,
calls `finish_sidebar_drag(false)` and returns — no menu, no selection
move, nothing written. This exists because Ghostty (on Linux, verified
with `--keydump` and mouse reporting off) delivers no key event for the
first Escape pressed while a mouse button is held — the second arrives;
the cause is unconfirmed and is Escape-specific. The app cannot recover
a key it never receives, so a right click is the in-band cancel. In
Ghostty this right click itself needs two presses too, for the same
underlying reason: with a left button held, `--keydump` shows Ghostty
drops the *first* right press outright (`Down(Right)` never arrives),
so the cancel only takes effect on the second right click. A right
click while a press is
armed but hasn't promoted yet just disarms the press (so the motion
that follows can't restart a drag out from under the menu) and falls
through to open that row's context menu as usual.

**Two guards survive a lost left release.** Continuing the same
Ghostty quirk: `--keydump` with a left button held shows the sequence
`Down(Left) → Down(Right) → Up(Right) → Drag(Left) ×N (button already
released) → Down(Left) → Up(Left)` — after the right click cancels the
drag, the terminal never sends `Up(Left)` for the original press; it
keeps reporting `Drag(Left)` motion until the next real press/release
pair. Two guards in `handle_mouse` (`crates/postui/src/app/mouse.rs`)
cope:
- The `Moved | Drag(Left)` arm used to return early on any non-`Moved`
  event with no `text_drag` armed, leaving `self.hovered` frozen. It
  now falls through to hover tracking whenever `text_drag` is `None`
  (the sidebar-drag and scrollbar-drag branches above it already
  handle those cases) — a button-held motion with no armed sweep
  carries no drag meaning, so hover is the right response, and it
  heals the lost release since hover keeps working until the next
  click.
- The `Down(Left)` arm now checks `self.sidebar.drag.is_some()` first:
  a real left press cannot arrive while a drag is genuinely live (the
  button that started it would still be down), so if one is live here
  the release was lost. It calls `finish_sidebar_drag(false)` to
  cancel it, then handles the press normally — arming it against the
  row underneath rather than letting its eventual release commit the
  stale reorder.

**`rebuild_sidebar` snaps `AnimKey::ListTravel(Sidebar)` too.** The
per-motion drag path (`App::rebuild_sidebar`) originally omitted the
snap that `refresh_sidebar` does after every rebuild, on the theory
that the band shouldn't chase the dragged row from slot to slot. But
the dragged row *is* the open row (a press opens its request before a
drag can start), so skipping the snap left the travel anim easing
toward the pressed row's pre-drag index while `rebuild` moved it,
and `draw`'s crossfade painted a ghost band on the row that used to be
open. `rebuild_sidebar` now snaps on every motion event, exactly like
`refresh_sidebar`; both call a shared `App::snap_sidebar_travel`
helper.

### Manual pass (tmux, 160x45, `animations = false`)

Six requests (`main/a`…`main/e` plus `main/grp/f`) against a scratch
project. All checks passed:

1. Press row 1, drag to row 4, release — rows reorder live and
   `project.toml` gains `[space.main] order = [...]`.
2. Dragging a request onto the `grp/` folder row, and onto `grp/f`
   inside the expanded folder, clamps it to the last slot of its own
   level. No file moves.
3. Press, drag, Escape — rows snap back and `project.toml` is byte
   identical. A following stray motion event does not restart the drag.
4. `alt+↓` twice walks the row down two slots and the selection follows
   it; the sidebar footer carries the `alt+↑↓ reorder` chip.
5. Right-clicking the first request row renders "Move up" in the muted
   foreground and inert (clicking it leaves the menu open and the order
   unchanged) while "Move down" reorders.
6. The `⋮` grip occupies exactly one cell on the dragged row, taking
   the place of the leading space, so the method and name columns stay
   aligned with the rows around it.

The OSC 22 `grabbing` pointer shape is **unverified through tmux** —
`capture-pane -e` reproduces SGR attributes but not OSC sequences.
The app-level test asserts the `Grabbing` shape is set during a drag
and restored after.
