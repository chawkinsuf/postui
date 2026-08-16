# Stage 4 — Mouse-first GUI design

Date: 2026-08-16
Status: approved in chat section-by-section; this document is the binding write-up.
Prior authority: `2026-08-15-postui-design.md` (foundation), stage 2/3 specs. Where this
document touches the same behavior, this document wins.

## 1. Goal and scope

Make postui feel like a GUI application for anyone who reaches for the mouse, while the
keyboard remains a complete, never-required alternative (no-vim rule: vim keys are only
optional aliases; nothing is gated behind modes or vim-style keys).

**In scope**

- Hit-testing infrastructure (`HitMap`) and hover highlighting.
- Clickable: sidebar rows + folder arrows, editor tabs, method dropdown, table-editor
  checkboxes/rows/cells, in-pane buttons (Send/Cancel, + New request, Copy body,
  Save body to file, Copy URL), header project/env selectors, palette/chooser/var-picker
  rows, response view tabs and JSON-tree arrows, footer hint chips + palette chip,
  click-outside-closes for modals.
- Draggable scrollbars (sidebar, response, body).
- Copy buttons + tiered clipboard (`clipboard_cmd` → arboard → OSC 52 with size threshold).
- Palette frequency ranking (frecency) + clickable palette.
- Parked stage-3 carryover fixes (§10).

**Out of scope (explicitly deferred)**

- Menu bar / toolbars (palette is the command surface; not planned).
- In-app text selection (document Shift+drag; terminal-native selection).
- Pane resizing by dragging borders.
- Copy-as-curl; copying JSON subtrees from the tree view.

## 2. Interaction surfaces

### Header bar

`project · env` becomes two click targets: project name opens the project chooser,
env name opens the env chooser (identical to ctrl+o / alt+e). Each renders with a
trailing `▾`; hover underlines the hovered name.

### Sidebar

- Single click on a request row opens it (existing open path, dirty-gate included).
- Folder rows: clicking the `▸`/`▾` arrow cell toggles expansion; clicking the name
  selects the row; double-click on the name toggles expansion.
- `+ New request` button pinned at the top of the pane, always visible (empty sidebar
  keeps its invitation to act).

### Editor

- Method cell is a click target opening the method dropdown (§4).
- `[ Send ]` button at the right end of the URL row; while a request is in flight the
  same slot renders `[ Cancel ]` at the same width (no layout jump). Click behaves
  exactly like ctrl+r / esc-cancel.
- `Copy URL` button adjacent to the URL field; copies the URL as written
  (`{{vars}}` intact).
- Tabs Params / Headers / Body clickable.
- Table editor: click a checkbox cell toggles enabled; click a row selects it; click a
  cell begins editing that cell. Body tab keeps click-to-place-cursor (unchanged).

### Response pane

- View tabs (Tree / Raw / Headers) clickable.
- JSON-tree `▸`/`▾` arrows toggle on click.
- Title row: `Copy body` and `Save body to file` buttons.
- Headers view: per-row copy affordance copying that header's value.

### Modals, choosers, palette

- Rows highlight on hover and are clickable. Choosers/var picker: click selects, click
  again or double-click confirms. Palette: single click runs the command (Enter remains
  for keyboard).
- Clicking outside any modal closes it (same as Esc). While a modal is open, only its
  registered hits plus the outside-close region are live.

### Footer

Stays the context-sensitive keyboard-hint bar, updating with focus as today. Each hint
chip becomes clickable and dispatches its action. A rightmost `⌘ Palette` chip opens
the palette.

### Visual language

One button style built from existing theme tokens; no new colors. Bracketed label
(`[ Send ]`), `accent` foreground at rest, inverted (accent background) on hover,
`text_muted` when disabled. Hover inversion is the single deliberate boldness spend;
everything else stays quiet. Interface writing: buttons are named for what they do,
the verb stays consistent through its flow (Send → "Sending…" → "Sent"; Copy →
"Copied response body"), errors state the fix, empty states invite action.

## 3. Hit-testing, hover, drag

### HitMap

New module `crates/postui/src/hit.rs`.

- `HitMap`: `Vec<(Rect, Hit)>` rebuilt each frame during render.
- `Hit`: one flat enum of typed targets, e.g. `SendButton`, `MethodSelector`,
  `EditorTabBtn(EditorTab)`, `SidebarRow(usize)`, `SidebarFolderArrow(usize)`,
  `HeaderProject`, `HeaderEnv`, `FooterChip(..)`, `TableCheckbox(usize)`,
  `ChooserRow(usize)`, `PaletteRow(usize)`, `ResponseTab(..)`, `JsonTreeArrow(usize)`,
  `ScrollbarThumb(Pane)`, `ScrollbarTrack(Pane, TrackSide)`, `CopyButton(CopyTarget)`,
  `ModalOutside`.
- Point lookup: linear scan, last-registered wins (drawn-on-top wins).

### Registration helpers

Render helpers that draw and register in one call so a target cannot be drawn without
being registered, and hover styling lives in one place:
`button(frame, hits, rect, label, hit, hovered, enabled)`, plus chip/tab/row variants.

### Event flow

- Mouse Down → HitMap lookup → dispatch through the existing `Action` enum (the same
  actions keyboard bindings emit). Parameterized hits map to existing parameterized
  actions or small new ones. No mouse-only behavior forks.
- Modal routing mirrors key routing: topmost modal first.

### Hover

- Enable mouse-motion capture.
- On Moved: HitMap lookup; redraw only when the resolved `Hit` differs from stored
  `hovered: Option<Hit>`. Components style via `hovered == Some(my_hit)`.

### Drag

Small state machine in app state: Down on `ScrollbarThumb` stores
`Dragging { pane, grab_offset }`; Moved maps y-delta to scroll offset; Up clears.
While dragging, other motion handling is suspended.

### Double-click

Store last click `(Hit, Instant)`; second Down on the same `Hit` within 400 ms is a
double-click. Users: folder-name toggle, chooser confirm.

## 4. Scrollbars

Visible in scrollable panes (sidebar, response, body) whenever content overflows.
Thumb is proportional and draggable; clicking the track above/below the thumb pages
up/down. Wheel scrolling unchanged.

## 5. Method dropdown and the popup primitive

- One lightweight popup primitive (bordered list floated at an anchor rect, rendered
  last so it registers on top). Not a full modal, but participates in the same
  interception: while open, only its hits are live and stray keys don't leak.
- Method selector: click the method cell (or run the new "Choose method" palette
  command) → popup anchored below the cell listing GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS
  with current marked + preselected. Hover highlights; click or arrows+Enter select and
  close; Esc / click-outside closes without change. Flips upward near the bottom edge;
  seven items, no internal scrolling.
- The existing method-cycling key keeps working unchanged.

## 6. Command palette

- Rows hover-highlight; single click runs the command; click-outside closes.
- Frecency ranking: each execution records `{count, last_used}` per command ID.
  Empty query → sort by frecency (count weighted toward recency). Non-empty query →
  fuzzy score dominates exactly as today; frecency breaks ties only.
- Persistence: app-level file `~/.config/postui/ui.toml`
  (`[palette.usage]` table), written on quit with existing state saves. Missing or
  corrupt file = empty stats, never an error. Absolute timestamps; counts capped by
  halving all counts when any reaches 1000.

## 7. Clipboard and copy actions

### Tiering (module `clipboard.rs`)

1. `clipboard_cmd` (new config option): if set, pipe payload to the user's command;
   takes priority at any size.
2. `arboard` (native OS clipboard): the normal desktop path; no size limit.
3. OSC 52 escape sequence: SSH/headless fallback, with a size threshold
   (default 64 KiB, configurable). Payloads at or over the threshold are **not sent
   and not auto-saved**; the toast explains the ways forward: "Too large for the
   terminal clipboard — use Save body to file, or set clipboard_cmd in config."
   Below the threshold, send in full. Nothing is ever silently truncated.

Every successful copy confirms with a toast naming what was copied. Total failure of
all tiers toasts the remedy ("Clipboard unavailable — try Shift+drag to select").

### Targets

- `Copy body` (response title row): raw response body text (same bytes from Tree or Raw).
- Per-row header copy (response Headers view): that header's value.
- `Copy URL` (editor): URL as written, `{{vars}}` intact.
- `Save body to file` (response title row): writes body to a prefilled path
  (`~/Downloads/<request>-response.<ext>`), toast shows the path. Always available;
  the guaranteed any-size path.

### Stretch (droppable)

OSC 52 query-verify: after an OSC 52 send, attempt the read-back query with a short
timeout; warn only on a mismatched response. Most terminals block the read direction,
so no response means "can't verify", not "failed". Drop this task if it turns into a
terminal-compatibility swamp.

## 8. Text selection (deferred)

Mouse capture disables terminal-native drag-select; document Shift+drag (most
terminals bypass capture with Shift) in README/help. In-app selection is a later stage.

## 9. Keyboard parity rule

Every mouse affordance in this stage is an addition to an existing keyboard path, or
ships with one. Nothing becomes mouse-only. Footer hints remain the keyboard
discoverability surface; the palette remains the command surface.

## 10. Stage-3 carryover fixes (folded in)

From the parked stage-3 final-review minors:

1. Chooser/var-picker lists scroll (offset tracking / ListState); selection can no
   longer move off-screen past 16 items.
2. `CreateProject` no longer registers + sets `last` before the dirty gate resolves;
   `last` is left to `ForceSwitchProject`.
3. `slugify` empty-result guards (non-ASCII names): non-empty checks + toast on the
   New Project prefill path; empty path no longer inits cwd.
4. `CycleEnv` performs the spec-§7 reload (symmetry with `OpenEnvChooser`).
5. Bare-root quit no longer writes `./.local/state.toml` when `project.root` is empty.
6. `postui --help`: leading-dash check + usage line in `main.rs`.
7. `ForceOpenRequest` persists `open_request` like the other force paths.
8. Variable-manager radar: case-insensitive suppression of templated default-header
   names; reject case-differing duplicate keys in `PrepareContext.default_headers`;
   clear stale `table.editing` when focus moves away (no InsertVarText capture).
9. Cosmetic: `CycleProject` skips dead registry paths like the chooser; env-switch
   failure no longer emits warning + stale "env:" toast back-to-back.

## 11. Testing

### Automated

- HitMap keeps everything unit-testable: render into a buffer, find the rect registered
  for a `Hit`, synthesize a click at its center, assert resulting state/actions.
- Coverage floor: every clickable surface has at least one click test; hover has
  change-detection tests (redraw only on hover change); drag has thumb-drag math tests;
  double-click has a timing test; palette frecency has ordering + persistence tests;
  clipboard has tier-selection tests including the OSC threshold and the over-threshold
  toast; each §10 fix lands with a regression test.
- Stage-4 acceptance test: a full mouse-only flow — open app → click `+ New request` →
  click method → pick POST from the dropdown → click tabs → toggle a checkbox → click
  Send (wiremock) → click Copy body.

### tmux visual verification (required per task)

Every UI task ends with running the real binary under tmux (socket on `$TMPDIR`),
injecting keys and SGR mouse sequences, capturing styled pane output, and inspecting
the rendering for polish: hover states, button alignment, dropdown clipping, scrollbar
proportionality. Findings are fixed before the task closes. The stage's final review
includes a full mouse-only walkthrough in tmux.

### Manual TTY sweep (user)

Real-terminal behavior tmux can't prove: the user's terminal's motion reporting,
native clipboard integration, double-click feel, hover latency, Shift+drag selection.
Checklist delivered with the implementation plan.

## 12. New/changed configuration

- `clipboard_cmd` (string, optional): external clipboard command; payload on stdin.
- `osc52_limit` (bytes, default 65536): OSC 52 size threshold.
- `~/.config/postui/ui.toml`: new app-level UI state file (palette usage).
- New palette command: "Choose method" (opens the method dropdown). No new default
  keybindings; everything else reuses existing actions.
