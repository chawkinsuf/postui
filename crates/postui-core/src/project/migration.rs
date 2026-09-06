//! The stage 6 → 7 variable-format conversion: probed at open and every
//! reload, offered once, applied with `.bak` copies. Journaled as one
//! entry — the `.bak` writes go through the same recorded text op as the
//! rewrites, so undo removes the backups and restores the originals.

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
    /// so a retry after a failed apply never overwrites the original —
    /// then written atomically. The whole conversion is one journal entry
    /// (backups included), so it undoes like any other project write; a
    /// failure part-way rolls every file back and stays pending for a
    /// retry. Reloads everything afterwards.
    pub fn apply_migration(&mut self) -> Result<Vec<Warning>, Error> {
        let Some(outcome) = self.pending_migration.take() else {
            return Err(Error::NothingPending);
        };
        let result = self.transaction("apply migration", EntryMeta::default(), |p| {
            if let Some(text) = &outcome.variables {
                p.write_with_backup(&rel(VARIABLES_TOML)?, text)?;
            }
            for (env, text) in &outcome.envs {
                p.write_with_backup(&env_rel(env)?, text)?;
            }
            if let Some(text) = &outcome.new_default_env {
                p.write_with_backup(&env_rel(DEFAULT_ENVIRONMENT)?, text)?;
            }
            Ok(())
        });
        if let Err(e) = result {
            // The transaction put every file back and re-derived memory
            // from disk; all that is left is the offer itself.
            self.pending_migration = Some(outcome);
            return Err(e);
        }
        let mut notes = outcome.notes;
        notes.extend(self.reload_all());
        Ok(notes)
    }

    /// The recorded write: `fs_write_text` both times, so undo removes a
    /// `.bak` this apply created and puts the rewritten file back.
    fn write_with_backup(&mut self, path: &RelPath, text: &str) -> Result<(), Error> {
        let backup = RelPath::new(format!("{}.bak", path.as_str()))?;
        if self.disk.is_file(path) && !self.disk.is_file(&backup) {
            let existing = self.disk.read(path)?.unwrap_or_default();
            self.fs_write_text(&backup, Some(&existing))?;
        }
        self.fs_write_text(path, Some(text))?;
        Ok(())
    }

    pub fn decline_migration(&mut self) {
        self.pending_migration = None;
        self.migration_declined = true;
    }
}
