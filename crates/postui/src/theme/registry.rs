//! The name-based theme registry: terminal colors + the built-in catalog +
//! custom `themes/*.toml` files, resolved by the `theme` config value.

use super::builtin::builtin_themes;
use super::osc::QueriedColors;
use super::{Seeds, Theme, seeds_from_queried};
use std::path::Path;

/// Where a registry entry's seeds come from.
pub enum ThemeSource {
    Builtin(Seeds),
    /// Seeds derive from the startup terminal-color query at resolve time.
    Terminal,
    Custom(Seeds),
}

/// One selectable theme: a stable kebab-case `name` (the config value and
/// picker row id), a display `label`, and its seed source.
pub struct ThemeEntry {
    pub name: String,
    pub label: String,
    pub source: ThemeSource,
    /// The name of this theme's opposite-polarity sibling, when it has
    /// one — built-ins ship theirs, customs pair by the `-dark`/`-light`
    /// stem convention or an explicit `counterpart` key. Validated at
    /// load: always names an existing entry, and the link is mutual.
    /// `None` for unpaired themes (Terminal, lone customs) — the picker's
    /// light/dark switch is disabled while one is highlighted.
    pub counterpart: Option<String>,
}

/// Every theme the picker offers, in display order: terminal first, then
/// built-ins, then custom files sorted by name. A custom file whose stem
/// matches a built-in name shadows it in place (deliberate, no warning).
pub struct ThemeRegistry {
    entries: Vec<ThemeEntry>,
}

impl ThemeRegistry {
    /// Terminal + the built-in catalog, no filesystem access. What `bare`
    /// app construction (and tests) use.
    pub fn builtin() -> Self {
        let mut entries = vec![ThemeEntry {
            name: "terminal".into(),
            label: "Terminal colors".into(),
            source: ThemeSource::Terminal,
            counterpart: None,
        }];
        entries.extend(builtin_themes().into_iter().map(|b| ThemeEntry {
            name: b.name.into(),
            label: b.label.into(),
            source: ThemeSource::Builtin(b.seeds),
            counterpart: b.counterpart.map(Into::into),
        }));
        Self { entries }
    }

    /// `builtin()` plus every parseable `*.toml` in `themes_dir`. Returns
    /// one warning per malformed file; a missing/unreadable dir (or `None`)
    /// is silently just the built-ins.
    pub fn load(themes_dir: Option<&Path>) -> (Self, Vec<String>) {
        let mut registry = Self::builtin();
        let mut warnings = Vec::new();
        let Some(dir) = themes_dir else {
            return (registry, warnings);
        };
        let Ok(read) = std::fs::read_dir(dir) else {
            return (registry, warnings);
        };
        let mut customs: Vec<ThemeEntry> = Vec::new();
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            match parse_theme_file(&path) {
                Ok((label, counterpart, seeds)) => {
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    customs.push(ThemeEntry {
                        label: label.unwrap_or_else(|| name.clone()),
                        counterpart: counterpart.or_else(|| conventional_counterpart(&name)),
                        name,
                        source: ThemeSource::Custom(seeds),
                    });
                }
                Err(e) => warnings.push(format!(
                    "theme file {}: {e}; skipped",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )),
            }
        }
        customs.sort_by(|a, b| a.name.cmp(&b.name));
        for custom in customs {
            match registry.entries.iter_mut().find(|e| e.name == custom.name) {
                Some(slot) => *slot = custom, // shadow a builtin in place
                None => registry.entries.push(custom),
            }
        }
        // Counterpart links must name a real entry other than themselves;
        // a dangling (or self-pointing) declaration drops to unpaired. A
        // valid one-way link is then made mutual when the named side
        // declares no counterpart of its own.
        let names: Vec<String> = registry.entries.iter().map(|e| e.name.clone()).collect();
        for e in &mut registry.entries {
            if let Some(cp) = &e.counterpart
                && (cp == &e.name || !names.contains(cp))
            {
                e.counterpart = None;
            }
        }
        let links: Vec<(String, String)> = registry
            .entries
            .iter()
            .filter_map(|e| e.counterpart.clone().map(|c| (e.name.clone(), c)))
            .collect();
        for (from, to) in links {
            if let Some(other) = registry.entries.iter_mut().find(|e| e.name == to)
                && other.counterpart.is_none()
            {
                other.counterpart = Some(from);
            }
        }
        (registry, warnings)
    }

    pub fn entries(&self) -> &[ThemeEntry] {
        &self.entries
    }

    pub fn get(&self, name: &str) -> Option<&ThemeEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The six seeds an entry would generate from — the picker's swatch
    /// colors. `queried` feeds the Terminal entry; static sources ignore it.
    pub fn seeds_of(&self, entry: &ThemeEntry, queried: &QueriedColors) -> Seeds {
        match &entry.source {
            ThemeSource::Builtin(s) | ThemeSource::Custom(s) => *s,
            ThemeSource::Terminal => seeds_from_queried(queried),
        }
    }

    /// Whether an entry is a dark-background theme, judged the same way
    /// `Theme::generate` picks its ladder polarity (Oklab lightness of the
    /// seed background < 0.5). The Terminal entry is judged from the
    /// queried (or cached) colors. Drives the picker's dark/light filter.
    pub fn entry_is_dark(&self, entry: &ThemeEntry, queried: &QueriedColors) -> bool {
        crate::theme::oklab_l(self.seeds_of(entry, queried).bg) < 0.5
    }

    /// Generates the full theme for `name`, or `None` for an unknown name
    /// (the caller warns and falls back).
    pub fn resolve(&self, name: &str, queried: &QueriedColors) -> Option<Theme> {
        let entry = self.get(name)?;
        Some(Theme::generate(&self.seeds_of(entry, queried)))
    }
}

/// The `-dark`/`-light` stem convention (plus the bare `dark`/`light`
/// pair): the counterpart name a custom theme is assumed to pair with
/// when its file declares no explicit `counterpart` key. Whether that
/// name actually exists is the load-time validation's business.
fn conventional_counterpart(name: &str) -> Option<String> {
    match name {
        "dark" => Some("light".into()),
        "light" => Some("dark".into()),
        _ => name
            .strip_suffix("-dark")
            .map(|base| format!("{base}-light"))
            .or_else(|| {
                name.strip_suffix("-light")
                    .map(|base| format!("{base}-dark"))
            }),
    }
}

/// Parses one custom theme file: six required `#rrggbb` color keys plus an
/// optional `name` display label and an optional `counterpart` (the name
/// of the theme its light/dark switch should land on, overriding the stem
/// convention). Any missing or malformed required piece fails the whole
/// file — no partial themes.
fn parse_theme_file(path: &Path) -> Result<(Option<String>, Option<String>, Seeds), String> {
    let contents = std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))?;
    let value: toml::Value = toml::from_str(&contents).map_err(|_| "invalid TOML".to_string())?;
    let color = |key: &str| -> Result<(u8, u8, u8), String> {
        let raw = value
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("missing key {key:?}"))?;
        parse_hex(raw)
            .ok_or_else(|| format!("invalid color {raw:?} for {key:?} (expected \"#rrggbb\")"))
    };
    let seeds = Seeds {
        bg: color("bg")?,
        fg: color("fg")?,
        accent: color("accent")?,
        success: color("success")?,
        warning: color("warning")?,
        error: color("error")?,
    };
    let label = value
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let counterpart = value
        .get("counterpart")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok((label, counterpart, seeds))
}

/// Parses `"#rrggbb"` (case-insensitive hex) into components.
fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::osc::QueriedColors;
    use ratatui::style::Color;
    use tempfile::tempdir;

    #[test]
    fn builtin_registry_lists_terminal_first_then_the_seven_builtins() {
        let r = ThemeRegistry::builtin();
        let names: Vec<&str> = r.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "terminal",
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
        assert!(matches!(
            r.get("terminal").unwrap().source,
            ThemeSource::Terminal
        ));
        assert_eq!(
            r.get("gruvbox-dark").unwrap().counterpart.as_deref(),
            Some("gruvbox-light")
        );
        assert_eq!(r.get("terminal").unwrap().counterpart, None);
    }

    /// Customs pair by the `-dark`/`-light` stem convention (mutualized at
    /// load), an explicit `counterpart` key overrides the convention, a
    /// dangling declaration drops to unpaired, and a lone custom is
    /// unpaired.
    #[test]
    fn custom_counterparts_by_convention_key_and_validation() {
        let dir = tempdir().unwrap();
        let seeds_body = "fg = \"#e2e2e6\"\naccent = \"#0178d4\"\n\
             success = \"#9ece6a\"\nwarning = \"#e0af68\"\nerror = \"#f7768e\"\n";
        std::fs::write(
            dir.path().join("zebra-dark.toml"),
            format!("bg = \"#101418\"\n{seeds_body}"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("zebra-light.toml"),
            format!("bg = \"#fafafa\"\n{seeds_body}"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("noir.toml"),
            format!("counterpart = \"gruvbox-light\"\nbg = \"#000000\"\n{seeds_body}"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("dangling-dark.toml"),
            format!("bg = \"#000005\"\n{seeds_body}"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("lone.toml"),
            format!("bg = \"#000009\"\n{seeds_body}"),
        )
        .unwrap();
        let (r, warnings) = ThemeRegistry::load(Some(dir.path()));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            r.get("zebra-dark").unwrap().counterpart.as_deref(),
            Some("zebra-light"),
            "stem convention pairs the two zebras"
        );
        assert_eq!(
            r.get("zebra-light").unwrap().counterpart.as_deref(),
            Some("zebra-dark")
        );
        assert_eq!(
            r.get("noir").unwrap().counterpart.as_deref(),
            Some("gruvbox-light"),
            "explicit key overrides the (absent) convention"
        );
        assert_eq!(
            r.get("gruvbox-light").unwrap().counterpart.as_deref(),
            Some("gruvbox-dark"),
            "an already-paired target keeps its own link"
        );
        assert_eq!(
            r.get("dangling-dark").unwrap().counterpart,
            None,
            "conventional name that doesn't exist drops to unpaired"
        );
        assert_eq!(r.get("lone").unwrap().counterpart, None);
    }

    #[test]
    fn resolve_builtin_matches_direct_generation_and_unknown_is_none() {
        let r = ThemeRegistry::builtin();
        let q = QueriedColors::default();
        let t = r.resolve("gruvbox-dark", &q).unwrap();
        assert_eq!(t.page, Color::Rgb(0x28, 0x28, 0x28));
        assert!(r.resolve("sepia", &q).is_none());
    }

    #[test]
    fn resolve_terminal_uses_queried_colors_and_falls_back_when_silent() {
        let r = ThemeRegistry::builtin();
        let mut ansi = [None; 16];
        ansi[4] = Some((1, 120, 212));
        let q = QueriedColors {
            bg: Some((16, 16, 20)),
            fg: None,
            ansi,
        };
        let t = r.resolve("terminal", &q).unwrap();
        assert_eq!(t.page, Color::Rgb(16, 16, 20));
        assert_eq!(t.accent, Color::Rgb(1, 120, 212));
        let silent = r.resolve("terminal", &QueriedColors::default()).unwrap();
        assert_eq!(silent.page, crate::theme::Theme::dark().page);
    }

    #[test]
    fn load_parses_a_valid_custom_file_and_sorts_customs_by_name() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("zebra.toml"),
            "bg = \"#101418\"\nfg = \"#e2e2e6\"\naccent = \"#0178d4\"\n\
             success = \"#9ece6a\"\nwarning = \"#e0af68\"\nerror = \"#f7768e\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("aardvark.toml"),
            "name = \"Aard Vark\"\nbg = \"#000000\"\nfg = \"#ffffff\"\naccent = \"#0178d4\"\n\
             success = \"#9ece6a\"\nwarning = \"#e0af68\"\nerror = \"#f7768e\"\n",
        )
        .unwrap();
        let (r, warnings) = ThemeRegistry::load(Some(dir.path()));
        assert!(warnings.is_empty(), "{warnings:?}");
        let customs: Vec<&str> = r
            .entries()
            .iter()
            .filter(|e| matches!(e.source, ThemeSource::Custom(_)))
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            customs,
            vec!["aardvark", "zebra"],
            "customs after builtins, sorted"
        );
        assert_eq!(
            r.get("aardvark").unwrap().label,
            "Aard Vark",
            "name key overrides the stem"
        );
        assert_eq!(
            r.get("zebra").unwrap().label,
            "zebra",
            "label defaults to the stem"
        );
        let t = r.resolve("zebra", &QueriedColors::default()).unwrap();
        assert_eq!(t.page, Color::Rgb(0x10, 0x14, 0x18));
    }

    #[test]
    fn load_skips_bad_files_with_a_warning_each() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("nokey.toml"), "bg = \"#101418\"\n").unwrap();
        std::fs::write(
            dir.path().join("badhex.toml"),
            "bg = \"purple\"\nfg = \"#e2e2e6\"\naccent = \"#0178d4\"\n\
             success = \"#9ece6a\"\nwarning = \"#e0af68\"\nerror = \"#f7768e\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a theme").unwrap();
        let (r, warnings) = ThemeRegistry::load(Some(dir.path()));
        assert_eq!(
            warnings.len(),
            2,
            "one warning per bad .toml; non-toml ignored: {warnings:?}"
        );
        assert!(warnings.iter().any(|w| w.contains("nokey")));
        assert!(warnings.iter().any(|w| w.contains("badhex")));
        assert_eq!(r.entries().len(), 10, "builtins only");
    }

    #[test]
    fn custom_file_shadows_a_builtin_of_the_same_name_in_place() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("dark.toml"),
            "bg = \"#000000\"\nfg = \"#ffffff\"\naccent = \"#0178d4\"\n\
             success = \"#9ece6a\"\nwarning = \"#e0af68\"\nerror = \"#f7768e\"\n",
        )
        .unwrap();
        let (r, warnings) = ThemeRegistry::load(Some(dir.path()));
        assert!(
            warnings.is_empty(),
            "shadowing is deliberate, no warning: {warnings:?}"
        );
        assert_eq!(r.entries().len(), 10, "shadow replaces, not appends");
        assert_eq!(r.entries()[1].name, "dark", "position preserved");
        let t = r.resolve("dark", &QueriedColors::default()).unwrap();
        assert_eq!(t.page, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn entry_polarity_follows_the_seed_background() {
        let r = ThemeRegistry::builtin();
        let q = QueriedColors::default();
        assert!(r.entry_is_dark(r.get("gruvbox-dark").unwrap(), &q));
        assert!(!r.entry_is_dark(r.get("gruvbox-light").unwrap(), &q));
        // Terminal entry: silent query falls back to the dark seeds...
        assert!(r.entry_is_dark(r.get("terminal").unwrap(), &q));
        // ...but a light queried background flips it.
        let light = QueriedColors {
            bg: Some((0xfa, 0xfa, 0xfa)),
            fg: None,
            ansi: [None; 16],
        };
        assert!(!r.entry_is_dark(r.get("terminal").unwrap(), &light));
    }

    #[test]
    fn load_missing_dir_is_builtins_with_no_warnings() {
        let (r, warnings) = ThemeRegistry::load(Some(std::path::Path::new("/nonexistent/themes")));
        assert!(warnings.is_empty());
        assert_eq!(r.entries().len(), 10);
        let (r2, w2) = ThemeRegistry::load(None);
        assert!(w2.is_empty());
        assert_eq!(r2.entries().len(), 10);
    }
}
