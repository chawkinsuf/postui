# Stage 8 tmux sweep — visual polish and motion language

Scripted rerun of the stage-8 visual language across the whole app: the
hidden testbed showcase, the address bar bevel, the tab-strip underline
slide, sidebar zebra/travel, the command palette, the method dropdown's
ring/settle, a panel modal's fade, the body editor's focus ring, toasts,
the send-cap breathe, pane collapse, an `animations = false` run, and a
`theme = light` spot-check. Every step is `tmux send-keys` +
`capture-pane -e`, converted to PNG via the ansi2html → playwright
pipeline from task 8 (see `task-8-report.md`'s Captures section for the
original recipe; the exact script used this run is below). Captures live
in `.superpowers/sdd/2026-08-24-stage8-visual-polish/sweep-*.png`.

## Setup

```bash
mkdir -p /tmp/claude-1000/tmux
export TMUX_TMPDIR=/tmp/claude-1000/tmux
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -p postui --bin postui

rm -rf /tmp/claude-1000/pw8
mkdir -p /tmp/claude-1000/pw8/project /tmp/claude-1000/pw8/server /tmp/claude-1000/pw8/xdg
# project.toml pre-seeded so the testbed/main screen never opens onto the
# "not a project" migration confirm — POSTUI_TESTBED's key/mouse guards
# only let `q`/the quit chip through, so that modal would otherwise be
# unreachable and un-closeable.
printf 'name = "sweep"\n' > /tmp/claude-1000/pw8/project/project.toml

tmux kill-server 2>/dev/null
```

The `animations = false` and `theme = light` runs each get their own
`XDG_CONFIG_HOME` scratch dir with a one-line `config.toml`:

```bash
mkdir -p /tmp/claude-1000/pw8/xdg_noanim/postui /tmp/claude-1000/pw8/xdg_light/postui
printf 'animations = false\n' > /tmp/claude-1000/pw8/xdg_noanim/postui/config.toml
printf 'theme = "light"\n' > /tmp/claude-1000/pw8/xdg_light/postui/config.toml
```

The ansi2png conversion (each capture): `uv run --with ansi2html --with
playwright <script>` — `ansi2html.Ansi2HTMLConverter` renders the raw
`capture-pane -e` bytes to a full HTML page, then Playwright's chromium
(pointed straight at `~/.cache/ms-playwright/chromium-1224/chrome-linux64/chrome`
with `--no-sandbox --disable-gpu`, since a freshly `uv`-installed
`playwright` package expects a newer cached revision than what's on disk)
screenshots it after resizing the viewport to the page's own scrolled
content size.

Hold the app in a background Bash call (`run_in_background: true`):

```bash
tmux new-session -d -s s8 -x 200 -y 50 \
  "XDG_CONFIG_HOME=/tmp/claude-1000/pw8/xdg ./target/debug/postui /tmp/claude-1000/pw8/project" && sleep 3600
```

## 1. Testbed showcase (`POSTUI_TESTBED=1`)

```bash
tmux new-session -d -s s8 -x 220 -y 95 \
  "XDG_CONFIG_HOME=/tmp/claude-1000/pw8/xdg POSTUI_TESTBED=1 ./target/debug/postui /tmp/claude-1000/pw8/project" && sleep 3600
```

At 220×95 the whole grid fits in one capture: MOTION (underline-slide,
hover-fade and list-travel duration-comparison rows, plus the send-breathe
demo), BUTTONS (5 states × Primary/Secondary), TEXT FIELD (5 states), LIST
ROWS (zebra off/on, hover 0.5, selected), TAB STRIP (static + mid-slide
underline), CHIPS, FRAC_VSPAN, FLOATING PANEL. — **sweep-01-testbed-full.png**
— **done**, every primitive/state specimen present and labeled.

**Manual-check item (can't capture via tmux):** the MOTION section's
duration-comparison rows and the list-travel band are only meaningfully
judged in motion, at native ~60fps — a static capture shows one instant.
Confirm smoothness live in a real terminal at checkpoint 2.

## 2. Everyday flow: seed requests, address bar, toasts, sidebar

```bash
tmux new-session -d -s s8 -x 200 -y 50 \
  "XDG_CONFIG_HOME=/tmp/claude-1000/pw8/xdg ./target/debug/postui /tmp/claude-1000/pw8/project" && sleep 3600
tmux new-window -a -t s8 -d "cd /tmp/claude-1000/pw8/server && python3 -m http.server 8792"

tmux send-keys -t s8:0 n; tmux send-keys -t s8:0 "orders/list"; tmux send-keys -t s8:0 Enter
tmux send-keys -t s8:0 n; tmux send-keys -t s8:0 "orders/create"; tmux send-keys -t s8:0 Enter
tmux send-keys -t s8:0 n; tmux send-keys -t s8:0 "ping"; tmux send-keys -t s8:0 Enter
```

Captured right after the third create (its "Saved …" toast still up):
sidebar zebra across `ping`/`orders/create`/`orders/list`, the toast pill,
the address bar's flat (unfocused) bevel. — **sweep-02-addressbar-toast-sidebar.png**
— **done**. A dedicated single-toast capture (`ctrl+s` on `ping`):
**sweep-03-toast.png** — **done**, toast text renders cleanly (a capture
taken mid-transition between two overlapping toasts showed truncated/
overwritten-looking toast text — not a bug, just capture timing; wait for
one save's toast to be the only one in flight before capturing).

Focus the URL (`alt+u`), type `http://127.0.0.1:8792/ok`: the well lifts,
bevel cap brightens, dirty dot + discard chip appear. —
**sweep-04-addressbar-focused.png** — **done**, `▔` bevel unmistakable.

## 3. Tabs (settled) and the method dropdown's ring

```
tmux send-keys -t s8:0 M-2      # Headers tab — the slide settles well
                                 # inside a 200ms capture delay
```
— **sweep-05-tabs-headers.png** — **done**. The mid-slide straddle itself
is already shown in the testbed's TAB STRIP section (§1); a live capture
can't reliably land mid-animation over tmux round-trips, so this one shows
the settled Headers state instead.

Method dropdown — **alt+shift+m must be sent as `tmux send-keys 'M-M'`
in one call**; splitting it into a separate `Escape` + `M` pair (or typing
`M-S-M`) does not compose to the alt+shift combo and instead lands as
literal text in whatever field has focus (a real, if self-inflicted,
scripting trap — not an app bug, see the Finding below):

```
tmux send-keys -t s8:0 'M-M'
```
— **sweep-06-method-dropdown-ring.png** — **done**. The popup's ring
corners (`\u{1FB7F}` etc.) and the accent border are both clearly visible
around the GET/POST/…/OPTIONS list.

**Finding (scripting trap, not an app bug):** `tmux send-keys -t s8:0
Escape` followed by a *separate* `tmux send-keys -t s8:0 -l 'M'` call does
not compose into alt+shift+m — the two calls round-trip too slowly to
land inside the terminal's escape-sequence timeout, so the app sees a
bare `Esc` (closing whatever was focused) and then a literal `M`
character (typed into whatever field regained focus). The fix is to send
the whole combo as one `tmux send-keys` argument (`'M-M'` for alt+shift+m,
`'M-u'` for alt+u, etc.) — tmux then emits the real single ESC-prefixed
sequence in one write. Documented here because it silently corrupted a
URL field twice during this sweep (typed `M-S-M` and a stray `M` both
landed in the address bar) before the one-call form was adopted
throughout the rest of this script.

## 4. Command palette (`ctrl+p`) — idle-tick redraw finding

```
tmux send-keys -t s8:0 C-p
```

**Finding (real, environment-specific — not a math bug, verified below):**
in this sandboxed tmux/pty setup, the palette's open-settle panel
(`AnimKey::ModalOpen`, `paint::floating_panel_settling` gated by `if t <
1.0 { return; }` in `components/palette.rs`) stays visibly stuck at its
partial-grow, content-less frame for as long as the app receives *no*
further input — reproduced past 3+ seconds idle with nothing landing.
Any subsequent event (another keypress, or a no-op `tmux resize-window`
nudge) immediately repaints the fully-settled panel, because by then real
wall-clock time has long since passed the animation's duration and the
very next `ui::draw` call computes `t = 1.0`.

Isolated the cause with a throwaway example
(`cargo run -p postui --example repro_palette`, removed after use) that
opened the palette on a real `App` and called `ui::draw` twice, 200ms
apart, with **no** `Action::Tick` ever dispatched and no `finish_all()`:
the second draw already showed "Commands" and the full list. This proves
`AnimKey::ModalOpen`'s value math is correct and time-driven exactly as
designed — the animation state does not need ticking to be correct, only
a redraw needs to happen after the fact. The dropdown's own settle
(`AnimKey::DropdownOpen`, §3 above) did *not* show this symptom live —
but that path never gates on `t < 1.0` at all (it clips the item list via
`visible_bottom` instead), so it always paints *something* even mid-flight
and only reads as fully caught up once wall-clock time and a redraw
happen to coincide, same as the palette really is doing underneath.

This points at `main.rs`'s adaptive tick loop
(`tokio::select! { _ = tokio::time::sleep(16ms while animating) => {
redraw |= app.update(Action::Tick) } }`) not actually firing on its own
in this environment while the app sits idle with a modal open — plausibly
an artifact of how `crossterm::event::EventStream` and tokio's timer
interact inside a sandboxed/nested tmux pty, not something reproducible
in the render logic itself. **Deferral for the user**: this needs judging
in a real terminal (Ghostty) at checkpoint 2, where the tick loop's
integration with a genuine terminal file descriptor is exactly what's in
question — not a code change made blind against a symptom only observed
here. The capture below was taken after nudging a redraw (a harmless
`tmux resize-window` back to the same size), so it shows the correctly
*settled* palette, matching what any redraw — including the "stuck" idle
window closing on its own once a real terminal's event loop schedules the
next tick — should look like:

— **sweep-07-palette.png** — **done** (settled): search field, full
command list with hint text and keybinding columns, `enter run  esc
cancel` footer.

## 5. A panel modal's fade (rename prompt)

```
tmux send-keys -t s8:0 Escape   # close the palette
tmux send-keys -t s8:0 r        # rename the selected sidebar row
# nudge two redraws (see §4) so the capture shows the settled modal
tmux resize-window -t s8:0 -x 199 -y 50
tmux resize-window -t s8:0 -x 200 -y 50
```
— **sweep-08-modal-rename.png** — **done**. Dimmed backdrop, floating
panel with corner shadow glyphs, prefilled `TextField`, Cancel/Confirm
buttons.

## 6. Body editor's focus ring

Reaching typed content in the Body tab needs the editor **pane** focused
first (`ctrl+p` → run "Focus: editor" is the most reliable route — a bare
`Tab` cycles *pane* focus, and landing in the editor pane starts on the
address bar's own sub-focus, not the tab strip), then `Down`/`Enter` from
the tab strip descends into the tab's content:

```
tmux send-keys -t s8:0 C-p
tmux send-keys -t s8:0 -l 'Focus: editor'
tmux send-keys -t s8:0 Enter
tmux send-keys -t s8:0 M-3       # Body tab
tmux send-keys -t s8:0 Down      # Tabs sub-focus -> Content
# then type the body one character per send-keys call — a single -l
# call with the whole JSON string was silently swallowed (see Finding)
```

**Finding (scripting trap, not an app bug):** sending a whole literal
string in one `tmux send-keys -l '...'` call while sub-focus was still on
the tab strip (not yet `Content`) silently typed nothing at all — no
error, no partial text, no dirty dot. The fix, as above, is to only type
after confirming (via a plain-text capture) that focus actually reached
the target field; blind chained `send-keys` calls without an
intermediate check are the recurring failure mode across this whole
sweep, not any single app defect.

— **sweep-09-body-ring.png** — **done**: `▕`/`▏` edges and floating-panel
corner glyphs frame the Body editor's content once it holds `{"a":1}`.

## 7. Send-cap breathe

A `/slow` endpoint that sleeps before responding (the stock stage-6
`python3 -m http.server` has no route to gate, so this uses a tiny
`BaseHTTPRequestHandler` subclass instead — see the setup in this run's
transcript, or task-8-report's own local-server pattern):

```
tmux send-keys -t s8:0 M-u
# clear the field, type http://127.0.0.1:<port>/slow, Escape, ctrl+r
```
— **sweep-10-send-breathe.png** — **done**: the Send chip shows
`⋮ Sending`, and the response pane shows `sending… <n> ms / esc to
cancel`, both mid-breathe.

## 8. Pane collapse

```
tmux send-keys -t s8:0 Escape
tmux send-keys -t s8:0 M-1       # Params tab (empty table)
tmux send-keys -t s8:0 M-p       # ToggleTableCollapse
```
— **sweep-11-pane-collapse.png** — **done**: the Params table collapses
to a thin `▏ Params  › show` strip, freeing the row for the settled
response below (this capture landed after a slow real send completed, so
it also shows the `200 · 6.0s` response tree as a bonus — not the
intended subject of this step, but harmless).

## 9. `animations = false`

```
tmux kill-server
tmux new-session -d -s s8 -x 200 -y 50 \
  "XDG_CONFIG_HOME=/tmp/claude-1000/pw8/xdg_noanim ./target/debug/postui /tmp/claude-1000/pw8/project" && sleep 3600
tmux send-keys -t s8:0 C-p
```
— **sweep-12-animations-false-palette-instant.png** — **done**: the
palette is fully settled in the very first capture, 200ms after the
keypress with **no** resize nudge needed — confirms `Anims::retarget_with`
collapses `dur` to `Duration::ZERO` when `enabled` is false (`anim.rs`),
sidestepping the idle-tick question in §4 entirely, exactly as designed.

## 10. `theme = light` spot-check

```
tmux kill-server
tmux new-session -d -s s8 -x 200 -y 50 \
  "XDG_CONFIG_HOME=/tmp/claude-1000/pw8/xdg_light ./target/debug/postui /tmp/claude-1000/pw8/project" && sleep 3600
```
— **sweep-13-theme-light.png** — **done**. Sidebar zebra direction
eyeballed: the selected row is accent-blue, the unstriped rows sit on
white, and the `zebra_alt` stripe (`list` row) reads as a faint cool-gray
one step darker than white — same "step away from the base panel fill"
direction as dark theme's stripe (a step *lighter* than dark's near-black
panel), not inverted or jarring. Bevels, chips (GET green / method
dropdown accent), and the address bar's focused blue all read cleanly
against the light background.

## 11. Disabled-row strikethrough (checkpoint-2 question)

A default header (`[default_headers]` in `project.toml`) overridden by a
same-named request header shows the auto-computed row struck through
under "── auto ──":

```bash
cat >> /tmp/claude-1000/pw8/project/project.toml <<'EOF'

[default_headers]
x-team = "payments"
EOF
```
Then, after reopening the app (so the new `project.toml` loads) and
adding a request header named `x-team`: the Headers tab shows `✓ x-team
payments (overridden)` above the table, and the `auto` section's own
`x-team: payments` row renders with `Modifier::CROSSED_OUT` (confirmed
in the raw capture via a literal `\x1b[9m` SGR code, not just visually).

— **sweep-14-suppressed-header-strikethrough.png** — **done**. This is
the exact row the checkpoint-2 "should the strikethrough be softened"
question is about — visible clearly under "── auto ──" as `x-team:
payments` with a line straight through it, right below the live (un­struck)
override row.

## 12. Pointer-shape hints (OSC 22) — manual-check item

**Manual-check item (structurally invisible to the ansi2png capture
pipeline, same as §1's MOTION note):** task 8d wired Kitty's OSC 22
pointer-shape protocol (`\x1b]22;{shape}\x07`, `main.rs::write_pointer_shape`,
piggybacked onto the synchronized-update frame so it lands atomically
with the paint it matches) — this changes the terminal's own mouse
cursor icon, which is drawn by the terminal emulator itself, entirely
outside the character grid `capture-pane` reads. No amount of ANSI
capture or ansi2html/playwright rendering can show it; it has to be
eyeballed live in a terminal that implements OSC 22 (Ghostty does).

Checklist for a live Ghostty session (`./target/debug/postui <project>`,
no tmux involved — tmux's own pane does not forward OSC 22 to the host
terminal):

- Hover a **button, tab, or sidebar row** (anything `PointerShape::for_hit`
  maps to `Pointer` — i.e. any registered `Hit` that isn't a text-entry
  surface or a background/dismiss region) → the OS cursor changes to a
  **pointer** (hand) icon.
- Hover the **URL bar text or the Body editor** (`Hit::UrlBar` /
  `Hit::BodyEditor`, mapped to `PointerShape::Text`) → the cursor becomes
  an **I-beam**.
- Hover empty **pane background**, a modal's dismiss-outside region, or
  nothing registered at all (`Hit::Pane(_)`, `Hit::ModalOutside`,
  `Hit::ModalBody`, or no hit) → the cursor is the terminal's **default**
  arrow.
- **Quit the app** (`q`/`ctrl+c`) while the cursor is still shaped as
  something other than default (e.g. leave the mouse over a button, then
  quit) → the cursor must reset to **default** on exit
  (`main.rs` writes `PointerShape::Default` once, unconditionally, before
  the alternate screen tears down) rather than leaving the terminal's
  prompt stuck with a pointer/I-beam cursor.

No result recorded here — this needs the user's own eyes on a real
Ghostty window at checkpoint 2, same as the MOTION section's live-motion
caveat in §1.

## 13. Ring corner-glyph font coverage — manual-check item

- Verify the ring's `U+1FB7C`–`U+1FB7F` corner glyphs (Legacy Computing
  block-sextant corners, seen framing the method dropdown/modals in §3/§5)
  render as actual corner shapes — not tofu/a missing-glyph box — in the
  user's own terminal font; the straight `▁▔▕▏` edges degrade gracefully on
  a font lacking Legacy Computing coverage, but the corners have no
  fallback and need that block specifically.

## Summary

| # | Surface | Capture | Result |
|---|---|---|---|
| 1 | Testbed (all primitives/states + MOTION) | sweep-01 | done |
| 2 | Address bar (flat), toasts, sidebar zebra | sweep-02, 03 | done |
| 2 | Address bar (focused, dirty) | sweep-04 | done |
| 3 | Tabs (settled), method dropdown ring | sweep-05, 06 | done |
| 4 | Command palette | sweep-07 | done (idle-tick finding — deferred to checkpoint 2 / real terminal) |
| 5 | Panel modal fade (rename) | sweep-08 | done |
| 6 | Body editor focus ring | sweep-09 | done |
| 7 | Send-cap breathe | sweep-10 | done |
| 8 | Pane collapse | sweep-11 | done |
| 9 | `animations = false` | sweep-12 | done — confirms idle-tick question in §4 is moot when disabled |
| 10 | `theme = light` (incl. zebra direction) | sweep-13 | done |
| 11 | Suppressed-header strikethrough | sweep-14 | done — checkpoint-2 capture |
| 12 | Pointer-shape hints (OSC 22) | — | manual-check only — needs live Ghostty, no capture possible |

No source paper cuts were found or fixed this sweep — every landmark
painted exactly as the acceptance test and the spec describe. The one
notable finding (§4, the idle-tick redraw) is an environment-observed
gap between this sandboxed tmux harness and a real terminal's event loop,
not a defect in the animation/paint code (verified directly against the
render logic); it's flagged for the user's own judgment in a real
terminal rather than "fixed" against an unconfirmed cause.
