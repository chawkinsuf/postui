use ratatui::style::Color;

pub mod osc;

pub use osc::{OscQuery, QueriedColors, TerminalPalette};

/// Which palette source to use, parsed from the `theme` config key.
/// `Terminal` (the default) queries the real terminal's colors via OSC and
/// generates a palette from them, falling back to [`Seeds::dark`] if the
/// terminal doesn't answer; `Dark`/`Light` always use the built-in seeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    Terminal,
    Dark,
    Light,
}

impl ThemeChoice {
    /// Parses the `theme` config value. Any value other than `"dark"` or
    /// `"light"` (including unrecognized strings) is treated as `Terminal`
    /// — the config-loading layer is responsible for warning about a
    /// genuinely unknown value, not this parser.
    pub fn parse(s: &str) -> Self {
        match s {
            "dark" => Self::Dark,
            "light" => Self::Light,
            "terminal" => Self::Terminal,
            _ => Self::Terminal,
        }
    }
}

/// The small set of hand-picked colors a palette is generated from. Everything
/// else in [`Theme`] is derived from these six seeds by [`Theme::generate`].
pub struct Seeds {
    pub bg: (u8, u8, u8),
    pub fg: (u8, u8, u8),
    pub accent: (u8, u8, u8),
    pub success: (u8, u8, u8),
    pub warning: (u8, u8, u8),
    pub error: (u8, u8, u8),
}

impl Seeds {
    /// Starting palette (Tokyo-Night-adjacent); visual direction iterates on
    /// these values during stage-1 polish with the frontend-design skill.
    pub fn dark() -> Self {
        Self {
            bg: (0x13, 0x17, 0x20),
            fg: (0xd8, 0xde, 0xe9),
            accent: (0x7a, 0xa2, 0xf7),
            success: (0x9e, 0xce, 0x6a),
            warning: (0xe0, 0xaf, 0x68),
            error: (0xf7, 0x76, 0x8e),
        }
    }

    pub fn light() -> Self {
        Self {
            bg: (0xf7, 0xf8, 0xfa),
            fg: (0x24, 0x29, 0x2f),
            accent: (0x1d, 0x63, 0xed),
            success: (0x16, 0xa3, 0x4a),
            warning: (0xd9, 0x77, 0x06),
            error: (0xdc, 0x26, 0x26),
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
        let selection = blend(accent, bg, 0.35);

        let mut text = fg;
        let text_muted = blend(fg, bg, 0.55);
        let text_disabled = blend(fg, bg, 0.35);

        // Contrast clamp: push text away from bg until |ΔL| >= 0.4.
        let page_l = oklab_l(page);
        if (oklab_l(text) - page_l).abs() < 0.4 {
            let direction = if oklab_l(text) >= page_l { 1.0 } else { -1.0 };
            let target_l = (page_l + direction * 0.4).clamp(0.0, 1.0);
            text = lift(text, target_l - oklab_l(text));
        }

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

    /// Builds the theme per the configured [`ThemeChoice`]. `Dark`/`Light`
    /// always use the built-in seeds; `Terminal` queries the real terminal
    /// via `term` and generates a palette from whatever it answers,
    /// falling back to [`Seeds::dark`] when the terminal reports no
    /// background color (either it stayed silent, or every reply arrived
    /// unparseable).
    pub fn from_environment(choice: ThemeChoice, term: &mut dyn TerminalPalette) -> Self {
        match choice {
            ThemeChoice::Dark => Self::dark(),
            ThemeChoice::Light => Self::light(),
            ThemeChoice::Terminal => {
                let answer = term.query();
                let seeds = match answer.bg {
                    Some(bg) => {
                        let builtin = Seeds::dark();
                        Seeds {
                            bg,
                            fg: answer.fg.unwrap_or_else(|| derive_fg_from_bg(bg)),
                            accent: answer.ansi[4].or(answer.ansi[12]).unwrap_or(builtin.accent),
                            success: answer.ansi[2].unwrap_or(builtin.success),
                            warning: answer.ansi[3].unwrap_or(builtin.warning),
                            error: answer.ansi[1].unwrap_or(builtin.error),
                        }
                    }
                    None => Seeds::dark(),
                };
                Self::generate(&seeds)
            }
        }
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

    struct FakePalette(QueriedColors);
    impl TerminalPalette for FakePalette {
        fn query(&mut self) -> QueriedColors {
            self.0
        }
    }

    #[test]
    fn from_environment_seeds_from_terminal_answer() {
        let mut ansi = [None; 16];
        ansi[4] = Some((1, 120, 212));
        let mut f = FakePalette(QueriedColors {
            bg: Some((16, 16, 20)),
            fg: Some((226, 226, 230)),
            ansi,
        });
        let t = Theme::from_environment(ThemeChoice::Terminal, &mut f);
        assert_eq!(t.page, Color::Rgb(16, 16, 20));
        assert_eq!(t.accent, Color::Rgb(1, 120, 212));
    }

    #[test]
    fn from_environment_falls_back_to_dark_when_silent() {
        let mut f = FakePalette(QueriedColors::default());
        let t = Theme::from_environment(ThemeChoice::Terminal, &mut f);
        assert_eq!(t.page, Theme::dark().page);
    }

    #[test]
    fn from_environment_uses_fallback_accent_when_ansi_slots_missing() {
        let mut f = FakePalette(QueriedColors {
            bg: Some((10, 10, 12)),
            fg: None,
            ansi: [None; 16],
        });
        let t = Theme::from_environment(ThemeChoice::Terminal, &mut f);
        assert_eq!(t.page, Color::Rgb(10, 10, 12));
        assert_eq!(
            t.accent,
            Theme::dark().accent,
            "no ansi[4]/[12]: built-in accent seed"
        );
    }

    #[test]
    fn from_environment_prefers_bright_ansi_slot_when_normal_missing() {
        let mut ansi = [None; 16];
        ansi[12] = Some((5, 5, 200)); // bright blue only
        let mut f = FakePalette(QueriedColors {
            bg: Some((10, 10, 12)),
            fg: None,
            ansi,
        });
        let t = Theme::from_environment(ThemeChoice::Terminal, &mut f);
        assert_eq!(t.accent, Color::Rgb(5, 5, 200));
    }

    #[test]
    fn from_environment_dark_and_light_choices_ignore_the_terminal() {
        let mut f = FakePalette(QueriedColors {
            bg: Some((250, 250, 250)),
            fg: None,
            ansi: [None; 16],
        });
        assert_eq!(
            Theme::from_environment(ThemeChoice::Dark, &mut f).page,
            Theme::dark().page
        );
        assert_eq!(
            Theme::from_environment(ThemeChoice::Light, &mut f).page,
            Theme::light().page
        );
    }

    #[test]
    fn theme_choice_default_is_terminal() {
        assert_eq!(ThemeChoice::default(), ThemeChoice::Terminal);
    }

    #[test]
    fn theme_choice_parse_known_and_unknown_values() {
        assert_eq!(ThemeChoice::parse("dark"), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::parse("light"), ThemeChoice::Light);
        assert_eq!(ThemeChoice::parse("terminal"), ThemeChoice::Terminal);
        assert_eq!(ThemeChoice::parse("sepia"), ThemeChoice::Terminal);
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
            t.accent_edge_light,
            t.accent_edge_dark,
            t.on_accent,
            t.focus_ring,
            t.text_disabled,
        ] {
            assert!(matches!(c, Color::Indexed(_)));
        }
    }
}
