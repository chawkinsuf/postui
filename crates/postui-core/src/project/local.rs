//! `.local/state.toml`: the machine-owned UI state (active space and
//! environment, per-space open request, expanded folders, selections,
//! split). Written immediately on every change, never journaled.

use super::*;

impl Project {
    /// Writes `.local/state.toml` from memory. Best effort at the call
    /// sites that were best effort before; callers that want the error
    /// get it.
    pub(crate) fn persist_local(&mut self) -> Result<(), Error> {
        let text = self.local_state_text();
        self.disk.write(&rel(STATE_TOML)?, &text)?;
        Ok(())
    }

    /// `.local/state.toml` as it should read right now. Shared by the
    /// immediate and the journaled writer so the two cannot drift.
    pub(crate) fn local_state_text(&self) -> String {
        let state = LocalState {
            environment: self.active_env.clone(),
            open_request: self.local.open_request.clone(),
            main_split: self.local.main_split.clone(),
            expanded: self.local.expanded.iter().cloned().collect(),
            selections: self.local.selections.clone(),
            shared_selections: self.local.shared_selections.clone(),
            space: Some(self.local.active_space.clone()),
            space_open: self.local.space_open.clone(),
        };
        toml::to_string(&state).expect("LocalState always serializes")
    }

    /// The editor told the project which request it has open. Persisted
    /// at once (best effort), as every setter here is.
    pub fn set_open_request(&mut self, slug: Option<&str>) {
        self.local.open_request = slug.map(str::to_string);
        let _ = self.persist_local();
    }

    /// Remembers `slug` as the active space's open request (`None`
    /// clears). Only slugs inside the active space are kept.
    pub fn record_space_open(&mut self, slug: Option<&str>) {
        let space = self.local.active_space.clone();
        match slug {
            Some(s) if crate::storage::space_of(s) == Some(space.as_str()) => {
                self.local.space_open.insert(space, s.to_string());
            }
            _ => {
                self.local.space_open.shift_remove(&space);
            }
        }
        let _ = self.persist_local();
    }

    pub fn space_open_for(&self, space: &str) -> Option<String> {
        self.local.space_open.get(space).cloned()
    }

    pub fn set_active_space(&mut self, name: &str) -> bool {
        if !self.spaces.iter().any(|s| s == name) {
            return false;
        }
        self.local.active_space = name.to_string();
        let _ = self.persist_local();
        true
    }

    pub fn set_expanded(&mut self, expanded: BTreeSet<String>) {
        self.local.expanded = expanded;
        let _ = self.persist_local();
    }

    pub fn set_main_split(&mut self, token: Option<String>) {
        self.local.main_split = token;
        let _ = self.persist_local();
    }

    pub fn selections_for(&self, env: &str) -> &IndexMap<String, String> {
        static EMPTY: std::sync::OnceLock<IndexMap<String, String>> = std::sync::OnceLock::new();
        self.local
            .selections
            .get(env)
            .unwrap_or_else(|| EMPTY.get_or_init(IndexMap::new))
    }

    pub fn set_selection(&mut self, name: &str, key: &str) {
        let env = self.env_key();
        self.set_selection_for(&env, name, key);
    }

    /// Records a selection for `env` (a shared selector's pick lands in
    /// the global table whichever env asked), persists, and re-resolves
    /// when the active environment is affected.
    pub fn set_selection_for(&mut self, env: &str, name: &str, key: &str) {
        if self.model.selectors.get(name).is_some_and(|d| d.shared) {
            self.local
                .shared_selections
                .insert(name.to_string(), key.to_string());
            let _ = self.persist_local();
            self.refresh_resolved();
            return;
        }
        self.local
            .selections
            .entry(env.to_string())
            .or_default()
            .insert(name.to_string(), key.to_string());
        let _ = self.persist_local();
        if self.env_key() == env {
            self.refresh_resolved();
        }
    }

    pub fn clear_selection_for(&mut self, env: &str, name: &str) {
        let removed_shared = self.local.shared_selections.shift_remove(name).is_some();
        let removed_env = self
            .local
            .selections
            .get_mut(env)
            .and_then(|sel| sel.shift_remove(name))
            .is_some();
        if !removed_shared && !removed_env {
            return;
        }
        let _ = self.persist_local();
        if removed_shared || self.env_key() == env {
            self.refresh_resolved();
        }
    }

    /// Drops what local memory holds about `name`: its open request and
    /// expanded folders. Memory only; the caller persists.
    pub(crate) fn forget_space_local(&mut self, name: &str) {
        self.local.space_open.shift_remove(name);
        let prefix = format!("{name}/");
        self.local
            .expanded
            .retain(|p| !p.starts_with(&prefix) && p != name);
    }

    /// Re-keys local memory after a space rename. Memory only.
    pub(crate) fn rename_space_local(&mut self, from: &str, to: &str) {
        let mut state = LocalState {
            space: Some(self.local.active_space.clone()),
            open_request: self.local.open_request.clone(),
            expanded: self.local.expanded.iter().cloned().collect(),
            space_open: self.local.space_open.clone(),
            ..LocalState::default()
        };
        state.rename_space(from, to);
        self.local.active_space = state.space.unwrap_or_default();
        self.local.open_request = state.open_request;
        self.local.expanded = state.expanded.into_iter().collect();
        self.local.space_open = state.space_open;
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;

    #[test]
    fn persist_writes_every_field_and_open_restores_them() {
        let (dir, mut p) = fixture();
        p.set_active_space("auth");
        p.set_open_request(Some("auth/login"));
        p.record_space_open(Some("auth/login"));
        p.set_expanded(["auth/sub".to_string()].into_iter().collect());
        p.set_main_split(Some("60".into()));
        p.persist_local().unwrap();
        let text = read(&dir, ".local/state.toml").unwrap();
        assert!(text.contains("space = \"auth\""), "{text}");
        assert!(text.contains("environment = \"dev\""), "{text}");
        let (p2, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(p2.local().active_space, "auth");
        assert_eq!(p2.local().open_request.as_deref(), Some("auth/login"));
        assert_eq!(p2.local().space_open.get("auth").map(String::as_str), Some("auth/login"));
        assert!(p2.local().expanded.contains("auth/sub"));
        assert_eq!(p2.local().main_split.as_deref(), Some("60"));
    }

    /// The app's setters are fire-and-forget: it never calls a separate
    /// save, so each one must land in `.local/state.toml` at once.
    #[test]
    fn every_local_setter_writes_state_toml_at_once() {
        let (dir, mut p) = fixture();
        assert!(read(&dir, ".local/state.toml").is_none(), "nothing written yet");
        p.set_main_split(Some("60".into()));
        assert!(read(&dir, ".local/state.toml").unwrap().contains("main_split = \"60\""));
        p.set_expanded(["main/deep".to_string()].into_iter().collect());
        assert!(read(&dir, ".local/state.toml").unwrap().contains("main/deep"));
        p.set_open_request(Some("main/ping"));
        assert!(read(&dir, ".local/state.toml").unwrap().contains("open_request = \"main/ping\""));
        p.record_space_open(Some("main/ping"));
        assert!(read(&dir, ".local/state.toml").unwrap().contains("[space_open]"));
        assert!(p.set_active_space("auth"));
        assert!(read(&dir, ".local/state.toml").unwrap().contains("space = \"auth\""));
        // A refused space change writes nothing new.
        let before = read(&dir, ".local/state.toml").unwrap();
        assert!(!p.set_active_space("ghost"));
        assert_eq!(read(&dir, ".local/state.toml").as_deref(), Some(before.as_str()));
    }

    #[test]
    fn set_selection_persists_re_resolves_and_shared_picks_are_global() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.region]\nfields = [\"host\"]\n\n[selectors.locale]\nfields = [\"lang\"]\nshared = true\n\n[options.locale.en]\nlang = \"en\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/dev.toml"),
            "[options.region.east]\nhost = \"east.local\"\n",
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.set_selection("region", "east");
        assert_eq!(p.resolved().values.get("host").map(String::as_str), Some("east.local"));
        p.set_selection_for("qa", "locale", "en");
        assert_eq!(p.local().shared_selections.get("locale").map(String::as_str), Some("en"));
        assert_eq!(p.resolved().values.get("lang").map(String::as_str), Some("en"));
        let text = read(&dir, ".local/state.toml").unwrap();
        assert!(text.contains("east"), "{text}");
        p.clear_selection_for("dev", "region");
        assert!(p.selections_for("dev").get("region").is_none());
    }

    #[test]
    fn a_selection_for_a_non_active_env_does_not_touch_resolved() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.region]\nfields = [\"host\"]\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/qa.toml"), "[options.region.west]\nhost = \"w\"\n").unwrap();
        std::fs::write(dir.path().join("environments/dev.toml"), "[options.region.east]\nhost = \"e\"\n").unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.set_selection_for("qa", "region", "west");
        assert!(p.resolved().values.get("host").is_none());
        assert_eq!(p.selections_for("qa").get("region").map(String::as_str), Some("west"));
    }

    #[test]
    fn rename_and_forget_re_key_the_local_memory() {
        let (_d, mut p) = fixture();
        p.set_active_space("auth");
        p.record_space_open(Some("auth/login"));
        p.set_expanded(["auth/x".to_string(), "main/y".to_string()].into_iter().collect());
        p.rename_space_local("auth", "login");
        assert_eq!(p.local().active_space, "login");
        assert_eq!(p.local().space_open.get("login").map(String::as_str), Some("login/login"));
        assert!(p.local().expanded.contains("login/x"));
        p.forget_space_local("login");
        assert!(p.local().space_open.get("login").is_none());
        assert!(!p.local().expanded.iter().any(|e| e.starts_with("login")));
        assert!(p.local().expanded.contains("main/y"));
    }
}
