//! A request's `[checks]` run over its response (suite checks spec
//! 2026-11-14 §2.2): the one runner `postui run` and `postui suite` share.

use crate::jq::{self, JqDocument, JqError};
use crate::model::Checks;

/// The most characters of a value or an error a failure line shows (§2.3).
const SHOWN_CHARS: usize = 60;

/// How one check came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The `status` check: the codes that pass, and the one that came.
    Status { expected: Vec<u16>, got: u16 },
    /// A time limit (spec §2.7): which figure, its limit and what it took,
    /// in milliseconds.
    Time { measure: Measure, limit: u64, got: u64 },
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
            Outcome::Time { limit, got, .. } => got <= limit,
            Outcome::Jq { got, error, .. } => {
                error.is_none() && !got.is_empty() && got.iter().all(|v| v == "true")
            }
        }
    }

    /// Why the check failed, as a failure line says it after
    /// `check failed: ` (§2.3), on one line; `None` when it passed. Each
    /// text from the request or the response (the filter, a value, an
    /// error) goes through `shown` first, and a value or an error is
    /// clipped after it, so a mask sees a secret whole. An error is clipped
    /// as a value is because jaq's runtime errors hold the value they
    /// failed on.
    pub fn failure(&self, shown: impl Fn(&str) -> String) -> Option<String> {
        if self.passed() {
            return None;
        }
        Some(one_line(&match self {
            Outcome::Status { expected, got } => {
                let codes: Vec<String> = expected.iter().map(u16::to_string).collect();
                format!("status {got}, expected {}", codes.join(" or "))
            }
            Outcome::Time { measure, limit, got } => measure.line(*limit, *got),
            Outcome::Jq { filter, got, error } => {
                let filter = shown(filter);
                match (error, got.iter().find(|v| *v != "true")) {
                    (Some(error), _) => format!("{filter}: {}", clip(&shown(error))),
                    (None, Some(value)) => format!("{filter} gave {}", clip(&shown(value))),
                    (None, None) => format!("{filter} gave no output"),
                }
            }
        }))
    }
}

/// Which figure a time limit holds to (spec §2.7, T1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// Until the response head (status and headers) is in: `max_ttfb_ms`.
    FirstByte,
    /// Until the body is fully read: `max_elapsed_ms`.
    Total,
}

impl Measure {
    /// Both, in the order they are checked and written.
    pub const ALL: [Measure; 2] = [Measure::FirstByte, Measure::Total];

    /// The `[checks]` key that sets it, which the JSON names it by.
    pub fn key(self) -> &'static str {
        match self {
            Measure::FirstByte => "max_ttfb_ms",
            Measure::Total => "max_elapsed_ms",
        }
    }

    /// Its limit in `checks`, the field [`Measure::key`] names.
    pub fn limit(self, checks: &Checks) -> Option<u64> {
        match self {
            Measure::FirstByte => checks.max_ttfb_ms,
            Measure::Total => checks.max_elapsed_ms,
        }
    }

    /// Its figure in `timing`.
    pub fn of(self, timing: Timing) -> u64 {
        match self {
            Measure::FirstByte => timing.ttfb_ms,
            Measure::Total => timing.elapsed_ms,
        }
    }

    /// The check as a line says it, passed or not: `first byte 812 ms,
    /// limit 500 ms` or `total 2310 ms, limit 2000 ms`.
    pub fn line(self, limit: u64, got: u64) -> String {
        let name = match self {
            Measure::FirstByte => "first byte",
            Measure::Total => "total",
        };
        format!("{name} {got} ms, limit {limit} ms")
    }
}

/// How long a response took, in whole milliseconds, as the reports give
/// it: `ttfb_ms` to the response head, `elapsed_ms` to the body's end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    pub ttfb_ms: u64,
    pub elapsed_ms: u64,
}

/// `text` with its lines joined by a space, each trimmed: a filter written
/// over several lines in the file, shown in a line of output.
pub fn one_line(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

/// `text` cut to [`SHOWN_CHARS`] characters, the last one `…` when cut.
fn clip(text: &str) -> String {
    if text.chars().count() <= SHOWN_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(SHOWN_CHARS - 1).collect();
    format!("{kept}…")
}

/// Runs `checks` over a response with `status` that took `timing`: the
/// status check first when there is one, then the time limits (first byte,
/// then total; spec §2.7), then each jq filter in file order over the body
/// `doc` gives, read as `run --jq` reads it (JSON, NDJSON or an SSE
/// stream). `doc` is called once, and only when there is a filter. A body
/// that doesn't read fails each filter with the reader's error, or with
/// the filter's own when it doesn't compile (R5).
pub fn evaluate(
    checks: &Checks,
    status: u16,
    timing: Timing,
    doc: impl FnOnce() -> Result<JqDocument, JqError>,
) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    if !checks.status.is_empty() {
        outcomes.push(Outcome::Status { expected: checks.status.clone(), got: status });
    }
    for measure in Measure::ALL {
        if let Some(limit) = measure.limit(checks) {
            outcomes.push(Outcome::Time { measure, limit, got: measure.of(timing) });
        }
    }
    if checks.jq.is_empty() {
        return outcomes;
    }
    let doc = doc();
    for filter in &checks.jq {
        let run = match &doc {
            Ok(doc) => jq::run(filter, doc),
            Err(e) => jq::check(filter).and(Err(e.clone())),
        };
        let (got, error) = match run {
            Ok(run) => (run.outputs, run.error.map(|e| e.message())),
            Err(e) => (Vec::new(), Some(e.message())),
        };
        outcomes.push(Outcome::Jq { filter: filter.clone(), got, error });
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checks(status: &[u16], jq: &[&str]) -> Checks {
        Checks {
            status: status.to_vec(),
            jq: jq.iter().map(|f| f.to_string()).collect(),
            ..Checks::default()
        }
    }

    /// `evaluate` over `body` as text, taking no time.
    fn over(checks: &Checks, status: u16, body: &str) -> Vec<Outcome> {
        evaluate(checks, status, Timing::default(), || JqDocument::parse(body))
    }

    /// Spec §2.7 (T1–T3): each limit passes at or under it and fails over
    /// it, with its line; the two are independent.
    #[test]
    fn a_time_limit_passes_at_or_under_it() {
        let limits =
            Checks { max_ttfb_ms: Some(500), max_elapsed_ms: Some(2000), ..Checks::default() };
        let at = |ttfb_ms, elapsed_ms| {
            evaluate(&limits, 200, Timing { ttfb_ms, elapsed_ms }, || panic!("the body was read"))
        };
        assert!(at(500, 2000).iter().all(Outcome::passed));
        assert!(at(0, 0).iter().all(Outcome::passed));
        assert_eq!(failures(&at(501, 2000)), ["first byte 501 ms, limit 500 ms"]);
        assert_eq!(failures(&at(500, 2310)), ["total 2310 ms, limit 2000 ms"]);
        assert_eq!(
            failures(&at(812, 2310)),
            ["first byte 812 ms, limit 500 ms", "total 2310 ms, limit 2000 ms"]
        );
        let only_total = Checks { max_elapsed_ms: Some(100), ..Checks::default() };
        let out = evaluate(&only_total, 200, Timing { ttfb_ms: 9000, elapsed_ms: 100 }, || {
            panic!("the body was read")
        });
        assert_eq!(out, [Outcome::Time { measure: Measure::Total, limit: 100, got: 100 }]);
        assert!(out[0].passed());
    }

    /// T3: the time checks come after the status check and before the jq
    /// ones, first byte then total.
    #[test]
    fn time_checks_come_between_status_and_jq() {
        let checks = Checks {
            status: vec![200],
            max_ttfb_ms: Some(10),
            max_elapsed_ms: Some(20),
            jq: vec![".a".into()],
        };
        let out = evaluate(&checks, 200, Timing { ttfb_ms: 5, elapsed_ms: 25 }, || {
            JqDocument::parse(r#"{"a":true}"#)
        });
        assert!(matches!(&out[0], Outcome::Status { .. }));
        assert_eq!(out[1], Outcome::Time { measure: Measure::FirstByte, limit: 10, got: 5 });
        assert_eq!(out[2], Outcome::Time { measure: Measure::Total, limit: 20, got: 25 });
        assert!(matches!(&out[3], Outcome::Jq { .. }));
        assert_eq!(failures(&out), ["total 25 ms, limit 20 ms"]);
    }

    fn failures(outcomes: &[Outcome]) -> Vec<String> {
        outcomes.iter().filter_map(|o| o.failure(str::to_string)).collect()
    }

    #[test]
    fn a_true_filter_passes() {
        let out = over(&checks(&[], &[".a == 1", ".b | length > 0"]), 200, r#"{"a":1,"b":[2]}"#);
        assert!(out.iter().all(Outcome::passed), "{out:?}");
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
            let out = over(&checks(&[], &[filter]), 200, body);
            let got = failures(&out);
            assert_eq!(got.len(), 1, "{filter}: {out:?}");
            assert!(got[0].starts_with(line), "{filter}: {got:?}");
        }
        let out = over(&checks(&[], &[".s"]), 200, &long);
        let line = &failures(&out)[0];
        let value = line.strip_prefix(".s gave ").unwrap();
        assert_eq!(value.chars().count(), 60, "{line}");
        assert!(value.starts_with("\"xxx") && value.ends_with('…'), "{line}");
    }

    /// jaq's runtime error holds the value it failed on: the line clips it
    /// as it clips a value, so a large body stays out of it.
    #[test]
    fn an_error_holding_the_body_is_clipped() {
        let body = format!(r#"{{"text":"{}"}}"#, "word ".repeat(2000));
        for filter in [".text | fromjson", ".text + 1"] {
            let line = &failures(&over(&checks(&[], &[filter]), 200, &body))[0];
            let error = line.strip_prefix(&format!("{filter}: ")).expect(line);
            assert_eq!(error.chars().count(), 60, "{line}");
            assert!(error.ends_with('…'), "{line}");
        }
    }

    /// A filter written over several lines fails on one line.
    #[test]
    fn a_multi_line_filter_fails_on_one_line() {
        let filter = ".a\n  | . == 2\n";
        let out = over(&checks(&[], &[filter]), 200, r#"{"a":1}"#);
        assert_eq!(failures(&out), [".a | . == 2 gave false"]);
    }

    #[test]
    fn a_filter_that_does_not_compile_fails_with_its_error() {
        let out = over(&checks(&[], &[".a ==", ".a"]), 200, r#"{"a":true}"#);
        let lines = failures(&out);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with(".a ==: "), "{lines:?}");
        assert!(out[1].passed(), "the other filter still runs");
    }

    /// R5: a body that doesn't read still shows a filter's compile error,
    /// so a broken filter is seen before the server is fixed.
    #[test]
    fn a_compile_error_shows_over_a_body_that_does_not_read() {
        let out = over(&checks(&[], &[".a ==", ".a"]), 502, "<html>");
        let lines = failures(&out);
        assert_eq!(lines[0], ".a ==: expected a term", "{lines:?}");
        assert!(lines[1].starts_with(".a: not JSON: "), "{lines:?}");
    }

    /// The body is read only for a filter: a status check alone never
    /// calls `doc`.
    #[test]
    fn a_status_check_alone_reads_no_body() {
        let out =
            evaluate(&checks(&[200], &[]), 200, Timing::default(), || panic!("the body was read"));
        assert_eq!(out, [Outcome::Status { expected: vec![200], got: 200 }]);
    }

    /// A stream body is read value by value, as `run --jq` reads it: every
    /// value's `true` passes.
    #[test]
    fn a_stream_body_is_read_value_by_value() {
        let ndjson = "{\"ok\":true}\n{\"ok\":true}\n";
        assert!(over(&checks(&[], &[".ok"]), 200, ndjson).iter().all(Outcome::passed));
        let sse = "data: {\"ok\":true}\n\ndata: {\"ok\":false}\n\ndata: [DONE]\n\n";
        let out = over(&checks(&[], &[".ok"]), 200, sse);
        assert_eq!(failures(&out), [".ok gave false"]);
    }

    /// A status check names the codes it expects, one or several.
    #[test]
    fn a_status_check_passes_on_a_code_it_names() {
        assert!(over(&checks(&[404], &[]), 404, "")[0].passed());
        let out = over(&checks(&[200, 201], &[]), 500, "");
        assert_eq!(failures(&out), ["status 500, expected 200 or 201"]);
        let out = over(&checks(&[200], &[]), 201, "");
        assert_eq!(failures(&out), ["status 201, expected 200"]);
    }

    /// The status check comes first, then the filters in file order.
    #[test]
    fn outcomes_are_in_file_order_status_first() {
        let out = over(&checks(&[200], &[".b", ".a"]), 200, r#"{"a":true,"b":true}"#);
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
        let out = over(&checks(&[], &[".t"]), 200, &body);
        let line = out[0].failure(|s| s.replace(secret, "••••")).unwrap();
        assert!(!line.contains("s3c"), "{line}");
        assert!(line.contains("••"), "{line}");
    }
}
