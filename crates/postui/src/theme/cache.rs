//! Persistent cache of the terminal's last successfully queried colors,
//! so a launch where the OSC reply loses the race (or never comes) can
//! render the same palette as every other launch in that terminal instead
//! of falling back to the built-in dark seeds.
//!
//! The cache is keyed by a terminal identity string (`$TERM_PROGRAM|$TERM`)
//! recorded alongside the colors: colors cached in one terminal are never
//! replayed in a different one.

use super::osc::QueriedColors;
use std::path::Path;

/// The current terminal's identity for cache keying. Both variables
/// missing degrades to `"|"` — still a stable key for that (odd)
/// environment, and never a match for a real terminal's entry.
pub fn term_identity() -> String {
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    format!("{program}|{term}")
}

/// Writes `colors` to the cache file at `path`, tagged with `term`.
/// Creates the parent directory if needed. The file is wholly owned by
/// this module — it is rewritten, not merged.
pub fn save(path: &Path, term: &str, colors: &QueriedColors) -> std::io::Result<()> {
    let mut out = String::new();
    out.push_str(&format!("term = {:?}\n", term));
    let hex = |(r, g, b): (u8, u8, u8)| format!("\"#{r:02x}{g:02x}{b:02x}\"");
    if let Some(bg) = colors.bg {
        out.push_str(&format!("bg = {}\n", hex(bg)));
    }
    if let Some(fg) = colors.fg {
        out.push_str(&format!("fg = {}\n", hex(fg)));
    }
    for (i, slot) in colors.ansi.iter().enumerate() {
        if let Some(c) = slot {
            out.push_str(&format!("ansi{i} = {}\n", hex(*c)));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)
}

/// Loads the cached colors from `path`, but only when the recorded
/// terminal identity matches `term` — a cache written by a different
/// terminal returns `None`, as does a missing, unreadable, or malformed
/// file, or one holding no background color (a bg-less cache is no better
/// than the built-in fallback it would replace).
pub fn load(path: &Path, term: &str) -> Option<QueriedColors> {
    let contents = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&contents).ok()?;
    if value.get("term").and_then(|v| v.as_str()) != Some(term) {
        return None;
    }
    let color = |key: &str| -> Option<(u8, u8, u8)> { parse_hex(value.get(key)?.as_str()?) };
    let mut colors = QueriedColors {
        bg: color("bg"),
        fg: color("fg"),
        ansi: [None; 16],
    };
    for (i, slot) in colors.ansi.iter_mut().enumerate() {
        *slot = color(&format!("ansi{i}"));
    }
    colors.bg?;
    Some(colors)
}

/// Parses `"#rrggbb"` into components. Kept private and duplicated from
/// the registry's parser rather than shared — two three-line parsers beat
/// a cross-module dependency for this.
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
    use tempfile::tempdir;

    fn sample() -> QueriedColors {
        let mut ansi = [None; 16];
        ansi[1] = Some((0xf7, 0x76, 0x8e));
        ansi[4] = Some((0x01, 0x78, 0xd4));
        QueriedColors {
            bg: Some((0x10, 0x14, 0x20)),
            fg: Some((0xd8, 0xde, 0xe9)),
            ansi,
        }
    }

    #[test]
    fn save_then_load_round_trips_for_the_same_terminal() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("terminal-colors.toml");
        save(&p, "ghostty|xterm-ghostty", &sample()).unwrap();
        let loaded = load(&p, "ghostty|xterm-ghostty").unwrap();
        assert_eq!(loaded, sample());
    }

    #[test]
    fn load_refuses_a_different_terminal_identity() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("terminal-colors.toml");
        save(&p, "ghostty|xterm-ghostty", &sample()).unwrap();
        assert!(load(&p, "kitty|xterm-kitty").is_none());
    }

    #[test]
    fn load_missing_or_malformed_file_is_none() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("terminal-colors.toml");
        assert!(load(&p, "a|b").is_none(), "missing file");
        std::fs::write(&p, "not toml [[[").unwrap();
        assert!(load(&p, "a|b").is_none(), "malformed file");
    }

    #[test]
    fn load_refuses_a_cache_without_a_background() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("terminal-colors.toml");
        save(&p, "a|b", &QueriedColors::default()).unwrap();
        assert!(load(&p, "a|b").is_none(), "bg-less cache is useless");
    }

    #[test]
    fn save_creates_the_parent_directory() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("sub").join("terminal-colors.toml");
        save(&p, "a|b", &sample()).unwrap();
        assert!(load(&p, "a|b").is_some());
    }
}
