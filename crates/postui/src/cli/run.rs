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
use crate::http::Clients;
use postui_core::model::Method;
use postui_core::prepare::{self, PrepareWarning, PreparedRequest};
use postui_core::project::Project;
use postui_core::{storage, varmodel};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// The status a failed run ends with.
const FAILED: u8 = 1;

/// A response status from here up is a failure (`run` exits 1).
const FIRST_FAILING_STATUS: u64 = 400;

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

/// What `prepare` makes of request `slug` against the session's
/// environment. The outer `Err` is a request that can't be read (not in the
/// project, or its file is broken: exit 2, naming its file); the inner
/// `Err` is `prepare` refusing it (an unresolved variable), which is a
/// result of the run, not a mistake on its line.
pub(super) fn prepare_one(
    s: &mut Session,
    slug: &str,
) -> Result<Result<(PreparedRequest, Vec<PrepareWarning>), String>, CliError> {
    let file = storage::request_path(&s.root, slug);
    let Some(listing) = s.project.requests().iter().find(|l| l.slug == slug) else {
        return Err(CliError::new(format!("no request \"{slug}\"; postui requests lists them")));
    };
    if let Some(error) = &listing.broken {
        return Err(CliError::at(error.clone(), file));
    }
    let req = s.project.read_request(slug).map_err(|e| CliError::at(reason(e), file))?;
    Ok(prepare::prepare(&req, &s.project.prepare_context()).map_err(|e| e.to_string()))
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
    response: Option<serde_json::Value>,
    jq: Option<serde_json::Value>,
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

/// Runs request `slug` of the project `g` names. With `dry_run` it only
/// prepares it.
pub async fn run(
    g: &Global,
    slug: &str,
    opts: &RunOpts,
    dry_run: bool,
) -> Result<RunReport, CliError> {
    let mut s = session(g, opts)?;
    let prepared = prepare_one(&mut s, slug)?;
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
            report.error = Some(s.mask.apply(&message));
            return Ok(report);
        }
    };
    // What the screens show, or what the wire carries with --show-secrets;
    // the mask then hides any secret value still in the text.
    let show = opts.show_secrets;
    report.method = Some(request.method);
    report.url = Some(s.mask.apply(if show { &request.url } else { &request.display_url }));
    report.request_headers = request
        .wire
        .iter()
        .map(|h| ShownHeader {
            name: s.mask.apply(if show { &h.name } else { &h.display_name }),
            value: s.mask.apply(if show { &h.value } else { &h.display }),
        })
        .collect();
    report.request_body = request.body.as_deref().map(|b| s.mask.apply(b));
    report.warnings.extend(warnings.iter().map(|w| s.mask.apply(&w.to_string())));
    if !dry_run {
        // Placeholder until the send lands; it reads the clients that send
        // will use.
        let _clients = &s.clients;
        return Err(CliError::new("sending is not implemented yet; use --dry-run"));
    }
    Ok(report)
}

impl RunReport {
    /// 1 when the run failed: an error, a status of 400 or more, or a jq
    /// error; else 0.
    pub fn status(&self) -> ExitCode {
        let status = self.response.as_ref().and_then(|r| r["status"].as_u64());
        let jq_failed = self.jq.as_ref().is_some_and(|j| !j["error"].is_null());
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
        if let Some(error) = &self.error {
            out.push_str(&format!("\nerror\n  {error}\n"));
        }
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}
