//! Environments: `environments/<slug>.toml`, their `[environment.<slug>]`
//! tables, and the secrets and selections keyed by them.

use super::*;

impl Project {
    pub fn environment_slug_for(&self, display: &str, exclude: Option<&str>) -> String {
        legacy::unique_slug_among(
            legacy::Kind::Environment,
            display,
            |slug| env_rel(slug).map(|p| self.disk.exists(&p)).unwrap_or(false),
            exclude,
        )
    }

    pub(crate) fn refresh_environments(&mut self) {
        self.environments = Self::list_environments(&mut self.disk);
    }

    /// Writes `.local/secrets.toml` from memory as a journaled text op.
    pub(crate) fn write_secrets_journaled(&mut self) -> Result<(), Error> {
        let text = toml::to_string(&self.secrets).expect("secrets always serialize");
        self.fs_write_text(&rel(SECRETS_TOML)?, Some(&text))
    }

    /// Records a secret for `env`, writes the secrets file, re-resolves
    /// when `env` is active. The error never carries the value.
    pub fn set_secret_for(&mut self, env: &str, name: &str, value: String) -> Result<(), Error> {
        let mut secrets = self.secrets.clone();
        secrets.entry(env.to_string()).or_default().insert(name.to_string(), value);
        let previous = std::mem::replace(&mut self.secrets, secrets);
        if let Err(e) = self.transaction("set secret", EntryMeta::default(), |p| p.write_secrets_journaled()) {
            self.secrets = previous;
            return Err(e);
        }
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
        let mut secrets = self.secrets.clone();
        if secrets.get_mut(env).and_then(|m| m.shift_remove(name)).is_none() {
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.secrets, secrets);
        if let Err(e) = self.transaction("remove secret", EntryMeta::default(), |p| p.write_secrets_journaled()) {
            self.secrets = previous;
            return Err(e);
        }
        if self.env_key() == env {
            self.refresh_resolved();
        }
        Ok(())
    }

    pub fn create_environment(&mut self, display: &str) -> Result<String, Error> {
        let display = legacy::display_name_of(display)?;
        let meta = self.meta.clone();
        let existing = self.environments.clone();
        if legacy::display_taken(&display, &existing, |s| legacy::env_display(&meta, s), None) {
            return Err(Error::AlreadyExists(display));
        }
        let slug = self.environment_slug_for(&display, None);
        self.transaction("create environment", EntryMeta::default(), |p| {
            p.fs_create_file(&env_rel(&slug)?, "")?;
            p.edit_project_toml(|doc| legacy::set_item_name(doc, legacy::Kind::Environment, &slug, &display))?;
            p.refresh_environments();
            Ok(())
        })?;
        Ok(slug)
    }

    pub fn rename_environment(&mut self, from: &str, display: &str) -> Result<String, Error> {
        let display = legacy::display_name_of(display)?;
        let from_path = env_rel(from)?;
        if !self.disk.is_file(&from_path) {
            return Err(Error::NotFound(from.to_string()));
        }
        let meta = self.meta.clone();
        let existing = self.environments.clone();
        if legacy::display_taken(&display, &existing, |s| legacy::env_display(&meta, s), Some(from)) {
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
                legacy::move_item_table(doc, legacy::Kind::Environment, &from, &to);
                legacy::set_item_name(doc, legacy::Kind::Environment, &to, &display);
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
            p.edit_project_toml(|doc| legacy::remove_item_table(doc, legacy::Kind::Environment, &name))?;
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
                Some(pol) => legacy::set_item_key(doc, legacy::Kind::Environment, &slug, "tls", pol.as_str()),
                None => {
                    if let Some(it) = doc
                        .get_mut(legacy::Kind::Environment.table())
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

    fn load_active_env(&mut self, env: &str) -> Result<(), Error> {
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
