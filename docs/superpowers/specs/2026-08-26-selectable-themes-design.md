# Selectable Themes with Preview — Design

Date: 2026-08-26
Status: approved

## Goal

Make the color scheme selectable in-app and editable on disk: a theme
picker (command-palette entry) listing built-in themes, custom theme files,
and the live terminal palette, with per-row color swatches and live
apply-as-you-highlight. Also fixes two related issues found during design:
the flaky terminal-colors-at-startup race, and the footer chips
highlighting the shortcut instead of the action.

## Decisions

- **Theme = named seeds.** A theme remains the existing six-seed set
  (`bg`, `fg`, `accent`, `success`, `warning`, `error`); `Theme::generate`
  keeps deriving every other token. No new token surface.
- **Editing is file-based, selection is in-app.** Custom themes are TOML
  files edited in a text editor; the app provides no color-editing UI.
  The picker rescans theme files each time it opens, so iterating on a
  custom theme needs no restart (reselecting the theme re-applies it).
- **Preview: swatches + live apply.** Each picker row shows a six-swatch
  strip; moving the highlight re-themes the entire app immediately. Esc
  reverts to the prior theme; Enter commits and persists to `config.toml`.
- **Built-ins:** Terminal (dynamic), Dark (current Tokyo-Night-adjacent),
  Light, Gruvbox Dark, Gruvbox Light, Catppuccin Mocha, Solarized Dark,
  Solarized Light.
- **OSC race fix: raise the deadline to 600ms.** The read loop already
  exits early on the DA1 fence, so responsive terminals see no delay;
  only a silent terminal waits the full deadline, once, at startup.
- **Footer emphasis inversion:** the action label becomes the prominent
  element, the shortcut keycap the quiet one.
- **Brightness setting: deferred.** Theme selection/editing provides the
  same lever. If muted text still reads too dark on macOS after the
  inversion and a theme the user likes, revisit contrast (or a knob) then.

## Architecture

### Theme registry (`theme/mod.rs` + new `theme/builtin.rs`)

`ThemeChoice` (Terminal/Dark/Light) is replaced by a name-based registry:

```rust
pub struct ThemeEntry {
    pub name: String,        // stable id, kebab-case: "gruvbox-dark"
    pub label: String,       // display: "Gruvbox Dark"
    pub source: ThemeSource, // Builtin(Seeds) | Terminal | Custom(PathBuf)
}

pub struct ThemeRegistry {
    entries: Vec<ThemeEntry>, // Terminal first, then built-ins, then custom (sorted by name)
}
```

- `ThemeRegistry::load(config_dir)` assembles Terminal + built-ins + every
  `themes/*.toml` under the config dir. A malformed custom file yields a
  startup-style warning and is skipped (same posture as bad config values).
- `registry.resolve(name, term) -> (Theme, Vec<String> /* warnings */)`:
  looks up by name; unknown names warn and fall back to `terminal`
  (preserving today's behavior for the legacy `"dark"`/`"light"`/
  `"terminal"` values, which remain valid names).
- The Terminal entry queries via the existing `TerminalPalette` trait and
  keeps the existing fallback-to-Dark-seeds behavior when silent. The
  queried `QueriedColors` is cached on the app after the startup query so
  the picker's Terminal swatches (and live re-apply) never re-query
  mid-session.

Built-in seeds (`theme/builtin.rs`) — each entry is data only, no logic:

| name | bg | fg | accent | success | warning | error |
|---|---|---|---|---|---|---|
| dark (existing) | #131720 | #d8dee9 | #7aa2f7 | #9ece6a | #e0af68 | #f7768e |
| light (existing) | #f7f8fa | #24292f | #1d63ed | #16a34a | #d97706 | #dc2626 |
| gruvbox-dark | #282828 | #ebdbb2 | #83a598 | #b8bb26 | #fabd2f | #fb4934 |
| gruvbox-light | #fbf1c7 | #3c3836 | #076678 | #79740e | #b57614 | #9d0006 |
| catppuccin-mocha | #1e1e2e | #cdd6f4 | #89b4fa | #a6e3a1 | #f9e2af | #f38ba8 |
| solarized-dark | #002b36 | #839496 | #268bd2 | #859900 | #b58900 | #dc322f |
| solarized-light | #fdf6e3 | #657b83 | #268bd2 | #859900 | #b58900 | #dc322f |

Note: the generator's contrast clamp (text ≥ 0.4 Oklab lightness from bg)
will brighten Solarized's muted fg somewhat; accepted as a readability
feature, not a bug — our Solarized is not pixel-faithful to canonical.

### Custom theme files

`<config_dir>/themes/<name>.toml`:

```toml
name = "My Theme"   # optional display label; defaults to the file stem
bg = "#101418"
fg = "#e2e2e6"
accent = "#0178d4"
success = "#9ece6a"
warning = "#e0af68"
error = "#f7768e"
```

- All six color keys required, `#rrggbb` only. Any missing/invalid key
  fails the whole file (warn + skip) — no partial themes.
- The theme's stable name (config value, dedup key) is the file stem. A
  custom file whose stem collides with a built-in name shadows the
  built-in (lets users tweak "dark" wholesale); collision emits no
  warning.

### Config (`config.rs`)

The `theme` key stays a string but now holds any registry name. Parsing no
longer maps unknown values to Terminal at parse time — the raw string is
kept and resolution (with its unknown-name warning) happens against the
registry at startup. Committing a theme from the picker rewrites the
`theme` key in `config.toml`, preserving the file's other content the same
way existing config writes do.

### Picker UI (`components/chooser.rs`, `components/palette.rs`, `app.rs`)

- `ChooserItem` gains `swatches: Option<Vec<Color>>`. When present, the
  chooser paints a strip of six solid swatches right-aligned on the row:
  two space cells per swatch with the cell background set to the swatch
  color, one blank column between swatches. Label and detail text render
  as today; a long label truncates before it can collide with the strip.
  Rows without swatches render exactly as today (zero impact on the
  project chooser).
- New palette command **"Change theme…"** (id `theme-choose`) opens
  `Modal::Chooser` with one row per registry entry: label, detail =
  source ("built-in", "custom", "terminal colors"), swatches from the
  entry's seeds (Terminal row: from the cached query, falling back to the
  Dark seeds exactly as resolution would).
- **Live apply:** the app snapshots the current theme name when the picker
  opens. Whenever the chooser's highlighted row changes (arrow keys,
  filter narrowing, mouse hover-select), the app resolves that row's theme
  and repaints. Esc restores the snapshot's theme. Enter keeps the
  highlighted theme, writes the config, closes.
- Filtering is the chooser's existing fuzzy filter; live apply follows the
  highlight wherever filtering moves it.
- The existing 256-color downgrade path applies to every applied theme,
  as today.

### OSC deadline (`theme/osc.rs`)

`QUERY_DEADLINE`: 150ms → 600ms. Comment updated to reflect the observed
race (a terminal answering late caused nondeterministic fallback to the
built-in dark seeds between launches).

### Footer emphasis inversion (`components/footer.rs`)

In `paint_chip_row`, clickable chips swap emphasis:

- Action label: `theme.text` (was `theme.text_muted`), stays non-bold.
- Shortcut keycap pill: tinted from `theme.text_muted` (was
  `theme.accent`), so it reads as a quiet keycap. Hover behavior (surface
  lift) unchanged.
- Non-clickable plain entries get the same swap (`key` in muted, label in
  `theme.text`).

## Error handling

- Malformed custom theme file → warning (existing startup-warning channel),
  file skipped; picker still opens with the rest.
- Unknown `theme` name in config → warning, resolve to `terminal`.
- Silent terminal on the Terminal theme → Dark-seed fallback (unchanged).
- Config write failure on Enter → surface the io error the same way other
  config writes do; the applied theme stays for the session.

## Testing

Follows existing patterns (TestBackend paint tests, pure unit tests):

- `theme/builtin.rs`: every built-in generates without panicking; ladder
  monotonicity holds for a dark and a light representative; Solarized text
  meets the contrast clamp.
- Registry: custom file parsing (valid, missing key, bad hex, shadowing a
  built-in), unknown-name resolution warns and falls back, legacy names
  resolve.
- Chooser: a row with swatches paints the strip cells with the given
  colors; a row without swatches is unchanged (regression).
- App: opening the picker + moving the highlight swaps the live theme;
  Esc restores the prior theme; Enter persists the name to config
  (round-trip through the config writer).
- Footer: label paints `theme.text`, keycap pill tint derives from
  `text_muted` not `accent` (regression on the inversion).
- OSC: deadline constant test not needed (covered by existing read-loop
  tests, which pass their own deadlines).

## Out of scope

- In-app color editing UI.
- Brightness/contrast knob (revisit after this ships if macOS muted text
  still reads too dark).
- Watching theme files for changes (rescan happens on picker open only).
- Per-project themes.
