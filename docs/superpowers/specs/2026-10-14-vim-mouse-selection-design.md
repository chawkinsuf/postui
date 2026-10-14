# The vim mouse on a field's text: design

Task #15, branch `vim-mouse-selection`. Status: **approved** (`you`, 2026-10-14,
with Q1 as (b), and R3, R8 and R10 kept).
It reworks the branch's first commit (`f9d7b1b`) after review round 1.

## 1. Findings

These are from the task review, probed against Vim 9.1 with `'mouse'` `a` and the
default `'mousemodel'` `extend`, by sending SGR mouse bytes through tmux and reading
`do_mouse()` at v9.1.0016.

1. **A double click in vim selected nothing.** The press made Visual. The release
   then settled as a click, which ended it.
2. **A double-click drag in a one-line field went by chars.** The press's settle
   dropped the field's word anchor.
3. **A mouse selection ended Insert.** Vim runs it inside `ctrl+o`, so Esc gives
   Insert back.
4. **Words were arrows' runs.** Vim's mouse word classes differ, and a double click
   on a bracket selects to its `%` match.
5. **There was no triple click.** Vim's selects the line.
6. **A right click opened the Copy/Paste menu.** Vim's extends Visual.

You asked for all six to be Vim's in the vim profile (`you`, 2026-10-14). Arrows does
not change.

## 2. The rules

| # | Gesture (vim, on the text of a field) | Rule | Source |
|---|---|---|---|
| R1 | press | Ends Visual and puts the caret on the char. Past the line's end that is the last char in Normal and the end in Insert. | Vim |
| R2 | release | Does nothing. | Vim |
| R3 | drag | Charwise Visual from the press, taking the char under the pointer (one char more than an arrows sweep at its moving end). | Vim |
| R4 | double press | Vim's mouse word (`get_mouse_class()`): blanks, word chars, the operator chars `-+*/%<>&|^!=` together, and each other punctuation char alone. Past ASCII it uses `class()`. A blank run that reaches the line's end takes the end. | Vim |
| R5 | double press on a non-word char | The `%` match from the pointer, if there is one (none under Insert). | Vim |
| R6 | drag after a double press | Goes by words. Leftward, the pressed word's end becomes the anchor. | Vim |
| R7 | triple press | Linewise Visual from the press. A drag after it goes by lines. | Vim |
| R8 | fourth press | A plain press. Vim's is blockwise Visual, which the engine lacks, and Vim's fifth is a plain press. | `mine` |
| R9 | right press on the field being edited | Extends Visual: the end nearer the pointer moves and the shape stays. Outside Visual it is charwise Visual from the caret. | Vim, `you` |
| R10 | right press elsewhere (a closed field, the response, a query box) | The Copy/Paste menu, as now. | `mine` |
| R11 | any selection made under Insert | Runs inside `ctrl+o`: Esc, an operator or a click gives Insert back. | Vim |
| R12 | click count, on a field's text | Counts on for a press on the same cell within 500 ms (`'mousetime'`), and starts over after four. Lists and buttons keep arrows' count in vim too. | Vim, `you` (Q1) |
| R13 | a query box (jq bar, search, pickers) | Keeps the field's own GUI selection; it has no Visual. | earlier |

The engine half (`vim/engine/mouse.rs`: `Settled::Mouse(at, Press(n) | Drag |
Extend)` and R1–R11) stays as committed, with its probe-based tests. This spec
changes how the pointer reaches it.

## 3. Why the first shape was wrong (review round 1)

The field arms in `app/mouse.rs` build arrows' GUI selection on every press and drag.
In vim the engine then throws that away and rebuilds from a pointer the arms left
behind as a side effect. That caused five findings:

- **Two decisions for a right press.** `vim_mouse_start` decided before the event, and
  the right arm decided again after closing a dropdown or ending a drag. The two can
  disagree.
- **A stale pointer.** When the left arm declined to place the caret, `vim_mouse_end`
  still read the last `mouse_at`.
- **Two click counters,** the app's (same control, 400 ms) and vim's (same cell,
  500 ms), counting each press.
- **Vim-only state in shared components.** `LineInput::mouse_at` and
  `Editor::body_mouse_at` had to be kept in sync at six call sites.
- **Hand-kept mappings.** `hit_field`, a second Hit→FieldId map, and `MouseEdge`'s
  `Option<Option<_>>`.

## 4. The design

**One place maps a field's text to a char.**
- `App::text_box(field: FieldId) -> Option<TextBox>` gives the text's left edge and
  width, from the field's hit rect and its inset: 0 for the URL and the cells, 2 for
  a dialog box, `WELL_PAD` for a well, and `jq_bar_text_geometry` for the jq bar.
- `App::text_char(field, column, open: bool) -> Option<usize>` clamps the column into
  the box and adds `window_start(open, width)`.
- Today the press arms (seven) and the drag fns (seven) each carry this math.
- `field_hit(FieldId) -> Option<Hit>` is the one map between the two. A right press
  asks `field_hit(live) == Some(hit)`; no reverse map is kept.

**One press helper and one drag helper for one-line fields.**
- Each arm keeps its field-specific open (focus, `click_table_cell`, `start_field_edit`,
  and so on) and then calls `press_text(field, column, clicks, was_open)`.
- The `drag_*_to` fns become one `drag_text(field, column)`.
- `TextDrag`'s one-line kinds become `TextDrag::Field(FieldId)`; `Body` and `Response`
  stay.
- Each helper first offers the event to the seam:
  `vim_text_mouse(field, at, how) -> bool`. That answers `false` outside vim, for a
  query box, and for a field that is not an engine field; the helper then does the
  plain GUI work (word select, or caret and anchor, or extend), as today.
- In vim it records the event for the end-of-event settle and the GUI does nothing,
  so no selection is built just to be discarded.

**The body follows the same shape.** The editor exposes `body_char(x, y, drag) ->
Option<Pos>` (its existing click and drag mappings). The body arm and the body drag
offer that to the seam first, and run `handle_mouse` / `body_select_word_at` /
`body_drag_to` only on the plain path.

**A right press is one decision, in one place.**
- The right arm asks `vim_text_mouse(field, at, Extend)` when `field_hit(live) ==
  Some(hit)` and `text_char` maps the column.
- It doesn't run the left arm and arms no sweep.
- It runs after the arm has closed a dropdown and ended any drag, so it sees what the
  menu would have seen.

**The seam settles what was recorded.** `vim_mouse_end` takes the recorded `(at,
how)` and settles `Settled::Mouse`. A left press with none recorded settles
`Settled::Click` (a press off the text), and so does nothing else. `MouseEdge` becomes
a plain "a button event" flag, and the `text_drag` gating goes.

**One click counter.** The app's `last_click` records each left press once (its hit,
cell and time) and gives two counts: arrows' (same hit, 400 ms, a pair), which every
list and button reads in both profiles, and Vim's (R12), which only vim's text takes
(Q1). `on_hit` gets both as `Clicks { gui, vim }`. `LastPress` and `vim_clicks` go.

**Removed:** `LineInput::mouse_at`, `Editor::body_mouse_at`, `hit_field`,
`MouseEdge`'s nested `Option`, `LastPress`, the right arm's left-press re-run, and the
seven `drag_*_to` fns. In the engine, `mouse()` no longer resets the caret to `rested`
(nothing moves it first), and `leave`/`enter` clear `pressed` (review finding 4).

**Kept:** `open_session`'s anchor re-plant, which a query box's drag still needs
(R13). Arrows' behaviour is unchanged; its existing mouse tests pin it.

## 5. Tests

- The engine tests in `vim/engine/tests.rs` (R1–R11) stay.
- The app tests in `field_tests.rs` cover each gesture through `handle_mouse` in the
  URL line, plus one per other field kind (a table cell, a dialog box, the body) for
  the shared helpers. They also cover a right press with a dropdown open (finding 1)
  and a right press the left path would have declined (finding 2).
- The counter is a pure function of `(last press, this press, now)`, unit-tested with
  explicit instants for both rules (finding 3). App tests make their multi-clicks
  with no draw between the presses.
- The arrows mouse tests (`url_bar_drag_selects…`, `table_cell_double_click…`,
  `modal_prompt_field_supports…`, `form_field_double_click…`,
  `double_clicking_a_settings_field…`, and the body and response ones) must pass
  unchanged.

## 6. Decisions (you approved R3, R8 and R10 as written)

- **Q1, decided (b)** (`you`, 2026-10-14). Vim's count applies only on a field's
  text. Lists and buttons keep arrows' count in vim (same row, 400 ms), so a sidebar
  double click works as before.
- **R8**, a fourth press is a plain press (`mine`).
- **R10**, the menu stays everywhere but the field being edited (`mine`).
- **R3** changes three tests that pinned arrows' sweep width in vim. A vim drag now
  takes the char under the pointer, as Vim does. The earlier width was an earlier
  ruling of mine (final review I-3), not yours.

## 7. Plan

1. Write the spec's tests: field kinds, findings 1–2, and the counter unit tests.
   They fail.
2. Add `text_box`, `text_char` and `field_hit`, and move the press arms and drag fns
   onto `press_text` and `drag_text`. The arrows tests must pass unchanged before
   going on.
3. Add the seam's `vim_text_mouse` and the recorded settle. Remove `mouse_at`,
   `body_mouse_at`, `hit_field`, `LastPress` and the nested `MouseEdge`.
4. Add the body's `body_char` path.
5. Make the right press one decision.
6. Make one counter giving `Clicks { gui, vim }` (Q1 (b)).
7. Engine: drop the `rested` reset, and clear `pressed` on `leave`/`enter`.
8. Amend the vim profile spec's 2026-10-14 paragraph to point here. Run the full
   suite, clippy and fmt, check in the TUI in both profiles, and commit.
9. Review rounds start over from round 1.
