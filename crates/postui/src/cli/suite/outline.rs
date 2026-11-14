//! The suite's list as the sidebar nests it: a line for each space, each
//! folder where the path changes, and each request, in sidebar order (spec
//! 2026-11-13 §5.1). The live view draws these lines; the run loop numbers
//! the requests by `index`.

use postui_core::model::Method;

/// One line of the suite's list. As visible as [`super::Plan`], which holds
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::cli) enum Line {
    /// A space, by its display name.
    Space { name: String },
    /// A folder, by its display name, `depth` levels in (1 is directly under
    /// its space).
    Folder { depth: usize, name: String },
    /// A request: `index` counts requests across every space in order;
    /// `method` is `None` for a broken file.
    Request { depth: usize, index: usize, method: Option<Method>, name: String },
}

/// A request as the listing gives it: its slug, method and display name.
pub(super) struct Listed {
    pub slug: String,
    pub method: Option<Method>,
    pub name: String,
}

/// A space's requests in sidebar order: its slug, display name and
/// requests.
pub(super) struct SpaceRequests {
    pub slug: String,
    pub name: String,
    pub requests: Vec<Listed>,
}

/// The lines of `spaces`, in order. A folder line goes out for each segment
/// of a request's folder path not shared with the previous request's, named
/// by `folder_name` of its full path (`"main/users"`).
pub(super) fn outline(spaces: &[SpaceRequests], folder_name: &dyn Fn(&str) -> String) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut index = 0;
    for space in spaces {
        lines.push(Line::Space { name: space.name.clone() });
        let mut open: Vec<&str> = Vec::new();
        for request in &space.requests {
            // The slug less its space and its leaf.
            let segments: Vec<&str> = request.slug.split('/').collect();
            let folders = segments.get(1..segments.len().saturating_sub(1)).unwrap_or_default();
            let shared = open.iter().zip(folders).take_while(|(a, b)| a == b).count();
            for (at, _) in folders.iter().enumerate().skip(shared) {
                let path = format!("{}/{}", space.slug, folders[..=at].join("/"));
                lines.push(Line::Folder { depth: 1 + at, name: folder_name(&path) });
            }
            open = folders.to_vec();
            lines.push(Line::Request {
                depth: 1 + folders.len(),
                index,
                method: request.method,
                name: request.name.clone(),
            });
            index += 1;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(slug: &str, method: Method, name: &str) -> Listed {
        Listed { slug: slug.into(), method: Some(method), name: name.into() }
    }

    fn space(slug: &str, name: &str, requests: Vec<Listed>) -> SpaceRequests {
        SpaceRequests { slug: slug.into(), name: name.into(), requests }
    }

    /// A folder's "display name": its whole path, spelled so the test sees
    /// the full path was what was asked for.
    fn folder_name(path: &str) -> String {
        format!("{path}/").replace('/', "|")
    }

    fn request(depth: usize, index: usize, method: Method, name: &str) -> Line {
        Line::Request { depth, index, method: Some(method), name: name.into() }
    }

    #[test]
    fn outline_nests_folders_under_their_space() {
        let spaces = [
            space(
                "main",
                "Main",
                vec![
                    listed("main/users/get", Method::Get, "Get user"),
                    listed("main/users/rm", Method::Delete, "Remove user"),
                    listed("main/list", Method::Post, "List"),
                ],
            ),
            space("admin", "Admin", vec![listed("admin/health", Method::Get, "Health")]),
        ];
        assert_eq!(
            outline(&spaces, &folder_name),
            [
                Line::Space { name: "Main".into() },
                Line::Folder { depth: 1, name: "main|users|".into() },
                request(2, 0, Method::Get, "Get user"),
                request(2, 1, Method::Delete, "Remove user"),
                request(1, 2, Method::Post, "List"),
                Line::Space { name: "Admin".into() },
                request(1, 3, Method::Get, "Health"),
            ]
        );
    }

    #[test]
    fn outline_reopens_a_folder_only_when_the_path_changes() {
        let spaces = [space(
            "main",
            "main",
            vec![
                listed("main/a/x", Method::Get, "x"),
                listed("main/a/b/y", Method::Get, "y"),
                listed("main/a/z", Method::Get, "z"),
                listed("main/q", Method::Get, "q"),
            ],
        )];
        assert_eq!(
            outline(&spaces, &folder_name),
            [
                Line::Space { name: "main".into() },
                Line::Folder { depth: 1, name: "main|a|".into() },
                request(2, 0, Method::Get, "x"),
                Line::Folder { depth: 2, name: "main|a|b|".into() },
                request(3, 1, Method::Get, "y"),
                request(2, 2, Method::Get, "z"),
                request(1, 3, Method::Get, "q"),
            ]
        );
    }

    #[test]
    fn a_space_with_no_requests_still_has_its_line() {
        let spaces = [space("main", "Main", vec![])];
        assert_eq!(outline(&spaces, &folder_name), [Line::Space { name: "Main".into() }]);
    }

    /// A broken file has no method; its line keeps that.
    #[test]
    fn a_broken_request_has_no_method() {
        let broken = Listed { slug: "main/bad".into(), method: None, name: "bad".into() };
        let spaces = [space("main", "main", vec![broken])];
        assert_eq!(
            outline(&spaces, &folder_name)[1],
            Line::Request { depth: 1, index: 0, method: None, name: "bad".into() }
        );
    }
}
