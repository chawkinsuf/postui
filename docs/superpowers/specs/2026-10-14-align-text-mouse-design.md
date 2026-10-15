# The text mouse, aligned between arrows and vim: design

Task #23, branch `align-text-mouse`. Status: **approved** (`you`, 2026-10-14). §5's
first call is "try it, but don't make it too hard to go back"; the second is accepted.
It follows task #15 (`2026-10-14-vim-mouse-selection-design.md`, merged `731ec3f`).

## 1. The ask

You, 2026-10-14: "get these aligned now. Unless something is pretty vim specific I'd
rather them behave the same." This covers the mouse on the text of a field (the URL,
cells, dialog boxes, form fields, Settings, the body). The response pane already
behaves the same in both profiles and stays as it is.

## 2. The rules

| # | Gesture | Arrows today | Both, after | Source |
|---|---|---|---|---|
| A1 | Double click | Selects a run: a whole run of punctuation is one word (`://`) | Vim's mouse word: blanks, word chars, the operator chars `-+*/%<>&|^!=` together (`->`, `!=`), and each other punctuation char alone | you (align) |
| A2 | Double click on a bracket, or on blanks before one | That char's run | Through to the matching bracket (`{` selects `{…}`, and the body can cross lines). In vim's Insert there is no match, as in Vim. **Amended after review round 1:** Vim also starts a block from any punctuation with a bracket later on its line (probed: `:` in `https://x.com/{{id}}` selects to the last `}`). That reach stays vim's, and arrows starts a block only from a bracket or blanks before one (`you`, 2026-10-14). | you (Q "Brackets") |
| A3 | Double click past the end of the text | Places the caret, selects nothing | The last word: the press counts as on the last char | you (Q "Past end") |
| A4 | Drag after a double click | Grows by arrows' runs | Grows by A1's words, with Vim's rule for which end moves | you (align) |
| A5 | Triple click | Acts as a single click | Selects the line: in a one-line field the whole value, in the body the line under the pointer | you (align) |
| A6 | Drag after a triple click | — | Grows by whole lines | you (align) |
| A7 | Click count on text | Same control, 0.4 s, a pair | Same control and cell, 0.5 s. The count goes to four, as Vim's does; a fourth acts as a single click (Vim's would be blockwise, which postui lacks), and a fifth starts over. Corrected after review round 1: this row said "up to three" | you (align), `mine` (the window) |
| K1 | Drag width | Stops before the char under the pointer (bar caret) | **Unchanged in each**: vim takes that char, arrows stops before it | you (Q "Drag width") |
| K2 | Right click on the edited field | Copy/Paste menu | **Unchanged in each**: arrows menu, vim extends | you (Q "Right click") |
| K3 | Typing over a selection | Replaces it | **Unchanged**: vim's keys are commands | mode-inherent |

Lists and buttons keep arrows' pair count in both profiles (Q1 (b) of task #15).

## 3. Design

**One neutral home for the shapes.** The word classes and the bracket matcher live in
the vim engine today (`vim/engine/class.rs`, `class_table.rs`, `motion::find_match`).
Arrows may not depend on `vim/`, since the vim profile must stay removable whole. They
move to a new profile-neutral module, `crate::text`:
- `text/class.rs` and `text/class_table.rs` hold Vim's `utf_class` table and its helpers,
  moved unchanged; `generate.py` writes the table to the new path.
- `text/brackets.rs` holds `find_match` and `MatchFrom`, taking the text as `(line_count,
  line(row))`, the only two things it reads.
- `text/mouse.rs` holds `mouse_class`, `word_start`, `word_end` and `block_end` (from
  `vim/engine/mouse.rs`), plus `Pos`, which the engine re-exports from `buf.rs`.

The engine imports all of these and behaves exactly as now; the probe-based engine
tests and the Vim conformance tests pin that.

**Arrows uses the same shapes.**
- `LineInput::select_word_at` uses `text::mouse` for A1–A3. Its word anchor drives A4
  by the same rule, and a new `select_line` handles A5 and A6, where a drag keeps the
  whole value selected.
- The body editor's `body_word_cells` and `body_drag_to` do the same over edtui's lines,
  plus a line anchor for A5 and A6.
- The GUI selection stays half-open (bar caret, K1). The shared functions return Vim's
  inclusive ends, converted once.

**One text count.** `Clicks { gui, vim }` becomes `Clicks { list, text }`. `text` is
A7's count, which every field's press reads in both profiles; `list` is the pair count
for rows and buttons.

**The response pane takes the same words** (amended 2026-10-14, `you`: "same rule
everywhere"). §1 left the response alone, but it shares `word_span_at`, so its double
click and word drag move to A1, A3 and A4 too, counted as text (A7). Its bracket block
and triple click stay out for now.

**Removed:** `word_nav::word_span_at` (its last mouse users go). The keyboard word
motions in `word_nav` (ctrl+←/→) are not mouse and stay arrows'.

## 4. Tests

- Arrows app tests for each of A1–A7 in the URL line, plus a cell, a dialog box and the
  body.
- The existing arrows mouse tests get A1's words where they pinned a punctuation run
  (and A3 where one pinned "past the end selects nothing"). Those are behaviour changes
  you asked for, and each changed expectation is listed in the commit.
- The engine and conformance suites pass unchanged after the move.

## 5. Decisions on my calls

- **A7's window, on trial** (`you`: "we can try 1 but don't make it too hard to go
  back"). Text uses 0.5 s and the same cell, as vim already does, so in arrows a slight
  pointer move between the clicks now breaks a double click on text. The rule is two
  named constants in `count_clicks` (`TEXT_WINDOW`, `TEXT_SAME_CELL`). To go back for
  arrows, its text presses take arrows' window and no cell check: a profile argument to
  `count_clicks` and nothing else.
- **A5 in the body, accepted** (`you`). A triple click selects the line's text without
  its line break (the GUI convention); vim's V-LINE stays Vim's.

## 6. Plan

1. Move the class tables, the bracket matcher and the mouse word helpers to `crate::text`,
   with no behaviour change (full suite green), and repoint `generate.py`.
2. Write arrows' tests for A1–A7 (failing).
3. Change `LineInput` and the body editor to the shared shapes and add triple-click
   select, then `Clicks { list, text }`.
4. Update the old arrows tests' expectations, amend the stage-4 mouse spec rows, and
   check in the TUI in both profiles.
5. Review rounds: mediums in blocks of three.
