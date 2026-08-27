//! The built-in theme catalog: hand-picked six-seed palettes expanded by
//! `Theme::generate`. Data only — no logic lives here.

use super::Seeds;

/// One built-in theme: a stable kebab-case `name` (the config value), a
/// display `label` for the picker, and its seed palette.
pub struct BuiltinTheme {
    pub name: &'static str,
    pub label: &'static str,
    pub seeds: Seeds,
    /// The name of this theme's opposite-polarity sibling, when the
    /// catalog ships one — the picker's light/dark switch follows it.
    pub counterpart: Option<&'static str>,
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
        BuiltinTheme {
            name: "dark",
            label: "Dark",
            seeds: Seeds::dark(),
            counterpart: Some("light"),
        },
        BuiltinTheme {
            name: "light",
            label: "Light",
            seeds: Seeds::light(),
            counterpart: Some("dark"),
        },
        // The exact ghostty terminal palette the default Dark seeds were
        // adapted from (Dark stands in palette slots 0/7 where this uses
        // the real background/foreground keys) — kept next to Dark for
        // side-by-side comparison. Unpaired: no light sibling exists.
        BuiltinTheme {
            name: "ghostty",
            label: "Ghostty",
            seeds: s([
                (0x0a, 0x20, 0x28),
                (0xcc, 0xd8, 0xe0),
                (0x78, 0xa8, 0xc8),
                (0x90, 0xac, 0x60),
                (0xc8, 0xa8, 0x68),
                (0xcc, 0x7e, 0x78),
            ]),
            counterpart: None,
        },
        BuiltinTheme {
            name: "gruvbox-dark",
            label: "Gruvbox Dark",
            seeds: s([
                (0x28, 0x28, 0x28),
                (0xeb, 0xdb, 0xb2),
                (0x83, 0xa5, 0x98),
                (0xb8, 0xbb, 0x26),
                (0xfa, 0xbd, 0x2f),
                (0xfb, 0x49, 0x34),
            ]),
            counterpart: Some("gruvbox-light"),
        },
        BuiltinTheme {
            name: "gruvbox-light",
            label: "Gruvbox Light",
            seeds: s([
                (0xfb, 0xf1, 0xc7),
                (0x3c, 0x38, 0x36),
                (0x07, 0x66, 0x78),
                (0x79, 0x74, 0x0e),
                (0xb5, 0x76, 0x14),
                (0x9d, 0x00, 0x06),
            ]),
            counterpart: Some("gruvbox-dark"),
        },
        BuiltinTheme {
            name: "catppuccin-mocha",
            label: "Catppuccin Mocha",
            seeds: s([
                (0x1e, 0x1e, 0x2e),
                (0xcd, 0xd6, 0xf4),
                (0x89, 0xb4, 0xfa),
                (0xa6, 0xe3, 0xa1),
                (0xf9, 0xe2, 0xaf),
                (0xf3, 0x8b, 0xa8),
            ]),
            counterpart: Some("catppuccin-latte"),
        },
        BuiltinTheme {
            name: "catppuccin-latte",
            label: "Catppuccin Latte",
            seeds: s([
                (0xef, 0xf1, 0xf5),
                (0x4c, 0x4f, 0x69),
                (0x1e, 0x66, 0xf5),
                (0x40, 0xa0, 0x2b),
                (0xdf, 0x8e, 0x1d),
                (0xd2, 0x0f, 0x39),
            ]),
            counterpart: Some("catppuccin-mocha"),
        },
        BuiltinTheme {
            name: "solarized-dark",
            label: "Solarized Dark",
            seeds: s([
                (0x00, 0x2b, 0x36),
                (0x83, 0x94, 0x96),
                (0x26, 0x8b, 0xd2),
                (0x85, 0x99, 0x00),
                (0xb5, 0x89, 0x00),
                (0xdc, 0x32, 0x2f),
            ]),
            counterpart: Some("solarized-light"),
        },
        BuiltinTheme {
            name: "solarized-light",
            label: "Solarized Light",
            seeds: s([
                (0xfd, 0xf6, 0xe3),
                (0x65, 0x7b, 0x83),
                (0x26, 0x8b, 0xd2),
                (0x85, 0x99, 0x00),
                (0xb5, 0x89, 0x00),
                (0xdc, 0x32, 0x2f),
            ]),
            counterpart: Some("solarized-dark"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, oklab_l, rgb_of};

    #[test]
    fn builtins_in_stable_order_with_unique_names() {
        let all = builtin_themes();
        let names: Vec<&str> = all.iter().map(|b| b.name).collect();
        assert_eq!(
            names,
            vec![
                "dark",
                "light",
                "ghostty",
                "gruvbox-dark",
                "gruvbox-light",
                "catppuccin-mocha",
                "catppuccin-latte",
                "solarized-dark",
                "solarized-light",
            ]
        );
    }

    /// A declared counterpart names a real catalog entry of the opposite
    /// polarity, and the link is mutual. Unpaired built-ins (ghostty) are
    /// allowed — the picker's light/dark switch hides for them.
    #[test]
    fn declared_counterparts_are_mutual_and_opposite_polarity() {
        let all = builtin_themes();
        assert!(
            all.iter().filter(|b| b.counterpart.is_some()).count() >= 8,
            "the paired families stay paired"
        );
        for b in &all {
            let Some(cp_name) = b.counterpart else {
                continue;
            };
            let cp = all
                .iter()
                .find(|o| o.name == cp_name)
                .unwrap_or_else(|| panic!("{}: counterpart {cp_name} missing", b.name));
            assert_ne!(
                oklab_l(b.seeds.bg) < 0.5,
                oklab_l(cp.seeds.bg) < 0.5,
                "{}: counterpart must have the opposite polarity",
                b.name
            );
            assert_eq!(
                cp.counterpart,
                Some(b.name),
                "{}: link must be mutual",
                b.name
            );
        }
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
            // Strict: the clamp's target overshoots u8 quantization, so
            // the 0.4 guarantee holds exactly for every catalog entry.
            assert!(d >= 0.4, "{}: contrast {d}", b.name);
        }
    }
}
