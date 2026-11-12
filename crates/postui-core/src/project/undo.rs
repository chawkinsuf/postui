//! Undo and redo: pop an entry, check every path it will touch, replay
//! its inverses through the same primitives (which record the redo
//! entry), then re-read every document from disk. A file that does not
//! hold the text the entry left is first put back to that text, as an
//! entry of its own (#283).

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
    /// The files that did not hold the text the popped entry left. When
    /// this is not empty the call replayed no part of that entry: it put
    /// these files back to that text, `id` and `label` are those of a new
    /// entry holding the text they held, and the popped entry is back on
    /// its stack.
    pub outside: Vec<String>,
}

/// The label of an entry `replay` makes from the files that did not hold
/// the text the popped entry left.
const OUTSIDE_LABEL: &str = "outside change";

/// Whether the app also writes `path` outside the journal, by a write the
/// journal does not follow: `state.toml` (`persist_local`) and a request's
/// file (`save_request`), taken to be any path under `requests/` but a
/// folder's file.
fn written_outside_journal(path: &RelPath) -> bool {
    path.as_str() == STATE_TOML
        || (path.as_str().starts_with(&format!("{REQUESTS_DIR}/"))
            && path.file_name() != crate::storage::FOLDER_FILE)
}

/// Where a path stands at a point of [`Project::preflight`]'s replay.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    /// It holds what is on disk at this path before the replay.
    Disk(RelPath),
    /// An inverse made it present or absent.
    Set(bool),
}

/// The paths the replay has touched so far, by the path they will have.
/// A path is looked up at itself, then at each directory above it, so a
/// path inside a directory the replay renames is found where the
/// directory is on disk (#286).
#[derive(Default)]
struct Overlay(std::collections::HashMap<RelPath, Node>);

impl Overlay {
    fn resolve(&self, p: &RelPath) -> Node {
        let mut at = Some(p.clone());
        while let Some(a) = at {
            match self.0.get(&a) {
                Some(Node::Disk(disk)) => {
                    return Node::Disk(p.rebase(&a, disk).unwrap_or_else(|| p.clone()));
                }
                Some(Node::Set(present)) if a == *p => return Node::Set(*present),
                // Nothing is under a path taken out.
                Some(Node::Set(false)) => return Node::Set(false),
                // What is under a path made present is looked up on disk.
                Some(Node::Set(true)) => break,
                None => at = a.parent(),
            }
        }
        Node::Disk(p.clone())
    }

    /// Takes `p` and everything under it out of the map.
    fn take_under(&mut self, p: &RelPath) -> Vec<(RelPath, Node)> {
        let keys: Vec<RelPath> = self.0.keys().filter(|k| k.starts_with(p)).cloned().collect();
        keys.into_iter().filter_map(|k| self.0.remove_entry(&k)).collect()
    }

    fn set(&mut self, p: &RelPath, present: bool) {
        self.take_under(p);
        self.0.insert(p.clone(), Node::Set(present));
    }

    /// A rename's inverse moves what is at `from` to `to`, with what is
    /// under it.
    fn moved(&mut self, from: &RelPath, to: &RelPath) {
        let node = self.resolve(from);
        let under = self.take_under(from);
        self.set(from, false);
        self.take_under(to);
        for (k, n) in under {
            if let Some(k) = k.rebase(from, to) {
                self.0.insert(k, n);
            }
        }
        self.0.insert(to.clone(), node);
    }
}

impl Project {
    pub fn clear_journal(&mut self) {
        self.journal.clear();
    }

    /// Simulates replaying `ops.iter().rev()` — the order `apply_inverse`
    /// actually runs in, for both undo and redo — against an [`Overlay`]
    /// of the paths the replay moves or writes, over the disk. For each op,
    /// checks its inverse's preconditions against the overlay (not raw disk),
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
    /// A text op's inverse writes the text from before the op over
    /// whatever the file holds. A file that does not hold the text the op
    /// left refuses nothing: it is returned as a text op from the op's text
    /// to the file's, which [`Self::replay`] reverses as an entry of its
    /// own before this one (#283, spec 2026-11-11 outside edit undo step).
    /// Two kinds of file are not compared, because what they hold since
    /// the op can be the app's own ([`written_outside_journal`]); a request
    /// rename's undo over an outside edit of the request is #285.
    fn preflight(&mut self, ops: &[Op]) -> Result<Vec<Op>, Error> {
        // The user never sees the machinery, only the cause: something
        // changed the file after the op that is being replayed.
        let conflict = |path: &RelPath, what: &str| {
            Error::Conflict(format!("{path} {what} (changed outside the app)"))
        };
        let mut differing = Vec::new();
        let mut overlay = Overlay::default();
        fn present(disk: &crate::disk::Disk, node: &Node) -> bool {
            match node {
                Node::Disk(at) => disk.exists(at),
                Node::Set(present) => *present,
            }
        }
        fn exists(disk: &crate::disk::Disk, overlay: &Overlay, p: &RelPath) -> bool {
            present(disk, &overlay.resolve(p))
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
                    overlay.moved(to, from);
                }
                Op::Created { path } => {
                    let node = overlay.resolve(path);
                    if !present(&self.disk, &node) {
                        return Err(conflict(path, "no longer exists"));
                    }
                    if let Node::Disk(at) = node
                        && self.disk.is_dir(&at)
                    {
                        // What it holds that postui shows (a folder, a
                        // request file, neither hidden), once this replay
                        // has taken out what the entry put in it: on disk
                        // and not marked gone, or put back by an earlier
                        // inverse.
                        let shown = |name: &str, is_dir: bool| {
                            !super::is_hidden(name) && (is_dir || name.ends_with(".toml"))
                        };
                        let mut held = false;
                        for e in self.disk.list(&at)? {
                            let Ok(child) = path.join(&e.name) else { continue };
                            held |=
                                shown(&e.name, e.is_dir) && exists(&self.disk, &overlay, &child);
                        }
                        let inside = format!("{path}/");
                        held |= overlay.0.iter().any(|(p, node)| {
                            !matches!(node, Node::Set(false))
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
                    overlay.set(path, false);
                }
                Op::Trashed { ticket } => {
                    if !exists(&self.disk, &overlay, &ticket.slot) {
                        return Err(conflict(&ticket.slot, "is gone from the trash"));
                    }
                    if exists(&self.disk, &overlay, &ticket.original) {
                        return Err(conflict(&ticket.original, "already exists"));
                    }
                    overlay.set(&ticket.slot, false);
                    overlay.set(&ticket.original, true);
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
                    overlay.set(&ticket.original, false);
                    overlay.set(&ticket.slot, true);
                }
                Op::Text { path, before, after } => {
                    let expect_present = after.is_some();
                    let node = overlay.resolve(path);
                    if expect_present != present(&self.disk, &node) {
                        return Err(conflict(
                            path,
                            if expect_present { "no longer exists" } else { "already exists" },
                        ));
                    }
                    // Compared in the file on disk that the path stands
                    // for at this point of the replay, until an earlier
                    // inverse has written it: after that it holds text that
                    // is not on disk yet. The difference names that file,
                    // since its step runs before this replay moves anything.
                    // A peek, so a change `poll` has not read yet is still
                    // `poll`'s to read. A file that cannot be read is not
                    // compared: the inverse reads it before it writes
                    // (`fs_write_text`) and fails there, which keeps the
                    // entry on its stack.
                    if let Node::Disk(at) = node
                        && !written_outside_journal(path)
                        && let Ok(held) = self.disk.peek(&at)
                        && held != *after
                    {
                        differing.push(Op::Text { path: at, before: after.clone(), after: held });
                    }
                    overlay.set(path, before.is_some());
                }
            }
        }
        Ok(differing)
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
    ///
    /// When the preflight returns files that do not hold the text the
    /// entry left, the entry goes back on its stack unreplayed and an entry
    /// made of those differences is replayed in its place, the same way:
    /// the files get the text the entry left, and the opposite stack gets
    /// an entry that writes back the text they held. `Undone::outside`
    /// names the files. The next call meets the popped entry again (#283,
    /// spec 2026-11-11 outside edit undo step, R2 to R5).
    fn replay(&mut self, entry: Entry, redo: bool) -> Result<Option<Undone>, Error> {
        debug_assert!(self.recording.is_none());
        let differing = self.preflight(&entry.ops)?;
        let outside: Vec<String> =
            differing.iter().flat_map(Op::touched).map(ToString::to_string).collect();
        let entry = if differing.is_empty() {
            entry
        } else {
            self.put_back(entry, redo);
            Entry {
                id: self.journal.take_id(),
                label: OUTSIDE_LABEL.to_string(),
                ops: differing,
                meta: EntryMeta::default(),
                merge: None,
            }
        };
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
                    outside,
                }))
            }
            Err(e) => {
                for op in ops.iter().rev() {
                    let _ = self.apply_inverse_unrecorded(op);
                }
                self.reload_all();
                // An entry made of differences was popped from no stack,
                // and the entry it stood in for is already back on its own.
                if outside.is_empty() {
                    self.put_back(entry, redo);
                }
                Err(e)
            }
        }
    }

    /// Puts `entry` back on the stack `undo` (or, for `redo`, `redo`)
    /// popped it from.
    fn put_back(&mut self, entry: Entry, redo: bool) {
        if redo {
            self.journal.push_redo(entry);
        } else {
            self.journal.push_undo_replayed(entry);
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

        p.place_environment("dev", 0).unwrap();
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

    /// `r` is a press that found text written outside in `files` and took
    /// it back.
    #[track_caller]
    fn assert_took_back(r: Result<Option<Undone>, Error>, files: &[&str]) {
        let u = r.unwrap().unwrap();
        assert_eq!(u.outside, files, "{u:?}");
    }

    /// `r` replayed the step labelled `label` and found no text written
    /// outside.
    #[track_caller]
    fn assert_replayed(r: Result<Option<Undone>, Error>, label: &str) {
        let u = r.unwrap().unwrap();
        assert_eq!((u.label.as_str(), u.outside.len()), (label, 0), "{u:?}");
    }

    fn add_port(p: &mut Project) {
        p.edit_variables(|doc| varedit::upsert_var(doc, "port", None, Some("8080"))).unwrap();
    }

    fn add_path(p: &mut Project) {
        p.edit_variables(|doc| varedit::upsert_var(doc, "path", None, Some("/v1"))).unwrap();
    }

    /// #283: a file changed outside since the op wrote it. The undo takes
    /// the outside text back as a step of its own: the file holds the text
    /// the op left, and the op is not undone. The next undo undoes the op.
    /// Two redos bring back the op, then the outside text. The same once
    /// `poll` has read the change.
    #[test]
    fn undo_over_text_changed_outside_takes_it_back_as_its_own_step() {
        for polled in [false, true] {
            let (dir, mut p) = fixture();
            let vars = || read(&dir, "variables.toml").unwrap();
            let start = vars();
            add_port(&mut p);
            let left = vars();
            let outside = edit_outside(&dir, "variables.toml");
            if polled {
                assert!(p.poll().0);
            }
            assert_took_back(p.undo(), &["variables.toml"]);
            assert_eq!(vars(), left, "polled {polled}");
            assert!(p.variables().vars.contains_key("port"), "polled {polled}: the op stays");
            assert!(!p.changed_on_disk(), "polled {polled}");
            assert_replayed(p.undo(), "edit variables");
            assert_eq!(vars(), start, "polled {polled}");
            assert!(!p.can_undo(), "polled {polled}");

            assert_replayed(p.redo(), "edit variables");
            assert_eq!(vars(), left, "polled {polled}");
            assert_replayed(p.redo(), OUTSIDE_LABEL);
            assert_eq!(vars(), outside, "polled {polled}");
            assert!(!p.can_redo(), "polled {polled}");

            assert_replayed(p.undo(), OUTSIDE_LABEL);
            assert_eq!(vars(), left, "polled {polled}");
            assert_replayed(p.undo(), "edit variables");
            assert_eq!(vars(), start, "polled {polled}");
        }
    }

    /// The redo side: the redo takes the outside text back, onto the undo
    /// stack. The next redo redoes the op, and two undos bring back the
    /// outside text.
    #[test]
    fn redo_over_text_changed_outside_takes_it_back_as_its_own_step() {
        let (dir, mut p) = fixture();
        let vars = || read(&dir, "variables.toml").unwrap();
        let start = vars();
        add_port(&mut p);
        let left = vars();
        p.undo().unwrap().unwrap();
        let outside = edit_outside(&dir, "variables.toml");
        let found = p.redo();
        assert!(matches!(&found, Ok(Some(u)) if u.redo), "{found:?}");
        assert_took_back(found, &["variables.toml"]);
        assert_eq!(vars(), start);
        assert!(!p.variables().vars.contains_key("port"), "the op is not redone");
        assert_replayed(p.redo(), "edit variables");
        assert_eq!(vars(), left);
        assert!(!p.can_redo());

        assert_replayed(p.undo(), "edit variables");
        assert_eq!(vars(), start);
        assert_replayed(p.undo(), OUTSIDE_LABEL);
        assert_eq!(vars(), outside);
        assert!(!p.can_undo());
    }

    /// An op made after the outside change took that text in, so its undo
    /// puts it back. The op from before the change left other text, so its
    /// undo takes the outside text back first.
    #[test]
    fn the_step_before_an_outside_change_takes_it_back_after_the_one_made_on_it_undoes() {
        let (dir, mut p) = fixture();
        let vars = || read(&dir, "variables.toml").unwrap();
        let start = vars();
        add_port(&mut p);
        let left = vars();
        let outside = edit_outside(&dir, "variables.toml");
        p.poll();
        add_path(&mut p);
        assert_replayed(p.undo(), "edit variables");
        assert_eq!(vars(), outside);
        assert_took_back(p.undo(), &["variables.toml"]);
        assert_eq!(vars(), left);
        assert_replayed(p.undo(), "edit variables");
        assert_eq!(vars(), start);
    }

    /// A cascade wrote two files and one of them changed outside. The
    /// outside step holds that file alone, with the text the cascade left
    /// and the text found. The other file is not written until the cascade
    /// is undone.
    #[test]
    fn only_the_file_that_differs_is_in_the_outside_step() {
        let (dir, mut p) = fixture();
        let vars = || read(&dir, "variables.toml").unwrap();
        let dev = || read(&dir, "environments/dev.toml").unwrap();
        let (vars_start, dev_start) = (vars(), dev());
        p.cascade("rename var", |p| {
            p.edit_variables(|doc| varedit::rename_var(doc, "host", "hostname"))?;
            p.edit_env("dev", |doc| varedit::rename_env_var(doc, "host", "hostname"))
        })
        .unwrap();
        let (vars_left, dev_left) = (vars(), dev());
        let written = || {
            std::fs::metadata(dir.path().join("environments/dev.toml")).unwrap().modified().unwrap()
        };
        let before = written();
        let outside = edit_outside(&dir, "variables.toml");

        assert_took_back(p.undo(), &["variables.toml"]);
        assert_eq!((vars(), dev()), (vars_left, dev_left));
        assert_eq!(written(), before, "the environment file is not rewritten");
        assert!(p.variables().vars.contains_key("hostname"), "the cascade stays");
        let step = p.journal.peek_redo().unwrap();
        assert!(
            matches!(
                &step.ops[..],
                [Op::Text { path, before, after }]
                    if path.as_str() == "variables.toml"
                        && before.as_deref() == Some(outside.as_str())
                        && *after == Some(vars())
            ),
            "{:?}",
            step.ops
        );

        assert_replayed(p.undo(), "rename var");
        assert_eq!((vars(), dev()), (vars_start, dev_start));
    }

    /// A space delete, then `project.toml` changes outside. Two undos
    /// restore the space: the first takes the outside text back, the second
    /// undoes the delete.
    #[test]
    fn a_space_delete_is_undone_after_an_outside_change_in_two_presses() {
        let (dir, mut p) = fixture();
        p.delete_space("auth").unwrap();
        let left = read(&dir, "project.toml").unwrap();
        edit_outside(&dir, "project.toml");
        assert_took_back(p.undo(), &["project.toml"]);
        assert_eq!(read(&dir, "project.toml").unwrap(), left);
        assert!(!dir.path().join("requests/auth").exists(), "the delete is not undone yet");
        assert_replayed(p.undo(), "delete space");
        assert!(dir.path().join("requests/auth/login.toml").is_file());
        assert_eq!(p.spaces(), ["main", "auth"]);
    }

    /// A folder's file is compared like any document: a change to it is
    /// taken back first, and the next undo undoes the rename.
    #[test]
    fn a_folder_file_changed_outside_is_taken_back_before_its_rename_is_undone() {
        let (dir, mut p) = fixture();
        p.create_folder("main", "Users", None).unwrap();
        p.rename_folder("main/users", "People").unwrap();
        let left = read(&dir, "requests/main/people/.folder.toml").unwrap();
        edit_outside(&dir, "requests/main/people/.folder.toml");
        assert_took_back(p.undo(), &["requests/main/people/.folder.toml"]);
        assert_eq!(read(&dir, "requests/main/people/.folder.toml").unwrap(), left);
        assert!(!dir.path().join("requests/main/users").exists(), "the rename is not undone yet");
        assert_replayed(p.undo(), "rename folder");
        assert!(dir.path().join("requests/main/users/.folder.toml").is_file());
        assert!(!dir.path().join("requests/main/people").exists());
    }

    /// #286: the redo of a folder rename moves the directory, then writes
    /// the folder's file at its new path. The file is found where it is
    /// before the replay, inside the directory the redo moves.
    #[test]
    fn redo_of_a_folder_rename_renames_it_again() {
        let (dir, mut p) = fixture();
        p.create_folder("main", "Users", None).unwrap();
        p.rename_folder("main/users", "Accounts").unwrap();
        let renamed = read(&dir, "requests/main/accounts/.folder.toml").unwrap();
        assert_replayed(p.undo(), "rename folder");
        assert!(dir.path().join("requests/main/users/.folder.toml").is_file());
        assert_replayed(p.redo(), "rename folder");
        assert_eq!(read(&dir, "requests/main/accounts/.folder.toml").unwrap(), renamed);
        assert!(!dir.path().join("requests/main/users").exists());
        assert_replayed(p.undo(), "rename folder");
        assert_replayed(p.undo(), "create folder");
        assert!(!dir.path().join("requests/main/users").exists());
        assert!(!dir.path().join("requests/main/accounts").exists());
    }

    /// The folder's file changed outside before that redo. The redo takes
    /// the change back in the file where it is, and the next redo renames
    /// the folder. Two undos bring the outside text back.
    #[test]
    fn a_folder_file_changed_outside_is_taken_back_before_its_rename_is_redone() {
        let (dir, mut p) = fixture();
        p.create_folder("main", "Users", None).unwrap();
        p.rename_folder("main/users", "People").unwrap();
        let renamed = read(&dir, "requests/main/people/.folder.toml").unwrap();
        assert_replayed(p.undo(), "rename folder");
        let left = read(&dir, "requests/main/users/.folder.toml").unwrap();
        let outside = edit_outside(&dir, "requests/main/users/.folder.toml");
        assert_took_back(p.redo(), &["requests/main/users/.folder.toml"]);
        assert_eq!(read(&dir, "requests/main/users/.folder.toml").unwrap(), left);
        assert!(!dir.path().join("requests/main/people").exists(), "the rename is not redone yet");
        assert_replayed(p.redo(), "rename folder");
        assert_eq!(read(&dir, "requests/main/people/.folder.toml").unwrap(), renamed);
        assert!(!dir.path().join("requests/main/users").exists());

        assert_replayed(p.undo(), "rename folder");
        assert_replayed(p.undo(), OUTSIDE_LABEL);
        assert_eq!(read(&dir, "requests/main/users/.folder.toml").unwrap(), outside);
    }

    /// Comment 1283, first case, with nothing changed outside: a folder
    /// renamed to People, to Users and back to People. One undo, its redo,
    /// and then every step undoes, down to the create.
    #[test]
    fn a_folder_renamed_back_and_forth_undoes_all_the_way_after_a_redo() {
        let (dir, mut p) = fixture();
        p.create_folder("main", "Users", None).unwrap();
        p.rename_folder("main/users", "People").unwrap();
        p.rename_folder("main/people", "Users").unwrap();
        p.rename_folder("main/users", "People").unwrap();
        assert_replayed(p.undo(), "rename folder");
        assert_replayed(p.redo(), "rename folder");
        assert_replayed(p.undo(), "rename folder");
        assert_replayed(p.undo(), "rename folder");
        assert_replayed(p.undo(), "rename folder");
        assert!(dir.path().join("requests/main/users/.folder.toml").is_file());
        assert_replayed(p.undo(), "create folder");
        assert!(!dir.path().join("requests/main/users").exists());
        assert!(!dir.path().join("requests/main/people").exists());
    }

    /// Comment 1283, second case: two steps wrote `variables.toml`, and the
    /// file is put back outside to the text the first one left. The first
    /// undo takes that back, so the file holds the second step's text
    /// again. The next two undo the two steps.
    #[test]
    fn a_file_put_back_outside_to_an_older_steps_text_undoes_all_the_way() {
        let (dir, mut p) = fixture();
        let vars = || read(&dir, "variables.toml").unwrap();
        let start = vars();
        add_port(&mut p);
        let first = vars();
        add_path(&mut p);
        let second = vars();
        std::fs::write(dir.path().join("variables.toml"), &first).unwrap();
        assert_took_back(p.undo(), &["variables.toml"]);
        assert_eq!(vars(), second);
        assert_replayed(p.undo(), "edit variables");
        assert_eq!(vars(), first);
        assert_replayed(p.undo(), "edit variables");
        assert_eq!(vars(), start);
    }

    /// An outside edit made between two undos is found by the second one.
    /// Redo brings it back above the step already undone, whose redo then
    /// finds it too and takes it back before it redoes.
    #[test]
    fn an_outside_edit_between_two_undos_is_found_by_the_undo_and_then_by_the_redo() {
        let (dir, mut p) = fixture();
        let vars = || read(&dir, "variables.toml").unwrap();
        let start = vars();
        add_port(&mut p);
        let first = vars();
        add_path(&mut p);
        let second = vars();
        assert_replayed(p.undo(), "edit variables");
        let outside = edit_outside(&dir, "variables.toml");
        assert_took_back(p.undo(), &["variables.toml"]);
        assert_eq!(vars(), first);
        assert_replayed(p.undo(), "edit variables");
        assert_eq!(vars(), start);

        assert_replayed(p.redo(), "edit variables");
        assert_replayed(p.redo(), OUTSIDE_LABEL);
        assert_eq!(vars(), outside);
        assert_took_back(p.redo(), &["variables.toml"]);
        assert_eq!(vars(), first);
        assert_replayed(p.redo(), "edit variables");
        assert_eq!(vars(), second);
        assert!(!p.can_redo());
    }

    /// A file left unparsable outside is taken back like any other: the
    /// step's text is written over it, and redo writes the broken text
    /// back.
    #[test]
    fn a_file_left_unparsable_outside_is_taken_back_and_kept_in_the_step() {
        let (dir, mut p) = fixture();
        let vars = || read(&dir, "variables.toml").unwrap();
        add_port(&mut p);
        let left = vars();
        std::fs::write(dir.path().join("variables.toml"), "[broken\n").unwrap();
        assert_took_back(p.undo(), &["variables.toml"]);
        assert_eq!(vars(), left);
        assert_replayed(p.redo(), OUTSIDE_LABEL);
        assert_eq!(vars(), "[broken\n");
    }

    /// A file that cannot be read as text fails the undo, which writes
    /// nothing and keeps its step. With the file readable again, the step
    /// undoes.
    #[test]
    fn a_file_that_is_not_text_fails_the_undo_and_keeps_the_step() {
        let (dir, mut p) = fixture();
        let vars = || read(&dir, "variables.toml").unwrap();
        let start = vars();
        add_port(&mut p);
        let left = vars();
        let bytes = [0xff, 0xfe, 0xfd];
        std::fs::write(dir.path().join("variables.toml"), bytes).unwrap();
        assert!(p.undo().is_err());
        assert_eq!(std::fs::read(dir.path().join("variables.toml")).unwrap(), bytes);
        assert!(p.can_undo() && !p.can_redo());
        std::fs::write(dir.path().join("variables.toml"), &left).unwrap();
        assert_replayed(p.undo(), "edit variables");
        assert_eq!(vars(), start);
    }

    /// The same for a space delete whose `project.toml` cannot be read as
    /// text: the undo fails with the space still in the trash, and with the
    /// file readable again the space is restored.
    #[test]
    fn a_space_delete_is_undone_once_a_file_that_was_not_text_is_readable_again() {
        let (dir, mut p) = fixture();
        p.delete_space("auth").unwrap();
        let left = read(&dir, "project.toml").unwrap();
        let bytes = [0xff, 0xfe, 0xfd];
        std::fs::write(dir.path().join("project.toml"), bytes).unwrap();
        assert!(p.undo().is_err());
        assert_eq!(std::fs::read(dir.path().join("project.toml")).unwrap(), bytes);
        assert!(!dir.path().join("requests/auth").exists());
        std::fs::write(dir.path().join("project.toml"), &left).unwrap();
        assert_replayed(p.undo(), "delete space");
        assert!(dir.path().join("requests/auth/login.toml").is_file());
        assert_eq!(p.spaces(), ["main", "auth"]);
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

    /// A completion written over text changed outside is not followed. The
    /// entry from before it finds both as one difference and takes them
    /// back together; the refresh then lists the request again, and the
    /// rename undoes. Two redos bring back the rename, then the file as it
    /// was found.
    #[test]
    fn an_order_completion_over_an_outside_change_is_taken_back_with_it() {
        let (dir, mut p) = fixture();
        let project = || read(&dir, "project.toml").unwrap();
        p.complete_order().unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        let left = project();
        edit_outside(&dir, "project.toml");
        std::fs::write(dir.path().join("requests/main/added.toml"), "url = \"u\"\n").unwrap();
        assert!(p.complete_order().unwrap());
        let found = project();
        assert!(found.contains("# outside") && found.contains("added"), "{found}");

        assert_took_back(p.undo(), &["project.toml"]);
        assert_eq!(project(), left);
        assert!(p.complete_order().unwrap(), "the refresh lists the request again");
        assert_replayed(p.undo(), "rename request");
        assert!(dir.path().join("requests/main/ping.toml").is_file());

        assert!(p.complete_order().unwrap());
        assert_replayed(p.redo(), "rename request");
        assert_replayed(p.redo(), OUTSIDE_LABEL);
        assert_eq!(project(), found);
    }

    /// A step refused over a path is dropped with its text still in the
    /// files it wrote. The step beneath that wrote one of them finds that
    /// text as a difference: its undo takes it back first, then undoes.
    #[test]
    fn the_step_beneath_a_dropped_one_takes_the_dropped_steps_text_back_first() {
        let (dir, mut p) = fixture();
        let project = || read(&dir, "project.toml").unwrap();
        p.complete_order().unwrap();
        let start = project();
        p.rename_request("auth/login", "Signin").unwrap();
        let left = project();
        p.rename_request("main/ping", "Pong").unwrap();
        std::fs::remove_file(dir.path().join("requests/main/pong.toml")).unwrap();
        let err = p.undo().unwrap_err();
        assert!(matches!(err, Error::Conflict(ref m) if m.contains("no longer exists")), "{err}");
        assert_took_back(p.undo(), &["project.toml"]);
        assert_eq!(project(), left);
        assert_replayed(p.undo(), "rename request");
        assert_eq!(project(), start);
        assert!(dir.path().join("requests/auth/login.toml").is_file());
    }

    /// The same for a redo refused over a path: the redo beneath it, which
    /// wrote `project.toml` too, takes the dropped step's text back first.
    #[test]
    fn the_redo_beneath_a_dropped_one_takes_the_dropped_steps_text_back_first() {
        let (dir, mut p) = fixture();
        p.complete_order().unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        p.rename_request("auth/login", "Signin").unwrap();
        p.undo().unwrap().unwrap();
        p.undo().unwrap().unwrap();
        std::fs::write(dir.path().join("requests/main/pong.toml"), "url = \"u\"\n").unwrap();
        let err = p.redo().unwrap_err();
        assert!(matches!(err, Error::Conflict(ref m) if m.contains("already exists")), "{err}");
        assert_took_back(p.redo(), &["project.toml"]);
        assert_replayed(p.redo(), "rename request");
        assert!(dir.path().join("requests/auth/signin.toml").is_file());
    }

    /// A redo that finds text written outside writes that file alone. The
    /// rename's own inverses have not run: the open request's file is as
    /// it was and reports no drift. The next redo redoes the rename.
    #[test]
    fn a_redo_that_takes_outside_text_back_leaves_an_open_request_reporting_no_drift() {
        let (dir, mut p) = fixture();
        p.complete_order().unwrap();
        p.open_request("main/ping").unwrap();
        p.rename_request("main/ping", "Pong").unwrap();
        p.undo().unwrap().unwrap();
        assert_eq!(p.held_request_drift("main/ping"), None);
        let request = read(&dir, "requests/main/ping.toml");
        edit_outside(&dir, "project.toml");
        assert_took_back(p.redo(), &["project.toml"]);
        assert_eq!(read(&dir, "requests/main/ping.toml"), request);
        assert_eq!(p.held_request_drift("main/ping"), None);
        assert_replayed(p.redo(), "rename request");
        assert!(dir.path().join("requests/main/pong.toml").is_file());
        assert_eq!(p.held_request_drift("main/pong"), None);
    }

    /// A save writes a request's file outside the journal. Its text is not
    /// compared, so the app's own save is not taken for an outside change:
    /// the rename undoes at the first press.
    #[test]
    fn a_save_after_a_rename_is_not_taken_for_an_outside_change() {
        let (dir, mut p) = fixture();
        p.rename_request("main/ping", "Pong").unwrap();
        let mut req = p.read_request("main/pong").unwrap();
        req.url = "https://saved".into();
        p.save_request("main/pong", &req).unwrap();
        assert_replayed(p.undo(), "rename request");
        assert!(read(&dir, "requests/main/ping.toml").unwrap().contains("name = \"Ping\""));
    }

    /// The same for `state.toml`, which every UI change writes outside the
    /// journal.
    #[test]
    fn a_ui_change_after_a_space_rename_is_not_taken_for_an_outside_change() {
        let (_dir, mut p) = fixture();
        p.set_active_space("auth");
        p.rename_space("auth", "Login").unwrap();
        p.set_expanded(["login/deep".to_string()].into_iter().collect());
        assert_replayed(p.undo(), "rename space");
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

    /// What the overlay knew under a renamed directory is found under its
    /// new path, a path inside it is looked up where it is on disk, and
    /// nothing is left at the old path; through a second rename too.
    #[test]
    fn the_overlay_follows_a_renamed_directory() {
        let r = |s: &str| RelPath::new(s).unwrap();
        let mut o = Overlay::default();
        o.set(&r("a/gone.toml"), false);
        o.set(&r("a/made.toml"), true);
        o.moved(&r("a"), &r("b"));
        assert_eq!(o.resolve(&r("b/gone.toml")), Node::Set(false));
        assert_eq!(o.resolve(&r("b/made.toml")), Node::Set(true));
        assert_eq!(o.resolve(&r("b/kept.toml")), Node::Disk(r("a/kept.toml")));
        assert_eq!(o.resolve(&r("a/kept.toml")), Node::Set(false));

        o.moved(&r("b/kept.toml"), &r("c.toml"));
        assert_eq!(o.resolve(&r("c.toml")), Node::Disk(r("a/kept.toml")));
        o.moved(&r("b"), &r("d"));
        assert_eq!(o.resolve(&r("d/kept.toml")), Node::Set(false));
        assert_eq!(o.resolve(&r("d/made.toml")), Node::Set(true));
        assert_eq!(o.resolve(&r("d/other.toml")), Node::Disk(r("a/other.toml")));
        assert_eq!(o.resolve(&r("b")), Node::Set(false));

        // Under a path made present, what is there is looked up on disk.
        o.set(&r("e"), true);
        assert_eq!(o.resolve(&r("e/x.toml")), Node::Disk(r("e/x.toml")));
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
