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

/// How a held request's file has drifted from the stamp taken when the
/// editor's buffer was last seeded from it. See [`Project::held_request_drift`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldDrift {
    Changed,
    Vanished,
}

/// Walks `requests/` through `Disk`: every `.toml` in a valid space,
/// parsed for its method and name; broken files listed with the error.
/// The warning names the first unreadable directory and every loose file.
pub(crate) fn list(disk: &mut Disk) -> (Vec<RequestListing>, Option<String>) {
    let Ok(base) = RelPath::new(REQUESTS_DIR) else {
        return (Vec::new(), None);
    };
    let (files, walk_warning) = disk.walk_files(&base, "toml");
    let mut out = Vec::new();
    let mut loose = Vec::new();
    for path in files {
        let rel = path.as_str().strip_prefix("requests/").unwrap_or(path.as_str());
        let slug = rel.strip_suffix(".toml").unwrap_or(rel).to_string();
        match crate::storage::space_of(&slug) {
            None => {
                loose.push(format!(
                    "requests/{rel} is not in a space (move it into a space directory)"
                ));
                continue;
            }
            Some(space) if crate::storage::validate_slug(space).is_err() => {
                loose.push(format!(
                    "requests/{rel} is not in a valid space (space names are a-z 0-9 - _)"
                ));
                continue;
            }
            Some(_) => {}
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
    let mut warnings: Vec<String> = walk_warning.into_iter().collect();
    warnings.extend(loose);
    let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
    (out, warning)
}

impl Project {
    pub fn requests(&self) -> &[RequestListing] {
        &self.listing
    }

    pub fn listing_warning(&self) -> Option<&str> {
        self.listing_warning.as_deref()
    }

    /// Re-walks `requests/`. Called by every request op and by the app's
    /// sidebar refresh, as `list_requests` was.
    pub fn relist(&mut self) {
        let (listing, warning) = list(&mut self.disk);
        self.listing = listing;
        self.listing_warning = warning;
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
            let (dir, leaf) = l.slug.rsplit_once('/').unwrap_or(("", l.slug.as_str()));
            dir == folder && l.name.as_deref().unwrap_or(leaf).to_lowercase() == wanted
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

    pub(super) fn order_arrive(&mut self, space: &str, rel: &str) -> Result<Vec<OrderEdit>, Error> {
        let exists: Vec<String> = self.listing.iter().map(|l| l.slug.clone()).collect();
        self.order_edit(space, |order| {
            order::arrive(space, order, rel, &|s| exists.iter().any(|e| e == s))
        })
    }

    pub(super) fn order_remove(&mut self, space: &str, rel: &str) -> Result<Vec<OrderEdit>, Error> {
        self.order_edit(space, |order| order::remove(space, order, rel))
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
        edits.extend(self.order_arrive(space, to)?);
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

    /// One level of `space`'s requests as relative slugs, in the order the
    /// sidebar draws them right now: the listed ones first, then the rest
    /// alphabetically ([`order::order_level`]'s display rule). It seeds an
    /// order array that does not exist yet, so a put and a first drag agree
    /// on what "the order" was before the edit.
    pub(super) fn level_shown(&self, space: &str, level: &str) -> Vec<String> {
        order::displayed_slugs(&self.listing, order::space_order(&self.meta, space), space)
            .iter()
            .filter_map(|slug| order::relative(slug, space))
            .filter(|rel| order::level_of(rel) == level)
            .map(str::to_string)
            .collect()
    }

    /// One read-modify-write per space, however many requests moved.
    pub(super) fn order_move_all(
        &mut self,
        from: &str,
        to: &str,
        moves: &[(String, String)],
    ) -> Result<(), Error> {
        self.order_edit(from, |order| {
            moves.iter().flat_map(|(f, _)| order::remove(from, order, f)).collect()
        })?;
        let exists: Vec<String> = self.listing.iter().map(|l| l.slug.clone()).collect();
        self.order_edit(to, |order| {
            moves
                .iter()
                .flat_map(|(_, t)| order::arrive(to, order, t, &|s| exists.iter().any(|e| e == s)))
                .collect()
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
                p.order_arrive(space, rel)?;
            }
            Ok(())
        })?;
        Ok((slug, leaf))
    }

    /// Renames to a new typed display path; rewrites `name` when the file
    /// parses (a broken file just moves).
    pub fn rename_request(
        &mut self,
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
        let to_slug = self.unique_slug(&folder, &leaf, Some(from_slug));
        let to_path = request_rel(&to_slug)?;
        // A rename stays in its space (the order bookkeeping below is
        // keyed by one space); crossing spaces is `move_request`'s job.
        if crate::storage::space_of(from_slug) != crate::storage::space_of(&to_slug) {
            return Err(Error::BadName(format!(
                "{display_path}: a rename stays in its space — move the request instead"
            )));
        }
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
        let base = format!("{space}/{rest}");
        let mut candidate = base.clone();
        let mut n = 2;
        while self.request_exists(&candidate) || reserved.contains(&candidate) {
            candidate = format!("{base}-{n}");
            n += 1;
        }
        Ok(candidate)
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
            p.order_arrive(to_space, to_rel)?;
            Ok(())
        })?;
        Ok(new_slug)
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
        self.duplicate_with(slug, slug, false, false)
    }

    /// The put entry point (piece 4 spec §5.2: `p`/`P`): a duplicate whose
    /// copy's place is chosen by the caller. `anchor` is a full slug, and
    /// may be the source itself (`yyP` on the yanked row). When it is not
    /// the source and lies in the source's space, the copy is made in the
    /// anchor's folder (same `<name> copy` collision rule, applied there).
    /// Its order entry lands directly below the anchor, or above it when
    /// `above`. Otherwise the copy is made beside its source. An anchor
    /// outside the copy's space leaves the copy unlisted. A put always
    /// seeds a missing order array for the anchor's level; only
    /// `duplicate_request` never does.
    pub fn duplicate_request_at(
        &mut self,
        slug: &str,
        anchor: &str,
        above: bool,
    ) -> Result<String, Error> {
        self.duplicate_with(slug, anchor, above, true)
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

    /// The shared body. `seed` says whether a missing order array is
    /// written (a put) or left alone (a plain duplicate).
    fn duplicate_with(
        &mut self,
        slug: &str,
        anchor: &str,
        above: bool,
        seed: bool,
    ) -> Result<String, Error> {
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
        // A put into a space that was never reordered has no order array,
        // so the anchor names no position and the insert below would do
        // nothing, leaving the copy to the alphabetical fallback rather
        // than beside the cursor. Seed the anchor's level from the order
        // the sidebar draws now, through the merge a first drag uses.
        // Computed before the copy exists (the pre-put order) and written
        // inside the transaction, so the undo takes it away. A plain
        // duplicate (`seed == false`) never seeds.
        let seed = seed
            .then_some(anchor)
            .and_then(Self::split_space)
            .filter(|(a_space, a_rel)| {
                !order::space_order(&self.meta, a_space).iter().any(|e| e == a_rel)
            })
            .map(|(a_space, a_rel)| {
                let level = order::level_of(a_rel);
                let shown = self.level_shown(a_space, level);
                let before = order::space_order(&self.meta, a_space).to_vec();
                (a_space.to_string(), order::merge_level(&before, level, &shown))
            });
        let meta = self.meta_for_moves(Vec::new());
        let anchor = anchor.to_string();
        self.transaction("duplicate request", meta, |p| {
            if let Some((space, after)) = seed {
                p.order_edit(&space, |order| {
                    *order = after;
                    Vec::new()
                })?;
            }
            let path = request_rel(&new_slug)?;
            p.disk.write_new(&path, "")?;
            p.record(Op::Created { path: path.clone() });
            p.disk.write_bytes(&path, &text)?;
            p.relist();
            if let (Some((a_space, a_rel)), Some((space, rel))) =
                (Self::split_space(&anchor), Self::split_space(&new_slug))
                && a_space == space
            {
                if above {
                    p.order_insert_before(space, a_rel, rel)?;
                } else {
                    p.order_insert_after(space, a_rel, rel)?;
                }
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

    #[test]
    fn move_all_arrives_in_the_order_the_sidebar_showed_not_alphabetically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main/sub")).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/auth")).unwrap();
        // `auth` is listed too: `order::arrive` deliberately appends
        // nothing to a level that has no displayed entry (such a level
        // sorts alphabetically), so an unlisted destination would record
        // no order whatever the arrival order was.
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
        assert_eq!(order::space_order(p.meta(), "auth"), ["login", "b", "a"]);
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
        assert!(dir.path().join(".local/trash/1/requests/main/ping.toml").is_file());
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

    /// A put in a project with no order array still lands where the
    /// cursor is: the level is seeded from the display order first.
    #[test]
    fn a_put_with_no_order_array_seeds_one_from_the_display_order() {
        let (_dir, mut p) = unordered_fixture();
        assert!(order::space_order(p.meta(), "main").is_empty());
        let copy = p.duplicate_request_at("main/a", "main/c", false).unwrap();
        assert_eq!(copy, "main/a-copy");
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "b", "c", "a-copy"]);
    }

    #[test]
    fn a_put_above_with_no_order_array_seeds_one_too() {
        let (_dir, mut p) = unordered_fixture();
        p.duplicate_request_at("main/a", "main/c", true).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a", "b", "a-copy", "c"]);
    }

    /// The seed is part of the put's own transaction: the undo takes the
    /// array away again with the copy (undo restores exactly).
    #[test]
    fn undoing_that_put_removes_the_order_array_it_created() {
        let (dir, mut p) = unordered_fixture();
        let before = read(&dir, "project.toml").unwrap();
        p.duplicate_request_at("main/a", "main/c", false).unwrap();
        p.undo().unwrap();
        assert!(order::space_order(p.meta(), "main").is_empty());
        assert_eq!(read(&dir, "project.toml").as_deref(), Some(before.as_str()));
        assert!(!p.request_exists("main/a-copy"));
    }

    /// `yyP` on the yanked row itself: anchor == source is still a put, so
    /// it seeds and the copy lands above (and below for `p`).
    #[test]
    fn a_put_on_its_own_source_seeds_and_lands_above_or_below() {
        let (_dir, mut p) = unordered_fixture();
        p.duplicate_request_at("main/a", "main/a", true).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["a-copy", "a", "b", "c"]);
        let (_dir, mut p) = unordered_fixture();
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

    /// A plain Duplicate behaves as it always did: it never writes an
    /// order array into a space that was never reordered.
    #[test]
    fn a_plain_duplicate_never_seeds_an_order_array() {
        let (_dir, mut p) = unordered_fixture();
        p.duplicate_request("main/a").unwrap();
        assert!(order::space_order(p.meta(), "main").is_empty());
    }

    /// A put's copy is made in the anchor's folder, directly after the
    /// anchor in that level, and the undo restores `project.toml` exactly.
    #[test]
    fn a_put_into_another_folder_creates_the_copy_there() {
        let (dir, mut p) = folder_fixture();
        let before = read(&dir, "project.toml").unwrap();
        let copy = p.duplicate_request_at("main/a", "main/f/x", false).unwrap();
        assert_eq!(copy, "main/f/a-copy");
        assert!(dir.path().join("requests/main/f/a-copy.toml").is_file());
        assert!(!dir.path().join("requests/main/a-copy.toml").exists());
        assert_eq!(order::space_order(p.meta(), "main"), ["f/x", "f/a-copy"]);
        p.undo().unwrap();
        assert!(!dir.path().join("requests/main/f/a-copy.toml").exists());
        assert_eq!(read(&dir, "project.toml").as_deref(), Some(before.as_str()));
    }

    #[test]
    fn a_put_above_in_another_folder_takes_the_anchors_slot() {
        let (_dir, mut p) = folder_fixture();
        let copy = p.duplicate_request_at("main/a", "main/f/x", true).unwrap();
        assert_eq!(copy, "main/f/a-copy");
        assert_eq!(order::space_order(p.meta(), "main"), ["f/a-copy", "f/x"]);
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
}
