# Manage Property Rows Design

The Settings tab shipped with a control language it invented for itself:
one-row rows painted through `ListRow` with `RowHighlight::Selected`, and
a one-row text "well" wired to a `LineInput` that the mouse cannot reach.
The other Manage tabs speak a different language entirely — labels above
three-row bevelled `TextField`s, three-row `Button`s — so the Manage
screen reads as two applications sharing a tab strip.

This design gives the Manage screen's detail panes one control language:
a compact **property row** — a label column and a one-row control on the
same line — and moves the Settings tab's text fields onto the text-surface
plumbing every other editable field in the app already uses.

## Scope

One branch, three commits, in this order:

1. **The text-surface plumbing.** A bug fix that depends on no repaint —
   the Settings fields become reachable by the mouse while still painted
   exactly as they are today. First, so it is reviewable and revertable
   on its own.
2. **The primitive** and its tests, plus its `testbed.rs` entry. Adds
   `paint/property.rs`; changes nothing on screen yet.
3. **The three tabs' conversion** onto it.

User-visible changes: the Settings tab loses its row bands; the Variables
detail pane and the Environments/Spaces detail panes shrink to one row per
control; Settings text fields gain click-to-place-caret, drag-to-select,
double-click word select and a right-click Copy/Paste menu.

Explicitly out of scope: the Manage screen's **left column item lists**
(they keep the `Selected` band — they are lists, see "The band belongs to
lists, not to controls"), the Variables tab's
**entry grid**, and every **dialog surface** in the app (modals, chooser,
palette, file picker, var picker), which keep the three-row `TextField`
and `Button`. The app deliberately ends with two registers: compact
inline properties inside a pane, full-size controls in a dialog.

## The band belongs to lists, not to controls

`RowHighlight::Selected` paints `blend(bg, accent, 0.35)` plus a `▌`
accent bar down the row's left edge (`paint/rows.rs`). Every surface that
uses it is a **list of items**: the sidebar, the Manage screen's left
columns, the chooser, the palette, the file and variable pickers, the
dropdown, the request editor's table. That is what the vocabulary means —
*this is the selected item*. And because a list of items is so often
reorderable — the sidebar's requests and the Environments/Spaces rows
both drag today — the band carries an expectation of drag with it,
whether or not the particular list happens to offer one.

The Settings tab borrowed that paint for **rows of controls**. A property
row is not a list row: there is no item, no selection, nothing to
reorder. Painting it in the list vocabulary promises list behaviour the
row can never have, which is why the tab invites a drag that does not
exist. The band is not merely too loud — it is the wrong vocabulary.

So the rule is about vocabulary, not about drag: lists keep the band,
control rows never get it. The Manage screen's left columns therefore
keep theirs, correctly, whether or not a given one drags.

**No property row paints a full-width band, ever.** The keyboard cursor
is shown by the row's *control* lifting its own fill — the same
`lift_color(control, 0.12)` the address bar's URL well uses, and the same
rule already recorded for the app's focus language: no rings, the control
lifts.

A row carries one *cursor-bearing* control — the well, toggle or segment
the row is named for — and that is what lifts. Trailing pills do not join
a cursor ring, because the app's keyboard model is context keys advertised
in the footer, not a Tab ring over every control: `reveal` keeps `r`,
`remove` keeps `x`, and both keep the footer chips they have today. The
one exception is a Files row, whose two buttons the cursor genuinely does
aim between — `SettingsTab::file_button` already models exactly that, and
the aimed button is the one that lifts.

Hover is a **wash**, not a band: `mix(page, control, 0.2)` across the row,
roughly a fifth of the old band's contrast, with no accent bar. The whole
row stays a click target — clicking the words "Hover hints" toggles its
checkbox — because the Manage screen is mouse-first and a three-cell
checkbox is a poor target. The wash is what tells you so.

## The primitive: `paint/property.rs`

One new module. Five painters, all flat; four of them exactly one row
tall, and `TallPill` — the panes' title-row buttons — one and a half.

**Flat is deliberate.** A one-row control has no spare rows for
`bevel_top`/`bevel_bottom`, so the Manage detail panes give up the
raised-surface language that `TextField` and `Button` carry. That is the
trade the compactness buys, and it is drawn consistently: within a Manage
detail pane, *nothing* is bevelled. Dialogs keep their bevels, so the
distinction reads as "inline property" versus "dialog control" rather
than as an inconsistency.

**`TallPill` is the one exception to one row**, and to that rule's
reasoning rather than against it. A title-row button (`Rename`,
`Delete`, `Edit fields`, `+ Option`, `Move all requests…`) acts on the
*item the pane is showing*, not on any one of its fields — so at a
property row's height it files itself under the grid, which is exactly
the wrong reading. It gets height instead of a bevel: a quarter-row cap
above the label row and a quarter below, drawn with `▂` over the page
and an inverted `▆`, leaving three quarters of page showing in each
neighbouring row. Half a row taller, not a whole one, so it reads as a
button floating over the pane rather than as a block the grid has grown.
It occupies the same three-row block a `Button` did, which is what let
the left columns' `+ New`, `+ Variable` and `+ Selector` join it. Those
sit on
`theme.panel` rather than `theme.page`, so `TallPill` takes the surface
it is sitting on as a parameter: the three quarters of each cap row that
are not button have to be the colour behind them, or the caps fringe the
button with a wedge of the wrong surface. With those converted, the
Manage screen paints no bevelled `Button` anywhere.

**One geometry, and it is easy to get wrong.** The block straddles its
label row (`y - 1 ..= y + 1`), so a caller that lays it out from the row
it wants the label on ends up a row low. Two places did: the selector
grid, which inherited `y + 1` from the `Button` it replaced, and the
left columns, which started their block at `left.y + 1`. Both moved up a
row, taking their content with them. With one control in both columns of
every tab, a single row of difference is plainly visible, so
`every_manage_button_lands_on_the_same_rows_in_both_columns` asserts the
whole screen shares one block.

```rust
/// The label column's width for a pane, from its own longest label:
/// `max(label widths) + 2`, clamped to `LABEL_W_MIN..=LABEL_W_MAX`. A
/// pane computes this once and passes it to every row it paints, so its
/// controls line up and no label is truncated. A fixed constant could
/// not do both: Settings' longest label is "Ask before sending to AI"
/// (24 cells) and the Variables pane's is "Value in <env>", whose width
/// depends on the environment's display name.
pub fn label_column(labels: &[&str]) -> u16;
pub const LABEL_W_MIN: u16 = 16;
pub const LABEL_W_MAX: u16 = 32;

/// The widest a property row is painted, however wide the pane is: a
/// text well stretched across a 200-column terminal is unreadable.
///
/// Applies to all three panes, so the control column does not jump as
/// the tab strip switches between them. This is a change for Variables
/// and Environments, which today size their fields to the pane
/// (`field_w = right.width - 4`) and so stretch without limit.
pub const MAX_W: u16 = 76;

/// How far a hovered property row's fill blends from `page` toward
/// `control`. Well below `RowHighlight::Selected`'s 0.35 accent blend,
/// and carries no accent bar.
pub const HOVER_WASH: f32 = 0.2;

/// A label and one control on a single row. `paint` fills the row
/// (washed when hovered), draws the label in the label column, and
/// returns the `Rect` the caller paints its control into.
pub struct PropertyRow<'a> {
    pub label: &'a str,
    pub label_w: u16,
    pub hovered: bool,
    pub disabled: bool,
    /// Controls pinned to the row's right edge, laid out right-to-left
    /// before the main slot is measured — the value row's `reveal` and
    /// `remove`, a Files row's `Reset`. The returned slot shrinks to
    /// meet them, so a trailing pill never paints over a well.
    pub trailing: &'a [TrailingPill<'a>],
}
impl PropertyRow<'_> {
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme) -> ControlSlot;
}

/// Where a row's control goes, and what background it landed on — the
/// `ListRow::resolve_fill` pattern, so a control painting on top of the
/// row knows its own backdrop.
pub struct ControlSlot { pub rect: Rect, pub bg: Color }

/// A one-row filled text box: one column of padding either side, faces
/// from the `ControlState` ladder. The compact `TextField`.
pub struct Well<'a> { pub content: Line<'a>, pub state: ControlState }

/// A one-row filled button. The compact `Button`, with the same
/// `ButtonKind` split so an active segment reads as `Primary`.
pub struct Pill<'a> { pub label: &'a str, pub kind: ButtonKind, pub state: ControlState }
pub fn pill_min_width(label: &str) -> u16;

/// A `Pill` pinned to a row's right edge, with the hit it registers.
/// `PropertyRow::paint` lays these out and registers them itself, so a
/// caller never has to compute right-to-left geometry by hand — the
/// mistake the three hand-rolled copies of that loop invite today.
pub struct TrailingPill<'a> { pub pill: Pill<'a>, pub hit: Hit }

/// A checkbox on a `Well`-sized face: `glyph::CHECKBOX` / `CHECKBOX_OFF`.
pub struct Toggle { pub on: bool, pub state: ControlState }
```

Faces come from the existing `ControlState` ladder verbatim —
`Normal` = `control`, `Hover` = `control_hover`, `Focused` =
`lift_color(control, 0.12)`, `Pressed` = `control_pressed`, `Disabled` =
`control` with its content blended by `DISABLED_LABEL_MIX`. Nothing new
enters the theme.

All four are added to `components/testbed.rs` beside the primitives it
already showcases, so the register is inspectable in one place.

## Rhythm

Property rows are **adjacent within a group and separated by one blank
row between groups**. No row-to-row gaps: the compactness is the point,
and a blank line between every property gives back most of what the
conversion buys.

The groups are the panes' existing structure, not a new one. Settings
already groups by heading (`Settings`, `Files`). The Variables pane
groups as the declaration (`Description`, `Default`, `Secret`), then the
environment value (`Value in <env>` with its trailing pills), then the
`used by:` line. Environments groups as the file path, then `TLS`.

## What each tab becomes

### Settings

Same nine rows, same order, same headings. `draw_settings` stops building
`ListRow`s and inline controls by hand and paints `PropertyRow` +
`Well`/`Toggle`/`Pill`. The disabled-row rules survive intact: while
`config.toml` will not parse, every *setting* row paints disabled and
registers no hit, while both Files rows stay live because Edit… and Reset
are the two ways out.

`SettingsTab::first_live_row`, `clamp_to_live`, `move_cursor` and
`file_button` are unchanged — this is a repaint, not a re-model.

### Variables detail pane

`draw_labeled_field` (label on its own line, `FIELD_HEIGHT` field below,
blank row after — eleven rows for three fields) collapses into one
`PropertyRow` per field. The pane's ad-hoc inline controls, which are
today bare accent text with a hover inversion, join the register too:

| Today | Becomes |
|---|---|
| label above a 3-row `TextField` (`Description`, `Default`, `Value in <env>`) | `PropertyRow` + `Well` |
| `[on]` / `[off]` accent text (`Hit::VmSecretToggle`) | `PropertyRow` + `Toggle` |
| `󰈈 reveal` / `hide` accent text (`Hit::VmRevealToggle`) | trailing `Pill` on the value row |
| `✕ remove` accent text (`Hit::VmRemoveEnvValue`) | trailing `Pill` on the value row |
| 3-row `Button` (`Rename`, `Delete`, `Edit fields`, `+ Option`) | `TallPill` |
| 3-row `Button` (`+ Variable`, `+ Selector`, and the Environments/Spaces `+ New`) | `TallPill` on `theme.panel` |
| 3-row `Button` (promote) | `Pill` |

Masking, the reveal state, `(not set)`, `promote_action`'s conditional
button and the `used by:` line all keep their current behaviour and
wording. Hit variants are unchanged, so every existing mouse and keyboard
path still resolves — including `r` (reveal/hide), `x` (remove value) and
`p` (promote), which keep both their bindings and their footer chips.
Painting those three as pills makes them *look* like the controls they
already were; it changes nothing about how they are reached.

### Environments and Spaces detail panes

The TLS segmented control's three-row `Button`s become `Pill`s inside a
`PropertyRow` labelled `TLS`; `Rename`, `Delete` and `Move all requests…`
become `TallPill`s on the title row; the environment file path and the space's
request list align to the same label column. `draw_tls_control`'s
keep-priority drop rule (a button that would collide with the title is
dropped, not painted over it, and stays reachable by key) is preserved.

## The text-input contract

This is the defect, stated plainly: `settings.rs` owns a real `LineInput`,
so ctrl+c and ctrl+v work (`app.rs:8763`, `app.rs:8844`) and the caret
paints while editing — but Settings was wired as a fifth text surface that
never joined the text-surface plumbing, so **the mouse cannot reach it at
all**. It has no click-to-place-caret (a click select-alls), no
drag-to-select, no double-click word select, and no right-click menu.

The fix is to join that plumbing, not to write a second copy of it:

- **`TextSurface::Settings`** (`action.rs:33`) — `text_surface_menu`
  (`app.rs:8168`) gains an arm returning Copy (greyed without a selection)
  and Paste for the field under edit, matching `Hit::VmFormField`'s arm
  exactly. Like the other in-place surfaces, the menu is offered only for
  the field *currently under edit*; any other part of a Settings row keeps
  the row menu. "Extract to variable…" stays excluded, as it is for the
  Variable Manager's own surfaces.
- **`TextDrag::Settings`** (`app.rs:64`) plus `settings_field_drag_to`,
  modelled on `vm_field_drag_to` — one row, so only the column maps, via
  `window_start`/`extend_mouse_selection_to`.
- **`Hit::SettingsControl`'s click arm** (`mouse.rs:1165`) gains the caret
  mapping `Hit::VmFormField` already has (`mouse.rs:1895`): map the
  clicked column through the well's geometry, `select_word_at` on a double
  click else `set_cursor` + `begin_mouse_selection`, and anchor the sweep.
- **`active_selection_text` and `paste_text`** already route Settings and
  are unchanged.

Everything else about the editing model stays: Enter commits, Esc
cancels, a click away commits, and a commit the field *refuses* swallows
the click and keeps the typed text (`commit_settings_edit_for_click`).

**Parity, defined.** The bar is what every other text surface does, no
more and no less: click places the caret, drag sweeps, double click
selects a word, right click offers Copy/Paste, ctrl+c/ctrl+v copy and
paste, and the full `LineInput` keyboard (word nav, shift+arrows,
Home/End) applies. Two behaviours are deliberately *not* on the list
because no surface in the app has them — triple-click select-all
(`mouse.rs:277` resets the counter at two, so a third click is a fresh
single) and shift+click extend (`SHIFT` appears nowhere in `mouse.rs`).
Both are recorded under "Open follow-ups"; adding them here would widen
this branch from the Manage panes into the shared click dispatcher.

Because `Well` is one primitive, the same geometry serves every compact
field, so the Variables pane's converted fields keep the mouse behaviour
they have today by construction rather than by a second implementation.

## Testing

- **Per painter, buffer tests**: each `ControlState`'s face; `Focused`
  outshines `Hover`; a hovered `PropertyRow` paints `HOVER_WASH` and
  **no** `▌` bar and no `selection` fill; a disabled row's content blends
  by `DISABLED_LABEL_MIX`.
- **A regression test for the vocabulary**: no `PropertyRow` paints
  `theme.selection` or a `▌` bar, in any state, and the three converted
  panes paint neither. The lists beside them — the left columns, the
  entry grid — are untouched and keep theirs, so the test must be scoped
  to property rows rather than to a pane's whole rect.
- **A parity test across text surfaces**: for each of `TableCell`,
  `VmFormField`, `VmEntryCell` and `Settings`, the field under edit
  registers a hit, offers a `text_surface_menu`, and anchors a
  `TextDrag`. This is the test that stops a sixth surface shipping
  half-wired.
- **Behaviour tests for the new Settings mouse paths**: click places the
  caret at the clicked column; drag extends the selection; double click
  selects a word; right-click on the live field opens Copy/Paste.
- Existing tests updated for the new geometry. The affected modules hold
  70 tests today (`settings.rs` 10, `varmanager.rs` 56, `manage_list.rs`
  4); most are behavioural and should not move, and only the ones
  asserting on painted rows and rects will. **Every one of them must
  survive with its intent unchanged** — in particular the
  disabled-config-row tests, the drop-a-colliding-button test, and the
  write-failure-keeps-the-typed-text tests. A test that has to be
  *deleted* to make this land is a signal that the conversion changed
  behaviour it was not supposed to.

Baseline before any change, on `manage-property-rows`: 1776 lib + 454
core + integration tests, all green.

## Open follow-ups (not this branch)

- Whether the compact register should reach the request editor's params
  and headers tables. Not evaluated here.
- **Triple-click select-all and shift+click extend**, missing from every
  text surface in the app rather than from Settings alone. Both live in
  the shared click dispatcher (`mouse.rs:265-284`) and each surface's
  click arm; `LineInput::set_cursor_extending` already exists for the
  shift+click half. Their own branch, and their own review.
