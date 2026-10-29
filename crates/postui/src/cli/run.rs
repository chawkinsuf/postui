//! `postui run`: the run session `run` and `suite` share, and the report of
//! one request (spec 2026-10-28 §4 "Piece 2 — run"). A session opens the
//! project read-only, picks the environment, applies `--select` overrides in
//! memory (the read-only disk refuses the persist, so nothing is written
//! back) and holds the one mask every printed string passes through.

use super::check::reason;
use super::mask::{self, Mask};
use super::out::table;
use super::read::{open, pick_env, warnings_text};
use super::{CliError, Global, target};
use crate::hostfs;
use crate::http::{Clients, RawResponse, send_raw};
use postui_core::model::{HttpRequest, Method};
use postui_core::prepare::{self, PrepareWarning, PreparedRequest};
use postui_core::project::Project;
use postui_core::{jq, storage, varmodel};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

/// The status a failed run ends with.
pub(super) const FAILED: u8 = 1;

/// A response status from here up is a failure (`run` exits 1).
pub(super) const FIRST_FAILING_STATUS: u16 = 400;

/// What `run` and `suite` share on their command lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOpts {
    /// `--env`: a slug or display name; `None` is the TUI's saved one.
    pub env: Option<String>,
    /// Each `--select SELECTOR=OPTION`, in the order given.
    pub selects: Vec<String>,
    pub show_secrets: bool,
    /// `--timeout`, already defaulted: `None` is no limit.
    pub timeout: Option<Duration>,
}

/// What a run needs to prepare and send requests.
pub(super) struct Session {
    pub project: Project,
    pub root: PathBuf,
    /// The environment's slug; `None` when the project has none.
    pub env: Option<String>,
    pub mask: Mask,
    pub clients: Clients,
    /// What opening the project warned about.
    pub warnings: Vec<String>,
}

/// Opens the project read-only, makes `--env` the environment and applies
/// the `--select`s, then builds the mask (from the secrets of the
/// environment as selected) and the clients. A selector or option that
/// doesn't exist is the error, naming the choices.
pub(super) fn session(g: &Global, opts: &RunOpts) -> Result<Session, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (mut project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning());
    let env = pick_env(&mut project, opts.env.as_deref())?;
    for select in &opts.selects {
        let (selector, option) = select.split_once('=').ok_or_else(|| {
            CliError::new(format!("--select takes SELECTOR=OPTION, not \"{select}\""))
        })?;
        select_option(&mut project, env.as_deref().unwrap_or_default(), selector, option)?;
    }
    let mask = if opts.show_secrets { Mask::none() } else { mask::of_env(&project) };
    Ok(Session { project, root, env, mask, clients: Clients::with_timeout(opts.timeout), warnings })
}

/// Picks `option` of `selector` for this run: in memory only.
fn select_option(
    project: &mut Project,
    env: &str,
    selector: &str,
    option: &str,
) -> Result<(), CliError> {
    let model = project.variables();
    if !model.selectors.contains_key(selector) {
        let names: Vec<&str> = model.selectors.keys().map(String::as_str).collect();
        return Err(CliError::new(if names.is_empty() {
            format!("no such selector \"{selector}\"; this project has none")
        } else {
            format!("no such selector \"{selector}\"; the selectors are {}", names.join(", "))
        }));
    }
    let options: Vec<String> = varmodel::options_of(model, project.env_data(), selector)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    if !options.iter().any(|o| o == option) {
        return Err(CliError::new(if options.is_empty() {
            format!("no such option \"{option}\"; selector \"{selector}\" has no options here")
        } else {
            format!(
                "no such option \"{option}\" for selector \"{selector}\"; the options are {}",
                options.join(", ")
            )
        }));
    }
    // The read-only disk refuses the persist; the pick stays in memory.
    project.set_selection_for(env, selector, option);
    Ok(())
}

/// What `prepare` made of a request: it and its warnings, or why it refused.
pub(super) type Prepared = Result<(PreparedRequest, Vec<PrepareWarning>), String>;

/// What `prepare` makes of request `slug` against the session's
/// environment. The outer `Err` is a request that can't be read (not in the
/// project, or its file is broken: exit 2, naming its file); the inner
/// `Err` is `prepare` refusing it (an unresolved variable), which is a
/// result of the run, not a mistake on its line. The request as read comes
/// back beside the outcome, so a refused one still has its method and URL.
/// The outer errors' messages have been through the mask.
pub(super) fn prepare_one(
    s: &mut Session,
    slug: &str,
) -> Result<(HttpRequest, Prepared), CliError> {
    let file = storage::request_path(&s.root, slug);
    let Some(listing) = s.project.requests().iter().find(|l| l.slug == slug) else {
        let message = format!("no request \"{slug}\"; postui requests lists them");
        return Err(CliError::new(s.mask.apply(&message)));
    };
    if let Some(error) = &listing.broken {
        return Err(CliError::at(s.mask.apply(error), file));
    }
    let req =
        s.project.read_request(slug).map_err(|e| CliError::at(s.mask.apply(&reason(e)), file))?;
    let outcome = prepare::prepare(&req, &s.project.prepare_context()).map_err(|e| e.to_string());
    Ok((req, outcome))
}

/// The URL as printed: what the screens show, or what the wire carries with
/// `--show-secrets`; the mask then hides any secret value still in it.
pub(super) fn shown_url(mask: &Mask, request: &PreparedRequest, show_secrets: bool) -> String {
    mask.apply(if show_secrets { &request.url } else { &request.display_url })
}

/// `postui run`: one request, prepared and, unless `--dry-run`, sent. Every
/// string in it has been through the session's mask.
#[derive(Serialize)]
pub struct RunReport {
    request: String,
    file: PathBuf,
    environment: Option<String>,
    method: Option<Method>,
    url: Option<String>,
    request_headers: Vec<ShownHeader>,
    request_body: Option<String>,
    dry_run: bool,
    /// What came back; `null` for a dry run or when nothing did.
    response: Option<ResponseReport>,
    jq: Option<JqReport>,
    output_file: Option<PathBuf>,
    /// Why there is no response (an unresolved variable, a send error).
    error: Option<String>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ShownHeader {
    name: String,
    value: String,
}

/// What came back, masked. `body` and `body_json` are `null` when `--jq`
/// or `-o` took the body; a failing `--jq` leaves them filled.
#[derive(Serialize)]
struct ResponseReport {
    status: u16,
    ttfb_ms: u64,
    elapsed_ms: u64,
    /// The raw length in bytes.
    size: usize,
    headers: Vec<ShownHeader>,
    body: Option<String>,
    /// The masked body parsed as JSON; `null` when it doesn't parse.
    body_json: Option<serde_json::Value>,
}

/// What `--jq` made of the body: each result parsed as JSON, or why not.
#[derive(Serialize)]
struct JqReport {
    filter: String,
    output: Option<Vec<serde_json::Value>>,
    error: Option<String>,
}

/// Whole milliseconds.
pub(super) fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Runs `filter` over the masked body `text`: each result parsed as the
/// JSON jq printed, then masked (strings and keys, so an escaped secret
/// doesn't get through), with the JSON line it prints as.
fn run_jq(
    filter: &str,
    text: &str,
    mask: &Mask,
) -> Result<Vec<(String, serde_json::Value)>, String> {
    let doc = jq::JqDocument::parse(text).map_err(|e| e.message())?;
    let results = jq::run(filter, &doc).map_err(|e| e.message())?;
    results
        .into_iter()
        .map(|r| {
            let value: serde_json::Value =
                serde_json::from_str(&r).map_err(|e| format!("a result isn't JSON: {e}"))?;
            let value = mask.apply_json(value);
            Ok((value.to_string(), value))
        })
        .collect()
}

/// Writes `bytes` to `file` (made absolute against the working directory)
/// and gives the absolute path.
pub(super) fn write_output(file: &Path, bytes: &[u8]) -> Result<PathBuf, CliError> {
    let absolute = std::path::absolute(file)
        .map_err(|e| CliError::at(format!("can't write {}: {e}", file.display()), file.into()))?;
    hostfs::write_user_file(&absolute, bytes).map_err(|e| {
        CliError::at(format!("can't write {}: {e}", absolute.display()), absolute.clone())
    })?;
    Ok(absolute)
}

/// Fills `report` from `raw`: the response, the jq results, the `-o` file.
fn record(
    report: &mut RunReport,
    mask: &Mask,
    raw: RawResponse,
    jq: Option<&str>,
    output: Option<&Path>,
) -> Result<(), CliError> {
    // Parsed unmasked (a mask inside JSON text can break it, a number for
    // one), then masked as values.
    let plain = String::from_utf8_lossy(&raw.bytes).into_owned();
    let mut response = ResponseReport {
        status: raw.status,
        ttfb_ms: millis(raw.ttfb),
        elapsed_ms: millis(raw.elapsed),
        size: raw.bytes.len(),
        headers: raw
            .headers
            .iter()
            .map(|(n, v)| ShownHeader { name: mask.apply(n), value: mask.apply(v) })
            .collect(),
        body: None,
        body_json: None,
    };
    // What `-o` would write, and whether the body is shown.
    let mut file_bytes = None;
    let mut show_body = false;
    match jq {
        Some(filter) => match run_jq(filter, &plain, mask) {
            Ok(results) => {
                let (lines, values): (Vec<String>, Vec<serde_json::Value>) =
                    results.into_iter().unzip();
                file_bytes = output.map(|_| {
                    lines.iter().flat_map(|l| [l.as_str(), "\n"]).collect::<String>().into_bytes()
                });
                report.jq = Some(JqReport {
                    filter: mask.apply(filter),
                    output: output.is_none().then_some(values),
                    error: None,
                });
            }
            // The filter failed: the body is shown (nothing is written to
            // `-o`), so the caller needn't send the request again.
            Err(message) => {
                show_body = true;
                report.jq = Some(JqReport {
                    filter: mask.apply(filter),
                    output: None,
                    error: Some(mask.apply(&message)),
                });
            }
        },
        None if output.is_some() => file_bytes = Some(raw.bytes),
        None => show_body = true,
    }
    if show_body {
        response.body_json = serde_json::from_str(&plain).ok().map(|v| mask.apply_json(v));
        // The mask covers a secret as sent, JSON-escaped (also `/` as `\/`
        // and non-ASCII as `\uXXXX`) and percent-encoded for a query string.
        // Other encodings (a path segment's percent-encoding, any transform
        // the server applies) stay as sent in `body`; `body_json` has the
        // parsed strings masked.
        response.body = Some(mask.apply(&plain));
    }
    if let (Some(file), Some(bytes)) = (output, file_bytes) {
        report.output_file = Some(write_output(file, &bytes)?);
    }
    report.response = Some(response);
    Ok(())
}

/// Runs request `slug` of the project `g` names. With `dry_run` it only
/// prepares it; else it sends it, and `jq` filters the body and `output`
/// takes it (or the jq results) to a file.
pub async fn run(
    g: &Global,
    slug: &str,
    opts: &RunOpts,
    jq: Option<&str>,
    output: Option<&Path>,
    dry_run: bool,
) -> Result<RunReport, CliError> {
    let mut s = session(g, opts)?;
    let (read, prepared) = prepare_one(&mut s, slug)?;
    let mut report = RunReport {
        request: slug.to_string(),
        file: storage::request_path(&s.root, slug),
        environment: s.env.clone(),
        method: None,
        url: None,
        request_headers: Vec::new(),
        request_body: None,
        dry_run,
        response: None,
        jq: None,
        output_file: None,
        error: None,
        warnings: s.warnings.iter().map(|w| s.mask.apply(w)).collect(),
    };
    let (request, warnings) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            // Refused: the method and URL as written.
            report.method = Some(read.method);
            report.url = Some(s.mask.apply(&read.url));
            report.error = Some(s.mask.apply(&message));
            return Ok(report);
        }
    };
    // The session's mask plus what this request derives from its secrets.
    let mask = s.mask.with_sent(&request);
    // What the screens show, or what the wire carries with --show-secrets;
    // the mask then hides any secret value still in the text.
    let show = opts.show_secrets;
    report.method = Some(request.method);
    report.url = Some(shown_url(&mask, &request, show));
    report.request_headers = request
        .wire
        .iter()
        .map(|h| ShownHeader {
            name: mask.apply(if show { &h.name } else { &h.display_name }),
            value: mask.apply(if show { &h.value } else { &h.display }),
        })
        .collect();
    report.request_body = request.body.as_deref().map(|b| mask.apply(b));
    report.warnings.extend(warnings.iter().map(|w| mask.apply(&w.to_string())));
    if dry_run {
        return Ok(report);
    }
    match send_raw(s.clients.for_request(&request), &request).await {
        Ok(raw) => record(&mut report, &mask, raw, jq, output)?,
        Err(message) => report.error = Some(mask.apply(&message)),
    }
    Ok(report)
}

impl RunReport {
    /// 1 when the run failed: an error, a status of 400 or more, or a jq
    /// error; else 0.
    pub fn status(&self) -> ExitCode {
        let status = self.response.as_ref().map(|r| r.status);
        let jq_failed = self.jq.as_ref().is_some_and(|j| j.error.is_some());
        if self.error.is_some() || jq_failed || status.is_some_and(|s| s >= FIRST_FAILING_STATUS) {
            ExitCode::from(FAILED)
        } else {
            ExitCode::SUCCESS
        }
    }

    /// The request line, what identifies the run, the request's headers and
    /// body, then the error and warnings.
    pub fn text(&self) -> String {
        let mut out = match (&self.method, &self.url) {
            (Some(method), Some(url)) => format!("{} {url}\n", method.as_str()),
            _ => format!("{}\n", self.request),
        };
        let mut rows = vec![
            vec!["request".to_string(), self.request.clone()],
            vec!["file".to_string(), self.file.display().to_string()],
            vec!["environment".to_string(), self.environment.as_deref().unwrap_or("(none)").into()],
        ];
        if self.dry_run {
            rows.push(vec!["dry run".to_string(), "nothing was sent".to_string()]);
        }
        out.push_str(&table(&rows));
        if !self.request_headers.is_empty() {
            let rows: Vec<Vec<String>> = self
                .request_headers
                .iter()
                .map(|h| vec![h.name.clone(), h.value.clone()])
                .collect();
            out.push_str("\nrequest headers\n");
            out.push_str(&table(&rows));
        }
        if let Some(body) = &self.request_body {
            out.push_str(&format!("\nrequest body\n{body}\n"));
        }
        if let Some(r) = &self.response {
            out.push_str(&format!(
                "\nresponse\n  {} in {} ms (first byte {} ms), {} bytes\n",
                r.status, r.elapsed_ms, r.ttfb_ms, r.size
            ));
            let rows: Vec<Vec<String>> =
                r.headers.iter().map(|h| vec![h.name.clone(), h.value.clone()]).collect();
            if !rows.is_empty() {
                out.push_str("\nresponse headers\n");
                out.push_str(&table(&rows));
            }
            if let Some(body) = &r.body {
                out.push_str(&format!("\nresponse body\n{body}\n"));
            }
        }
        if let Some(j) = &self.jq {
            out.push_str(&format!("\njq {}\n", j.filter));
            for value in j.output.iter().flatten() {
                out.push_str(&format!("{value}\n"));
            }
            if let Some(error) = &j.error {
                out.push_str(&format!("  error: {error}\n"));
            }
        }
        if let Some(file) = &self.output_file {
            out.push_str(&format!("\nwritten to {}\n", file.display()));
        }
        if let Some(error) = &self.error {
            out.push_str(&format!("\nerror\n  {error}\n"));
        }
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}
