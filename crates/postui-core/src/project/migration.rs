//! The stage 6 → 7 variable-format conversion: probed at open and every
//! reload, offered once, applied with `.bak` copies. Not journaled (it
//! makes its own backups, as today).

use super::*;

pub(super) fn probe(disk: &mut Disk) -> (bool, Option<MigrationOutcome>, Vec<Warning>) {
    let vars = disk
        .read(&RelPath::new(VARIABLES_TOML).expect("constant"))
        .ok()
        .flatten()
        .unwrap_or_default();
    let envs: Vec<(String, String)> = Project::list_environments(disk)
        .into_iter()
        .map(|env| {
            let text = RelPath::new(format!("{ENVIRONMENTS_DIR}/{env}.toml"))
                .ok()
                .and_then(|p| disk.read(&p).ok().flatten())
                .unwrap_or_default();
            (env, text)
        })
        .collect();
    if !crate::migrate::needs_migration(&vars, &envs) {
        return (false, None, Vec::new());
    }
    match crate::migrate::migrate(&vars, &envs) {
        Ok(outcome) => (true, Some(outcome), Vec::new()),
        Err(e) => (
            true,
            None,
            vec![format!("variables use the old format and can't be converted automatically: {e}")],
        ),
    }
}

impl Project {
    pub fn pending_migration(&self) -> Option<&MigrationOutcome> {
        self.pending_migration.as_ref()
    }

    /// Each rewritten file is copied to `<file>.bak` first — only once,
    /// so a retry after a partial apply never overwrites the original —
    /// then written atomically. Reloads everything afterwards.
    pub fn apply_migration(&mut self) -> Result<Vec<Warning>, Error> {
        let Some(outcome) = self.pending_migration.take() else {
            return Err(Error::NothingPending);
        };
        let result = (|| -> Result<(), Error> {
            if let Some(text) = &outcome.variables {
                self.write_with_backup(&rel(VARIABLES_TOML)?, text)?;
            }
            for (env, text) in &outcome.envs {
                self.write_with_backup(&env_rel(env)?, text)?;
            }
            if let Some(text) = &outcome.new_default_env {
                self.write_with_backup(&env_rel(DEFAULT_ENVIRONMENT)?, text)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.pending_migration = Some(outcome);
            // A partial write (e.g. the default env's backup wrote but the
            // next file's did not) can leave disk ahead of memory; force
            // the next `poll` to re-sync rather than run on stale reads.
            self.force_reload = true;
            return Err(e);
        }
        let mut notes = outcome.notes;
        notes.extend(self.reload_all());
        Ok(notes)
    }

    fn write_with_backup(&mut self, path: &RelPath, text: &str) -> Result<(), Error> {
        let backup = RelPath::new(format!("{}.bak", path.as_str()))?;
        if self.disk.is_file(path) && !self.disk.is_file(&backup) {
            let existing = self.disk.read(path)?.unwrap_or_default();
            self.disk.write(&backup, &existing)?;
        }
        self.disk.write(path, text)?;
        Ok(())
    }

    pub fn decline_migration(&mut self) {
        self.pending_migration = None;
        self.migration_declined = true;
    }
}
