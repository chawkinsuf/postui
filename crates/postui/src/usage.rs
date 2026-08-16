//! Palette command usage stats ("frecency"): how often and how recently
//! each command id has been run, persisted to `ui.toml`'s `[palette.usage]`
//! table so the command palette can list frequently/recently used commands
//! first. See spec §6.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The count at which an id's usage count (and every other id's, in the
/// same store) gets halved, so long-lived stores don't grow without bound.
const HALVING_THRESHOLD: u32 = 1000;

/// Per-command-id usage: how many times it's been run, and when it was last
/// run (unix seconds).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageStore {
    entries: HashMap<String, (u32, i64)>,
}

impl UsageStore {
    /// Loads the store from `path`'s `[palette.usage]` table. Never errors:
    /// a missing file, corrupt TOML, or a malformed entry degrades to an
    /// empty store (or just drops that one malformed entry).
    pub fn load_from(path: &Path) -> Self {
        let mut store = Self::default();
        let Ok(contents) = std::fs::read_to_string(path) else {
            return store;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&contents) else {
            return store;
        };
        let Some(usage) = value
            .get("palette")
            .and_then(|v| v.get("usage"))
            .and_then(|v| v.as_table())
        else {
            return store;
        };
        for (id, entry) in usage {
            let Some(table) = entry.as_table() else {
                continue;
            };
            let (Some(count), Some(last_used)) = (
                table.get("count").and_then(|v| v.as_integer()),
                table.get("last_used").and_then(|v| v.as_integer()),
            ) else {
                continue;
            };
            let Ok(count) = u32::try_from(count) else {
                continue;
            };
            store.entries.insert(id.clone(), (count, last_used));
        }
        store
    }

    /// Writes the store to `path`'s `[palette.usage]` table, round-tripping
    /// through `toml_edit::DocumentMut` so unrelated keys are preserved.
    /// Creates the parent directory if needed.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = existing.parse().unwrap_or_default();

        let mut usage_table = toml_edit::Table::new();
        for (id, (count, last_used)) in &self.entries {
            let mut entry = toml_edit::InlineTable::new();
            entry.insert("count", (*count as i64).into());
            entry.insert("last_used", (*last_used).into());
            usage_table.insert(
                id,
                toml_edit::Item::Value(toml_edit::Value::InlineTable(entry)),
            );
        }
        let mut palette_table = toml_edit::Table::new();
        palette_table.insert("usage", toml_edit::Item::Table(usage_table));
        doc.insert("palette", toml_edit::Item::Table(palette_table));

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, doc.to_string())?;
        Ok(())
    }

    /// Bumps `id`'s count and sets its last-used timestamp to `now_secs`.
    /// If that bump brings any id's count to `HALVING_THRESHOLD` or beyond,
    /// every id's count in the store is halved (integer division), keeping
    /// relative weighting while capping unbounded growth.
    pub fn record(&mut self, id: &str, now_secs: i64) {
        let entry = self.entries.entry(id.to_string()).or_insert((0, now_secs));
        entry.0 += 1;
        entry.1 = now_secs;

        if self
            .entries
            .values()
            .any(|(count, _)| *count >= HALVING_THRESHOLD)
        {
            for (count, _) in self.entries.values_mut() {
                *count /= 2;
            }
        }
    }

    /// `count × 0.5^(age_days / 30)` — recently/frequently used ids score
    /// higher; unknown ids score `0.0`.
    pub fn score(&self, id: &str, now_secs: i64) -> f64 {
        let Some(&(count, last_used)) = self.entries.get(id) else {
            return 0.0;
        };
        let age_days = (now_secs - last_used).max(0) as f64 / 86400.0;
        count as f64 * 0.5f64.powf(age_days / 30.0)
    }
}

/// The current time as unix seconds, `0` if the system clock is somehow
/// before the epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const DAY: i64 = 86_400;

    #[test]
    fn unknown_id_scores_zero() {
        let store = UsageStore::default();
        assert_eq!(store.score("quit", 1000), 0.0);
    }

    #[test]
    fn equal_counts_recent_beats_old() {
        let mut store = UsageStore::default();
        store.record("recent", 1000);
        store.record("old", 1000 - 30 * DAY);
        assert!(store.score("recent", 1000) > store.score("old", 1000));
    }

    #[test]
    fn equal_age_higher_count_wins() {
        let mut store = UsageStore::default();
        store.record("a", 1000);
        store.record("b", 1000);
        store.record("b", 1000);
        assert!(store.score("b", 1000) > store.score("a", 1000));
    }

    #[test]
    fn reaching_1000_halves_every_count() {
        let mut store = UsageStore::default();
        store.record("bystander", 0);
        for i in 0..999 {
            store.record("popular", i);
        }
        // popular is now at 999; bystander at 1.
        store.record("popular", 999); // bumps popular to 1000 -> halve all.
        assert_eq!(store.entries.get("popular").unwrap().0, 500);
        assert_eq!(store.entries.get("bystander").unwrap().0, 0);
    }

    #[test]
    fn save_and_load_round_trip_preserves_scores() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ui.toml");

        let mut store = UsageStore::default();
        store.record("quit", 1000);
        store.record("send", 2000);
        store.save_to(&path).unwrap();

        let loaded = UsageStore::load_from(&path);
        assert_eq!(loaded.score("quit", 5000), store.score("quit", 5000));
        assert_eq!(loaded.score("send", 5000), store.score("send", 5000));
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let store = UsageStore::load_from(&path);
        assert_eq!(store.entries.len(), 0);
    }

    #[test]
    fn corrupt_file_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        std::fs::write(&path, "not valid { toml").unwrap();
        let store = UsageStore::load_from(&path);
        assert_eq!(store.entries.len(), 0);
    }

    #[test]
    fn save_preserves_unrelated_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        std::fs::write(&path, "some_other_key = \"kept\"\n").unwrap();

        let mut store = UsageStore::default();
        store.record("quit", 1000);
        store.save_to(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("some_other_key"));
    }
}
