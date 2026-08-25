use crate::anim::{AnimKey, Anims};
use crate::paint::{fill, text};
use crate::theme::{self, Theme};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Span;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
    Warning,
}

const TOAST_LIFETIME_TICKS: u32 = 30; // 3 s at the 100 ms tick

/// Total on-screen height (in rows) of one painted toast: a filled `panel`
/// rect with its message vertically centered on the middle row.
const TOAST_HEIGHT: u16 = 3;

/// How many of a toast's final lifetime ticks are spent easing out via
/// `AnimKey::ToastFade` — 5 ticks is 500ms at the nominal 100ms tick period
/// [`TOAST_LIFETIME_TICKS`]'s own doc comment assumes, matching
/// [`TOAST_FADE_DUR`] and the testbed's "500ms (current demo)" comparison
/// row. The tick period is actually adaptive (faster while any animation,
/// including this fade, is in flight — see `main.rs`), so the fade's own
/// color easing (wall-clock, via `Anims`) may finish before the toast's
/// tick-counted removal in that case; a fade cut a little short by a busy
/// frame reads fine, and the removal timing itself is untouched from
/// before this task.
const TOAST_FADE_TICKS: u32 = 5;

/// Wall-clock duration of the fade-out eased via `AnimKey::ToastFade`, once
/// a toast enters its final [`TOAST_FADE_TICKS`] ticks.
const TOAST_FADE_DUR: Duration = Duration::from_millis(500);

/// First id handed to a real toast's `AnimKey::ToastFade(id)`. The
/// testbed's duration-comparison rows borrow ids 70..=500 for the same
/// `AnimKey` variant (see `components::testbed`); starting real toast ids
/// at 1000 keeps the two from ever colliding.
const FIRST_TOAST_ID: u64 = 1000;

struct Toast {
    message: String,
    kind: ToastKind,
    remaining_ticks: u32,
    /// This toast's stable id, keying its own `AnimKey::ToastFade(id)` —
    /// see [`FIRST_TOAST_ID`].
    id: u64,
}

pub struct Toasts {
    entries: Vec<Toast>,
    next_id: u64,
}

impl Default for Toasts {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next_id: FIRST_TOAST_ID,
        }
    }
}

impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(Toast {
            message: message.into(),
            kind,
            remaining_ticks: TOAST_LIFETIME_TICKS,
            id,
        });
    }

    /// Starts the slide-in ease (`AnimKey::ToastFade(id)`, 0 → 1 over
    /// `slide_dur`) for every toast whose key isn't tracked by `anims` yet
    /// — i.e. every toast pushed since the last call. Called once per
    /// `App::update` (after `apply(action)`, so it picks up anything that
    /// action just pushed) rather than from `push` itself, so the ~100
    /// `Toasts::push` call sites across `app.rs` don't each need `&mut
    /// Anims`/`now`/the config duration threaded through. Idempotent: a
    /// toast already tracked (mid-slide, settled, or fading) is untouched.
    pub fn start_pending_anims(&self, anims: &mut Anims, now: Instant, slide_dur: Duration) {
        for t in &self.entries {
            if anims.value(AnimKey::ToastFade(t.id), now).is_none() {
                anims.snap(AnimKey::ToastFade(t.id), 0.0);
                anims.retarget(AnimKey::ToastFade(t.id), 1.0, slide_dur, now);
            }
        }
    }

    /// Advances every toast's countdown by one tick and drops any that
    /// expired. Returns `true` while any toast is still visible/animating
    /// (i.e. a redraw is needed), `false` when idle. The instant a toast's
    /// countdown crosses into its final `TOAST_FADE_TICKS` ticks, retargets
    /// its `AnimKey::ToastFade(id)` from wherever it sits (1.0, if it's
    /// already settled from its slide-in) down to 0.0 over
    /// `TOAST_FADE_DUR` — exactly once, since the crossing only happens on
    /// one tick per toast.
    pub fn on_tick(&mut self, anims: &mut Anims, now: Instant) -> bool {
        // Captured before pruning: on the tick where the last toast expires,
        // `entries` goes from non-empty to empty, and that transition is
        // itself a state change that needs one final redraw to erase the
        // toast. Returning based on the post-prune state would swallow that
        // frame and leave the toast painted until the next keypress.
        let was_visible = !self.entries.is_empty();
        for t in &mut self.entries {
            let was_above_fade_window = t.remaining_ticks > TOAST_FADE_TICKS;
            t.remaining_ticks = t.remaining_ticks.saturating_sub(1);
            if was_above_fade_window && t.remaining_ticks <= TOAST_FADE_TICKS {
                anims.retarget(AnimKey::ToastFade(t.id), 0.0, TOAST_FADE_DUR, now);
            }
        }
        // Drop the expiring toasts' own `AnimKey::ToastFade(id)` entries
        // along with the toasts themselves -- otherwise every toast ever
        // pushed leaves a permanently-done (but never-removed) `Anim` in
        // `Anims`'s map for the life of the process.
        for t in self.entries.iter().filter(|t| t.remaining_ticks == 0) {
            anims.clear(AnimKey::ToastFade(t.id));
        }
        self.entries.retain(|t| t.remaining_ticks > 0);
        was_visible
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every currently-visible toast's message text, oldest first — for
    /// tests that need to assert on wording, not just presence.
    #[cfg(test)]
    pub fn messages(&self) -> Vec<&str> {
        self.entries.iter().map(|t| t.message.as_str()).collect()
    }

    /// Paints every visible toast as a floating filled `theme.panel` rect
    /// with a 1-col `█` left bar in its semantic color (accent/success/
    /// error/warning by kind) and `theme.text` message text, stacked
    /// top-right the same as before — but with no `Block`/border chrome of
    /// its own.
    ///
    /// Each toast's own `AnimKey::ToastFade(id)` value `t` (sampled at
    /// `now`, defaulting to `1.0` — fully settled — for a toast this
    /// `Anims` handle has no entry for yet) drives two effects at
    /// different points in the toast's life, since they never overlap:
    /// while sliding in (the first `ui_settings.anim_ms.toast` after
    /// `push`, `t` easing 0 → 1), it offsets the toast rightward off its
    /// resting position by `(1 - t)` of its own width, so it slides in
    /// from the edge; while fading out (the final `TOAST_FADE_TICKS`
    /// ticks, `t` easing back 1 → 0), the offset is pinned to 0 (the fade
    /// doesn't also creep back off-screen) and every painted color instead
    /// blends toward `theme.page` by `theme::mix(_, theme.page, 1 - t)` —
    /// replacing the old hard cutoff with a continuous fade. Reusing `t`
    /// for the color blend during the slide too (not just the offset) is
    /// a deliberate bonus: the toast fades in color-wise as it slides,
    /// rather than snapping to full color mid-slide.
    pub fn draw(
        &self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        anims: &Anims,
        now: Instant,
    ) {
        let mut y = screen.y + 1;
        for toast in &self.entries {
            // Use display-width instead of char count to handle double-width chars (emoji, CJK)
            let display_width = Span::raw(toast.message.as_str()).width();
            // Clamp to usize before arithmetic to prevent overflow on very long messages
            let padding_width: usize = 6; // 1 bar col + 1 gap col + 4 right margin
            let clamped_width = display_width.saturating_add(padding_width);
            let width = (clamped_width as u16).min(screen.width);
            let rest_x = screen.right().saturating_sub(width.saturating_add(1));

            let t = anims.value_or(AnimKey::ToastFade(toast.id), now, 1.0);
            let fading = toast.remaining_ticks <= TOAST_FADE_TICKS;
            let x_offset = if fading {
                0
            } else {
                ((1.0 - t).clamp(0.0, 1.0) * width as f32).round() as u16
            };
            let fade_toward_page = (1.0 - t).clamp(0.0, 1.0);

            let area = Rect::new(rest_x.saturating_add(x_offset), y, width, TOAST_HEIGHT);
            if area.bottom() > screen.bottom() {
                break;
            }
            let color = match toast.kind {
                ToastKind::Info => theme.accent,
                ToastKind::Success => theme.success,
                ToastKind::Error => theme.error,
                ToastKind::Warning => theme.warning,
            };
            let panel_c = theme::mix(theme.panel, theme.page, fade_toward_page);
            let bar_c = theme::mix(color, theme.page, fade_toward_page);
            let text_c = theme::mix(theme.text, theme.page, fade_toward_page);

            let buf = frame.buffer_mut();
            fill(buf, area, panel_c);
            for row in area.top()..area.bottom() {
                if let Some(cell) = buf.cell_mut((area.x, row)) {
                    cell.set_symbol("\u{2588}");
                    cell.set_fg(bar_c);
                    cell.set_bg(panel_c);
                }
            }
            let text_y = area.y + area.height / 2;
            text(
                buf,
                area.x + 2,
                text_y,
                &toast.message,
                text_c,
                panel_c,
                false,
            );
            y += TOAST_HEIGHT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn toast_expires_after_lifetime_ticks() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push("Saved", ToastKind::Success);
        assert!(!t.is_empty());
        for _ in 0..TOAST_LIFETIME_TICKS - 1 {
            t.on_tick(&mut anims, Instant::now());
        }
        assert!(!t.is_empty(), "alive one tick before expiry");
        t.on_tick(&mut anims, Instant::now());
        assert!(t.is_empty(), "expired at lifetime");
    }

    /// A toast's `AnimKey::ToastFade(id)` entry must not outlive the toast
    /// itself -- `on_tick` must drop it (not just leave it `done`) the same
    /// tick the toast expires, so `Anims`'s map doesn't grow unbounded over
    /// a long-running app's lifetime.
    #[test]
    fn expiring_a_toast_clears_its_anim_entry() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push("Saved", ToastKind::Success);
        let now = Instant::now();
        assert!(
            anims
                .value(AnimKey::ToastFade(FIRST_TOAST_ID), now)
                .is_none(),
            "start_pending_anims hasn't run yet in this bare-Toasts test"
        );
        anims.snap(AnimKey::ToastFade(FIRST_TOAST_ID), 0.0);
        anims.retarget(
            AnimKey::ToastFade(FIRST_TOAST_ID),
            1.0,
            Duration::from_millis(100),
            now,
        );
        assert!(
            anims
                .value(AnimKey::ToastFade(FIRST_TOAST_ID), now)
                .is_some(),
            "the anim entry exists before expiry"
        );

        for _ in 0..TOAST_LIFETIME_TICKS {
            t.on_tick(&mut anims, now);
        }
        assert!(t.is_empty(), "the toast has expired");
        assert!(
            anims
                .value(AnimKey::ToastFade(FIRST_TOAST_ID), now)
                .is_none(),
            "the expired toast's anim entry must be cleared, not just left done"
        );
    }

    #[test]
    fn on_tick_returns_true_on_the_expiring_tick_and_false_after() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push("bye", ToastKind::Info);
        for _ in 0..TOAST_LIFETIME_TICKS - 1 {
            assert!(
                t.on_tick(&mut anims, Instant::now()),
                "still visible while counting down"
            );
        }
        assert!(
            t.on_tick(&mut anims, Instant::now()),
            "the expiring tick must still request a redraw to erase it"
        );
        assert!(t.is_empty(), "the toast is gone after the expiring tick");
        assert!(
            !t.on_tick(&mut anims, Instant::now()),
            "the tick after expiry is idle: no redraw needed"
        );
    }

    #[test]
    fn multiple_toasts_expire_independently() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push("first", ToastKind::Info);
        for _ in 0..10 {
            t.on_tick(&mut anims, Instant::now());
        }
        t.push("second", ToastKind::Error);
        for _ in 0..TOAST_LIFETIME_TICKS - 10 {
            t.on_tick(&mut anims, Instant::now());
        }
        assert!(!t.is_empty(), "second toast still alive");
        for _ in 0..10 {
            t.on_tick(&mut anims, Instant::now());
        }
        assert!(t.is_empty());
    }

    #[test]
    fn draw_renders_message_top_right() {
        let mut t = Toasts::default();
        t.push("Copied \u{2713}", ToastKind::Success);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let anims = Anims::new(true);
        terminal
            .draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now()))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Copied"));
    }

    #[test]
    fn draw_handles_double_width_chars_correctly() {
        let mut t = Toasts::default();
        // "已复制 ✓" contains double-width CJK chars and a double-width emoji
        t.push("已复制 ✓", ToastKind::Success);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let anims = Anims::new(true);
        terminal
            .draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now()))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        // Verify that the toast was rendered with the double-width message
        assert!(content.contains("已"), "CJK character should be rendered");
    }

    /// Panel fill + a `█` left bar in the semantic color, no `Block`/border
    /// chrome (no rounded-corner glyphs).
    #[test]
    fn draw_paints_panel_fill_and_a_colored_left_bar() {
        let mut t = Toasts::default();
        t.push("oops", ToastKind::Error);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let anims = Anims::new(true);
        terminal
            .draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now()))
            .unwrap();
        let buf = terminal.backend().buffer();

        // The bar column is the toast area's left edge; find it by scanning
        // for the "o" of "oops" and walking back to the bar just left of the
        // 2-col text gutter.
        let mut found_bar = false;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = buf[(x, y)].clone();
                if cell.symbol() == "\u{2588}" {
                    assert_eq!(cell.fg, theme.error, "left bar is in the error color");
                    assert_eq!(cell.bg, theme.panel);
                    found_bar = true;
                }
            }
        }
        assert!(found_bar, "expected a `█` left bar cell somewhere");

        let content = format!("{buf:?}");
        for glyph in ['\u{256d}', '\u{256e}', '\u{2570}', '\u{256f}'] {
            assert!(
                !content.contains(glyph),
                "no rounded border glyph {glyph:?} expected: {content}"
            );
        }
        assert!(content.contains("oops"));
    }
}
