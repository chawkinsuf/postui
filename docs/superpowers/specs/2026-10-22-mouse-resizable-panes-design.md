# Mouse-resizable panes (#90)

## 1. Why

The request sidebar is too wide on the user's large screen. It is a fixed
28% of the window (`layout.rs`, `Percentage(28)`), and lowering the
percentage would make it too narrow on a small screen (the user: "its too
large on my large screen. but would be too small on a small screen if i
adjusted it"). The user tests small screens by resizing the window on the
same machine.

Once one divider can be dragged, the other has to be too (the user: "if we
make that draggable, we have to make both of them"). So this covers both
dividers:

- **Sidebar | main**: the 1-column gutter between the sidebar and the
  editor/response column.
- **Editor / response**: the boundary between the two main panes, today
  set only by the five-stop split control (`split.rs`,
  `paint/split_control.rs`), the `alt+w` cycle and the response header's
  ▲/▼ step pill.

Success: the user drags the sidebar to the width the request names need,
and it stays that width on any window size. The user drags the response
header to put the line where they want it. Someone who never touches a mouse
can do both from the keyboard (the user: "some people will use this app and
never touch a mouse").

## 2. What exists

- **Drags.** `app/mouse.rs` already runs four drag kinds: scrollbar thumbs
  (`App::drag`), text sweeps (`App::text_drag`), sidebar row reorder and
  Manage row reorder. Each follows the same lifecycle: armed on
  `Down(Left)`, driven by `Moved | Drag(Left)`, ended on `Up(Left)`.
  Lost releases are cleaned up in `cancel_stale_drags` and in the
  right-click cancel (the Ghostty/Linux quirks).
- **Layout.** `layout::compute_layout(area, collapse_t, response_t,
  editor_share)` runs every frame. The hit map is rebuilt with it.
- **Split state.** `split::SplitState { editor_minimized,
  response_minimized, ratio: SplitRatio }`, where `SplitRatio` is one of
  three stops (0.75 / 0.50 / 0.25). The ratio is sticky through the
  minimized endpoints. `AnimKey::SplitRatio`, `PaneCollapse` and
  `ResponseCollapse` ease every change. The state is saved per project as
  a token (`even`, `editor-big`, …) in `.local/state.toml`
  (`LocalState::main_split`). Today a minimized token loses the sticky
  ratio.
- **Pointer shapes.** `hit::PointerShape` sends Kitty OSC 22 (`default`,
  `pointer`, `text`, `grabbing`).
- **The response header strip** (`response.rs::draw_header_strip`) is 3
  rows on `theme.panel`:
  - row 0: the status chip, timing, size, media type and URL as plain
    text, then the ▲/▼ step pill at the right end;
  - row 1: the Tree/Raw/Headers tabs, right-aligned;
  - row 2: the icon actions (search/edit/save/copy/filter) on the left and
    the tabs' underline on the right.

  When the response is minimized, the strip is row 0 only
  (`response::COLLAPSED_HEIGHT = 1`). The not-yet-ready pane draws the
  same row 0 (a `—`/`failed`/spinner chip plus the pill).

## 3. Decisions

Attribution: **user** = the user said it; **mine (approved)** = I proposed
it and the user approved the whole design ("looks right, write it up");
**builder** = left to the implementer.

### 3.1 Sidebar | main

| # | Decision | From |
|---|---|---|
| S1 | The gutter column is the handle. Press on it and drag sideways to set the sidebar width. | mine (approved) |
| S2 | The gutter's look does not change: no hover highlight, no new glyph. The colour edge already shows where it is (the user: "i dont think we need to change anything about the line between the sidebar. there is already an edge there in the colors"). | user |
| S3 | The pointer shows a left-right resize shape (OSC 22 `ew-resize`) over the gutter and during the drag. It is not a paint change, so it is outside S2. Terminals without OSC 22 ignore it. | mine (approved) |
| S4 | The width is a **column count**, not a percentage, so it means the same thing on any window size. Until the user sets one, the sidebar keeps today's 28% (within S5's limits since the rework, S8b). | mine (approved) |
| S5 | Limits: the sidebar is at least about 16 columns, and the main column keeps at least about 40. When the window is too narrow for the saved width, the sidebar is squeezed for that frame, and the saved width is unchanged. Exact numbers are the builder's. (2026-10-23, review round 8: when a window is too narrow for both minimums, the sidebar's minimum wins, so a saved width never squeezes the sidebar below 16 columns or makes it vanish (§4 non-goal). The main column takes what is left, which can be under 40 on a tiny window.) (2026-10-23, rework round 7: the main column's minimum is **65**, not 40. At 40 the split chips painted over the Body tab and took its clicks, which a sidebar drag could now reach on any window. 65 is what the editor's tab row needs beside the chips: single-digit counts on its tabs, the Body badge and the "vars" indicator, pinned by `the_tab_row_fits_beside_the_split_control_at_the_main_minimum`. Larger counts on a narrow window can still overlap, as on main. Cost: the sidebar's widest is 54 columns on a 120-wide window.) | mine (approved); numbers: builder; 65: mine (approved) |
| S6 | Keyboard, both profiles: `alt+w` is context-based. While the sidebar has focus, `alt+w` widens the sidebar and `shift+alt+w` narrows it, a few columns per press, stopping at the limits (S5) with no wrap. Anywhere else, `alt+w` cycles the editor/response split as before (E11), so the split can't be cycled while the sidebar has focus. There are no separate sidebar-width keys. (2026-10-23, review round 3: an earlier version of this row had the sidebar footer advertise `alt+w` as the width key. That chip is dropped in both profiles: footers carry actions on the focused area's content, not window-level keys. See S6b for where the width keys are discoverable instead.) Step size: builder. The first press from the unset 28% default starts from the width on screen. | user (context-based on `alt+w`, step a few columns, no separate keys) |
| S6a | (2026-10-23, review round 2) The switch belongs to the key, not the split action. Where a key's keymap binding becomes an action, `cycle_split` / `cycle_split_back` turn into widen/narrow-the-sidebar (S6b's `StepSidebar`) while the sidebar has focus on the main screen. Everything else that sends the split action always moves the split, such as Vim's `:split`. A remap of `cycle_split` keeps the sidebar behaviour. (Corrected in round 3: an earlier version named a palette split entry as a cost, but the palette has no split entry.) The split chips' hover hint names `alt+w` only when `alt+w` would move the split, so not while the sidebar has focus. | mine (approved) |
| S6b | (2026-10-23, review round 3) The palette gets "Widen sidebar" and "Narrow sidebar" so a keyboard user can discover them (the user: "Add it to the palette so it is discoverable"). They send a sidebar-only action, `StepSidebar(±SIDEBAR_STEP_COLS)`, which always means the sidebar wherever focus is. They can't reuse `PaneResize`, because that follows Vim's current window: from the editor, `ctrl+w >` narrows the sidebar. S6a's `alt+w` translation turns into the same `StepSidebar`, and `PaneResize` remains vim's `ctrl+w` only. | you (in the palette); action shape: mine (approved) |
| S7 | Vim profile: (corrected 2026-10-23, review round 2: the vim keymap drops `alt+w` and `shift+alt+w` in favour of `:split` / `:splitback`, an earlier vim-profile ruling, so S6's context-based `alt+w` is arrows-only. Vim has no footer and never advertises `ctrl+w` sizing keys, so the vim profile shows no width chip either; the palette entries (S6b) are in both profiles. Round 6: a press on the sidebar gutter is not a click in the vim profile either. It keeps the body's Visual mode and pending operator, as Vim does for a press on a window separator. A half-typed router key (a list's `d`, a count, a `<C-w>` prefix) is still dropped, because every mouse press drops it (plan M1 Deviation 15). The response strip's press stays a click (E2a), because a click there ended Visual on main too.) (Rework round 8, corrected: in vim the strip is the editor window's status line, and Vim 9.1 keeps the current window and Visual mode through a press on a status line. Probed with `test_setmouse`, on both the current window's status line and the one above it. So in vim the strip press is a window-separator press like the gutter's. It keeps Visual and a pending operator, moves no focus and clicks nothing away. One predicate, `App::is_separator_press`, says which presses those are. E2a's click applies outside vim.) In addition, `ctrl+w <` and `ctrl+w >` (with counts) resize as Vim 9.1 does. In Vim they act on the current window, so with the editor or response focused, `ctrl+w >` widens that window and the sidebar narrows. Probe Vim for the exact semantics. The project rule is that every vim-profile choice matches Vim and is never asked of the user. | rule (follow what Vim does) |

### 3.2 Editor / response

| # | Decision | From |
|---|---|---|
| E1 | The response header strip is the handle, because it is always visible and sits in the middle (the user). The **whole strip** is draggable (the user asked "maybe the whole thing can actually trigger a drag?"), including the not-yet-ready and minimized one-row strip. | user |
| E2 | The tabs and icon buttons on the strip stay buttons: a press on one never starts a drag, so a click that wobbles a little never resizes. Everything else on the strip starts a drag: blank space, the status facts and the grip. (2026-10-23, task #95: the icon group and the tab group are each carved out of the handle, from one cell before the group's first control to one cell after its last, on the rows the group uses (icons: row 2; tabs: rows 1–2). The user: "i dont like how there is a space between the buttons where it swaps to draggable. can we carve out a space around the whole button and tab sections so it doesnt trigger the drag?" The one-cell margin is mine (approved). A press in the carve-out (`Hit::StripControls`) does what a strip press does (E2a, and S7 in vim) without arming the drag; it shows the plain pointer, doesn't warm the strip, and a right click there still opens the response's menu. Row 0, row 1 left of the tabs, and row 2 between the two groups still drag; on a strip narrow enough that the two margins meet, row 2 has no drag left, and a control always stays on top of a margin.) | mine (approved); carve-out: you; margin: mine (approved) |
| E2a | (2026-10-23, review round 1) A press on the strip first does what a click there did before this change: it focuses the response, clears the response's text selection and lit bracket pair, and runs the usual click-away, which closes an open URL or cell edit and blurs the jq bar. Then it arms the resize. Dragging the strip therefore also closes an open field, as clicking there always did. The sidebar gutter press still disturbs nothing, because a click there never did anything. (Outside vim only: in vim the strip press is a separator press, see S7.) | mine (approved) |
| E3 | A grip glyph sits at the right end of row 0, where the ▲/▼ pill is today (the user: "move the drag handle to the right"). It is a hint only. Glyph choice: builder, one-cell (memory: no VS16 emoji). | user; glyph: builder |
| E4 | Hover over a draggable part of the strip: the strip warms slightly, the grip lights up, and the pointer shows an up-down resize shape (`ns-resize`), as approved on the mockup. | mine (approved) |
| E5 | The line follows the pointer row by row with no snapping to stops and no easing during the drag. The ratio becomes free (any share), not one of three stops. | mine (approved) |
| E6 | Dragging into the editor's chrome (address bar + tab bar) lands on "response only" (editor minimized). Dragging into the bottom, so the response would lose its header strip, lands on "editor only" (response minimized). These are the same states the chips give. Snap thresholds: builder. (Amended 2026-10-27, task #129: the request body pane never has fewer than two rows. The user: "Shouldn't we not allow it to be less than 2 rows?" A drag, or a `ctrl+w` height key, stops with the body at two rows, and one row further minimizes the editor, as a drag into its chrome did; a minimized editor's first `ctrl+w +` lands on that floor. A floor, not a snap: the share is kept as it is, and a share that would draw fewer rows (a stop on a short window, a window made shorter) draws the floor. A window too short to give the body two rows draws the share's rows, and the body counts as minimized there, with no text and no search prompt. One rule for both profiles, on every tab, though only vim's search row needs it: `layout::editor_rows_for`, `layout::split_for_editor_rows`, `editor::CONTENT_MIN_HEIGHT`. Cost: vim's `{N}<C-w>_` on the editor with a count under the floor minimizes it.) | mine (approved); the two-row floor: you, its shape mine (approved) |
| E7 | Dragging the strip out of either minimized state restores the split to where the pointer is. | mine (approved) |
| E8 | The five-chip split control is replaced by **two chips**: the two end stops, "editor only" (▁▁) and "response only" (▇▇), in their current spot at the right of the editor's tab bar. The `alt+w` label is removed (the user: "c but no 'alt-w' in the header"). | user |
| E9 | The lit chip shows the active minimized state. Clicking the lit chip again restores the dragged split. Today a re-click is a no-op. | mine (approved) |
| E9a | The "dragged split" that E9 restores is the last share the split **settled** at, never a share the pointer passed on its way into a snap. A drag that ends minimized (E6) keeps the share the split had before that drag began, so a restore never brings back a response only a row or two tall. A keyboard step into an end stop keeps the share it left, as the sticky ratio does today. This share is also what P3 saves and what the collapse animation eases from. | mine (builder check) |
| E10 | The ▲/▼ step pill in the response header is removed (the user picked option C, which drops it). `Hit::SplitStep` and `Action::SplitStep` go with it, unless the builder finds another caller. | user |
| E11 | Keyboard: outside the sidebar (S6), `alt+w` / `shift+alt+w` keep cycling the five stops 100/75/50/25/0 (the user: stops "stay for the keyboard"). From a dragged share between stops, the next press goes to the nearest stop in that direction. (2026-10-23, after the rework's round 6: a drag stores its exact share and never snaps to a stop, and a press skips any stop that would draw the editor rows already on screen at the current height, so every press moves the line. This replaces old review round 5's K1 rule, which snapped a dragged share to a stop at the drag's own height and could leave a key press looking dead after the window changed height. The user: "I do not want any snapping when dragging. Just make it skip to the next stop". A skipped stop never passes its share on: a minimized end keeps the share the press left, per E9a.) | user (stops for keyboard; no drag snapping, skip to the next stop); in-between rule: mine (approved) |
| E12 | With the label gone, hovering a chip names `alt+w` in the footer, so the key can still be discovered. | mine (approved) |
| E12a | (2026-10-23, review round 4; replaces E12) Rounds 2–4 found that the chip hints paired a label with a key that does something else: `alt+w` steps to the next stop, not to that end. The chips are the two hide/show toggles, so their hints say that, in the palette's words, with the key that does exactly the same thing. ▁▁: `Hide the response` (lit: `Show the response`), `ToggleResponseCollapse`. ▇▇: `Hide the editor` (lit: `Show the editor`), `ToggleTableCollapse`. Each hint carries its toggle's key in the active profile, if it has one. `alt+w` is discoverable from the palette instead: "Step the split" and "Step the split back" (`CycleSplit` / `CycleSplitBack`), whose key column shows `alt+w` / `shift+alt+w`. In vim the column is empty: it shows keymap combos only, never Ex verbs, and the vim keymap drops these keys. (Amended 2026-10-27, task #132, replacing the last sentence: every palette row's key is spelled through `keyroute::spell`, as the footer spells a chip's action key, with the `SpellCtx` of where the palette opened, taken at both open sites. So the vim column shows `:split` / `:splitback` for these rows, and the others as the footer does: `u` for undo, `:w` for save. The sidebar's S6a check still drops the keymap's `alt+w` there, but not an Ex verb, which always moves the split; arrows opened from the sidebar still shows no key on these rows. Opened from a field that is typing, only keymap combos spell, as in the footer. The footer's chip-specific keycaps are not copied: the Quit row keeps `^C`, the key of the quit it runs (the footer's `:q` runs another), Send shows its combo, not `⇧enter`, and arrow names stay `alt+left`, not glyphs.) | mine (approved); amendment: mine (approved) |
| E12b | (2026-10-23, review round 4) Hiding the response gets a default key, **`shift+alt+p`** (`ToggleResponseCollapse`), paired with `alt+p` (`ToggleTableCollapse`, hide the editor) the way `shift+alt+w` pairs with `alt+w`. Both profiles. Until now only the editor toggle had a key. | mine (approved) |
| E13 | Vim profile: `ctrl+w +` / `ctrl+w -` (with counts) resize the editor/response line as Vim 9.1 does, following the current window. Probe Vim. Other `ctrl+w` sizing keys (`=`, `_`, `\|`) are out of scope unless the builder finds them trivial. (2026-10-23, review round 4: `ctrl+w _` and `ctrl+w =` are now trivial, so they're added. `_` maximizes the current window: from the editor it hides the response, from the response it hides the editor, and from the full-height sidebar it does nothing. `=` evens the split to 50/50 and, as Vim's CTRL-W = makes all windows equally wide too (corrected in round 6), sets the sidebar to half the body (`(body − 1) / 2`), clamped by S5. With a count, `N ctrl+w _` sets the current window's height to N, as in Vim. postui measures it in the focused pane's content rows (the editor below its chrome, the response below its header strip), clamped or snapped like any other row step. `\|` stays out.) | rule (follow what Vim does); scope: mine (approved) |
| E14 | Chip clicks and keyboard stops still ease as today. Only a live drag is un-eased. (2026-10-23, rework round 9: vim's `ctrl+w +`, `-` and `_` are row steps, not stops, and move the line un-eased, as Vim resizes a window at once. `ctrl+w =` goes to the 50/50 stop and eases like the other stops.) (2026-10-25, task #93: a project's seeded split (P5) is drawn un-eased too. It is the layout arriving, not a split change.) | mine (approved); row steps: mine; seed: mine |

### 3.3 Saving

| # | Decision | From |
|---|---|---|
| P1 | Both sizes are saved per project in `.local/state.toml`, next to `main_split`. Per-project is enough because the small-screen case is one machine with a resized window (the user). | user (one machine); placement: mine (approved) |
| P2 | Saved once when a drag ends, or on a keyboard step, never on every motion event. | mine (approved) |
| P2a | A resize drag that ends any way other than a release keeps where it got to and saves it, like a scrollbar drag, not like a row reorder (which reverts). That covers a lost release caught by `cancel_stale_drags`, the right-click cancel and `on_focus_lost`. Each of those sites saves. A resize is applied live, so what's on screen is what the user chose. Esc does not cancel a resize drag. | mine (builder check) |
| P3 | The split is saved as its exact share plus the minimized state, so a minimized split restores to the dragged share after a restart (today's token drops it). Token format: builder. (2026-10-23: old tokens (`even`, `editor-big`, `editor-full`, …) no longer load. They are junk like any unknown token, so the split opens at the default. The user: "There are no old projects this is unreleased".) | mine (approved); format: builder; no old tokens: you |
| P3a | Pane sizes stay out of undo entirely (the user: "i dont think resizes normally count in undo in other apps"). Today a journaled op (e.g. a space rename) snapshots the whole `.local/state.toml`, and undoing it puts the old `main_split` back into core memory through `Project::reload_all` (`postui-core/src/project/mod.rs`, ~line 798). The app never reseeds from it, so the screen keeps the current split but the next local-state write saves the stale one, and the next launch opens with it. This is an existing bug. Fix it here for both `main_split` and the new sidebar width: `reload_all` keeps the in-memory sizes, and the test `undo_of_a_space_rename_puts_the_local_state_back` (`project/undo.rs`) stops asserting that `main_split` comes back. Instead, it asserts that the size undo found is the size that stays. | user (sizes out of undo); fix shape: mine |
| P3b | (2026-10-23, review round 7; replaces P3a's fix shape and P1/P4's placement) Pane sizes move out of the journaled `.local/state.toml` into their own file, **`.local/layout.toml`** (`main_split`, `sidebar_width`). The journal never records that file, so undo and redo never see the sizes, and `reload_all`'s write-back is deleted. A test had shown the write-back grows each undo/redo entry by one `state.toml` write per replay whenever resizes and undo/redo interleave. The file is written through `Project` (file-access rule), atomically, at once and best effort, like `set_main_split` today. A `layout.toml` that can't be parsed is reported, never defaulted and overwritten (P4). There is no migration: postui is unreleased, so no project keeps its sizes in `state.toml` (the user, 2026-10-23: "There are no old projects this is unreleased"). A `main_split` left in a development `state.toml` is an unknown key, ignored, and opening a project never writes `layout.toml`. The sizes still stay out of undo (P3a, the user's rule); only the mechanism changes. | mine (approved); no migration: you |
| P4 | The sidebar width is a new `LocalState` field, written through `postui_core::project::Project` like `set_main_split` (file-access rule in `CLAUDE.md`). An unparseable value is reported, never silently defaulted and overwritten (memory: parse errors never default). | mine (approved) / project rules |

### 3.4 Rework after review round 9 (2026-10-23)

Nine review rounds didn't converge. Most of the later findings came from
two mechanisms that kept getting patched:

1. **Sidebar sizing measured against a copy of the last Main frame**
   (`App::main_layout`). Round 1 had the edge move only toward what was
   asked, judged against the drawn width. Findings in rounds 2, 3, 5, 7,
   8 and 9 trace back to that copy going stale. Round 9's O1: only Main
   frames refresh the copy, so a palette "Widen sidebar" run twice from
   the Manage screen widens once.
2. **Where sizes come from was decided at each open, switch and init on
   its own.** Round 6 seeded init from the new project's file, and round 8
   reset an unsaved split to the default. Round 9's O2: together they throw
   away the sizes chosen before "Create project", which main kept.

The user chose to replace both mechanisms rather than patch them ("lets
work on option 1"; the option was mine, so the rules below are `mine
(approved)`). The review count restarts at round 1.

| # | Decision | From |
|---|---|---|
| S8 | The sidebar's width on screen is a pure function of the saved width and the body's width: `layout::sidebar_cols(body_width, saved)`. `compute_layout` draws it, and every sizing path (the gutter drag, `alt+w`, the palette's Widen/Narrow, vim's `ctrl+w <`/`>`/`=`) reads the current width from it. A change is `clamp_sidebar_cols(body_width, wanted)`, stored only when it differs from the current width. The current width is always within S5's limits, and the clamp keeps it there, so a step or drag can only move the edge the way it was asked or not at all. No direction guard is needed and nothing compares against a drawn frame. A saved width the window squeezes stays saved until the user moves the edge narrower than what's on screen. | mine (approved) |
| S8a | The body's width comes from the terminal's size, recorded by every frame of every screen (`App::term_area`), not by Main frames alone. The terminal's size changes only with a resize event, and the event loop redraws after a resize event before it reads the next event, so the record is never stale when a key or click reads it. The split's row maths (the strip drag, vim's `ctrl+w +`/`-`/`_`) read the main column from the same layout at that size, using the split's state rather than its eased values. | mine (approved) |
| S8b | The unset default (S4's 28%) is clamped by S5's limits too, so it obeys the same function. On a window wide enough that the 28% is between the limits (59 columns and up; 92 and up since the main minimum became 65, see S5), it draws exactly as before. On a narrower window the default sidebar is 16 columns rather than 28% of the body (and between 59 and 91 columns it narrows to leave the main column 65, mine (approved)). This is what lets S8 drop the direction guard: an unclamped default could sit below the minimum, where a narrow step's clamp would widen it. | mine (approved) |
| P5 | One rule for where the sizes come from when a project becomes the open one: **a project's saved sizes win; a project opened or switched to with nothing saved opens at the defaults; a project created in place ("Create project", `InitProjectHere`) keeps the sizes on screen and saves them as its own.** Startup's `InitDefault` creates its project before the first frame, when nothing has been sized, so it opens at the defaults like any project with nothing saved. This replaces round 6's seeding of init from the new project's file: a `layout.toml` another postui wrote into the folder before the init is overwritten by the sizes on screen. One that can't be parsed still refuses the open (P3b, P4), the create's included: an unreadable file is reported, never replaced. A launch seeds the sizes whether or not the project's spaces could be made, as a switch does. (2026-10-25, task #93: with animations on, the seeded split is drawn at once at a launch and at a switch. It never eases out of the default or out of the previous project's split, E14.) | mine (approved); drawn at once: mine |

## 4. Goals, non-goals, anti-goals

**Goals:** both dividers draggable; both resizable from the keyboard in
both profiles; widths and the split remembered per project.

**Non-goals:**
- Resizing anything else: the header bar, the footer, the table's
  columns, the Manage screen.
- Collapsing the sidebar entirely.
- Any change to the sidebar edge's look (S2).

**Anti-goals (a failure even if it works):**
- A tab or icon click that turns into a resize (E2).
- A drag that lags the pointer because it eases (E5, E14).
- A drag state that a lost release leaves live. The new drag kind must be
  cleared everywhere the others are: `Up(Left)`, `cancel_stale_drags`,
  and the right-click cancel. List every reader of the new state first
  (memory: new state needs a consumer audit).
- Anything that only the mouse can do (memory: no mouse-only and no
  vim-only UI).

## 5. Left to the builder

- Step sizes, limits and snap thresholds (S5, S6, E6).
- The grip glyph (E3) and the exact hover tint (E4), within the theme's
  existing hover ladder.
- The saved split's token format (P3).
- How the drag state is shaped: one new enum next to `Drag`, or a variant
  of it.
- Whether the sidebar's scrollbar column, right next to the gutter, needs
  any hit-priority care. The gutter is one column wide, like a tmux pane
  border.
- Rewriting the tests that cover the five-chip control, the step pill
  and fixed-coordinate clicks.
