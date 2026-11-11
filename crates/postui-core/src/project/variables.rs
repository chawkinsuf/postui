//! `variables.toml` and the environment overlays: text edits through
//! `varedit`, validated before they land, journaled as prior text.

use super::*;
use crate::varedit::EditError;
// `varedit` itself is only referenced from the tests below (the public
// API here only names `EditError`); importing it unconditionally warns
// as unused outside test builds.
#[cfg(test)]
use crate::varedit;

impl Project {
    /// `variables.toml` as it reads right now. See [`Self::peek_text`].
    pub fn variables_text(&self) -> Result<String, Error> {
        self.peek_text(&rel(VARIABLES_TOML)?)
    }

    /// `environments/<env>.toml` as it reads right now. See
    /// [`Self::peek_text`].
    pub fn env_text(&self, env: &str) -> Result<String, Error> {
        self.peek_text(&env_rel(env)?)
    }

    /// The text at `path`, empty when there is no file. `peek`, not
    /// `read`: a scan must never record the stamp `poll` watches.
    fn peek_text(&self, path: &RelPath) -> Result<String, Error> {
        Ok(self.disk.peek(path)?.unwrap_or_default())
    }

    /// The app's multi-step variable cascades run under one label so
    /// undo reverses the whole cascade.
    pub fn cascade<T>(
        &mut self,
        label: &str,
        f: impl FnOnce(&mut Project) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.transaction(label, EntryMeta::default(), f)
    }

    /// Rewrites `variables.toml` through `f`. This and the two edits below
    /// take the text they edit through `peek_text`: that text is not
    /// loaded into the model, so recording its
    /// stamp would let a refused edit hide an outside change to the file
    /// from `poll` (#81). `Disk::read` records on every read, so the
    /// choice is made here. The write, when one lands, records the stamp.
    pub fn edit_variables(
        &mut self,
        f: impl FnOnce(&str) -> Result<String, EditError>,
    ) -> Result<(), Error> {
        let path = rel(VARIABLES_TOML)?;
        let text = self.peek_text(&path)?;
        let new_text = f(&text).map_err(|e| Error::Edit(e.to_string()))?;
        let new_model =
            varmodel::parse_variables(&new_text).map_err(|e| Error::Edit(e.to_string()))?;
        if self.active_env.is_some() {
            varmodel::validate_env(&new_model, &self.env_data)
                .map_err(|e| Error::Edit(e.to_string()))?;
        }
        self.transaction("edit variables", EntryMeta::default(), |p| {
            p.fs_write_text(&path, Some(&new_text))
        })?;
        self.model = new_model;
        self.refresh_resolved();
        Ok(())
    }

    pub fn edit_env(
        &mut self,
        env: &str,
        f: impl FnOnce(&str) -> Result<String, EditError>,
    ) -> Result<(), Error> {
        let path = env_rel(env)?;
        let text = self.peek_text(&path)?;
        let new_text = f(&text).map_err(|e| Error::Edit(e.to_string()))?;
        let new_env =
            varmodel::parse_environment(&new_text).map_err(|e| Error::Edit(e.to_string()))?;
        varmodel::validate_env(&self.model, &new_env).map_err(|e| Error::Edit(e.to_string()))?;
        self.transaction("edit environment", EntryMeta::default(), |p| {
            p.fs_write_text(&path, Some(&new_text))
        })?;
        self.refresh_environments();
        if self.active_env.as_deref() == Some(env) {
            self.env_data = new_env;
        }
        self.refresh_resolved();
        Ok(())
    }

    /// Both halves built and validated together before anything lands.
    pub fn edit_variables_and_envs(
        &mut self,
        vf: impl FnOnce(&str) -> Result<String, EditError>,
        ef: impl Fn(&str) -> Result<String, EditError>,
    ) -> Result<(), Error> {
        let vars_path = rel(VARIABLES_TOML)?;
        let vars_text = self.peek_text(&vars_path)?;
        let new_vars_text = vf(&vars_text).map_err(|e| Error::Edit(e.to_string()))?;
        let new_model =
            varmodel::parse_variables(&new_vars_text).map_err(|e| Error::Edit(e.to_string()))?;
        let mut envs: Vec<(String, RelPath, String, EnvData)> = Vec::new();
        for env in self.environments.clone() {
            let path = env_rel(&env)?;
            let text = self.peek_text(&path)?;
            let new_text = ef(&text).map_err(|e| Error::Edit(e.to_string()))?;
            let data =
                varmodel::parse_environment(&new_text).map_err(|e| Error::Edit(e.to_string()))?;
            varmodel::validate_env(&new_model, &data).map_err(|e| Error::Edit(e.to_string()))?;
            envs.push((env, path, new_text, data));
        }
        let active = self.active_env.clone();
        let mut active_data = None;
        self.transaction("edit variables", EntryMeta::default(), |p| {
            p.fs_write_text(&vars_path, Some(&new_vars_text))?;
            for (env, path, text, data) in envs {
                p.fs_write_text(&path, Some(&text))?;
                if active.as_deref() == Some(env.as_str()) {
                    active_data = Some(data);
                }
            }
            Ok(())
        })?;
        // A write under `environments/` changes the directory's stamp.
        // Listing it again records the new one, so `changed_on_disk` does
        // not report the edit's own writes (#284).
        self.refresh_environments();
        self.model = new_model;
        if let Some(data) = active_data {
            self.env_data = data;
        }
        self.refresh_resolved();
        Ok(())
    }

    /// Slugs of every request whose text uses `{{name}}`. Walks the
    /// listing already in memory and peeks each file, so the scan neither
    /// re-lists `requests/` nor stamps anything.
    pub fn scan_usage(&self, name: &str) -> Vec<String> {
        self.listing
            .iter()
            .map(|l| l.slug.clone())
            .filter(|slug| {
                request_rel(slug)
                    .ok()
                    .and_then(|p| self.disk.peek(&p).ok().flatten())
                    .map(|text| crate::vars::find_tokens(&text).iter().any(|t| t.name == name))
                    .unwrap_or(false)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;

    #[test]
    fn edit_variables_applies_the_edit_keeps_comments_and_re_resolves() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("variables.toml"),
            "# keep me\n[host]\ndefault = \"localhost\"\n",
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.edit_variables(|doc| varedit::upsert_var(doc, "port", None, Some("8080"))).unwrap();
        let text = read(&dir, "variables.toml").unwrap();
        assert!(text.starts_with("# keep me"), "{text}");
        assert!(p.variables().vars.contains_key("port"));
        assert_eq!(p.resolved().values.get("port").map(String::as_str), Some("8080"));
        assert_eq!(p.journal_len(), 1);
    }

    // The brief's original premise (`dev.toml` sets an undeclared
    // selector's options) is rejected by `validate_env` at `open`, so the
    // fixture would land with no active env and `edit_variables` would
    // skip validation entirely, letting the delete succeed. Rewritten per
    // controller ruling: a shared-free `region` selector with an
    // environment overlay, so renaming it out from under `dev`'s options
    // is what `validate_env` actually refuses.
    #[test]
    fn a_refused_edit_leaves_the_file_and_the_model_alone() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.region]\nfields = [\"host\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/dev.toml"),
            "[options.region.east]\nhost = \"e\"\n",
        )
        .unwrap();
        let before = read(&dir, "variables.toml").unwrap();
        let (mut p2, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let r = p2.edit_variables(|doc| varedit::rename_selector(doc, "region", "zone"));
        assert!(r.is_err());
        assert_eq!(read(&dir, "variables.toml").unwrap(), before);
        assert!(p2.variables().selectors.contains_key("region"));
        assert_eq!(p2.journal_len(), 0);
    }

    /// #81: a refused edit read the file but changed nothing, so an outside
    /// change to that file is still the next `poll`'s to pick up.
    #[test]
    fn a_refused_edit_leaves_an_outside_change_for_the_next_poll() {
        let refuse = |_: &str| Err::<String, _>(EditError::Conflict("refused".into()));
        type Edit =
            fn(&mut Project, &dyn Fn(&str) -> Result<String, EditError>) -> Result<(), Error>;
        // What the outside change writes, and what the poll must show for it.
        let vars_outside = ("variables.toml", "[host]\ndefault = \"localhost\"\n[extra]\n");
        let env_outside = ("environments/dev.toml", "host = \"changed.outside\"\n");
        let vars_landed = |p: &Project| p.variables().vars.contains_key("extra");
        let env_landed = |p: &Project| {
            p.resolved().values.get("host").map(String::as_str) == Some("changed.outside")
        };
        type Landed = fn(&Project) -> bool;
        let edits: [((&str, &str), Edit, Landed); 4] = [
            (vars_outside, |p, f| p.edit_variables(f), vars_landed),
            (env_outside, |p, f| p.edit_env("dev", f), env_landed),
            (
                vars_outside,
                |p, f| p.edit_variables_and_envs(f, |doc| Ok(doc.to_string())),
                vars_landed,
            ),
            (
                env_outside,
                |p, f| p.edit_variables_and_envs(|doc| Ok(doc.to_string()), f),
                env_landed,
            ),
        ];
        for ((file, outside), edit, landed) in edits {
            let (dir, mut p) = fixture();
            std::fs::write(dir.path().join(file), outside).unwrap();
            assert!(edit(&mut p, &refuse).is_err(), "{file}");
            assert!(!landed(&p), "{file}: not read before the poll");
            assert!(p.changed_on_disk(), "{file}: the change is still pending");
            assert!(p.poll().0, "{file}");
            assert!(landed(&p), "{file}: the poll read the change");
        }
    }

    #[test]
    fn edit_env_refreshes_the_active_data_only_for_the_active_env() {
        let (dir, mut p) = fixture();
        p.edit_env("qa", |doc| varedit::set_env_value(doc, "host", Some("qa.local"))).unwrap();
        assert!(read(&dir, "environments/qa.toml").unwrap().contains("qa.local"));
        assert_eq!(p.resolved().values.get("host").map(String::as_str), Some("dev.local"));
        p.edit_env("dev", |doc| varedit::set_env_value(doc, "host", Some("dev2"))).unwrap();
        assert_eq!(p.resolved().values.get("host").map(String::as_str), Some("dev2"));
    }

    #[test]
    fn edit_variables_and_envs_is_all_or_nothing_and_one_entry() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("variables.toml"),
            "[selectors.region]\nfields = [\"host\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/dev.toml"),
            "[options.region.east]\nhost = \"e\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("environments/qa.toml"),
            "[options.region.west]\nhost = \"w\"\n",
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.edit_variables_and_envs(
            |doc| varedit::rename_selector(doc, "region", "zone"),
            |doc| varedit::rename_selector_options(doc, "region", "zone"),
        )
        .unwrap();
        assert!(read(&dir, "variables.toml").unwrap().contains("[selectors.zone]"));
        assert!(read(&dir, "environments/qa.toml").unwrap().contains("[options.zone.west]"));
        assert_eq!(p.journal_len(), 1);
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.ops.len(), 3, "variables + two envs");
    }

    #[test]
    fn a_cascade_is_one_entry_and_rolls_back_whole_on_failure() {
        let (dir, mut p) = fixture();
        let r = p.cascade("rename var", |p| {
            p.edit_variables(|doc| varedit::rename_var(doc, "host", "hostname"))?;
            p.edit_env("dev", |doc| varedit::rename_env_var(doc, "host", "hostname"))?;
            Err::<(), Error>(Error::Edit("late failure".into()))
        });
        assert!(r.is_err());
        assert!(read(&dir, "variables.toml").unwrap().contains("[host]"));
        assert!(read(&dir, "environments/dev.toml").unwrap().contains("host = "));
        assert_eq!(p.journal_len(), 0);
        p.cascade("rename var", |p| {
            p.edit_variables(|doc| varedit::rename_var(doc, "host", "hostname"))?;
            p.edit_env("dev", |doc| varedit::rename_env_var(doc, "host", "hostname"))
        })
        .unwrap();
        assert_eq!(p.journal_len(), 1);
    }

    /// #284: the environment files `edit_variables_and_envs` writes are
    /// its own change, so they leave no poll due.
    #[test]
    fn edit_variables_and_envs_leaves_no_poll_due() {
        let (dir, mut p) = fixture();
        p.edit_variables_and_envs(
            |doc| varedit::rename_var(doc, "host", "hostname"),
            |doc| varedit::rename_env_var(doc, "host", "hostname"),
        )
        .unwrap();
        assert!(read(&dir, "environments/dev.toml").unwrap().contains("hostname = "));
        assert!(!p.changed_on_disk());
    }

    /// #284: an environment file written and then put back by a rollback
    /// leaves no poll due either.
    #[test]
    fn a_rollback_under_environments_leaves_no_poll_due() {
        let (dir, mut p) = fixture();
        let r = p.cascade("set value", |p| {
            p.edit_env("qa", |doc| varedit::set_env_value(doc, "host", Some("qa.local")))?;
            Err::<(), Error>(Error::Edit("late failure".into()))
        });
        assert!(r.is_err());
        assert_eq!(read(&dir, "environments/qa.toml").unwrap(), "");
        assert!(!p.changed_on_disk());
    }

    #[test]
    fn scan_usage_reads_request_files_through_the_project() {
        let (_d, p) = fixture();
        assert_eq!(p.scan_usage("host"), ["auth/login", "main/ping"]);
        assert!(p.scan_usage("nope").is_empty());
    }

    /// The scan peeks whole request files, so a token counts wherever it
    /// sits — not just in the url the other tests use. Replaces the
    /// `varedit::scan_usage_finds_tokens_in_every_field_and_ignores_other_names`
    /// coverage the free function took with it.
    #[test]
    fn scan_usage_finds_tokens_in_every_field_and_ignores_other_names() {
        let (dir, _p) = fixture();
        let write = |slug: &str, body: &str| {
            std::fs::write(
                dir.path().join(format!("requests/{slug}.toml")),
                format!("method = \"GET\"\nurl = \"https://x.test\"\n{body}"),
            )
            .unwrap();
        };
        std::fs::write(
            dir.path().join("requests/main/in-url.toml"),
            "method = \"GET\"\nurl = \"https://x.test/{{base_url}}\"\n",
        )
        .unwrap();
        write("main/in-params", "[params]\nq = \"{{base_url}}\"\n");
        write("main/in-headers", "[headers]\nX-Auth = \"{{base_url}}\"\n");
        write("main/in-variables", "[variables]\nlocal = \"{{base_url}}\"\n");
        write(
            "main/in-body",
            "[body]\ntype = \"json\"\ntext = \"{\\\"root\\\": \\\"{{base_url}}\\\"}\"\n",
        );
        // A different token only, and no token at all: neither is a hit.
        std::fs::write(
            dir.path().join("requests/main/unrelated.toml"),
            "method = \"GET\"\nurl = \"https://x.test/{{other}}\"\n",
        )
        .unwrap();
        write("main/none", "");

        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let mut hits = p.scan_usage("base_url");
        hits.sort();
        assert_eq!(
            hits,
            [
                "main/in-body",
                "main/in-headers",
                "main/in-params",
                "main/in-url",
                "main/in-variables",
            ]
        );
        assert_eq!(p.scan_usage("other"), ["main/unrelated"]);
        assert!(p.scan_usage("base").is_empty(), "no prefix matching");
    }

    /// The app calls these while it holds the project immutably, and a
    /// scan must not stamp files so that a later `poll` skips them.
    #[test]
    fn the_readers_take_shared_self_and_record_no_stamps() {
        fn read_everything(p: &Project) -> (Vec<String>, String, String) {
            (p.scan_usage("host"), p.variables_text().unwrap(), p.env_text("dev").unwrap())
        }
        let (dir, mut p) = fixture();
        let (used, vars, env) = read_everything(&p);
        assert_eq!(used, ["auth/login", "main/ping"]);
        assert!(vars.contains("[host]"), "{vars}");
        assert!(env.contains("dev.local"), "{env}");
        // A scan of `variables.toml` must not count as `poll` having seen
        // the outside edit that landed before it.
        std::fs::write(
            dir.path().join("variables.toml"),
            "[host]\ndefault = \"changed\"\n[extra]\n",
        )
        .unwrap();
        let _ = read_everything(&p);
        p.invalidate_stamps();
        assert!(p.poll().0);
        assert!(p.variables().vars.contains_key("extra"));
    }
}
