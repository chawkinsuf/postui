use std::fmt;

/// Error returned from JSON operations.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for JsonError {}

impl From<serde_json::Error> for JsonError {
    fn from(e: serde_json::Error) -> Self {
        JsonError {
            line: e.line(),
            column: e.column(),
            message: e.to_string(),
        }
    }
}

/// Validates JSON text and returns an error with position information if invalid.
pub fn validate(text: &str) -> Result<(), JsonError> {
    serde_json::from_str::<serde_json::Value>(text)
        .map(|_| ())
        .map_err(JsonError::from)
}

/// Formats JSON text into pretty-printed format (2-space indentation).
/// Preserves key order and number precision with arbitrary_precision feature.
pub fn format(text: &str) -> Result<String, JsonError> {
    let value = serde_json::from_str::<serde_json::Value>(text).map_err(JsonError::from)?;
    serde_json::to_string_pretty(&value).map_err(JsonError::from)
}

/// Minifies JSON text into compact form (no extra whitespace).
pub fn minify(text: &str) -> Result<String, JsonError> {
    let value = serde_json::from_str::<serde_json::Value>(text).map_err(JsonError::from)?;
    serde_json::to_string(&value).map_err(JsonError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_reports_position() {
        assert!(validate("{\"a\": 1}").is_ok());
        let e = validate("{\n  \"a\": oops\n}").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.column > 0);
    }

    #[test]
    fn format_pretty_prints_preserving_key_order_and_number_text() {
        let out = format("{\"z\":1,\"a\":{\"n\":1e3,\"m\":1.50}}").unwrap();
        let z = out.find("\"z\"").unwrap();
        let a = out.find("\"a\"").unwrap();
        assert!(z < a, "preserve_order: keys must not be alphabetized");
        assert!(
            out.contains("1e3") || out.contains("1e+3"),
            "arbitrary_precision: exponent format preserved (1e3 or 1e+3)"
        );
        assert!(out.contains("1.50"), "arbitrary_precision: decimal precision preserved");
        assert!(out.contains("\n"), "actually pretty");
    }

    #[test]
    fn minify_round_trips() {
        let min = minify("{\n  \"a\": [ 1, 2 ]\n}").unwrap();
        assert_eq!(min, "{\"a\":[1,2]}");
        assert!(format("{oops").is_err() && minify("{oops").is_err());
    }

    #[test]
    fn format_does_not_corrupt_string_values() {
        let out = format("{\"note\":\"value with 1e+3 inside\"}").unwrap();
        assert!(
            out.contains("value with 1e+3 inside"),
            "string values must not be mutated during formatting"
        );
    }

    #[test]
    fn minify_does_not_corrupt_string_values() {
        let out = minify("{\"note\":\"value with 1e+3 inside\"}").unwrap();
        assert!(
            out.contains("value with 1e+3 inside"),
            "string values must not be mutated during minification"
        );
    }
}
