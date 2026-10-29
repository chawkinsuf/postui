//! The one mask for secret values in `run` and `suite` output (spec
//! 2026-10-28 R11): every string printed passes through [`Mask::apply`] (or
//! one of the ways in below that end in it), which swaps each of the
//! environment's secret values for `SECRET_MASK`.

use postui_core::prepare::{PreparedRequest, SECRET_MASK};
use postui_core::project::Project;
use postui_core::varmodel::VarMeta;

/// A secret shorter than this many characters is not masked: it would
/// mangle ordinary text. `check` warns about such secrets.
pub const MASK_MIN_CHARS: usize = 4;

/// The secret values to hide.
pub struct Mask {
    /// The values themselves, those long enough, each once.
    values: Vec<String>,
    /// `--show-secrets`: nothing is hidden, and nothing is added later.
    shows_all: bool,
}

impl Mask {
    /// A mask for `values`, leaving out those under [`MASK_MIN_CHARS`]
    /// characters and repeats. Only the values as they are: a request adds
    /// what it sends of them in other spellings ([`Mask::with_wire`]), and
    /// JSON's escapes are read through ([`Mask::apply_literals`]).
    pub fn new(values: impl IntoIterator<Item = String>) -> Mask {
        let mut values: Vec<String> =
            values.into_iter().filter(|v| v.chars().count() >= MASK_MIN_CHARS).collect();
        values.sort_unstable();
        values.dedup();
        Mask { values, shows_all: false }
    }

    /// A mask that hides nothing (`--show-secrets`).
    pub fn none() -> Mask {
        Mask { values: Vec::new(), shows_all: true }
    }

    /// This mask plus what `request` sends of a secret in another spelling
    /// (the Basic credentials of a URL's userinfo, a query's encoding, the
    /// client's percent-encoding of the URL): `prepare` lists each exactly
    /// as it goes out ([`PreparedRequest::secret_wire`]).
    pub fn with_wire(&self, request: &PreparedRequest) -> Mask {
        if self.shows_all {
            return Mask::none();
        }
        Mask::new(self.values.iter().chain(&request.secret_wire).cloned())
    }

    /// `text` with every secret value replaced by `SECRET_MASK`: one pass,
    /// all matches of every value found in `text` itself, overlapping or
    /// adjacent ones merged, one mask written per merged range. (Replacing
    /// value by value would leave the tail of a secret that overlaps
    /// another one, or match inside a mask already written.)
    pub fn apply(&self, text: &str) -> String {
        let mut ranges: Vec<(usize, usize)> = self
            .values
            .iter()
            .flat_map(|v| text.match_indices(v.as_str()).map(|(at, m)| (at, at + m.len())))
            .collect();
        if ranges.is_empty() {
            return text.to_string();
        }
        ranges.sort_unstable();
        let mut out = String::with_capacity(text.len());
        let mut done = 0;
        let mut iter = ranges.into_iter();
        let (mut start, mut end) = iter.next().unwrap_or_default();
        for (s, e) in iter {
            if s <= end {
                end = end.max(e);
                continue;
            }
            out.push_str(&text[done..start]);
            out.push_str(SECRET_MASK);
            done = end;
            (start, end) = (s, e);
        }
        out.push_str(&text[done..start]);
        out.push_str(SECRET_MASK);
        out.push_str(&text[end..]);
        out
    }

    /// `text` masked JSON string literal by literal: each well-formed one
    /// (a key too) is decoded, masked, and only when that changed it written
    /// back as serde_json writes the masked text; the text between them
    /// (numbers, punctuation) gets [`Mask::apply`]. A secret a server wrote
    /// with escapes (`\u002d`, `\/`, `\u0026`) is caught however it was
    /// escaped, and what the mask doesn't touch stays byte for byte. A `"`
    /// that doesn't open a well-formed literal is plain text. For a JSON
    /// body's raw text, and for text that quotes JSON values (jq's errors).
    pub fn apply_literals(&self, text: &str) -> String {
        if self.values.is_empty() {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        // Where the text not yet written starts, and where to look next.
        let (mut written, mut at) = (0, 0);
        while let Some(offset) = text[at..].find('"') {
            let start = at + offset;
            let mut literals =
                serde_json::Deserializer::from_str(&text[start..]).into_iter::<String>();
            let Some(Ok(decoded)) = literals.next() else {
                at = start + 1;
                continue;
            };
            let end = start + literals.byte_offset();
            out.push_str(&self.apply(&text[written..start]));
            let masked = self.apply(&decoded);
            if masked == decoded {
                out.push_str(&text[start..end]);
            } else {
                out.push_str(&serde_json::to_string(&masked).unwrap_or_default());
            }
            (written, at) = (end, end);
        }
        out.push_str(&self.apply(&text[written..]));
        out
    }

    /// `value` with the mask applied to every string, number and object key:
    /// a secret the server wrote with JSON escapes (`-`, `\/`) is plain
    /// text once parsed, so masking the parsed value is what catches it.
    /// Two keys that mask to the same text collapse to one entry: accepted
    /// (it takes secret-bearing keys), and `body` still shows the raw text.
    pub fn apply_json(&self, value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::String(s) => Value::String(self.apply(&s)),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| self.apply_json(v)).collect())
            }
            Value::Object(map) => Value::Object(
                map.into_iter().map(|(k, v)| (self.apply(&k), self.apply_json(v))).collect(),
            ),
            // A number the mask would change (a numeric secret) is shown
            // as the masked text.
            Value::Number(n) => {
                let text = n.to_string();
                let masked = self.apply(&text);
                if masked == text { Value::Number(n) } else { Value::String(masked) }
            }
            other => other,
        }
    }
}

/// The secrets of the environment `project` has active, as (name, value);
/// a secret with no value isn't listed.
pub fn secret_values(project: &Project) -> Vec<(String, String)> {
    let resolved = project.resolved();
    resolved
        .meta
        .iter()
        .filter(|(_, meta)| **meta == VarMeta::Secret)
        .filter_map(|(name, _)| Some((name.clone(), resolved.values.get(name)?.clone())))
        .collect()
}

/// The mask for the secrets of the environment `project` has active.
pub fn of_env(project: &Project) -> Mask {
    Mask::new(secret_values(project).into_iter().map(|(_, value)| value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(values: &[&str]) -> Mask {
        Mask::new(values.iter().map(|v| v.to_string()))
    }

    #[test]
    fn every_occurrence_is_masked() {
        assert_eq!(
            mask(&["s3cret"]).apply("a s3cret b s3cret"),
            format!("a {SECRET_MASK} b {SECRET_MASK}")
        );
    }

    #[test]
    fn a_value_under_the_minimum_is_left_alone() {
        assert_eq!(mask(&["abc"]).apply("abc"), "abc");
    }

    #[test]
    fn a_secret_inside_another_is_masked_whole() {
        let m = mask(&["token", "token-long"]);
        assert_eq!(m.apply("token-long"), SECRET_MASK);
        assert_eq!(m.apply("token"), SECRET_MASK);
    }

    #[test]
    fn none_masks_nothing() {
        assert_eq!(Mask::none().apply("s3cret"), "s3cret");
        assert_eq!(Mask::none().apply_literals(r#"{"t":"s3cret"}"#), r#"{"t":"s3cret"}"#);
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        let m = mask(&["pässwörd", "äöü"]);
        assert_eq!(m.apply("pässwörd äöü"), format!("{SECRET_MASK} äöü"));
    }

    #[test]
    fn apply_json_masks_strings_and_keys_at_any_depth() {
        let value = serde_json::json!({
            "s3cret": ["a s3cret", 1, null, {"k": "s3cret\"x"}],
            "n": true
        });
        let masked = mask(&["s3cret"]).apply_json(value);
        let want = serde_json::json!({
            SECRET_MASK: [format!("a {SECRET_MASK}"), 1, null, {"k": format!("{SECRET_MASK}\"x")}],
            "n": true
        });
        assert_eq!(masked, want);
    }

    #[test]
    fn apply_json_masks_a_number_that_is_the_secret() {
        let m = mask(&["123456"]);
        let value = serde_json::json!({"a": 123456, "b": 12, "c": [123456.5, 0.25]});
        assert_eq!(
            m.apply_json(value),
            serde_json::json!({"a": SECRET_MASK, "b": 12, "c": [format!("{SECRET_MASK}.5"), 0.25]})
        );
    }

    /// However a server escapes a secret in a JSON string, decoding the
    /// literal reads through it: JSON escapes, `\/`, `\uXXXX` for ASCII
    /// and non-ASCII alike, and Go's `\u0026`, `\u003c`, `\u003e`.
    #[test]
    fn apply_literals_reads_through_every_escape() {
        let m = mask(&["ab/cd\"ef\\gh"]);
        for spelled in [r#"ab/cd\"ef\\gh"#, r#"ab\/cd\"ef\\gh"#, r#"\u0061b/cd\"ef\\gh"#] {
            let body = format!(r#"{{"t":"{spelled}"}}"#);
            assert_eq!(m.apply_literals(&body), format!(r#"{{"t":"{SECRET_MASK}"}}"#), "{body}");
        }
        let m = mask(&["pässwörd😀"]);
        let body = r#"["p\u00e4ssw\u00f6rd\ud83d\ude00"]"#;
        assert_eq!(m.apply_literals(body), format!(r#"["{SECRET_MASK}"]"#));
        let m = mask(&["a&b<c>d"]);
        let body = r#"{"t":"a\u0026b\u003cc\u003ed"}"#;
        assert_eq!(m.apply_literals(body), format!(r#"{{"t":"{SECRET_MASK}"}}"#));
    }

    /// What the mask doesn't change stays byte for byte, escapes and
    /// spacing included; a masked literal is written as serde_json writes
    /// it; keys and numbers are masked too.
    #[test]
    fn apply_literals_leaves_untouched_text_as_it_was() {
        let m = mask(&["s3cret", "123456"]);
        let body = "{ \"keep\" : \"a\\/b \\u00e9\",\n  \"s3cret\": [123456, 7, \"x s3cret \\/\"] }";
        assert_eq!(
            m.apply_literals(body),
            format!(
                "{{ \"keep\" : \"a\\/b \\u00e9\",\n  \"{SECRET_MASK}\": \
                 [{SECRET_MASK}, 7, \"x {SECRET_MASK} /\"] }}"
            )
        );
        assert_eq!(m.apply_literals(r#"{"a":"plain"}"#), r#"{"a":"plain"}"#);
    }

    /// Text that isn't JSON: each well-formed literal in it is read through,
    /// the rest masked as plain text, a stray quote left as it is.
    #[test]
    fn apply_literals_in_text_that_quotes_json() {
        let m = mask(&["ab\"cd"]);
        assert_eq!(
            m.apply_literals(r#"cannot use "ab\"cd" (ab"cd) as a number"#),
            format!(r#"cannot use "{SECRET_MASK}" ({SECRET_MASK}) as a number"#)
        );
        let m = mask(&["s3cret"]);
        assert_eq!(m.apply_literals("a \" s3cret"), format!("a \" {SECRET_MASK}"));
    }

    fn sent(secret_wire: &[&str]) -> PreparedRequest {
        PreparedRequest {
            method: Default::default(),
            url: String::new(),
            display_url: String::new(),
            headers: Vec::new(),
            wire: Vec::new(),
            body: None,
            insecure: false,
            secret_wire: secret_wire.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn with_wire_adds_what_prepare_lists_under_the_same_minimum() {
        let m = mask(&["s3cret"]).with_wire(&sent(&["dXNlcjpzM2NyZXQ=", "ab"]));
        assert_eq!(m.apply("Basic dXNlcjpzM2NyZXQ="), format!("Basic {SECRET_MASK}"));
        assert_eq!(m.apply("s3cret"), SECRET_MASK);
        assert_eq!(m.apply("ab"), "ab", "a spelling under the minimum is left");
    }

    #[test]
    fn with_wire_leaves_show_secrets_alone() {
        let m = Mask::none().with_wire(&sent(&["dXNlcjpzM2NyZXQ="]));
        assert_eq!(m.apply("dXNlcjpzM2NyZXQ="), "dXNlcjpzM2NyZXQ=");
    }

    #[test]
    fn overlapping_secrets_are_masked_as_one_range() {
        let m = mask(&["abcdef", "defghi"]);
        assert_eq!(m.apply("x abcdefghi y"), format!("x {SECRET_MASK} y"));
        // Adjacent ranges merge too; apart ones stay two.
        assert_eq!(m.apply("abcdefdefghi"), SECRET_MASK);
        assert_eq!(m.apply("abcdef abcdef"), format!("{SECRET_MASK} {SECRET_MASK}"));
        assert_eq!(m.apply("no secret"), "no secret");
    }

    #[test]
    fn a_secret_holding_the_mask_character_is_masked() {
        let m = mask(&["pa●●word"]);
        assert_eq!(m.apply("a pa●●word b"), format!("a {SECRET_MASK} b"));
        // The text's own mask is not a secret.
        assert_eq!(m.apply(SECRET_MASK), SECRET_MASK);
    }

    #[test]
    fn duplicates_are_harmless() {
        assert_eq!(mask(&["s3cret", "s3cret"]).apply("s3cret"), SECRET_MASK);
    }
}
