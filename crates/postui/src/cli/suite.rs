//! `postui suite`: every request of a project, or of one space, sent one at a
//! time in sidebar order, with its status and timing (spec 2026-10-28 §4
//! "Piece 2 — run"). A request that can't be read or prepared is that
//! request's result, and the suite goes on.

use super::out::table;
use super::read::{displayed, spaces_of, warnings_text};
use super::run::{FAILED, FIRST_FAILING_STATUS, RunOpts, millis, prepare_one, session, shown_url};
use super::{CliError, Global};
use crate::http::send_raw;
use postui_core::model::Method;
use serde::Serialize;
use std::process::ExitCode;

/// `postui suite`. Every string in it has been through the session's mask.
#[derive(Serialize)]
pub struct SuiteReport {
    environment: Option<String>,
    space: Option<String>,
    results: Vec<SuiteResult>,
    totals: Totals,
    warnings: Vec<String>,
    /// `-o` could not be written (a warning says so): the run failed.
    #[serde(skip)]
    write_failed: bool,
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
    let spaces = spaces_of(&s.project, space)?;
    let mut results = Vec::new();
    // What `run` would warn of for each request, in suite order.
    let mut request_warnings = Vec::new();
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
                    result.error = Some(format!("{}{at}", e.message.trim_end()));
                }
                // The file read; only the variables failed: it keeps its
                // method and URL as written.
                Ok((read, Err(message))) => {
                    result.method = Some(read.method);
                    result.url = Some(s.mask.apply(&read.url));
                    result.error = Some(s.mask.apply(&message));
                }
                Ok((_, Ok((request, prepare_warnings)))) => {
                    request_warnings.extend(
                        prepare_warnings.iter().map(|w| s.mask.apply(&format!("{slug}: {w}"))),
                    );
                    let mask = s.mask.with_sent(&request);
                    result.method = Some(request.method);
                    result.url = Some(shown_url(&mask, &request, opts.show_secrets));
                    match send_raw(s.clients.for_request(&request), &request).await {
                        Ok(raw) => {
                            result.status = Some(raw.status);
                            result.elapsed_ms = Some(millis(raw.elapsed));
                            result.size = Some(raw.bytes.len());
                        }
                        Err(message) => result.error = Some(mask.apply(&message)),
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
        warnings: s.warnings.iter().map(|w| s.mask.apply(w)).chain(request_warnings).collect(),
        write_failed: false,
    })
}

impl SuiteReport {
    /// 1 when any request failed to prepare or send or returned 400 or more,
    /// or `-o` could not be written.
    pub fn status(&self) -> ExitCode {
        if self.totals.failed > 0 || self.write_failed {
            ExitCode::from(FAILED)
        } else {
            ExitCode::SUCCESS
        }
    }

    /// Records that `-o` could not be written: the requests were sent, so the
    /// report still prints, with `message` as a warning.
    pub fn note_write_failure(&mut self, message: String) {
        self.warnings.push(message);
        self.write_failed = true;
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
                    // One line a row; the JSON has the whole text.
                    r.error.as_deref().and_then(|e| e.lines().next()).unwrap_or_default().into(),
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
