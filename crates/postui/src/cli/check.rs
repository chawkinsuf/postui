//! `postui check`: every project file parsed and validated, then every
//! request's variables resolved against one environment (spec 2026-10-28
//! §4). It is what an agent runs after editing the TOML by hand, so each
//! finding names its file. Errors are what is wrong in the files: one that
//! won't parse or validate, an invalid name, a `{{name}}` nothing declares.
//! Everything else is a warning, a missing selection or secret among them
//! (both live in `.postui/`, which a fresh clone doesn't have).

use super::mask::MASK_MIN_CHARS;
use super::read::{displayed, open, pick_env};
use super::{CliError, Global, target};
use postui_core::prepare::{self, PrepareError, UnresolvedCause};
use postui_core::project::{self, Project, Skipped};
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
/// that refused it; otherwise every name postui passes over (in
/// `project.toml`, or a file or directory's), every environment, every
/// request file, and every request's variables against
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

    // Each name postui passes over is an error on where it is written; the
    // open's warning for it, and the walk's for the files in a skipped
    // directory, would only say it again. A hidden directory's files are
    // passed over without a word, as the directory is.
    let skipped = project.skipped();
    for s in &skipped {
        errors.push(Finding::at(root.join(s.rel()), skipped_message(s)));
    }
    let said = |w: &String| skipped.iter().any(|s| s.warning().as_ref() == Some(w));
    warnings.extend(opened_warnings.into_iter().filter(|w| !said(w)).map(Finding::general));
    let passed_over = |dir: &str| {
        project::is_hidden(dir) || skipped.contains(&Skipped::SpaceDir(dir.to_string()))
    };
    for w in project.listing_warnings() {
        if !w.space_dir().is_some_and(passed_over) {
            let file = w.file.as_ref().map(|f| root.join(f));
            warnings.push(Finding { file, message: w.message.clone() });
        }
    }

    for slug in project.environments().to_vec() {
        if let Err(e) = project.load_environment(&slug) {
            errors.push(Finding::at(env_file(&root, &slug), reason(e)));
        }
    }

    // A secret too short to mask would show in run and suite output, in
    // whichever environment they run: every environment's is checked, even
    // when the environment asked for fails to load. Only names
    // `variables.toml` declares secret count: an entry it doesn't is never
    // used.
    let model = project.variables();
    for (env_slug, secrets) in project.secrets() {
        for (name, value) in secrets {
            let declared_secret = model.vars.get(name).is_some_and(|v| v.secret);
            // An empty value can't leak, so it isn't warned of.
            if declared_secret && !value.is_empty() && value.chars().count() < MASK_MIN_CHARS {
                warnings.push(Finding::at(
                    root.join(project::SECRETS_TOML),
                    format!(
                        "secret {name} of environment {env_slug} is shorter than \
                         {MASK_MIN_CHARS} characters, so run and suite output can't mask it"
                    ),
                ));
            }
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
        // A name only a disabled request variable gives is switched off,
        // not misspelled.
        let (off, undeclared): (BTreeMap<_, _>, BTreeMap<_, _>) = undeclared
            .into_iter()
            .partition(|(name, _)| req.variables.get(name).is_some_and(|e| !e.enabled));
        if !off.is_empty() {
            let names: Vec<String> = off.keys().map(|n| format!("{{{{{n}}}}}")).collect();
            let verb = if names.len() == 1 { "is" } else { "are" };
            errors.push(Finding::at(
                file.clone(),
                format!("{} {verb} disabled in the request's [variables]", names.join(", ")),
            ));
        }
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

/// What is wrong with a name postui passes over.
fn skipped_message(s: &Skipped) -> String {
    match s {
        Skipped::ListedSpace(name) => format!("{name:?} is not a valid space name (a-z 0-9 - _)"),
        Skipped::ListedEnvironment(name) => {
            format!("{name:?} is not a valid environment name (a-z 0-9 - _)")
        }
        Skipped::EnvironmentFile(_) => {
            "not a valid environment name (a-z 0-9 - _); postui ignores this file".to_string()
        }
        Skipped::SpaceDir(_) => {
            "not a valid space name (a-z 0-9 - _); postui ignores this directory".to_string()
        }
    }
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
pub(super) fn reason(e: project::Error) -> String {
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
