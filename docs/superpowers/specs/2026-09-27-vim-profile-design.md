# Vim profile (piece 4) — design

Status: draft by Claude, 2026-09-27, for review. Piece 4 of the vim
rebuild. It implements §6–§9 of the target key list
(`docs/superpowers/specs/2026-09-27-vim-target-keys.md`, "the key list")
and integrates piece 3's text engine into every text surface. Where this
spec and the key list disagree, the key list wins and this spec is fixed.
It supersedes `2026-09-18-vim-mode-design.md` for everything outside the
text engine.

Every decision is labelled:

- `you`: one of your stated rules, or a choice you made on 2026-09-27.
- `mine`: my recommendation. Accept, change or reject it.
- `earlier`: decided during the vim-mode rounds, and I can't show it was
  yours. It is a default, not a rule.

## 1. Background

Branch `vim-mode` added a vim profile in 155 commits over seven sweep
rounds. The review of 2026-09-27 found the router sound in intent but
grown by special cases. `Router::resolve` has 17 order-dependent
early-return arms, 7 pending mechanisms, 5 count accumulators and about 14
App→Router setter seams. One drifted copy of the "is a field open" gate
made a real wrong edit: `daw` in an open table cell committed the cell and
added a row. You decided not to merge the branch and to rebuild from
`main` in four pieces (`you`). This is piece 4. It runs last. It depends
on piece 2 (undo and list-cursor model) and piece 3 (the text engine),
and it uses `vim-mode` as reference only.

## 2. Goals and success criteria

Goals:

1. A vim user can use the whole app with common keys at once, and it
   doesn't feel like "vim in the text boxes" (`you`).
2. The arrows profile stays complete and modeless. Nothing requires vim
   (`you`, rule 4).
3. There is no default profile. You pick vim or arrows on first launch
   (`you`).
4. One router with one pending machine and one table, so a key's meaning
   is looked up once and never guessed again later.

Success criteria. Piece 4 is done when all of these hold:

- Every tier-1 row of key-list §6–§9, and every tier-2 row this spec
  ships (§9, M5), has at least one App-level behavioural test in the
  matrix of §8. The coverage test proves it.
- Every row marked Out has a negative test. The key does nothing else,
  and the next key is not eaten.
- Every named action and every list verb can be reached by keyboard in
  both profiles. The reachability test in §8 checks this.
- Every change a key makes passes the undo-restores-exactly check in §8
  (`you`, rule 1).
- The acceptance tmux sweep in §8 passes: every scripted step matches,
  and there is no data loss or wrong edit.
- `Router::resolve` has no early-return arm ordered for correctness. A
  key's meaning is exactly one table lookup plus the pending machine's
  transition.

## 3. Architecture

### 3.1 Where the router sits

`App::handle_key_inner` (`crates/postui/src/app.rs:9846`) keeps its
numbered steps. The router goes in one place: after the super-key
normalization, the cmd+c copy, step 1 (the modified-quit escape hatch)
and the two live-drag guards, and before step 1b. Everything the router
declines runs today's steps 1b–6 unchanged.

```
handle_key_inner(ev):
  normalize_super_keys, cmd+c, step 1 (ctrl+c), drag guards   // unchanged
  let ctx = self.key_ctx();                  // one snapshot per event (§3.2)
  let edge = self.router.sync(&ctx);         // field-session edges (§3.3)
  self.apply_session_edge(edge);             // engine.open / carry / close
  match self.router.resolve(ev, &ctx) {
      Resolved::Declined     => self.handle_key_routed(ev),   // steps 1b–6 as on main
      Resolved::Consumed     => true,        // pending grew, or a key swallowed
      Resolved::Run(steps)   => self.run_steps(steps),        // see below
  }
  end of event: ctx2 = self.key_ctx(); edge = router.sync(&ctx2); apply it
```

`run_steps` runs each `Step` in order:

- `Step::Action(a)` goes to `self.update(a)`.
- `Step::Key(ev)` goes to `handle_key_routed(ev)`, which is today's
  routing. It never re-enters the router. This is how a count replays a
  named key (`3j` is three `Down`s).
- `Step::Engine(ev)` goes to the piece-3 engine against the live field.

`OpenAs` is carried by `Resolved::Run` when a key opens a field. An
example is `i` on a table row: `[Key(Enter)]` plus `OpenAs::Insert(Keep)`.
It is applied by the end-of-event `sync`, from a local variable. There
is no one-event router flag for it (vim-mode's `open_insert`) (`mine`).

`App::handle_mouse` also ends with `sync`. A click that opens or moves a
field gets its edge in the same event.

### 3.2 `KeyCtx`: one snapshot per event

`App::key_ctx()` computes this fresh for every key. The router reads
nothing else from `App`. This replaces vim-mode's roughly 14 setters
(`set_focus`, `set_body_typing`, `set_body_visual`, `set_url_closed`,
`set_known_singles`, `note_field_caret`, …) (`mine`, from the review).

```rust
pub struct KeyCtx {
    pub profile: KeyProfile,           // Arrows | Vim
    pub surface: Surface,              // where the keys go (§3.3)
    pub field: FieldCaret,             // None | Live(FieldId) | Parked(FieldId)
    pub engine: EngineView,            // mode + half_typed, meaningful when Live
    pub answers: SmallVec<[char; 4]>,  // the top dialog's letter answers (§5.10)
    pub user_bare: bool,               // vim: keys.toml binds this bare char (§4.4)
}
```

### 3.3 `Surface`, `FieldId`, `FieldSession`

`Surface` replaces vim-mode's `Focus` (`vim/focus.rs`) and extends it. It
names the thing the keys go to. It never names a stale `PaneId`.

```rust
pub enum Surface {
    Sidebar, UrlLine, Method, Table(EditorTab), Body, Response(ViewMode),
    VarList, VarGrid, VarForm, ManageList(ManageTab), ManageDetail(ManageTab),
    Settings, Prompt, Form, Dialog, Picker, CommandLine, Other,
}
```

`FieldId` enumerates every text surface once (`mine`, from the review).
Kept from vim-mode (`vim/field.rs:79`), with `Body` and the query inputs
added:

```rust
pub enum FieldId {
    Url, TableCell { row: usize, col: usize }, Body,
    VmForm(VmField), VmGrid { row: usize, col: usize }, Settings(SettingsField),
    ModalInput(usize), FilePicker, Jq, Search, Palette, ChooserFilter, VarPickerFilter,
}
impl FieldId { pub fn kind(self) -> FieldKind }   // Buffer | Body | Prompt | Cmdline
```

`App::field_caret() -> FieldCaret` is the one match over App state that
says which field has the caret. `open_text_field_mut` (`app.rs:2308`),
`field_open` (`:2350`), `pane_field_open` (`:2383`) and
`open_text_field_edited` (`:2395`) are rewritten to call it. Today they
are four parallel enumerations. They become four views of one.

The router keeps one piece of field state across events:
`FieldSession { None, Live(FieldId), Parked(FieldId) }` (`mine`, from
the review). `sync` compares the session with `ctx.field` and returns
the edge. The edge semantics are the same as vim-mode
`note_field_caret` (`earlier`):

| Before → after | Edge | Engine call |
|---|---|---|
| None → Live(a) | open | `engine.open(kind, start mode or OpenAs)` |
| Live(a) → Live(b), a ≠ b | carry (Tab, or a click into the next field) | `engine.carry(kind)`, mode kept |
| Live(a) → None | close | `engine.close()`, and the router's pending is cleared |
| Live(a) → Parked(a) | park (palette, `{{` picker, menu over the field) | none; the session keeps its mode |
| Parked(a) → Live(a) | unpark | none |

While a session is Parked, `ctx.surface` is the modal on top. No
field-only table row can match, so the rule "a parked field never claims
the modal's keys" holds by construction, not by a guard.

### 3.4 The pending machine

One state machine replaces vim-mode's seven mechanisms: `seq::Pending`,
`window_pending`, `field_tab_pending`, `field_tab_count`, `field_prefix`,
`body_count` and the `known_singles` rescue (`mine`, from the review).

```rust
enum Pending { Idle, Count(u32), Seq { count: Option<u32>, keys: SmallVec<[Tok; 3]> } }
enum Tok { Char(char), Ctrl(char), Named(KeyCode) }   // table keys also use Tok::Any
```

Transitions for one key `k` on surface `s`, in vim:

1. **Esc.** If the state is not Idle, clear it and return `Consumed`
   with no note, even after `q` or `@`. If the state is Idle, Esc goes
   to the table like any other key.
2. **Digit.** `1`–`9`, or `0` after a count has started, is added to the
   count. The count saturates at 9 999 (vim-mode `seq::accumulate_count`
   and `MAX_COUNT`, copied) and the result is `Consumed`. A leading `0`
   is a key, not a count.
3. **Table step.** Look up `buffer + k` against `s` (§3.5):
   - `Complete(b)`: run `b.intent` with the count under `b.count_rule`,
     go back to Idle, return `Run`.
   - `Prefix`: store it, return `Consumed`. The footer echo shows it.
   - `NoMatch`: go to step 4.
4. **No match.** What happens depends on what is buffered:
   - Nothing buffered: the key is `Declined`, except in the vim profile
     on a non-field surface. There a bare char is `Consumed` silently,
     because vim beeps and there is no letter arm left to reach (§3.6).
   - Only a count, and `k` is an arrow key, PgUp or PgDn: `k` repeats N
     times, so `3<Down>` moves three rows as in Vim. For any other key the
     count is dropped and the key is re-stepped from Idle, so a stray
     count never multiplies a later key.
   - A prefix, and `k` is a modified chord: the prefix is dropped and the
     chord is `Declined`, so `d` followed by ctrl+c still quits.
   - A prefix, and `k` is anything else: both are swallowed and the state
     goes back to Idle. This is vim's beep.
   - In an open field in Normal (§3.7): the buffered keys and `k` are
     replayed into the engine, in order, as `Step::Engine`. This is the
     lookahead rule.
5. **Macro and mark prefixes.** `q`, `@`, `m` in a field, `'` and `` ` ``
   match the table row `x<any>`. Any next key completes the pair,
   including Enter, arrows and chords. Esc cancels silently. §5.11 says
   what each pair does.

In the arrows profile, the machine never leaves Idle. Every arrows table
row is one key long, and a test enforces that (§8.3).

A surface change (`ctx.surface` differs from the last event's) clears
the machine before step 1. A key typed on one surface never completes a
sequence on another (`earlier`).

### 3.5 The binding table

`crates/postui/src/keyroute/table.rs` holds one static table for both
profiles and every surface. vim-mode kept this knowledge in four places:
`seq::is_known`, `seq::PREFIXES`, `Router::dispatch_seq`, and
`intent::action_for` together with `pane_letter` and `normalize_open`.
They merge here (`mine`, from the review).

```rust
pub struct Binding {
    pub keys: &'static str,        // vim notation: "dd", "gt", "<C-w>h", "q<any>", "<CR>"
    pub profile: Profiles,         // ARROWS | VIM | BOTH
    pub on: Surfaces,              // bitset of surface classes, incl. FIELD_NORMAL_1LINE etc.
    pub intent: Intent,
    pub count: CountRule,          // Ignore | Repeat | Pass | Refuse(&'static str)
    pub caption: &'static str,     // footer/hint label: "delete", "add", "tabs"
}
pub enum Intent {
    Key(KeyCode),                  // replay a named key: j → Down
    Keys(&'static [KeyCode]),      // e.g. "gg" on the response → ctrl+Home
    Verb(Verb),                    // → Action::OnSelection(verb)   (§6)
    Act(fn(Option<u32>) -> Action),// fixed actions: Undo, OpenExPalette, SelectTab(n)…
    Open(OpenAs),                  // i / a / I / A / o on a closed field
    LeaveField(Dir),               // j / k in an open one-line field: CloseField + Up/Down
    Window(char),                  // ctrl+w targets, resolved by keyroute/panes.rs
    Note(&'static str),            // "Macros are not supported" …
    Recording(char),               // q{reg}: note, and arm the recording flag (§5.11)
    Swallow,                       // a known key with no meaning here
}
```

Lookup is a linear scan with `keys` pre-parsed once into `Tok`s. The
table has about 150 rows, so this is cheap. `Prefix` means "some row on
this surface extends the buffer". It is derived from the rows, so no
separate prefix list can drift.

`CountRule` applies the count:

- `Repeat` runs the intent N times, as N separate dispatches and N undo
  steps.
- `Pass` hands N to the intent, as in `SelectTab(N)`, `Delete{count:N}`
  and `NG`.
- `Ignore` runs it once.
- `Refuse(note)` runs nothing and shows the note.

The same table answers "how is this spelled?" for the footer and the
hover hints (§4.6). That makes it the one source for key spellings.

### 3.6 The `Declined` contract

`Declined` means exactly "route this key as main does today". The router
declines these:

- named keys, when no Seq is pending;
- modified chords with no table row, which reach step 4's keymap
  precedence;
- every key while the live field's engine is in Insert, after the engine
  has declined it;
- in the arrows profile, every key with no arrows row.

The router never returns `Declined` for a key it partly consumed. There
is one change on main: the components lose their letter arms (§4.5 and
§6). Main's routing therefore gets named keys and chords only, plus the
bare chars that fields type.

### 3.7 The engine boundary

Piece 3 provides the engine. This spec assumes an API roughly like the
one below. If piece 3 differs, only `keyroute/engine_glue.rs` changes.

```rust
engine.open(kind, mode, seat, target); engine.carry(kind); engine.close();
engine.handle(key, target) -> Consumed | Declined | Did(EngineEffect);
engine.mode() -> Normal | Insert | Visual | VisualLine | Replace;
engine.half_typed() -> bool; engine.echo() -> String;
// target: Line(&mut LineInput) | Body(&mut edtui::EditorState)
```

Piece 4 needs these from the engine. If one is missing, M3 stops and the
user is asked:

1. It declines exactly the keys in piece 3's decline table
   (`2026-09-27-vim-text-engine-design.md` §4.3), without changing state —
   that table is the single source; this spec must not assume a broader
   rule. Esc in idle Normal is declined there. Other plain keys with no
   engine command are consumed silently (with an "unsupported" note for
   the few listed there); every app key piece 4 needs in a field is caught
   by the look-ahead below before the engine sees it, and every
   ctrl/alt chord that is not the engine's is declined.
2. `FieldKind::Cmdline` runs locked to Insert, and ctrl+w and ctrl+u work
   there.
3. It handles register prefixes (`"x`) itself: it consumes the register
   key, and for an unsupported register it returns `Did(Note)`.
4. In the body it drives edtui's public `EditorState`, and edtui's own
   vim keys never run in the vim profile (`you`: body keeps edtui for
   buffer, render and undo only).

**Who sees a key in an open field.** In Insert, Visual, or Normal with
`half_typed()` true, the engine sees every key first. The router adds
nothing, and a key the engine declines goes to today's routing.

In idle Normal, the router looks first, but only at rows whose surface
class is `FIELD_NORMAL_*`. There are few of them:

| Keys | Meaning in an open field (Normal, idle) | Who |
|---|---|---|
| digits | buffered, then replayed into the engine on NoMatch | mine |
| `gt` `gT` `{N}gt` | tab keys (§5.8); `g` + any other key is replayed into the engine (`gg`, `ge`, `gu`…) | earlier |
| `<C-w>`+target | the field closes (commits), then the pane moves (§5.8) | earlier |
| `:` | opens the command line over the field, which is parked | earlier |
| `ZZ` `ZQ` | quit (§5.11) | you |
| `q<any>` `@<any>` | macro note (§5.11) | earlier |
| `m<any>` `'<any>` `` `<any> `` | marks note, and the key after is consumed | mine |
| `j` `k` (one-line fields only) | leave the field and move rows: `CloseField`, then Down/Up × count | you |

On a NoMatch, the buffered keys replay into the engine. `2gg`, `3dw`,
`ge`, `gU` and `0` all reach the engine exactly as typed. The engine
never knows the router looked ahead (`mine`). This one rule replaces
vim-mode's `field_tab_pending`, `field_prefix`, the table-cell `a` arm
and the count lifting (`yield_count`).

**`u` and `ctrl+r` in a field.** The field's own history goes first. If
it has none, the key goes to the app history. This is the same rule main
applies to ctrl+z (2026-09-15, step 1c, `app.rs:9913`) (`earlier`).

### 3.8 Echo and mode indicator: one function

`Router::echo(&self, ctx: &KeyCtx, engine: &EngineView) -> Echo` is the
only source. The footer calls it through `App::key_echo()`. It takes the
first of these that applies:

1. the router's pending, e.g. `3`, `2d`, `g`, `^W`;
2. otherwise the engine's `echo()` when a session is Live;
3. otherwise the recording note `recording @a — not supported` (§5.11).

The mode indicator is a function of `ctx`:

| State | Indicator |
|---|---|
| an open field | the engine's mode |
| a Cmdline, prompt in Insert, or picker query | `INSERT` |
| anything else | `NORMAL` |

It only shows in the vim profile. It sits at the footer's right end
before the palette and quit chips. It is gated on room like the hover
hint, and is never truncated (`earlier`, vim-mode 056dc75). The rule
"the mode indicator always names the thing taking the keys" (`earlier`)
holds by construction, because the indicator reads the same `ctx` the
router just used.

## 4. Profiles

### 4.1 The setting

`config.rs` gains `KeyProfile { Arrows, Vim }` and
`UiSettings.keymap: Option<KeyProfile>`. `None` means "never asked". It
is written as `keymap = "arrows"` or `keymap = "vim"`.

vim-mode spelled these `Default` and `"default"` (11808df). There is no
default any more, so the name goes (`you`: "vim and arrows are equal";
spelling `mine`). An unknown value warns and leaves the choice unmade, so
the question is asked again (`earlier`, 11808df). `config_seed()` gains a
commented line.

### 4.2 The first-launch question

`App::raise_keymap_choice` is copied from vim-mode a21a167, with these
changes (`mine` unless marked):

- **Wording.** Title "Keyboard". Body: "Use vim keys, or arrows and Enter?
  You can change this later on the Settings tab." Answers are `a`
  "Arrows and Enter" and `v` "Vim keys". Both are painted as Secondary and
  neither is aimed. Enter does nothing until an answer is aimed with ←/→
  or a letter is pressed. Esc does nothing, because there is no default
  to fall back to (`you`: no default). ctrl+c still quits.
- **Stacking.** It stacks under the startup config gate and is re-raised
  at every event boundary (`earlier`, a21a167).
- **When the write fails.** This happens if `config.toml` is unwritable,
  or the gate's "Continue unsaved" was taken. The choice applies for this
  session only, a toast says it could not be saved, and the question
  comes back next launch. vim-mode re-raised the modal forever here.
- **Test apps.** `App::bare` sets `keymap: Some(Arrows)`. Vim cases set
  Vim explicitly.

### 4.3 The Settings row

This is the "Keyboard profile" row. It uses segments `Arrows` and `Vim`
and `Hit::SettingsKeymap(profile)`, copied from 6668bdc: the
`components/settings.rs` segment painter, `hit.rs`, `hint.rs` and the
`app/mouse.rs` click arm. It writes through `Action::SetKeyProfile`
(a21a167).

A switch is recorded as a config undo step, like other Settings rows
(`mine`). A switch clears the router's pending state. An open field stays
open. Arrows has no Normal mode, so the engine is closed and the field
goes on as main's modeless field (`mine`).

### 4.4 Per-profile keymaps

`Keymap::default_bindings_for(profile)` and `try_from_overrides_for` are
copied from 11808df and a21a167, including the profile-aware
`Config::read_all`. The chords that differ are the 2026-09-18 spec's list
(`earlier`), with one change: `u`, `ctrl+r` and `:` move out of the vim
keymap and into the vim table (§3.5). That removes vim-mode's
`known_singles`/`bare_singles` rescue seam (`mine`).

| Key | Arrows | Vim |
|---|---|---|
| `q` | Quit | macro prefix (§5.11); quit is `:q` or `ZZ` |
| `ctrl+r` | Send | Redo (table row) |
| `ctrl+s` | Save | unbound (`:w`) |
| `alt+w` / `shift+alt+w` | cycle split | unbound (`:split`, `:splitback`) |
| `alt+←` / `alt+→` | cycle tabs | unbound (`gt` / `gT`) |
| `alt+a` | add table row | unbound (`a`) |
| `u`, `:` | see open question 1 | table rows |

Every other chord is the same in both profiles. That covers ctrl+c,
ctrl+p, ctrl+z/ctrl+shift+z, ctrl+v (paste), ctrl+o (project chooser),
ctrl+enter/shift+enter (send), ctrl+1–9 and every alt chord (`earlier`).

**keys.toml.** It layers on the active profile, and your bindings win
(`earlier`, a21a167). A keys.toml binding of a bare char in the vim
profile wins over the vim table while the machine is Idle. At load,
`Keymap::vim_shadow_warnings()` reports it, e.g. "keys.toml binds `x`,
which shadows vim's `x`", the same shape as `caret_warnings`
(`keys.rs:509`) (`mine`).

### 4.5 Letters leave the components

The per-component letter arms move into the table's arrows rows and vim
rows. These are the verb letters in `sidebar.rs:717–725`,
`table_editor.rs:447–501`, `manage_list.rs:577–585`,
`varmanager.rs:1129–1523`, `response.rs:2486` (`c`) and `:2601–2608`
(`n`/`N`), and the Settings arms in `app.rs:10194–10256`. The components
keep named keys only: arrows, Home/End, PgUp/PgDn, Enter, Space, Tab, Esc
and their own ctrl chords (`earlier`, vim-mode 0bced15/7599f94). Each
letter's spelling then lives in one place.

The vim aliases main has in the arrows profile are the subject of open
question 1: `j` `k` `h` `l` `g` `G` in every list and the response, `i`
in `keys::opens_field` (`keys.rs:36`), and the bare `u` and `:` keymap
rows. Until you answer, this spec assumes they leave arrows (`earlier`,
2026-09-18 spec "Default mode gets its letters back").

### 4.6 One source for spellings: footer chips and hints

vim-mode's two-spelling `FooterChip` struct (056dc75) and its
`hint::vim_native_key` table (83bdf1f) are replaced by computing the
spelling:

```rust
keyroute::spell(profile, surface, &action) -> Option<Cow<str>>
```

1. If `action` is `OnSelection(verb)` or a fixed intent action, the
   answer is the table row's `keys`, rendered for display. `<C-w>h`
   becomes `^W h`, `dd` stays `dd`.
2. Otherwise it is `Keymap::combo_for`, as today.

The chip type becomes `(label, Option<Action>)` plus a keycap computed at
paint time. The hover hint (`hint.rs`, `HintCtx` gains `profile`) calls
the same `spell`. A chip or hint can therefore never name a key the
router doesn't bind.

Only the pieces that name keys change. `footer_chips` (`footer.rs:50`)
keeps its per-pane shape and ordering rules. `QUIT_CHIP` (`footer.rs:278`)
shows `q quit` in arrows and `:q quit` in vim.

## 5. Behaviour, surface by surface

The tables below give the vim keys, then the arrows spelling where the
verb exists in both. Each row is a row of key-list §6–§9 unless it says
otherwise. Any unsupported key does nothing visible, except where a vim user would
expect something, in which case a short note shows. It never arms a
pending state that eats the next key (key list §9, `earlier`).

### 5.1 Rules every list and table shares

| Keys (vim) | Arrows | Behaviour | Count | Who |
|---|---|---|---|---|
| `j` `k` | ↓ ↑ | move the row cursor | Repeat | earlier |
| `gg` `G` | Home End | first / last row | `{N}G`/`{N}gg`: row N, 1-based, clamped (Home, then N−1 × Down) | earlier; `{N}G` mine |
| `ctrl+d` `ctrl+u` `ctrl+f` `ctrl+b` | same chords | half page / page, as main's arms today | Ignore | earlier |
| `Enter` `o` | Enter | open the row | Ignore | earlier |
| `a` | `a` | add, the same letter in both profiles | Ignore | you (shared letters); letter `a` earlier |
| `r` | `r` | rename the row | Ignore | you; earlier |
| `m` | `m` | move (sidebar: to a space; Spaces tab: move all) | Ignore | you; earlier |
| `dd` | `d`, Delete | delete, never confirms | `{N}dd` Pass (§5.2, §5.3) | you (rules 2, 3) |
| `yy` | `y` | yank the row into the in-app register (invisible: no file, step or toast) | `{N}yy` Refuse "Counted yank is not supported" | earlier; refuse mine |
| `p` `P` | `p` `P` | put a copy below / above the cursor, one undo step each | Repeat | earlier |
| `u` `ctrl+r` | ctrl+z, ctrl+shift+z | app undo / redo, which reselects what came back (piece 2) | Repeat | you (rule 1) |
| `.` | — | repeat the last list change (§5.12, tier 2) | Pass | you |
| `H` `M` `L` `zz` `zt` `zb`, `/` `n` `N` | — | Swallow. `/` shows the note "No search in this list yet" | — | you (`/` parked); rest mine |

`O`, `x`, `i` on a row with no field, and other letters with no row are
swallowed silently (`mine`).

### 5.2 Sidebar

| Keys (vim) | Arrows | Behaviour | Who |
|---|---|---|---|
| `h` `l` | ← → | collapse a folder or go to its parent / expand it (main's `Left`/`Right` arms) | earlier |
| `zo` `zc` `za` | — | open / close / toggle the folder under the cursor, or the folder holding the request (tier 2) | mine |
| `a` | `a` (was `n`) | `PromptNewRequest` in the cursor's folder | you; earlier |
| `dd`, `{N}dd` | `d` | delete the request, or N request rows from the cursor down as **one** undo step (core `delete_requests`, ada2e90, via piece 2). Folder rows are skipped, collapsed folders are untouched, a count past the end is clamped, and on a folder row nothing happens | you; count earlier |
| `yy` `p` `P` | `y` `p` `P` | put duplicates the yanked request into the slot under (`p`) or over (`P`) the cursor with core `duplicate_request_at` (684893c), seeding the level's order array first (571b519). A put in another space is refused with a toast and the register is kept (ac03b90). The register is dropped on a project switch | earlier |

### 5.3 Tables (Params, Headers, Vars)

| Keys (vim) | Arrows | Behaviour | Who |
|---|---|---|---|
| `h` `l` | ← → | move the cell cursor | earlier |
| `Enter` `o` | Enter | open the cell under the cursor in Normal | earlier |
| `i` | — | open that cell in Insert | earlier |
| `a` | `a` | add a row; its key cell opens in Insert | you; Insert earlier |
| `Space` | Space | toggle the row's enabled flag (the surface owns Space) | earlier |
| `dd`, `{N}dd` | `d`, Delete | delete the row, or N rows as one editor undo step (new `Editor::delete_rows(start, n)`). Undo reselects the first restored row and never lands on the ghost row (piece 2) | you; count earlier |
| `yy` `p` `P` | `y` `p` `P` | copy a row below or above it with `DuplicateTableRow`'s `-copy` naming. A row yanked on another tab is refused with a toast (684893c, ac03b90) | earlier |

### 5.4 Manage lists and the Variable Manager

The rule for the vim profile: lists bind the motions, the shared letters
`a` `A` `r` `m`, `dd` `yy` `p` `P`, `o`/Enter/`i` and `u` `ctrl+r`. Every
other arrows letter on these screens is reached in vim by an ex verb
(§5.9), because each of those letters has a vim meaning a vim hand
expects (`earlier`, 2026-09-18 spec "Vim mode on navigation surfaces").

| Surface | Vim | Arrows | Who |
|---|---|---|---|
| Environments / Spaces list | `a` new, `r` rename, `dd` delete, `m` move all (Spaces). `{N}dd` refused: "Counted delete works on requests and table rows". `:tls` for TLS | `a` (was `n`), `r`, `d`, `m`, `t` | you; earlier; refuse mine |
| Variables list | `a` new variable, `A` new selector, `r` rename, `dd` delete; `:secret`, `:fields`, `:promote` | `a` (was `n`), `A` (was `a`), `r` (was `e`/F2; F2 kept), `d`, `s`, `m` | you; earlier (38353d4) |
| Options grid | `yy` / `p` copy / paste an option through the grid's own stash, which is scoped to its selector (`earlier`); `dd` delete; `a` new option; `:rename`, `:value` | `c` `v` `d`, `o` new option and `a` new option, `r`, `e` | earlier; `a` = new option mine (not new variable as in vim-mode: a row here is an option) |
| Variable form | fields only: `j` `k` between fields, Enter/`i` open; `:promote` `:reveal` `:secret` `:clear` `:delete` | `p` `r` `s` `x` | earlier |
| Manage detail panes | fields and buttons: `j` `k`, Enter/`i`; `:delete` `:rename` reach the row | as today | earlier |

### 5.5 Settings tab

`j` `k` `gg` `G` move the row cursor. `h` and `l` aim a Files row's two
buttons. `Enter` opens a value in Normal and `i` opens it in Insert. Esc
closes the screen. In arrows the same keys are ↓ ↑ Home End ← → Enter,
from `app.rs:10178`, minus the letter arms (`earlier`).

### 5.6 Text fields at the boundary

This is the field two-level model (`earlier`, key list §5). A field is
**closed**, meaning its slot is selected, or **open**, meaning it is
being edited.

| Situation | Behaviour | Who |
|---|---|---|
| Opening a closed field by key | `Enter` and `o` open in Normal. `i` opens in Insert. On the URL line `a`, `I` and `A` open in Insert, seated after the char, at the first non-blank, or at the end. On a table row `a` adds a row instead (§5.3) | earlier |
| Opening by an add verb (`a` on a table or grid) | Insert | earlier |
| A prompt, command line, picker query, jq bar or search | opens in Insert; cmdlines have no Normal mode | earlier |
| Opening by click | the kind's start mode (Buffer and Body: Normal; Prompt and Cmdline: Insert), with the caret where clicked | earlier |
| Tab, or a click from one open field into another | carries the mode | earlier |
| `Esc` in Insert | back to Normal (the engine) | earlier |
| `Esc` in idle Normal | today's routing: the field closes and keeps its text. The body blurs (`editor.rs:1668`) | earlier |
| `j` `k` in idle Normal, one-line field | `CloseField`, then Down/Up × count. URL: to the content below. Cell: the row cursor moves. Form: the next field is selected. Prompt: aims the button row. The close is its own undo step | you |
| `Enter` in an open one-line field (not a prompt) | today's routing: commit and close | earlier |
| `a` in an open table cell, Normal | vim's append (the engine). vim-mode made it add a row, which made the open cell the one place `a` was not append | mine (reverses earlier) |

### 5.7 Response viewer

These keys work on Tree, Raw and Headers. `gg`/`G` map to ctrl+Home and
ctrl+End here, because the response's bare Home/End scroll sideways
(`earlier`, vim-mode `pane_override`).

| Keys (vim) | Tier | Behaviour | Who |
|---|---|---|---|
| `j` `k` `gg` `G`, counts, `{N}G` | 1 | cursor line; `{N}G` goes to line N | earlier; `{N}G` mine |
| `h` `l`, `0` `$` | 1 | scroll sideways / to line start or end (the named keys main already handles) | earlier |
| `ctrl+d` `ctrl+u` | 1 | scroll the view **and** move the cursor by half a page, keeping the cursor's screen row. Today they only move the cursor (`response.rs:2505–2512`). Fixed in the chord arm, so arrows gets it too | mine |
| `ctrl+f` `ctrl+b` | 2 | the same for a full page | mine |
| `zo` `zc` `za` | 1 | open / close / toggle the container under the cursor (`Action::TreeFold`) on the Pretty view; nothing elsewhere | earlier |
| `zR` `zM` | 2 | open / close every container (`TreeFold(OpenAll/CloseAll)`) | mine |
| `zz` `zt` `zb` | 2 | `Action::ScrollCaretTo(Center/Top/Bottom)` (vim-mode) | earlier |
| `/` `n` `N` | 1 | search, with the match counter main already paints (`response.rs:4024`) | earlier |
| `*` `#` | 2 | search forward / back for the word under the cursor | mine |
| `yy` | 2 | copy the line (Raw), the node's value (Pretty) or the header value (Headers), with the same toast as `c`. Arrows keeps `c` | mine |
| `V` then `j`/`k`, `y` | 2 | line selection (main's `select_line_extend`), then `y` copies and clears it. `v` shows the note "Only line selection (V) here" | mine |

### 5.8 Panes and tabs

| Keys (vim) | Arrows | Behaviour | Who |
|---|---|---|---|
| `ctrl+w h/j/k/l` | Tab, shift+Tab, click | by geometry: sidebar left, editor over response. `l` from the sidebar goes to the last right pane. On Manage, `l` goes list → detail and `h` goes back. A direction with no pane does nothing | earlier (2026-09-23) |
| `ctrl+w w` `W` `p` | — | next pane, previous pane, last pane | earlier |
| `ctrl+w` + arrow keys | — | the same as h/j/k/l (tier 2) | mine |
| any `ctrl+w` from an open field in Normal | — | the field closes (commits) first. In Insert, ctrl+w deletes a word back (the engine) | earlier |
| a count before `ctrl+w` | — | dropped | earlier |
| `gt` `gT` | alt+→ alt+← | `CycleTabs(±1)` on the focused pane's strip (editor, response, Manage). The sidebar borrows the editor's strip. Disabled tabs are skipped | earlier |
| `{N}gt` | — | `SelectTab(N)`; a disabled or out-of-range tab does nothing | earlier |
| `{N}gT` | — | back N tabs (Repeat) | mine |
| after a keyboard tab move with the editor focused | — | `FocusEditorContent`: the caret lands in the new tab's content, so `gt a` adds a row. Focus never moves panes | earlier (2026-09-21/22) |

### 5.9 The `:` command line

`:` opens the command palette in ex mode (`Action::OpenExPalette`,
vim-mode 9a5acce/0e64dc7/071f292, `earlier`). The query is a Cmdline
field. Command-line editing works in it:

- ctrl+u clears to the start (`line_input.rs:406`).
- ctrl+w deletes a word back. The router maps it to the LineInput's
  word-delete key.
- Backspace on an empty query closes the line (`mine`, as in Vim).
- Esc cancels.
- ctrl+n and ctrl+p move the highlight.
- ↑ and ↓ recall this session's command history (tier 2, `mine`).

**Rows.** Row 0 is the pinned verb the query resolves to: an exact match
first, otherwise the shortest verb it is a prefix of that resolves on
this surface. Under it are the fuzzy palette matches, where a scattered
match must be dense (071f292). Enter runs the highlighted row (`earlier`).

**Queries that run nothing** (`mine`; key list §8 Out rows):

- A Vim command we don't support. These are named in the table: `s`,
  `substitute`, `g`, `global`, `v`, `vglobal`, `noh`, `nohlsearch`,
  `se`, `set`, `sor`, `sort`, `norm`, `normal`, `r`, `read`, `!`, and
  any query that starts with a range (`%`, a digit, `.`, `$`, `'`, `<`).
  The palette shows one muted row, "Not supported: :noh", and no fuzzy
  rows. Enter closes the line and runs nothing.
- A query that pins nothing and fuzzy-matches nothing. The list is
  empty, and Enter closes the line with the note "Not a command: :xyz".

Before any verb runs, a parked field is closed (committed), so `:w` saves
what is in the URL line (`mine`). A count typed before `:` is dropped.

One table, `keyroute/ex.rs`, holds `(name, abbrev, run, caption)` rows.
`run` is a fixed `Action` or `Verb`. A `Verb` resolves against the
surface under the palette through `OnSelection` (§6.1) (`mine`, from the
review).

| Verb | Runs | Who |
|---|---|---|
| `:w` `:write` `:up` `:update` `:wa` | `SaveRequest`. A never-saved request prompts Save as | earlier |
| `:q` `:quit` `:qa` | `Quit`. Unsaved edits raise the existing dirty gate (`app.rs:2451`), where Vim would refuse with E37 | earlier |
| `:q!` `:qa!` | `ForceQuit`: quit without saving | earlier |
| `:wq` `:x` `:xit` `:wqa` `:xa` | new `Action::WriteQuit`. Clean: quit. Unsaved with a file: `SaveRequestThen(ForceQuit)` (`app.rs:9054`). Never saved: `PromptSaveScratch(ForceQuit)`. A refused or cancelled save does not quit | earlier; resolution mine |
| `:e` `:edit` | `ReloadFromDisk`, which keeps unsaved edits (tier 2) | earlier |
| `:e!` | `DiscardChanges` then `ReloadFromDisk` (tier 2) | mine |
| `:send` `:project` `:manage` `:theme` `:split` `:splitback` | as the palette commands | earlier |
| `:new` `:rename` `:delete` `:move` | `OnSelection(Add/Rename/Delete/Move)` on the surface underneath | earlier |
| `:group` `:fields` `:promote` `:secret` `:clear` `:reveal` `:tls` | the Variable Manager and Manage verbs (§5.4) | earlier |
| `:value` | the options grid's "Edit…" prompt. vim-mode called it `:edit`, which is Vim's reload | mine |

### 5.10 Prompts, pickers and dialogs

| Surface | Vim | Arrows | Who |
|---|---|---|---|
| One-field prompt (name, rename, save as) | opens in Insert. `Esc` goes to Normal, and `Esc` in idle Normal cancels the dialog. `Enter` confirms from Insert or Normal. `j` in idle Normal aims the button row | unchanged from main: Enter and Esc close the field, and confirming is the button row or ctrl+enter (`modal.rs:1817–1839`). See open question 2 | earlier (vim); mine (arrows) |
| Multi-field forms (new project, value popup, fields editor) | the field two-level model (§5.6) inside main's form ladder. `Esc` in idle Normal closes the field; the next `Esc` goes to the buttons | unchanged | earlier |
| Pickers (palette, `{{` variables, choosers, file picker) | type to filter; ctrl+n and ctrl+p move (already on main); Enter picks; Esc closes. The footer advertises `^N/^P` in vim | unchanged | earlier |
| Confirm and message dialogs | the answer letters (`y`/`n`, `s`/`d`, …), Esc cancels. `h` `j` `k` `l` `G` become arrows unless the letter is one of the dialog's answers. Prefixes and counts are not taken | unchanged | earlier (d2fb68d) |

The vim prompt rules are table rows on `Surface::Prompt`
(`<CR>` → `ConfirmTopModal`, `<Esc>` in idle Normal → `CancelTopModal`).
`modal.rs` never learns the profile. Main's step 1e (`app.rs:9953`) goes
away once `u` and `:` are vim table rows.

### 5.11 Global keys

| Keys (vim) | Behaviour | Who |
|---|---|---|
| `ZZ` | `:x`, i.e. `WriteQuit` | you |
| `ZQ` | `:q!`, i.e. `ForceQuit` | you |
| `Z` + any other key | swallowed | earlier |
| `q{reg}` | a muted note "Macros are not supported". It arms the recording flag, and the echo shows `recording @{reg} — not supported` until the next bare `q` in Normal, which is swallowed silently and clears it. `qa x q @a` then runs `x` once, drops the `q`, and notes `@a`. Nothing types into a field and nothing opens | note earlier; closing `q` mine |
| `@{reg}` | the same note, with the register key consumed | earlier |
| `m{a-z}` in an open field, `'x` `` `x `` anywhere | the note "Marks are not supported". On lists `m` stays move | you (shared letters); note mine |
| `"x` on a closed surface | the note "Registers are not supported here". In fields the engine owns `"` | mine |
| `ctrl+o` `ctrl+i` | ctrl+o stays the project chooser; ctrl+i is Tab | earlier |
| `ctrl+v` | paste in both profiles; no block Visual | earlier |
| `u` `ctrl+r` `:` | table rows on every surface except Cmdlines and pickers | earlier |

### 5.12 `.` repeats a list change (tier 2)

The router keeps a `ListChange { surface_class, verb, count }`, set by a
list `dd`, `p` or `P` that ran. `.` replays it on the same surface class,
with a new count if one was typed. Each repeat is its own undo step
(`you`).

`.` after anything else, on a different surface class, or with nothing
recorded, shows the note "Nothing to repeat". Prompt verbs (`a` `r` `m`)
are not recorded, because they change nothing until confirmed (`mine`).
In a field, `.` is the engine's.

## 6. Integration cleanups

### 6.1 One `OnSelection(Verb)` action

vim-mode added about 20 zero-argument wrappers: `DeleteSelectedTableRow`,
`DeleteSelectedVar`, `DeleteSelectedEntry`, `DeleteSelectedManageRow`,
`PromptRenameSelectedManageRow`, `PromptRenameSelectedVar`,
`StartSelectedOptionNameEdit`, `CopySelectedOption`,
`PasteSelectedOption`, `YankSelectedRequest`, `PutRequest`,
`YankSelectedTableRow`, `PutTableRow`, `DeleteSelectedRequests`,
`PromptNewSelectedManageRow`, `CycleSelectedManageRowTls`,
`PromptPromoteSelectedVar`, `PromptGroupFieldsForSelectedVar`,
`OpenEditSelectedOptionPrompt`, `ToggleSecretForSelectedVar`,
`RemoveSelectedVarEnvValue`. None of these is added. Instead
(`mine`, from the review):

```rust
pub enum Verb { Add, AddAlt, Rename, Move, Delete { count: u32 }, Yank, Put { above: bool },
                EditValue, Promote, Fields, Secret, ClearValue, Reveal, Tls }
Action::OnSelection(Verb)
fn App::selection_action(&self, surface: Surface, verb: Verb) -> Option<Action>
```

`selection_action` is the one table from (surface, verb) to main's
existing parameterized action. Examples: `DeleteTableRow(i)`,
`DeleteVar { name }`, `DeleteEntry { .. }`, `DeleteEnv(name)`,
`PromptRenameSpace(name)`. Every guard those actions already have still
runs. `None` means the verb has no meaning there, and nothing happens.

The surface is `App::surface()`, the same function `key_ctx()` uses.
From a palette or ex row it is the surface under the modal stack. Key
and meaning therefore can't drift apart.

Footer chips and palette commands for these verbs carry
`OnSelection(verb)` too, so a chip, its key and its ex verb are one
action. The `yank.rs` register (vim-mode, 65 lines) is copied.

Main's existing `DeleteSelectedRequest` and
`PromptMoveSelectedRequestToSpace` stay as they are. `selection_action`
returns them, so nothing outside piece 4's scope churns.

### 6.2 One source for key spellings

This is §4.6. The table is the source, and the footer, hints, intents,
the ex table's captions and the reachability test all read it.

### 6.3 Text fields enumerated once

This is §3.3. `FieldId` plus `App::field_caret()` replace the
enumerations at `app.rs:2308–2404`, and vim-mode's third copy in
`App::field_vim_caret`.

## 7. What is copied from vim-mode, and what is rewritten

| vim-mode | In piece 4 | Why |
|---|---|---|
| `keys.rs` profile split (11808df), first-launch flow and `SetKeyProfile` (a21a167), Settings row (6668bdc) | **copied**, with the renames and no-default changes in §4 | sound; the review kept them |
| `vim/seq.rs` `accumulate_count`, `MAX_COUNT` | **copied** into `keyroute/pending.rs` | small and correct |
| `vim/seq.rs` `Pending`, `PREFIXES`, `is_known` | **rewritten** as the pending machine plus derived prefixes (§3.4–§3.5) | prefixes and known sequences drifted from `dispatch_seq` (the `^` bug) |
| `vim/mod.rs` `Router::resolve` and its 17 arms, `resolve_body`, `resolve_behind_modal`, `macro_or_save_pair`, setters | **rewritten** (§3) | the review's main finding; the `daw` wrong edit |
| `Router::window_move`, `pane_toward`, `pane_override` | **copied** into `keyroute/panes.rs` | the geometry ruling is encoded correctly |
| `vim/focus.rs` `Focus` | **extended** into `Surface` | now names prompts, dialogs, pickers and the URL line |
| `vim/intent.rs` `action_for` | **folded** into table rows plus `App::selection_action` | one table instead of two |
| `vim/ex.rs` `VERBS`, `resolve_ex_query` prefix rule, captions | **rewritten** as one `(name, abbrev, run, caption)` table, adding the unsupported-name rows | the review's finding; `:s` must not pin `send` |
| palette ex mode (`OpenExPalette`, pinned row, dense scattered match, 071f292) | **copied** | the rulings were settled there |
| `vim/normalize.rs` `normalize_open` | **dropped**: `i`/`a`/`A`/`I` become `Intent::Open` rows | one table |
| `note_field_caret` edge semantics, `FieldId`, `FieldKind` | **copied** as `FieldSession` plus `sync`, with `Body` and query fields added to `FieldId` | sound; now one enum |
| per-component letter-arm removal (0bced15, 7599f94) | **redone** on main's current components | same approach, fresh diff |
| shared letters (38353d4, 0b11e69, 83bdf1f), yank/put (684893c, ac03b90), order seeding (571b519) | **copied**: core `duplicate_request_at`, `order_insert_before`, the seed through `order::merge_level`, and `yank.rs`. Footer and hint parts are rewritten to §4.6 | behaviour is right; the spelling plumbing changes |
| two-spelling `FooterChip`, `hint::vim_native_key` | **dropped** for `spell()` | a third copy of the spellings |
| ~20 `*Selected*` actions | **dropped** for `OnSelection` (§6.1) | the review |
| `ScrollCaretTo`, `TreeFold`, `SelectTab`, `FocusEditorContent` actions | **copied**; `TreeFold` gains OpenAll and CloseAll | small, tested |
| prompt changes 9cbfcd1 / d9134ff (Enter confirms, one-Esc cancel, both profiles) | **not copied into modal.rs**. The vim behaviour is table rows (§5.10); arrows waits on open question 2 | keeps arrows untouched |
| `vim/field.rs`, `line_input.rs` additions, body layer in `editor.rs` | piece 3's | not this piece |
| ~24k lines of tests | **not ported wholesale**. Each behaviour becomes one matrix row (§8) | the matrix covers the key list; old tests pinned intermediate rulings |

## 8. Test plan and acceptance sweep

### 8.1 The key-list matrix

**Row IDs.** Piece 4 adds an ID column to §6–§9 of the key list, e.g.
`L.dd`, `L.Ndd`, `R.ctrl-d`, `P.gt`, `X.wq`, `G.ZZ`, `G.qa`. The same
rows live in `crates/postui/src/keyroute/keylist.rs` as
`KEY_LIST: &[(id, tier, Status::{Ship, Out, Parked})]`. A test checks
that every ID in `keylist.rs` appears in the doc and the doc has none
that `keylist.rs` lacks.

**Cases.** `crates/postui/src/app/key_matrix.rs` is a new test module.
It is table-driven:

```rust
struct Case {
    id: &'static str,                    // key-list row
    profile: Prof,                       // Vim | Arrows | Both { vim: "dd", arrows: "d" }
    fixture: Fixture,                    // named App state, built from postui_core::fixtures
    keys: &'static str,                  // vim notation: "3j", "<C-w>l", ":wq<CR>", "qa"
    expect: &'static [Expect],           // structured assertions
    undo: UndoCheck,                     // None | RestoresExactly
}
```

**Fixtures** include `SidebarThree`, where requests alpha, beta and gamma
have the cursor on beta; `SidebarFolders`; `TableThreeHeaders`;
`UrlClosed`; `UrlOpenNormal`; `BodyJson`; `ResponseJson200Lines`;
`ManageEnvs`; `ManageSpaces`; `VarList`; `VarGrid`; `RenamePrompt`;
`DirtySaved`; `DirtyScratch`; `ConfirmDialog`; and `PaletteOpen`.

**`Expect` variants** cover:

- rows and selection: `SidebarRows`, `Selected`, `TableRows`;
- focus: `Focus(PaneId)`, `Tab`, `FieldOpen(FieldId)`;
- mode and echo: `Mode`, `Echo`;
- feedback: `Toast(contains)`, `NoToast`, `Modal(kind)`, `NoModal`;
- disk and exit: `DiskUnchanged`, `DiskFile(path, contains)`, `Quit`;
- `Custom(fn(&App))` for anything else.

**Undo check.** `UndoCheck::RestoresExactly` first captures an
`AppSnapshot`. The snapshot holds the project directory's file bytes,
the sidebar listing and cursor, the editor request and the open field.
The case then runs, sends `u` (vim) or ctrl+z (arrows), and asserts the
snapshot is equal (`you`, rule 1).

**Every Out row** gets a negative case: its keys, then `j`. The case
asserts no disk change, no modal, the expected note or none, and that the
`j` moved the cursor, so the next key was not eaten.

The runner builds the App at 160×48, feeds keys through
`App::handle_key`, and renders once after each key, so hit maps and
viewport heights are real. Each failure names the case ID.

### 8.2 Coverage and reachability

- **Coverage.** `every_shipped_key_list_row_has_a_case`: every `Ship`
  row has at least one vim case. Every row whose verb exists in arrows
  has an arrows case. Every `Out` row has a negative case.
- **Reachability.** `every_action_is_keyboard_reachable_in_both_profiles`
  covers, for each profile, every `keys::named_actions()` entry and every
  `(surface, verb)` pair where `selection_action` is `Some` on its
  fixture. Each must be reachable by that profile's keymap, by a table
  row for that profile, or in vim by an ex row. The exception list is
  main's `keyboard_only_navigation` (`app/tests.rs:17144`) and nothing
  else. It walks the real tables, not copies (`you`, rule 4, for the
  arrows half).
- **Spelling.** `every_painted_keycap_resolves_to_its_chip`: for each
  profile and fixture, every footer chip's computed keycap, fed through
  the router, dispatches the chip's action (vim-mode b34df69's idea).

### 8.3 Table invariants (unit tests in `keyroute`)

- `arrows_rows_are_one_key_long`.
- `no_two_rows_collide`: in each profile and surface class, no two rows
  have the same keys, and no complete row is a strict prefix of another.
- `unknown_after_prefix_never_leaks`: for every prefix on every surface
  and every printable ASCII char `c`, feeding the prefix then `c` leaves
  the machine Idle unless `prefix + c` is itself a prefix. A following
  `j` then resolves as a fresh `j`.
- `counts_saturate`: a 30-digit run followed by `j` gives 9 999 Downs,
  the Down replay capped by the surface.

### 8.4 Acceptance tmux sweep

This runs once, at the end of M5, against the merge candidate. It follows
the tmux harness recipe (server held in the background,
`escape-time 0`, `/` sent as `send-keys -H 2f`). Every timing check runs
as one script, because toasts live 3.4 s. A fresh XDG config and a
fixture project are copied from `postui_core::fixtures`.

The brief is fixed and scripted. Each step has one expected observation,
which is screen text and, where marked, a disk check:

- **A. Profile.** First launch shows "Keyboard", and Esc and Enter do
  nothing. `v` gives `NORMAL` in the footer. Settings, then `Arrows`:
  `j` on the sidebar does nothing (if open question 1 is accepted) and
  ↓ moves. Switch back to Vim.
- **B. Sidebar.** Run `3j`, `dd` (one row gone, no dialog), `u` (back
  and reselected), `2dd` then `u` (both back in one step), `yy` `p`
  (`beta copy` under the cursor), `P`, `u` `u`, `a` + name + Enter
  (created), `r` Esc Esc (cancelled, name unchanged). Disk: only the
  expected files.
- **C. Table.** Run `gt` to Headers, `a`, type `X-A`, Tab, `1`, Esc, Esc;
  `yy` `p`; `dd`; `u`; `i`, `cw` `Y` Esc; `j` (the cell closes and the
  cursor goes down one row); `3dd` then `u`.
- **D. Panes and command line.** Run `ctrl+w l`, `j`, `k`, `h`, `p`;
  `2gt`; `:w` (saved); `:s` Enter (nothing runs); `:noh` Enter (nothing
  runs); `:man` Enter (Manage opens); `gt`; `:q` with unsaved edits (the
  dirty gate). Disk: only the `:w` save.
- **E. Response.** Send; `G`; `gg`; `ctrl+d` (the view and the cursor
  both move); `zc` `zo`; `/id` Enter; `n` `N` (the counter changes).
- **F. Unsupported and quitting.** Run `qa` (note), `x`, `q`, `@a`
  (note), `j` (moves). Open the URL, `ma` (note), `j` (the field closes
  and the cursor moves). `ZZ` with unsaved edits: the app exits and disk
  has the save. Relaunch, edit, `ZQ`: the app exits and disk is unchanged.

**Pass criterion:** every scripted step matches its observation, and the
final project-directory diff equals the expected diff, with no data loss
and no wrong edit. Anything the testers find outside the key list goes to
a backlog document for a later round. It is not fixed on the piece-4
branch.

## 9. Milestones

Each milestone is a set of commits on one branch. It lands as a `--no-ff`
merge commit (`you`, rule 6). Tests and clippy are green at every
milestone.

- **M1: profiles and the router spine.** Needs piece 2 on main.
  - `KeyProfile`, `keymap`, the first-launch question, the Settings row,
    per-profile keymaps and keys.toml shadow warnings (§4.1–§4.4).
  - `keyroute/` with `KeyCtx`, `Surface`, `FieldId`/`field_caret`,
    `FieldSession`, the pending machine, the table, `spell`, and
    `Declined` wired into `handle_key_inner`.
  - Letter arms move out of the components (§4.5). List and response
    motions with counts, and `u` `ctrl+r` `:` rows.
  - Footer mode indicator and echo (§3.8), computed keycaps (§4.6), the
    matrix harness (§8.1) and the invariant tests (§8.3).
- **M2: list and table verbs.**
  - Shared letters `a` `A` `r` `m` in both profiles, `dd`/`d`,
    `{N}dd` (sidebar batch via piece 2's `delete_requests`, and
    `Editor::delete_rows`).
  - `yy`/`y` + `p`/`P`, with core `duplicate_request_at` and the order
    seed.
  - `OnSelection(Verb)` and `selection_action` (§6.1).
  - The Manage and Variable Manager rows (§5.4) and the reachability
    test (§8.2).
- **M3: engine integration.** Needs piece 3.
  - `engine_glue.rs`, the session edges, the open modes (§5.6), the
    field-Normal lookahead rows (§3.7) and `j`/`k` leaving a field.
  - Body routing through the engine in vim, while arrows keeps edtui
    input as today.
  - Vim prompt rows (§5.10) and the Cmdline ctrl+w mapping.
- **M4: panes, tabs, command line, response, globals.** The closed-surface
  half can start before M3.
  - `ctrl+w` geometry; `gt` `gT` `{N}gt` and `FocusEditorContent`.
  - The ex table, the unsupported rows, `WriteQuit`, `ZZ`/`ZQ`, the
    macro, mark and register notes, and the recording flag.
  - Response tier 1: the ctrl+d/ctrl+u fix, `zo` `zc` `za`, `/` `n` `N`.
- **M5: tier 2 and acceptance.**
  - `.`; sidebar folds; `zR` `zM` `zz` `zt` `zb`; `ctrl+f` `ctrl+b` in
    the response; `*` `#`; `yy` and `V`+`y` in the response.
  - `:e` `:e!`; `ctrl+w` + arrows; command-line history.
  - Then the acceptance sweep (§8.4). Any tier-2 item that proves costly
    moves to a follow-up and its key-list row is marked Parked.

## 10. Open questions

1. **Do the vim letter aliases leave the arrows profile?** Main binds
   `j` `k` `h` `l` `g` `G` on lists and the response, `i` to open fields,
   and bare `u` (undo) and `:` (palette) in every profile. The
   2026-09-18 spec removed them from the non-vim profile (`earlier`). The
   payoff is a truly modeless arrows profile and letters free for the
   shared verbs. **My recommendation: remove them, and keep `q` quit and
   the ctrl+d/u/f/b paging chords in arrows.** If you'd rather keep them,
   they become arrows table rows, and no other part of this spec changes.
2. **Should Enter confirm a one-field prompt in the arrows profile too?**
   On main, Enter only closes the field. Confirming takes Esc, Esc, Enter
   or ctrl+enter. vim-mode changed this for both profiles (9cbfcd1,
   `earlier`). This spec keeps arrows as main has it and gives vim the
   key list's model. **My recommendation: yes, make Enter confirm (and
   Esc cancel) the one-field prompt in both profiles.** It is a two-arm
   change in `modal.rs:1817–1839` plus the footer chips, and renaming
   something should not take three keys.
3. **Five choices in this draft change earlier behaviour or extend the key
   list — review each (all `mine`):**
   - `a` inside an open table cell appends text as Vim does; vim-mode made
     it add a row (`earlier`). Recommendation: Vim's meaning inside an open
     cell, `a` adds a row only on the closed row.
   - `a` on the Manage options grid adds an option, not a variable.
   - The closing `q` of `qa…q` is swallowed, with a "recording — not
     supported" echo until then (key list §9 row, `mine`, not yet
     explicitly accepted).
   - The options-grid edit verb is `:value`, because `:edit` is Vim's
     reload.
   - `ctrl+d`/`ctrl+u` in the response scroll the view as well as the
     cursor — and the fix applies to the arrows profile's paging too.

## 11. Out of scope

- The text engine: key list §1–§5, the Vim conformance harness, and the
  body's operator grammar. That is piece 3; this piece only calls it
  (§3.7).
- The undo journal and list-cursor redesign: reselect after undo,
  `land_on_request`, the cursor rules. That is piece 2, and piece 4
  depends on it. (The core `delete_requests` batch for `Ndd`, vim-mode
  ada2e90, is NOT in piece 2 — it lands here, in M2, on top of piece 2's
  step recording.)
- `/` search and filter in lists. Parked for a future update (`you`).
- Macros, marks, named registers (`"+` is piece 3's tier 2), the
  jumplist (`ctrl+o`/`ctrl+i`), block Visual (`ctrl+v`), and
  `:s` `:g` `:noh` `:set`. They are Out: notes only, never another
  action.
- `H` `M` `L` and `zz` `zt` `zb` in lists (Out).
- Per-profile sections in keys.toml, and rebinding vim sequences through
  keys.toml.
- The seven app fixes of piece 1, already on main, and any app-wide
  finding from the acceptance sweep (backlog, §8.4).
