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
    listed
        .into_iter()
        .map(|(_, e)| e)
        .chain(unlisted)
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
}
