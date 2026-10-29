//! `postui check`: every project file parsed and validated, then every
//! request's variables resolved against one environment (spec 2026-10-28
//! §4). It is what an agent runs after editing the TOML by hand, so each
//! finding names its file. Errors are what is wrong in the files: one that
//! won't parse or validate, an invalid name, a `{{name}}` nothing declares.
//! Everything else is a warning, a missing selection or secret among them
//! (both live in `.postui/`, which a fresh clone doesn't have).

use super::read::{displayed, open, pick_env};
use super::{CliError, Global, target};
use postui_core::prepare::{self, PrepareError, UnresolvedCause};
use postui_core::project::{self, Project};
use postui_core::storage;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The status a report with an error ends the run with.
const FOUND_ERRORS: u8 = 1;

/// `postui check`.
#[derive(Serialize)]
pub struct CheckReport {
    ok: bool,
    /// The environment the requests were resolved against.
    environment: Option<String>,
    errors: Vec<Finding>,
    warnings: Vec<Finding>,
}

/// One thing `check` found, and the file it is in (`None` when it is about
/// no one file).
#[derive(Serialize)]
pub struct Finding {
    file: Option<PathBuf>,
    message: String,
}

impl Finding {
    fn at(file: PathBuf, message: impl Into<String>) -> Self {
        Finding { file: Some(file), message: message.into() }
    }

    fn general(message: String) -> Self {
        Finding { file: None, message }
    }
}

/// Checks the project: a project that won't open is one error on the file
/// that refused it; otherwise the names in `project.toml`, every
/// environment, every request file, and every request's variables against
/// `env` (as `vars` picks it). Only a project that isn't one, or an `env`
/// that doesn't exist, is the error rather than a report.
pub fn check(g: &Global, env: Option<&str>) -> Result<CheckReport, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let (mut project, opened_warnings) = match open(&root) {
        Ok(opened) => opened,
        Err(e) => {
            errors.push(Finding { file: e.file, message: e.message });
            return Ok(CheckReport::new(None, errors, warnings));
        }
    };

    let bad_names = invalid_names(&project);
    for (name, kind) in &bad_names {
        let message = format!("{name:?} is not a valid {kind} name ({kind} names are a-z 0-9 - _)");
        errors.push(Finding::at(root.join("project.toml"), message));
    }
    // The open warned about each bad name; the error above says it.
    let about_bad_name = |w: &String| {
        bad_names.iter().any(|(name, _)| w.starts_with(&format!("project.toml lists {name:?},")))
    };
    warnings.extend(
        opened_warnings
            .into_iter()
            .chain(project.listing_warning().map(str::to_string))
            .filter(|w| !about_bad_name(w))
            .map(Finding::general),
    );

    for slug in project.environments().to_vec() {
        if let Err(e) = project.load_environment(&slug) {
            errors.push(Finding::at(env_file(&root, &slug), reason(e)));
        }
    }

    let mut parsed = Vec::new();
    for space in project.spaces().to_vec() {
        for slug in displayed(&project, &space) {
            let broken =
                project.requests().iter().find(|l| l.slug == slug).and_then(|l| l.broken.clone());
            match broken {
                Some(error) => errors.push(Finding::at(storage::request_path(&root, &slug), error)),
                None => parsed.push(slug),
            }
        }
    }

    let environment = match pick_env(&mut project, env) {
        Ok(slug) => slug,
        // The environment asked for doesn't load; its file's error is
        // already found, and there is nothing to resolve against.
        Err(e) if e.file.is_some() => {
            if !errors.iter().any(|f| f.file == e.file) {
                errors.push(Finding { file: e.file, message: e.message });
            }
            return Ok(CheckReport::new(None, errors, warnings));
        }
        Err(e) => return Err(e),
    };
    let ctx = project.prepare_context();
    for slug in parsed {
        let file = storage::request_path(&root, &slug);
        let req = match project.read_request(&slug) {
            Ok(req) => req,
            Err(e) => {
                errors.push(Finding::at(file, reason(e)));
                continue;
            }
        };
        let Err(PrepareError::Unresolved(causes)) = prepare::prepare(&req, &ctx) else {
            continue;
        };
        let (undeclared, rest): (BTreeMap<_, _>, BTreeMap<_, _>) =
            causes.into_iter().partition(|(name, cause)| {
                *cause == UnresolvedCause::Undefined && !declared(&project, name)
            });
        if !undeclared.is_empty() {
            let names: Vec<String> = undeclared.keys().map(|n| format!("{{{{{n}}}}}")).collect();
            errors.push(Finding::at(file.clone(), format!("{} not declared", names.join(", "))));
        }
        if !rest.is_empty() {
            warnings.push(Finding::at(file, PrepareError::Unresolved(rest).to_string()));
        }
    }
    Ok(CheckReport::new(environment, errors, warnings))
}

/// The names `project.toml` lists that can't be a space or an environment,
/// each once, with which kind it was listed as.
fn invalid_names(project: &Project) -> Vec<(String, &'static str)> {
    let meta = project.meta();
    let spaces = meta.spaces.iter().filter(|n| !project::valid_space_name(n)).map(|n| (n, "space"));
    let envs = meta
        .environments
        .iter()
        .filter(|n| !project::valid_env_name(n))
        .map(|n| (n, "environment"));
    let mut out: Vec<(String, &'static str)> = Vec::new();
    for (name, kind) in spaces.chain(envs) {
        if !out.iter().any(|(n, k)| n == name && *k == kind) {
            out.push((name.clone(), kind));
        }
    }
    out
}

/// Whether `variables.toml` declares `name`, as a variable or a selector's
/// field. An unresolved name it declares lacks a value; one it doesn't is
/// likely a typo.
fn declared(project: &Project, name: &str) -> bool {
    let model = project.variables();
    model.vars.contains_key(name)
        || model.selectors.values().any(|s| s.fields.iter().any(|f| f == name))
}

fn env_file(root: &Path, slug: &str) -> PathBuf {
    root.join("environments").join(format!("{slug}.toml"))
}

/// What is wrong with a file, without the relative path the finding's
/// `file` already gives.
fn reason(e: project::Error) -> String {
    match e {
        project::Error::Parse { error, .. } => error,
        other => other.to_string(),
    }
}

impl CheckReport {
    fn new(environment: Option<String>, errors: Vec<Finding>, warnings: Vec<Finding>) -> Self {
        CheckReport { ok: errors.is_empty(), environment, errors, warnings }
    }

    /// 1 when an error was found, else 0.
    pub fn status(&self) -> ExitCode {
        if self.ok { ExitCode::SUCCESS } else { ExitCode::from(FOUND_ERRORS) }
    }

    /// Each finding as `file: message`, compiler style, then a verdict.
    pub fn text(&self) -> String {
        let env = self.environment.as_deref().unwrap_or("(none)");
        let mut out = format!("environment  {env}\n");
        for (heading, findings) in [("errors", &self.errors), ("warnings", &self.warnings)] {
            if findings.is_empty() {
                continue;
            }
            out.push_str(&format!("\n{heading}\n"));
            for f in findings {
                match &f.file {
                    Some(file) => out.push_str(&format!("  {}: {}\n", file.display(), f.message)),
                    None => out.push_str(&format!("  {}\n", f.message)),
                }
            }
        }
        let verdict = match (self.errors.len(), self.warnings.len()) {
            (0, 0) => "ok".to_string(),
            (0, w) => format!("ok, {}", count(w, "warning")),
            (e, w) => format!("{}, {}", count(e, "error"), count(w, "warning")),
        };
        out.push_str(&format!("\n{verdict}\n"));
        out
    }
}

/// `n` `thing`s, the plural when `n` isn't 1.
fn count(n: usize, thing: &str) -> String {
    if n == 1 { format!("1 {thing}") } else { format!("{n} {thing}s") }
}
