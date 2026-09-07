//! Environments: `environments/<slug>.toml`, their `[environment.<slug>]`
//! tables, and the secrets and selections keyed by them.

use super::*;

impl Project {
    pub fn environment_slug_for(&self, display: &str, exclude: Option<&str>) -> String {
        meta::unique_slug_among(
            meta::Kind::Environment,
            display,
            |slug| env_rel(slug).map(|p| self.disk.exists(&p)).unwrap_or(false),
            exclude,
        )
    }

    /// Re-lists from disk; a listing that fails keeps the list we have
    /// rather than emptying it.
    pub(crate) fn refresh_environments(&mut self) {
        if let Ok(environments) = Self::list_environments(&mut self.disk) {
            self.environments = environments;
        }
    }

    /// Writes `.local/secrets.toml` from memory as a journaled text op.
    pub(crate) fn write_secrets_journaled(&mut self) -> Result<(), Error> {
        let text = toml::to_string(&self.secrets).expect("secrets always serialize");
        self.fs_write_text(&rel(SECRETS_TOML)?, Some(&text))
    }

    /// Records a secret for `env`, writes the secrets file, re-resolves
    /// when `env` is active. The error never carries the value.
    pub fn set_secret_for(&mut self, env: &str, name: &str, value: String) -> Result<(), Error> {
        let env = env.to_string();
        let name = name.to_string();
        self.transaction("set secret", EntryMeta::default(), |p| {
            p.secrets.entry(env.clone()).or_default().insert(name.clone(), value);
            p.write_secrets_journaled()
        })?;
        if self.env_key() == env {
            self.refresh_resolved();
        }
        Ok(())
    }

    pub fn set_secret(&mut self, name: &str, value: String) -> Result<(), Error> {
        let env = self.env_key();
        self.set_secret_for(&env, name, value)
    }

    pub fn remove_secret_for(&mut self, env: &str, name: &str) -> Result<(), Error> {
        if !self.secrets.get(env).is_some_and(|m| m.contains_key(name)) {
            return Ok(());
        }
        let env = env.to_string();
        let name = name.to_string();
        self.transaction("remove secret", EntryMeta::default(), |p| {
            p.secrets.get_mut(&env).map(|m| m.shift_remove(&name));
            p.write_secrets_journaled()
        })?;
        if self.env_key() == env {
            self.refresh_resolved();
        }
        Ok(())
    }

    pub fn create_environment(&mut self, display: &str) -> Result<String, Error> {
        let display = meta::display_name_of(display)?;
        let meta = self.meta.clone();
        let existing = self.environments.clone();
        if meta::display_taken(&display, &existing, |s| meta::env_display(&meta, s), None) {
            return Err(Error::AlreadyExists(display));
        }
        let slug = self.environment_slug_for(&display, None);
        // Creating an environment switches to it, as today's app does;
        // the transition is recorded so undo puts the old one back.
        let entry_meta = EntryMeta {
            active_env: Some((self.active_env.clone(), Some(slug.clone()))),
            ..EntryMeta::default()
        };
        self.transaction("create environment", entry_meta, |p| {
            p.fs_create_file(&env_rel(&slug)?, "")?;
            p.edit_project_toml(|doc| meta::set_item_name(doc, meta::Kind::Environment, &slug, &display))?;
            p.refresh_environments();
            p.load_active_env(&slug)?;
            p.refresh_resolved();
            p.persist_local_journaled()?;
            Ok(())
        })?;
        Ok(slug)
    }

    pub fn rename_environment(&mut self, from: &str, display: &str) -> Result<String, Error> {
        let display = meta::display_name_of(display)?;
        let from_path = env_rel(from)?;
        if !self.disk.is_file(&from_path) {
            return Err(Error::NotFound(from.to_string()));
        }
        let meta = self.meta.clone();
        let existing = self.environments.clone();
        if meta::display_taken(&display, &existing, |s| meta::env_display(&meta, s), Some(from)) {
            return Err(Error::AlreadyExists(display));
        }
        let to = self.environment_slug_for(&display, Some(from));
        let from = from.to_string();
        let was_active = self.active_env.as_deref() == Some(from.as_str());
        let entry_meta = EntryMeta {
            active_env: (to != from && was_active).then(|| (Some(from.clone()), Some(to.clone()))),
            ..EntryMeta::default()
        };
        self.transaction("rename environment", entry_meta, |p| {
            if to != from {
                p.fs_rename(&from_path, &env_rel(&to)?)?;
            }
            p.edit_project_toml(|doc| {
                meta::move_item_table(doc, meta::Kind::Environment, &from, &to);
                meta::set_item_name(doc, meta::Kind::Environment, &to, &display);
            })?;
            if to != from {
                if let Some(sel) = p.local.selections.shift_remove(&from) {
                    p.local.selections.insert(to.clone(), sel);
                }
                if let Some(sec) = p.secrets.shift_remove(&from) {
                    p.secrets.insert(to.clone(), sec);
                    p.write_secrets_journaled()?;
                }
                if was_active {
                    p.active_env = Some(to.clone());
                }
                p.persist_local_journaled()?;
            }
            p.refresh_environments();
            Ok(())
        })?;
        Ok(to)
    }

    /// Trashes the file, drops the table, its secrets and selections; the
    /// active environment falls to the first remaining one.
    pub fn delete_environment(&mut self, name: &str) -> Result<(), Error> {
        let path = env_rel(name)?;
        if !self.disk.is_file(&path) {
            return Err(Error::NotFound(name.to_string()));
        }
        let name = name.to_string();
        let was_active = self.active_env.as_deref() == Some(name.as_str());
        let fallback = self.environments.iter().find(|e| **e != name).cloned();
        let entry_meta = EntryMeta {
            active_env: was_active.then(|| (Some(name.clone()), fallback.clone())),
            ..EntryMeta::default()
        };
        self.transaction("delete environment", entry_meta, |p| {
            p.edit_project_toml(|doc| meta::remove_item_table(doc, meta::Kind::Environment, &name))?;
            p.fs_trash(&path)?;
            p.local.selections.shift_remove(&name);
            if p.secrets.shift_remove(&name).is_some() {
                p.write_secrets_journaled()?;
            }
            p.refresh_environments();
            if was_active {
                p.active_env = None;
                p.env_data = EnvData::default();
                if let Some(env) = fallback.clone() {
                    // A broken fallback file leaves no env active, as
                    // today's `set_env` warning path does.
                    let _ = p.load_active_env(&env);
                }
            }
            p.persist_local_journaled()?;
            p.refresh_resolved();
            Ok(())
        })
    }

    pub fn set_env_tls(&mut self, slug: &str, policy: Option<TlsPolicy>) -> Result<(), Error> {
        let slug = slug.to_string();
        self.transaction("set tls policy", EntryMeta::default(), |p| {
            p.edit_project_toml(|doc| match policy {
                Some(pol) => meta::set_item_key(doc, meta::Kind::Environment, &slug, "tls", pol.as_str()),
                None => {
                    if let Some(it) = doc
                        .get_mut(meta::Kind::Environment.table())
                        .and_then(|i| i.as_table_mut())
                        .and_then(|t| t.get_mut(&slug))
                        .and_then(|i| i.as_table_mut())
                    {
                        it.remove("tls");
                    }
                }
            })
        })
    }

    /// Any environment's data, read now and validated against the model.
    pub fn load_environment(&mut self, name: &str) -> Result<EnvData, Error> {
        let path = env_rel(name)?;
        let text = self.disk.read(&path)?.ok_or_else(|| Error::NotFound(name.to_string()))?;
        let data = varmodel::parse_environment(&text).map_err(|e| Error::Parse { file: path.to_string(), error: e.to_string() })?;
        varmodel::validate_env(&self.model, &data).map_err(|e| Error::Parse { file: path.to_string(), error: e.to_string() })?;
        Ok(data)
    }

    pub(super) fn load_active_env(&mut self, env: &str) -> Result<(), Error> {
        let data = self.load_environment(env)?;
        self.env_data = data;
        self.active_env = Some(env.to_string());
        Ok(())
    }

    /// Switches the active environment (re-reading its file) and persists
    /// the choice. A broken or missing file keeps the previous one with a
    /// warning. Not journaled (navigation, as today).
    pub fn set_active_env(&mut self, env: Option<String>) -> Vec<Warning> {
        let mut warnings = Vec::new();
        match env {
            None => {
                self.active_env = None;
                self.env_data = EnvData::default();
            }
            Some(name) => {
                if let Err(e) = self.load_active_env(&name) {
                    warnings.push(format!("could not load environment {name:?}: {e}"));
                }
            }
        }
        self.refresh_resolved();
        let _ = self.persist_local();
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;

    #[test]
    fn create_writes_an_empty_file_records_the_name_and_refuses_duplicates() {
        let (dir, mut p) = fixture();
        let slug = p.create_environment("Staging 2").unwrap();
        assert_eq!(slug, "staging-2");
        assert_eq!(read(&dir, "environments/staging-2.toml").as_deref(), Some(""));
        assert_eq!(p.environments(), ["dev", "qa", "staging-2"]);
        assert_eq!(p.env_name("staging-2"), "Staging 2");
        assert!(matches!(p.create_environment("dev"), Err(Error::AlreadyExists(_))));
        assert_eq!(p.journal_len(), 1);
    }

    #[test]
    fn create_activates_the_new_environment_and_undo_puts_the_old_one_back() {
        let (dir, mut p) = fixture();
        assert_eq!(p.active_env(), Some("dev"));
        let slug = p.create_environment("QA 2").unwrap();
        assert_eq!(slug, "qa-2");
        assert_eq!(p.active_env(), Some("qa-2"), "a new environment is switched to");
        assert!(read(&dir, ".local/state.toml").unwrap().contains("environment = \"qa-2\""));
        let (_id, label) = p.last_entry().unwrap();
        assert_eq!(label, "create environment");
        let u = p.undo().unwrap().unwrap();
        assert_eq!(u.meta.active_env, Some((Some("dev".into()), Some("qa-2".into()))));
        assert_eq!(p.active_env(), Some("dev"));
        assert!(!dir.path().join("environments/qa-2.toml").exists());
        p.redo().unwrap().unwrap();
        assert_eq!(p.active_env(), Some("qa-2"));
        assert_eq!(p.environments(), ["dev", "qa", "qa-2"]);
    }

    #[test]
    fn rename_moves_the_file_the_table_the_secrets_and_the_selections() {
        let (dir, mut p) = fixture();
        p.set_secret_for("dev", "token", "s3cret".into()).unwrap();
        p.set_selection_for("dev", "region", "east");
        let to = p.rename_environment("dev", "Development").unwrap();
        assert_eq!(to, "development");
        assert!(dir.path().join("environments/development.toml").is_file());
        assert!(!dir.path().join("environments/dev.toml").exists());
        assert_eq!(p.active_env(), Some("development"));
        assert_eq!(p.secrets().get("development").and_then(|m| m.get("token")).map(String::as_str), Some("s3cret"));
        assert!(p.secrets().get("dev").is_none());
        assert_eq!(p.selections_for("development").get("region").map(String::as_str), Some("east"));
        assert!(read(&dir, ".local/secrets.toml").unwrap().contains("[development]"));
        assert!(read(&dir, "project.toml").unwrap().contains("[environment.development]"));
        assert_eq!(p.journal_len(), 2);
    }

    #[test]
    fn delete_trashes_drops_everything_keyed_by_it_and_falls_to_the_first_remaining() {
        let (dir, mut p) = fixture();
        p.set_secret_for("dev", "token", "x".into()).unwrap();
        p.delete_environment("dev").unwrap();
        assert!(dir.path().join(".local/trash/1/environments/dev.toml").is_file());
        assert_eq!(p.environments(), ["qa"]);
        assert_eq!(p.active_env(), Some("qa"));
        assert!(p.secrets().get("dev").is_none());
        assert!(!read(&dir, "project.toml").unwrap().contains("[environment.dev]"));
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.meta.active_env, Some((Some("dev".into()), Some("qa".into()))));
    }

    #[test]
    fn set_active_env_re_reads_and_a_broken_file_keeps_the_previous() {
        let (dir, mut p) = fixture();
        let w = p.set_active_env(Some("qa".into()));
        assert!(w.is_empty());
        assert_eq!(p.active_env(), Some("qa"));
        assert_eq!(p.resolved().values.get("host").map(String::as_str), Some("localhost"));
        std::fs::write(dir.path().join("environments/dev.toml"), "host = [\n").unwrap();
        let w = p.set_active_env(Some("dev".into()));
        assert_eq!(w.len(), 1);
        assert_eq!(p.active_env(), Some("qa"));
        assert!(read(&dir, ".local/state.toml").unwrap().contains("environment = \"qa\""));
    }

    #[test]
    fn a_failed_rename_leaves_memory_matching_disk() {
        let (dir, mut p) = fixture();
        // `.local` as a regular file makes `create_dir_all(".local")` fail,
        // so the write of `.local/state.toml` inside the rename's
        // transaction fails after the disk rename and table edit already
        // happened — exercising the rollback.
        std::fs::write(dir.path().join(".local"), "not a directory").unwrap();
        let r = p.rename_environment("dev", "Development");
        assert!(r.is_err());
        assert_eq!(p.environments(), ["dev", "qa"]);
        assert_eq!(p.active_env(), Some("dev"));
        assert_eq!(p.env_name("dev"), "Dev");
        assert!(dir.path().join("environments/dev.toml").is_file());
        assert!(!dir.path().join("environments/development.toml").exists());
        assert!(p.secrets().get("development").is_none());
    }

    /// Ported from the app's `ProjectContext` tests
    /// (`set_secret_writes_secrets_file_and_resolves`,
    /// `set_secret_resolves_immediately_with_no_active_environment`): the
    /// value reaches `.local/secrets.toml` and `resolved` at once, in the
    /// active environment and in the no-environment slot alike.
    #[test]
    fn set_secret_writes_the_file_and_resolves_with_and_without_an_environment() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("variables.toml"), "[token]\nsecret = true\n").unwrap();
        p.invalidate_stamps();
        p.poll();

        p.set_secret("token", "s3cret".into()).unwrap();
        assert!(read(&dir, ".local/secrets.toml").unwrap().contains("s3cret"));
        assert_eq!(p.resolved().values.get("token").map(String::as_str), Some("s3cret"));

        // With no environment active, the secret lives under the shared
        // `""` key and still resolves immediately — the send-time secret
        // prompt depends on it.
        p.set_active_env(None);
        assert!(p.resolved().values.get("token").is_none());
        p.set_secret("token", "no-env".into()).unwrap();
        assert_eq!(p.resolved().values.get("token").map(String::as_str), Some("no-env"));
    }

    /// Ported from `prepare_context_carries_the_active_environment_tls_force`.
    #[test]
    fn prepare_context_carries_the_active_environments_tls_force() {
        let (_dir, mut p) = fixture();
        assert_eq!(p.prepare_context().tls_override, None, "no force: per request");
        p.set_env_tls("dev", Some(TlsPolicy::Verify)).unwrap();
        assert_eq!(p.prepare_context().tls_override, Some(TlsPolicy::Verify));
        p.set_active_env(Some("qa".into()));
        assert_eq!(p.prepare_context().tls_override, None, "qa has no force");
    }

    #[test]
    fn a_failed_secret_write_leaves_memory_unchanged() {
        let (dir, mut p) = fixture();
        // A regular file at `.local` makes `create_dir_all(".local")` fail,
        // so the secrets write inside the transaction fails.
        std::fs::write(dir.path().join(".local"), "not a directory").unwrap();
        let r = p.set_secret_for("dev", "token", "x".into());
        assert!(r.is_err());
        assert!(p.secrets().get("dev").is_none());
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn set_env_tls_writes_and_clears_the_key_keeping_the_name() {
        let (dir, mut p) = fixture();
        p.set_env_tls("dev", Some(TlsPolicy::Insecure)).unwrap();
        assert_eq!(p.env_tls(), Some(TlsPolicy::Insecure));
        assert!(read(&dir, "project.toml").unwrap().contains("tls = \"insecure\""));
        p.set_env_tls("dev", None).unwrap();
        assert_eq!(p.env_tls(), None);
        assert_eq!(p.env_name("dev"), "Dev");
        assert_eq!(p.journal_len(), 2);
    }
}
