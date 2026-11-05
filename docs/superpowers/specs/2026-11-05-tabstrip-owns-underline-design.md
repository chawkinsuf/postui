# The tab strip owns its underline (#119)

## Problem

The three tab strips (editor, Response, Manage) each slide an accent
underline under the active tab. On main its target is computed in
`update`, away from the draw that lays the tabs out:

- the editor strip by `App::sync_editor_tab_underline` in the tail of every
  dispatch (#117), from `Editor::tab_strip_spans`;
- the Manage strip by `retarget_manage_tab_underline(prev)` from
  `SelectManageTab` and `OpenManage`, from `ManageTab::strip_spans` at a
  width `ui::draw` records on `App::manage_strip_width`, plus a clear when
  the screen opens;
- the Response strip by `retarget_response_tab_underline(prev_mode)` from
  `ResponseViewMode`, plus `reset_response_tab_underline` at three sites
  (a response arriving, a not-JSON parse, a request swap).

The first attempt (branch `tab-strip-underline`, comments 1035-1038) put all
three on one per-dispatch pass. Its review block failed: every round's
significant finding was a span that moved without a switch (the jq badge,
a resize moving the right-anchored Settings tab, the right-aligned Response
strip), because each fix taught the update-time pass a bit more of the
draw's layout. Only the paint knows the real area and spans.

## Rules

| # | Rule | Who |
|---|---|---|
| R1 | The underline slides when it moves to another tab; the user does not see it pop. | you: "i like the underline sliding when it moves. i dont want to user to see it just popping to a new place" |
| R2 | One reusable tabs component for all three strips (editor, Response, Manage), the editor included. | you: "we could even have a single tabs compoent that is reused", and "yes" to the editor on the same rule |
| R3 | When the active tab itself moves or resizes (a jq badge, a count, a window resize moving the right-anchored Settings tab), the underline stays attached to its label, in the same frame. | you: "yes, keep it attached to the label" (on my proposal) |
| R4 | A regression the branch causes is fixed on the branch: the first attempt's jq-typing back-and-forth must not happen. | you: "if the branch changes the behvior to create a bug, it should probably be fixed on the branch" |
| R5 | At paint: the active tab differs from the strip's last paint, so glide from the underline's current drawn position, clamped into the strip, to the new tab's columns. | mine (for veto) |
| R6 | At paint: same tab, new columns, so each moved edge goes on from where it is drawn to its new column over what is left of its glide: at once when it has landed (R3), and with no jump when a glide toward this tab is under way. | mine (for veto) |
| R7 | At paint: the strip was not painted last frame (Manage just opened, a pane expanded, the first frame), so place it with no glide. | mine (for veto) |
| R8 | `animations = false` places every move: a glide has zero duration, and turning animations off lands one in flight. | mine (the app's existing rule for every animation) |

Consequences of R5-R7 worth naming:

- A request switch that brings in a response on another tab glides there
  (main snapped it, through `reset_response_tab_underline`). A new response
  that drops the Tree tab glides to the tab it lands on (R1, R5). The strip
  is right-aligned, so dropping Tree narrows it from the left: where Tree's
  underline was drawn is now outside the strip, so the glide starts there
  clamped onto the strip's left edge, with no width, and grows to Raw. Its
  first frames (the in-out curve's slow start) are under half a cell wide,
  so they show nothing.
- An editor count or badge change, and a request switch whose tabs have
  other counts, follow at once (#117 glided them). R3, R6.
- A tab a single dispatch passes through (#117's project switch through an
  empty GET) is never painted, so it starts no glide. #117's per-dispatch
  pass existed for this; the paint gets it for free.

## Design

`paint::TabStrip` takes an `Underline`:

- `Underline::Aimed { anims, strip, tab, now }` for the three strips. `tab`
  is the active tab's identity (the editor's draw position, the Manage
  tab's index, the response `ViewMode`), which is not always its index: the
  Response strip's Tree tab comes and goes.
- `Underline::At(left, width)`, a fixed strip-relative segment, for the
  testbed's demos and the paint tests.

`TabStrip::paint` computes the active tab's columns from the area it paints
into and its own spans (`spans_in`, so the right-anchored tail counts), in
absolute screen columns, and calls `Anims::aim_underline(strip, tab,
columns, within, now)`, which applies R5-R7 and answers where to draw the
edges. Absolute columns keep a still tab still: the Response strip is
right-aligned, so a badge on Tree leaves Raw's columns where they were.

Amended 2026-11-05 (#119 review round 1, mine): every motion starts from
one place, the segment as drawn at that moment, clamped into the strip
(`within`). The first build moved an in-flight glide's start by its
target's move (R6), which jumped the segment mid-glide, and started a
glide outside a strip that had moved or narrowed. The start is not
converted when the strip's origin moves: it is where the user last saw
the segment, and a strip-relative frame would not suit the Response strip,
whose origin moves when Tree comes and goes while its other labels stay.

Each strip's underline state lives in `Anims`, keyed by `StripId`: the tab
it is aimed at, its two edges (the same eased value an `AnimKey` holds, in
in-out cubic), and the frame that last painted it. `Anims` is where it
belongs:

- `Anims` is already in every `DrawCtx`, and it lives on `App`, so the
  state survives the swaps of `Session::response` (per request) and of the
  editor (a scratch landing), which a component field would not.
- `App::animating()` reads `Anims::active`, so a glide started at paint
  keeps the main loop ticking at 60 fps until it lands, and the tick's
  `was_animating` catches its last frame, with no new wiring. Only a strip
  the last frame painted counts: an unpainted one is placed when it next
  paints.
- `Anims::set_enabled(false)` and `finish_all` already settle everything;
  they settle the underlines too (R8).

The paint has only `&Anims`, so the underline map sits in a `RefCell`
(the component tests' shared `Anims`, `Anims::for_tests`, is one per test
thread). `ui::draw` calls `Anims::begin_frame(tab_slide)`
first in every frame, with `&mut App`: it counts the frame (R7's "last
frame") and records the glide's duration from
`ui_settings.anim_ms.tab_slide`.

### Removed

- `App::sync_editor_tab_underline` (#117) and its call in `update`'s tail.
- `App::retarget_manage_tab_underline`, `App::retarget_response_tab_underline`,
  `App::reset_response_tab_underline`, their callers, and the clear in
  `OpenManage`.
- `App::manage_strip_width` and its record in `ui::draw`;
  `ManageTab::strip_spans`; `Editor::tab_strip_spans`. Each existed only to
  aim the underline away from the paint.
- `AnimKey::TabUnderline` and `AnimKey::TabUnderlineWidth`. `StripId` stays,
  as the underline map's key. `Anims::retarget_with`, whose one in-out
  caller was the underline, folds into `retarget`.
- The per-strip static fallbacks in `editor.rs`, `response.rs` and
  `manage.rs`.

## Tests

Each is driven through a draw (`ui::draw` on a test terminal):

- A tab switch glides, on each strip (R1, R5).
- With Tree active, a jq badge toggle keeps the underline on Tree's
  columns in the same frame, and repeated toggles never glide (R3, R4, R6).
- With Raw active, a badge toggle leaves Raw's underline still (R6).
- On Manage Settings, a resize keeps the underline under Settings in the
  same frame (R3, R6).
- Opening Manage places the underline with no glide (R7).
- A new response that drops the Tree tab glides to the tab it lands on (R5).
- A header count change on the editor strip follows at once (R3, R6).
- `animations = false` places a switch with no glide (R8).

Also: typing a jq filter with Tree active keeps the underline on Tree
(R4); showing the hidden Response pane places it (R7); a method change off
Body glides, a POST-to-POST project switch starts no glide, and a request
switch with other counts follows at once (#117's cases); a request switch
to a response on another tab glides (R5). `anim.rs` pins R6's in-flight case
and R7 directly. The tests live in `app/tab_underline_tests.rs`.

Tests that checked the anim right after `update` now draw first.

## Mine, for veto

- R5-R7, and that the state lives in `Anims`, in a `RefCell`.
- R6's in-flight case goes on from the drawn segment over the glide's
  remaining time, restarting the in-out curve there (a brief ease, never a
  jump).
- A glide's start clamped into the strip, so the tab-drop glide grows
  from the strip's left edge (above).
