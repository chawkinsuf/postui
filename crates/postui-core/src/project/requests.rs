//! Requests: the listing (always in memory), requests loaded on demand and
//! held while open, and the file operations with their order-list
//! cascades. Request ops journal paths and trash tickets, never content.

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
                loose.push(format!("requests/{rel} is not in a space (move it into a space directory)"));
                continue;
            }
            Some(space) if crate::storage::validate_slug(space).is_err() => {
                loose.push(format!("requests/{rel} is not in a valid space (space names are a-z 0-9 - _)"));
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
    /// taken when it was opened or last saved.
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

    /// Parses the request and holds it until `close_request`.
    pub fn open_request(&mut self, slug: &str) -> Result<&HttpRequest, Error> {
        let path = request_rel(slug)?;
        let text = self
            .disk
            .read(&path)?
            .ok_or_else(|| Error::NotFound(slug.to_string()))?;
        let req = HttpRequest::from_toml_str(&text).map_err(|e| Error::Parse {
            file: path.to_string(),
            error: e.to_string(),
        })?;
        let stamp = self.disk.stamp(&path);
        self.open_requests.insert(slug.to_string(), Held { req, stamp });
        Ok(&self.open_requests[slug].req)
    }

    pub fn close_request(&mut self, slug: &str) {
        self.open_requests.shift_remove(slug);
    }

    /// Writes the request, refreshes the held copy and its listing row.
    /// Not journaled: undoing past a save is the editor's memory-only
    /// step, as today.
    pub fn save_request(&mut self, slug: &str, req: &HttpRequest) -> Result<(), Error> {
        let path = request_rel(slug)?;
        self.disk.write(&path, &req.to_toml_string())?;
        if self.open_requests.contains_key(slug) {
            let stamp = self.disk.stamp(&path);
            self.open_requests.insert(
                slug.to_string(),
                Held {
                    req: req.clone(),
                    stamp,
                },
            );
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
    pub fn sibling_name_taken(&self, folder: &str, leaf_display: &str, exclude: Option<&str>) -> bool {
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

    pub(super) fn order_rename(&mut self, space: &str, from: &str, to: &str) -> Result<Vec<OrderEdit>, Error> {
        if order::level_of(from) == order::level_of(to) {
            return self.order_edit(space, |order| {
                for e in order.iter_mut() {
                    if e == from {
                        *e = to.to_string();
                    }
                }
                vec![OrderEdit::Renamed { space: space.to_string(), from: from.to_string(), to: to.to_string() }]
            });
        }
        let mut edits = self.order_remove(space, from)?;
        edits.extend(self.order_arrive(space, to)?);
        Ok(edits)
    }

    pub(super) fn order_insert_after(&mut self, space: &str, anchor: &str, rel: &str) -> Result<Vec<OrderEdit>, Error> {
        self.order_edit(space, |order| {
            if order.iter().any(|e| e == rel) {
                return Vec::new();
            }
            let Some(i) = order.iter().position(|e| e == anchor) else { return Vec::new() };
            order.insert(i + 1, rel.to_string());
            vec![OrderEdit::Inserted { space: space.to_string(), rel: rel.to_string(), at: i + 1 }]
        })
    }

    /// One read-modify-write per space, however many requests moved.
    pub(super) fn order_move_all(&mut self, from: &str, to: &str, moves: &[(String, String)]) -> Result<(), Error> {
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
        EntryMeta {
            moves,
            active_env: None,
            reopen: self.local.open_request.clone(),
        }
    }

    /// Creates a request from a typed display path ("Folder/My Request!").
    pub fn create_request(&mut self, display_path: &str, mut req: HttpRequest) -> Result<(String, String), Error> {
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
    pub fn rename_request(&mut self, from_slug: &str, display_path: &str) -> Result<(String, String), Error> {
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
            if let Some(req) = p.open_requests.shift_remove(&from_slug) {
                p.open_requests.insert(to_slug.clone(), req);
            }
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
        let source = request_rel(slug)?;
        let bytes = self
            .disk
            .read_bytes(&source)?
            .ok_or_else(|| Error::NotFound(slug.to_string()))?;
        let (folder, leaf) = slug.rsplit_once('/').unwrap_or(("", slug));
        let parsed = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| HttpRequest::from_toml_str(s).ok());
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
                let mut new_slug = format!("{slug}-copy");
                let mut n = 2;
                while self.request_exists(&new_slug) {
                    new_slug = format!("{slug}-copy-{n}");
                    n += 1;
                }
                (new_slug, bytes)
            }
        };
        let meta = self.meta_for_moves(Vec::new());
        let slug = slug.to_string();
        self.transaction("duplicate request", meta, |p| {
            let path = request_rel(&new_slug)?;
            p.disk.write_new(&path, "")?;
            p.record(Op::Created { path: path.clone() });
            p.disk.write_bytes(&path, &text)?;
            p.relist();
            if let (Some((space, anchor)), Some((_, rel))) =
                (Self::split_space(&slug), Self::split_space(&new_slug))
            {
                p.order_insert_after(space, anchor, rel)?;
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
        assert!(matches!(p.create_request("main/my ping!", req("u")), Err(Error::AlreadyExists(_))));
        let (slug2, _) = p.create_request("main/My-Ping", req("u")).unwrap();
        assert_eq!(slug2, "main/my-ping-2");
        assert_eq!(p.journal_len(), 2);
        let e = p.journal.pop_undo().unwrap();
        assert!(matches!(e.ops[0], Op::Created { .. }));
        assert_eq!(e.meta.reopen, None);
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
        std::fs::write(dir.path().join("requests/auth/r3.toml"), "method = \"GET\"\nurl = \"u\"\n").unwrap();
        p.relist();
        let (moved, _) = p.move_all_requests("main", "auth").unwrap();
        assert_eq!(moved.len(), 6);
        assert!(moved.iter().any(|(f, t)| f == "main/r3" && t == "auth/r3-2"));
        assert!(!dir.path().join("requests/main/r0.toml").exists());
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.meta.moves.len(), 6);
        assert!(e.ops.iter().all(|o| matches!(o, Op::Renamed { .. } | Op::Text { .. })));
        assert!(
            !e.ops.iter().any(|o| matches!(o, Op::Text { path, .. } if path.as_str().starts_with("requests/"))),
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
        std::fs::write(dir.path().join("requests/main/Get User.toml"), "method = \"GET\"\nurl = \"u\"\n").unwrap();
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
        std::fs::write(dir.path().join("requests/main/r1.toml"), "method = \"GET\"\nurl = \"u\"\n").unwrap();
        std::fs::write(dir.path().join("requests/main/r2.toml"), "method = \"GET\"\nurl = \"u\"\n").unwrap();
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
        assert!(matches!(&e.ops[0], Op::Created { path } if path.as_str() == format!("requests/{copy}.toml")));
    }

    fn bump_mtime(path: &std::path::Path) {
        // Coarse-mtime filesystems need the clock to move.
        let t = std::fs::metadata(path).unwrap().modified().unwrap() + std::time::Duration::from_secs(2);
        std::fs::File::open(path).unwrap().set_modified(t).unwrap();
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
        bump_mtime(&dir.path().join("requests/main/ping.toml"));
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
        bump_mtime(&dir.path().join("requests/main/ping.toml"));
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
    fn held_request_drift_is_none_for_the_new_slug_after_a_rename_reopens_it() {
        let (_dir, mut p) = fixture();
        p.open_request("main/ping").unwrap();
        let (to_slug, _) = p.rename_request("main/ping", "main/renamed").unwrap();
        p.open_request(&to_slug).unwrap();
        assert_eq!(p.held_request_drift(&to_slug), None);
    }
}
