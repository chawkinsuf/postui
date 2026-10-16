# Theme text floors hold for tinted and near-black colours (task #31)

## 1. Problem

`Theme::generate` promises drawn text sits at least 0.4 Oklab lightness
from the page (`readable_text`). Chip pills promise 0.35 between their
text and fill (`ensure_min_contrast`, called from `paint/chip.rs` twice
and `components/footer.rs`). Both push the text to a target lightness
with `lift`, which converts through `oklab_to_rgb` and clamps each sRGB
channel on its own. Two things break the promise:

1. **Gamut clip.** A tinted text whose target lies outside sRGB is
   clipped per channel. That changes its lightness, so the drawn gap
   lands short (0.384–0.395 in #31's report). It also turns the hue,
   and running the clamp again on its output gives a different colour.
2. **Rounding near black.** Next to black one u8 step is a big
   lightness step: `#010101` is L 0.067, and a lone blue `1` is about
   0.03. A target in that range rounds back toward the page by more
   than `TEXT_FLOOR`'s 0.005 slack. This hits greys too.
   `ensure_min_contrast` aims at exactly its floor with no slack, so
   ordinary rounding lands it just under, mid-range too. #31 was filed
   for cause 1 only. Cause 2 is in scope too (you, 2026-10-16).

Evidence: a probe swept 494,460 seed pairs (pages L 0..1 in 0.005 steps,
texts L 0..1 in 0.025, chroma 0/0.02/0.04/0.08/0.12, 12 hues).
`readable_text` on main fails 30,272 of them: the gap is under 0.4, or a
second pass changes the colour. `ensure_min_contrast(…, 0.35)` fails
159,881, the worst at a 0.257 gap. Fitting chroma alone still left 5,193
`readable_text` failures, 384 of them grey (cause 2). Fitting plus the
walk below leaves 0. All eight built-ins already hold the text floor.

## 2. Rules

| # | Rule | Source |
|---|------|--------|
| F1 | Drawn text sits ≥ 0.4 from the page, for every seed pair, tinted or grey, near black or white. | existing promise (`generate`, theme-contrast C13) |
| F2 | The text clamp is idempotent: clamping its own output returns it unchanged. | #31 |
| F3 | A pushed text keeps its hue: its chroma is trimmed to fit sRGB at the target lightness, never clipped per channel. | #31; same rule as theme-contrast round 4 |
| F4 | Chip text sits ≥ its `min_delta` (0.35) from its fill, under the same mechanism. | you (scope, 2026-10-16) |
| F5 | The aim stays at 0.405 for text and `min_delta` for chips. Only when the rounded colour lands under the floor does the target walk further out, in 0.005 steps. A seed that clears today renders the same. | you (2026-10-16) |
| F6 | If the walk reaches L 0 or 1, the text is pure black or white, the only colour at that lightness. The tint is dropped there. | you (2026-10-16) |

F2 is promised for text only, because chip output is never clamped
again. F3 is checked only on results with real chroma, since u8 rounding
swings the hue angle of a near-grey.

## 3. Design

One function replaces the two copies of the push-apart logic:

```rust
/// `fg` moved away from `page` in Oklab lightness until the drawn pair
/// is at least `floor` apart, aiming at `aim` (>= floor) ...
fn push_apart(fg: Rgb, page: Rgb, floor: f32, aim: f32) -> Rgb
```

- It returns `fg` unchanged when it already clears `floor`.
- Otherwise it picks a direction: the text's own side of the page, or
  the other side when that one has no room for `aim`. This is today's
  flip, kept as is.
- It converts at `page_l ± aim` with `oklab_to_rgb_in_gamut` (fit the
  chroma, keep L and hue). While the rounded colour is under `floor`,
  the target steps 0.005 further. If the target reaches 0 or 1, the
  result is black or white.
- `readable_text(fg, page)` becomes `push_apart(fg, page, 0.4, TEXT_FLOOR)`.
- `ensure_min_contrast(fg, bg, d)` keeps its `Color` signature and
  non-RGB passthrough, and calls `push_apart(…, d, d)`.

No contract changes: both callers keep their signatures. `lift` stays,
since the surface ladder still uses it for small steps that promise no
floor. Out of scope: `ensure_wcag_contrast` also walks with `lift`, but
it measures the real ratio at every step, so its floor holds. Only its
hue can turn. `oklab_to_rgb_in_gamut` stays as is: its float slack can
answer `#000001` at L 0, and the black/white end case avoids that rather
than changing what `with_contrast` draws. Both left alone with your
approval (2026-10-16).

Behaviour that changes:
- Affected custom themes and Terminal palettes draw their text at the
  floor with its hue kept, instead of short of it with the hue turned.
- Some chip texts move a step further from their fill.
- Of 262,925 swept seeds that clear today, 52 move one u8 step in a
  channel already at 255. Those sit on sRGB's edge, where the
  per-channel clip and the gamut fit disagree by a float.

## 4. Tests

- **F1, F2, F3:** a sweep over tinted and grey seeds near both ends checks
  `readable_text`: the gap is ≥ 0.4, a second clamp returns the same
  colour, and a result with real chroma keeps the seed's hue. It
  includes #31's example: page L 0.45, chroma 0.04, hue 1; text L 0.1.
- **F4:** the same sweep checks `ensure_min_contrast(…, 0.35)`: gap ≥ 0.35.
- **F5:** every built-in's drawn text equals its seed or today's clamp
  (the existing built-in tests keep passing unchanged).
- **F6:** a tinted text on a page at L ≈ 0.41 that must go dark comes out `#000000`.
- `contrast_invariants_hold_across_a_sweep_of_themes` drops its #31
  skip, so all 2,016 of its seeds run. If one of the 98 seeds that sat
  out breaks a contrast-level invariant, that is a `with_contrast` issue
  the skip hid: stop and bring it to you, don't patch around it.
- The new sweep is smaller than the 494k probe, to keep the suite fast,
  but steps finely through near-black pages.

## 5. Choices of mine

- The 0.005 walk step. It is finer than one u8 step anywhere but next to
  black, and it costs a few conversions only on the rare short landing.
- One `push_apart(floor, aim)` with two numbers. A single number would
  shift every pushed text (F5).
- Leaving `ensure_wcag_contrast` alone (§3).
