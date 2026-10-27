# Body keyboard selections take line breaks (#128)

## Problem

In the arrows profile, a keyboard selection in the body is a run of
character cells, so it can never hold a line break.
`set_body_selection_boundaries` turns the caret's exclusive boundary into
an inclusive end cell and clamps both ends onto characters.

1. **shift+Right across a break selects the character before it.** Body
   `ab` / `cd` / `ef`, caret after `ab`, shift+Right: the selection is `b`,
   and Delete gives `a` / `cd` / `ef`. A GUI editor selects the break, and
   Delete joins the lines (`abcd` / `ef`).
2. **shift+Down from a line start selects the line without its break.**
   Caret at (0,0), shift+Down, Delete: an empty row is left. A GUI editor
   removes the line. `shift_down_selects_the_first_line` (6e47b23) pins
   today's rule, so changing it is your call.

### What edtui does (probed against edtui 0.11.6, `DeleteSelection`)

An inclusive end cell at `col == len` (one past a row's last character)
already means "and this row's break":

| selection | result |
|---|---|
| `ab\ncd\nef`, (0,1)..=(0,2) | `acd\nef` (b and the break) |
| (0,1)..=(1,2) | `aef` |
| (0,0)..=(0,1), the whole row, no break | `cd\nef` (it takes the break anyway: #124's case) |
| (0,2)..=(0,2), the break alone | `acd\nef` (**wrong**: the start is clamped back onto `b`) |
| (0,2)..=(1,0), break then `c` | `ad\nef` (**wrong**: same clamp) |

So the end works. A selection that starts on a break needs correcting
around edtui, and whole-row shapes already do (#124).

## Rules

| # | Rule | Source |
|---|---|---|
| R1 | A keyboard selection covers everything strictly between the anchor and the caret, line breaks included. One shift+Right at a line's end selects its break. | mine (the GUI rule the task names) |
| R2 | shift+Down from a line start selects that line and its break, and Delete removes the line. `shift_down_selects_the_first_line` changes to expect `hello\n`. | you, 2026-10-26: "yes, select the line and its line break" |
| R3 | Copy (ctrl+c) of a selection holding a break copies the `\n`. | mine |
| R4 | Delete, Backspace, typing over and pasting over remove exactly the selected text, breaks included: the one rule `deleting_any_selection_removes_exactly_its_text` checks for every shape, now with breaks in the shapes. | mine |
| R5 | A selected break paints as one selected cell after its line's last character (empty line: at column 0). That includes the breaks a mouse drag across rows already takes; a line that fills the width has no cell to paint. | mine |
| R6 | Mouse selections are unchanged: a drag still selects characters, as today. | mine (out of this task's scope) |
| R7 | The vim profile is unchanged (its Visual is the engine's). | mine |

## Design

- One representation: each row but the last has a break cell at
  `(row, max(len, 1))` (`body_break_cell`). On a non-empty row that is
  `col == len`, which edtui already reads as "and the break"; an empty row's
  column 0 stays its empty char cell, so a mouse selection ending there
  means what it always did. A selection takes a break exactly when the
  break cell lies inside it (`body_selects_break`), which is also why a
  selection spanning rows takes the breaks between them.
- `set_body_selection_boundaries` maps a lower boundary at a line's end to
  that line's break cell, and an upper boundary at a line start to the
  previous line's break cell; `set_body_selection_span` stores both ends as
  given (edtui's Visual would clamp the start back onto a char).
- `body_selected_text`, `delete_body_selection`, the Tab indent and the
  paint read that one rule. The delete stays edtui's `DeleteSelection`
  (#124, your ruling) and corrects around it: it counts the breaks the
  selection takes and, at the selection start, puts back a break edtui took
  too many or joins one it left (edtui's `LineBreak`, `JoinLineWithLineBelow`).
  A start on a break deletes from the next row, and the count joins the
  break.
- The mouse path keeps calling `set_body_selection` (character cells).
- Implemented as designed. *Amended in implementation:* the Tab indent
  keeps a break end on its row's break; the paint covers every break a
  selection takes (R5).

## Tests

- R1: shift+Right at a line's end selects `\n`; shift+Left from a line
  start selects the previous break; shift+End then shift+Right takes the
  break.
- R2: `shift_down_selects_the_first_line` expects `hello\n`; Delete leaves
  `world\n`.
- R3: the copy text of a selection ending on a break ends with `\n`.
- R4: `deleting_any_selection_removes_exactly_its_text` gains shapes that
  start and end on breaks (Delete, typing, paste).
- R5: a paint test for the selected break cell.
- R6: an existing mouse-drag test still passes unchanged.

## Choices for you to veto

R1 and R3–R7 above are mine; none was vetoed when the spec was presented
(2026-10-26). R2 is yours.
