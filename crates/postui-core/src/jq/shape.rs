//! A structure-only summary of a JSON body for the AI prompt: key names
//! and types, never values.

use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub struct ShapeLimits {
    pub max_depth: usize,
    pub max_bytes: usize,
    pub max_keys: usize,
}

impl Default for ShapeLimits {
    fn default() -> Self {
        Self { max_depth: 6, max_bytes: 4096, max_keys: 40 }
    }
}

/// Structure only: keys kept, scalars replaced by type names, arrays sampled
/// to one element with a length hint, depth and size capped. Non-JSON →
/// `None`.
pub fn shape(body: &str, limits: ShapeLimits) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let mut out = String::new();
    write_shape(&value, 0, &limits, &mut out);
    if out.len() > limits.max_bytes {
        let mut cut = limits.max_bytes;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    Some(out)
}

fn write_shape(value: &Value, depth: usize, limits: &ShapeLimits, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(_) => out.push_str("boolean"),
        Value::Number(_) => out.push_str("number"),
        Value::String(s) => {
            out.push_str("string");
            if let Some(tag) = string_tag(s) {
                out.push_str(" (");
                out.push_str(tag);
                out.push(')');
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            if depth >= limits.max_depth {
                out.push('…');
                return;
            }
            out.push('[');
            write_shape(&items[0], depth + 1, limits, out);
            out.push(']');
            out.push_str(&format!(" /* {} items */", items.len()));
        }
        Value::Object(map) => {
            if depth >= limits.max_depth {
                out.push('…');
                return;
            }
            out.push('{');
            for (i, (k, v)) in map.iter().take(limits.max_keys).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&serde_json::to_string(k).expect("strings always encode"));
                out.push_str(": ");
                write_shape(v, depth + 1, limits, out);
            }
            if map.len() > limits.max_keys {
                out.push_str(&format!(r#", "…": "+{} more keys""#, map.len() - limits.max_keys));
            }
            out.push('}');
        }
    }
}

fn string_tag(s: &str) -> Option<&'static str> {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|x| x.iter().all(u8::is_ascii_digit));
    if b.len() >= 10 && digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10) {
        return Some("date-time");
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return Some("url");
    }
    if b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
    {
        return Some("uuid");
    }
    if let Some((user, host)) = s.split_once('@')
        && !user.is_empty()
        && host.contains('.')
        && !host.contains(' ')
    {
        return Some("email");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_become_type_names_and_keys_are_kept() {
        let s = shape(r#"{"id": 1, "name": "x", "ok": true, "none": null}"#, ShapeLimits::default()).unwrap();
        assert_eq!(s, r#"{"id": number, "name": string, "ok": boolean, "none": null}"#);
    }

    #[test]
    fn arrays_are_sampled_to_their_first_element_with_a_length_hint() {
        let s = shape(r#"{"items": [{"a": 1}, {"a": 2}, {"a": 3}], "empty": []}"#, ShapeLimits::default()).unwrap();
        assert_eq!(s, r#"{"items": [{"a": number}] /* 3 items */, "empty": []}"#);
    }

    #[test]
    fn recognisable_string_formats_are_tagged() {
        let s = shape(
            r#"{"t": "2026-09-03T10:00:00Z", "u": "https://x.test/a", "e": "a@b.co", "id": "123e4567-e89b-12d3-a456-426614174000"}"#,
            ShapeLimits::default(),
        )
        .unwrap();
        assert_eq!(
            s,
            r#"{"t": string (date-time), "u": string (url), "e": string (email), "id": string (uuid)}"#
        );
    }

    #[test]
    fn keys_past_the_cap_are_summarised() {
        let body = r#"{"a":1,"b":2,"c":3,"d":4}"#;
        let s = shape(body, ShapeLimits { max_keys: 2, ..ShapeLimits::default() }).unwrap();
        assert_eq!(s, r#"{"a": number, "b": number, "…": "+2 more keys"}"#);
    }

    #[test]
    fn depth_past_the_cap_is_elided() {
        let body = r#"{"a":{"b":{"c":{"d":1}}}}"#;
        let s = shape(body, ShapeLimits { max_depth: 2, ..ShapeLimits::default() }).unwrap();
        assert_eq!(s, r#"{"a": {"b": …}}"#);
    }

    #[test]
    fn output_past_the_byte_cap_is_truncated_with_an_ellipsis() {
        let body = r#"{"alpha": 1, "beta": 2, "gamma": 3}"#;
        let s = shape(body, ShapeLimits { max_bytes: 20, ..ShapeLimits::default() }).unwrap();
        assert!(s.len() <= 20 + '…'.len_utf8(), "len {}: {s}", s.len());
        assert!(s.ends_with('…'), "{s}");
        assert!(s.is_char_boundary(s.len() - '…'.len_utf8()));
    }

    #[test]
    fn non_json_has_no_shape() {
        assert_eq!(shape("nope", ShapeLimits::default()), None);
    }
}
