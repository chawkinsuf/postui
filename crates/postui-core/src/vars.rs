use indexmap::IndexMap;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSpan {
    pub name: String,
    /// Byte offset of the opening `{{`.
    pub start: usize,
    /// Byte offset one past the closing `}}`.
    pub end: usize,
}

pub fn is_valid_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Scans for well-formed `{{ name }}` tokens (optional inner whitespace).
/// Anything malformed is left for the caller to treat as literal text; a
/// failed match advances by one byte so `{{{a}}` still finds `{{a}}`.
pub fn find_tokens(text: &str) -> Vec<TokenSpan> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'{' || bytes[i + 1] != b'{' {
            i += 1;
            continue;
        }
        // try to parse `{{ \s* name \s* }}` starting at i
        let inner_start = i + 2;
        let Some(close) = text[inner_start..].find("}}").map(|p| inner_start + p) else {
            break; // no closing braces anywhere: nothing further can match
        };
        let name = text[inner_start..close].trim();
        if is_valid_var_name(name) {
            out.push(TokenSpan {
                name: name.to_string(),
                start: i,
                end: close + 2,
            });
            i = close + 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Replaces every token whose name is in `values`; tokens with no value stay
/// verbatim and their names are collected into `missing` (a set: each name
/// reported once, sorted).
pub fn substitute(
    text: &str,
    values: &IndexMap<String, String>,
    missing: &mut BTreeSet<String>,
) -> String {
    let tokens = find_tokens(text);
    if tokens.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for t in tokens {
        out.push_str(&text[last..t.start]);
        match values.get(&t.name) {
            Some(v) => out.push_str(v),
            None => {
                missing.insert(t.name.clone());
                out.push_str(&text[t.start..t.end]);
            }
        }
        last = t.end;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use std::collections::BTreeSet;

    fn vals(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn valid_names_are_alnum_dash_underscore() {
        assert!(is_valid_var_name("base_url"));
        assert!(is_valid_var_name("Token-2"));
        assert!(!is_valid_var_name(""));
        assert!(!is_valid_var_name("has space"));
        assert!(!is_valid_var_name("dotted.name"));
    }

    #[test]
    fn finds_simple_and_whitespace_padded_tokens() {
        let t = find_tokens("{{base_url}}/x/{{ id }}");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].name, "base_url");
        assert_eq!((t[0].start, t[0].end), (0, 12));
        assert_eq!(t[1].name, "id");
    }

    #[test]
    fn malformed_tokens_stay_literal() {
        assert!(find_tokens("{{unclosed").is_empty());
        assert!(find_tokens("{{bad name}}").is_empty());
        assert!(find_tokens("{ {x} }").is_empty());
        assert!(find_tokens("{{}}").is_empty());
        // a stray '{' immediately before a real token must not hide it
        let t = find_tokens("{{{a}}");
        assert_eq!(t.len(), 1, "{{ + {{a}} : the trailing {{a}} parses");
        assert_eq!(t[0].name, "a");
        assert_eq!((t[0].start, t[0].end), (1, 6));
    }

    #[test]
    fn adjacent_tokens_get_exact_touching_spans() {
        let t = find_tokens("{{a}}{{b}}");
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].start, t[0].end), (0, 5));
        assert_eq!((t[1].start, t[1].end), (5, 10));
        assert_eq!(t[1].name, "b");
    }

    #[test]
    fn spans_are_byte_offsets_past_multi_byte_text() {
        // "héllo " is 7 bytes (é is two), so the token starts at 7 — a
        // char-index would say 6 and slice the wrong bytes.
        let text = "héllo {{id}}";
        let t = find_tokens(text);
        assert_eq!((t[0].start, t[0].end), (7, 13));
        assert_eq!(&text[t[0].start..t[0].end], "{{id}}");
    }

    #[test]
    fn substitute_replaces_known_and_records_missing() {
        let mut missing = BTreeSet::new();
        let out = substitute(
            "{{base}}/u/{{id}}?q={{gone}}",
            &vals(&[("base", "http://x"), ("id", "7")]),
            &mut missing,
        );
        assert_eq!(out, "http://x/u/7?q={{gone}}");
        assert_eq!(
            missing.into_iter().collect::<Vec<_>>(),
            vec!["gone".to_string()]
        );
    }

    #[test]
    fn substitute_without_tokens_is_identity() {
        let mut missing = BTreeSet::new();
        assert_eq!(
            substitute("plain { braces }", &vals(&[]), &mut missing),
            "plain { braces }"
        );
        assert!(missing.is_empty());
    }

    #[test]
    fn substitute_is_single_pass_not_recursive() {
        let mut missing = BTreeSet::new();
        let out = substitute(
            "{{a}}",
            &vals(&[("a", "{{b}}"), ("b", "boom")]),
            &mut missing,
        );
        assert_eq!(out, "{{b}}", "a substituted value must not be re-scanned");
        assert!(missing.is_empty());
    }
}
