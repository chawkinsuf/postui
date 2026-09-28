# Vim profile — target key list

Status: approved by the user 2026-09-28 (drafted by Claude 2026-09-27); the
user's decisions on the open questions are applied (see "Decisions" at the
end). This file is the source. The earlier review copy on the Claude Docs
page "Vim profile — target key list (draft)" is superseded and out of date.
Implemented by pieces 3 (text engine: §1–§5) and 4 (vim profile: §6–§9).

## How to read this

- **Tier 1** — a vim power user reaches for it in the first minutes; missing
  it breaks "this is vim". Ships in the first vim release.
- **Tier 2** — noticed within a day or two of real use. Ships in the first
  release if it costs little, otherwise right after.
- **Out** — deliberately not supported. Typing it must never do something
  else by surprise (see §9).
- **Test** — `V`: compared against real Vim 9.1 (text editing, via the
  comparison harness). `S`: a spec test in postui (app actions that have no
  Vim equivalent to compare with).
- **Who** — `you`: a rule you stated in your own words (quoted in memory).
  `mine`: my recommendation — accept, change or reject. `earlier`: decided
  during the vim-mode rounds but I can't show you stated it; treat as mine.

Everything in §1–§4 applies to every text field: the URL, table cells,
prompt inputs, form fields, the jq bar where noted, and the request body.
Multi-line keys (`j`, `k`, `o`, `J`, …) apply to the body only; §5 says what
they do in one-line fields.

Pinned editing settings (the comparison runs Vim with the same):
`expandtab shiftwidth=2 autoindent`, Vim's default `whichwrap=b,s`
(`Backspace` and `Space` wrap to the previous/next line in Normal, `h`
and `l` do not), `selection=inclusive`. The full list is piece 3 §3.4.
— Who: mine.

---

## 1. Text: moving (Normal and Visual)

| Keys | Tier | Test | Notes |
|---|---|---|---|
| `h` `j` `k` `l`, arrow keys | 1 | V | `j`/`k` body only |
| `w` `b` `e` `W` `B` `E` | 1 | V | |
| `ge` `gE` | 2 | V | |
| `0` `^` `$` | 1 | V | |
| `gg` `G`, `{N}G` | 1 | V | body; land on first non-blank |
| `f` `F` `t` `T` `;` `,` | 1 | V | |
| `%` | 1 | V | brackets/braces, nesting-aware |
| `{` `}` | 2 | V | body (blank-line paragraphs) |
| `ctrl+d` `ctrl+u` | 2 | V | body; window height pinned in the harness |
| `ctrl+f` `ctrl+b`, `H` `M` `L` | 2 | V | body |
| `zz` `zt` `zb` | 2 | V | body |
| Counts on every motion (`5j`, `3w`, `2f,`) | 1 | V | |
| `/` `?` `n` `N` `*` `#` search, also after operators (`d/foo`) and in Visual | 2 | V | second wave; prompt on the body's bottom row, matches highlighted, `n`/`N` wrap. Pattern syntax: a subset of Vim's regex (piece 3 §3.14). Offsets (`/foo/e`), `gn`, `g*` Out — you (accepted my recommendation) |

## 2. Text: changing (Normal)

| Keys | Tier | Test | Notes |
|---|---|---|---|
| `d` `c` `y` + any motion or text object | 1 | V | the operator grammar edtui lacks |
| `dd` `cc` `yy`, `D` `C`, `x` `X`, `s` `S` | 1 | V | |
| `Y` | 1 | V | Vim's `Y` = `yy` (Neovim changed it to `y$`; we follow Vim 9.1) — mine |
| `p` `P` (charwise and linewise) | 1 | V | |
| `r{c}`, `{N}r{c}` | 1 | V | |
| `J` | 1 | V | body |
| `u` `ctrl+r` (with counts) | 1 | V | |
| `.` repeat, with counts | 1 | V | every change, incl. inserts |
| Counts on operators (`3dd`, `d3w`, `2p`) | 1 | V | |
| `~`, `g~` `gu` `gU` + motion | 2 | V | |
| `>>` `<<`, `>` `<` + motion | 2 | V | body |
| `ctrl+a` `ctrl+x` (increment/decrement number) | 2 | V | handy for JSON numbers |
| `R` (replace mode) | 2 | V | |
| `gJ` | Out | | rare |

## 3. Text objects (after an operator or in Visual)

| Keys | Tier | Test | Notes |
|---|---|---|---|
| `iw` `aw` `iW` `aW` | 1 | V | |
| `i"` `a"` `i'` `a'` | 1 | V | |
| `i(` `a(` `ib` `ab`, `i[` `a[`, `i{` `a{` `iB` `aB` | 1 | V | nesting-aware and multi-line — the key JSON commands |
| `ip` `ap` | 2 | V | body |
| `it` `at` (tags), `is` `as` (sentences) | Out | | not useful in request bodies |

## 4. Text: Insert and Visual modes

| Keys | Tier | Test | Notes |
|---|---|---|---|
| `i` `a` `I` `A` | 1 | V | |
| `o` `O` | 1 | V | body; see §5 for one-line fields |
| `Esc` (and `ctrl+[`) leaves Insert, caret steps back one | 1 | V | |
| Insert: `Backspace`, `ctrl+w` (word back), `ctrl+u` (to line start) | 1 | V | |
| Insert: `ctrl+o {cmd}` (one Normal command) | 2 | V | Insert only; in Normal `ctrl+o` stays the project chooser — you (accepted my recommendation) |
| Insert: `ctrl+r {reg}` (paste register) | 2 | V | |
| Insert: `ctrl+t` `ctrl+d` (indent) | 2 | V | body |
| `gi` (insert where you last left Insert) | 2 | V | |
| Counts on inserts (`3ia<Esc>`) | 2 | V | |
| `v` `V`, motions extend, `o` swaps ends | 1 | V | |
| Visual `d` `c` `y` `x` `p` `r` `J` `~` `u` `U` `>` `<` | 1 | V | |
| `gv` (reselect) | 2 | V | |
| `ctrl+v` block Visual | Out | | niche; `ctrl+v` keeps its paste meaning — earlier |

## 5. Text: one-line fields specifically

| Situation | Proposal | Who |
|---|---|---|
| Fields have two levels: closed (the row/slot is selected) and open (editing). `i` `a` `I` `A` `Enter` open it; `Esc` from Normal closes it. | keep | earlier |
| A field opens in Normal when entered by keyboard, Insert when created by an add verb (`a` on a table) or when it is a prompt/command line | keep | earlier |
| `j` `k` inside an open field | leave the field and move to the next/previous row, like closing then moving | you (accepted my recommendation) |
| `o` `O` `J` in a one-line field | no-op (there is no second line) | mine |
| `dd` `cc` `yy` in a field | whole field text (a field is one line) | earlier |
| Field register and body register | one shared unnamed register across all text | mine |
| Paste of multi-line text into a one-line field | joined with spaces (as today's paste) | mine |

## 6. Lists and tables

Lists: the sidebar, the Manage lists (Variables, Environments, Spaces), the
variable picker lists. Tables: Headers / Params / Vars rows.

| Keys | Tier | Test | Notes / Who |
|---|---|---|---|
| `j` `k` `gg` `G`, counts, `ctrl+d` `ctrl+u` | 1 | S | |
| `h` `l` between cells (tables) | 1 | S | earlier |
| `Enter` / `o` open a row; `i` opens a cell | 1 | S | earlier |
| `a` add, `r` rename (lists), `m` move (sidebar) | 1 | S | same letters in both profiles — **you** (shared single-letter keys) |
| `dd` delete, never asks to confirm | 1 | S | **you** (delete never confirms) |
| `{N}dd` deletes N rows as one undo step | 2 | S | earlier |
| `yy` then `p` / `P` duplicates below / above | 1 | S | earlier |
| `u` `ctrl+r` app undo/redo, reselecting what came back | 1 | S | undo restores exactly — **you** |
| `/` search or filter the list, `n` `N` | Out | | parked for a future update: a search that only moves to one row isn't enough, and the scope is already large (you) |
| `zo` `zc` `za` fold/unfold sidebar folders | 2 | S | mine |
| `.` repeats the last list action (e.g. `dd..`) | 2 | S | you (accepted my recommendation); each repeat is its own undo step |
| `H` `M` `L`, `zz` `zt` `zb` in lists | Out | | little value in short lists |

## 7. Response viewer (Tree / Raw / Headers)

| Keys | Tier | Test | Notes |
|---|---|---|---|
| `j` `k` `gg` `G`, counts | 1 | S | |
| `ctrl+d` `ctrl+u` scroll the view AND move the cursor by half a page | 1 | S | sweep 7 found only the cursor moves today; the fix applies to the arrows profile too — you (accepted my recommendation) |
| `ctrl+f` `ctrl+b`, `zz` `zt` `zb` | 2 | S | |
| `zo` `zc` `za`, `zR` `zM` (open/close all) | 1 / 2 | S | `zR`/`zM` tier 2 |
| `/` `n` `N` search with a match counter | 1 | S | |
| `*` `#` search for the word under the cursor | 2 | S | |
| `yy` copy the line / node value | 2 | S | mine |
| `v` `V` select + `y` in Raw | 2 | S | mine |

## 8. Panes, tabs, command line

| Keys | Tier | Test | Notes / Who |
|---|---|---|---|
| `ctrl+w h/j/k/l`, `ctrl+w w/W/p` by pane geometry (sidebar left; editor over response) | 1 | S | earlier |
| `ctrl+w` + arrow keys | 2 | S | |
| `gt` `gT` `{N}gt` on every tab strip (editor, response, Manage) | 1 | S | earlier |
| `:w` `:q` `:q!` `:wq` `:x` `:qa` `:wa` | 1 | S | |
| `:e` / `:e!` reload from disk | 2 | S | |
| `:` + app verbs (`:send`, `:new`, `:rename`, `:manage`, …) | 1 | S | earlier |
| Command line editing: `ctrl+u` `ctrl+w`, `Backspace`, `Esc` cancels | 1 | S | |
| Command-line history (`↑` `↓`) | 2 | S | |
| `:s`, `:g`, `:set …` | Out | | show no match; never run something else |
| `:noh` | Out, then 2 | S | Out until body search ships; then it clears the search highlight |

## 9. Prompts, pickers, dialogs, and everything global

| Keys | Tier | Test | Notes / Who |
|---|---|---|---|
| A prompt opens in Insert; `Esc` → Normal; `Esc` again cancels; `Enter` confirms from anywhere | 1 | S | earlier. In the arrows profile too, `Enter` confirms and `Esc` cancels a one-field prompt — you (accepted my recommendation) |
| Pickers (palette, `{{` variables, choosers): type to filter, `ctrl+n` `ctrl+p` move | 1 | S | earlier |
| Confirm dialogs answer with their letter keys (`y`/`n`, `s`/`d`), `Esc` cancels | 1 | S | earlier |
| `ZZ` `ZQ` | 1 | S | follow Vim: `ZZ` = `:x`, `ZQ` = `:q!` — you (accepted my recommendation; replaces the vim-mode branch's save/discard-without-quitting) |
| Macros `q{reg}` … `q`, `@{reg}` | Out | S | Shows "not supported"; must never type into a field or eat the next key. After `q{reg}` shows the note, the closing `q` is swallowed too (fixes the `qa x q @a` → INSERT problem sweep 7 found) — you (accepted my recommendation) |
| Marks `m{a-z}`, `'` `` ` `` jumps | Out | S | `m` keeps its list meaning (move) — you (shared letters) |
| `"0` (the yank register: `"0p` puts the last yank even after a delete) | 2 | V | first wave (piece 3 §5) — mine |
| Named registers `"a`–`"z`, `"+` | Out / 2 | | `"+` (system clipboard) tier 2 — mine |
| Jumps `ctrl+o` `ctrl+i` | Out | | `ctrl+o` keeps the project chooser — earlier |
| Any unsupported key | — | S | does nothing visible except, where a vim user would expect something, a short "not supported" note; never arms a half-typed state that eats the next key |
| The footer echoes counts and half-typed commands (`3`, `d`, `2d`, `g`) | 1 | S | earlier |
| Mode indicator (NORMAL / INSERT / VISUAL) always names the thing taking the keys | 1 | S | earlier |

---

## Decisions (user, 2026-09-27)

1. `ZZ` / `ZQ` follow Vim: `ZZ` = `:x`, `ZQ` = `:q!` (accepted my
   recommendation).
2. `j` / `k` inside an open one-line field leave the field and move rows
   (accepted my recommendation).
3. `.` on list actions is tier 2 (accepted my recommendation).
4. `/` search in lists is parked for a future update: "i dont think having a
   search that just highlights one row is good enough", and the scope is
   already large.
5. Items marked `earlier` stay as the defaults unless the user objects to a
   specific row (accepted my recommendation).

Later on 2026-09-27, while reviewing the piece 2–4 specs, the user accepted
these recommendations of mine (each is a decision on that question, not a
standing rule):

6. Search inside the body (`/ ? n N * #`) is tier 2, second wave (§1 row;
   piece 3 §3.14). Patterns use a subset of Vim's regex. Anything outside
   it shows "pattern not supported" and searches nothing.
7. Insert `ctrl+o` runs one Normal command. In Normal, `ctrl+o` stays the
   project chooser.
8. The vim letter aliases (`j k h l g G`, `i`, bare `u` and `:`) leave the
   arrows profile. Arrows keeps `q` quit and the ctrl+d/u/f/b chords.
9. `Enter` confirms a one-field prompt in both profiles.
10. The closing `q` of `q{reg}…q` is swallowed. `a` in an open cell
    appends. `a` on the options grid adds an option, and its edit verb is
    `:value`. The response `ctrl+d`/`ctrl+u` fix also applies in arrows.
