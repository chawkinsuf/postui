//! Requests: the listing (always in memory), requests loaded on demand and
//! held while open, and the file operations with their order-list
//! cascades. Request ops journal paths and trash tickets, never content.
//!
//! Each held request (`Held`) carries the `Disk::stamp` its file had when
//! the editor's buffer was last seeded from it: taken before the read in
//! `open_request`, after the write in `save_request`. `held_request_drift`
//! compares a fresh stamp against that seed to tell the app whether the
//! file has moved outside it since. A poll's `reload_held_requests`
//! re-reads the body to keep the held copy current but keeps the seed
//! stamp — the editor's own buffer was not re-seeded by the poll, so the
//! file is still "moved since the editor last saw it" as far as the app's
//! drift check is concerned. An op of the app's own that moves a held
//! request re-keys it through `rekey_held_after_own_op`, which states the
//! one rule those ops share: re-stamp only an entry that still matched its
//! file BEFORE the op started. Re-stamping one that had already drifted
//! would launder an outside edit into "clean" and let the next save
//! overwrite it silently.

use super::*;
use crate::journal::Op;
use crate::model::HttpRequest;
use crate::order::{self, OrderEdit};

/// `(from_slug, to_slug)` pairs of a batch move.
pub type Moves = Vec<(String, String)>;

/// One sidebar item [`Project::move_to_level`] moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeItem {
    /// A request, by its full slug (`"main/users/login"`).
    Request(String),
    /// A folder, by its full path (`"main/users/v1"`, no trailing slash).
    Folder(String),
}

/// The directory of the folder at full path `path` (`"main/users"`).
fn folder_rel(path: &str) -> Result<RelPath, Error> {
    crate::storage::validate_slug(path).map_err(|e| Error::BadName(e.to_string()))?;
    rel(&format!("{REQUESTS_DIR}/{path}"))
}

/// How a held request's file has drifted from the stamp taken when the
/// editor's buffer was last seeded from it. See [`Project::held_request_drift`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldDrift {
    Changed,
    Vanished,
}

/// A warning from walking `requests/`, with the file it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingWarning {
    /// Relative to the root; `None` when it is about no one file.
    pub file: Option<String>,
    pub message: Warning,
}

impl ListingWarning {
    /// The directory under `requests/` its file is in; `None` for a file
    /// directly in `requests/`, or a warning about no one file.
    pub fn space_dir(&self) -> Option<&str> {
        let file = self.file.as_deref()?.strip_prefix("requests/")?;
        file.split_once('/').map(|(dir, _)| dir)
    }
}

/// Walks `requests/` through `Disk`: every `.toml` in a valid space,
/// parsed for its method and name; broken files listed with the error.
/// The warnings name the first unreadable directory and every loose file.
pub(crate) fn list(disk: &mut Disk) -> (Vec<RequestListing>, Vec<ListingWarning>) {
    let Ok(base) = RelPath::new(REQUESTS_DIR) else {
        return (Vec::new(), Vec::new());
    };
    let (files, walk_warning) = disk.walk_files(&base, "toml");
    let mut out = Vec::new();
    let mut loose = Vec::new();
    for path in files {
        let rel = path.as_str().strip_prefix("requests/").unwrap_or(path.as_str());
        let slug = rel.strip_suffix(".toml").unwrap_or(rel).to_string();
        let why = match crate::storage::space_of(&slug) {
            None => Some("is not in a space (move it into a space directory)"),
            Some(space) if crate::storage::validate_slug(space).is_err() => {
                Some("is not in a valid space (space names are a-z 0-9 - _)")
            }
            Some(_) => None,
        };
        if let Some(why) = why {
            let file = format!("requests/{rel}");
            loose.push(ListingWarning { message: format!("{file} {why}"), file: Some(file) });
            continue;
        }
        let (method, name, broken) = match disk.read(&path) {
            Ok(Some(text)) => match HttpRequest::from_toml_str(&text) {
                Ok(req) => (Some(req.method), req.name, None),
                Err(e) => (None, None, Some(e.to_string())),
            },
            Ok(None) => continue,
            Err(e) => (None, None, Some(e.to_string())),
        };
        out.push(RequestListing { slug, broken, method, name });
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    let mut warnings: Vec<ListingWarning> =
        walk_warning.into_iter().map(|message| ListingWarning { file: None, message }).collect();
    warnings.extend(loose);
    (out, warnings)
}

impl Project {
    pub fn requests(&self) -> &[RequestListing] {
        &self.listing
    }

    /// The walk's warnings as one line; `None` when there are none.
    pub fn listing_warning(&self) -> Option<String> {
        let messages: Vec<Warning> =
            self.listing_warnings.iter().map(|w| w.message.clone()).collect();
        join_warnings(&messages)
    }

    /// The walk's warnings, each with the file it is about.
    pub fn listing_warnings(&self) -> &[ListingWarning] {
        &self.listing_warnings
    }

    /// Re-walks `requests/`. Called by every request op and by the app's
    /// sidebar refresh, as `list_requests` was.
    pub fn relist(&mut self) {
        let (listing, warnings) = list(&mut self.disk);
        self.listing = listing;
        self.listing_warnings = warnings;
    }

    pub fn request_exists(&self, slug: &str) -> bool {
        request_rel(slug).map(|p| self.disk.is_file(&p)).unwrap_or(false)
    }

    pub fn held_request(&self, slug: &str) -> Option<&HttpRequest> {
        self.open_requests.get(slug).map(|held| &held.req)
    }

    /// `None` when `slug` is not held or its file still matches the stamp
    /// taken when it was opened or last saved. A file that exists but
    /// cannot be statted reports `Changed`, not `Vanished`: the user is
    /// told the truth and keeps the Reload choice, which surfaces the real
    /// io error. See [`Stamp`] for what the comparison can and cannot see.
    pub fn held_request_drift(&self, slug: &str) -> Option<HeldDrift> {
        let path = request_rel(slug).ok()?;
        let held = self.open_requests.get(slug)?;
        let fresh = self.disk.stamp(&path);
        if fresh == held.stamp {
            None
        } else if matches!(fresh, Stamp::Absent) {
            Some(HeldDrift::Vanished)
        } else {
            Some(HeldDrift::Changed)
        }
    }

    /// The held slugs whose file still matches their seed stamp, taken
    /// BEFORE an op runs. Feed it to [`Self::rekey_held_after_own_op`]:
    /// only an entry that was clean going in may be re-stamped afterwards.
    pub(crate) fn held_clean_before(&self) -> std::collections::HashSet<String> {
        self.open_requests
            .keys()
            .filter(|slug| self.held_request_drift(slug).is_none())
            .cloned()
            .collect()
    }
    /// Follows held entries through the `(old, new)` slug pairs an op the
    /// app itself performed produced, re-reading each body at its new path
    /// (keeping the last-good one if that fails).
    ///
    /// The one rule this exists to express: an entry is re-stamped only
    /// when it was in `clean_before`, i.e. still matched its file before
    /// this op wrote anything. Re-stamping an entry that had already
    /// drifted would launder an outside edit into "clean" and let the next
    /// save overwrite it with no prompt. A pure rename (no rewrite of the
    /// bytes) needs neither half — the old stamp still matches at the new
    /// path — so `move_request`, `move_all_requests` and the space rename
    /// re-key inline; `rename_request` (which rewrites `name`) and the
    /// undo/redo replay (which can rewrite anything) come through here.
    pub(crate) fn rekey_held_after_own_op(
        &mut self,
        moves: &[(String, String)],
        clean_before: &std::collections::HashSet<String>,
    ) {
        for (old, new) in moves {
            // Only the map key is conditional. A pair whose slug did not
            // change is still an op that rewrote the file — a rename that
            // only changes the display name (`ping` → `Ping!!!`) keeps the
            // slug — so the re-read and the clean-gated re-stamp run
            // either way, or the app would report its own write as an
            // outside edit on the next save.
            let Some(mut held) = self.open_requests.shift_remove(old.as_str()) else {
                continue;
            };
            if let Ok(path) = request_rel(new) {
                if let Ok(Some(text)) = self.disk.read(&path)
                    && let Ok(req) = HttpRequest::from_toml_str(&text)
                {
                    held.req = req;
                }
                if clean_before.contains(old) {
                    held.stamp = self.disk.stamp(&path);
                }
            }
            self.open_requests.insert(new.clone(), held);
        }
    }

    /// Parses the request and holds it until `close_request`.
    pub fn open_request(&mut self, slug: &str) -> Result<&HttpRequest, Error> {
        let path = request_rel(slug)?;
        // Stamped before the read: a write landing between the stamp and
        // the read would otherwise go undetected forever (fails unsafe —
        // drift would never report). Stamping first means that same race
        // instead reports `Changed` on the next check, a harmless extra
        // prompt.
        let stamp = self.disk.stamp(&path);
        let text = self.disk.read(&path)?.ok_or_else(|| Error::NotFound(slug.to_string()))?;
        let req = HttpRequest::from_toml_str(&text)
            .map_err(|e| Error::Parse { file: path.to_string(), error: e.to_string() })?;
        self.open_requests.insert(slug.to_string(), Held { req, stamp });
        Ok(&self.open_requests[slug].req)
    }

    pub fn close_request(&mut self, slug: &str) {
        self.open_requests.shift_remove(slug);
    }

    /// Parses the request without holding it — a one-off read (the
    /// sidebar's Copy as curl on a request that is not open).
    pub fn read_request(&mut self, slug: &str) -> Result<HttpRequest, Error> {
        let path = request_rel(slug)?;
        let text = self.disk.read(&path)?.ok_or_else(|| Error::NotFound(slug.to_string()))?;
        HttpRequest::from_toml_str(&text)
            .map_err(|e| Error::Parse { file: path.to_string(), error: e.to_string() })
    }

    /// Writes the request, refreshes the held copy and its listing row.
    /// Not journaled: undoing past a save is the editor's memory-only
    /// step, as today.
    pub fn save_request(&mut self, slug: &str, req: &HttpRequest) -> Result<(), Error> {
        let path = request_rel(slug)?;
        self.disk.write(&path, &req.to_toml_string())?;
        if self.open_requests.contains_key(slug) {
            // The stamp the write itself recorded (taken from the staged
            // file before the rename), not a fresh stat: an outside write
            // landing in the window between the two would otherwise
            // become the seed and never be reported.
            let stamp = self.disk.recorded(&path).unwrap_or_else(|| self.disk.stamp(&path));
            self.open_requests.insert(slug.to_string(), Held { req: req.clone(), stamp });
        }
        match self.listing.iter_mut().find(|l| l.slug == slug) {
            Some(row) => {
                row.method = Some(req.method);
                row.name = req.name.clone();
                row.broken = None;
            }
            None => self.relist(),
        }
        Ok(())
    }

    /// Whether a request in `folder` already answers to `leaf_display`
    /// (case-insensitive; a legacy file's display name is its slug leaf).
    pub fn sibling_name_taken(
        &self,
        folder: &str,
        leaf_display: &str,
        exclude: Option<&str>,
    ) -> bool {
        let wanted = leaf_display.to_lowercase();
        self.listing.iter().any(|l| {
            if exclude == Some(l.slug.as_str()) {
                return false;
            }
            let dir = l.slug.rsplit_once('/').map_or("", |(dir, _)| dir);
            dir == folder && l.display_name().to_lowercase() == wanted
        })
    }

    pub fn unique_slug(&self, folder: &str, leaf_display: &str, exclude: Option<&str>) -> String {
        let base = if folder.is_empty() {
            crate::storage::slugify(leaf_display)
        } else {
            format!("{folder}/{}", crate::storage::slugify(leaf_display))
        };
        let mut candidate = base.clone();
        let mut n = 2;
        while exclude != Some(candidate.as_str()) && self.request_exists(&candidate) {
            candidate = format!("{base}-{n}");
            n += 1;
        }
        candidate
    }
}

/// The arrival cascade for the request at `rel`, one of `arriving` (every
/// request the op brings into `space`, relative slugs): each folder it
/// sits in, outermost first, then the request itself. `slugs` is the
/// relisted listing, which no longer shows what the op took away, so
/// the op names that itself: `departed` holds the relative slugs it
/// moved out of `space`'s levels (a rename's old slug).
///
/// A folder is new when nothing was under it before the op: every listed
/// request under it is arriving, and none of the departed ones sat in
/// it. A new folder arrives (lands last in its level, spec 2026-11-02
/// drag into folders D4); one already there keeps its place. A rename
/// from `a/x` to `a/sub/x` leaves `a/` where it was. `seen` holds the
/// folders this op already handled, so a folder several arrivals share
/// is placed once.
fn arrive_with_folders(
    space: &str,
    order: &mut Vec<String>,
    rel: &str,
    arriving: &std::collections::HashSet<&str>,
    departed: &[&str],
    seen: &mut std::collections::HashSet<String>,
    slugs: &[String],
) -> Vec<OrderEdit> {
    let mut edits = Vec::new();
    for folder in order::ancestors(rel) {
        if !seen.insert(folder.to_string()) {
            continue;
        }
        let inside = format!("{folder}/");
        let under = format!("{space}/{inside}");
        let new = !departed.iter().any(|d| d.starts_with(&inside))
            && slugs
                .iter()
                .filter_map(|s| s.strip_prefix(&under).map(|_| &s[space.len() + 1..]))
                .all(|r| arriving.contains(r));
        if new {
            edits.extend(order::arrive(space, order, &order::folder_entry(folder)));
        }
    }
    edits.extend(order::arrive(space, order, rel));
    edits
}

impl Project {
    /// Reads `space`'s list from `meta`, hands it to `f`, and writes it
    /// back through `project.toml` only if `f` changed it. Refuses a
    /// space that does not exist on disk (no orphan tables).
    pub(super) fn order_edit(
        &mut self,
        space: &str,
        f: impl FnOnce(&mut Vec<String>) -> Vec<OrderEdit>,
    ) -> Result<Vec<OrderEdit>, Error> {
        let before = order::space_order(&self.meta, space).to_vec();
        let mut after = before.clone();
        let edits = f(&mut after);
        if after == before {
            return Ok(Vec::new());
        }
        // Only a real edit needs the space's directory to exist (an
        // orphan table would otherwise be written for it); a no-op edit
        // (e.g. moving zero requests out of a space with no directory)
        // must stay a no-op, as it always was before order lists existed.
        if !self.disk.is_dir(&space_rel(space)?) {
            return Err(Error::NotFound(space.to_string()));
        }
        self.edit_project_toml(|doc| order::write_order(doc, space, &after))?;
        Ok(edits)
    }

    /// Every listed request's full slug: what the order cascades test
    /// existence against (`order::exists_in`). Callers have relisted.
    fn listed_slugs(&self) -> Vec<String> {
        self.listing.iter().map(|l| l.slug.clone()).collect()
    }

    /// The request at `rel` (relative to `space`) has just appeared, with
    /// the listing already relisted: each folder it sits in that is new,
    /// outermost first, then the request itself takes the arrival rule
    /// (spec 2026-11-02 drag into folders D4). `departed` names what
    /// the same op took out of `space` (see [`arrive_with_folders`]).
    pub(super) fn order_arrive(
        &mut self,
        space: &str,
        rel: &str,
        departed: &[&str],
    ) -> Result<Vec<OrderEdit>, Error> {
        let slugs = self.listed_slugs();
        let arriving = std::collections::HashSet::from([rel]);
        let mut seen = std::collections::HashSet::new();
        self.order_edit(space, |order| {
            arrive_with_folders(space, order, rel, &arriving, departed, &mut seen, &slugs)
        })
    }

    /// The request at `rel` has just left, with the listing already
    /// relisted: its entry, then every folder entry it left empty (R9).
    pub(super) fn order_remove(&mut self, space: &str, rel: &str) -> Result<Vec<OrderEdit>, Error> {
        let slugs = self.listed_slugs();
        self.order_edit(space, |order| {
            let mut edits = order::remove(space, order, rel);
            edits.extend(order::prune_folders(space, order, rel, &|s| order::exists_in(&slugs, s)));
            edits
        })
    }

    pub(super) fn order_rename(
        &mut self,
        space: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<OrderEdit>, Error> {
        if order::level_of(from) == order::level_of(to) {
            return self.order_edit(space, |order| {
                for e in order.iter_mut() {
                    if e == from {
                        *e = to.to_string();
                    }
                }
                vec![OrderEdit::Renamed {
                    space: space.to_string(),
                    from: from.to_string(),
                    to: to.to_string(),
                }]
            });
        }
        let mut edits = self.order_remove(space, from)?;
        edits.extend(self.order_arrive(space, to, &[from])?);
        Ok(edits)
    }

    pub(super) fn order_insert_after(
        &mut self,
        space: &str,
        anchor: &str,
        rel: &str,
    ) -> Result<Vec<OrderEdit>, Error> {
        self.order_edit(space, |order| {
            if order.iter().any(|e| e == rel) {
                return Vec::new();
            }
            let Some(i) = order.iter().position(|e| e == anchor) else { return Vec::new() };
            order.insert(i + 1, rel.to_string());
            vec![OrderEdit::Inserted { space: space.to_string(), rel: rel.to_string(), at: i + 1 }]
        })
    }

    /// `order_insert_after`'s twin for a put *above* the anchor: the new
    /// entry takes the anchor's own index and pushes it down.
    pub(super) fn order_insert_before(
        &mut self,
        space: &str,
        anchor: &str,
        rel: &str,
    ) -> Result<Vec<OrderEdit>, Error> {
        self.order_edit(space, |order| {
            if order.iter().any(|e| e == rel) {
                return Vec::new();
            }
            let Some(i) = order.iter().position(|e| e == anchor) else { return Vec::new() };
            order.insert(i, rel.to_string());
            vec![OrderEdit::Inserted { space: space.to_string(), rel: rel.to_string(), at: i }]
        })
    }

    /// The items of `level` (relative, `""` for the top level) of `space`
    /// as list entries, both kinds, in the order the sidebar draws them
    /// now ([`order::order_entries`]).
    fn level_entries(&self, space: &str, level: &str) -> Vec<String> {
        let prefix =
            if level.is_empty() { format!("{space}/") } else { format!("{space}/{level}/") };
        let entries: Vec<&RequestListing> =
            self.listing.iter().filter(|l| l.slug.starts_with(&prefix)).collect();
        let items = order::split_level(&entries, &prefix);
        let order = order::space_order(&self.meta, space);
        order::order_entries(&items.requests, &items.folder_paths(), order, space)
            .iter()
            .filter_map(|i| i.entry(space))
            .collect()
    }

    /// Writes every space's list complete, as it draws (spec 2026-11-02
    /// drag into folders D5, D5a, D7: [`order::complete_order`]), in one
    /// write of `project.toml` through `edit_project_toml`. The TUI calls
    /// it whenever it re-lists, outside any transaction, so it is never an
    /// undo step. Returns whether it wrote. A read-only project (the CLI)
    /// writes nothing, and neither does a directory with no
    /// `project.toml`, which never gains one behind the user's back (see
    /// [`Self::ensure_spaces`]).
    pub fn complete_order(&mut self) -> Result<bool, Error> {
        if self.disk.is_read_only() || !self.disk.is_file(&rel(PROJECT_TOML)?) {
            return Ok(false);
        }
        let changed: Vec<(String, Vec<String>)> = self
            .spaces
            .iter()
            .filter_map(|space| {
                let before = order::space_order(&self.meta, space);
                let after = order::complete_order(&self.listing, before, space);
                (after != before).then(|| (space.clone(), after))
            })
            .collect();
        if changed.is_empty() {
            return Ok(false);
        }
        self.edit_project_toml(|doc| {
            for (space, after) in &changed {
                order::write_order(doc, space, after);
            }
        })?;
        Ok(true)
    }

    /// One read-modify-write per space, however many requests moved.
    pub(super) fn order_move_all(
        &mut self,
        from: &str,
        to: &str,
        moves: &[(String, String)],
    ) -> Result<(), Error> {
        let slugs = self.listed_slugs();
        let exists = |s: &str| order::exists_in(&slugs, s);
        self.order_edit(from, |order| {
            moves
                .iter()
                .flat_map(|(f, _)| {
                    let mut edits = order::remove(from, order, f);
                    edits.extend(order::prune_folders(from, order, f, &exists));
                    edits
                })
                .collect()
        })?;
        let arriving: std::collections::HashSet<&str> =
            moves.iter().map(|(_, t)| t.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        self.order_edit(to, |order| {
            moves
                .iter()
                .flat_map(|(_, t)| {
                    arrive_with_folders(to, order, t, &arriving, &[], &mut seen, &slugs)
                })
                .collect::<Vec<_>>()
        })?;
        Ok(())
    }

    /// A slug's `(space, rel)` halves, or `None` for a loose slug.
    fn split_space(slug: &str) -> Option<(&str, &str)> {
        let space = crate::storage::space_of(slug)?;
        Some((space, order::relative(slug, space)?))
    }
}

impl Project {
    fn meta_for_moves(&self, moves: Vec<(String, String)>) -> EntryMeta {
        EntryMeta { moves, active_env: None }
    }

    /// Creates a request from a typed display path ("Folder/My Request!").
    pub fn create_request(
        &mut self,
        display_path: &str,
        mut req: HttpRequest,
    ) -> Result<(String, String), Error> {
        let Some((folder, leaf)) = crate::storage::split_display_path(display_path) else {
            return Err(Error::BadName(display_path.to_string()));
        };
        if self.sibling_name_taken(&folder, &leaf, None) {
            return Err(Error::AlreadyExists(leaf));
        }
        let slug = self.unique_slug(&folder, &leaf, None);
        req.name = Some(leaf.clone());
        let meta = self.meta_for_moves(Vec::new());
        self.transaction("create request", meta, |p| {
            p.fs_create_file(&request_rel(&slug)?, &req.to_toml_string())?;
            p.relist();
            if let Some((space, rel)) = Self::split_space(&slug) {
                p.order_arrive(space, rel, &[])?;
            }
            Ok(())
        })?;
        Ok((slug, leaf))
    }

    /// What [`Self::rename_request`] would refuse, without writing: the
    /// slug and display name it would land at, else its error. A caller
    /// that must refuse before it changes anything else (replace with
    /// curl's rename, which runs before the dirty gate) asks here first.
    pub fn check_rename_request(
        &self,
        from_slug: &str,
        display_path: &str,
    ) -> Result<(String, String), Error> {
        let from_path = request_rel(from_slug)?;
        if !self.disk.is_file(&from_path) {
            return Err(Error::NotFound(from_slug.to_string()));
        }
        let Some((folder, leaf)) = crate::storage::split_display_path(display_path) else {
            return Err(Error::BadName(display_path.to_string()));
        };
        if self.sibling_name_taken(&folder, &leaf, Some(from_slug)) {
            return Err(Error::AlreadyExists(leaf));
        }
        // The slug is derived from the name, but a file need not sit at the
        // slug its name derives: a nameless hand-made `ping-.toml` is named
        // `ping-`, which `slugify` makes `ping`, and a file can carry any
        // name. So a rename that leaves the name as it is, in the same
        // folder, keeps the slug instead of deriving one and moving the file
        // (#209). "As it is" means as the Rename prompt shows it and reads
        // it back: the prompt prefills the current name and the leaf comes
        // back trimmed, so the current name is compared trimmed too. Any
        // other name, a case change included, derives its slug as before,
        // so a file at a `-2` slug still returns to a freed base slug on a
        // rename that changes its name.
        let from_folder = from_slug.rsplit_once('/').map_or("", |(dir, _)| dir);
        let same_name = folder == from_folder
            && self
                .listing
                .iter()
                .find(|l| l.slug == from_slug)
                .is_some_and(|l| l.display_name().trim() == leaf);
        let to_slug = if same_name {
            from_slug.to_string()
        } else {
            self.unique_slug(&folder, &leaf, Some(from_slug))
        };
        // A rename stays in its space (the order bookkeeping below is
        // keyed by one space); crossing spaces is `move_request`'s job.
        if crate::storage::space_of(from_slug) != crate::storage::space_of(&to_slug) {
            return Err(Error::BadName(format!(
                "{display_path}: a rename stays in its space — move the request instead"
            )));
        }
        Ok((to_slug, leaf))
    }

    /// Renames to a new typed display path; rewrites `name` when the file
    /// parses (a broken file just moves).
    pub fn rename_request(
        &mut self,
        from_slug: &str,
        display_path: &str,
    ) -> Result<(String, String), Error> {
        let from_path = request_rel(from_slug)?;
        let (to_slug, leaf) = self.check_rename_request(from_slug, display_path)?;
        let to_path = request_rel(&to_slug)?;
        // Which held entries still matched their file BEFORE this op
        // touched anything: only those may be re-stamped afterwards (see
        // `rekey_held_after_own_op`).
        let clean_before = self.held_clean_before();
        let meta = self.meta_for_moves(vec![(from_slug.to_string(), to_slug.clone())]);
        let from_slug = from_slug.to_string();
        self.transaction("rename request", meta, |p| {
            if to_slug != from_slug {
                p.fs_rename(&from_path, &to_path)?;
            }
            if let Some(text) = p.disk.read(&to_path)?
                && let Ok(mut req) = HttpRequest::from_toml_str(&text)
                && req.name.as_deref() != Some(leaf.as_str())
            {
                req.name = Some(leaf.clone());
                p.fs_write_text(&to_path, Some(&req.to_toml_string()))?;
            }
            // A rename can rewrite the file's `name` (above), so this op
            // owns a write the app must never see as an outside edit —
            // hence the shared re-key, which re-stamps only an entry that
            // was clean going in.
            p.rekey_held_after_own_op(&[(from_slug.clone(), to_slug.clone())], &clean_before);
            p.relist();
            if let (Some((space, from_rel)), Some((_, to_rel))) =
                (Self::split_space(&from_slug), Self::split_space(&to_slug))
                && to_slug != from_slug
            {
                p.order_rename(space, from_rel, to_rel)?;
            }
            Ok(())
        })?;
        Ok((to_slug, leaf))
    }

    /// The slug `slug` lands at in `space`: same sub-path, `-2`, `-3`, …
    /// while taken (in `reserved` too, for a batch).
    fn move_target(&self, slug: &str, space: &str, reserved: &[String]) -> Result<String, Error> {
        let Some((_, rest)) = slug.split_once('/') else {
            return Err(Error::BadName(slug.to_string()));
        };
        Ok(self.free_slug(&format!("{space}/{rest}"), reserved))
    }

    /// `base`, else `base-2`, `base-3`, … : the first slug no file has
    /// (nor `reserved`, for a batch). How a moved request's file keeps its
    /// name.
    fn free_slug(&self, base: &str, reserved: &[String]) -> String {
        let mut candidate = base.to_string();
        let mut n = 2;
        while self.request_exists(&candidate) || reserved.contains(&candidate) {
            candidate = format!("{base}-{n}");
            n += 1;
        }
        candidate
    }

    pub fn move_request(&mut self, slug: &str, space: &str) -> Result<String, Error> {
        let from_path = request_rel(slug)?;
        if !self.disk.is_file(&from_path) {
            return Err(Error::NotFound(slug.to_string()));
        }
        space_rel(space)?;
        let new_slug = self.move_target(slug, space, &[])?;
        let to_path = request_rel(&new_slug)?;
        let meta = self.meta_for_moves(vec![(slug.to_string(), new_slug.clone())]);
        let slug = slug.to_string();
        self.transaction("move request", meta, |p| {
            p.fs_rename(&from_path, &to_path)?;
            if let Some(req) = p.open_requests.shift_remove(&slug) {
                p.open_requests.insert(new_slug.clone(), req);
            }
            p.relist();
            let (from_space, from_rel) = Self::split_space(&slug).expect("validated");
            let (to_space, to_rel) = Self::split_space(&new_slug).expect("validated");
            p.order_remove(from_space, from_rel)?;
            p.order_arrive(to_space, to_rel, &[])?;
            Ok(())
        })?;
        Ok(new_slug)
    }

    /// A sidebar drop in another folder (spec 2026-11-02 drag into folders
    /// D17–D19): moves `item` into `level` of its own space (relative,
    /// `""` for the top level) as one undo step, and returns its new slug
    /// (a request) or full path (a folder).
    ///
    /// `at` is the slot among `level`'s items, both kinds, as they draw
    /// before the move; it is read before anything moves, so a folder the
    /// move is about to empty still counts, as the sidebar drew it (D15).
    /// The item's entry leaves its old level and lands above the item at
    /// `at` (last past the end); an emptied folder's entry goes (R9).
    ///
    /// A request keeps its file name, `-2`, `-3`, … when the folder has it,
    /// and its text. A folder's directory moves whole, every request under
    /// it in the entry's moves, and its entries are renamed in place, so
    /// its contents keep their order. Held requests follow.
    ///
    /// Refused, with nothing written: `level` being the item's own (a
    /// same-level drop is `set_request_order`'s, D20), a level or item that
    /// does not exist, a folder going inside itself or one of its own
    /// folders (D2), and a clash (D18): the folder already shows a request
    /// with the name the request will show there (a nameless one shows its
    /// file name after any `-2`), or already has a folder of that name.
    pub fn move_to_level(
        &mut self,
        item: &TreeItem,
        level: &str,
        at: usize,
    ) -> Result<String, Error> {
        let (full, is_folder) = match item {
            TreeItem::Request(slug) => (slug.as_str(), false),
            TreeItem::Folder(path) => (path.as_str(), true),
        };
        let Some((space, rel)) = Self::split_space(full) else {
            return Err(Error::NotFound(full.to_string()));
        };
        let (space, rel) = (space.to_string(), rel.to_string());
        let listed = self.listed_slugs();
        let present = if is_folder {
            folder_rel(full)?;
            order::exists_in(&listed, &format!("{full}/"))
        } else {
            self.disk.is_file(&request_rel(full)?)
        };
        if !present {
            return Err(Error::NotFound(full.to_string()));
        }
        if order::level_of(&rel) == level {
            return Err(Error::BadName(format!("{full} is already there: reorder it instead")));
        }
        if is_folder && (level == rel || level.starts_with(&format!("{rel}/"))) {
            return Err(Error::BadName(format!("{full} can't go inside itself")));
        }
        if !level.is_empty() && !order::exists_in(&listed, &format!("{space}/{level}/")) {
            return Err(Error::NotFound(format!("{space}/{level}")));
        }
        let anchor = self.level_entries(&space, level).into_iter().nth(at);
        let dest = if level.is_empty() { space.clone() } else { format!("{space}/{level}") };
        if is_folder {
            self.move_folder_to(full, &space, &rel, &dest, anchor)
        } else {
            self.move_request_to(full, &space, &rel, &dest, anchor)
        }
    }

    /// [`Self::move_to_level`] for a request, checked but for D18: into
    /// the folder at full path `dest`, its entry above `anchor`.
    fn move_request_to(
        &mut self,
        slug: &str,
        space: &str,
        rel: &str,
        dest: &str,
        anchor: Option<String>,
    ) -> Result<String, Error> {
        let leaf = slug.rsplit_once('/').map_or(slug, |(_, leaf)| leaf);
        let new_slug = self.free_slug(&format!("{dest}/{leaf}"), &[]);
        let new_leaf = new_slug.rsplit_once('/').map_or(new_slug.as_str(), |(_, leaf)| leaf);
        let shown = match self.listing.iter().find(|l| l.slug == slug).and_then(|l| l.name.clone())
        {
            Some(name) => name,
            None => new_leaf.to_string(),
        };
        if self.sibling_name_taken(dest, &shown, None) {
            return Err(Error::AlreadyExists(shown));
        }
        let from_path = request_rel(slug)?;
        let to_path = request_rel(&new_slug)?;
        let new_rel = order::relative(&new_slug, space).unwrap_or(&new_slug).to_string();
        let meta = self.meta_for_moves(vec![(slug.to_string(), new_slug.clone())]);
        self.transaction("move request to folder", meta, |p| {
            p.fs_rename(&from_path, &to_path)?;
            if let Some(held) = p.open_requests.shift_remove(slug) {
                p.open_requests.insert(new_slug.clone(), held);
            }
            p.relist();
            let slugs = p.listed_slugs();
            p.order_edit(space, |order| {
                let mut edits = order::remove(space, order, rel);
                edits.extend(order::insert_before(space, order, &new_rel, anchor.as_deref()));
                edits.extend(order::prune_folders(space, order, rel, &|s| {
                    order::exists_in(&slugs, s)
                }));
                edits
            })?;
            Ok(())
        })?;
        Ok(new_slug)
    }

    /// [`Self::move_to_level`] for a folder, checked but for D18: the way
    /// [`Self::rename_space`] moves a space's directory, one rename with
    /// every request under it in the entry's moves.
    fn move_folder_to(
        &mut self,
        path: &str,
        space: &str,
        rel: &str,
        dest: &str,
        anchor: Option<String>,
    ) -> Result<String, Error> {
        let name = rel.rsplit_once('/').map_or(rel, |(_, name)| name);
        let new_path = format!("{dest}/{name}");
        let from_dir = folder_rel(path)?;
        let to_dir = folder_rel(&new_path)?;
        if self.disk.exists(&to_dir) {
            return Err(Error::AlreadyExists(name.to_string()));
        }
        let new_rel = order::relative(&new_path, space).unwrap_or(&new_path).to_string();
        let inside = format!("{path}/");
        let moves: Vec<(String, String)> = self
            .listing
            .iter()
            .filter_map(|l| {
                let rest = l.slug.strip_prefix(&inside)?;
                Some((l.slug.clone(), format!("{new_path}/{rest}")))
            })
            .collect();
        let meta = self.meta_for_moves(moves.clone());
        self.transaction("move folder to folder", meta, |p| {
            p.fs_rename(&from_dir, &to_dir)?;
            for (old, new) in &moves {
                if let Some(held) = p.open_requests.shift_remove(old) {
                    p.open_requests.insert(new.clone(), held);
                }
            }
            p.relist();
            let slugs = p.listed_slugs();
            let entry = order::folder_entry(&new_rel);
            p.order_edit(space, |order| {
                order::rename_prefix(order, rel, &new_rel);
                let mut edits = vec![OrderEdit::Renamed {
                    space: space.to_string(),
                    from: order::folder_entry(rel),
                    to: entry.clone(),
                }];
                edits.extend(order::insert_before(space, order, &entry, anchor.as_deref()));
                edits.extend(order::prune_folders(space, order, rel, &|s| {
                    order::exists_in(&slugs, s)
                }));
                edits
            })?;
            Ok(())
        })?;
        Ok(new_path)
    }

    /// Every request of `from` into `to`, as one entry. Pre-flights the
    /// whole batch (every destination computed and every source present)
    /// before the first rename, so a refusal moves nothing. A listed file
    /// whose name is not a valid slug (`Get User.toml`) cannot be moved
    /// through the slug API; it stays behind, named in the warnings,
    /// rather than holding the rest of the space hostage.
    pub fn move_all_requests(
        &mut self,
        from: &str,
        to: &str,
    ) -> Result<(Moves, Vec<Warning>), Error> {
        space_rel(from)?;
        space_rel(to)?;
        // In the order the sidebar shows them (every level of the space),
        // so the requests arrive in `to` looking as they did in `from`.
        let sources: Vec<String> =
            order::displayed_slugs(&self.listing, order::space_order(&self.meta, from), from);
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut reserved: Vec<String> = Vec::new();
        let mut left_behind: Vec<Warning> = Vec::new();
        for slug in &sources {
            if request_rel(slug).is_err() {
                left_behind.push(format!(
                    "left behind: requests/{slug}.toml (its name is not a valid slug; rename the file first)"
                ));
                continue;
            }
            if !self.request_exists(slug) {
                return Err(Error::NotFound(slug.clone()));
            }
            let target = self.move_target(slug, to, &reserved)?;
            reserved.push(target.clone());
            pairs.push((slug.clone(), target));
        }
        let meta = self.meta_for_moves(pairs.clone());
        let rel_pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(f, t)| {
                (
                    order::relative(f, from).unwrap_or(f).to_string(),
                    order::relative(t, to).unwrap_or(t).to_string(),
                )
            })
            .collect();
        self.transaction("move all requests", meta, |p| {
            for (f, t) in &pairs {
                p.fs_rename(&request_rel(f)?, &request_rel(t)?)?;
                if let Some(req) = p.open_requests.shift_remove(f) {
                    p.open_requests.insert(t.clone(), req);
                }
            }
            p.relist();
            p.order_move_all(from, to, &rel_pairs)?;
            Ok(())
        })?;
        Ok((pairs, left_behind))
    }

    /// Deletes every request in `slugs` as one undo entry (piece 4 spec
    /// §5.2: a counted `dd` is one step). Duplicates are dropped. Every
    /// slug is pre-flighted (valid and present) before the first file
    /// moves, the way [`Self::move_all_requests`] does, so a refusal
    /// deletes nothing. The per-slug deletes nest inside one transaction,
    /// which they join, so the journal gets one entry.
    pub fn delete_requests(&mut self, slugs: &[String]) -> Result<(), Error> {
        let mut batch: Vec<String> = Vec::new();
        for s in slugs {
            if !batch.contains(s) {
                batch.push(s.clone());
            }
        }
        for s in &batch {
            let path = request_rel(s)?;
            if !self.disk.is_file(&path) {
                return Err(Error::NotFound(s.clone()));
            }
        }
        let meta = self.meta_for_moves(Vec::new());
        self.transaction("delete requests", meta, |p| {
            for s in &batch {
                p.delete_request(s)?;
            }
            Ok(())
        })
    }

    pub fn delete_request(&mut self, slug: &str) -> Result<(), Error> {
        let path = request_rel(slug)?;
        if !self.disk.is_file(&path) {
            return Err(Error::NotFound(slug.to_string()));
        }
        let meta = self.meta_for_moves(Vec::new());
        let slug = slug.to_string();
        self.transaction("delete request", meta, |p| {
            p.fs_trash(&path)?;
            p.open_requests.shift_remove(&slug);
            p.relist();
            if let Some((space, rel)) = Self::split_space(&slug) {
                p.order_remove(space, rel)?;
            }
            Ok(())
        })
    }

    /// `<name> copy`, `<name> copy 2`, … next to its source; a broken
    /// file is byte-copied to `<slug>-copy`.
    pub fn duplicate_request(&mut self, slug: &str) -> Result<String, Error> {
        self.duplicate_with(slug, slug, false)
    }

    /// The put entry point (piece 4 spec §5.2: `p`/`P`): a duplicate whose
    /// copy's place is chosen by the caller. `anchor` is a full slug, and
    /// may be the source itself (`yyP` on the yanked row). When it is not
    /// the source and lies in the source's space, the copy is made in the
    /// anchor's folder (same `<name> copy` collision rule, applied there).
    /// Its order entry lands directly below the anchor, or above it when
    /// `above`. Otherwise the copy is made beside its source. An anchor
    /// outside the copy's space gives the copy no place, and it arrives,
    /// last in its level (spec 2026-11-02 drag into folders D4).
    pub fn duplicate_request_at(
        &mut self,
        slug: &str,
        anchor: &str,
        above: bool,
    ) -> Result<String, Error> {
        self.duplicate_with(slug, anchor, above)
    }

    /// A counted put (`{N}p`): `count` copies as one undo entry, the way
    /// [`Self::delete_requests`] makes a counted `dd` one. The copies sit
    /// together beside `anchor` in the order they were made: below it each
    /// lands under the copy before, above it each takes the anchor's slot
    /// in turn. Returns the copies' slugs, first made first.
    pub fn duplicate_requests_at(
        &mut self,
        slug: &str,
        anchor: &str,
        above: bool,
        count: usize,
    ) -> Result<Vec<String>, Error> {
        let meta = self.meta_for_moves(Vec::new());
        self.transaction("duplicate requests", meta, |p| {
            let mut made: Vec<String> = Vec::new();
            for _ in 0..count.max(1) {
                let at = match made.last() {
                    Some(prev) if !above => prev.clone(),
                    _ => anchor.to_string(),
                };
                made.push(p.duplicate_request_at(slug, &at, above)?);
            }
            Ok(made)
        })
    }

    /// The shared body.
    fn duplicate_with(&mut self, slug: &str, anchor: &str, above: bool) -> Result<String, Error> {
        let source = request_rel(slug)?;
        let bytes =
            self.disk.read_bytes(&source)?.ok_or_else(|| Error::NotFound(slug.to_string()))?;
        let (src_folder, leaf) = slug.rsplit_once('/').unwrap_or(("", slug));
        let folder = match (Self::split_space(slug), Self::split_space(anchor)) {
            (Some((src_space, _)), Some((a_space, _)))
                if anchor != slug && src_space == a_space =>
            {
                anchor.rsplit_once('/').map_or("", |(f, _)| f)
            }
            _ => src_folder,
        };
        let parsed =
            std::str::from_utf8(&bytes).ok().and_then(|s| HttpRequest::from_toml_str(s).ok());
        let (new_slug, text) = match parsed {
            Some(mut req) => {
                let base = req.name.clone().unwrap_or_else(|| leaf.to_string());
                let mut copy_name = format!("{base} copy");
                let mut n = 2;
                while self.sibling_name_taken(folder, &copy_name, None) {
                    copy_name = format!("{base} copy {n}");
                    n += 1;
                }
                let new_slug = self.unique_slug(folder, &copy_name, None);
                req.name = Some(copy_name);
                (new_slug, req.to_toml_string().into_bytes())
            }
            None => {
                let stem =
                    if folder.is_empty() { leaf.to_string() } else { format!("{folder}/{leaf}") };
                let mut new_slug = format!("{stem}-copy");
                let mut n = 2;
                while self.request_exists(&new_slug) {
                    new_slug = format!("{stem}-copy-{n}");
                    n += 1;
                }
                (new_slug, bytes)
            }
        };
        let meta = self.meta_for_moves(Vec::new());
        let anchor = anchor.to_string();
        self.transaction("duplicate request", meta, |p| {
            let path = request_rel(&new_slug)?;
            p.disk.write_new(&path, "")?;
            p.record(Op::Created { path: path.clone() });
            p.disk.write_bytes(&path, &text)?;
            p.relist();
            // The anchor is listed (every list names every item, spec
            // 2026-11-02 drag into folders D5), so the copy goes beside it;
            // an anchor in another space has no place here, and the copy
            // arrives (D4).
            match (Self::split_space(&anchor), Self::split_space(&new_slug)) {
                (Some((a_space, a_rel)), Some((space, rel))) if a_space == space => {
                    if above {
                        p.order_insert_before(space, a_rel, rel)?;
                    } else {
                        p.order_insert_after(space, a_rel, rel)?;
                    }
                }
                (_, Some((space, rel))) => {
                    p.order_arrive(space, rel, &[])?;
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok(new_slug)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, read};
    use super::*;
    use crate::journal::Op;

    fn req(url: &str) -> HttpRequest {
        HttpRequest {
            name: None,
            method: crate::model::Method::Get,
            url: url.to_string(),
            substitute_body: false,
            insecure: false,
            jq: None,
            jq_enabled: true,
            jq_views: Vec::new(),
            jq_current: None,
            params: Default::default(),
            headers: Default::default(),
            variables: Default::default(),
            body: None,
        }
    }

    fn slugs(p: &Project) -> Vec<String> {
        p.requests().iter().map(|l| l.slug.clone()).collect()
    }

    #[test]
    fn the_listing_is_loaded_at_open_and_requests_are_parsed_on_demand() {
        let (_d, mut p) = fixture();
        assert_eq!(slugs(&p), ["auth/login", "main/ping"]);
        assert!(p.held_request("main/ping").is_none());
        let r = p.open_request("main/ping").unwrap();
        assert_eq!(r.url, "https://{{host}}/ping");
        assert!(p.held_request("main/ping").is_some());
        p.close_request("main/ping");
        assert!(p.held_request("main/ping").is_none());
        assert!(matches!(p.open_request("main/nope"), Err(Error::NotFound(_))));
    }

    #[test]
    fn read_request_parses_without_holding() {
        let (_d, mut p) = fixture();
        let r = p.read_request("main/ping").unwrap();
        assert_eq!(r.url, "https://{{host}}/ping");
        assert!(p.held_request("main/ping").is_none(), "a one-off read holds nothing");
        assert!(matches!(p.read_request("main/nope"), Err(Error::NotFound(_))));
    }

    #[test]
    fn save_writes_updates_the_held_copy_and_the_listing_and_is_not_journaled() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        let mut r = req("https://x/new");
        r.name = Some("Renamed Ping".into());
        p.save_request("main/ping", &r).unwrap();
        assert!(read(&dir, "requests/main/ping.toml").unwrap().contains("https://x/new"));
        assert_eq!(p.held_request("main/ping").unwrap().url, "https://x/new");
        assert_eq!(
            p.requests().iter().find(|l| l.slug == "main/ping").unwrap().name.as_deref(),
            Some("Renamed Ping")
        );
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn create_derives_and_dedupes_the_slug_and_journals_a_created_op() {
        let (dir, mut p) = fixture();
        let (slug, leaf) = p.create_request("main/My Ping!", req("u")).unwrap();
        assert_eq!((slug.as_str(), leaf.as_str()), ("main/my-ping", "My Ping!"));
        assert!(dir.path().join("requests/main/my-ping.toml").is_file());
        assert!(slugs(&p).contains(&"main/my-ping".to_string()));
        assert!(matches!(
            p.create_request("main/my ping!", req("u")),
            Err(Error::AlreadyExists(_))
        ));
        let (slug2, _) = p.create_request("main/My-Ping", req("u")).unwrap();
        assert_eq!(slug2, "main/my-ping-2");
        assert_eq!(p.journal_len(), 2);
        let e = p.journal.pop_undo().unwrap();
        assert!(matches!(e.ops[0], Op::Created { .. }));
    }

    #[test]
    fn rename_moves_the_file_rewrites_the_name_and_records_the_move() {
        let (dir, mut p) = fixture();
        let (slug, _) = p.rename_request("main/ping", "main/Pong").unwrap();
        assert_eq!(slug, "main/pong");
        assert!(!dir.path().join("requests/main/ping.toml").exists());
        assert!(read(&dir, "requests/main/pong.toml").unwrap().contains("name = \"Pong\""));
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.meta.moves, vec![("main/ping".to_string(), "main/pong".to_string())]);
        assert!(e.ops.iter().any(|o| matches!(o, Op::Renamed { .. })));
        assert!(e.ops.iter().any(|o| matches!(o, Op::Text { .. })), "the name rewrite");
    }

    #[test]
    fn move_keeps_the_sub_path_dedupes_and_cascades_the_order_lists() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("project.toml"),
            "spaces = [\"main\", \"auth\"]\n[space.main]\norder = [\"ping\"]\n[space.auth]\norder = [\"login\"]\n",
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let new = p.move_request("main/ping", "auth").unwrap();
        assert_eq!(new, "auth/ping");
        assert_eq!(order::space_order(p.meta(), "main"), Vec::<String>::new().as_slice());
        assert_eq!(order::space_order(p.meta(), "auth"), ["login", "ping"]);
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.meta.moves, vec![("main/ping".to_string(), "auth/ping".to_string())]);
    }

    #[test]
    fn move_all_pre_flights_the_whole_space_and_stores_pairs_not_bodies() {
        let (dir, mut p) = fixture();
        for i in 0..5 {
            std::fs::write(
                dir.path().join(format!("requests/main/r{i}.toml")),
                "method = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("requests/auth/r3.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        let (moved, _) = p.move_all_requests("main", "auth").unwrap();
        assert_eq!(moved.len(), 6);
        assert!(moved.iter().any(|(f, t)| f == "main/r3" && t == "auth/r3-2"));
        assert!(!dir.path().join("requests/main/r0.toml").exists());
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.meta.moves.len(), 6);
        assert!(e.ops.iter().all(|o| matches!(o, Op::Renamed { .. } | Op::Text { .. })));
        assert!(
            !e.ops.iter().any(
                |o| matches!(o, Op::Text { path, .. } if path.as_str().starts_with("requests/"))
            ),
            "no request text in the journal"
        );
    }

    /// Spec 2026-11-02 drag into folders D4, D22: everything arrives last,
    /// folders included, in the mixed order the sidebar showed.
    #[test]
    fn move_all_arrives_in_the_order_the_sidebar_showed_not_alphabetically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main/sub")).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/auth")).unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            "spaces = [\"main\", \"auth\"]\n\n[space.main]\norder = [\"b\", \"a\"]\n\n[space.auth]\norder = [\"login\"]\n",
        )
        .unwrap();
        for slug in ["main/a", "main/b", "main/sub/c", "auth/login"] {
            std::fs::write(
                dir.path().join(format!("requests/{slug}.toml")),
                "method = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
        }
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let (moved, _) = p.move_all_requests("main", "auth").unwrap();
        assert_eq!(
            moved.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(),
            ["main/b", "main/a", "main/sub/c"],
            "the displayed order, not the listing's alphabetical one"
        );
        assert_eq!(order::space_order(p.meta(), "auth"), ["login", "b", "a", "sub/", "sub/c"]);
        assert!(dir.path().join("requests/auth/sub/c.toml").is_file());
    }

    #[test]
    fn move_all_from_a_space_with_no_directory_is_a_no_op() {
        let (_dir, mut p) = fixture();
        let (moved, _) = p.move_all_requests("ghost", "auth").unwrap();
        assert!(moved.is_empty());
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn move_all_leaves_a_non_slug_file_behind_with_a_warning() {
        let (dir, mut p) = fixture();
        std::fs::write(
            dir.path().join("requests/main/Get User.toml"),
            "method = \"GET\"\nurl = \"u\"\n",
        )
        .unwrap();
        p.relist();
        let (moved, warnings) = p.move_all_requests("main", "auth").unwrap();
        assert_eq!(moved, vec![("main/ping".to_string(), "auth/ping".to_string())]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("requests/main/Get User.toml"));
        assert!(dir.path().join("requests/main/Get User.toml").is_file());
        assert!(dir.path().join("requests/auth/ping.toml").is_file());
    }

    #[test]
    fn rename_refuses_to_cross_spaces() {
        let (dir, mut p) = fixture();
        let r = p.rename_request("main/ping", "auth/Pong");
        assert!(matches!(r, Err(Error::BadName(_))), "{r:?}");
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert_eq!(p.journal_len(), 0);
    }

    /// #206: a hand-made folder whose name `slugify` would change (`old-`,
    /// `-wip`) is still the folder a rename that names it lands in.
    #[test]
    fn rename_in_a_hand_made_folder_keeps_the_folder() {
        let (dir, mut p) = fixture();
        for folder in ["old-", "-wip"] {
            std::fs::create_dir_all(dir.path().join(format!("requests/main/{folder}"))).unwrap();
            std::fs::write(
                dir.path().join(format!("requests/main/{folder}/ping.toml")),
                "name = \"ping\"\nmethod = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
        }
        p.relist();
        for folder in ["old-", "-wip"] {
            let from = format!("main/{folder}/ping");
            let (slug, leaf) = p.rename_request(&from, &format!("main/{folder}/Ping2")).unwrap();
            assert_eq!(
                (slug.as_str(), leaf.as_str()),
                (format!("main/{folder}/ping2").as_str(), "Ping2")
            );
            assert!(dir.path().join(format!("requests/main/{folder}/ping2.toml")).is_file());
        }
        let (slug, _) = p.create_request("main/old-/Fresh", req("u")).unwrap();
        assert_eq!(slug, "main/old-/fresh", "a create names the folder the same way");
    }

    /// #206: in a hand-made space whose name `slugify` would change, a
    /// rename stays in the space and a create lands in it, never in a new
    /// `requests/old/` beside it.
    #[test]
    fn a_hand_made_space_keeps_its_creates_and_renames() {
        let (dir, _p) = fixture();
        std::fs::create_dir_all(dir.path().join("requests/old-")).unwrap();
        std::fs::write(
            dir.path().join("requests/old-/ping.toml"),
            "name = \"ping\"\nmethod = \"GET\"\nurl = \"u\"\n",
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let (slug, _) = p.rename_request("old-/ping", "old-/Ping2").unwrap();
        assert_eq!(slug, "old-/ping2");
        let (slug, _) = p.create_request("old-/Fresh", req("u")).unwrap();
        assert_eq!(slug, "old-/fresh");
        assert!(dir.path().join("requests/old-/fresh.toml").is_file());
        assert!(!dir.path().join("requests/old").exists());
    }

    /// #209: a rename that leaves the name as it is leaves the file where
    /// it is, even when the file's slug is not the one its name derives: a
    /// nameless hand-made `ping-` (whose name `slugify` turns into `ping`,
    /// free or taken), and a named file under a slug of its own, its name
    /// read as the prompt reads it back (trimmed). Another name, or the
    /// same name in another folder, derives its slug as before.
    #[test]
    fn a_rename_to_the_same_name_keeps_the_slug() {
        for ping_taken in [false, true] {
            let (dir, mut p) = fixture();
            std::fs::write(
                dir.path().join("requests/main/ping-.toml"),
                "method = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
            // The fixture has a `main/ping`.
            if !ping_taken {
                std::fs::remove_file(dir.path().join("requests/main/ping.toml")).unwrap();
            }
            p.relist();
            let (slug, leaf) = p.rename_request("main/ping-", "main/ping-").unwrap();
            assert_eq!((slug.as_str(), leaf.as_str()), ("main/ping-", "ping-"), "{ping_taken}");
            assert!(dir.path().join("requests/main/ping-.toml").is_file());
            assert!(!dir.path().join("requests/main/ping-2.toml").exists());
            assert_eq!(dir.path().join("requests/main/ping.toml").exists(), ping_taken);
        }

        let (dir, mut p) = fixture();
        for (name, typed) in [("Get User", "main/Get User"), (" Pong ", "main/ Pong ")] {
            std::fs::write(
                dir.path().join("requests/main/odd.toml"),
                format!("name = {name:?}\nmethod = \"GET\"\nurl = \"u\"\n"),
            )
            .unwrap();
            p.relist();
            let (slug, _) = p.rename_request("main/odd", typed).unwrap();
            assert_eq!(slug, "main/odd", "{typed:?}");
        }
        assert_eq!(slugs(&p).iter().filter(|s| s.starts_with("main/")).count(), 2);
        let (slug, _) = p.rename_request("main/odd", "main/sub/ Pong ").unwrap();
        assert_eq!(slug, "main/sub/pong", "the same name in another folder moves the file");
        let (slug, _) = p.rename_request("main/sub/pong", "main/Other").unwrap();
        assert_eq!(slug, "main/other", "another name derives its slug");
    }

    /// #209 keeps only an unchanged name's slug: a request at a `-2` slug
    /// renamed to another spelling still moves to its freed base slug.
    #[test]
    fn a_changed_name_still_takes_a_freed_base_slug() {
        let (_dir, mut p) = fixture();
        p.create_request("main/Get User", req("u")).unwrap();
        let (b, _) = p.create_request("main/get user!", req("u")).unwrap();
        assert_eq!(b, "main/get-user-2");
        p.delete_request("main/get-user").unwrap();
        let (slug, _) = p.rename_request(&b, "main/Get User").unwrap();
        assert_eq!(slug, "main/get-user");
    }

    #[test]
    fn move_all_refuses_whole_when_a_source_is_missing() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("requests/main/r1.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        std::fs::write(dir.path().join("requests/main/r2.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        std::fs::remove_file(dir.path().join("requests/main/r1.toml")).unwrap();
        let r = p.move_all_requests("main", "auth");
        assert!(matches!(r, Err(Error::NotFound(_))), "{r:?}");
        assert!(dir.path().join("requests/main/r2.toml").is_file());
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert!(!dir.path().join("requests/auth/r2.toml").exists());
        assert!(!dir.path().join("requests/auth/ping.toml").exists());
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn delete_trashes_and_duplicate_copies_next_to_its_source() {
        let (dir, mut p) = fixture();
        p.delete_request("main/ping").unwrap();
        assert!(!dir.path().join("requests/main/ping.toml").exists());
        assert!(dir.path().join(TRASH_DIR).join("1/requests/main/ping.toml").is_file());
        assert!(!slugs(&p).contains(&"main/ping".to_string()));
        let copy = p.duplicate_request("auth/login").unwrap();
        assert_eq!(copy, "auth/login-copy");
        assert!(read(&dir, "requests/auth/login-copy.toml").unwrap().contains("Login copy"));
        assert_eq!(p.journal_len(), 2);
    }

    #[test]
    fn duplicate_journals_a_single_created_op_for_the_copy() {
        let (_dir, mut p) = fixture();
        let copy = p.duplicate_request("auth/login").unwrap();
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.ops.len(), 1);
        assert!(
            matches!(&e.ops[0], Op::Created { path } if path.as_str() == format!("requests/{copy}.toml"))
        );
    }

    /// A project whose `main` space holds `slugs` and whose `[space.main]
    /// order` is `order`; `auth` exists and is empty but listed.
    fn folder_project(order: &[&str], slugs: &[&str]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main")).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/auth")).unwrap();
        let list = order.iter().map(|e| format!("\"{e}\"")).collect::<Vec<_>>().join(", ");
        std::fs::write(
            dir.path().join("project.toml"),
            format!(
                "spaces = [\"main\", \"auth\"]\n[space.main]\norder = [{list}]\n[space.auth]\norder = [\"x/\"]\n"
            ),
        )
        .unwrap();
        for slug in slugs {
            let path = dir.path().join(format!("requests/{slug}.toml"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "method = \"GET\"\nurl = \"u\"\n").unwrap();
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    /// Spec 2026-11-01 reorder folders R9: a folder whose last request
    /// leaves loses its entry, with every ancestor the same op emptied,
    /// and undo puts the list back byte for byte.
    #[test]
    fn deleting_a_folders_last_request_drops_its_entry_and_undo_restores_it() {
        let (dir, mut p) = folder_project(
            &["b/", "a/", "a/c/", "a/c/x", "z/"],
            &["main/a/c/x", "main/b/y", "main/z/q"],
        );
        let before = read(&dir, "project.toml");
        p.delete_request("main/a/c/x").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "z/"]);
        p.undo().unwrap();
        assert_eq!(read(&dir, "project.toml"), before);
    }

    #[test]
    fn deleting_a_request_whose_folder_keeps_others_leaves_the_entry() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/a/y", "main/b/z"]);
        p.delete_request("main/a/x").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/"]);
    }

    /// Spec 2026-11-02 drag into folders D4: the new folder and the
    /// request in it land last.
    #[test]
    fn a_rename_out_of_a_folder_drops_it_and_into_a_new_one_appends_it() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/b/z"]);
        p.rename_request("main/a/x", "main/c/x").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "c/", "c/x"]);
    }

    /// A rename that changes level inside a folder leaves that folder where
    /// it was: it never stopped existing, so it is not new. What does
    /// arrive (the new `sub/`, the request) lands last (D4).
    #[test]
    fn a_rename_within_a_folder_keeps_the_folders_slot() {
        let (_dir, mut p) = folder_project(&["a/", "b/"], &["main/a/x", "main/b/z"]);
        p.rename_request("main/a/x", "main/a/sub/x").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a/", "b/", "a/sub/", "a/sub/x"]);
        p.rename_request("main/a/sub/x", "main/a/x").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a/", "b/", "a/x"]);
    }

    /// D4: a create lands last, in a level whatever it lists (replaces
    /// "a create in an unarranged level writes nothing").
    #[test]
    fn a_create_in_a_new_folder_lands_last() {
        let (_dir, mut p) = folder_project(&["r"], &["main/r", "main/a/x"]);
        p.create_request("main/c/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["r", "c/", "c/new"]);
    }

    /// D4: every new ancestor arrives, outermost first, then the request.
    #[test]
    fn a_create_in_a_new_nested_folder_arrives_every_new_ancestor() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/b/z"]);
        p.create_request("main/c/d/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "c/", "c/d/", "c/d/new"]);
    }

    /// An entry left behind by a folder emptied outside the app never
    /// hands its slot to a new folder of that name: the arrival removes it
    /// and appends (D4; `arrive` never lists an entry twice).
    #[test]
    fn a_create_in_a_folder_made_again_drops_its_leftover_entry() {
        let (_dir, mut p) = folder_project(&["c/", "b/", "a/"], &["main/a/x", "main/b/z"]);
        p.create_request("main/c/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "c/", "c/new"]);
    }

    /// D4: the folder keeps its slot; the request lands last in it.
    #[test]
    fn a_create_in_an_existing_folder_keeps_its_slot() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/b/z"]);
        p.create_request("main/b/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "b/new"]);
    }

    /// D4: the new folder and the request land last in the other space.
    #[test]
    fn a_move_to_another_space_cascades_folders_on_both_sides() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/b/z"]);
        std::fs::create_dir_all(p.root().join("requests/auth/x")).unwrap();
        std::fs::write(p.root().join("requests/auth/x/k.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        p.move_request("main/a/x", "auth").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/"]);
        assert_eq!(order::space_order(p.meta(), "auth"), ["x/", "a/", "a/x"]);
    }

    /// D4, D22: everything arrives last, in the order `main` showed it.
    #[test]
    fn move_all_cascades_folders_on_both_sides() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/a/y", "main/b/z"]);
        std::fs::create_dir_all(p.root().join("requests/auth/x")).unwrap();
        std::fs::write(p.root().join("requests/auth/x/k.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        p.move_all_requests("main", "auth").unwrap();
        assert!(order::space_order(p.meta(), "main").is_empty());
        assert_eq!(order::space_order(p.meta(), "auth"), ["x/", "b/", "b/z", "a/", "a/x", "a/y"]);
    }

    /// Spec §5.2: a batch delete is one journal entry, refused whole on a
    /// bad slug, and its undo restores every file byte for byte plus the
    /// order list.
    #[test]
    fn delete_requests_is_one_entry_and_undoes_byte_identical() {
        let (dir, _p) = fixture();
        std::fs::write(
            dir.path().join("project.toml"),
            "spaces = [\"main\", \"auth\"]\n[space.main]\norder = [\"r1\", \"r2\", \"ping\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("requests/main/r1.toml"),
            "method = \"GET\"\nurl = \"u1\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("requests/main/r2.toml"),
            "# kept\nmethod = \"GET\"\nurl = \"u2\"\n",
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let before = (read(&dir, "requests/main/r1.toml"), read(&dir, "requests/main/r2.toml"));
        p.delete_requests(&["main/r1".into(), "main/r2".into()]).unwrap();
        assert_eq!(p.journal_len(), 1, "one entry");
        assert!(!dir.path().join("requests/main/r1.toml").exists());
        assert!(!dir.path().join("requests/main/r2.toml").exists());
        assert_eq!(crate::order::space_order(p.meta(), "main"), ["ping"]);
        p.undo().unwrap();
        assert_eq!(
            (read(&dir, "requests/main/r1.toml"), read(&dir, "requests/main/r2.toml")),
            before
        );
        assert_eq!(crate::order::space_order(p.meta(), "main"), ["r1", "r2", "ping"]);
        p.redo().unwrap();
        assert!(!dir.path().join("requests/main/r1.toml").exists());
        assert!(!dir.path().join("requests/main/r2.toml").exists());
    }

    #[test]
    fn delete_requests_refuses_whole_when_one_is_missing() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("requests/main/r1.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        let r = p.delete_requests(&["main/r1".into(), "main/nope".into()]);
        assert!(matches!(r, Err(Error::NotFound(_))), "{r:?}");
        assert!(dir.path().join("requests/main/r1.toml").is_file());
        assert_eq!(p.journal_len(), 0);
    }

    /// A listed file whose name is not a valid slug refuses the whole
    /// batch before anything moves.
    #[test]
    fn delete_requests_refuses_whole_on_an_invalid_slug() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("requests/main/r1.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        let r = p.delete_requests(&["main/r1".into(), "main/Get User".into()]);
        assert!(matches!(r, Err(Error::BadName(_))), "{r:?}");
        assert!(dir.path().join("requests/main/r1.toml").is_file());
        assert_eq!(p.journal_len(), 0);
    }

    #[test]
    fn delete_requests_dedupes() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("requests/main/r1.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        p.delete_requests(&["main/r1".into(), "main/r1".into()]).unwrap();
        assert_eq!(p.journal_len(), 1);
    }

    /// Nested transactions join the outer one (nothing pinned it before).
    #[test]
    fn a_nested_transaction_joins_the_outer_entry() {
        let (dir, mut p) = fixture();
        std::fs::write(dir.path().join("requests/main/r1.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        p.transaction("outer", EntryMeta::default(), |p| {
            p.delete_request("main/r1")?;
            p.delete_request("main/ping")
        })
        .unwrap();
        assert_eq!(p.journal_len(), 1);
        p.undo().unwrap();
        assert!(dir.path().join("requests/main/r1.toml").is_file());
        assert!(dir.path().join("requests/main/ping.toml").is_file());
    }

    /// Three listed requests in one space, so a put's anchor is a real
    /// position in the order list rather than the alphabetical fallback.
    fn ordered_fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main")).unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            "spaces = [\"main\"]\n\n[space.main]\norder = [\"a\", \"b\", \"c\"]\n",
        )
        .unwrap();
        for slug in ["main/a", "main/b", "main/c"] {
            std::fs::write(
                dir.path().join(format!("requests/{slug}.toml")),
                "method = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    /// `ordered_fixture`'s twin with no order array at all: a fresh
    /// project, where the sidebar falls back to alphabetical.
    fn unordered_fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\"]\n").unwrap();
        for slug in ["main/a", "main/b", "main/c"] {
            std::fs::write(
                dir.path().join(format!("requests/{slug}.toml")),
                "method = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    /// `unordered_fixture` plus a folder `f` holding `x`, for a put whose
    /// anchor sits in another folder than its source.
    fn folder_fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main/f")).unwrap();
        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\"]\n").unwrap();
        for slug in ["main/a", "main/f/x"] {
            std::fs::write(
                dir.path().join(format!("requests/{slug}.toml")),
                "method = \"GET\"\nurl = \"u\"\n",
            )
            .unwrap();
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    /// `P`: the copy takes the anchor's own slot, pushing it down.
    #[test]
    fn duplicate_at_inserts_the_copy_above_its_anchor() {
        let (_dir, mut p) = ordered_fixture();
        let copy = p.duplicate_request_at("main/a", "main/c", true).unwrap();
        assert_eq!(copy, "main/a-copy");
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "b", "a-copy", "c"]);
    }

    /// `p`: the copy lands directly below the anchor.
    #[test]
    fn duplicate_at_inserts_the_copy_below_its_anchor() {
        let (_dir, mut p) = ordered_fixture();
        let copy = p.duplicate_request_at("main/a", "main/c", false).unwrap();
        assert_eq!(copy, "main/a-copy");
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "b", "c", "a-copy"]);
    }

    /// A put in a project whose list the TUI completed (spec 2026-11-02
    /// drag into folders D5) lands where the cursor is: the anchor is
    /// listed, so nothing is seeded (replaces the seed from the display
    /// order).
    #[test]
    fn a_put_in_a_completed_list_lands_below_its_anchor() {
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        let copy = p.duplicate_request_at("main/a", "main/c", false).unwrap();
        assert_eq!(copy, "main/a-copy");
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "b", "c", "a-copy"]);
    }

    #[test]
    fn a_put_above_in_a_completed_list_takes_the_anchors_slot() {
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        p.duplicate_request_at("main/a", "main/c", true).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "b", "a-copy", "c"]);
    }

    /// Undo restores `project.toml` as the put found it.
    #[test]
    fn undoing_a_put_restores_project_toml_exactly() {
        let (dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        let before = read(&dir, "project.toml").unwrap();
        p.duplicate_request_at("main/a", "main/c", false).unwrap();
        p.undo().unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "b", "c"]);
        assert_eq!(read(&dir, "project.toml").as_deref(), Some(before.as_str()));
        assert!(!p.request_exists("main/a-copy"));
    }

    /// `yyP` on the yanked row itself: anchor == source is still a put, so
    /// the copy lands above (and below for `p`).
    #[test]
    fn a_put_on_its_own_source_lands_above_or_below() {
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        p.duplicate_request_at("main/a", "main/a", true).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a-copy", "a", "b", "c"]);
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        p.duplicate_request_at("main/a", "main/a", false).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "a-copy", "b", "c"]);
    }

    /// The old entry point is the new one anchored on its own source.
    #[test]
    fn duplicate_request_anchors_on_its_own_source() {
        let (_dir, mut p) = ordered_fixture();
        p.duplicate_request("main/a").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "a-copy", "b", "c"]);
    }

    /// D4: a plain Duplicate keeps its place beside the original in a
    /// completed list (replaces "a plain duplicate never seeds").
    #[test]
    fn a_plain_duplicate_lands_beside_its_original() {
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        p.duplicate_request("main/a").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "a-copy", "b", "c"]);
    }

    /// D4: a copy whose anchor is in another space is made beside its
    /// source and arrives, last in its level.
    #[test]
    fn a_put_whose_anchor_is_in_another_space_arrives_last() {
        let (dir, mut p) = fixture();
        std::fs::write(
            dir.path().join("requests/main/zed.toml"),
            "method = \"GET\"\nurl = \"u\"\n",
        )
        .unwrap();
        p.relist();
        p.complete_order().unwrap();
        let copy = p.duplicate_request_at("main/ping", "auth/login", false).unwrap();
        assert_eq!(copy, "main/ping-copy");
        assert_eq!(order::space_order(p.meta(), "main"), ["ping", "zed", "ping-copy"]);
        assert_eq!(order::space_order(p.meta(), "auth"), ["login"]);
    }

    /// A put's copy is made in the anchor's folder, directly after the
    /// anchor in that level, and the undo restores `project.toml` exactly.
    #[test]
    fn a_put_into_another_folder_creates_the_copy_there() {
        let (dir, mut p) = folder_fixture();
        p.complete_order().unwrap();
        let before = read(&dir, "project.toml").unwrap();
        let copy = p.duplicate_request_at("main/a", "main/f/x", false).unwrap();
        assert_eq!(copy, "main/f/a-copy");
        assert!(dir.path().join("requests/main/f/a-copy.toml").is_file());
        assert!(!dir.path().join("requests/main/a-copy.toml").exists());
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "f/", "f/x", "f/a-copy"]);
        p.undo().unwrap();
        assert!(!dir.path().join("requests/main/f/a-copy.toml").exists());
        assert_eq!(read(&dir, "project.toml").as_deref(), Some(before.as_str()));
    }

    #[test]
    fn a_put_above_in_another_folder_takes_the_anchors_slot() {
        let (_dir, mut p) = folder_fixture();
        p.complete_order().unwrap();
        let copy = p.duplicate_request_at("main/a", "main/f/x", true).unwrap();
        assert_eq!(copy, "main/f/a-copy");
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "f/", "f/a-copy", "f/x"]);
    }

    /// The `<name> copy` collision rule runs in the anchor's folder.
    #[test]
    fn a_put_into_another_folder_numbers_a_name_clash_there() {
        let (dir, _p) = folder_fixture();
        std::fs::write(
            dir.path().join("requests/main/f/a-copy.toml"),
            "name = \"a copy\"\nmethod = \"GET\"\nurl = \"u\"\n",
        )
        .unwrap();
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let copy = p.duplicate_request_at("main/a", "main/f/x", false).unwrap();
        assert_eq!(copy, "main/f/a-copy-2");
        assert!(read(&dir, "requests/main/f/a-copy-2.toml").unwrap().contains("a copy 2"));
    }

    /// `3p`: three copies directly below the anchor, in the order they were
    /// made, as one undo step (vim's `3p` is one change).
    #[test]
    fn a_counted_put_lands_its_copies_below_the_anchor_as_one_undo_step() {
        let (dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        let before = read(&dir, "project.toml").unwrap();
        let copies = p.duplicate_requests_at("main/a", "main/b", false, 3).unwrap();
        assert_eq!(copies, ["main/a-copy", "main/a-copy-2", "main/a-copy-3"]);
        assert_eq!(
            order::space_order(p.meta(), "main"),
            ["a", "b", "a-copy", "a-copy-2", "a-copy-3", "c"]
        );
        p.undo().unwrap();
        for c in &copies {
            assert!(!p.request_exists(c), "{c}");
        }
        assert_eq!(read(&dir, "project.toml").as_deref(), Some(before.as_str()));
    }

    /// `3P`: the copies sit above the anchor in the same reading order.
    #[test]
    fn a_counted_put_above_keeps_its_copies_in_order() {
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        p.duplicate_requests_at("main/a", "main/b", true, 3).unwrap();
        assert_eq!(
            order::space_order(p.meta(), "main"),
            ["a", "a-copy", "a-copy-2", "a-copy-3", "b", "c"]
        );
    }

    #[test]
    fn held_request_drift_is_none_right_after_open() {
        let (_dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        assert_eq!(p.held_request_drift("main/ping"), None);
    }

    #[test]
    fn held_request_drift_reports_changed_after_an_outside_write() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        std::fs::write(
            dir.path().join("requests/main/ping.toml"),
            "name = \"Ping\"\nmethod = \"GET\"\nurl = \"https://{{host}}/ping-changed\"\n",
        )
        .unwrap();
        assert_eq!(p.held_request_drift("main/ping"), Some(HeldDrift::Changed));
    }

    #[test]
    fn held_request_drift_reports_vanished_after_an_outside_delete() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        std::fs::remove_file(dir.path().join("requests/main/ping.toml")).unwrap();
        assert_eq!(p.held_request_drift("main/ping"), Some(HeldDrift::Vanished));
    }

    #[test]
    fn held_request_drift_is_none_again_after_save_request_refreshes_the_stamp() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        std::fs::write(
            dir.path().join("requests/main/ping.toml"),
            "name = \"Ping\"\nmethod = \"GET\"\nurl = \"https://{{host}}/ping-changed\"\n",
        )
        .unwrap();
        assert_eq!(p.held_request_drift("main/ping"), Some(HeldDrift::Changed));
        p.save_request("main/ping", &req("https://{{host}}/ping-saved")).unwrap();
        assert_eq!(p.held_request_drift("main/ping"), None);
    }

    #[test]
    fn held_request_drift_survives_a_poll_reload_the_seed_stamp_is_kept() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        std::fs::write(
            dir.path().join("requests/main/ping.toml"),
            "name = \"Ping\"\nmethod = \"GET\"\nurl = \"https://{{host}}/ping-changed\"\n",
        )
        .unwrap();
        p.invalidate_stamps();
        assert!(p.poll().0);
        assert_eq!(p.held_request("main/ping").unwrap().url, "https://{{host}}/ping-changed");
        assert_eq!(
            p.held_request_drift("main/ping"),
            Some(HeldDrift::Changed),
            "poll re-read the held copy but the seed stamp is kept"
        );
    }

    #[test]
    fn held_request_drift_is_none_for_a_slug_that_is_not_held() {
        let (_dir, p) = fixture();
        assert_eq!(p.held_request_drift("main/ping"), None);
    }

    #[test]
    fn held_request_drift_is_none_for_the_new_slug_after_a_rename() {
        let (_dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        let (to_slug, _) = p.rename_request("main/ping", "main/renamed").unwrap();
        assert_eq!(
            p.held_request_drift(&to_slug),
            None,
            "the rename rewrote the file's `name` and re-stamped the held \
             entry itself — its own write is never an outside edit"
        );
        // And re-opening it (what the app used to rely on) changes nothing.
        p.open_request(&to_slug).unwrap();
        assert_eq!(p.held_request_drift(&to_slug), None);
    }

    #[test]
    fn a_rename_does_not_launder_an_outside_edit_into_a_clean_stamp() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        // The outside world edits the file (a different length, so the
        // stamp differs whatever the mtime granularity is), and only then
        // does the user rename the open request.
        std::fs::write(
            dir.path().join("requests/main/ping.toml"),
            "url = \"https://example.test/edited-outside-the-app\"\n",
        )
        .unwrap();
        let (to_slug, _) = p.rename_request("main/ping", "main/renamed").unwrap();
        assert_eq!(
            p.held_request_drift(&to_slug),
            Some(HeldDrift::Changed),
            "the rename only rewrote `name`; the outside edit is still \
             unseen by the editor and must still be reported"
        );
    }

    #[test]
    fn a_poll_after_an_outside_delete_keeps_the_held_entry_and_still_reports_vanished() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        std::fs::remove_file(dir.path().join("requests/main/ping.toml")).unwrap();
        p.invalidate_stamps();
        assert!(p.poll().0);
        assert!(
            p.held_request("main/ping").is_some(),
            "the entry (and its seed stamp) survives a file that vanished"
        );
        assert_eq!(
            p.held_request_drift("main/ping"),
            Some(HeldDrift::Vanished),
            "a reload must never blind the drift check"
        );
    }

    #[test]
    fn a_poll_after_an_outside_write_of_unparsable_text_still_reports_changed() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        std::fs::write(dir.path().join("requests/main/ping.toml"), "url = \"unclosed\n").unwrap();
        p.invalidate_stamps();
        assert!(p.poll().0);
        assert_eq!(
            p.held_request("main/ping").unwrap().url,
            "https://{{host}}/ping",
            "the last-good body is kept"
        );
        assert_eq!(p.held_request_drift("main/ping"), Some(HeldDrift::Changed));
    }

    #[test]
    fn undo_of_a_move_carries_the_seed_stamp_and_still_reports_an_outside_edit() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        p.move_request("main/ping", "auth").unwrap();
        // The outside world edits the moved file (a different length, so
        // the stamp differs whatever the mtime granularity is).
        std::fs::write(
            dir.path().join("requests/auth/ping.toml"),
            "url = \"https://example.test/edited-outside-the-app\"\n",
        )
        .unwrap();
        p.undo().unwrap().unwrap();
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert_eq!(
            p.held_request_drift("main/ping"),
            Some(HeldDrift::Changed),
            "the undo moved the file back byte-for-byte; the outside edit \
             the editor never saw must still be reported"
        );
    }

    #[test]
    fn undo_of_a_move_re_stamps_an_entry_that_was_still_clean() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        p.move_request("main/ping", "auth").unwrap();
        p.undo().unwrap().unwrap();
        assert!(dir.path().join("requests/main/ping.toml").is_file());
        assert_eq!(
            p.held_request_drift("main/ping"),
            None,
            "the app's own move and undo are never an outside edit"
        );
    }

    #[test]
    fn redo_of_a_move_follows_the_held_entry_to_the_new_slug() {
        let (_dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        p.move_request("main/ping", "auth").unwrap();
        p.undo().unwrap().unwrap();
        p.redo().unwrap().unwrap();
        assert!(p.held_request("auth/ping").is_some());
        assert_eq!(p.held_request_drift("auth/ping"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_statted_reports_changed_not_vanished() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        let space = dir.path().join("requests/main");
        std::fs::set_permissions(&space, std::fs::Permissions::from_mode(0o000)).unwrap();
        let drift = p.held_request_drift("main/ping");
        std::fs::set_permissions(&space, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            drift,
            Some(HeldDrift::Changed),
            "\"deleted outside the app\" would be a lie, and would withhold Reload"
        );
    }

    #[test]
    fn a_display_name_only_rename_re_stamps_the_held_entry_it_rewrote() {
        // `Ping!!!` slugifies back to `ping`, so the file never moves —
        // but its `name` is rewritten, so the stamp still has to follow.
        let (_dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        let (to_slug, leaf) = p.rename_request("main/ping", "main/Ping!!!").unwrap();
        assert_eq!((to_slug.as_str(), leaf.as_str()), ("main/ping", "Ping!!!"));
        assert_eq!(
            p.held_request_drift("main/ping"),
            None,
            "the rename rewrote `name` in place; the app must never see its \
             own write as an outside edit"
        );
    }

    #[test]
    fn a_display_name_only_rename_does_not_launder_an_outside_edit() {
        let (dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        std::fs::write(
            dir.path().join("requests/main/ping.toml"),
            "url = \"https://example.test/edited-outside-the-app\"\n",
        )
        .unwrap();
        let (to_slug, _) = p.rename_request("main/ping", "main/Ping!!!").unwrap();
        assert_eq!(to_slug, "main/ping");
        assert_eq!(
            p.held_request_drift("main/ping"),
            Some(HeldDrift::Changed),
            "the outside edit is still unseen by the editor"
        );
    }

    // --- spec 2026-11-02 drag into folders: complete lists, moves ---

    fn main_order(p: &Project) -> Vec<String> {
        order::space_order(p.meta(), "main").to_vec()
    }

    fn write(dir: &tempfile::TempDir, rel: &str, text: &str) {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn req_item(slug: &str) -> TreeItem {
        TreeItem::Request(slug.to_string())
    }

    fn folder_item(path: &str) -> TreeItem {
        TreeItem::Folder(path.to_string())
    }

    /// `main` holds `users/ { login, v1/ { list, get } }`, `health` and
    /// `orders/ { new }`, its list complete in that order. `get` has no
    /// `name`.
    fn tree_fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main")).unwrap();
        std::fs::write(
            dir.path().join("project.toml"),
            "spaces = [\"main\"]\n\n[space.main]\norder = [\"users/\", \"users/login\", \"users/v1/\", \"users/v1/list\", \"users/v1/get\", \"health\", \"orders/\", \"orders/new\"]\n",
        )
        .unwrap();
        for (slug, name) in [
            ("main/users/login", Some("Login")),
            ("main/users/v1/list", Some("List")),
            ("main/users/v1/get", None),
            ("main/health", Some("Health")),
            ("main/orders/new", Some("New")),
        ] {
            let name = name.map(|n| format!("name = \"{n}\"\n")).unwrap_or_default();
            write(
                &dir,
                &format!("requests/{slug}.toml"),
                &format!("{name}method = \"GET\"\nurl = \"https://x/{slug}\"\n"),
            );
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    /// Spec D5a: the write names every item, is not an undo step, and a
    /// complete list writes nothing.
    #[test]
    fn complete_order_writes_what_a_list_misses_once_and_unjournaled() {
        let (dir, mut p) = fixture();
        write(&dir, "requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        assert!(p.complete_order().unwrap());
        assert_eq!(main_order(&p), ["ping", "f/", "f/x"]);
        assert_eq!(order::space_order(p.meta(), "auth"), ["login"]);
        assert_eq!(p.journal_len(), 0, "not an undo step");
        let after = read(&dir, "project.toml");
        assert!(after.as_deref().unwrap().contains("name = \"Auth\""), "the rest is kept");
        assert!(!p.complete_order().unwrap(), "a complete list writes nothing");
        assert_eq!(read(&dir, "project.toml"), after);
    }

    #[test]
    fn complete_order_writes_nothing_for_a_complete_list() {
        let (dir, mut p) = tree_fixture();
        let before = read(&dir, "project.toml");
        assert!(!p.complete_order().unwrap());
        assert_eq!(read(&dir, "project.toml"), before);
    }

    /// D7: entries naming nothing drop out.
    #[test]
    fn complete_order_drops_entries_naming_nothing() {
        let (dir, mut p) = ordered_fixture();
        std::fs::remove_file(dir.path().join("requests/main/b.toml")).unwrap();
        p.relist();
        assert!(p.complete_order().unwrap());
        assert_eq!(main_order(&p), ["a", "c"]);
    }

    /// D5a: the CLI never writes it.
    #[test]
    fn complete_order_on_a_read_only_project_writes_nothing() {
        let (dir, _p) = unordered_fixture();
        let before = read(&dir, "project.toml");
        let (mut p, _w) = Project::open_read_only(dir.path().to_path_buf()).unwrap();
        assert!(!p.complete_order().unwrap());
        assert_eq!(read(&dir, "project.toml"), before);
        assert!(main_order(&p).is_empty());
    }

    /// A directory with no `project.toml` never gains one behind the
    /// user's back (as `ensure_spaces` keeps it).
    #[test]
    fn complete_order_never_makes_a_project_toml() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "requests/main/a.toml", "method = \"GET\"\nurl = \"u\"\n");
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        assert!(!p.complete_order().unwrap());
        assert!(!dir.path().join("project.toml").exists());
    }

    /// D4: in a completed list a create (a curl import is one) lands
    /// last, and so does a rename into a new folder; a same-level rename
    /// keeps its place.
    #[test]
    fn after_completion_arrivals_land_last_and_a_same_level_rename_keeps_its_place() {
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        p.create_request("main/Aaa", req("u")).unwrap();
        assert_eq!(main_order(&p), ["a", "b", "c", "aaa"]);
        p.rename_request("main/b", "main/Bee").unwrap();
        assert_eq!(main_order(&p), ["a", "bee", "c", "aaa"]);
        p.rename_request("main/a", "main/f/A").unwrap();
        assert_eq!(main_order(&p), ["bee", "c", "aaa", "f/", "f/a"]);
    }

    /// D17: a request moves into a folder at the slot it was dropped on,
    /// its file and text untouched, as one undo step.
    #[test]
    fn a_request_moves_into_a_folder_at_its_slot() {
        let (dir, mut p) = tree_fixture();
        let text = read(&dir, "requests/main/health.toml");
        // `users`' items before the move: login, v1/; slot 1 is between.
        let new = p.move_to_level(&req_item("main/health"), "users", 1).unwrap();
        assert_eq!(new, "main/users/health");
        assert!(!dir.path().join("requests/main/health.toml").exists());
        assert_eq!(read(&dir, "requests/main/users/health.toml"), text, "the text is untouched");
        assert_eq!(
            main_order(&p),
            [
                "users/",
                "users/login",
                "users/health",
                "users/v1/",
                "users/v1/list",
                "users/v1/get",
                "orders/",
                "orders/new"
            ]
        );
        assert_eq!(p.journal_len(), 1);
    }

    #[test]
    fn a_request_moves_to_the_top_level_first_or_last() {
        let (_dir, mut p) = tree_fixture();
        let new = p.move_to_level(&req_item("main/users/v1/get"), "", 0).unwrap();
        assert_eq!(new, "main/get");
        // The top level now: get, users/, health, orders/; 4 is the end.
        let new = p.move_to_level(&req_item("main/users/login"), "", 4).unwrap();
        assert_eq!(new, "main/login");
        assert_eq!(
            main_order(&p),
            [
                "get",
                "users/",
                "users/v1/",
                "users/v1/list",
                "health",
                "orders/",
                "orders/new",
                "login"
            ]
        );
    }

    /// D15: the slot counts a folder the move empties, as the sidebar drew
    /// it, and the emptied folder's entry goes (2026-11-01 R9).
    #[test]
    fn a_request_leaving_its_folder_empty_prunes_it_and_keeps_its_slot() {
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/zeta.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        // The top level before: users/, health, orders/, zeta; slot 3 is
        // below orders/, above zeta.
        let new = p.move_to_level(&req_item("main/orders/new"), "", 3).unwrap();
        assert_eq!(new, "main/new");
        assert_eq!(
            main_order(&p),
            [
                "users/",
                "users/login",
                "users/v1/",
                "users/v1/list",
                "users/v1/get",
                "health",
                "new",
                "zeta"
            ]
        );
    }

    /// D17: the file keeps its name, numbered when the folder has it.
    #[test]
    fn a_moved_request_keeps_its_file_name_numbered_when_taken() {
        let (dir, mut p) = tree_fixture();
        write(
            &dir,
            "requests/main/users/get.toml",
            "name = \"Fetch\"\nmethod = \"GET\"\nurl = \"u\"\n",
        );
        p.relist();
        p.complete_order().unwrap();
        let text = read(&dir, "requests/main/users/v1/get.toml");
        let new = p.move_to_level(&req_item("main/users/v1/get"), "users", 0).unwrap();
        assert_eq!(new, "main/users/get-2");
        assert_eq!(read(&dir, "requests/main/users/get-2.toml"), text);
        assert_eq!(main_order(&p)[..3], ["users/", "users/get-2", "users/login"]);
    }

    /// D18: the folder already shows a request of that name (case aside):
    /// refused, nothing written.
    #[test]
    fn a_request_move_onto_a_shown_name_is_refused_with_nothing_written() {
        let (dir, mut p) = tree_fixture();
        write(
            &dir,
            "requests/main/users/hc.toml",
            "name = \"HEALTH\"\nmethod = \"GET\"\nurl = \"u\"\n",
        );
        p.relist();
        p.complete_order().unwrap();
        let before = read(&dir, "project.toml");
        let r = p.move_to_level(&req_item("main/health"), "users", 0);
        assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "Health"), "{r:?}");
        assert!(dir.path().join("requests/main/health.toml").is_file());
        assert!(!dir.path().join("requests/main/users/health.toml").exists());
        assert_eq!(read(&dir, "project.toml"), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// D18: a nameless request shows its file name after any `-2`.
    #[test]
    fn a_nameless_request_clashes_by_its_numbered_file_name() {
        let (dir, mut p) = tree_fixture();
        write(
            &dir,
            "requests/main/users/get.toml",
            "name = \"Fetch\"\nmethod = \"GET\"\nurl = \"u\"\n",
        );
        write(
            &dir,
            "requests/main/users/other.toml",
            "name = \"Get-2\"\nmethod = \"GET\"\nurl = \"u\"\n",
        );
        p.relist();
        p.complete_order().unwrap();
        let before = read(&dir, "project.toml");
        let r = p.move_to_level(&req_item("main/users/v1/get"), "users", 0);
        assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "get-2"), "{r:?}");
        assert!(dir.path().join("requests/main/users/v1/get.toml").is_file());
        assert_eq!(read(&dir, "project.toml"), before);
    }

    /// D18: a display name holding `/` is only a name.
    #[test]
    fn a_name_holding_a_slash_moves_as_a_name() {
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/slashy.toml", "name = \"a/b\"\nmethod = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        let new = p.move_to_level(&req_item("main/slashy"), "users", 0).unwrap();
        assert_eq!(new, "main/users/slashy");
        assert!(dir.path().join("requests/main/users/slashy.toml").is_file());
        assert!(!dir.path().join("requests/main/users/a").exists());
        let row = p.requests().iter().find(|l| l.slug == "main/users/slashy").unwrap();
        assert_eq!(row.display_name(), "a/b");
    }

    /// The app sends a same-level drop as `set_request_order` (D20), and
    /// a drop must name a level that exists.
    #[test]
    fn a_move_to_its_own_level_or_a_missing_one_is_refused() {
        let (dir, mut p) = tree_fixture();
        let before = read(&dir, "project.toml");
        let r = p.move_to_level(&req_item("main/health"), "", 0);
        assert!(matches!(r, Err(Error::BadName(_))), "{r:?}");
        let r = p.move_to_level(&folder_item("main/users/v1"), "users", 0);
        assert!(matches!(r, Err(Error::BadName(_))), "{r:?}");
        let r = p.move_to_level(&req_item("main/health"), "nope", 0);
        assert!(matches!(r, Err(Error::NotFound(_))), "{r:?}");
        let r = p.move_to_level(&req_item("main/gone"), "users", 0);
        assert!(matches!(r, Err(Error::NotFound(_))), "{r:?}");
        let r = p.move_to_level(&folder_item("main/gone"), "users", 0);
        assert!(matches!(r, Err(Error::NotFound(_))), "{r:?}");
        assert_eq!(read(&dir, "project.toml"), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// D17: a folder's directory moves whole into another folder; its
    /// entries are renamed in place and its own lands at the slot.
    #[test]
    fn a_folder_moves_into_another_folder_with_its_contents() {
        let (dir, mut p) = tree_fixture();
        let text = read(&dir, "requests/main/orders/new.toml");
        let new = p.move_to_level(&folder_item("main/orders"), "users", 0).unwrap();
        assert_eq!(new, "main/users/orders");
        assert_eq!(read(&dir, "requests/main/users/orders/new.toml"), text);
        assert!(!dir.path().join("requests/main/orders").exists());
        assert_eq!(
            main_order(&p),
            [
                "users/",
                "users/orders/",
                "users/login",
                "users/v1/",
                "users/v1/list",
                "users/v1/get",
                "health",
                "users/orders/new"
            ]
        );
        assert_eq!(p.journal_len(), 1);
    }

    /// D17, D19: a nested folder moves to the top level whole, its
    /// contents keep their order, and the held request follows.
    #[test]
    fn a_folder_moves_to_the_top_level_with_its_order_and_held_requests() {
        let (dir, mut p) = tree_fixture();
        p.open_request("main/users/v1/get").unwrap();
        // The top level before: users/, health, orders/; slot 2 is above
        // orders/.
        let new = p.move_to_level(&folder_item("main/users/v1"), "", 2).unwrap();
        assert_eq!(new, "main/v1");
        assert!(dir.path().join("requests/main/v1/get.toml").is_file());
        assert!(!dir.path().join("requests/main/users/v1").exists());
        assert_eq!(
            main_order(&p),
            [
                "users/",
                "users/login",
                "v1/list",
                "v1/get",
                "health",
                "v1/",
                "orders/",
                "orders/new"
            ]
        );
        assert_eq!(
            order::displayed_slugs(p.requests(), &main_order(&p), "main"),
            ["main/users/login", "main/health", "main/v1/list", "main/v1/get", "main/orders/new"]
        );
        assert!(p.held_request("main/users/v1/get").is_none());
        assert!(p.held_request("main/v1/get").is_some());
        assert_eq!(p.held_request_drift("main/v1/get"), None);
        let e = p.journal.pop_undo().unwrap();
        let mut moves = e.meta.moves.clone();
        moves.sort();
        assert_eq!(
            moves,
            [
                ("main/users/v1/get".to_string(), "main/v1/get".to_string()),
                ("main/users/v1/list".to_string(), "main/v1/list".to_string()),
            ]
        );
    }

    /// R9: a folder whose move empties its parent takes the parent's entry
    /// with it.
    #[test]
    fn a_folder_move_that_empties_its_parent_prunes_it() {
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/deep/inner/x.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        assert_eq!(main_order(&p)[8..], ["deep/", "deep/inner/", "deep/inner/x"]);
        p.move_to_level(&folder_item("main/deep/inner"), "", 0).unwrap();
        assert_eq!(main_order(&p)[..1], ["inner/"]);
        assert_eq!(main_order(&p)[9..], ["inner/x"]);
        assert!(!main_order(&p).contains(&"deep/".to_string()));
    }

    /// D18: the destination already has a folder of that name.
    #[test]
    fn a_folder_move_onto_a_folder_of_its_name_is_refused() {
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/v1/x.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        let before = read(&dir, "project.toml");
        let r = p.move_to_level(&folder_item("main/users/v1"), "", 0);
        assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "v1"), "{r:?}");
        assert!(dir.path().join("requests/main/users/v1/get.toml").is_file());
        assert_eq!(read(&dir, "project.toml"), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// D2: a folder can't go inside itself or one of its own folders.
    #[test]
    fn a_folder_cannot_move_into_itself_or_its_own_folder() {
        let (dir, mut p) = tree_fixture();
        let before = read(&dir, "project.toml");
        let r = p.move_to_level(&folder_item("main/users"), "users", 0);
        assert!(matches!(r, Err(Error::BadName(_))), "{r:?}");
        let r = p.move_to_level(&folder_item("main/users"), "users/v1", 0);
        assert!(matches!(r, Err(Error::BadName(_))), "{r:?}");
        assert!(dir.path().join("requests/main/users/v1/get.toml").is_file());
        assert_eq!(read(&dir, "project.toml"), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// Undo puts the files and `project.toml` back byte for byte; redo
    /// does the move again.
    #[test]
    fn undo_and_redo_of_request_and_folder_moves_restore_exactly() {
        let (dir, mut p) = tree_fixture();
        let start = read(&dir, "project.toml");
        let health = read(&dir, "requests/main/health.toml");
        p.move_to_level(&req_item("main/health"), "users", 1).unwrap();
        let moved = read(&dir, "project.toml");
        p.undo().unwrap().unwrap();
        assert_eq!(read(&dir, "project.toml"), start);
        assert_eq!(read(&dir, "requests/main/health.toml"), health);
        assert!(!dir.path().join("requests/main/users/health.toml").exists());
        p.redo().unwrap().unwrap();
        assert_eq!(read(&dir, "project.toml"), moved);
        assert_eq!(read(&dir, "requests/main/users/health.toml"), health);

        p.open_request("main/users/v1/get").unwrap();
        p.move_to_level(&folder_item("main/users/v1"), "", 0).unwrap();
        let folder_moved = read(&dir, "project.toml");
        p.undo().unwrap().unwrap();
        assert_eq!(read(&dir, "project.toml"), moved);
        assert!(dir.path().join("requests/main/users/v1/get.toml").is_file());
        assert!(!dir.path().join("requests/main/v1").exists());
        assert!(p.held_request("main/users/v1/get").is_some(), "the held request follows back");
        p.redo().unwrap().unwrap();
        assert_eq!(read(&dir, "project.toml"), folder_moved);
        assert!(dir.path().join("requests/main/v1/get.toml").is_file());
        assert!(p.held_request("main/v1/get").is_some());
    }
}
