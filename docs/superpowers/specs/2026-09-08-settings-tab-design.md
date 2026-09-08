# Settings Tab (Manage screen) Design

The global settings in `config.toml` reach the GUI as a fourth Manage
tab, right-anchored in the tab strip so it reads as distinct from the
three project tabs. To free the room for it, the Manage bar's right
cluster empties: `Close` is deleted outright and `Reload All` moves up
into the header bar's save/discard slot, which is unoccupied whenever
the Manage screen is open.

## Scope

One branch. User-visible changes: the Manage bar loses two buttons, the
header gains one on the Manage screen only, and a new tab exposes the
settings listed under "Settings body". No change to what
`Action::ReloadFromDisk` does — only where its button lives and what it
is called.

Explicitly out of scope: splitting reload into project and config
halves (considered and rejected — see "Reload stays whole"), a
keybinding editor, and a theme authoring UI.

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

With both gone the strip no longer competes for width, so its current
"strip has priority, buttons drop rather than overlap" logic goes too.

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
  so the screen lands on something usable instead of an apology. This
  applies only to `Action::OpenManage { tab: None }`; an explicit
  `tab: Some(..)` request (such as `app.rs:3566`'s jump to Environments)
  is honoured as asked and shows the message. The auto-selection sets
  `manage.tab` for real; it is not a display-only override, and the tab
  stays where it landed once a project opens.

## Settings body

A single-column list of labelled rows, following `ManageList`'s
keyboard idiom: up/down move the cursor, enter/space activates the
focused row's control. Every row is also directly clickable.

Primary rows, in order:

| Setting | Control |
|---|---|
| `theme` | Row dispatches `Action::OpenThemeChooser` — the existing chooser, with its live-preview hook, is reused rather than a new dropdown built |
| `animations` | Checkbox |
| `hover_hints` | Checkbox |
| `jq_tab` | Two-state control: `Menu` / `Cycle` |
| `ai_cmd` | `TextField` |
| `ai_confirmed` | Checkbox, worded as consent and inverted: "Ask before sending response shape to the AI command". This flag is set once by the "Always send" choice and is currently unrevocable without hand-editing TOML; exposing it is a privacy requirement, not a convenience |
| `projects.root` | Path plus a Browse… button opening the existing `Modal::FilePicker` |

Behind an "Advanced" disclosure, collapsed by default:

| Setting | Control |
|---|---|
| `clipboard_cmd` | `TextField` |
| `osc52_limit` | Numeric field, in bytes |
| `[animation_ms]` | The nine duration fields, shown dimmed and not editable while `animations` is off |

**New glyphs.** No checkbox icon exists in `glyph.rs`. Two are added
through the `icons!` macro — `nf-md-checkbox-marked` and
`nf-md-checkbox-blank-outline`. Their codepoints must be looked up from
the Nerd Font cheat sheet at implementation time, not recalled; the
existing `every_icon_is_one_cell_wide` test then holds them to the
one-cell rule.

## Writing settings

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

## Deferred

- Keybinding editing (`keys.toml`) and theme authoring
  (`themes/*.toml`) stay text-editor jobs; the tab may point at them but
  does not edit them.
- `projects.known` / `projects.last` stay out: registry state, already
  managed by the Projects chooser, and a second editing surface invites
  drift.
- The drift confirm that `reload_held_requests` can raise on an
  unrelated request during a theme-only reload is a real annoyance, but
  it is a question of when that confirm fires, not of this tab.
