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
    fn duplicates_are_harmless() {
        assert_eq!(mask(&["s3cret", "s3cret"]).apply("s3cret"), SECRET_MASK);
    }
}
