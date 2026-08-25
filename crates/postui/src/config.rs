//! Global project registry stored in the app's `config.toml`, under the
//! `[projects]` table: known project paths (cycle order), an optional
//! configured root directory, and the last-used project.

use crate::theme::ThemeChoice;
use std::path::{Path, PathBuf};

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
        if self.known.len() < 2 {
            return None;
        }
        let start = self
            .known
            .iter()
            .position(|p| p == current)
            .map(|i| (i + 1) % self.known.len())
            .unwrap_or(0);
        for step in 0..self.known.len() {
            let idx = (start + step) % self.known.len();
            let candidate = &self.known[idx];
            if candidate != current && candidate.is_dir() {
                return Some(candidate.clone());
            }
        }
        None
    }
}

/// Mouse-first-GUI UI settings stored at the top level of `config.toml`:
/// the tiered clipboard's optional external command and the OSC 52 size
/// threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSettings {
    pub clipboard_cmd: Option<String>,
    pub osc52_limit: usize,
    /// Which palette source to seed the theme from; see [`ThemeChoice`].
    pub theme: ThemeChoice,
    /// Whether eased transitions (tab underline, hover, modal open, ...)
    /// play, or every animated value jumps straight to its target.
    pub animations: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            clipboard_cmd: None,
            osc52_limit: 65536,
            theme: ThemeChoice::default(),
            animations: true,
        }
    }
}

/// Reads the top-level `clipboard_cmd` (string), `osc52_limit` (integer),
/// `theme` (string), and `animations` (bool) keys from `config.toml`. Never
/// errors: a missing file, corrupt TOML, or a mistyped key degrades that
/// piece to its default. An unrecognized `theme` value degrades to
/// [`ThemeChoice::Terminal`] and is reported in the returned warnings, for
/// the caller to surface as a startup toast (the same pattern
/// `ProjectContext::open` uses).
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
        settings.theme = ThemeChoice::parse(raw);
        if !matches!(raw, "terminal" | "dark" | "light") {
            warnings.push(format!(
                "unknown theme {raw:?} in config.toml; using terminal"
            ));
        }
    }
    if let Some(b) = value.get("animations").and_then(|v| v.as_bool()) {
        settings.animations = b;
    }

    (settings, warnings)
}

/// The path to the global config file: `<config dir>/config.toml`.
pub fn config_file_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", postui_core::APP_NAME)
        .map(|d| d.config_dir().join("config.toml"))
}

/// The path to the mouse-first-GUI UI-state file (currently just palette
/// usage stats): `<config dir>/ui.toml`.
pub fn ui_file_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", postui_core::APP_NAME)
        .map(|d| d.config_dir().join("ui.toml"))
}

/// Expands a leading `~/` to the home directory. Paths not starting with
/// `~/` are returned unchanged.
pub fn expand_tilde(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => match directories::BaseDirs::new() {
            Some(dirs) => dirs.home_dir().join(rest),
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
}

/// Parses `postui`'s single optional argument. Any value starting with `-`
/// (e.g. `--help`, `-x`) is treated as a request for usage rather than a
/// project directory, since no real path starts with a dash without `./`.
pub fn parse_cli(arg: Option<String>) -> CliParse {
    match arg {
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
        assert_eq!(s.theme, ThemeChoice::Terminal);
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
    fn load_ui_settings_theme_round_trips() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"light\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.theme, ThemeChoice::Light);
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_ui_settings_missing_theme_key_defaults_to_terminal() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "clipboard_cmd = \"xclip\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.theme, ThemeChoice::Terminal);
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_ui_settings_unknown_theme_value_falls_back_to_terminal_with_warning() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "theme = \"sepia\"\n").unwrap();
        let (s, warnings) = load_ui_settings(&p);
        assert_eq!(s.theme, ThemeChoice::Terminal);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("sepia"));
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
    fn parse_cli_leading_dash_is_usage() {
        assert_eq!(parse_cli(Some("--help".into())), CliParse::Usage);
        assert_eq!(parse_cli(Some("-x".into())), CliParse::Usage);
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
        assert_eq!(expand_tilde("/abs/x"), PathBuf::from("/abs/x"));
        assert_eq!(expand_tilde("rel"), PathBuf::from("rel"));
    }
}
