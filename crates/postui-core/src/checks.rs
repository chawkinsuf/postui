//! A request's `[checks]` run over its response (suite checks spec
//! 2026-11-14 §2.2): the one runner `postui run` and `postui suite` share.

use crate::jq::{self, JqDocument};
use crate::model::Checks;

/// A status from here up fails a request that has no `status` check (R3).
pub const FIRST_FAILING_STATUS: u16 = 400;

/// The most characters of a value a failure line shows (§2.3).
const SHOWN_CHARS: usize = 60;

/// How one check came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The `status` check: the codes that pass, and the one that came.
    Status { expected: Vec<u16>, got: u16 },
    /// One `jq` filter: each output as compact JSON, in order, and the
    /// error that ended a run, the filter's or the body's.
    Jq { filter: String, got: Vec<String>, error: Option<String> },
}

impl Outcome {
    /// A status check passes on a code it names. A jq check passes when it
    /// gave at least one output, every output is `true`, and nothing
    /// errored (R4): stricter than `jq -e`, so a filter that forgot its
    /// comparison fails instead of passing on any value.
    pub fn passed(&self) -> bool {
        match self {
            Outcome::Status { expected, got } => expected.contains(got),
            Outcome::Jq { got, error, .. } => {
                error.is_none() && !got.is_empty() && got.iter().all(|v| v == "true")
            }
        }
    }

    /// Why the check failed, as a failure line says it after
    /// `check failed: ` (§2.3); `None` when it passed. Each text from the
    /// request or the response (the filter, a value, an error) goes through
    /// `shown` first, and a value is clipped after it, so a mask sees a
    /// secret whole.
    pub fn failure(&self, shown: impl Fn(&str) -> String) -> Option<String> {
        if self.passed() {
            return None;
        }
        Some(match self {
            Outcome::Status { expected, got } => {
                let codes: Vec<String> = expected.iter().map(u16::to_string).collect();
                format!("status {got}, expected {}", codes.join(" or "))
            }
            Outcome::Jq { filter, got, error } => {
                let filter = shown(filter);
                match (error, got.iter().find(|v| *v != "true")) {
                    (Some(error), _) => format!("{filter}: {}", shown(error)),
                    (None, Some(value)) => format!("{filter} gave {}", clip(&shown(value))),
                    (None, None) => format!("{filter} gave no output"),
                }
            }
        })
    }
}

/// `text` cut to [`SHOWN_CHARS`] characters, the last one `…` when cut.
fn clip(text: &str) -> String {
    if text.chars().count() <= SHOWN_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(SHOWN_CHARS - 1).collect();
    format!("{kept}…")
}

/// Runs `checks` over a response with `status` and `body`: the status check
/// first when there is one, then each jq filter in file order. The body is
/// read as `run --jq` reads it (JSON, NDJSON or an SSE stream), once, and
/// only when there is a filter; a body that doesn't read fails every filter
/// with the reader's error.
pub fn evaluate(checks: &Checks, status: u16, body: &str) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    if !checks.status.is_empty() {
        outcomes.push(Outcome::Status { expected: checks.status.clone(), got: status });
    }
    if checks.jq.is_empty() {
        return outcomes;
    }
    let doc = JqDocument::parse(body);
    for filter in &checks.jq {
        let run = doc.as_ref().map_err(Clone::clone).and_then(|doc| jq::run(filter, doc));
        let (got, error) = match run {
            Ok(run) => (run.outputs, run.error.map(|e| e.message())),
            Err(e) => (Vec::new(), Some(e.message())),
        };
        outcomes.push(Outcome::Jq { filter: filter.clone(), got, error });
    }
    outcomes
}

/// Whether a response with `status` passes, given its checks' `outcomes`:
/// its status is one the status check names, or below 400 when there is no
/// status check (R3), and every check passed.
pub fn passed(status: u16, outcomes: &[Outcome]) -> bool {
    let status_checked = outcomes.iter().any(|o| matches!(o, Outcome::Status { .. }));
    (status_checked || status < FIRST_FAILING_STATUS) && outcomes.iter().all(Outcome::passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checks(status: &[u16], jq: &[&str]) -> Checks {
        Checks { status: status.to_vec(), jq: jq.iter().map(|f| f.to_string()).collect() }
    }

    fn failures(outcomes: &[Outcome]) -> Vec<String> {
        outcomes.iter().filter_map(|o| o.failure(str::to_string)).collect()
    }

    #[test]
    fn a_true_filter_passes() {
        let out =
            evaluate(&checks(&[], &[".a == 1", ".b | length > 0"]), 200, r#"{"a":1,"b":[2]}"#);
        assert!(out.iter().all(Outcome::passed), "{out:?}");
        assert!(passed(200, &out));
        assert!(failures(&out).is_empty());
    }

    /// §2.3: false, no output, any other value, a filter error, a body that
    /// isn't JSON, each fails with its own line.
    #[test]
    fn each_way_a_filter_fails_has_its_line() {
        let long = format!(r#"{{"s":"{}"}}"#, "x".repeat(80));
        let cases = [
            (".a == 2", r#"{"a":1}"#, ".a == 2 gave false"),
            (".a[] | . > 5", r#"{"a":[]}"#, ".a[] | . > 5 gave no output"),
            (".a", r#"{"a":"red"}"#, r#".a gave "red""#),
            (".a[] | . > 1", r#"{"a":[2,1]}"#, ".a[] | . > 1 gave false"),
            (".a | fromjson", r#"{"a":"{"}"#, ".a | fromjson: "),
            (".a == 1", "<html>", ".a == 1: not JSON: "),
            (".a == 1", "", ".a == 1: not JSON: the body holds no value"),
        ];
        for (filter, body, line) in cases {
            let out = evaluate(&checks(&[], &[filter]), 200, body);
            let got = failures(&out);
            assert_eq!(got.len(), 1, "{filter}: {out:?}");
            assert!(got[0].starts_with(line), "{filter}: {got:?}");
            assert!(!passed(200, &out));
        }
        let out = evaluate(&checks(&[], &[".s"]), 200, &long);
        let line = &failures(&out)[0];
        let value = line.strip_prefix(".s gave ").unwrap();
        assert_eq!(value.chars().count(), 60, "{line}");
        assert!(value.starts_with("\"xxx") && value.ends_with('…'), "{line}");
    }

    #[test]
    fn a_filter_that_does_not_compile_fails_with_its_error() {
        let out = evaluate(&checks(&[], &[".a ==", ".a"]), 200, r#"{"a":true}"#);
        let lines = failures(&out);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with(".a ==: "), "{lines:?}");
        assert!(out[1].passed(), "the other filter still runs");
    }

    /// A stream body is read value by value, as `run --jq` reads it: every
    /// value's `true` passes.
    #[test]
    fn a_stream_body_is_read_value_by_value() {
        let ndjson = "{\"ok\":true}\n{\"ok\":true}\n";
        assert!(passed(200, &evaluate(&checks(&[], &[".ok"]), 200, ndjson)));
        let sse = "data: {\"ok\":true}\n\ndata: {\"ok\":false}\n\ndata: [DONE]\n\n";
        let out = evaluate(&checks(&[], &[".ok"]), 200, sse);
        assert_eq!(failures(&out), [".ok gave false"]);
    }

    /// R3: a status check replaces the below-400 rule; without one it
    /// stays.
    #[test]
    fn a_status_check_replaces_the_400_rule() {
        let out = evaluate(&checks(&[404], &[]), 404, "");
        assert!(passed(404, &out), "{out:?}");
        let out = evaluate(&checks(&[200, 201], &[]), 500, "");
        assert_eq!(failures(&out), ["status 500, expected 200 or 201"]);
        assert!(!passed(500, &out));
        let out = evaluate(&checks(&[200], &[]), 201, "");
        assert_eq!(failures(&out), ["status 201, expected 200"]);
        assert!(!passed(201, &out));
        // No checks: the rule.
        assert!(passed(399, &[]));
        assert!(!passed(400, &[]));
    }

    /// The status check comes first, then the filters in file order.
    #[test]
    fn outcomes_are_in_file_order_status_first() {
        let out = evaluate(&checks(&[200], &[".b", ".a"]), 200, r#"{"a":true,"b":true}"#);
        assert!(matches!(&out[0], Outcome::Status { got: 200, .. }));
        assert!(matches!(&out[1], Outcome::Jq { filter, .. } if filter == ".b"));
        assert!(matches!(&out[2], Outcome::Jq { filter, .. } if filter == ".a"));
    }

    /// `shown` sees each text whole before a value is clipped, so a mask
    /// over a secret past the clip still hides all of it.
    #[test]
    fn shown_is_applied_before_the_clip() {
        let secret = "s3cret-token";
        let body = format!(r#"{{"t":"{}{secret}"}}"#, "x".repeat(55));
        let out = evaluate(&checks(&[], &[".t"]), 200, &body);
        let line = out[0].failure(|s| s.replace(secret, "••••")).unwrap();
        assert!(!line.contains("s3c"), "{line}");
        assert!(line.contains("••"), "{line}");
    }
}
