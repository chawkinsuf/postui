//! The user-chosen request order inside a space: `[space.<slug>] order =
//! [...]` in `project.toml`. Entries are slugs relative to the space.
//! Only the relative order among the siblings of one level (the space
//! root, or one folder) means anything; entries naming no file are
//! ignored for display and preserved on write.

use crate::project::ProjectMeta;
use crate::project::{ProjectError, edit_project_toml, load_meta};
use crate::storage::RequestListing;
use std::path::Path;

/// The order list of `space`, empty when none is written.
pub fn space_order<'a>(meta: &'a ProjectMeta, space: &str) -> &'a [String] {
    meta.space
        .get(space)
        .map(|s| s.order.as_slice())
        .unwrap_or(&[])
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
    unlisted.sort_by_key(|e| {
        let leaf = e.slug.rsplit('/').next().unwrap_or(&e.slug);
        let display = e.name.as_deref().unwrap_or(leaf);
        (display.to_lowercase(), e.slug.clone())
    });
    listed
        .into_iter()
        .map(|(_, e)| e)
        .chain(unlisted)
        .collect()
}

/// Writes `[space.<space>] order = [...]`, creating the table if needed
/// and dropping the key when the list is empty. Keeps the table's other
/// keys and the file's comments.
fn write_order(doc: &mut toml_edit::DocumentMut, space: &str, order: &[String]) {
    let table = doc
        .entry("space")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(t) = table.as_table_mut() else { return };
    t.set_implicit(true);
    let item = t
        .entry(space)
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(it) = item.as_table_mut() else { return };
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

/// Reads the list, hands it to `f`, and writes back only if `f` changed
/// it — so every cascade is a no-op write when there is nothing to do.
fn edit_order(
    root: &Path,
    space: &str,
    f: impl FnOnce(&mut Vec<String>),
) -> Result<(), ProjectError> {
    let meta = load_meta(root)?;
    let before = space_order(&meta, space).to_vec();
    let mut after = before.clone();
    f(&mut after);
    if after == before {
        return Ok(());
    }
    edit_project_toml(root, |doc| write_order(doc, space, &after))
}

fn level_is_listed(order: &[String], level: &str) -> bool {
    order.iter().any(|e| level_of(e) == level)
}

/// A request newly appearing in a level (created, renamed in, moved in):
/// appended after the level's listed entries so it shows last; nothing
/// at all when the level has no entries (it then sorts alphabetically
/// with its unlisted siblings).
pub fn order_arrive(root: &Path, space: &str, rel: &str) -> Result<(), ProjectError> {
    edit_order(root, space, |order| {
        if order.iter().any(|e| e == rel) || !level_is_listed(order, level_of(rel)) {
            return;
        }
        order.push(rel.to_string());
    })
}

/// Drops `rel`'s entry (every occurrence).
pub fn order_remove(root: &Path, space: &str, rel: &str) -> Result<(), ProjectError> {
    edit_order(root, space, |order| order.retain(|e| e != rel))
}

/// A rename inside one level keeps the slot; one that changes level is a
/// remove from the old level plus an arrival in the new one.
pub fn order_rename(root: &Path, space: &str, from: &str, to: &str) -> Result<(), ProjectError> {
    if level_of(from) == level_of(to) {
        return edit_order(root, space, |order| {
            for e in order.iter_mut() {
                if e == from {
                    *e = to.to_string();
                }
            }
        });
    }
    order_remove(root, space, from)?;
    order_arrive(root, space, to)
}

/// Puts `rel` directly after `anchor`; a no-op when `anchor` is unlisted
/// (the level has no list, so the copy sorts next to its source anyway),
/// and equally a no-op when `rel` is already listed at *any* level — an
/// existing slot is never moved by a copy landing beside its source.
pub fn order_insert_after(
    root: &Path,
    space: &str,
    anchor: &str,
    rel: &str,
) -> Result<(), ProjectError> {
    edit_order(root, space, |order| {
        if order.iter().any(|e| e == rel) {
            return;
        }
        if let Some(i) = order.iter().position(|e| e == anchor) {
            order.insert(i + 1, rel.to_string());
        }
    })
}

/// The list after one level is rewritten to `slugs`: entries of `level`
/// that appear in `slugs` are replaced in place, first slot first; slugs
/// with no slot to take are appended; every other entry (other levels,
/// entries of this level not named in `slugs` — stale ones) keeps its
/// slot. Duplicates in `existing` collapse to their first occurrence.
fn merge_level(existing: &[String], level: &str, slugs: &[String]) -> Vec<String> {
    let mut deduped: Vec<&String> = Vec::new();
    for e in existing {
        if !deduped.contains(&e) {
            deduped.push(e);
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut next = slugs.iter();
    for e in deduped {
        if level_of(e) == level && slugs.contains(e) {
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

/// Rewrites one level of `space`'s order to exactly `slugs` (relative
/// slugs, all of the same `level`). See [`merge_level`] for what happens
/// to everything else in the list.
pub fn set_level_order(
    root: &Path,
    space: &str,
    level: &str,
    slugs: &[String],
) -> Result<(), ProjectError> {
    debug_assert!(
        slugs.iter().all(|s| level_of(s) == level),
        "set_level_order got slugs from another level: {slugs:?} is not all under {level:?}"
    );
    let meta = load_meta(root)?;
    let merged = merge_level(space_order(&meta, space), level, slugs);
    edit_project_toml(root, |doc| write_order(doc, space, &merged))
}

/// The level's requests in display order, as relative slugs — what the
/// sidebar shows for that level right now.
fn displayed_level(root: &Path, space: &str, level: &str) -> Result<Vec<String>, ProjectError> {
    let meta = load_meta(root)?;
    let (listing, _) = crate::storage::list_requests(root);
    let entries: Vec<&RequestListing> = listing
        .iter()
        .filter(|e| relative(&e.slug, space).is_some_and(|r| level_of(r) == level))
        .collect();
    Ok(order_level(&entries, space_order(&meta, space), space)
        .into_iter()
        .filter_map(|e| relative(&e.slug, space).map(str::to_string))
        .collect())
}

/// Moves `slug` (a full slug, `space/…`) by `delta` positions among its
/// siblings, clamped to the level's ends. The level's displayed order is
/// materialised into the list first, so what is on disk afterwards is
/// exactly what was on screen.
pub fn move_request(root: &Path, slug: &str, delta: i32) -> Result<(), ProjectError> {
    let space = crate::storage::space_of(slug)
        .ok_or_else(|| ProjectError::NotFound(slug.to_string()))?;
    let rel = relative(slug, space).ok_or_else(|| ProjectError::NotFound(slug.to_string()))?;
    let level = level_of(rel);
    let mut shown = displayed_level(root, space, level)?;
    let Some(pos) = shown.iter().position(|s| s == rel) else {
        return Err(ProjectError::NotFound(slug.to_string()));
    };
    let target = (pos as i32 + delta).clamp(0, shown.len() as i32 - 1) as usize;
    // Already at the end the move asks for: write nothing. Otherwise a
    // no-op alt+↑ on the first row of a never-ordered level would
    // materialise the whole level into `project.toml`.
    if target == pos {
        return Ok(());
    }
    let moved = shown.remove(pos);
    shown.insert(target, moved);
    set_level_order(root, space, level, &shown)
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

    use crate::model::HttpRequest;
    use crate::project::load_meta;
    use tempfile::tempdir;

    fn req() -> HttpRequest {
        HttpRequest::from_toml_str("url = \"https://x\"").unwrap()
    }

    fn project_with(slugs: &[&str]) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        crate::storage::ensure_project(dir.path()).unwrap();
        for s in slugs {
            crate::storage::save_request(dir.path(), s, &req()).unwrap();
        }
        dir
    }

    fn order_of(root: &std::path::Path, space: &str) -> Vec<String> {
        space_order(&load_meta(root).unwrap(), space).to_vec()
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
    fn set_level_order_writes_only_that_level_and_keeps_comments() {
        let dir = project_with(&["main/a", "main/b", "main/auth/x"]);
        std::fs::write(
            dir.path().join("project.toml"),
            "# keep me\nspaces = [\"main\"]\n\n[space.main]\nname = \"Main\"\norder = [\"auth/x\"]\n",
        )
        .unwrap();
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["auth/x", "b", "a"]));
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(text.starts_with("# keep me\n"), "{text}");
        assert!(text.contains("name = \"Main\""), "{text}");
    }

    #[test]
    fn set_level_order_creates_the_space_table_when_missing() {
        let dir = project_with(&["main/a", "main/b"]);
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["b", "a"]));
    }

    #[test]
    fn move_request_materialises_the_level_on_first_use_and_clamps() {
        let dir = project_with(&["main/a", "main/b", "main/c", "main/auth/x"]);
        move_request(dir.path(), "main/c", -1).unwrap();
        // Display order was a, b, c (alphabetical); c moved up one.
        assert_eq!(order_of(dir.path(), "main"), v(&["a", "c", "b"]));
        move_request(dir.path(), "main/a", -1).unwrap(); // already first
        assert_eq!(order_of(dir.path(), "main"), v(&["a", "c", "b"]));
        move_request(dir.path(), "main/a", 5).unwrap(); // clamps to last
        assert_eq!(order_of(dir.path(), "main"), v(&["c", "b", "a"]));
        assert!(matches!(
            move_request(dir.path(), "main/nope", 1),
            Err(crate::project::ProjectError::NotFound(_))
        ));
    }

    #[test]
    fn a_move_that_changes_nothing_writes_nothing() {
        // alt+↑ on the first row of a never-ordered level must not
        // materialise the whole level into `project.toml`.
        let dir = project_with(&["main/a", "main/b", "main/c"]);
        let path = dir.path().join("project.toml");
        let before = std::fs::read(&path).ok();
        move_request(dir.path(), "main/a", -1).unwrap(); // already first
        move_request(dir.path(), "main/c", 3).unwrap(); // already last
        move_request(dir.path(), "main/b", 0).unwrap(); // nowhere
        assert_eq!(
            std::fs::read(&path).ok(),
            before,
            "project.toml is byte identical (still absent, here)"
        );
        assert!(order_of(dir.path(), "main").is_empty());
    }

    #[test]
    fn move_request_in_a_folder_touches_only_that_level() {
        let dir = project_with(&["main/a", "main/auth/x", "main/auth/y"]);
        set_level_order(dir.path(), "main", "", &v(&["a"])).unwrap();
        move_request(dir.path(), "main/auth/y", -1).unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["a", "auth/y", "auth/x"]));
    }

    #[test]
    fn move_request_keeps_a_stale_entry_in_its_slot() {
        let dir = project_with(&["main/a", "main/b"]);
        set_level_order(dir.path(), "main", "", &v(&["gone", "a", "b"])).unwrap();
        move_request(dir.path(), "main/b", -1).unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["gone", "b", "a"]));
    }

    #[test]
    fn rename_within_a_level_keeps_the_slot() {
        let dir = project_with(&["main/a", "main/b"]);
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        order_rename(dir.path(), "main", "b", "bee").unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["bee", "a"]));
        // Unlisted: nothing to do, nothing written.
        order_rename(dir.path(), "main", "zzz", "yyy").unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["bee", "a"]));
    }

    #[test]
    fn rename_across_levels_removes_then_arrives() {
        let dir = project_with(&["main/a", "main/b", "main/auth/x"]);
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        set_level_order(dir.path(), "main", "auth", &v(&["auth/x"])).unwrap();
        order_rename(dir.path(), "main", "b", "auth/b").unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["a", "auth/x", "auth/b"]));
    }

    #[test]
    fn arrive_appends_only_when_the_level_is_listed() {
        let dir = project_with(&["main/a", "main/auth/x"]);
        order_arrive(dir.path(), "main", "new").unwrap();
        assert!(order_of(dir.path(), "main").is_empty(), "unlisted level: no write");
        set_level_order(dir.path(), "main", "", &v(&["a"])).unwrap();
        order_arrive(dir.path(), "main", "new").unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["a", "new"]));
        order_arrive(dir.path(), "main", "new").unwrap(); // already there
        assert_eq!(order_of(dir.path(), "main"), v(&["a", "new"]));
        // A folder level with no entries stays unlisted even though the
        // root is listed.
        order_arrive(dir.path(), "main", "auth/y").unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["a", "new"]));
    }

    #[test]
    fn remove_drops_the_entry_and_the_key_when_empty() {
        let dir = project_with(&["main/a"]);
        set_level_order(dir.path(), "main", "", &v(&["a"])).unwrap();
        order_remove(dir.path(), "main", "a").unwrap();
        assert!(order_of(dir.path(), "main").is_empty());
        let text = std::fs::read_to_string(dir.path().join("project.toml")).unwrap();
        assert!(!text.contains("order"), "{text}");
        order_remove(dir.path(), "main", "a").unwrap(); // absent: no-op
    }

    #[test]
    fn insert_after_lands_the_copy_next_to_its_source() {
        let dir = project_with(&["main/a", "main/b"]);
        set_level_order(dir.path(), "main", "", &v(&["b", "a"])).unwrap();
        order_insert_after(dir.path(), "main", "b", "b-copy").unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["b", "b-copy", "a"]));
        // Unlisted anchor: the level has no list, nothing written.
        order_insert_after(dir.path(), "main", "zzz", "zzz-copy").unwrap();
        assert_eq!(order_of(dir.path(), "main"), v(&["b", "b-copy", "a"]));
    }
}
