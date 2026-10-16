# Theme Contrast — Design

Date: 2026-10-16
Task: #17 (Add contrast adjustment for themes)
Status: approved 2026-10-16 (`you`, after a section-by-section review; ±3
and negative steps confirmed by you)

## 1. Problem

The themes spec (2026-08-26) deferred a "brightness/contrast knob" until a
theme still read too soft after its other fixes. The user now asks for it:
"its text vs background but also the overall look of the theme feeling
washed out" (`you`).

A theme is six seeds (`bg`, `fg`, `accent`, `success`, `warning`, `error`)
that `Theme::generate` (`theme/mod.rs`) expands into every token. The
washed-out look has three sources, all in the seeds:

1. text close to the page (light-grey on dark-grey);
2. a page that is not deep enough (soft charcoal rather than near-black, or
   off-white rather than white);
3. low-saturation colours (the built-in Dark's pastel accents).

Today the only lever is writing a custom `themes/*.toml`.

## 2. Rules

| # | Rule | Source |
|---|---|---|
| C1 | One contrast step moves the theme's text away from its background, deepens the background, and saturates the four colours (accent, success, warning, error). A negative step does the reverse. | `you` (text vs background + washed-out look) |
| C2 | The background moves for built-in and custom themes. The Terminal colors theme keeps the terminal's own background (no seam at the window padding); its text and colours still move. | `you` |
| C3 | One global level, applied to whichever theme is active; switching themes keeps it. | `you` |
| C4 | The theme picker has a Contrast row under the list: a `−` button, a level meter, a `+` button, and the level as text. | `you` (preview picked) |
| C5 | Keys: `-` lowers, `+` raises, in both profiles. Alt+arrows are not used. | `you` ("plus and minus are better than alt arrows") |
| C6 | `=` also raises (the `+` key unshifted), and the keypad's `-`/`+` work (they arrive as the same characters). | `mine` |
| C7 | The level runs from −3 to +3, default 0. Level 0 is today's theme exactly, bit for bit. | `mine` |
| C8 | The level previews live, like the highlighted theme. Enter (or a click outside, today's accept) keeps theme and level; Esc reverts both to what they were when the picker opened. | `mine`, following the picker's existing preview rule |
| C9 | At ±3 the matching key does nothing and its button paints disabled. | `mine` |
| C10 | Stored as `theme_contrast = <n>` in `config.toml`, written in the same edit as `theme`. Level 0 removes the key, so picking a theme does not add a line to every config. | `mine` |
| C11 | A `theme_contrast` that is not an integer from −3 to 3 warns ("invalid value … expected an integer from -3 to 3; using default") and uses 0, as `indent_width` does. | `mine`, following precedent |
| C12 | Reset settings removes `theme_contrast` along with `theme`. | `mine` |
| C13 | A negative level can never make text unreadable: `generate`'s text clamp (Oklab ΔL ≥ 0.4) and the muted-text WCAG floor still run on the adjusted seeds. | follows from the design |
| C14 | A step never flips a theme's polarity: a dark background stays below Oklab L 0.5, a light one above. The picker's dark/light sets are judged from the unadjusted seeds. | `mine` |

## 3. Design

### 3.1 The transform (`theme/mod.rs`)

A new newtype holds the level and its range in one place:

```rust
pub struct Contrast(i8);           // -3..=3, 0 = the theme as authored
impl Contrast {
    pub const MIN: i8 = -3; pub const MAX: i8 = 3;
    pub fn new(level: i8) -> Option<Self>;   // None out of range
    pub fn level(self) -> i8;
    pub fn step(self, delta: i8) -> Option<Self>;  // None past an end (C9)
}
```

`Seeds::with_contrast(self, c: Contrast, keep_bg: bool) -> Seeds` applies
one step `n` (`n = c.level()`) in Oklab:

- `bg` (unless `keep_bg`): lightness moves `0.02·n` toward its own end
  (down for a dark background, up for a light one), clamped to its side of
  0.5 (C14).
- `fg`: lightness moves `0.03·n` away from the (adjusted) `bg`.
- `accent`, `success`, `warning`, `error`: chroma scaled by `1 + 0.10·n`,
  lightness and hue kept.
- `n = 0` returns `self` untouched (C7).

The constants are a starting point and are tuned in the real TUI.
`Theme::generate` does not change, so its clamps run on the result (C13).

### 3.2 The registry (`theme/registry.rs`)

The contract changes:

- before: `resolve(&self, name, queried) -> Option<Theme>`
- after: `resolve(&self, name, queried, contrast: Contrast) -> Option<Theme>`

`resolve` applies `with_contrast` to `seeds_of(entry)`, with
`keep_bg = matches!(entry.source, ThemeSource::Terminal)` (C2).
`seeds_of` and `entry_is_dark` stay on the unadjusted seeds (C14).

### 3.3 Config (`config.rs`)

- `UiSettings` gains `theme_contrast: Contrast`, parsed per C11 and listed in
  `config_seed()`'s comments.
- `save_ui_theme(name)` becomes `save_ui_theme(name, contrast)`: one edit
  that sets `theme` and sets or removes `theme_contrast` (C10).
- `reset_ui_settings` removes `theme_contrast` (C12).

### 3.4 App (`app.rs`, `app/mouse.rs`)

- `App` gains `theme_contrast: Contrast` next to `theme_name`; startup and the
  Reload arm take it from `UiSettings`. `set_theme_by_name` passes it to
  `resolve`.
- `theme_preview: Option<String>` becomes `Option<(String, Contrast)>`, the
  snapshot both revert sites (`Action::Close` and `apply_modal_result`)
  restore (C8).
- New `Action::StepThemeContrast(i8)`: steps the level (no-op at an end),
  re-resolves the current theme and updates the picker's Contrast row.
- `Action::ApplyTheme(name)` saves both values in one edit.

### 3.5 Picker (`components/chooser.rs`, `components/modal.rs`, `hint.rs`)

The chooser stays generic. It gains an optional stepper row, the same way it
has an optional toggle:

```rust
pub struct ChooserStepper {
    pub label: String,     // "Contrast"
    pub level: i8, pub min: i8, pub max: i8,
    pub action: fn(i8) -> Action,   // delta → Action::StepThemeContrast
}
```

- `with_stepper(..)` builds it, and `set_stepper_level(n)` updates it.
- `handle_key`: on a chooser with a stepper and no filter box, `-` fires
  `action(-1)`, and `+`/`=` fire `action(+1)`, without closing, like the
  toggle (C5, C6).
- `draw`: when a stepper is attached, the panel grows one row. The row is
  `Contrast   [−]  ▁▂▃▄▅▆▇  [+]   +1`. The meter has one cell per level,
  lit in accent up to the current level and quiet beyond it. The `−`/`+`
  buttons register `Hit::ChooserStepDown` / `Hit::ChooserStepUp`, and a
  button at its end paints disabled (C9).
- Footer: the chooser's chips add `−+  contrast` while a stepper is
  attached.
- Hover hints: "Less contrast" / "More contrast".

Nothing is removed. No other chooser attaches a stepper, so they are
unchanged.

## 4. Tests

| Rule | Test |
|---|---|
| C7 | `with_contrast(0)` returns identical seeds for every built-in, so `resolve` at 0 equals today's `resolve`. |
| C1 | For every built-in at +1..+3: `bg` L moves monotonically toward its end, `fg`–`bg` ΔL grows, the four colours' chroma grows. Mirror test for −1..−3. |
| C2 | Terminal entry at ±3: `page` equals the level-0 `page`; `text` differs. |
| C13 | Every built-in at −3: text ΔL ≥ 0.4 and muted meets its floor. |
| C14 | Every built-in at ±3 keeps `is_dark()`; `entry_is_dark` ignores the level. |
| C11 | Parse: `theme_contrast = 2` → 2; `9`, `"high"`, `1.5` → warning + 0. |
| C10 | `save_ui_theme("dark", +2)` writes both keys; `save_ui_theme("dark", 0)` removes `theme_contrast`, keeping unrelated content. |
| C12 | `reset_ui_settings` removes `theme_contrast`. |
| C5, C6 | App: in the open picker, `+`, `=` and `-` step the level and change `app.theme.page` live (arrows and vim profiles, via the key matrix). |
| C9 | `+` at +3 and `-` at −3 change nothing. Paint: the end button is disabled. |
| C8 | Step then Esc → level and theme restored. Step then Enter → config holds both. Step then click outside → kept. |
| C4 | Paint: the stepper row's buttons, meter and level text paint and register hits. A click on `[+]` steps. A chooser without a stepper paints as today (regression). |
| C4 | Footer advertises `−+ contrast` only on the theme picker. |

## 5. `mine` choices for you to veto

1. `=` also raises, and the keypad works (C6).
2. Range −3..+3, default 0 (C7).
3. Live preview, with Esc reverting the level too (C8).
4. The key does nothing and the button is disabled at an end (C9).
5. Level 0 removes the config key (C10).
6. A bad value warns and uses 0 (C11).
7. Reset settings clears the level (C12).
8. A step never flips polarity (C14).
9. The step sizes in §3.1 (0.02 / 0.03 / 10% chroma), tuned in the TUI.
10. The contrast control lives only in the picker, not in the Settings tab.
11. The hit and hint names (`ChooserStepDown/Up`, "Less/More contrast").

## 6. Risks and costs

- No background work, no OS prompts, no polling. The transform runs once per
  resolve, the same as `generate` today.
- 256-colour terminals: the downgrade quantizes, so a single step may land
  on the same palette index and look like nothing changed.
- At the extremes a step has no room: near-white text on a near-black page
  cannot get brighter, so +2 and +3 may look the same on a crisp theme.
- High chroma can leave the sRGB gamut. `oklab_to_rgb` clamps per channel,
  which can shift the hue slightly at +3. This is checked in the TUI.

## 7. Plan

One sequence, TDD at each step:

1. `Contrast` + `Seeds::with_contrast` + transform tests (C1, C2, C7, C13, C14).
2. `resolve` takes `Contrast`; update callers.
3. Config: parse, save, reset, seed (C10–C12).
4. App state, snapshot, `StepThemeContrast`, `ApplyTheme` (C3, C8, C9).
5. Chooser stepper: keys, draw, hits, footer, hints (C4–C6, C9).
6. Tune the constants in tmux; full suite, clippy, fmt.
