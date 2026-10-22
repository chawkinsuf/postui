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
| R1 | `^C` quits only on a second press. The first press changes nothing (no dialog closes, no input closes, a live drag stays live) and toasts "Press ^C again to quit". | double press: `you` (task text); "the first press never changes anything": `mine (approved)` |
| R2 | The second press counts while that toast is showing (a short toast lives 3 s). Once it is gone, a press is a first press again. The quit the second press runs takes the toast down, so the toast never shows when a press would not quit. Nothing else disarms it. | `mine` (review round 1: the toast is the arm, so the two can't disagree) |
| R3 | The second press runs `Quit`, and `Quit` meets the unsaved-changes check whether or not a dialog is open. The modal exemption goes. | `you` (the task's ask) |
| R4 | The check opens over whatever dialog is open, as `:q`'s does today; its Cancel returns to that dialog with its typing intact. | `mine` (in chat I said the quit would cancel open dialogs first; this keeps them instead, see §5) |
| R5 | `Quit` while the quit's own check is the top dialog leaves without saving: `^C ^C` on the check is "quit anyway". | `mine (approved)` |
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

- **`App::route_key` step 1** (the modified quit combo): after the
  selection copy, if the toast the last first press showed is still live,
  dismiss it and `update(Action::Quit)`; otherwise push the toast and keep
  its id (`App::quit_armed`). `Toasts` gains `push_id`, `is_live` and
  `dismiss`. (Review round 1 replaced a stored timestamp and a copy of
  the toast lifetime, which could disagree with the toast on screen.)
- **`App::apply`**: the `Quit` gate arm loses `self.modals.is_empty()`.
  A new arm before it: `Action::Quit` when the top dialog is the quit's
  check → `ForceQuit`. "Is the quit's check" reads the dialog itself: a
  `Confirm` with a `[ForceQuit]` choice, which only `dirty_gate("quit",
  ForceQuit)` builds (both its named and never-saved forms).
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
  the toast down.
- R3/R4: `^C ^C` over a dialog with unsaved edits opens the check over
  it; Esc on the check returns to the dialog, typing intact; nothing quit.
- R5: `^C ^C` on the check quits without writing the request.
- R6: `^C` with a selection copies; the next `^C` only arms.
- R8: a quit-chip click under a dialog with unsaved edits opens the check.
- R10: `^C ^C` over each of the three blocking dialogs quits.
- Existing tests that quit with one `^C` press twice, and tests that pin
  the modal exemption ("Ctrl+C quits with modal open") get the new rule.

## 5. `mine` choices to veto

Approved with the spec on 2026-10-22 (picked from my options, so
`mine (approved)`):

1. R2's 3 s window, and nothing but time disarms it.
2. R4: the check stacks over open dialogs instead of cancelling them, so
   cancelling the quit gives the dialog back with its typing.
3. R5: on the check, `^C ^C` (and one click of the arrows quit chip)
   means quit without saving.
4. R7's toast wording, and R9's unchanged chip keycap.
5. R1 during a live row drag: the first `^C` doesn't cancel the drag.
