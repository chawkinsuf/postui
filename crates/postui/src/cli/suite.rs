//! `postui suite`: every request of a project, or of one space, sent one at a
//! time in sidebar order, with its status and timing (spec 2026-10-28 §4
//! "Piece 2 — run"). A request that can't be read or prepared is that
//! request's result, and the suite goes on.

use super::out::table;
use super::read::{displayed, warnings_text};
use super::run::{RunOpts, millis, prepare_one, session, shown_url};
use super::{CliError, Global};
use crate::http::send_raw;
use postui_core::model::Method;
use serde::Serialize;
use std::process::ExitCode;

/// The status a suite with a failed request ends with.
const FAILED: u8 = 1;

/// A response status from here up is a failure.
const FIRST_FAILING_STATUS: u16 = 400;

/// `postui suite`. Every string in it has been through the session's mask.
#[derive(Serialize)]
pub struct SuiteReport {
    environment: Option<String>,
    space: Option<String>,
    results: Vec<SuiteResult>,
    totals: Totals,
    warnings: Vec<String>,
}

/// One request's outcome. `status`, `elapsed_ms` and `size` are `null` when
/// nothing came back; `method` and `url` too when its file is broken.
#[derive(Serialize)]
struct SuiteResult {
    request: String,
    method: Option<Method>,
    url: Option<String>,
    status: Option<u16>,
    elapsed_ms: Option<u64>,
    size: Option<usize>,
    error: Option<String>,
}

#[derive(Serialize)]
struct Totals {
    requests: usize,
    passed: usize,
    failed: usize,
    /// The sum of the results' own.
    elapsed_ms: u64,
}

impl SuiteResult {
    fn passed(&self) -> bool {
        self.status.is_some_and(|s| s < FIRST_FAILING_STATUS)
    }
}

/// Runs the requests of `space` (every space when `None`) of the project `g`
/// names, one at a time in the order `postui requests` lists them.
pub async fn suite(
    g: &Global,
    space: Option<&str>,
    opts: &RunOpts,
) -> Result<SuiteReport, CliError> {
    let mut s = session(g, opts)?;
    let spaces: Vec<String> = match space {
        None => s.project.spaces().to_vec(),
        Some(wanted) if s.project.spaces().iter().any(|n| n == wanted) => vec![wanted.to_string()],
        Some(wanted) => {
            return Err(CliError::new(format!("no such space \"{wanted}\" in this project")));
        }
    };
    let mut results = Vec::new();
    for space in spaces {
        for slug in displayed(&s.project, &space) {
            let mut result = SuiteResult {
                request: slug.clone(),
                method: None,
                url: None,
                status: None,
                elapsed_ms: None,
                size: None,
                error: None,
            };
            match prepare_one(&mut s, &slug) {
                // A broken file is this request's result, naming the file.
                Err(e) => {
                    let at = e.file.map(|f| format!(" ({})", f.display())).unwrap_or_default();
                    result.error = Some(s.mask.apply(&format!("{}{at}", e.message)));
                }
                Ok(Err(message)) => result.error = Some(s.mask.apply(&message)),
                Ok(Ok((request, _))) => {
                    result.method = Some(request.method);
                    result.url = Some(shown_url(&s.mask, &request, opts.show_secrets));
                    match send_raw(s.clients.for_request(&request), &request).await {
                        Ok(raw) => {
                            result.status = Some(raw.status);
                            result.elapsed_ms = Some(millis(raw.elapsed));
                            result.size = Some(raw.bytes.len());
                        }
                        Err(message) => result.error = Some(s.mask.apply(&message)),
                    }
                }
            }
            results.push(result);
        }
    }
    let passed = results.iter().filter(|r| r.passed()).count();
    let totals = Totals {
        requests: results.len(),
        passed,
        failed: results.len() - passed,
        elapsed_ms: results.iter().filter_map(|r| r.elapsed_ms).sum(),
    };
    Ok(SuiteReport {
        environment: s.env.clone(),
        space: space.map(str::to_string),
        results,
        totals,
        warnings: s.warnings.iter().map(|w| s.mask.apply(w)).collect(),
    })
}

impl SuiteReport {
    /// 1 when any request failed to prepare or send or returned 400 or more.
    pub fn status(&self) -> ExitCode {
        if self.totals.failed > 0 { ExitCode::from(FAILED) } else { ExitCode::SUCCESS }
    }

    /// A heading, one line per request (status, time, what failed), then the
    /// totals.
    pub fn text(&self) -> String {
        let mut out = format!(
            "suite  environment {}  space {}\n",
            self.environment.as_deref().unwrap_or("(none)"),
            self.space.as_deref().unwrap_or("(all)")
        );
        let rows: Vec<Vec<String>> = self
            .results
            .iter()
            .map(|r| {
                vec![
                    r.status.map_or_else(|| "error".to_string(), |s| s.to_string()),
                    r.elapsed_ms.map_or_else(String::new, |ms| format!("{ms} ms")),
                    r.request.clone(),
                    r.error.clone().unwrap_or_default(),
                ]
            })
            .collect();
        out.push_str(&table(&rows));
        let t = &self.totals;
        out.push_str(&format!(
            "\n{} requests, {} passed, {} failed, {} ms\n",
            t.requests, t.passed, t.failed, t.elapsed_ms
        ));
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}
