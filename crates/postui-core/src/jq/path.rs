//! jq paths for tree lines, and the one rule for growing a pipeline.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}

pub(crate) fn is_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `.` for the root, `.data.items[3].name`, `.["odd key"]` where needed.
pub fn render_path(path: &[PathSeg]) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let mut out = String::new();
    for seg in path {
        match seg {
            PathSeg::Key(k) => push_key(&mut out, k),
            PathSeg::Index(i) => push_bracket(&mut out, &i.to_string()),
        }
    }
    out
}

/// A key as jq spells it: bare when it is an identifier, a quoted string
/// otherwise (serde_json's string encoding is exactly jq's).
pub(crate) fn key_label(key: &str) -> String {
    if is_identifier(key) {
        key.to_string()
    } else {
        serde_json::to_string(key).expect("strings always encode")
    }
}

/// Appends `.key`, or `["odd key"]` for a key that is not an identifier.
pub(crate) fn push_key(out: &mut String, key: &str) {
    if is_identifier(key) {
        out.push('.');
        out.push_str(key);
    } else {
        push_bracket(out, &key_label(key));
    }
}

/// Appends `[inner]`: `[3]`, `["k"]`, `[]`.
pub(crate) fn push_bracket(out: &mut String, inner: &str) {
    // A leading "." is jq's identity before an index; bracket indexing
    // after another segment applies directly.
    if out.is_empty() {
        out.push('.');
    }
    out.push('[');
    out.push_str(inner);
    out.push(']');
}

/// Appends `path` and `expr` to `current` as one pipeline; see rules below.
pub fn compose(current: &str, path: &str, expr: Option<&str>) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if path != "." {
        parts.push(path);
    }
    if let Some(e) = expr {
        parts.push(e);
    }
    let current = current.trim_end();
    if parts.is_empty() {
        return current.to_string();
    }
    let tail = parts.join(" | ");
    if current.trim().is_empty() {
        tail
    } else {
        format!("{current} | {tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> PathSeg {
        PathSeg::Key(s.into())
    }

    #[test]
    fn the_root_renders_as_a_dot() {
        assert_eq!(render_path(&[]), ".");
    }

    #[test]
    fn identifier_keys_and_indexes_render_in_jq_dot_form() {
        let p = [key("data"), key("items"), PathSeg::Index(3), key("name")];
        assert_eq!(render_path(&p), ".data.items[3].name");
    }

    #[test]
    fn keys_that_are_not_identifiers_use_bracket_quoting_with_json_escapes() {
        assert_eq!(render_path(&[key("odd key")]), r#".["odd key"]"#);
        assert_eq!(render_path(&[key("a\"b")]), r#".["a\"b"]"#);
        assert_eq!(render_path(&[key("1st")]), r#".["1st"]"#);
        assert_eq!(render_path(&[key("_ok9")]), "._ok9");
        assert_eq!(render_path(&[key("")]), r#".[""]"#);
    }

    #[test]
    fn compose_on_an_empty_bar_is_just_the_path_and_verb() {
        assert_eq!(
            compose("", ".data.items", Some("length")),
            ".data.items | length"
        );
        assert_eq!(compose("   ", ".data.items", None), ".data.items");
        assert_eq!(compose("", ".", Some("length")), "length");
    }

    #[test]
    fn compose_appends_to_an_existing_filter_with_a_pipe() {
        assert_eq!(
            compose(".data", ".items", Some("length")),
            ".data | .items | length"
        );
        assert_eq!(compose(".data ", ".", Some("length")), ".data | length");
        assert_eq!(compose(".data", ".", None), ".data");
    }
}
