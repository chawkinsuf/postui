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

/// Total wall-clock lifetime of a toast, from `push` to removal.
const TOAST_LIFETIME: Duration = Duration::from_secs(3);

/// Total on-screen height (in rows) of one painted toast: a filled `panel`
/// rect with its message vertically centered on the middle row.
const TOAST_HEIGHT: u16 = 3;

/// How much of a toast's final lifetime is spent easing out via
/// `AnimKey::ToastFade`, matching [`TOAST_FADE_DUR`] and the testbed's
/// "500ms (current demo)" comparison row. Wall-clock, like
/// [`TOAST_LIFETIME`] — unaffected by the tick period being adaptive
/// (faster while any animation, including this fade, is in flight — see
/// `main.rs`).
const TOAST_FADE_DUR: Duration = Duration::from_millis(500);

/// First id handed to a real toast's `AnimKey::ToastFade(id)`. The
/// testbed's duration-comparison rows borrow ids 70..=500 for the same
/// `AnimKey` variant (see `components::testbed`); starting real toast ids
/// at 1000 keeps the two from ever colliding.
const FIRST_TOAST_ID: u64 = 1000;

struct Toast {
    message: String,
    kind: ToastKind,
    /// When this toast should be removed. `None` until stamped by whichever
    /// of `start_pending_anims`/`on_tick` observes it first — both receive
    /// an injected `now`, so `push`'s ~100 call sites across `app.rs` don't
    /// need `Instant::now()` threaded through. Mirrors how
    /// `start_pending_anims` already lazily starts each toast's own
    /// `AnimKey::ToastFade(id)` slide-in.
    expires_at: Option<Instant>,
    /// Whether the fade-out retarget (`expires_at - TOAST_FADE_DUR` →
    /// `expires_at`) has already fired for this toast. Wall-clock ticks
    /// arrive at irregular, adaptive intervals (see `main.rs`), so this
    /// flag — rather than comparing two consecutive samples — is what
    /// guarantees the retarget happens exactly once.
    fade_started: bool,
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
            expires_at: None,
            fade_started: false,
            id,
        });
    }

    /// Stamps `expires_at` on every toast that doesn't have one yet --
    /// i.e. every toast pushed since the last call to this or `on_tick`.
    /// Not done in `push` itself, so the ~100 `Toasts::push` call sites
    /// across `app.rs` don't each need `now` threaded through.
    ///
    /// The lifetime scales with reading time: [`TOAST_LIFETIME`] covers
    /// the first 40 chars, then ~35ms per additional char, capped at 8s —
    /// a long validation error stays readable instead of vanishing on the
    /// same clock as a one-word "Saved".
    fn stamp_pending(&mut self, now: Instant) {
        for t in &mut self.entries {
            if t.expires_at.is_none() {
                let extra_chars = t.message.chars().count().saturating_sub(40) as u64;
                let lifetime = (TOAST_LIFETIME + Duration::from_millis(extra_chars * 35))
                    .min(Duration::from_secs(8));
                t.expires_at = Some(now + lifetime);
            }
        }
    }

    /// Starts the slide-in ease (`AnimKey::ToastFade(id)`, 0 → 1 over
    /// `slide_dur`) for every toast whose key isn't tracked by `anims` yet
    /// — i.e. every toast pushed since the last call. Called once per
    /// `App::update` (after `apply(action)`, so it picks up anything that
    /// action just pushed) rather than from `push` itself, so the ~100
    /// `Toasts::push` call sites across `app.rs` don't each need `&mut
    /// Anims`/`now`/the config duration threaded through. Idempotent: a
    /// toast already tracked (mid-slide, settled, or fading) is untouched.
    /// Also stamps `expires_at` for the same newly-pushed toasts (see
    /// `stamp_pending`) — the two are lazily initialized together since
    /// they're both keyed off "have I seen this toast before".
    pub fn start_pending_anims(&mut self, anims: &mut Anims, now: Instant, slide_dur: Duration) {
        self.stamp_pending(now);
        for t in &self.entries {
            if anims.value(AnimKey::ToastFade(t.id), now).is_none() {
                anims.snap(AnimKey::ToastFade(t.id), 0.0);
                anims.retarget(AnimKey::ToastFade(t.id), 1.0, slide_dur, now);
            }
        }
    }

    /// Drops any toast whose wall-clock `expires_at` has passed. Returns
    /// `true` while any toast is still visible/animating (i.e. a redraw is
    /// needed), `false` when idle. The instant a toast's remaining life
    /// drops to `TOAST_FADE_DUR` or less, retargets its
    /// `AnimKey::ToastFade(id)` from wherever it sits (1.0, if it's already
    /// settled from its slide-in) down to 0.0 over `TOAST_FADE_DUR` —
    /// exactly once (guarded by `fade_started`), since ticks arrive at
    /// irregular, adaptive intervals (see `main.rs`) rather than a fixed
    /// period a single crossing check could rely on.
    pub fn on_tick(&mut self, anims: &mut Anims, now: Instant) -> bool {
        // Captured before pruning: on the tick where the last toast expires,
        // `entries` goes from non-empty to empty, and that transition is
        // itself a state change that needs one final redraw to erase the
        // toast. Returning based on the post-prune state would swallow that
        // frame and leave the toast painted until the next keypress.
        let was_visible = !self.entries.is_empty();
        self.stamp_pending(now);
        for t in &mut self.entries {
            let expires_at = t.expires_at.expect("stamped above");
            let fade_start = expires_at - TOAST_FADE_DUR;
            if !t.fade_started && now >= fade_start {
                t.fade_started = true;
                anims.retarget(AnimKey::ToastFade(t.id), 0.0, TOAST_FADE_DUR, now);
            }
        }
        // Drop the expiring toasts' own `AnimKey::ToastFade(id)` entries
        // along with the toasts themselves -- otherwise every toast ever
        // pushed leaves a permanently-done (but never-removed) `Anim` in
        // `Anims`'s map for the life of the process.
        for t in self
            .entries
            .iter()
            .filter(|t| now >= t.expires_at.expect("stamped above"))
        {
            anims.clear(AnimKey::ToastFade(t.id));
        }
        self.entries
            .retain(|t| now < t.expires_at.expect("stamped above"));
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

    /// The most recently pushed still-live toast's text, for tests that
    /// only care about the last thing reported.
    #[cfg(test)]
    pub fn last_message(&self) -> Option<&str> {
        self.entries.last().map(|t| t.message.as_str())
    }

    /// Every live toast as `(message, kind)` — the kind-aware companion to
    /// [`Self::messages`], for callers that care *how* something was
    /// reported and not just what it said.
    pub fn entries(&self) -> Vec<(&str, &ToastKind)> {
        self.entries
            .iter()
            .map(|t| (t.message.as_str(), &t.kind))
            .collect()
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
    /// from the edge; while fading out (the final `TOAST_FADE_DUR` of its
    /// life, `t` easing back 1 → 0), the offset is pinned to 0 (the fade
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
        // Bottom-right of `screen` (the caller keeps the footer out of that
        // rect), one breathing row off its bottom edge; oldest on top,
        // newest at the bottom. When more toasts are alive than fit, the
        // oldest are the ones held off screen — they expire first anyway.
        let bottom = screen.bottom().saturating_sub(1);
        let visible = (bottom.saturating_sub(screen.y) / TOAST_HEIGHT) as usize;
        let skip = self.entries.len().saturating_sub(visible);
        let shown = self.entries.len() - skip;
        let mut y = bottom.saturating_sub(shown as u16 * TOAST_HEIGHT);
        for toast in self.entries.iter().skip(skip) {
            // Use display-width instead of char count to handle double-width chars (emoji, CJK)
            let display_width = Span::raw(toast.message.as_str()).width();
            // Clamp to usize before arithmetic to prevent overflow on very long messages
            let padding_width: usize = 6; // 1 bar col + 1 gap col + 4 right margin
            let clamped_width = display_width.saturating_add(padding_width);
            let width = (clamped_width as u16).min(screen.width);
            let rest_x = screen.right().saturating_sub(width.saturating_add(1));

            let t = anims.value_or(AnimKey::ToastFade(toast.id), now, 1.0);
            // `expires_at` may still be unstamped here only in a bare
            // `Toasts` test that draws before ever calling `on_tick`/
            // `start_pending_anims` -- treat that as "freshly pushed, not
            // fading yet", matching the real app (where `App::update`
            // always stamps it before the first draw).
            let fading = toast.expires_at.is_some_and(|e| now + TOAST_FADE_DUR >= e);
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
    fn toast_expires_after_lifetime() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        let base = Instant::now();
        t.push("Saved", ToastKind::Success);
        assert!(!t.is_empty());
        // First on_tick stamps expires_at = base + TOAST_LIFETIME.
        t.on_tick(&mut anims, base);
        t.on_tick(&mut anims, base + TOAST_LIFETIME - Duration::from_millis(1));
        assert!(!t.is_empty(), "alive one millisecond before expiry");
        t.on_tick(&mut anims, base + TOAST_LIFETIME);
        assert!(t.is_empty(), "expired at lifetime");
    }

    #[test]
    fn a_long_message_lives_long_enough_to_read() {
        // Lifetime scales with message length past the first ~40 chars —
        // a one-word "Saved" gets the base 3s, a full-sentence error
        // sticks around long enough to actually read.
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        let base = Instant::now();
        let msg = "\"user_id\" already belongs to selector \"creds\" — pick a different field name";
        t.push(msg, ToastKind::Error);
        t.on_tick(&mut anims, base);
        t.on_tick(&mut anims, base + TOAST_LIFETIME);
        assert!(
            !t.is_empty(),
            "a long message must outlive the base lifetime"
        );
        t.on_tick(&mut anims, base + Duration::from_secs(9));
        assert!(t.is_empty(), "but not forever");
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

        t.on_tick(&mut anims, now); // stamps expires_at = now + TOAST_LIFETIME
        t.on_tick(&mut anims, now + TOAST_LIFETIME);
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
        let base = Instant::now();
        t.push("bye", ToastKind::Info);
        assert!(
            t.on_tick(&mut anims, base),
            "still visible right after stamping"
        );
        assert!(
            t.on_tick(&mut anims, base + TOAST_LIFETIME - Duration::from_millis(1)),
            "still visible one millisecond before expiry"
        );
        assert!(
            t.on_tick(&mut anims, base + TOAST_LIFETIME),
            "the expiring tick must still request a redraw to erase it"
        );
        assert!(t.is_empty(), "the toast is gone after the expiring tick");
        assert!(
            !t.on_tick(&mut anims, base + TOAST_LIFETIME + Duration::from_millis(1)),
            "the tick after expiry is idle: no redraw needed"
        );
    }

    #[test]
    fn multiple_toasts_expire_independently() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        let base = Instant::now();
        t.push("first", ToastKind::Info);
        t.on_tick(&mut anims, base); // stamps "first"'s expires_at = base + TOAST_LIFETIME

        let midpoint = base + Duration::from_secs(1);
        t.push("second", ToastKind::Error);
        t.on_tick(&mut anims, midpoint); // stamps "second"'s expires_at = midpoint + TOAST_LIFETIME

        t.on_tick(&mut anims, base + TOAST_LIFETIME);
        assert!(!t.is_empty(), "second toast still alive");
        assert_eq!(t.messages(), vec!["second"]);

        t.on_tick(&mut anims, midpoint + TOAST_LIFETIME);
        assert!(t.is_empty());
    }

    /// The fade-out retarget fires wall-clock, at exactly
    /// `TOAST_LIFETIME - TOAST_FADE_DUR` after push — not tick-counted —
    /// and only once, even across ticks that land past that boundary.
    #[test]
    fn fade_retarget_fires_once_at_the_wall_clock_fade_window() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        let base = Instant::now();
        t.push("fading", ToastKind::Info);
        // Stamps expires_at and starts (then settles) the slide-in anim, the
        // same as `App::update` does after every `apply(action)`.
        t.start_pending_anims(&mut anims, base, Duration::from_millis(1));
        let fade_start = base + TOAST_LIFETIME - TOAST_FADE_DUR;

        t.on_tick(&mut anims, fade_start - Duration::from_millis(1));
        let before = anims.value(AnimKey::ToastFade(FIRST_TOAST_ID), fade_start);
        assert_eq!(
            before,
            Some(1.0),
            "still fully settled just before the fade window"
        );

        t.on_tick(&mut anims, fade_start);
        let just_after = anims.value_or(
            AnimKey::ToastFade(FIRST_TOAST_ID),
            fade_start + Duration::from_millis(1),
            -1.0,
        );
        assert!(
            just_after < 1.0,
            "fade must have started retargeting toward 0.0 at fade_start: {just_after}"
        );

        // A later tick landing well past fade_start must not re-retarget
        // (which would restart the ease from wherever it had drifted to).
        t.on_tick(&mut anims, fade_start + Duration::from_millis(50));
        let after_second_tick = anims.value_or(
            AnimKey::ToastFade(FIRST_TOAST_ID),
            fade_start + Duration::from_millis(51),
            -1.0,
        );
        assert!(
            after_second_tick <= just_after,
            "fade must keep easing monotonically toward 0, not restart"
        );
    }

    #[test]
    fn draw_renders_message_bottom_right() {
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

    /// Toasts stack bottom-right of the given rect: the newest rests one
    /// breathing row off its bottom edge, older ones above it.
    #[test]
    fn draw_stacks_bottom_right_newest_at_the_bottom() {
        let mut t = Toasts::default();
        t.push("older", ToastKind::Info);
        t.push("newest", ToastKind::Info);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let anims = Anims::new(true);
        terminal
            .draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now()))
            .unwrap();
        let buf = terminal.backend().buffer();
        let row_of = |needle: &str| {
            (0..buf.area.height)
                .find(|y| {
                    let row: String = (0..buf.area.width)
                        .map(|x| buf[(x, *y)].symbol().to_string())
                        .collect();
                    row.contains(needle)
                })
                .unwrap_or_else(|| panic!("{needle:?} not on screen"))
        };
        let newest = row_of("newest");
        let older = row_of("older");
        // Text sits on the middle row of a 3-row toast whose bottom edge
        // is 1 row off the rect's bottom (height 20 → rows 16..19, text 17).
        assert_eq!(newest, 20 - 1 - TOAST_HEIGHT + TOAST_HEIGHT / 2);
        assert_eq!(older, newest - TOAST_HEIGHT, "older stacks above");
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
