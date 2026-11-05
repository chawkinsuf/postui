//! Eased animated values driving the motion catalog. Time is always passed
//! in, never sampled, so tests are deterministic.
use std::collections::HashMap;
use std::sync::Mutex;
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
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// Which easing curve an [`Anim`] eases through: `OutCubic` for every
/// [`Anims::retarget`], `InOutCubic` for the tab-strip underline glide
/// ([`Anims::aim_underline`]).
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

/// Identifies which horizontal tab strip an underline belongs to (see
/// [`Anims::aim_underline`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StripId {
    EditorTabs,
    ResponseTabs,
    ManageTabs,
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
    ListTravel(ListId),
    Hover,
    /// The address bar's URL well hover fade, kept apart from the shared
    /// [`AnimKey::Hover`] because the well is a *composite* control: the
    /// lock, the copy chip and the URL's `{{token}}`s all sit inside it and
    /// register their own hits, so the shared key — which restarts on every
    /// hit change — would dip the well's fill back to rest each time the
    /// pointer crossed onto one of them. This one restarts only when the
    /// pointer enters the well's rect from outside it
    /// (`App::begin_hover_fade`).
    UrlWellHover,
    FocusFade,
    ModalOpen,
    PaneCollapse,
    /// The Response pane's hide/show collapse (0.0 shown → 1.0 hidden),
    /// driven by `Action::ToggleResponseCollapse` with the same easing and
    /// duration as [`AnimKey::PaneCollapse`].
    ResponseCollapse,
    /// The editor's share of the main column while both panes are shown
    /// (see `layout::compute_layout`'s `editor_share`), easing to each new
    /// share a chip or keyboard stop sets, with the same duration as
    /// [`AnimKey::PaneCollapse`]. A live drag snaps it (spec E14).
    SplitRatio,
    SendBreathe,
    ToastFade(u64),
}

#[derive(Clone, Copy)]
struct Anim {
    start: f32,
    target: f32,
    started: Instant,
    dur: Duration,
    easing: Easing,
}

impl Anim {
    /// Sits at `value`, with nothing in flight.
    fn at(value: f32) -> Self {
        Anim {
            start: value,
            target: value,
            started: Instant::now() - Duration::from_secs(1),
            dur: Duration::ZERO,
            easing: Easing::OutCubic,
        }
    }

    /// Lands at once on its target, as if it had finished.
    fn land(&mut self) {
        self.start = self.target;
        self.dur = Duration::ZERO;
    }

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

/// One tab strip's accent underline (#119): the tab it is aimed at, its
/// two edges in absolute screen columns (eased apart, so the segment
/// stretches toward the new tab rather than only translating), and the
/// frame that last painted it.
#[derive(Clone, Copy)]
struct Underline {
    tab: usize,
    left: Anim,
    right: Anim,
    painted: u64,
}

/// Tracks a set of eased animated values keyed by [`AnimKey`]. Time is
/// always supplied by the caller (never sampled internally), so behavior
/// is fully deterministic and testable.
pub struct Anims {
    pub enabled: bool,
    entries: HashMap<AnimKey, Anim>,
    /// The tab strips' underlines, aimed while each strip paints
    /// ([`Self::aim_underline`]), from the area and spans only the paint
    /// knows. The paint holds `&Anims` (through `DrawCtx`), hence the
    /// lock; nothing contends for it, and a `Mutex` rather than a
    /// `RefCell` keeps `Anims` `Sync` for the test helpers that share one
    /// in a `static`.
    underlines: Mutex<HashMap<StripId, Underline>>,
    /// Counts the frames `ui::draw` has begun ([`Self::begin_frame`]), so
    /// an underline can tell whether its strip was painted last frame.
    frame: u64,
    /// How long an underline glide takes, from
    /// `ui_settings.anim_ms.tab_slide`, as of the frame being drawn.
    tab_slide: Duration,
}

impl Anims {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            entries: HashMap::new(),
            underlines: Mutex::default(),
            frame: 0,
            tab_slide: Duration::ZERO,
        }
    }

    fn underlines(&self) -> std::sync::MutexGuard<'_, HashMap<StripId, Underline>> {
        // A panic mid-paint poisons nothing this map relies on: each entry
        // is replaced whole.
        self.underlines.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a frame: `ui::draw` calls it first, with the configured glide
    /// duration. A strip that was not painted in the frame before this one
    /// has its underline placed rather than glided ([`Self::aim_underline`]).
    pub fn begin_frame(&mut self, tab_slide: Duration) {
        self.frame += 1;
        self.tab_slide = tab_slide;
    }

    /// Aims `strip`'s underline at its active tab, `tab` (an identity the
    /// strip keeps for that tab whatever its index), whose label paints at
    /// absolute columns `[left, right)` this frame, and answers where to
    /// draw the edges at `now`:
    ///
    /// - the strip was not painted last frame (a screen just opened, a pane
    ///   shown again, the first frame): placed on the tab;
    /// - another tab than last frame's: a glide from where the underline is
    ///   drawn now, over the glide duration, in-out cubic (zero while
    ///   animations are off);
    /// - the same tab at new columns (a badge, a count, a resize): each edge
    ///   moves by what the label's moved, at once. A glide still running
    ///   toward this tab moves with it, so it lands on the label.
    ///
    /// Absolute columns keep a still tab still when the strip around it
    /// moves (the Response strip is right-aligned).
    pub fn aim_underline(
        &self,
        strip: StripId,
        tab: usize,
        (left, right): (f32, f32),
        now: Instant,
    ) -> (f32, f32) {
        let mut underlines = self.underlines();
        let painted = self.frame;
        let last = underlines.get(&strip).filter(|u| u.painted + 1 >= painted);
        let next = match last {
            None => Underline { tab, left: Anim::at(left), right: Anim::at(right), painted },
            Some(u) if u.tab != tab => {
                let dur = if self.enabled { self.tab_slide } else { Duration::ZERO };
                let glide = |from: &Anim, to: f32| Anim {
                    start: from.value(now),
                    target: to,
                    started: now,
                    dur,
                    easing: Easing::InOutCubic,
                };
                Underline {
                    tab,
                    left: glide(&u.left, left),
                    right: glide(&u.right, right),
                    painted,
                }
            }
            Some(u) => {
                let follow = |edge: Anim, to: f32| {
                    let by = to - edge.target;
                    Anim { start: edge.start + by, target: to, ..edge }
                };
                Underline {
                    tab,
                    left: follow(u.left, left),
                    right: follow(u.right, right),
                    painted,
                }
            }
        };
        underlines.insert(strip, next);
        (next.left.value(now), next.right.value(now))
    }

    /// Flips the enabled flag without replacing the entries. What a live
    /// config reload uses: replacing `Anims` wholesale would wipe every
    /// in-flight animation, freezing whatever was mid-transition at its
    /// start value.
    ///
    /// Turning motion *off* is an instruction about now, not about the
    /// next transition: everything in flight lands on its target
    /// immediately, so the user sees the still frame they asked for and
    /// the tick loop stops redrawing for animations that would otherwise
    /// go on easing invisibly. Turning it on leaves the entries where
    /// they are — only the next `retarget` eases.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.finish_all();
        }
    }

    /// Sets `key`'s value instantly, with no transition. Used for
    /// first-frame init and overlay close.
    pub fn snap(&mut self, key: AnimKey, value: f32) {
        self.entries.insert(key, Anim::at(value));
    }

    /// Starts `key` easing toward `target` over `dur`, beginning from its
    /// current value at `now` (so reversing mid-flight doesn't jump). When
    /// animations are disabled, or `dur` is zero, the value jumps instantly.
    /// Always eases out-cubic (only the underline glide eases in-out, in
    /// [`Anims::aim_underline`]).
    pub fn retarget(&mut self, key: AnimKey, target: f32, dur: Duration, now: Instant) {
        let start = self.value(key, now).unwrap_or(target);
        let dur = if self.enabled { dur } else { Duration::ZERO };
        let easing = Easing::OutCubic;
        self.entries.insert(key, Anim { start, target, started: now, dur, easing });
    }

    /// The value `key` is easing toward (its resting value once done), or
    /// `None` if it was never set. Lets a sync pass ask "is this already
    /// headed where I want?" without cutting a glide short.
    pub fn target(&self, key: AnimKey) -> Option<f32> {
        self.entries.get(&key).map(|a| a.target)
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

    /// Whether any tracked animation is still in flight at `now`, an
    /// underline glide included: one started at paint keeps the main loop
    /// ticking (`App::animating`) until it lands.
    pub fn active(&self, now: Instant) -> bool {
        self.entries.values().any(|a| !a.done(now))
            || self.underlines().values().any(|u| !u.left.done(now) || !u.right.done(now))
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
    /// toast's own slide-in). Production code reaches it only through
    /// [`Self::set_enabled`], turning motion off.
    pub fn finish_all(&mut self) {
        self.entries.values_mut().for_each(Anim::land);
        for u in self.underlines().values_mut() {
            u.left.land();
            u.right.land();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turning animations off is an instruction to stop moving, not to
    /// keep easing invisibly: whatever is in flight lands on its target
    /// at once, so the tick loop stops redrawing for it.
    #[test]
    fn disabling_snaps_everything_in_flight_to_its_target() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);

        a.set_enabled(false);

        assert_eq!(a.value(AnimKey::Hover, t0), Some(1.0), "landed at once");
        assert!(!a.active(t0), "and nothing is left to redraw for");
    }

    /// Turning them back on leaves the entries alone — their values are
    /// where the app last put them; only the *next* transition eases.
    #[test]
    fn enabling_leaves_in_flight_entries_alone() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);

        a.set_enabled(true);

        let mid = a.value(AnimKey::Hover, t0 + Duration::from_millis(50)).unwrap();
        assert!(mid > 0.0 && mid < 1.0, "still mid-transition, got {mid}");
    }

    #[test]
    fn retarget_eases_toward_target_and_finishes() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        assert_eq!(a.value(AnimKey::Hover, t0), Some(0.0));
        let mid = a.value(AnimKey::Hover, t0 + Duration::from_millis(50)).unwrap();
        assert!(mid > 0.5, "ease-out is past halfway at t=0.5, got {mid}"); // ease_out_cubic(0.5)=0.875
        assert!(mid < 1.0);
        assert_eq!(a.value(AnimKey::Hover, t0 + Duration::from_millis(100)), Some(1.0));
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
        assert_eq!(a.value(AnimKey::Hover, t_half), Some(v_half), "no jump on reversal");
    }

    #[test]
    fn is_done_tracks_a_single_key_and_defaults_true_when_untracked() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        assert!(a.is_done(AnimKey::Hover, t0), "an untracked key has nothing in flight");
        a.snap(AnimKey::Hover, 0.0);
        a.retarget(AnimKey::Hover, 1.0, Duration::from_millis(100), t0);
        assert!(!a.is_done(AnimKey::Hover, t0 + Duration::from_millis(50)));
        assert!(a.is_done(AnimKey::Hover, t0 + Duration::from_millis(100)));
    }

    #[test]
    fn is_static_distinguishes_a_hold_from_a_move() {
        let t0 = Instant::now();
        let mut a = Anims::new(true);
        assert!(!a.is_static(AnimKey::Hover), "an untracked key isn't a hold");
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

    /// #119 R6: a glide still running toward a tab whose label moves (a
    /// badge, a count) moves with it, edge by edge, and lands on the new
    /// columns.
    #[test]
    fn a_glide_toward_a_tab_that_moves_lands_on_its_new_columns() {
        let t0 = Instant::now();
        let dur = Duration::from_millis(100);
        let mut a = Anims::new(true);
        let strip = StripId::EditorTabs;
        a.begin_frame(dur);
        a.aim_underline(strip, 0, (0.0, 4.0), t0);
        a.begin_frame(dur);
        assert_eq!(a.aim_underline(strip, 1, (10.0, 14.0), t0), (0.0, 4.0), "a glide starts");
        a.begin_frame(dur);
        let (quarter, _) = a.aim_underline(strip, 1, (10.0, 14.0), t0 + dur / 4);
        assert!(quarter < 2.5, "in-out cubic starts slow, got {quarter} at t=0.25");
        let mid = t0 + Duration::from_millis(50);
        a.begin_frame(dur);
        let (l, r) = a.aim_underline(strip, 1, (10.0, 14.0), mid);
        a.begin_frame(dur);
        assert_eq!(a.aim_underline(strip, 1, (12.0, 17.0), mid), (l + 2.0, r + 3.0));
        a.begin_frame(dur);
        assert_eq!(a.aim_underline(strip, 1, (12.0, 17.0), t0 + dur), (12.0, 17.0));
    }

    /// #119 R7: a strip that missed a frame is placed, whatever tab its
    /// underline was last aimed at.
    #[test]
    fn a_strip_not_painted_last_frame_is_placed() {
        let t0 = Instant::now();
        let dur = Duration::from_millis(100);
        let mut a = Anims::new(true);
        let strip = StripId::ManageTabs;
        a.begin_frame(dur);
        a.aim_underline(strip, 0, (0.0, 4.0), t0);
        a.begin_frame(dur);
        a.begin_frame(dur);
        assert_eq!(a.aim_underline(strip, 3, (30.0, 38.0), t0), (30.0, 38.0));
        assert!(!a.active(t0), "nothing in flight");
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
