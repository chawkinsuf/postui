//! The user-chosen request order inside a space: `[space.<slug>] order =
//! [...]` in `project.toml`. Entries are slugs relative to the space.
//! Only the relative order among the siblings of one level (the space
//! root, or one folder) means anything; entries naming no file are
//! ignored for display and preserved on write.

use crate::project::ProjectMeta;
use crate::storage::RequestListing;

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
    listed.into_iter().map(|(_, e)| e).chain(unlisted).collect()
}

/// Writes `[space.<space>] order = [...]`, creating the table if needed
/// and dropping the key when the list is empty. Keeps the table's other
/// keys and the file's comments.
pub(crate) fn write_order(doc: &mut toml_edit::DocumentMut, space: &str, order: &[String]) {
    let table = doc
        .entry("space")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(t) = table.as_table_mut() else {
        return;
    };
    t.set_implicit(true);
    let item = t
        .entry(space)
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
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
    Inserted {
        space: String,
        rel: String,
        at: usize,
    },
    /// `rel` was removed from index `at`.
    Removed {
        space: String,
        rel: String,
        at: usize,
    },
    /// Every entry `from` became `to`.
    Renamed {
        space: String,
        from: String,
        to: String,
    },
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

/// Whether `level` has a displayed listed entry: one naming a request
/// that exists (`exists` answers for a full slug).
pub(crate) fn level_is_listed(
    space: &str,
    order: &[String],
    level: &str,
    exists: &dyn Fn(&str) -> bool,
) -> bool {
    order
        .iter()
        .any(|e| level_of(e) == level && exists(&format!("{space}/{e}")))
}

pub(crate) fn arrive(
    space: &str,
    order: &mut Vec<String>,
    rel: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<OrderEdit> {
    if order.iter().any(|e| e == rel) || !level_is_listed(space, order, level_of(rel), exists) {
        return Vec::new();
    }
    let at = order.len();
    order.push(rel.to_string());
    vec![OrderEdit::Inserted {
        space: space.to_string(),
        rel: rel.to_string(),
        at,
    }]
}

/// [`order_remove`]'s edit, on a list already in hand: every occurrence,
/// recorded last index first so that replaying the inverses backwards
/// re-inserts first index first.
pub(crate) fn remove(space: &str, order: &mut Vec<String>, rel: &str) -> Vec<OrderEdit> {
    let mut edits = Vec::new();
    for at in (0..order.len()).rev() {
        if order[at] == rel {
            order.remove(at);
            edits.push(OrderEdit::Removed {
                space: space.to_string(),
                rel: rel.to_string(),
                at,
            });
        }
    }
    edits
}

/// The list after one level is rewritten to `slugs`: entries of `level`
/// that appear in `slugs` are replaced in place, first slot first; slugs
/// with no slot to take are appended; every other entry (other levels,
/// entries of this level not named in `slugs` — stale ones) keeps its
/// slot. Duplicates in `existing` collapse to their first occurrence.
pub(crate) fn merge_level(existing: &[String], level: &str, slugs: &[String]) -> Vec<String> {
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

/// Every request of `space` in `listing`, as full slugs, in display
/// order: level by level (levels in slug order), each level under the
/// display rule of [`order_level`].
pub fn displayed_slugs(listing: &[RequestListing], order: &[String], space: &str) -> Vec<String> {
    let mut by_level: std::collections::BTreeMap<&str, Vec<&RequestListing>> =
        std::collections::BTreeMap::new();
    for e in listing {
        if let Some(rel) = relative(&e.slug, space) {
            by_level.entry(level_of(rel)).or_default().push(e);
        }
    }
    by_level
        .into_values()
        .flat_map(|entries| {
            order_level(&entries, order, space)
                .into_iter()
                .map(|e| e.slug.clone())
        })
        .collect()
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
