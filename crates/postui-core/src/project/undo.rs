//! Undo and redo: pop an entry, check every path it will touch, replay
//! its inverses through the same primitives (which record the redo
//! entry), then re-read every document from disk.

use super::*;
use crate::journal::{EntryId, Op};

/// What the app refreshes after an undo or redo: the entry's `meta`
/// exactly as recorded (never swapped), plus which direction just ran.
/// `active_env` is `(before, after)` of the original forward action; the
/// app applies `before` on an undo (`redo` is `false`) and `after` on a
/// redo (`redo` is `true`). `moves` is likewise used as-recorded in both
/// directions. `warnings` are whatever the post-replay reload turned up
/// (a document that no longer parses, a stale reference, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undone {
    pub id: EntryId,
    pub label: String,
    pub meta: EntryMeta,
    /// `true` for a redo.
    pub redo: bool,
    pub warnings: Vec<Warning>,
}

/// What an undo or redo is refused with. The user never sees the
/// machinery, only the cause: something changed the file after the op
/// that is being replayed.
fn conflict(path: &RelPath, what: &str) -> Error {
    Error::Conflict(format!("{path} {what} (changed outside the app)"))
}

/// Whether the app also writes `path` outside the journal, by a write the
/// journal does not follow: `state.toml` (`persist_local`) and a request's
/// file (`save_request`), taken to be any path under `requests/` but a
/// folder's file.
fn written_outside_journal(path: &RelPath) -> bool {
    path.as_str() == STATE_TOML
        || (path.as_str().starts_with(&format!("{REQUESTS_DIR}/"))
            && path.file_name() != crate::storage::FOLDER_FILE)
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
    ///
    /// A created directory is taken back whole (`fs_trash`), so its
    /// inverse also needs it to hold nothing postui shows but what this
    /// replay has already taken out of it: a request or folder written into
    /// it since (an editor, an agent) refuses the entry rather than going
    /// to the trash unsaid. What postui doesn't show (a `.DS_Store`, an
    /// editor's swap file, a `README`) goes to the trash with the
    /// directory, and redo brings it back (spec 2026-11-03 create folders,
    /// review rounds 5 and 6; mine (approved: "A")).
    ///
    /// A text op's inverse writes the text from before the op, so the file
    /// must still hold the text the op left: text changed since would be
    /// written over, and is refused as a conflict instead (#283, mine).
    /// Two kinds of file are not compared, because what they hold since
    /// the op can be the app's own ([`written_outside_journal`]); a request
    /// rename's undo over an outside edit of the request is #285.
    fn preflight(&mut self, ops: &[Op]) -> Result<(), Error> {
        let mut overlay: std::collections::HashMap<RelPath, bool> =
            std::collections::HashMap::new();
        fn exists(
            disk: &crate::disk::Disk,
            overlay: &std::collections::HashMap<RelPath, bool>,
            p: &RelPath,
        ) -> bool {
            overlay.get(p).copied().unwrap_or_else(|| disk.exists(p))
        }
        for op in ops.iter().rev() {
            match op {
                Op::Renamed { from, to } => {
                    if !exists(&self.disk, &overlay, to) {
                        return Err(conflict(to, "no longer exists"));
                    }
                    if exists(&self.disk, &overlay, from) {
                        return Err(conflict(from, "already exists"));
                    }
                    overlay.insert(to.clone(), false);
                    overlay.insert(from.clone(), true);
                }
                Op::Created { path } => {
                    if !exists(&self.disk, &overlay, path) {
                        return Err(conflict(path, "no longer exists"));
                    }
                    if self.disk.is_dir(path) {
                        // What it holds that postui shows (a folder, a
                        // request file, neither hidden), once this replay
                        // has taken out what the entry put in it: on disk
                        // and not marked gone, or put back by an earlier
                        // inverse.
                        let shown = |name: &str, is_dir: bool| {
                            !super::is_hidden(name) && (is_dir || name.ends_with(".toml"))
                        };
                        let mut held = false;
                        for e in self.disk.list(path)? {
                            let Ok(child) = path.join(&e.name) else { continue };
                            held |=
                                shown(&e.name, e.is_dir) && exists(&self.disk, &overlay, &child);
                        }
                        let inside = format!("{path}/");
                        held |= overlay.iter().any(|(p, &present)| {
                            present
                                && p.as_str()
                                    .strip_prefix(&inside)
                                    // The overlay knows no kind: a name with
                                    // no extension reads as a folder.
                                    .is_some_and(|r| !r.contains('/') && shown(r, !r.contains('.')))
                        });
                        if held {
                            return Err(conflict(path, "holds something added since"));
                        }
                    }
                    overlay.insert(path.clone(), false);
                }
                Op::Trashed { ticket } => {
                    if !exists(&self.disk, &overlay, &ticket.slot) {
                        return Err(conflict(&ticket.slot, "is gone from the trash"));
                    }
                    if exists(&self.disk, &overlay, &ticket.original) {
                        return Err(conflict(&ticket.original, "already exists"));
                    }
                    overlay.insert(ticket.slot.clone(), false);
                    overlay.insert(ticket.original.clone(), true);
                }
                Op::Restored { ticket } => {
                    // Known gap, left for #243: a restored directory (a
                    // folder or a space put back by undo) goes back to the
                    // trash whole, with no "holds something added since"
                    // check as `Op::Created` has. The journal records the
                    // path, not what the directory held, so it can't tell
                    // what came later. What goes is still in the trash
                    // until the project is next opened, and one more undo
                    // brings it back. #243 replaces whole-directory undo
                    // for spaces and folders alike (main has the space
                    // case); patching it here would change the journal's
                    // format (#172, option A: mine (approved)).
                    if !exists(&self.disk, &overlay, &ticket.original) {
                        return Err(conflict(&ticket.original, "no longer exists"));
                    }
                    if exists(&self.disk, &overlay, &ticket.slot) {
                        return Err(conflict(&ticket.slot, "is occupied in the trash"));
                    }
                    overlay.insert(ticket.original.clone(), false);
                    overlay.insert(ticket.slot.clone(), true);
                }
                Op::Text { path, before, after } => {
                    let expect_present = after.is_some();
                    if expect_present != exists(&self.disk, &overlay, path) {
                        return Err(conflict(
                            path,
                            if expect_present { "no longer exists" } else { "already exists" },
                        ));
                    }
                    // Compared where this replay first meets the path: after
                    // that the file holds what an earlier inverse put there,
                    // which is not on disk yet. A peek, so a change `poll`
                    // has not read yet is still `poll`'s to read.
                    if !overlay.contains_key(path)
                        && !written_outside_journal(path)
                        && self.disk.peek(path)? != *after
                    {
                        return Err(conflict(path, "has been edited since"));
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
    /// undo, `after` on redo). `reload_all`'s `state.toml`-based
    /// restore already covers the steady-state case; this uses the entry's
    /// own record instead, so it always wins when the two disagree. (Before
    /// `run_transaction` started materialising `state.toml` up
    /// front, this was also the only thing that saved an environment
    /// switch whose `persist_local_journaled` call was the file's first
    /// ever write — a recorded `before: None` that undo would turn into
    /// deleting the file. `run_transaction` now rules that case out for
    /// every journaled `state.toml` write, not only this one, but this
    /// method is still the source of truth for `active_env` on undo/redo.)
    ///
    /// Its `state.toml` write is journaled (`persist_local_journaled`) and
    /// [`Self::replay`] calls this while the replay is still recording, so
    /// the write becomes an op of the entry the opposite stack gets. An
    /// unrecorded write here would leave `state.toml` present where that
    /// entry's `preflight` expects it absent, and the opposite direction
    /// would be refused as a conflict.
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
                            let _ = self.persist_local_journaled();
                        }
                        Err(e) => warnings.push(format!("could not load environment {env:?}: {e}")),
                    }
                }
            }
            Some(env) => warnings.push(format!("environment {env:?} no longer exists")),
            None => {
                self.active_env = None;
                self.env_data = EnvData::default();
                let _ = self.persist_local_journaled();
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
        if let Err(e) = self.preflight(&entry.ops) {
            // The entry is dropped, and its text stays in the files it
            // wrote. The journal follows that text as the app's own, so
            // the entry beneath that wrote one of those files is compared
            // with it (review round 2, mine).
            for op in &entry.ops {
                self.journal.follow(op);
            }
            return Err(e);
        }
        // Which held entries still matched their file before the replay
        // rewrote anything — the only ones `rekey_held_after_own_op` may
        // re-stamp below. Taken here, before the first inverse runs.
        let clean_before = self.held_clean_before();
        self.recording = Some(Vec::new());
        let mut result = Ok(());
        for op in entry.ops.iter().rev() {
            result = self.apply_inverse(op);
            if result.is_err() {
                break;
            }
        }
        // The reload and the environment restore run *inside* the
        // recording window, so the `state.toml` write the restore makes is
        // an op of the entry the opposite stack gets (see
        // `apply_meta_active_env`).
        let mut warnings = Vec::new();
        if result.is_ok() {
            warnings = self.reload_all();
            // Every request this entry moved changes slug again; a held
            // entry follows its file, and is re-stamped only if it was
            // clean going in (the replay's own writes are never an
            // outside edit — anything else is). `moves` is recorded in
            // the forward direction, so an undo walks the pairs backwards.
            let moves: Vec<(String, String)> = entry
                .meta
                .moves
                .iter()
                .map(
                    |(old, new)| {
                        if redo { (old.clone(), new.clone()) } else { (new.clone(), old.clone()) }
                    },
                )
                .collect();
            self.rekey_held_after_own_op(&moves, &clean_before);
            warnings.extend(self.apply_meta_active_env(&entry.meta, redo));
        }
        let ops = self.recording.take().unwrap_or_default();
        match result {
            Ok(()) => {
                let replayed = Entry {
                    id: entry.id,
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
                    id: entry.id,
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
    use super::super::tests::{
        assert_none_lost, fixture, read, request_files, requests_tree, sizes,
    };
    use super::*;
    use crate::varedit;

    fn slugs(p: &Project) -> Vec<String> {
        p.requests().iter().map(|l| l.slug.clone()).collect()
    }

    #[test]
    fn undo_and_redo_of_a_rename_move_the_file_back_and_forth() {
        let (dir, mut p) = fixture();
        p.rename_request("main/ping", "Pong").unwrap();
        let u = p.undo().unwrap().unwrap();
        assert_eq!(u.label, "rename request");
        assert_eq!(u.meta.moves, vec![("main/ping".to_string(), "main/pong".to_string())]);
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert!(!dir.path().join("requests/main/pong.toml").exists());
        assert!(
            read(&dir, "requests/main/ping.toml").unwrap().contains("name = \"Ping\""),
            "the name rewrite is undone too"
        );
        assert!(slugs(&p).contains(&"main/ping".to_string()));
        let r = p.redo().unwrap().unwrap();
        assert!(r.redo);
        assert!(dir.path().join("requests/main/pong.toml").is_file());
        assert!(p.undo().unwrap().is_some());
        assert!(p.undo().unwrap().is_none(), "history exhausted");
    }

    #[test]
    fn undo_of_an_environment_reorder_restores_the_exact_previous_array() {
        let (dir, _p) = fixture();
        let text = read(&dir, "project.toml").unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            format!("environments = [\"dev\", \"qa\"]\n{text}"),
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        p.move_environment("dev", 1).unwrap();
        assert_eq!(p.meta().environments, ["qa", "dev"]);
        let u = p.undo().unwrap().unwrap();
        assert_eq!(u.label, "move environment");
        assert_eq!(p.meta().environments, ["dev", "qa"]);
        assert_eq!(p.environments(), ["dev", "qa"]);
        p.redo().unwrap();
        assert_eq!(p.meta().environments, ["qa", "dev"]);

        p.set_environment_order(&["dev".into(), "qa".into()]).unwrap();
        let u = p.undo().unwrap().unwrap();
        assert_eq!(u.label, "reorder environments");
        assert_eq!(p.meta().environments, ["qa", "dev"]);
        assert_eq!(p.environments(), ["qa", "dev"]);
    }

    #[test]
    fn undo_of_a_delete_restores_from_trash_and_the_order_list() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("project.toml"),
            "spaces = [\"main\", \"auth\"]\n[space.main]\norder = [\"ping\"]\n",
        )
        .unwrap();
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
        p.create_request(
            "main/New",
            crate::model::HttpRequest::from_toml_str("method = \"GET\"\nurl = \"u\"\n").unwrap(),
        )
        .unwrap();
        p.undo().unwrap();
        assert!(!dir.path().join("requests/main/new.toml").exists());
        assert!(dir.path().join(TRASH_DIR).join("1/requests/main/new.toml").is_file());
        p.redo().unwrap();
        assert!(dir.path().join("requests/main/new.toml").is_file());
    }

    #[test]
    fn undo_of_move_all_restores_every_pair_and_stores_no_bodies() {
        let (dir, mut p) = fixture();
        for i in 0..20 {
            std::fs::write(
                dir.path().join(format!("requests/main/r{i}.toml")),
                "method = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
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
        p.rename_request("main/ping", "Pong").unwrap();
        std::fs::write(
            dir.path().join("requests/main/ping.toml"),
            "method = \"GET\"\nurl = \"hand made\"\n",
        )
        .unwrap();
        let err = p.undo().unwrap_err();
        assert!(
            matches!(err, Error::Conflict(ref m) if m.contains("requests/main/ping.toml")),
            "{err}"
        );
        assert!(dir.path().join("requests/main/pong.toml").is_file(), "nothing moved");
        assert!(read(&dir, "requests/main/ping.toml").unwrap().contains("hand made"));
        assert!(p.undo().unwrap().is_none(), "the entry was dropped");
    }

    /// The outside world appends a line to `file`. Returns its new text.
    fn edit_outside(dir: &tempfile::TempDir, file: &str) -> String {
        let text = format!("{}# outside\n", read(dir, file).unwrap());
        std::fs::write(dir.path().join(file), &text).unwrap();
        text
    }

    /// `r` is the conflict an undo or redo over changed text gives, naming
    /// `file`.
    #[track_caller]
    fn assert_edited_since(r: Result<Option<Undone>, Error>, file: &str) {
        let Err(Error::Conflict(m)) = &r else { panic!("{r:?}") };
        assert!(m.starts_with(file) && m.contains("has been edited since"), "{m}");
    }

    fn add_port(p: &mut Project) {
        p.edit_variables(|doc| varedit::upsert_var(doc, "port", None, Some("8080"))).unwrap();
    }

    /// #283: a file changed outside since the op wrote it refuses the undo,
    /// which would write the op's old text over the change. The file keeps
    /// the change, the entry is dropped, and nothing is read: a change the
    /// project has not re-read yet is still the next `poll`'s.
    #[test]
    fn undo_over_text_changed_outside_is_refused_and_keeps_it() {
        for polled in [false, true] {
            let (dir, mut p) = fixture();
            add_port(&mut p);
            let outside = edit_outside(&dir, "variables.toml");
            if polled {
                assert!(p.poll().0);
            }
            assert_edited_since(p.undo(), "variables.toml");
            assert_eq!(read(&dir, "variables.toml").unwrap(), outside, "polled {polled}");
            assert!(!p.can_undo() && !p.can_redo(), "polled {polled}: the entry was dropped");
            assert_eq!(p.changed_on_disk(), !polled);
        }
    }

    #[test]
    fn redo_over_text_changed_outside_is_refused_and_keeps_it() {
        let (dir, mut p) = fixture();
        add_port(&mut p);
        p.undo().unwrap().unwrap();
        let outside = edit_outside(&dir, "variables.toml");
        assert_edited_since(p.redo(), "variables.toml");
        assert_eq!(read(&dir, "variables.toml").unwrap(), outside);
        assert!(!p.can_undo() && !p.can_redo(), "the entry was dropped");
    }

    /// An op made after the outside change took that text in, so its undo
    /// puts it back. The op before the change left other text, so its undo
    /// is refused.
    #[test]
    fn the_step_before_an_outside_change_is_refused_after_the_one_made_on_it_undoes() {
        let (dir, mut p) = fixture();
        add_port(&mut p);
        let outside = edit_outside(&dir, "variables.toml");
        p.poll();
        p.edit_variables(|doc| varedit::upsert_var(doc, "path", None, Some("/v1"))).unwrap();
        p.undo().unwrap().unwrap();
        assert_eq!(read(&dir, "variables.toml").unwrap(), outside);
        assert_edited_since(p.undo(), "variables.toml");
        assert_eq!(read(&dir, "variables.toml").unwrap(), outside);
    }

    /// The refusal comes before any inverse runs. The cascade's environment
    /// file, whose inverse comes first, is not written.
    #[test]
    fn a_refused_cascade_writes_none_of_its_files() {
        let (dir, mut p) = fixture();
        p.cascade("rename var", |p| {
            p.edit_variables(|doc| varedit::rename_var(doc, "host", "hostname"))?;
            p.edit_env("dev", |doc| varedit::rename_env_var(doc, "host", "hostname"))
        })
        .unwrap();
        let dev = read(&dir, "environments/dev.toml").unwrap();
        let written = || {
            std::fs::metadata(dir.path().join("environments/dev.toml")).unwrap().modified().unwrap()
        };
        let before = written();
        let outside = edit_outside(&dir, "variables.toml");
        assert_edited_since(p.undo(), "variables.toml");
        assert_eq!(read(&dir, "variables.toml").unwrap(), outside);
        assert_eq!(read(&dir, "environments/dev.toml").unwrap(), dev);
        assert_eq!(written(), before, "not rewritten");
        assert!(p.variables().vars.contains_key("hostname"), "memory is as it was");
        assert!(!p.can_undo(), "the entry was dropped");
    }

    /// The same for a space delete: the space stays in the trash.
    #[test]
    fn a_refused_undo_of_a_space_delete_leaves_the_space_in_the_trash() {
        let (dir, mut p) = fixture();
        p.delete_space("auth").unwrap();
        let outside = edit_outside(&dir, "project.toml");
        assert_edited_since(p.undo(), "project.toml");
        assert_eq!(read(&dir, "project.toml").unwrap(), outside);
        assert!(!dir.path().join("requests/auth").exists());
        assert!(dir.path().join(TRASH_DIR).join("1/requests/auth/login.toml").is_file());
        assert_eq!(p.spaces(), ["main"]);
    }

    /// A folder's file is compared like any document: a rename's undo is
    /// refused over a change to it, and the folder keeps its new directory.
    #[test]
    fn undo_of_a_folder_rename_over_its_changed_file_is_refused() {
        let (dir, mut p) = fixture();
        p.create_folder("main", "Users", None).unwrap();
        p.rename_folder("main/users", "People").unwrap();
        let outside = edit_outside(&dir, "requests/main/people/.folder.toml");
        let project = read(&dir, "project.toml");
        assert_edited_since(p.undo(), "requests/main/people/.folder.toml");
        assert_eq!(read(&dir, "requests/main/people/.folder.toml").unwrap(), outside);
        assert!(!dir.path().join("requests/main/users").exists());
        assert_eq!(read(&dir, "project.toml"), project);
    }

    /// The TUI's refresh completes an order list outside the journal when
    /// a request appears from outside. That write is the app's own: the
    /// entry from before it still undoes, and redoes after the refresh
    /// has completed the list the undo put back.
    #[test]
    fn undo_and_redo_past_an_order_completion_outside_the_journal_still_work() {
        let (dir, mut p) = fixture();
        p.complete_order().unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        std::fs::write(dir.path().join("requests/main/added.toml"), "url = \"u\"\n").unwrap();
        assert!(p.complete_order().unwrap(), "the refresh writes the new entry");
        p.undo().unwrap().unwrap();
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert!(p.complete_order().unwrap(), "and again, in the list the undo put back");
        p.redo().unwrap().unwrap();
        assert!(dir.path().join("requests/main/pong.toml").is_file());
        assert_eq!(crate::order::space_order(p.meta(), "main"), ["pong", "added"]);
    }

    /// The same when the op itself first listed the request made outside:
    /// its undo puts back a list without it, which the refresh completes
    /// outside the journal before the redo, or before the undo of the step
    /// beneath.
    #[test]
    fn redo_after_a_refresh_completed_the_list_the_undo_put_back_still_works() {
        let (dir, mut p) = fixture();
        p.complete_order().unwrap();
        p.rename_request("auth/login", "Signin").unwrap();
        std::fs::write(dir.path().join("requests/main/added.toml"), "url = \"u\"\n").unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        p.undo().unwrap().unwrap();
        assert!(p.complete_order().unwrap(), "the refresh lists the request again");
        p.redo().unwrap().unwrap();
        assert!(dir.path().join("requests/main/pong.toml").is_file());
        p.undo().unwrap().unwrap();
        p.undo().unwrap().unwrap();
        assert!(dir.path().join("requests/auth/login.toml").is_file(), "the step beneath");
    }

    /// A completion written over text changed outside does not cover for
    /// that change: the entry from before it is still refused.
    #[test]
    fn an_order_completion_over_an_outside_change_still_refuses_the_undo() {
        let (dir, mut p) = fixture();
        p.complete_order().unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        edit_outside(&dir, "project.toml");
        std::fs::write(dir.path().join("requests/main/added.toml"), "url = \"u\"\n").unwrap();
        assert!(p.complete_order().unwrap());
        assert_edited_since(p.undo(), "project.toml");
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("# outside") && text.contains("added"), "{text}");
        assert!(dir.path().join("requests/main/pong.toml").is_file());
    }

    /// A refused entry is dropped with its text still in the files it
    /// wrote. The entry beneath that wrote `variables.toml` still undoes,
    /// and the dropped entry's change to that file goes with its own. The
    /// one beneath that wrote the environment file changed outside is
    /// refused.
    #[test]
    fn the_steps_beneath_a_refused_one_are_refused_only_over_the_changed_file() {
        let (dir, mut p) = fixture();
        p.edit_env("dev", |doc| varedit::set_env_value(doc, "host", Some("dev2.local"))).unwrap();
        let variables = read(&dir, "variables.toml");
        add_port(&mut p);
        p.cascade("rename var", |p| {
            p.edit_variables(|doc| varedit::rename_var(doc, "host", "hostname"))?;
            p.edit_env("dev", |doc| varedit::rename_env_var(doc, "host", "hostname"))
        })
        .unwrap();
        let outside = edit_outside(&dir, "environments/dev.toml");
        assert_edited_since(p.undo(), "environments/dev.toml");
        p.undo().unwrap().unwrap();
        assert_eq!(read(&dir, "variables.toml"), variables);
        assert_edited_since(p.undo(), "environments/dev.toml");
        assert_eq!(read(&dir, "environments/dev.toml").unwrap(), outside);
    }

    /// The same for a redo refused over a path: the redo beneath it, which
    /// wrote `project.toml` too, still redoes.
    #[test]
    fn the_redo_beneath_a_refused_one_still_redoes() {
        let (dir, mut p) = fixture();
        p.complete_order().unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        p.rename_request("auth/login", "Signin").unwrap();
        p.undo().unwrap().unwrap();
        p.undo().unwrap().unwrap();
        std::fs::write(dir.path().join("requests/main/pong.toml"), "url = \"u\"\n").unwrap();
        let err = p.redo().unwrap_err();
        assert!(matches!(err, Error::Conflict(ref m) if m.contains("already exists")), "{err}");
        p.redo().unwrap().unwrap();
        assert!(dir.path().join("requests/auth/signin.toml").is_file());
    }

    /// A refused redo writes nothing. The rename's inverse of the open
    /// request's name comes before `project.toml`'s, and the request
    /// reports no drift.
    #[test]
    fn a_refused_redo_leaves_an_open_request_reporting_no_drift() {
        let (dir, mut p) = fixture();
        p.complete_order().unwrap();
        p.open_request("main/ping").unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        p.undo().unwrap().unwrap();
        assert_eq!(p.held_request_drift("main/ping"), None);
        let request = read(&dir, "requests/main/ping.toml");
        edit_outside(&dir, "project.toml");
        assert_edited_since(p.redo(), "project.toml");
        assert_eq!(read(&dir, "requests/main/ping.toml"), request);
        assert_eq!(p.held_request_drift("main/ping"), None);
    }

    /// A save writes a request's file outside the journal. Its text is not
    /// compared, so the app's own save never refuses a rename's undo.
    #[test]
    fn a_save_after_a_rename_never_refuses_its_undo() {
        let (dir, mut p) = fixture();
        p.rename_request("main/ping", "Pong").unwrap();
        let mut req = p.read_request("main/pong").unwrap();
        req.url = "https://saved".into();
        p.save_request("main/pong", &req).unwrap();
        p.undo().unwrap().unwrap();
        assert!(read(&dir, "requests/main/ping.toml").unwrap().contains("name = \"Ping\""));
    }

    /// The same for `state.toml`, which every UI change writes outside the
    /// journal.
    #[test]
    fn a_ui_change_after_a_space_rename_never_refuses_its_undo() {
        let (_dir, mut p) = fixture();
        p.set_active_space("auth");
        p.rename_space("auth", "Login").unwrap();
        p.set_expanded(["login/deep".to_string()].into_iter().collect());
        p.undo().unwrap().unwrap();
        assert_eq!(p.local().active_space, "auth");
    }

    /// A directory an op made is taken back whole, so a file written into
    /// it since refuses the undo: it never goes to the trash unsaid.
    #[test]
    fn undo_of_a_made_directory_holding_a_file_added_since_is_refused() {
        let (dir, mut p) = fixture();
        p.create_folder("main", "Users", None).unwrap();
        std::fs::write(dir.path().join("requests/main/users/y.toml"), "url = \"u\"\n").unwrap();
        let err = p.undo().unwrap_err();
        assert!(
            matches!(err, Error::Conflict(ref m) if m.contains("requests/main/users")),
            "{err}"
        );
        assert!(dir.path().join("requests/main/users/y.toml").is_file(), "nothing went");
        assert!(dir.path().join("requests/main/users/.folder.toml").is_file());
    }

    /// What postui doesn't show (a `.DS_Store`, a swap file) never blocks
    /// the undo of a directory an op made: it goes to the trash with the
    /// directory, and redo brings it back.
    #[test]
    fn undo_of_a_made_directory_takes_unshown_files_with_it() {
        let (dir, mut p) = fixture();
        p.create_folder("main", "Users", None).unwrap();
        let users = dir.path().join("requests/main/users");
        std::fs::write(users.join(".DS_Store"), "junk").unwrap();
        std::fs::write(users.join("notes.txt"), "hi").unwrap();
        p.undo().unwrap().unwrap();
        assert!(!users.exists());
        p.redo().unwrap().unwrap();
        assert_eq!(read(&dir, "requests/main/users/.DS_Store").as_deref(), Some("junk"));
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
        p.set_pane_sizes(sizes("60", 30));
        p.persist_local().unwrap();
        p.rename_space("auth", "Login").unwrap();
        p.undo().unwrap();
        assert!(dir.path().join("requests/auth/login.toml").is_file());
        assert_eq!(p.local().active_space, "auth");
        assert_eq!(p.local().space_open.get("auth").map(String::as_str), Some("auth/login"));
        assert_eq!(p.local().open_request.as_deref(), Some("auth/login"));
        assert_eq!(p.local().layout, sizes("60", 30));
        assert_eq!(p.spaces(), ["main", "auth"]);
    }

    #[test]
    fn undo_and_redo_keep_the_pane_sizes_set_after_the_op() {
        let (dir, mut p) = fixture();
        p.set_pane_sizes(sizes("60", 30));
        p.persist_local().unwrap();
        p.rename_space("auth", "Login").unwrap();
        p.set_pane_sizes(sizes("70", 40));
        p.undo().unwrap();
        assert_eq!(p.local().layout, sizes("70", 40));
        let text = read(&dir, LAYOUT_TOML).unwrap();
        assert!(
            text.contains("main_split = \"70\"") && text.contains("sidebar_width = 40"),
            "{text}"
        );
        p.redo().unwrap();
        assert_eq!(p.local().layout, sizes("70", 40));
        let (p2, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(p2.local().layout, sizes("70", 40));
    }

    /// An entry's op count and how many of its ops write `state.toml`.
    type Counts = (usize, usize);

    /// `e`'s [`Counts`].
    fn op_counts(e: &Entry) -> Counts {
        let state = e
            .ops
            .iter()
            .filter(|op| matches!(op, Op::Text { path, .. } if path.as_str() == STATE_TOML))
            .count();
        (e.ops.len(), state)
    }

    /// A space rename whose entry rewrites `state.toml` (the space is the
    /// active one) and a resize, then five undo/redo pairs, with `resize`
    /// run before each undo and each redo. Returns the rename's entry's
    /// counts and, per cycle, the counts of the entry the undo pushed and
    /// the one the redo pushed.
    fn cycle_counts(resize: impl Fn(&mut Project, u16)) -> (Counts, Vec<(Counts, Counts)>) {
        let (_dir, mut p) = fixture();
        p.set_active_space("auth");
        p.persist_local().unwrap();
        p.rename_space("auth", "Login").unwrap();
        let first = op_counts(p.journal.peek_undo().unwrap());
        p.set_pane_sizes(sizes("70", 30));
        let mut cycles = Vec::new();
        for i in 0..5u16 {
            resize(&mut p, 2 * i);
            p.undo().unwrap().unwrap();
            let undone = op_counts(p.journal.peek_redo().unwrap());
            resize(&mut p, 2 * i + 1);
            p.redo().unwrap().unwrap();
            let redone = op_counts(p.journal.peek_undo().unwrap());
            cycles.push((undone, redone));
        }
        (first, cycles)
    }

    /// Review round 7 M1: a resize after the op never reaches the journal
    /// (the sizes live in `layout.toml`, spec P3b), so every replay's entry
    /// is exactly the op's own.
    #[test]
    fn undo_redo_after_a_resize_keeps_entries_bounded() {
        let (first, cycles) = cycle_counts(|_, _| {});
        assert_eq!(cycles, vec![(first, first); 5], "the rename's entry: {first:?}");
    }

    /// Review round 7 M1: a resize before each undo and each redo used to
    /// make `reload_all`'s write-back a new op every time, so each replay's
    /// entry was one op longer than the last. The sizes now live in the
    /// unjournaled `layout.toml` (spec P3b), so no replay ever records them.
    #[test]
    fn undo_redo_with_a_resize_between_keeps_entries_bounded() {
        let (first, cycles) = cycle_counts(|p, n| p.set_pane_sizes(sizes("70", 31 + n)));
        assert_eq!(cycles, vec![(first, first); 5], "the rename's entry: {first:?}");
    }

    /// `state.toml` does not exist yet when this op runs.
    /// `run_transaction` now materialises it from pre-op memory before any
    /// journaled write happens, so this op's own `state.toml` write is no
    /// longer recorded as `before: None` and undo no longer deletes the
    /// file. Kept as a regression test for the case that motivated
    /// `apply_meta_active_env`'s own protection.
    #[test]
    fn redo_survives_an_env_switch_whose_entry_first_created_state_toml() {
        let (dir, mut p) = fixture();
        assert!(read(&dir, STATE_TOML).is_none(), "never written yet");
        let slug = p.create_environment("QA 2").unwrap();
        assert_eq!(p.active_env(), Some(slug.as_str()));
        p.undo().unwrap().unwrap();
        assert_eq!(p.active_env(), Some("dev"));
        p.redo().unwrap().unwrap();
        assert_eq!(p.active_env(), Some("qa-2"));
        assert!(read(&dir, STATE_TOML).unwrap().contains("environment = \"qa-2\""));
    }

    /// Same as above, `delete_environment`'s path.
    #[test]
    fn redo_of_an_environment_delete_survives_the_same_first_state_toml_write() {
        let (dir, mut p) = fixture();
        assert!(read(&dir, STATE_TOML).is_none(), "never written yet");
        p.delete_environment("dev").unwrap();
        assert_eq!(p.active_env(), Some("qa"));
        p.undo().unwrap().unwrap();
        assert_eq!(p.active_env(), Some("dev"));
        p.redo().unwrap().unwrap();
        assert_eq!(p.active_env(), Some("qa"));
        assert_eq!(p.environments(), ["qa"]);
        assert!(read(&dir, STATE_TOML).unwrap().contains("environment = \"qa\""));
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
        assert_eq!(
            p.secrets().get("dev").and_then(|m| m.get("token")).map(String::as_str),
            Some("x")
        );
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
        assert!(
            dir.path().join("requests/main/pong.toml").is_file(),
            "the rename was never touched"
        );
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

    /// Space `auth`'s directory was renamed by hand to `Auth/` after a move
    /// or a delete took a request out of it. The undo is an ordinary one
    /// (#278): the request goes back to `requests/auth/…`, what `Auth/`
    /// holds is kept, and redo takes the request out again. Where the disk
    /// folds case, the test checks only that no request file is lost, the
    /// trash counted: that arm was traced, not run.
    #[test]
    fn undo_into_a_space_whose_directory_is_in_another_case_is_ordinary() {
        let ops: [fn(&mut Project); 2] = [
            |p| drop(p.move_request("auth/login", "main").unwrap()),
            |p| p.delete_request("auth/login").unwrap(),
        ];
        for (is_delete, op) in [false, true].into_iter().zip(ops) {
            let (dir, mut p) = fixture();
            let kept = dir.path().join("requests/Auth/kept.toml");
            std::fs::write(dir.path().join("requests/auth/kept.toml"), "url = \"u\"\n").unwrap();
            let files = request_files(&dir);
            op(&mut p);
            let requests = dir.path().join("requests");
            std::fs::rename(requests.join("auth"), requests.join("Auth")).unwrap();
            if requests.join("auth").exists() {
                let _ = p.undo();
                assert_none_lost(&dir, &files);
                let _ = p.redo();
                assert_none_lost(&dir, &files);
                continue;
            }
            let before = requests_tree(&dir);
            p.undo().unwrap().unwrap();
            assert!(requests.join("auth/login.toml").is_file());
            assert!(kept.is_file());
            p.redo().unwrap().unwrap();
            assert!(!requests.join("auth/login.toml").exists());
            assert!(kept.is_file());
            let mut after = requests_tree(&dir);
            // The undo of the delete made an empty `auth/`, and the redo
            // leaves it.
            let left = after.remove("requests/auth").is_some();
            assert_eq!(left, is_delete);
            assert_eq!(after, before);
        }
    }
}
