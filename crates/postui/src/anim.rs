//! Eased animated values driving the motion catalog. Time is always passed
//! in, never sampled, so tests are deterministic.
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
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
    TabUnderline(StripId),
    TabUnderlineWidth(StripId),
    ListTravel(ListId),
    Hover,
    FocusFade,
    ModalOpen,
    DropdownOpen,
    PaneCollapse,
    SendBreathe,
    ToastFade(u64),
}

struct Anim {
    start: f32,
    target: f32,
    started: Instant,
    dur: Duration,
}

impl Anim {
    fn value(&self, now: Instant) -> f32 {
        if self.dur.is_zero() {
            return self.target;
        }
        let t = now.saturating_duration_since(self.started).as_secs_f32() / self.dur.as_secs_f32();
        self.start + (self.target - self.start) * ease_out_cubic(t)
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
            },
        );
    }

    /// Starts `key` easing toward `target` over `dur`, beginning from its
    /// current value at `now` (so reversing mid-flight doesn't jump). When
    /// animations are disabled, or `dur` is zero, the value jumps instantly.
    pub fn retarget(&mut self, key: AnimKey, target: f32, dur: Duration, now: Instant) {
        let start = self.value(key, now).unwrap_or(target);
        let dur = if self.enabled { dur } else { Duration::ZERO };
        self.entries.insert(
            key,
            Anim {
                start,
                target,
                started: now,
                dur,
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
    fn disabled_anims_jump_instantly() {
        let t0 = Instant::now();
        let mut a = Anims::new(false);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        assert_eq!(a.value(AnimKey::Hover, t0), Some(1.0));
        assert!(!a.active(t0));
    }
}
