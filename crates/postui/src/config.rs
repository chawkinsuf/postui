//! Global project registry stored in the app's `config.toml`, under the
//! `[projects]` table: known project paths (cycle order), an optional
//! configured root directory, and the last-used project.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The set of known projects plus the configured root and last-used project,
/// as stored under `[projects]` in the global config file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectsRegistry {
    pub known: Vec<PathBuf>,
    pub root: Option<PathBuf>,
    pub last: Option<PathBuf>,
}

impl ProjectsRegistry {
    /// Loads the registry from `path`. Never errors: a missing or corrupt
    /// file, or a mistyped piece of the `[projects]` table, degrades to the
    /// default for that piece.
    pub fn load_from(path: &Path) -> Self {
        let mut registry = Self::default();
        let Ok(contents) = std::fs::read_to_string(path) else {
            return registry;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&contents) else {
            return registry;
        };
        let Some(projects) = value.get("projects").and_then(|v| v.as_table()) else {
            return registry;
        };

        if let Some(known) = projects.get("known").and_then(|v| v.as_array()) {
            registry.known = known
                .iter()
                .filter_map(|v| v.as_str())
                .map(expand_tilde)
                .collect();
        }
        registry.root = projects
            .get("root")
            .and_then(|v| v.as_str())
            .map(expand_tilde);
        registry.last = projects
            .get("last")
            .and_then(|v| v.as_str())
            .map(expand_tilde);

        registry
    }

    /// Writes the registry to `path`, round-tripping through
    /// `toml_edit::DocumentMut` so only the `[projects]` table is touched;
    /// unrelated keys are preserved byte-for-byte. Creates the parent
    /// directory if needed.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = existing.parse().unwrap_or_default();

        let mut table = toml_edit::Table::new();

        let mut known = toml_edit::Array::new();
        for p in &self.known {
            known.push(p.to_string_lossy().into_owned());
        }
        table["known"] = toml_edit::value(known);

        match &self.root {
            Some(root) => table["root"] = toml_edit::value(root.to_string_lossy().into_owned()),
            None => {
                table.remove("root");
            }
        }
        match &self.last {
            Some(last) => table["last"] = toml_edit::value(last.to_string_lossy().into_owned()),
            None => {
                table.remove("last");
            }
        }

        doc["projects"] = toml_edit::Item::Table(table);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, doc.to_string())?;
        Ok(())
    }

    /// Registers `path` as known and as the last-used project. Dedups on
    /// path: if `path` is already known it is not re-pushed, but `last` is
    /// always updated.
    pub fn register(&mut self, path: PathBuf) {
        self.add_known(path.clone());
        self.last = Some(path);
    }

    /// Adds `path` to `known` if not already present. Does not touch
    /// `last` — for callers that must not commit to a project as current
    /// until some later gate (e.g. a dirty-editor confirm) resolves.
    pub fn add_known(&mut self, path: PathBuf) {
        if !self.known.contains(&path) {
            self.known.push(path);
        }
    }

    /// The configured root, or `~/postui-projects` if unset (falling back to
    /// `.` if the home directory can't be determined).
    pub fn default_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| {
            directories::BaseDirs::new()
                .map(|dirs| dirs.home_dir().join("postui-projects"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
    }

    /// The next project after `current` in cycle order, wrapping, skipping
    /// over entries that no longer exist on disk or that equal `current`.
    /// `None` when fewer than two projects are known or none qualify.
    pub fn next_after(&self, current: &Path) -> Option<PathBuf> {
        self.neighbor(current, 1)
    }

    /// The registered project `delta` steps (`1` next, `-1` previous)
    /// from `current`, wrapping and skipping roots that no longer exist
    /// on disk. `None` with fewer than two projects registered or none
    /// other than `current` present.
    pub fn neighbor(&self, current: &Path, delta: i32) -> Option<PathBuf> {
        if self.known.len() < 2 {
            return None;
        }
        let len = self.known.len() as i32;
        let pos = self.known.iter().position(|p| p == current);
        let start = match pos {
            Some(i) => (i as i32 + delta).rem_euclid(len) as usize,
            None => 0,
        };
        // Step onward in `delta`'s direction past any missing roots.
        let step_dir = if delta < 0 { -1 } else { 1 };
        for step in 0..self.known.len() {
            let idx = (start as i32 + step_dir * step as i32).rem_euclid(len) as usize;
            let candidate = &self.known[idx];
            if candidate != current && candidate.is_dir() {
                return Some(candidate.clone());
            }
        }
        None
    }
}

/// Config-tunable eased-transition durations for the motion catalog,
/// parsed from the optional `[animation_ms]` table in `config.toml` (each
/// key an integer count of milliseconds). Missing keys take these
/// defaults; an unknown key degrades to being ignored and is reported in
/// the returned warnings, the same posture `theme` uses for a bad value.
/// This table only tunes *how long* an eased transition takes; the
/// top-level `animations` bool on [`UiSettings`] remains the separate,
/// all-or-nothing kill switch for whether it eases at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimDurations {
    /// The tab-underline slide, e.g. switching between Params/Headers/Body.
    pub tab_slide: Duration,
    /// A hovered control's fill/edges easing in.
    pub hover: Duration,
    /// A focused control's fill/edges easing in.
    pub focus: Duration,
    /// A scrollable list's selection band sliding row-to-row.
    pub list_travel: Duration,
    /// A modal dialog's open transition.
    pub modal_open: Duration,
    /// A dropdown's open transition.
    pub dropdown_open: Duration,
    /// A collapsing pane's transition.
    pub pane_collapse: Duration,
    /// A toast's fade.
    pub toast: Duration,
    /// The in-flight Send button's breathe, per pole.
    pub send_breathe: Duration,
}

impl Default for AnimDurations {
    fn default() -> Self {
        Self {
            tab_slide: Duration::from_millis(250),
            hover: Duration::from_millis(70),
            focus: Duration::from_millis(90),
            list_travel: Duration::from_millis(100),
            modal_open: Duration::from_millis(100),
            dropdown_open: Duration::from_millis(90),
            pane_collapse: Duration::from_millis(120),
            toast: Duration::from_millis(100),
            send_breathe: Duration::from_millis(700),
        }
    }
}

/// What Tab does in the response pane's jq bar while a completion ghost
/// is showing (`jq_tab` in `config.toml`): list the candidates under the
/// bar and step through them, shell-style (`menu`, the default; `accept`
/// is accepted as an older name for it), or ghost the best one after the
/// caret and step through the rest in place (`cycle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JqTab {
    Cycle,
    #[default]
    Menu,
}

/// Mouse-first-GUI UI settings stored at the top level of `config.toml`:
/// the tiered clipboard's optional external command and the OSC 52 size
/// threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSettings {
    pub clipboard_cmd: Option<String>,
    pub osc52_limit: usize,
    /// The configured theme's registry name (e.g. `"terminal"`,
    /// `"gruvbox-dark"`, or a custom theme file's stem). Free-form: name
    /// validity is the registry's business at resolve time, not this
    /// loader's.
    pub theme: String,
    /// Whether eased transitions (tab underline, hover, modal open, ...)
    /// play, or every animated value jumps straight to its target.
    pub animations: bool,
    /// Per-transition durations, tunable via the optional `[animation_ms]`
    /// table; see [`AnimDurations`].
    pub anim_ms: AnimDurations,
    /// The shell command "Describe a filter…" runs, with the prompt piped
    /// in on stdin: `claude -p` by default. Only its first word (the
    /// program name) is looked up on PATH to gate the menu item.
    pub ai_cmd: String,
    /// Whether the user has already confirmed sending the response's shape
    /// to `ai_cmd` — set once via the "Always send" choice and persisted,
    /// so later requests skip the confirmation.
    pub ai_confirmed: bool,
    /// Tab's job in the jq bar while a completion ghost shows; see
    /// [`JqTab`].
    pub jq_tab: JqTab,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            clipboard_cmd: None,
            osc52_limit: 65536,
            theme: "terminal".into(),
            animations: true,
            anim_ms: AnimDurations::default(),
            ai_cmd: "claude -p".into(),
            ai_confirmed: false,
            jq_tab: JqTab::Menu,
        }
    }
}

/// Reads the top-level `clipboard_cmd` (string), `osc52_limit` (integer),
/// `theme` (string), `animations` (bool), `ai_cmd` (string), `ai_confirmed`
/// (bool), and `jq_tab` (string) keys from `config.toml`. Never errors: a
/// missing file, corrupt TOML, or a mistyped key degrades that piece to its
/// default. `theme` is taken verbatim as a raw name string — whether it names
/// a real registry entry is the registry's business at resolve time, not this
/// loader's, so no warning is produced here for an unrecognized value.
pub fn load_ui_settings(path: &Path) -> (UiSettings, Vec<String>) {
    let mut settings = UiSettings::default();
    let mut warnings = Vec::new();
    let Ok(contents) = std::fs::read_to_string(path) else {
        return (settings, warnings);
    };
    let Ok(value) = toml::from_str::<toml::Value>(&contents) else {
        return (settings, warnings);
    };

    if let Some(cmd) = value.get("clipboard_cmd").and_then(|v| v.as_str()) {
        settings.clipboard_cmd = Some(cmd.to_string());
    }
    if let Some(limit) = value.get("osc52_limit").and_then(|v| v.as_integer())
        && let Ok(limit) = usize::try_from(limit)
    {
        settings.osc52_limit = limit;
    }
    if let Some(raw) = value.get("theme").and_then(|v| v.as_str()) {
        settings.theme = raw.to_string();
    }
    if let Some(b) = value.get("animations").and_then(|v| v.as_bool()) {
        settings.animations = b;
    }
    if let Some(cmd) = value.get("ai_cmd").and_then(|v| v.as_str()) {
        settings.ai_cmd = cmd.to_string();
    }
    if let Some(b) = value.get("ai_confirmed").and_then(|v| v.as_bool()) {
        settings.ai_confirmed = b;
    }
    if let Some(raw) = value.get("jq_tab").and_then(|v| v.as_str()) {
        match raw {
            "cycle" => settings.jq_tab = JqTab::Cycle,
            "menu" | "accept" => settings.jq_tab = JqTab::Menu,
            other => warnings.push(format!(
                "invalid value {other:?} for jq_tab in config.toml \
                 (expected \"menu\" or \"cycle\"); using \"menu\""
            )),
        }
    }

    if let Some(table) = value.get("animation_ms").and_then(|v| v.as_table()) {
        const KNOWN_KEYS: [&str; 9] = [
            "tab_slide",
            "hover",
            "focus",
            "list_travel",
            "modal_open",
            "dropdown_open",
            "pane_collapse",
            "toast",
            "send_breathe",
        ];
        let mut set_ms = |key: &str, field: &mut std::time::Duration| {
            let Some(v) = table.get(key) else {
                return;
            };
            match v.as_integer().and_then(|ms| u64::try_from(ms).ok()) {
                Some(ms) => *field = std::time::Duration::from_millis(ms),
                None => warnings.push(format!(
                    "invalid value for {key:?} in [animation_ms] section of \
                     config.toml (expected a non-negative integer); using default"
                )),
            }
        };
        set_ms("tab_slide", &mut settings.anim_ms.tab_slide);
        set_ms("hover", &mut settings.anim_ms.hover);
        set_ms("focus", &mut settings.anim_ms.focus);
        set_ms("list_travel", &mut settings.anim_ms.list_travel);
        set_ms("modal_open", &mut settings.anim_ms.modal_open);
        set_ms("dropdown_open", &mut settings.anim_ms.dropdown_open);
        set_ms("pane_collapse", &mut settings.anim_ms.pane_collapse);
        set_ms("toast", &mut settings.anim_ms.toast);
        set_ms("send_breathe", &mut settings.anim_ms.send_breathe);

        for key in table.keys() {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                warnings.push(format!(
                    "unknown key {key:?} in [animation_ms] section of config.toml"
                ));
            }
        }
    }

    (settings, warnings)
}

/// The path to the global config file: `<config dir>/config.toml`.
pub fn config_file_path() -> Option<PathBuf> {
    postui_core::config_dir().map(|d| d.join("config.toml"))
}

/// Writes `name` as the top-level `theme` key in the config file at
/// `path`, round-tripping through `toml_edit::DocumentMut` so every other
/// key is preserved byte-for-byte (same posture as
/// `ProjectsRegistry::save_to`). Creates the parent directory if needed.
pub fn save_ui_theme(path: &Path, name: &str) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = existing.parse().unwrap_or_default();
    doc["theme"] = toml_edit::value(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, doc.to_string())?;
    Ok(())
}

/// Sets one top-level boolean in `config.toml`, keeping everything else
/// byte-for-byte (the `ai_confirmed` "don't ask again" flag).
pub fn save_ui_flag(path: &Path, key: &str, value: bool) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = existing.parse().unwrap_or_default();
    doc[key] = toml_edit::value(value);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, doc.to_string())?;
    Ok(())
}

/// The directory custom theme files live in: `<config dir>/themes`.
pub fn themes_dir_path() -> Option<PathBuf> {
    postui_core::config_dir().map(|d| d.join("themes"))
}

/// The path to the terminal-color cache written after a successful OSC
/// query: `<config dir>/terminal-colors.toml`. See `theme::cache`.
pub fn terminal_colors_path() -> Option<PathBuf> {
    postui_core::config_dir().map(|d| d.join("terminal-colors.toml"))
}

/// The path to the mouse-first-GUI UI-state file (currently just palette
/// usage stats): `<config dir>/ui.toml`.
pub fn ui_file_path() -> Option<PathBuf> {
    postui_core::config_dir().map(|d| d.join("ui.toml"))
}

/// Expands a leading `~/` to the home directory. Paths not starting with
/// `~/` are returned unchanged.
pub fn expand_tilde(s: &str) -> PathBuf {
    let home = || directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    if s == "~" {
        return home().unwrap_or_else(|| PathBuf::from(s));
    }
    match s.strip_prefix("~/") {
        Some(rest) => match home() {
            Some(h) => h.join(rest),
            None => PathBuf::from(s),
        },
        None => PathBuf::from(s),
    }
}

/// Result of parsing the single optional CLI argument `postui` accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliParse {
    /// A project directory (or none), tilde-expanded.
    Root(Option<PathBuf>),
    /// A leading-dash argument (`--help`, `-x`, ...): print usage and exit.
    Usage,
    /// `--setup`: print terminal keyboard-config guidance and exit.
    Setup,
    /// `--keydump[=FLAGS]`: run the raw key-event echo loop and exit.
    /// `flags` is an explicit kitty-keyboard-protocol enhancement bitmask
    /// to push (`0` pushes nothing); `None` means "push what the app
    /// itself pushes", so a plain `--keydump` reproduces the app's exact
    /// input conditions.
    Keydump { flags: Option<u8> },
}

/// Parses `postui`'s single optional argument. Any value starting with `-`
/// (e.g. `--help`, `-x`) is treated as a request for usage rather than a
/// project directory, since no real path starts with a dash without `./`.
pub fn parse_cli(arg: Option<String>) -> CliParse {
    match arg {
        Some(s) if s == "--setup" => CliParse::Setup,
        Some(s) if s == "--keydump" => CliParse::Keydump { flags: None },
        Some(s) if s.starts_with("--keydump=") => match s["--keydump=".len()..].parse::<u8>() {
            Ok(bits) => CliParse::Keydump { flags: Some(bits) },
            Err(_) => CliParse::Usage,
        },
        Some(s) if s.starts_with('-') => CliParse::Usage,
        Some(s) => CliParse::Root(Some(expand_tilde(&s))),
        None => CliParse::Root(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn load_missing_or_corrupt_is_default() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let r = ProjectsRegistry::load_from(&p);
        assert!(r.known.is_empty() && r.last.is_none());
        std::fs::write(&p, "projects = 5\n").unwrap();
        let r = ProjectsRegistry::load_from(&p);
        assert!(r.known.is_empty(), "corrupt config degrades to default");
    }

    #[test]
    fn save_round_trips_and_preserves_unrelated_keys() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"dark\"\n").unwrap();
        let mut r = ProjectsRegistry::load_from(&p);
        r.register(PathBuf::from("/tmp/a"));
        r.register(PathBuf::from("/tmp/b"));
        r.register(PathBuf::from("/tmp/a")); // dedup, but last updates
        r.root = Some(PathBuf::from("/tmp/root"));
        r.save_to(&p).unwrap();

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("theme = \"dark\""),
            "unrelated key preserved: {text}"
        );
        let r2 = ProjectsRegistry::load_from(&p);
        assert_eq!(
            r2.known,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
        assert_eq!(r2.last, Some(PathBuf::from("/tmp/a")));
        assert_eq!(r2.root, Some(PathBuf::from("/tmp/root")));
    }

    #[test]
    fn next_after_cycles_and_wraps() {
        let mut r = ProjectsRegistry::load_from(&PathBuf::from("/nonexistent"));
        assert!(
            r.next_after(&PathBuf::from("/tmp/a")).is_none(),
            "fewer than two projects"
        );
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let c = tempdir().unwrap();
        r.register(a.path().to_path_buf());
        r.register(b.path().to_path_buf());
        r.register(c.path().to_path_buf());
        assert_eq!(r.next_after(b.path()), Some(c.path().to_path_buf()));
        assert_eq!(
            r.next_after(c.path()),
            Some(a.path().to_path_buf()),
            "wraps"
        );
        assert_eq!(
            r.next_after(&PathBuf::from("/elsewhere")),
            Some(a.path().to_path_buf()),
            "unknown current starts from the top"
        );
    }

    #[test]
    fn next_after_skips_dead_registry_paths() {
        let live_a = tempdir().unwrap();
        let dead = tempdir().unwrap();
        let dead_path = dead.path().to_path_buf();
        drop(dead); // directory no longer exists on disk
        let live_b = tempdir().unwrap();

        let mut r = ProjectsRegistry::default();
        r.register(live_a.path().to_path_buf());
        r.register(dead_path);
        r.register(live_b.path().to_path_buf());

        assert_eq!(
            r.next_after(live_a.path()),
            Some(live_b.path().to_path_buf()),
            "dead entry between the two live ones is skipped"
        );
    }

    #[test]
    fn load_ui_settings_missing_file_is_default() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s, UiSettings::default());
        assert_eq!(s.clipboard_cmd, None);
        assert_eq!(s.osc52_limit, 65536);
        assert_eq!(s.theme, "terminal");
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_ui_settings_parses_configured_values() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "clipboard_cmd = \"xclip\"\nosc52_limit = 1000\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.clipboard_cmd, Some("xclip".to_string()));
        assert_eq!(s.osc52_limit, 1000);
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_ui_settings_wrong_types_degrade_to_defaults() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "clipboard_cmd = 5\nosc52_limit = \"not a number\"\n").unwrap();
        let (s, _warnings) = load_ui_settings(&p);
        assert_eq!(s, UiSettings::default());
    }

    #[test]
    fn load_ui_settings_corrupt_file_is_default() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "not valid toml [[[").unwrap();
        let (s, _warnings) = load_ui_settings(&p);
        assert_eq!(s, UiSettings::default());
    }

    #[test]
    fn load_ui_settings_theme_is_a_raw_name_string() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"gruvbox-dark\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.theme, "gruvbox-dark");
        assert!(
            warnings.is_empty(),
            "name validity is the registry's business at resolve time, not load's: {warnings:?}"
        );
    }

    #[test]
    fn save_ui_theme_sets_the_key_and_preserves_unrelated_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "clipboard_cmd = \"xclip\"\n\n[projects]\nknown = []\n").unwrap();
        save_ui_theme(&p, "catppuccin-mocha").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("theme = \"catppuccin-mocha\""), "{text}");
        assert!(text.contains("clipboard_cmd = \"xclip\""), "{text}");
        assert!(text.contains("[projects]"), "{text}");
        let (s, _) = load_ui_settings(&p);
        assert_eq!(s.theme, "catppuccin-mocha");
        save_ui_theme(&p, "dark").unwrap();
        let (s, _) = load_ui_settings(&p);
        assert_eq!(s.theme, "dark", "overwrites an existing key");
    }

    #[test]
    fn save_ui_theme_creates_a_missing_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("sub").join("config.toml");
        save_ui_theme(&p, "light").unwrap();
        let (s, _) = load_ui_settings(&p);
        assert_eq!(s.theme, "light");
    }

    #[test]
    fn ai_settings_default_and_parse() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let (s, _) = load_ui_settings(&p);
        assert_eq!(s.ai_cmd, "claude -p");
        assert!(!s.ai_confirmed);
        std::fs::write(&p, "ai_cmd = \"my-llm --jq\"\nai_confirmed = true\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.ai_cmd, "my-llm --jq");
        assert!(s.ai_confirmed);
        assert!(warnings.is_empty());
    }

    #[test]
    fn jq_tab_defaults_to_menu_and_parses_cycle() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let (s, _) = load_ui_settings(&p);
        assert_eq!(s.jq_tab, JqTab::Menu);
        std::fs::write(&p, "jq_tab = \"cycle\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.jq_tab, JqTab::Cycle);
        assert!(warnings.is_empty());
        std::fs::write(&p, "jq_tab = \"accept\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.jq_tab, JqTab::Menu, "the older name still works");
        assert!(warnings.is_empty());
        std::fs::write(&p, "jq_tab = \"menu\"\n").unwrap();
        assert_eq!(load_ui_settings(&p).0.jq_tab, JqTab::Menu);
        std::fs::write(&p, "jq_tab = \"popup\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.jq_tab, JqTab::Menu, "a bad value falls back");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("jq_tab"), "{warnings:?}");
    }

    #[test]
    fn save_ui_flag_sets_the_key_and_preserves_unrelated_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"x\"\n\n[projects]\nknown = []\n").unwrap();
        save_ui_flag(&p, "ai_confirmed", true).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("ai_confirmed = true"), "{text}");
        assert!(
            text.contains("theme = \"x\"") && text.contains("[projects]"),
            "{text}"
        );
        assert!(load_ui_settings(&p).0.ai_confirmed);
    }

    #[test]
    fn load_ui_settings_missing_theme_key_defaults_to_terminal() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "clipboard_cmd = \"xclip\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.theme, "terminal");
        assert!(warnings.is_empty());
    }

    #[test]
    fn animations_key_parses_and_defaults_true() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "animations = false\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert!(!s.animations);
        assert!(warnings.is_empty());
        assert!(UiSettings::default().animations);
    }

    #[test]
    fn animation_ms_defaults_without_a_table() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(s.anim_ms.tab_slide, Duration::from_millis(250));
        assert_eq!(s.anim_ms.hover, Duration::from_millis(70));
        assert_eq!(s.anim_ms.focus, Duration::from_millis(90));
        assert_eq!(s.anim_ms.list_travel, Duration::from_millis(100));
        assert_eq!(s.anim_ms.modal_open, Duration::from_millis(100));
        assert_eq!(s.anim_ms.dropdown_open, Duration::from_millis(90));
        assert_eq!(s.anim_ms.pane_collapse, Duration::from_millis(120));
        assert_eq!(s.anim_ms.toast, Duration::from_millis(100));
        assert_eq!(s.anim_ms.send_breathe, Duration::from_millis(700));
        assert!(warnings.is_empty());
    }

    #[test]
    fn animation_ms_table_overrides_one_key_and_defaults_the_rest() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[animation_ms]\ntab_slide = 400\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.anim_ms.tab_slide, Duration::from_millis(400));
        assert_eq!(s.anim_ms.hover, Duration::from_millis(70));
        assert_eq!(s.anim_ms.focus, Duration::from_millis(90));
        assert_eq!(s.anim_ms.list_travel, Duration::from_millis(100));
        assert_eq!(s.anim_ms.modal_open, Duration::from_millis(100));
        assert_eq!(s.anim_ms.dropdown_open, Duration::from_millis(90));
        assert_eq!(s.anim_ms.pane_collapse, Duration::from_millis(120));
        assert_eq!(s.anim_ms.toast, Duration::from_millis(100));
        assert_eq!(s.anim_ms.send_breathe, Duration::from_millis(700));
        assert!(warnings.is_empty());
    }

    #[test]
    fn animation_ms_table_can_override_every_key() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "[animation_ms]\n\
             tab_slide = 1\n\
             hover = 2\n\
             focus = 3\n\
             list_travel = 4\n\
             modal_open = 5\n\
             dropdown_open = 6\n\
             pane_collapse = 7\n\
             toast = 8\n\
             send_breathe = 9\n",
        )
        .unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(
            s.anim_ms,
            AnimDurations {
                tab_slide: Duration::from_millis(1),
                hover: Duration::from_millis(2),
                focus: Duration::from_millis(3),
                list_travel: Duration::from_millis(4),
                modal_open: Duration::from_millis(5),
                dropdown_open: Duration::from_millis(6),
                pane_collapse: Duration::from_millis(7),
                toast: Duration::from_millis(8),
                send_breathe: Duration::from_millis(9),
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn animation_ms_unknown_key_warns_and_is_ignored() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[animation_ms]\nbogus = 5\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bogus"));
    }

    /// A present-but-unusable value (not an integer, e.g. a string) must
    /// warn and default, the same as an unknown key -- not silently default
    /// with no warning.
    #[test]
    fn animation_ms_non_integer_value_warns_and_defaults() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[animation_ms]\nhover = \"fast\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hover"));
    }

    /// A negative value fails the `u64` conversion the same way a
    /// non-integer does, and must warn rather than silently default.
    #[test]
    fn animation_ms_negative_value_warns_and_defaults() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[animation_ms]\nhover = -5\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.anim_ms, AnimDurations::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hover"));
    }

    #[test]
    fn parse_cli_leading_dash_is_usage() {
        assert_eq!(parse_cli(Some("--help".into())), CliParse::Usage);
        assert_eq!(parse_cli(Some("-x".into())), CliParse::Usage);
    }

    #[test]
    fn parse_cli_setup_flag() {
        assert_eq!(parse_cli(Some("--setup".into())), CliParse::Setup);
        // Not a prefix match: anything else dashed is still usage.
        assert_eq!(parse_cli(Some("--setup-x".into())), CliParse::Usage);
    }

    #[test]
    fn parse_cli_keydump_flag_with_optional_bitmask() {
        assert_eq!(
            parse_cli(Some("--keydump".into())),
            CliParse::Keydump { flags: None }
        );
        assert_eq!(
            parse_cli(Some("--keydump=0".into())),
            CliParse::Keydump { flags: Some(0) }
        );
        assert_eq!(
            parse_cli(Some("--keydump=5".into())),
            CliParse::Keydump { flags: Some(5) }
        );
        // A malformed bitmask is usage, not a silent default.
        assert_eq!(parse_cli(Some("--keydump=x".into())), CliParse::Usage);
        assert_eq!(parse_cli(Some("--keydump=".into())), CliParse::Usage);
        assert_eq!(parse_cli(Some("--keydump-x".into())), CliParse::Usage);
    }

    #[test]
    fn parse_cli_normal_path_and_none() {
        assert_eq!(
            parse_cli(Some("/abs/x".into())),
            CliParse::Root(Some(PathBuf::from("/abs/x")))
        );
        assert_eq!(parse_cli(None), CliParse::Root(None));
    }

    #[test]
    fn tilde_expansion() {
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert_eq!(expand_tilde("~/x/y"), home.join("x/y"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/abs/x"), PathBuf::from("/abs/x"));
        assert_eq!(expand_tilde("rel"), PathBuf::from("rel"));
    }
}
