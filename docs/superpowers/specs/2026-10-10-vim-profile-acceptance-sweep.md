# Vim profile (piece 4): acceptance sweep and backlog

The §8.4 acceptance sweep of the vim profile design
(`2026-09-27-vim-profile-design.md`), run once at the end of M5, on main at
5635d42 (M5b merged). This file holds the run's result and, as §8.4 asks, the
backlog of what was found outside the scripted steps. Nothing here is fixed on
the piece-4 work.

## The run

- **Harness.** tmux held in the background (`escape-time 0`), keys sent with
  `send-keys`, `/` as `send-keys -H 2f`; each step's keys, sleeps and captures
  in one script so toasts are still up. Sidebar and response cursors were
  read from the captured styles. 160×45.
- **Fixture.** A fresh XDG config (`animations = false`, no keymap: the
  first-launch question is raised) and a project `Sweep` with one space and
  requests `alpha` (GET `/users.json`, five headers), `beta`, `gamma`,
  `delta`, `epsilon`, `zeta`. A local `http.server` serves `users.json` (seven
  users, nested objects and arrays, eight matches of `id`).
- **Result: pass.** Every scripted step matched its observation (A–F below).
  The final diff of `requests/` against the fixture is exactly the expected
  one: `zeta.toml` renamed to `zulu.toml` (A), `omega.toml` created (B),
  `alpha.toml` gains `X-A = "1"` and `X-A-copy = "Y"` (`:w`, D) and
  `[params] p1 = ""` (`ZZ`, F). The undone copies sit in `.local/trash/`. No
  data loss, no wrong edit.

| Step | Observation |
|---|---|
| A | "Keyboard" shown; Esc and Enter do nothing; `v` gives NORMAL. Settings → Arrows: `j` does nothing on the sidebar, ↓ moves. `r`, a new name, one Enter renames (disk too). Back to Vim. |
| B | `3j` `dd` (one row, no dialog, toast) `u` (back, reselected); `2dd` `u` (both back, one step); `yy` `p` (`epsilon copy` under the cursor) `P` (`epsilon copy 2` over it) `u` `u`; `a` + name + Enter (created); `r` Esc Esc (unchanged). |
| C | Headers: `a` `X-A` Tab `1` Esc Esc; `yy` `p` (`X-A-copy`); `dd`; `u`; `cw` `Y` Esc in the open cell; `j` (cell closes, cursor down one); `3dd` `u` (one step, first restored row selected). |
| D | `ctrl+w l j k h p` (footers show sidebar, editor, response, editor, sidebar, editor); `2gt` (Params); `:w` (saved); `:s` Enter ("Not supported: :s", nothing runs); `:man` Enter (Manage); `gt`; `:q` with unsaved edits (the dirty gate). |
| E | `:send`; `G`; `gg`; `ctrl+d` (view and cursor both down half a page); `zc` `zo` (cursor back on its line); `/id` Enter (from the cursor); `n` `N` (counter 2/8 → 3/8 → 4/8 → 3/8). |
| F | `qa` (note, "recording @a — not supported"), `x` (nothing), `q` (recording ends), `@a` (note), `j` (moves). URL open: `ma` (note), `j` (field closes, cursor into the table). `ZZ` with unsaved edits: exits, saved. Relaunch, edit the URL, `ZQ`: exits, disk unchanged. |

### Brief corrections for a later run

The brief was written before the later milestones and against another
fixture. Run as written, these steps do not do what they say:

- **A.** Settings is Manage's last tab (`:man`, then `gT`); Manage is left
  with Esc. `:q` quits the app on every surface (§5.9), Manage included.
- **B.** The names (`beta copy`) belong to another fixture: they are whatever
  row `3j` lands on.
- **C.** Start with `ctrl+w l` from the sidebar: `gt` on the sidebar changes
  the editor's tab but keeps the focus (the sidebar borrows the strip, §5.8).
  `i` opens the cell in Insert, so `cw` must follow an Esc (`i` Esc `cw` `Y`
  Esc), or `cw` is typed as text.
- **D.** Start from the sidebar (`ctrl+w h`): from the editor, `ctrl+w l` has
  no pane to go to. `:w` leaves nothing unsaved, so add an edit after it for
  `:q` to meet the dirty gate.
- **E.** `ctrl+Enter` may not reach the app through tmux; `:send` does.

## Backlog

Found during the run, outside the scripted observations. Each is open; none
blocks the milestone.

1. **The undo hint names `^Z` in the vim profile.** "Deleted epsilon — ^Z
   undoes", "Deleted 3 rows — ^Z undoes". ctrl+z does undo in both profiles
   (§4.4), so the toast is true, but vim's own key is `u`. `App::undo_hint`
   spells the key with `Keymap::combo_for("undo")`, not through
   `keyroute::spell`, which §4.6 makes the one source for spellings (the
   footer already reads `u`).
2. **`dd` on a table's last row leaves the cursor on the ghost row.** Headers
   `X-A`, `X-A-copy`, `+ Add header`: `dd` on `X-A-copy` selects `+ Add
   header`. Vim's `dd` on the last line lands on the new last line (`X-A`).
   Undo already never lands on the ghost row (§5.3). Not checked in arrows.
3. **Sidebar `h`/`l` against Vim's `'foldopen'`** (the §5.2 backlog item).
   Probed on Vim 9.1.0016 with a buffer that mirrors the sidebar (`alpha`,
   `api`, `ping`, `pong`, `beta`, one manual fold over `api`..`pong`), keys
   typed through the main loop (`feedkeys` `'nt'`), and in the app on an
   `api/` folder holding `ping` and `pong`:

   | Start | Keys | Vim 9.1 | App |
   |---|---|---|---|
   | on `pong` | `zc` `l` | fold opens, cursor on `pong` | fold opens, cursor on `api/` |
   | on `pong` | `zc` `h` | fold stays shut, cursor keeps `pong` (`zo` returns there) | fold stays shut, the hidden row is forgotten (`zo` lands on `api/`) |
   | on `api/` | `zc` `l` | fold opens, cursor on `api` | the same |
   | on `api/` | `zc` `h` | nothing | the same |

   Vim's `l` opens a closed fold because `'foldopen'` has `hor` and the cursor
   moves; its `h` at column 0 does not move, so nothing opens. The app's
   folder-row behaviour matches. The two differences are the hidden row:
   after `l` the cursor should come back to it, and `h` should keep it (spec
   §5.2 marked "`h` and `l` forgetting it" `mine`, unprobed). Outside this
   probe, the sidebar's `h` on an open folder collapses it (main's tree
   rule, kept by ruling); Vim's `h` does not close folds.
4. **Manage advertises no way back.** Its footer (both profiles) lists no key
   for leaving; Esc leaves. In vim the obvious `:q` quits the app instead.

## Round 8: the round-7 brief rerun on main

The old vim-mode branch's last muscle-memory brief (five scenarios, about
75 steps), refreshed for every ruling made since, was dry-run by me and then
run by five agents on main at 5ae3b34 (piece 4 plus the vim seam). Result:
A 13/14, B 20/20, C 14/15, D 13/14 (one step blocked: tmux does not forward
ctrl+1/ctrl+2), E 16/16. No text loss anywhere. Every mismatch is below,
checked by me.

Items 5–8 are resolved; see "Round 8 resolved" below.

5. **Reopening the jq bar puts the Insert caret on the last character.**
   With a filter applied, `alt+q` opens the bar in Insert with the caret on
   the filter's last character instead of after it: typing lands one
   character early (`.count` + `X` → `.countXe`). The engine also takes that
   position as where Insert began, so `ctrl+u`, even after `End`, deletes
   only that one character; a second `ctrl+u` clears the line. Found in my
   dry run, reproduced by agent A four times.
6. **`Esc` on a closed URL line moves focus into the tab content.** With the
   URL line selected but not open (footer `enter edit`), `Esc` moves the
   editor's focus down to the tab content. Vim's `Esc` in Normal never moves
   anything, and a reflexive extra `Esc` is common; afterwards `ctrl+w l`
   from the sidebar returns to the content, so a URL typed there runs as
   keys (it cost me and an agent a step each). Not in the spec.
7. **`S`, `C` and `cc` do nothing on a closed URL line.** `i`, `a`, `I`, `A`
   and `Enter` open it (key list: "`i` `a` `I` `A` `Enter` open it"); a vim
   user replacing the URL types `S` or `cc` and gets nothing. Spec gap:
   these would open the field with the text cleared (or, for `C`, cleared
   from the caret). Reproduced by agent C and by me.
8. **Smaller notes**, not vim deviations or not isolated:
   - A one-field prompt's button row says `enter save` under a "Confirm"
     button (both profiles; `ModalStack::footer_chips` labels every prompt's
     button row "save").
   - `Tab` does not move from a prompt's field to its buttons; `j`/`↓` does,
     as §5.10 rules (the brief's `Tab` was the old branch's), but nothing in
     the field's footer says so.
   - Undoing a field back to its saved text keeps the request's `●` until
     the field closes (the app history takes the step at the close; Vim
     clears "modified" at once).
   - `i` on a closed table cell may seat the caret at the end (agent B, not
     isolated; I could not confirm it).
   - Already filed or by ruling: the `^Z` toast (item 1), `dd` on the last
     table row landing on the ghost row (item 2), the sidebar cursor
     skipping folder rows after a delete (piece 2's "neighbour" is a
     request row), the sidebar showing the saved method until `:w`, a
     response not kept across a relaunch.

### Round 8 resolved (2026-10-11)

Items 1–4 stay open. The spec's §5.6 amendment records the rulings.

- **5, fixed.** The glue's `leave` stepped a query box's caret back one,
  as leaving Insert does, and the reopened bar kept it there. A query box
  now keeps its caret when its session ends. `ctrl+u` after the fix is
  Vim's: the typed text first, then the line before it.
- **6, fixed.** Vim's `Esc` on the method badge, the closed URL line and a
  closed table is a no-op row. Before, it fell through to arrows' `Esc`,
  which steps out of the badge or line (to no selection) and deselects a
  table row. A probe of every surface found these three. The body's blur
  (§5.6, earlier) and the Manage, Settings and Variables screens' `Esc`
  (leaving a level, item 4) are unchanged.
- **7, fixed.** `cc`, `S` and `C` on the closed URL line open it and run
  the engine's own command, so register, undo and count are Vim's.
- **8.** The button row's `enter` chip now names the aimed button,
  "confirm" or "cancel", in every form dialog (the user's decision,
  2026-10-11; it said "save" or "apply", and Enter on an aimed Cancel
  cancels). No change for `Tab` in a one-field prompt (the user's
  decision: `Enter` and `Esc` already do the buttons' work and are in the
  footer). The `●` after an undo back to the saved text did not
  reproduce: on the URL line and the body it clears at the next tick
  with the field still open (test kept). `i` on a closed cell does seat
  the caret after the last character: the cell's caret starts at the
  end, and `Enter` then `i` inserts before it. Left as it is (§5.6).
