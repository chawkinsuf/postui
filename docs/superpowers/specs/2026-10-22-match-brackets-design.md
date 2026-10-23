# Matching brackets light up (#24)

## 1. Problem and findings

The request asks for the IDE feature: when the caret is on or next to a
brace or bracket, it and its partner light up, so you can see where the
block closes (task #24; the user, in chat: "when the cursor is on or next
to a brace or a braket, the matching one highlights so you can see where it
goes"). The response panel has no caret (the user's 2026-10-10 ruling), so
the user proposed that it take clicks instead: "if you click on one side
they both highlight".

What exists:

- **The matcher.** `text/brackets.rs::find_match` is Vim's `findmatchlimit()`
  for `()` `[]` `{}`, shared by `%`, the block text objects and a double
  click on a bracket. A bracket inside a double-quoted string is skipped,
  unless the search starts inside one.
- **The body's paint.** `Editor::body_overlays` gives edtui a list of
  highlights (Visual, search landing, search matches) that paint over the
  JSON colours. The first one that contains a cell wins. The draw splices
  them in only when the list changes.
- **The response's column.** A click puts `ReadyView::at_col` on the clicked
  char in both profiles (`click_row`). In the vim profile, `cur_col()` is
  Vim's cursor column: `at_col`, or `want`'s column after a move to another
  line. A move to another line, a new tree and a view switch all forget
  `at_col` (`cursor_moved`).

Vim 9.1.0016 probes (the `matchparen` plugin, an 80×30 window, with and
without `syntax on` / `ft=json`, file
`{"a": [1, 2], "b": "x{y", "c": ")"}` / `][` / `{` / 57 filler lines / `}` /
`(x)(y)`):

| Probe | Cursor | Vim lights |
|---|---|---|
| N1 | Normal, on `{` col 1 | json: `{`1 + `}`35. No syntax: nothing (the string's `{` counts) |
| N2 | Normal, on `,` just after `]` | nothing (Normal looks only under the cursor) |
| N3 | Normal, on the `{` inside `"x{y"` | `{`22 + `}`35 (inside a string, nothing is skipped) |
| N3b | Normal, on the `)` inside `")"` | nothing (no `(` before it) |
| N4 | Normal, on `{` line 3, partner on line 61, window 1–29 | nothing |
| N5 | Normal, on `}` line 61, partner on line 3, window 34–62 | nothing |
| I1 | Insert, caret on `,` just after `]` | `]`12 + `[`7 (the char before the caret) |
| I2 | Insert, caret on `(` just after `)` in `(x)(y)` | `(`4 + `)`6 (the char under it wins) |
| I3 | Insert, caret past the end of `(x)(y)` | `)`6 + `(`4 |

`find_match` is `%`, not matchparen, and the two differ on N3. `%` from the
`{` inside `"x{y"` does not move, because its quote parity starts outside
the string and then reads the closing `}` as quoted. N3's light comes from
matchparen's skip, which asks the syntax whether a position is in a string.
Two consequences:

- The highlight needs its own matcher: a `searchpairpos()` walk with that
  skip, not `find_match`.
- JSON strings never span lines, so "is this position in a string" is exact
  from the line alone: walk it from column 0, toggling on each `"` that
  isn't escaped by a backslash.

## 2. Rules

| # | Rule | From |
|---|---|---|
| R1 | Body: when the caret is on a bracket that has a partner, both light. | you (the feature); both-light is mine (approved) |
| R2 | Body, which char: the one under the caret. When it isn't a bracket and the caret is between chars (arrows always; vim Insert and Replace), the one before it. A vim Normal or Visual caret looks only under itself. | Vim (N2, I1–I3); the arrows half is mine (approved) |
| R3 | The pairs are `()` `[]` `{}`. | Vim (`matchpairs` default) |
| R4 | A bracket inside a string is skipped, unless the caret's own bracket is inside one; then nothing is skipped. A string is a run between unescaped `"` on one line. This can differ from where `%` goes (N3). | Vim with JSON syntax (N1, N3, N3b); applying it to non-JSON text (Raw, Headers) is mine (approved) |
| R5 | When the partner isn't on a row in view, nothing lights. Rows only: a partner scrolled out sideways still counts. | Vim (N4, N5); the arrows half is mine (approved) |
| R6 | When there's no partner, nothing lights. | Vim |
| R7 | The body lights only while it has focus. | mine (approved) |
| R8 | Style: `on_accent` text on an `accent` background. Visual, the search landing and search matches paint over it. The cell under the body's caret keeps the caret's look (edtui reverses it), as Vim's cursor covers its own bracket, so only the partner shows the plain style there. | mine (approved) |
| R9 | Response: a click on a bracket lights it and its partner. | you |
| R10 | Response, arrows: the pair lights only from a click. The next click moves it or clears it. A new response and a view switch clear it, and so does a new tree (jq, a parse landing) for a click made on the Tree view; a Raw or Headers click outlives a tree swap, since its text didn't change. Scrolling keeps it, and since #89 so does a key that moves the cursor (amended 2026-10-22, §6). A key never lights a pair of its own: arrows' PageDown puts the column on the first non-blank, often a `}`, and that must not light. | mine (approved) |
| R11 | Response, vim: Vim's cursor column (`cur_col`) drives it, so `%`, `l` or `j` onto a bracket lights the pair, as Vim does. | Vim |
| R12 | Response: works in Pretty, Raw and Headers. | mine (approved) |
| R13 | Response: a closed fold's line never lights (it shows a summary, not its text). | mine (approved) |
| R14 | Response: lights only while the pane has focus. | mine (approved) |
| R15 | `{{var}}` braces match like any other brace. | mine (approved) |
| R16 | No gutter line spanning the block this round. | mine (approved) |
| R17 | The vim response has no Insert mode, so R2's char-before fallback never applies there. | Vim |
| R18 | A click in Raw or Headers puts the response's cursor on the clicked char, as a click in Pretty already does: the cursor row moves there, and so does Vim's column. Before this, those clicks only anchored a selection. Amended 2026-10-22, see below. | mine (approved) |

**Amendment, 2026-10-22 (during implementation).** §3 assumed every view's
click goes through `click_row`. Only Pretty's does: Raw and Headers register
no row hit, so their clicks reach `Hit::Pane(Response)`, which only anchors
a selection (b6e07be, added for selection alone; no spec rules on the
cursor there). The user picked routing those clicks through `click_row`
like Pretty's (R18) over two alternatives:

- lighting from the clicked cell without moving the cursor, which leaves the
  vim profile unlit;
- Pretty only.

Cost, named when asked: the cursor row now follows a click in Raw and
Headers. In the vim profile, searches, `V` and yanks then start from the
clicked line, as in Pretty and in Vim.

Found in review round 2 (mine (approved), kept by the user at handoff): a click on
the pane's empty space below the text gets the same treatment. The
selection anchor already clamped such a click onto the last line's last
char (`cell_at`), so the cursor now goes there too, as an editor's caret
does. In all three views this lights the last line's bracket, often the
root `}`.

## 3. Design

**`text/brackets.rs`** gains one entry point for both panels:

```rust
/// The bracket a caret at `col` stands by: the char at `col`, or with
/// `before` the one before it (R2).
pub(crate) fn bracket_at(line: &[char], col: usize, before: bool) -> Option<usize>
/// The partner of the bracket at `at`, on rows `rows`: Vim's matchparen
/// with JSON syntax (R3–R6).
pub(crate) fn partner_of<B: Lines + ?Sized>(
    buf: &B, at: Pos, rows: RangeInclusive<usize>,
) -> Option<Pos>
```

(The #89 cleanup split these apart; they began as one `matching_pair`
with a `before` flag. The body, the response and R20's comma rule each
pick their bracket with `bracket_at`; R20 is `before` switched on for a
clicked `,`.)

It is its own walk, not `find_match`. It counts the bracket and its partner
forward or backward, and with R4 it skips positions inside a string, worked
out per line by the quote walk in §1. It never leaves `rows`, so an
unmatched bracket in a big body walks only the lines in view. `find_match`
and `%` don't change.

**Body (`components/editor.rs`).**

- `paint_bracket_pair` paints the two cells onto the frame after edtui
  renders, while the body is focused, the way the `{{var}}` tokens are
  repainted.
  - Rows in view are edtui's own viewport after its scroll, walked in wrap
    rows down to the bottom, or to the vim search prompt's row when it's
    open.
  - A cell the Visual span, a search hit or the selection already paints
    keeps that paint.
  - The caret's cell keeps its reverse, because only the colours are
    patched.
- R2's `before` is `self.body.mode == EditorMode::Insert`. The engine
  already paints Insert and Replace as edtui's `Insert`, and Normal and
  Visual as `Normal` and `Visual` (`BodyBuf::show`). The arrows body is
  always `Insert`. No seam change.
- Review round 2 replaced the first design. That design added
  `OverlayKind::Bracket` to edtui's highlights before render, which meant
  predicting edtui's scroll. Two rounds found frames where the prediction
  and the real scroll differ: edtui's private `num_rows`, and a first
  render that doesn't scroll. Painting after the render reads the real
  viewport instead.

**Response (`components/response.rs`).**

- `ReadyView` answers `bracket_pair() -> Option<(Pos, Pos)>` in visible
  (line, char) cells, matched over the lines as painted (its `Lines`
  impl). A closed fold's summary (`{…}`) is balanced, so walking past one
  never miscounts. The rows are the visible lines the viewport shows. It
  answers `None` on a closed fold's line. The column depends on the
  profile:
  - vim: `cur_col()`;
  - arrows: a new `clicked: Option<(line, col)>`. `click_row` sets it
    beside `at_col`, and `cursor_moved` forgets it beside `at_col`.
  
  `at_col` itself can't serve arrows: arrows' PageUp and PageDown set it
  (first non-blank), and arrows' search starts from it, so clearing it
  there would move searches.
- The app hands the response the profile before each draw, as it hands
  the editor `env_tls`.
- The draw paints the two cells with R8's style. A cell under a search hit
  or the selection keeps that paint instead.
- R18: `Hit::Pane(Response)`'s single click calls `click_row` on the
  clicked line after anchoring, as `Hit::JsonRow` does.
- The pair is worked out in the draw, but only when something it reads has
  changed since the last frame (`BracketInputs`, review round 1). Before
  this, a lit bracket on a 3 MB minified Raw line cost about 10 ms per
  frame, measured in release. In the vim profile that's the default state:
  the cursor starts on the first line's `{`. The memo's key is everything
  the matcher reads:
  - the profile and the mode;
  - the cursor, its fold line and its column's inputs (`at_col`, `want`,
    V, the clicked char);
  - the scroll and the height;
  - the tree: `tree_epoch` for a swap, and `tree_edits` for a fold.
    `tree_edits` counts `active_tree_mut`, the only mutable way to the tree
    on screen.

  Raw's and Headers' lines never change for a view's life. A repeat frame
  costs about 0.4 µs.

**Removed:** nothing.

## 4. Tests

- `brackets.rs`: `bracket_at` with `partner_of` covers each row of the probe table (N1,
  N2, N3, N3b, N4/N5 via `rows`, I1–I3). The row limit stops an unmatched
  walk. An escaped quote (`"a\"{"`) doesn't end a string. The existing `%`
  tests pass unchanged.
- `editor.rs` (render tests, like `the_vim_visual_span_draws_over_the_json_colours`):
  - arrows caret on `{` lights both, and a caret just after `}` lights both (R1, R2);
  - an unfocused body lights nothing (R7);
  - a partner outside the view lights nothing (R5);
  - Visual and a search match paint over the bracket style (R8);
  - vim Normal just after `]` lights nothing, and vim Insert there lights the pair (R2).
- `response.rs`:
  - a click on `[` in Pretty lights both cells, and a click on a non-bracket clears them (R9, R10);
  - ↓ in arrows clears, PageDown onto a `}` line lights nothing, and scrolling keeps them (R10);
  - in vim, `j` onto a bracket column lights the pair (R11);
  - Raw and Headers light, and a closed fold line doesn't (R12, R13);
  - an app-level click in Raw moves the cursor row to the clicked line (R18);
  - an unfocused pane lights nothing (R14).
- The real TUI in tmux: arrows body, vim body Normal and Insert, and a
  response click in both profiles.

## 5. `mine` choices to veto

All approved by the user on 2026-10-22 (an option they picked, so
`mine (approved)` in the table above).

- R2 (arrows): the caret's right-hand char first, then the left-hand one,
  as Vim's Insert mode does.
- R5 (arrows): no light when the partner is off screen. An IDE would light
  the near one. Following Vim keeps one rule and bounds the cost.
- R7, R14: only the focused pane lights.
- R4: the JSON string rule also applies to Raw and Headers text, which
  may not be JSON.
- R8: accent background with `on_accent` text. The caret's own cell keeps
  the caret look.
- R10: arrows' response lights only from a click, never from a key.
- R13: no light on a closed fold's line.
- R15: `{{var}}` braces are ordinary braces.

## 6. Amendment, 2026-10-22: the response's reach and a trailing comma (#89)

The user, after #24 merged:

> does it not match in the response if the other one is not on the screen?
> it would be nice to be able to scroll down to see it. also, something like
> `{` i can click at the end of the line to highlight, but the end is `},`
> so that doesnt work. it probably makes sense to trigger it when its just a
> comma and then a brace/bracket

Scope: the response in the arrows profile only. The user picked that for
both changes, over also changing the arrows body (`mine (approved)`). The
vim profile keeps Vim's rules (R5, R11): Vim's matchparen lights nothing
when the partner is off screen, and looks only under the cursor.

| # | Rule | From |
|---|---|---|
| R19 | Arrows response, Tree view: a clicked container bracket lights with its partner wherever the partner is, on screen or not. Scrolling brings the lit partner into view. This replaces R5 there. Raw, Headers, a bracket inside a string, the vim response and the body keep R5 (a one-line Raw body's partner is on the clicked row, so it lights). | you (the ask); arrows-response-only is mine (approved); Tree-view-only is mine (approved, review round 2) |
| R20 | Arrows response: a click on a `,` that directly follows a bracket and ends its line (blanks may follow it), which includes a click past the end of a `},` or `],` line since that lands on the `,`, lights that bracket's pair. A mid-line `,` takes nothing. | you (the ask); response-only, comma-only and line-end-only are mine (approved) |

**Design.**

- R19: in arrows on the Tree view, `JsonTree::bracket_partner(line,
  col)` names the partner's (line, char) when the clicked char is the
  container's own bracket. That's a lookup, not a walk. Everything else
  goes through the same rows-in-view walk as Vim (R4, R5).
- R20: in arrows, a clicked `,` with a bracket right before it starts
  from that bracket instead.

**Rework (review round 2).** The first design walked every line in arrows
for every view. Review round 1 found two problems with that: it froze on
a big tree, and it paired a string's bracket with a stray one far away.
The round-1 fix added a "pairs only within its string" rule and a tree
shortcut. Round 2 then found that the string rule misreads non-JSON Raw
text, that it splits vim from arrows, and that the whole-body walk can
still freeze on an unmatched bracket in Raw. The user picked narrowing
R19 to the Tree view (`mine (approved)`). That removes the unbounded walk
and the string rule together. The review count restarts at round 1.

**Cost.** No walk: the Tree view's lookup reads the two container lines,
counting the opening line's chars (its key's length), and everything else
walks only the rows in view, as before #89.

**Tests.**

- Arrows, Tree view: a click on `[` with its `]` 41 lines down lights the
  `[` now and the `]` once scrolled to, and back up from the `]`.
- `JsonTree::bracket_partner`: both directions; a key's quote, a comma
  and a scalar line name nothing.
- Arrows, Raw: a partner below the view lights nothing; a one-line body
  lights its far-right partner.
- Arrows, Tree view: a bracket inside a string pairs by the text rules.
- Vim: a partner out of view lights nothing, and a scroll that takes it
  out goes dark.
- Arrows: a click past the end of a `},` line lights the `}` and its `{`;
  a click on a `,` after a non-bracket lights nothing.

**`mine` choices to veto.**

- Only `,` counts, and only directly after the bracket: not `;`, not
  whitespace, and not `} ,`.
- In vim, the comma rule doesn't apply: Vim looks only under the cursor.

**At handoff, 2026-10-22.** Two points came back across review rounds,
and the user picked the recommended option for each (`mine (approved)`):

- **Keys keep the clicked pair lit.** A key that moved the cursor cleared
  the pair (R10), so only the wheel or the scrollbar scrolled to a lit
  partner; ↓ or PageDown turned it off.
  - Now `clicked` holds the real line it was made on, and the pair is
    anchored there, not at the cursor.
  - Every click forgets it, a fold arrow's included (`click_row`), and so
    does a view switch (`set_mode`); `cursor_moved` no longer does.
  - A click carries the mode and the tree it was made on (`Clicked`), and
    lights only while both still match. So a tree swap retires a Tree view
    click and leaves a Raw one, with no clear at each tree writer (review
    round 5).
  - The bracket memo keys arrows on the click alone, so a key that keeps
    the pair never walks again.
  - Only an arrows click on a line's own text records one. A click in vim
    doesn't, so a later switch to arrows doesn't light a stale pair. Nor
    does a click on a closed fold's summary, so opening that fold doesn't
    light its column 0 (review round 4).
  - A fold that hides the clicked line darkens the pair (R13).
  - Cost: the highlight no longer follows the cursor row.
- **R20 is a line-end comma.** Only a `,` that ends its line (blanks may
  follow) takes the bracket before it. Before, any `,` right after a
  bracket did, so a click on the comma in Raw's `f(a),g(b)` lit `(a)`.
