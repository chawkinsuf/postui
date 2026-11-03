//! The user-chosen order inside a space: `[space.<slug>] order = [...]`
//! in `project.toml`. An entry is a request's slug relative to the space
//! (`"auth/login"`) or a folder's relative path with a trailing slash
//! (`"auth/"`, spec 2026-11-01 reorder folders R6). Only the relative
//! order among the items of one level (the space root, or one folder)
//! means anything, and a level mixes both kinds in list order (spec
//! 2026-11-02 drag into folders D3).
//!
//! Every list names every request and folder of its space (D5): every op
//! that changes a list first writes in what it misses through
//! [`complete_order`], where it draws (D6), dropping entries naming
//! nothing when the listing is whole (D7), and the TUI does the same when
//! it re-lists (D5a). So an op never seeds a level before changing it: an
//! arrival appends ([`arrive`]), a departure removes, a rename renames in
//! place. A list read before it was completed (a project from 0.3.0, a
//! file added outside postui) still draws by [`order_entries`]' rule.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::project::ProjectMeta;
use crate::storage::RequestListing;

/// The order list of `space`, empty when none is written.
pub fn space_order<'a>(meta: &'a ProjectMeta, space: &str) -> &'a [String] {
    meta.space.get(space).map(|s| s.order.as_slice()).unwrap_or(&[])
}

/// `slug` with its leading `space/` stripped, or `None` if it is not in
/// `space`.
pub fn relative<'a>(slug: &'a str, space: &str) -> Option<&'a str> {
    let rest = slug.strip_prefix(space)?;
    rest.strip_prefix('/')
}

/// The level a relative slug belongs to: its folder part, no trailing
/// slash; `""` for the space root.
pub fn level_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(f, _)| f).unwrap_or("")
}

/// The list entry for the folder at relative path `rel` (`"api/v2"`):
/// the path with a trailing slash, which no request slug ends in.
pub fn folder_entry(rel: &str) -> String {
    format!("{rel}/")
}

/// Whether a list entry names a folder rather than a request.
pub fn is_folder_entry(e: &str) -> bool {
    e.ends_with('/')
}

/// The level a list entry belongs to, of either kind: `"auth/login"` and
/// `"auth/v2/"` both sit in `"auth"`.
pub fn entry_level(e: &str) -> &str {
    level_of(e.strip_suffix('/').unwrap_or(e))
}

/// Whether `full` names something in `slugs` (a listing's full slugs): a
/// request when it is a slug, a folder when it ends in `/` and some
/// request sits under it.
pub fn exists_in(slugs: &[String], full: &str) -> bool {
    if full.ends_with('/') {
        slugs.iter().any(|s| s.starts_with(full))
    } else {
        slugs.iter().any(|s| s == full)
    }
}

/// One item of a level, as [`order_entries`] draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelItem<'a> {
    Request(&'a RequestListing),
    /// A folder, by its full path (`"main/users"`).
    Folder(&'a str),
}

impl LevelItem<'_> {
    /// The item's list entry in `space`: a request's relative slug, a
    /// folder's relative path with its slash. `None` outside `space`.
    pub fn entry(&self, space: &str) -> Option<String> {
        match self {
            LevelItem::Request(r) => relative(&r.slug, space).map(str::to_string),
            LevelItem::Folder(p) => relative(p, space).map(folder_entry),
        }
    }
}

/// One level of a space split by kind (see [`split_level`]).
#[derive(Debug, Default)]
pub struct Level<'a> {
    /// The requests directly in the level.
    pub requests: Vec<&'a RequestListing>,
    /// The level's folders by full path (`"main/users"`), each with every
    /// request under it, at any depth.
    pub folders: BTreeMap<String, Vec<&'a RequestListing>>,
}

impl Level<'_> {
    /// The folders' full paths, in byte order.
    pub fn folder_paths(&self) -> Vec<&str> {
        self.folders.keys().map(String::as_str).collect()
    }
}

/// The one partition of a level into its requests and its folders: the
/// sidebar's rows, the tree walk and [`complete_order`] all read it.
/// `prefix` is the level's full path with a trailing slash (`"main/"`,
/// `"main/users/"`); entries outside it are skipped.
pub fn split_level<'a>(entries: &[&'a RequestListing], prefix: &str) -> Level<'a> {
    let mut level = Level::default();
    for e in entries {
        let Some(rest) = e.slug.strip_prefix(prefix) else { continue };
        match rest.split_once('/') {
            Some((seg, _)) => level.folders.entry(format!("{prefix}{seg}")).or_default().push(e),
            None => level.requests.push(e),
        }
    }
    level
}

/// An order list's entries by slot, for [`order_entries`]: built once per
/// walk of a space (or sidebar rebuild) and read at every level. An entry
/// the list holds twice keeps its first slot.
#[derive(Debug)]
pub struct OrderIndex<'a>(HashMap<&'a str, usize>);

impl<'a> OrderIndex<'a> {
    pub fn new(order: &'a [String]) -> Self {
        let mut first: HashMap<&str, usize> = HashMap::new();
        for (i, e) in order.iter().enumerate() {
            first.entry(e.as_str()).or_insert(i);
        }
        OrderIndex(first)
    }
}

/// One level's items of both kinds in the order they draw (spec
/// 2026-11-02 drag into folders D3, D6): the ones the list (`order`, its
/// [`OrderIndex`]) names in list order, whatever the kind; then the
/// unlisted requests by display name (slug as the tiebreak); then the
/// unlisted folders by path. On a complete list this is plain list order.
/// `requests` and `folders` (full paths) must all sit in the same level
/// of `space`.
pub fn order_entries<'a>(
    requests: &[&'a RequestListing],
    folders: &[&'a str],
    order: &OrderIndex<'_>,
    space: &str,
) -> Vec<LevelItem<'a>> {
    let first = &order.0;
    let mut listed: Vec<(usize, LevelItem<'a>)> = Vec::new();
    let mut unlisted_requests: Vec<&'a RequestListing> = Vec::new();
    let mut unlisted_folders: Vec<&'a str> = Vec::new();
    let items = requests
        .iter()
        .map(|r| LevelItem::Request(r))
        .chain(folders.iter().map(|p| LevelItem::Folder(p)));
    for item in items {
        match (item.entry(space).and_then(|e| first.get(e.as_str()).copied()), item) {
            (Some(i), _) => listed.push((i, item)),
            (None, LevelItem::Request(r)) => unlisted_requests.push(r),
            (None, LevelItem::Folder(p)) => unlisted_folders.push(p),
        }
    }
    listed.sort_by_key(|(i, _)| *i);
    unlisted_requests.sort_by_key(|e| (e.display_name().to_lowercase(), e.slug.clone()));
    unlisted_folders.sort();
    listed
        .into_iter()
        .map(|(_, item)| item)
        .chain(unlisted_requests.into_iter().map(LevelItem::Request))
        .chain(unlisted_folders.into_iter().map(LevelItem::Folder))
        .collect()
}

/// Writes `[space.<space>] order = [...]`, creating the table if needed
/// and dropping the key when the list is empty. Keeps the table's other
/// keys and the file's comments.
pub(crate) fn write_order(doc: &mut toml_edit::DocumentMut, space: &str, order: &[String]) {
    let table = doc.entry("space").or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(t) = table.as_table_mut() else {
        return;
    };
    t.set_implicit(true);
    let item = t.entry(space).or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(it) = item.as_table_mut() else {
        return;
    };
    if order.is_empty() {
        it.remove("order");
        return;
    }
    let mut arr = toml_edit::Array::new();
    for s in order {
        arr.push(s.as_str());
    }
    it["order"] = toml_edit::value(arr);
}

/// One change a cascade made to a space's list, precise enough to be
/// reversed: the undo of the file op that caused the cascade replays
/// these backwards, so the list ends up exactly as it was — and an
/// unrelated reorder made in between survives, which a whole-list
/// snapshot could not manage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderEdit {
    /// `rel` was inserted at index `at`.
    Inserted { space: String, rel: String, at: usize },
    /// `rel` was removed from index `at`.
    Removed { space: String, rel: String, at: usize },
    /// Every entry `from` became `to`.
    Renamed { space: String, from: String, to: String },
}

impl OrderEdit {
    /// Re-keys the edit after its space was renamed `from` -> `to`, so a
    /// recorded undo step keeps pointing at the space it was made in.
    pub fn rename_space(&mut self, from: &str, to: &str) {
        let space = match self {
            OrderEdit::Inserted { space, .. }
            | OrderEdit::Removed { space, .. }
            | OrderEdit::Renamed { space, .. } => space,
        };
        if space == from {
            *space = to.to_string();
        }
    }
}

/// The arrival rule for a list entry of either kind (a request slug or a
/// folder entry, spec 2026-11-02 drag into folders D4): it is appended,
/// so it lands last in its level. An entry already naming it (a folder's
/// leftover, emptied where no cascade saw it) goes first, so the list
/// never holds it twice.
pub(crate) fn arrive(space: &str, order: &mut Vec<String>, entry: &str) -> Vec<OrderEdit> {
    insert_before(space, order, entry, None)
}

/// `entry` placed right above `anchor` (an entry of its level), or last
/// when there is no anchor or the list doesn't name it: the slot a move
/// to another level takes (D17). An entry already naming it goes first,
/// as in [`arrive`].
pub(crate) fn insert_before(
    space: &str,
    order: &mut Vec<String>,
    entry: &str,
    anchor: Option<&str>,
) -> Vec<OrderEdit> {
    let mut edits = remove(space, order, entry);
    let at = anchor.and_then(|a| order.iter().position(|e| e == a)).unwrap_or(order.len());
    order.insert(at, entry.to_string());
    edits.push(OrderEdit::Inserted { space: space.to_string(), rel: entry.to_string(), at });
    edits
}

/// Every entry under the folder at relative path `from` (its own entry
/// and everything inside it, at any depth) renamed to sit under `to`
/// instead, in place: a folder move (D17) keeps its contents' order.
pub(crate) fn rename_prefix(order: &mut [String], from: &str, to: &str) {
    let from = folder_entry(from);
    let to = folder_entry(to);
    for e in order.iter_mut() {
        if let Some(rest) = e.strip_prefix(&from) {
            *e = format!("{to}{rest}");
        }
    }
}

/// Every folder `rel` sits in, outermost first: `"a/b/c"` gives `"a"`,
/// `"a/b"`.
pub(crate) fn ancestors(rel: &str) -> Vec<&str> {
    rel.match_indices('/').map(|(i, _)| &rel[..i]).collect()
}

/// After the request at `rel` left its level: the entry of every folder
/// it sat in that nothing is under any more, innermost first (spec
/// 2026-11-01 R9). A folder made again later starts with no slot.
pub(crate) fn prune_folders(
    space: &str,
    order: &mut Vec<String>,
    rel: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<OrderEdit> {
    let mut edits = Vec::new();
    for folder in ancestors(rel).into_iter().rev() {
        let entry = folder_entry(folder);
        if !exists(&format!("{space}/{entry}")) {
            edits.extend(remove(space, order, &entry));
        }
    }
    edits
}

/// [`order_remove`]'s edit, on a list already in hand: every occurrence,
/// recorded last index first so that replaying the inverses backwards
/// re-inserts first index first.
pub(crate) fn remove(space: &str, order: &mut Vec<String>, rel: &str) -> Vec<OrderEdit> {
    let mut edits = Vec::new();
    for at in (0..order.len()).rev() {
        if order[at] == rel {
            order.remove(at);
            edits.push(OrderEdit::Removed { space: space.to_string(), rel: rel.to_string(), at });
        }
    }
    edits
}

/// The list after one level is rewritten to `slugs` (the level's
/// entries, request slugs and folder entries alike): entries of `level`
/// that appear in `slugs` are replaced in place, first slot first; slugs
/// with no slot to take are appended; every other entry (other levels,
/// entries of the level not named in `slugs`) keeps its slot. Duplicates
/// in `existing` collapse to their first occurrence.
pub fn merge_level(existing: &[String], level: &str, slugs: &[String]) -> Vec<String> {
    let mut deduped: Vec<&String> = Vec::new();
    for e in existing {
        if !deduped.contains(&e) {
            deduped.push(e);
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut next = slugs.iter();
    for e in deduped {
        if entry_level(e) == level && slugs.contains(e) {
            if let Some(s) = next.next() {
                out.push(s.clone());
            }
        } else {
            out.push(e.clone());
        }
    }
    for s in next {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

/// Every request of `space` in `listing`, as full slugs, in the order the
/// sidebar draws them with every folder open: each level under
/// [`order_entries`], recursing into a folder at its place in its level
/// (spec 2026-11-02 drag into folders D22).
pub fn displayed_slugs(listing: &[RequestListing], order: &[String], space: &str) -> Vec<String> {
    let mut out = Vec::new();
    walk_space(listing, order, space, &mut |item| {
        if let LevelItem::Request(r) = item {
            out.push(r.slug.clone());
        }
    });
    out
}

/// `space`'s list as it draws (D5, D6): the entries `order` already holds,
/// in place, then every item it misses in the order it draws, a tree walk
/// through [`order_entries`]. With `drop_dead` (the listing is whole),
/// entries naming nothing drop out and each entry is kept once (D7);
/// without it (a walk that failed part way) every entry `order` holds is
/// kept as it is, so an unreadable folder keeps its arrangement. Only the
/// order within a level means anything, so the result draws exactly as
/// `order` did, and a complete list comes back unchanged.
pub fn complete_order(
    listing: &[RequestListing],
    order: &[String],
    space: &str,
    drop_dead: bool,
) -> Vec<String> {
    let mut drawn: Vec<String> = Vec::new();
    walk_space(listing, order, space, &mut |item| drawn.extend(item.entry(space)));
    if !drop_dead {
        let held: HashSet<&str> = order.iter().map(String::as_str).collect();
        let missing = drawn.iter().filter(|e| !held.contains(e.as_str())).cloned();
        return order.iter().cloned().chain(missing).collect();
    }
    let named: HashSet<&str> = drawn.iter().map(String::as_str).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for e in order.iter().chain(drawn.iter()) {
        if named.contains(e.as_str()) && seen.insert(e.as_str()) {
            out.push(e.clone());
        }
    }
    out
}

/// Visits every item of `space` in draw order, each folder followed by
/// its contents.
fn walk_space(
    listing: &[RequestListing],
    order: &[String],
    space: &str,
    visit: &mut dyn FnMut(LevelItem<'_>),
) {
    let prefix = format!("{space}/");
    let entries: Vec<&RequestListing> =
        listing.iter().filter(|e| e.slug.starts_with(&prefix)).collect();
    walk_level(&entries, &prefix, &OrderIndex::new(order), space, visit);
}

fn walk_level(
    entries: &[&RequestListing],
    prefix: &str,
    order: &OrderIndex<'_>,
    space: &str,
    visit: &mut dyn FnMut(LevelItem<'_>),
) {
    let level = split_level(entries, prefix);
    for item in order_entries(&level.requests, &level.folder_paths(), order, space) {
        visit(item);
        if let LevelItem::Folder(path) = item {
            walk_level(&level.folders[path], &format!("{path}/"), order, space, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Method;
    use crate::storage::RequestListing;

    fn entry(slug: &str, name: Option<&str>) -> RequestListing {
        RequestListing {
            slug: slug.to_string(),
            broken: None,
            method: Some(Method::Get),
            name: name.map(str::to_string),
        }
    }

    /// A level's items as `[R slug]` / `[F path]` strings, for one
    /// assertion over both kinds.
    fn drawn(items: &[LevelItem<'_>]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                LevelItem::Request(r) => format!("R {}", r.slug),
                LevelItem::Folder(p) => format!("F {p}"),
            })
            .collect()
    }

    /// One level's requests in draw order (a level with no folders).
    fn requests_drawn(requests: &[&RequestListing], order: &[String]) -> Vec<String> {
        order_entries(requests, &[], &OrderIndex::new(order), "main")
            .into_iter()
            .map(|i| match i {
                LevelItem::Request(r) => r.slug.clone(),
                LevelItem::Folder(p) => p.to_string(),
            })
            .collect()
    }

    /// One level's folders in draw order (a level with no requests).
    fn folders_drawn(paths: &[String], order: &[String]) -> Vec<String> {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        order_entries(&[], &paths, &OrderIndex::new(order), "main")
            .into_iter()
            .map(|i| match i {
                LevelItem::Request(r) => r.slug.clone(),
                LevelItem::Folder(p) => p.to_string(),
            })
            .collect()
    }

    /// Spec 2026-11-02 drag into folders D3, D6: one level mixes both
    /// kinds in list order; the unlisted requests follow by display name,
    /// then the unlisted folders by path.
    #[test]
    fn order_entries_mixes_both_kinds_then_unlisted_requests_then_unlisted_folders() {
        let zeta = entry("main/zeta", Some("Aardvark"));
        let alpha = entry("main/alpha", Some("Zebra"));
        let b = entry("main/b", None);
        let c = entry("main/c", None);
        let order = v(&["f/", "b", "e/"]);
        let out = order_entries(
            &[&zeta, &alpha, &b, &c],
            &["main/f", "main/e", "main/d"],
            &OrderIndex::new(&order),
            "main",
        );
        assert_eq!(
            drawn(&out),
            [
                "F main/f",
                "R main/b",
                "F main/e",
                "R main/zeta",
                "R main/c",
                "R main/alpha",
                "F main/d"
            ]
        );
    }

    #[test]
    fn split_level_partitions_one_level_into_requests_and_folders() {
        let r = entry("main/r", None);
        let x = entry("main/a/x", None);
        let y = entry("main/a/b/y", None);
        let q = entry("main/z/q", None);
        let level = split_level(&[&r, &x, &y, &q], "main/");
        assert_eq!(level.requests.iter().map(|e| e.slug.as_str()).collect::<Vec<_>>(), ["main/r"]);
        assert_eq!(level.folders.keys().collect::<Vec<_>>(), ["main/a", "main/z"]);
        assert_eq!(level.folders["main/a"].len(), 2, "everything under the folder");
        let inner = split_level(&level.folders["main/a"], "main/a/");
        assert_eq!(inner.requests.len(), 1);
        assert_eq!(inner.folders.keys().collect::<Vec<_>>(), ["main/a/b"]);
    }

    #[test]
    fn relative_and_level() {
        assert_eq!(relative("main/login", "main"), Some("login"));
        assert_eq!(relative("main/auth/refresh", "main"), Some("auth/refresh"));
        assert_eq!(relative("auth/login", "main"), None);
        assert_eq!(level_of("login"), "");
        assert_eq!(level_of("auth/refresh"), "auth");
        assert_eq!(level_of("auth/tokens/refresh"), "auth/tokens");
    }

    #[test]
    fn empty_order_is_todays_alphabetical_by_display_name() {
        let z = entry("main/zeta", Some("Aardvark"));
        let a = entry("main/alpha", Some("Zebra"));
        let l = entry("main/legacy", None);
        let out = requests_drawn(&[&a, &l, &z], &[]);
        assert_eq!(out, ["main/zeta", "main/legacy", "main/alpha"]);
    }

    #[test]
    fn listed_first_in_list_order_then_unlisted_alphabetically() {
        let a = entry("main/a", None);
        let b = entry("main/b", None);
        let c = entry("main/c", None);
        let d = entry("main/d", None);
        let order = vec!["c".to_string(), "a".to_string()];
        let out = requests_drawn(&[&a, &b, &c, &d], &order);
        assert_eq!(out, ["main/c", "main/a", "main/b", "main/d"]);
    }

    #[test]
    fn stale_duplicate_and_other_level_entries_are_ignored() {
        let a = entry("main/a", None);
        let b = entry("main/b", None);
        let order = vec![
            "gone".to_string(),
            "auth/b".to_string(), // another level's entry, same leaf
            "b".to_string(),
            "b".to_string(),
            "a".to_string(),
        ];
        let out = requests_drawn(&[&a, &b], &order);
        assert_eq!(out, ["main/b", "main/a"]);
    }

    /// An entry the list holds twice draws at its first slot.
    #[test]
    fn a_duplicate_entry_keeps_its_first_slot() {
        let a = entry("main/a", None);
        let b = entry("main/b", None);
        let out = requests_drawn(&[&a, &b], &v(&["b", "a", "b"]));
        assert_eq!(out, ["main/b", "main/a"]);
    }

    #[test]
    fn folder_level_entries_match_their_relative_slug() {
        let x = entry("main/auth/x", None);
        let y = entry("main/auth/y", None);
        let order = vec!["auth/y".to_string()];
        let out = requests_drawn(&[&x, &y], &order);
        assert_eq!(out, ["main/auth/y", "main/auth/x"]);
    }

    #[test]
    fn space_order_reads_the_space_table() {
        let meta: crate::project::ProjectMeta =
            toml::from_str("[space.main]\norder = [\"b\", \"a\"]\n").unwrap();
        assert_eq!(space_order(&meta, "main"), ["b", "a"]);
        assert!(space_order(&meta, "other").is_empty());
    }

    #[test]
    fn merge_level_replaces_slots_in_place_then_appends_and_keeps_others() {
        let existing = v(&["a", "auth/x", "stale", "b"]);
        let out = merge_level(&existing, "", &v(&["b", "c", "a"]));
        // Slots are handed out in list order: a's slot takes b, b's slot
        // takes c, and a (left over) is appended; the other level's entry
        // and the stale root entry keep their slots. Displayed root order
        // is then b, c, a — exactly `slugs`.
        assert_eq!(out, v(&["b", "auth/x", "stale", "c", "a"]));
    }

    #[test]
    fn merge_level_dedupes_existing_entries() {
        let out = merge_level(&v(&["a", "a", "b"]), "", &v(&["b", "a"]));
        assert_eq!(out, v(&["b", "a"]));
    }

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn entry_level_reads_request_and_folder_entries() {
        assert_eq!(entry_level("login"), "");
        assert_eq!(entry_level("auth/login"), "auth");
        assert_eq!(entry_level("auth/"), "");
        assert_eq!(entry_level("api/v2/"), "api");
        assert_eq!(folder_entry("api/v2"), "api/v2/");
        assert!(is_folder_entry("auth/"));
        assert!(!is_folder_entry("auth/login"));
    }

    #[test]
    fn folders_listed_first_then_the_rest_by_path() {
        let paths = v(&["main/a", "main/b", "main/c", "main/d"]);
        // Stale, duplicate, request and other-level entries are ignored.
        let order = v(&["gone/", "c/", "a", "x/c/", "c/", "a/"]);
        assert_eq!(folders_drawn(&paths, &order), v(&["main/c", "main/a", "main/b", "main/d"]));
    }

    #[test]
    fn unlisted_folders_draw_in_byte_order() {
        let paths = v(&["main/b", "main/a-b", "main/a"]);
        assert_eq!(folders_drawn(&paths, &v(&["b"])), v(&["main/a", "main/a-b", "main/b"]));
    }

    #[test]
    fn nested_folders_match_by_their_relative_path() {
        let paths = v(&["main/api/v1", "main/api/v2"]);
        let order = v(&["api/v2/"]);
        assert_eq!(folders_drawn(&paths, &order), v(&["main/api/v2", "main/api/v1"]));
    }

    #[test]
    fn merge_level_writes_a_folder_level_and_leaves_its_requests_alone() {
        let existing = v(&["b", "users/", "a", "orders/", "users/list"]);
        let out = merge_level(&existing, "", &v(&["orders/", "users/"]));
        assert_eq!(out, v(&["b", "orders/", "a", "users/", "users/list"]));
        // …and the reverse: a request write keeps the folder entries.
        let out = merge_level(&out, "", &v(&["a", "b"]));
        assert_eq!(out, v(&["a", "orders/", "b", "users/", "users/list"]));
    }

    #[test]
    fn merge_level_appends_folder_entries_to_a_list_without_them() {
        let out = merge_level(&v(&["b", "a"]), "", &v(&["y/", "x/"]));
        assert_eq!(out, v(&["b", "a", "y/", "x/"]));
    }

    /// Spec 2026-11-02 drag into folders D4, D5: every list is complete,
    /// so an arrival of either kind is a plain append, whatever its level
    /// already lists (replaces 2026-11-01 R10's "only when the level has
    /// listed entries of its kind").
    #[test]
    fn arrive_appends_whatever_the_level_lists() {
        let mut order = v(&["x"]);
        arrive("main", &mut order, "c/");
        arrive("main", &mut order, "c/new");
        arrive("main", &mut order, "r");
        assert_eq!(order, v(&["x", "c/", "c/new", "r"]));
    }

    /// An arrival never lists an entry twice: one already naming it (a
    /// folder's leftover entry, emptied where no cascade saw it) goes
    /// first, so the arrival lands last (replaces 2026-11-01 R10a).
    #[test]
    fn arrive_never_lists_an_entry_twice() {
        let mut order = v(&["c/", "b/", "c/", "a/"]);
        arrive("main", &mut order, "c/");
        assert_eq!(order, v(&["b/", "a/", "c/"]));
    }

    /// D17: a folder move renames its own entry and everything under it in
    /// place, and nothing else (`users/v10/` is not under `users/v1/`).
    #[test]
    fn rename_prefix_renames_a_nested_folder_and_its_contents_in_place() {
        let mut order =
            v(&["a", "users/", "users/v1/", "users/v1/get", "users/v1/deep/", "users/v1/deep/x"]);
        order.extend(v(&["users/v10/", "users/v1x", "b"]));
        rename_prefix(&mut order, "users/v1", "v1");
        assert_eq!(
            order,
            v(&[
                "a",
                "users/",
                "v1/",
                "v1/get",
                "v1/deep/",
                "v1/deep/x",
                "users/v10/",
                "users/v1x",
                "b"
            ])
        );
    }

    /// D5, D6: a list naming nothing is written as it draws: each level's
    /// requests by name, then its folders by path.
    #[test]
    fn complete_order_on_an_empty_list_writes_every_item_as_it_draws() {
        let listing = vec![
            entry("main/b", None),
            entry("main/a", None),
            entry("main/z/q", None),
            entry("main/y/p", None),
            entry("other/o", None),
        ];
        assert_eq!(
            complete_order(&listing, &[], "main", true),
            v(&["a", "b", "y/", "y/p", "z/", "z/q"])
        );
    }

    /// D5, D6: what a list misses is written after what it names, where it
    /// draws; what it names keeps its place.
    #[test]
    fn complete_order_writes_a_partial_list_in_where_it_draws() {
        let listing = vec![
            entry("main/a", None),
            entry("main/b", None),
            entry("main/c", None),
            entry("main/f/x", None),
            entry("main/f/y", None),
        ];
        let order = v(&["f/y", "c", "f/"]);
        let out = complete_order(&listing, &order, "main", true);
        assert_eq!(out, v(&["f/y", "c", "f/", "f/x", "a", "b"]));
        // Drawn from the completed list, every level reads as it did.
        assert_eq!(
            displayed_slugs(&listing, &out, "main"),
            displayed_slugs(&listing, &order, "main")
        );
    }

    /// D6: a 0.3.0 level whose list names only its folders shows them
    /// above its requests, and is written that way.
    #[test]
    fn complete_order_keeps_a_0_3_0_folder_only_level_folders_first() {
        let listing = vec![
            entry("main/r2", None),
            entry("main/r1", None),
            entry("main/a/x", None),
            entry("main/b/y", None),
        ];
        let out = complete_order(&listing, &v(&["b/", "a/"]), "main", true);
        assert_eq!(out, v(&["b/", "a/", "b/y", "a/x", "r1", "r2"]));
        assert_eq!(
            displayed_slugs(&listing, &out, "main"),
            v(&["main/b/y", "main/a/x", "main/r1", "main/r2"])
        );
    }

    /// D7: entries naming nothing (and repeats) drop out.
    #[test]
    fn complete_order_drops_entries_naming_nothing() {
        let listing = vec![entry("main/a/x", None), entry("main/r", None)];
        let order = v(&["gone", "a/", "x/", "a/gone", "a/x", "a/x", "r", "", "a//x"]);
        assert_eq!(complete_order(&listing, &order, "main", true), v(&["a/", "a/x", "r"]));
    }

    /// D7: a listing that is not whole drops nothing. Every entry stays as
    /// it is (one naming nothing listed may sit in an unreadable folder),
    /// and what the list misses is still written in where it draws.
    #[test]
    fn complete_order_from_a_partial_walk_adds_but_drops_nothing() {
        let listing = vec![entry("main/a/x", None), entry("main/r", None), entry("main/q", None)];
        let order = v(&["gone/", "gone/y", "r", "r", "a/x"]);
        assert_eq!(
            complete_order(&listing, &order, "main", false),
            v(&["gone/", "gone/y", "r", "r", "a/x", "q", "a/"])
        );
    }

    /// A complete list comes back as it is, whatever its levels'
    /// interleaving: a complete list never causes a write.
    #[test]
    fn complete_order_leaves_a_complete_list_alone() {
        let listing = vec![entry("main/a/x", None), entry("main/r", None), entry("main/s", None)];
        let order = v(&["a/x", "s", "a/", "r"]);
        assert_eq!(complete_order(&listing, &order, "main", true), order);
        assert_eq!(complete_order(&listing, &order, "main", false), order);
    }

    #[test]
    fn exists_in_answers_for_requests_and_folders() {
        let listing = v(&["main/a", "main/users/list"]);
        assert!(exists_in(&listing, "main/a"));
        assert!(!exists_in(&listing, "main/users"));
        assert!(exists_in(&listing, "main/users/"));
        assert!(!exists_in(&listing, "main/use/"));
    }

    #[test]
    fn prune_folders_drops_every_emptied_ancestor_innermost_first() {
        let listing = v(&["main/a/keep"]);
        let exists = |s: &str| exists_in(&listing, s);
        let mut order = v(&["a/", "a/b/", "a/b/c/", "z/"]);
        prune_folders("main", &mut order, "a/b/c/gone", &exists);
        assert_eq!(order, v(&["a/", "z/"]));
    }

    /// Spec 2026-11-02 drag into folders D6: the listed folders come
    /// before the unlisted requests of their level, `r` at the top and
    /// `x` in `a/` (replaces 2026-11-01 R1's requests-first walk).
    #[test]
    fn displayed_slugs_walks_the_tree_in_folder_order() {
        let listing = vec![
            entry("main/r", None),
            entry("main/a/x", None),
            entry("main/a/b/y", None),
            entry("main/z/q", None),
        ];
        let order = v(&["z/", "a/", "a/b/"]);
        assert_eq!(
            displayed_slugs(&listing, &order, "main"),
            v(&["main/z/q", "main/a/b/y", "main/a/x", "main/r"])
        );
    }

    /// D22: the walk recurses into a folder at its place in its level.
    #[test]
    fn displayed_slugs_recurses_into_a_folder_at_its_mixed_place() {
        let listing = vec![
            entry("main/r1", None),
            entry("main/r2", None),
            entry("main/a/x", None),
            entry("main/a/y", None),
        ];
        let order = v(&["r1", "a/", "a/y", "a/x", "r2"]);
        assert_eq!(
            displayed_slugs(&listing, &order, "main"),
            v(&["main/r1", "main/a/y", "main/a/x", "main/r2"])
        );
    }

    #[test]
    fn displayed_slugs_puts_a_folder_before_a_longer_name_like_the_sidebar() {
        // `-` sorts before `/`, so a walk by level slug put `a-b` first.
        let listing = vec![entry("main/a-b/y", None), entry("main/a/x", None)];
        assert_eq!(displayed_slugs(&listing, &[], "main"), v(&["main/a/x", "main/a-b/y"]));
    }

    #[test]
    fn displayed_slugs_lists_every_level_under_the_display_rule() {
        let listing = vec![
            entry("main/b", None),
            entry("main/a", None),
            entry("main/auth/y", None),
            entry("main/auth/x", None),
            entry("other/q", None),
        ];
        let order = v(&["b", "auth/y"]);
        assert_eq!(
            displayed_slugs(&listing, &order, "main"),
            v(&["main/b", "main/a", "main/auth/y", "main/auth/x"])
        );
    }
}
