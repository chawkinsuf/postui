//! A request's checks as `run` and `suite` print them (suite checks spec
//! 2026-11-14 §2.3): each [`Outcome`] masked, for the text and the JSON.

use super::mask::Mask;
use postui_core::checks::Outcome;
use serde::Serialize;

/// One check's outcome, every string in it through the run's mask.
#[derive(Clone, Debug, Serialize)]
pub(super) struct ShownCheck {
    #[serde(flatten)]
    kind: Kind,
    /// Why it failed, the line after `check failed: `; `None` when it
    /// passed.
    #[serde(skip)]
    failure: Option<String>,
}

/// The JSON's two shapes (§2.3): `{"check": "status", "expected", "got",
/// "passed"}` and `{"check": FILTER, "passed", "got": [outputs], "error"}`.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
enum Kind {
    Status { check: &'static str, expected: Vec<u16>, got: u16, passed: bool },
    Jq { check: String, passed: bool, got: Vec<serde_json::Value>, error: Option<String> },
}

impl ShownCheck {
    pub(super) fn passed(&self) -> bool {
        match &self.kind {
            Kind::Status { passed, .. } | Kind::Jq { passed, .. } => *passed,
        }
    }

    /// What `run -v` lists for the check: its failure, else what passed
    /// (`status 200`, or the filter).
    pub(super) fn summary(&self) -> String {
        if let Some(failure) = &self.failure {
            return failure.clone();
        }
        match &self.kind {
            Kind::Status { got, .. } => format!("status {got}"),
            Kind::Jq { check, .. } => check.clone(),
        }
    }
}

/// `outcomes` masked by `mask`. An output is parsed as the JSON jq printed
/// and masked as a tree, as `run --jq` masks its results; one serde can't
/// read (jaq's `nan`) is kept as a masked string.
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
                Outcome::Jq { filter, got, error } => Kind::Jq {
                    check: mask.apply(filter),
                    passed,
                    got: got
                        .iter()
                        .map(|value| match serde_json::from_str(value) {
                            Ok(json) => mask.apply_json(json),
                            Err(_) => serde_json::Value::String(mask.apply(value)),
                        })
                        .collect(),
                    error: error.as_deref().map(|e| mask.apply(e)),
                },
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

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::model::Checks;

    fn evaluate(status: &[u16], jq: &[&str], got: u16, body: &str) -> Vec<Outcome> {
        let checks =
            Checks { status: status.to_vec(), jq: jq.iter().map(|f| f.to_string()).collect() };
        postui_core::checks::evaluate(&checks, got, body)
    }

    /// §2.3's JSON: the status check's object and a jq check's, in file
    /// order.
    #[test]
    fn the_json_has_one_object_a_check() {
        let out = shown(&evaluate(&[200], &[".a == 1"], 404, r#"{"a":2}"#), &Mask::none());
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"check": "status", "expected": [200], "got": 404, "passed": false},
                {"check": ".a == 1", "passed": false, "got": [false], "error": null},
            ])
        );
        let lines: Vec<String> = failure_lines(&out).collect();
        assert_eq!(
            lines,
            ["check failed: status 404, expected 200", "check failed: .a == 1 gave false"]
        );
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
}
