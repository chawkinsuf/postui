//! The one mask for secret values in `run` and `suite` output (spec
//! 2026-10-28 R11): every string printed passes through [`Mask::apply`],
//! which swaps each of the environment's secret values for `SECRET_MASK`.

use postui_core::prepare::{PreparedRequest, SECRET_MASK};
use postui_core::project::Project;
use postui_core::varmodel::VarMeta;

/// A secret shorter than this many characters is not masked: it would
/// mangle ordinary text. `check` warns about such secrets.
pub const MASK_MIN_CHARS: usize = 4;

/// The secret values to hide, in every spelling, longest first.
pub struct Mask {
    /// The secret values themselves (those long enough), for
    /// [`Mask::with_sent`] to build on.
    secrets: Vec<String>,
    /// Every spelling of them.
    values: Vec<String>,
    /// `--show-secrets`: nothing is hidden, and nothing is added later.
    shows_all: bool,
}

/// `v` as a JSON string spells it between the quotes (`"`, `\` and control
/// characters escaped).
fn json_spelling(v: &str) -> String {
    let quoted = serde_json::to_string(v).unwrap_or_default();
    let inner = quoted.strip_prefix('"').unwrap_or(&quoted);
    inner.strip_suffix('"').unwrap_or(inner).to_string()
}

/// `text` with every non-ASCII character as lowercase `\uXXXX` (a surrogate
/// pair for an astral one), as Python's `json.dumps` writes by default.
fn ascii_only(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// The spellings of `v` an echoing server may send back.
fn spellings(v: &str, into: &mut Vec<String>) {
    let json = json_spelling(v);
    let mut forms = vec![v.to_string(), json.clone()];
    if !v.is_ascii() {
        forms.push(ascii_only(&json));
    }
    for form in forms {
        if form.contains('/') {
            into.push(form.replace('/', "\\/"));
        }
        into.push(form);
    }
    // What `prepare` puts in a query string.
    into.push(form_urlencoded::byte_serialize(v.as_bytes()).collect());
}

impl Mask {
    /// A mask for `values`, leaving out those under [`MASK_MIN_CHARS`]
    /// characters and repeats. Each kept value is also masked as a JSON
    /// string spells it (`"`, `\` and control characters escaped; with `/`
    /// as `\/`, as PHP's `json_encode` does; non-ASCII as `\uXXXX`, as
    /// Python's does) and as a query string carries it (percent-encoded).
    /// Other encodings are not covered. Longest first, so a secret that
    /// contains another is masked whole.
    pub fn new(values: impl IntoIterator<Item = String>) -> Mask {
        let mut secrets: Vec<String> =
            values.into_iter().filter(|v| v.chars().count() >= MASK_MIN_CHARS).collect();
        secrets.sort();
        secrets.dedup();
        let mut values = Vec::new();
        for v in &secrets {
            spellings(v, &mut values);
        }
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        Mask { secrets, values, shows_all: false }
    }

    /// A mask that hides nothing (`--show-secrets`).
    pub fn none() -> Mask {
        Mask { secrets: Vec::new(), values: Vec::new(), shows_all: true }
    }

    /// This mask plus what `request` sends that is derived from a secret: a
    /// wire header whose `display` differs from its `value` (reqwest's
    /// `Authorization: Basic <base64>` from URL userinfo, say) carries a
    /// secret in some derived form, so the part of the value that differs
    /// from the display, past their common prefix and suffix, is hidden too.
    pub fn with_sent(&self, request: &PreparedRequest) -> Mask {
        if self.shows_all {
            return Mask::none();
        }
        let mut all = self.secrets.clone();
        for h in request.wire.iter().filter(|h| h.value != h.display) {
            let (value, display): (Vec<char>, Vec<char>) =
                (h.value.chars().collect(), h.display.chars().collect());
            let prefix = value.iter().zip(&display).take_while(|(a, b)| a == b).count();
            let suffix = value[prefix..]
                .iter()
                .rev()
                .zip(display[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            all.push(value[prefix..value.len() - suffix].iter().collect());
        }
        Mask::new(all)
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
    fn json_escaped_spellings_are_masked() {
        let m = mask(&["ab/cd\"ef\\gh"]);
        assert_eq!(m.apply("ab/cd\"ef\\gh"), SECRET_MASK);
        assert_eq!(m.apply(r#"ab/cd\"ef\\gh"#), SECRET_MASK);
        assert_eq!(m.apply(r#"ab\/cd\"ef\\gh"#), SECRET_MASK);
        let slash = mask(&["ab/cd/ef"]);
        assert_eq!(slash.apply(r"x ab\/cd\/ef y"), format!("x {SECRET_MASK} y"));
        // A value with nothing to escape adds no spelling.
        assert_eq!(mask(&["plain-1"]).values.len(), 1);
    }

    #[test]
    fn a_secret_ending_in_a_quote_keeps_its_escaped_quote() {
        // Only the JSON string's own two quotes are cut, not the escaped one.
        let m = mask(&["ab\"cd\""]);
        assert_eq!(m.apply(r#"x ab\"cd\" y"#), format!("x {SECRET_MASK} y"));
        assert_eq!(m.apply("x ab\"cd\" y"), format!("x {SECRET_MASK} y"));
    }

    #[test]
    fn the_query_string_spelling_is_masked() {
        let m = mask(&["a+b/c d=e&f"]);
        assert_eq!(m.apply("?k=a%2Bb%2Fc+d%3De%26f&z=1"), format!("?k={SECRET_MASK}&z=1"));
    }

    #[test]
    fn the_ascii_only_json_spelling_is_masked() {
        let m = mask(&["pässwörd😀"]);
        assert_eq!(m.apply(r"pässwörd😀"), SECRET_MASK);
        assert_eq!(m.apply("pässwörd😀"), SECRET_MASK);
    }

    fn sent(value: &str, display: &str) -> PreparedRequest {
        use postui_core::prepare::SentHeader;
        let header = SentHeader {
            name: "authorization".into(),
            display_name: "authorization".into(),
            value: value.into(),
            display: display.into(),
        };
        PreparedRequest {
            method: Default::default(),
            url: String::new(),
            display_url: String::new(),
            headers: Vec::new(),
            wire: vec![header],
            body: None,
            insecure: false,
        }
    }

    #[test]
    fn with_sent_masks_the_part_of_a_header_that_differs_from_its_display() {
        let m = mask(&["s3cret"]);
        let request = sent("Basic dXNlcjpzM2NyZXQ=", &format!("Basic {SECRET_MASK}"));
        let m = m.with_sent(&request);
        assert_eq!(m.apply("Basic dXNlcjpzM2NyZXQ="), format!("Basic {SECRET_MASK}"));
        assert_eq!(m.apply("s3cret"), SECRET_MASK);
        // Common suffix too, and a derived part under the minimum is left.
        let m = mask(&[]).with_sent(&sent("Token ABCDEFGH; v=1", "Token ●; v=1"));
        assert_eq!(m.apply("Token ABCDEFGH; v=1"), format!("Token {SECRET_MASK}; v=1"));
        let m = mask(&[]).with_sent(&sent("Token ab", "Token ●"));
        assert_eq!(m.apply("Token ab"), "Token ab");
    }

    #[test]
    fn with_sent_leaves_show_secrets_alone() {
        let request = sent("Basic dXNlcjpzM2NyZXQ=", &format!("Basic {SECRET_MASK}"));
        assert_eq!(Mask::none().with_sent(&request).apply("dXNlcjpzM2NyZXQ="), "dXNlcjpzM2NyZXQ=");
    }

    #[test]
    fn duplicates_are_harmless() {
        assert_eq!(mask(&["s3cret", "s3cret"]).apply("s3cret"), SECRET_MASK);
    }
}
