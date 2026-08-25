# Stage 8 — Visual Polish & Motion (design)

Date: 2026-08-24. Status: approved direction from screenshot-driven review with the user; supersedes the stage-5 control anatomy where they conflict.

## 1. Goal and thesis

Make postui read as a modern GUI that happens to run in a terminal — the most polished TUI on GitHub. The quality bar is the user's Textual app (caption-reviewer), per the reference screenshots reviewed in chat: one dark ground, controls carved from subtle fill deltas, a single accent doing all the talking, thin strokes instead of chunky caps, dense rows, and motion as the polish layer.

Boldness is spent in exactly one place: the accent (buttons, selection bars, focus, the sliding underline). Everything else is quiet lightness steps on the ground color.

This is a restyle stage: **no behavior changes**. Same actions, same hit targets (geometry may shift; semantics may not), same keyboard model, same storage. The previously approved visual-ceiling spike (kitty-graphics icons + sextant button treatment) is **deferred**, to be re-evaluated after this stage ships.

## 2. Decisions (user-approved in chat)

1. Restyle now; kitty/graphics spike later, re-evaluated after this stage.
2. **Thin bevel everywhere**: the `▔` light-top / `▁` dark-bottom eighth-block treatment on a solid fill replaces the half-block-cap control anatomy app-wide. Half-caps are retired.
3. **Focus**: thin accent ring on big containers (body editor, open dropdown overlay); fill-lift (+0.12, edges following) on small controls — the shipped no-rings-on-inputs decision stands for one-line controls.
4. **Lists**: single-line dense pitch everywhere; the 2-line `PillRow` pitch is retired. Palette keeps two-line entries (bold title + muted description) with zero gap between entries.
5. **Sidebar separation**: zebra striping (two near-identical ground tints), not hairlines or blank lines.
6. **Tabs**: flat text labels + thin accent underline under the active tab, replacing block-fill tabs. The underline **animates** (slides) on switch.
7. **Motion**: full catalog (§6), not just the tab slide — hover lerps, overlay settles, selection travel, toasts. One easing family, ≤150ms, one config kill-switch.
8. **Buttons stay 3 cell rows tall** (user-confirmed) — the Generate-button proportion; inputs likewise. Only in-table/in-row elements are 1-row flat.
9. **Full sweep in one stage**: every surface converts; no mixed-style interim on main.

## 3. Visual language — primitives (`src/paint/`)

### 3.1 Thin-bevel control (replaces half-cap)

A 3-cell-row control: top row carries `▔` (fg = light edge, bg = fill), middle row carries the label/text on the fill, bottom row carries `▁` (fg = dark shadow, bg = fill). This is the existing var-picker-input anatomy, promoted to *the* control anatomy.

State behavior (unchanged vocabulary, `ControlState`):
- **Hover**: whole control lifts — fill steps up, edges recomputed from the new fill (`face_edges`). Animated (§6.7).
- **Pressed**: edge swap (dark top / light bottom), fill steps down.
- **Focused** (small controls): fill lifts +0.12, edges follow. Animated fade-in.
- **Disabled**: flat fill, no edges, muted label.

Applies to: buttons (accent primary, neutral secondary), address-bar URL well, method badge, search/text inputs in popups and modals. `bevel_top`/`bevel_bottom` already exist; `half_cap_top`/`half_cap_bottom` are deleted at the end of the sweep.

### 3.2 Focus/open ring (resurrected, big containers only)

Cell-tight stroke drawn in a container's margin cells: `▁ ▔ ▏ ▕` edges + Legacy Computing corners `🭽 🭾 🭼 🭿` (U+1FB7C–7F; Ghostty renders these algorithmically). Two variants:
- **Accent ring**: body editor when Content has focus; open dropdown overlay.
- **Quiet ring** (hairline tint, non-accent): modal/palette/floating-panel outlines.

Constraints on record: text cells can only take attribute edges; a cap row cannot carry both shading and a stroke — treatments swap, never stack.

### 3.3 Dense list row (replaces `PillRow`)

Single-line pitch, zero spacing lines.
- **Selected**: full-width accent-tinted fill (existing `selection` token family) + 1-col accent bar at the left edge.
- **Hover**: subtle tint (`control_hover`-class delta).
- **Zebra variant** (sidebar only): rows alternate two near-identical ground tints; the zebra restarts per visual block so folder groups read as units. Popup lists (dropdown, choosers, palette, var picker) are plain-dense — no zebra.
- Palette entries span two lines (title + muted description); the selection fill spans both.

### 3.4 Flat tab strip (replaces block tabs)

Muted inactive labels; active = bright + bold. A dedicated underline row beneath the labels: faint full-width hairline rule with the accent segment on top of it under the active tab. Badges/counts stay inline in labels ("Params · 2", Body ✓/✗). Right-aligned chips (save/vars/discard, ⌄ collapse) stay on the label row, restyled flat. Keyboard-focused strip recolors the underline segment with `focus_ring` as today's caps do.

### 3.5 Tokens

The Oklab seeds→generator→tokens engine is untouched. Additions: `zebra_alt` (ground ±~0.02 lightness), `hairline` (quiet stroke tint). Existing `selection`, `focus_ring`, `accent_edge_*` tokens carry over. No new seed inputs.

## 4. Surface conversion map

| Surface | Becomes |
|---|---|
| Address bar | Method badge + URL well in thin-bevel anatomy (3 rows); Send = accent thin-bevel button; focus = fill-lift. Copy-URL 1-click chip returns. |
| Editor + response tab strips | Flat labels + sliding accent underline (§3.4); chips flat on the label row. |
| Sidebar | Dense single-line rows, zebra, selection bar; folders on the same pitch as group headers; method badges become short colored text tags (no fill) — final badge look judged in the testbed round. |
| Dropdowns / context menus | Floating panel with accent ring when open; dense borderless rows; selection = full-width accent bar; anchor shows right-aligned `▼`/`▲`. |
| Command palette / choosers / var picker | Shared dense list treatment; palette two-line entries; search input = thin-bevel well; var-picker `proj`/`grp` origin tags become quiet colored text tags in an aligned column. |
| Modals | Quiet-ring panel outline; thin-bevel fields; accent/neutral thin-bevel button rows. |
| Body editor | Accent focus ring when Content focused. |
| Var manager | Master-detail lists dense; grid/radios/forms restyled; no structural change. |
| Response pane | Chrome only: tabs, scrollbars, search input, header rows dense; JSON tree body unchanged. |
| Footer / toasts | Footer hints as quiet key chips in the flat language; toasts = panel + motion (§6.6). |

Carry-along fixes folded in: palette keybinding column via reverse keymap lookup; disabled-row strikethrough softened if the user still objects on sight.

## 5. Testbed screen

`Screen::Testbed`, gated behind `POSTUI_TESTBED=1`: every primitive × state (normal/hover/pressed/focused/disabled), zebra + dense lists, tabs with a freeze-frame mid-animation. It is the screenshot suite's target for per-primitive visual captures and **ships hidden** (not torn down) as the permanent looking-glass for future taste rounds.

## 6. Motion catalog

Infrastructure in §7.1. One easing family (ease-out cubic), durations ≤150ms except the Send breathe, `animations = false` in config zeroes every duration.

**Navigation & structure**
1. Tab underline slide, ~140ms — the signature move. Sub-cell precision via the left-eighth-block family; right edge = same glyphs fg/bg-swapped.
2. List selection travel, ~100ms — sidebar/popup selection bar slides to the new row on keyboard nav (vertical analogue of the underline).
3. Pane collapse/expand (⌄ hide, sidebar toggle) height animation, ~120ms.

**Overlays**
4. Modal/palette open: backdrop dim fades in ~100ms (color lerp) while the panel settles ~80%→100% height around its center. **Close is instant.**
5. Dropdown open: expands downward from the anchor, ~90ms. Close instant.
6. Toasts: slide in from the edge; fade out by color lerp toward ground (replaces the stepped fade).

**Micro-interactions**
7. Hover fills lerp to/from hover color, ~70ms — the full-time "modern feel" carrier.
8. Focus transitions (fill-lift and accent ring) fade in, ~90ms.
9. Send in-flight: button fill breathes (slow accent↔accent-dim lerp) while a request runs.

**Excluded by design** (restraint): scroll smoothing (wheel latency stays zero), typing/caret effects, zebra/text animation, ambient effects.

## 7. Technical architecture

### 7.1 Animation core (`anim.rs`)

`Anim` value: `start_value`, `target`, `started_at`, `duration`, ease-out cubic; `value(now)`, `done(now)`, retargeting preserves current value as the new start. The app keeps a registry of live animations; while any is live the event loop schedules redraws at ~30fps (generalizing the existing toast tick). Time is injected (`Instant` parameter) for deterministic tests. Color animation lerps in Oklab using the existing theme math.

Because hover/focus lerp, the registry is first-class in the draw path: painted fills read `value(now)` rather than a static state color. Controls key their animations by a stable identity (hit id / control id) so a redraw retargets rather than restarts.

### 7.2 Sub-cell rendering helpers

`frac_hline(buf, y, x0: f32, x1: f32, fg, bg)` — fractional horizontal segment via `▏▎▍▌▋▊▉█`, right edge via fg/bg swap; property-tested for coverage and no gaps. A vertical sibling `frac_vline` (top/bottom-eighth family) serves selection travel and height settles.

### 7.3 Replace, not accumulate

`half_cap_top`/`half_cap_bottom` and `PillRow` are deleted at the end of the sweep; no legacy chrome survives (stage-5 rule). `ControlState` stays the shared vocabulary. `HitMap` dispatch is untouched; per-surface hit geometry (denser list row→index math) updates inside each conversion task.

### 7.4 Config

`animations = true|false` (default true) in the existing ui/config TOML. False ⇒ all durations zero; no separate code path.

## 8. Testing & verification

- Behavior tests keep passing per task — the restyle must not change semantics.
- Paint-level unit tests per primitive (cell asserts, as today); property test for `frac_hline`/`frac_vline`; animation math tests with injected clocks.
- Per-UI-task tmux screenshot verification (pty+pyte fallback when tmux is sandbox-blocked), ansi2png pipeline for real images. Standing user requirement.
- **User checkpoint 1**: after the primitives + testbed tasks — the user judges the language in Ghostty before any surface converts.
- **User checkpoint 2**: final whole-app sweep against the reference screenshots.
- Acceptance: stage acceptance test updated for new glyph assertions; all suites green; `cargo clippy -D warnings` clean; `cargo fmt` clean.

## 9. Out of scope

- Kitty-graphics icons / sextant button spike (deferred; re-evaluate after this stage).
- Any behavior/feature work (competition-research backlog stays parked).
- Light-theme-specific redesign beyond what the token generator produces automatically.
- GUI frontend track.
