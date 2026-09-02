use ratatui::style::Color;

pub mod builtin;
pub mod cache;
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
    // bevel pair (relative to `control`; paint layer derives per-surface variants)
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
        let on_accent = if oklab_l(accent) < 0.6 {
            (0xff, 0xff, 0xff)
        } else {
            (0x11, 0x11, 0x11)
        };
        let focus_ring = accent;
        // 35% accent over the background: visibly "the accent's" selection
        // without fighting the text drawn on top of it.
        let selection = blend(bg, accent, 0.35);

        let mut text = fg;
        // Contrast clamp: push text away from bg until |ΔL| >= 0.4. The
        // target overshoots to 0.405 because the Oklab→sRGB→u8 round-trip
        // can quantize the result ~0.001 short — aiming at exactly 0.4
        // would leave clamped text just under the guarantee.
        let page_l = oklab_l(page);
        if (oklab_l(text) - page_l).abs() < 0.4 {
            let direction = if oklab_l(text) >= page_l { 1.0 } else { -1.0 };
            let mut target_l = page_l + direction * 0.405;
            // A very dark (or very light) page leaves no room on the
            // text's natural side — a darker-than-bg text on a near-black
            // page would clamp at black and never reach the floor. Flip to
            // the side that has the room (0.405 < 0.5, so one always does).
            if !(0.0..=1.0).contains(&target_l) {
                target_l = page_l - direction * 0.405;
            }
            text = lift(text, target_l.clamp(0.0, 1.0) - oklab_l(text));
        }

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
    if oklab_l(bg) < 0.5 {
        Seeds::dark().fg
    } else {
        Seeds::light().fg
    }
}

/// Whether `c` reads as a light color (Oklab lightness >= 0.5) — used to
/// pick a legible text color for content painted directly on `c` (e.g. a
/// key-pill's fill), rather than assuming the fill is always dark the way
/// most of this dark-first theme's surfaces are.
/// Returns `fg` with its Oklab lightness pushed away from `bg` until
/// |ΔL| >= `min_delta`, keeping hue/chroma. The push continues in `fg`'s
/// natural direction (lighter stays lighter) unless that side lacks the
/// room, in which case it flips — mirroring `Theme::generate`'s text
/// clamp. Non-RGB colors pass through unchanged. Used by chip painting,
/// where a dim pill color (`text_muted` on a soft palette) can land
/// within a whisper of its own tinted fill.
pub(crate) fn ensure_min_contrast(fg: Color, bg: Color, min_delta: f32) -> Color {
    let (Color::Rgb(fr, fgc, fb), Color::Rgb(br, bgc, bb)) = (fg, bg) else {
        return fg;
    };
    let fg_l = oklab_l((fr, fgc, fb));
    let bg_l = oklab_l((br, bgc, bb));
    if (fg_l - bg_l).abs() >= min_delta {
        return fg;
    }
    let direction = if fg_l >= bg_l { 1.0 } else { -1.0 };
    let mut target = bg_l + direction * min_delta;
    if !(0.0..=1.0).contains(&target) {
        target = bg_l - direction * min_delta;
    }
    let (r, g, b) = lift((fr, fgc, fb), target.clamp(0.0, 1.0) - fg_l);
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
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
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
fn oklab_to_rgb((l, a, b): (f32, f32, f32)) -> (u8, u8, u8) {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l_ = l_.powi(3);
    let m_ = m_.powi(3);
    let s_ = s_.powi(3);

    let r = 4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_;
    let g = -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_;
    let b = -0.0041960863 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_;

    (linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b))
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
    if dist(cube_rgb) <= dist((gray_val, gray_val, gray_val)) {
        cube_idx
    } else {
        gray_idx
    }
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
    let (r, g, b) = oklab_to_rgb((
        al + (bl - al) * t,
        aa + (ba - aa) * t,
        ab_ + (bb_ - ab_) * t,
    ));
    Color::Rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let soft = Seeds {
            bg: (0x22, 0x28, 0x2c),
            fg: (0x8a, 0x92, 0x96),
            ..Seeds::dark()
        };
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
            assert!(
                disabled <= muted * 0.5,
                "disabled {disabled} vs muted {muted}"
            );
        }
    }

    #[test]
    fn dark_theme_tokens_are_rgb() {
        let t = Theme::dark();
        for c in [t.page, t.text, t.accent, t.edge_light, t.focus_ring] {
            assert!(
                matches!(c, Color::Rgb(..)),
                "token must be truecolor: {c:?}"
            );
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
        assert!(
            Theme::dark().downgrade_to_256().is_dark(),
            "downgrade keeps the variant"
        );
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
            assert!(
                matches!(c, Color::Indexed(_)),
                "expected indexed, got {c:?}"
            );
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
        let s = Seeds {
            fg: (30, 30, 34),
            ..Seeds::dark()
        };
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
        let s = seeds_from_queried(&QueriedColors {
            bg: Some((16, 16, 20)),
            fg: None,
            ansi,
        });
        assert_eq!(s.bg, (16, 16, 20));
        assert_eq!(s.accent, (1, 120, 212));
        assert_eq!(s.fg, Seeds::dark().fg, "fg derived from the dark bg");
        assert_eq!(
            s.success,
            Seeds::dark().success,
            "missing slot: builtin seed"
        );
    }

    #[test]
    fn seeds_from_queried_silent_terminal_is_dark_seeds() {
        let s = seeds_from_queried(&QueriedColors::default());
        assert_eq!(s.bg, Seeds::dark().bg);
        assert_eq!(s.accent, Seeds::dark().accent);
    }
}
