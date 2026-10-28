# Header copy keys (#136, #137)

## Problem

- **#136.** In arrows, `y` on an auto header row copies its value
  (spec 2026-10-25 S14). On the Headers tab's own rows, `y` only fills the
  hidden row register that `p` pastes from (piece 4 spec §5.1: no clipboard,
  no toast). So the keyboard can't copy a main row's value, though its copy
  pill can.
- **#137.** Nothing copies a whole header (`Content-Type: application/json`),
  by key or by mouse, on the request or in the response.
- **Shift only partly drives it.** The task proposed shift switching the
  button, the key and the footer text. Terminals send no event for shift
  alone unless kitty's report-all-keys mode is on (not in Terminal.app or
  tmux): the kitty-only footer switch is #146, after 0.2. Most terminals
  (kitty, Ghostty, WezTerm, xterm) keep shift+click for their own text
  selection, so a pill sees shift+click only where the terminal passes it
  on (H6). *2026-10-27 (#159):* in practice none did, Ghostty included, so
  the pill's whole-header click moved to alt.
- **Two copy letters.** In arrows, the response Headers tab copies a value
  with `c`, with no footer chip. The tables and the sidebar copy with `y`.
- **Vim.** Vim's `yy` never touches the system clipboard; `"+yy` does.
  postui's vim body and response already follow that (the response's `yy`
  fills the engine's register, `you`, spec 2026-09-27 M5a). On lists and the
  response, every `"` + key gives the note "Registers are not supported
  here" (`keyroute/table.rs`, the `"<any>` row), so a vim user has no key
  that copies a header to the clipboard.

## Rules

| # | Rule | Who |
|---|---|---|
| H1 | Arrows `y` on a main row of Params, Headers or Vars fills the row register (`p` still pastes it) **and** copies the row's value to the clipboard, as the row's copy pill does (same toast). The sidebar's request `y` stays invisible. | mine (approved) |
| H2 | Arrows `Y` on a Headers-tab row, main or auto, copies the whole header, `Name: value`. The register is untouched. On Params and Vars rows, and on `+ Add`, `Y` gives the note "No header to copy here" (*amended at review round 1:* it did nothing, a dead key). | mine (approved): `Y` is the shift of the copy letter; the note is mine |
| H3 | A whole-header copy uses the real name and value, even when the row shows them masked (as the value copy does, spec 2026-10-18 R7). The toast names the header as the screen shows it: `Copied Content-Type header`. | mine (approved) |
| H4 | The response Headers tab copies with `y` (the value) and `Y` (the whole header) in arrows, instead of `c`. `c` is unbound there. Sent and received rows both copy. On the divider or a note they give "No header to copy here". On the body views they stay silent, as `y` was before (*amended after review block 1:* a toast on every reflex `y` there was noise). | mine (approved); the note and the silence are mine |
| H5 | One footer chip, never two: on a Headers-tab row (request main or auto) and on a response header row, the chip is `copy`, with keycap `y/Y` in arrows. On a Params or Vars row it's `copy` with `y`. The response Headers tab gains this chip (it has none today): `y/Y` in arrows, `"+yy` alone in vim, whose `yy` there is the engine's line yank (*amended at review round 5:* the vim chip used to drop). In vim the request rows' chip reads `yy/"+yy` (`yy` copies the row for `p`). A chip's click is its row's pill: the value to the clipboard, in both profiles, whatever the keycap's yank does. The new chip costs the footer ten cells: a client-owned row's error message (#59) now needs 150 columns, not 140, since chips win over it. | you ("I don't want two chips in the header at the same time"); the `y/Y` keycap mine (approved); the click mine (approved, Q1 after review block 1) |
| H6 | Mouse: a copy pill's click copies the value, as before; its alt+click copies the whole header, on a Headers-tab main row, an auto row and a response header (a Params or Vars pill copies the value whatever is held). A shift+click a terminal passes on is a plain click. Where a terminal or window manager keeps alt+click for itself (iTerm2 is said to keep option+click; window managers that move windows on alt+drag), the whole header is `Y`. A header's pill says so on hover: "Copy value · alt+click: whole header" (`opt+click` on macOS, as the footer spells alt), in every terminal; a footer shows a tip only with 24 free cells and ellipsizes it below its length. *History:* right-click menus with `Copy header` were built first, and dropped after review block 1. The click was shift+click until 2026-10-27 (#159): terminals keep shift+click for their own selection under mouse capture, so it never reached the pill, even in Ghostty. Alt alone, not alt or shift, was weighed at #159's review round 1. | you ("can we just do shift click for kitty or terminals that support it?"; "Shorten it"; #159: "Can we switch the copy full header to alt instead of shift?"); the hover mine (approved); dropping shift is mine (approved, #159 review round 1); the hover's alt spelling is mine |
| H7 | Vim: `yy` on a main row is unchanged (the register only, as Vim's `yy` never writes the clipboard). | Vim |
| H8 | Vim: `"+yy` and `"+Y` on a Headers-tab row (main or auto) or on a response header row copy the row as its line, `Name: value`, as Vim's `"+yy` copies a line. Where the cursor is on no header, `"+yy` gives "No header to copy here" (H2, H4). Any other key after `"+` or `"+y` keeps the "Registers are not supported here" note. | Vim (the line); the scope is mine (approved) |
| H9 | Vim: `yy` on an auto row keeps copying its value (S14), though Vim's `yy` would not touch the clipboard. Without it, an auto row's value has no vim key at all. | mine (approved), a deviation from Vim |
| H10 | The shift-switching footer chip in kitty-protocol terminals is #146, not this task. | mine (approved) |

## Design

- **Actions.** `CopyTarget` gains `ResponseHeaderLine`, `ComputedHeaderLine`
  and `TableHeaderLine` (a Headers-tab main row), the whole-header twins of
  `ResponseHeader`, `ComputedHeader` and `TableRow`. `resolve_copy` builds
  `Name: value` for them, one helper for all six, which asks
  `CopyTarget::whole_header` (an exhaustive match).
- **One copy action.** `Action::CopyResponseHeader` becomes
  `Action::CopyRow { whole }`, for the table and the response alike
  (arrows `y` on the response, arrows `Y` and vim `"+yy`/`"+Y` on both).
  `App` resolves it against the focused pane, through
  `Editor::row_copy_target` or the response's `copyable_header_row`,
  and is the one place that gives the "No header to copy here" note.
  *Amended at review round 2:* a `Verb::CopyHeader` list verb first did
  the table's half; it resolved to a note off a header, which broke
  `selection_action`'s "None means no meaning here", and put the note in
  two places.
- **Yank.** `Verb::Yank` stays the register yank in both profiles. Arrows'
  clipboard half of `y` (H1) is one profile check in `App`'s
  `YankTableRow` arm: in arrows it copies the value after filling the
  register. A separate arrows-only copy verb was rejected: the
  reachability test wants every verb that resolves on a surface to have a
  vim key, and vim has none for it.
- **Router.** `best` used to return a complete `<any>` row before a longer
  exact row that extends the keys, so `"+` hit the `"<any>` note. A longer
  exact row now wins, so `"+` waits for `yy`/`Y`. This is Vim's rule, kept
  general on purpose (mine (approved), Q2 after review block 1). New
  `"+<any>` and `"+y<any>` rows, on the table and the response only (where
  `"+yy` exists), keep the note for anything else. The response's copy
  rows bind on `Surfaces::RESPONSE_HEADERS`, a class the Headers view
  carries with `RESPONSE`, so the body views route `y` and `"+yy` exactly
  as before this branch (*amended at review round 6:* `App` used to
  re-decide by view mode and profile).
- **Footer.** A header row's chip is `Key::Pair(yank, CopyRow{whole})`,
  spelled `y/Y` in arrows and `yy/"+yy` in vim. A Params or Vars row's chip
  is the yank key alone. Every row chip's action is its pill's
  `CopyToClipboard` naming the row, as the delete chip names its row, so
  the action never depends on where the cursor is when it runs.
  (*Amended 2026-10-27, task #125, mine:* this used to say a chip click is
  a click away, which drops the table's cursor first. The table's
  cursor-row chips now keep the cursor and the table's caret, as their
  keys do. *Amended 2026-10-27, task #183, mine:* so does every other chip
  on the main screen's footer while the table's cursor is up on any stop
  (a row, an auto row, a section row, `+ Add`), with no cell open (send or
  cancel, add, tabs, commands, quit). The app bar's save/discard, the
  address bar's lock and the Body toolbar are not the footer and stay a
  click away, as do the Manage screen's footer chips. *Amended 2026-10-27,
  task #192, mine:* the same holds with a table cell open, with the Body's
  caret up and with the URL line selected: a footer chip's click leaves the
  cell, its typing, the cursor and the caret as its key does, and its
  action does the rest (send and quit commit the cell, `done` closes it).
  *Amended 2026-10-27, task #194, you ("yes, it should not blur the
  input", typed in answer to whether a chip click should keep the open URL
  line and the method badge's selection):* so it does with the URL line
  open, its typing and caret kept (`done` closes it), and with the method
  badge selected. With no sub-focus a chip click is still a click away, as
  before. *Vim, and you ("Yes let's follow vim for mouse interactions as
  well"):* for vim's engine the press is still a click: it ends Visual and
  drops a half-typed operator and a ctrl+o (settled at the end of the
  event; only the vars chip's picker reads the mode, and takes it as
  alt+v does). Vim:
  gVim's `:amenu` inserts `<C-C>` in Visual/Op-pending (gui.txt; probed in
  Vim 9.1: `d`, an `:amenu` item, `l` deletes nothing). A footer chip is
  that kind of item. Here the click and its key differ: alt+i or
  ctrl+Enter keeps Visual up on the open URL line. The click follows Vim's
  menu rule, the key follows the key.)
  The response chip is `Key::Pair` of the two `CopyRow` actions.
  `TableCursor` gains `headers` on `Row` and a `ResponseHeader` variant
  for the response's cursor.
- **Pills.** The three copy pills' click handlers read the mouse event's
  alt (shift until #159) and pick the `*Line` target.

## Tests

- H1: arrows `y` on a main header row fills the register and copies the
  value (test clipboard sink); vim `yy` fills the register and doesn't copy.
- H2/H3: `Y` on main and auto rows copies `Name: value` with the real
  value of a masked secret; `Y` on a Params row gives the note.
- H4: arrows `y`/`Y` on a response header row (received and sent); `c`
  does nothing; the body views stay silent.
- H5: footer chips per surface and profile; never two copy chips; the
  chip's click copies the value in both profiles.
- H6: each pill's click, alt+click and shift+click; a Params pill copies
  the value.
- H8: the router: `"+yy`, `"+Y` run the copy; `"+x` and `"+yx` still give
  the note.
- H9: the existing S14 tests stay green.

## Choices (`mine (approved)`, user 2026-10-26: "Good")

- H1 also applies to Params and Vars, so `y` means the same on every table.
- H2: `Y` on Params and Vars doesn't copy `name=value`; it gives the note
  (amended at review round 1, mine).
- H3: the toast wording `Copied <Name> header`.
- H8: `"+yy` copies only header rows, and ignores a count (mine
  (approved) after review round 5: header rows only, no dead keys). On
  the response's body views it gives the registers note it had before
  this branch, while arrows' `y` and `Y` stay silent there (H4). Vim's
  line copy on body lines would be a separate task.
- H9: the auto row's vim `yy` stays a deviation from Vim.
