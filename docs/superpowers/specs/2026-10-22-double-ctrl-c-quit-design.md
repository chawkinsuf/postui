# No quit while a dialog is open; ^C twice to quit (task #85)

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
be quit with control-c while a dialog is open. The dialog must be
resolved and then the application can be quit"; "too easy to quit and
lose unsaved edits while a dialog is open"; "the dialogs needing to be
resolved before we potentially have to alert the user that there are
unsaved edits before quitting"; "maybe we could have a double control-c
as a final escape hatch … a toast or footer notification that says,
press control-c again".

Esc already closes every dialog but three, and those three each have
their own way out, so nobody gets stuck:

| Dialog | Esc | Way out |
|---|---|---|
| Bad `config.toml` at startup (`ConfigStartup`) | refused | its Quit / Continue / Edit / Reset |
| `$EDITOR` config edit didn't apply (`ConfigEditInvalid`) | refused (holds a temp file) | Keep editing / Discard |
| First-launch keyboard question (`KeymapChoice`) | refused (no default) | pick a profile, or its Quit (task #86, 2026-10-22) |

## 2. Rules

| # | Rule | Source |
|---|---|---|
| R1 | With any dialog open, every quit is refused: `^C`, the quit chip, the palette's Quit row and every vim quit verb (`:q`, `:qa`, `:q!`, `:qa!`, `:wq`, `:x`, `:wq!`), before any of them saves; none opens the unsaved-changes question. A refusal toasts "Close the dialog first", once however often it repeats, and a `^C` does not arm. The three dialogs Esc cannot close are no exception: they are left by their own answers. A dialog's own answer that quits (the question's Discard, the startup dialog's Quit) runs after its dialog closed, so it is not refused. | refusal: `you` (task text: "The dialog must be resolved and then the application can be quit"); no exception for the three: `you` ("no ^c ^c in dialogs"); the rework: `mine (approved)` ("lets go with your plan"); the forcing and saving vim verbs too: `mine (approved)` (rework review round 1); the toast wording: `mine` (no "(esc)": three dialogs refuse Esc) |
| R2 | With no dialog open, `^C` quits only on a second press. The first press changes nothing (no input closes, a live drag stays live) and toasts "Press ^C again to quit". | double press: `you` (task text); "the first press changes nothing": `mine (approved)` |
| R3 | The second press is the same combo, pressed next, while that toast is showing (a short toast lives 3 s). Any other key or mouse press between the two (a copy, a `q`, a click, another quit combo) takes the arm and its toast down, so the next press is a first press again. The toast is the arm: it never shows when a press would not quit. Input that is not a key or a mouse press (a paste, the wheel, focus, an async result) leaves the arm; if it opened a dialog, the arm toast stays up until the second press, which is refused with R1's toast. | `mine (approved)` (review rounds 1–3) |
| R4 | The quit the second press runs meets the unsaved-changes question when the request holds edits. The question is a dialog, so a quit on it is refused (R1): a held or mashed `^C` stops there, and leaving without saving takes `d`. | `you` ("lets not let ^C again discard changes") |
| R5 | Every `^C` that does not quit drops a half-typed vim key (`d`, `g`, a count, the body's `/` prompt; the response's search box is an open input and stays), as Vim's `^C` cancels it, so `d` `^C` `d` is not `dd`. | `Vim` (review round 3) |
| R6 | With a live text selection `^C` copies, as now, and neither arms nor is refused. | `you` (vim profile spec, 2026-10-03) |
| R7 | The arm toast spells the combo pressed (`^C`, `^Q`, …). | `mine` |
| R8 | One-step quits stay one step with no dialog open: arrows' `q`, `:q`/`:qa`, the palette's Quit row, the footer quit chip; `:q!`, `:qa!`, `ZQ`, `ZZ`, `:wq` and `:x` are unchanged there (they force or save first). | `mine (approved)` |
| R9 | The footer hides the quit chip while any dialog is open, as it hides a key that won't work. | `mine (approved)` |
| R10 | A quit cancels a live row drag before it opens the unsaved-changes question, or the drag would swallow the question's keys and land on release under it. | `mine` (review round 8) |
| R11 | Esc: unchanged. | verified, no change |

Spec rows this replaces (each gets a dated amendment pointing here):

- stage 2 spec §"Global quit escape hatch": "Ctrl+C always quits, modal
  open or not";
- vim profile spec, M4b amendment: "`:q` runs `GatedQuit`, which meets the
  dirty gate even with a dialog under the line (ctrl+c keeps its modal
  exemption …)" and "the palette's own Quit row runs `GatedQuit` in both
  palettes, so picking it on the line over a dialog meets the dirty gate";
  both are now refused under a dialog. Also §3 "`d` followed by ctrl+c
  still quits" and §4.2 "ctrl+c still quits" (now: twice, and not from a
  dialog);
- commit-cell-first spec: "Quit checks `modals.is_empty()` first, so
  `ctrl+c` over a modal still exits at once";
- request reordering spec: "the modified quit combo … still quits" during
  a live drag (now: on its second press, cancelling the drag);
- the vim profile spec's "a dialog over Manage quits, as one over the main
  screen does" for `:q` (now refused, `:q!` too).

## 3. Design

- **`App::quit_refused`** (any dialog open) is the one rule, applied
  where a quit request comes in, so a dialog's own answer that quits is
  never refused (rework review round 2: refusing inside `apply` could not
  tell the startup dialog's Quit, over the "not a project" question, from
  a `^C`, and refused it):
  - `route_key` step 1: a `^C` press with a dialog open, first or second,
    is refused (`refuse_quit`, which toasts with `Toasts::push_once`);
  - `apply_modal_result`: a command run from the palette or the `:` line
    while another dialog is still open, one whose actions quit
    (`Action::quits_under_a_dialog`), is refused whole, so its leading `CloseField`
    never closes the dialog's field;
  - the footer hides the quit chip, and a click on it resolved against
    an older frame is refused too.
  Arrows' plain `q` never reaches `Quit` under a dialog (dialogs take
  every key), and `ZZ`/`ZQ` bind only on the navigation surfaces.
- **`App::apply`**: the gate arm (`Quit | GatedQuit if leaving_unsaved`)
  loses `modals.is_empty()`; it cancels a live row drag, then opens the
  question (`dirty_gate`).
- **`App::disarm_quit`**, called first by every key (`handle_key`) and
  every mouse press (`handle_mouse`): takes `App::quit_armed` (the arm
  toast's id and the combo that armed it), dismisses a live toast, and
  answers the combo if the toast was still live.
- **`App::route_key` step 1** (the modified quit combo): after the
  selection copy, if this key is the combo `disarm_quit` answered,
  `update(Action::Quit)`. Otherwise drop the vim pending keys
  (`vim_drop_pending`), then: with a dialog open, `update(Action::Quit)`
  (refused, R1); else push the arm toast and arm with its id and this
  combo. `Toasts` gains `push_id`, `push_once`, `is_live` and `dismiss`.
- **Footer**: the quit chip is drawn only with no dialog open.
- **Removed**: the modal exemption in the gate arm. The footer quit
  chip's exception from the dialog click rule (`is_quit_chip` in
  `in_dialog`) goes too, since the chip is not drawn under a dialog.
- **Unchanged**: what `ForceQuit` and `WriteQuit` do once they run; the
  cmd+c rule; stdin EOF in `main.rs` (still quits at once); the startup
  dialog's Quit answer.

Contract change: a quit request is refused with any dialog open; `Quit`
no longer means "skip the gate under a dialog", and `GatedQuit` no longer
"meets the gate over a dialog". `Action::quits_under_a_dialog` names the
quit actions.

## 4. Tests

- R1: `^C ^C` over a dialog (the palette, a prompt with typing) neither
  quits nor opens the question; the dialog and its typing stay; the
  refusal toast shows once for ten presses; Esc, then `^C ^C`, quits. The
  three Esc-proof dialogs refuse it too. Every quit action, and every vim
  quit verb on the line over a dialog (`:q`, `:q!`, `:wq`, `:x`, `:qa!`),
  is refused with nothing written; the palette's Quit row too. The
  startup dialog's own Quit answer still quits.
- R2: one `^C` on the plain screen does not quit and toasts; a second
  quits.
- R3: a second press after the toast expired only re-arms; a key, a click
  or another quit combo between the presses disarms.
- R4: ten `^C` presses (a held key) with unsaved edits stop at one
  question and write nothing; `d` then quits.
- R5: `d` `^C` `d` deletes nothing; `g` `^C` `g` moves nowhere.
- R6: `^C` with a selection copies; the next `^C` only arms.
- R9: no quit chip is drawn under a dialog.
- R10: a quit during a live drag cancels it, and the question answers.

## 5. History

The first design (approved 2026-10-22) let a quit run with a dialog open,
opening the unsaved-changes question over it ("R4: stack over dialogs",
`mine (approved)`), and made a quit on the question itself "quit anyway".
Nine review rounds found edge cases that all grew from that stacking:
questions piling on follow-up dialogs, a held `^C` running through the
question (the user then ruled "lets not let ^C again discard changes"), a
marker field on `Modal::Confirm` to recognise the question, a dead quit
chip on it, and the question opening under a live drag. The step back
after round 9 (`mine (approved)`, "lets go with your plan. no ^c ^c in
dialogs") returned to the task's own words: the dialog must be resolved
before the app can quit. The stacking, the marker and the "quit anyway"
rule are gone. Kept from those rounds: the toast as the arm, back-to-back
presses, dropping vim pending keys, cancelling a drag.
