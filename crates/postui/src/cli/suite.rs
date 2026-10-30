//! `postui suite`: every request of a project, or of one space, sent one at a
//! time in sidebar order, with its status and timing (spec 2026-10-28 §4
//! "Piece 2 — run"). A request that can't be read or prepared is that
//! request's result, and the suite goes on.

use super::mask::Mask;
use super::out::{count, table_under};
use super::read::{displayed, spaces_of, warnings_text};
use super::run::{
    FAILED, FIRST_FAILING_STATUS, RunOpts, millis, prepare_one, refused, session, shown_url,
};
use super::{CliError, Global};
use crate::http::send_raw;
use postui_core::model::Method;
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

/// `postui suite`. Every string in it has been through the session's mask.
#[derive(Serialize)]
pub struct SuiteReport {
    environment: Option<String>,
    space: Option<String>,
    results: Vec<SuiteResult>,
    totals: Totals,
    warnings: Vec<String>,
    /// The absolute path `-o` writes the report to; `null` without `-o`, or
    /// when it could not be written.
    output_file: Option<PathBuf>,
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
/// names, one at a time in the order `postui requests` lists them. The
/// session's mask comes back beside the report, for what is added to it
/// after the run.
pub async fn suite(
    g: &Global,
    space: Option<&str>,
    opts: &RunOpts,
) -> Result<(SuiteReport, Mask), CliError> {
    let mut s = session(g, opts)?;
    let slugs: Vec<String> = spaces_of(&s.project, space)?
        .iter()
        .flat_map(|space| displayed(&s.project, space))
        .collect();
    // Before anything is sent: a misspelt `--var` would change no request.
    s.check_vars(&slugs, "no request's [variables] has it")?;
    let mut results = Vec::new();
    // What `run` would warn of for each request, in suite order.
    let mut request_warnings = Vec::new();
    for slug in slugs {
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
            // A broken file is this request's result, naming the file
            // first and absolute, as an error does (R22).
            Err(e) => {
                let at = e.file.map(|f| format!("{}: ", f.display())).unwrap_or_default();
                result.error = Some(format!("{at}{}", e.message.trim_end()));
            }
            // The file read; only the variables failed: it keeps its
            // method and URL as written.
            Ok((read, Err(message))) => {
                let (method, url, error) = refused(&s.mask, &read, &message);
                (result.method, result.url, result.error) = (Some(method), Some(url), Some(error));
            }
            Ok((_, Ok((request, prepare_warnings)))) => {
                request_warnings
                    .extend(prepare_warnings.iter().map(|w| s.mask.apply(&format!("{slug}: {w}"))));
                result.method = Some(request.method);
                result.url = Some(shown_url(&request, opts.show_secrets));
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
    let passed = results.iter().filter(|r| r.passed()).count();
    let totals = Totals {
        requests: results.len(),
        passed,
        failed: results.len() - passed,
        elapsed_ms: results.iter().filter_map(|r| r.elapsed_ms).sum(),
    };
    let report = SuiteReport {
        environment: s.env.clone(),
        space: space.map(str::to_string),
        results,
        totals,
        warnings: s.warnings.iter().map(|w| s.mask.apply(w)).chain(request_warnings).collect(),
        output_file: None,
        write_failed: false,
    };
    Ok((report, s.mask))
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

    /// Names the file `-o` will write the report to, before it is rendered.
    pub fn set_output_file(&mut self, file: PathBuf) {
        self.output_file = Some(file);
    }

    /// Records that `-o` could not be written: the requests were sent, so the
    /// report still prints, with `message` (already masked) as a warning.
    pub fn note_write_failure(&mut self, message: String) {
        self.output_file = None;
        self.warnings.push(message);
        self.write_failed = true;
    }

    /// A heading, a row a request (status, time, method, slug, URL, size)
    /// with what failed under it, the totals, and where `-o` wrote it.
    pub fn text(&self) -> String {
        let mut out = format!(
            "suite  environment {}  space {}\n",
            self.environment.as_deref().unwrap_or("(none)"),
            self.space.as_deref().unwrap_or("(all)")
        );
        let rows: Vec<(Vec<String>, &str)> = self
            .results
            .iter()
            .map(|r| {
                let cells = vec![
                    r.status.map_or_else(|| "error".to_string(), |s| s.to_string()),
                    r.elapsed_ms.map_or_else(String::new, |ms| format!("{ms} ms")),
                    r.method.map_or("", Method::as_str).to_string(),
                    r.request.clone(),
                    r.url.clone().unwrap_or_default(),
                    r.size.map_or_else(String::new, |n| count(n, "byte")),
                ];
                (cells, r.error.as_deref().unwrap_or_default())
            })
            .collect();
        out.push_str(&table_under(&rows));
        let t = &self.totals;
        out.push_str(&format!(
            "\n{} requests, {} passed, {} failed, {} ms\n",
            t.requests, t.passed, t.failed, t.elapsed_ms
        ));
        // Absolute: the file is the user's, named from their working
        // directory, not the project's (R20).
        if let Some(file) = &self.output_file {
            out.push_str(&format!("written to {}\n", file.display()));
        }
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}
