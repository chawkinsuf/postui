use ratatui::style::Color;

pub mod builtin;
pub mod osc;
pub mod registry;

pub use osc::{OscQuery, QueriedColors, TerminalPalette};
pub use registry::{ThemeEntry, ThemeRegistry, ThemeSource};

/// The small set of hand-picked colors a palette is generated from. Everything
/// else in [`Theme`] is derived from these six seeds by [`Theme::generate`].
#[derive(Debug, Clone, Copy)]
pub struct Seeds {
    pub bg: (u8, u8, u8),
    pub fg: (u8, u8, u8),
    pub accent: (u8, u8, u8),
    pub success: (u8, u8, u8),
    pub warning: (u8, u8, u8),
    pub error: (u8, u8, u8),
}

impl Seeds {
    /// The default dark palette: a soft, brightened, blue-shifted
    /// Solarized — deep blue-teal ground, warm bright foreground, and
    /// desaturated pastel accents (much gentler than canonical Solarized's
    /// saturated ones).
    pub fn dark() -> Self {
        Self {
            // The user's ghostty theme verbatim: background/foreground
            // from its config keys, accents from palette slots 4/2/3/1. A
            // warmer and a neutral foreground were both auditioned against
            // this cool one and rejected — the cool-on-cool softness is
            // the point.
            bg: (0x0a, 0x20, 0x28),
            fg: (0xcc, 0xd8, 0xe0),
            accent: (0x78, 0xa8, 0xc8),
            success: (0x90, 0xac, 0x60),
            warning: (0xc8, 0xa8, 0x68),
            error: (0xcc, 0x7e, 0x78),
        }
    }

    /// The default light palette, derived from [`Seeds::dark`] in Oklab:
    /// the dark variant's warm paper white as ground, its ground as (a
    /// slightly lifted) foreground, and the same accent hues darkened to
    /// read on a light surface.
    pub fn light() -> Self {
        Self {
            bg: (0xee, 0xe8, 0xe4),
            fg: (0x1b, 0x31, 0x3a),
            accent: (0x40, 0x6f, 0x8c),
            success: (0x63, 0x7c, 0x31),
            warning: (0x94, 0x75, 0x35),
            error: (0xa3, 0x59, 0x54),
        }
    }

    /// These seeds at contrast `contrast` (spec 2026-10-16): each step up
    /// widens the drawn page–text gap by one fixed amount and saturates
    /// the four colours; each step down narrows it by the same amount, as
    /// far as the readability floor, and desaturates. `bg` takes its share
    /// by moving away from the text (for the usual pair, deeper: toward
    /// black on a dark theme, white on a light one) or back toward it, and
    /// never crosses to the other polarity; `fg` moves the rest. `keep_bg`
    /// leaves `bg` as it is (the Terminal theme, whose background must
    /// match the terminal's own). Level 0 returns the seeds untouched.
    pub fn with_contrast(self, contrast: Contrast, keep_bg: bool) -> Self {
        let n = contrast.level();
        if n == 0 {
            return self;
        }
        // Every step moves lightness by the same fixed amount, out toward
        // a colour's own end (black for a dark page, white for its text)
        // or back, clamped at the end. Fixed amounts make a step down
        // undo exactly what a step up did, and a colour already at its
        // end (white text, a black page) still moves the way that has
        // room. Easing by a share of some room instead either made one
        // step down outweigh all three up (the room toward the other
        // colour) or froze colours at their ends (the room to their own).
        let n = n as f32;
        let set_l = |c: (u8, u8, u8), l: f32| {
            let (_, a, b) = oklab(c);
            oklab_to_rgb_in_gamut((l.clamp(0.0, 1.0), a, b))
        };

        // A step changes the page–text gap by one amount on every theme
        // (you, 2026-10-16), measured from the text `generate` draws: a
        // seed under the readability floor is drawn at the floor, so that
        // is where a level starts (review round 6). Softening stops at the
        // floor: going past it, the floor would drag the text along
        // behind the page and draw it brighter, not softer.
        let fg0 = readable_text(self.fg, self.bg);
        let (fg_l, bg_l) = (oklab_l(fg0), oklab_l(self.bg));
        let side = if fg_l >= bg_l { 1.0 } else { -1.0 };
        let change = if n > 0.0 {
            CONTRAST_GAP_STEP * n
        } else {
            -(CONTRAST_GAP_STEP * -n).min(((fg_l - bg_l).abs() - TEXT_FLOOR).max(0.0))
        };

        // The page takes a fixed share of that change (its 0.025 of each
        // 0.055), moving away from the text to widen and toward it to
        // narrow. A share of the change itself, not of the level, so when
        // softening stops at the floor the page stops with it (review
        // round 9: a share of the level went on lifting the page, and the
        // text drifted back to hold the gap). It never crosses the middle,
        // so the theme keeps its polarity; and a page authored close to
        // the middle is held, never pushed the other way.
        let new_bg_l = if keep_bg {
            bg_l
        } else {
            let toward = if n > 0.0 { -side } else { side };
            let l = bg_l + toward * change.abs() * (CONTRAST_BG_STEP / CONTRAST_GAP_STEP);
            let l = if bg_l < 0.5 { l.min(bg_l.max(0.49)) } else { l.max(bg_l.min(0.51)) };
            l.clamp(0.0, 1.0)
        };
        // A page that doesn't move (kept, or held at the middle) stays its
        // exact seed, not an Oklab round trip of it.
        let bg = if new_bg_l == bg_l { self.bg } else { set_l(self.bg, new_bg_l) };

        // The text moves the rest: a near-white page has little room left,
        // so on a light theme the text does most of the work. Measured on
        // the page's lightness before it rounds to sRGB, so both moves are
        // fixed functions of the one change and neither ever turns back:
        // next to black, rounding holds a page at #000000 for two steps
        // and then jumps it to #010101 (L 0.067), and text that made up for
        // the rounded page went back on the jump (the sweep found it). The
        // floor keeps the pair well apart, so it never comes near crossing.
        let page_share = (bg_l - new_bg_l) * side;
        let fg = set_l(fg0, fg_l + side * (change - page_share));

        // A raise goes only as far as the gamut allows (Gruvbox's warning
        // yellow already sits on its edge): see `oklab_to_rgb_in_gamut`.
        let factor = 1.0 + CONTRAST_CHROMA_STEP * n;
        let saturate = |c: (u8, u8, u8)| {
            let (l, a, b) = oklab(c);
            oklab_to_rgb_in_gamut((l, a * factor, b * factor))
        };
        Self {
            bg,
            fg,
            accent: saturate(self.accent),
            success: saturate(self.success),
            warning: saturate(self.warning),
            error: saturate(self.error),
        }
    }
}

/// How far one contrast step moves the background's Oklab lightness.
const CONTRAST_BG_STEP: f32 = 0.025;

/// How far one contrast step widens (or narrows) the Oklab lightness gap
/// between the page and its text, on every theme: the page moves its
/// [`CONTRAST_BG_STEP`] of it where it has room, the text the rest.
const CONTRAST_GAP_STEP: f32 = 0.055;

/// How much one contrast step scales the four colours' Oklab chroma.
const CONTRAST_CHROMA_STEP: f32 = 0.1;

/// A theme's contrast level (spec 2026-10-16): `MIN..=MAX`, 0 being the
/// theme as authored. One global level applies to whichever theme is
/// active; [`Seeds::with_contrast`] is what a level does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Contrast(i8);

impl Contrast {
    pub const MIN: i8 = -3;
    pub const MAX: i8 = 3;

    /// The level `level`, or `None` outside `MIN..=MAX`.
    pub fn new(level: i8) -> Option<Self> {
        (Self::MIN..=Self::MAX).contains(&level).then_some(Self(level))
    }

    pub fn level(self) -> i8 {
        self.0
    }

    /// The level `delta` steps away, or `None` past either end.
    pub fn step(self, delta: i8) -> Option<Self> {
        Self::new(self.0.checked_add(delta)?)
    }
}

pub struct Theme {
    /// Whether this is a dark-background palette. Consumers that must pick
    /// between a dark and a light variant of an external asset (e.g. the
    /// bundled syntect themes used for JSON syntax highlighting) key off
    /// this rather than guessing from a color token.
    dark: bool,
    // surface ladder
    pub page: Color,
    pub panel: Color,
    pub control: Color,
    pub control_hover: Color,
    pub control_pressed: Color,
    // Bevel pair (relative to `control`; paint layer derives per-surface
    // variants). Only `edge_dark` has a consumer today, via `paint::rule` —
    // the flat control register has no lit edges. `edge_light` is kept as
    // its half of the pair, ready for whatever wants a raised edge next.
    pub edge_light: Color,
    pub edge_dark: Color,
    /// Alternate zebra-stripe fill for dense list rows: the ground surface
    /// lifted a small step, distinct from `panel`/`control`.
    pub zebra_alt: Color,
    /// A subtle divider line color, darker than `panel` (edge_dark family).
    pub hairline: Color,
    // accent family
    pub accent: Color,
    pub accent_edge_light: Color,
    pub accent_edge_dark: Color,
    pub on_accent: Color,
    pub focus_ring: Color,
    /// Text-selection background: a muted accent that keeps `text` legible
    /// on top of it, shared by the body editor and the response view.
    pub selection: Color,
    // text
    pub text: Color,
    pub text_muted: Color,
    pub text_disabled: Color,
    // semantics (kept from stage 4)
    pub success: Color,
    pub warning: Color,
    pub error: Color,
}

impl Theme {
    /// Builds a full token set from a small seed palette: a surface ladder
    /// (page/panel/control/hover/pressed) plus bevel, accent, and text
    /// families, all derived by lifting seed lightness in Oklab space.
    pub fn generate(seeds: &Seeds) -> Self {
        let bg = seeds.bg;
        let fg = seeds.fg;
        let dark = oklab_l(bg) < 0.5;
        let step = if dark { 1.0 } else { -1.0 };

        let page = bg;
        let panel = lift(bg, step * 0.03);
        let control = lift(bg, step * 0.06);
        let control_hover = lift(bg, step * 0.10);
        let control_pressed = lift(bg, step * -0.02);
        let edge_light = lift(control, 0.08);
        let edge_dark = lift(control, -0.08);
        // Zebra stripe: same unconditional-offset family as `edge_light`
        // (an absolute lightening relative to its base, regardless of theme
        // polarity), so it stays lighter than `panel` in both themes.
        let zebra_alt = lift(panel, 0.045);
        // Hairline divider: same unconditional-offset family as `edge_dark`
        // (an absolute darkening relative to its base, regardless of theme
        // polarity), just a subtler step than a full bevel edge.
        let hairline = lift(panel, -0.05);

        let accent = seeds.accent;
        let accent_edge_light = lift(accent, 0.12);
        let accent_edge_dark = lift(accent, -0.12);
        let on_accent = if oklab_l(accent) < 0.6 { (0xff, 0xff, 0xff) } else { (0x11, 0x11, 0x11) };
        let focus_ring = accent;
        // 35% accent over the background: visibly "the accent's" selection
        // without fighting the text drawn on top of it.
        let selection = blend(bg, accent, 0.35);

        let text = readable_text(fg, page);

        // Muted text carries real content (ghost-row labels, inactive tab
        // names, response status/placeholder copy), so it has to clear a
        // readable contrast on its own. Derived from the *clamped* text so
        // a soft seed pair doesn't drag it down twice: 0.38 lands the
        // built-in dark seeds at ~5:1 against the page where 0.55 left
        // them at 3.4:1 — visibly washed out on macOS, whose text
        // rasterizer renders light-on-dark glyphs thinner than Linux does.
        //
        // Seeds can also arrive from the terminal's own palette (OSC
        // query), so the ratio is only the starting point: a floor lifts
        // muted toward WCAG AA (4.5:1), but never closer to `text` than a
        // fixed gap (text's ratio / MUTED_TEXT_GAP), so on a palette whose
        // text itself sits near the floor (Solarized) the two tones stay
        // distinguishable rather than collapsing into one.
        let text_ratio = wcag_contrast(text, page);
        let muted_floor = MUTED_MIN_CONTRAST.min(text_ratio / MUTED_TEXT_GAP);
        let text_muted = ensure_wcag_contrast(blend(text, bg, 0.38), page, muted_floor);
        // `blend`'s `t` is the *bg* weight, so disabled must sit closer to
        // 1.0 than muted to actually read dimmer — 0.35 had it the wrong
        // way around, leaving disabled text brighter than muted.
        let text_disabled = blend(text, bg, 0.82);

        let to_color = |c: (u8, u8, u8)| Color::Rgb(c.0, c.1, c.2);

        Self {
            dark,
            page: to_color(page),
            panel: to_color(panel),
            control: to_color(control),
            control_hover: to_color(control_hover),
            control_pressed: to_color(control_pressed),
            edge_light: to_color(edge_light),
            edge_dark: to_color(edge_dark),
            zebra_alt: to_color(zebra_alt),
            hairline: to_color(hairline),
            accent: to_color(accent),
            accent_edge_light: to_color(accent_edge_light),
            accent_edge_dark: to_color(accent_edge_dark),
            on_accent: to_color(on_accent),
            focus_ring: to_color(focus_ring),
            selection: to_color(selection),
            text: to_color(text),
            text_muted: to_color(text_muted),
            text_disabled: to_color(text_disabled),
            success: to_color(seeds.success),
            warning: to_color(seeds.warning),
            error: to_color(seeds.error),
        }
    }

    pub fn dark() -> Self {
        Self::generate(&Seeds::dark())
    }

    pub fn light() -> Self {
        Self::generate(&Seeds::light())
    }

    pub fn for_terminal() -> Self {
        Self::dark()
    }

    /// Whether this palette is the dark variant.
    pub fn is_dark(&self) -> bool {
        self.dark
    }

    /// Maps an HTTP method to a theme token color for its badge, reusing
    /// existing palette tokens rather than inventing new ones.
    pub fn method_color(&self, method: postui_core::model::Method) -> Color {
        use postui_core::model::Method;
        match method {
            Method::Get => self.success,
            Method::Post => self.accent,
            Method::Put | Method::Patch => self.warning,
            Method::Delete => self.error,
            Method::Head | Method::Options => self.text_muted,
        }
    }

    /// Maps an HTTP status code to a semantic token: 2xx success, 3xx
    /// accent (redirects are informational, not alarming), else error.
    pub fn status_color(&self, status: u16) -> Color {
        match status {
            200..=299 => self.success,
            300..=399 => self.accent,
            _ => self.error,
        }
    }

    /// Blends `c` toward `surface` at 22% opacity, for chip/badge fills that
    /// need to sit on top of an arbitrary surface color without a hard edge.
    pub fn tint(&self, c: Color, surface: Color) -> Color {
        let a = rgb_of(c);
        let b = rgb_of(surface);
        let t = blend(a, b, 0.22);
        Color::Rgb(t.0, t.1, t.2)
    }

    pub fn downgrade_to_256(&self) -> Self {
        let f = |c: Color| match c {
            Color::Rgb(r, g, b) => Color::Indexed(rgb_to_indexed(r, g, b)),
            other => other,
        };
        Self {
            dark: self.dark,
            page: f(self.page),
            panel: f(self.panel),
            control: f(self.control),
            control_hover: f(self.control_hover),
            control_pressed: f(self.control_pressed),
            edge_light: f(self.edge_light),
            edge_dark: f(self.edge_dark),
            zebra_alt: f(self.zebra_alt),
            hairline: f(self.hairline),
            accent: f(self.accent),
            accent_edge_light: f(self.accent_edge_light),
            accent_edge_dark: f(self.accent_edge_dark),
            on_accent: f(self.on_accent),
            focus_ring: f(self.focus_ring),
            selection: f(self.selection),
            text: f(self.text),
            text_muted: f(self.text_muted),
            text_disabled: f(self.text_disabled),
            success: f(self.success),
            warning: f(self.warning),
            error: f(self.error),
        }
    }
}

/// How far text must sit from its page in Oklab lightness: what
/// [`readable_text`] pushes a closer pair out to. It aims past the 0.4
/// guarantee because the Oklab→sRGB→u8 round-trip can quantize the result
/// ~0.001 short, and aiming at exactly 0.4 would leave clamped text just
/// under it.
const TEXT_FLOOR: f32 = 0.405;

/// The text `Theme::generate` draws for seed text `fg` on `page`: `fg`
/// itself when it clears the readability floor (|ΔL| >= 0.4), else pushed
/// away from the page to [`TEXT_FLOOR`]. `Seeds::with_contrast` starts from
/// this too, so a level moves the text that is drawn, not a seed the floor
/// would override.
fn readable_text(fg: (u8, u8, u8), page: (u8, u8, u8)) -> (u8, u8, u8) {
    push_apart(fg, page, 0.4, TEXT_FLOOR)
}

/// `fg` itself when it sits at least `floor` from `page` in Oklab
/// lightness, else `fg` moved away from `page` to `aim` (>= `floor`),
/// keeping its hue (spec 2026-10-16, task #31). The drawn pair always
/// clears `floor`:
/// - a tint that doesn't fit sRGB at the new lightness is trimmed to the
///   most that does, not clipped per channel, which would cost lightness
///   and turn the hue;
/// - where rounding to u8 lands short (next to black one unit is up to
///   0.067), the target walks further out until it clears;
/// - a walk that reaches either end draws black or white, the only
///   colour there.
fn push_apart(fg: (u8, u8, u8), page: (u8, u8, u8), floor: f32, aim: f32) -> (u8, u8, u8) {
    let (text_l, a, b) = oklab(fg);
    let page_l = oklab_l(page);
    let clears = |c: (u8, u8, u8)| (oklab_l(c) - page_l).abs() >= floor;
    if clears(fg) {
        return fg;
    }
    let mut direction = if text_l >= page_l { 1.0 } else { -1.0 };
    // A very dark (or very light) page leaves no room on the text's
    // natural side — a darker-than-bg text on a near-black page would
    // clamp at black and never reach the floor. Flip to the side that has
    // the room (every aim is under 0.5, so one always does).
    if !(0.0..=1.0).contains(&(page_l + direction * aim)) {
        direction = -direction;
    }
    let mut target_l = page_l + direction * aim;
    loop {
        let end = target_l.clamp(0.0, 1.0);
        if end != target_l {
            return oklab_to_rgb((end, 0.0, 0.0));
        }
        let text = oklab_to_rgb_in_gamut((target_l, a, b));
        if clears(text) {
            return text;
        }
        target_l += direction * 0.005;
    }
}

/// Builds a seed palette from a terminal's OSC query answer: the queried
/// bg/fg with ANSI slots for the semantic colors, falling back per-slot to
/// the built-in dark seeds, and to `Seeds::dark()` wholesale when the
/// terminal reported no background at all.
pub fn seeds_from_queried(q: &QueriedColors) -> Seeds {
    match q.bg {
        Some(bg) => {
            let builtin = Seeds::dark();
            Seeds {
                bg,
                fg: q.fg.unwrap_or_else(|| derive_fg_from_bg(bg)),
                accent: q.ansi[4].or(q.ansi[12]).unwrap_or(builtin.accent),
                success: q.ansi[2].unwrap_or(builtin.success),
                warning: q.ansi[3].unwrap_or(builtin.warning),
                error: q.ansi[1].unwrap_or(builtin.error),
            }
        }
        None => Seeds::dark(),
    }
}

/// Picks a legible foreground for a queried background that the terminal
/// didn't also give us an `OSC 10` answer for: light text on a dark
/// background, dark text on a light one, using the same built-in seeds
/// `Theme::generate`'s contrast clamp would converge on anyway.
fn derive_fg_from_bg(bg: (u8, u8, u8)) -> (u8, u8, u8) {
    if oklab_l(bg) < 0.5 { Seeds::dark().fg } else { Seeds::light().fg }
}

/// Whether `c` reads as a light color (Oklab lightness >= 0.5) — used to
/// pick a legible text color for content painted directly on `c` (e.g. a
/// key-pill's fill), rather than assuming the fill is always dark the way
/// most of this dark-first theme's surfaces are.
/// Returns `fg` with its Oklab lightness pushed away from `bg` until
/// |ΔL| >= `min_delta`, keeping its hue: [`push_apart`], the same push as
/// `Theme::generate`'s text clamp. Non-RGB colors pass through unchanged.
/// Used by chip painting, where a dim pill color (`text_muted` on a soft
/// palette) can land within a whisper of its own tinted fill.
pub(crate) fn ensure_min_contrast(fg: Color, bg: Color, min_delta: f32) -> Color {
    let (Color::Rgb(fr, fgc, fb), Color::Rgb(br, bgc, bb)) = (fg, bg) else {
        return fg;
    };
    let (r, g, b) = push_apart((fr, fgc, fb), (br, bgc, bb), min_delta, min_delta);
    Color::Rgb(r, g, b)
}

/// WCAG AA minimum contrast ratio for body text, the floor `Theme::generate`
/// holds `text_muted` to against `page`.
pub(crate) const MUTED_MIN_CONTRAST: f32 = 4.5;

/// Minimum ratio between `text`'s and `text_muted`'s page contrast, so
/// muted always reads as a visibly quieter tone than text.
pub(crate) const MUTED_TEXT_GAP: f32 = 1.3;

/// WCAG 2.x relative luminance of an srgb color (0 = black, 1 = white).
fn relative_luminance((r, g, b): (u8, u8, u8)) -> f32 {
    0.2126 * srgb_to_linear(r) + 0.7152 * srgb_to_linear(g) + 0.0722 * srgb_to_linear(b)
}

/// WCAG 2.x contrast ratio between two srgb colors, 1.0 (identical) to 21.0
/// (black on white). Symmetric in its arguments.
pub(crate) fn wcag_contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Returns `fg` with its Oklab lightness pushed away from `bg` — keeping
/// hue/chroma, in `fg`'s natural direction — until the pair's WCAG contrast
/// reaches `min_ratio`, or that side runs out of room. Unlike
/// `ensure_min_contrast`'s fixed Oklab ΔL, this targets the perceptual
/// ratio directly, which is what makes a light-page palette (where the
/// same ΔL buys far less legibility) clear the bar too.
fn ensure_wcag_contrast(fg: (u8, u8, u8), bg: (u8, u8, u8), min_ratio: f32) -> (u8, u8, u8) {
    if wcag_contrast(fg, bg) >= min_ratio {
        return fg;
    }
    let fg_l = oklab_l(fg);
    let direction = if fg_l >= oklab_l(bg) { 1.0 } else { -1.0 };
    // Walk lightness outward in small steps: a 0.01 step is finer than
    // any u8 quantization, so the first step that clears the ratio
    // overshoots by at most one rung.
    let mut delta = 0.0f32;
    let mut best = fg;
    while (0.0..=1.0).contains(&(fg_l + direction * delta)) {
        delta += 0.01;
        best = lift(fg, direction * delta);
        if wcag_contrast(best, bg) >= min_ratio {
            return best;
        }
    }
    best
}

pub(crate) fn is_light(c: Color) -> bool {
    oklab_l(rgb_of(c)) >= 0.5
}

/// Extracts the `(r, g, b)` components from a truecolor [`Color`]. Non-RGB
/// variants (already-indexed colors) fall back to black, since callers only
/// ever feed this the truecolor tokens produced by [`Theme::generate`].
pub(crate) fn rgb_of(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Converts srgb (0..=255) to Oklab `(L, a, b)`. Standard Oklab: linearize,
/// project into the LMS cone space, cube-root, then mix into Lab axes.
fn oklab((r, g, b): (u8, u8, u8)) -> (f32, f32, f32) {
    let r = srgb_to_linear(r);
    let g = srgb_to_linear(g);
    let b = srgb_to_linear(b);

    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    (
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    )
}

/// Converts Oklab `(L, a, b)` back to srgb (0..=255).
fn oklab_to_rgb(lab: (f32, f32, f32)) -> (u8, u8, u8) {
    let (r, g, b) = oklab_to_linear(lab);
    (linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b))
}

/// [`oklab_to_rgb`] for a colour that may lie outside sRGB: keeps its
/// lightness and hue and trims its chroma (`a`, `b` scaled down together)
/// to the most that fits. Clamping each channel instead, as `oklab_to_rgb`
/// does, turns the hue and can even leave less chroma than a fitted colour
/// has. Lightness in 0..1 always fits at zero chroma, a grey.
fn oklab_to_rgb_in_gamut(lab: (f32, f32, f32)) -> (u8, u8, u8) {
    if in_gamut(lab) {
        return oklab_to_rgb(lab);
    }
    let (l, a, b) = lab;
    let scaled = |t: f32| (l, a * t, b * t);
    let (mut inside, mut outside) = (0.0, 1.0);
    for _ in 0..16 {
        let mid = (inside + outside) / 2.0;
        if in_gamut(scaled(mid)) { inside = mid } else { outside = mid }
    }
    oklab_to_rgb(scaled(inside))
}

/// Whether an Oklab color lies inside the sRGB gamut, i.e. converts back
/// without clamping a channel.
fn in_gamut(lab: (f32, f32, f32)) -> bool {
    // A hair of slack: an sRGB colour on the gamut's edge (a 0 or 255
    // channel) round-trips through Oklab a float error past it.
    let (r, g, b) = oklab_to_linear(lab);
    [r, g, b].iter().all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
}

/// Converts Oklab `(L, a, b)` to unclamped linear sRGB.
fn oklab_to_linear((l, a, b): (f32, f32, f32)) -> (f32, f32, f32) {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l_ = l_.powi(3);
    let m_ = m_.powi(3);
    let s_ = s_.powi(3);

    let r = 4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_;
    let g = -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_;
    let b = -0.0041960863 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_;

    (r, g, b)
}

/// Oklab lightness of an srgb color; used by the generator ladder math and
/// exercised directly by tests as a contrast-check helper.
pub(crate) fn oklab_l(rgb: (u8, u8, u8)) -> f32 {
    oklab(rgb).0
}

/// Lifts an srgb color's Oklab lightness by `delta_l`, keeping hue/chroma
/// fixed. Negative deltas darken, positive deltas lighten.
fn lift(rgb: (u8, u8, u8), delta_l: f32) -> (u8, u8, u8) {
    let (l, a, b) = oklab(rgb);
    oklab_to_rgb((l + delta_l, a, b))
}

/// Linearly interpolates between two srgb colors at `t` (0 = a, 1 = b) in
/// srgb space, which is sufficient for the muted/disabled text tints.
fn blend(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let mix = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Nearest xterm-256 color: compares the best 6x6x6 cube match against the
/// best grayscale-ramp match and returns the closer of the two.
pub fn rgb_to_indexed(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_step = |v: u8| -> (u8, u8) {
        let mut best = (0u8, u8::MAX);
        for (i, s) in STEPS.iter().enumerate() {
            let d = v.abs_diff(*s);
            if d < best.1 {
                best = (i as u8, d);
            }
        }
        best
    };
    let (ri, _) = nearest_step(r);
    let (gi, _) = nearest_step(g);
    let (bi, _) = nearest_step(b);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (STEPS[ri as usize], STEPS[gi as usize], STEPS[bi as usize]);

    let gray_level = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    let gi2 = (gray_level.saturating_sub(8) / 10).min(23);
    let gray_idx = 232 + gi2;
    let gray_val = 8 + 10 * gi2;

    let dist = |(ar, ag, ab): (u8, u8, u8)| -> u32 {
        let dr = ar.abs_diff(r) as u32;
        let dg = ag.abs_diff(g) as u32;
        let db = ab.abs_diff(b) as u32;
        dr * dr + dg * dg + db * db
    };
    if dist(cube_rgb) <= dist((gray_val, gray_val, gray_val)) { cube_idx } else { gray_idx }
}

/// Inverse of [`rgb_to_indexed`]'s cube/gray math: maps an xterm-256 index
/// back to its nominal srgb value. Indices 16..=231 are the 6x6x6 color
/// cube; 232..=255 are the grayscale ramp; 0..=15 are the basic ANSI
/// colors, approximated with their conventional terminal values since
/// `rgb_to_indexed` never emits them itself.
pub fn indexed_to_rgb(idx: u8) -> (u8, u8, u8) {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    const ANSI16: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    if idx >= 232 {
        let level = idx - 232;
        let v = 8 + 10 * level;
        (v, v, v)
    } else if idx >= 16 {
        let i = idx - 16;
        let ri = i / 36;
        let gi = (i % 36) / 6;
        let bi = i % 6;
        (STEPS[ri as usize], STEPS[gi as usize], STEPS[bi as usize])
    } else {
        ANSI16[idx as usize]
    }
}

/// Blends a color 55% of the way toward black, for dimmed backdrops and
/// panel shadows. `Rgb` blends directly; `Indexed` round-trips through the
/// nominal xterm-256 rgb; other variants (e.g. `Reset`, named ANSI colors)
/// pass through unchanged since they carry no rgb value to blend.
pub fn dim55(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let (r, g, b) = blend((r, g, b), (0, 0, 0), 0.55);
            Color::Rgb(r, g, b)
        }
        Color::Indexed(i) => {
            let rgb = indexed_to_rgb(i);
            let (r, g, b) = blend(rgb, (0, 0, 0), 0.55);
            Color::Indexed(rgb_to_indexed(r, g, b))
        }
        other => other,
    }
}

/// Lifts a color's Oklab lightness by `delta_l`, keeping hue/chroma fixed,
/// same as the private `lift` helper the generator ladder uses but exposed
/// for callers outside this module — the paint layer's `face_edges` (which
/// derives a colored control's bevel edges from its face color), the
/// editor's focused-URL fill, and integration tests asserting the latter.
/// `Rgb` lifts directly; `Indexed` round-trips through the nominal
/// xterm-256 rgb; other variants pass through unchanged since they carry no
/// rgb value to lift.
pub fn lift_color(c: Color, delta_l: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let (r, g, b) = lift((r, g, b), delta_l);
            Color::Rgb(r, g, b)
        }
        Color::Indexed(i) => {
            let rgb = indexed_to_rgb(i);
            let (r, g, b) = lift(rgb, delta_l);
            Color::Indexed(rgb_to_indexed(r, g, b))
        }
        other => other,
    }
}

/// Blends `a` toward `b` in Oklab space by `t`, clamped to `[0, 1]`.
/// `t <= 0.0` returns `a` and `t >= 1.0` returns `b` exactly (an Oklab
/// roundtrip can drift a channel by ±1, so the endpoints are short-circuited
/// rather than computed). Only `Rgb` carries the truecolor components this
/// needs; if either `a` or `b` has no RGB form (`Indexed`, a named ANSI
/// color, `Reset`, ...), this degrades to returning `b`, same posture as
/// [`Theme::tint`].
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return b;
    };
    let (al, aa, ab_) = oklab((ar, ag, ab));
    let (bl, ba, bb_) = oklab((br, bg, bb));
    let (r, g, b) = oklab_to_rgb((al + (bl - al) * t, aa + (ba - aa) * t, ab_ + (bb_ - ab_) * t));
    Color::Rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every built-in's seeds, for the contrast tests below.
    fn builtin_seeds() -> Vec<(&'static str, Seeds)> {
        builtin::builtin_themes().into_iter().map(|b| (b.name, b.seeds)).collect()
    }

    fn chroma(c: (u8, u8, u8)) -> f32 {
        hue_and_chroma(c).1
    }

    /// A colour's Oklab hue angle and chroma, for the hue-keeping checks.
    fn hue_and_chroma(c: (u8, u8, u8)) -> (f32, f32) {
        let (_, a, b) = oklab(c);
        (b.atan2(a), (a * a + b * b).sqrt())
    }

    fn level(n: i8) -> Contrast {
        Contrast::new(n).expect("in range")
    }

    /// Spec 2026-10-16 C7: level 0 is the theme as authored, bit for bit.
    #[test]
    fn contrast_zero_leaves_every_builtin_untouched() {
        for (name, s) in builtin_seeds() {
            for keep_bg in [false, true] {
                let t = s.with_contrast(Contrast::default(), keep_bg);
                assert_eq!(
                    (t.bg, t.fg, t.accent, t.success, t.warning, t.error),
                    (s.bg, s.fg, s.accent, s.success, s.warning, s.error),
                    "{name}"
                );
            }
        }
    }

    /// Spec C1: each step up deepens the page and saturates the colours;
    /// each step down does the reverse. (The text gap's own steps are
    /// `every_step_moves_the_drawn_gap_equally_on_every_theme`'s.)
    #[test]
    fn contrast_steps_deepen_the_page_and_saturate() {
        for (name, s) in builtin_seeds() {
            let dark = oklab_l(s.bg) < 0.5;
            let at = |n: i8| s.with_contrast(level(n), false);
            for n in Contrast::MIN..Contrast::MAX {
                let (lo, hi) = (at(n), at(n + 1));
                let depth = |x: &Seeds| if dark { -oklab_l(x.bg) } else { oklab_l(x.bg) };
                // Strictly deeper while a whole step of room is left; a
                // page that reaches its end (Gruvbox Light's white) stays,
                // and so does one softening has stopped at the readability
                // floor (Solarized Dark below 0).
                let room = if dark { oklab_l(lo.bg) } else { 1.0 - oklab_l(lo.bg) };
                let softening_stopped = n < 0 && drawn_gap(&lo) <= TEXT_FLOOR + 0.01;
                if room > CONTRAST_BG_STEP && !softening_stopped {
                    assert!(depth(&hi) > depth(&lo), "{name} {n}: page did not deepen");
                } else {
                    assert!(depth(&hi) >= depth(&lo) - 0.002, "{name} {n}: page went back");
                }
                for (a, b) in [
                    (lo.accent, hi.accent),
                    (lo.success, hi.success),
                    (lo.warning, hi.warning),
                    (lo.error, hi.error),
                ] {
                    assert!(chroma(b) >= chroma(a), "{name} {n}: chroma fell {a:?} -> {b:?}");
                }
                // A colour already on the gamut's edge stays put (all four
                // of Gruvbox Light's are); the pastel defaults, the
                // washed-out case, have room and must saturate.
                if matches!(name, "dark" | "light") {
                    assert!(chroma(hi.accent) > chroma(lo.accent), "{name} {n}: accent flat");
                }
            }
        }
    }

    /// Spec C2: the Terminal theme keeps its background; text still moves.
    #[test]
    fn contrast_with_keep_bg_moves_text_but_not_the_page() {
        let s = Seeds::dark();
        for n in [Contrast::MIN, Contrast::MAX] {
            let t = s.with_contrast(level(n), true);
            assert_eq!(t.bg, s.bg);
            assert_ne!(t.fg, s.fg);
        }
    }

    /// The page–text gap as drawn: after `generate`'s readability clamp.
    fn drawn_gap(s: &Seeds) -> f32 {
        let t = Theme::generate(s);
        (oklab_l(rgb_of(t.text)) - oklab_l(rgb_of(t.page))).abs()
    }

    /// Spec C1 as amended (you, 2026-10-16), measured on the drawn theme
    /// (review round 6): every step up widens the gap by the same amount
    /// on every built-in, light or dark, the text making up what a
    /// near-white page has no room for; every step down narrows it by the
    /// same amount until the readability floor, where softening stops.
    #[test]
    fn every_step_moves_the_drawn_gap_equally_on_every_theme() {
        for (name, s) in builtin_seeds() {
            for n in Contrast::MIN..Contrast::MAX {
                let lo = drawn_gap(&s.with_contrast(level(n), false));
                let hi = drawn_gap(&s.with_contrast(level(n + 1), false));
                let step = hi - lo;
                if n >= 0 || lo > TEXT_FLOOR + 0.01 {
                    assert!(
                        (step - CONTRAST_GAP_STEP).abs() < 0.012,
                        "{name} {n}->{}: the gap moved {step:.3}",
                        n + 1
                    );
                } else {
                    assert!(step >= -0.005, "{name} {n}->{}: softer was wider", n + 1);
                }
            }
        }
        // A theme that keeps its page (Terminal) widens by its text alone.
        let s = Seeds::dark();
        let step = drawn_gap(&s.with_contrast(level(1), true)) - drawn_gap(&s);
        assert!((step - CONTRAST_GAP_STEP).abs() < 0.012, "{step:.3}");
    }

    /// Every invariant of a contrast level, swept over a grid of synthetic
    /// themes rather than the built-ins alone: pages and texts from black
    /// to white, grey and tinted at six hues, near the floor, near the
    /// middle and at the ends, with and without a kept page (Terminal).
    /// Nine review rounds each found one more corner of `with_contrast`
    /// by example (round 9: the page kept moving after softening stopped,
    /// so the text drifted back); this checks them all at once, on the
    /// drawn theme, at every level:
    /// - level 0 draws exactly what `generate` draws from the seeds;
    /// - the drawn gap never narrows on a step up;
    /// - the page only ever moves away from the text on a step up, and
    ///   the text only away from the page, so neither drifts back;
    /// - the text stays on its side of the page, and the theme keeps its
    ///   polarity;
    /// - a tinted page or text keeps its hue.
    #[test]
    fn contrast_invariants_hold_across_a_sweep_of_themes() {
        // Rounding to u8 moves Oklab L by up to ~0.004 mid-range, but right
        // next to black sRGB's first step is already L 0.067 (#010101), so
        // a check finer than that there measures one u8 unit, not the rule.
        let tol = |ls: &[f32]| if ls.iter().any(|&l| l < 0.1) { 0.07 } else { 0.008 };
        let colour = |l: f32, chroma: f32, hue: f32| {
            oklab_to_rgb_in_gamut((l, chroma * hue.cos(), chroma * hue.sin()))
        };
        let pages =
            [0.0, 0.05, 0.15, 0.25, 0.35, 0.45, 0.49, 0.51, 0.55, 0.65, 0.75, 0.85, 0.95, 1.0];
        let texts = [0.0, 0.1, 0.3, 0.45, 0.55, 0.7, 0.9, 1.0];
        let mut checked = 0;
        for &page_l in &pages {
            for &text_l in &texts {
                for chroma in [0.0, 0.04, 0.08] {
                    for h in 0..6 {
                        let hue = h as f32 * std::f32::consts::TAU / 6.0;
                        let s = Seeds {
                            bg: colour(page_l, chroma, hue),
                            fg: colour(text_l, chroma, hue + 2.0),
                            ..Seeds::dark()
                        };
                        for keep_bg in [false, true] {
                            let at = |n: i8| Theme::generate(&s.with_contrast(level(n), keep_bg));
                            let drawn: Vec<Theme> =
                                (Contrast::MIN..=Contrast::MAX).map(at).collect();
                            let base = &drawn[(-Contrast::MIN) as usize];
                            let plain = Theme::generate(&s);
                            assert_eq!((base.page, base.text), (plain.page, plain.text));
                            let l = |c: Color| oklab_l(rgb_of(c));
                            let side = if l(base.text) >= l(base.page) { 1.0 } else { -1.0 };
                            let tag = format!(
                                "page {page_l} text {text_l} chroma {chroma} hue {h} keep {keep_bg}"
                            );
                            for (i, pair) in drawn.windows(2).enumerate() {
                                let (lo, hi) = (&pair[0], &pair[1]);
                                let n = Contrast::MIN + i as i8;
                                let gap = |t: &Theme| (l(t.text) - l(t.page)).abs();
                                let tol = tol(&[l(lo.page), l(lo.text), l(hi.page), l(hi.text)]);
                                assert!(
                                    gap(hi) >= gap(lo) - tol,
                                    "{tag} {n}: gap narrowed on a step up"
                                );
                                assert!(
                                    (l(hi.page) - l(lo.page)) * side <= tol,
                                    "{tag} {n}: page moved toward the text on a step up"
                                );
                                assert!(
                                    (l(hi.text) - l(lo.text)) * side >= -tol,
                                    "{tag} {n}: text drifted back toward the page on a step up"
                                );
                            }
                            for (i, t) in drawn.iter().enumerate() {
                                let n = Contrast::MIN + i as i8;
                                assert_eq!(
                                    (l(t.text) - l(t.page)).signum(),
                                    side,
                                    "{tag} {n}: text crossed"
                                );
                                assert_eq!(
                                    t.is_dark(),
                                    base.is_dark(),
                                    "{tag} {n}: polarity flipped"
                                );
                                for (what, before, after) in
                                    [("page", base.page, t.page), ("text", base.text, t.text)]
                                {
                                    let ((h0, c0), (h1, c1)) = (
                                        hue_and_chroma(rgb_of(before)),
                                        hue_and_chroma(rgb_of(after)),
                                    );
                                    // Only where sRGB can hold a hue: next to black or
                                    // white a single u8 unit of one channel sets it
                                    // (#000002 -> #000105 "turns"), which is noise.
                                    let holds = |c: Color| (0.15..0.95).contains(&l(c));
                                    if c0 > 0.02 && c1 > 0.02 && holds(before) && holds(after) {
                                        assert!(
                                            (h1 - h0).sin().abs() < 0.1,
                                            "{tag} {n}: {what} hue turned {before:?} -> {after:?}"
                                        );
                                    }
                                }
                            }
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 2016 * 2, "the sweep shrank");
    }

    /// Review round 9: Catppuccin Latte has 0.117 of room above the floor,
    /// less than three steps' worth. Its text used to darken again from -2
    /// to -3, as the page went on lifting after the gap had stopped.
    #[test]
    fn catppuccin_latte_s_last_softening_steps_do_not_drift_back() {
        let (_, s) =
            builtin_seeds().into_iter().find(|(n, _)| *n == "catppuccin-latte").expect("built-in");
        let text = |n: i8| oklab_l(rgb_of(Theme::generate(&s.with_contrast(level(n), false)).text));
        // Light theme: softer text is lighter, so each step down never darkens it.
        for n in (Contrast::MIN + 1)..=0 {
            assert!(text(n - 1) >= text(n) - 0.003, "{}..{n}: text darkened", n - 1);
        }
    }

    /// Review round 6: on a theme whose text already sits on the
    /// readability floor (Solarized Dark), softening moves nothing. It
    /// used to lift the page, and the floor dragged the text up with it:
    /// "softer" drew brighter text.
    #[test]
    fn softening_a_theme_on_the_floor_changes_neither_page_nor_text() {
        let (_, s) =
            builtin_seeds().into_iter().find(|(n, _)| *n == "solarized-dark").expect("built-in");
        let base = Theme::generate(&s);
        for n in Contrast::MIN..0 {
            let t = Theme::generate(&s.with_contrast(level(n), false));
            assert_eq!((t.page, t.text), (base.page, base.text), "{n}");
        }
    }

    /// Review round 4: a step changes lightness, not hue. Moving a tinted
    /// colour toward its end (Gruvbox Dark's cream text toward white) while
    /// keeping its whole tint leaves sRGB, and clipping each channel there
    /// turns the hue; the tint is trimmed to fit instead.
    #[test]
    fn contrast_keeps_the_page_s_and_text_s_hue() {
        for (name, s) in builtin_seeds() {
            for n in [Contrast::MIN, Contrast::MAX] {
                let t = s.with_contrast(level(n), false);
                for (what, before, after) in [("page", s.bg, t.bg), ("text", s.fg, t.fg)] {
                    let ((h0, c0), (h1, c1)) = (hue_and_chroma(before), hue_and_chroma(after));
                    // Hue is noise on a near-grey; only a visible tint has one.
                    if c0 > 0.02 && c1 > 0.02 {
                        let turn = (h1 - h0).sin().abs();
                        assert!(turn < 0.1, "{name} {n} {what}: hue turned {turn:.2}");
                    }
                }
            }
        }
    }

    /// Review round 3: softening a pair that sits close together never
    /// takes the text past the page. Crossed, `generate`'s clamp would
    /// flip it to the page's other side: dark text on a dark page.
    #[test]
    fn softened_text_never_crosses_the_page() {
        let close = Seeds { bg: (0x55, 0x55, 0x55), fg: (0x72, 0x72, 0x72), ..Seeds::dark() };
        assert!(oklab_l(close.bg) < 0.5 && oklab_l(close.fg) > oklab_l(close.bg), "fixture");
        for n in Contrast::MIN..=0 {
            let t = Theme::generate(&close.with_contrast(level(n), false));
            assert!(oklab_l(rgb_of(t.text)) > oklab_l(rgb_of(t.page)), "{n}: text flipped");
        }
    }

    /// Review round 5: the guard holds for tinted pairs too, whose rounding
    /// back to sRGB can land the text a hair past an exact meeting point.
    /// Swept over hues and near-middle pairs on both polarities.
    #[test]
    fn softened_tinted_text_never_crosses_the_page() {
        let tinted =
            |l: f32, hue: f32| oklab_to_rgb_in_gamut((l, 0.06 * hue.cos(), 0.06 * hue.sin()));
        for step in 0..24 {
            let hue = step as f32 * std::f32::consts::TAU / 24.0;
            for (bg_l, fg_l) in [(0.45, 0.55), (0.47, 0.53), (0.55, 0.45), (0.53, 0.47)] {
                let s =
                    Seeds { bg: tinted(bg_l, hue), fg: tinted(fg_l, hue + 1.0), ..Seeds::dark() };
                let light_text = oklab_l(s.fg) > oklab_l(s.bg);
                for n in Contrast::MIN..=0 {
                    let t = Theme::generate(&s.with_contrast(level(n), false));
                    let above = oklab_l(rgb_of(t.text)) > oklab_l(rgb_of(t.page));
                    assert_eq!(
                        above, light_text,
                        "hue {step} bg {bg_l} fg {fg_l} at {n}: text flipped"
                    );
                }
            }
        }
    }

    /// Review round 1: a colour already at its end (white text, a black
    /// page) must still move the way that has room. White text is the
    /// usual Terminal answer, and stark themes are the ones most worth
    /// softening.
    #[test]
    fn contrast_moves_colours_already_at_their_ends() {
        let stark = Seeds { bg: (0, 0, 0), fg: (255, 255, 255), ..Seeds::dark() };
        let soft = stark.with_contrast(level(-1), false);
        assert!(oklab_l(soft.fg) < oklab_l(stark.fg), "white text dims");
        // sRGB's first step above black (#010101) is already Oklab L
        // 0.067, so a black page shows its lift from the third step.
        let softest = stark.with_contrast(level(Contrast::MIN), false);
        assert!(oklab_l(softest.bg) > oklab_l(stark.bg), "a black page lifts");
        let terminal = stark.with_contrast(level(-1), true);
        assert!(oklab_l(terminal.fg) < oklab_l(stark.fg), "Terminal's white text dims");
        let crisp = stark.with_contrast(level(3), false);
        assert_eq!((crisp.bg, crisp.fg), (stark.bg, stark.fg), "no room to go further");
    }

    /// Review round 1: the polarity guard never pushes a page the wrong
    /// way. A dark page just under the middle stays put when softened
    /// rather than being dragged darker.
    #[test]
    fn contrast_never_moves_a_page_against_its_step() {
        let near_mid = Seeds { bg: (0x61, 0x61, 0x61), ..Seeds::dark() };
        assert!((0.48..0.5).contains(&oklab_l(near_mid.bg)), "fixture");
        for n in [-1, -3] {
            let t = near_mid.with_contrast(level(n), false);
            assert_eq!(t.bg, near_mid.bg, "{n}: a held page moved");
        }
    }

    /// Spec C13 and C14: the lowest level still clears both text floors,
    /// and no level flips a theme's polarity.
    #[test]
    fn contrast_extremes_keep_text_readable_and_polarity_fixed() {
        for (name, s) in builtin_seeds() {
            for n in [Contrast::MIN, Contrast::MAX] {
                let t = Theme::generate(&s.with_contrast(level(n), false));
                assert_eq!(t.is_dark(), Theme::generate(&s).is_dark(), "{name} {n}");
                let (page, text, muted) = (rgb_of(t.page), rgb_of(t.text), rgb_of(t.text_muted));
                assert!((oklab_l(text) - oklab_l(page)).abs() >= 0.4, "{name} {n}: text");
                let floor = MUTED_MIN_CONTRAST.min(wcag_contrast(text, page) / MUTED_TEXT_GAP);
                assert!(wcag_contrast(muted, page) >= floor, "{name} {n}: muted");
            }
        }
    }

    #[test]
    fn contrast_range_and_steps_stop_at_the_ends() {
        assert!(Contrast::new(Contrast::MAX + 1).is_none());
        assert!(Contrast::new(Contrast::MIN - 1).is_none());
        assert_eq!(Contrast::default().level(), 0);
        assert_eq!(level(2).step(1).map(Contrast::level), Some(3));
        assert!(level(Contrast::MAX).step(1).is_none());
        assert!(level(Contrast::MIN).step(-1).is_none());
    }

    #[test]
    fn mix_endpoints_and_midpoint() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(200, 100, 50);
        assert_eq!(crate::theme::mix(a, b, 0.0), a);
        assert_eq!(crate::theme::mix(a, b, 1.0), b);
        let m = crate::theme::mix(a, b, 0.5);
        let Color::Rgb(r, ..) = m else { panic!() };
        assert!(r > 0 && r < 200);
    }

    /// A soft terminal palette (queried fg/bg closer together than any
    /// built-in) must still get its muted text lifted past what the blend
    /// alone would give — up to AA, or as far as the gap under `text`
    /// allows — without touching a pair that already clears the bar.
    #[test]
    fn wcag_floor_lifts_soft_seeds_and_leaves_strong_pairs_alone() {
        let soft = Seeds { bg: (0x22, 0x28, 0x2c), fg: (0x8a, 0x92, 0x96), ..Seeds::dark() };
        let t = Theme::generate(&soft);
        let (page, text, muted) = (rgb_of(t.page), rgb_of(t.text), rgb_of(t.text_muted));
        let floor = MUTED_MIN_CONTRAST.min(wcag_contrast(text, page) / MUTED_TEXT_GAP);
        let ratio = wcag_contrast(muted, page);
        assert!(ratio >= floor, "soft muted {ratio:.2} < floor {floor:.2}");
        let raw = blend(text, page, 0.38);
        assert!(wcag_contrast(raw, page) < floor, "fixture too strong");
        assert!(ratio > wcag_contrast(raw, page), "floor didn't lift");

        let strong = (0xdd, 0xdd, 0xdd);
        assert_eq!(ensure_wcag_contrast(strong, (0, 0, 0), 4.5), strong);
        assert!((wcag_contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 1e-3);
    }

    /// Disabled text must read clearly dimmer than muted text — at most
    /// half of muted's lightness distance from the page surface. At the
    /// old `blend(fg, bg, 0.35)` a disabled tab label was barely
    /// distinguishable from its enabled neighbours.
    #[test]
    fn disabled_text_is_at_most_half_of_muted_s_contrast() {
        for t in [Theme::dark(), Theme::light()] {
            let l = |c: Color| {
                let Color::Rgb(r, g, b) = c else { panic!() };
                oklab_l((r, g, b))
            };
            let page = l(t.page);
            let muted = (l(t.text_muted) - page).abs();
            let disabled = (l(t.text_disabled) - page).abs();
            assert!(disabled <= muted * 0.5, "disabled {disabled} vs muted {muted}");
        }
    }

    #[test]
    fn dark_theme_tokens_are_rgb() {
        let t = Theme::dark();
        for c in [t.page, t.text, t.accent, t.edge_light, t.focus_ring] {
            assert!(matches!(c, Color::Rgb(..)), "token must be truecolor: {c:?}");
        }
    }

    #[test]
    fn is_light_reports_lightness_directly() {
        assert!(is_light(Color::Rgb(255, 255, 255)), "white is light");
        assert!(!is_light(Color::Rgb(0, 0, 0)), "black is dark");
        // The dark theme's accent is itself a light blue — this is the
        // fixture behind the chip key-pill contrast fix.
        assert!(is_light(Theme::dark().accent));
    }

    #[test]
    fn variant_is_reported_and_survives_downgrade() {
        assert!(Theme::dark().is_dark());
        assert!(!Theme::light().is_dark());
        assert!(Theme::dark().downgrade_to_256().is_dark(), "downgrade keeps the variant");
    }

    #[test]
    fn focus_ring_differs_from_the_unfocused_edge() {
        let t = Theme::dark();
        assert_ne!(t.edge_light, t.focus_ring);
        let t = Theme::light();
        assert_ne!(t.edge_light, t.focus_ring);
    }

    #[test]
    fn downgrade_maps_every_token_to_indexed() {
        let t = Theme::dark().downgrade_to_256();
        for c in [
            t.page,
            t.panel,
            t.text,
            t.text_muted,
            t.accent,
            t.success,
            t.error,
            t.warning,
            t.edge_light,
            t.focus_ring,
        ] {
            assert!(matches!(c, Color::Indexed(_)), "expected indexed, got {c:?}");
        }
    }

    #[test]
    fn method_color_maps_each_method_to_a_distinct_semantic_token() {
        use postui_core::model::Method;
        let t = Theme::dark();
        assert_eq!(t.method_color(Method::Get), t.success);
        assert_eq!(t.method_color(Method::Post), t.accent);
        assert_eq!(t.method_color(Method::Put), t.warning);
        assert_eq!(t.method_color(Method::Patch), t.warning);
        assert_eq!(t.method_color(Method::Delete), t.error);
        assert_eq!(t.method_color(Method::Head), t.text_muted);
        assert_eq!(t.method_color(Method::Options), t.text_muted);
    }

    #[test]
    fn rgb_to_indexed_hits_cube_corners() {
        assert_eq!(rgb_to_indexed(0, 0, 0), 16); // cube black
        assert_eq!(rgb_to_indexed(255, 255, 255), 231); // cube white
        assert_eq!(rgb_to_indexed(255, 0, 0), 196); // cube red corner
    }

    #[test]
    fn generator_ladder_is_monotonic_dark() {
        let t = Theme::dark();
        let l = |c: Color| oklab_l(rgb_of(c));
        assert!(l(t.page) < l(t.panel));
        assert!(l(t.panel) < l(t.control));
        assert!(l(t.control) < l(t.control_hover));
        assert!(l(t.control_pressed) < l(t.control));
    }

    #[test]
    fn zebra_alt_is_lighter_than_page_and_panel() {
        let t = Theme::dark();
        let l = |c: Color| oklab_l(rgb_of(c));
        assert!(l(t.zebra_alt) > l(t.panel));
        assert!(l(t.zebra_alt) > l(t.page));
        // `hairline`-style cross-polarity coverage: `zebra_alt` stays
        // lighter than `panel` regardless of theme polarity (unconditional
        // offset, same family as `edge_light`). It is not asserted against
        // `page` here — `panel` itself sits below `page` for light seeds
        // (see `generator_ladder_inverts_for_light_seeds`), so "lighter
        // than page" is a dark-seeds-only consequence of the ladder, not
        // part of `zebra_alt`'s own contract.
        let t = Theme::light();
        assert!(l(t.zebra_alt) > l(t.panel));
    }

    #[test]
    fn hairline_is_darker_than_panel() {
        let t = Theme::dark();
        let l = |c: Color| oklab_l(rgb_of(c));
        assert!(l(t.hairline) < l(t.panel));
        let t = Theme::light();
        assert!(l(t.hairline) < l(t.panel));
    }

    #[test]
    fn generator_ladder_inverts_for_light_seeds() {
        let t = Theme::light();
        let l = |c: Color| oklab_l(rgb_of(c));
        assert!(l(t.page) > l(t.panel));
        assert!(l(t.panel) > l(t.control));
    }

    #[test]
    fn text_contrast_is_clamped() {
        // pathological seeds: fg nearly equal to bg
        let s = Seeds { fg: (30, 30, 34), ..Seeds::dark() };
        let t = Theme::generate(&s);
        // Strict: the clamp's target overshoots quantization, so the
        // guarantee holds exactly.
        assert!((oklab_l(rgb_of(t.text)) - oklab_l(rgb_of(t.page))).abs() >= 0.4);
    }

    #[test]
    fn status_color_classes() {
        let t = Theme::dark();
        assert_eq!(t.status_color(200), t.success);
        assert_eq!(t.status_color(301), t.accent);
        assert_eq!(t.status_color(404), t.error);
        assert_eq!(t.status_color(500), t.error);
    }

    #[test]
    fn indexed_to_rgb_inverts_rgb_to_indexed_at_cube_corners() {
        assert_eq!(indexed_to_rgb(16), (0, 0, 0));
        assert_eq!(indexed_to_rgb(231), (255, 255, 255));
        assert_eq!(indexed_to_rgb(196), (255, 0, 0));
    }

    #[test]
    fn indexed_to_rgb_inverts_rgb_to_indexed_on_the_gray_ramp() {
        // Explicit boundary/mid-range/top checks on the 232..=255 gray ramp
        // (232 = darkest step, 255 = lightest), in addition to the full-range
        // loop below.
        assert_eq!(rgb_to_indexed(8, 8, 8), 232);
        assert_eq!(indexed_to_rgb(232), (8, 8, 8));
        assert_eq!(rgb_to_indexed(128, 128, 128), 244);
        assert_eq!(indexed_to_rgb(244), (128, 128, 128));
        assert_eq!(rgb_to_indexed(238, 238, 238), 255);
        assert_eq!(indexed_to_rgb(255), (238, 238, 238));
    }

    #[test]
    fn rgb_to_indexed_round_trips_through_indexed_to_rgb_for_every_emitted_index() {
        // `rgb_to_indexed` only ever emits 16..=255 (cube + gray ramp; never
        // the basic 0..=15 ANSI slots), so that's the full contract
        // `indexed_to_rgb` must invert. Verified once by hand that this
        // holds for the entire range before committing to a single-loop
        // assertion rather than a handful of spot checks.
        for i in 16u16..=255 {
            let i = i as u8;
            let (r, g, b) = indexed_to_rgb(i);
            assert_eq!(
                rgb_to_indexed(r, g, b),
                i,
                "index {i} -> rgb {:?} -> index {}, expected round trip",
                (r, g, b),
                rgb_to_indexed(r, g, b)
            );
        }
    }

    #[test]
    fn dim55_blends_rgb_and_indexed_toward_black() {
        assert_eq!(dim55(Color::Rgb(200, 100, 50)), Color::Rgb(90, 45, 23));
        match dim55(Color::Indexed(196)) {
            Color::Indexed(i) => assert_ne!(i, 196),
            other => panic!("expected indexed, got {other:?}"),
        }
        assert_eq!(dim55(Color::Reset), Color::Reset);
    }

    #[test]
    fn downgrade_maps_every_new_token_to_indexed() {
        let t = Theme::dark().downgrade_to_256();
        for c in [
            t.page,
            t.panel,
            t.control,
            t.control_hover,
            t.control_pressed,
            t.edge_light,
            t.edge_dark,
            t.zebra_alt,
            t.hairline,
            t.accent_edge_light,
            t.accent_edge_dark,
            t.on_accent,
            t.focus_ring,
            t.text_disabled,
        ] {
            assert!(matches!(c, Color::Indexed(_)));
        }
    }

    #[test]
    fn seeds_from_queried_uses_answer_and_falls_back_per_slot() {
        let mut ansi = [None; 16];
        ansi[4] = Some((1, 120, 212));
        let s = seeds_from_queried(&QueriedColors { bg: Some((16, 16, 20)), fg: None, ansi });
        assert_eq!(s.bg, (16, 16, 20));
        assert_eq!(s.accent, (1, 120, 212));
        assert_eq!(s.fg, Seeds::dark().fg, "fg derived from the dark bg");
        assert_eq!(s.success, Seeds::dark().success, "missing slot: builtin seed");
    }

    #[test]
    fn seeds_from_queried_silent_terminal_is_dark_seeds() {
        let s = seeds_from_queried(&QueriedColors::default());
        assert_eq!(s.bg, Seeds::dark().bg);
        assert_eq!(s.accent, Seeds::dark().accent);
    }

    /// An Oklab colour at lightness `l`, tinted `chroma` at angle `hue`,
    /// fitted into sRGB.
    fn tinted(l: f32, chroma: f32, hue: f32) -> (u8, u8, u8) {
        oklab_to_rgb_in_gamut((l, chroma * hue.cos(), chroma * hue.sin()))
    }

    /// Seed pairs for the floor sweeps: pages in fine steps (next to
    /// black one u8 unit is a big lightness step, task #31), texts across
    /// the range, grey and tinted at six hues.
    fn floor_sweep() -> impl Iterator<Item = ((u8, u8, u8), (u8, u8, u8), String)> {
        (0..=200).flat_map(|p| {
            (0..=20).flat_map(move |t| {
                [0.0, 0.04, 0.08, 0.12].into_iter().flat_map(move |chroma| {
                    (0..6).map(move |h| {
                        let (page_l, text_l) = (p as f32 / 200.0, t as f32 / 20.0);
                        let hue = h as f32 * std::f32::consts::TAU / 6.0;
                        let tag = format!("page {page_l} text {text_l} chroma {chroma} hue {h}");
                        (tinted(page_l, chroma, hue), tinted(text_l, chroma, hue + 2.0), tag)
                    })
                })
            })
        })
    }

    /// Task #31 (spec 2026-10-16 F1–F3): the text `generate` draws sits
    /// at least 0.4 from the page for every seed, tinted or grey, near
    /// black or white; clamping it again changes nothing; and a pushed
    /// tint keeps its hue. `lift` used to clip each channel, which cost
    /// lightness and turned the hue, and rounding next to black overshot
    /// the 0.005 slack.
    #[test]
    fn readable_text_holds_the_floor_and_the_hue_for_every_seed() {
        for (page, fg, tag) in floor_sweep() {
            let text = readable_text(fg, page);
            let gap = (oklab_l(text) - oklab_l(page)).abs();
            assert!(gap >= 0.4, "{tag}: {fg:?} on {page:?} drew {text:?}, gap {gap}");
            assert_eq!(readable_text(text, page), text, "{tag}: not idempotent");
            let ((h0, c0), (h1, c1)) = (hue_and_chroma(fg), hue_and_chroma(text));
            // Same bounds as the contrast sweep: hue is noise on a
            // near-grey, and next to black or white one u8 unit sets it.
            if c0 > 0.02 && c1 > 0.02 && (0.15..0.95).contains(&oklab_l(text)) {
                assert!((h1 - h0).sin().abs() < 0.1, "{tag}: hue turned {fg:?} -> {text:?}");
            }
        }
    }

    /// #31's own example: a tinted page at L 0.45 with dark tinted text,
    /// pushed down next to black where its tint doesn't fit and one u8
    /// unit is a big lightness step.
    #[test]
    fn readable_text_holds_the_floor_for_31_s_example() {
        let page = tinted(0.45, 0.04, 1.0 * std::f32::consts::TAU / 6.0);
        let fg = tinted(0.1, 0.04, 1.0 * std::f32::consts::TAU / 6.0 + 2.0);
        let text = readable_text(fg, page);
        assert!((oklab_l(text) - oklab_l(page)).abs() >= 0.4, "{text:?}");
    }

    /// Spec 2026-10-16 F6: a push that walks all the way to the end draws
    /// black (or white), the only colour at that lightness, rather than a
    /// tinted u8 step that rounds back toward the page.
    #[test]
    fn readable_text_at_the_dark_end_is_black() {
        let page = tinted(0.415, 0.0, 0.0);
        for h in 0..6 {
            let fg = tinted(0.3, 0.08, h as f32);
            assert_eq!(readable_text(fg, page), (0, 0, 0), "hue {h}");
        }
    }

    /// Spec 2026-10-16 F4: chip text sits at least its `min_delta` from
    /// its fill, under the same push as the text floor.
    #[test]
    fn chip_contrast_holds_its_floor_for_every_seed() {
        let rgb = |(r, g, b): (u8, u8, u8)| Color::Rgb(r, g, b);
        for (fill, fg, tag) in floor_sweep() {
            let text = rgb_of(ensure_min_contrast(rgb(fg), rgb(fill), 0.35));
            let gap = (oklab_l(text) - oklab_l(fill)).abs();
            assert!(gap >= 0.35, "{tag}: {fg:?} on {fill:?} drew {text:?}, gap {gap}");
        }
    }
}
