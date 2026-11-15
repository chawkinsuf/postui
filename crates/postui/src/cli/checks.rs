//! A request's checks as `run` and `suite` print them (suite checks spec
//! 2026-11-14 §2.3): each [`Outcome`] masked, for the text and the JSON,
//! and the one rule both pass a response by ([`passed`]).

use super::mask::Mask;
use postui_core::checks::{Measure, Outcome, one_line};
use serde::Serialize;

/// A status from here up fails a response that has no status check (R3).
pub(super) const FIRST_FAILING_STATUS: u16 = 400;

/// The status check's verdict; `None` when the request has none.
pub(super) fn status_check(checks: &[ShownCheck]) -> Option<bool> {
    checks.iter().find_map(|c| match c.kind {
        Kind::Status { passed, .. } => Some(passed),
        Kind::Time { .. } | Kind::Jq { .. } => None,
    })
}

/// Whether a time limit of `checks` failed (spec §2.7): the live view paints
/// the time red.
pub(super) fn time_failed(checks: &[ShownCheck]) -> bool {
    checks.iter().any(|c| matches!(c.kind, Kind::Time { passed: false, .. }))
}

/// Whether a response with `status` passes, given its `checks`: its status
/// passes the status check, or is below 400 when there is none (R3), and
/// every check passed.
pub(super) fn passed(status: u16, checks: &[ShownCheck]) -> bool {
    status_check(checks).unwrap_or(status < FIRST_FAILING_STATUS)
        && checks.iter().all(ShownCheck::passed)
}

/// One check's outcome, every string in it through the run's mask.
#[derive(Clone, Debug, Serialize)]
pub(super) struct ShownCheck {
    #[serde(flatten)]
    kind: Kind,
    /// Why it failed, the line after `check failed: `; `None` when it
    /// passed. The JSON gives this line, clipped as the text is, rather
    /// than a filter's outputs, which can be as large as the body (§2.3).
    failure: Option<String>,
}

/// The JSON's three shapes (§2.3, §2.7): `{"check": "status", "expected",
/// "got", "passed", "failure"}`, `{"check": "max_ttfb_ms" or
/// "max_elapsed_ms", "limit", "got", "passed", "failure"}` and `{"check":
/// FILTER, "passed", "failure"}`.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
enum Kind {
    Status {
        check: &'static str,
        expected: Vec<u16>,
        got: u16,
        passed: bool,
    },
    Time {
        check: &'static str,
        limit: u64,
        got: u64,
        passed: bool,
        #[serde(skip)]
        measure: Measure,
    },
    Jq {
        check: String,
        passed: bool,
    },
}

impl ShownCheck {
    pub(super) fn passed(&self) -> bool {
        match &self.kind {
            Kind::Status { passed, .. } | Kind::Time { passed, .. } | Kind::Jq { passed, .. } => {
                *passed
            }
        }
    }

    /// What `run -v` lists for the check: its failure, else what passed
    /// (`status 200`, `first byte 45 ms, limit 500 ms`, or the filter).
    pub(super) fn summary(&self) -> String {
        if let Some(failure) = &self.failure {
            return failure.clone();
        }
        match &self.kind {
            Kind::Status { got, .. } => format!("status {got}"),
            Kind::Time { measure, limit, got, .. } => measure.line(*limit, *got),
            Kind::Jq { check, .. } => one_line(check),
        }
    }
}

/// `outcomes` masked by `mask`.
pub(super) fn shown(outcomes: &[Outcome], mask: &Mask) -> Vec<ShownCheck> {
    outcomes
        .iter()
        .map(|outcome| {
            let failure = outcome.failure(|text| mask.apply(text));
            let passed = outcome.passed();
            let kind = match outcome {
                Outcome::Status { expected, got } => {
                    Kind::Status { check: "status", expected: expected.clone(), got: *got, passed }
                }
                Outcome::Time { measure, limit, got } => Kind::Time {
                    check: measure.key(),
                    limit: *limit,
                    got: *got,
                    passed,
                    measure: *measure,
                },
                Outcome::Jq { filter, .. } => Kind::Jq { check: mask.apply(filter), passed },
            };
            ShownCheck { kind, failure }
        })
        .collect()
}

/// A `check failed: …` line for each check of `checks` that failed, in
/// order.
pub(super) fn failure_lines(checks: &[ShownCheck]) -> impl Iterator<Item = String> + '_ {
    checks.iter().filter_map(|c| c.failure.as_ref()).map(|f| format!("check failed: {f}"))
}

/// Checks with `status` codes and `jq` filters run over a response with
/// status `got` and `body`, unmasked: a test's fixture.
#[cfg(test)]
pub(super) fn checked(status: &[u16], jq: &[&str], got: u16, body: &str) -> Vec<ShownCheck> {
    shown(&tests::evaluate(status, jq, got, body), &Mask::none())
}

/// A `max_elapsed_ms` of `limit` run over a response that took
/// `elapsed_ms`, unmasked: a test's fixture.
#[cfg(test)]
pub(super) fn timed(limit: u64, elapsed_ms: u64) -> Vec<ShownCheck> {
    use postui_core::checks::Timing;
    let checks = postui_core::model::Checks { max_elapsed_ms: Some(limit), ..Default::default() };
    let timing = Timing { ttfb_ms: 0, elapsed_ms };
    shown(&postui_core::checks::evaluate(&checks, 200, timing, || unreachable!()), &Mask::none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::checks::Timing;
    use postui_core::jq::JqDocument;
    use postui_core::model::Checks;

    pub(super) fn evaluate(status: &[u16], jq: &[&str], got: u16, body: &str) -> Vec<Outcome> {
        let checks = Checks {
            status: status.to_vec(),
            jq: jq.iter().map(|f| f.to_string()).collect(),
            ..Checks::default()
        };
        postui_core::checks::evaluate(&checks, got, Timing::default(), || JqDocument::parse(body))
    }

    /// R3: a status check replaces the below-400 rule; without one it
    /// stays. Every check must pass too.
    #[test]
    fn a_status_check_replaces_the_400_rule() {
        assert!(passed(404, &checked(&[404], &[], 404, "")));
        assert_eq!(status_check(&checked(&[404], &[], 404, "")), Some(true));
        assert!(!passed(201, &checked(&[200], &[], 201, "")));
        assert_eq!(status_check(&checked(&[200], &[], 201, "")), Some(false));
        assert!(!passed(500, &checked(&[200, 201], &[], 500, "")));
        // No status check: the rule.
        assert!(passed(399, &[]));
        assert!(!passed(400, &[]));
        assert_eq!(status_check(&checked(&[], &[".ok"], 200, r#"{"ok":true}"#)), None);
        assert!(!passed(404, &checked(&[], &[".ok"], 404, r#"{"ok":true}"#)));
        // A status that passes and a filter that fails.
        assert!(!passed(200, &checked(&[200], &[".ok"], 200, r#"{"ok":false}"#)));
        assert!(passed(200, &checked(&[200], &[".ok"], 200, r#"{"ok":true}"#)));
    }

    /// §2.3's JSON: the status check's object and a jq check's, in file
    /// order, each with its failure line.
    #[test]
    fn the_json_has_one_object_a_check() {
        let out = shown(&evaluate(&[200], &[".a == 1", ".a"], 404, r#"{"a":2}"#), &Mask::none());
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"check": "status", "expected": [200], "got": 404, "passed": false,
                 "failure": "status 404, expected 200"},
                {"check": ".a == 1", "passed": false, "failure": ".a == 1 gave false"},
                {"check": ".a", "passed": false, "failure": ".a gave 2"},
            ])
        );
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(lines[0], "check failed: status 404, expected 200");
        let out = shown(&evaluate(&[], &[".a == 2"], 200, r#"{"a":2}"#), &Mask::none());
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!([{"check": ".a == 2", "passed": true, "failure": null}])
        );
    }

    /// A value or an error in the JSON is clipped as the line clips it, so
    /// a check that forgot its comparison doesn't put the body there.
    #[test]
    fn the_json_clips_what_a_filter_gave() {
        let body = format!(r#"{{"text":"{}"}}"#, "word ".repeat(2000));
        let out = shown(&evaluate(&[], &[".text", ".text | fromjson"], 200, &body), &Mask::none());
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.len() < 400, "{json}");
    }

    /// Every string from the response is masked: an output, the failure
    /// line, an error.
    #[test]
    fn a_secret_in_a_value_is_masked() {
        let mask = Mask::new(["hunter22".to_string()]);
        let out =
            shown(&evaluate(&[], &[".t", ".t | fromjson"], 200, r#"{"t":"hunter22"}"#), &mask);
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("hunter22"), "{json}");
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| !l.contains("hunter22")), "{lines:?}");
    }

    #[test]
    fn a_passed_check_summarises_as_what_passed() {
        let out = shown(&evaluate(&[201], &[".ok"], 201, r#"{"ok":true}"#), &Mask::none());
        let summaries: Vec<String> = out.iter().map(ShownCheck::summary).collect();
        assert_eq!(summaries, ["status 201", ".ok"]);
        assert!(out.iter().all(ShownCheck::passed));
        assert_eq!(failure_lines(&out).count(), 0);
    }

    /// Spec §2.7 (T3): a time check's JSON names its key, limit and time,
    /// after the status check and before the jq ones; a breach fails the
    /// response whatever its status, and `time_failed` says so.
    #[test]
    fn a_time_check_has_its_own_json_and_fails_the_response() {
        let checks = Checks {
            status: vec![200],
            max_ttfb_ms: Some(500),
            max_elapsed_ms: Some(2000),
            jq: vec![".ok".into()],
        };
        let timing = Timing { ttfb_ms: 812, elapsed_ms: 900 };
        let outcomes = postui_core::checks::evaluate(&checks, 200, timing, || {
            JqDocument::parse(r#"{"ok":true}"#)
        });
        let out = shown(&outcomes, &Mask::none());
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!([
                {"check": "status", "expected": [200], "got": 200, "passed": true, "failure": null},
                {"check": "max_ttfb_ms", "limit": 500, "got": 812, "passed": false,
                 "failure": "first byte 812 ms, limit 500 ms"},
                {"check": "max_elapsed_ms", "limit": 2000, "got": 900, "passed": true,
                 "failure": null},
                {"check": ".ok", "passed": true, "failure": null},
            ])
        );
        assert!(!passed(200, &out));
        assert!(time_failed(&out));
        assert_eq!(status_check(&out), Some(true));
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(lines, ["check failed: first byte 812 ms, limit 500 ms"]);
        let summaries: Vec<String> = out.iter().map(ShownCheck::summary).collect();
        assert_eq!(
            summaries,
            ["status 200", "first byte 812 ms, limit 500 ms", "total 900 ms, limit 2000 ms", ".ok"]
        );

        // Within its limits: passed, and no time failure.
        let timing = Timing { ttfb_ms: 500, elapsed_ms: 2000 };
        let outcomes = postui_core::checks::evaluate(&checks, 200, timing, || {
            JqDocument::parse(r#"{"ok":true}"#)
        });
        let out = shown(&outcomes, &Mask::none());
        assert!(passed(200, &out));
        assert!(!time_failed(&out));
        // A time check alone keeps the 400 rule.
        let alone = Checks { max_elapsed_ms: Some(10), ..Checks::default() };
        let out = shown(
            &postui_core::checks::evaluate(&alone, 404, Timing::default(), || unreachable!()),
            &Mask::none(),
        );
        assert_eq!(status_check(&out), None);
        assert!(!passed(404, &out));
        assert!(passed(204, &out));
    }

    /// A filter written over several lines summarises and fails on one.
    #[test]
    fn a_multi_line_filter_is_one_line_of_output() {
        let filter = ".ok\n| . == true";
        let out = shown(&evaluate(&[], &[filter], 200, r#"{"ok":true}"#), &Mask::none());
        assert_eq!(out[0].summary(), ".ok | . == true");
        let out = shown(&evaluate(&[], &[filter], 200, r#"{"ok":false}"#), &Mask::none());
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(lines, ["check failed: .ok | . == true gave false"]);
    }
}
