//! Environments: `environments/<slug>.toml`, their `[environment.<slug>]`
//! tables, and the secrets and selections keyed by them.

use super::*;
use crate::journal::MergeKey;

/// `listed` (an `environments` array) exactly as written, with duplicates
/// dropped and invalid and stale names kept in place, then the `shown`
/// environments it lacks: the list an environment op writes back. Never
/// filters a hand-written entry away.
fn write_list_over(listed: &[String], shown: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in listed.iter().chain(shown) {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// `doc`'s `environments` list as `ProjectMeta` parses it, not the array
/// read from the document: `None` for a document that parse rejects,
/// whose write `edit_project_toml` then refuses. Environment ops build
/// the list they write from this, inside `edit_project_toml`, so an
/// outside edit postui hasn't read yet is kept (#284, #287).
fn listed_in(doc: &toml_edit::DocumentMut) -> Option<Vec<String>> {
    toml::from_str::<ProjectMeta>(&doc.to_string()).ok().map(|m| m.environments)
}

impl Project {
    /// The slug `display` gets as an environment's file: `slugify`, then
    /// `-2`, `-3`, … while the disk says `environments/<slug>.toml`
    /// exists. `exclude` is the environment a rename renames. The disk
    /// answers for its own case rule (#279), as for a space's slug.
    pub fn environment_slug_for(&self, display: &str, exclude: Option<&str>) -> String {
        meta::unique_slug_among(
            meta::Kind::Environment,
            display,
            |slug| env_rel(slug).map(|p| self.disk.exists(&p)).unwrap_or(false),
            exclude,
        )
    }

    /// Re-lists from `meta` and disk; a listing that fails keeps the list
    /// we have rather than emptying it.
    pub(crate) fn refresh_environments(&mut self) {
        if let Ok((environments, invalid)) = Self::list_environments(&mut self.disk, &self.meta) {
            self.environments = environments;
            self.environments_warning = super::join_warnings(&invalid);
        }
    }

    /// Moves `name` by `delta` places among the displayed environments,
    /// stopping at the ends.
    pub fn move_environment(
        &mut self,
        name: &str,
        delta: i32,
    ) -> Result<Option<ListChange>, Error> {
        let key = MergeKey::EnvOrder { name: name.to_string() };
        self.reorder_environment("move environment", Some(key), name, |pos| {
            pos.saturating_add_signed(delta as isize)
        })
    }

    /// Puts `name` at place `to` among the displayed environments, as a
    /// drag left it. A place past the end is the last one.
    pub fn place_environment(
        &mut self,
        name: &str,
        to: usize,
    ) -> Result<Option<ListChange>, Error> {
        self.reorder_environment("reorder environments", None, name, |_| to)
    }

    /// Moves `name` among the displayed environments to the place `place`
    /// gives from its place now (capped at the last one), as one undo
    /// entry.
    ///
    /// The list is built from the document `edit_project_toml` has just
    /// read, never from `self.meta`, so the row moves in the order the
    /// file holds at this write and an outside edit postui hasn't read
    /// yet is kept (#284). Entries that name no displayed environment
    /// keep their slots. When the row is already at that place, the
    /// document is left as it was read. The rows are listed again either
    /// way, from the `meta` that edit loaded, so they then show the
    /// file's order. Nothing else is listed again, and no re-read is left
    /// due for the rest of what changed in the file (spec 2026-09-04
    /// request reordering, the #284 costs).
    fn reorder_environment(
        &mut self,
        label: &str,
        merge: Option<MergeKey>,
        name: &str,
        place: impl FnOnce(usize) -> usize,
    ) -> Result<Option<ListChange>, Error> {
        // The rows the tab shows, from the last listing: the places are
        // counted among them. A row whose file was removed outside since
        // is still one, so the list written names it.
        let shown = self.environments.clone();
        if !shown.iter().any(|e| e == name) {
            return Err(Error::NotFound(name.to_string()));
        }
        let mut change = None;
        self.run_transaction(label, EntryMeta::default(), merge, |p| {
            p.edit_project_toml(|doc| {
                let Some(listed) = listed_in(doc) else { return };
                let mut envs = write_list_over(&listed, &shown);
                let slots: Vec<usize> =
                    (0..envs.len()).filter(|i| shown.contains(&envs[*i])).collect();
                let before: Vec<String> = slots.iter().map(|i| envs[*i].clone()).collect();
                let Some(pos) = before.iter().position(|n| n == name) else { return };
                let to = place(pos).min(before.len() - 1);
                if to == pos {
                    return;
                }
                let mut after = before.clone();
                let moved = after.remove(pos);
                after.insert(to, moved);
                for (slot, n) in slots.iter().zip(&after) {
                    envs[*slot] = n.clone();
                }
                doc["environments"] = toml_edit::value(meta::slug_array(&envs));
                change = Some(ListChange { before, after });
            })?;
            p.refresh_environments();
            Ok(())
        })?;
        Ok(change)
    }

    /// Writes `secrets.toml` from memory as a journaled text op.
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
        // The rows shown, then the new one: the file's list gains the ones
        // it lacks, the new one last.
        let mut shown = existing;
        shown.push(slug.clone());
        // Creating an environment switches to it, as today's app does;
        // the transition is recorded so undo puts the old one back.
        let entry_meta = EntryMeta {
            active_env: Some((self.active_env.clone(), Some(slug.clone()))),
            ..EntryMeta::default()
        };
        self.transaction("create environment", entry_meta, |p| {
            p.fs_create_file(&env_rel(&slug)?, "")?;
            p.edit_project_toml(|doc| {
                if let Some(listed) = listed_in(doc) {
                    let envs = write_list_over(&listed, &shown);
                    doc["environments"] = toml_edit::value(meta::slug_array(&envs));
                }
                meta::set_item_name(doc, meta::Kind::Environment, &slug, &display);
            })?;
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
        let shown = existing;
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
                // Only rewritten when `project.toml` lists `from` at this
                // write: a rename never creates an order list the project
                // does without. When no array is present, the renamed
                // environment re-sorts within the alphabetical unlisted
                // tail rather than keeping the slot `from` held — the tail
                // is alphabetical by definition, so there is no slot to
                // keep. This is deliberate: unlike `rename_space`, which
                // always writes the full list, an environment rename does
                // not promote a project into having an order array just
                // because one entry changed name.
                if let Some(listed) = listed_in(doc).filter(|l| l.contains(&from)) {
                    let mut envs = write_list_over(&listed, &shown);
                    if let Some(i) = envs.iter().position(|n| *n == from) {
                        envs[i] = to.clone();
                    }
                    doc["environments"] = toml_edit::value(meta::slug_array(&envs));
                }
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
    /// active environment falls to the first remaining one. The last
    /// environment stays (`LastEnvironment`): a project always has one,
    /// and secrets written with none active would land under no key.
    pub fn delete_environment(&mut self, name: &str) -> Result<(), Error> {
        let path = env_rel(name)?;
        if !self.disk.is_file(&path) {
            return Err(Error::NotFound(name.to_string()));
        }
        if self.environments.iter().all(|e| e == name) {
            return Err(Error::LastEnvironment);
        }
        let name = name.to_string();
        let shown = self.environments.clone();
        let was_active = self.active_env.as_deref() == Some(name.as_str());
        let fallback = self.environments.iter().find(|e| **e != name).cloned();
        let entry_meta = EntryMeta {
            active_env: was_active.then(|| (Some(name.clone()), fallback.clone())),
            ..EntryMeta::default()
        };
        self.transaction("delete environment", entry_meta, |p| {
            p.edit_project_toml(|doc| {
                // As in `rename_environment`: the list is only rewritten
                // when `project.toml` lists this environment at this write.
                if let Some(listed) = listed_in(doc).filter(|l| l.contains(&name)) {
                    let mut envs = write_list_over(&listed, &shown);
                    envs.retain(|n| *n != name);
                    doc["environments"] = toml_edit::value(meta::slug_array(&envs));
                }
                meta::remove_item_table(doc, meta::Kind::Environment, &name);
            })?;
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
                Some(pol) => {
                    meta::set_item_key(doc, meta::Kind::Environment, &slug, "tls", pol.as_str())
                }
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
        let data = varmodel::parse_environment(&text)
            .map_err(|e| Error::Parse { file: path.to_string(), error: e.to_string() })?;
        varmodel::validate_env(&self.model, &data)
            .map_err(|e| Error::Parse { file: path.to_string(), error: e.to_string() })?;
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

    /// [`fixture`] with an explicit `environments` order already in
    /// `project.toml` — a project that has been reordered once.
    fn ordered_fixture() -> (tempfile::TempDir, Project) {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!("environments = [\"dev\", \"qa\"]\n{text}"),
        )
        .unwrap();
        let (p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        (dir, p)
    }

    #[test]
    fn move_environment_moves_a_place_clamps_and_a_burst_merges_into_one_entry() {
        let (_d, mut p) = ordered_fixture();
        assert!(p.move_environment("dev", -1).unwrap().is_none());
        let c = p.move_environment("dev", 1).unwrap().unwrap();
        assert_eq!(c.before, ["dev", "qa"]);
        assert_eq!(c.after, ["qa", "dev"]);
        assert_eq!(p.environments(), ["qa", "dev"]);
        p.move_environment("dev", -1).unwrap();
        assert_eq!(p.environments(), ["dev", "qa"]);
        assert_eq!(
            p.journal_len(),
            0,
            "two keyboard moves within 2 s merge into one step, which nets to identity and is dropped"
        );
        assert!(matches!(p.move_environment("nope", 1), Err(Error::NotFound(_))));
    }

    #[test]
    fn move_environment_keeps_an_invalid_middle_entry_in_place() {
        // An `environments` array can hold an entry with no file behind it
        // (or an invalid name) that a move must not disturb: it sits in
        // the array between two real environments and stays there.
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!("environments = [\"prod\", \"Not Valid\", \"qa\"]\n{text}"),
        )
        .unwrap();
        std::fs::write(dir.path().join("environments/prod.toml"), "").unwrap();
        std::fs::remove_file(dir.path().join("environments/dev.toml")).unwrap();
        let (mut p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(p.environments(), ["prod", "qa"]);

        let c = p.move_environment("qa", -1).unwrap().unwrap();
        assert_eq!(c.before, ["prod", "qa"]);
        assert_eq!(c.after, ["qa", "prod"]);
        assert_eq!(p.environments(), ["qa", "prod"]);

        let text = read(&dir, "project.toml").unwrap();
        assert!(
            text.contains("environments = [\"qa\", \"Not Valid\", \"prod\"]"),
            "the invalid entry keeps its middle slot: {text}"
        );
    }

    #[test]
    fn place_environment_puts_the_row_at_its_place_and_refuses_a_row_not_shown() {
        let (dir, mut p) = fixture();
        let c = p.place_environment("qa", 0).unwrap().unwrap();
        assert_eq!(c.before, ["dev", "qa"]);
        assert_eq!(c.after, ["qa", "dev"]);
        assert_eq!(p.environments(), ["qa", "dev"]);
        assert!(
            read(&dir, "project.toml").unwrap().contains("environments = [\"qa\", \"dev\"]"),
            "the drag materialises the whole displayed order"
        );
        assert!(p.place_environment("qa", 0).unwrap().is_none());
        assert!(matches!(p.place_environment("nope", 0), Err(Error::NotFound(_))));
        assert_eq!(p.journal_len(), 1, "a drag never merges");

        // The rows between the old place and the new one shift, and a
        // place past the end is the last one.
        p.create_environment("Prod").unwrap();
        let c = p.place_environment("prod", 0).unwrap().unwrap();
        assert_eq!(c.before, ["qa", "dev", "prod"]);
        assert_eq!(c.after, ["prod", "qa", "dev"]);
        let c = p.place_environment("prod", 9).unwrap().unwrap();
        assert_eq!(c.after, ["qa", "dev", "prod"]);
        assert_eq!(p.environments(), ["qa", "dev", "prod"]);
    }

    /// Rewrites the `environments` list in `project.toml` by hand, as
    /// another program would, and returns the file's new text.
    fn hand_order(dir: &tempfile::TempDir, order: &[&str]) -> String {
        let path = dir.path().join("project.toml");
        let mut doc: toml_edit::DocumentMut =
            std::fs::read_to_string(&path).unwrap().parse().unwrap();
        doc["environments"] = toml_edit::value(toml_edit::Array::from_iter(order.iter().copied()));
        let text = format!("# pulled\n{doc}");
        std::fs::write(&path, &text).unwrap();
        text
    }

    /// #284: both reorders move the row in the list `project.toml` holds
    /// at the write, not in the list as last read, and undo puts the
    /// file's text back.
    #[test]
    fn a_reorder_moves_the_row_in_the_list_the_file_holds() {
        let (dir, mut p) = ordered_fixture();
        p.create_environment("Prod").unwrap();
        assert_eq!(p.environments(), ["dev", "qa", "prod"]);

        let outside = hand_order(&dir, &["qa", "dev", "prod"]);
        let c = p.move_environment("prod", -1).unwrap().unwrap();
        assert_eq!(c.before, ["qa", "dev", "prod"]);
        assert_eq!(c.after, ["qa", "prod", "dev"]);
        assert_eq!(p.meta().environments, ["qa", "prod", "dev"]);
        assert_eq!(p.environments(), ["qa", "prod", "dev"]);
        p.undo().unwrap().unwrap();
        assert_eq!(read(&dir, "project.toml").unwrap(), outside);

        let outside = hand_order(&dir, &["prod", "dev", "qa"]);
        let c = p.place_environment("qa", 0).unwrap().unwrap();
        assert_eq!(c.before, ["prod", "dev", "qa"]);
        assert_eq!(c.after, ["qa", "prod", "dev"]);
        assert_eq!(p.meta().environments, ["qa", "prod", "dev"]);
        assert_eq!(p.environments(), ["qa", "prod", "dev"]);
        p.undo().unwrap().unwrap();
        assert_eq!(read(&dir, "project.toml").unwrap(), outside);
    }

    /// A row already at its place in the file's order: `project.toml` is
    /// left as it is, no entry is recorded, and the rows show the file's
    /// order.
    #[test]
    fn a_reorder_that_moves_nothing_in_the_files_order_leaves_the_file_as_it_is() {
        let (dir, mut p) = ordered_fixture();
        let outside = hand_order(&dir, &["qa", "dev"]);
        assert_eq!(p.environments(), ["dev", "qa"]);
        assert!(p.move_environment("qa", -1).unwrap().is_none());
        assert!(p.place_environment("dev", 1).unwrap().is_none());
        assert_eq!(read(&dir, "project.toml").unwrap(), outside);
        assert_eq!(p.journal_len(), 0);
        assert_eq!(p.environments(), ["qa", "dev"]);
    }

    /// A `project.toml` that `ProjectMeta` rejects is never rewritten by a
    /// reorder: one with an unknown key, and one whose list holds an entry
    /// that is not a string.
    #[test]
    fn a_reorder_never_writes_a_project_toml_that_will_not_parse() {
        let (dir, mut p) = ordered_fixture();
        let good = read(&dir, "project.toml").unwrap();
        let broken = [
            format!("bogus_key = 1\n{good}"),
            good.replacen("[\"dev\", \"qa\"]", "[\"dev\", 1, \"qa\"]", 1),
        ];
        for text in broken {
            assert_ne!(text, good);
            std::fs::write(dir.path().join("project.toml"), &text).unwrap();
            assert!(matches!(p.move_environment("dev", 1), Err(Error::Parse { .. })), "{text}");
            assert!(matches!(p.place_environment("qa", 0), Err(Error::Parse { .. })), "{text}");
            assert_eq!(read(&dir, "project.toml").unwrap(), text);
            assert_eq!(p.journal_len(), 0);
        }
    }

    /// In a directory with no `project.toml`, a reorder that moves nothing
    /// makes none and records no entry.
    #[test]
    fn a_reorder_that_moves_nothing_makes_no_project_toml() {
        let (dir, _p) = fixture();
        std::fs::remove_file(dir.path().join("project.toml")).unwrap();
        let (mut p, _warnings) = Project::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(p.environments(), ["dev", "qa"]);
        assert!(p.move_environment("dev", -1).unwrap().is_none());
        assert!(p.place_environment("qa", 1).unwrap().is_none());
        assert!(!dir.path().join("project.toml").exists());
        assert_eq!(p.journal_len(), 0);
    }

    /// Over a `project.toml` that `toml_edit` would print differently (no
    /// final newline, CRLF endings, a byte-order mark), a reorder that
    /// moves nothing leaves the file's bytes as they were and records no
    /// entry. A real move writes one entry, and undo puts the bytes back.
    #[test]
    fn a_reorder_that_moves_nothing_leaves_a_file_toml_edit_would_reprint() {
        let (dir, _p) = ordered_fixture();
        let good = read(&dir, "project.toml").unwrap();
        let path = dir.path().join("project.toml");
        let texts = [
            good.trim_end_matches('\n').to_string(),
            good.replace('\n', "\r\n"),
            format!("\u{feff}{good}"),
        ];
        for text in texts {
            std::fs::write(&path, &text).unwrap();
            let (mut p, warnings) = Project::open(dir.path().to_path_buf()).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(p.environments(), ["dev", "qa"]);

            assert!(p.move_environment("dev", -1).unwrap().is_none());
            assert!(p.place_environment("qa", 1).unwrap().is_none());
            assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes(), "{text:?}");
            assert_eq!(p.journal_len(), 0, "{text:?}");

            assert!(p.move_environment("dev", 1).unwrap().is_some());
            assert_ne!(std::fs::read(&path).unwrap(), text.as_bytes(), "{text:?}");
            assert_eq!(p.journal_len(), 1, "{text:?}");
            p.undo().unwrap().unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes(), "{text:?}");
        }
    }

    #[test]
    fn create_appends_rename_replaces_and_delete_removes_the_array_entry() {
        let (dir, mut p) = ordered_fixture();
        p.create_environment("Prod").unwrap();
        assert_eq!(p.meta().environments, ["dev", "qa", "prod"]);
        p.rename_environment("qa", "Staging").unwrap();
        assert_eq!(p.meta().environments, ["dev", "staging", "prod"]);
        assert_eq!(p.environments(), ["dev", "staging", "prod"]);
        p.delete_environment("staging").unwrap();
        assert_eq!(p.meta().environments, ["dev", "prod"]);
        assert_eq!(p.environments(), ["dev", "prod"]);
        assert!(!read(&dir, "project.toml").unwrap().contains("staging"));
    }

    /// #287: create, rename and delete change the list `project.toml`
    /// holds at the write, not the list as last read: an order changed
    /// outside is kept, and so is an environment added outside. Undo puts
    /// the file's text back.
    #[test]
    fn create_rename_and_delete_change_the_list_the_file_holds() {
        let (dir, mut p) = ordered_fixture();
        p.create_environment("Prod").unwrap();
        assert_eq!(p.environments(), ["dev", "qa", "prod"]);
        std::fs::write(dir.path().join("environments/stage.toml"), "").unwrap();

        let outside = hand_order(&dir, &["qa", "stage", "prod", "dev"]);
        p.create_environment("New").unwrap();
        assert_eq!(p.meta().environments, ["qa", "stage", "prod", "dev", "new"]);
        assert_eq!(p.environments(), ["qa", "stage", "prod", "dev", "new"]);
        p.undo().unwrap().unwrap();
        assert_eq!(read(&dir, "project.toml").unwrap(), outside);
        p.redo().unwrap().unwrap();

        hand_order(&dir, &["new", "qa", "stage", "prod", "dev"]);
        p.rename_environment("dev", "Dev2").unwrap();
        assert_eq!(p.meta().environments, ["new", "qa", "stage", "prod", "dev2"]);
        assert_eq!(p.environments(), ["new", "qa", "stage", "prod", "dev2"]);

        hand_order(&dir, &["dev2", "new", "qa", "stage", "prod"]);
        p.delete_environment("qa").unwrap();
        assert_eq!(p.meta().environments, ["dev2", "new", "stage", "prod"]);
        assert_eq!(p.environments(), ["dev2", "new", "stage", "prod"]);
    }

    /// #287: a rename or delete rewrites the list when `project.toml`
    /// lists the environment at the write, whatever the last read held.
    #[test]
    fn a_rename_or_a_delete_rewrites_the_list_only_when_the_file_lists_it() {
        // Listed outside since the last read: the rename keeps its slot.
        let (dir, mut p) = fixture();
        assert!(p.meta().environments.is_empty());
        hand_order(&dir, &["qa", "dev"]);
        p.rename_environment("qa", "Staging").unwrap();
        assert_eq!(p.meta().environments, ["staging", "dev"]);
        p.delete_environment("staging").unwrap();
        assert_eq!(p.meta().environments, ["dev"]);

        // The list removed outside since the last read: none comes back.
        let (dir, mut p) = ordered_fixture();
        let path = dir.path().join("project.toml");
        let mut doc: toml_edit::DocumentMut =
            std::fs::read_to_string(&path).unwrap().parse().unwrap();
        doc.remove("environments");
        std::fs::write(&path, doc.to_string()).unwrap();
        assert_eq!(p.meta().environments, ["dev", "qa"]);
        p.rename_environment("qa", "Staging").unwrap();
        p.delete_environment("dev").unwrap();
        let text = read(&dir, "project.toml").unwrap();
        assert!(!text.contains("environments"), "{text}");
    }

    #[test]
    fn a_rename_or_a_delete_never_creates_an_array_the_project_did_without() {
        let (dir, mut p) = fixture();
        p.rename_environment("qa", "Staging").unwrap();
        assert!(p.meta().environments.is_empty());
        p.delete_environment("staging").unwrap();
        assert!(p.meta().environments.is_empty());
        assert_eq!(p.environments(), ["dev"]);
        let text = read(&dir, "project.toml").unwrap();
        assert!(!text.contains("environments"), "{text}");
    }

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
        assert!(read(&dir, STATE_TOML).unwrap().contains("environment = \"qa-2\""));
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
        assert_eq!(
            p.secrets().get("development").and_then(|m| m.get("token")).map(String::as_str),
            Some("s3cret")
        );
        assert!(p.secrets().get("dev").is_none());
        assert_eq!(p.selections_for("development").get("region").map(String::as_str), Some("east"));
        assert!(read(&dir, SECRETS_TOML).unwrap().contains("[development]"));
        assert!(read(&dir, "project.toml").unwrap().contains("[environment.development]"));
        assert_eq!(p.journal_len(), 2);
    }

    /// #279: beside a hand-made `environments/Prod.toml`, the disk decides
    /// the slug of a new or renamed environment "Prod": its own file where
    /// the disk tells the two names apart, a numbered one where it
    /// doesn't. Either way the hand-made file keeps its text, through the
    /// undos too.
    #[test]
    fn an_environment_beside_a_hand_made_file_in_another_case_leaves_it_alone() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("environments/Prod.toml"), "host = \"mine\"\n").unwrap();
        let same_name = dir.path().join("environments/prod.toml").exists();
        let slug = if same_name { "prod-2" } else { "prod" };
        let kept = |what: &str| {
            let text = read(&dir, "environments/Prod.toml");
            assert_eq!(text.as_deref(), Some("host = \"mine\"\n"), "{what}");
        };

        assert_eq!(p.create_environment("Prod").unwrap(), slug);
        kept("create");
        assert_eq!(read(&dir, &format!("environments/{slug}.toml")).as_deref(), Some(""));
        p.undo().unwrap().unwrap();
        kept("undo of the create");

        assert_eq!(p.rename_environment("qa", "Prod").unwrap(), slug);
        kept("rename");
        p.undo().unwrap().unwrap();
        kept("undo of the rename");
        assert!(dir.path().join("environments/qa.toml").is_file());

        // A file at exactly the slug's path takes it on every disk.
        assert_eq!(p.environment_slug_for("QA", None), "qa-2");
    }

    #[test]
    fn the_last_environment_cannot_be_deleted() {
        let (dir, mut p) = fixture();
        p.delete_environment("qa").unwrap();
        assert!(matches!(p.delete_environment("dev"), Err(Error::LastEnvironment)));
        assert!(dir.path().join("environments/dev.toml").is_file());
        assert_eq!(p.environments(), ["dev"]);
    }

    #[test]
    fn delete_trashes_drops_everything_keyed_by_it_and_falls_to_the_first_remaining() {
        let (dir, mut p) = fixture();
        p.set_secret_for("dev", "token", "x".into()).unwrap();
        p.delete_environment("dev").unwrap();
        assert!(dir.path().join(TRASH_DIR).join("1/environments/dev.toml").is_file());
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
        assert!(read(&dir, STATE_TOML).unwrap().contains("environment = \"qa\""));
    }

    #[test]
    fn a_failed_rename_leaves_memory_matching_disk() {
        let (dir, mut p) = fixture();
        // A regular file where the local folder goes makes creating it
        // fail, so the write of `state.toml` inside the rename's
        // transaction fails after the disk rename and table edit already
        // happened — exercising the rollback.
        std::fs::write(dir.path().join(LOCAL_DIR), "not a directory").unwrap();
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
    /// value reaches `secrets.toml` and `resolved` at once, in the
    /// active environment and in the no-environment slot alike.
    #[test]
    fn set_secret_writes_the_file_and_resolves_with_and_without_an_environment() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("variables.toml"), "[token]\nsecret = true\n").unwrap();
        p.invalidate_stamps();
        p.poll();

        p.set_secret("token", "s3cret".into()).unwrap();
        assert!(read(&dir, SECRETS_TOML).unwrap().contains("s3cret"));
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
        // A regular file where the local folder goes makes creating it
        // fail, so the secrets write inside the transaction fails.
        std::fs::write(dir.path().join(LOCAL_DIR), "not a directory").unwrap();
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
