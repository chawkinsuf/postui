//! The built-in theme catalog: hand-picked six-seed palettes expanded by
//! `Theme::generate`. Data only — no logic lives here.

use super::Seeds;

/// One built-in theme: a stable kebab-case `name` (the config value), a
/// display `label` for the picker, and its seed palette.
pub struct BuiltinTheme {
    pub name: &'static str,
    pub label: &'static str,
    pub seeds: Seeds,
}

/// Every built-in, in picker order. `dark`/`light` reuse the existing
/// `Seeds::dark()`/`Seeds::light()` values so the legacy config names keep
/// their exact look.
pub fn builtin_themes() -> Vec<BuiltinTheme> {
    let s = |hex: [(u8, u8, u8); 6]| Seeds {
        bg: hex[0],
        fg: hex[1],
        accent: hex[2],
        success: hex[3],
        warning: hex[4],
        error: hex[5],
    };
    vec![
        BuiltinTheme { name: "dark", label: "Dark", seeds: Seeds::dark() },
        BuiltinTheme { name: "light", label: "Light", seeds: Seeds::light() },
        BuiltinTheme {
            name: "gruvbox-dark",
            label: "Gruvbox Dark",
            seeds: s([
                (0x28, 0x28, 0x28), (0xeb, 0xdb, 0xb2), (0x83, 0xa5, 0x98),
                (0xb8, 0xbb, 0x26), (0xfa, 0xbd, 0x2f), (0xfb, 0x49, 0x34),
            ]),
        },
        BuiltinTheme {
            name: "gruvbox-light",
            label: "Gruvbox Light",
            seeds: s([
                (0xfb, 0xf1, 0xc7), (0x3c, 0x38, 0x36), (0x07, 0x66, 0x78),
                (0x79, 0x74, 0x0e), (0xb5, 0x76, 0x14), (0x9d, 0x00, 0x06),
            ]),
        },
        BuiltinTheme {
            name: "catppuccin-mocha",
            label: "Catppuccin Mocha",
            seeds: s([
                (0x1e, 0x1e, 0x2e), (0xcd, 0xd6, 0xf4), (0x89, 0xb4, 0xfa),
                (0xa6, 0xe3, 0xa1), (0xf9, 0xe2, 0xaf), (0xf3, 0x8b, 0xa8),
            ]),
        },
        BuiltinTheme {
            name: "solarized-dark",
            label: "Solarized Dark",
            seeds: s([
                (0x00, 0x2b, 0x36), (0x83, 0x94, 0x96), (0x26, 0x8b, 0xd2),
                (0x85, 0x99, 0x00), (0xb5, 0x89, 0x00), (0xdc, 0x32, 0x2f),
            ]),
        },
        BuiltinTheme {
            name: "solarized-light",
            label: "Solarized Light",
            seeds: s([
                (0xfd, 0xf6, 0xe3), (0x65, 0x7b, 0x83), (0x26, 0x8b, 0xd2),
                (0x85, 0x99, 0x00), (0xb5, 0x89, 0x00), (0xdc, 0x32, 0x2f),
            ]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, oklab_l, rgb_of};

    #[test]
    fn seven_builtins_in_stable_order_with_unique_names() {
        let all = builtin_themes();
        let names: Vec<&str> = all.iter().map(|b| b.name).collect();
        assert_eq!(
            names,
            vec![
                "dark",
                "light",
                "gruvbox-dark",
                "gruvbox-light",
                "catppuccin-mocha",
                "solarized-dark",
                "solarized-light",
            ]
        );
    }

    /// Every built-in must generate a full theme without panicking, with
    /// the ladder polarity matching its background (dark bg -> is_dark).
    #[test]
    fn every_builtin_generates_and_reports_its_polarity() {
        for b in builtin_themes() {
            let t = Theme::generate(&b.seeds);
            let dark_bg = oklab_l(b.seeds.bg) < 0.5;
            assert_eq!(t.is_dark(), dark_bg, "{}", b.name);
        }
    }

    /// The generator's contrast clamp must hold for the low-contrast
    /// palettes too (Solarized's fg sits close to its bg).
    #[test]
    fn text_contrast_clamp_holds_for_every_builtin() {
        for b in builtin_themes() {
            let t = Theme::generate(&b.seeds);
            let d = (oklab_l(rgb_of(t.text)) - oklab_l(rgb_of(t.page))).abs();
            // The clamp targets ΔL = 0.4, but the Oklab→sRGB→u8 round-trip can
            // quantize a boundary color slightly (~0.0005) short of the 0.4 target.
            assert!(d >= 0.399, "{}: contrast {d}", b.name);
        }
    }
}
