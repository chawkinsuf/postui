use crate::anim::{AnimKey, Anims};
use crate::hit::{Hit, HitMap};
use crate::paint::{Button, ButtonKind, ControlState, fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
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

/// How long the curl import notice shows (spec 2026-10-15 curl import
/// visibility V3, #240), not counting the time the pointer holds it.
const NOTICE_LIFETIME: Duration = Duration::from_secs(10);

/// The curl import notice's button label.
const NOTICE_IMPORT: &str = "Import";

/// The notice's close control: the × with a cell either side, so its
/// hover fill has room.
const NOTICE_CLOSE: &str = " \u{d7} ";

/// Where a click on the curl import notice lands (#240).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticePart {
    /// The notice between its controls: the click stops there.
    Panel,
    /// Imports the curl the notice reported, as ctrl+v on the sidebar
    /// imports the clipboard's.
    Import,
    /// The ×: dismisses the notice.
    Close,
}

/// The curl import notice's own part of its toast.
struct Notice {
    /// The hint after its Import button ("or ^V in the sidebar"), empty
    /// when the key is unbound.
    hint: String,
    /// The clipboard text it reported, which its Import imports.
    curl: String,
    /// Whether the pointer has moved onto it since it last left.
    pointed: bool,
}

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
    /// While `Some`, its clock stands still with this much life left
    /// ([`Toasts::hold_curl_notice`]).
    held: Option<Duration>,
    /// `Some` for the curl import notice.
    notice: Option<Notice>,
}

impl Toast {
    /// Whether it is still showing at `now`: not past its expiry, which
    /// the next tick would prune (one not stamped yet was just pushed).
    fn live(&self, now: Instant) -> bool {
        self.expires_at.is_none_or(|at| now < at)
    }
}

pub struct Toasts {
    entries: Vec<Toast>,
    next_id: u64,
    /// The curl import notice before it joins the stack
    /// ([`Self::show_curl_notice`]).
    waiting: Option<Toast>,
}

impl Default for Toasts {
    fn default() -> Self {
        Self { entries: Vec::new(), next_id: FIRST_TOAST_ID, waiting: None }
    }
}

impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.push_id(message, kind);
    }

    /// [`Self::push`], answering the toast's id for [`Self::is_live`] and
    /// [`Self::dismiss`]: for a toast whose showing is itself state, as
    /// "press ^C again to quit" is (task #85).
    pub fn push_id(&mut self, message: impl Into<String>, kind: ToastKind) -> u64 {
        let toast = self.new_toast(message.into(), kind, None);
        let id = toast.id;
        self.entries.push(toast);
        id
    }

    fn new_toast(&mut self, message: String, kind: ToastKind, notice: Option<Notice>) -> Toast {
        let id = self.next_id;
        self.next_id += 1;
        Toast { message, kind, expires_at: None, fade_started: false, id, held: None, notice }
    }

    /// The curl import notice (spec 2026-10-15 curl import visibility V3,
    /// #240) for the clipboard text `curl`: `message` with an Import
    /// button, `hint` and a ×. It waits to join the stack
    /// ([`Self::show_curl_notice`]), and replaces a notice still up, which
    /// spoke of an earlier clipboard.
    pub fn push_curl_notice(
        &mut self,
        message: impl Into<String>,
        hint: impl Into<String>,
        curl: impl Into<String>,
        anims: &mut Anims,
    ) {
        self.take_curl_notice(anims);
        let notice = Notice { hint: hint.into(), curl: curl.into(), pointed: false };
        self.waiting = Some(self.new_toast(message.into(), ToastKind::Info, Some(notice)));
    }

    /// Puts a waiting curl import notice into the stack, as the newest
    /// toast.
    pub fn show_curl_notice(&mut self) {
        self.entries.extend(self.waiting.take());
    }

    /// Takes the curl import notice down at once, waiting or up: the text
    /// it reported.
    pub fn take_curl_notice(&mut self, anims: &mut Anims) -> Option<String> {
        let toast = match self.waiting.take() {
            Some(toast) => toast,
            None => {
                let at = self.entries.iter().position(|t| t.notice.is_some())?;
                self.entries.remove(at)
            }
        };
        anims.clear(AnimKey::ToastFade(toast.id));
        toast.notice.map(|n| n.curl)
    }

    /// The pointer moved onto the curl import notice: from now until it
    /// leaves, it holds the notice ([`Self::hold_curl_notice`]).
    pub fn point_at_curl_notice(&mut self) {
        if let Some(notice) = self.entries.iter_mut().find_map(|t| t.notice.as_mut()) {
            notice.pointed = true;
        }
    }

    /// Holds the curl import notice while `pointer_on_it`, once the pointer
    /// has moved onto it ([`Self::point_at_curl_notice`]): its clock stands
    /// still with the life it had left, at least the fade, and a fade begun
    /// is undone ([`Self::on_tick`]). Let go, it runs on from there.
    pub fn hold_curl_notice(&mut self, pointer_on_it: bool, now: Instant) {
        for t in &mut self.entries {
            let Some(notice) = &mut t.notice else { continue };
            notice.pointed &= pointer_on_it;
            let left = || {
                let at =
                    t.expires_at.map_or(NOTICE_LIFETIME, |at| at.saturating_duration_since(now));
                at.max(TOAST_FADE_DUR)
            };
            t.held = notice.pointed.then(|| t.held.unwrap_or_else(left));
        }
    }

    /// The curl import notice's message and hint, waiting or up.
    #[cfg(test)]
    pub fn curl_notice(&self) -> Option<(&str, &str)> {
        self.waiting
            .iter()
            .chain(&self.entries)
            .find_map(|t| Some((t.message.as_str(), t.notice.as_ref()?.hint.as_str())))
    }

    /// [`Self::push`], unless the same message is already up: for a toast a
    /// held key would otherwise repeat (task #85's "close the dialog
    /// first").
    pub fn push_once(&mut self, message: &str, kind: ToastKind) {
        let now = Instant::now();
        if !self.entries.iter().any(|t| t.message == message && t.live(now)) {
            self.push(message, kind);
        }
    }

    /// Whether toast `id` is still showing at `now`: in the list and not
    /// past its expiry, which the next tick would prune. A toast `draw`
    /// leaves off a screen too short for the whole stack still counts:
    /// the quit's arm (task #85) is pushed newest, and the presses are back
    /// to back, so only a toast landing between them could hide it, and
    /// the quit it allows still meets the unsaved-changes question.
    pub fn is_live(&self, id: u64, now: Instant) -> bool {
        self.entries.iter().any(|t| t.id == id && t.live(now))
    }

    /// Removes toast `id` at once, its animation with it.
    pub fn dismiss(&mut self, id: u64, anims: &mut Anims) {
        self.entries.retain(|t| t.id != id);
        anims.clear(AnimKey::ToastFade(id));
    }

    /// Stamps `expires_at` on every toast that doesn't have one yet --
    /// i.e. every toast pushed since the last call to this or `on_tick`.
    /// Not done in `push` itself, so the ~100 `Toasts::push` call sites
    /// across `app.rs` don't each need `now` threaded through.
    ///
    /// The lifetime scales with reading time: [`TOAST_LIFETIME`] covers
    /// the first 40 chars, then ~35ms per additional char, capped at 8s —
    /// a long validation error stays readable instead of vanishing on the
    /// same clock as a one-word "Saved". The curl import notice lives
    /// [`NOTICE_LIFETIME`].
    fn stamp_pending(&mut self, now: Instant) {
        for t in &mut self.entries {
            if t.expires_at.is_none() {
                let extra_chars = t.message.chars().count().saturating_sub(40) as u64;
                let lifetime = match t.notice {
                    Some(_) => NOTICE_LIFETIME,
                    None => (TOAST_LIFETIME + Duration::from_millis(extra_chars * 35))
                        .min(Duration::from_secs(8)),
                };
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
    ///
    /// A held toast's clock stands still at full strength
    /// ([`Self::hold_curl_notice`]).
    pub fn on_tick(&mut self, anims: &mut Anims, now: Instant) -> bool {
        // Captured before pruning: on the tick where the last toast expires,
        // `entries` goes from non-empty to empty, and that transition is
        // itself a state change that needs one final redraw to erase the
        // toast. Returning based on the post-prune state would swallow that
        // frame and leave the toast painted until the next keypress.
        let was_visible = !self.entries.is_empty();
        self.stamp_pending(now);
        for t in &mut self.entries {
            if let Some(left) = t.held {
                t.expires_at = Some(now + left);
                if t.fade_started {
                    t.fade_started = false;
                    anims.retarget(AnimKey::ToastFade(t.id), 1.0, TOAST_FADE_DUR, now);
                }
                continue;
            }
            let fade_start = t.expires_at.expect("stamped above") - TOAST_FADE_DUR;
            if !t.fade_started && now >= fade_start {
                t.fade_started = true;
                anims.retarget(AnimKey::ToastFade(t.id), 0.0, TOAST_FADE_DUR, now);
            }
        }
        // Drop the expiring toasts' own `AnimKey::ToastFade(id)` entries
        // along with the toasts themselves -- otherwise every toast ever
        // pushed leaves a permanently-done (but never-removed) `Anim` in
        // `Anims`'s map for the life of the process.
        for t in self.entries.iter().filter(|t| now >= t.expires_at.expect("stamped above")) {
            anims.clear(AnimKey::ToastFade(t.id));
        }
        self.entries.retain(|t| now < t.expires_at.expect("stamped above"));
        was_visible
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every toast's message text, oldest first, then a waiting curl import
    /// notice's — for tests that need to assert on wording, not just
    /// presence.
    #[cfg(test)]
    pub fn messages(&self) -> Vec<&str> {
        self.entries.iter().chain(&self.waiting).map(|t| t.message.as_str()).collect()
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
        self.entries.iter().map(|t| (t.message.as_str(), &t.kind)).collect()
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
    /// blends toward `theme.page` by `1 - t` ([`crate::paint::fade_to`]) —
    /// replacing the old hard cutoff with a continuous fade. Reusing `t`
    /// for the color blend during the slide too (not just the offset) is
    /// a deliberate bonus: the toast fades in color-wise as it slides,
    /// rather than snapping to full color mid-slide.
    ///
    /// The curl import notice paints its controls ([`paint_notice`]),
    /// registering them in `hits` and lighting the hovered one. With
    /// `hits` `None` (a dialog is up), or on a screen too narrow for
    /// them, it paints its message alone, at its own width.
    pub fn draw(
        &self,
        frame: &mut Frame,
        screen: Rect,
        theme: &Theme,
        anims: &Anims,
        now: Instant,
        mut hits: Option<(&mut HitMap, Option<&Hit>)>,
    ) {
        let notice_fits = usize::from(screen.width) >= notice_width("", "");
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
            // Display cells, not chars, for double-width chars (emoji, CJK).
            let display_width = crate::ui::cells(&toast.message);
            // Clamp in usize before narrowing, so a very long message can't wrap.
            let padding_width: usize = 6; // 1 bar col + 1 gap col + 4 right margin
            let clamped_width = match &toast.notice {
                Some(n) => notice_width(&toast.message, &n.hint),
                None => display_width.saturating_add(padding_width),
            };
            let width = clamped_width.min(screen.width as usize) as u16;
            let rest_x = screen.right().saturating_sub(width.saturating_add(1));

            let t = anims.value_or(AnimKey::ToastFade(toast.id), now, 1.0);
            // `expires_at` may still be unstamped here only in a bare
            // `Toasts` test that draws before ever calling `on_tick`/
            // `start_pending_anims` -- treat that as "freshly pushed, not
            // fading yet", matching the real app (where `App::update`
            // always stamps it before the first draw).
            let fading = toast.expires_at.is_some_and(|e| now + TOAST_FADE_DUR >= e);
            let x_offset =
                if fading { 0 } else { ((1.0 - t).clamp(0.0, 1.0) * width as f32).round() as u16 };
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
            let buf = frame.buffer_mut();
            fill(buf, area, theme.panel);
            for row in area.top()..area.bottom() {
                if let Some(cell) = buf.cell_mut((area.x, row)) {
                    cell.set_symbol("\u{2588}");
                    cell.set_fg(color);
                }
            }
            match (&toast.notice, hits.as_mut()) {
                (Some(n), Some((map, hovered))) if notice_fits => {
                    paint_notice(buf, area, &toast.message, &n.hint, theme, map, *hovered);
                }
                _ => {
                    let text_y = area.y + area.height / 2;
                    text(buf, area.x + 2, text_y, &toast.message, theme.text, theme.panel, false);
                }
            }
            crate::paint::fade_to(buf, area, theme.page, fade_toward_page);
            y += TOAST_HEIGHT;
        }
    }
}

/// The curl import notice's width with its hint: the bar and a gap, the
/// message, the Import button, the hint, the × and a cell of margin, two
/// cells between each.
fn notice_width(message: &str, hint: &str) -> usize {
    let hint_w = match crate::ui::cells(hint) {
        0 => 0,
        w => w + 2,
    };
    2 + crate::ui::cells(message)
        + 2
        + usize::from(crate::paint::button_min_width(NOTICE_IMPORT))
        + hint_w
        + 2
        + crate::ui::cells(NOTICE_CLOSE)
        + 1
}

/// Paints the curl import notice's contents into `area`, over the toast's
/// panel and bar, laid out from its right edge: the ×, the hint, the
/// Import button, and the message in the room left, cut to fit. The hint
/// is left out when `area` is narrower than the notice with it. The
/// controls register in `hits`, the panel's hit first, so the controls'
/// win.
fn paint_notice(
    buf: &mut Buffer,
    area: Rect,
    message: &str,
    hint: &str,
    theme: &Theme,
    hits: &mut HitMap,
    hovered: Option<&Hit>,
) {
    let mid = area.y + area.height / 2;

    let close_w = crate::ui::cells(NOTICE_CLOSE) as u16;
    let close = Rect::new(area.right().saturating_sub(close_w + 1), mid, close_w, 1);
    let close_hit = Hit::CurlNotice(NoticePart::Close);
    crate::paint::action(buf, close, NOTICE_CLOSE, close_hit.clone(), hovered, theme.panel, theme);

    let mut right = close.x.saturating_sub(2);
    if !hint.is_empty() && usize::from(area.width) >= notice_width(message, hint) {
        let x = right.saturating_sub(crate::ui::cells(hint) as u16);
        text(buf, x, mid, hint, theme.text_muted, theme.panel, false);
        right = x.saturating_sub(2);
    }

    let import_w = crate::paint::button_min_width(NOTICE_IMPORT);
    let import = Rect::new(right.saturating_sub(import_w), area.y, import_w, TOAST_HEIGHT);
    let import_hit = Hit::CurlNotice(NoticePart::Import);
    let state =
        if hovered == Some(&import_hit) { ControlState::Hover } else { ControlState::Normal };
    let block =
        Button { label: NOTICE_IMPORT, kind: ButtonKind::Primary, state }.paint(buf, import, theme);

    let room = usize::from(import.x.saturating_sub(area.x + 4));
    let shown = crate::ui::ellipsize(message, room);
    text(buf, area.x + 2, mid, &shown, theme.text, theme.panel, false);

    hits.register(area, Hit::CurlNotice(NoticePart::Panel));
    hits.register(block, import_hit);
    hits.register(close, close_hit);
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
        assert!(!t.is_empty(), "a long message must outlive the base lifetime");
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
            anims.value(AnimKey::ToastFade(FIRST_TOAST_ID), now).is_none(),
            "start_pending_anims hasn't run yet in this bare-Toasts test"
        );
        anims.snap(AnimKey::ToastFade(FIRST_TOAST_ID), 0.0);
        anims.retarget(AnimKey::ToastFade(FIRST_TOAST_ID), 1.0, Duration::from_millis(100), now);
        assert!(
            anims.value(AnimKey::ToastFade(FIRST_TOAST_ID), now).is_some(),
            "the anim entry exists before expiry"
        );

        t.on_tick(&mut anims, now); // stamps expires_at = now + TOAST_LIFETIME
        t.on_tick(&mut anims, now + TOAST_LIFETIME);
        assert!(t.is_empty(), "the toast has expired");
        assert!(
            anims.value(AnimKey::ToastFade(FIRST_TOAST_ID), now).is_none(),
            "the expired toast's anim entry must be cleared, not just left done"
        );
    }

    #[test]
    fn on_tick_returns_true_on_the_expiring_tick_and_false_after() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        let base = Instant::now();
        t.push("bye", ToastKind::Info);
        assert!(t.on_tick(&mut anims, base), "still visible right after stamping");
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
        assert_eq!(before, Some(1.0), "still fully settled just before the fade window");

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

    /// The curl import notice (#240) lives 10 s, whatever its length.
    #[test]
    fn the_curl_notice_lives_ten_seconds() {
        let mut anims = Anims::new(true);
        let mut t = shown_notice(&mut anims);
        let base = Instant::now();
        t.on_tick(&mut anims, base);
        t.on_tick(&mut anims, base + NOTICE_LIFETIME - Duration::from_millis(1));
        assert!(t.curl_notice().is_some(), "alive just before 10 s");
        t.on_tick(&mut anims, base + NOTICE_LIFETIME);
        assert!(t.is_empty());
    }

    /// A newer notice takes the older one's place: one clipboard, one
    /// notice. Other toasts stay. Taking it down answers its text.
    #[test]
    fn a_new_curl_notice_replaces_the_old_one() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push("Saved", ToastKind::Success);
        t.push_curl_notice("first", "", "curl a", &mut anims);
        t.show_curl_notice();
        t.push_curl_notice("second", "", "curl b", &mut anims);
        assert_eq!(t.messages(), vec!["Saved", "second"]);
        assert_eq!(t.take_curl_notice(&mut anims).as_deref(), Some("curl b"));
        assert_eq!(t.messages(), vec!["Saved"]);
        assert_eq!(t.take_curl_notice(&mut anims), None);
    }

    /// A notice in the stack.
    fn shown_notice(anims: &mut Anims) -> Toasts {
        let mut t = Toasts::default();
        t.push_curl_notice("Clipboard holds a curl command", "or ^V in the sidebar", "c", anims);
        t.show_curl_notice();
        t
    }

    /// A notice the pointer has moved onto.
    fn pointed_notice(anims: &mut Anims) -> Toasts {
        let mut t = shown_notice(anims);
        t.point_at_curl_notice();
        t
    }

    /// The app's tick: the hold, then the toasts' own tick.
    fn tick(t: &mut Toasts, anims: &mut Anims, now: Instant, pointer_on_it: bool) -> bool {
        t.hold_curl_notice(pointer_on_it, now);
        t.on_tick(anims, now)
    }

    /// While the pointer holds the notice its clock stops; let go, it runs
    /// on from where it stopped.
    #[test]
    fn a_held_curl_notice_waits_and_then_runs_on() {
        let mut anims = Anims::new(true);
        let mut t = pointed_notice(&mut anims);
        let base = Instant::now();
        tick(&mut t, &mut anims, base, true);
        let held_at = base + Duration::from_secs(4);
        tick(&mut t, &mut anims, held_at, false);
        t.point_at_curl_notice();
        tick(&mut t, &mut anims, held_at, true);
        let let_go = base + Duration::from_secs(60);
        tick(&mut t, &mut anims, let_go, true);
        assert!(t.curl_notice().is_some(), "held: no expiry");
        tick(&mut t, &mut anims, let_go, false);
        tick(&mut t, &mut anims, let_go + Duration::from_millis(5999), false);
        assert!(t.curl_notice().is_some(), "the 6 s it had left");
        tick(&mut t, &mut anims, let_go + Duration::from_secs(6), false);
        assert!(t.is_empty());
    }

    /// A pointer that has not moved onto the notice (it came to rest where
    /// the notice was then drawn) holds nothing; one that left must move
    /// back onto it to hold it again.
    #[test]
    fn only_a_pointer_moved_onto_the_curl_notice_holds_it() {
        let mut anims = Anims::new(true);
        let mut t = shown_notice(&mut anims);
        let base = Instant::now();
        tick(&mut t, &mut anims, base, true);
        tick(&mut t, &mut anims, base + NOTICE_LIFETIME, true);
        assert!(t.is_empty(), "a resting pointer");

        let mut t = pointed_notice(&mut anims);
        tick(&mut t, &mut anims, base, true);
        tick(&mut t, &mut anims, base + Duration::from_secs(1), false);
        tick(&mut t, &mut anims, base + NOTICE_LIFETIME, true);
        assert!(t.is_empty(), "left, then back without moving onto it");
    }

    /// A hold during the fade brings it back to full strength, and the
    /// fade runs again once the pointer leaves.
    #[test]
    fn holding_a_fading_curl_notice_restores_it() {
        let mut anims = Anims::new(true);
        let mut t = shown_notice(&mut anims);
        let base = Instant::now();
        t.start_pending_anims(&mut anims, base, Duration::from_millis(1));
        let fading = base + NOTICE_LIFETIME - Duration::from_millis(200);
        tick(&mut t, &mut anims, fading, false);
        let mid = fading + Duration::from_millis(100);
        assert!(anims.value_or(AnimKey::ToastFade(FIRST_TOAST_ID), mid, 1.0) < 1.0, "fading");
        t.point_at_curl_notice();
        tick(&mut t, &mut anims, mid, true);
        let later = mid + Duration::from_secs(5);
        tick(&mut t, &mut anims, later, true);
        assert_eq!(anims.value(AnimKey::ToastFade(FIRST_TOAST_ID), later), Some(1.0));
        tick(&mut t, &mut anims, later, false);
        tick(&mut t, &mut anims, later + TOAST_FADE_DUR, false);
        assert!(t.is_empty(), "fades out once let go");
    }

    /// A notice waits out of the stack, with no slide and no clock, and is
    /// no reason to redraw. Shown, it slides in as a new toast does and has
    /// its 10 s from then.
    #[test]
    fn a_waiting_curl_notice_slides_in_once_shown() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        let base = Instant::now();
        let slide = Duration::from_millis(100);
        t.push_curl_notice("n", "", "c", &mut anims);
        t.start_pending_anims(&mut anims, base, slide);
        assert!(!t.on_tick(&mut anims, base));
        let shown = base + Duration::from_secs(60);
        assert!(!t.on_tick(&mut anims, shown));
        assert_eq!(anims.value(AnimKey::ToastFade(FIRST_TOAST_ID), shown), None, "no slide yet");
        t.show_curl_notice();
        t.start_pending_anims(&mut anims, shown, slide);
        assert_eq!(anims.value(AnimKey::ToastFade(FIRST_TOAST_ID), shown), Some(0.0), "slides in");
        t.on_tick(&mut anims, shown + NOTICE_LIFETIME - Duration::from_secs(1));
        assert!(t.curl_notice().is_some(), "its 10 s run from when it showed");
        t.on_tick(&mut anims, shown + NOTICE_LIFETIME);
        assert!(t.is_empty());
    }

    /// Only the notice holds: an ordinary toast runs out under the pointer
    /// as it always did.
    #[test]
    fn a_hold_leaves_ordinary_toasts_alone() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        let base = Instant::now();
        t.push("Saved", ToastKind::Success);
        tick(&mut t, &mut anims, base, true);
        tick(&mut t, &mut anims, base + TOAST_LIFETIME, true);
        assert!(t.is_empty());
    }

    /// Draws `t` on a screen `width` columns wide, with a map for its
    /// clicks unless `no_clicks` (as while a dialog is up).
    fn draw_toasts(
        t: &Toasts,
        width: u16,
        hovered: Option<&Hit>,
        no_clicks: bool,
    ) -> (Terminal<TestBackend>, HitMap) {
        let anims = Anims::new(true);
        let theme = Theme::dark();
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                let map = (!no_clicks).then_some((&mut hits, hovered));
                t.draw(f, f.area(), &theme, &anims, Instant::now(), map)
            })
            .unwrap();
        (terminal, hits)
    }

    fn draw_notice(
        width: u16,
        hint: &str,
        hovered: Option<&Hit>,
    ) -> (Terminal<TestBackend>, HitMap) {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push_curl_notice("Clipboard holds a curl command", hint, "c", &mut anims);
        t.show_curl_notice();
        draw_toasts(&t, width, hovered, false)
    }

    /// Waiting, the notice is not in the stack: the toast before it keeps
    /// the bottom place.
    #[test]
    fn a_waiting_curl_notice_is_not_drawn() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push("Saved", ToastKind::Success);
        t.push_curl_notice("Clipboard holds a curl command", "", "c", &mut anims);
        let (terminal, hits) = draw_toasts(&t, 80, None, false);
        let rows: Vec<String> = (0..20).map(|y| screen_row(&terminal, y)).collect();
        assert!(!rows.iter().any(|r| r.contains("Clipboard")));
        let saved = rows.iter().position(|r| r.contains("Saved")).expect("Saved");
        assert_eq!(saved, 20 - 1 - TOAST_HEIGHT as usize + 1, "the bottom place");
        assert_eq!(hits.rect_of(&Hit::CurlNotice(NoticePart::Import)), None);
    }

    /// With no map for its clicks (a dialog is up) the notice draws its
    /// message only, in its own place and at its own width: no control is
    /// painted or registered, and the toast above it does not move.
    #[test]
    fn with_no_clicks_the_curl_notice_draws_its_message_only_in_its_place() {
        let mut anims = Anims::new(true);
        let mut t = Toasts::default();
        t.push("Saved", ToastKind::Success);
        t.push_curl_notice(
            "Clipboard holds a curl command",
            "or ^V in the sidebar",
            "c",
            &mut anims,
        );
        t.show_curl_notice();
        let (live, _) = draw_toasts(&t, 100, None, false);
        let (bare, hits) = draw_toasts(&t, 100, None, true);
        for part in [NoticePart::Panel, NoticePart::Import, NoticePart::Close] {
            assert_eq!(hits.rect_of(&Hit::CurlNotice(part)), None, "{part:?}");
        }
        for y in 0..20 {
            let (a, b) = (screen_row(&live, y), screen_row(&bare, y));
            assert_eq!(a.find('\u{2588}'), b.find('\u{2588}'), "the bar, row {y}");
            if a.contains("Saved") {
                assert_eq!(a, b, "the toast above, row {y}");
            }
        }
        let mid = screen_row(&bare, 20 - 1 - TOAST_HEIGHT + 1);
        assert!(mid.contains("Clipboard holds a curl command"), "{mid:?}");
        for control in ["Import", "sidebar", "\u{d7}"] {
            assert!(!mid.contains(control), "{control}: {mid:?}");
        }
    }

    fn screen_row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    /// The notice is a toast with an Import button, the hint and a ×, each
    /// a hit, over a panel hit that takes the clicks between them.
    #[test]
    fn the_curl_notice_paints_its_controls_and_registers_them() {
        let (terminal, hits) = draw_notice(100, "or ^V in the sidebar", None);
        let import = hits.rect_of(&Hit::CurlNotice(NoticePart::Import)).expect("Import");
        let close = hits.rect_of(&Hit::CurlNotice(NoticePart::Close)).expect("×");
        let panel = hits.rect_of(&Hit::CurlNotice(NoticePart::Panel)).expect("panel");
        assert_eq!(panel.height, TOAST_HEIGHT);
        assert_eq!(import.height, TOAST_HEIGHT, "the button's caps are the toast's rows");
        let mid = screen_row(&terminal, panel.y + 1);
        let words: Vec<&str> = mid.split_whitespace().collect();
        assert_eq!(
            words[1..].join(" "),
            "Clipboard holds a curl command Import or ^V in the sidebar \u{d7}",
            "{mid:?}"
        );
        assert!(panel.x < import.x && import.right() < close.x && close.right() <= panel.right());
        assert_eq!(hits.hit_at(close.x + 1, close.y), Some(&Hit::CurlNotice(NoticePart::Close)));
        assert_eq!(hits.hit_at(panel.x + 3, panel.y), Some(&Hit::CurlNotice(NoticePart::Panel)));
    }

    /// Too narrow for everything, the hint goes first; the buttons stay.
    #[test]
    fn a_narrow_curl_notice_drops_its_hint() {
        let (terminal, hits) = draw_notice(60, "or ^V in the sidebar", None);
        let panel = hits.rect_of(&Hit::CurlNotice(NoticePart::Panel)).unwrap();
        let mid = screen_row(&terminal, panel.y + 1);
        assert!(!mid.contains("sidebar"), "{mid:?}");
        assert!(mid.contains("Import") && mid.contains('\u{d7}'), "{mid:?}");
        assert!(hits.rect_of(&Hit::CurlNotice(NoticePart::Import)).is_some());
    }

    /// A screen too narrow for the Import button and the × paints the
    /// notice's message alone: no control is painted or registered past
    /// the toast.
    #[test]
    fn a_screen_too_narrow_for_the_controls_paints_the_message_alone() {
        let min = notice_width("", "") as u16;
        let (terminal, hits) = draw_notice(min - 1, "or ^V in the sidebar", None);
        for part in [NoticePart::Panel, NoticePart::Import, NoticePart::Close] {
            assert_eq!(hits.rect_of(&Hit::CurlNotice(part)), None, "{part:?}");
        }
        let mid = screen_row(&terminal, 20 - 1 - TOAST_HEIGHT + 1);
        assert!(mid.contains("Clipboard") && !mid.contains("Import"), "{mid:?}");
        let (_, hits) = draw_notice(min, "or ^V in the sidebar", None);
        let panel = hits.rect_of(&Hit::CurlNotice(NoticePart::Panel)).unwrap();
        let import = hits.rect_of(&Hit::CurlNotice(NoticePart::Import)).unwrap();
        assert!(panel.x < import.x, "{panel:?} {import:?}");
    }

    /// The hovered Import lifts as every button does.
    #[test]
    fn the_hovered_import_button_lifts() {
        let theme = Theme::dark();
        let import = Hit::CurlNotice(NoticePart::Import);
        let face = |hovered| {
            let (terminal, hits) = draw_notice(100, "", hovered);
            let r = hits.rect_of(&import).unwrap();
            terminal.backend().buffer()[(r.x + 1, r.y + 1)].bg
        };
        let rest = crate::paint::control_face(&theme, ButtonKind::Primary, ControlState::Normal).0;
        let lit = crate::paint::control_face(&theme, ButtonKind::Primary, ControlState::Hover).0;
        assert_eq!(face(None), rest);
        assert_eq!(face(Some(&import)), lit);
    }

    #[test]
    fn draw_renders_message_bottom_right() {
        let mut t = Toasts::default();
        t.push("Copied \u{2713}", ToastKind::Success);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let anims = Anims::new(true);
        terminal.draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now(), None)).unwrap();
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
        terminal.draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now(), None)).unwrap();
        let buf = terminal.backend().buffer();
        let row_of = |needle: &str| {
            (0..buf.area.height)
                .find(|y| {
                    let row: String =
                        (0..buf.area.width).map(|x| buf[(x, *y)].symbol().to_string()).collect();
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
        terminal.draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now(), None)).unwrap();
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
        terminal.draw(|f| t.draw(f, f.area(), &theme, &anims, Instant::now(), None)).unwrap();
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
