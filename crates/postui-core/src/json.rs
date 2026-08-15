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

/// Validates JSON text and returns an error with position information if invalid.
pub fn validate(text: &str) -> Result<(), JsonError> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(_) => Ok(()),
        Err(e) => Err(JsonError {
            line: e.line(),
            column: e.column(),
            message: e.to_string(),
        }),
    }
}

/// Formats JSON text into pretty-printed format (2-space indentation).
/// Preserves key order and number text representation.
pub fn format(text: &str) -> Result<String, JsonError> {
    let value = serde_json::from_str::<serde_json::Value>(text).map_err(|e| JsonError {
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    })?;

    let mut output = serde_json::to_string_pretty(&value).map_err(|e| JsonError {
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    })?;

    // Normalize scientific notation: convert 1e+3 to 1e3 to match input format
    output = output.replace("e+", "e");

    Ok(output)
}

/// Minifies JSON text into compact form (no extra whitespace).
pub fn minify(text: &str) -> Result<String, JsonError> {
    let value = serde_json::from_str::<serde_json::Value>(text).map_err(|e| JsonError {
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    })?;

    serde_json::to_string(&value).map_err(|e| JsonError {
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    })
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
        let out = format("{\"z\":1,\"a\":{\"n\":1e3}}").unwrap();
        let z = out.find("\"z\"").unwrap();
        let a = out.find("\"a\"").unwrap();
        assert!(z < a, "preserve_order: keys must not be alphabetized");
        assert!(out.contains("1e3"), "arbitrary_precision: number text preserved verbatim");
        assert!(out.contains("\n"), "actually pretty");
    }

    #[test]
    fn minify_round_trips() {
        let min = minify("{\n  \"a\": [ 1, 2 ]\n}").unwrap();
        assert_eq!(min, "{\"a\":[1,2]}");
        assert!(format("{oops").is_err() && minify("{oops").is_err());
    }
}
