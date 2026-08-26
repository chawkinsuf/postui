//! Eased animated values driving the motion catalog. Time is always passed
//! in, never sampled, so tests are deterministic.
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Symmetric in-out cubic: slow start, fast middle, slow finish. Used only
/// by the tab-strip underline slide (Task 10, controller amendment 2) — the
/// reference app's stretchy leading-edge motion reads better eased in-out
/// than the default ease-out every other animation uses.
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Which easing curve an [`Anim`] eases through. Defaults to `OutCubic`
/// (every existing call site, via [`Anims::retarget`]); `InOutCubic` is
/// opt-in via [`Anims::retarget_with`] — currently only the tab-strip
/// underline slide (Task 10).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Easing {
    #[default]
    OutCubic,
    InOutCubic,
}

impl Easing {
    fn apply(self, t: f32) -> f32 {
        match self {
            Easing::OutCubic => ease_out_cubic(t),
            Easing::InOutCubic => ease_in_out_cubic(t),
        }
    }
}

/// Identifies which horizontal tab strip an animation belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StripId {
    EditorTabs,
    ResponseTabs,
}

/// Identifies which scrollable list an animation belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ListId {
    Sidebar,
    Palette,
    Chooser,
    VarPicker,
    Dropdown,
    VarManager,
}

/// Identifies a single animated value tracked by [`Anims`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AnimKey {
    /// The tab strip's accent segment LEFT edge, in fractional columns
    /// relative to the strip's own origin. Task 10 controller amendment:
    /// originally planned as a `(left, width)` pair, reinterpreted as two
    /// independently animated edges — paired with `TabUnderlineWidth`,
    /// which (despite its name, kept for continuity with the original
    /// plan) now holds the segment's RIGHT edge, not a width. Animating
    /// the edges independently lets the segment stretch instead of just
    /// translating, reproducing the reference app's leading-edge slide.
    TabUnderline(StripId),
    /// The tab strip's accent segment RIGHT edge — see [`AnimKey::TabUnderline`]'s
    /// doc comment for the reinterpretation.
    TabUnderlineWidth(StripId),
    ListTravel(ListId),
    Hover,
    FocusFade,
    ModalOpen,
    DropdownOpen,
    PaneCollapse,
    /// The Response pane's hide/show collapse (0.0 shown → 1.0 hidden),
    /// driven by `Action::ToggleResponseCollapse` with the same easing and
    /// duration as [`AnimKey::PaneCollapse`].
    ResponseCollapse,
    SendBreathe,
    ToastFade(u64),
}

struct Anim {
    start: f32,
    target: f32,
    started: Instant,
    dur: Duration,
    easing: Easing,
}

impl Anim {
    fn value(&self, now: Instant) -> f32 {
        if self.dur.is_zero() {
            return self.target;
        }
        let t = now.saturating_duration_since(self.started).as_secs_f32() / self.dur.as_secs_f32();
        self.start + (self.target - self.start) * self.easing.apply(t)
    }
    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.dur
    }
}

/// Tracks a set of eased animated values keyed by [`AnimKey`]. Time is
/// always supplied by the caller (never sampled internally), so behavior
/// is fully deterministic and testable.
pub struct Anims {
    pub enabled: bool,
    entries: HashMap<AnimKey, Anim>,
}

impl Anims {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            entries: HashMap::new(),
        }
    }

    /// Sets `key`'s value instantly, with no transition. Used for
    /// first-frame init and overlay close.
    pub fn snap(&mut self, key: AnimKey, value: f32) {
        self.entries.insert(
            key,
            Anim {
                start: value,
                target: value,
                started: Instant::now() - Duration::from_secs(1),
                dur: Duration::ZERO,
                easing: Easing::OutCubic,
            },
        );
    }

    /// Starts `key` easing toward `target` over `dur`, beginning from its
    /// current value at `now` (so reversing mid-flight doesn't jump). When
    /// animations are disabled, or `dur` is zero, the value jumps instantly.
    /// Always eases out-cubic; use [`Anims::retarget_with`] for a different
    /// curve.
    pub fn retarget(&mut self, key: AnimKey, target: f32, dur: Duration, now: Instant) {
        self.retarget_with(key, target, dur, now, Easing::OutCubic);
    }

    /// Like [`Anims::retarget`], but with an explicit easing curve. Used by
    /// the tab-strip underline slide (Task 10) for its `InOutCubic` motion;
    /// every other call site keeps using `retarget`'s default `OutCubic`.
    pub fn retarget_with(
        &mut self,
        key: AnimKey,
        target: f32,
        dur: Duration,
        now: Instant,
        easing: Easing,
    ) {
        let start = self.value(key, now).unwrap_or(target);
        let dur = if self.enabled { dur } else { Duration::ZERO };
        self.entries.insert(
            key,
            Anim {
                start,
                target,
                started: now,
                dur,
                easing,
            },
        );
    }

    /// The current eased value of `key` at `now`, or `None` if it was never
    /// set.
    pub fn value(&self, key: AnimKey, now: Instant) -> Option<f32> {
        self.entries.get(&key).map(|a| a.value(now))
    }

    /// Like [`Anims::value`], but returns `default` instead of `None`.
    pub fn value_or(&self, key: AnimKey, now: Instant, default: f32) -> f32 {
        self.value(key, now).unwrap_or(default)
    }

    /// Whether any tracked animation is still in flight at `now`.
    pub fn active(&self, now: Instant) -> bool {
        self.entries.values().any(|a| !a.done(now))
    }

    /// Whether `key`'s own tracked animation (if any) has finished at
    /// `now`. A key with no tracked value at all counts as done — there's
    /// nothing in flight to wait on. Unlike [`Anims::active`] (which asks
    /// about every key at once), this is what a looping demo driver polls
    /// per key to decide when to retarget it.
    pub fn is_done(&self, key: AnimKey, now: Instant) -> bool {
        self.entries.get(&key).is_none_or(|a| a.done(now))
    }

    /// Whether `key`'s current entry is a "hold" — its `start` and
    /// `target` are the same value, so [`Anims::value`] doesn't move even
    /// while the entry counts as active. A looping demo driver uses
    /// [`Anims::retarget`] with `target` equal to the value it just
    /// arrived at to implement a dwell pause (still `active` for the
    /// dwell's duration, so the main loop keeps ticking through it) —
    /// this is how the driver tells a finished dwell apart from a
    /// finished move without any extra state of its own. Returns `false`
    /// for an untracked key.
    pub fn is_static(&self, key: AnimKey) -> bool {
        self.entries.get(&key).is_some_and(|a| a.start == a.target)
    }

    /// Drops `key`'s tracked value entirely.
    pub fn clear(&mut self, key: AnimKey) {
        self.entries.remove(&key);
    }

    /// Force-settles every tracked animation to its own target, as if it
    /// had already finished. For tests whose whole purpose is asserting on
    /// otherwise-static content (e.g. `app::tests::rendered_text`'s toast
    /// wording checks) that would otherwise land mid-flight of some
    /// unrelated animation the same action happens to have started (e.g. a
    /// toast's own slide-in) — production code never calls this.
    pub fn finish_all(&mut self) {
        for a in self.entries.values_mut() {
            a.start = a.target;
            a.dur = Duration::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retarget_eases_toward_target_and_finishes() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        assert_eq!(a.value(AnimKey::Hover, t0), Some(0.0));
        let mid = a
            .value(AnimKey::Hover, t0 + Duration::from_millis(50))
            .unwrap();
        assert!(mid > 0.5, "ease-out is past halfway at t=0.5, got {mid}"); // ease_out_cubic(0.5)=0.875
        assert!(mid < 1.0);
        assert_eq!(
            a.value(AnimKey::Hover, t0 + Duration::from_millis(100)),
            Some(1.0)
        );
        assert!(a.active(t0 + Duration::from_millis(50)));
        assert!(!a.active(t0 + Duration::from_millis(150)));
    }

    #[test]
    fn retarget_mid_flight_starts_from_current_value() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        let t_half = t0 + Duration::from_millis(50);
        let v_half = a.value(AnimKey::Hover, t_half).unwrap();
        a.retarget(AnimKey::Hover, 0.0, Duration::from_millis(100), t_half); // reverse
        assert_eq!(
            a.value(AnimKey::Hover, t_half),
            Some(v_half),
            "no jump on reversal"
        );
    }

    #[test]
    fn is_done_tracks_a_single_key_and_defaults_true_when_untracked() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        assert!(
            a.is_done(AnimKey::Hover, t0),
            "an untracked key has nothing in flight"
        );
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        assert!(!a.is_done(AnimKey::Hover, t0 + Duration::from_millis(50)));
        assert!(a.is_done(AnimKey::Hover, t0 + Duration::from_millis(100)));
    }

    #[test]
    fn is_static_distinguishes_a_hold_from_a_move() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        assert!(
            !a.is_static(AnimKey::Hover),
            "an untracked key isn't a hold"
        );
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        assert!(!a.is_static(AnimKey::Hover), "start != target: a move");
        let done_at = t0 + Duration::from_millis(100);
        // Hold at the value it just arrived at, for a dwell duration.
        let arrived = a.value(AnimKey::Hover, done_at).unwrap();
        a.retarget(AnimKey::Hover, arrived, Duration::from_millis(50), done_at);
        assert!(a.is_static(AnimKey::Hover), "start == target: a hold");
        assert!(
            !a.is_done(AnimKey::Hover, done_at),
            "the hold itself is still active until its own duration elapses"
        );
    }

    #[test]
    fn ease_in_out_cubic_is_symmetric_and_slow_at_both_ends() {
        assert_eq!(ease_in_out_cubic(0.0), 0.0);
        assert_eq!(ease_in_out_cubic(1.0), 1.0);
        assert_eq!(ease_in_out_cubic(0.5), 0.5, "symmetric at the midpoint");
        // Slow start: at t=0.25 the eased value is well under a linear 0.25.
        let quarter = ease_in_out_cubic(0.25);
        assert!(quarter < 0.25, "slow start, got {quarter}");
        // Slow finish: at t=0.75 the eased value is well over a linear 0.75.
        let three_quarter = ease_in_out_cubic(0.75);
        assert!(three_quarter > 0.75, "slow finish, got {three_quarter}");
        // Clamped outside [0, 1].
        assert_eq!(ease_in_out_cubic(-1.0), 0.0);
        assert_eq!(ease_in_out_cubic(2.0), 1.0);
    }

    #[test]
    fn retarget_with_uses_the_given_easing_curve() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget_with(
            AnimKey::Hover,
            1.0,
            Duration::from_millis(100),
            t0,
            Easing::InOutCubic,
        );
        let quarter = a
            .value(AnimKey::Hover, t0 + Duration::from_millis(25))
            .unwrap();
        assert!(
            quarter < 0.25,
            "in-out cubic starts slow, got {quarter} at t=0.25"
        );
        assert_eq!(
            a.value(AnimKey::Hover, t0 + Duration::from_millis(100)),
            Some(1.0)
        );
    }

    #[test]
    fn disabled_anims_jump_instantly() {
        let t0 = Instant::now();
        let mut a = Anims::new(false);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        assert_eq!(a.value(AnimKey::Hover, t0), Some(1.0));
        assert!(!a.active(t0));
    }
}
