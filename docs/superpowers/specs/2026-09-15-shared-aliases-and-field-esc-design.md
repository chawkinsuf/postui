# Shared Aliases and the Field Esc Rule

postui is getting a vim mode. Before that mode exists, this round lands
everything a vim user wants that a mouse-and-arrows user never notices:
one rule for what Esc does in a text field, undo inside every text
field, and the list-navigation aliases (j/k, h/l, g/G, half and full
page, `:`, `u`) that vim users try first. None of it is gated behind a
mode. The vim mode that follows then only has to add modal text editing
on top of states this round already defines.

The design question that shaped the round: a user learns "Esc closes
the text box" in a table cell, presses it inside a rename prompt, and
the prompt vanishes. The fix is to make every text field close the same
way and to give form modals somewhere for focus to land that is not a
field.

## Scope

One branch. User-visible changes:

- Esc closes any open text field and keeps what was typed. Nothing
  discards on Esc any more.
- ctrl+z / ctrl+shift+z undo and redo inside an open text field.
- Form modals gain a keyboard-focusable button row; cancelling one from
  inside a field now takes two presses of Esc, with the footer saying
  which press is which.
- The palette, chooser and variable picker filter boxes become real
  single-line inputs (caret, word navigation, undo).
- Vim navigation aliases in every list surface, as strict synonyms of
  the arrow and page keys already there.
- Two relabels: the response pane's `r` (raw⇄pretty) and `h`
  (headers⇄body) collapse into one `t` that cycles the three views, and
  the Variable Manager's new selector moves from `g` to `a`.

Out of scope, and listed under "Deferred": the vim mode itself (modal
editing, first-launch choice, Settings row), a footer label profile, a
sidebar `/` filter, keyboard access to context menus, `gg` and counts.

## One rule for text fields

A text field is **open** while it has the caret and **closed**
otherwise. The rule, everywhere:

- **Esc closes the field and keeps its text.** Where the surface has a
  commit step (a table cell writing back into the row), closing commits.
- **Enter keeps its current meaning** in every surface: commit a cell,
  confirm a modal, apply a jq filter, run a search.
- **Discard is undo.** ctrl+z inside the open field walks its edits
  back; after the field has closed, ctrl+z reverts the commit through
  the app history, which already restores exactly.

No key reverts a field's text on the way out. The old "Esc reverts"
behaviour is gone from the five surfaces that had it: table cell, URL
line, Variable Manager form field and grid cell, Settings field.

### Where focus lands when a field closes

| Surface | Today's Esc | Now |
|---|---|---|
| Table cell (`table_editor.rs`) | revert, back to row | commit, back to row |
| Variable Manager grid cell | revert | commit, back to grid |
| Variable Manager form field | revert | keep, back to form navigation |
| URL line (`editor.rs`) | abandon, blur | keep, blur (sub-focus `None`) |
| Settings field | cancel edit | keep, back to the row |
| jq bar (`response.rs`) | revert filter, blur | keep, blur; the filter is live already, so Esc and Enter now do the same thing. alt+shift+q remains the explicit off switch |
| Response search bar | close search | same as Enter: run the search and return to the tree. Esc *in the tree* still clears the search, as today |
| Body editor | blur | unchanged: the body is live in the request, Esc is blur |
| Form modal field | cancel modal | close the field onto the modal's button row (next section) |
| Filter picker (palette, chooser, file picker, variable picker) | close | unchanged, one press. See "Filter pickers" |

### Form modals: the button row

`Modal::Prompt`, `Modal::NewProject`, `Modal::MultiPrompt` and
`Modal::FieldsEditor` all paint a Cancel and a Confirm button already
(`draw_cancel_confirm_row`, hits `ModalCancel` / `ModalConfirm`). They
gain a focus state for it. Each such modal carries

```
enum FormFocus { Field(usize), Buttons(FormButton) }
enum FormButton { Cancel, Confirm }
```

where `Field(i)` is exactly today's focus index (`Prompt` = 0,
`NewProject` name/path = 0/1, `MultiPrompt` = its field index,
`FieldsEditor` = its row). Any non-field row a modal has today (the
NewSelector prompt's toggle row) counts as a field position for this
purpose.

Keys while in `Field(i)`: unchanged, except **Esc → `Buttons(Confirm)`**.
Enter still confirms the modal in one press: forms submit on Enter.
Tab / BackTab still cycle among the fields only, so a user who never
presses Esc sees no change.

Keys while in `Buttons(_)`:

- ←/→, h/l, Tab / BackTab: aim the other button.
- Enter: activate the aimed button.
- Esc: Cancel, whichever button is aimed.
- ↑: back to the field focus was in. Any printable key is swallowed;
  the footer's `↑ back` chip is the way home.

Painting: the aimed button takes the focused control state the Settings
segments already use; nothing else moves.

**Mouse.** `Hit::ModalCancel` today synthesizes an Esc key event. Under
the new rule a synthesized Esc inside a field would only close the field,
so the Cancel click dispatches the modal's cancel directly (a
`ModalResult { close: true }` with no actions). `Hit::ModalConfirm`
keeps synthesizing Enter, which still confirms from any focus. Clicking
a field's input (`Hit::ModalInput`) sets `Field(i)` as it does today,
including from the button row.

**Secret prompt.** Its cancel still cancels the whole send; it just now
happens from the button row. One more press, and the footer shows
`esc cancel` there and not while typing.

**`Modal::Confirm`, `Message`, `ConfigStartup`, `ConfigEditInvalid`,
`Dropdown`** have no text field and are unchanged.

### Filter pickers

The palette, chooser, file picker and variable picker are not forms:
the filter box *is* the picker, there is nothing to submit but a row,
and one-press Esc is the fuzzy-finder convention everyone knows. They
keep one-press close. Their footer chip reads `esc close`, never
`esc cancel`, and they get no button row.

### Footer

The footer is what makes the two-press cancel legible:

- Open field, anywhere: `esc done`. The chip carries the surface's close
  action so it stays clickable. Never `esc cancel` while a field is open.
- Form modal button row: `enter confirm` (or the modal's verb), `esc
  cancel`, `↑ back`.
- Filter picker: `esc close`.
- In-flight send and jq-edit chips (`footer.rs:78`, `:163`) are
  relabelled to match: the jq bar shows `esc done`; the in-flight
  `esc cancel` is unchanged because it is not a field.

## Undo inside a text field

### Two text classes

Every single-line field is a `LineInput` (`components/line_input.rs`):
the URL line, table cells, jq and search bars, the modal fields, file
picker name, Settings edits, Variable Manager fields. The body editor is
the multi-line class on edtui. That stays two classes. The three filter
boxes that are bare `String`s today (`palette.rs:401`,
`chooser.rs:41`, `var_picker.rs:166`) move onto `LineInput`, so they
gain a caret, word navigation, paste flattening and undo like every
other field. They are append-and-backspace boxes today, so the change
is a type swap plus using the existing `draw_line_*` helpers.

### The history

`LineInput` gains a step history:

- A step is a run of typed characters, a run of deletions (Backspace or
  Delete in one direction), or one paste. A cursor move, a selection
  change or a change of run kind starts a new step. This is
  deterministic (no timer), which keeps the tests plain.
- `undo()` restores the text and caret from before the newest step and
  pushes it on the redo stack; `redo()` reverses that. Any new edit
  clears the redo stack.
- The stack is bounded (the app history's `MAX_STEPS` is fine) but in
  practice a field's life is short.

### Two histories, one handover

The model, for every single-line field:

- **While the field is open**, ctrl+z / ctrl+shift+z walk the field's
  own step history. The app history records nothing for that field's
  edits during this time.
- **When the field closes** (Esc or Enter), its final state becomes
  **one** step in the app history, whatever the surface's commit path
  is. Open it again, change it, close it: a second step. Cancelling a
  modal produces no step.
- **The field's own history is dropped on close.** A later ctrl+z
  outside the field steps through closes, never through keystrokes.

This holds for fields that write into the request live, too. The jq
bar and the URL line sync into the request on every keystroke (the jq
bar so the filter stays live, `sync_jq`), and today `capture_undo`
turns those live writes into app-history steps in two-second bursts.
That capture is **gated while the field is open**: the shadow copy the
diff runs against stays at the pre-edit state, and the close triggers a
single capture, which yields exactly one step. The live view keeps
updating; only the recording waits. Consequences:

- `Action::CancelJqEdit` (Esc reverts the bar to the text at focus
  time) is deleted; ctrl+z inside the bar replaces it.
- An in-field undo that returns the text to where it started, followed
  by a close, is a no-op diff and records nothing.
- A send fired while the URL line is open (ctrl+enter) uses the live
  text as today; the undo step still lands on close.

**Exception: the body editor.** It is a document, not a field, and has
no close. It keeps its current burst coalescing in the app history; one
step per blur would make a long editing session a single undo. When the
vim mode lands, `u` in the body is edtui's own undo.

### Lifetime

The field history lives as long as the `LineInput` instance holds an
open edit:

- A table cell, grid cell, Settings edit or Variable Manager field
  creates its input when the edit opens and drops it on close.
- A form modal's inputs live as long as the modal. Tab out to the
  buttons, come back, and undo still works. Cancel or confirm the modal
  and the history is gone. Nothing in a modal touches the app history
  until Confirm dispatches its action, which is one step as today.
- The URL line and jq bar inputs are long-lived; their history is
  cleared on close, per the handover above.

### Routing

Undo and redo reach the field through the keymap's actions, not through
a hard-coded ctrl+z inside `LineInput`, so `keys.toml` rebinds keep
working:

- On the Main screen modified combos go to the global keymap before the
  focused component (`handle_key_inner` step 7). `Action::Undo` /
  `Action::Redo` therefore ask the focused component for its open text
  field first; if there is one with history, the field undoes and the
  app history is untouched. Otherwise today's behaviour.
- Modals and non-Main screens capture keys before the global keymap.
  The router already digs a bound Paste past modals (step 3); Undo and
  Redo get the same carve-out: if the combo is bound to Undo/Redo and
  the top modal (or the Manage screen's focused surface) has an open
  field, it undoes there. Otherwise the modal swallows it as today.
- `LineInput` itself exposes `undo()` / `redo()` and never matches the
  combo.

## Aliases

Every alias is a strict synonym for an existing key in that surface. A
mouse or arrow user sees no change; the footer keeps advertising the
arrow form, and an alias appears in the footer only where it is the sole
key (the two relabels).

### Global (default keymap, plain keys)

Plain keys reach the focused component first and fall through to the
global keymap only if unclaimed, so a text field still gets the
character.

- `:` → `Action::OpenPalette`, alongside ctrl+p.
- `u` → `Action::Undo`. Unclaimed in every list surface. Redo stays
  ctrl+shift+z; ctrl+r is Send.
- `q` stays as it is.

Both are named actions already, so they appear in `keys.toml` as extra
combos and can be unbound.

The Manage screens swallow every plain key they do not name, so they
claim `:` and `u` themselves — in `manage_list`'s `handle_key`, the
Variable Manager's list and grid handlers, and `handle_settings_key` —
rather than letting them fall through to the global keymap.

### Per surface

| Surface | j/k | h/l | g/G | ctrl+d / ctrl+u | ctrl+f / ctrl+b |
|---|---|---|---|---|---|
| Sidebar tree | have | ←/→: collapse-or-parent, expand | first/last row | half page | full page |
| Response viewer | have | ←/→: horizontal scroll | have | half page | full page |
| Table editor (nav) | have | — | first/last row | half page | full page |
| Manage lists | ↑/↓ | — | first/last | half page | full page |
| Variable Manager list | ↑/↓ | ←/→: list⇄grid | first/last | half page | full page |
| Variable Manager grid | ↑/↓ | ←/→ | first/last row | — | — |
| Settings tab | ↑/↓ | ←/→: aim a Files row's buttons | first/last row | — | — |
| Context menu / dropdown | ↑/↓ | — | first/last | — | — |
| Form modal button row | — | ←/→: aim | — | — | — |

"—" means the surface has no such motion today and none is invented.
Half page is the visible height / 2 rounded down (minimum 1), full page
is the visible height, both clamped like PageDown. Surfaces that have no
PageUp/PageDown today get the page motions implemented on the same
cursor-move path their arrows use.

**Filter pickers** (palette, chooser, file picker, variable picker):
letters are the filter, so ctrl+n / ctrl+p move down / up, the vim
completion-menu convention. Modals capture before the global keymap, so
ctrl+p there never opens the palette. No page keys: ctrl+u is
kill-to-start in a text box.

**Text fields and the body editor** are untouched. Readline keys stay.

### Relabels

- **Response views**: `r` (raw⇄pretty) and `h` (headers⇄body) are
  replaced by a single `t` that cycles Pretty → Raw → Headers → Pretty.
  The tabs stay clickable and the palette's response-toggle command
  stays, so this is the keyboard's one path rather than two, and it
  frees `h` for the motion without a shifted letter. The two footer
  chips (`r raw`, `h headers`) become one `t view` chip; hover hints
  follow.
- **Variable Manager new selector**: `g` → `a`, matching the table's
  `a add row`. Footer chip and hint follow.

`g` for top-of-list is deliberately a single key: pressing it twice
(`gg` by habit) goes to the top twice, so no prefix-key machinery is
needed in this round.

## Testing

- **Field rule**: per surface, an Esc test asserting the typed text
  survives and focus lands where the table above says; the old
  "Esc reverts" tests are inverted, and a new one per surface shows
  ctrl+z in the open field restoring the original.
- **Form modals**: Esc from a field lands on `Buttons(Confirm)`; Esc
  there closes with no actions; Enter there activates the aimed button;
  ↑ returns to the same field index; Enter in a field still confirms in
  one press; a Cancel click closes from inside a field; `Modal::Confirm`
  and friends are untouched.
- **Undo**: `LineInput` unit tests for step boundaries (typing run,
  deletion run, paste, cursor move splits), redo cleared by a new edit,
  and the bound; router tests that ctrl+z with an open cell does not pop
  the app history, and that it does once the cell has closed; the
  modal carve-out mirrors the existing paste carve-out test. For the
  live-synced fields: typing in the jq bar across more than two seconds
  then closing yields exactly one app-history step, a second open/edit/
  close yields a second, and an in-field undo back to the original
  followed by close yields none. The body editor's burst test is
  unchanged.
- **Aliases**: one table-driven parity test per surface, each row an
  (alias, canonical key) pair asserted to produce identical state from
  the same starting state, including the clamps at the ends.
- **Footer**: `esc done` whenever a field is open and never `esc
  cancel`; the button row's three chips; `esc close` in pickers; the
  two relabels; `every_named_action_is_mouse_reachable` still passes
  with `:` and `u` added.
- **Filter boxes on `LineInput`**: existing palette/chooser/var picker
  typing and paste tests pass unchanged; new tests for caret movement
  and undo in the filter.

## Deferred

- **Vim mode itself**: the `keymap = "vim" | "default"` setting, the
  first-launch choice (a `Modal::Confirm` with two choices, triggered by
  the key being absent from `config.toml`), the Settings row (copy of
  the `jq_tab` segments), edtui's native vim mode in the body editor,
  and an insert/normal layer in `LineInput`. In that mode the field
  rule gains one step: Esc leaves insert mode first, then closes the
  field. Everything else in this spec stays as is.
- **Footer label profile**: chips are `&'static str` today. Vim mode
  should show the vim spelling (`j/k navigate`) where sane mode shows
  arrows, and some vim keys may be worth showing in sane mode too. Needs
  each chip to carry both spellings and the footer to pick by mode.
- **Sidebar `/` filter**: a new filter row and state, its own round.
- **Keyboard access to context menus**: no key opens one today.
- **`gg`, counts, operator+motion**: need a pending-key buffer in the
  router and a sequence-aware keymap; only worth it with the vim mode.
