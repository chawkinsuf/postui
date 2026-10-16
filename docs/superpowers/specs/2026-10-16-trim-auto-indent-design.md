# Arrows body: an unused auto-indent is never kept (task #27)

Follow-up to #8 (`2026-10-15-editor-indent-design.md`, R14 and its round-1 note).

## 1. Problem

R14 made plain Enter in the arrows body copy the line's indent. Enter at the end of `    foo` (or with only blanks after the caret) leaves a new line that is nothing but `    `, with the caret after it. If the user then leaves that line without typing (an arrow off it, a click elsewhere, Esc, a pane switch) the blanks stay. They're trailing whitespace in the saved file, and they count as an edit.

R14 already covers repeated Enters: Enter on a blank line opens an empty line *above*, so `Enter, Enter` leaves nothing behind. The case left open is "Enter, then go somewhere else".

Vim removes an autoindent that nothing followed (probed for #8: `A<CR><CR>x` on `    a` leaves an empty middle line). VS Code does the same by default (`editor.trimAutoWhitespace`).

Code path today: `Editor::body_line_break` (`components/editor.rs`), else-branch. `LineBreak`, then the moved text's leading blanks go and `line[..lead]` is spliced in. `body_text()` is `self.body.lines.to_string()`, and `current_request()` / save / send / dirty / undo all read it.

## 2. Rules

| # | Rule | Source |
|---|---|---|
| R1 | Arrows body: when Enter (R14's after-the-text branch) leaves the new line holding nothing but the copied indent, that indent is **untouched**. Enter on a line with no indent copies nothing and marks nothing. | task: you |
| R2 | While untouched, **the request has that line empty**: save, send, the unsaved marker, undo steps, Format/Minify, `$EDITOR`, Copy as curl. Only the screen shows the blanks. Saving with the caret still on the line writes no trailing blanks, and the caret stays where it is. | you ("never saved", over removing real text on leave) |
| R3 | When the caret leaves the line, the screen drops the blanks too. "Leaves" means: the caret's row changes (any key, a click, a drag), or the caret is no longer in the arrows body (Esc, Up out of the body, a tab switch, focus moved to another pane, another screen such as Manage, or the vim profile owning the body). Nothing in the request changes, so this is never an undo step and never makes the request dirty. | task: you; the list: mine |
| R4 | Any change to the line's text makes the indent ordinary text, and it is never dropped after that: typing, Tab, Backspace, a paste, a variable insert. This holds even if a later key brings the line back to exactly the indent. | mine (Vim: typing ends the autoindent) |
| R5 | A modal over the editor (palette, variable picker, a confirm) is not leaving: the caret is still on the line underneath. | mine |
| R6 | Enter on an untouched line (R14's open-a-line-above branch) moves the line down whole, and it stays untouched. The new line above is empty. | mine (follows from R14) |
| R7 | Replacing the body wholesale (opening another request, undo/redo, Format, Clear, `$EDITOR`, the vim seam replacing the body) forgets the mark. The new text is exactly the request's text. | mine |
| R8 | A selection end, or the shift-select anchor, on the dropped line moves to column 0 when the blanks go. | mine |
| R9 | The vim profile is unchanged. Its engine has its own `autoindent`; any change there belongs to #26. | scope: mine |

## 3. Design

Everything stays in `Editor` except one call in `App::capture_undo`.

**State.** `Editor` gains a private `auto_indent: Option<AutoIndent>`, where `AutoIndent { row: usize, indent: Vec<char> }`.
- `body_line_break` sets it in the after-the-text branch when `lead > 0` and the new row equals `line[..lead]`.
- `body_line_break`'s above branch moves it to `row + 1` when Enter is pressed on the marked row (R6).
- `set_body_text` clears it (R7). That is the one place every wholesale replacement goes through.

**The request's view.** `body_text()` changes from "the buffer's text" to "the body as the request holds it". It is the buffer's text with the untouched row emptied, and it equals `lines.to_string()` when there is no live mark.

A mark is live while `auto_indent` is set **and** the row still equals `indent`. The second check means an edit is never hidden by a stale mark, even before the settle below has run.

Every request reader (`current_request`, `is_scratch_dirty`, send, Format, `$EDITOR`, BodyClear's emptiness check) goes through `body_text()`, so they all get R2 with no change of their own. The draw's highlight cache is the one reader that needs the screen's text, so it switches to `self.body.lines.to_string()`.

**Settling.** `pub fn settle_auto_indent(&mut self, caret_in_body: bool)`:
- no mark: nothing;
- the line no longer equals `indent`: forget the mark (R4);
- `!caret_in_body`, or the caret's row is not the mark's row: empty the row in the buffer, clamp the cursor, the selection ends and `body_sel_anchor` on that row to column 0 (R8), and forget the mark (R3);
- otherwise: keep the mark.

`App::capture_undo` calls it first, before the diff, beside the URL's `end_edit` (which is the same "the caret left" check). Its `caret_in_body` is: screen is Main, focus is the editor, the editor's sub-focus is Content on the Body tab, and `!vim_owns_body()`. Modals don't enter the test (R5).

`capture_undo` runs after every event in the main loop, so the mark never outlives the event that leaves the line or edits it. That also gives R4's "even if a later key brings it back". Since `body_text()` already left the line out, the settle changes nothing that `capture_undo` diffs.

**Contract change.** `body_text()`'s meaning, as above. No enum, seam or History change.

### Amendment 2026-10-16, review round 1 (mine)

- **The mark also remembers the buffer's line count** (`AutoIndent::lines`). It holds only while the count is the one Enter left; R6 counts its own new line.
  - Before this, an edit that moved rows (Home, then Backspace joining the untouched line up) left the mark naming a row the user's own `    ` line had slid into. The settle then emptied that line, which lost data.
  - Now any edit that moves rows forgets the mark, which is R4 again: the blanks are ordinary text from then on.
- **Arrows to vim drops the blanks inside the seam** (`sync_session`, when `switched_in`), before the engine takes the body. `capture_undo` runs too late for this, because the engine has already opened Insert on the blanks by then.
- **`settle_auto_indent` answers whether it changed the screen**, and `capture_undo` returns that as a redraw. The request didn't change, so nothing else asks for one.

### Amendment 2026-10-16, review round 3 (mine)

Rounds 1–3 kept finding the same design point. The buffer and the request disagree on one row, so any operation that reads the buffer as content has to pick a view: copy (rounds 1–3) and Tab over a select-all (round 3). A selection is how those operations reach the untouched row while the caret stays on it.

Instead of patching each reader, **R3 now counts a selection as leaving.** The untouched indent lives only while the caret rests on its line with nothing selected. So a select-all, or shift+arrow inside the blanks, drops them first, and copy, cut, Tab and delete only ever see the request's text.

This removes the copy cost accepted at the design walk-through ("a select-all copy while the caret is on the line includes the blanks"). A selection left wholly inside the emptied row is cleared, along with a double-click span that named the blanks. R8's clamp still applies when a shift motion carries the selection off the row.

### Amendment 2026-10-16, review round 4 (mine)

- **The indent rests only with the caret right after it** (row and column), not anywhere on its row.
  - A click, Left or Home into the blanks drops them, and the caret lands at column 0 of the empty line the request holds.
  - Before this, clicking at column 2 and typing `x` left `  x  `, with trailing blanks R4 then made permanent.
- **A selection wholly inside the emptied row is dropped, but its anchor stays** (clamped to column 0), as a collapsed selection's does elsewhere in the body. So does a double-click span's (clamped).
  - Round 3 cleared both, which killed a mouse drag that passed through the blanks: `body_drag_to` found no anchor.

## 4. Tests

| Rule | Test |
|---|---|
| R1 | `editor.rs`: Enter at the end of `    a` → the buffer's row 1 is `    `, `body_text()` is `"    a\n"`. Enter at the end of `a` (no indent) → no mark. Enter mid-text (`    a\|b`) → row 1 is `    b` and `body_text()` includes it. Enter with only blanks after the caret (`    a\|  `) → marked. A tab indent is marked the same way. |
| R2 | app test: open a saved `    a`, Enter at the end, save → the file's body is `"    a\n"` and the caret is still at row 1 col 4 with the blanks on screen. After that, the request is not dirty. |
| R3 | `editor.rs`: settle with the caret moved off the row → the row is empty and the mark is gone. Settle with `caret_in_body = false` → the same. App tests: Enter then Up; Enter then Esc; Enter then a click on another pane. Each leaves the row empty, records no extra undo step, and one undo restores `    a` exactly. |
| R4 | `editor.rs`: Enter, type `x`, settle → `body_text()` has `    x`. Enter, type `x`, Backspace, settle, move away, settle → the row keeps `    `. Round 1: on `    a` / `    `, Enter, Home, Backspace → the user's `    ` line stays in the body and on screen. |
| R3 (round 3) | `editor.rs`: Enter, select-all → the blanks drop, the copy is `    a\n`, and Tab leaves the empty line alone. shift+Left inside the blanks → dropped, caret at column 0, nothing selected, anchor at column 0. |
| R3 (round 4) | `editor.rs`: a click inside the blanks drops them and `x` types at column 0. A press at the indent's end keeps them; a drag into them drops them, and the drag goes on selecting up into row 0. |
| R3 (round 1) | app: switching to vim drops the blanks before `vim_catch_up` returns, and `x` then types at column 0. A drop makes `capture_undo` answer true (repaint) with no new undo step. |
| R5 | app test: Enter, open the palette, close it → the row still has the blanks and is still marked. |
| R6 | `editor.rs`: Enter, Enter → row 1 empty, row 2 `    ` and marked, `body_text()` `"    a\n\n"`. |
| R7 | `editor.rs`: Enter, then `set_body_text` → no mark, `body_text()` equals the buffer. |
| R8 | `editor.rs`: Enter at the end of `    a` with a line below it, shift+Left twice (anchor at row 1 col 4), then shift+Down (the caret leaves the row with the selection live) → settle; the anchor and the selection's row-1 end sit at column 0, and the selected text reads from there without a panic. |

## 5. `mine` choices, for veto

- R3's list of what counts as leaving, especially that another pane, another screen, or the vim profile owning the body counts.
- R4: any edit makes the indent permanent, even one undone by hand on the same line.
- R5: a modal is not leaving.
- R6: Enter carries the mark down with the line.
- R7: every wholesale replacement forgets the mark.
- R8: selection ends on the dropped row clamp to column 0.
- R9: the vim profile is out of scope (#26).
- `body_text()` changes meaning rather than adding a second accessor. Every caller today wants the request's view, except the highlight cache.

## 6. Plan (one sequence)

1. Editor unit tests for R1, R3, R4, R6, R7, R8 (failing).
2. `AutoIndent` state, `body_line_break` marking, `set_body_text` clearing, `body_text()` view, highlight cache on raw lines, `settle_auto_indent`.
3. App tests for R2, R3 (Up, Esc, pane click, undo), R5 (failing), then the `capture_undo` call.
4. Amend `2026-10-15-editor-indent-design.md` R14 with a dated pointer to this spec.
5. Full suite, clippy, fmt, a tmux check, commit.
