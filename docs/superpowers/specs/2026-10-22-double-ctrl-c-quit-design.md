# ^C twice to quit, always through the unsaved-changes check (task #85)

## 1. Problem

The modified quit combo (`^C`, or any ctrl/alt combo bound to `quit`)
pre-empts everything (`App::route_key` step 1). With any dialog open it
also skips the unsaved-changes check: `apply`'s gate arm is
`Action::Quit if self.modals.is_empty() && self.leaving_unsaved()`, so
`Quit` under a dialog goes straight to `ForceQuit`. So:

- one `^C` over any dialog (a menu, the palette, a rename prompt) quits
  and drops the request's unsaved edits;
- `^C ^C` from the plain screen opens the check, then quits past it,
  because the check is itself a dialog;
- the footer's quit chip under a dialog (`Quit` in arrows) does the same
  on one click.

The user's words (task #85, and in chat): "not allow the application to
be quit with control-c while a dialog is open"; "too easy to quit and
lose unsaved edits while a dialog is open"; "the dialogs needing to be
resolved before we potentially have to alert the user that there are
unsaved edits before quitting"; "maybe we could have a double control-c
as a final escape hatch … a toast or footer notification that says,
press control-c again".

Esc already closes every dialog but three, and those three each have
their own way out, so nobody gets stuck today:

| Dialog | Esc | Way out |
|---|---|---|
| Bad `config.toml` at startup (`ConfigStartup`) | refused | its Quit / Continue / Edit / Reset |
| `$EDITOR` config edit didn't apply (`ConfigEditInvalid`) | refused (holds a temp file) | Keep editing / Discard |
| First-launch keyboard question (`KeymapChoice`) | refused (no default) | pick a profile |

## 2. Rules

| # | Rule | Source |
|---|---|---|
| R1 | `^C` quits only on a second press. The first press changes nothing (no dialog closes, no input closes, a live drag stays live) and toasts "Press ^C again to quit". The one exception is a half-typed vim key (`d`, `g`, a count, the body's `/` prompt; the response's search box is an open input and stays), which goes as Vim's ^C cancels it, so `d` ^C `d` is not `dd` (review round 3; `Vim`, per the follow-Vim rule). | double press: `you` (task text); "the first press never changes anything": `mine (approved)` |
| R2 | The second press is the same combo, pressed next, while that toast is showing (a short toast lives 3 s). Any other key or mouse press between the two (a copy, a `q`, a click, another quit combo) takes the arm and its toast down, so the next press is a first press again. The toast is the arm: it never shows when a press would not quit. | `mine (approved)` (review round 1: the toast is the arm; round 2: back to back, replacing "nothing but time disarms it", after three paths let an input between the presses count as the second, one of them a single ^C quitting past the check). Input that is not a key or a mouse press (a paste, the wheel, focus, an async result) leaves the arm: none of it can open the unsaved-changes gate, so the second press still meets it (`mine`, review round 3) |
| R3 | The second press runs `Quit`, and `Quit` meets the unsaved-changes check whether or not a dialog is open. The modal exemption goes. | `you` (the task's ask) |
| R4 | The check opens over whatever dialog is open, as `:q`'s does today; its Cancel returns to that dialog with its typing intact. Cost (kept, as `:q` has always had it): the quit closes open inputs first (task #75's rule), so a table cell under the extract or var-picker dialog is committed on the way to the check, and after a Cancel that dialog finds no open field; the cell's typing is kept and undoable. | `mine (approved)` (in chat I said the quit would cancel open dialogs first; this keeps them instead, see §5) |
| R5 | A quit never discards edits by itself. `Quit` or `GatedQuit` while the quit's own check is the top dialog does not quit and does not stack another check: it toasts "Unsaved changes: s saves, d discards, esc cancels". A `^C` there does not arm. Leaving without saving takes `d`. | `you` (review round 5, 2026-10-22: "lets not let ^C again discard changes"), replacing "`^C ^C` on the check is quit anyway" (`mine (approved)`), which let a held or mashed `^C` (terminal auto-repeat arrives as presses) run through the check unread. The toast wording is `mine`. |
| R6 | With a live text selection `^C` copies, as now, and does not arm. | `you` (vim profile spec, 2026-10-03) |
| R7 | The toast spells the combo pressed (`^C`, `^Q`, …). | `mine` |
| R8 | One-step quits stay one step: arrows' `q`, `:q`/`:qa`, the palette's Quit row, the footer quit chip. Under a dialog the chip's `Quit` now meets the check (R3). `:q!`, `:qa!` and `ZQ` still force. | follows R3; `mine` |
| R9 | The footer quit chip keeps its `^C quit` keycap. | `mine` |
| R10 | Esc: unchanged. Every dialog but the three in §1 closes with it, and those three have their own exits; `^C ^C` quits from all three (at startup there is nothing unsaved). | verified, no change |

Spec rows this replaces (each gets a dated amendment pointing here):

- stage 2 spec §"Global quit escape hatch": "Ctrl+C always quits, modal
  open or not";
- vim profile spec, M4b amendment: "ctrl+c keeps its modal exemption"; and
  §3 "`d` followed by ctrl+c still quits", §4.2 "ctrl+c still quits" (both
  now: arms);
- commit-cell-first spec: "Quit checks `modals.is_empty()` first, so
  `ctrl+c` over a modal still exits at once";
- request reordering spec: "the modified quit combo … still quits" during
  a live drag (now: arms; the drag stays live).

## 3. Design

- **`App::disarm_quit`**, called first by every key (`handle_key`) and
  every mouse press (`handle_mouse`): takes `App::quit_armed` (the toast's
  id and the combo that armed it), dismisses the toast, and answers the
  combo if the toast was still live.
- **`App::route_key` step 1** (the modified quit combo): after the
  selection copy, if this key is the combo `disarm_quit` answered,
  `update(Action::Quit)`; otherwise push the toast and arm with its id and
  this combo. `Toasts` gains `push_id`, `is_live` and `dismiss`. (Review
  round 1 replaced a stored timestamp and a copy of the toast lifetime;
  round 2 made the presses back to back.)
- **`App::apply`**: the `Quit` gate arm loses `self.modals.is_empty()`.
  A new arm before it: `Action::Quit` when the top dialog is the quit's
  check → `ForceQuit`. "Is the quit's check" reads the dialog itself: a
  `Confirm` with a `[ForceQuit]` choice, which only `dirty_gate("quit",
  ForceQuit)` builds (both its named and never-saved forms).
- **Amended after review block 1 (2026-10-22, `you`: "i do like having
  it more expicit"):** the quit's question is marked, not recognised by
  its answers. `Modal::Confirm` gains `quit_gate: bool`, true only on the
  question `App::quit_gate` builds (it takes the shared, unmarked
  `unsaved_question` and marks it; review round 5 dropped a `quit_gate`
  parameter every gate caller had to pass). Every other confirm is built with
  `Modal::confirm(title, body, choices)`, which sets it false, so it
  defaults to false in all but name (Rust's default field values are
  still unstable; `you` asked "couldnt it just default to false?"). The
  R5 arm matches `Confirm { quit_gate: true, .. }`. This replaces reading
  a bare `[ForceQuit]` answer, which a later dialog could match by
  accident; a test pins that such a dialog now gets the gate.
- **Amended after review round 5 (`you`):** the R5 arm no longer
  force-quits; it toasts the reminder and stays. Step 1 does not arm on
  the quit's question, so the press goes straight to that reminder.
- **Removed:** the modal exemption, and the comments that explain it
  (`apply`, `Action::GatedQuit`'s doc, `spell::quit_action`, footer and
  `ui.rs` notes on `^C` pre-empting).
- **Unchanged:** `GatedQuit`, `ForceQuit`, `WriteQuit`; the cmd+c rule;
  stdin EOF in `main.rs` (still quits at once).

Contract change: `Action::Quit` no longer means "skip the gate under a
dialog". Its callers (the combo, arrows' `q`, the chip, the startup
dialog's Quit) all want the gated meaning.

## 4. Tests

- R1: one `^C` on the plain screen does not quit and toasts; a second
  quits. Under a dialog, one `^C` leaves the dialog open and does not
  quit.
- R2: a second press after the toast expired only re-arms; the quit takes
  the toast down; a key, a click or another quit combo between the presses
  disarms.
- R5 in vim: the `:` line does not open on the gate, and a click on the
  quit chip there neither quits nor stacks a second gate.
- R3/R4: `^C ^C` over a dialog with unsaved edits opens the check over
  it; Esc on the check returns to the dialog, typing intact; nothing quit.
- R5: ten `^C` presses (a held key) stop at one check, toast the reminder,
  never arm, and write nothing; `d` then quits. A quit during a live drag
  ends the drag, so the check gets its keys.
- R6: `^C` with a selection copies; the next `^C` only arms.
- R8: a quit-chip click under a dialog with unsaved edits opens the check.
- R10: `^C ^C` over each of the three blocking dialogs quits.
- Existing tests that quit with one `^C` press twice, and tests that pin
  the modal exemption ("Ctrl+C quits with modal open") get the new rule.

## 5. `mine` choices to veto

Approved with the spec on 2026-10-22 (picked from my options, so
`mine (approved)`):

1. R2's 3 s window, and nothing but time disarms it (superseded in review
   round 2: back to back, also `mine (approved)`).
2. R4: the check stacks over open dialogs instead of cancelling them, so
   cancelling the quit gives the dialog back with its typing.
3. R5: on the check, `^C ^C` (and one click of the arrows quit chip)
   means quit without saving (superseded in review round 5 by the user:
   a quit never discards).
4. R7's toast wording, and R9's unchanged chip keycap.
5. R1 during a live row drag: the first `^C` doesn't cancel the drag.
