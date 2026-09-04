# jq Filter Completion — Design

Date: 2026-09-03
Status: approved, not yet implemented

## Goal

While the user types a jq filter in the response pane's bar, offer the
next thing they probably mean — a key of the JSON the cursor is looking
at, or a jq builtin — as **ghost text** after the caret, accepted with one
key. The bar stays a single row; nothing overlays the tree.

Key completion is the real payoff: `.us` → `.users`, `.users[] |
select(.active) | .na` → `.name`. Builtin completion (`sel` → `select(`)
rides along because jaq already exposes its definitions.

## Decisions

- **Suggestions come from evaluating the filter's prefix with jaq.** The
  text before the partial token is turned into a *context expression*
  (see below) and run against the already-parsed `JqDocument`, taking at
  most a bounded number of outputs; the keys of the object outputs are
  the candidates. This is the only approach that is right under pipes,
  `select`, `map`, `[]`, `[0]` and `..`, and jaq's laziness makes the
  bounded run cheap. A hand-written path walker was rejected (dead on
  the first `|`), as was a flat "every key anywhere" index (suggests
  keys that don't exist at that position).
- **Ghost text, not a dropdown.** The best candidate is drawn dimmed after
  the caret. It fits the one-row bar and never covers the tree the user
  is reading. Several candidates are reached by cycling, not by a list.
- **Two Tab behaviours, chosen in config.** `jq_tab = "cycle"`: Tab
  shows the next candidate, shift+Tab the previous, Right/End accept.
  `jq_tab = "menu"` (2026-09-04; it replaced `"accept"`, which is kept as
  an alias, and became the default the same day — the row's preview
  beat the ghost in use): no ghost at all. The candidates are on show as a row of
  chips under the bar whenever there are any — one per candidate, the
  whole token each, nothing selected — narrowing live as the user types,
  so nothing is ever pushed at them as *the* guess (a wrong ghost made
  Tab feel unsafe, when Tab was also the way to the alternatives). Tab
  writes the first candidate into the bar and enters the row (the chip
  fills), and further Tab/shift+Tab step through it, rewriting the bar
  text from the pre-completion base each step so a rewrite never
  compounds. Shift+Tab on the offered row enters it at the last
  candidate. Enter on the entered row confirms the selected chip and
  leaves the row, staying in the bar (Enter again leaves the bar). Esc
  on the entered row un-picks it: the text goes back to what was typed
  before Tab, the row shows again unentered, the caret stays (Esc again
  cancels the whole edit as usual). Any other key leaves the row, keeps
  the selection, and is handled as usual — the row then simply tracks
  what is typed next. Enter on the unentered row,
  or on a cycle-mode ghost, leaves the bar with what was typed: a
  ghost is an offer, never text. Down leaves the bar in both modes. A lone
  candidate is previewed like any other and just accepted by Tab. The
  row lives in the bar's second row (the error/note row, which it takes
  over while showing) and is a window that slides only as far as it must
  to keep the selected chip whole, snapping home on wrap; with nothing
  selected it starts at the left. Right/End accept only a ghost, so in
  menu mode they are plain caret moves. A top-level
  `config.toml` key, loaded like `theme` and `ai_cmd`; a bad value warns
  and falls back to `menu`. Not settable from the UI in this round.
  The original `"accept"` (Tab accepts, type more to reach a later
  candidate) shipped first and was replaced the next day: it was thinner
  than users expect from Tab.
- **Only at the end of the text.** The ghost is offered only when the
  caret is at the end of the filter with no selection. Mid-text
  completion would have to draw the ghost inside existing text and shift
  it, and the payoff is small: filters are written left to right.
- **A ghost is always a continuation.** Only candidates that strictly
  extend the typed partial are offered; a key equal to it is dropped
  (nothing to add), so `.id` ghosts `s` when `ids` also exists, and the
  user always sees when Tab has somewhere to go. This is how fish,
  browser address bars and Copilot behave. Body keys keep the body's
  order (the order the tree shows, first appearance across the context's
  outputs); builtins are alphabetical.
- **Accepting is a plain edit.** Accept inserts the missing characters
  through the bar's `LineInput`, so the filter re-runs live exactly as if
  they were typed, undo/redo sees an ordinary edit, and the completion
  state re-derives from the new text (after accepting `.users` the ghost
  disappears until the next `.` or letter).
- **Nothing is offered on an empty partial.** After `.` with nothing typed
  yet the first key *is* offered (that is the most useful moment: "what's
  in here?"). After `|` or `(` with no letter typed, nothing is offered —
  a ghost `abs` after every pipe is noise.
- **No debounce.** The context expression only changes when the prefix
  changes; typing more of the partial narrows the cached candidate list
  without touching jaq. Runs follow the filter's own rule: inline for a
  body under `SYNC_PRETTY_BYTES`, on the blocking pool otherwise, tagged
  with the view's generation so a late result for an old response or an
  older request is dropped.

## Engine (`postui-core::jq::complete`)

A new module, pure apart from one function that runs jaq. jaq's types
never leave it.

### Splitting the text

`pub fn context(text: &str) -> Option<Context>` inspects the whole filter
text (the caller guarantees the caret is at its end) and returns:

```rust
pub struct Context {
    /// What is being completed.
    pub kind: Kind,
    /// The characters already typed of the token being completed
    /// (`us` for `.us`, `sel` for `sel`, `my k` for `."my k`).
    pub partial: String,
    /// Byte offset in `text` where the token being completed starts —
    /// the `.` for a key, the first letter for a word.
    pub token_start: usize,
    /// For `Kind::Key`: the jq expression whose outputs the caret's `.`
    /// refers to. `None` for `Kind::Word`.
    pub input_expr: Option<String>,
}

pub enum Kind {
    /// `.` followed by an identifier prefix, or `."` followed by any text
    /// (a quoted key being typed; `quoted` is true).
    Key { quoted: bool },
    /// A bare word not preceded by `.`, `$` or `@`: a builtin name.
    Word,
}
```

Returns `None` when the text ends in anything else (whitespace, an
operator, `..`, a number, inside a string literal that is not a key,
`$name`, `@fmt`), or when the partial word has no letters.

### The context expression

For `Kind::Key`, `input_expr` is built from the text before `token_start`
(`head`) by a small scanner that understands strings, brackets and
top-level pipes — not a jq parser:

1. Find the innermost unclosed opener in `head` (`(`, `[`, `{`), skipping
   string literals. Everything after it is the *segment*; everything
   before it is resolved recursively as the *outer* expression. With no
   unclosed opener the segment is all of `head` and the outer expression
   is `.`.
2. If the opener is `(` immediately preceded by an identifier `f`, the
   outer expression is resolved from the text before `f`. If `f` is one
   of the per-element builtins (`map`, `map_values`, `sort_by`,
   `group_by`, `unique_by`, `min_by`, `max_by`, `any`, `all`; one
   `const`, easy to extend) then `| .[]` is appended to the outer
   expression, because inside `map(` the `.` is each element. Any other
   function (`select`, `has`, `del`, `path`, user-typed names) passes its
   input through unchanged, and so do `[` and `{` openers.
3. Within the segment, split at the last top-level `|` (again skipping
   strings and nested brackets). The text before it is the *stage*
   prefix; the text after it is the *tail*.
4. From the end of the tail, scan back over a *path chain*: a run of
   `.ident`, `."string"`, `[...]` (balanced), `?`, and a bare `.` that
   is immediately followed by `[`. The chain stops at whitespace, an
   operator, a comma, `|`, or the segment start. `.a == .b.c` yields the
   chain `.b` (the partial `.c` was removed first); `.users[] | .na`
   yields an empty chain and the stage `.users[]`; `.users[].na` yields
   the chain `.users[]` and no stage. Anything between the stage and the
   chain (`.a == `, `if .x then `) is ignored: the caret's `.` still
   sees the stage's output.
5. `input_expr` = outer, stage and chain joined with ` | `, skipping the
   empty ones; when all are empty it is `.`.

Examples the tests pin down (`text` → `input_expr`, partial):

| text                                        | input_expr                     | partial |
|---------------------------------------------|--------------------------------|---------|
| `.us`                                       | `.`                            | `us`    |
| `.`                                         | `.`                            | ``      |
| `.data.it`                                  | `.data`                        | `it`    |
| `.data.items[].na`                          | `.data.items[]`                | `na`    |
| `.data.items[0].`                           | `.data.items[0]`               | ``      |
| `.data.items[] \| .na`                      | `.data.items[]`                | `na`    |
| `.data.items[] \| select(.st`               | `.data.items[]`                | `st`    |
| `.data.items \| map(.na`                    | `.data.items \| .[]`           | `na`    |
| `.data.items[] \| select(.status == "a") \| .i` | `.data.items[] \| select(.status == "a")` | `i` |
| `.data.items[] \| {name: .name, s: .st`     | `.data.items[]`                | `st`    |
| `[.data.items[] \| .na`                     | `.data.items[]`                | `na`    |
| `.data.items[] \| .id == .na`               | `.data.items[]`                | `na`    |
| `.data."my k`                               | `.data`                        | `my k` (quoted) |
| `..`                                        | none                           |         |
| `.data.items \| leng`                       | word                           | `leng`  |
| `.data \| `                                 | none                           |         |
| `$x.na`                                     | `$x` (does not compile → no candidates) | `na` |
| `.a as $x \| $x.na`                         | `.a as $x \| $x`               | `na`    |

`context` is a pure function with a table test; it must never panic on
any input (fuzz-style test over random ASCII and the existing test
filters).

### Keys at the context

```rust
pub const COMPLETE_OUTPUTS: usize = 64;

/// Runs `input_expr` against `doc`, takes at most `COMPLETE_OUTPUTS`
/// outputs, and returns the keys of those that are objects, in order of
/// first appearance, deduplicated. An error yields whatever was collected
/// before it (a half-typed prefix that does not compile is normal, not a
/// failure to report).
pub fn keys_at(input_expr: &str, doc: &JqDocument) -> Vec<String>;
```

Uses the module's existing `with_compiled`; stops iterating at the cap,
so `.users[]` over 200k items never walks them all. An output that is an
array contributes nothing (jq would error on `.arr.key` anyway).

### Builtins

```rust
pub struct Builtin { pub name: &'static str, pub arity: usize }

/// Every definition and native function jaq loads, deduplicated by
/// name (lowest arity kept), names starting with `_` dropped, plus the
/// keywords `if then elif else end and or not reduce foreach try catch
/// as def label`. Sorted. Built once (`OnceLock`).
pub fn builtins() -> &'static [Builtin];
```

`jaq_core::defs()`, `jaq_std::defs()`, `jaq_json::defs()` give
`Def { name, args, .. }`; `funs()` gives `(name, arity, _)` tuples. Both
are already assembled in `jq/mod.rs`.

### Candidates

```rust
pub struct Candidate {
    /// What the ghost shows after the caret.
    pub ghost: String,
    /// What accepting inserts. Usually equal to `ghost`; differs only
    /// when the token has to be rewritten (see `replace_from`).
    pub insert: String,
    /// When `Some`, accepting first deletes from this byte offset to the
    /// caret, then inserts `insert`: a key that needs quoting turns the
    /// typed `.my` into `."my key"`.
    pub replace_from: Option<usize>,
}

/// Filters `keys` (for `Kind::Key`) or `builtins()` (for `Kind::Word`)
/// to those that start with `partial` (case-sensitive) and are longer
/// than it, orders as decided above, and renders each into a candidate.
pub fn candidates(ctx: &Context, keys: &[String]) -> Vec<Candidate>;
```

Rendering rules:

- Identifier key, unquoted partial: ghost and insert are the rest of the
  key (`.us` → `ers`).
- Key that is not an identifier (`my key`, `0abc`, `a-b`), unquoted
  partial: offered when the key starts with the partial; ghost is
  `"my key"` minus nothing (drawn whole so the user sees the quotes
  coming), insert is `."my key"`, `replace_from` is `token_start` (the
  `.`).
- Quoted partial (`."my k`): only keys starting with the partial; ghost
  and insert are the rest of the key plus the closing `"`.
- Builtin: the rest of the name, plus `(` when arity > 0.

## UI (`postui::components::response`)

### State

```rust
pub struct JqCompletion {
    /// The context the candidates were built for; recomputed on every
    /// bar edit, compared to decide whether a new key fetch is needed.
    input_expr: Option<String>,
    /// Keys fetched for `input_expr` (empty for `Kind::Word`).
    keys: Vec<String>,
    /// The current context — `None` when the caret's position offers
    /// nothing.
    ctx: Option<Context>,
    candidates: Vec<Candidate>,
    /// Which candidate the ghost shows; reset to 0 by any edit.
    index: usize,
    /// A key fetch is outstanding on the blocking pool (its sequence
    /// number). Only the newest fetch's result is kept.
    pending: Option<u64>,
    seq: u64,
}
```

Lives on `JqBar` as `completion: JqCompletion`. `JqBar::ghost()` returns
the current candidate's ghost text when the bar is focused, the caret is
at the end with no selection, `ai_pending` is false and a candidate
exists.

### Refresh

`Response::refresh_jq_completion(&mut self, sync_limit) ->
Option<JqCompleteRequest>` runs right after `apply_jq` in the app's
jq-bar reconcile (the same place the filter is re-applied after a bar
edit), so every edit keeps both in step:

1. If the bar is not focused, the caret is not at the end, or there is
   a selection, clear `ctx` and `candidates` and return `None`.
2. Compute `context(text)`. `None` → clear and return.
3. `Kind::Word` → candidates from `builtins()`, no fetch.
4. `Kind::Key` with `input_expr` equal to the cached one → rebuild
   candidates from the cached keys (typing more of the partial).
5. Otherwise a fetch is needed. Body under `sync_limit` → `keys_at`
   inline, cache, build candidates. Larger → bump `seq`, set `pending`,
   return `JqCompleteRequest { generation, seq, input_expr, doc }` for
   the app to run on the blocking pool; the ghost stays empty until it
   lands. The doc is the view's `jq_doc`; with no document (non-JSON
   body, or the filter's own first run is still parsing it) there is no
   fetch and no key candidates (the next edit will find the document).

`Action::JqCompleteFinished { generation, seq, input_expr, keys }` is
attached only when `generation` matches the view and `seq` matches
`pending`; it caches the keys and rebuilds candidates against the
*current* text's context, which may have moved on (a different
`input_expr` by now means the result is cached but not shown).

`spawn_jq_complete` mirrors `spawn_jq_run`, including the no-runtime
fallback used by tests.

### Keys

In `ready_key`'s jq-focused branch, before the event reaches the
`LineInput`, when `ghost()` is `Some`:

| key             | cycle mode (ghost showing)    | menu mode (row on show)                          |
|-----------------|-------------------------------|--------------------------------------------------|
| Tab             | next candidate (wraps)        | enter the row at the first (entered: next, wraps) |
| shift+Tab       | previous candidate (wraps)    | enter the row at the last (entered: previous)     |
| Right, End      | accept                        | a plain caret move (entered: leaves the row first) |
| Enter           | leave the bar, typed text kept | leave the bar (entered: confirm the chip, stay)   |
| Esc             | cancel the edit               | cancel the edit (entered: un-pick, stay)          |
| Down            | leave the bar                 | leave the bar                                    |
| anything else   | falls through to the input; index resets to 0 (menu mode: an entered row is left first) |

When `ghost()` is `None`, Tab and shift+Tab are ignored by the bar
(today they fall to `LineInput`, which ignores them too), and Right/End
behave as before.

Accept: if the candidate has `replace_from`, delete from there to the
caret first; then `insert_str(candidate.insert)`; mark `edited`. The
reconcile then re-runs the filter and recomputes the completion, so the
result is indistinguishable from typing.

Esc keeps its meaning (cancel the edit and blur); a ghost is dismissed by
typing on, never by a key of its own.

### Drawing

`draw_jq_bar` appends one `Span` in `theme.text_muted` after the input's
windowed line when `bar.ghost()` is `Some`, clipped to the row's
remaining width (`text_w` minus the input's visible width). The window
calculation is unchanged: the ghost never pushes the caret off screen;
when the text already fills the row the ghost is simply not visible.

The focused bar's caret is the terminal's own cursor, shaped as a steady
bar once at startup (`SetCursorStyle::SteadyBar`, restored to the user's
default on every exit path), not the painted REVERSED cell the other
inputs use: the ghost starts in the caret's own cell, so a thin bar
reads as "the text continues here" where a block would swallow the
ghost's first letter. `LineInput::draw_line_windowed_no_caret` draws
the text without that cell and `caret_column` says where the cursor
goes; `draw_jq_bar` reports the cell through `Response::jq_caret_cell`,
and `ui::draw` places the cursor (`Frame::set_cursor_position`) only
while no modal covers the pane. Everywhere else the cursor stays hidden.

Footer, jq-bar-focused chip set: when a ghost is up, `("tab", "next")`
plus `("→", "accept")` in cycle mode; when menu mode's row is on show,
`("tab", "complete")`; with the row entered, `("tab", "next")` plus
`("shift+tab", "prev")`. All inserted before the existing chips. `JqBarState` has a
`Completing { cycle: bool }` variant and a `Menu` variant, so
`footer_chips` stays a pure function of its inputs.

### Mouse

None in this round. A click in the bar places the caret as today; the
ghost is not a hit target.

## Config

`UiSettings` gains `jq_tab: JqTab` (`enum JqTab { Cycle, Menu }`,
default `Menu`), read from the top-level `jq_tab` string key by
`load_ui_settings`, with an unrecognised value producing a warning in the
returned list and the default. `App` hands the mode to the response
component at construction (the same way other settings reach it).

## Errors and edge cases

- A context expression that fails to compile or run yields no candidates
  and no error: the bar's `error`/`stale`/`note` belong to the filter,
  never to completion.
- A completion fetch is bounded by `COMPLETE_OUTPUTS`, so it cannot hit
  `OUTPUT_CAP`; a pathological prefix (`sort_by` over a huge array) is
  slow only off-thread, and a newer fetch supersedes it.
- Response shed/reparse (`shed_derived`, `take_reparse`) clears the
  completion cache with the rest of the jq state.
- A new response landing in the slot keeps the bar text and re-derives
  the completion from the new document on the next edit or focus.
- Non-JSON bodies: no document, so no key candidates; builtins still
  complete (they don't need one).

## Testing

- `complete.rs`: the table above as one test per row group; a
  no-panic sweep over random input; `keys_at` over the module's fixture
  (`.data.items[]` → `id, name, status`; a non-compiling prefix → empty;
  the output cap stops a `range(1e9)` context); `builtins()` contains
  `select` (arity 1), `length` (0), `map` (1), `if`, and nothing
  starting with `_`; `candidates` ordering, identifier-vs-quoted
  rendering, `(` suffix.
- `response.rs` tests: ghost appears after `.` on a JSON body and not on
  an unfocused bar; Tab cycles and shift+Tab cycles back in cycle mode;
  Right accepts and the text and filter output match typing; accept mode
  Tab accepts; `."my` completes to `."my key"`; a word completes to
  `select(`; a mid-text caret shows no ghost; the note/error rows are
  unaffected.
- `app/tests.rs`: a large-body completion request goes to the pool and
  its result attaches; a result for a stale generation or sequence is
  dropped; `jq_tab = "accept"` in config reaches the bar; a bad value
  warns.
- Manual, via the tmux recipe: the 7.7 MB sample — typing `.` shows the
  root keys without a visible pause; `.data.items[] | .` shows item keys.
