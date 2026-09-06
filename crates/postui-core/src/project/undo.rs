//! Undo and redo: pop an entry, check every path it will touch, replay
//! its inverses through the same primitives (which record the redo
//! entry), then re-read every document from disk.

use super::*;
use crate::journal::Op;

/// What the app refreshes after an undo or redo: the entry's `meta`
/// exactly as recorded (never swapped), plus which direction just ran.
/// `active_env` is `(before, after)` of the original forward action; the
/// app applies `before` on an undo (`redo` is `false`) and `after` on a
/// redo (`redo` is `true`). `moves` is likewise used as-recorded in both
/// directions. `warnings` are whatever the post-replay reload turned up
/// (a document that no longer parses, a stale reference, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undone {
    pub label: String,
    pub meta: EntryMeta,
    /// `true` for a redo.
    pub redo: bool,
    pub warnings: Vec<Warning>,
}

impl Project {
    pub fn clear_journal(&mut self) {
        self.journal.clear();
    }

    /// Simulates replaying `ops.iter().rev()` — the order `apply_inverse`
    /// actually runs in, for both undo and redo — against a virtual
    /// overlay of path existence seeded from disk. For each op, checks
    /// its inverse's preconditions against the overlay (not raw disk),
    /// then applies the inverse's effect to the overlay before moving to
    /// the next op, so a later check sees what an earlier op in this same
    /// replay will have done. Refuses the whole entry at the first op
    /// whose precondition fails, naming that path.
    fn preflight(&self, ops: &[Op]) -> Result<(), Error> {
        let conflict = |path: &RelPath, what: &str| Error::Conflict(format!("{path} {what}; the step was dropped"));
        let mut overlay: std::collections::HashMap<RelPath, bool> = std::collections::HashMap::new();
        let exists = |overlay: &std::collections::HashMap<RelPath, bool>, p: &RelPath| {
            overlay.get(p).copied().unwrap_or_else(|| self.disk.exists(p))
        };
        for op in ops.iter().rev() {
            match op {
                Op::Renamed { from, to } => {
                    if !exists(&overlay, to) {
                        return Err(conflict(to, "no longer exists"));
                    }
                    if exists(&overlay, from) {
                        return Err(conflict(from, "already exists"));
                    }
                    overlay.insert(to.clone(), false);
                    overlay.insert(from.clone(), true);
                }
                Op::Created { path } => {
                    if !exists(&overlay, path) {
                        return Err(conflict(path, "no longer exists"));
                    }
                    overlay.insert(path.clone(), false);
                }
                Op::Trashed { ticket } => {
                    if !exists(&overlay, &ticket.slot) {
                        return Err(conflict(&ticket.slot, "is gone from the trash"));
                    }
                    if exists(&overlay, &ticket.original) {
                        return Err(conflict(&ticket.original, "already exists"));
                    }
                    overlay.insert(ticket.slot.clone(), false);
                    overlay.insert(ticket.original.clone(), true);
                }
                Op::Restored { ticket } => {
                    if !exists(&overlay, &ticket.original) {
                        return Err(conflict(&ticket.original, "no longer exists"));
                    }
                    if exists(&overlay, &ticket.slot) {
                        return Err(conflict(&ticket.slot, "is occupied in the trash"));
                    }
                    overlay.insert(ticket.original.clone(), false);
                    overlay.insert(ticket.slot.clone(), true);
                }
                Op::Text { path, before, after } => {
                    let expect_present = after.is_some();
                    if expect_present != exists(&overlay, path) {
                        return Err(conflict(path, if expect_present { "no longer exists" } else { "already exists" }));
                    }
                    overlay.insert(path.clone(), before.is_some());
                }
            }
        }
        Ok(())
    }

    /// Reverses one op through the recording primitives.
    fn apply_inverse(&mut self, op: &Op) -> Result<(), Error> {
        match op {
            Op::Renamed { from, to } => self.fs_rename(to, from),
            Op::Created { path } => self.fs_trash(path).map(|_| ()),
            Op::Trashed { ticket } => self.fs_restore(ticket),
            Op::Restored { ticket } => self.fs_retrash(ticket),
            Op::Text { path, before, .. } => self.fs_write_text(path, before.as_deref()),
        }
    }

    /// Applies this entry's `active_env` transition directly (`before` on
    /// undo, `after` on redo). `reload_all`'s `.local/state.toml`-based
    /// restore already covers the steady-state case, but can't recover an
    /// environment switch whose `persist_local_journaled` call was the
    /// *first* write `state.toml` ever got (its recorded `before` is
    /// `None`, so undoing it removes the file rather than restoring old
    /// content) — this uses the entry's own record instead, so it always
    /// wins when the two disagree.
    fn apply_meta_active_env(&mut self, meta: &EntryMeta, redo: bool) -> Vec<Warning> {
        let mut warnings = Vec::new();
        let Some((before, after)) = &meta.active_env else {
            return warnings;
        };
        match if redo { after } else { before } {
            Some(env) if self.environments.contains(env) => {
                if self.active_env.as_deref() != Some(env.as_str()) {
                    match self.load_active_env(env) {
                        Ok(()) => {
                            let _ = self.persist_local();
                        }
                        Err(e) => warnings.push(format!("could not load environment {env:?}: {e}")),
                    }
                }
            }
            Some(env) => warnings.push(format!("environment {env:?} no longer exists")),
            None => {
                self.active_env = None;
                self.env_data = EnvData::default();
                let _ = self.persist_local();
            }
        }
        self.refresh_resolved();
        warnings
    }

    /// Pops nothing — `undo`/`redo` do that and hand the entry here, so
    /// this same replay serves both directions. Preflights (a conflict
    /// drops the entry, unchanged), then applies every op's inverse in
    /// reverse order, recording as it goes so the ops it actually
    /// performed become the opposite stack's entry. On success: reloads
    /// everything, applies this entry's own `active_env` transition,
    /// pushes the replay's ops onto the opposite stack, and returns
    /// `Undone`. On a mid-replay failure (preflight passed but a later op
    /// still failed, e.g. a permission error): reverses the ops already
    /// applied (best effort, same loop as `run_transaction`'s Err path),
    /// reloads, and puts the *original* entry back on the stack it came
    /// from so the user can retry — nothing is lost.
    fn replay(&mut self, entry: Entry, redo: bool) -> Result<Option<Undone>, Error> {
        debug_assert!(self.recording.is_none());
        self.preflight(&entry.ops)?;
        self.recording = Some(Vec::new());
        let mut result = Ok(());
        for op in entry.ops.iter().rev() {
            result = self.apply_inverse(op);
            if result.is_err() {
                break;
            }
        }
        let ops = self.recording.take().unwrap_or_default();
        match result {
            Ok(()) => {
                let mut warnings = self.reload_all();
                warnings.extend(self.apply_meta_active_env(&entry.meta, redo));
                let replayed = Entry {
                    label: entry.label.clone(),
                    ops,
                    meta: entry.meta.clone(),
                    merge: None,
                };
                if redo {
                    self.journal.push_undo_replayed(replayed);
                } else {
                    self.journal.push_redo(replayed);
                }
                Ok(Some(Undone {
                    label: entry.label,
                    meta: entry.meta,
                    redo,
                    warnings,
                }))
            }
            Err(e) => {
                for op in ops.iter().rev() {
                    let _ = self.apply_inverse_unrecorded(op);
                }
                self.reload_all();
                if redo {
                    self.journal.push_redo(entry);
                } else {
                    self.journal.push_undo_replayed(entry);
                }
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> Result<Option<Undone>, Error> {
        let Some(entry) = self.journal.pop_undo() else { return Ok(None) };
        self.replay(entry, false)
    }

    pub fn redo(&mut self) -> Result<Option<Undone>, Error> {
        let Some(entry) = self.journal.pop_redo() else { return Ok(None) };
        self.replay(entry, true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;
    use crate::varedit;

    fn slugs(p: &Project) -> Vec<String> {
        p.requests().iter().map(|l| l.slug.clone()).collect()
    }

    #[test]
    fn undo_and_redo_of_a_rename_move_the_file_back_and_forth() {
        let (dir, mut p) = fixture();
        p.rename_request("main/ping", "main/Pong").unwrap();
        let u = p.undo().unwrap().unwrap();
        assert_eq!(u.label, "rename request");
        assert_eq!(u.meta.moves, vec![("main/ping".to_string(), "main/pong".to_string())]);
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert!(!dir.path().join("requests/main/pong.toml").exists());
        assert!(read(&dir, "requests/main/ping.toml").unwrap().contains("name = \"Ping\""), "the name rewrite is undone too");
        assert!(slugs(&p).contains(&"main/ping".to_string()));
        let r = p.redo().unwrap().unwrap();
        assert!(r.redo);
        assert!(dir.path().join("requests/main/pong.toml").is_file());
        assert!(p.undo().unwrap().is_some());
        assert!(p.undo().unwrap().is_none(), "history exhausted");
    }

    #[test]
    fn undo_of_a_delete_restores_from_trash_and_the_order_list() {
        let (dir, _p) = fixture();
        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\", \"auth\"]\n[space.main]\norder = [\"ping\"]\n").unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.delete_request("main/ping").unwrap();
        assert!(crate::order::space_order(p.meta(), "main").is_empty());
        p.undo().unwrap();
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert_eq!(crate::order::space_order(p.meta(), "main"), ["ping"]);
        p.redo().unwrap();
        assert!(!dir.path().join("requests/main/ping.toml").exists());
    }

    #[test]
    fn undo_of_a_create_trashes_it_and_redo_brings_it_back() {
        let (dir, mut p) = fixture();
        p.create_request("main/New", crate::model::HttpRequest::from_toml_str("method = \"GET\"\nurl = \"u\"\n").unwrap()).unwrap();
        p.undo().unwrap();
        assert!(!dir.path().join("requests/main/new.toml").exists());
        assert!(dir.path().join(".local/trash/1/requests/main/new.toml").is_file());
        p.redo().unwrap();
        assert!(dir.path().join("requests/main/new.toml").is_file());
    }

    #[test]
    fn undo_of_move_all_restores_every_pair_and_stores_no_bodies() {
        let (dir, mut p) = fixture();
        for i in 0..20 {
            std::fs::write(dir.path().join(format!("requests/main/r{i}.toml")), "method = \"GET\"\nurl = \"u\"\n").unwrap();
        }
        p.relist();
        p.move_all_requests("main", "auth").unwrap();
        assert!(!slugs(&p).iter().any(|s| s.starts_with("main/")));
        p.undo().unwrap();
        assert_eq!(slugs(&p).iter().filter(|s| s.starts_with("main/")).count(), 21);
        assert_eq!(slugs(&p).iter().filter(|s| s.starts_with("auth/")).count(), 1);
    }

    #[test]
    fn a_conflict_refuses_the_whole_entry_and_drops_it() {
        let (dir, mut p) = fixture();
        p.rename_request("main/ping", "main/Pong").unwrap();
        std::fs::write(dir.path().join("requests/main/ping.toml"), "method = \"GET\"\nurl = \"hand made\"\n").unwrap();
        let err = p.undo().unwrap_err();
        assert!(matches!(err, Error::Conflict(ref m) if m.contains("requests/main/ping.toml")), "{err}");
        assert!(dir.path().join("requests/main/pong.toml").is_file(), "nothing moved");
        assert!(read(&dir, "requests/main/ping.toml").unwrap().contains("hand made"));
        assert!(p.undo().unwrap().is_none(), "the entry was dropped");
    }

    #[test]
    fn undo_of_a_variable_cascade_restores_every_file_exactly() {
        let (dir, mut p) = fixture();
        let vars_before = read(&dir, "variables.toml").unwrap();
        let dev_before = read(&dir, "environments/dev.toml").unwrap();
        p.cascade("rename var", |p| {
            p.edit_variables(|doc| varedit::rename_var(doc, "host", "hostname"))?;
            p.edit_env("dev", |doc| varedit::rename_env_var(doc, "host", "hostname"))
        })
        .unwrap();
        p.undo().unwrap();
        assert_eq!(read(&dir, "variables.toml").unwrap(), vars_before);
        assert_eq!(read(&dir, "environments/dev.toml").unwrap(), dev_before);
        assert!(p.variables().vars.contains_key("host"));
        assert_eq!(p.resolved().values.get("host").map(String::as_str), Some("dev.local"));
    }

    #[test]
    fn undo_of_a_space_rename_puts_the_local_state_back() {
        let (dir, mut p) = fixture();
        p.set_active_space("auth");
        p.record_space_open(Some("auth/login"));
        p.set_open_request(Some("auth/login"));
        p.set_main_split(Some("60".into()));
        p.persist_local().unwrap();
        p.rename_space("auth", "Login").unwrap();
        p.undo().unwrap();
        assert!(dir.path().join("requests/auth/login.toml").is_file());
        assert_eq!(p.local().active_space, "auth");
        assert_eq!(p.local().space_open.get("auth").map(String::as_str), Some("auth/login"));
        assert_eq!(p.local().open_request.as_deref(), Some("auth/login"));
        assert_eq!(p.local().main_split.as_deref(), Some("60"));
        assert_eq!(p.spaces(), ["main", "auth"]);
    }

    #[test]
    fn undo_of_an_environment_delete_restores_the_file_secrets_and_active_env() {
        let (dir, mut p) = fixture();
        p.set_secret_for("dev", "token", "x".into()).unwrap();
        p.delete_environment("dev").unwrap();
        let u = p.undo().unwrap().unwrap();
        assert_eq!(u.meta.active_env, Some((Some("dev".into()), Some("qa".into()))));
        assert!(dir.path().join("environments/dev.toml").is_file());
        assert_eq!(p.environments(), ["dev", "qa"]);
        assert_eq!(p.secrets().get("dev").and_then(|m| m.get("token")).map(String::as_str), Some("x"));
        assert_eq!(p.env_name("dev"), "Dev");
        assert_eq!(p.active_env(), Some("dev"));
    }

    #[test]
    #[cfg(unix)]
    fn a_replay_that_fails_midway_rolls_back_and_keeps_the_entry() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = fixture();
        let a = RelPath::new("requests/main/ping.toml").unwrap();
        let b = RelPath::new("requests/main/pong.toml").unwrap();
        let newdir = RelPath::new("requests/newdir").unwrap();
        // An entry with two ops: a rename inside `requests/main` (its
        // inverse will be blocked below) and a directory create outside
        // it (its inverse — trashing — is unaffected, so it applies and
        // records fine before the rename's inverse fails).
        p.transaction("t", EntryMeta::default(), |p| {
            p.fs_rename(&a, &b)?;
            p.fs_create_dir(&newdir)
        })
        .unwrap();
        assert_eq!(p.journal_len(), 1);
        let main_dir = dir.path().join("requests/main");
        let original_perms = std::fs::metadata(&main_dir).unwrap().permissions();
        // No write permission on `requests/main`: renaming inside it
        // (undoing the `fs_rename`) will fail with a permission error,
        // deterministically, after the directory-create's inverse
        // (trashing `requests/newdir`, elsewhere) has already applied.
        std::fs::set_permissions(&main_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = p.undo();
        std::fs::set_permissions(&main_dir, original_perms).unwrap();
        assert!(result.is_err(), "{result:?}");
        assert!(
            dir.path().join("requests/newdir").is_dir(),
            "the first-applied inverse (trashing newdir) was rolled back"
        );
        assert!(dir.path().join("requests/main/pong.toml").is_file(), "the rename was never touched");
        assert!(p.can_undo());
        assert_eq!(p.journal_len(), 1);
        assert!(!p.can_redo());
    }

    #[test]
    fn preflight_simulates_earlier_ops_in_the_entry() {
        let (dir, mut p) = fixture();
        let a = RelPath::new("requests/main/ping.toml").unwrap();
        let b = RelPath::new("requests/main/pong.toml").unwrap();
        p.transaction("t", EntryMeta::default(), |p| {
            p.fs_rename(&a, &b)?;
            p.fs_write_text(&b, Some("name = \"Pong\"\nmethod = \"GET\"\nurl = \"u\"\n"))
        })
        .unwrap();
        p.undo().unwrap();
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert!(!dir.path().join("requests/main/pong.toml").exists());
        p.redo().unwrap();
        assert!(dir.path().join("requests/main/pong.toml").is_file());
        assert!(!dir.path().join("requests/main/ping.toml").exists());
    }

    #[test]
    fn clear_journal_forgets_both_stacks() {
        let (_d, mut p) = fixture();
        p.create_space("x").unwrap();
        p.undo().unwrap();
        assert!(p.can_redo());
        p.clear_journal();
        assert!(!p.can_undo() && !p.can_redo());
    }
}
