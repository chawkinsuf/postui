//! The one mask for secret values in `run` and `suite` output (spec
//! 2026-10-28 R11): [`Mask::apply`] swaps each of the environment's secret
//! values, as written, for `SECRET_MASK` in what is printed. The request side
//! is not masked here: it prints as the TUI shows it (`prepare`'s display
//! forms). A secret the server sends back re-encoded or escaped is not caught.

use postui_core::prepare::SECRET_MASK;
use postui_core::project::Project;
use postui_core::varmodel::VarMeta;

/// A secret shorter than this many characters is not masked: it would
/// mangle ordinary text. `check` warns about such secrets.
pub const MASK_MIN_CHARS: usize = 4;

/// The secret values to hide.
pub struct Mask {
    /// The values themselves, those long enough, each once.
    values: Vec<String>,
}

impl Mask {
    /// A mask for `values`, leaving out those under [`MASK_MIN_CHARS`]
    /// characters and repeats.
    pub fn new(values: impl IntoIterator<Item = String>) -> Mask {
        let mut values: Vec<String> =
            values.into_iter().filter(|v| v.chars().count() >= MASK_MIN_CHARS).collect();
        values.sort_unstable();
        values.dedup();
        Mask { values }
    }

    /// A mask that hides nothing (`--show-secrets`).
    pub fn none() -> Mask {
        Mask { values: Vec::new() }
    }

    /// `text` with every secret value replaced by `SECRET_MASK`: one pass,
    /// all matches of every value found in `text` itself (a value's matches
    /// may overlap each other), overlapping or adjacent ones merged, one
    /// mask written per merged range. (Replacing value by value would leave
    /// the tail of a secret that overlaps another one, or match inside a
    /// mask already written.)
    pub fn apply(&self, text: &str) -> String {
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for v in &self.values {
            let mut from = 0;
            while let Some(at) = text[from..].find(v.as_str()).map(|i| from + i) {
                ranges.push((at, at + v.len()));
                // The next try starts one character on, so an occurrence
                // that overlaps this one is found too.
                from = at + text[at..].chars().next().map_or(1, char::len_utf8);
            }
        }
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

    /// `value` with the mask applied to every string, number and object key,
    /// so the masked JSON is still valid JSON (a mask inside the raw text
    /// could break it). Two keys that mask to the same text collapse to one
    /// entry: accepted (it takes secret-bearing keys), and `body` still
    /// shows the raw text.
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
            // A number whose text holds a secret becomes a masked string:
            // the raw `body` text masks the same digits, and leaving them
            // would print the secret's digits. The type change is accepted;
            // it only happens for a number holding a numeric secret of 4 or
            // more characters.
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

/// The mask for the secrets of the environment `project` has active, and
/// for `given`: values the command line gives secrets (`--var`).
pub fn of_env(project: &Project, given: impl IntoIterator<Item = String>) -> Mask {
    Mask::new(secret_values(project).into_iter().map(|(_, value)| value).chain(given))
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

    #[test]
    fn overlapping_secrets_are_masked_as_one_range() {
        let m = mask(&["abcdef", "defghi"]);
        assert_eq!(m.apply("x abcdefghi y"), format!("x {SECRET_MASK} y"));
        // Adjacent ranges merge too; apart ones stay two.
        assert_eq!(m.apply("abcdefdefghi"), SECRET_MASK);
        assert_eq!(m.apply("abcdef abcdef"), format!("{SECRET_MASK} {SECRET_MASK}"));
        assert_eq!(m.apply("no secret"), "no secret");
    }

    /// An occurrence that overlaps an earlier one of the same secret is
    /// found too: `abcab` in `abcabcab` leaves no `cab` behind.
    #[test]
    fn a_secret_overlapping_itself_is_masked_whole() {
        assert_eq!(mask(&["abcab"]).apply("abcabcab"), SECRET_MASK);
        assert_eq!(mask(&["äbcä"]).apply("äbcäbcä"), SECRET_MASK);
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
