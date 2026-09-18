# Field States: Selected and Open

Every single-line text field in postui is the same `LineInput`
(`components/line_input.rs`), but each surface wraps it in its own
focus rules. Three of those rules leak: the URL line starts typing the
moment `k` lands on it, so `j`/`k` stop navigating; after Esc the
blurred editor answers arrows but not `j`; and a modal field's edits
cannot be undone from the modal's button row, while every other field's
commit can be undone from anywhere. This round gives every field the
same two states and the same undo levels, and folds the Settings tab
into the app history so its commits undo like the rest.

The previous round (`2026-09-15-shared-aliases-and-field-esc-design.md`)
defined a field as *open* while it has the caret and *closed*
otherwise, and left each surface to decide when a field opens. This
spec decides that once.

## Scope

One branch, on top of `shared-aliases-field-esc`. User-visible changes:

- A field navigation lands on is **selected**, not open. Enter, Space
  or `i` opens it. Esc and Enter close it back to selected.
- The URL line gets that selected state; `j`/`k` work on and around it.
- Form modals get the same states. Esc from a selected field goes to
  the button row; shift+Enter or ctrl+Enter confirm from any field.
- Undo outside an open field steps commits on every surface. In a
  modal that means field closes; on the Settings tab it means config
  writes, which become app-history steps, Reset included.
- The table's Enter keeps the row selected instead of dropping it.

Out of scope: vim mode itself, `a`/`A`/`I` as open keys, a shared
field widget replacing the per-surface state machines (all under
"Deferred").

## The two states

A text field that keyboard navigation can reach is always in one of
two states:

- **Selected.** The field is highlighted and has no caret. It is an
  ordinary navigation stop: arrows, `j`/`k`, `g`/`G`, Tab and BackTab
  move off it exactly as they move between any other stops of that
  surface. Plain letters are not typed; the surface's own letter keys
  (`a` add row, `d` delete, `i` open) and the keymap fall-through
  (`:`, `u`) apply. Home/End are the surface's first/last motions.
- **Open.** The field has the caret and every `LineInput` key: letters
  type, Left/Right/Home/End move the caret, Backspace/Delete edit,
  ctrl+z / ctrl+shift+z walk the field's own step history.

Transitions, the same on every surface:

| Key | Selected | Open |
|---|---|---|
| Enter | opens, caret where the field last left it (end on first open) | closes, keeps text, runs the surface's commit, stays selected |
| Space | opens (same as Enter) | types a space |
| `i` | opens (same as Enter) | types `i` |
| Esc | leaves the field level: the surface's existing "one level out" | closes, keeps text, runs the commit, stays selected |
| Tab / BackTab | next / previous stop, **selected** | closes, then next / previous stop, **open** (a Tab-fill keeps the state it started in) |
| ↑ / ↓, `j` / `k` (plain) | neighbouring stop, selected (in a form modal the button row is the stop below the last field; arrows never wrap) | ↑/↓ close and select the neighbour; `j`/`k` are typed |
| shift+Enter, ctrl+Enter | the container's confirm (send, or confirm the modal) | same |
| ctrl+z, ctrl+shift+z, `u` | the container's commit history | the field's step history (`u` is typed) |

"Runs the commit" is whatever the surface does when a field closes
today: write the cell into the row, sync the URL into the request,
save the setting, keep the modal field's text. Nothing reverts on the
way out; discard is undo, as the last round decided.

**How focus arrives decides the state.** Navigation (arrows, `j`/`k`,
Tab, `g`/`G`) lands selected. An explicit go-there action opens: a
mouse click on the field, `Action::FocusUrl` (the URL shortcut and
`Hit::UrlBar`), the jq and search shortcuts, and a form
modal opening. The distinction is intent: a click or a shortcut names
the field to type into; a `k` only passes through it.

**Fields navigation cannot reach have no selected state.** The jq bar,
the response search bar and the filter pickers' boxes are only ever
entered by a shortcut or a click, which open, and their Esc goes
straight back to the surface behind them. They are unchanged by this
spec, and their Esc chip stays `esc done` / `esc close`.

The **body editor** is a document, not a field, and is untouched: it
has no selected state and Esc is blur.

## Per surface

### Request panel address bar (`editor.rs`)

`SubFocus::Url` gains an open flag (`Editor.url_open: bool`, `false`
whenever `sub_focus != Url`). `Editor::plain_keys_type` becomes
`sub_focus == Url && url_open`.

- `k` / ↑ from the tab strip → `Url`, selected. `l` / → from the
  method badge → `Url`, selected. ↓ / `j` from `SubFocus::None` →
  `Url`, selected.
- Selected: Enter / Space / `i` open. ← / `h` → `Method`. ↓ / `j` →
  `Tabs`. Esc → `None` (blur, as the method badge and tab strip do).
  `:` and `u` fall through to the keymap; other letters do nothing.
- Open: as today. ← at caret 0 → `Method` (closes first). ↓ closes
  and lands on `Tabs`. Enter / Esc close to selected, sub-focus stays
  `Url`. ↑ closes and stays selected (there is nothing above).
- `Action::FocusUrl` and `Hit::UrlBar` open.
- `SubFocus::None`: ↓ and `j` both → `Url` selected (`j` is the one
  alias `1a000de` missed).
- `App::flush_field_session` and `field_gate` key off `url_open`, not
  `sub_focus == Url`: a selected URL line is a closed field, so the app
  history captures normally while it is selected.

Send: shift+Enter / ctrl+Enter send from either state, as today.

### Table cells (`table_editor.rs`)

Already selected-vs-open. Two changes:

- Enter in an open cell commits and **keeps the row selected**, like
  Esc; the row stays expanded. A commit warning still points the
  selection at the offending row as today. Esc on a selected row
  deselects as today.
- `i` opens the selected row's cell, as Enter does.

Tab / BackTab, ↑ / ↓ from an open cell keep their current meaning
(commit, then move; the neighbouring row is selected, not opened),
which already matches the table above.

### Settings tab, Variable Manager form and grid, Manage lists

Already selected-vs-open with Enter / Space opening. Add `i` as a
third open key on each. No other change to their focus rules.

### Form modals (`modal.rs`)

`ModalStack.button_focus: Option<FormButton>` becomes

```
enum FormFocus {
    Field { idx: usize, open: bool },
    Buttons(FormButton),
}
```

held on the stack as today (cleared by push / pop), where `idx` is
each modal's existing focus index (`Prompt` = 0, `NewProject` name /
path = 0 / 1, `MultiPrompt` = its field index, `FieldsEditor` = its
row). A non-text stop at a field position (a `MultiPrompt` choice
field, the NewSelector toggle) is never open: Enter / Space activate
it, ← / → cycle it, as today.

**On open, the first field is open.** A prompt exists to be typed
into; "rename → type" stays one motion. `Prompt`, `NewProject`,
`MultiPrompt` and `FieldsEditor` all start at `Field { idx: 0, open:
true }` (the secret prompt too).

Keys in `Field { open: true }`: letters type; Enter and Esc close to
`Field { idx, open: false }`; Tab / BackTab close and open the
neighbouring field; ↑ / ↓ close and select the neighbour; ctrl+r
toggles the secret prompt's reveal; shift+Enter / ctrl+Enter confirm.

Keys in `Field { open: false }`: Enter / Space / `i` open; ↑ / ↓,
`j` / `k`, Tab / BackTab select the neighbouring field (Tab wraps
within the fields, as today); Esc → `Buttons(Confirm)`; shift+Enter /
ctrl+Enter confirm; ctrl+z / ctrl+shift+z / `u` step the modal's
history (next section); every other key is swallowed.

Keys in `Buttons(_)`: unchanged from the last round: ← / → / `h` / `l`
/ Tab / BackTab aim, Enter activates, Esc cancels, ↑ / `k` return to
the field, **selected**; ctrl+z and `u` step the modal's history.

**The button row is the bottom stop** (ruling 2026-09-17, after the
round shipped: "↑ gets from the buttons to the input, but ↓ doesn't get
from the input to the buttons"). ↓ / `j` from the last field — the
prompt's only field, the new-selector prompt's toggle, `NewProject`'s
path, `MultiPrompt`'s last field, the fields editor's last row — land
on `Buttons(Confirm)`, closing an open field first (a close step is
recorded; `j` in an open field is still typed). ↑ / `k` from the row
return to that field, selected, as before. Arrows never wrap: ↑ from
the first field stays put. Tab / BackTab keep cycling within the
fields and never reach the row, so the 2026-09-15 spec's "Tab / BackTab
still cycle among the fields only" holds. The fields editor's ↑ / ↓
now land selected like every other surface (they used to share Tab's
keep-the-state arm), and a selected row keeps its `alt+a` / `alt+d`
footer chips, which never needed the caret.

**Confirm from a field.** The keymap's `Send` bindings (shift+Enter and
ctrl+Enter by default) are the "confirm the container" keys: on the
main screen the container's confirm is Send, in a form modal it is
Confirm. The router's dig-past step (`handle_key_inner` 1b/1c) gains
a sibling: a modified combo bound to `Action::Send` with a form modal
on top confirms that modal, from any focus, closing an open field first
(its close records a step, then the modal's confirm dispatches). A
`keys.toml` rebind of Send therefore rebinds modal confirm too. Plain
Enter is not bound to Send, so it never confirms from a field. On a
terminal without the kitty keyboard protocol shift+Enter arrives as
Enter and closes the field; the fallback is Esc, Enter on the aimed
Confirm button.

Cancelling from an open field is three Escs (open → selected →
buttons → cancel), each one the same "one level out" the rest of the
app uses. The footer names each press.

**Mouse.** A click on a field's input opens it at the click (from any
focus). `Hit::ModalCancel` dispatches cancel directly and
`Hit::ModalConfirm` confirms directly, both from any focus, as the
last round set up.

**Filter pickers** (`Palette`, `Chooser`, `VarPicker`, `FilePicker`)
are not forms and are unchanged.

## Undo: the same two levels everywhere

Every surface has a field level and a commit level:

- **In an open field**, ctrl+z / ctrl+shift+z walk the field's own
  `LineInput` step history. `u` is a letter and is typed.
- **Anywhere else**, ctrl+z / ctrl+shift+z and `u` step the
  container's commit history: a step per field close whose text
  changed, and a step per other committed change.

On the main screen and the Manage screens the commit history is the
app `History`. This round adds the two containers that lacked one.

### Modals: a per-modal step stack

`ModalStack` gains `steps: Vec<FieldStep>` and `redo: Vec<FieldStep>`
for the top form modal, cleared on push and pop:

```
struct FieldStep { idx: usize, before: FieldValue, after: FieldValue }
enum FieldValue { Text(String), Choice(usize), Toggle(bool) }
```

- A field close (Enter, Esc, Tab, ↑ / ↓, a click elsewhere, the
  confirm chord) whose text differs from the text at open pushes one
  step and clears `redo`. A choice or toggle change pushes one step
  per change.
- ctrl+z or `u` from a selected field or the button row pops the
  newest step, restores `before` into field `idx`, selects that field
  (not open), and pushes the step onto `redo`. ctrl+shift+z / ctrl+y
  reverse it. Out of steps: nothing happens, no toast; the modal's
  fields visibly change, so no toast on success either.
- The field's own `LineInput` history is cleared on close
  (`end_edit`), as the last round decided: undo outside the field
  steps closes, never keystrokes. Reopening a field starts a fresh
  keystroke history.
- Cancel discards the stack. Confirm dispatches the modal's action,
  which is one app-history step as today.

Routing: the 1c carve-out digs a modified Undo / Redo combo past the
modal when the top modal has either an open field with keystrokes
(`field_edited`) or steps on its stack. Plain `u` reaches the modal
as a swallowed key today; a form modal's handler, when its focus is
not an open field, checks an unclaimed plain key against the same
whitelist the Manage screens use (`App::unclaimed_screen_key`), so
`u` → `Action::Undo` → the modal's stack, and a `keys.toml` unbind of
`u` holds there too. `Action::Undo` / `Redo` therefore ask, in order:
open field with keystrokes → top form modal's step stack → (a modal is
open: stop) → app history. Filter pickers keep their own
`undo_filter` path.

### Settings: config writes become history steps

Every Settings write today goes through `Action::SetUiFlag`,
`SetUiString`, `SetUiInt` and `ForceResetConfigFile` (`app.rs:2607`
onward) and records nothing, so a closed Settings field cannot be
undone, and a ctrl+z on that tab silently undoes the last request edit
behind the screen. `undo::StepKind` gains two variants:

```
Config     { key: &'static str, before: UiValue, after: UiValue }
ConfigFile { file: ConfigFile, before: Option<String>, after: String }
```

- `apply_ui_write` records a `Config` step on a write that landed
  (`Ok`), with `before` read from `ui_settings` before `apply` runs.
  A refused write records nothing. Text, flag, int and the `jq_tab`
  segments all pass through here, so every Settings control gets undo
  from the one arm. Undo writes `before` back through the same
  `save_ui_*` path and `reapply_ui_settings`; redo writes `after`.
  Coalescing: none; each commit is a step.
- `ForceResetConfigFile` reads the file's current bytes through
  `Config` before overwriting (`None` when the file did not exist),
  writes, then records a `ConfigFile` step. Undo writes `before` back
  (or removes the file when `None`) through `Config::write_validated`
  and reloads; redo writes `after`. `Config` stays the sole owner of
  config-file I/O.
- Toasts follow the file-change wording: `Undid change to AI command`,
  `Undid reset of config.toml`. Labels come from
  `SettingsField::label` and `ConfigFile::name`.
- A `Config` step whose key no longer parses on undo (the file was
  hand-edited meanwhile) toasts the refusal as `apply_ui_write` does
  and is popped anyway, matching how a skipped `Project` marker
  behaves.

`u` and ctrl+z on the Settings tab already reach `Action::Undo`
through the whitelist; the arm's "commit the open table edit" prelude
becomes "close any open field on the current screen", so a live
Settings edit is committed (one step) and then that step is what the
undo pops. This is the same order the table gets today.

### Footer

- Selected field: `enter edit` (with `space` and `i` unadvertised, as
  aliases are), plus the surface's motions. In a form modal, also
  `⇧enter confirm` and `esc buttons`.
- Open field: `esc done`, as today; in a form modal also
  `⇧enter confirm`.
- Button row: unchanged (`enter confirm`, `esc cancel`, `↑ back`).
- Hover hint on a field: "Edit" when selected, "Done" when open.
- `u undo` is advertised where it already is; a modal with steps on
  its stack shows it on the selected-field and button-row rows.

## Implementation shape

Per-surface state additions, one shared helper, two history variants.

- `keys.rs`: `pub fn opens_field(ev) -> bool` (Enter, Space, plain
  `i`) so the open keys are written once and every selected-field arm
  calls it. `plain_letter` already guards the aliases.
- `editor.rs`: `url_open`, the selected arm, the `None` alias.
- `modal.rs`: `FormFocus`, the step stack, the open-on-push default,
  the unclaimed-key whitelist hook.
- `app.rs`: `open_text_field_mut` / `field_open` /
  `open_text_field_edited` become "open, not merely selected"; the
  Send-confirm dig-past; `Action::Undo` / `Redo` consult the modal
  stack; `apply_ui_write` and `ForceResetConfigFile` record steps;
  `History` undo / redo arms replay the two new kinds.
- `undo.rs`: the two variants and their toast wording.
- `table_editor.rs`: Enter keeps the selection; `i` opens.
- `settings.rs`, `varmanager.rs`, `manage_list.rs`: `i` opens.
- `footer.rs`, `hint.rs`: the chips above.

A shared `FieldStop` widget owning `LineInput` plus the open flag and
the whole key table would remove the per-surface duplication, but the
three surfaces already on the model agree with each other, so the
refactor buys nothing this round. Listed under "Deferred".

## Testing

- **URL line**: `k` from the tab strip lands selected and a following
  `j`/`k` navigates; Enter / Space / `i` open; Esc and Enter close to
  selected with the text kept; Esc from selected blurs; `j` from
  blurred selects the URL like ↓; `FocusUrl` and a click open;
  `plain_keys_type` is false while selected. The existing
  `up_down_walks_url_tabs_content_and_back`, the alias-parity sweep and
  `tests/focus_deselect.rs` are updated to the new landings. The
  field-gate tests (`a_url_edit_lands_as_one_step_when_the_line_closes`
  and friends) pass with the gate keyed on `url_open`.
- **Table**: Enter in an open cell keeps the row selected; a warning
  still moves the selection; `i` opens.
- **Modals**: the modal opens with field 0 open; Enter / Esc close to
  selected; Enter / Space / `i` reopen with the text intact; Esc from
  selected lands on `Buttons(Confirm)` and Esc there cancels; ↑ from
  the buttons lands selected; Tab from an open field opens the next,
  Tab from a selected field selects the next; ↑/↓ from open select the
  neighbour; shift+Enter and ctrl+Enter confirm from open, selected
  and the button row, and a `keys.toml` rebind of Send moves them;
  plain Enter never confirms from a field; a Cancel click cancels from
  any focus; the secret prompt follows the same path. The last round's
  `the_button_row_hides_the_field_from_paste_and_undo` inverts for
  undo (paste still needs an open field).
- **Modal undo**: a close with changed text is one step and a close
  with unchanged text is none; ctrl+z and `u` from selected and from
  the button row restore `before` and select the field; redo restores
  `after`; a new close after undo clears redo; a choice change is a
  step; Cancel discards; Confirm dispatches one app step; ctrl+z in an
  open field still walks keystrokes and does not touch the stack; a
  `keys.toml` unbind of `u` holds in a modal; the router test mirrors
  the paste carve-out.
- **Settings undo**: each of a text, flag, int and segment commit
  records one `Config` step; ctrl+z on the tab reverts it on disk and
  in `ui_settings` and redo reapplies; a refused write records
  nothing; a Reset records a `ConfigFile` step whose undo restores the
  previous bytes (and removes a file that did not exist) and reloads;
  a ctrl+z on Settings with a live edit commits it first and then
  pops that step, not the last request edit; toast wording per kind.
- **Aliases**: `i` added to each surface's parity table as a synonym
  of Enter on a selected field; the response, sidebar and Manage list
  sweeps are untouched.
- **Footer**: `enter edit` on a selected field everywhere, `esc done`
  when open, the modal's `⇧enter confirm` on both field states,
  `esc buttons` on a selected modal field, and
  `every_named_action_is_mouse_reachable` still passes.

## Deferred

- **`a` / `A` / `I` as open keys** with caret placement: `a` is "add
  row" in the table and "new selector" in the Variable Manager, so
  they wait for the vim-mode round and its keymap profile.
- **Vim mode**: the selected / open pair already is the normal /
  insert layer the last round deferred for `LineInput`, so that round
  no longer needs one of its own; it adds the keymap setting, the
  first-launch choice, edtui's vim mode in the body, and the footer
  label profile.
- **A shared `FieldStop` widget** folding `LineInput` plus the open
  flag plus the key table into one type each surface embeds.
- **Tab from an open table cell** keeping the open state (Tab-fill
  across key and value): the table's Tab is left as it is this round.
- **Undo for the Variable Manager's non-text controls** already lands
  as project journal steps; nothing to do, noted for completeness.
