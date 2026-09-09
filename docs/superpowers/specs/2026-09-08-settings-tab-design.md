# Settings Tab (Manage screen) Design

The global settings in `config.toml`, and `keys.toml` alongside them,
reach the GUI as a fourth Manage tab, right-anchored in the tab strip so
it reads as distinct from the three project tabs. To free the room for
it, the Manage bar's right cluster empties: `Close` is deleted outright
and `Reload All` moves up into the header bar's save/discard slot, which
is unoccupied whenever the Manage screen is open.

Landing it also closes a hole the tab would otherwise put on display:
`config.toml` is the last file in the app that silently defaults when it
will not parse. See "Startup never defaults".

## Scope

One branch, with "Startup never defaults" landing first as its own
commit — the Settings tab is what makes that hole visible, and fixing it
first lets the tab assume a parsed file.

User-visible changes: startup refuses a broken `config.toml` instead of
silently running on defaults, the Manage bar loses two buttons, the
header gains one on the Manage screen only, and a new tab exposes the
settings listed under "Settings body" plus per-file Edit… and Reset
buttons for `config.toml` and `keys.toml`. No change to what
`Action::ReloadFromDisk` does — only where its button lives and what it
is called.

Explicitly out of scope: splitting reload into project and config
halves (considered and rejected — see "Reload stays whole"), a
structured keybinding editor (Edit… hands `keys.toml` to `$EDITOR`
instead), and a theme authoring UI.

## Startup never defaults

Lands first, as its own commit: the Settings tab would otherwise render
a screenful of values that are not the user's.

Today `Config::load_from` answers every read failure the same way — *"run
on the defaults and say so"* — and there are two failure tiers behind
that one answer:

- **A syntax error** (`toml::from_str` fails) discards the *whole file*:
  `UiSettings::default()` **and** `ProjectsRegistry::default()`. Since
  `registry.last` is what picks the project at startup
  (`app.rs:461-470`), a stray bracket does not merely reset preferences
  — it can silently open a different project, or none at all.
- **A mistyped key** (`animations = "yes"`) parses, `as_bool()` returns
  `None`, and that key alone falls back to its default — for most keys
  **with no warning at all**. Only `jq_tab` and `[animation_ms]` warn.

This contradicts the rule already applied to project files: a file that
exists but will not parse is fatal and reported, never defaulted or
overwritten (`ProjectContext::open` is fallible; a refused project gives
an empty state with the error shown). `config.toml` is the last file
still defaulting.

**A syntax error now blocks startup** with a modal that cannot be
dismissed into a normal session, showing the parse error and the file
path, offering:

- **Edit…** — the same round-trip described below, on the real file's
  text. On a valid save, startup continues normally.
- **Reset** — the settings reset described below, behind its own
  confirm. `[projects]` is preserved if and only if the file parses far
  enough to recover it; when it does not, the confirm says the project
  list will be lost.
- **Ignore** — starts anyway, on defaults for this session, leaving the
  broken file alone. `Config::edit` refuses to write an unparseable
  file, so nothing persists in this mode — no settings changes, no
  newly-registered projects. That cost is the button's hover hint
  ("Run this session on defaults; nothing will be saved") rather than
  its label: a label that spelled it out ran the button row wider than
  the panel on a narrow terminal, and this modal is raised before the
  user can do anything about their window size.
- **Quit**.

**A mistyped key warns.** Every key that falls back because its value
had the wrong type produces a warning naming the key and the expected
type, matching what `jq_tab` and `[animation_ms]` already do. Silent
per-key defaulting is how a customised app quietly becomes a default
one.

Reload is unchanged: it already refuses a broken file and keeps the
settings in memory. Only startup changes.

## Reload stays whole

`Action::ReloadFromDisk` keeps all four of its current jobs: the forced
project re-read via `resync_project`, the config re-read
(`config.toml`, `keys.toml`, `themes/`), the theme re-resolve, and the
refused-project retry. Splitting it was explored and dropped: the two
halves overlap on the `[projects]` registry (which lives inside
`config.toml`) and on theme resolution (whose inputs span both), and no
user wants one half reloaded but not the other. A split would also make
`alt+r` and the button beside it stop meaning the same thing.

The button is labelled **`Reload`**, not `Reload All` and not
`Reload Config`. Both qualifiers mislead: `project.toml`,
`variables.toml` and `environments/` are config too, so "Config" does
not carve the action, while "All" invites reading it as "all known
projects". The hover hint carries the precision — "Re-read this
project's files and your config from disk". The palette keeps its
existing longer "Reload from disk" wording.

## Manage bar

`draw_manage_bar` (`components/manage.rs`) loses both right-cluster
buttons and becomes nothing but the tab strip across the full bar width.

- **`Close` is deleted.** It is redundant: `Action::OpenManage` toggles
  (`app.rs:4394-4397` — a request for the tab already up closes the
  screen), so the header's Manage chip is already a working close for
  mouse users, and `esc` remains bound.
- **The doc comment at `manage.rs:88-91` is deleted** along with it. It
  justifies dropping `Close` on narrow bars by claiming it "stays
  reachable by key (esc) and through the footer chips" — but no
  unconditional close chip exists in `VarManager::footer_chips`; the
  only `esc` chips there are the `("esc", "cancel", None)` pair shown
  during a live cell edit. The claim is stale and must not be carried
  forward.
- **`Reload All` moves to the header** (below).

With both gone the strip no longer competes with buttons for width, so
the current "strip has priority, buttons drop rather than overlap" logic
goes too. Width competition does not disappear, though — it moves
*inside* the strip, between the three left tabs and the right-anchored
Settings. See "Narrow bars".

## Header bar

`header_bar.rs` gains a `Reload` chip in the slot the save/discard group
occupies, gated on `screen == Screen::Manage`.

- The two are **mutually exclusive by construction**: save/discard
  requires `screen == Screen::Main` (`ui.rs:80`), so no arbitration
  between them is needed.
- Registered as `Hit::FooterChip(Action::ReloadFromDisk)`, matching how
  save/discard already register (`header_bar.rs:337-338`). No new `Hit`
  variant, and clicks route through existing footer-chip dispatch.
- Same keycap-then-name idiom as its neighbours: an `alt+r` pill then
  the label.
- Inherits the slot's layout: anchored at `manage_x - SAVE_GROUP_GAP`,
  dropping rather than overlapping the selectors on a narrow bar.
- Shown whether or not a project is open — with none, it is the
  affordance for the refused-project retry.

The retry's existing hazard is unchanged, only relocated: it runs
`SwitchProject`, so it can raise the unsaved-request confirm and reset
the session. That is current behaviour and stays as-is.

## Tab strip

`ManageTab` gains a `Settings` variant, appended to `ALL`.

- **Cycling includes it.** `alt+←/→` (`app.rs:9039-9045`) walks `ALL`
  with wrapping, so `alt+→` from Spaces lands on Settings and wraps on
  to Variables. The right-anchored position is a grouping cue, not a
  claim of being outside the rotation.
- **`TabStrip` grows a split layout.** `TabStrip::spans`
  (`paint/chip.rs:88-98`) currently lays tabs out strictly contiguously
  (`x += width + 2`) with no notion of a right-anchored tail. It gains
  one, rather than the Manage bar composing two strips: the underline
  animation reads its geometry from `ManageTab::strip_spans()`, and two
  sources of truth for where the accent sits would drift.
- **The underline glides across the gap first.** Build the plain glide
  and look at it. If the sweep through empty space reads as broken, the
  fallback is to snap across the group boundary while keeping the glide
  among the three project tabs — a small change, with precedent and
  machinery already in place at `app.rs:4409-4412`, where a freshly
  opened screen clears `AnimKey::TabUnderline` to snap rather than glide
  from wherever the strip last was. Two further options, if neither
  satisfies: fade out/in, or a rubber band that stretches the underline
  across the gap and contracts onto the target (cheap, because position
  and width already animate as separate keys, `AnimKey::TabUnderline`
  and `AnimKey::TabUnderlineWidth`).

### Narrow bars

Settings keeps a **minimum 2-column gap** after Spaces. Because
`TabStrip`'s inter-tab gap is also 2, the layout degrades with no
fallback branch and no visual jump: as the bar narrows, Settings slides
leftward until it sits exactly 2 columns after Spaces, at which point
the right-anchored strip *is* a contiguous strip.

The floor is **50 columns** — `Variables` 11 + gap 2 + `Environments`
14 + gap 2 + `Spaces` 8 = 37, plus the 2-column minimum gap, plus
`Settings` at 10, plus the 1-column left edge. Below that the strip
clips at the right edge rather than dropping a tab, as the header's
Manage chip already does; every tab stays reachable by `alt+←/→`
regardless.

## No project open

Today `draw_manage_without_a_project` (`ui.rs:668-685`) replaces the
whole Manage body, on every tab, with a centered "no project is open —
open or create one first". Settings is the one tab that works without a
project, so that blanket rule becomes tab-conditional.

- The three project tabs keep today's message verbatim, and stay
  **clickable rather than disabled** — the message explains the state
  better than a dead tab does.
- Settings renders normally.
- **Opening the Manage screen with no project auto-selects Settings**,
  so the screen lands on something usable instead of an apology. It
  applies only to `Action::OpenManage { tab: None }`; an explicit
  `tab: Some(..)` request (such as `app.rs:3566`'s jump to Environments)
  is honoured as asked and shows the message. The auto-selection sets
  `manage.tab` for real; it is not a display-only override, and the tab
  stays where it landed once a project opens.
- **It fires only on the opening path.** `OpenManage { tab: None }` is
  both the open *and* the close half of the toggle
  (`app.rs:4394-4397`): with the screen already up it closes. The
  auto-selection must be evaluated only when `screen != Screen::Manage`,
  or pressing `alt+v` to close would reopen on Settings instead.

## Settings body

Two sections. A single-column list of labelled rows, following
`ManageList`'s keyboard idiom — up/down move the cursor, enter/space
activates the focused row's control — then a Files section of per-file
buttons. Every row and button is also directly clickable.

```
Settings
  Animations                    [✓]
  Hover hints                   [✓]
  jq Tab behavior         [Menu|Ghost]
  AI command        [ claude -p        ]
  Ask before sending to AI       [ ]
  Clipboard command [                  ]
  OSC 52 limit      [ 65536            ]

Files
  Settings (config.toml)     [Edit…] [Reset]
  Key bindings (keys.toml)   [Edit…] [Reset]
```

| Setting | Control |
|---|---|
| `animations` | Checkbox |
| `hover_hints` | Checkbox |
| `jq_tab` | Two-state control, `Menu` / `Ghost` |
| `ai_cmd` | `TextField` |
| `ai_confirmed` | Checkbox, worded as consent and inverted: "Ask before sending response shape to the AI command". This flag is set once by the "Always send" choice and is currently unrevocable without hand-editing TOML; exposing it is a privacy requirement, not a convenience |
| `clipboard_cmd` | `TextField` |
| `osc52_limit` | Numeric field, in bytes — see "Validation" |

### Keyboard

The tab is fully operable without a mouse, like every other surface.
Following the established idiom rather than a Tab ring over every
control:

- Up/down move a single cursor through **every** row in order, the two
  Files rows included.
- Enter or space activates the focused row: a checkbox toggles, the
  two-state control advances, a `TextField` enters edit mode, a Files
  row's buttons activate.
- A Files row holds two buttons; left/right choose between Edit… and
  Reset, and enter activates the chosen one.
- In a live `TextField` edit, enter commits and esc cancels — the same
  pair the Manage grid already advertises.
- The footer advertises the focused row's keys, as every other focused
  area does. The Settings tab supplies its own chip set through the
  same path the other tabs use (`ui.rs:273-275`); it must not inherit
  whatever the previously open tab left behind.

### Validation

`osc52_limit` is the one field whose text is not free-form. On commit,
input that is not a non-negative integer within `usize` is **rejected**:
the previous value stands, and a toast says what was expected. It is
never silently coerced or zeroed.

`ai_cmd` and `clipboard_cmd` are free-form shell commands and are not
validated here — only the first word is ever looked up on `PATH`, and
that gating already exists.

### While `config.toml` is broken on disk

Startup can no longer begin in this state, but a reload can *enter* it:
the file goes bad while the app runs, reload refuses it, and the
settings in memory stay as loaded. The tab must not pretend otherwise.
A banner across the top says the file has a syntax error and that the
values shown are the ones the session started with, and every row is
disabled — writes would be refused anyway, since `Config::edit` will not
write over a file it cannot parse. **Edit… stays enabled**: it is the
way out.

**`theme` is deliberately absent** — the header's Theme chip is present
on every screen, and a second control for it would be redundant.
**`projects.root` is also absent**: it lives in the `[projects]` table,
which Reset deliberately does not touch (see "Reset"), so a row for it
would sit inconsistently among rows that Reset does cover. It stays
reachable through Edit…; a home for it near project creation is a
separate question.

**`jq_tab` gains a `"ghost"` spelling.** `JqTab::Cycle`'s behaviour is
ghosting the best candidate after the caret, so `Ghost` is the label.
`"ghost"` becomes the value written going forward; `"cycle"` is parsed
forever as its older name, exactly as `"accept"` is already kept as the
older name for `"menu"`. The invalid-value warning in
`UiSettings::from_value` is updated to name all accepted spellings.

**New glyphs.** No checkbox icon exists in `glyph.rs`. Two are added
through the `icons!` macro — `nf-md-checkbox-marked` and
`nf-md-checkbox-blank-outline`. Their codepoints must be looked up from
the Nerd Font cheat sheet at implementation time, not recalled; the
existing `every_icon_is_one_cell_wide` test then holds them to the
one-cell rule.

## Writing settings

There is no save/discard step and no undo integration. Changes apply and
persist immediately, as the Theme chip already does today.

Save/discard is unavailable anyway — the header slot it would occupy now
holds `Reload` on this screen — and undo is project-scoped
(`StepKind`, the journal, `History`), so folding config changes into it
would interleave "undo my hover-hints change" with "undo my request
edit" in one stack, which cannot honour the *undo restores exactly*
rule. Neither is needed: a checkbox toggled by accident is undone by
clicking it again, and text fields commit on enter and cancel on esc,
the same live-edit idiom the Manage grid already uses. That leaves the
two Reset buttons as the only hard-to-reverse actions, and both are
behind confirms.

Each change writes through `Config::edit`, which preserves every
unrelated key byte-for-byte and **refuses to write a `config.toml` that
does not parse**. That refusal is a real, reachable state: a user with a
broken config file toggles a checkbox and the write is rejected. It must
raise an error toast naming the file, never fail silently, and the
control must revert to the on-disk value rather than showing a state
that was not persisted.

After a successful write the in-memory settings are applied through the
same path `ReloadFromDisk` uses (`reapply_ui_settings`), so a change
made in the tab and a hand-edit picked up by `alt+r` converge on one
code path.

## Edit… — the external editor round-trip

One mechanism serves both files, differing only in which validator runs.

1. Seed the file if it does not exist (see "Seeding"), then copy its
   text verbatim into a temp file via `hostfs::editor_tempfile`. Editing
   a copy rather than the live file means a broken save can never leave
   the app's real config unusable, and `Config::edit`'s parse-refusal can
   never fire spuriously.
2. Run `$EDITOR` through the existing `run_editor_and_restore`
   machinery: `vi` fallback, whitespace-split so `code -w` works, full
   TUI teardown and rebuild.
3. On exit, validate the temp file's text — `toml::from_str` plus
   `UiSettings::from_value` for `config.toml`,
   `Keymap::try_from_overrides` for `keys.toml`.
   - **Valid** → write the text back atomically through `Config`, apply
     live, discard the temp file. For `keys.toml`, also surface
     `Keymap::caret_warnings` as toasts, so a rebind that costs a macOS
     caret gesture says so rather than applying silently.
   - **Invalid** → a modal showing the parse error, offering **Keep
     editing** (reopens `$EDITOR` on the *same* temp file, so the user's
     work is still there) and **Discard** (removes the temp file; the
     live file is untouched).

**`Config` gains a raw-text write.** Config files are `Config`'s to own
(a `fs_lint` test enforces that only `disk.rs` and `hostfs.rs` may name
`std::fs`), but `Config::edit` is the wrong door here: it operates on a
parsed `toml_edit::DocumentMut` and refuses input that will not parse.
The round-trip has *already* validated the text and must preserve it
byte-for-byte, comments and all. So `Config` gains a method that writes
validated text verbatim and atomically — distinct from `edit`, and
documented as being for text that a validator has just accepted.

**`run_editor_and_restore` needs two changes**, not one:

- It removes the file unconditionally today (`main.rs:361`), which would
  destroy the temp file between "invalid" and "Keep editing". Removal
  moves to the caller, once the outcome is settled.
- Its `read_back: bool` is hardwired to feed the edited text into the
  *request body*. That coupling breaks: the parameter becomes the edited
  text handed back to the caller (or nothing, for the view-only case),
  and each caller decides what it means. `OpenBodyInEditor` and
  `OpenResponseInEditor` keep their current behaviour through it.

**Keep editing is a loop.** Re-entering the editor means parking the
action in `App::pending_terminal_action` again, since only the main loop
may suspend the terminal. It can repeat indefinitely; each pass reuses
the same temp file, and only Discard or a valid save ends it.

## Seeding

Both files are seeded on first Edit… when absent, so the editor never
opens an empty buffer.

- `config.toml` is seeded with every setting **commented out** at its
  default value.
- `keys.toml` is seeded with every entry from `named_actions()`,
  **commented out**, at its current binding.

Commented rather than live, in both files, and for the same reason:
`keys.toml` is an overrides file, and a fully uncommented seed would pin
every binding forever — ship a changed default later and anyone who
seeded would never receive it. Commented, the file documents everything,
uncommenting one line overrides exactly that line, and defaults keep
flowing for the rest.

**The seeder needs a plural accessor.** `Keymap::combo_for`
(`keys.rs:517`) returns a *single* combo, but an action can hold several
— the vim-key aliases share actions with the arrow keys — and
`apply_overrides` does `bindings.retain(|_, a| *a != action)` before
binding, replacing *all* of an action's combos. Seeding from the
singular accessor would emit `# scroll_down = ["down"]`, and a user
uncommenting that line would silently lose `j`. A `combos_for` listing
every combo bound to an action is required, so each seeded line is
faithful to what is actually bound.

## Reset

One Reset per file, each behind a confirm. Config changes are outside
the undo system, so the modal is the only guard.

- **Settings reset** clears the `UiSettings` keys from `config.toml` by
  **removing** them rather than writing default values, so the file
  stays minimal and unrelated keys survive byte-for-byte — the pattern
  `ProjectsRegistry::write_into` already uses for `root` and `last`.
  It **does not touch the `[projects]` table**: wiping `known`/`root`/
  `last` would silently destroy the user's project list, which has
  nothing to do with resetting preferences. The confirm says so.
- **Key bindings reset** truncates `keys.toml` back to the commented
  seed, restoring `Keymap::default_bindings()`. Truncating rather than
  deleting avoids needing a file-removal capability that `Config` does
  not have. With no `keys.toml` on disk it writes the seed, so the
  outcome is the same file either way rather than a no-op.

Resetting settings clears `ai_confirmed` to `false`, so the AI consent
prompt returns on the next use. That is correct, not a side effect to
design around.

## Reload while the Settings tab is open

`alt+r` re-reads `config.toml` underneath the tab's own controls. If a
`TextField` is mid-edit (`ai_cmd`, say), a reload would otherwise stomp
what is being typed.

An in-progress field edit is **preserved across a reload**, matching the
rule the editor buffer already follows: reload re-reads what is on disk
around unsaved work, it does not discard it. Every other row updates to
the reloaded values.

## Testing

- The Manage bar renders with no `Close` and no `Reload All`; the
  existing assertion at `manage.rs:340` is updated, not deleted.
- The header shows `Reload` on the Manage screen and not on Main; the
  save/discard group still shows on Main when dirty and never appears
  alongside `Reload`.
- The header chip's rect registers as
  `Hit::FooterChip(Action::ReloadFromDisk)` and a click dispatches it.
- `alt+→` from Spaces reaches Settings and wraps to Variables.
- `strip_spans()` right-anchors Settings and the three project tabs stay
  left-anchored, at a width where both fit and at one where they do not.
- With no project: `OpenManage { tab: None }` lands on Settings;
  `OpenManage { tab: Some(Environments) }` lands on Environments and
  shows the no-project message; the Settings body renders its controls.
- A settings write lands in `config.toml` leaving unrelated keys
  untouched; a write against an unparseable `config.toml` is refused,
  toasts, and leaves the control showing the on-disk value.
- Every new glyph passes `every_icon_is_one_cell_wide`.
- `jq_tab = "ghost"` and `jq_tab = "cycle"` both parse to `JqTab::Cycle`;
  a write from the tab emits `"ghost"`; an unrecognised value still
  warns, naming every accepted spelling.
- The seeded `keys.toml` round-trips: uncommenting a seeded line
  reproduces the binding it names, including every alias, so no combo is
  lost. This is the regression test for the `combos_for` requirement.
- The editor round-trip: valid text is written back and applied; invalid
  text leaves the live file untouched and offers Keep editing / Discard;
  Keep editing reopens the *same* temp file with the user's edits
  intact; Discard removes the temp file. A `keys.toml` apply that trips
  a caret conflict toasts the warning.
- Settings reset removes the `UiSettings` keys and leaves the
  `[projects]` table intact; keys reset restores the default bindings,
  and writes the seed when no `keys.toml` exists.
- Startup with a syntax error in `config.toml` raises the blocking modal
  and does **not** reach a normal session; "Ignore" runs on defaults and
  every subsequent settings write is refused. Startup with a *valid* file is unaffected.
- A mistyped key (`animations = "yes"`) warns, naming the key and the
  expected type, and does not silently default.
- The strip right-anchors Settings at 80 columns, degrades to exactly
  contiguous at 50, and clips rather than dropping a tab below it.
- `alt+v` with the Manage screen already open **closes** it and does not
  reopen on Settings, with and without a project.
- The Settings tab publishes its own footer chips and never shows the
  previously open tab's.
- Up/down reach every row including the two Files rows; left/right
  choose between Edit… and Reset within a Files row.
- `osc52_limit` rejects non-numeric and out-of-range input, keeps the
  previous value, and toasts.
- With `config.toml` broken on disk after a reload, the tab shows the
  banner, disables its rows, and leaves Edit… enabled.

## Deferred

- Theme authoring (`themes/*.toml`) stays a text-editor job.
- A structured keybinding editor. Edit… on `keys.toml` covers the need
  for now.
- `projects.known` / `projects.last` stay out: registry state, already
  managed by the Projects chooser, and a second editing surface invites
  drift. `projects.root` is out of the row list for the same reason
  (Reset does not cover `[projects]`), but wants a home near project
  creation eventually.
- The drift confirm that `reload_held_requests` can raise on an
  unrelated request during a config-only reload is a real annoyance, but
  it is a question of when that confirm fires, not of this tab.
