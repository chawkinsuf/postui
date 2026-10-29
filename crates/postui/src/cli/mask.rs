//! The one mask for secret values in `run` and `suite` output (spec
//! 2026-10-28 R11): every string printed passes through [`Mask::apply`],
//! which swaps each of the environment's secret values for `SECRET_MASK`.

use postui_core::prepare::SECRET_MASK;
use postui_core::project::Project;
use postui_core::varmodel::VarMeta;

/// A secret shorter than this many characters is not masked: it would
/// mangle ordinary text. `check` warns about such secrets.
pub const MASK_MIN_CHARS: usize = 4;

/// The secret values to hide, longest first.
pub struct Mask {
    values: Vec<String>,
}

impl Mask {
    /// A mask for `values`, leaving out those under [`MASK_MIN_CHARS`]
    /// characters and repeats. Longest first, so a secret that contains
    /// another is masked whole.
    pub fn new(values: impl IntoIterator<Item = String>) -> Mask {
        let mut values: Vec<String> =
            values.into_iter().filter(|v| v.chars().count() >= MASK_MIN_CHARS).collect();
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        Mask { values }
    }

    /// A mask that hides nothing (`--show-secrets`).
    pub fn none() -> Mask {
        Mask { values: Vec::new() }
    }

    /// `text` with every secret value replaced by `SECRET_MASK`.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for value in &self.values {
            out = out.replace(value.as_str(), SECRET_MASK);
        }
        out
    }

    /// `value` with the mask applied to every string, number and object key:
    /// a secret the server wrote with JSON escapes (`-`, `\/`) is plain
    /// text once parsed, so masking the parsed value is what catches it.
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

/// The mask for the secrets of the environment `project` has active.
pub fn of_env(project: &Project) -> Mask {
    let resolved = project.resolved();
    Mask::new(
        resolved
            .meta
            .iter()
            .filter(|(_, meta)| **meta == VarMeta::Secret)
            .filter_map(|(name, _)| resolved.values.get(name).cloned()),
    )
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
    fn duplicates_are_harmless() {
        assert_eq!(mask(&["s3cret", "s3cret"]).apply("s3cret"), SECRET_MASK);
    }
}
