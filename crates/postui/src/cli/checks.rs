//! A request's checks as `run` and `suite` print them (suite checks spec
//! 2026-11-14 §2.3): each [`Outcome`] masked, for the text and the JSON,
//! and the one rule both pass a response by ([`passed`]). The rule and
//! each check's wording are core's (`postui_core::checks`), which the app
//! reads too.

use super::mask::Mask;
pub(super) use postui_core::checks::FIRST_FAILING_STATUS;
use postui_core::checks::{Measure, Outcome};
use serde::Serialize;

/// The status check's verdict; `None` when the request has none.
pub(super) fn status_check(checks: &[ShownCheck]) -> Option<bool> {
    checks.iter().find_map(|c| match c.kind {
        Kind::Status { passed, .. } => Some(passed),
        Kind::Time { .. } | Kind::Jq { .. } => None,
    })
}

/// Whether the `max_elapsed_ms` check of `checks` failed (spec §2.7): the
/// live view paints its time, the total, red.
pub(super) fn total_failed(checks: &[ShownCheck]) -> bool {
    checks
        .iter()
        .any(|c| matches!(c.kind, Kind::Time { measure: Measure::Total, passed: false, .. }))
}

/// Whether a response with `status` passes, given its `checks`: its status
/// passes the status check, or is below 400 when there is none (R3), and
/// every check passed.
pub(super) fn passed(status: u16, checks: &[ShownCheck]) -> bool {
    let all = checks.iter().all(ShownCheck::passed);
    postui_core::checks::passes(status, status_check(checks), all)
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
    /// What `run -v` lists for it ([`Outcome::summary`]); not in the JSON.
    #[serde(skip)]
    summary: String,
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
        #[serde(rename = "check", serialize_with = "measure_key")]
        measure: Measure,
        limit: u64,
        got: u64,
        passed: bool,
    },
    Jq {
        check: String,
        passed: bool,
    },
}

/// A time check's `check`: the key that set it.
fn measure_key<S: serde::Serializer>(measure: &Measure, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(measure.key())
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
        self.summary.clone()
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
                Outcome::Time { measure, limit, got } => {
                    Kind::Time { measure: *measure, limit: *limit, got: *got, passed }
                }
                Outcome::Jq { filter, .. } => Kind::Jq { check: mask.apply(filter), passed },
            };
            ShownCheck { kind, failure, summary: outcome.summary(|text| mask.apply(text)) }
        })
        .collect()
}

/// A `check failed: …` line for each check of `checks` that failed, in
/// order.
pub(super) fn failure_lines(checks: &[ShownCheck]) -> impl Iterator<Item = String> + '_ {
    checks.iter().filter_map(|c| c.failure.as_ref()).map(|f| format!("check failed: {f}"))
}

/// `checks` run over a response with `status` and `body` that took
/// `timing`, masked by `mask`: a test's fixture.
#[cfg(test)]
pub(super) fn over(
    checks: &postui_core::model::Checks,
    status: u16,
    timing: postui_core::checks::Timing,
    body: &str,
    mask: &Mask,
) -> Vec<ShownCheck> {
    let doc = || postui_core::jq::JqDocument::parse(body);
    shown(&postui_core::checks::evaluate(checks, status, timing, doc), mask)
}

/// Checks with `status` codes and `jq` filters run over a response with
/// status `got` and `body` that took no time, unmasked: a test's fixture.
#[cfg(test)]
pub(super) fn checked(status: &[u16], jq: &[&str], got: u16, body: &str) -> Vec<ShownCheck> {
    let checks = postui_core::model::Checks {
        status: status.to_vec(),
        jq: jq.iter().map(|f| f.to_string()).collect(),
        ..Default::default()
    };
    over(&checks, got, Default::default(), body, &Mask::none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::checks::Timing;
    use postui_core::model::Checks;

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
        let out = checked(&[200], &[".a == 1", ".a"], 404, r#"{"a":2}"#);
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
        let out = checked(&[], &[".a == 2"], 200, r#"{"a":2}"#);
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
        let out = checked(&[], &[".text", ".text | fromjson"], 200, &body);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.len() < 400, "{json}");
    }

    /// Every string from the response is masked: an output, the failure
    /// line, an error.
    #[test]
    fn a_secret_in_a_value_is_masked() {
        let mask = Mask::new(["hunter22".to_string()]);
        let checks = Checks { jq: vec![".t".into(), ".t | fromjson".into()], ..Checks::default() };
        let out = over(&checks, 200, Timing::default(), r#"{"t":"hunter22"}"#, &mask);
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("hunter22"), "{json}");
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| !l.contains("hunter22")), "{lines:?}");
    }

    #[test]
    fn a_passed_check_summarises_as_what_passed() {
        let out = checked(&[201], &[".ok"], 201, r#"{"ok":true}"#);
        let summaries: Vec<String> = out.iter().map(ShownCheck::summary).collect();
        assert_eq!(summaries, ["status 201", ".ok"]);
        assert!(out.iter().all(ShownCheck::passed));
        assert_eq!(failure_lines(&out).count(), 0);
    }

    /// Checks tabs spec 2026-11-16 (P2, P3): what `run -v` lists and passes
    /// is what the app's Checks tab reads from core, check for check, over
    /// every kind of outcome and both status rules.
    #[test]
    fn run_and_the_app_give_one_verdict_and_one_set_of_lines() {
        use postui_core::checks::{evaluate, lines, passed as core_passed};
        let body = r#"{"ok":true,"items":[1,2]}"#;
        let jq = vec![".ok".to_string(), ".items | length == 4".into(), ".ok ==".into()];
        let limits =
            Checks { max_ttfb_ms: Some(1), max_elapsed_ms: Some(50), jq, ..Checks::default() };
        let named = Checks { status: vec![200, 201], ..limits.clone() };
        for (checks, status) in [(&limits, 200), (&limits, 500), (&named, 201), (&named, 404)] {
            let timing = Timing { ttfb_ms: 3, elapsed_ms: 40 };
            let doc = || postui_core::jq::JqDocument::parse(body);
            let outcomes = evaluate(checks, status, timing, doc);
            let run = shown(&outcomes, &Mask::none());
            let app = lines(status, &outcomes);
            assert_eq!(passed(status, &run), core_passed(&app), "{status}");
            // The app adds the below-400 line, which `run` says on stderr.
            let app = &app[app.len() - run.len()..];
            let listed: Vec<(bool, String)> =
                run.iter().map(|c| (c.passed(), c.summary())).collect();
            assert_eq!(app, listed, "{status}");
        }
    }

    /// Spec §2.7 (T3): a time check's JSON names its key, limit and time,
    /// after the status check and before the jq ones; a breach fails the
    /// response whatever its status; a first-byte breach alone is not
    /// `total_failed`.
    #[test]
    fn a_time_check_has_its_own_json_and_fails_the_response() {
        let checks = Checks {
            status: vec![200],
            max_ttfb_ms: Some(500),
            max_elapsed_ms: Some(2000),
            jq: vec![".ok".into()],
        };
        let timed = |ttfb_ms, elapsed_ms| {
            let timing = Timing { ttfb_ms, elapsed_ms };
            over(&checks, 200, timing, r#"{"ok":true}"#, &Mask::none())
        };
        let out = timed(812, 900);
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
        assert_eq!(
            serde_json::to_string(&out[1]).unwrap(),
            r#"{"check":"max_ttfb_ms","limit":500,"got":812,"passed":false,"failure":"first byte 812 ms, limit 500 ms"}"#
        );
        assert!(!passed(200, &out));
        assert!(!total_failed(&out));
        assert_eq!(status_check(&out), Some(true));
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(lines, ["check failed: first byte 812 ms, limit 500 ms"]);
        let summaries: Vec<String> = out.iter().map(ShownCheck::summary).collect();
        assert_eq!(
            summaries,
            ["status 200", "first byte 812 ms, limit 500 ms", "total 900 ms, limit 2000 ms", ".ok"]
        );

        // Within its limits: passed, and no time failure.
        let out = timed(500, 2000);
        assert!(passed(200, &out));
        assert!(!total_failed(&out));
        // A time check alone keeps the 400 rule.
        let alone = Checks { max_elapsed_ms: Some(10), ..Checks::default() };
        let out = over(&alone, 404, Timing::default(), "", &Mask::none());
        assert_eq!(status_check(&out), None);
        assert!(!passed(404, &out));
        assert!(passed(204, &out));
    }

    /// A filter written over several lines summarises and fails on one.
    #[test]
    fn a_multi_line_filter_is_one_line_of_output() {
        let filter = ".ok\n| . == true";
        let out = checked(&[], &[filter], 200, r#"{"ok":true}"#);
        assert_eq!(out[0].summary(), ".ok | . == true");
        let out = checked(&[], &[filter], 200, r#"{"ok":false}"#);
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(lines, ["check failed: .ok | . == true gave false"]);
    }
}
