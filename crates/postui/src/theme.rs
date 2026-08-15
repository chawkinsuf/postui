use ratatui::style::Color;

pub struct Theme {
    pub surface: Color,
    pub surface_raised: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub border: Color,
    pub border_focused: Color,
}

impl Theme {
    /// Starting palette (Tokyo-Night-adjacent); visual direction iterates on
    /// these values during stage-1 polish with the frontend-design skill.
    pub fn dark() -> Self {
        Self {
            surface: Color::Rgb(0x13, 0x17, 0x20),
            surface_raised: Color::Rgb(0x1a, 0x1f, 0x2b),
            text: Color::Rgb(0xd8, 0xde, 0xe9),
            text_muted: Color::Rgb(0x7b, 0x84, 0x96),
            accent: Color::Rgb(0x7a, 0xa2, 0xf7),
            success: Color::Rgb(0x9e, 0xce, 0x6a),
            error: Color::Rgb(0xf7, 0x76, 0x8e),
            warning: Color::Rgb(0xe0, 0xaf, 0x68),
            border: Color::Rgb(0x2a, 0x2f, 0x3a),
            border_focused: Color::Rgb(0x7a, 0xa2, 0xf7),
        }
    }

    pub fn light() -> Self {
        Self {
            surface: Color::Rgb(0xf7, 0xf8, 0xfa),
            surface_raised: Color::Rgb(0xff, 0xff, 0xff),
            text: Color::Rgb(0x24, 0x29, 0x2f),
            text_muted: Color::Rgb(0x6e, 0x77, 0x81),
            accent: Color::Rgb(0x1d, 0x63, 0xed),
            success: Color::Rgb(0x16, 0xa3, 0x4a),
            error: Color::Rgb(0xdc, 0x26, 0x26),
            warning: Color::Rgb(0xd9, 0x77, 0x06),
            border: Color::Rgb(0xd0, 0xd7, 0xde),
            border_focused: Color::Rgb(0x1d, 0x63, 0xed),
        }
    }

    pub fn for_terminal() -> Self {
        Self::dark()
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

    pub fn downgrade_to_256(&self) -> Self {
        let f = |c: Color| match c {
            Color::Rgb(r, g, b) => Color::Indexed(rgb_to_indexed(r, g, b)),
            other => other,
        };
        Self {
            surface: f(self.surface),
            surface_raised: f(self.surface_raised),
            text: f(self.text),
            text_muted: f(self.text_muted),
            accent: f(self.accent),
            success: f(self.success),
            error: f(self.error),
            warning: f(self.warning),
            border: f(self.border),
            border_focused: f(self.border_focused),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_tokens_are_rgb() {
        let t = Theme::dark();
        for c in [t.surface, t.text, t.accent, t.border, t.border_focused] {
            assert!(matches!(c, Color::Rgb(..)), "token must be truecolor: {c:?}");
        }
    }

    #[test]
    fn focused_border_differs_from_unfocused() {
        let t = Theme::dark();
        assert_ne!(t.border, t.border_focused);
        let t = Theme::light();
        assert_ne!(t.border, t.border_focused);
    }

    #[test]
    fn downgrade_maps_every_token_to_indexed() {
        let t = Theme::dark().downgrade_to_256();
        for c in [
            t.surface, t.surface_raised, t.text, t.text_muted, t.accent,
            t.success, t.error, t.warning, t.border, t.border_focused,
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
        assert_eq!(rgb_to_indexed(0, 0, 0), 16);      // cube black
        assert_eq!(rgb_to_indexed(255, 255, 255), 231); // cube white
        assert_eq!(rgb_to_indexed(255, 0, 0), 196);   // cube red corner
    }
}
