//! Global project registry stored in the app's `config.toml`, under the
//! `[projects]` table: known project paths (cycle order), an optional
//! configured root directory, and the last-used project.

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
        if !self.known.contains(&path) {
            self.known.push(path.clone());
        }
        self.last = Some(path);
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

    /// The next project after `current` in cycle order, wrapping. `None`
    /// when fewer than two projects are known. An unknown `current` starts
    /// the cycle from the top.
    pub fn next_after(&self, current: &Path) -> Option<PathBuf> {
        if self.known.len() < 2 {
            return None;
        }
        let idx = self
            .known
            .iter()
            .position(|p| p == current)
            .map(|i| (i + 1) % self.known.len())
            .unwrap_or(0);
        Some(self.known[idx].clone())
    }
}

/// The path to the global config file: `<config dir>/config.toml`.
pub fn config_file_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", postui_core::APP_NAME)
        .map(|d| d.config_dir().join("config.toml"))
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
        r.register(PathBuf::from("/tmp/a"));
        r.register(PathBuf::from("/tmp/b"));
        r.register(PathBuf::from("/tmp/c"));
        assert_eq!(
            r.next_after(&PathBuf::from("/tmp/b")),
            Some(PathBuf::from("/tmp/c"))
        );
        assert_eq!(
            r.next_after(&PathBuf::from("/tmp/c")),
            Some(PathBuf::from("/tmp/a")),
            "wraps"
        );
        assert_eq!(
            r.next_after(&PathBuf::from("/elsewhere")),
            Some(PathBuf::from("/tmp/a")),
            "unknown current starts from the top"
        );
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
