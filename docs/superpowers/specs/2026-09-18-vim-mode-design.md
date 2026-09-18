# Vim Mode

The two rounds before this one (`2026-09-15-shared-aliases-and-field-esc-design.md`
and `2026-09-16-field-states-design.md`) added vim spellings to every
navigation surface and gave every text field a selected/open pair. They
did it *unconditionally*: `j`/`k`/`h`/`l`, `g`/`G`, `i`, `:` and `u`
work today whether or not anyone asked for vim. The deferred item both
specs left behind — "vim mode itself" — was written when those keys
were expected to arrive with the mode. They arrived without it.

So the gap is not what those lists said. An audit of every surface
finds navigation essentially complete and text editing untouched:

- **Navigation surfaces** (sidebar, table editor, Variable Manager,
  Manage list, response, modals) already have `j`/`k`/`h`/`l`,
  `g`/`G`, `ctrl+d`/`u`/`f`/`b`, `i` to open, `d` to delete; the
  response view even has `/`, `n`, `N`.
- **Text fields** have no vim at all. `line_input.rs` binds four
  letter keys and all four are emacs nav bytes (`ctrl+a`, `ctrl+A`,
  `ctrl+h`, at `:388-406`). The body editor is pinned to
  `EditorMode::Insert` at roughly a dozen sites in `editor.rs`.
- **Sequences are impossible.** `g` fires immediately on every surface
  (`sidebar.rs:666`), so `gg` can never work, and `g` is a prefix
  namespace in vim (`gg`, `gt`, `gT`). There are no counts anywhere.

This round builds the mode properly: a real profile, a sequence-aware
router, vim inside text fields, and ex-commands.

## Scope

One branch. User-visible changes:

- A `keymap = "vim" | "default"` setting, chosen on first launch and
  changeable from the Settings tab.
- In **default** mode the vim letters are **removed**: `j`, `k`, `h`,
  `l`, `i`, `g`, `G` and `u` stop navigating. Arrows, Enter, Home/End
  and `ctrl+z` do that work. Those eight letters return to the surfaces
  as free keys.
- In **vim** mode the plain-letter app verbs are removed instead. The
  letters mean what vim means.
- Sequences and counts work: `gg`, `3j`, `10G`, `gt`/`2gt`, `dd`,
  `yy`, `zz`, `ctrl+w h/j/k/l`, and operator+motion in text fields.
- Text fields get Normal/Insert/Visual with motions, the `d`/`c`/`y`
  operators, `p`/`P`, `x`/`X`, `v`/`V` and counts.
- `:` opens the palette in ex mode: `:w`, `:q`, `:new`, `:rename`,
  `:delete`, and the rest of the CRUD verbs.
- The footer shows a mode indicator, the pending sequence, and chip
  labels spelled for the active profile.

Out of scope, under "Deferred": `.` repeat, named registers, marks and
macros, text objects in `LineInput` beyond `iw`/`aw`, blockwise visual,
visual mode on *lists* (multi-row selection), the `ctrl+o`/`ctrl+i`
jump list, and `/` as a filter on the list surfaces.

## The central decision: resolve before dispatch

The mode must not be distributed. Every surface's key handling is a
hardcoded `match`, and the obvious two approaches both scale badly:

- **Ambient mode** (a global or TLS the existing predicates consult)
  turns `plain_letter()` into an order-dependent function. The failure
  mode is silent: a test that forgets to set the mode passes while
  asserting the wrong one. The `ManageList::col_range` TLS arm already
  needed its own review ruling; nine surfaces of this would be worse.
- **Threading a `Mode` parameter** is honest but writes the check out
  at roughly sixty arms, doubles every surface's key tests, and makes
  each future surface remember.

Instead, a `vim` module sits between the event loop and the surfaces
and resolves each key *before* any surface sees it. Surfaces never ask
what mode they are in. They receive `KeyCode::Down` whether the user
pressed `↓`, `j`, or `3j`.

The router takes a raw `KeyEvent` plus one bit — whether the focused
thing is currently accepting text — and returns exactly one of:

- **`Action(a)`** — the sequence resolved to a named action.
- **`Keys(evs)`** — one or more normalized keys the surface already
  handles.
- **`Pending`** — a sequence is incomplete; the key is consumed and
  echoed in the footer.
- **`Declined`** — the router does not own this key; it passes through
  untouched.

The one bit of context is not new state: selected-versus-open is the
model the field-states round already established.

**Counts are applied by replay, never handed onward.** `3j` returns
three `Down` events, not one `Down` carrying a `3`. No surface, and no
edtui call site, ever learns what a count is — which is the same reason
the router exists at all. The single exception lives entirely inside
the `vim` module: in an operator+motion like `d3w` the count belongs to
the motion the operator consumes, and is resolved before any key
leaves. This is also how counts reach edtui, which has none of its own.

`Keys` returning a vector rather than one event is what makes replay
possible; it is the only reason that variant is plural.

**Normalization is part of the contract, not a side effect.** The
open-field set that `opens_field()` defines today — Enter, Space, and
`i` — moves into the router: Space normalizes to the open key in *both*
profiles, and `i` only in vim mode. A silently dropped Space is exactly
the failure §"Risks" names, so the normalize cases are table-tested
beside the decline cases.

### How a verb is "removed" without the surface knowing

Later sections say the plain-letter app verbs are removed in vim mode
and the vim letters are removed in default mode. Neither is a branch in
a surface. Both are the router **claiming or declining** a key:

- In vim mode the router claims `n` as search-next, so it never
  reaches the sidebar and the sidebar's `n` arm is dead code for that
  profile. The arm itself stays written once, unconditionally.
- In default mode the router declines `n`, the sidebar's arm fires,
  and the surface behaves exactly as it does today.

A surface therefore never tests the profile. What changes per profile
is only which keys the router hands it.

Sequences that mean different things on different surfaces resolve the
same way. `dd` does not resolve to a delete-row action directly;
it resolves to a delete intent, which the router turns into the
`Action` the focused surface names for it — delete row in the table,
delete request in the sidebar, delete line in a text field. The
surface-to-action mapping is a table in the `vim` module, not a branch
in each surface.

### What this deletes

The vim aliases stop being duplicated. Today `KeyCode::Char('j') |
KeyCode::Down` and its siblings are written out about fifteen times
across nine files (`sidebar.rs:641`, `table_editor.rs:394`,
`varmanager.rs:1094`, `manage_list.rs:469`, `modal.rs:1711`,
`response.rs:2529`, and more). Each collapses back to the named key
alone. Both predicates that exist only to paper over that duplication
— `plain_letter()` (`keys.rs:47`) and `opens_field()` (`keys.rs:38`) —
are deleted.

Surface tests stay single-mode and keep asserting arrows. Only the
router carries a two-profile test matrix. That is the difference
between a doubling and an addition, and it is the whole reason for
this shape.

### Ordering

The arm-collapsing refactor lands **first**, as its own commit, with
the existing test suite proving default-mode behavior is unchanged.
Only then does the router learn vim. A refactor that changes nothing
and a feature that changes everything must not share a commit.

## The profile

`keymap = "vim" | "default"` is read by `Config` beside the other
settings. Absent from `config.toml` on launch, it raises a
`Modal::Confirm` with two choices — the same first-launch shape the
earlier deferred list described — and the answer is written back. A
Settings-tab row modelled on the `jq_tab` segments changes it later.

`Keymap::default_bindings()` (`keys.rs:341`) becomes profile-aware and
returns one of two tables. A user's `keys.toml` layers on top of
whichever is active and always wins.

### Global chords that change

| Chord | Default mode | Vim mode | Why |
| --- | --- | --- | --- |
| `q` | Quit (`keys.rs:344`) | *unbound* | vim's `q` records a macro; `:q` quits |
| `u` | *unbound* | Undo | vim undo (`keys.rs:424` today, unconditional) |
| `:` | *unbound* | Palette (ex) | vim ex (`keys.rs:352` today, unconditional) |
| `ctrl+r` | Send (`keys.rs:408`) | Redo | vim redo; Send keeps `ctrl+enter` and `shift+enter` |
| `ctrl+s` | Save (`keys.rs:405`) | *unbound* | `:w` |
| `alt+w` / `shift+alt+w` | Cycle split (`keys.rs:417`) | *unbound* | `ctrl+w w` and `ctrl+w h/j/k/l` |
| `alt+←` / `alt+→` | Cycle tabs (`keys.rs:385`) | *unbound* | `gt` / `gT`, and `2gt` |
| `alt+a` | Table add row (`keys.rs:415`) | *unbound* | `o` / `O` |

**Ruling: the vim spelling replaces the chord, it does not alias it.**
Where a verb gains a vim-native spelling in vim mode, the old chord is
unbound there. One way to do each thing per profile.

**`ctrl+v` keeps Paste in both profiles**, and this is not a
compromise. Vim's paste is `p`/`P`, which puts from vim's *own*
register — what `yy` or `dd` just filled. The system clipboard is a
different register, reached in vim as `"+p`. So `p` and `ctrl+v` read
different sources and do not compete; edtui binds `ctrl+v` to paste in
insert mode for the same reason (`key.rs:912`). Blockwise visual, which
`ctrl+v` would otherwise carry, is deferred: edtui's `EditorMode` has
only `Normal`, `Insert`, `Visual` and `Search` (`state/mode.rs:3-9`),
with no blockwise variant, and a block selection is meaningless in a
single-line `LineInput`.

**`ctrl+o` keeps the project chooser in both profiles.** Vim's `ctrl+o`
is jumplist-back, but the jumplist is deferred, so unbinding it here
would buy a dead key and cost the chooser its chord. Revisit if and
when the jumplist is built.

`ctrl+p` (palette) and `ctrl+z` (undo) stay bound in both. `ctrl+p` as
a finder is idiomatic in vim configs, and `ctrl+z` costs nothing beside
`u`. Every other chord — `alt+z/x/c` selectors, `alt+m`, `alt+u`,
`alt+j`/`g`/`k`, `alt+s`, `alt+i`, `alt+t`, `alt+y`, `alt+d`, `alt+e`,
`alt+v`, `alt+p`, `alt+r`, `alt+q`, `ctrl+1..9`, `ctrl+shift+e`,
`ctrl+shift+d` — has no vim meaning and is unchanged in both profiles.

## Default mode gets its letters back

`j`, `k`, `h`, `l`, `i`, `g`, `G` and `u` no longer navigate in default
mode. This is deliberate and it is a removal of shipped behavior: the
shared-aliases round put them everywhere, and this round takes them out
of the profile that did not ask for them. The payoff is eight plain
letters free for per-surface verbs, and two profiles that actually mean
something.

Arrows, Enter, Space, Home/End, PageUp/PageDown and `ctrl+z` carry all
of that work in default mode, as they did before that round.

## Vim mode on navigation surfaces

Plain letters belong to vim. The per-surface app verbs bound to them
today — `n` new, `r` rename, `d` delete, `m` move, `e` edit, `p`
promote, `c` copy, `v` paste, `o`, `s`, `x`, `t`, `a` add, and the
per-surface `q` arms (`varmanager.rs:1146`, `:1299`,
`manage_list.rs:526`) alongside the global one — are all removed in vim
mode. Every one of them has a vim meaning that a vim user's fingers
already expect.

Four take vim spellings directly:

| Verb | Vim mode |
| --- | --- |
| Delete row | `dd` |
| Copy row | `yy` |
| Paste row | `p` (`P` above) |
| Add row | `o` below, `O` above |

`yy` and `p` act on an in-app register, not the system clipboard —
which is what the Variable Manager's `c`/`v` option stash already is
(App state, never leaving its own selector). `ctrl+v` remains the
system-clipboard paste, in both profiles.

The rest — new, rename, move, promote, group fields, reveal, set
default environment — have no vim analogue and are reached **only** by
ex-command. That makes the palette load-bearing rather than optional,
which is why §"Ex-commands" is part of this round and not a follow-up.

Sequences and counts arrive on these surfaces with the router: `gg`
home, `G` end, `3j`, `10G`, `gt`/`gT`/`2gt` for tabs, `ctrl+w h/j/k/l`
and `ctrl+w w` for panes (`Action::FocusPane` already exists at
`action.rs:114`, so the panes are addressable today), and `zz`/`zt`/`zb`
in the response body.

`/` stays where it already works — the response view. Extending it to
the sidebar, table, Variable Manager and Manage list is **not** part of
this round: those surfaces have no filter row and no filter state, so
`/` there is a feature to build rather than a key to bind, and it is
worth having in default mode too. It is listed under "Deferred" as its
own task.

## Text fields

This is where the round's real work is.

### `LineInput`

`LineInput` gains Normal, Insert and Visual layers over the caret,
anchor and selection primitives it already has:

- **Motions**: `w` `b` `e` `W` `B` `0` `^` `$` `f` `t` (and `F`/`T`).
- **Enter insert**: `i` at the caret, `a` one right, `A` end of line,
  `I` first non-blank, `o`/`O` where the surface has rows.
- **Operators**: `d`, `c`, `y` with motions, doubled forms (`dd`,
  `cc`, `yy`) and the capital shorthands `D`, `C`, `Y`.
- **Text objects**: `iw` and `aw` only, so that `ciw` and `daw` work.
  No bracket or quote objects — see "Deferred".
- **Direct edits**: `x`, `X`, `p`, `P`.
- **Visual**: `v`, `V`, with the operators applying to the selection.
- **Counts** on all of the above, and `u` / `ctrl+r`.

**Ruling: vim ops commit through the existing undo path.** The field's
undo sessions (`flush_field_session`, the anchor rules from the
review-round fixes) are untouched. A vim edit is an edit; the
field-Esc rule and one-step-per-close hold exactly as they do now.

**Ruling: Esc gains one step in vim mode.** Insert → Normal, then
Normal → close the field. This is the extra step the shared-aliases
spec predicted. In default mode Esc closes the field as it does today.
Esc also always clears a pending sequence first.

### The body editor

edtui already ships a full vim implementation and it is not feature
gated. `events/key.rs:139` builds a `vim_keybindings()` register keyed
on `Vec<KeyInput>` sequences with a `pending_char`, covering `hjkl`,
`w`/`b`/`e`, `f`/`t`, `0`/`_`/`$`, `i`/`a`/`o`, `v`, `d`/`c`/`x`, `g`,
`n`, `/`, text objects on `"`/`(`/`[`/`{`/`%`, `yy`/`y`/`p`/`P`
(`:803-817`), `u` and `ctrl+r` (`:793-795`).

So the body editor stops pinning `EditorMode::Insert` (the dozen sites
in `editor.rs`, including `:892`, `:969`, `:1020`, `:1054`) and lets
the register drive. Two things the app must still do:

- **Intercept edtui's insert-mode chords.** It binds `ctrl+u`,
  `ctrl+p` and `ctrl+y` in insert mode (`key.rs:998-1000`), which
  collide with app chords. The app claims those first.
- **Supply counts.** edtui has none. The router's replay covers this
  with no special case: `3w` reaches edtui as three `w` events.

In default mode the body stays pinned to Insert exactly as today.

## Ex-commands

`:` opens the existing palette in ex mode. The palette already knows
every named `Action`, so this is a verb-name surface over machinery
that exists:

- `:w` save, `:q` quit, `:wq`, `:q!` (discard changes).
- `:new`, `:rename`, `:delete`, `:move`, `:promote`, `:group`,
  `:reveal`, `:project`, `:manage`, `:theme` — resolved against the
  focused surface, so `:new` means new request in the sidebar and new
  variable in the Variable Manager.
- Unrecognized verbs fall back to the palette's existing fuzzy match
  rather than erroring, so `:` remains a superset of `ctrl+p`.

Because ex is the only keyboard path to the orphaned CRUD verbs, the
reachability guarantee grows an arm: every named action must be
reachable by mouse **and**, in vim mode, by an ex-command or a vim
key. The existing `every_named_action_is_mouse_reachable` test
(`app/tests.rs:16949`) gains a vim-profile sibling.

## Footer, hints and the mode indicator

- Footer chips carry both spellings. The inline `("↑↓", "navigate")`
  tuples scattered across `settings.rs:378`, `manage_list.rs:623`,
  `modal.rs:1619` and their neighbours become two-spelling entries and
  the footer picks by profile.
- A mode indicator — `NORMAL`, `INSERT`, `VISUAL` — sits in the footer
  in vim mode, with the pending sequence echoed beside it the way vim
  echoes to the bottom right. Neither appears in default mode.
- `HintCtx` gains the profile so a hint names the key actually
  dispatched. That is the existing rule from the footer-hints round,
  extended by one axis.

## Implementation shape

- `crates/postui/src/vim/` — a new module. `mod.rs` holds `Mode` and
  the router; `seq.rs` the pending buffer, count accumulator and
  sequence table; `field.rs` the `LineInput` Normal/Visual layer.
- `keys.rs` — `default_bindings()` becomes profile-aware;
  `plain_letter()` and `opens_field()` are deleted.
- `config.rs` — the `keymap` key, its parse, and the first-launch gate.
- Each surface loses its vim-letter arms and, in vim mode, its
  plain-letter app-verb arms.
- `hint.rs` and every `footer_chips()` gain the profile axis.

## Testing

- **Router table tests, per profile.** Key sequence in, resolved
  outcome out: `gg`, `3j`, `10G`, `gt`, `2gt`, `dd`, `ctrl+w h`, a
  pending prefix, and an Esc that clears it. Three cases matter more
  than the verbs:
  - **Decline** — a key the router does not own passes through
    byte-identical.
  - **Normalize** — Space reaches the surface as the open key in both
    profiles, `i` only in vim mode.
  - **Replay** — `3j` yields exactly three `Down` events, and no
    outcome ever carries a count outward.
- **Surface tests stay single-mode** and keep asserting arrows. Their
  vim-letter assertions are deleted with the arms they test.
- **`LineInput` vim unit tests** for each motion, operator and count,
  and for the undo-session boundaries around them.
- **Body editor**: a thin integration test that the register is live
  in vim mode, that Esc leaves insert, and that the intercepted
  insert-mode chords still reach the app. Not a re-test of edtui.
- **Profile parity**: every named action reachable by mouse in both
  profiles, and by keyboard in vim mode.
- **Config**: the first-launch gate, the Settings row, a `keys.toml`
  override winning over both profiles.

## Risks

- **Decline correctness.** A key the router wrongly swallows simply
  vanishes, with no error. Mitigated by making decline explicit in the
  return type, table-testing it, and landing the refactor separately
  first so the suite proves the default-mode path before vim exists.
- **Two engines disagreeing.** `LineInput`'s vim and edtui's vim will
  drift in edge cases — `cw` at the end of a word, `$` on an empty
  line, `dd` on the last row. This spec names the intended behavior
  only for cases we care about and accepts edtui's answer for the
  rest; the alternative is reimplementing edtui.
- **Size.** This is larger than either round that preceded it and it
  touches key handling everywhere. The refactor-first ordering is the
  main control.
- **Removing shipped keys.** Default mode loses eight letters that
  have worked since the shared-aliases round. Deliberate, and the
  first-launch prompt is what makes it survivable.

## Deferred

- **`.` repeat**, named registers, marks and macros. `.` in particular
  would constrain how every edit operation is written, and it is not
  worth that shape.
- **Visual mode on lists** (`V` to mark several rows for a bulk
  delete). The app has no multi-select at all, so this is a feature,
  not a binding.
- **`ctrl+o` / `ctrl+i` jump list.** The app has no concept of visited
  locations to jump between. Until it does, `ctrl+o` keeps the project
  chooser in both profiles.
- **Blockwise visual (`ctrl+v`).** edtui's `EditorMode` has no
  blockwise variant (`state/mode.rs:3-9`) and a block selection means
  nothing in a single-line field, so there is nowhere to put it.
  `ctrl+v` stays the system-clipboard paste in both profiles.
- **Text objects in `LineInput`** beyond `iw`/`aw`. The body gets
  edtui's set; the single-line fields do not need `i{`.
- **`/` as a filter on the list surfaces** — sidebar, table, Variable
  Manager, Manage list. Each needs a filter row and filter state that
  do not exist, which makes it a feature rather than a binding, and it
  is worth having in default mode too. Its own task, independent of
  vim mode.
- **`gq`, `>>`/`<<`, `J` join** and the other line-shaping verbs: the
  body is the only multi-line surface and edtui does not bind them.
