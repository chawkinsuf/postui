# Stage 5: Painted UI — design

Date: 2026-08-16
Status: approved direction, pending mock sign-off

## Problem

Stage 4 made postui mouse-driven, but visually it is still a text app:
line-drawn borders on an unpainted background, buttons rendered as
`[ + New request ]` bracket text, method/tab/field affordances carried by
characters instead of surfaces. The bar, set by the user's caption-reviewer
app (Textual), is a UI that reads as a desktop application: every cell
painted, controls rendered as filled surfaces with padding, bevels, and
state tints. The raster-image side of Textual is explicitly out of scope;
the painted-control side is the whole point.

Cohesion is a hard requirement: every control — input, dropdown, button,
tab, list row — is rendered in one visual language. No mixed idioms, no
"a few painted buttons pasted onto text."

## Approach

A custom widget-paint layer inside postui (new `crates/postui/src/paint/`
module). Pure functions render each control kind onto the ratatui `Buffer`,
taking `(area, state, tokens)`. Existing components keep their logic,
layout, and hit-testing (`hit.rs`) and delegate all drawing to the paint
layer. Components may not draw controls any other way; that constraint is
what enforces cohesion.

Rejected alternatives: existing ratatui widget libraries (none implement
painted Textual-style controls; all inherit the line-border idiom) and a
framework switch (nothing in Rust is Textual-equivalent; a rewrite buys
nothing the paint layer does not).

## Theme engine

`theme.rs` is rebuilt around **seeds → generator → tokens**.

- **Seeds (6):** `bg`, `fg`, `accent`, `success`, `warning`, `error`.
- **Generator:** derives ~20 tokens by lightness manipulation in a
  perceptual color space (OKLab or OKLCh):
  - surface ladder: `page`, `panel`, `control`, `control_hover`,
    `control_pressed`
  - bevel pair: `edge_light`, `edge_dark` (computed relative to the
    surface they sit on)
  - `focus_ring`, `text`, `text_muted`, `text_disabled`, `on_accent`
  - method colors mapped from semantic seeds (GET=success, POST=accent,
    PUT/PATCH=warning, DELETE=error, HEAD/OPTIONS=muted), as today
  - status-class colors for response chips (2xx=success, 3xx=accent,
    4xx/5xx=error)
  - light seeds produce a descending ladder automatically; the light
    theme stops being hand-maintained.
- **Terminal fit:** at startup, query the terminal for its real colors
  (OSC 11 background, OSC 10 foreground, OSC 4 for the ANSI accent
  slots; short timeout; must work under tmux). On success, seed the
  generator from the queried scheme, clamped to minimum contrast. On
  no answer, fall back to built-in neutral-dark or light seeds, picked
  by a background-luminance probe when available.
- **Config:** `theme = "terminal" | "dark" | "light"` plus optional
  per-seed overrides. Default is `terminal` with dark fallback.
- The existing 256-color downgrade path operates on generated tokens,
  unchanged in role.
- OSC querying sits behind a trait so tests inject fake responses.

## Control anatomy

One grammar for every control: **filled surface + padding + bevel +
state tints.** No brackets, no `< >`, no line-borders-as-widget-edges.

- **Button** — 3 rows: `▔` row in `edge_light`, padded bold centered
  label, `▁` row in `edge_dark`. Primary = accent fill + `on_accent`
  text; secondary = `control` fill. Hover lifts the fill one ladder
  step; pressed inverts the bevel and drops one step; focus adds the
  ring.
- **TextField** — 3 rows minimum: `control`-shade surface, 1-col inner
  padding, placeholder in `text_muted`. Focus = accent ring drawn in
  the surrounding panel's cells. Multi-line editors (body, response)
  are the same surface, taller. Applies to standalone fields only;
  table cells edit in place (see Request pane).
- **Select** (method, project, env) — TextField surface with
  right-aligned `▼`; the open state is the existing chooser restyled
  as a floating panel.
- **Tabs** — 1-row labels with breathing room on the panel surface;
  active tab gets accent text plus a `▁` accent underline row beneath
  the strip.
- **Chips** — method badge, status code, timing/size: small filled
  chip, low-intensity fill of the semantic color behind full-intensity
  text of the same color.
- **List rows** (sidebar tree, palette/chooser results) — full-width
  fills: hover = `control` across the row; selection = `control_hover`
  (a NEUTRAL raised fill — the accent appears only in the 1-col accent
  bar on the left edge and bold text, never as a row tint); never
  inverted text. Rows sit on a 2-line pitch, and highlight fills
  extend half a row into the adjacent spacing lines via half-blocks
  (`▄` above, `▀` below — glyph color carries one pill, cell
  background the other), producing a vertically padded pill with the
  text centered. Two adjacent highlighted rows share a spacing line
  cleanly: `▀` with fg = upper pill fill, bg = lower pill fill. In the
  params/headers table, rows are compact 1-liners and only the ACTIVE
  row expands into this pill (see Request pane); highlighted rows
  whose neighbors carry text simply keep the 1-line fill.
- **Panels** — surfaces separated by shade, not lines: sidebar on
  `panel`, editors on `page`, a 1-col painted gutter between panes
  instead of `│`. Panel titles are muted uppercase labels sitting on
  the surface.
- **Footer** — painted toolbar on `panel`: each binding a chip (key in
  accent, label in muted) with real spacing.
- **Toasts** — floating filled panels with an accent/success/error
  left bar.

## Screens

- **Header bar** — app bar on `panel`: `postui` wordmark, project and
  env as Select chips, right-aligned status/usage in muted text. No
  reversed-video title cell.
- **Sidebar** — `panel` edge to edge; "REQUESTS" muted uppercase
  label; primary **+ New request** button; tree as full-width rows
  (method chip + name). Disclosure `›`/`⌄` in muted.
- **Request pane** — the fused address bar (signature, below), then
  the tab strip, then params/headers as a painted table. The table is
  ONE contiguous element, not per-row fields: a section header row on
  `panel` (see collapse, below) with muted uppercase NAME/VALUE
  column labels, then a single `control` body surface with COMPACT
  1-line rows, a full-height `▏` column divider, and an edge line
  closing the block. The ACTIVE row (editing or hovered) expands to
  the padded pill: `control_hover` fill across the full row extending
  half a row up/down via half-blocks, the 1-col accent bar at the
  row's left edge, the cursor in the active cell, and the `✕` delete
  affordance at the right; leaving the row collapses it back to one
  line. Dense at rest, comfortable at the point of work; the
  expansion itself signals focus. The focus ring is NOT used inside
  tables — it is reserved for standalone fields (URL, modal inputs,
  palette search). The last table row is a ghost `+ Add param` row in
  muted text (hover lifts it), replacing a separate button.
  **Section collapse:** the collapse control lives on the TAB STRIP
  (the section header of this area): the active tab carries a count
  chip (e.g. `3` in accent-tinted fill), and a muted `⌄ hide` / `›
  show` toggle sits at the strip's right edge; clicking it (or a key
  binding) hides or restores the table body, reclaiming the space for
  the response pane. Collapsed, the tab counts keep the contents
  legible.
- **Response pane** — header strip of chips (status colored by class,
  time, size), response tabs, body on `page`. Empty state stays an
  invitation, centered muted text.
- **Modals & choosers** — floating panels one ladder step above the
  backdrop with a 1-cell darkened drop-shadow band right+below, title
  label, body, right-aligned painted OK/Cancel buttons. Command
  palette and var picker share the shell: TextField on top, list rows
  below.

## Signature element

**The address bar**: a single fused 3-row control — method chip, URL
field, Send button — sharing one surface and one bevel, browser-omnibox
crossed with Postman. The method segment is filled with the method's
color at full strength (the only full-saturation color outside Send);
the URL field is the widest segment; Send is the accent-filled right
cap. While a request is in flight, Send pulses subtly and its label
becomes spinner + "Sending"; it STAYS clickable, and clicking it
cancels the in-flight request (mouse-first: cancel must never be
keyboard-only). In-flight is a distinct state from disabled — only
disabled controls unregister from hit-testing. Everything else on screen stays quiet so
this is the one memorable object.

## Interaction states

One rule table enforced by the paint layer:

| State    | Effect                                              |
|----------|-----------------------------------------------------|
| hover    | fill +1 ladder step                                 |
| pressed  | bevel inverted, fill −1 step (mouse-down only)      |
| focus    | accent ring                                         |
| disabled | `text_disabled`, no hover, unregistered from hits   |

Keyboard focus and mouse hover compose.

## Testing

- Paint layer is pure `(area, state, tokens) → Buffer`: unit tests in
  the style of the existing component style tests (e.g.
  `hovered_row_gets_background_not_inverted_text`).
- Theme generator property tests: ladder monotonicity, minimum
  contrast after clamping, 256-color downgrade mapping, light seeds
  inverting the ladder.
- OSC query trait faked in tests; a silent terminal must fall back
  cleanly.
- Existing stage 1–4 acceptance tests keep passing (logic untouched;
  tests that assert on old visual idioms — brackets, border glyphs —
  are updated to assert the new idiom, not deleted).
- Visual verification via tmux capture + ansi2png before/after PNGs
  per screen at implementation checkpoints.

## Rollout

1. **Mocks (pre-implementation gate):** full-screen PNG mocks of the
   real postui layout in this system — main screen, a modal, the
   command palette — approved by the user before any Rust changes.
2. **Implementation** (via writing-plans): theme engine → paint layer
   with tests → component-by-component reskin, screenshot-verified at
   each checkpoint.
