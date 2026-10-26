# Header copy keys (#136, #137)

## Problem

- **#136.** In arrows, `y` on an auto header row copies its value
  (spec 2026-10-25 S14). On the Headers tab's own rows, `y` only fills the
  hidden row register that `p` pastes from (piece 4 spec §5.1: no clipboard,
  no toast). So the keyboard can't copy a main row's value, though its copy
  pill can.
- **#137.** Nothing copies a whole header (`Content-Type: application/json`),
  by key or by mouse, on the request or in the response.
- **Shift can't drive it.** The task proposed shift switching the button,
  the key and the footer text. Terminals send no event for shift alone
  unless kitty's report-all-keys mode is on (not in Terminal.app or tmux),
  and kitty, Ghostty, WezTerm and xterm keep shift+click for their own
  text selection, so the pill never sees it. The kitty-only footer switch
  is #146, after 0.2.
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
| H1 | Arrows `y` on a main row of Params, Headers or Vars fills the row register (`p` still pastes it) **and** copies the row's value to the clipboard, as the row's copy pill does (same toast). The sidebar's request `y` stays invisible. | mine (not vetoed when raised in chat) |
| H2 | Arrows `Y` on a Headers-tab row, main or auto, copies the whole header, `Name: value`. The register is untouched. On Params and Vars rows `Y` does nothing. | mine (approved): `Y` is the shift of the copy letter |
| H3 | A whole-header copy uses the real name and value, even when the row shows them masked (as the value copy does, spec 2026-10-18 R7). The toast names the header as the screen shows it: `Copied Content-Type header`. | mine |
| H4 | The response Headers tab copies with `y` (the value) and `Y` (the whole header) in arrows, instead of `c`. `c` is unbound there. Sent and received rows both copy. | mine (approved) |
| H5 | One footer chip, never two: on a Headers-tab row (request main or auto) and on a response header row, the chip is `copy`, with keycap `y/Y` in arrows. On a Params or Vars row it's `copy` with `y`. The response Headers tab gains this chip (it has none today). | you ("I don't want two chips in the header at the same time"); the `y/Y` keycap is mine (approved) |
| H6 | Mouse: a Headers-tab main row's right-click menu gains `Copy header`. An auto row, which has no menu today, gets one with `Copy header`. The response pane's right-click menu gains `Copy header` when the pointer is on a header row. The pills keep copying the value. | mine (approved) |
| H7 | Vim: `yy` on a main row is unchanged (the register only, as Vim's `yy` never writes the clipboard). | Vim |
| H8 | Vim: `"+yy` and `"+Y` on a Headers-tab row (main or auto) or on a response header row copy the row as its line, `Name: value`, as Vim's `"+yy` copies a line. Elsewhere, `"+` + key keeps the "Registers are not supported here" note. | Vim (the line); the scope is mine |
| H9 | Vim: `yy` on an auto row keeps copying its value (S14), though Vim's `yy` would not touch the clipboard. Without it, an auto row's value has no vim key at all. | mine, a deviation from Vim |
| H10 | The shift-switching footer chip in kitty-protocol terminals is #146, not this task. | mine (approved) |

## Design

- **Actions.** `CopyTarget` gains `TableHeader(usize)` (a Headers-tab main
  row), `ComputedHeaderLine(usize)` and `ResponseHeaderLine(usize)`, the
  whole-header twins of `TableRow`, `ComputedHeader` and `ResponseHeader`.
  `resolve_copy` builds `Name: value` for them. `Action::CopyResponseHeader`
  takes a `whole: bool`.
- **Verbs.** `Verb::Yank` stays the register yank. A main-row yank in
  arrows runs `YankTableRow` and then the copy. The router row decides which
  profile does what, so arrows `y` expands to both actions and vim `yy` to
  the yank alone. A new `Verb::YankHeader` (arrows `Y`, vim `"+yy`/`"+Y`)
  asks the focused surface for its whole-header target, as `Verb::Yank`
  asks for its row.
- **Router.** `best` today returns a complete `<any>` row before a longer
  exact row that extends the keys, so `"+` hits the `"<any>` note. That
  changes: a longer exact row wins, so `"+` waits for `yy`/`Y`. A new
  `"+<any>` row keeps the note for anything else.
- **Footer.** `Key` gains a two-action form whose keycap joins both keys
  with `/` (`y/Y`). In vim, the keycap shows `yy`, the same as today's
  auto-row chip.
- **Menus.** `table_row_context_menu` adds `Copy header` on the Headers
  tab. `context_menu_for` gains the auto row. `text_surface_menu` adds
  `Copy header` on a response header row.

## Tests

- H1: arrows `y` on a main header row fills the register and copies the
  value (test clipboard sink); vim `yy` fills the register and doesn't copy.
- H2/H3: `Y` on main and auto rows copies `Name: value` with the real
  value of a masked secret; `Y` on a Params row does nothing.
- H4: arrows `y`/`Y` on a response header row (received and sent); `c`
  does nothing.
- H5: footer chips per surface and profile; never two copy chips.
- H6: the three menus have `Copy header`, and it copies.
- H8: the router: `"+yy`, `"+Y` run the copy; `"+x` still gives the note.
  The key matrix `CASES` rows change accordingly.
- H9: the existing S14 tests stay green.

## Choices for you to veto (`mine`)

- H1 also applies to Params and Vars, so `y` means the same on every table.
- H2: `Y` on Params and Vars does nothing, rather than copying `name=value`.
- H3: the toast wording `Copied <Name> header`.
- H8: `"+yy` only on header rows for now. On body lines it keeps the
  note, rather than becoming Vim's line copy, which would be a separate task.
- H9: the auto row's vim `yy` stays a deviation from Vim.
