# jq Response Filter — Design

Date: 2026-09-03
Status: approved, not yet implemented

## Goal

Let the user reshape a JSON response with jq without leaving the app, and
without knowing jq: a **filter bar** in the response pane runs a jq filter
live over the body, a **structural right-click menu** on any line of the
Pretty tree writes jq into that bar for the common questions ("how many",
"just the names", "only the active ones"), and a **describe-a-filter**
command asks an external AI CLI to write the filter from a sentence.

The engine is embedded, pure Rust (jaq), so the feature works on every
machine the app runs on and needs no `jq` binary.

## Decisions

- **Engine: jaq, embedded.** Crates `jaq-core`, `jaq-std`, `jaq-json`
  (3.x / 2.x, MIT, maintained — 3.1.1 released 2026-08-28). Spike results
  on a release build: 9.4 MB body parses in ~105 ms, filters compile in
  <1 ms, `map(select(...)) | length` over 200k objects runs in ~160 ms;
  typical API bodies are sub-millisecond end to end, so the bar applies on
  every keystroke with no debounce. Documented jq divergences (IEEE
  division by zero, `"NaN"` output, slicing `null` fails, evaluation order
  of binary operators) are all edge cases a menu-driven user will not hit.
  `jaq-json` is built with its `sync` feature so a parsed document can be
  shared with the blocking pool.
- **The filter is a mode of the Pretty view, not a fourth tab.** While a
  filter is set, Pretty shows the filter's output through the same
  `JsonTree`, so collapse, in-pane search, selection, copy and the extract
  actions keep working unchanged. Raw and Headers are untouched.
- **The bar behaves like the search line.** Same header chrome, toggled by
  a `jq` footer chip / `alt+q` / palette "Response: jq filter", applies
  live as you type. `Esc` blurs the bar back to the tree; the bar is
  hidden only when its text is empty, so a bar with text stays visible
  while unfocused and the user can see why the tree looks different.
  Clearing the text is how the filter is removed (there is no separate
  close).
- **The filter text is saved with the request** (`jq = "..."` in the
  request TOML, omitted when empty). It never affects the request that is
  sent. Reopening the request restores it; the next response is filtered
  through it immediately.
- **Clicks compose readable jq.** Every structural menu item writes an
  expression into the bar rather than doing anything opaque, so the bar
  always shows what the click meant and the user can edit it. Appending
  with ` | ` turns a few clicks into one pipeline.
- **The AI path sends the body's shape, not its values.** Key names and
  types only, arrays sampled to one element, depth- and size-capped. Sent
  through a configurable shell command (default `claude -p`) with the
  whole prompt on stdin, so any CLI that reads stdin and prints a reply
  works. First use asks for confirmation once per machine.
- **First cut of verbs:** Filter to this, Copy path, Count, Pluck field,
  Where field, Only items where, Collect into array, Describe a filter.
  Sort by / Group by / Keys / Pick fields / Without field are deferred.

## Engine (`postui-core::jq`)

A new module wrapping jaq so that jaq's types never leave it.

```rust
pub struct JqDocument { /* jaq_json::Val, Arc-shared */ }
pub struct JqFilter   { /* compiled jaq filter, owns its Arena */ }

pub enum JqError {
    /// Lex/parse failure; `span` is a byte range into the filter text.
    Syntax  { message: String, span: Option<Range<usize>> },
    /// `nosuchfn/1` — name and arity, with the call's span when known.
    Unknown { name: String, arity: usize, span: Option<Range<usize>> },
    /// A runtime error from jaq (`cannot index number with "foo"`).
    Runtime { message: String },
}

impl JqDocument { pub fn parse(body: &str) -> Result<Self, JqError>; }
impl JqFilter   { pub fn compile(code: &str) -> Result<Self, JqError>; }
/// Every output of the filter, each serialised as compact JSON.
pub fn run(filter: &JqFilter, doc: &JqDocument) -> Result<Vec<String>, JqError>;
```

- `JqDocument::parse` uses jaq's own reader on the body text; the parse is
  done once per response and cached (see Response component).
- `run` collects all outputs. An error mid-stream discards the partial
  outputs and returns the error, matching jq's exit behaviour.
- Spans: jaq's lex/parse errors carry the offending token as a sub-slice
  of the source; the module recovers the byte offset by pointer arithmetic
  against the code string and falls back to `None` if the slice is not
  within it. Unknown-function errors are reported by jaq with the name
  slice, handled the same way.
- Output size guard: if the joined outputs exceed 64 MB the run returns a
  `Runtime` error ("output too large to display") rather than allocating
  unboundedly — a `[range(1e9)]` typo must not take the app down.

### Shape summary (`postui-core::jq::shape`)

```rust
pub fn shape(body: &str, limits: ShapeLimits) -> Option<Shape>;
pub struct ShapeLimits { pub max_bytes: usize /* 16 KiB */ }
pub struct Shape { pub outline: String, pub example_path: Option<String> }
```

**Revised 2026-10-13.** The first cut sent one JSON-ish line capped at six
levels (arrays counted), sampled only the first array element and cut at
4096 bytes. Nested keys the user named were often missing, so the model
wrote a bare `.leaf`. The user chose an indented outline over a full-path
list (too many repeated prefixes) and a TypeScript type (one nested line
again):

```
page: number
next: string (url)
data[]
  id: string (uuid)
  roles[]: string
  address
    city: string
  manager_id: string (uuid) | null
  meta?
    code: string
```

- One key per line, indented under its parent; no depth limit. Keys that
  are not identifiers are JSON-quoted.
- `name[]` is an array; the lines under it describe its elements, merged
  across every element. A key some of the merged objects lack is `key?`.
- An object with more than 20 distinct keys whose values all hold the
  same kinds (a value that is only ever null aside) is folded into one
  line naming its keys, with every value merged under a `[]` line:
  `rates: object, 2000 keys (AED, AFN, ALL, … 1960 more)`. Keys are
  written as in the outline (bare identifiers, others quoted), whole keys
  up to 400 bytes, then `… N more`; a first key longer than that is cut
  with `…`. This covers lookup tables keyed by data (currency codes,
  dates, ids, also when each array element carries its own key), which
  listed one per line would fill the byte cap and hide the keys after
  them, and wide records of same-kind fields (25 counters), which keep
  every name the model has to write. The example path goes through a
  folded object by its first key (`.stats.views`). A record with mixed
  kinds is listed in full however wide. History: the user chose folding
  lookup tables on 2026-10-13; the first version showed only one example
  key and lost a wide record's field names, a data-looking-keys test was
  tried and dropped, and the user chose listing the names instead. The
  20-key and 400-byte limits are my defaults.
- Scalars are type names; a position holding several types is
  `a | b` in the order object, array, string, number, boolean, null. A
  container that holds both an object and an array lists the object's
  keys and a `[]` line under the union. Always-empty containers are
  `[] (always empty)` / `{}`.
- Strings that look like ISO dates, URLs, emails or UUIDs are labelled
  `string (date-time)` etc. while every string at that position agrees.
- The root is named by its jq path when it is not an object (`.[]`,
  `.: number`).
- Past `max_bytes` the outline is cut at a whole line and ends with
  `… N more lines not shown`.
- `example_path` is the deepest leaf as a jq path
  (`.data[].address.geo.lat`), shown to the model as a template. An
  always-empty `{}`/`[]` is never the leaf; with nothing else there is no
  example.

## Response component

`ReadyView` gains:

```rust
pub jq: JqBar,                 // text, cursor, open/focused flags, error, pending flag
jq_doc: Option<Arc<JqDocument>>,   // parsed once, lazily on first filter use
jq_tree: Option<JsonTree>,     // the filtered rendering, when a filter is applied
jq_stale: bool,                // last run failed; jq_tree is the previous good result
```

Data flow on every bar edit (and once on attach when the saved filter is
non-empty):

1. Empty text → drop `jq_tree`, clear error, Pretty shows the body tree.
2. `JqFilter::compile(text)`; on error set `jq.error`, set `jq_stale`,
   keep the old `jq_tree`. Compile is synchronous (sub-millisecond).
3. Ensure `jq_doc` is parsed. Bodies under `SYNC_PRETTY_BYTES` parse and
   run inline. Larger bodies go to `tokio::task::spawn_blocking` with the
   response generation and a per-view run counter; the result comes back
   through the same channel `attach_tree` uses, and a result whose counter
   is not the latest is dropped. While a run is outstanding the bar shows
   the spinner glyphs; the previous tree stays.
4. On success, outputs are rendered into `jq_tree` via
   `JsonTree::parse_many(&[String])`: one document after another,
   separated by a blank line, exactly as `jq` prints. Collapse state is
   fresh per run (the tree is rebuilt), but the vertical scroll offset is
   clamped rather than reset so live typing doesn't jump.
5. Search, selection, copy, "save body", "open in $EDITOR" and both
   extract actions operate on the view text, which is the filtered text
   while a filter is applied. The Raw tab is always the unfiltered body.

The header chip reads `jq` normally, `jq ·` (dimmed) while stale, and the
error message renders on the line under the bar in the error tone with the
span underlined in the bar text when known. A body that fails
`JqDocument::parse` (not JSON) disables the chip, the key, the palette
entry and the structural menu; the text menu is unchanged.

## Paths on tree lines

`JsonTree::parse` records `path: Vec<PathSeg>` per line (`Key(String)` /
`Index(usize)`), built from the same stack that already tracks
`parent_ids`. A closing-bracket line carries its container's path.
`JsonTree::jq_path(line) -> String` renders it in the form jq accepts:

- identifier-safe keys as `.name`; anything else as `.["odd key"]` with
  JSON string escaping;
- indexes as `[3]`;
- the root as `.`.

`JsonTree` also exposes `nearest_array_ancestor(line) -> Option<(line, relative_path)>`
for the "only items where" verb, and `first_element_keys(line) -> Vec<String>`
for an array whose first element is an object.

## Structural menu

Right-click on a Pretty-tree line opens a menu whose top section is the
structural items, a separator, then today's text items (Copy / Paste /
Extract…). The set depends on the clicked node (empty containers count as
scalars for menu purposes):

| Node | Item | Effect |
| --- | --- | --- |
| any | **Filter to this** | apply `<path>` |
| any | **Copy path** | clipboard ← `<path>` |
| array | **Count** | apply `<path> \| length` |
| array of objects | **Pluck field…** | chooser of first-element keys → apply `<path> \| map(.key)` |
| array of objects | **Where field…** | chooser of keys → tee up `<path> \| map(select(.key == ▮))` |
| scalar inside an array element | **Only items where `key` == `value`** | apply `<array path> \| map(select(<rel path> == <value literal>))` |
| any, when the current filter yields >1 output | **Collect into array** | wrap the bar text: `[ <text> ]` |
| any | **Describe a filter…** | see AI section |

"Apply" writes the expression and runs it. "Tee up" writes it, focuses the
bar with the cursor at `▮`, and runs nothing until the text next changes
(a tee-up whose blank is never filled just sits as a syntax error, which
the bar already displays, with the previous tree kept).

Composition: with an empty bar, `<path>` is the node's absolute path. With
a filter already applied, the clicked node lives in the filtered tree, so
`<path>` is its path relative to that output and the item appends
` | <expression>` to the existing text. Paths are omitted when they are
`.` (so Count on the root of a filtered array appends just ` | length`).
When the applied filter produced more than one output the composing
items are disabled with the hint "collect into array first", and
Collect into array is offered.

The scalar literal in "only items where" is the value's JSON text, so
numbers stay bare and strings stay quoted. The item is absent when no
ancestor is an array, or when the scalar is the array element itself (no
relative path). It uses the nearest enclosing array, with the relative
path being everything below that array's element.

Pluck and Where use the existing chooser modal (there is no submenu
support in the menu component today) titled with the verb; picking a key
completes the item; `Esc` cancels with the bar untouched.

Keyboard: while the response pane is focused the footer advertises `alt+q
jq`; with the bar focused it shows `esc close`, `enter apply` (applies
live anyway, `enter` just blurs back to the tree), and `alt+q` toggles
focus between bar and tree. (Superseded — see the usability round below:
`alt+q` always focuses, `alt+shift+q` is the switch, `esc` cancels.) Pluck/Where/Only-items are mouse-menu items in
this cut; the palette carries "Response: jq filter" and "Response:
describe a filter (AI)…".

## Describe a filter (AI)

Entry points: the structural menu, the palette, and a `✦` button at the
right end of the bar. Opens a one-line prompt modal, "What do you want to
see?".

On confirm the app builds one text on stdin (there is no separate system
prompt; the command sees it all as one message):

```
<system section — the SYSTEM text in postui-core/src/jq/ai.rs: one-line
reply, no prose/fences/backticks, preferred builtins, how to read the
outline, "write its whole path from the root … never a bare .key unless
that key is at the top level", the structure is the document before any
filter>

Structure:
<shape.outline>

A full path in this document: <shape.example_path>   (when there is one)
Current filter: <bar text, or "(none)">

Request: <user's sentence>
```

and runs the configured command through `sh -c "<ai_cmd>"` with that on
stdin, stdout and stderr captured, in a tokio task with the response
generation and a request counter. The bar shows the spinner and `asking…`;
while a request is pending `Esc` in the bar cancels it (kills the child)
instead of blurring, and a stale reply is dropped.

Reply handling: trim; strip a surrounding ``` fence if present; take the
first non-empty line; when it holds backticks, keep what is inside the
first pair (backticks are never jq, so prose around a quoted filter goes
too). `JqFilter::compile` it: on success the text lands
in the bar and applies; on failure the text still lands in the bar with
the jaq error under it so the user can repair it by hand. A non-zero exit
or empty output toasts the last line of stderr ("claude: not logged in").
If `ai_cmd`'s program is not on PATH the menu item and palette entry are
disabled with the hint "`claude` not found — set ai_cmd in config.toml".

Config (`config.toml`, top level, beside `clipboard_cmd`):

```toml
ai_cmd = "claude -p"          # default; any command that reads stdin, prints a reply
ai_confirmed = true           # written by the one-time confirm
```

`postui --debug-ai [directory]` (added 2026-10-13) appends every exchange
to `$XDG_STATE_HOME/postui/ai.log` (default `~/.local/state`, on macOS
too like the config dir; the local data directory on Windows): timestamp
and command, the whole prompt, raw stdout, stderr, exit status, and the
filter taken from the reply. A startup toast names the file. The writes go
through `hostfs::append_log`: the prompt synchronously before the task is
spawned (so a request cancelled before it ever runs still logs it), the
reply on the blocking pool.

First use shows a modal: "Send the response's structure (key names and
types, no values) to `claude -p`?" with Send / Cancel and a "don't ask
again" toggle that writes `ai_confirmed`. Values are never sent in this
cut; a `ai_send_values` opt-in is a possible follow-up.

## Request model and persistence

`HttpRequest` gains `pub jq: Option<String>` (serde default, skipped when
`None`); `to_toml_string` writes `jq = "…"` after `insecure`, omitted when
empty. The editor's dirty tracking includes it, so editing the bar marks
the request modified and `ctrl+s` persists it; discard restores it. Undo
treats a bar edit like a URL-bar edit (one undo entry per pause in
typing, using the existing coalescing).

*Amended 2026-10-24 (#96):* commit a3aa6ca had since masked `jq` and
`jq_enabled` out of dirty tracking. The jq views amendment
(`2026-10-23-jq-views-design.md`, A2, the user's) restores this
section's rule: the filter, now the scratch view, and its switch dirty
the request.

## Error handling summary

- Syntax / unknown function: shown under the bar with a span underline;
  previous tree kept; chip dimmed.
- Runtime error: same, no span.
- Output too large: same, message names the cap.
- Non-JSON body: feature disabled, no message.
- AI command missing / failing / cancelled: disabled item, toast, or
  silent drop respectively; the bar is never left in `asking…`.
- Response replaced mid-run (new send): generation mismatch drops the
  result; the saved filter is re-applied to the new response.

## Testing

- `postui-core::jq` unit tests: compile + run, multi-output, each
  `JqError` variant with its span (`.foo | select(` → span on `(`;
  `nosuchfn(1)` → name/arity), runtime error message, output cap, and
  `JqDocument::parse` rejecting non-JSON.
- `shape` tests: unlimited nesting as indentation, array elements merged
  with `key?` for keys some lack, wide same-kind objects folded into one
  line naming their keys (cut at a whole key with `… N more`; records of
  mixed kinds not folded), type unions, root naming, quoted keys,
  the byte cap's "N more lines" note, the example path, scalar type names
  and the date/url tags.
- `JsonTree` tests: per-line path for nested keys, indexes, odd keys
  needing bracket quoting, closing-line path, `nearest_array_ancestor`,
  `first_element_keys`, `parse_many` separator lines.
- App tests (existing `App` harness, ready response injected via
  `ResponseState::Ready`): open the bar and type `.data.items | length`,
  tree shows `2`; right-click an array line → Count writes
  `.data.items | length`; second click composes with ` | `; Only-items
  emits `map(select(.status == "active"))` on the nearest array; a syntax
  error keeps the old tree and dims the chip; multi-output disables
  composing items and Collect wraps in `[ ]`; the filter round-trips
  through `to_toml_string`/`from_toml_str` and survives request reload;
  a new response re-applies the saved filter; large bodies go through
  the blocking path and a stale generation is dropped.
- AI tests point `ai_cmd` at shell stubs: one echoes a canned filter
  (lands and applies), one wraps it in fences (stripped), one prints
  garbage (lands with error), one exits 1 (toast), one sleeps (Esc
  cancels). No network, no real `claude`.

## Out of scope (follow-ups)

- Sort by / Group by / Keys / Pick fields / Without field verbs.
- Sending values to the AI command (`ai_send_values`).
- Using a jq path as a live extractor: "Extract to variable" from a tree
  line recording the path so the variable refreshes on every send. The
  per-line path work here is the foundation for it.
- jq on the request side (body templating).
- Keyboard-only access to the structural verbs.

## Implementation notes

A few details of the shipped behaviour are worth recording against this
design, either because they differ from what's described above or
because they're easy to get wrong reading the code cold:

- **Closing the bar is the filter's off switch** (2026-09-03 usability
  round). The design above said the bar hides only when its text is
  empty and "clearing the text is how the filter is removed". Shipped:
  the header 󰈲 button and `alt+shift+q` (`Action::ToggleJqBar`) *close*
  an open bar whether or not it is focused — the text stays, the filter
  is switched off, and the tree shows the full body. The same two routes
  open a closed bar switched on and focused. `alt+q`, the palette entry
  and the footer's `filter` chip are `Action::OpenJqBar` (2026-09-04):
  they always put the caret in the bar, switching it on if it was off,
  and never switch it off — alt+q is one gesture, "type a filter", so
  the switch lives on its own key.
  `Esc` in the bar *cancels the edit* (`Action::CancelJqEdit`): the bar
  remembers the text as it stood when the typing started
  (`JqBar::edit_origin`, taken by `begin_edit` just before the first
  change to a focused bar — a key, a paste, an accepted completion, a
  tee-up or an AI reply landing — and forgotten on blur), puts that
  back, and blurs. Nothing typed yet, and Esc only blurs. The on/off
  switch is never part of the edit: a bar opened from off stays on
  (opening it was the user's doing). One opened onto no filter is empty
  again and, unfocused, hidden (the switch is left on — nothing left to
  be off). `JqTeeUp` focuses before it sets the text so Esc cancels the
  tee-up back to the filter before it. The revert is an undoable edit
  whenever the text changed. From the tree, Esc stops at the selection
  and the search: it never touches the saved filter. `Enter` (and
  `Down`, the URL field's leave key) blurs to the tree with the edit
  kept and the filter on and the bar still showing — with what was
  typed, never a ghost (fish's rule: a ghost is an offer, and `.id`
  must be reachable when `.identifier` exists). The one exception is
  menu mode's entered row, where Enter first confirms the selected chip
  and stays in the bar (the footer reads `enter select`); Enter again
  leaves. The off state persists as `jq_enabled = false` in the request
  TOML (omitted when on, and never written without a filter to switch
  off); toggling dirties the request like a text edit. A structural verb
  or an AI reply landing a filter switches a closed bar back on.
- The footer chips shown while the bar is focused are `enter apply /
  esc cancel / alt+shift+q unfilter / ✦ describe…`; the response pane's own
  chips always carry `alt+q filter`, plus `alt+shift+q unfilter` while a
  bar is open. Clicking in the bar places the caret, dragging selects,
  double-click selects the word, and right-click offers Copy / Paste
  (`TextSurface::Jq`; no extract-to-variable/selector items — the text is
  a filter, not a value). The same as the URL bar otherwise.
- Structural verbs compose onto `ReadyView::jq_tree_code` — the filter
  whose output is actually on screen — not the bar text, so a verb
  clicked on the body under a null-note (or on the previous good tree
  under a syntax error) starts from the right document. Enter does not "commit" anything separately;
  the filter is already live on every keystroke. The 󰈲 button paints
  pressed (inverted) while a filter is on.
  *Amended 2026-10-14 (task #3, your request):* pressed means the bar is
  showing (`jq_open`), so an empty bar just opened is pressed too, and a
  pressed button always closes on click. Clicking the bar's own switch
  (the 󰈲 button, the footer's `unfilter` chip) is not a click away from
  a focused bar. Before this, the click blurred an empty bar first, which
  hid it, and the toggle then opened it again. The bar shows while an AI
  request is outstanding, so closing it (the toggle) now ends that request
  and closes the bar. Esc, by contrast, ends the request and leaves the
  bar open. Otherwise the pressed button could not close the bar (my call
  in code review, not yours). The button's hint follows the paint:
  pressed reads as the toggle ("Close the jq filter"; the toggle is not a
  palette command, so no key is named), unpressed as the palette's "Filter
  the JSON with jq · alt+q", the way the Send button's hint flips to
  Cancel.
- **A run whose outputs are all `null` — every document `null`, or one
  array of nothing but nulls — or that produces no output at all does
  not replace the tree.** The full body shows, the chip dims as for an
  error, and a red `invalid filter` line under the bar says why (the
  bar's `note` still records `null` vs `no output` internally). This is what a half-typed path (`.mo`)
  yields on every keystroke, and blanking the tree under someone who is
  reading it while they write the filter was the complaint. It is the
  committed behaviour too, not only a preview while typing: a saved
  filter that matches nothing on a new response shows the body with the
  note. An empty array is a real answer and is shown.
- **Multiple outputs are not separated by a blank line.** `parse_many`
  runs the documents together, exactly as `jq` prints a stream; the
  design's "separated by a blank line" was wrong about jq.
- The palette's "describe" entry stays listed even when `ai_cmd` isn't
  configured or the program is missing — the palette has no gating
  mechanism. Choosing it in that state toasts the missing-program hint
  instead of running.
- Staleness (the last run failed; the tree on screen is the previous
  good one) is shown by the `jq` chip's colour dimming, not by a `jq ·`
  suffix or any other text marker.
- `OUTPUT_CAP` bounds the *serialised* size of all outputs collected from
  one run, so it guards a filter that emits many documents (or one very
  verbose one) — it does not stop a single giant output completing
  before the cap is checked (e.g. `[range(1e9)]` building one huge array
  in memory before serialisation). A per-output size guard in
  `run_with_cap` is a tracked follow-up.
- The filtered tree (`JsonTree::parse_many`) is built on the UI thread
  once a run's outputs come back, whether the run itself was inline or
  handed to the blocking pool. Moving that build off-thread too, for
  large bodies, is a tracked follow-up.
- There is no `JqFilter` (compiled-and-cached) value anywhere in this
  feature: `postui_core::jq::check` and `postui_core::jq::run` both
  recompile the filter text from scratch on every call. A `JqDocument`
  (the *body's* parse) is what gets cached and reused.
- The bar's `LineInput` lives on `Response::jq` (a `JqBar`), not on the
  editor. `Editor.jq: String` is the persisted source of truth (what
  round-trips through `to_toml_string`); `App::sync_jq` reconciles the
  two once per `update` — editor → bar on everything but a bar edit,
  bar → editor on a bar edit — and is also where the filter is actually
  applied (`Response::apply_jq`).
