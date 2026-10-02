# Vim text engine and Vim conformance tests (piece 3)

Status: approved by the user 2026-09-28, after a section-by-section
review (drafted 2026-09-27). Nothing was implemented at approval. The user
decided all five open questions on 2026-09-27, each by accepting my
recommendation (§9). Every decision
is labelled: `you` (a rule you stated, or one of your 2026-09-27 decisions),
`mine` (my recommendation, accept or reject), `earlier` (settled during the
vim-mode rounds but not shown to be yours, so treat it as mine).

Implementation: plan 3a (buffers, the conformance harness, every tier-1
key, the session edges and the API) is `docs/superpowers/plans/2026-09-29-vim-engine-3a.md`,
on branch `vim-engine-3a`.
Plan 3b (the tier-2 first wave) is `docs/superpowers/plans/2026-09-30-vim-engine-3b.md`
and plan 3c (the second wave) `docs/superpowers/plans/2026-10-01-vim-engine-3c.md`,
on branches `vim-engine-3b` and `vim-engine-3c`. Each plan lists its
deviations from this text; the decisions below marked 2026-10-01 come
from 3c's review.

Companion documents: the key list
`docs/superpowers/specs/2026-09-27-vim-target-keys.md` (§1–§5 are this
piece's scope) and the old design `2026-09-18-vim-mode-design.md` (its "Text
fields" section is replaced by this spec).

## 1. Background

The `vim-mode` branch gave one-line fields a sound vim core
(`vim/field.rs`), but gave the request Body a shim over edtui 0.11.6's own
vim handler. edtui's `Action` enum is closed, its pending state is private,
`capture()` (its undo snapshot) is `pub(crate)`, it has no operator+motion
grammar, and its `i{`/`i"` objects are single-line and ignore nesting, so they
make wrong edits on JSON. You chose to rebuild in four pieces and to "complete
our own" vim engine instead of patching edtui or adopting modalkit (`you`).
This piece is that engine, plus tests that compare it with real Vim 9.1.
Piece 4 (the vim profile: router, lists, panes, ex commands) calls it.

## 2. Goals and success criteria

Goals:

1. One engine handles every text surface in the vim profile: the URL, table
   cells, prompt inputs, form fields, Settings edits, the query boxes
   (Insert only, §4.2), and the request Body.
   The same code and the same key grammar run on a one-line and a multi-line
   buffer (`you`: own engine; `mine`: one engine for both).
2. The arrows profile is not changed at all. `LineInput::handle_key` and the
   Body's `EditorEventHandler::emacs_mode()` keep serving it (`you`: nothing
   may require vim).
3. In the vim profile, edtui stores and draws the body text. Its key handler
   is never called, and the engine never calls `EditorState::execute`
   (`you`: this was the accepted direction). The engine, not edtui's undo
   stack, owns the body's undo (§3.11; my recommendation, accepted
   2026-09-27 as open question 1).
4. Undo restores exactly (`you`, rule 1). Undo granularity and the caret
   position after `u` and `ctrl+r` match Vim (`mine`).
5. The engine makes no app decisions. It returns an outcome and piece 4
   decides what a declined key means (§4).

Success criteria:

- Every tier-1 text row of the key list (§1–§5) passes the conformance test
  on the body buffer. It also passes on the one-line buffer wherever Vim's
  result is one line (§6.5). Divergences are allowed only through
  `divergences.toml`, each one labelled and explained. Target at the first
  release: zero divergences, apart from the ones the user accepted (the
  case operators, and plan 3c's R2 and search-offset entries) and the two
  `mine` `gi` entries still awaiting the user's review (§6.6).
- `cargo test` needs no Vim installed. The golden file is committed.
- The tier-2 rows marked "first wave" in §5 pass before piece 4 ships. The
  rest are generated into the golden file but reported as "not yet" without
  failing the test.
- No file under `crates/postui/src/vim/engine/` names an app type (`App`,
  `Action`, `Editor`, `FieldId`). The one exception is `buf.rs`, which
  adapts `LineInput` and `EditorState`.

## 3. Architecture

### 3.1 Modules

All new, under `crates/postui/src/vim/engine/`. Piece 3 creates
`vim/mod.rs` containing only `pub mod engine;`. Piece 4 adds its router
next to it.

| File | Holds |
|---|---|
| `mod.rs` | `Engine`, `BufState`, `Outcome`, `ViewCtx`, `Mode`, the public API (§4) |
| `buf.rs` | `Pos`, the `TextBuf` trait, `OneLineBuf` (over `LineInput`), `BodyBuf` (over `EditorState`) |
| `settings.rs` | the pinned settings as constants, plus `SETTINGS_LINE`, the exact `:set` string the oracle runs |
| `keys.rs` | the key parser (`Pending` → `Cmd`), the count rules, the echo string |
| `motion.rs` | every motion: target position plus exclusive/inclusive/linewise |
| `object.rs` | word, WORD, quote, bracket and paragraph text objects |
| `op.rs` | turns a motion or object into a `Range`, and applies operators |
| `insert.rs` | Insert and Replace sessions, and the record of typed keys that `.` replays |
| `history.rs` | undo/redo steps as edit records, and the caret rule |
| `register.rs` | the unnamed register (tier 2: `"0`, `"+`) |
| `class.rs` | Vim's character classes for words (§3.13) |
| `search.rs` | tier 2, second wave: pattern translation, the search motion, and `matches` for the highlight (§3.14) |

`components/word_nav.rs` (the arrows profile's ctrl+arrow word hops) is not
changed. The engine does not use it (§3.13).

### 3.2 Positions and the buffer abstraction

`Pos { row: usize, col: usize }` counts chars, never bytes. A one-line
buffer always has `row == 0`. Every motion, object and operator works on
`Pos` and on lines of `char`s. This generalises field.rs's rule that every
offset is a char index.

```rust
pub trait TextBuf {
    /// false for a one-line field: `splice` never receives '\n'.
    const MULTILINE: bool;
    fn line_count(&self) -> usize;            // an empty field is one empty line
    fn line(&self, row: usize) -> Cow<'_, [char]>;
    fn cursor(&self) -> Pos;
    fn set_cursor(&mut self, at: Pos);        // raw: no clamping, no side effects
    /// The one mutation. Replaces [start, end) with `text`. `end` may be
    /// (row + 1, 0) to take a line break; `text` may contain '\n'
    /// (MULTILINE only).
    fn splice(&mut self, start: Pos, end: Pos, text: &str);
    /// Paint only: the mode shape (block/bar caret) and the Visual span.
    fn show(&mut self, paint: Paint);
    /// Rows scrolled off the top (the body; always 0 for one-line fields).
    fn top(&self) -> usize;
    fn set_top(&mut self, row: usize);
}
```

Every text change goes through `splice`, so recording undo and `.` happens
in one place, the engine (§3.11). The engine keeps the Visual anchor and the
mode itself. The buffer's selection is paint only. The one exception is a
selection made by the mouse or a GUI key, which `settle` adopts (§4.2).

### 3.3 The two buffers

**`OneLineBuf<'a>(&'a mut LineInput)`**: `line(0)` collects the chars.
Fields are short, so no cache is needed. `splice` calls a new
`LineInput::splice_raw(start, end, &str)` that changes text and caret and
records nothing in LineInput's own undo stack. `show` calls
`LineInput::set_visual(Option<VisualShape>)`. `VisualShape` and
`paint_span()` are copied from vim-mode's `line_input.rs`, so paint and
action can never disagree. vim-mode's `edit_span`, `SpanEdit`,
`insert_pinned`, `record_pinned` and `Snapshot.pin` are not copied, because
the engine's history replaces them (§3.11). An arrows-profile test pins that
`LineInput`'s own behaviour is unchanged.

**`BodyBuf<'a>(&'a mut EditorState)`** uses only edtui's public surface:

- the fields `lines` (`Lines = Jagged<char>`, where each row is a
  `Vec<char>` reached with `get_mut(RowIndex)`), `cursor` (`Index2`),
  `mode`, and `selection` (`Selection { start, end, line_mode, anchor }`,
  whose end is inclusive, like Vim's)
- `viewport_offset()` and `set_viewport_offset()`

`splice` edits rows in place: it splits, truncates, extends, and inserts or
removes rows. `show` sets `state.mode` to Normal, Insert or Visual (edtui's
renderer clamps a Normal caret to `len-1` itself) and `state.selection` for
Visual. The body never uses `execute`, `undo`, `redo`,
`EditorEventHandler` or edtui's clipboard in the vim profile. The plan
checks the exact `Jagged` row-removal call against edtui-jagged 0.1.13. If
none fits, `splice` rebuilds the affected rows.

### 3.4 Pinned settings

These are the engine's fixed behaviour, and the oracle runs Vim with the
same values. They live in `settings.rs`. `SETTINGS_LINE` is the exact
string the generator passes to `:set`, and the golden file's header repeats
it (§6.4).

| Setting | Why |
|---|---|
| `expandtab shiftwidth=2 autoindent selection=inclusive` | key list (`mine`) |
| `tabstop=2 softtabstop=0` | Insert `Tab` and `>>` produce 2-space steps; a literal tab counts as 2 columns for indent maths (`mine`) |
| `backspace=indent,eol,start` | Insert `BS`, `ctrl+w` and `ctrl+u` can cross the indent, line breaks and the insert start, as every vim user's vimrc allows (`mine`) |
| `whichwrap=b,s` | Vim's default: `BS`/`Space` wrap in Normal, `h`/`l` do not (`mine`) |
| `startofline` | `gg`, `G`, `dd`, `>>` land on the first non-blank (key list) |
| `nojoinspaces` | `J` adds one space after `.` (`mine`) |
| `nrformats=bin,hex` | `007` + `ctrl+a` is `008`, not octal (`mine`) |
| `noshiftround nosmartindent nocindent textwidth=0 noignorecase notildeop matchpairs=(:),{:},[:] iskeyword=@,48-57,_,192-255` | Vim defaults, pinned so a distro vimrc can't change them |

These are for the harness only and do not affect engine behaviour:
`noesckeys notimeout ttimeout ttimeoutlen=0 nomore scrolloff=0 lines=24
columns=80 nowrap`.

### 3.5 Key grammar and state machine

vim-mode used a `Pending` enum with seven variants plus separate count
fields. Here one parser struct replaces it (`mine`):

```rust
struct Pending {
    reg: Option<char>,          // after `"x`
    count1: usize,              // before the operator; 0 = none
    op: Option<Op>,             // d c y g~ gu gU > <
    count2: usize,              // after the operator
    prefix: Option<Prefix>,     // g, z (body, tier 2), " (register), i/a (object), f/t/F/T, r
}
```

Each key either extends `Pending` or completes a `Cmd`. `Cmd` is a plain
value with no borrowed state:

```rust
enum Cmd {
    Move { motion: Motion, count: Count },
    Operate { op: Op, target: Target, count: Count, reg: Reg },   // Target = Motion | Object | Line (doubled)
    Visual { op: VisualOp, reg: Reg },                              // d c y x p P r J ~ u U > < on the live selection
    Edit { what: Simple, count: Count, reg: Reg },                  // x X D C Y s S p P r{c} J ~ ctrl+a ctrl+x
    Enter { how: InsertHow, count: Count },                         // i a I A o O gi R, and c-family openings
    Undo(Count), Redo(Count), Repeat(Count), VisualStart(Shape), Gv,
}
```

The echo string comes straight from `Pending`: register, count1, operator,
count2, then prefix (for example `"02d3`, `gU`, `di`, `f`, `r`). This is
field.rs's echo format extended to cover the register (`earlier` for the
footer echo). Counts cap at `MAX_COUNT = 9_999` with vim-mode's saturating
`accumulate_count` (copied). A leading `0` is the motion `0` until a count
has started. Two counts multiply, and the product is capped
(`combine_counts`, copied). A modified chord that is not the engine's own
(§4.3) clears `Pending` and is declined. This is field.rs's rule, which
keeps `ctrl+c` reaching the app.

Executing a `Cmd` is separate from parsing it. That is what makes `.`
exact: `.` stores the `Cmd` plus the insert record and runs them again
(§3.12).

### 3.6 Motions, ranges and operators

A motion returns `MotionResult { to: Pos, kind: Exclusive | Inclusive |
Linewise, failed: bool }`. A failed motion (a find with no match, `k` on row
0) leaves the caret where it was and cancels any operator waiting for it. A
counted find fails entirely rather than moving part of the way (copied from
field.rs). `op.rs` turns a motion or object into `Range { start, end, kind:
Char | Line }` using Vim's operator rules. Each rule has corpus cases:

1. **Exclusive at column 0.** If an exclusive motion ends at column 0, the
   end moves to the end of the previous line and becomes inclusive. If the
   start is also at or before the first non-blank, the range becomes
   linewise (`:help exclusive`).
2. **`w` inside an operator.** When the last word moved over ends a line,
   the range stops at that line's end instead of the next line's first word
   (`dw` on the last word never joins lines).
3. **`cw`/`cW` on a non-blank.** These act like `ce`/`cE`. The first
   repetition does not skip ahead when the caret is already on a word end
   (field.rs's `change_word_span`, copied and generalised).
4. **Counts past the end.** `5j` and `5dd` clamp to the last line, while
   `3x` and `5D` clamp to the end of the line. `{N}r{c}` fails when fewer
   than N chars remain (copied rule).
5. **Where the caret lands after an operator.** `d` and `c` leave it at the
   range start. `y` leaves it at the range start, and a linewise yank never
   moves it. `g~`, `gu` and `gU` leave it at the start. `>` and `<` leave it
   on the first non-blank of the first line. A Visual operator lands at the
   selection start. The one-line special cases `dd`, `cc` and `yy` take the
   whole field (`earlier`) and fall out of the linewise rule naturally.
6. **Linewise put.** `p` puts below and `P` above, with the caret on the
   first non-blank of the first new line. A charwise put rests on the last
   char put. A counted put repeats the text. The golden file decides any
   case these sentences get wrong.

The word motions (`w b e W B E ge gE`) cross lines and treat an empty line
as a word, as Vim does. `h` and `l` never cross lines; `BS` and `Space` do
(`whichwrap=b,s`). `%` works with the three bracket pairs, counts nesting, searches
forward on the line when the caret is not on a bracket, and crosses lines.

### 3.7 Text objects

- **`iw` `aw` `iW` `aW`, with counts.** field.rs's one-line `iw`/`aw` is
  generalised. `aw` takes trailing blanks, or leading blanks when there are
  none. A count takes more words.
- **`i"` `a"` `i'` `a'`.** These work within one line, as Vim's do. Quote
  pairs are counted from the start of the line, and `\` escapes a quote.
  When the caret is before the first quote, the next quoted string is used.
  `a"` takes trailing white space, or leading white space when there is no
  trailing. The case where the caret sits between two strings
  (`"a": "b"` with the caret on `:`) is a corpus trap, and the golden file
  decides it.
- **`i(` `a(` `ib` `ab`, `i[` `a[`, `i{` `a{` `iB` `aB`, with counts.**
  These search outward for the enclosing pair, counting nesting across
  lines. A bracket inside a quoted string counts exactly as Vim counts it
  (the `quotes` and `brackets_deep` texts pin this). A count selects outer blocks (`2i{`). For a
  multi-line inner block whose opening bracket ends its line and whose
  closing bracket starts its line after indent, the inner range is linewise
  over the lines between. This is why `di{` on pretty JSON leaves `{` and
  `}` on their own lines, and why `ci{` leaves one empty line indented by
  `autoindent`. This is the command that matters most for JSON, so the
  corpus runs every bracket object on both `json_flat` and `json_pretty` at
  every nesting depth.
- **`ip` `ap`** (tier 2): paragraphs separated by blank lines, as Vim
  defines them (`you`, 2026-09-30, accepting my recommendation). `ip` and
  `ap` treat a blank-only line as blank. `{` and `}` stop only at an empty
  line, a form feed, or an nroff macro line (`.PP`, `.SH`: Vim's default
  `paragraphs` and `sections`, pinned in `SETTINGS_LINE`).

### 3.8 Insert and Replace sessions

Entering Insert opens an `InsertSession { start: Pos, origin: Cmd, typed:
Vec<InsertKey>, step: StepId }`. The engine types into the buffer itself
through `splice`. `LineInput::handle_key` is never called in the vim profile,
so every change is recorded.

| Key | What it does |
|---|---|
| printable chars, `Space`, bracketed paste | typed into the buffer and into `typed` |
| `Enter` | body: a line break, copying the indent (`autoindent`). One-line: declined (§4.3) |
| `BS`, `ctrl+h` | delete back, joining lines at column 0 (`backspace=…eol`) |
| `Del` | delete forward, joining at the end of the line |
| `ctrl+w`, `ctrl+u` | Vim's word-back and delete-to-start, including the "stop once at the insert start" rule |
| `Tab` | body: spaces up to the next multiple of 2 (`mine`). One-line: declined |
| `Left` `Right` `Home` `End` (and `Up` `Down` in the body) | move, and close the undo step and the `.` record as Vim does; typing after the move starts a new record |
| `Esc`, `ctrl+[` | leave: the caret steps back one unless at column 0. An indent that `autoindent` added and nothing followed is removed. A counted insert (`3ia<Esc>`, tier 2) repeats `typed` |
| `ctrl+o {cmd}` (tier 2) | one Normal command, then back to Insert (Replace after `R`). Mode `InsertNormal { replace }`. Ends the undo step and the `.` record as Esc does; the caret goes back past the end where it was; `.` inside it repeats the change before the insert; a key the engine declines resumes Insert first (plan 3c Deviation 8) |
| `ctrl+r {reg}` (tier 2) | insert register text as typed |
| `ctrl+t` `ctrl+d` (tier 2) | indent and outdent to the next multiple of `shiftwidth`, in one-line fields too (plan 3c Deviation 11, after 3b's `>>` ruling; confirmed by the user 2026-10-01). `0<C-d>` and `^<C-d>` remove the indent |

`R` (tier 2) runs the same session in Replace: typing overwrites, and `BS`
restores the original chars. `c`, `s`, `S`, `C`, `cc`, `o`, `O`, `A`, `I`,
`a` and `i` are `Cmd`s that open a session, and the change they made plus
the typing is one undo step.

The Replace session keeps Vim's replace stack, so `BS` restores what was typed over, and before the session's start it only moves the caret (plan 3c Deviation 10).

### 3.9 Visual mode

The engine holds `Visual { anchor: Pos, shape: Char | Line }`, with the
caret as the moving end. `v`, `V` and `o` work as in Vim (`v` inside `V`
switches shape, and pressing the same key again leaves Visual, as field.rs
does). Motions extend the selection, and objects (`viw`, `vi{`) replace it.
Selections are inclusive (`selection=inclusive`), so `v$` includes the line
break and `v$d` joins lines (corpus trap). Visual `d c y x p P r J ~ u U > <`
act on the selection and return to Normal. Visual `p` puts the text it
replaced into the unnamed register, and `P` does not (Vim 9.1). `gv` (tier
2) reselects `BufState.last_visual`. `ctrl+v` block Visual is Out and
`ctrl+v` stays paste (`earlier`).

### 3.10 Registers

`Register { text: String, kind: Char | Line }`. A linewise register's text
ends with `\n`, as `getreg()` reports it, so the golden comparison is direct.
There is one unnamed register, owned by the `Engine` and shared by every
field and the body (`mine`, key list §5). Tier 1 has only the unnamed
register. Tier 2 adds these:

- `"0`, the yank register.
- `"+`: `"+y…`, and `"+d…`/`"+c…` (a cut), return `AppRequest::CopyToClipboard(text)` and write the registers as the plain command does; the app copies it through its existing OSC 52 path. `"+p` and Insert `ctrl+r +` show the note "paste with your terminal (ctrl+v)" and change nothing, because the app cannot read the system clipboard (`you`: my recommendation, confirmed 2026-10-01 together with `"+d` copying; plan 3c Deviation 14).

`"a`–`"z` and the other registers are Out. `"x` for any register that is
not supported consumes both keys, shows the note `register "x not
supported`, and arms nothing. The keys that follow run as ordinary commands
(key list §9: "never arms a half-typed state").

Rules for one-line fields (`mine`, key list §5):

- A charwise register with line breaks is put with each `\n` replaced by a
  space. This is today's `flatten_paste` rule.
- A linewise register is put charwise after the caret without its trailing
  `\n`. This matches vim-mode's `capital_v_y_then_p_is_a_characterwise_put`
  (`earlier`).

### 3.11 Undo and redo

**What Vim does, which the engine matches (`you`, rule 1; `mine`, Vim's
granularity).** Each Normal change is one step. An Insert session together
with the command that opened it is one step, but cursor keys inside Insert
split it. `.` is one step. `u` and `ctrl+r` take counts. After undo or redo
the caret goes to the topmost changed line. If the step's saved caret was
on that line, the caret takes its column; otherwise it goes to the first
non-blank. This is Vim's `u_undoredo` rule, and the redo side is pinned by
corpus cases for `x`, `dw`, `dd`, `3dd`, `p`, `P`, `o`, `cw…<Esc>`, `J`,
`>>`, Visual `d` and multi-line `ci{`. vim-mode's idea of a caret "pinned"
to the start of the change survives as this rule.

**Store.** `history.rs` holds `Step { edits: Vec<Edit>, caret_before: Pos,
caret_after: Pos }`, where `Edit { at: Pos, removed: String, inserted:
String }` is recorded by the engine's `splice` wrapper. Undo applies the
inverse edits in reverse order, and redo applies them forward. A new change
clears the redo branch. The cap is 1000 steps, Vim's `undolevels`. Each step
stores only the changed text, never a snapshot of the whole buffer. The
history lives in `BufState`, which the app keeps per buffer (§4.1). The
same code serves both buffers, and the same golden cases test both.

**Options for the body.** edtui's `capture()` is `pub(crate)`, so the body
cannot use edtui's stack cleanly:

| Option | For | Against |
|---|---|---|
| **A. The engine owns the body's undo in the vim profile** (the store above) | Vim's exact granularity and caret, on the same code the conformance tests run on both buffers. No edtui patch. Undo records are sized to the change | The vim-profile body undo is a different store from the arrows profile's (arrows keeps app-history steps coalesced over 2 s; that path is untouched) |
| B. Vendor edtui with a one-line patch making `capture()` public (`[patch.crates-io]`) | Small diff | We carry a vendored crate through every edtui upgrade. edtui's stack still restores the caret from capture time and redoes to the caret at undo time. `capture()` never clears the redo branch (`state/undo.rs`), so redo after a new edit brings back a stale state. The cap is 100 snapshots of the whole buffer. We would have to fix the caret after every undo, which is exactly the piecemeal overriding you want to avoid |
| C. vim-mode's `execute(RemoveChar(0))` trick, which snapshots as a side effect | No vendoring | Relies on an undocumented side effect. `RemoveChar` also calls `clamp_column`, which pulls an Insert caret at the end of the line back onto the last char. Same caret and redo defects as B. Breaks silently on an upgrade |

Recommendation (`mine`): **A**. In the vim profile edtui stores and draws,
and the engine owns vim semantics, undo included. This narrows the accepted
direction ("edtui for buffer/render/undo"). The user accepted A on
2026-09-27 (open question 1).

**How this meets the app history** (a requirement on piece 4,
recommendation `mine`, accepted 2026-09-27 as open question 2). The engine's history answers `u`
and `ctrl+r` while its buffer has the caret. When it has nothing left, `u`
is declined and piece 4 passes it to the app history, as `LineInput::edited`
gates it today. The app records one `EditorDelta` per buffer session, from
the text at the session start to the text at its end, as the one-line field
gate does today. The body's per-key app-history recording is suppressed
while an engine session is live.

`BufState` exposes `edited()`, `text_at_start()` and `end_session()` with
the same meanings as `LineInput`'s. That lets piece 4 switch the field
queries to it in one place (the review's "one `FieldSession`").

For the body, I recommend keeping the `BufState` for as long as the request
stays open, so `u` after a trip to the response pane still undoes the last
small change. Two things drop it: a request switch, and re-entry when the
text no longer matches the text the history ends at (after an app undo, a
reload, a format, or an `$EDITOR` round trip). One-line fields get a fresh
`BufState` each time they open. This stays consistent: undoing a step inside
a later session is just an edit in that session, so the app history's net
deltas still undo in order.

### 3.12 `.` repeat

`Engine.dot: Option<Dot { cmd: Cmd, insert: Option<Vec<InsertKey>> }>` is
set by every command that changes text. Visual commands record the size of
the selection: its line count when linewise, and its line count plus
last-line width when charwise. `.` runs the command again from the current
caret over the same size, as Vim does. Moves, yanks, undo and redo don't
set it. `{N}.` replaces the stored count. Vim stores `2d3w` with its
combined count 6, so `4.` deletes four words; corpus cases pin this. A register named in the command is reused. `.` is
one undo step. `.` is global across buffers (`mine`, as in Vim), so `cwfoo`
in the URL followed by `.` in a cell repeats there.

### 3.13 Word classes and Unicode

Vim's word motions use `iskeyword=@,48-57,_,192-255` for chars below 256
and `utf_class()` (mbyte.c) above. NBSP counts as blank, all of 192–255
count as word chars (including `×` and `÷`), CJK ideographs form their own
class separate from Latin, and emoji have their own class. `class.rs` copies
that table as a static range list (`mine`). `word_nav::is_word`
(`is_alphanumeric() || '_'`) disagrees at those edges, which is why the
engine doesn't reuse it the way field.rs did. The corpus covers `é`, `ö`,
`×`, `日本語`, `😀` and NBSP. Columns are chars, as Vim's `charcol` counts
them. Combining characters and display width are drawing concerns and are
not the engine's.

### 3.14 Search (tier 2, second wave)

**Who:** the user decided on 2026-09-27 that body search is tier 2,
second wave, by accepting my recommendation (open question 4). The details
below are `mine` unless marked otherwise. The pattern language was open
question 5 (decided: the subset below).

**Keys.**

- `/` and `?` type a pattern, and `Enter` runs the search.
- `n` and `N` repeat it.
- `*` and `#` search for the keyword under the caret as a whole word
  (`\<word\>`).
- Counts work (`3n`, `2/foo<CR>`).
- A search is a motion: it follows an operator (`d/foo<CR>`, `c?bar<CR>`,
  `yn`), and it extends a Visual selection.
- Search offsets (`/foo/e`), `gn`/`gN`, `g*`/`g#` and search history are
  Out for this release.

**Motion kind.** A search is charwise and exclusive (`:help /`), so the
exclusive-at-column-0 rule (§3.6 rule 1) applies after an operator. A
search that finds nothing is a failed motion (§3.6): the caret stays, and
a waiting operator is cancelled.

**Typing the pattern.** `/` and `?` enter a nested mode,
`Mode::Search { dir, typed }`, much as Insert `ctrl+o` enters
`InsertNormal`. The engine owns the pattern text so that a waiting operator
survives while it is typed.

- `Engine::search_line()` returns `/` or `?` followed by the pattern and
  the caret position in it. Piece 4 draws it as a prompt on the bottom
  row of the body pane, as accepted in decision 4. It is not the `:`
  palette, which is an overlay with result rows. `echo()` also shows
  the waiting operator and count (`d2/`).
- Printable chars and paste type into the pattern.
- `BS` deletes back. `BS` on an empty pattern cancels, as in Vim.
- The other prompt keys (plan 3c Deviation 16, each probed in Vim's command
  line):
  - `Tab` types a literal tab.
  - `ctrl+h` is `BS`.
  - `Del` deletes under the caret; at the end it deletes back; on an empty
    pattern it cancels.
  - `ctrl+w` deletes a word back by Vim's command-line rule (blanks, then
    one char class) and never cancels.
  - `ctrl+u` deletes back to the start.
  - `Left`, `Right`, `Home` and `End` move in the pattern.
  - `Up` and `Down` are inert (no search history).
  - `ctrl+r` is inert, with the note `ctrl+r not supported in a search`.
  - `ctrl+[` is `Esc`.
  - Any other chord cancels the prompt (the waiting operator with it) and
    is declined to the app, field.rs's chord rule.
- An offset after an unescaped `/` (or `?`) is refused with "search offsets not supported"; the pattern before it is still saved (plan 3c Deviation 3).
- `Esc` cancels: the operator is dropped and the caret does not move.
- `Enter` runs the search. An empty pattern reuses the last one, as in Vim.
- While the pattern is typed, the caret does not move, and the match it
  would land on is painted (Vim's `incsearch`, drawing only).

**Pinned behaviour.** These are added to `SETTINGS_LINE` (§3.4):
`wrapscan magic noignorecase nosmartcase`, which are Vim's defaults.

- A search that wraps shows the note `search hit BOTTOM, continuing at TOP`
  (or TOP/BOTTOM for `?`), Vim's wording.
- A miss shows `Pattern not found: {pattern}`.
- The last pattern and its direction are `Engine` state, global as in Vim,
  so `n` in a one-line field reuses a search made in the body. On a
  one-line buffer, search runs within its single line.
- `n` with no previous pattern: "No previous regular expression"; `*` with nothing under or after the caret: "No string under cursor". These are `Note::Message`, a second variant beside `Note::Unsupported` (plan 3c Deviation 6).

**Highlighting.** After a search, every match in the body is painted with a
search highlight. The highlight stays until `:noh`, a new search, or a
request switch. The engine exposes `last_search()`, `hlsearch()`, `no_hlsearch()`, `search_matches(&buf, rows)` (the live pattern: the prompt's text while open, else the last pattern) and `search_preview(&buf)` (the match Enter would land on; plan 3c Deviation 18) so that the renderer paints exactly what `n`
would visit. Piece 4 moves `:noh`/`:nohlsearch` from its "runs nothing"
list to a supported verb.

**Pattern language** (open question 5; my recommendation, accepted
2026-09-27): a translated subset of Vim's `magic` syntax, compiled with
the `regex` crate.
`regex` 1.13 is already in `Cargo.lock` as a transitive dependency, so this
adds a direct dependency but no new crate. The subset is:

- literal chars;
- `.` `*` `^` `$` and `[…]` (including ranges and `^` negation);
- `\+` `\=` `\?` and `\{n,m}`;
- `\(…\)` and `\|`;
- `\s` `\S` `\d` `\D` `\w` `\W`;
- `\<` `\>`;
- `\` before any of these to make it literal.

`\<` and `\>` are matched exactly as Vim's `BOW`/`EOW` (character classes, so CJK and emoji boundaries count): the line is matched with a sentinel char before every word start and word end and the atoms translate to those chars, because `regex`'s `\b` defines a word differently and a check after the fact cannot reproduce Vim's backtracking (plan 3c Deviation 4).
Anything outside the subset (`\v`, `\zs`, `\%`, `~`, `\{-}`,
and so on) shows `pattern not supported: {atom}`, and nothing is searched.
An unsupported pattern is still saved as the last pattern, as in Vim (Deviation 5).
A backslash before a char that has no entry in Vim's `META_flags` table
(`\q`, `\j`, and so on), and a trailing backslash, are literal, as Vim's
`peekchr()` reads them (plan 3c Task 11, pinned by `search/patterns`).
`\n` inside `[…]` (a collection that would match a line break) is refused
with `pattern not supported: \n`.
The engine never searches for something other than what was typed.

`history.rs` is untouched: a search changes no text. `.` does not repeat a
search on its own, but `d/foo<CR>` followed by `.` repeats the delete with
the same pattern, as Vim does. `Dot` stores the motion, including its
pattern.

## 4. The engine ↔ app API

### 4.1 Types

```rust
pub struct Engine { /* mode, Pending, Visual, InsertSession, registers, dot, last_find */ }
pub struct BufState { /* history, last_visual (gv), last_insert (gi) */ }
pub struct Target<'a, B: TextBuf> { pub buf: &'a mut B, pub state: &'a mut BufState }

/// Named `ViewCtx`, not `KeyCtx`: piece 4's router has its own `KeyCtx`.
pub struct ViewCtx {
    /// Rows the body shows, for ctrl+d/u/f/b, H/M/L, zz/zt/zb.
    /// None for a one-line field, or a body that has not been drawn yet
    /// (then the whole text is the window, Deviation 12).
    pub viewport_rows: Option<usize>,
}

pub enum Outcome {
    /// The engine used the key: an edit, a move, a mode change, or one more
    /// key of a half-typed command.
    Consumed { changed: bool, note: Option<Note>, request: Option<AppRequest> },
    /// Not the engine's. `keys` holds every key the engine swallowed for it,
    /// the declined key included (e.g. [g, t]), and `count` is the count
    /// typed before them. Engine state is as if those keys were never typed.
    Declined { count: Option<usize>, keys: Vec<KeyEvent> },
}
pub enum Note { Unsupported(String), Message(String) }  // footer text, e.g. `register "a not supported`
pub enum AppRequest { CopyToClipboard(String) }      // "+y (tier 2)

pub enum Mode { Normal, Insert, Replace, InsertNormal { replace: bool }, Visual(Shape), Search(Dir) }
/// The open search prompt (§3.14): its direction, its text and the caret in it.
pub struct SearchLine { pub dir: Dir, pub text: String, pub cursor: usize }

impl Engine {
    pub fn handle<B: TextBuf>(&mut self, key: KeyEvent, t: Target<'_, B>, ctx: &ViewCtx) -> Outcome;
    pub fn paste<B: TextBuf>(&mut self, text: &str, t: Target<'_, B>) -> Outcome;
    pub fn mode(&self) -> Mode;
    pub fn echo(&self) -> String;         // "" when nothing is half-typed
    pub fn pending(&self) -> bool;        // a count, register, operator or prefix is in flight
    pub fn registers(&self) -> &Registers;
    pub fn search_line(&self) -> Option<SearchLine>;
}
impl BufState {
    pub fn can_undo(&self) -> bool; pub fn can_redo(&self) -> bool;
    pub fn edited(&self) -> bool; pub fn text_at_start(&self) -> Option<&str>;
    pub fn end_session(&mut self);
}
```

`Engine` holds the state that is global in Vim: the mode, the half-typed
command, the registers, `.`, and the last `f`/`t`. The mode is global
because only one buffer has the caret at a time. `BufState` holds what Vim
keeps per buffer. The app owns one `Engine` and one `BufState` per live
buffer.

### 4.2 Session edges

These come from field.rs's `open`, `carry`, `settle` and `close`, which were
reviewed as sound. Piece 4 calls them. The engine never decides when they
happen.

- **`enter(start: Start, seat: Seat, t)`** is called when a buffer gets the
  caret. `Start::Normal` is for a buffer entered by keyboard.
  `Start::Insert` is for a buffer created by an add verb, or a prompt; Esc
  then goes to Normal and a second Esc is declined. `Start::InsertOnly` is
  for every query box: the `:` command line, the palette query, the jq
  bar, the response search bar, the `{{` variable picker filter, the
  chooser filters and the file-picker filter (`you`, 2026-09-28: these go
  through the engine, not a second text path). There is no Normal layer,
  the Insert keys work, and Esc is declined. Which surface
  gets which is piece 4's rule (`earlier`). `Seat` is one of `Keep`,
  `ColZero`, `End` or `FirstNonBlank`, as in field.rs. A session opened in
  Insert records as an `i` for `.` (`mine`), but only once something is
  typed: Esc alone keeps the old `.`, as after Vim's `:startinsert` (`you`,
  2026-09-30: "match vim"; checked against Vim 9.1).
- **`carry(t)`**: the caret moves straight from one buffer to another in the
  same event (Tab to the next cell). Insert and Normal carry over. Visual,
  pending keys and the insert record are dropped (field.rs, `earlier`).
- **`settle(t, how: Settled)`** runs after anything the engine did not
  handle: a click, a mouse sweep, a key piece 4 handled. A Normal caret
  never rests past the last char. A selection the mouse or a GUI key made
  is adopted as Visual with Vim's inclusive shape (field.rs's
  `adopt_selection`). `Settled` is `Key`, `Click`, `Sweep` or `Release`, as
  in field.rs.
- **`leave(t)`**: the buffer loses the caret. An open Insert session ends
  as one undo step (the caret does not step back), Visual and pending keys
  are dropped, and the mode becomes Normal.
- **`external_edit(t, f)`** is for a change made outside the key path:
  inserting a picked `{{token}}`, format or minify, paste in Normal. `f`
  edits through `splice`, and the whole call becomes one undo step. It is
  not repeatable with `.`. Paste in Insert goes through `paste()` instead
  and becomes part of the insert session.

### 4.3 What the engine declines

The engine returns `Declined` for these keys. Anything not listed and not
the engine's own is consumed with no effect in Normal and Visual (Normal
never types). Where a vim user expects something, it also shows a
`Note::Unsupported`: `U`, `K`, `Q`, `&`, `gJ`, and the operator `d:`.
`g*`, `g#`, `gn`, `gN` show "g* not supported" and so on (plan 3c Deviation 15).
`z` reads a count after it (`z5`, as Vim's `nv_zet()` does) and then
swallows the next key: `zz`, `zt` and `zb` act, anything else is inert
(plan 3c Task 7), so `zo` and friends never reach the app from a text
buffer.

| Key | When | Why |
|---|---|---|
| `Esc` | Normal with nothing pending, or InsertOnly | piece 4 closes or cancels the field (`earlier`) |
| `:` | Normal | the ex line |
| `j` `k` `Down` `Up`, `ctrl+d` `ctrl+u` `ctrl+f` `ctrl+b` (with count) | Normal in a one-line buffer | piece 4 leaves the field and moves rows (`you`: accepted decision 2). In Visual on a one-line buffer these are consumed as failed motions and the selection stays (`mine`) |
| `Enter`, `Tab`, `BackTab` | one-line buffer, any mode | the app confirms or moves to the next field |
| `u`, `ctrl+r` (with count) | Normal, `BufState` has nothing to undo or redo | the app history (§3.11) |
| `g` + a key that doesn't complete an engine `g` command (`gt`, `gT`) | Normal | piece 4's tab verbs. `keys = [g, t]` with the count |
| `Z`, `q`, `@`, `m`, `'`, `` ` `` | Normal | piece 4's `ZZ`/`ZQ` and its notes for macros and marks (key list §9) |
| any ctrl/alt/super chord that is not the engine's | any mode | app bindings. The engine's own chords: Normal `ctrl+r` `ctrl+a` `ctrl+x` (plus `ctrl+d/u/f/b` in the body, in Normal and Visual; with an operator pending they cancel it); Insert `ctrl+w` `ctrl+u` `ctrl+h` `ctrl+[` `ctrl+o` `ctrl+r` `ctrl+t` `ctrl+d`; in the search prompt `ctrl+w` `ctrl+u` `ctrl+h` `ctrl+[`, and `ctrl+r` (inert, with its note; §3.14) (any other chord cancels the prompt and is declined) |
| `ctrl+o` | `Start::InsertOnly` | a query box has no Normal layer |

Keys that are Out and are not listed above, such as `ctrl+v` or `ctrl+o` in
Normal, fall under the last row and reach the app with their existing
meaning (`earlier`: `ctrl+v` paste, `ctrl+o` project chooser).

### 4.4 What piece 4 must provide

Piece 4 must:

- Call `handle` with every key while a buffer has the caret, unless its
  own sequence is in flight (`ZZ`, the `ctrl+w` pane chord).
- Route `Declined` keys exactly as if no engine were there, applying
  `count`.
- Show `echo()` and `mode()` in the footer, and show `Note`s.
- Carry out `AppRequest`.
- Pass `ViewCtx.viewport_rows` from the body's last drawn area.
- Call the session edges (§4.2).
- Suppress the body's per-key app-history capture while an engine session
  is live (§3.11).
- Enumerate the buffers once, through its `FieldId`.
- Tier 2, second wave: draw `search_line()` as a prompt on the body pane's
  bottom row, paint `matches` as the search highlight, and make `:noh`
  clear it (§3.14).

## 5. Key coverage by tier

Rows refer to the key list's section and row. "Ships" says when a row
passes the conformance test (`mine`).

| Key list rows | Tier | Engine home | Ships |
|---|---|---|---|
| §1 `h j k l`/arrows, `w b e W B E`, `0 ^ $`, `gg G {N}G`, `f F t T ; ,`, `%`, counts | 1 | motion.rs | first release |
| §2 operator grammar, `dd cc yy D C x X s S Y p P r {N}r J u ctrl+r .`, counts on operators | 1 | keys.rs, op.rs, history.rs | first release |
| §3 `iw aw iW aW`, quote objects, bracket objects (nesting, multi-line) | 1 | object.rs | first release |
| §4 `i a I A o O`, `Esc`, Insert `BS ctrl+w ctrl+u`, `v V o`, Visual operators | 1 | insert.rs, mod.rs | first release |
| §5 one-line rules (`o O J` no-op, whole-field `dd`, shared register, joined paste) | 1 | buf.rs, register.rs | first release (S tests) |
| §1 `ge gE`, `{ }`; §2 `~ g~ gu gU`, `>> << > <`, `ctrl+a ctrl+x`; §3 `ip ap`; §4 counted inserts, `gv`, `gi`, Insert `ctrl+r`; §9 `"0` | 2 | same modules | **first wave**, before piece 4 ships: each is a small addition to grammar that already exists |
| §2 `R`; §4 Insert `ctrl+o`, `ctrl+t ctrl+d`; §1 `ctrl+d ctrl+u ctrl+f ctrl+b`, `H M L`, `zz zt zb`; §9 `"+` | 2 | insert.rs, motion.rs, register.rs | **shipped** (plan 3c) |
| §1 search `/ ? n N * #`, as a motion and after operators (§3.14) | 2 | search.rs, keys.rs | **shipped** (plan 3c) |
| §2 `gJ`; §3 `it at is as`; §4 `ctrl+v` block; named registers | Out | §4.3 | never |

Beyond the key list, the Normal motions `+`, `-` and `Enter` in the body
(tier 2, `mine`) are cheap and complete the line motions. `Space` and `BS`
follow `whichwrap=b,s`.

The key list marks `>> << > <`, `{ }` and `ip ap` "body". They act as
Vim in one-line fields too (`you`, 2026-09-30, accepting my
recommendation): `>>` indents the field, `}` goes to its end, and `dip`
empties it, as 3a's Visual `>` and `<` already did.

## 6. Conformance harness

### 6.1 Files

| Path | What |
|---|---|
| `scripts/vim_oracle/corpus.toml` | the hand-written case matrix (§6.2) |
| `scripts/vim_oracle/generate.py` | expands the corpus, runs Vim, writes the golden file |
| `scripts/vim_oracle/oracle.vim` | the per-case runner inside Vim (§6.3) |
| `crates/postui/tests/vim_conformance/golden.jsonl` | generated and committed: one header line, then one case per line, sorted by id |
| `crates/postui/tests/vim_conformance/divergences.toml` | the known, intentional differences (§6.6) |
| `crates/postui/tests/vim_conformance.rs` | the `cargo test` (§6.5). It reads both files with `include_str!`, so no `std::fs` is needed and the lint stays clean |

Python follows `scripts/tty_sweep.py`'s precedent. It needs 3.11+ for
`tomllib`.

### 6.2 Corpus format

```toml
[text.json_pretty]
lines = ['{', '  "a": {"b": 1},', '  "c": [1, 2, 3],', '  "d": "hello world"', '}']

[[group]]
id = "objects/brace"            # stable; becomes the case-id prefix
tier = 1
status = "ship"                 # "ship" | "later" (generated, reported, not failing)
keys = ["di{", "da{", "ci{X<Esc>", "2di{", "vi{d", "di{u", "di{u<C-r>"]
on = ["json_flat", "json_pretty", "brackets_deep"]
at = ["each"]                   # seats: bol fnb mid eol each, "empty", or [row, col] (1-based chars)
reg = { text = "q\n", type = "V" }   # optional register preset
```

Each group expands to keys × texts × seats. Case ids look like
`objects/brace/json_pretty@2:5/di{`. The `each` seat puts the caret on every
char of every line, and the generator refuses it on texts longer than 12
lines. Keys use Vim notation (`<Esc> <CR> <BS> <Del> <Tab> <C-w> <C-u> <C-r>
<C-a> <C-x> <C-o> <Left> <Right> <Up> <Down> <Home> <End> <lt>`). The
generator and the Rust test share one list of names.

Fixed texts:

- `words`
- `punct` (`foo.bar(baz, qux);`)
- `url` (a query-string URL)
- `json_flat`
- `json_pretty` (nested object and array, 2-space indent)
- `json_gaps` (with blank lines, for `{ } ip`)
- `brackets_deep` (3 levels, mixed `([{`)
- `quotes` (escaped quotes, an empty `""`, two strings on one line)
- `unicode` (`héllo wörld 日本語 😀 x×y` plus an NBSP)
- `blank_edges` (`["", "a", "", "  b  ", ""]`)
- `one_char` (`["x"]`)
- `empty` (`[""]`)
- `numbers` (`id: 41, n: -7, hex: 0x1f, pad: 007`)
- `tabs` (a tab-indented line)
- `long` (100 lines, numbered, for tier-2 scroll and search cases; records `top`)
- `lines25` (25 lines, a window and a bit; records `top`)
- `deep_indent` (6- and 3-space indents, for how Insert `ctrl+t ctrl+d`
  round to `shiftwidth`)
- `search` (repeated words, wide chars and punctuation, for `n N * #`)
- `search_question` (`?` and `\?` runs, for `#` and `N`)
- `search_cases` (the pattern-language edge cases: brackets, classes,
  multis, word boundaries)
- `search_brackets` (a delimiter inside `[…]`, Vim's `skip_regexp()`)

The corpus has one group per key-list row. Every tier-1 row runs on at
least `words`, `json_flat`, `json_pretty`, `unicode` and `blank_edges`, with
the caret at the start, the middle and the end, plus each edge seat. Every
operator runs with `w e b $ 0 ^ f t F T % iw aw i" i{ a{` and with a count.
Every change gets a `u`, a `u<C-r>` and a `.` variant. The corpus has
85,275 cases and the golden file is about 15 MB; the full conformance run
takes about 15 s in a debug build (the regex compiles per search case).

Lints the generator enforces:

- A case may not end with a command half-typed. The Rust side checks this
  too: if `pending()` is true at the end of a case, the case fails and must
  move to an S test, because the echo can't be read from Vim.
- `.` may appear only after a change in the same case.
- Keys may not contain `:`. `/` and `?` are allowed (the search groups,
  §3.14); a pattern must be ended by `<CR>` or `<Esc>`, which the
  half-typed rule above already enforces.

### 6.3 Generator: the proven recipe and its traps

Vim runs once for the whole corpus:

```
vim -Nu NONE -i NONE -n --not-a-term -c "set <SETTINGS_LINE> <harness settings>" -S oracle.vim
```

stdin, stdout and stderr go to `/dev/null`, and there is a timeout. For each
case, `oracle.vim` does the following:

- `enew!` with `buftype=nofile noswapfile`, then primes `.` with a change
  no corpus text can repeat (`normal! 0df☃` on the line `a☃`; trap 8).
- `setline(1, lines)`, with `set undolevels=-1` around it, which clears the
  history, so `u` cannot undo the setup (an undo break alone does not stop
  it).
- Resets the state that outlives a buffer: `setreg('"', '')`,
  `setreg('0', '')`, `setcharsearch({'char': ''})`, `setlocal scroll=0`,
  `let @/ = ''`, `let v:searchforward = 1` (trap 12) and `histdel('/')`
  (trap 14), and applies the optional register preset with
  `setreg('"', text, type)`.
- `cursor(row, 1)`, then `setcursorcharpos(row, col)`, then
  `let v:errmsg = ''`.
- `feedkeys(keys . "\<Cmd>call Capture()\<CR>\<C-\>\<C-n>", 'ntx')`, called
  plainly: no `try` and no `silent!` around it (traps 13 and 15).
- `setline(1, 'wiped')`, then `bwipeout!` (trap 11).

`Capture()` records everything from inside the final mode: `mode(1)`,
`getline(1, '$')`, the cursor (`line('.')`, `charcol('.')`),
`getcharpos('v')` when the mode is `v` or `V`, `getreg('"')`,
`getregtype('"')`, `line('w0')` (scroll cases only), and `v:errmsg` (kept
for diagnosis, not compared). Keys arrive in `<Name>` notation and the
runner converts each token inside Vim with `eval('"\<Name>"')`, so a
literal `"` or `\` in keys needs no escaping.

Traps (from the spike, plus the ones this design adds):

1. **Not `-Es`.** Ex mode breaks commands that take a char argument (`f`,
   `t`, `r`). Use `--not-a-term`.
2. **Not `normal!`.** It makes the whole string one undo block, and a
   failed motion aborts the rest. `feedkeys(…, 't')` gets typed semantics:
   one undo step per command, and a failure only beeps.
3. **`x` ends Insert.** With the `x` flag, feedkeys force-ends Insert and
   Visual after the keys, which steps the caret back. Every value,
   including the cursor, must be captured in the `<Cmd>`. The spike
   captured only mode and anchor there and read the cursor afterwards,
   which was wrong for any case ending in Insert.
4. **Settings after startup.** Settings go in `-c`, not `--cmd`. With
   `-u NONE`, `defaults.vim` is not loaded, so every value that matters is
   set explicitly (§3.4), including `backspace`, `startofline`,
   `nrformats` and `scrolloff`.
5. **Esc must be immediate.** `noesckeys notimeout ttimeout ttimeoutlen=0`,
   so an `<Esc>` followed by more keys is not read as a meta key.
6. **Window size.** `ctrl+d`, `H/M/L` and `zz` depend on the window size.
   Pin `lines=24 columns=80 nowrap scrolloff=0` and record `winheight(0)`
   in the golden header. The Rust test passes it as `viewport_rows`.
7. **Char columns, not bytes.** Use `setcursorcharpos` and `charcol` only.
8. **Per-case reset.** The register, the last find and `.` survive
   `bwipeout`. `.` can't be cleared, which is why `.` is linted to follow a
   change in the same case.
9. **Ending half-typed.** In a case ending in operator-pending, the
   trailing `<Cmd>` reaches the pending operator, so the result means
   nothing. The lint forbids such cases. The same goes for a case that
   ends while a search pattern is still being typed.
10. **A missed search does not flush the typeahead** (checked
    2026-09-28 on Vim 9.1.0016 with this recipe). E486 `Pattern not
    found` leaves the keys after it queued: `/zzz<CR>` still captures,
    `/zzz<CR>x` runs the `x`, and `d/zzz<CR>` cancels the operator and
    changes nothing. A miss is therefore an ordinary no-effect case. The
    generator still treats a missing capture as a harness error, never
    as a result. The search cases also pin `nohlsearch noincsearch` so
    that no redraw state leaks between cases.
11. **An empty buffer is reused, not wiped** (found 2026-09-30 by the 3b
    fuzz on Vim 9.1.0697). `bwipeout!` of the only buffer opens an empty
    one in its place, and Vim reuses the current buffer for that when it
    is empty, so a case that ends with the text empty left the next case
    its Visual area and marks: `gv` there reselected the previous case's
    area. The runner fills the buffer before wiping it. With the
    fix, all 53,109 corpus cases regenerated unchanged; only two fuzz
    cases (seed 31) had read a stale area.
12. **`'scroll'`, the last pattern and the search direction outlive a
    buffer** (found 2026-10-01 by plan 3c's probes). A counted `ctrl+d`
    sets the window's `'scroll'` for every later case, `n` in the next
    case found the previous case's pattern, and `?` left the direction
    backward. The runner resets all three per case (`setlocal scroll=0`,
    `let @/ = ''`, `let v:searchforward = 1`).
13. **A pending Insert restart outlives a case** (found 2026-10-01). A case
    ending inside Insert `ctrl+o`, or in Visual entered from it, leaves
    `restart_edit` set; `:normal! <Esc>` cannot clear it, since `:normal`
    saves and restores it, and the next case's first command restarted
    Insert and typed the rest of its keys. The keys now end with
    `<Cmd>call Capture()<CR><C-\><C-n>` inside the same `feedkeys()`:
    `CTRL-\ CTRL-N` ends any mode and clears the restart.
14. **The search history outlives a buffer** (found 2026-10-01 by plan 3c
    Task 11). `<Up>` in the prompt recalled the previous case's pattern:
    `/xyz<CR>` in one case, then `/<Up>foo<CR>` in the next, searched
    `xyzfoo`. The runner calls `histdel('/')` per case.
15. **A `try` or `silent!` around `feedkeys()` changes what an error does**
    (found 2026-10-01 by plan 3c Task 11). Inside a `try` an error (E486)
    becomes an exception, and under `silent!` it returns early: either way
    `emsg()` skips `flush_buffers()`, so a `.` whose search fails ran the
    rest of its stuffed redo text as commands, which real Vim never does
    (`c/o<CR>Y<Esc>j.` yanked the line in the oracle, not in plain Vim).
    The runner calls `feedkeys()` plainly, and the `errmsg` field of each
    raw result records the error (`v:errmsg`, kept for diagnosis).

The golden header records `vim_version`, `v:versionlong`, the patch list,
`SETTINGS_LINE`, `winheight`, and the corpus file's SHA-256. The generator
refuses Vim older than 9.1.

### 6.4 Golden format

```json
{"header":{"vim":"9.1.0016","settings":"expandtab shiftwidth=2 …","winheight":23,"corpus_sha256":"…"}}
{"id":"ops/dw/words@1:5/d3w","tier":1,"status":"ship","lines":["foo bar baz qux"],"cursor":[1,5],"keys":"d3w","reg":null,
 "expect":{"lines":["foo "],"cursor":[1,4],"mode":"n","visual":null,"reg":"bar baz qux","regtype":"v","top":null}}
```

(The expected values shown are illustrative. The generator writes the real
ones.)

### 6.5 The `cargo test`

`vim_conformance.rs` parses the header and asserts
`header.settings == engine::settings::SETTINGS_LINE`. A settings change
without regenerating the golden file fails loudly. Each case then runs on
up to two buffers:

- **Body**: always. `EditorState::new(Lines::from(lines.join("\n")))`.
- **One-line**: only when both the input and Vim's result are one line,
  and the keys contain no key the one-line buffer declines in the mode
  it is typed in (§4.3): Insert `Tab`, `Up` and `Down` among them
  (`ctrl+t`/`ctrl+d` act in a field: plan 3c Deviation 11). Vim gives
  those keys an effect (spaces, an undo break), while the one-line field
  hands them to the app, so "declined = no effect" would be false; plan
  3c extends the declined-Insert-key skip to Replace and `ctrl+o`'s
  Normal. The first rule skips `o`,
  `yyp`, `J` and Insert `Enter`; plan 3b (its Deviation 10) adds a
  counted `o`/`O` (`3oX`), a put from a linewise `"0` (`yy"0p`), and
  Insert `ctrl+r` of a register holding a control char (Vim types the
  char's key, a field flattens it). Every rule's one-line behaviour is
  covered by S tests. The buffer is `LineInput::new(line)`.
- **No corpus case** for `"+` (S tests only): the pinned Vim is built
  without a clipboard, where `"+` is an invalid register. The engine's
  `"+y` and its cut (`"+d…`, `"+c…`) copy out through
  `AppRequest::CopyToClipboard` (plan 3c Deviation 14).

For each run the test does the following:

- Sets the caret and register preset, then creates a fresh `Engine` and
  `BufState` and calls `enter(Start::Normal, Seat::Keep)`.
- Feeds the parsed keys through `handle`. A `Declined` outcome counts as
  "no effect", which matches Vim's failed motion or no-op for every
  declined key that reaches a run (the one-line skip rule above removes
  the rest).
- Compares lines, cursor (0-based on the Rust side), the mode (`n`, `i`,
  `R`, `v`, `V`, `niI`, `niR` map to `Mode`), the Visual anchor, the unnamed
  register text and its type, and `top` when recorded.

The test collects every mismatch rather than stopping at the first. It
prints a per-group summary and the first 50 diffs (id, buffer, keys, input,
expected and actual). `VIM_CASES=<glob>` narrows the run. Cases with
`status = "later"` run and are counted, but only fail the test once their
group is switched to `ship`. A later case that already passes is reported
so its group can be switched.

### 6.6 Divergences

```toml
[[divergence]]
id = "objects/quote/quotes@1:9/di\""     # exact id or a glob
buffer = "both"                          # "body" | "one-line" | "both"
expect = { lines = ["…"], cursor = [1, 9] }   # what the engine does instead; or `skip = true`
who = "mine"                             # you | mine | earlier — required
why = "…"                                # required
```

An entry replaces the golden expectation for the matching runs. The test
fails if an entry matches no case, or if the engine now matches Vim on a
listed case, so stale entries can't pile up. Divergences should be rare.
The one-line rules in §5 need none, because those cases are skipped by the
one-line-result rule and pinned by S tests instead.

Case operators (`you`, decided 2026-09-30): `g~ gu gU`, `~` and Visual
`~ u U` re-case only their range, char by char, and their undo is exact.
Vim's `op_tilde()` walks the range by a byte count, so it overruns the range
when re-casing changes a char's UTF-8 length (`İ ı ſ K`), re-cases a whole
line for `gu0` over an empty range in column 0, and its undo leaves the
overrun text changed. Three `divergences.toml` entries record this. Where
the caret lands after a linewise case op (Normal or Visual) is the range
start moved by the chars the re-case added before it, so after `gUj` it
stays on the same letter when a `ß` before it became "SS", as in Vim. Vim
gets there by keeping the caret's byte offset, and the engine by counting
chars (`you`, decided 2026-10-01: the more correct rule), so they part only
when a char before the caret shrinks in UTF-8 (`ı İ ſ`, 2 bytes to 1): Vim
drifts right, the engine stays. Two more `divergences.toml` entries record
that. These five are the `you` exceptions to the zero-divergence target
(§2).

Plan 3c adds three kinds. The first and last were my recommendations,
which the user confirmed on 2026-10-01 (`you`); the second is `mine`,
pending the user's review:

- The `'^`-through-undo entries (R2): Vim moves `'^` per undo entry and
  the engine per step, so after an Insert session whose line count changed
  and changed back (`AX<CR><Del><Esc>u`) Vim's `gi` starts at the caret and
  the engine's on the mark's row. The user ruled on 2026-09-30 that the
  undo-exact rule covers edited text, not `'^`; plan 3c dropped Vim's
  undo-entry merging (its Deviation 21), confirmed 2026-10-01.
- `gi` after `Rab<Esc>u` over wide chars (`insert/replace/unicode`): Vim's
  `'^` keeps a byte column and lands inside a char; the engine counts
  chars, by the user's 2026-10-01 rule for the caret after a case op.
- Search offsets (`search/patterns-cases`, `/a/b<CR>`): Vim honours the
  offset, the engine refuses it with "search offsets not supported" and
  the caret stays (plan 3c Deviation 3), confirmed 2026-10-01.

### 6.7 Regenerating

Run `python3 scripts/vim_oracle/generate.py` from the repo root. It
rewrites `golden.jsonl` and prints how many cases were added, removed and
changed. `--only <glob>` regenerates a subset and merges it in. `--fuzz N
--seed S` writes random key strings over the fixed texts to
`$TMPDIR/vim_fuzz.jsonl`, which is not committed. The test can replay it
with `VIM_FUZZ=<path>` (read by the test only, never in `src/`). Fuzz
failures that matter are copied into the corpus. Any change to
`SETTINGS_LINE` or the corpus is committed together with the regenerated
golden file.

## 7. What is copied from vim-mode and what is new

| From vim-mode | Fate |
|---|---|
| `field.rs` word motions `w b e W B`, `f t F T ; ,` (including the rule that a repeated till skips an adjacent match), `cw` special case, `recase`, `iw`/`aw` | **copied**, then generalised to `Pos` and multi-line |
| `seq.rs` `MAX_COUNT`, `accumulate_count`; `field.rs` `combine_counts`, the echo format | **copied** |
| `field.rs` `open`/`carry`/`settle`/`close`, `Settled`, `InsertSeat`, `adopt_selection`, `clamp_caret` | **copied** as §4.2's edges (`FieldKind` becomes `Start`) |
| `line_input.rs` `VisualShape`, `set_visual`, `paint_span` | **copied** |
| field.rs tests (about 80) | assertions about Vim behaviour become **corpus cases**. Tests of app rulings stay as S tests only where §4 keeps the rule |
| `field.rs` `Pending` enum (7 variants), `known_singles`, `declined_count`, `start_operator`, `yield_count` | **replaced** by one `Pending` struct and `Outcome::Declined { count, keys }` |
| `line_input.rs` `edit_span`, `SpanEdit`, `insert_pinned`, `record_pinned`, `Snapshot.pin`; `overwrite_span` typing chars through `handle_key` | **replaced** by `splice` plus the engine history |
| `word_nav` reuse for word classes | **replaced** by `class.rs` (§3.13) |
| `editor.rs` body vim layer (about lines 2425–2900 and 3076–3270), `body_vim_handler`, the pending mirror, `RemoveChar(0)` captures | **dropped** |
| `FieldId`, `FieldKind::Cmdline` routing | **not here**; they belong to piece 4 |

New: `TextBuf` with both implementations, multi-line motions and ranges,
the rules in §3.6, bracket/quote/paragraph objects, `%`, `J`, `>`/`<`,
`ctrl+a`/`ctrl+x`, `R`, register types, `.`, the history, Insert sessions,
Visual line mode, `gv`/`gi`, and the harness.

## 8. Test plan

1. **Conformance (V).** §6.5, on both buffers. This is the main evidence
   for §1–§4 of the key list.
2. **Engine S tests** (`vim/engine/tests.rs`) for behaviour with no Vim
   counterpart:
   - Every row of §4.3 returns `Declined` with the right `count` and
     `keys`.
   - Keys that should be inert are consumed and show their `Note`.
   - `"a` shows a note and arms nothing.
   - Echo strings (`3`, `2d`, `d3`, `"0y`, `gU`, `di`, `f`, `r`) and
     `pending()`.
   - Each `Mode` value.
   - The one-line rules: `o`, `O` and `J` are no-ops; `dd`, `cc` and `yy`
     take the whole field; a multi-line charwise put is joined with spaces;
     a linewise register is put charwise.
   - `enter`/`carry`/`settle`/`leave`, including a mouse sweep adopted as
     Visual and `leave` during Insert as one step.
   - `external_edit` is one step and not `.`-repeatable.
   - Paste in Insert is part of the session.
   - The history cap.
   - `u` declines once the history is empty.
   - `BufState::edited`, `text_at_start` and `end_session`.
3. **Buffer contract tests.** One generic suite run on `OneLineBuf` and
   `BodyBuf`: splicing across lines, across a line break, and with Unicode;
   `set_cursor`; `show`.
4. **Arrows profile unchanged.** Existing `LineInput` and body tests pass
   untouched. A test pins that `LineInput::handle_key` never touches
   `visual` and that the body in the arrows profile still uses the emacs
   handler.
5. **Performance smoke test.** `dG`, `u` and `ci{` on a 5,000-line body
   each finish under 50 ms in a debug build.

## 9. Open questions (decided 2026-09-27)

All five were decided by the user on 2026-09-27, each by accepting my
recommendation. They stay labelled `mine`: decisions on these questions,
not standing rules.

1. **Body undo store. Decided: A.** In the vim profile edtui only stores
   and draws, and the engine owns undo (§3.11), because edtui's stack gets
   the caret and the redo branch wrong and cannot be fed without patching
   or tricking it. Options B and C were rejected.
2. **How long body undo lasts. Decided: while the request stays open.**
   `u` after a trip to the response pane still undoes the last small edit.
   The history is dropped on a request switch or on any change made
   outside the engine, and the app history keeps one step per body session
   (§3.11). The rejected alternative was a fresh history each time the
   body gets the caret.
3. **Insert-mode `ctrl+o` (tier 2). Decided: the engine takes it in Insert
   only**, and runs one Normal command as Vim does. In Normal, `ctrl+o`
   keeps opening the project chooser (`earlier`), which stays one `Esc`
   away.
4. **Search inside the body (`/ ? n N * #`). Decided: tier 2, second
   wave**, designed in §3.14: a search prompt at the bottom of the body,
   matches highlighted, `n`/`N` that wrap, and `/` motions after
   operators, with the oracle testing it.
5. **Search pattern language (raised by decision 4). Decided: A**, the
   Vim `magic` subset (`mine`, accepted 2026-09-27). Should `/pattern`
   understand Vim's regex syntax?
   - (A) A translated subset of Vim's `magic` syntax (§3.14 lists it). A
     pattern outside the subset shows `pattern not supported` and searches
     nothing. The oracle tests every atom in the subset.
   - (B) Literal text only: every char matches itself. This is simpler,
     but `/a.b` finds only the literal `a.b`, and `/^  "id"` finds
     nothing, so a vim user's patterns silently mean something else.
   - **Accepted recommendation: A.** It is the "instantly use, with common
     keys" goal applied to search. The subset covers the atoms people type
     from memory, and anything else is refused rather than misread.

## 10. Out of scope

- Piece 4: the router, lists, tables, panes, ex commands, footer wording,
  `ZZ`/`ZQ`, macro and mark notes, deciding which surface starts in which
  mode, and wiring `BufState` into the app history.
- Piece 2: the app undo and list-cursor redesign. Piece 3 needs nothing
  from it; §3.11's app-history requirement is piece 4's.
- The arrows profile's text editing.
- Everything the key list marks Out: `gJ`, `it at is as`, block Visual,
  named registers `"a`–`"z`, marks, macros, `:s`, `:g`, `:set`. Search
  offsets, `gn`/`gN`, `g*`/`g#` and search history (§3.14).
- Changes to edtui itself, including vendoring it.
- Syntax colouring of the body, which is the renderer's and unchanged.
  The search highlight is in scope (§3.14).
