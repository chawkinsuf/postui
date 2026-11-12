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
use crate::order;

/// `(from_slug, to_slug)` pairs of a batch move.
pub type Moves = Vec<(String, String)>;

/// The folders along a typed path, outside in: each one's full path and
/// the text typed for it (see [`Project::resolve_display_path`]).
pub type Along = Vec<(String, String)>;

/// One sidebar item [`Project::move_to_level`] moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeItem {
    /// A request, by its full slug (`"main/users/login"`).
    Request(String),
    /// A folder, by its full path (`"main/users/v1"`, no trailing slash).
    Folder(String),
}

/// The directory of the folder at full path `path` (`"main/users"`),
/// refused as an invalid name when it isn't a valid slug.
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

/// Whether `rel` (relative to `requests/`) passes through, or is, a file
/// or directory whose name starts with `.` inside its space (spec
/// 2026-11-03 create folders F16). The space segment itself is left to
/// the space rules.
///
/// Skipped silently, like a dot file anywhere else: the project format
/// already documents that a file or folder starting with `.` is ignored
/// (the bundled skill's reference), so a `.wip/` of drafts is the user's
/// to keep out of the tree. The cost: a request kept there before this
/// rule no longer shows, and nothing says so. A warning per dot entry
/// would nag every project that keeps a `.DS_Store` in a folder.
fn hidden_in_space(rel: &str) -> bool {
    rel.split('/').skip(1).any(super::is_hidden)
}

/// The name a [`crate::storage::FOLDER_FILE`]'s `text` gives its folder:
/// `Ok(None)` for none (or an empty one), `Err` for a file that is no
/// usable folder file. The one reading of the file: the listing's, and a
/// copy's test of whether it may carry a file over (spec 2026-11-03 create
/// folders F24).
fn folder_file_name(text: &str) -> Result<Option<String>, String> {
    #[derive(serde::Deserialize)]
    struct FolderMeta {
        name: Option<String>,
    }
    let meta = toml::from_str::<FolderMeta>(text).map_err(|e| e.to_string())?;
    // Trimmed, as a prompt trims it; a name holding `/` (written by hand:
    // postui refuses one) counts as none, as an empty one does, so the
    // folder shows its directory and its prefilled path finds it again.
    Ok(meta.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && !n.contains('/')))
}

/// A folder name as typed into a prompt, trimmed: refused when empty, or
/// when it holds `/`, which would make a prefilled path read as more than
/// one folder (review block 1, mine (approved): "No").
fn folder_name(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::BadName("a folder needs a name".to_string()));
    }
    if name.contains('/') {
        return Err(Error::BadName(format!("{name:?}: a folder name can't hold /")));
    }
    Ok(name)
}

/// Every directory below a valid space in `dirs` (the walk's, relative to
/// the root) as a folder, its [`crate::storage::FOLDER_FILE`] read for a
/// name. A file that doesn't parse leaves the folder nameless, `broken`,
/// with a warning naming the file (F24); one that can't be read leaves it
/// nameless only (see below).
fn list_folders(
    disk: &mut Disk,
    dirs: Vec<RelPath>,
) -> (Vec<crate::storage::FolderListing>, Vec<ListingWarning>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        let rel = dir.as_str().strip_prefix("requests/").unwrap_or(dir.as_str());
        let Some(space) = crate::storage::space_of(rel) else { continue };
        if crate::storage::validate_slug(space).is_err() || hidden_in_space(rel) {
            continue;
        }
        let Ok(file) = dir.join(crate::storage::FOLDER_FILE) else { continue };
        // A file that can't be read leaves the folder nameless but not
        // broken: what can't be read (most often a directory the walk
        // couldn't list, already warned about) is no file that doesn't
        // parse. A rename of it reports the read error from the op itself;
        // it never overwrites a file it couldn't read (parse errors never
        // default).
        let read = match disk.read(&file) {
            Ok(Some(text)) => folder_file_name(&text),
            Ok(None) | Err(_) => Ok(None),
        };
        let (name, broken) = match read {
            Ok(name) => (name, None),
            Err(e) => {
                warnings.push(ListingWarning {
                    message: format!("{file}: {e}"),
                    file: Some(file.as_str().to_string()),
                });
                (None, Some(e))
            }
        };
        out.push(crate::storage::FolderListing { path: rel.to_string(), name, broken });
    }
    (out, warnings)
}

/// Walks `requests/` through `Disk`: every `.toml` in a valid space,
/// parsed for its method and name; broken files listed with the error;
/// and every folder (spec 2026-11-03 create folders F1). Files and
/// directories whose names start with `.` inside a space are skipped
/// (F16). The warnings name the first unreadable directory, every loose
/// file and every unreadable folder file.
pub(crate) fn list(
    disk: &mut Disk,
) -> (Vec<RequestListing>, Vec<crate::storage::FolderListing>, Vec<ListingWarning>) {
    let Ok(base) = RelPath::new(REQUESTS_DIR) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let (files, dirs, walk_warning) = disk.walk(&base, "toml");
    let mut out = Vec::new();
    let mut loose = Vec::new();
    for path in files {
        let rel = path.as_str().strip_prefix("requests/").unwrap_or(path.as_str());
        if hidden_in_space(rel) {
            continue;
        }
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
    let (folders, folder_warnings) = list_folders(disk, dirs);
    let mut warnings: Vec<ListingWarning> =
        walk_warning.into_iter().map(|message| ListingWarning { file: None, message }).collect();
    warnings.extend(loose);
    warnings.extend(folder_warnings);
    (out, folders, warnings)
}

impl Project {
    pub fn requests(&self) -> &[RequestListing] {
        &self.listing
    }

    /// Every folder of every space, by path (spec 2026-11-03 create
    /// folders F1).
    pub fn folders(&self) -> &[crate::storage::FolderListing] {
        &self.folder_listing
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
        let (listing, folders, warnings) = list(&mut self.disk);
        self.listing = listing;
        self.folder_listing = folders;
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

    /// `display_path` ("main/User Accounts/My Request!") split for a create
    /// or a rename: the full path of the folder it lands in
    /// (`"main/user-accounts"`), the folders on that path below the space
    /// with the text typed for each, outside in, and the leaf. Refused for
    /// an empty leaf, and for a segment more than one sibling folder shows
    /// (folders made outside postui, which never makes two, F19): the name
    /// can't say which one, and filing into either could be the wrong one.
    ///
    /// A folder segment is a name, as a request's leaf is (spec 2026-11-03
    /// create folders F18a; the user: "There's no reason to search the name
    /// other than the display name", "It should work just like the
    /// requests do"). It goes into the sibling folder that shows it,
    /// ignoring case, and is never read as a directory. Any other segment
    /// is a folder still to be made, named by the typed text, its
    /// directory given by the one naming rule ([`Self::folder_slot`]).
    /// The TUI shows and prefills shown names, so a prefilled path finds
    /// its own folder. The space segment is a directory, as before.
    pub fn resolve_display_path(
        &self,
        display_path: &str,
    ) -> Result<(String, Along, String), Error> {
        let (folders, leaf) = display_path.rsplit_once('/').unwrap_or(("", display_path));
        let leaf = leaf.trim();
        if leaf.is_empty() {
            return Err(Error::BadName(String::new()));
        }
        let mut path = String::new();
        let mut along = Vec::new();
        for seg in folders.split('/').map(str::trim).filter(|s| !s.is_empty()) {
            if path.is_empty() {
                path = crate::storage::segment_slug(seg);
                continue;
            }
            // Below a folder this resolution made, nothing is on disk yet:
            // no sibling shows anything, and no name is taken.
            let mut shown = self.siblings_showing(&path, seg, None);
            path = match (shown.next(), shown.next()) {
                (Some(_), Some(_)) => {
                    return Err(Error::Conflict(format!(
                        "more than one folder here is named {seg:?}: rename one"
                    )));
                }
                (Some(f), None) => f.path.clone(),
                (None, _) => self.folder_slot(&path, seg, None),
            };
            along.push((path.clone(), seg.to_string()));
        }
        Ok((path, along, leaf.to_string()))
    }

    /// `folder` (a full path, `"main/accounts"`) spelled as a typed path
    /// spells it: the space, then each folder's shown name (spec 2026-11-03
    /// create folders F18a). The one owner of that spelling: the curl
    /// import's prefill reads it (F29).
    pub fn shown_folder_path(&self, folder: &str) -> String {
        let mut segs = folder.split('/');
        let mut out = segs.next().unwrap_or_default().to_string();
        let mut path = out.clone();
        for seg in segs {
            path = format!("{path}/{seg}");
            let shown = self
                .folder_listing
                .iter()
                .find(|f| f.path == path)
                .map_or(seg, crate::storage::FolderListing::display_name);
            out = format!("{out}/{shown}");
        }
        out
    }

    /// The one naming rule for a folder's directory, used by a create, a
    /// rename, a typed path and a drop onto a taken directory alike (review
    /// block 1, mine (approved); drag spec D18): in
    /// `level` (a full path), the slug of `name` as one segment
    /// (`slugify`, `folder` when nothing safe survives), `-2`, `-3`, …
    /// while something on disk already has it (F19), as a request's slug
    /// is numbered. `current` is the folder a rename renames: a name whose
    /// slug it already has, numbered or not, keeps its directory, as a
    /// request rename keeps its slug (#209).
    fn folder_slot(&self, level: &str, name: &str, current: Option<&str>) -> String {
        let slug = |name: &str| format!("{level}/{}", crate::storage::slugify_or(name, "folder"));
        let base = slug(name);
        if let Some(current) = current {
            // A numbered directory is kept only when it is this name's own:
            // the folder shows a name with the same slug now (`f-2` showing
            // "f", renamed "F"). A slug that merely ends in digits
            // (`users-2024`) is no number.
            let shown_base = self
                .folder_listing
                .iter()
                .find(|f| f.path == current)
                .is_some_and(|f| slug(f.display_name()) == base);
            let numbered = shown_base
                && current
                    .strip_prefix(&format!("{base}-"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if current == base || numbered {
                return current.to_string();
            }
        }
        let mut path = base.clone();
        let mut n = 2;
        while self.folder_path_taken(&path, current) {
            path = format!("{base}-{n}");
            n += 1;
        }
        path
    }

    /// Whether a folder's directory can't go at full path `path`: something
    /// on disk has it, or a listed folder spelled in any case (a case-only
    /// twin can't sit beside it on every filesystem), `current` (the folder
    /// being renamed) aside.
    fn folder_path_taken(&self, path: &str, current: Option<&str>) -> bool {
        current != Some(path)
            && (rel(&format!("{REQUESTS_DIR}/{path}")).is_ok_and(|r| self.disk.exists(&r))
                || folder_ignoring_case(&self.folder_listing, path).is_some())
    }

    /// Refuses an op in or on a level (a space, or a folder) whose
    /// directory name isn't a valid slug (made in a file manager:
    /// `My Stuff/`), as an op on a request file of such a name is refused:
    /// it shows, and nothing changes it (the user, 2026-11-03: "handle
    /// invalid folder names the same way we handle invalid requests
    /// names").
    fn valid_level(level: &str) -> Result<(), Error> {
        folder_rel(level).map(|_| ())
    }

    /// Makes each folder of `folders` (its full path and the text of its
    /// [`crate::storage::FOLDER_FILE`]), outside in, that isn't on disk:
    /// the directory, then its file, both journaled, so undo takes them
    /// back (spec 2026-11-03 create folders F5, F17). Runs inside its
    /// caller's transaction, before the file op that needs the folder.
    /// Returns the paths it made.
    pub(super) fn make_folders(
        &mut self,
        folders: &[(String, String)],
    ) -> Result<Vec<String>, Error> {
        let mut made = Vec::new();
        for (path, text) in folders {
            if self.disk.is_dir(&rel(&format!("{REQUESTS_DIR}/{path}"))?) {
                continue;
            }
            let dir = folder_rel(path)?;
            self.fs_create_dir(&dir)?;
            self.fs_create_file(&rel(&format!("{dir}/{}", crate::storage::FOLDER_FILE))?, text)?;
            // The listing knows what the op made, as `save_request` keeps
            // its row: a later name check in the same op (the next copy of
            // a Move all) sees this folder's name (F19). The next relist
            // reads it from disk again.
            self.folder_listing.push(crate::storage::FolderListing {
                path: path.clone(),
                name: folder_file_name(text).ok().flatten(),
                broken: None,
            });
            made.push(path.clone());
        }
        Ok(made)
    }

    /// The folders `to` (a request's slug in another space, at the same
    /// sub-path as `from`) sits in, for [`Self::make_folders`], each with
    /// the text of the [`crate::storage::FOLDER_FILE`] of the folder at the
    /// same sub-path over `from`, other keys included, or naming it by its
    /// directory when that has none (spec 2026-11-03 create folders §3).
    fn copied_folders(&mut self, from: &str, to: &str) -> Result<Vec<(String, String)>, Error> {
        let (Some((from_space, _)), Some((to_space, rel_path))) =
            (Self::split_space(from), Self::split_space(to))
        else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for sub in order::ancestors(rel_path) {
            out.push(self.copied_folder(from_space, to_space, sub)?);
        }
        Ok(out)
    }

    /// The folder at sub-path `sub` of `from_space`, as [`Self::make_folders`]
    /// makes it in `to_space`: its full path there and the text of the
    /// source's [`crate::storage::FOLDER_FILE`] as it is, other keys
    /// included, or naming it by its directory when the source has none,
    /// it can't be read, or the listing would call it broken
    /// ([`folder_file_name`]): a move never fails on a name, and never
    /// spreads a broken file to another space, where F24 would then block
    /// renaming the copy (spec 2026-11-03 create folders §3, F27). The
    /// broken source stays as it is, or goes to the trash with its folder.
    /// A copy whose name a sibling in `to_space` already shows is named by
    /// its directory too (F19 on every write).
    fn copied_folder(
        &mut self,
        from_space: &str,
        to_space: &str,
        sub: &str,
    ) -> Result<(String, String), Error> {
        let file =
            rel(&format!("{REQUESTS_DIR}/{from_space}/{sub}/{}", crate::storage::FOLDER_FILE))?;
        let leaf = sub.rsplit('/').next().unwrap_or(sub);
        let path = format!("{to_space}/{sub}");
        let usable = self
            .disk
            .read(&file)
            .ok()
            .flatten()
            .and_then(|text| folder_file_name(&text).ok().map(|name| (text, name)));
        let level = path.rsplit_once('/').map_or(to_space, |(l, _)| l);
        let taken = |name: &str| self.sibling_folder_name_taken(level, name, Some(&path));
        let text = match usable {
            Some((text, name)) if !taken(name.as_deref().unwrap_or(leaf)) => text,
            // A name a sibling shows, or no usable file: named by its
            // directory, `<dir> 2`, `<dir> 3`, … while a sibling shows that
            // too, ignoring case (F19), as a curl import numbers a request.
            _ => {
                let name = std::iter::once(leaf.to_string())
                    .chain((2..).map(|n| format!("{leaf} {n}")))
                    .find(|n| !taken(n))
                    .expect("some suffix is free");
                folder_file_text(&name)
            }
        };
        Ok((path, text))
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

/// The arrival cascade for the request at `rel`: each folder it sits in
/// that the op made (`made`, full paths, from [`Project::make_folders`]),
/// outermost first, then the request itself, each landing last in its
/// level (spec 2026-11-02 drag into folders D4). The op knows what it
/// made, so nothing is guessed from the list: a folder already there keeps
/// its place, empty or not, and one the list misses (its completion
/// couldn't run) draws by D6 until a completion writes it in.
fn arrive_with_folders(space: &str, order: &mut Vec<String>, rel: &str, made: &[String]) {
    for folder in order::ancestors(rel) {
        if made.iter().any(|m| m == &format!("{space}/{folder}")) {
            order::arrive(order, &order::folder_entry(folder));
        }
    }
    order::arrive(order, rel);
}

/// Where a new item's `entry` goes (spec 2026-11-03 create folders F14,
/// F30): right after `anchor` (the cursor's item) when the list names it,
/// else by `arrive` (last in its level, D4). The one placement rule a new
/// request and a new folder share.
fn place_new(
    order: &mut Vec<String>,
    entry: &str,
    anchor: Option<&str>,
    arrive: impl FnOnce(&mut Vec<String>),
) {
    match anchor {
        Some(a) if order.iter().any(|e| e == a) => {
            order::insert_beside(order, entry, Some(a), true)
        }
        _ => arrive(order),
    }
}

/// The text of a [`crate::storage::FOLDER_FILE`] naming its folder `name`
/// (spec 2026-11-03 create folders F15).
pub(super) fn folder_file_text(name: &str) -> String {
    let mut doc = toml_edit::DocumentMut::new();
    doc["name"] = toml_edit::value(name);
    doc.to_string()
}

/// `along` (folders with the text typed for each, from
/// [`Project::resolve_display_path`]) as [`Project::make_folders`] takes
/// them: each named by its typed text.
fn named(along: &[(String, String)]) -> Vec<(String, String)> {
    along.iter().map(|(path, typed)| (path.clone(), folder_file_text(typed))).collect()
}

/// The folder `folders` lists at full path `path` spelled in any case: a
/// path this finds is taken, whatever the filesystem's case rule. It compares
/// Unicode lowercase, not ASCII case: a case-insensitive disk folds
/// non-ASCII letters too.
fn folder_ignoring_case<'a>(
    folders: &'a [crate::storage::FolderListing],
    path: &str,
) -> Option<&'a crate::storage::FolderListing> {
    let wanted = path.to_lowercase();
    folders.iter().find(|f| f.path.to_lowercase() == wanted)
}

/// Each of `spaces` whose list in `meta` [`order::complete_order`]
/// changes, with the completed list.
fn completed_lists(
    listing: &[RequestListing],
    folders: &[crate::storage::FolderListing],
    meta: &ProjectMeta,
    spaces: &[String],
    drop_dead: bool,
) -> Vec<(String, Vec<String>)> {
    spaces
        .iter()
        .filter_map(|space| {
            let before = order::space_order(meta, space);
            let after = order::complete_order(listing, folders, before, space, drop_dead);
            (after != before).then(|| (space.clone(), after))
        })
        .collect()
}

impl Project {
    /// Reads `space`'s list from `meta`, hands it to `f`, and writes it
    /// back through `project.toml` only if `f` changed it. Refuses a
    /// space that does not exist on disk (no orphan tables). Every order
    /// write goes through here.
    ///
    /// In a directory with no `project.toml` it writes nothing, so no op
    /// ever creates one: a create, a rename, a move, a put or a drop does
    /// its file part and the items draw by D6 (spec 2026-11-02 drag into
    /// folders §3, `Project`). The two reorders, which write nothing else,
    /// refuse first ([`Self::require_project_toml`]).
    pub(super) fn order_edit(
        &mut self,
        space: &str,
        f: impl FnOnce(&mut Vec<String>),
    ) -> Result<(), Error> {
        if !self.has_project_toml() {
            return Ok(());
        }
        let before = order::space_order(&self.meta, space).to_vec();
        let mut after = before.clone();
        f(&mut after);
        if after == before {
            return Ok(());
        }
        // Only a real edit needs the space's directory to exist (an
        // orphan table would otherwise be written for it); a no-op edit
        // (e.g. moving zero requests out of a space with no directory)
        // must stay a no-op, as it always was before order lists existed.
        if !self.disk.is_dir(&space_rel(space)?) {
            return Err(Error::NotFound(space.to_string()));
        }
        self.edit_project_toml(|doc| order::write_order(doc, space, &after))
    }

    /// Whether the project's directory has a `project.toml`.
    fn has_project_toml(&self) -> bool {
        rel(PROJECT_TOML).is_ok_and(|path| self.disk.is_file(&path))
    }

    /// Refuses an op whose only write is an order (the two reorders) in a
    /// directory with no `project.toml`, where [`Self::order_edit`] writes
    /// nothing: the user is told the order can't be saved rather than the
    /// rows snapping back silently (spec 2026-11-02 drag into folders §3,
    /// `Project`).
    pub(super) fn require_project_toml(&self) -> Result<(), Error> {
        if self.has_project_toml() {
            return Ok(());
        }
        Err(Error::Conflict("the order can't be saved without a project.toml".to_string()))
    }

    /// The request at `rel` (relative to `space`) has just appeared: each
    /// folder it sits in that the op made, outermost first, then the
    /// request itself takes the arrival rule (spec 2026-11-02 drag into
    /// folders D4; see [`arrive_with_folders`]).
    pub(super) fn order_arrive(
        &mut self,
        space: &str,
        rel: &str,
        made: &[String],
    ) -> Result<(), Error> {
        self.order_edit(space, |order| arrive_with_folders(space, order, rel, made))
    }

    /// The request at `rel` has just left: its entry. The folders it sat
    /// in stay, entries and all (spec 2026-11-03 create folders F2).
    pub(super) fn order_remove(&mut self, space: &str, rel: &str) -> Result<(), Error> {
        self.order_edit(space, |order| order::remove(order, rel))
    }

    /// A request renamed in its own level: its entry, in place
    /// ([`order::rename`]).
    pub(super) fn order_rename(&mut self, space: &str, from: &str, to: &str) -> Result<(), Error> {
        self.order_edit(space, |order| order::rename(order, from, to))
    }

    pub(super) fn order_insert_after(
        &mut self,
        space: &str,
        anchor: &str,
        rel: &str,
    ) -> Result<(), Error> {
        self.order_insert_beside(space, anchor, rel, true)
    }

    /// `order_insert_after`'s twin for a put *above* the anchor: the new
    /// entry takes the anchor's own index and pushes it down.
    pub(super) fn order_insert_before(
        &mut self,
        space: &str,
        anchor: &str,
        rel: &str,
    ) -> Result<(), Error> {
        self.order_insert_beside(space, anchor, rel, false)
    }

    /// `rel` put right above or `below` `anchor`, by
    /// [`order::insert_beside`] (an entry already naming `rel`, a stale one
    /// a partial walk kept (D7), goes first, so the copy lands beside its
    /// anchor, D4). An anchor the list doesn't name writes nothing.
    fn order_insert_beside(
        &mut self,
        space: &str,
        anchor: &str,
        rel: &str,
        below: bool,
    ) -> Result<(), Error> {
        self.order_edit(space, |order| {
            if order.iter().any(|e| e == anchor) {
                order::insert_beside(order, rel, Some(anchor), below);
            }
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
        let folders: Vec<&crate::storage::FolderListing> =
            self.folder_listing.iter().filter(|f| f.path.starts_with(&prefix)).collect();
        let items = order::split_level(&entries, &folders, &prefix);
        let order = order::OrderIndex::new(order::space_order(&self.meta, space));
        order::order_entries(&items.requests, &items.folder_paths(), &order, space)
            .iter()
            .filter_map(|i| i.entry(space))
            .collect()
    }

    /// Re-lists, then writes every space's list complete, as it draws
    /// (spec 2026-11-02 drag into folders D5, D5a, D7:
    /// [`order::complete_order`]), in one write of `project.toml` through
    /// `edit_project_toml`. Returns whether it wrote.
    ///
    /// Every op that changes an order calls it first inside its own
    /// transaction ([`Self::order_transaction`], best effort), so the
    /// write is part of the op's undo step (D5), and the op then works on
    /// a complete list; the TUI also calls it
    /// whenever it re-lists, outside any transaction (D5a).
    ///
    /// Each list is built from the order in `project.toml` as it is on
    /// disk, never from `self.meta`, so an outside edit postui hasn't read
    /// yet (a `git checkout`) is kept (D5a). When the file's lists differ
    /// from `self.meta`'s, it goes through `edit_project_toml` even with
    /// nothing to write, so `self.meta` holds the lists the op goes on to
    /// change. Deciding that peeks at the file rather than reading it: a
    /// read records the file's stamp, and the next `poll` would then never
    /// re-read the rest of an outside edit. For the same reason a call
    /// that did read a file changed outside leaves that `poll` due.
    ///
    /// Only a whole listing drops entries naming nothing (D7): after a walk
    /// that failed part way (an unreadable folder, reported as a warning
    /// about no one file) what is missing is still added, but every entry
    /// is kept. A space whose directory is missing is left alone. A
    /// read-only project (the CLI) writes nothing, and neither does a
    /// directory with no `project.toml`, which never gains one behind the
    /// user's back (see [`Self::ensure_spaces`]).
    pub fn complete_order(&mut self) -> Result<bool, Error> {
        self.relist();
        let path = rel(PROJECT_TOML)?;
        if self.disk.is_read_only() || !self.disk.is_file(&path) {
            return Ok(false);
        }
        let whole = !self.listing_warnings.iter().any(|w| w.file.is_none());
        let spaces: Vec<String> = self
            .spaces
            .iter()
            .filter(|space| space_rel(space).is_ok_and(|dir| self.disk.is_dir(&dir)))
            .cloned()
            .collect();
        // A file removed since the `is_file` check is no project.toml: an
        // empty document here would lead `edit_project_toml` to make one.
        let Some(text) = self.disk.peek(&path)? else {
            return Ok(false);
        };
        // A `project.toml` that doesn't parse is never overwritten, and
        // completing an order is never the reason an op fails: the op goes
        // on as it would have, and fails at its own order write if it has
        // one (a name-only rename has none). The reload reports the file.
        let Ok(on_disk) = toml::from_str::<ProjectMeta>(&text) else {
            return Ok(false);
        };
        let unread = spaces
            .iter()
            .any(|s| order::space_order(&on_disk, s) != order::space_order(&self.meta, s));
        // The peek only decides whether there is anything to do.
        if !unread
            && completed_lists(&self.listing, &self.folder_listing, &on_disk, &spaces, whole)
                .is_empty()
        {
            return Ok(false);
        }
        let outside_edit = self.disk.changed(&path);
        // The lists written are built from the document `edit_project_toml`
        // reads, not from the peek: the file can change between the two,
        // and a list built from the peek would overwrite that change. The
        // listing is cloned because the closure can't borrow `self`; this
        // only happens when there is something to write.
        let listing = self.listing.clone();
        let folders = self.folder_listing.clone();
        let mut wrote = false;
        self.edit_project_toml(|doc| {
            // A document that no longer parses gets no list; the write
            // then refuses it (`edit_project_toml` validates).
            let Ok(meta) = toml::from_str::<ProjectMeta>(&doc.to_string()) else { return };
            for (space, after) in completed_lists(&listing, &folders, &meta, &spaces, whole) {
                order::write_order(doc, &space, &after);
                wrote = true;
            }
        })?;
        if outside_edit {
            self.force_reload = true;
        }
        Ok(wrote)
    }

    /// [`Self::complete_order`] as the first step of an op (D5), best
    /// effort: a completion that can't be written (a read-only file, a
    /// full disk) never fails the op, which goes on with the list as it
    /// is. The op's own order writes, if any, report their failures, and
    /// the refresh after it toasts this one (spec 2026-11-02 drag into
    /// folders §3, `Project`).
    fn complete_order_for_op(&mut self) {
        let _ = self.complete_order();
    }

    /// `transaction` for an op that changes an order: it completes the
    /// list first ([`Self::complete_order_for_op`]), then runs `f`, so the
    /// completion is part of the op's undo step (D5) and `f` works on a
    /// complete list. A batch opens one, so it completes once; the bodies
    /// its items share don't open their own.
    ///
    /// The completion lives here, not in [`Self::order_edit`] or
    /// `transaction` (D5): it must run before the op's file change, since
    /// a same-level rename needs the old name still listed to complete the
    /// list where it drew, and `order_edit` runs after that change;
    /// `transaction` is shared by ops that never touch an order
    /// (variables, environments). The two reorders stay on `transaction`
    /// (see [`Self::set_request_order`]).
    fn order_transaction<T>(
        &mut self,
        label: &str,
        meta: EntryMeta,
        f: impl FnOnce(&mut Project) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.transaction(label, meta, |p| {
            p.complete_order_for_op();
            f(p)
        })
    }

    /// One read-modify-write per space, however many requests moved.
    ///
    /// `drawn` is `from`'s items as the sidebar drew them before the move
    /// ([`order::drawn_entries`]), `moves` the requests' `(from, to)`
    /// relative slugs, `carried` the entries of the folders the move made
    /// in `to`, and `emptied` those of the source folders the move took away
    /// (spec 2026-11-03 create folders F27). `from` loses the moved
    /// requests and the emptied folders; `to` gains each item it lacks in
    /// draw order, a folder before what is in it, so every level arrives
    /// looking as it drew in `from` (D4, D22). The list as stored won't
    /// do: it may keep a level's entries apart and in any order across
    /// levels. A folder `to` already had keeps its place.
    pub(super) fn order_move_all(
        &mut self,
        from: &str,
        to: &str,
        drawn: &[String],
        moves: &[(String, String)],
        carried: &[String],
        emptied: &[String],
    ) -> Result<(), Error> {
        self.order_edit(from, |order| {
            for e in moves.iter().map(|(f, _)| f).chain(emptied) {
                order::remove(order, e);
            }
        })?;
        // The destination side only adds arrivals.
        let carried: std::collections::HashSet<&str> = carried.iter().map(String::as_str).collect();
        let moved: std::collections::HashMap<&str, &str> =
            moves.iter().map(|(f, t)| (f.as_str(), t.as_str())).collect();
        self.order_edit(to, |order| {
            // The move made the carried folders, so entries naming one or
            // under it name nothing (a partial walk kept them, D7). They go
            // here, once, before anything arrives. The folders then arrive
            // by `drawn`, not through `arrive_with_folders`: an empty one
            // has no request to arrive with.
            for e in &carried {
                order::remove_under(order, e);
            }
            for e in drawn {
                if order::is_folder_entry(e) {
                    if carried.contains(e.as_str()) {
                        order::arrive(order, e);
                    }
                } else if let Some(t) = moved.get(e.as_str()) {
                    // Its folders arrived above, at their own places.
                    arrive_with_folders(to, order, t, &[]);
                }
            }
        })
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

    /// Creates a request from a typed display path ("Folder/My Request!"):
    /// the curl import's, the one input that reads a folder path (spec
    /// 2026-11-03 create folders F29). Each folder segment is the sibling
    /// folder showing that name when there is one, else a folder made for
    /// it; the leaf is the request's name, refused when a request in that
    /// folder already shows it. It lands last in its level (D4).
    pub fn create_request(
        &mut self,
        display_path: &str,
        req: HttpRequest,
    ) -> Result<(String, String), Error> {
        // Folder names resolve against the disk as it is now: a folder made
        // outside since the last relist is found by its name.
        self.relist();
        let (folder, along, leaf) = self.resolve_display_path(display_path)?;
        self.create_at(&folder, &along, leaf, None, req)
    }

    /// Creates a request named `name` in `level` (a full path: a space or
    /// a folder). The name is only a name: a `/` in it is part of it (spec
    /// 2026-11-03 create folders F28). Its entry goes right after
    /// `after`'s (the cursor's item, F30), or last in its level (D4).
    pub fn create_request_in(
        &mut self,
        level: &str,
        name: &str,
        after: Option<&TreeItem>,
        req: HttpRequest,
    ) -> Result<(String, String), Error> {
        self.relist();
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::BadName(name.to_string()));
        }
        if !self.level_exists(level) {
            return Err(Error::NotFound(level.to_string()));
        }
        self.create_at(level, &[], name.to_string(), after, req)
    }

    /// The create both entry points share: `leaf` in `folder`, making the
    /// folders of `along` that are missing first.
    fn create_at(
        &mut self,
        folder: &str,
        along: &[(String, String)],
        leaf: String,
        after: Option<&TreeItem>,
        mut req: HttpRequest,
    ) -> Result<(String, String), Error> {
        Self::valid_level(folder)?;
        if self.sibling_name_taken(folder, &leaf, None) {
            return Err(Error::AlreadyExists(leaf));
        }
        let slug = self.unique_slug(folder, &leaf, None);
        let Some((space, rel)) = Self::split_space(&slug) else {
            return Err(Error::BadName(slug));
        };
        let (space, rel) = (space.to_string(), rel.to_string());
        let anchor = after.and_then(|a| Self::item_entry(a, &space));
        req.name = Some(leaf.clone());
        let meta = self.meta_for_moves(Vec::new());
        self.order_transaction("create request", meta, |p| {
            let made = p.make_folders(&named(along))?;
            p.fs_create_file(&request_rel(&slug)?, &req.to_toml_string())?;
            p.relist();
            p.order_edit(&space, |order| {
                place_new(order, &rel, anchor.as_deref(), |order| {
                    arrive_with_folders(&space, order, &rel, &made)
                })
            })
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
        name: &str,
    ) -> Result<(String, String), Error> {
        let from_path = request_rel(from_slug)?;
        if !self.disk.is_file(&from_path) {
            return Err(Error::NotFound(from_slug.to_string()));
        }
        let leaf = name.trim().to_string();
        if leaf.is_empty() {
            return Err(Error::BadName(name.to_string()));
        }
        let folder = from_slug.rsplit_once('/').map_or("", |(dir, _)| dir);
        if self.sibling_name_taken(folder, &leaf, Some(from_slug)) {
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
        let same_name = self
            .listing
            .iter()
            .find(|l| l.slug == from_slug)
            .is_some_and(|l| l.display_name().trim() == leaf);
        let to_slug = if same_name {
            from_slug.to_string()
        } else {
            self.unique_slug(folder, &leaf, Some(from_slug))
        };
        Ok((to_slug, leaf))
    }

    /// Renames the request to `name`, in its own folder: the name is only
    /// a name, and a `/` in it is part of it (spec 2026-11-03 create
    /// folders F28; moving between folders is a drag). Rewrites `name`
    /// when the file parses (a broken file just moves). Its entry keeps
    /// its place.
    pub fn rename_request(
        &mut self,
        from_slug: &str,
        name: &str,
    ) -> Result<(String, String), Error> {
        let from_path = request_rel(from_slug)?;
        self.relist();
        let (to_slug, leaf) = self.check_rename_request(from_slug, name)?;
        let to_path = request_rel(&to_slug)?;
        // Which held entries still matched their file BEFORE this op
        // touched anything: only those may be re-stamped afterwards (see
        // `rekey_held_after_own_op`).
        let clean_before = self.held_clean_before();
        let meta = self.meta_for_moves(vec![(from_slug.to_string(), to_slug.clone())]);
        let from_slug = from_slug.to_string();
        self.order_transaction("rename request", meta, |p| {
            if to_slug != from_slug {
                p.fs_rename(&from_path, &to_path)?;
            }
            if let Some(from) = p.set_name(&to_path, Some(&leaf))?
                && from.as_deref() != Some(leaf.as_str())
            {
                p.record(Op::Named {
                    path: to_path.clone(),
                    before: from,
                    after: Some(leaf.clone()),
                });
            }
            // A rename can rewrite the file's `name` (above), so this op
            // owns a write the app must never see as an outside edit —
            // hence the shared re-key, which re-stamps only an entry that
            // was clean going in.
            p.rekey_held_after_own_op(&[(from_slug.clone(), to_slug.clone())], &clean_before);
            p.relist();
            if let (Some((space, from_rel)), Some((_, to_rel))) =
                (Self::split_space(&from_slug), Self::split_space(&to_slug))
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
        self.order_transaction("move request", meta, |p| {
            // The listing these copies' name checks read is fresh: the
            // transaction's completion (`complete_order_for_op`) relisted
            // just before this closure runs, so a folder made outside since
            // the last refresh counts (F19).
            let copies = p.copied_folders(&slug, &new_slug)?;
            let made = p.make_folders(&copies)?;
            p.fs_rename(&from_path, &to_path)?;
            if let Some(req) = p.open_requests.shift_remove(&slug) {
                p.open_requests.insert(new_slug.clone(), req);
            }
            p.relist();
            let (from_space, from_rel) = Self::split_space(&slug).expect("validated");
            let (to_space, to_rel) = Self::split_space(&new_slug).expect("validated");
            p.order_remove(from_space, from_rel)?;
            p.order_arrive(to_space, to_rel, &made)
        })?;
        Ok(new_slug)
    }

    /// A sidebar drop in another folder (spec 2026-11-02 drag into folders
    /// D17–D19): moves `item` into `level` of its own space (relative,
    /// `""` for the top level) as one undo step, and returns its new slug
    /// (a request) or full path (a folder) with the `(old, new)` slug of
    /// every request it moved.
    ///
    /// `at` is the slot among `level`'s items, both kinds, as they draw
    /// before the move; it is read before anything moves, so a folder the
    /// move is about to empty still counts, as the sidebar drew it (D15).
    /// It is read as the item it names, not as an index, so it holds
    /// after the move completes the list (D5): completion lists every item
    /// that draws, that one included, without moving any. The item's entry
    /// leaves its old level and lands above the item at `at` (last past
    /// the end); a folder it empties stays, with its entry (spec
    /// 2026-11-03 create folders F2, replacing 2026-11-01 R9).
    ///
    /// A request keeps its file name, `-2`, `-3`, … when the folder has it,
    /// and its text. A folder's directory moves whole, every request under
    /// it in the entry's moves, and its entries are renamed in place, so
    /// its contents keep their order. When something there has its
    /// directory name, the directory is named from the name it shows, as a
    /// create names one (`-2`, `-3`, … while taken), and a folder that
    /// showed its directory name gets it written in its `.folder.toml`.
    /// Held requests follow.
    ///
    /// Refused, with nothing written: `level` being the item's own (a
    /// same-level drop is `set_request_order`'s, D20), a level or item that
    /// does not exist, a folder going inside itself or one of its own
    /// folders (D2), and a clash (D18): the folder already shows a request
    /// with the name the request will show there (a nameless one shows its
    /// file name after any `-2`), or a folder with the name the folder
    /// shows. A folder whose directory would be renamed and whose
    /// `.folder.toml` doesn't parse is refused too (F24).
    pub fn move_to_level(
        &mut self,
        item: &TreeItem,
        level: &str,
        at: usize,
    ) -> Result<(String, Moves), Error> {
        let (full, is_folder) = match item {
            TreeItem::Request(slug) => (slug.as_str(), false),
            TreeItem::Folder(path) => (path.as_str(), true),
        };
        let Some((space, rel)) = Self::split_space(full) else {
            return Err(Error::NotFound(full.to_string()));
        };
        let (space, rel) = (space.to_string(), rel.to_string());
        // `at` indexes the rows the sidebar drew from the listing as it
        // stands, so the anchor is read from that listing, before the
        // relist below: one removed outside above the slot since would
        // otherwise shift it. It is an entry string, so it holds through
        // the relist; if its item has gone, the op's completion drops its
        // entry (D7) and the drop lands last.
        let anchor = self.level_entries(&space, level).into_iter().nth(at);
        // The checks read the disk as it is now: a request added outside
        // since the last relist can clash (D18).
        self.relist();
        let present = if is_folder {
            folder_rel(full)?;
            order::folder_exists(&self.folder_listing, full)
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
        if !level.is_empty()
            && !order::folder_exists(&self.folder_listing, &format!("{space}/{level}"))
        {
            return Err(Error::NotFound(format!("{space}/{level}")));
        }
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
    ) -> Result<(String, Moves), Error> {
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
        let moves = vec![(slug.to_string(), new_slug.clone())];
        let meta = self.meta_for_moves(moves.clone());
        self.order_transaction("move request to folder", meta, |p| {
            p.fs_rename(&from_path, &to_path)?;
            if let Some(held) = p.open_requests.shift_remove(slug) {
                p.open_requests.insert(new_slug.clone(), held);
            }
            p.relist();
            // The folder the request left stays (spec 2026-11-03 create
            // folders F2).
            p.order_edit(space, |order| {
                order::remove(order, rel);
                order::insert_before(order, &new_rel, anchor.as_deref());
            })
        })?;
        Ok((new_slug, moves))
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
    ) -> Result<(String, Moves), Error> {
        let name = rel.rsplit_once('/').map_or(rel, |(_, name)| name);
        let Some(listed) = self.folder_listing.iter().find(|f| f.path == path) else {
            return Err(Error::NotFound(path.to_string()));
        };
        let shown = listed.display_name().to_string();
        let broken = listed.broken.clone();
        let nameless = listed.name.is_none();
        // D18: a sibling there that already shows the folder's name, in any
        // case, is the clash, named as it shows (spec 2026-11-03 create
        // folders F19): two rows of one name would be ambiguous to a typed
        // path (F18a).
        if let Some(clash) = self.siblings_showing(dest, &shown, Some(path)).next() {
            return Err(Error::AlreadyExists(clash.display_name().to_string()));
        }
        // It keeps its directory name (D17) unless that is taken; then the
        // name it shows gets a directory by the rule a create uses (F19's
        // one naming rule, #255).
        let own = format!("{dest}/{name}");
        let renamed = self.folder_path_taken(&own, None);
        let new_path = if renamed { self.folder_slot(dest, &shown, None) } else { own };
        let from_dir = folder_rel(path)?;
        // A renamed directory can't show a nameless folder's name, so its
        // file must hold it; a file that doesn't parse can't (F24).
        if renamed && let Some(e) = broken {
            return Err(Error::BadName(format!("{from_dir}/{}: {e}", crate::storage::FOLDER_FILE)));
        }
        let to_dir = folder_rel(&new_path)?;
        let new_rel = order::relative(&new_path, space).unwrap_or(&new_path).to_string();
        let moves = self.moves_under(path, &new_path);
        let meta = self.meta_for_moves(moves.clone());
        self.order_transaction("move folder to folder", meta, |p| {
            p.shift_folder(&from_dir, &to_dir, &moves)?;
            if renamed && nameless {
                p.write_folder_name(&to_dir, &shown)?;
            }
            p.relist();
            let entry = order::folder_entry(&new_rel);
            p.order_edit(space, |order| {
                // Nothing was on disk at the new path (it is free).
                order::rename_folder_entries(order, rel, &new_rel);
                order::insert_before(order, &entry, anchor.as_deref());
            })
        })?;
        Ok((new_path, moves))
    }

    /// Every request under the folder at full path `path` with its slug
    /// once the folder is at `new_path`.
    fn moves_under(&self, path: &str, new_path: &str) -> Moves {
        self.slugs_under(path)
            .into_iter()
            .map(|slug| {
                let to = format!("{new_path}{}", &slug[path.len()..]);
                (slug, to)
            })
            .collect()
    }

    /// The slug of every request under the folder at full path `path`, at
    /// any depth.
    fn slugs_under(&self, path: &str) -> Vec<String> {
        let inside = format!("{path}/");
        self.listing
            .iter()
            .filter(|l| l.slug.starts_with(&inside))
            .map(|l| l.slug.clone())
            .collect()
    }

    /// A folder's directory renamed `from_dir` → `to_dir` inside its
    /// caller's transaction, with every held request in `moves` following:
    /// the one directory move a folder drop and a folder rename share. The
    /// caller relists once its folder file is written too.
    fn shift_folder(
        &mut self,
        from_dir: &RelPath,
        to_dir: &RelPath,
        moves: &Moves,
    ) -> Result<(), Error> {
        self.fs_rename(from_dir, to_dir)?;
        for (old, new) in moves {
            if let Some(held) = self.open_requests.shift_remove(old) {
                self.open_requests.insert(new.clone(), held);
            }
        }
        Ok(())
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
        let sources: Vec<String> = order::displayed_slugs(
            &self.listing,
            &self.folder_listing,
            order::space_order(&self.meta, from),
            from,
        );
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
        // The folders go too, empty ones included (spec 2026-11-03 create
        // folders F27), by path, so a parent comes before what is in it.
        // Every folder whose name is a valid slug is made in `to`; one
        // whose name isn't stays, as its requests do (F33), and so does
        // everything under it. A valid path's parents are valid, so each
        // folder's parent is made before it, with its file and journaled.
        // The cost of carrying every valid folder: one that holds only what
        // is left behind gets an empty twin in `to`. Picking which folders
        // to carry by what they hold took a patch per case in review
        // (rounds 5 and 6), and a twin loses nothing.
        let prefix = format!("{from}/");
        let mut folders: Vec<String> = Vec::new();
        for f in self.folder_listing.iter().filter(|f| f.path.starts_with(&prefix)) {
            let Ok(dir) = folder_rel(&f.path) else {
                // Named once, where its own name is invalid, as `check`
                // names it (F33); what is under it stays with it unsaid.
                if !f.own_name_is_valid() {
                    left_behind.push(format!(
                        "left behind: requests/{}/ (its name is not a valid slug; rename it first)",
                        f.path
                    ));
                }
                continue;
            };
            // One removed outside since the last relist is simply gone: an
            // empty folder holds nothing to lose, unlike a request, whose
            // absence refuses the whole move above.
            if !self.disk.is_dir(&dir) {
                continue;
            }
            folders.push(f.path.clone());
        }
        // Moving nothing changes no order, so it writes nothing (D5).
        if pairs.is_empty() && folders.is_empty() {
            return Ok((pairs, left_behind));
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
        self.order_transaction("move all requests", meta, |p| {
            // The items as they draw, read before anything moves.
            let drawn = order::drawn_entries(
                &p.listing,
                &p.folder_listing,
                order::space_order(&p.meta, from),
                from,
            );
            // A folder `to` already has keeps its own file (F27).
            // One at a time, so each copy's name is checked against the
            // copies made before it, not only the target's own (F19).
            let mut made = Vec::new();
            for path in &folders {
                let copy = p.copied_folder(from, to, &path[prefix.len()..])?;
                made.extend(p.make_folders(&[copy])?);
            }
            for (f, t) in &pairs {
                p.fs_rename(&request_rel(f)?, &request_rel(t)?)?;
                if let Some(req) = p.open_requests.shift_remove(f) {
                    p.open_requests.insert(t.clone(), req);
                }
            }
            // The source folders, inside out, once nothing is left in them
            // but their own `.folder.toml`: one that still holds anything
            // else (a request left behind, a dot file the user keeps there,
            // a `.DS_Store`) stays where it is with it (F16: dot files are
            // the user's).
            let mut emptied = Vec::new();
            for path in folders.iter().rev() {
                let dir = folder_rel(path)?;
                let only_its_file =
                    |e: &crate::disk::DirEntry| !e.is_dir && e.name == crate::storage::FOLDER_FILE;
                if p.disk.list(&dir)?.iter().all(only_its_file) {
                    p.fs_trash(&dir)?;
                    emptied.push(order::folder_entry(&path[prefix.len()..]));
                }
            }
            p.relist();
            // Only the folders the op made arrive: one `to` already had keeps
            // its place, even when its list misses it (its completion
            // couldn't run), as every other op's arrivals do.
            let to_prefix = format!("{to}/");
            let carried: Vec<String> = made
                .iter()
                .filter_map(|m| m.strip_prefix(&to_prefix))
                .map(order::folder_entry)
                .collect();
            p.order_move_all(from, to, &drawn, &rel_pairs, &carried, &emptied)?;
            Ok(())
        })?;
        Ok((pairs, left_behind))
    }

    /// Whether a folder directly in `level` (a full path: a space, or a
    /// folder) already shows `name`, ignoring case (spec 2026-11-03 create
    /// folders F19), `exclude` aside.
    pub fn sibling_folder_name_taken(
        &self,
        level: &str,
        name: &str,
        exclude: Option<&str>,
    ) -> bool {
        self.siblings_showing(level, name, exclude).next().is_some()
    }

    /// The folders directly in `level` (a full path) that show `name`,
    /// ignoring case, `exclude` aside.
    fn siblings_showing<'a>(
        &'a self,
        level: &str,
        name: &str,
        exclude: Option<&'a str>,
    ) -> impl Iterator<Item = &'a crate::storage::FolderListing> + use<'a> {
        let wanted = name.trim().to_lowercase();
        let parent = format!("{level}/");
        self.folder_listing.iter().filter(move |f| {
            exclude != Some(f.path.as_str())
                && f.path.strip_prefix(&parent).is_some_and(|rest| !rest.contains('/'))
                && f.display_name().to_lowercase() == wanted
        })
    }

    /// Whether `level` (a full path) is a space or a listed folder.
    /// A space counts with or without its directory: one listed in
    /// `project.toml` whose directory git didn't keep (it had no file yet)
    /// takes a new request or folder, as a create made it before folders
    /// did (the write makes the directory).
    fn level_exists(&self, level: &str) -> bool {
        if level.contains('/') {
            order::folder_exists(&self.folder_listing, level)
        } else {
            self.spaces.iter().any(|s| s == level)
                || space_rel(level).is_ok_and(|dir| self.disk.is_dir(&dir))
        }
    }

    /// A list entry for `item` in `space`, for an anchor.
    fn item_entry(item: &TreeItem, space: &str) -> Option<String> {
        match item {
            TreeItem::Request(slug) => order::relative(slug, space).map(str::to_string),
            TreeItem::Folder(path) => order::relative(path, space).map(order::folder_entry),
        }
    }

    /// Makes a folder named `name` in `level` (a full path: a space, or a
    /// folder), with its [`crate::storage::FOLDER_FILE`], as one undo step
    /// (spec 2026-11-03 create folders F14, F18, F19). Its entry goes right
    /// after `after`'s (the cursor's item) or, with none, last in its level
    /// (2026-11-02 D4). Returns its full path.
    ///
    /// Refused, with nothing written: an empty name, a missing level, and a
    /// name a sibling folder already shows. A slug that clashes while the
    /// shown names differ gets `-2`, `-3`, …
    pub fn create_folder(
        &mut self,
        level: &str,
        name: &str,
        after: Option<&TreeItem>,
    ) -> Result<String, Error> {
        let name = folder_name(name)?;
        self.relist();
        if !self.level_exists(level) {
            return Err(Error::NotFound(level.to_string()));
        }
        Self::valid_level(level)?;
        if self.sibling_folder_name_taken(level, name, None) {
            return Err(Error::AlreadyExists(name.to_string()));
        }
        let path = self.folder_slot(level, name, None);
        let Some((space, rel_path)) = Self::split_space(&path) else {
            return Err(Error::BadName(path));
        };
        let (space, entry) = (space.to_string(), order::folder_entry(rel_path));
        let anchor = after.and_then(|a| Self::item_entry(a, &space));
        let meta = self.meta_for_moves(Vec::new());
        self.order_transaction("create folder", meta, |p| {
            p.make_folders(&[(path.clone(), folder_file_text(name))])?;
            p.relist();
            p.order_edit(&space, |order| {
                place_new(order, &entry, anchor.as_deref(), |order| order::arrive(order, &entry))
            })
        })?;
        Ok(path)
    }

    /// Renames the folder at full path `path` to show `name`, as one undo
    /// step (spec 2026-11-03 create folders F21, F23): a new slug renames
    /// the directory, its entry and its contents' entries in place (its
    /// slot is kept), and held requests follow; then its
    /// [`crate::storage::FOLDER_FILE`] gets the name, other keys kept, or
    /// is made (F20). Returns its new full path and the `(old, new)` slug of
    /// every request it moved.
    ///
    /// Refused, with nothing written: a folder not listed, an empty name, a
    /// name a sibling folder shows, a folder file that doesn't parse (F24),
    /// and a folder whose directory name isn't a valid slug (made in a file
    /// manager: `My Stuff/`), as an op on a request file of such a name is.
    /// A new slug something on disk already has is numbered (`-2`, `-3`, …),
    /// not refused (F19's one naming rule).
    pub fn rename_folder(&mut self, path: &str, name: &str) -> Result<(String, Moves), Error> {
        let name = folder_name(name)?;
        self.relist();
        let Some(listed) = self.folder_listing.iter().find(|f| f.path == path).cloned() else {
            return Err(Error::NotFound(path.to_string()));
        };
        let from_dir = folder_rel(path)?;
        if let Some(e) = &listed.broken {
            return Err(Error::BadName(format!("{from_dir}/{}: {e}", crate::storage::FOLDER_FILE)));
        }
        let Some((level, _)) = path.rsplit_once('/') else {
            return Err(Error::BadName(path.to_string()));
        };
        if self.sibling_folder_name_taken(level, name, Some(path)) {
            return Err(Error::AlreadyExists(name.to_string()));
        }
        // The one naming rule: the directory it has when the name's slug
        // is that one, else the slug, numbered while taken (F19).
        let new_path = self.folder_slot(level, name, Some(path));
        let to_dir = folder_rel(&new_path)?;
        let moves = if new_path == path { Vec::new() } else { self.moves_under(path, &new_path) };
        let space = crate::storage::space_of(path).unwrap_or(path).to_string();
        let (from_rel, to_rel) = (
            order::relative(path, &space).unwrap_or(path).to_string(),
            order::relative(&new_path, &space).unwrap_or(&new_path).to_string(),
        );
        let meta = self.meta_for_moves(moves.clone());
        self.order_transaction("rename folder", meta, |p| {
            if new_path != path {
                p.shift_folder(&from_dir, &to_dir, &moves)?;
            }
            p.write_folder_name(&to_dir, name)?;
            p.relist();
            if new_path != path {
                // Nothing was on disk at the new path (refused above).
                p.order_edit(&space, |order| {
                    order::rename_folder_entries(order, &from_rel, &to_rel)
                })?;
            }
            Ok(())
        })?;
        Ok((new_path, moves))
    }

    /// Gives the [`crate::storage::FOLDER_FILE`] in `dir` the name `name`,
    /// other keys kept, writing only when that changes it, or makes the
    /// file (F20). Inside its caller's transaction, so undo takes it back.
    fn write_folder_name(&mut self, dir: &RelPath, name: &str) -> Result<(), Error> {
        let file = rel(&format!("{dir}/{}", crate::storage::FOLDER_FILE))?;
        match self.disk.read(&file)? {
            Some(text) => {
                let after = super::toml_with_name(&text, Some(name))
                    .map_err(|e| Error::BadName(e.to_string()))?;
                if after != text {
                    self.fs_write_text(&file, Some(&after))?;
                }
                Ok(())
            }
            None => self.fs_create_file(&file, &folder_file_text(name)),
        }
    }

    /// Deletes the folder at full path `path` and everything in it, as one
    /// undo step, with no confirmation (spec 2026-11-03 create folders
    /// F21): its directory goes to the trash, held requests in it are
    /// dropped, and its entry and every entry under it leave the list.
    /// Returns the slug of each request it took.
    pub fn delete_folder(&mut self, path: &str) -> Result<Vec<String>, Error> {
        self.relist();
        if !order::folder_exists(&self.folder_listing, path) {
            return Err(Error::NotFound(path.to_string()));
        }
        let dir = folder_rel(path)?;
        let gone = self.slugs_under(path);
        let space = crate::storage::space_of(path).unwrap_or(path).to_string();
        let entry = order::folder_entry(order::relative(path, &space).unwrap_or(path));
        let meta = self.meta_for_moves(Vec::new());
        self.order_transaction("delete folder", meta, |p| {
            p.fs_trash(&dir)?;
            for slug in &gone {
                p.open_requests.shift_remove(slug);
            }
            p.relist();
            p.order_edit(&space, |order| order::remove_under(order, &entry))
        })?;
        Ok(gone)
    }

    /// Deletes every request in `slugs` as one undo entry (piece 4 spec
    /// §5.2: a counted `dd` is one step). Duplicates are dropped. Every
    /// slug is pre-flighted (valid and present) before the first file
    /// moves, the way [`Self::move_all_requests`] does, so a refusal
    /// deletes nothing. The per-slug deletes share one transaction, so the
    /// journal gets one entry, and the list is completed once at its top.
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
        // The list is completed once, before any file moves (D5).
        self.order_transaction("delete requests", meta, |p| {
            for s in &batch {
                p.delete_one(s)?;
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
        self.order_transaction("delete request", meta, |p| p.delete_one(slug))
    }

    /// One delete inside its caller's transaction, the list already
    /// completed: the file trashed, then its entry out of the list. The
    /// folder it sat in stays (spec 2026-11-03 create folders F2).
    fn delete_one(&mut self, slug: &str) -> Result<(), Error> {
        self.fs_trash(&request_rel(slug)?)?;
        self.open_requests.shift_remove(slug);
        self.relist();
        if let Some((space, rel)) = Self::split_space(slug) {
            self.order_remove(space, rel)?;
        }
        Ok(())
    }

    /// `<name> copy`, `<name> copy 2`, … next to its source; a broken
    /// file is byte-copied to `<slug>-copy`.
    pub fn duplicate_request(&mut self, slug: &str) -> Result<String, Error> {
        self.duplicate_request_at(slug, slug, false)
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
        self.duplicate_source(slug)?;
        let meta = self.meta_for_moves(Vec::new());
        self.order_transaction("duplicate request", meta, |p| p.duplicate_one(slug, anchor, above))
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
        self.duplicate_source(slug)?;
        let meta = self.meta_for_moves(Vec::new());
        // The list is completed once, before any file is made (D5).
        self.order_transaction("duplicate requests", meta, |p| {
            let mut made: Vec<String> = Vec::new();
            for _ in 0..count.max(1) {
                let at = match made.last() {
                    Some(prev) if !above => prev.clone(),
                    _ => anchor.to_string(),
                };
                made.push(p.duplicate_one(slug, &at, above)?);
            }
            Ok(made)
        })
    }

    /// Refuses a duplicate whose source is not a request file, before
    /// anything is written.
    fn duplicate_source(&self, slug: &str) -> Result<(), Error> {
        if !self.disk.is_file(&request_rel(slug)?) {
            return Err(Error::NotFound(slug.to_string()));
        }
        Ok(())
    }

    /// The body a single duplicate and a counted put share: one copy,
    /// inside its caller's transaction, the list already completed.
    fn duplicate_one(&mut self, slug: &str, anchor: &str, above: bool) -> Result<String, Error> {
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
        let path = request_rel(&new_slug)?;
        self.disk.write_new(&path, "")?;
        self.record(Op::Created { path: path.clone() });
        self.disk.write_bytes(&path, &text)?;
        self.relist();
        // The anchor is listed (every list names every item, spec
        // 2026-11-02 drag into folders D5), so the copy goes beside it; an
        // anchor in another space has no place here, and the copy arrives
        // (D4).
        match (Self::split_space(anchor), Self::split_space(&new_slug)) {
            (Some((a_space, a_rel)), Some((space, rel))) if a_space == space => {
                if above {
                    self.order_insert_before(space, a_rel, rel)?;
                } else {
                    self.order_insert_after(space, a_rel, rel)?;
                }
            }
            (_, Some((space, rel))) => {
                // A copy lands in its anchor's folder, which is there.
                self.order_arrive(space, rel, &[])?;
            }
            _ => {}
        }
        Ok(new_slug)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{assert_none_lost, fixture, read, request_files, requests_tree};
    use super::*;
    use crate::journal::Op;
    use crate::storage::FolderListing;

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
        let (slug, _) = p.rename_request("main/ping", "Pong").unwrap();
        assert_eq!(slug, "main/pong");
        assert!(!dir.path().join("requests/main/ping.toml").exists());
        assert!(read(&dir, "requests/main/pong.toml").unwrap().contains("name = \"Pong\""));
        let e = p.journal.pop_undo().unwrap();
        assert_eq!(e.meta.moves, vec![("main/ping".to_string(), "main/pong".to_string())]);
        assert!(e.ops.iter().any(|o| matches!(o, Op::Renamed { .. })));
        assert!(
            e.ops.iter().any(
                |o| matches!(o, Op::Named { path, .. } if path.as_str() == "requests/main/pong.toml")
            ),
            "the name rewrite"
        );
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

    /// Spec 2026-11-03 create folders F28: a rename takes a name only. A
    /// `/` in it is part of the name, so the request stays in its folder
    /// and its space.
    #[test]
    fn a_rename_is_a_name_only() {
        let (dir, mut p) = fixture();
        let (slug, leaf) = p.rename_request("main/ping", "auth/Pong").unwrap();
        assert_eq!((slug.as_str(), leaf.as_str()), ("main/auth-pong", "auth/Pong"));
        assert!(dir.path().join("requests/main/auth-pong.toml").is_file());
        assert!(!dir.path().join("requests/main/auth").exists());
        assert!(!dir.path().join("requests/auth/pong.toml").exists());
    }

    /// F28, F30: a name-only create lands in its level right after the
    /// item it is given, or last; a `/` in the name is part of it.
    #[test]
    fn create_request_in_lands_after_its_anchor_or_last() {
        let (dir, mut p) =
            folder_project(&["a", "f/", "f/x", "b"], &["main/a", "main/b", "main/f/x"]);
        let (slug, _) =
            p.create_request_in("main", "N/1", Some(&req_item("main/a")), req("u")).unwrap();
        assert_eq!(slug, "main/n-1");
        assert!(!dir.path().join("requests/main/n").exists());
        let (slug, _) = p.create_request_in("main/f", "Last", None, req("u")).unwrap();
        assert_eq!(slug, "main/f/last");
        // `f/last` is last in its level as it draws (D4).
        assert_eq!(main_order(&p), ["a", "n-1", "f/", "f/x", "b", "f/last"]);
        assert!(matches!(
            p.create_request_in("main", "N/1", None, req("u")),
            Err(Error::AlreadyExists(_))
        ));
        assert!(matches!(
            p.create_request_in("main/gone", "x", None, req("u")),
            Err(Error::NotFound(_))
        ));
    }

    /// #206: a request in a hand-made folder whose name `slugify` would
    /// change (`old-`, `-wip`) stays in it on a rename, and a curl import's
    /// path naming it lands in it.
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
            let (slug, leaf) = p.rename_request(&from, "Ping2").unwrap();
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
        let (slug, _) = p.rename_request("old-/ping", "Ping2").unwrap();
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
    /// read as the prompt reads it back (trimmed). Another name derives its
    /// slug as before.
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
            let (slug, leaf) = p.rename_request("main/ping-", "ping-").unwrap();
            assert_eq!((slug.as_str(), leaf.as_str()), ("main/ping-", "ping-"), "{ping_taken}");
            assert!(dir.path().join("requests/main/ping-.toml").is_file());
            assert!(!dir.path().join("requests/main/ping-2.toml").exists());
            assert_eq!(dir.path().join("requests/main/ping.toml").exists(), ping_taken);
        }

        let (dir, mut p) = fixture();
        for (name, typed) in [("Get User", "Get User"), (" Pong ", " Pong ")] {
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
        let (slug, _) = p.rename_request("main/odd", "Other").unwrap();
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
        let (slug, _) = p.rename_request(&b, "Get User").unwrap();
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

    /// The copy's file is one `Created` op, its bytes never journaled; the
    /// rest of the step is `project.toml` (the completion, spec 2026-11-02
    /// drag into folders D5, and the copy's entry).
    #[test]
    fn duplicate_journals_a_single_created_op_for_the_copy() {
        let (_dir, mut p) = fixture();
        let copy = p.duplicate_request("auth/login").unwrap();
        let e = p.journal.pop_undo().unwrap();
        let (toml, files): (Vec<&Op>, Vec<&Op>) = e
            .ops
            .iter()
            .partition(|o| matches!(o, Op::Text { path, .. } if path.as_str() == PROJECT_TOML));
        assert_eq!(files.len(), 1, "{:?}", e.ops);
        assert!(
            matches!(files[0], Op::Created { path } if path.as_str() == format!("requests/{copy}.toml"))
        );
        assert_eq!(toml.len(), 2, "{:?}", e.ops);
    }

    /// A project whose `main` space holds `slugs` and whose `[space.main]
    /// order` is `order`; `auth` exists and is empty but listed. The lists
    /// can miss items: every op completes them first (spec 2026-11-02 drag
    /// into folders D5), writing what they miss in where it drew.
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

    /// Spec 2026-11-03 create folders F2 (replacing 2026-11-01 R9): a
    /// folder whose last request leaves stays, its entry and every
    /// ancestor's with it, and undo puts the list back byte for byte. The
    /// requests the list missed are written in first (2026-11-02 D5).
    #[test]
    fn deleting_a_folders_last_request_keeps_the_folder_and_its_entry() {
        let (dir, mut p) = folder_project(
            &["b/", "a/", "a/c/", "a/c/x", "z/"],
            &["main/a/c/x", "main/b/y", "main/z/q"],
        );
        let before = read(&dir, "project.toml");
        p.delete_request("main/a/c/x").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "a/c/", "z/", "b/y", "z/q"]);
        assert!(p.folders().iter().any(|f| f.path == "main/a/c"), "the folder stays");
        p.undo().unwrap();
        assert_eq!(read(&dir, "project.toml"), before);
    }

    /// F1, F4: an empty folder takes an entry when the list is completed.
    #[test]
    fn complete_order_writes_an_entry_for_an_empty_folder() {
        let (dir, mut p) = folder_project(&["r"], &["main/r"]);
        std::fs::create_dir_all(dir.path().join("requests/main/e")).unwrap();
        p.relist();
        assert!(p.complete_order().unwrap());
        assert_eq!(order::space_order(p.meta(), "main"), ["r", "e/"]);
    }

    /// F2: a request created into an empty folder keeps the folder's slot.
    #[test]
    fn a_create_in_an_empty_folder_keeps_its_slot() {
        let (dir, mut p) = folder_project(&["e/", "r"], &["main/r"]);
        std::fs::create_dir_all(dir.path().join("requests/main/e")).unwrap();
        p.relist();
        p.create_request("main/e/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["e/", "r", "e/new"]);
    }

    /// D5: the folder keeps its entry, and the list is completed first.
    #[test]
    fn deleting_a_request_whose_folder_keeps_others_leaves_the_entry() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/a/y", "main/b/z"]);
        p.delete_request("main/a/x").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "b/z", "a/y"]);
    }

    /// D4: a create lands last, in a level whatever it lists (replaces
    /// "a create in an unarranged level writes nothing"); the folder the
    /// list missed is written in first (D5).
    #[test]
    fn a_create_in_a_new_folder_lands_last() {
        let (_dir, mut p) = folder_project(&["r"], &["main/r", "main/a/x"]);
        p.create_request("main/c/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["r", "a/", "a/x", "c/", "c/new"]);
    }

    /// D4: every new ancestor arrives, outermost first, then the request.
    #[test]
    fn a_create_in_a_new_nested_folder_arrives_every_new_ancestor() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/b/z"]);
        p.create_request("main/c/d/New", req("u")).unwrap();
        assert_eq!(
            order::space_order(p.meta(), "main"),
            ["b/", "a/", "b/z", "a/x", "c/", "c/d/", "c/d/new"]
        );
    }

    /// An entry left behind by a folder emptied outside the app never
    /// hands its slot to a new folder of that name: the op's completion
    /// drops it (D5, D7), and the folder arrives last (D4).
    #[test]
    fn a_create_in_a_folder_made_again_drops_its_leftover_entry() {
        let (_dir, mut p) = folder_project(&["c/", "b/", "a/"], &["main/a/x", "main/b/z"]);
        p.create_request("main/c/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "b/z", "a/x", "c/", "c/new"]);
    }

    /// D4: the folder keeps its slot; the request lands last in it.
    #[test]
    fn a_create_in_an_existing_folder_keeps_its_slot() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/b/z"]);
        p.create_request("main/b/New", req("u")).unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "b/z", "a/x", "b/new"]);
    }

    /// D4: the new folder and the request land last in the other space,
    /// after what the completion (D5) wrote in on both sides.
    #[test]
    fn a_move_to_another_space_cascades_folders_on_both_sides() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/b/z"]);
        std::fs::create_dir_all(p.root().join("requests/auth/x")).unwrap();
        std::fs::write(p.root().join("requests/auth/x/k.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        p.move_request("main/a/x", "auth").unwrap();
        assert_eq!(order::space_order(p.meta(), "main"), ["b/", "a/", "b/z"], "a/ stays (F2)");
        assert_eq!(order::space_order(p.meta(), "auth"), ["x/", "x/k", "a/", "a/x"]);
    }

    /// D4, D22: everything arrives last, in the order `main` showed it,
    /// after what the completion (D5) wrote in.
    #[test]
    fn move_all_cascades_folders_on_both_sides() {
        let (_dir, mut p) = folder_project(&["b/", "a/"], &["main/a/x", "main/a/y", "main/b/z"]);
        std::fs::create_dir_all(p.root().join("requests/auth/x")).unwrap();
        std::fs::write(p.root().join("requests/auth/x/k.toml"), "method = \"GET\"\nurl = \"u\"\n")
            .unwrap();
        p.relist();
        p.move_all_requests("main", "auth").unwrap();
        // Spec 2026-11-03 create folders F27: the folders go too, so
        // `main` ends empty; each level of `auth` gains them as `main`
        // drew them.
        assert!(order::space_order(p.meta(), "main").is_empty());
        assert_eq!(
            order::space_order(p.meta(), "auth"),
            ["x/", "x/k", "b/", "b/z", "a/", "a/x", "a/y"]
        );
    }

    /// F27: empty and nested folders cross too, each with its file, and
    /// the source ends empty; undo is exact.
    #[test]
    fn move_all_carries_empty_and_nested_folders_and_empties_the_source() {
        let (dir, mut p) = folder_project(&[], &["main/a/x"]);
        write(&dir, "requests/main/a/.folder.toml", "name = \"Ay\"\n");
        std::fs::create_dir_all(dir.path().join("requests/main/a/b")).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main/e")).unwrap();
        p.relist();
        undo_redo_tree_exact(&dir, &mut p, |p| {
            p.move_all_requests("main", "auth").unwrap();
        });
        let under = |space: &str| -> Vec<String> {
            let prefix = format!("{space}/");
            p.folders()
                .iter()
                .filter(|f| f.path.starts_with(&prefix))
                .map(|f| f.path.clone())
                .collect()
        };
        assert!(under("main").is_empty(), "{:?}", under("main"));
        assert_eq!(under("auth"), ["auth/a", "auth/a/b", "auth/e"]);
        assert_eq!(read(&dir, "requests/auth/a/.folder.toml").as_deref(), Some("name = \"Ay\"\n"));
        assert!(dir.path().join("requests/auth/a/x.toml").is_file());
        assert!(order::space_order(p.meta(), "main").is_empty());
        let auth = order::space_order(p.meta(), "auth");
        for e in ["a/", "a/b/", "e/", "a/x"] {
            assert!(auth.contains(&e.to_string()), "{e} in {auth:?}");
        }
    }

    /// F27, D22: each level arrives as it drew, whatever order the source
    /// list keeps its entries in across levels (review round 1: walking
    /// the list as stored put `f/` above `c`).
    #[test]
    fn move_all_keeps_each_level_as_it_drew_from_an_interleaved_list() {
        let (_dir, mut p) =
            folder_project(&["f/y", "c", "f/", "f/x"], &["main/c", "main/f/x", "main/f/y"]);
        let drawn = order::displayed_slugs(p.requests(), p.folders(), &main_order(&p), "main");
        assert_eq!(drawn, ["main/c", "main/f/y", "main/f/x"]);
        p.move_all_requests("main", "auth").unwrap();
        let auth = order::space_order(p.meta(), "auth").to_vec();
        assert_eq!(
            order::displayed_slugs(p.requests(), p.folders(), &auth, "auth"),
            ["auth/c", "auth/f/y", "auth/f/x"]
        );
    }

    /// F27: a folder the target has at the same path is merged into, and
    /// keeps the target's file.
    #[test]
    fn move_all_merges_into_a_same_path_folder_keeping_its_file() {
        let (dir, mut p) = folder_project(&[], &["main/a/x", "auth/a/y"]);
        write(&dir, "requests/main/a/.folder.toml", "name = \"From\"\n");
        write(&dir, "requests/auth/a/.folder.toml", "name = \"To\"\n");
        p.relist();
        p.move_all_requests("main", "auth").unwrap();
        assert_eq!(read(&dir, "requests/auth/a/.folder.toml").as_deref(), Some("name = \"To\"\n"));
        assert!(dir.path().join("requests/auth/a/x.toml").is_file());
        assert!(dir.path().join("requests/auth/a/y.toml").is_file());
        assert!(!dir.path().join("requests/main/a").exists());
    }

    /// F27: a space holding only empty folders moves them.
    #[test]
    fn move_all_from_a_space_with_only_empty_folders_moves_them() {
        let (dir, mut p) = folder_project(&[], &[]);
        std::fs::create_dir_all(dir.path().join("requests/main/e")).unwrap();
        p.relist();
        p.move_all_requests("main", "auth").unwrap();
        assert!(dir.path().join("requests/auth/e").is_dir());
        assert!(!dir.path().join("requests/main/e").exists());
    }

    /// F27: a folder still holding a file postui can't move (a left-behind
    /// request, a note) stays, with what it holds.
    #[test]
    fn move_all_keeps_a_source_folder_that_still_holds_files() {
        let (dir, mut p) = folder_project(&[], &["main/a/x"]);
        write(&dir, "requests/main/a/notes.md", "keep me");
        p.relist();
        p.move_all_requests("main", "auth").unwrap();
        assert!(dir.path().join("requests/main/a/notes.md").is_file());
        assert!(dir.path().join("requests/auth/a/x.toml").is_file());
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
        let (to_slug, _) = p.rename_request("main/ping", "renamed").unwrap();
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
        let (to_slug, _) = p.rename_request("main/ping", "renamed").unwrap();
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
        let (to_slug, leaf) = p.rename_request("main/ping", "Ping!!!").unwrap();
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
        let (to_slug, _) = p.rename_request("main/ping", "Ping!!!").unwrap();
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

    /// D7 (amended 2026-11-03): a walk that failed part way (an unreadable
    /// folder) still writes in what the list misses, but drops nothing, so
    /// the unreadable folder and an entry naming nothing keep their places
    /// (replaces "a listing that is not whole writes nothing"). The next
    /// whole listing drops the dead entry. A space whose directory is
    /// missing is left alone.
    #[cfg(unix)]
    #[test]
    fn complete_order_from_a_partial_walk_adds_but_drops_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = tree_fixture();
        let before = main_order(&p);
        write(&dir, "requests/main/zeta.toml", "method = \"GET\"\nurl = \"u\"\n");
        std::fs::remove_file(dir.path().join("requests/main/health.toml")).unwrap();
        let users = dir.path().join("requests/main/users");
        std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o000)).unwrap();
        let wrote = p.complete_order();
        let warned = p.listing_warnings().iter().any(|w| w.file.is_none());
        std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(warned, "the walk failed part way");
        assert!(wrote.unwrap(), "what is missing is added");
        let mut added = before.clone();
        added.push("zeta".to_string());
        assert_eq!(main_order(&p), added, "every entry is kept");

        assert!(p.complete_order().unwrap(), "a whole listing drops the dead entry");
        assert!(!main_order(&p).contains(&"health".to_string()));
        assert!(main_order(&p).contains(&"users/v1/get".to_string()));

        let kept = read(&dir, "project.toml");
        std::fs::rename(dir.path().join("requests/main"), dir.path().join("main-away")).unwrap();
        assert!(!p.complete_order().unwrap(), "a missing space keeps its list");
        assert_eq!(read(&dir, "project.toml"), kept);
    }

    /// D4 over D7: a put whose copy's name is already listed (a stale entry
    /// a partial walk kept) still lands beside its anchor, below or above
    /// it, the stale entry dropping out, never in the old entry's slot.
    #[cfg(unix)]
    #[test]
    fn a_put_beside_its_anchor_replaces_a_stale_entry_of_its_name() {
        use std::os::unix::fs::PermissionsExt;
        for above in [false, true] {
            let (dir, mut p) = tree_fixture();
            write(
                &dir,
                "project.toml",
                "spaces = [\"main\"]\n\n[space.main]\norder = [\"health-copy\", \"users/\", \"users/login\", \"users/v1/\", \"users/v1/list\", \"users/v1/get\", \"health\", \"orders/\", \"orders/new\"]\n",
            );
            let users = dir.path().join("requests/main/users");
            std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o000)).unwrap();
            let copy = p.duplicate_request_at("main/health", "main/health", above);
            std::fs::set_permissions(&users, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(copy.unwrap(), "main/health-copy");
            let order = main_order(&p);
            let beside: &[&str] =
                if above { &["health-copy", "health"] } else { &["health", "health-copy"] };
            assert_eq!(order[5..7], *beside, "above: {above}: {order:?}");
            assert_eq!(order.iter().filter(|e| *e == "health-copy").count(), 1, "{order:?}");
        }
    }

    /// D4 over D7, for a same-level rename: a stale entry of the new name
    /// (one a partial walk kept) drops out, and the request keeps its own
    /// slot rather than taking the stale one's.
    #[cfg(unix)]
    #[test]
    fn a_rename_onto_a_stale_entry_of_its_name_keeps_its_place() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = folder_tree(&[
            ("requests/main/b.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/locked/y.toml", "method = \"GET\"\nurl = \"u\"\n"),
            (
                "project.toml",
                "spaces = [\"main\"]\n[space.main]\norder = [\"zed\", \"b\", \"a\", \"locked/\", \"locked/y\"]\n",
            ),
        ]);
        let locked = dir.path().join("requests/main/locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let renamed = p.rename_request("main/a", "Zed");
        let warned = p.listing_warnings().iter().any(|w| w.file.is_none());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(warned, "the walk failed part way");
        assert_eq!(renamed.unwrap().0, "main/zed");
        assert_eq!(main_order(&p), ["b", "zed", "locked/", "locked/y"]);
    }

    /// `files` moved all from `main` to `auth` while `auth/locked` can't be
    /// read, so the walk fails part way (D7): `auth`'s order after it.
    #[cfg(unix)]
    fn move_all_on_a_partial_walk(files: &[(&str, &str)]) -> Vec<String> {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = folder_tree(files);
        let locked = dir.path().join("requests/auth/locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let moved = p.move_all_requests("main", "auth").map(|_| ());
        let warned = p.listing_warnings().iter().any(|w| w.file.is_none());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(warned, "the walk failed part way");
        moved.unwrap();
        order::space_order(p.meta(), "auth").to_vec()
    }

    /// D4 over D7, for a Move all: a folder the move carries into `to`
    /// lands last in its level, and stale entries of its name and under it
    /// (ones a partial walk kept) drop out rather than lend it their slot.
    #[cfg(unix)]
    #[test]
    fn move_all_carries_a_folder_past_a_stale_entry_of_its_name() {
        let order = move_all_on_a_partial_walk(&[
            ("requests/main/c.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/auth/k.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/auth/locked/y.toml", "method = \"GET\"\nurl = \"u\"\n"),
            (
                "project.toml",
                "spaces = [\"main\", \"auth\"]\n\
                 [space.main]\norder = [\"a\", \"c\", \"f/\", \"f/x\"]\n\
                 [space.auth]\norder = [\"f/\", \"f/old\", \"k\", \"locked/\", \"locked/y\"]\n",
            ),
        ]);
        assert_eq!(order, ["k", "locked/", "locked/y", "a", "c", "f/", "f/x"]);
    }

    /// The same for a nested carried folder, and for a stale entry naming a
    /// request the move brings: each item the move carries is listed once,
    /// in the order it drew in `from`, and no stale entry under a carried
    /// folder stays.
    #[cfg(unix)]
    #[test]
    fn move_all_drops_stale_entries_under_a_nested_carried_folder() {
        let order = move_all_on_a_partial_walk(&[
            ("requests/main/c.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/f/g/y.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/auth/k.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/auth/locked/y.toml", "method = \"GET\"\nurl = \"u\"\n"),
            (
                "project.toml",
                "spaces = [\"main\", \"auth\"]\n\
                 [space.main]\norder = [\"a\", \"c\", \"f/\", \"f/x\", \"f/g/\", \"f/g/y\"]\n\
                 [space.auth]\norder = [\"f/g/y\", \"f/\", \"f/x\", \"f/g/\", \"f/g/old\", \
                 \"f/old\", \"k\", \"locked/\", \"locked/y\"]\n",
            ),
        ]);
        assert_eq!(order, ["k", "locked/", "locked/y", "a", "c", "f/", "f/x", "f/g/", "f/g/y"]);
    }

    /// D5a (amended 2026-11-03): the lists are built from `project.toml` as
    /// it is on disk, so an outside edit to `order` postui hasn't read yet
    /// (a `git checkout`) is kept, and the poll that reads the rest of it
    /// is still due.
    #[test]
    fn complete_order_keeps_an_outside_edit_not_yet_read() {
        let (dir, mut p) = ordered_fixture();
        write(
            &dir,
            "project.toml",
            "spaces = [\"main\"]\n\n[space.main]\n# reordered\norder = [\"c\", \"a\", \"b\"]\n",
        );
        write(&dir, "requests/main/d.toml", "method = \"GET\"\nurl = \"u\"\n");
        assert!(p.complete_order().unwrap());
        assert_eq!(main_order(&p), ["c", "a", "b", "d"]);
        let text = read(&dir, "project.toml").unwrap();
        assert!(text.contains("# reordered"), "{text}");
        assert!(p.changed_on_disk(), "the poll still re-reads the outside edit");
    }

    /// Nothing to write and nothing new in the lists: the file is not
    /// read, so an outside edit elsewhere in it still reaches the poll.
    #[test]
    fn complete_order_with_nothing_to_do_leaves_an_outside_edit_to_the_poll() {
        let (dir, mut p) = ordered_fixture();
        write(
            &dir,
            "project.toml",
            "spaces = [\"main\"]\n# a comment\n\n[space.main]\norder = [\"a\", \"b\", \"c\"]\n",
        );
        assert!(!p.complete_order().unwrap());
        assert!(p.changed_on_disk());
    }

    /// Runs `op`, then checks that undo puts `project.toml` back byte for
    /// byte, completion and all, and that redo writes it again.
    fn undo_redo_exact(dir: &tempfile::TempDir, p: &mut Project, op: impl FnOnce(&mut Project)) {
        let before = read(dir, "project.toml");
        op(p);
        assert_eq!(p.journal_len(), 1, "one undo step");
        let after = read(dir, "project.toml");
        assert_ne!(after, before);
        p.undo().unwrap().unwrap();
        assert_eq!(read(dir, "project.toml"), before, "undo restores project.toml");
        p.redo().unwrap().unwrap();
        assert_eq!(read(dir, "project.toml"), after, "redo writes it again");
    }

    /// Spaces `main` (`a`, `b`) and `auth` (`z`), neither with a list.
    fn two_space_unordered() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\", \"auth\"]\n").unwrap();
        for slug in ["main/a", "main/b", "auth/z"] {
            write(&dir, &format!("requests/{slug}.toml"), "method = \"GET\"\nurl = \"u\"\n");
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    /// D5: an op on a list naming nothing completes it first, in its own
    /// undo step, so a create lands last (D4), not first.
    #[test]
    fn a_create_on_a_list_naming_nothing_lands_last() {
        let (dir, mut p) = unordered_fixture();
        undo_redo_exact(&dir, &mut p, |p| {
            p.create_request("main/Aaa", req("u")).unwrap();
        });
        assert_eq!(main_order(&p), ["a", "b", "c", "aaa"]);
    }

    /// D5: a file added outside since the last re-list is listed by the
    /// op's own completion, so a create lands after it.
    #[test]
    fn a_create_after_a_file_added_outside_lands_after_it() {
        let (dir, mut p) = ordered_fixture();
        write(&dir, "requests/main/zz.toml", "method = \"GET\"\nurl = \"u\"\n");
        undo_redo_exact(&dir, &mut p, |p| {
            p.create_request("main/New", req("u")).unwrap();
        });
        assert_eq!(main_order(&p), ["a", "b", "c", "zz", "new"]);
    }

    /// D5a: an op builds on an outside reorder postui hasn't read yet.
    #[test]
    fn an_op_keeps_an_outside_reorder_not_yet_read() {
        let (dir, mut p) = ordered_fixture();
        write(
            &dir,
            "project.toml",
            "spaces = [\"main\"]\n\n[space.main]\n# reordered\norder = [\"c\", \"a\", \"b\"]\n",
        );
        p.create_request("main/New", req("u")).unwrap();
        assert_eq!(main_order(&p), ["c", "a", "b", "new"]);
        assert!(p.changed_on_disk(), "the poll still re-reads the outside edit");
    }

    /// D5: a same-level rename keeps its place on a list naming nothing.
    #[test]
    fn a_rename_on_a_list_naming_nothing_keeps_its_place() {
        let (dir, mut p) = unordered_fixture();
        undo_redo_exact(&dir, &mut p, |p| {
            p.rename_request("main/b", "Zed").unwrap();
        });
        assert_eq!(main_order(&p), ["a", "zed", "c"]);
    }

    /// D5: a put on a list naming nothing finds its anchor, below or above.
    #[test]
    fn a_put_on_a_list_naming_nothing_lands_beside_its_anchor() {
        let (dir, mut p) = unordered_fixture();
        undo_redo_exact(&dir, &mut p, |p| {
            p.duplicate_request_at("main/c", "main/a", false).unwrap();
        });
        assert_eq!(main_order(&p), ["a", "c-copy", "b", "c"]);
        let (dir, mut p) = unordered_fixture();
        undo_redo_exact(&dir, &mut p, |p| {
            p.duplicate_request_at("main/c", "main/a", true).unwrap();
        });
        assert_eq!(main_order(&p), ["c-copy", "a", "b", "c"]);
        let (dir, mut p) = unordered_fixture();
        undo_redo_exact(&dir, &mut p, |p| {
            p.duplicate_requests_at("main/c", "main/a", false, 2).unwrap();
        });
        assert_eq!(main_order(&p), ["a", "c-copy", "c-copy-2", "b", "c"]);
    }

    /// D5: a delete on a list naming nothing writes the rest complete.
    #[test]
    fn a_delete_on_a_list_naming_nothing_completes_it() {
        let (dir, mut p) = unordered_fixture();
        undo_redo_exact(&dir, &mut p, |p| p.delete_request("main/b").unwrap());
        assert_eq!(main_order(&p), ["a", "c"]);
        let (dir, mut p) = unordered_fixture();
        undo_redo_exact(&dir, &mut p, |p| {
            p.delete_requests(&["main/a".into(), "main/c".into()]).unwrap()
        });
        assert_eq!(main_order(&p), ["b"]);
    }

    /// D5: a request dropped in a folder on a list naming nothing lands at
    /// its slot, above the item it was dropped on.
    #[test]
    fn a_request_drop_on_a_list_naming_nothing_lands_at_its_slot() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\"]\n").unwrap();
        for slug in ["main/users/login", "main/users/x", "main/health"] {
            write(&dir, &format!("requests/{slug}.toml"), "method = \"GET\"\nurl = \"u\"\n");
        }
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        // `users`' items before the move: login, x; slot 1 is between.
        undo_redo_exact(&dir, &mut p, |p| {
            p.move_to_level(&req_item("main/health"), "users", 1).unwrap();
        });
        assert_eq!(main_order(&p), ["users/", "users/login", "users/health", "users/x"]);
    }

    /// D5: a folder dropped in a folder on a list naming nothing lands at
    /// its slot too.
    #[test]
    fn a_folder_drop_on_a_list_naming_nothing_lands_at_its_slot() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.toml"), "spaces = [\"main\"]\n").unwrap();
        for slug in ["main/users/login", "main/users/x", "main/v1/get"] {
            write(&dir, &format!("requests/{slug}.toml"), "method = \"GET\"\nurl = \"u\"\n");
        }
        let (mut p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        undo_redo_exact(&dir, &mut p, |p| {
            p.move_to_level(&folder_item("main/v1"), "users", 1).unwrap();
        });
        assert_eq!(
            main_order(&p),
            ["users/", "users/login", "users/v1/", "users/x", "users/v1/get"]
        );
    }

    /// D5, D4: Move to space and Move all land last in a list naming
    /// nothing.
    #[test]
    fn moves_to_a_space_naming_nothing_land_last() {
        let (dir, mut p) = two_space_unordered();
        undo_redo_exact(&dir, &mut p, |p| {
            p.move_request("main/a", "auth").unwrap();
        });
        assert_eq!(order::space_order(p.meta(), "auth"), ["z", "a"]);
        assert_eq!(main_order(&p), ["b"]);
        let (dir, mut p) = two_space_unordered();
        undo_redo_exact(&dir, &mut p, |p| {
            p.move_all_requests("main", "auth").unwrap();
        });
        assert_eq!(order::space_order(p.meta(), "auth"), ["z", "a", "b"]);
        assert!(main_order(&p).is_empty());
    }

    /// A reorder on a list naming nothing writes its level as the sidebar
    /// drew it, unlisted items included, so that level ends complete, a
    /// drag and a keyboard move alike. It does not complete the rest of
    /// the list (the folder's inside is left to the refresh write): the
    /// reorders are the ops that don't (spec 2026-11-02 drag into folders
    /// §3, `Project`, new round 1; replaces "merges into the completed
    /// list").
    #[test]
    fn a_reorder_on_a_list_naming_nothing_writes_its_level_as_drawn() {
        let (dir, mut p) = unordered_fixture();
        write(&dir, "requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        let shown: Vec<String> = ["c", "a", "b", "f/"].iter().map(|s| s.to_string()).collect();
        undo_redo_exact(&dir, &mut p, |p| {
            p.set_request_order("main", "", &shown).unwrap();
        });
        assert_eq!(main_order(&p), ["c", "a", "b", "f/"]);
        let (dir, mut p) = unordered_fixture();
        let shown: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        undo_redo_exact(&dir, &mut p, |p| {
            p.move_request_shown("main", "", &shown, "c", -1).unwrap();
        });
        assert_eq!(main_order(&p), ["a", "c", "b"]);
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

    /// A `project.toml` that doesn't parse is never written over, and
    /// completing the order never fails an op that writes no order of its
    /// own: a rename that only changes the display name still goes through.
    #[test]
    fn a_broken_project_toml_is_left_alone_and_a_name_only_rename_still_works() {
        let (dir, mut p) = tree_fixture();
        write(&dir, "project.toml", "spaces = [");
        assert!(!p.complete_order().unwrap());
        assert_eq!(read(&dir, "project.toml").as_deref(), Some("spaces = ["));
        let (slug, _) = p.rename_request("main/health", "HEALTH").unwrap();
        assert_eq!(slug, "main/health");
        assert_eq!(read(&dir, "project.toml").as_deref(), Some("spaces = ["));
    }

    /// D5 (spec 2026-11-02 drag into folders §3, `Project`): completing
    /// the list inside an op is best effort. With `project.toml`
    /// unwritable (the project folder read-only, so no atomic write lands)
    /// and a file added by hand, so the list misses it, an op with no
    /// order write of its own (a name-only rename) still goes through.
    #[cfg(unix)]
    #[test]
    fn a_completion_that_cannot_be_written_never_fails_an_op() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/zeta.toml", "method = \"GET\"\nurl = \"u\"\n");
        let before = read(&dir, "project.toml");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let renamed = p.rename_request("main/health", "HEALTH");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let (slug, leaf) = renamed.unwrap();
        assert_eq!((slug.as_str(), leaf.as_str()), ("main/health", "HEALTH"));
        let text = read(&dir, "requests/main/health.toml").unwrap();
        assert!(text.contains("name = \"HEALTH\""), "{text}");
        assert_eq!(read(&dir, "project.toml"), before, "the list stays as it was");
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

    /// D4: in a completed list a curl import's create lands last, a new
    /// folder on its path with it; a rename keeps its place.
    #[test]
    fn after_completion_arrivals_land_last_and_a_rename_keeps_its_place() {
        let (_dir, mut p) = unordered_fixture();
        p.complete_order().unwrap();
        p.create_request("main/Aaa", req("u")).unwrap();
        assert_eq!(main_order(&p), ["a", "b", "c", "aaa"]);
        p.rename_request("main/b", "Bee").unwrap();
        assert_eq!(main_order(&p), ["a", "bee", "c", "aaa"]);
        p.create_request("main/f/A", req("u")).unwrap();
        assert_eq!(main_order(&p), ["a", "bee", "c", "aaa", "f/", "f/a"]);
    }

    /// D17: a request moves into a folder at the slot it was dropped on,
    /// its file and text untouched, as one undo step.
    #[test]
    fn a_request_moves_into_a_folder_at_its_slot() {
        let (dir, mut p) = tree_fixture();
        let text = read(&dir, "requests/main/health.toml");
        // `users`' items before the move: login, v1/; slot 1 is between.
        let (new, moves) = p.move_to_level(&req_item("main/health"), "users", 1).unwrap();
        assert_eq!(new, "main/users/health");
        assert_eq!(moves, [("main/health".to_string(), new.clone())], "the move it made");
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
        let (new, _) = p.move_to_level(&req_item("main/users/v1/get"), "", 0).unwrap();
        assert_eq!(new, "main/get");
        // The top level now: get, users/, health, orders/; 4 is the end.
        let (new, _) = p.move_to_level(&req_item("main/users/login"), "", 4).unwrap();
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
    /// it, and the emptied folder stays with its entry (spec 2026-11-03
    /// create folders F2, replacing 2026-11-01 R9).
    #[test]
    fn a_request_leaving_its_folder_empty_keeps_the_folder_and_its_slot() {
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/zeta.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        // The top level before: users/, health, orders/, zeta; slot 3 is
        // below orders/, above zeta.
        let (new, _) = p.move_to_level(&req_item("main/orders/new"), "", 3).unwrap();
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
                "orders/",
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
        let (new, _) = p.move_to_level(&req_item("main/users/v1/get"), "users", 0).unwrap();
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
        let (new, _) = p.move_to_level(&req_item("main/slashy"), "users", 0).unwrap();
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
        let (new, _) = p.move_to_level(&folder_item("main/orders"), "users", 0).unwrap();
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
        let (new, mut moves) = p.move_to_level(&folder_item("main/users/v1"), "", 2).unwrap();
        assert_eq!(new, "main/v1");
        moves.sort();
        assert_eq!(
            moves,
            [
                ("main/users/v1/get".to_string(), "main/v1/get".to_string()),
                ("main/users/v1/list".to_string(), "main/v1/list".to_string()),
            ],
            "every request it carried"
        );
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
            order::displayed_slugs(p.requests(), p.folders(), &main_order(&p), "main"),
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

    /// Spec 2026-11-03 create folders F2 (replacing R9): a folder whose
    /// move empties its parent leaves the parent and its entry in place.
    #[test]
    fn a_folder_move_that_empties_its_parent_keeps_the_parent() {
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/deep/inner/x.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        assert_eq!(main_order(&p)[8..], ["deep/", "deep/inner/", "deep/inner/x"]);
        p.move_to_level(&folder_item("main/deep/inner"), "", 0).unwrap();
        assert_eq!(main_order(&p)[..1], ["inner/"]);
        assert_eq!(main_order(&p)[9..], ["deep/", "inner/x"]);
        assert!(p.folders().iter().any(|f| f.path == "main/deep"));
    }

    /// F1: an empty folder moves like any folder, and into an empty one.
    #[test]
    fn an_empty_folder_moves_into_another_empty_folder() {
        let (dir, mut p) = tree_fixture();
        std::fs::create_dir_all(dir.path().join("requests/main/e")).unwrap();
        std::fs::create_dir_all(dir.path().join("requests/main/f")).unwrap();
        p.relist();
        let (new, moves) = p.move_to_level(&folder_item("main/e"), "f", 0).unwrap();
        assert_eq!(new, "main/f/e");
        assert!(moves.is_empty());
        assert!(dir.path().join("requests/main/f/e").is_dir());
        assert!(main_order(&p).ends_with(&["f/".to_string(), "f/e/".to_string()]));
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

    /// D18 (amended 2026-11-03, replacing "a leftover empty directory goes
    /// to the trash with the move"): anything of the folder's name on disk
    /// at the destination that shows no folder refuses, naming the path,
    /// whatever it holds: an empty directory left behind, nested empty
    /// directories, one holding only a `.DS_Store`, a plain file. Nothing
    /// moves and nothing is journaled. *Amended 2026-11-03 (spec create
    /// folders F3, F8):* a directory is a folder now, shown even when
    /// empty, so the first three clash as a shown folder. *Amended
    /// 2026-11-06 (#255):* the plain file only takes the directory name,
    /// so the drop is numbered past it.
    #[test]
    fn a_folder_move_onto_a_leftover_directory_clashes_as_a_shown_folder() {
        type Leftover = fn(&tempfile::TempDir);
        let cases: [(&str, Leftover); 3] = [
            ("an empty directory", |d| {
                std::fs::create_dir_all(d.path().join("requests/main/v1")).unwrap()
            }),
            ("nested empty directories", |d| {
                std::fs::create_dir_all(d.path().join("requests/main/v1/sub/deeper")).unwrap()
            }),
            ("a .DS_Store", |d| write(d, "requests/main/v1/.DS_Store", "x")),
        ];
        for (what, make) in cases {
            let (dir, mut p) = tree_fixture();
            make(&dir);
            let before = read(&dir, "project.toml");
            let r = p.move_to_level(&folder_item("main/users/v1"), "", 0);
            assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "v1"), "{what}: {r:?}");
            assert!(dir.path().join("requests/main/users/v1/get.toml").is_file(), "{what}");
            assert!(dir.path().join("requests/main/v1").exists(), "{what}: left as it was");
            assert_eq!(read(&dir, "project.toml"), before, "{what}");
            assert_eq!(p.journal_len(), 0, "{what}");
        }
        let (dir, mut p) = tree_fixture();
        write(&dir, "requests/main/v1", "notes");
        let (new, _) = p.move_to_level(&folder_item("main/users/v1"), "", 0).unwrap();
        assert_eq!(new, "main/v1-2");
        assert_eq!(read(&dir, "requests/main/v1").as_deref(), Some("notes"));
        assert_eq!(
            read(&dir, "requests/main/v1-2/.folder.toml").as_deref(),
            Some("name = \"v1\"\n")
        );
    }

    /// D18: a shown folder whose name differs only in case clashes even
    /// where no directory of the dragged folder's exact spelling exists
    /// (Linux: `Users` is shown, `users` is not on disk).
    #[test]
    fn a_folder_move_onto_a_shown_folder_in_another_case_is_refused() {
        let (dir, mut p) = fixture();
        write(&dir, "requests/main/Users/a.toml", "method = \"GET\"\nurl = \"u\"\n");
        write(&dir, "requests/main/x/users/b.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        let before = read(&dir, "project.toml");
        let r = p.move_to_level(&folder_item("main/x/users"), "", 0);
        assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "Users"), "{r:?}");
        assert!(dir.path().join("requests/main/x/users/b.toml").is_file());
        assert_eq!(read(&dir, "project.toml"), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// D18 (amended 2026-11-06, #255): a directory taken by a folder that
    /// shows another name is numbered, as a create numbers it (F19), and
    /// the moved folder gets a `.folder.toml` naming it as it showed. Undo
    /// takes the move and the file back.
    #[test]
    fn a_folder_drop_numbers_a_directory_another_name_has() {
        let (dir, mut p) = fixture();
        write(&dir, "requests/main/users/.folder.toml", "name = \"Accounts\"\n");
        write(&dir, "requests/main/users/a.toml", "method = \"GET\"\nurl = \"u\"\n");
        write(&dir, "requests/main/x/users/b.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        p.complete_order().unwrap();
        let (before, tree) = (read(&dir, "project.toml"), requests_tree(&dir));
        let (new, moves) = p.move_to_level(&folder_item("main/x/users"), "", 0).unwrap();
        assert_eq!(new, "main/users-2");
        assert_eq!(moves, [("main/x/users/b".to_string(), "main/users-2/b".to_string())]);
        assert!(dir.path().join("requests/main/users-2/b.toml").is_file());
        assert_eq!(
            read(&dir, "requests/main/users-2/.folder.toml").as_deref(),
            Some("name = \"users\"\n")
        );
        assert_eq!(
            read(&dir, "requests/main/users/.folder.toml").as_deref(),
            Some("name = \"Accounts\"\n")
        );
        assert!(
            p.folders().iter().any(|f| f.path == "main/users-2" && f.display_name() == "users")
        );
        assert_eq!(main_order(&p), ["users-2/", "ping", "users/", "users/a", "x/", "users-2/b"]);
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(&dir), tree);
        assert_eq!(read(&dir, "project.toml"), before);
    }

    /// #255's steps: "Ping!" in main has directory `ping`; "Ping" dragged
    /// in from main/x moves to `ping-2`, its own `.folder.toml` untouched.
    #[test]
    fn a_folder_drop_keeps_a_named_folder_file_when_numbered() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/ping/.folder.toml", "name = \"Ping!\"\n"),
            ("requests/main/x/ping/.folder.toml", "name=\"Ping\"  # kept\n"),
        ]);
        let (new, _) = p.move_to_level(&folder_item("main/x/ping"), "", 0).unwrap();
        assert_eq!(new, "main/ping-2");
        assert_eq!(
            read(&dir, "requests/main/ping-2/.folder.toml").as_deref(),
            Some("name=\"Ping\"  # kept\n")
        );
    }

    /// A numbered folder whose file has other keys but no name gets the
    /// name, its other keys kept.
    #[test]
    fn a_folder_drop_names_a_numbered_folder_in_its_file_keeping_other_keys() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/f/.folder.toml", "name = \"Other\"\n"),
            ("requests/main/x/f/.folder.toml", "color = \"red\"\n"),
        ]);
        let (new, _) = p.move_to_level(&folder_item("main/x/f"), "", 0).unwrap();
        assert_eq!(new, "main/f-2");
        assert_eq!(
            read(&dir, "requests/main/f-2/.folder.toml").as_deref(),
            Some("color = \"red\"\nname = \"f\"\n")
        );
    }

    /// A folder file that doesn't parse can't be given the name a numbered
    /// directory needs, so that drop is refused, as a rename is (F24).
    #[test]
    fn a_folder_drop_that_needs_a_number_refuses_a_broken_folder_file() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/f/.folder.toml", "name = \"Other\"\n"),
            ("requests/main/x/f/.folder.toml", "name = "),
        ]);
        let before = requests_tree(&dir);
        let r = p.move_to_level(&folder_item("main/x/f"), "", 0);
        assert!(
            matches!(&r, Err(Error::BadName(m)) if m.starts_with("requests/main/x/f/.folder.toml")),
            "{r:?}"
        );
        assert_eq!(requests_tree(&dir), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// A listed folder spelled in another case takes the directory too
    /// (a case-insensitive disk would file into it): numbered.
    #[test]
    fn a_folder_drop_numbers_past_a_listed_case_twin() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/Users/.folder.toml", "name = \"Accounts\"\n"),
            ("requests/main/x/users/b.toml", "method = \"GET\"\nurl = \"u\"\n"),
        ]);
        let (new, _) = p.move_to_level(&folder_item("main/x/users"), "", 0).unwrap();
        assert_eq!(new, "main/users-2");
        assert!(dir.path().join("requests/main/users-2/b.toml").is_file());
    }

    /// A taken directory is named from the name the folder shows, by the
    /// rule a create uses (F19), not numbered onto its own directory name:
    /// `a-2` showing "a" goes to `a` when that is free, else `a-3`.
    #[test]
    fn a_folder_drop_onto_a_taken_directory_names_it_from_its_shown_name() {
        let (_dir, mut p) = folder_tree(&[
            ("requests/main/a-2/.folder.toml", "name = \"b\"\n"),
            ("requests/main/box/a-2/.folder.toml", "name = \"a\"\n"),
        ]);
        let (new, _) = p.move_to_level(&folder_item("main/box/a-2"), "", 0).unwrap();
        assert_eq!(new, "main/a");
        let (_dir, mut p) = folder_tree(&[
            ("requests/main/a/.folder.toml", "name = \"c\"\n"),
            ("requests/main/a-2/.folder.toml", "name = \"b\"\n"),
            ("requests/main/box/a-2/.folder.toml", "name = \"a\"\n"),
        ]);
        let (new, _) = p.move_to_level(&folder_item("main/box/a-2"), "", 0).unwrap();
        assert_eq!(new, "main/a-3");
    }

    /// A `name` the listing can't show (holding `/`, or blank) leaves the
    /// folder showing its directory name, so a drop that renames the
    /// directory writes that name over it, as a rename would: the folder
    /// still shows "users", never `users-2`, which a sibling shows here.
    #[test]
    fn a_folder_drop_names_a_folder_whose_name_it_cannot_show() {
        for hand_written in ["name = \"a/b\"\n", "name = \"  \"\n"] {
            let (dir, mut p) = folder_tree(&[
                ("requests/main/users/.folder.toml", "name = \"Accounts\"\n"),
                ("requests/main/zzz/.folder.toml", "name = \"users-2\"\n"),
                ("requests/main/x/users/.folder.toml", hand_written),
            ]);
            let (new, _) = p.move_to_level(&folder_item("main/x/users"), "", 0).unwrap();
            assert_eq!(new, "main/users-2", "{hand_written}");
            assert_eq!(
                read(&dir, "requests/main/users-2/.folder.toml").as_deref(),
                Some("name = \"users\"\n"),
                "{hand_written}"
            );
            assert!(
                p.folders().iter().any(|f| f.path == "main/users-2" && f.display_name() == "users"),
                "{hand_written}"
            );
        }
    }

    /// D18: a sibling showing the dragged folder's name from a directory
    /// of another name refuses the drop, though the directory is free.
    #[test]
    fn a_folder_drop_is_refused_by_a_sibling_showing_its_name_from_another_directory() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/acc/.folder.toml", "name = \"Users\"\n"),
            ("requests/main/x/users/b.toml", "method = \"GET\"\nurl = \"u\"\n"),
        ]);
        let (before, tree) = (read(&dir, "project.toml"), requests_tree(&dir));
        let r = p.move_to_level(&folder_item("main/x/users"), "", 0);
        assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "Users"), "{r:?}");
        assert_eq!(requests_tree(&dir), tree);
        assert_eq!(read(&dir, "project.toml"), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// A folder file that can't be read fails a drop that would rename the
    /// directory, and the move is rolled back.
    #[cfg(unix)]
    #[test]
    fn a_folder_drop_that_cannot_read_its_folder_file_moves_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = folder_tree(&[
            ("requests/main/f/.folder.toml", "name = \"Other\"\n"),
            ("requests/main/x/f/b.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/x/f/.folder.toml", "color = \"red\"\n"),
        ]);
        let before = read(&dir, "project.toml");
        let file = dir.path().join("requests/main/x/f/.folder.toml");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = p.move_to_level(&folder_item("main/x/f"), "", 0);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).ok();
        assert!(r.is_err(), "{r:?}");
        assert!(dir.path().join("requests/main/x/f/b.toml").is_file());
        assert!(!dir.path().join("requests/main/f-2").exists());
        assert_eq!(read(&dir, "project.toml"), before);
        assert_eq!(p.journal_len(), 0);
    }

    /// D18 against the disk as it is: a request of the clashing name
    /// written outside since the last relist still refuses the drop.
    #[test]
    fn a_request_move_onto_a_name_written_since_the_last_relist_is_refused() {
        let (dir, mut p) = tree_fixture();
        write(
            &dir,
            "requests/main/users/hc.toml",
            "name = \"HEALTH\"\nmethod = \"GET\"\nurl = \"u\"\n",
        );
        let r = p.move_to_level(&req_item("main/health"), "users", 0);
        assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "Health"), "{r:?}");
        assert!(dir.path().join("requests/main/health.toml").is_file());
        assert!(!dir.path().join("requests/main/users/health.toml").exists());
        assert_eq!(p.journal_len(), 0);
    }

    /// D17: `at` counts the rows as the sidebar drew them, from the
    /// listing before the move relists, so a request removed outside
    /// above the slot doesn't shift the drop.
    #[test]
    fn a_drop_slot_counts_the_rows_as_drawn_before_an_outside_removal() {
        let (dir, mut p) = tree_fixture();
        std::fs::remove_file(dir.path().join("requests/main/health.toml")).unwrap();
        // The top level as drawn: users/, health, orders/; slot 2 is
        // above orders/.
        let (new, _) = p.move_to_level(&req_item("main/users/login"), "", 2).unwrap();
        assert_eq!(new, "main/login");
        assert_eq!(
            main_order(&p),
            [
                "users/",
                "users/v1/",
                "users/v1/list",
                "users/v1/get",
                "login",
                "orders/",
                "orders/new"
            ]
        );
    }

    /// D17 over D7: entries already under a moved folder's new path (a
    /// partial walk kept them) name nothing, since the new path is one
    /// nothing on disk has, so they go: the moved contents keep their own
    /// order and appear once.
    #[cfg(unix)]
    #[test]
    fn a_folder_move_drops_stale_entries_under_its_new_path() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = tree_fixture();
        write(
            &dir,
            "project.toml",
            "spaces = [\"main\"]\n\n[space.main]\norder = [\"v1/get\", \"users/\", \"users/login\", \"users/v1/\", \"users/v1/list\", \"users/v1/get\", \"health\", \"v1/\", \"orders/\", \"orders/new\"]\n",
        );
        let orders = dir.path().join("requests/main/orders");
        std::fs::set_permissions(&orders, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = p.move_to_level(&folder_item("main/users/v1"), "", 0);
        std::fs::set_permissions(&orders, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.unwrap().0, "main/v1");
        assert_eq!(
            main_order(&p),
            [
                "v1/",
                "users/",
                "users/login",
                "v1/list",
                "v1/get",
                "health",
                "orders/",
                "orders/new"
            ]
        );
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

    /// A bare directory with requests and no `project.toml`.
    fn bare_dir() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        for slug in ["main/a", "main/b", "main/f/x", "auth/z"] {
            write(&dir, &format!("requests/{slug}.toml"), "method = \"GET\"\nurl = \"u\"\n");
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    /// No op ever creates a `project.toml` (spec 2026-11-02 drag into
    /// folders §3, `Project`, new round 2, replacing main's reorders and
    /// puts creating one): in a bare directory (the TUI opens one when a
    /// registered project lost its `project.toml`) a create, a rename, Move
    /// to space, a put and a drop in another folder do their
    /// file part and write no order. The two reorders have nothing but an
    /// order to write, so they are refused, writing nothing and recording
    /// no step.
    #[test]
    fn no_op_makes_a_project_toml_and_the_reorders_are_refused() {
        let (dir, mut p) = bare_dir();
        let none = |dir: &tempfile::TempDir| !dir.path().join("project.toml").exists();
        assert!(none(&dir));
        let (slug, _) = p.create_request("main/New", req("u")).unwrap();
        assert!(p.request_exists(&slug));
        assert!(none(&dir), "a create");
        let (slug, _) = p.create_request("main/g/A", req("u")).unwrap();
        assert!(p.request_exists(&slug));
        assert!(none(&dir), "a create into a new folder");
        let (slug, _) = p.rename_request("main/a", "Ay").unwrap();
        assert!(p.request_exists(&slug));
        assert!(!dir.path().join("requests/main/a.toml").exists());
        assert!(none(&dir), "a rename");
        let slug = p.move_request("main/b", "auth").unwrap();
        assert!(p.request_exists(&slug));
        assert!(!dir.path().join("requests/main/b.toml").exists());
        assert!(none(&dir), "Move to space");
        let copy = p.duplicate_request_at("main/new", "main/new", false).unwrap();
        assert!(dir.path().join(format!("requests/{copy}.toml")).is_file());
        assert!(none(&dir), "a put");
        let (slug, _) = p.move_to_level(&req_item(&copy), "f", 0).unwrap();
        assert!(dir.path().join(format!("requests/{slug}.toml")).is_file());
        assert!(!dir.path().join(format!("requests/{copy}.toml")).exists());
        assert!(none(&dir), "a drop in another folder");
        let (slug, _) = p.move_to_level(&folder_item("main/g"), "f", 0).unwrap();
        assert_eq!(slug, "main/f/g");
        assert!(dir.path().join("requests/main/f/g").is_dir());
        assert!(none(&dir), "a folder's drop");
        assert!(main_order(&p).is_empty());

        let steps = p.journal_len();
        let refused = |r: Result<Option<meta::ListChange>, Error>| matches!(r, Err(Error::Conflict(m)) if m.contains("without a project.toml"));
        // main's top level draws `f/`, `new` (D6).
        let shown: Vec<String> = ["new", "f/"].iter().map(|s| s.to_string()).collect();
        assert!(refused(p.set_request_order("main", "", &shown)), "a drag reorder");
        assert!(refused(p.move_request_shown("main", "", &shown, "new", 1)), "Move down");
        assert!(none(&dir), "a refused reorder writes nothing");
        assert_eq!(p.journal_len(), steps, "and records no step");
        assert!(main_order(&p).is_empty());
    }

    /// D18: a shown folder whose path differs only in case is the folder a
    /// folder move clashes with.
    #[test]
    fn a_shown_folder_matches_its_path_in_any_case() {
        let folders: Vec<FolderListing> = ["main/Users", "main/usersx"]
            .iter()
            .map(|s| FolderListing { path: s.to_string(), name: None, broken: None })
            .collect();
        let found = |path| folder_ignoring_case(&folders, path).map(|f| f.path.as_str());
        assert_eq!(found("main/users"), Some("main/Users"));
        assert_eq!(found("main/Users"), Some("main/Users"));
        assert_eq!(found("main/USERSX"), Some("main/usersx"));
        assert_eq!(found("main/user"), None, "a whole segment only");
        assert_eq!(found("main/Users/login"), None, "nothing deeper");
    }

    // --- spec 2026-11-03 create folders ---

    /// A project with one space, `main`, holding `a`, and `files` written
    /// on top (paths relative to the root; a path ending in `/` is an
    /// empty directory).
    fn folder_tree(files: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "project.toml", "spaces = [\"main\"]\n");
        write(&dir, "requests/main/a.toml", "method = \"GET\"\nurl = \"u\"\n");
        for (rel, text) in files {
            if let Some(d) = rel.strip_suffix('/') {
                std::fs::create_dir_all(dir.path().join(d)).unwrap();
            } else {
                write(&dir, rel, text);
            }
        }
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        (dir, p)
    }

    fn folder_paths(p: &Project) -> Vec<String> {
        p.folders().iter().map(|f| f.path.clone()).collect()
    }

    /// F1: a directory is a folder, empty or not.
    #[test]
    fn an_empty_directory_lists_as_a_folder() {
        let (_d, p) = folder_tree(&[("requests/main/empty/", "")]);
        assert_eq!(
            p.folders(),
            [FolderListing { path: "main/empty".into(), name: None, broken: None }]
        );
        assert_eq!(p.folders()[0].display_name(), "empty");
    }

    /// F15: `.folder.toml` names its folder, and is no request.
    #[test]
    fn a_folder_toml_names_its_folder() {
        let (_d, p) =
            folder_tree(&[("requests/main/users/.folder.toml", "name = \"User Accounts\"\n")]);
        assert_eq!(p.folders()[0].name.as_deref(), Some("User Accounts"));
        assert_eq!(p.folders()[0].display_name(), "User Accounts");
        assert_eq!(slugs(&p), ["main/a"]);
    }

    /// F16: dot files and dot directories inside a space are skipped.
    #[test]
    fn dot_files_and_dot_directories_inside_a_space_are_skipped() {
        let (_d, p) = folder_tree(&[
            ("requests/main/.drafts/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/.x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/f/.DS_Store", "junk"),
        ]);
        assert_eq!(slugs(&p), ["main/a"]);
        assert_eq!(folder_paths(&p), ["main/f"]);
    }

    /// F24: a `.folder.toml` that doesn't parse leaves the folder listed
    /// under its directory name, with a warning naming the file.
    #[test]
    fn a_broken_folder_toml_lists_the_folder_with_a_warning() {
        let (_d, p) = folder_tree(&[("requests/main/f/.folder.toml", "name = ")]);
        let f = &p.folders()[0];
        assert_eq!((f.path.as_str(), f.name.as_deref()), ("main/f", None));
        assert!(f.broken.is_some());
        assert_eq!(f.display_name(), "f");
        let files: Vec<Option<&str>> =
            p.listing_warnings().iter().map(|w| w.file.as_deref()).collect();
        assert_eq!(files, [Some("requests/main/f/.folder.toml")]);
    }

    /// Review focus 2: a directory whose name isn't a slug lists by it.
    #[test]
    fn a_directory_with_a_non_slug_name_lists_by_its_name() {
        let (_d, p) = folder_tree(&[("requests/main/My Stuff/", "")]);
        assert_eq!(folder_paths(&p), ["main/My Stuff"]);
        assert_eq!(p.folders()[0].display_name(), "My Stuff");
    }

    /// `op` is one undo step; undo gives back `requests/` and
    /// `project.toml` exactly, and redo gives back what `op` made.
    fn undo_redo_tree_exact(
        dir: &tempfile::TempDir,
        p: &mut Project,
        op: impl FnOnce(&mut Project),
    ) {
        let (files, meta) = (requests_tree(dir), read(dir, "project.toml"));
        op(p);
        assert_eq!(p.journal_len(), 1, "one undo step");
        let (files_after, meta_after) = (requests_tree(dir), read(dir, "project.toml"));
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(dir), files, "undo restores requests/");
        assert_eq!(read(dir, "project.toml"), meta, "undo restores project.toml");
        p.redo().unwrap().unwrap();
        assert_eq!(requests_tree(dir), files_after, "redo makes it again");
        assert_eq!(read(dir, "project.toml"), meta_after);
    }

    /// F5, F17: a create in a new folder makes it with its `.folder.toml`,
    /// named by the typed segment, and undo leaves nothing behind.
    #[test]
    fn a_create_in_a_new_folder_makes_it_with_its_name_and_undo_leaves_nothing() {
        let (dir, mut p) = folder_tree(&[]);
        undo_redo_tree_exact(&dir, &mut p, |p| {
            let (slug, _) = p.create_request("main/User Accounts/login", req("u")).unwrap();
            assert_eq!(slug, "main/user-accounts/login");
        });
        assert_eq!(
            read(&dir, "requests/main/user-accounts/.folder.toml").as_deref(),
            Some("name = \"User Accounts\"\n")
        );
        assert_eq!(main_order(&p), ["a", "user-accounts/", "user-accounts/login"]);
        p.undo().unwrap().unwrap();
        assert!(!dir.path().join("requests/main/user-accounts").exists());
    }

    /// F17: every new ancestor gets its own file.
    #[test]
    fn a_create_in_a_new_nested_folder_makes_every_ancestor_with_its_file() {
        let (dir, mut p) = folder_tree(&[]);
        undo_redo_tree_exact(&dir, &mut p, |p| {
            p.create_request("main/Outer/Inner \"Q\"/x", req("u")).unwrap();
        });
        assert_eq!(
            read(&dir, "requests/main/outer/.folder.toml").as_deref(),
            Some("name = \"Outer\"\n")
        );
        assert_eq!(
            read(&dir, "requests/main/outer/inner-q/.folder.toml").as_deref(),
            Some("name = 'Inner \"Q\"'\n")
        );
    }

    /// F17: a create into a folder already there writes no folder file.
    #[test]
    fn a_create_in_an_existing_folder_writes_no_folder_file() {
        let (dir, mut p) = folder_tree(&[("requests/main/f/", "")]);
        p.create_request("main/f/x", req("u")).unwrap();
        assert!(!dir.path().join("requests/main/f/.folder.toml").exists());
    }

    /// F28: a rename with a `/` makes no folder, and undo is exact.
    #[test]
    fn a_rename_with_a_slash_makes_no_folder() {
        let (dir, mut p) = folder_tree(&[]);
        undo_redo_tree_exact(&dir, &mut p, |p| {
            assert_eq!(p.rename_request("main/a", "New Place/a").unwrap().0, "main/new-place-a");
        });
        assert!(!dir.path().join("requests/main/new-place").exists());
    }

    /// §3: Move to space copies the source folder's file into the folder
    /// it makes; undo is exact.
    #[test]
    fn a_move_to_space_copies_the_folder_file_into_the_folder_it_makes() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/f/.folder.toml", "name = \"Eff\"\ncolor = \"red\"\n"),
            ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/auth/", ""),
            ("project.toml", "spaces = [\"main\", \"auth\"]\n"),
        ]);
        undo_redo_tree_exact(&dir, &mut p, |p| {
            p.move_request("main/f/x", "auth").unwrap();
        });
        assert_eq!(
            read(&dir, "requests/auth/f/.folder.toml").as_deref(),
            Some("name = \"Eff\"\ncolor = \"red\"\n")
        );
    }

    /// A source folder file that can't be read doesn't stop a move: the
    /// folder made in the other space is named by its directory (review
    /// round 1), in Move to space and Move all alike.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_file_names_the_copy_by_its_directory() {
        use std::os::unix::fs::PermissionsExt;
        for all in [false, true] {
            let (dir, mut p) = folder_tree(&[
                ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
                ("requests/main/f/.folder.toml", "name = \"Eff\"\n"),
                ("requests/auth/", ""),
                ("project.toml", "spaces = [\"main\", \"auth\"]\n"),
            ]);
            let file = dir.path().join("requests/main/f/.folder.toml");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            let moved = if all {
                p.move_all_requests("main", "auth").map(|_| ())
            } else {
                p.move_request("main/f/x", "auth").map(|_| ())
            };
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).ok();
            moved.unwrap_or_else(|e| panic!("all={all}: {e}"));
            assert_eq!(
                read(&dir, "requests/auth/f/.folder.toml").as_deref(),
                Some("name = \"f\"\n"),
                "all={all}"
            );
        }
    }

    /// F18a: a typed segment matches a sibling folder by its shown name
    /// only, never its directory (the user: "There's no reason to search
    /// the name other than the display name").
    #[test]
    fn a_typed_segment_matches_only_shown_names() {
        let (_dir, mut p) = folder_tree(&[
            ("requests/main/a/.folder.toml", "name = \"B\"\n"),
            ("requests/main/b/.folder.toml", "name = \"A\"\n"),
        ]);
        assert_eq!(p.create_request("main/B/x", req("u")).unwrap().0, "main/a/x");
        assert_eq!(p.create_request("main/a/y", req("u")).unwrap().0, "main/b/y");
    }

    /// F18a, F19: a segment matching no shown name makes a new folder named
    /// by it, even when its slug is a directory showing another name: that
    /// directory is taken, so the new one is numbered.
    #[test]
    fn a_segment_whose_slug_is_taken_by_another_name_makes_a_numbered_folder() {
        let (dir, mut p) =
            folder_tree(&[("requests/main/users/.folder.toml", "name = \"Accounts\"\n")]);
        let (slug, _) = p.create_request("main/Users/login", req("u")).unwrap();
        assert_eq!(slug, "main/users-2/login");
        assert_eq!(
            read(&dir, "requests/main/users-2/.folder.toml").as_deref(),
            Some("name = \"Users\"\n")
        );
    }

    /// D18 with F19: a folder drag onto a level whose sibling already shows
    /// the dragged folder's name is refused (review round 2).
    #[test]
    fn a_folder_drag_onto_a_shown_name_is_refused() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/x/v1/.folder.toml", "name = \"Users\"\n"),
            ("requests/main/users/.folder.toml", "name = \"Users\"\n"),
        ]);
        let before = requests_tree(&dir);
        let r = p.move_to_level(&folder_item("main/x/v1"), "", 0);
        assert!(matches!(&r, Err(Error::AlreadyExists(n)) if n == "Users"), "{r:?}");
        assert_eq!(requests_tree(&dir), before);
    }

    /// A move doesn't copy a folder file that doesn't parse: the copy is
    /// named by its directory (review rounds 1 and 2).
    #[test]
    fn a_broken_folder_file_is_not_copied_by_a_move() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/f/.folder.toml", "name = "),
            ("requests/auth/", ""),
            ("project.toml", "spaces = [\"main\", \"auth\"]\n"),
        ]);
        p.move_request("main/f/x", "auth").unwrap();
        assert_eq!(read(&dir, "requests/auth/f/.folder.toml").as_deref(), Some("name = \"f\"\n"));
    }

    /// F18a: a typed segment goes into the sibling folder whose shown name
    /// it matches, in any case, even when its directory is named otherwise.
    #[test]
    fn a_typed_segment_matches_a_folder_by_its_shown_name() {
        let (_dir, mut p) = folder_tree(&[(
            "requests/main/accounts/.folder.toml",
            "name = \"Customer Accounts\"\n",
        )]);
        let (slug, _) = p.create_request("main/customer accounts/x", req("u")).unwrap();
        assert_eq!(slug, "main/accounts/x");
    }

    // --- create, rename and delete folder (F14, F18–F21, F23, F24) ---

    /// `main` holds `a`, `b` and folder `f/` holding `x`, its list complete.
    fn three_items() -> (tempfile::TempDir, Project) {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/b.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
        ]);
        p.complete_order().unwrap();
        assert_eq!(main_order(&p), ["a", "b", "f/", "f/x"]);
        (dir, p)
    }

    #[test]
    fn create_folder_makes_the_directory_and_file_and_lands_after_its_anchor() {
        let (dir, mut p) = three_items();
        let path = p.create_folder("main", "User Accounts", Some(&req_item("main/a"))).unwrap();
        assert_eq!(path, "main/user-accounts");
        assert_eq!(
            read(&dir, "requests/main/user-accounts/.folder.toml").as_deref(),
            Some("name = \"User Accounts\"\n")
        );
        assert_eq!(main_order(&p), ["a", "user-accounts/", "b", "f/", "f/x"]);
        assert_eq!(
            p.folders().iter().find(|f| f.path == path).unwrap().display_name(),
            "User Accounts"
        );
    }

    #[test]
    fn create_folder_after_a_folder_lands_after_its_contents_entry() {
        let (_dir, mut p) = three_items();
        p.create_folder("main", "G", Some(&folder_item("main/f"))).unwrap();
        assert_eq!(main_order(&p), ["a", "b", "f/", "g/", "f/x"]);
    }

    #[test]
    fn create_folder_with_no_anchor_lands_last_inside_its_level() {
        let (_dir, mut p) = three_items();
        let path = p.create_folder("main/f", "Inner", None).unwrap();
        assert_eq!(path, "main/f/inner");
        assert_eq!(main_order(&p), ["a", "b", "f/", "f/x", "f/inner/"]);
    }

    /// F19: a shown name is refused in any case; nothing is written.
    #[test]
    fn create_folder_refuses_a_shown_name_in_any_case() {
        let (dir, mut p) = three_items();
        write(&dir, "requests/main/f/.folder.toml", "name = \"Eff\"\n");
        p.relist();
        let before = requests_tree(&dir);
        assert!(
            matches!(p.create_folder("main", "EFF", None), Err(Error::AlreadyExists(n)) if n == "EFF")
        );
        assert_eq!(requests_tree(&dir), before);
        assert_eq!(p.journal_len(), 0);
        // The directory name alone is no shown name: a slug clash, numbered.
        assert_eq!(p.create_folder("main", "f", None).unwrap(), "main/f-2");
    }

    /// F19: a slug that clashes while the shown names differ is numbered.
    #[test]
    fn create_folder_numbers_a_slug_clash_with_another_shown_name() {
        let (_dir, mut p) =
            folder_tree(&[("requests/main/ping/.folder.toml", "name = \"Ping\"\n")]);
        assert_eq!(p.create_folder("main", "Ping!", None).unwrap(), "main/ping-2");
    }

    /// A space listed in `project.toml` whose directory isn't on disk (a
    /// fresh clone of a space with no file yet) takes a new request and a
    /// new folder at its top, the write making its directory.
    #[test]
    fn a_listed_space_without_its_directory_takes_creates() {
        let (dir, mut p) = folder_project(&[], &["main/a"]);
        std::fs::remove_dir(dir.path().join("requests/auth")).unwrap();
        p.relist();
        // Undo takes the space's directory back with the create.
        undo_redo_tree_exact(&dir, &mut p, |p| {
            let (slug, _) = p.create_request_in("auth", "Ping", None, req("u")).unwrap();
            assert_eq!(slug, "auth/ping");
        });
        p.undo().unwrap().unwrap();
        undo_redo_tree_exact(&dir, &mut p, |p| {
            assert_eq!(p.create_folder("auth", "Users", None).unwrap(), "auth/users");
        });
        assert!(dir.path().join("requests/auth/users/.folder.toml").is_file());
        assert!(matches!(
            p.create_request_in("nope", "x", None, req("u")),
            Err(Error::NotFound(_))
        ));
    }

    /// Every journaled write makes the directories above it journaled
    /// (`fs_make_parents`): Move to space and Move all into a space with
    /// no directory are undone exactly, its directory included.
    #[test]
    fn moves_into_a_space_without_its_directory_undo_exactly() {
        let (dir, mut p) = folder_project(&[], &["main/api/x", "main/y"]);
        std::fs::remove_dir(dir.path().join("requests/auth")).unwrap();
        p.relist();
        undo_redo_tree_exact(&dir, &mut p, |p| {
            assert_eq!(p.move_request("main/api/x", "auth").unwrap(), "auth/api/x");
        });
        p.undo().unwrap().unwrap();
        undo_redo_tree_exact(&dir, &mut p, |p| {
            p.move_all_requests("main", "auth").unwrap();
        });
    }

    #[test]
    fn create_folder_refuses_an_empty_name_and_a_missing_level() {
        let (_dir, mut p) = three_items();
        assert!(matches!(p.create_folder("main", "  ", None), Err(Error::BadName(_))));
        assert!(matches!(p.create_folder("main/nope", "x", None), Err(Error::NotFound(_))));
    }

    /// F5: create, rename and delete are each one undo step, exact.
    #[test]
    fn create_rename_and_delete_folder_undo_exactly() {
        let (dir, mut p) = three_items();
        undo_redo_tree_exact(&dir, &mut p, |p| {
            p.create_folder("main", "New", Some(&req_item("main/b"))).unwrap();
        });
        let (dir, mut p) = three_items();
        undo_redo_tree_exact(&dir, &mut p, |p| {
            p.rename_folder("main/f", "Renamed").unwrap();
        });
        let (dir, mut p) = three_items();
        undo_redo_tree_exact(&dir, &mut p, |p| {
            p.delete_folder("main/f").unwrap();
        });
    }

    /// F21, F23: a rename renames the directory and the file, keeps the
    /// folder's slot, and its contents' entries follow in place.
    #[test]
    fn rename_folder_renames_directory_and_file_and_keeps_its_slot() {
        let (dir, mut p) = three_items();
        p.complete_order().unwrap();
        let (path, moves) = p.rename_folder("main/f", "Customers").unwrap();
        assert_eq!(path, "main/customers");
        assert_eq!(moves, [("main/f/x".to_string(), "main/customers/x".to_string())]);
        assert!(dir.path().join("requests/main/customers/x.toml").is_file());
        assert!(!dir.path().join("requests/main/f").exists());
        assert_eq!(
            read(&dir, "requests/main/customers/.folder.toml").as_deref(),
            Some("name = \"Customers\"\n"),
            "F20: a folder without the file gets one"
        );
        assert_eq!(main_order(&p), ["a", "b", "customers/", "customers/x"]);
    }

    /// A rename that keeps the slug only rewrites the file, keeping its
    /// other keys (F24).
    #[test]
    fn rename_folder_same_slug_writes_only_the_file_and_keeps_other_keys() {
        let (dir, mut p) = three_items();
        write(&dir, "requests/main/f/.folder.toml", "name = \"Eff\"\ncolor = \"red\"\n");
        p.relist();
        let (path, moves) = p.rename_folder("main/f", "F").unwrap();
        assert_eq!(path, "main/f");
        assert!(moves.is_empty());
        assert_eq!(
            read(&dir, "requests/main/f/.folder.toml").as_deref(),
            Some("name = \"F\"\ncolor = \"red\"\n")
        );
    }

    /// F24: a broken file refuses the rename, and nothing is written.
    #[test]
    fn rename_folder_refuses_a_broken_file_and_writes_nothing() {
        let (dir, mut p) = three_items();
        write(&dir, "requests/main/f/.folder.toml", "name = ");
        p.relist();
        let before = requests_tree(&dir);
        assert!(matches!(p.rename_folder("main/f", "G"), Err(Error::BadName(_))));
        assert_eq!(requests_tree(&dir), before);
    }

    /// F19: a name a sibling shows is refused; a name whose slug something
    /// on disk has (a plain file) numbers, as a create does (review block
    /// 1's one naming rule, replacing "refused, naming the path").
    #[test]
    fn rename_folder_refuses_a_shown_name_and_numbers_a_taken_slug() {
        let (dir, mut p) = three_items();
        write(&dir, "requests/main/g", "notes");
        std::fs::create_dir_all(dir.path().join("requests/main/h")).unwrap();
        p.relist();
        assert!(matches!(p.rename_folder("main/f", "H"), Err(Error::AlreadyExists(_))));
        assert!(dir.path().join("requests/main/f/x.toml").is_file());
        assert_eq!(p.rename_folder("main/f", "g").unwrap().0, "main/g-2");
    }

    /// A rename drops entries already under its new path before renaming
    /// its own, as a folder move does: left in (a partial walk keeps them,
    /// D7), they'd name the folder twice (review round 1).
    #[cfg(unix)]
    #[test]
    fn rename_folder_drops_stale_entries_under_its_new_path() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, mut p) = folder_tree(&[
            ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/locked/y.toml", "method = \"GET\"\nurl = \"u\"\n"),
            (
                "project.toml",
                "spaces = [\"main\"]\n[space.main]\norder = [\"a\", \"g/\", \"g/x\", \"f/\", \"f/x\"]\n",
            ),
        ]);
        let locked = dir.path().join("requests/main/locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let renamed = p.rename_folder("main/f", "G");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        renamed.unwrap();
        let order = main_order(&p);
        for e in ["g/", "g/x"] {
            assert_eq!(order.iter().filter(|o| *o == e).count(), 1, "{e} once in {order:?}");
        }
    }

    #[test]
    fn rename_folder_moves_held_requests() {
        let (_dir, mut p) = three_items();
        p.open_request("main/f/x").unwrap();
        p.rename_folder("main/f", "G").unwrap();
        assert!(p.held_request("main/g/x").is_some());
        assert!(p.held_request("main/f/x").is_none());
    }

    /// F21: delete takes the folder, its contents and their entries.
    #[test]
    fn delete_folder_trashes_it_and_its_entries() {
        let (dir, mut p) = three_items();
        p.open_request("main/f/x").unwrap();
        let gone = p.delete_folder("main/f").unwrap();
        assert_eq!(gone, ["main/f/x"]);
        assert!(!dir.path().join("requests/main/f").exists());
        assert_eq!(main_order(&p), ["a", "b"]);
        assert!(p.held_request("main/f/x").is_none());
    }

    /// Review focus 1: a folder holding only a `.DS_Store` renames and
    /// deletes, the dot file going with it.
    #[test]
    fn a_folder_holding_only_ds_store_renames_and_deletes() {
        let (dir, mut p) = folder_tree(&[("requests/main/d/.DS_Store", "junk")]);
        p.rename_folder("main/d", "E").unwrap();
        assert!(dir.path().join("requests/main/e/.DS_Store").is_file());
        p.delete_folder("main/e").unwrap();
        assert!(!dir.path().join("requests/main/e").exists());
    }

    /// F33: a directory whose name isn't a slug shows as a folder and is
    /// changed by nothing, as a request file of such a name is: its rename,
    /// its delete, a create or a drop in it, and a create or rename of a
    /// folder in it are refused as an invalid name, with nothing written.
    #[test]
    fn a_non_slug_directory_shows_and_nothing_changes_it() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/My Stuff/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/My Stuff/ok/", ""),
        ]);
        assert!(folder_paths(&p).contains(&"main/My Stuff".to_string()));
        let before = requests_tree(&dir);
        let bad = |r: Result<(), Error>| matches!(r, Err(Error::BadName(_)));
        assert!(bad(p.rename_folder("main/My Stuff", "Mine").map(|_| ())));
        assert!(bad(p.delete_folder("main/My Stuff").map(|_| ())));
        assert!(bad(p.create_request_in("main/My Stuff", "N", None, req("u")).map(|_| ())));
        assert!(bad(p.create_request("main/My Stuff/N", req("u")).map(|_| ())));
        assert!(bad(p.create_folder("main/My Stuff", "N", None).map(|_| ())));
        assert!(bad(p.rename_folder("main/My Stuff/ok", "Fine").map(|_| ())));
        assert!(p.move_to_level(&req_item("main/a"), "My Stuff", 0).is_err());
        assert_eq!(requests_tree(&dir), before);
    }

    // --- review block 1 adjustment: one naming rule (mine (approved)) ---

    /// A folder name can't hold `/` (replacing F18's "a `/` is part of the
    /// name"), in a create and a rename; nothing is written.
    #[test]
    fn a_folder_name_holding_a_slash_is_refused() {
        let (dir, mut p) = three_items();
        let before = requests_tree(&dir);
        assert!(matches!(p.create_folder("main", "a/b", None), Err(Error::BadName(_))));
        assert!(matches!(p.rename_folder("main/f", "a/b"), Err(Error::BadName(_))));
        assert_eq!(requests_tree(&dir), before);
    }

    /// A rename keeps the directory when the name's slug is the one it has,
    /// numbered or not, as a request rename keeps its slug (#209).
    #[test]
    fn a_numbered_folder_renames_to_its_base_name_in_place() {
        let (dir, mut p) = folder_tree(&[("requests/main/f/.folder.toml", "name = \"Other\"\n")]);
        assert_eq!(p.create_folder("main", "f", None).unwrap(), "main/f-2");
        let (path, _) = p.rename_folder("main/f-2", "F").unwrap();
        assert_eq!(path, "main/f-2");
        assert_eq!(read(&dir, "requests/main/f-2/.folder.toml").as_deref(), Some("name = \"F\"\n"));
        let (path, _) = p.rename_folder("main/f-2", "Zed").unwrap();
        assert_eq!(path, "main/zed");
    }

    /// Create and rename make one slug of a name, as requests do: a case
    /// change keeps the directory, and a name with nothing safe in it
    /// gets `folder` either way.
    #[test]
    fn create_and_rename_slug_a_name_alike() {
        let (_dir, mut p) = folder_tree(&[]);
        let wip = p.create_folder("main", "-wip", None).unwrap();
        assert_eq!(p.rename_folder(&wip, "-WIP").unwrap().0, wip);
        let bang = p.create_folder("main", "!!!", None).unwrap();
        assert_eq!(bang, "main/folder");
        assert_eq!(p.rename_folder(&bang, "???").unwrap().0, bang);
    }

    /// A rename onto a directory taken by a folder showing another name
    /// numbers, as a create does, instead of refusing.
    #[test]
    fn a_rename_onto_a_taken_directory_numbers() {
        let (_dir, mut p) = three_items();
        std::fs::create_dir_all(p.root().join("requests/main/g")).unwrap();
        std::fs::write(p.root().join("requests/main/g/.folder.toml"), "name = \"Gee\"\n").unwrap();
        p.relist();
        assert_eq!(p.rename_folder("main/f", "g").unwrap().0, "main/g-2");
    }

    /// An empty name in a folder file counts as no name.
    #[test]
    fn an_empty_folder_name_counts_as_none() {
        let (_dir, p) = folder_tree(&[("requests/main/e/.folder.toml", "name = \"  \"\n")]);
        assert_eq!(p.folders()[0].name, None);
        assert_eq!(p.folders()[0].display_name(), "e");
    }

    /// The copy checks a folder file as the listing does: `name = 5` is no
    /// usable file, so the copy is named by its directory.
    #[test]
    fn a_copy_skips_a_folder_file_the_listing_calls_broken() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/f/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/f/.folder.toml", "name = 5\n"),
            ("requests/auth/", ""),
            ("project.toml", "spaces = [\"main\", \"auth\"]\n"),
        ]);
        assert!(p.folders().iter().find(|f| f.path == "main/f").unwrap().broken.is_some());
        p.move_request("main/f/x", "auth").unwrap();
        assert_eq!(read(&dir, "requests/auth/f/.folder.toml").as_deref(), Some("name = \"f\"\n"));
    }

    /// F19 on a copy: a folder made in the target whose name a sibling
    /// there already shows is named by its directory.
    #[test]
    fn a_copy_whose_name_a_target_sibling_shows_is_named_by_its_directory() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/u/x.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/u/.folder.toml", "name = \"Users\"\n"),
            ("requests/auth/a/.folder.toml", "name = \"Users\"\n"),
            ("project.toml", "spaces = [\"main\", \"auth\"]\n"),
        ]);
        p.move_request("main/u/x", "auth").unwrap();
        assert_eq!(read(&dir, "requests/auth/u/.folder.toml").as_deref(), Some("name = \"u\"\n"));
    }

    // --- review round 4 ---

    /// A copy's fallback name is numbered while a target sibling shows it
    /// too, ignoring case (F19).
    #[test]
    fn a_copys_fallback_name_is_numbered_while_shown() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/users/get.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/auth/people/.folder.toml", "name = \"Users\"\n"),
            ("project.toml", "spaces = [\"main\", \"auth\"]\n"),
        ]);
        p.move_request("main/users/get", "auth").unwrap();
        assert_eq!(
            read(&dir, "requests/auth/users/.folder.toml").as_deref(),
            Some("name = \"users 2\"\n")
        );
    }

    /// Move all checks each copy's name against the copies made before it
    /// too: no two folders it leaves side by side show one name (F19).
    #[test]
    fn move_all_copies_never_share_a_shown_name() {
        let (_dir, mut p) = folder_tree(&[
            ("requests/main/a/.folder.toml", "name = \"Q\"\n"),
            ("requests/main/q/.folder.toml", "name = \"A\"\n"),
            ("requests/auth/z/.folder.toml", "name = \"Q\"\n"),
            ("project.toml", "spaces = [\"main\", \"auth\"]\n"),
        ]);
        p.move_all_requests("main", "auth").unwrap();
        p.relist();
        let mut shown: Vec<String> = p
            .folders()
            .iter()
            .filter(|f| f.path.starts_with("auth/"))
            .map(|f| f.display_name().to_lowercase())
            .collect();
        let count = shown.len();
        shown.sort();
        shown.dedup();
        assert_eq!(shown.len(), count, "{shown:?}");
        assert_eq!(count, 3);
    }

    /// A slug counts as taken by a listed folder in any case, but a rename
    /// that only changes its own directory's case is no clash.
    #[test]
    fn folder_slots_compare_listed_directories_ignoring_case() {
        let (_dir, mut p) =
            folder_tree(&[("requests/main/Accounts/.folder.toml", "name = \"Billing\"\n")]);
        assert_eq!(p.create_folder("main", "accounts", None).unwrap(), "main/accounts-2");
    }

    /// A folder file's name is trimmed, and a name holding `/` (hand-made)
    /// counts as none, so the folder's prefill finds it again.
    #[test]
    fn a_folder_files_name_is_trimmed_and_a_slash_name_counts_as_none() {
        let (_dir, mut p) = folder_tree(&[
            ("requests/main/u/.folder.toml", "name = \" Users \"\n"),
            ("requests/main/s/.folder.toml", "name = \"a/b\"\n"),
        ]);
        let shown = |p: &Project, path: &str| {
            p.folders().iter().find(|f| f.path == path).unwrap().display_name().to_string()
        };
        assert_eq!(shown(&p, "main/u"), "Users");
        assert_eq!(shown(&p, "main/s"), "s");
        assert_eq!(p.create_request("main/Users/x", req("u")).unwrap().0, "main/u/x");
    }

    /// A typed folder name more than one sibling shows (made outside
    /// postui) refuses the import with nothing written, rather than filing
    /// into whichever comes first.
    #[test]
    fn a_folder_name_two_siblings_show_refuses_the_import() {
        let (dir, mut p) = folder_tree(&[
            ("requests/main/accounts/.folder.toml", "name = \"Users\"\n"),
            ("requests/main/users/", ""),
        ]);
        let before = requests_tree(&dir);
        let r = p.create_request("main/users/ping", req("u"));
        assert!(matches!(r, Err(Error::Conflict(_))), "{r:?}");
        assert_eq!(requests_tree(&dir), before);
    }

    /// A curl import's create reads the disk as it is: a folder made
    /// outside since the last relist is found by its name.
    #[test]
    fn a_create_finds_a_folder_made_since_the_last_relist() {
        let (dir, mut p) = folder_tree(&[]);
        write(&dir, "requests/main/users/.folder.toml", "name = \"Users\"\n");
        assert_eq!(p.create_request("main/Users/ping", req("u")).unwrap().0, "main/users/ping");
    }

    /// The numbered directory a rename keeps is one of its own name's: a
    /// folder whose slug merely ends in digits takes the new name's slug.
    #[test]
    fn a_rename_keeps_only_its_own_numbered_directory() {
        let (_dir, mut p) =
            folder_tree(&[("requests/main/users-2024/.folder.toml", "name = \"Users 2024\"\n")]);
        assert_eq!(p.rename_folder("main/users-2024", "Users").unwrap().0, "main/users");
    }

    /// A directory the walk can't list is warned about once, by the walk,
    /// and isn't marked broken.
    #[cfg(unix)]
    #[test]
    fn an_unlistable_directory_warns_once_and_is_not_broken() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, _) =
            folder_tree(&[("requests/main/secret/x.toml", "method = \"GET\"\nurl = \"u\"\n")]);
        let secret = dir.path().join("requests/main/secret");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (p, _w) = Project::open(dir.path().to_path_buf()).unwrap();
        let warnings = p.listing_warnings().len();
        let broken = p.folders().iter().find(|f| f.path == "main/secret").map(|f| f.broken.clone());
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(warnings, 1);
        assert_eq!(broken, Some(None));
    }

    // --- review round 5 ---

    /// A rename stays in the request's own folder, even beside a sibling
    /// that shows the same name (two folders of one name, made outside
    /// postui): it never reads a folder from a name (F28).
    #[test]
    fn a_rename_stays_in_its_own_folder() {
        let (_dir, mut p) = folder_tree(&[
            ("requests/main/accounts/.folder.toml", "name = \"Users\"\n"),
            ("requests/main/users/x.toml", "name = \"X\"\nmethod = \"GET\"\nurl = \"u\"\n"),
        ]);
        let (slug, _) = p.rename_request("main/users/x", "Y").unwrap();
        assert_eq!(slug, "main/users/y");
    }

    /// Move all reads the disk as it is: a folder deleted outside since the
    /// last relist doesn't fail it.
    #[test]
    fn move_all_survives_a_folder_deleted_since_the_last_relist() {
        let (dir, mut p) = folder_project(&[], &["main/a/x"]);
        std::fs::create_dir_all(dir.path().join("requests/main/old")).unwrap();
        p.relist();
        std::fs::remove_dir(dir.path().join("requests/main/old")).unwrap();
        p.move_all_requests("main", "auth").unwrap();
        assert!(dir.path().join("requests/auth/a/x.toml").is_file());
        assert!(!dir.path().join("requests/auth/old").exists());
    }

    /// Only a folder the op made arrives in the list: an existing folder the
    /// list misses (no completion ran for its space) keeps drawing where it
    /// draws, never jumps last.
    #[test]
    fn only_folders_the_op_made_arrive() {
        // A list missing `users/` (its completion couldn't write).
        let mut order = vec!["b".to_string()];
        arrive_with_folders("main", &mut order, "users/new", &[]);
        assert_eq!(order, ["b", "users/new"], "an existing folder doesn't jump last");
        let mut order = vec!["b".to_string()];
        arrive_with_folders("main", &mut order, "fresh/n", &["main/fresh".to_string()]);
        assert_eq!(order, ["b", "fresh/", "fresh/n"], "one the op made arrives");
    }

    /// Move all carries every valid folder. One holding only a request it
    /// leaves behind stays with that request, and gets an empty twin in the
    /// target, made with its file. Undo is exact.
    #[test]
    fn move_all_keeps_a_folder_holding_a_left_behind_request() {
        let (dir, mut p) = folder_project(&[], &["main/ok"]);
        write(&dir, "requests/main/users/Get User.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        let before = requests_tree(&dir);
        let (_, warnings) = p.move_all_requests("main", "auth").unwrap();
        assert!(warnings.iter().any(|w| w.contains("Get User.toml")), "{warnings:?}");
        assert!(dir.path().join("requests/main/users/Get User.toml").is_file());
        assert!(dir.path().join("requests/auth/users/.folder.toml").is_file());
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(&dir), before);
    }

    /// A folder whose parent holds only what is left behind is made under
    /// a parent made with it, both journaled: undo leaves nothing behind.
    #[test]
    fn move_all_makes_each_carried_folders_parent_with_it() {
        let (dir, mut p) = folder_project(&[], &["main/ok"]);
        write(&dir, "requests/main/f/Bad Name.toml", "method = \"GET\"\nurl = \"u\"\n");
        std::fs::create_dir_all(dir.path().join("requests/main/f/g")).unwrap();
        p.relist();
        let before = requests_tree(&dir);
        p.move_all_requests("main", "auth").unwrap();
        assert!(dir.path().join("requests/auth/f/.folder.toml").is_file());
        assert!(dir.path().join("requests/auth/f/g/.folder.toml").is_file());
        assert!(!dir.path().join("requests/main/f/g").exists(), "g is emptied and goes");
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(&dir), before);
    }

    /// A dot file the user keeps in a folder is theirs (F16): Move all
    /// leaves the folder in place with it, instead of trashing both.
    #[test]
    fn move_all_leaves_a_folder_holding_a_dot_file() {
        let (dir, mut p) = folder_project(&[], &["main/users/list"]);
        write(&dir, "requests/main/users/.draft.toml", "x = 1\n");
        p.relist();
        p.move_all_requests("main", "auth").unwrap();
        assert!(dir.path().join("requests/auth/users/list.toml").is_file());
        assert_eq!(read(&dir, "requests/main/users/.draft.toml").as_deref(), Some("x = 1\n"));
    }

    /// A folder holding a left-behind file and a moved request is carried:
    /// the folder the request lands in is made with its file, journaled,
    /// and the source keeps what stayed.
    #[test]
    fn move_all_carries_a_folder_holding_a_moved_and_a_left_behind_request() {
        let (dir, mut p) = folder_project(&[], &["main/users/ok"]);
        write(&dir, "requests/main/users/Get User.toml", "method = \"GET\"\nurl = \"u\"\n");
        p.relist();
        let before = requests_tree(&dir);
        p.move_all_requests("main", "auth").unwrap();
        assert!(dir.path().join("requests/auth/users/ok.toml").is_file());
        assert!(
            dir.path().join("requests/auth/users/.folder.toml").is_file(),
            "the folder it made has its file"
        );
        assert!(dir.path().join("requests/main/users/Get User.toml").is_file());
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(&dir), before, "undo leaves nothing behind");
    }

    /// Nested folders list outside in, by path; a space is no folder.
    #[test]
    fn nested_folders_list_by_path_and_spaces_are_not_folders() {
        let (_d, p) = folder_tree(&[
            ("requests/main/u/v/w.toml", "method = \"GET\"\nurl = \"u\"\n"),
            ("requests/main/e/", ""),
        ]);
        assert_eq!(folder_paths(&p), ["main/e", "main/u", "main/u/v"]);
    }

    // --- #278: a directory spelled in another case is not refused ---

    /// Space `auth`'s directory was renamed by hand to `Auth/`. A create
    /// or a move into the space is an ordinary one (#278): it writes at
    /// `requests/auth/…`, the hand-renamed directory keeps what it held,
    /// and undo takes the op back exactly. Where the disk folds case, the
    /// test checks only that no request file is lost: that arm was traced,
    /// not run.
    #[test]
    fn an_op_into_a_space_whose_directory_is_in_another_case_is_ordinary() {
        let (dir, mut p) = folder_project(&[], &["main/x", "auth/a"]);
        std::fs::rename(dir.path().join("requests/auth"), dir.path().join("requests/Auth"))
            .unwrap();
        if dir.path().join("requests/auth").exists() {
            let files = request_files(&dir);
            let _ = p.create_request_in("auth", "New", None, req("u"));
            assert_none_lost(&dir, &files);
            let _ = p.undo();
            assert_none_lost(&dir, &files);
            let _ = p.move_request("main/x", "auth");
            assert_none_lost(&dir, &files);
            let _ = p.undo();
            assert_none_lost(&dir, &files);
            return;
        }
        let before = requests_tree(&dir);
        assert_eq!(p.create_request_in("auth", "New", None, req("u")).unwrap().0, "auth/new");
        assert!(dir.path().join("requests/auth/new.toml").is_file());
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(&dir), before);
        assert_eq!(p.move_request("main/x", "auth").unwrap(), "auth/x");
        assert!(dir.path().join("requests/auth/x.toml").is_file());
        assert!(dir.path().join("requests/Auth/a.toml").is_file());
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(&dir), before);
    }

    /// The same for a folder: the target has a hand-made `Users/` where a
    /// move makes `users/`. Where the disk folds case, the test checks only
    /// that no request file is lost: that arm was traced, not run.
    #[test]
    fn a_move_beside_a_folder_in_another_case_is_ordinary() {
        let (dir, mut p) = folder_project(&[], &["main/users/x", "auth/Users/y"]);
        if dir.path().join("requests/auth/users").exists() {
            let files = request_files(&dir);
            let _ = p.move_request("main/users/x", "auth");
            assert_none_lost(&dir, &files);
            let _ = p.undo();
            assert_none_lost(&dir, &files);
            return;
        }
        let before = requests_tree(&dir);
        assert_eq!(p.move_request("main/users/x", "auth").unwrap(), "auth/users/x");
        assert!(dir.path().join("requests/auth/users/x.toml").is_file());
        assert!(dir.path().join("requests/auth/Users/y.toml").is_file());
        // The new `users/` is named past "Users" (F19).
        assert_eq!(
            std::fs::read_to_string(dir.path().join("requests/auth/users/.folder.toml")).unwrap(),
            "name = \"users 2\"\n"
        );
        p.undo().unwrap().unwrap();
        assert_eq!(requests_tree(&dir), before);
    }
}
