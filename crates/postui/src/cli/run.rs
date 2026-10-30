//! `postui run`: the run session `run` and `suite` share, and the report of
//! one request (spec 2026-10-28 §4 "Piece 2 — run"). A session opens the
//! project read-only, picks the environment, applies `--select` overrides in
//! memory (the read-only disk refuses the persist, so nothing is written
//! back) and holds the one mask every printed string passes through.

use super::check::{declared, reason};
use super::mask::{self, Mask};
use super::out::{count, fields, rel, table};
use super::read::{open, pick_env, warnings_text};
use super::{CliError, Global, target};
use crate::hostfs;
use crate::http::{Clients, RawResponse, send_raw};
use postui_core::model::{Entry, HttpRequest, JqView, Method, applied_jq_filter};
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
    /// Each `--var NAME=VALUE` as (name, value), in the order given: a value
    /// for this run that beats every other source (spec 2026-10-30 D4).
    pub vars: Vec<(String, String)>,
    pub show_secrets: bool,
    /// `--timeout`, already defaulted: how long the request may take to get
    /// its final response started, and then each gap between reads of the
    /// body; `None` is no limit.
    pub timeout: Option<Duration>,
}

/// The jq filter `run` prints the results of instead of the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    /// `--jq F`.
    Jq(String),
    /// `--view` (`None`: the filter the request's response pane applies) or
    /// `--view=NAME` (a saved view), read off the request (D3).
    View(Option<String>),
}

/// How much of a run the text prints (D5): the body alone, as curl does,
/// unless asked for more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// The body (or the jq results, or nothing when `-o` took the body);
    /// the rest that matters goes to stderr.
    Body,
    /// `-i`: the status line and response headers, then the body.
    Include,
    /// `-v`: the whole report.
    Verbose,
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
    /// The `--var`s, which every request prepared gets as its own
    /// `[variables]`, enabled: the source `prepare` ranks first (D4).
    vars: Vec<(String, String)>,
}

/// Opens the project read-only, makes `--env` the environment and applies
/// the `--select`s, then builds the mask (from the secrets of the
/// environment as selected, and any `--var` given for one), the prepare
/// context and the clients. A selector or option that doesn't exist is the
/// error, naming the choices.
pub(super) fn session(g: &Global, opts: &RunOpts) -> Result<Session, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (mut project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning());
    let env = pick_env(&mut project, opts.env.as_deref())?;
    for (selector, option) in &opts.selects {
        select_option(&mut project, env.as_deref().unwrap_or_default(), selector, option)?;
    }
    let model = project.variables();
    let given_secrets: Vec<&(String, String)> = opts
        .vars
        .iter()
        .filter(|(name, _)| model.vars.get(name).is_some_and(|v| v.secret))
        .collect();
    let mask = if opts.show_secrets {
        Mask::none()
    } else {
        mask::of_env(&project, given_secrets.iter().map(|(_, value)| value.clone()))
    };
    let mut context = project.prepare_context();
    // A secret `--var` supplies (one with no stored value, on a fresh clone)
    // is a secret all the same: `prepare` masks it on the request side.
    for (name, _) in &given_secrets {
        context.meta.insert(name.clone(), varmodel::VarMeta::Secret);
    }
    let clients = Clients::with_timeout(opts.timeout)
        .map_err(|e| CliError::new(format!("could not start the HTTP client: {e}")))?;
    Ok(Session { project, root, env, mask, context, clients, warnings, vars: opts.vars.clone() })
}

impl Session {
    /// Refuses a `--var` that `variables.toml` doesn't declare (as a
    /// variable or a selector's field), that the environment gives no value
    /// (it may set one it doesn't declare), and that none of `slugs`'
    /// requests has in its `[variables]`: a misspelt name would otherwise
    /// change nothing. `not_local` says, in the error, that those requests
    /// don't have it.
    pub(super) fn check_vars(&mut self, slugs: &[String], not_local: &str) -> Result<(), CliError> {
        let undeclared: Vec<&str> = self
            .vars
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| !declared(&self.project, name) && !self.context.vars.contains_key(*name))
            .collect();
        if undeclared.is_empty() {
            return Ok(());
        }
        // Read only when a name needs them, which is the rare case (a name
        // only a request's `[variables]` holds, or a misspelling); then each
        // request file is read again here, a small TOML parse, rather than
        // threading the names out of the send loop, which `suite` must run
        // only after this check passes. A broken file is the run's to
        // report, so it has no names here.
        let mut local = std::collections::BTreeSet::new();
        for slug in slugs {
            if let Ok(req) = self.project.read_request(slug) {
                local.extend(req.variables.into_keys());
            }
        }
        match undeclared.into_iter().find(|name| !local.contains(*name)) {
            Some(name) => Err(CliError::new(format!(
                "--var {name}: variables.toml doesn't declare it, and {not_local}"
            ))),
            None => Ok(()),
        }
    }
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
/// result of the run, not a mistake on its line. The request as read (with
/// the `--var`s in its `[variables]`) comes back beside the outcome, so a
/// refused one still has its method and URL. The outer errors' messages
/// have been through the mask.
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
    let mut req =
        s.project.read_request(slug).map_err(|e| CliError::at(s.mask.apply(&reason(e)), file))?;
    // In memory only; what comes back is the request as read in all but
    // these rows (its method, URL and jq filters are untouched).
    for (name, value) in &s.vars {
        req.variables.insert(name.clone(), Entry { value: value.clone(), enabled: true });
    }
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
/// took the body (their own keys say so), a failing `--jq` too (#223); a
/// `-o` that can't be written leaves the body shown.
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
    /// Whether a failure's text says `-o FILE` saves the body: no `-o` was
    /// given and there is a body to save. Only the text says so; for a
    /// script the JSON's null body and `output_file` are the same facts.
    #[serde(skip)]
    suggest_output: bool,
}

impl JqReport {
    /// The failing filter's error as the text prints it: with how to keep
    /// the body it failed on, which isn't printed (#223), when nothing kept
    /// it.
    fn failure(&self) -> Option<String> {
        let error = self.error.as_ref()?;
        Some(match self.suggest_output {
            true => format!("{error}; -o FILE saves the body"),
            false => error.clone(),
        })
    }
}

/// `404 Not Found`: the code and its reason, or the code alone when it has
/// none.
fn status_line(status: u16) -> String {
    match reqwest::StatusCode::from_u16(status).ok().and_then(|s| s.canonical_reason()) {
        Some(reason) => format!("{status} {reason}"),
        None => status.to_string(),
    }
}

/// Whole milliseconds.
pub(super) fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Runs `filter` over the body `text`, unmasked (masking first would break
/// the JSON where a mask lands in a number or an escape): each result is
/// parsed as the JSON jq printed, then masked as a tree.
fn run_jq(filter: &str, text: &str, mask: &Mask) -> Result<Vec<serde_json::Value>, String> {
    let doc = jq::JqDocument::parse(text).map_err(|e| e.message())?;
    let results = jq::run(filter, &doc).map_err(|e| e.message())?;
    results
        .into_iter()
        .map(|r| {
            let value: serde_json::Value =
                serde_json::from_str(&r).map_err(|e| format!("a result isn't JSON: {e}"))?;
            Ok(mask.apply_json(value))
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
    // The results print, the filter's or its error. Whether it fails or
    // not, the body doesn't print with them: a large body is what the
    // filter is there to keep out of the reader's context, and `-o` keeps
    // it for a filter that fails (#223). The cost: without `-o`, a failed
    // filter on a request that can't be sent again (a POST that changes
    // something, a model's answer) loses the body. That was weighed
    // against printing 24 KB of stream on a failure (mine (approved)); the
    // skill tells the agent to add `-o` for such requests.
    if let Some(filter) = jq {
        let (results, error) = match run_jq(filter, text(), mask) {
            Ok(results) => (Some(results), None),
            Err(message) => (None, Some(mask.apply(&message))),
        };
        report.jq = Some(JqReport {
            filter: mask.apply(filter),
            output: results,
            error,
            suggest_output: output.is_none() && !raw.bytes.is_empty(),
        });
    }
    let mut show_body = jq.is_none() && output.is_none();
    if let Some(file) = output {
        // Alone, the file is the response itself, kept byte-exact (a binary
        // download), so it is not masked (spec §4, §8 #9). With a filter it
        // is the body the filter ran on, there to be read when the filter
        // fails, so it is the text the report would print, masked (#223).
        let bytes = match jq {
            Some(_) => Cow::Owned(mask.apply(text()).into_bytes()),
            None => Cow::Borrowed(&raw.bytes[..]),
        };
        match absolute(file).and_then(|path| write_output(&path, &bytes).map(|()| path)) {
            Ok(written) => report.output_file = Some(written),
            // The request is sent and can't be resent safely: the report
            // keeps what came back, as if there were no `-o` (a filter's
            // results, then, and no body: they are what the reader asked
            // to see).
            Err(e) => {
                report.error = Some(mask.apply(&e.message));
                show_body = jq.is_none();
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

/// The jq filter `filter` names for request `read` (`slug`, in `file`):
/// `--jq`'s own, or the request's (D3). A view that isn't there is an error
/// naming the request's views.
fn filter_text(
    filter: &Filter,
    read: &HttpRequest,
    slug: &str,
    file: &Path,
) -> Result<String, CliError> {
    let missing = |what: String| {
        let views: Vec<&str> = read.jq_views.iter().map(|v| v.name.as_str()).collect();
        let choices = if views.is_empty() {
            "it has no saved views".to_string()
        } else {
            format!("its views are {}", views.join(", "))
        };
        CliError::at(format!("{what}; {choices}"), file.to_path_buf())
    };
    match filter {
        Filter::Jq(text) => Ok(text.clone()),
        // What the response pane applies. Whether the pane's switch is off
        // doesn't matter: asking for the view asks for its filter.
        Filter::View(None) => {
            // The reader already spells `jq_current` as its view's own name
            // (matched as `--view=NAME` matches, `HttpRequest::from_toml_str`),
            // so the exact match here finds it.
            let scratch = read.jq.as_deref().unwrap_or_default();
            Some(applied_jq_filter(&read.jq_views, read.jq_current.as_deref(), scratch))
                .filter(|f| !f.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| missing(format!("request {slug} has no jq filter")))
        }
        Filter::View(Some(name)) => {
            let key = JqView::name_key(name);
            read.jq_views
                .iter()
                .find(|v| JqView::name_key(&v.name) == key)
                .map(|v| v.filter.clone())
                .ok_or_else(|| missing(format!("request {slug} has no view \"{name}\"")))
        }
    }
}

/// Runs request `slug` of the project `g` names. With `dry_run` it only
/// prepares it; else it sends it, and `filter` filters the body and
/// `output` takes it to a file.
pub async fn run(
    g: &Global,
    slug: &str,
    opts: &RunOpts,
    filter: Option<&Filter>,
    output: Option<&Path>,
    dry_run: bool,
) -> Result<RunReport, CliError> {
    let mut s = session(g, opts)?;
    // The request first: a slug that names none is the mistake to report.
    let (read, prepared) = prepare_one(&mut s, slug)?;
    s.check_vars(&[slug.to_string()], "the request's [variables] don't have it")?;
    let file = storage::request_path(&s.root, slug);
    let jq = filter.map(|f| filter_text(f, &read, slug, &file)).transpose()?;
    let jq = jq.as_deref();
    let mut report = RunReport {
        request: slug.to_string(),
        file,
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

    /// Prints the report as `shown` asks (D5): the JSON with `json`; the
    /// whole report for `-v` or a dry run (whose point is the request);
    /// otherwise [`Self::streams`]. `None` when it all went out, as
    /// [`crate::cli_out::print_out`] says.
    pub fn print(&self, json: bool, shown: Shown) -> Option<ExitCode> {
        if json || self.dry_run || shown == Shown::Verbose {
            return super::out::emit(self, json, RunReport::text);
        }
        let (out, err) = self.streams(shown == Shown::Include);
        let printed = crate::cli_out::print_out(&out);
        crate::cli_out::print_err(&err);
        printed
    }

    /// What a run prints like curl, as (stdout, stderr). Stdout: with
    /// `include`, the status line and response headers and a blank line;
    /// then the body as it came (nothing when `-o` took it), or the jq
    /// results one a line. Stderr, a `postui: ` line each (D6): the error, a
    /// failing jq filter, a status of 400 or more, and the warnings.
    pub fn streams(&self, include: bool) -> (String, String) {
        let mut out = String::new();
        let mut err = String::new();
        if let Some(r) = &self.response {
            if include {
                out.push_str(&format!(
                    "{}  {} ms (first byte {} ms), {}\n",
                    status_line(r.status),
                    r.elapsed_ms,
                    r.ttfb_ms,
                    count(r.size, "byte")
                ));
                for h in &r.headers {
                    out.push_str(&format!("{}: {}\n", h.name, h.value));
                }
                out.push('\n');
            }
            // A failed `-o` write leaves the body here (`record`); a filter,
            // failing or not, never does (#223).
            //
            // The body is text (decoded lossily, masked), not bytes: masking
            // needs text, and the reader is mostly an agent's pipe, not a
            // terminal. A binary body is what `-o` is for, as the usage and
            // the skill say; there is no curl-like refusal to print one.
            if let Some(body) = &r.body {
                out.push_str(body);
                // A body without a last newline would run into the next
                // line printed (a prompt, the stderr notes); `-o` keeps the
                // bytes exact.
                if !body.is_empty() && !body.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
        if let Some(j) = &self.jq {
            for value in j.output.iter().flatten() {
                out.push_str(&format!("{value}\n"));
            }
            if let Some(failure) = j.failure() {
                err.push_str(&format!("postui: jq: {failure}\n"));
            }
        }
        if let Some(error) = &self.error {
            err.push_str(&format!("postui: {}\n", error.trim_end()));
        }
        if let Some(r) = self.response.as_ref().filter(|r| r.status >= FIRST_FAILING_STATUS) {
            err.push_str(&format!("postui: {}\n", status_line(r.status)));
        }
        for w in &self.warnings {
            err.push_str(&format!("postui: warning: {}\n", w.trim_end()));
        }
        (out, err)
    }

    /// The request line, what identifies the run, the request's headers and
    /// body, the response with its body exactly as it came (or the jq
    /// results), then the error and warnings.
    pub fn text(&self) -> String {
        let mut out = match (&self.method, &self.url) {
            (Some(method), Some(url)) => format!("{} {url}\n", method.as_str()),
            _ => format!("{}\n", self.request),
        };
        let mut facts = vec![
            ("request", self.request.clone()),
            ("root", self.root.display().to_string()),
            ("file", rel(&self.root, &self.file)),
            ("environment", self.environment.clone().unwrap_or_else(|| "(none)".to_string())),
        ];
        if self.dry_run {
            facts.push(("dry run", "nothing was sent".to_string()));
        }
        out.push_str(&fields(&facts));
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
            if let Some(failure) = j.failure() {
                out.push_str(&table(&[vec![format!("error: {failure}")]]));
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
