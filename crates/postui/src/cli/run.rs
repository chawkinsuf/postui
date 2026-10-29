//! `postui run`: the run session `run` and `suite` share, and the report of
//! one request (spec 2026-10-28 §4 "Piece 2 — run"). A session opens the
//! project read-only, picks the environment, applies `--select` overrides in
//! memory (the read-only disk refuses the persist, so nothing is written
//! back) and holds the one mask every printed string passes through.

use super::check::reason;
use super::mask::{self, Mask};
use super::out::{count, rel, table};
use super::read::{open, pick_env, warnings_text};
use super::{CliError, Global, target};
use crate::hostfs;
use crate::http::{Clients, RawResponse, send_raw};
use postui_core::model::{HttpRequest, Method};
use postui_core::prepare::{self, PrepareContext, PrepareWarning, PreparedRequest};
use postui_core::project::Project;
use postui_core::{jq, storage, varmodel};
use serde::Serialize;
use std::borrow::Cow;
use std::cell::OnceCell;
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
    /// Each `--select SELECTOR=OPTION` as (selector, option), in the order
    /// given.
    pub selects: Vec<(String, String)>,
    pub show_secrets: bool,
    /// `--timeout`, already defaulted: how long the request may take to get
    /// its final response started, and then each gap between reads of the
    /// body; `None` is no limit.
    pub timeout: Option<Duration>,
}

/// What a run needs to prepare and send requests.
pub(super) struct Session {
    pub project: Project,
    pub root: PathBuf,
    /// The environment's slug; `None` when the project has none.
    pub env: Option<String>,
    pub mask: Mask,
    /// What `prepare` needs of the project: the selections are fixed
    /// before any request is prepared, so it is built once.
    pub context: PrepareContext,
    pub clients: Clients,
    /// What opening the project warned about.
    pub warnings: Vec<String>,
}

/// Opens the project read-only, makes `--env` the environment and applies
/// the `--select`s, then builds the mask (from the secrets of the
/// environment as selected), the prepare context and the clients. A
/// selector or option that doesn't exist is the error, naming the choices.
pub(super) fn session(g: &Global, opts: &RunOpts) -> Result<Session, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (mut project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning());
    let env = pick_env(&mut project, opts.env.as_deref())?;
    for (selector, option) in &opts.selects {
        select_option(&mut project, env.as_deref().unwrap_or_default(), selector, option)?;
    }
    let mask = if opts.show_secrets { Mask::none() } else { mask::of_env(&project) };
    let context = project.prepare_context();
    let clients = Clients::with_timeout(opts.timeout)
        .map_err(|e| CliError::new(format!("could not start the HTTP client: {e}")))?;
    Ok(Session { project, root, env, mask, context, clients, warnings })
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
    let outcome = prepare::prepare(&req, &s.context).map_err(|e| e.to_string());
    Ok((req, outcome))
}

/// A request `prepare` refused, as printed: the method and URL as read, the
/// URL and the refusal masked.
pub(super) fn refused(mask: &Mask, read: &HttpRequest, message: &str) -> (Method, String, String) {
    (read.method, mask.apply(&read.url), mask.apply(message))
}

/// The URL as printed: what the screens show, or what the wire carries with
/// `--show-secrets`.
pub(super) fn shown_url(request: &PreparedRequest, show_secrets: bool) -> String {
    if show_secrets { &request.url } else { &request.display_url }.clone()
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
    /// What the text's paths are relative to; the JSON's are absolute.
    #[serde(skip)]
    root: PathBuf,
}

#[derive(Serialize)]
struct ShownHeader {
    name: String,
    value: String,
}

/// What came back, masked. The text prints the body as it came, in `body`.
/// The JSON gives it once: as `body_json` when it parses as JSON, else as
/// the string `body` (spec §5, #217). Both are `null` when `--jq` or `-o`
/// took the body (their own keys say so); a failing `--jq` leaves the body
/// shown.
#[derive(Serialize)]
struct ResponseReport {
    status: u16,
    ttfb_ms: u64,
    elapsed_ms: u64,
    /// The raw length in bytes.
    size: usize,
    headers: Vec<ShownHeader>,
    /// The body as text: always for the text, and for the JSON when it
    /// doesn't parse as JSON.
    body: Option<String>,
    /// The body parsed as JSON (from the unmasked text), then masked as a
    /// tree; only for the JSON.
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

/// Runs `filter` over the body `text`, unmasked (masking first would break
/// the JSON where a mask lands in a number or an escape): each result is
/// parsed as the JSON jq printed, then masked as a tree, with the JSON line
/// it prints as.
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

/// `file` made absolute against the working directory.
pub(super) fn absolute(file: &Path) -> Result<PathBuf, CliError> {
    std::path::absolute(file)
        .map_err(|e| CliError::at(format!("could not write {}: {e}", file.display()), file.into()))
}

/// Writes `bytes` to the absolute path `file`.
pub(super) fn write_output(file: &Path, bytes: &[u8]) -> Result<(), CliError> {
    hostfs::write_output_file(file, bytes)
        .map_err(|e| CliError::at(format!("could not write {}: {e}", file.display()), file.into()))
}

/// Fills `report` from `raw`: the response, the jq results, the `-o` file.
/// A file that can't be written is the report's `error`, with the response
/// shown as if there were no `-o`. The body takes the form the report will
/// print in: text, or with `json` parsed when it parses.
fn record(
    report: &mut RunReport,
    mask: &Mask,
    raw: RawResponse,
    jq: Option<&str>,
    output: Option<&Path>,
    json: bool,
) {
    // The body as text, decoded once and only when something reads it (a
    // byte-exact `-o` that succeeds never does).
    let decoded = OnceCell::new();
    let text = || decoded.get_or_init(|| String::from_utf8_lossy(&raw.bytes)).as_ref();
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
    // What `-o` would write, the jq results it would take, and whether the
    // body is shown.
    let mut file_bytes: Option<Cow<[u8]>> = None;
    let mut taken_results = None;
    let mut show_body = false;
    match jq {
        Some(filter) => match run_jq(filter, text(), mask) {
            Ok(results) => {
                let (lines, values): (Vec<String>, Vec<serde_json::Value>) =
                    results.into_iter().unzip();
                // The results go to the file or into the report, not both.
                let (shown, taken) = match output {
                    Some(_) => {
                        let joined: String =
                            lines.iter().flat_map(|l| [l.as_str(), "\n"]).collect();
                        file_bytes = Some(Cow::Owned(joined.into_bytes()));
                        (None, Some(values))
                    }
                    None => (Some(values), None),
                };
                report.jq =
                    Some(JqReport { filter: mask.apply(filter), output: shown, error: None });
                taken_results = taken;
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
        // The file is the response itself, kept byte-exact (a binary
        // download), so it is not masked (spec §4, §8 #9); `--jq -o` writes
        // masked text because it is derived output.
        None if output.is_some() => file_bytes = Some(Cow::Borrowed(&raw.bytes)),
        None => show_body = true,
    }
    if let (Some(file), Some(bytes)) = (output, file_bytes) {
        match absolute(file).and_then(|path| write_output(&path, &bytes).map(|()| path)) {
            Ok(written) => report.output_file = Some(written),
            // The request is sent and can't be resent safely: the report
            // keeps what came back, as if there were no `-o`.
            Err(e) => {
                report.error = Some(mask.apply(&e.message));
                match (taken_results, report.jq.as_mut()) {
                    (Some(values), Some(j)) => j.output = Some(values),
                    _ => show_body = true,
                }
            }
        }
    }
    if show_body {
        let text = text();
        // Parsed from the unmasked text, then masked as a tree: masking the
        // text first would break the JSON where a mask lands in a number or
        // an escape. A body that parses is given only parsed (its exact
        // spacing and duplicate keys are lost; `-o` keeps the bytes).
        let parsed = json.then(|| serde_json::from_str::<serde_json::Value>(text).ok()).flatten();
        match parsed {
            Some(value) => response.body_json = Some(mask.apply_json(value)),
            // Masked by exact value, so a secret the server echoes
            // re-encoded (JSON-escaped, percent-encoded) is not caught: the
            // masking was cut back to a plain replace (user-approved, spec
            // §4), and the skill tells the agent so.
            None => response.body = Some(mask.apply(text)),
        }
    }
    report.response = Some(response);
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
        root: s.root.clone(),
    };
    let (request, warnings) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            let (method, url, error) = refused(&s.mask, &read, &message);
            (report.method, report.url, report.error) = (Some(method), Some(url), Some(error));
            return Ok(report);
        }
    };
    let mask = &s.mask;
    // The request side as the screens show it, or as the wire carries it
    // with --show-secrets.
    let show = opts.show_secrets;
    report.method = Some(request.method);
    report.url = Some(shown_url(&request, show));
    report.request_headers = request
        .wire
        .iter()
        .map(|h| ShownHeader {
            name: if show { &h.name } else { &h.display_name }.clone(),
            value: if show { &h.value } else { &h.display }.clone(),
        })
        .collect();
    // `prepare` has no display form for the body, so it goes through the
    // output mask: a secret under MASK_MIN_CHARS prints (the spec doesn't
    // mask values under 4 characters; `check` warns).
    report.request_body = request.body.as_deref().map(|b| mask.apply(b));
    report.warnings.extend(warnings.iter().map(|w| mask.apply(&w.to_string())));
    if dry_run {
        return Ok(report);
    }
    match send_raw(s.clients.for_request(&request), &request).await {
        Ok(raw) => record(&mut report, mask, raw, jq, output, g.json),
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
    /// body, the response with its body exactly as it came (or the jq
    /// results), then the error and warnings.
    pub fn text(&self) -> String {
        let mut out = match (&self.method, &self.url) {
            (Some(method), Some(url)) => format!("{} {url}\n", method.as_str()),
            _ => format!("{}\n", self.request),
        };
        let mut rows = vec![
            vec!["request".to_string(), self.request.clone()],
            vec!["root".to_string(), self.root.display().to_string()],
            vec!["file".to_string(), rel(&self.root, &self.file)],
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
                "\nresponse\n  {} in {} ms (first byte {} ms), {}\n",
                r.status,
                r.elapsed_ms,
                r.ttfb_ms,
                count(r.size, "byte")
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
                out.push_str(&table(&[vec![format!("error: {error}")]]));
            }
        }
        if let Some(file) = &self.output_file {
            // Absolute: the file is the user's, named from their working
            // directory, not the project's (R20).
            out.push_str(&format!("\nwritten to {}\n", file.display()));
        }
        if let Some(error) = &self.error {
            out.push_str("\nerror\n");
            out.push_str(&table(&[vec![error.clone()]]));
        }
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}
