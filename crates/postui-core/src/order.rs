//! The user-chosen order inside a space: `[space.<slug>] order = [...]`
//! in `project.toml`. An entry is a request's slug relative to the space
//! (`"auth/login"`) or a folder's relative path with a trailing slash
//! (`"auth/"`, spec 2026-11-01 reorder folders R6). Only the relative
//! order among the siblings of one kind at one level (the space root, or
//! one folder) means anything: a level's requests always come before its
//! folders. Entries naming nothing are ignored for display and preserved
//! on write.

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

/// Applies the display rule to the folders of one level, given as full
/// paths (`"main/users"`): the ones named in `order` first, in list
/// order, then the rest by path. With no folder entries this is the byte
/// order the sidebar has always drawn folders in.
pub fn order_folders(paths: &[String], order: &[String], space: &str) -> Vec<String> {
    let position = |p: &str| -> Option<usize> {
        let entry = folder_entry(relative(p, space)?);
        order.iter().position(|o| *o == entry)
    };
    let mut listed: Vec<(usize, &String)> = Vec::new();
    let mut unlisted: Vec<&String> = Vec::new();
    for p in paths {
        match position(p) {
            Some(i) => listed.push((i, p)),
            None => unlisted.push(p),
        }
    }
    listed.sort_by_key(|(i, _)| *i);
    unlisted.sort();
    listed.into_iter().map(|(_, p)| p).chain(unlisted).cloned().collect()
}

/// Applies the display rule to the requests of one level: the ones named
/// in `order` first, in list order, then the rest by display name (slug
/// as the tiebreak). `entries` must all be requests of the same level of
/// `space`; the caller has already partitioned them.
pub fn order_level<'a>(
    entries: &[&'a RequestListing],
    order: &[String],
    space: &str,
) -> Vec<&'a RequestListing> {
    let position = |e: &RequestListing| -> Option<usize> {
        let rel = relative(&e.slug, space)?;
        order.iter().position(|o| o == rel)
    };
    let mut listed: Vec<(usize, &'a RequestListing)> = Vec::new();
    let mut unlisted: Vec<&'a RequestListing> = Vec::new();
    for e in entries {
        match position(e) {
            Some(p) => listed.push((p, e)),
            None => unlisted.push(e),
        }
    }
    listed.sort_by_key(|(p, _)| *p);
    unlisted.sort_by_key(|e| (e.display_name().to_lowercase(), e.slug.clone()));
    listed.into_iter().map(|(_, e)| e).chain(unlisted).collect()
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

/// Whether `level` has a displayed listed entry of one kind (`folders`
/// or requests): one naming something that exists (`exists` answers for
/// a full slug, or a full folder path ending in `/`; see [`exists_in`]).
pub(crate) fn level_is_listed(
    space: &str,
    order: &[String],
    level: &str,
    folders: bool,
    exists: &dyn Fn(&str) -> bool,
) -> bool {
    order.iter().any(|e| {
        is_folder_entry(e) == folders && entry_level(e) == level && exists(&format!("{space}/{e}"))
    })
}

/// The arrival rule for a list entry of either kind (a request slug or a
/// folder entry): when its level already has a listed entry of its kind,
/// it is appended (it lands last on screen); otherwise nothing is written
/// and it sorts by name.
pub(crate) fn arrive(
    space: &str,
    order: &mut Vec<String>,
    entry: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<OrderEdit> {
    let folders = is_folder_entry(entry);
    if order.iter().any(|e| e == entry)
        || !level_is_listed(space, order, entry_level(entry), folders, exists)
    {
        return Vec::new();
    }
    let at = order.len();
    order.push(entry.to_string());
    vec![OrderEdit::Inserted { space: space.to_string(), rel: entry.to_string(), at }]
}

/// A folder at relative path `rel` appearing (spec 2026-11-01 R10,
/// R10a). A folder that is `new` (nothing but the arriving requests is
/// under it) first loses any entry left over from an earlier folder of
/// that name, emptied where no cascade saw it, so it never inherits that
/// slot; then the arrival rule runs. A folder already there is left as
/// it is.
pub(crate) fn arrive_folder(
    space: &str,
    order: &mut Vec<String>,
    rel: &str,
    new: bool,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<OrderEdit> {
    if !new {
        return Vec::new();
    }
    let entry = folder_entry(rel);
    let mut edits = remove(space, order, &entry);
    edits.extend(arrive(space, order, &entry, exists));
    edits
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

/// The list after one level is rewritten to `slugs` (entries of one kind:
/// request slugs, or folder entries): entries of `level` that appear in
/// `slugs` are replaced in place, first slot first; slugs with no slot
/// to take are appended; every other entry (other levels, the level's
/// entries of the other kind, entries not named in `slugs` — stale ones)
/// keeps its slot. Duplicates in `existing` collapse to their first
/// occurrence.
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
/// sidebar draws them with every folder open: at each level its requests
/// under [`order_level`], then each of its folders under
/// [`order_folders`], recursing into each.
pub fn displayed_slugs(listing: &[RequestListing], order: &[String], space: &str) -> Vec<String> {
    let prefix = format!("{space}/");
    let entries: Vec<&RequestListing> =
        listing.iter().filter(|e| e.slug.starts_with(&prefix)).collect();
    let mut out = Vec::new();
    walk_level(&entries, &prefix, order, space, &mut out);
    out
}

fn walk_level(
    entries: &[&RequestListing],
    prefix: &str,
    order: &[String],
    space: &str,
    out: &mut Vec<String>,
) {
    let mut folders: std::collections::BTreeMap<String, Vec<&RequestListing>> =
        std::collections::BTreeMap::new();
    let mut requests: Vec<&RequestListing> = Vec::new();
    for e in entries {
        match e.slug[prefix.len()..].split_once('/') {
            Some((seg, _)) => folders.entry(format!("{prefix}{seg}")).or_default().push(e),
            None => requests.push(e),
        }
    }
    out.extend(order_level(&requests, order, space).into_iter().map(|e| e.slug.clone()));
    let paths: Vec<String> = folders.keys().cloned().collect();
    for path in order_folders(&paths, order, space) {
        walk_level(&folders[&path], &format!("{path}/"), order, space, out);
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

    fn slugs(v: &[&RequestListing]) -> Vec<String> {
        v.iter().map(|e| e.slug.clone()).collect()
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
        let out = order_level(&[&a, &l, &z], &[], "main");
        assert_eq!(slugs(&out), ["main/zeta", "main/legacy", "main/alpha"]);
    }

    #[test]
    fn listed_first_in_list_order_then_unlisted_alphabetically() {
        let a = entry("main/a", None);
        let b = entry("main/b", None);
        let c = entry("main/c", None);
        let d = entry("main/d", None);
        let order = vec!["c".to_string(), "a".to_string()];
        let out = order_level(&[&a, &b, &c, &d], &order, "main");
        assert_eq!(slugs(&out), ["main/c", "main/a", "main/b", "main/d"]);
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
        let out = order_level(&[&a, &b], &order, "main");
        assert_eq!(slugs(&out), ["main/b", "main/a"]);
    }

    #[test]
    fn folder_level_entries_match_their_relative_slug() {
        let x = entry("main/auth/x", None);
        let y = entry("main/auth/y", None);
        let order = vec!["auth/y".to_string()];
        let out = order_level(&[&x, &y], &order, "main");
        assert_eq!(slugs(&out), ["main/auth/y", "main/auth/x"]);
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
    fn order_folders_lists_named_folders_first_then_the_rest_by_path() {
        let paths = v(&["main/a", "main/b", "main/c", "main/d"]);
        // Stale, duplicate, request and other-level entries are ignored.
        let order = v(&["gone/", "c/", "a", "x/c/", "c/", "a/"]);
        assert_eq!(
            order_folders(&paths, &order, "main"),
            v(&["main/c", "main/a", "main/b", "main/d"])
        );
    }

    #[test]
    fn order_folders_with_no_folder_entries_is_todays_byte_order() {
        let paths = v(&["main/b", "main/a-b", "main/a"]);
        assert_eq!(order_folders(&paths, &v(&["b"]), "main"), v(&["main/a", "main/a-b", "main/b"]));
    }

    #[test]
    fn order_folders_matches_nested_folders_by_their_relative_path() {
        let paths = v(&["main/api/v1", "main/api/v2"]);
        let order = v(&["api/v2/"]);
        assert_eq!(order_folders(&paths, &order, "main"), v(&["main/api/v2", "main/api/v1"]));
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

    #[test]
    fn level_is_listed_counts_only_its_own_kind() {
        let listing = v(&["main/a", "main/users/list"]);
        let exists = |s: &str| exists_in(&listing, s);
        let order = v(&["users/"]);
        assert!(level_is_listed("main", &order, "", true, &exists));
        assert!(!level_is_listed("main", &order, "", false, &exists));
        let order = v(&["a"]);
        assert!(!level_is_listed("main", &order, "", true, &exists));
        assert!(level_is_listed("main", &order, "", false, &exists));
        // A folder entry naming no folder does not count.
        assert!(!level_is_listed("main", &v(&["gone/"]), "", true, &exists));
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
    fn arrive_folder_appends_only_when_the_level_has_listed_folders() {
        let listing = v(&["main/a/x", "main/b/y", "main/c/z"]);
        let exists = |s: &str| exists_in(&listing, s);
        let mut order = v(&["b/", "a/"]);
        arrive_folder("main", &mut order, "c", true, &exists);
        assert_eq!(order, v(&["b/", "a/", "c/"]));
        let mut order = v(&["x"]);
        arrive_folder("main", &mut order, "c", true, &exists);
        assert_eq!(order, v(&["x"]));
    }

    #[test]
    fn arrive_folder_drops_a_leftover_entry_of_a_new_folder() {
        // `c/` was emptied outside the app and kept its slot; made again,
        // it is a new folder and lands last (spec 2026-11-01 R10a).
        let listing = v(&["main/a/x", "main/b/y", "main/c/z"]);
        let exists = |s: &str| exists_in(&listing, s);
        let mut order = v(&["c/", "b/", "a/"]);
        arrive_folder("main", &mut order, "c", true, &exists);
        assert_eq!(order, v(&["b/", "a/", "c/"]));
        // A folder that was already there keeps its slot.
        let mut order = v(&["c/", "b/", "a/"]);
        arrive_folder("main", &mut order, "c", false, &exists);
        assert_eq!(order, v(&["c/", "b/", "a/"]));
    }

    #[test]
    fn prune_folders_drops_every_emptied_ancestor_innermost_first() {
        let listing = v(&["main/a/keep"]);
        let exists = |s: &str| exists_in(&listing, s);
        let mut order = v(&["a/", "a/b/", "a/b/c/", "z/"]);
        prune_folders("main", &mut order, "a/b/c/gone", &exists);
        assert_eq!(order, v(&["a/", "z/"]));
    }

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
            v(&["main/r", "main/z/q", "main/a/x", "main/a/b/y"])
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
