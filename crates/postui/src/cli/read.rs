//! The read commands' reports: what each one finds, as the `Serialize`
//! shape its JSON takes, and its `--text` rendering. Every project is
//! opened read-only ([`open`]), so nothing here can write (R16).

use super::out::table;
use super::{CliError, Global, target};
use postui_core::model::Method;
use postui_core::project::{Project, Warning};
use postui_core::{order, storage};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Opens `root` read-only. A refused open is the error, naming the file
/// that refused it.
pub(super) fn open(root: &Path) -> Result<(Project, Vec<Warning>), CliError> {
    Project::open_read_only(root.to_path_buf()).map_err(|e| {
        let file = (!e.file.is_empty()).then(|| e.root.join(&e.file));
        CliError { message: e.error, file }
    })
}

/// `postui projects`.
#[derive(Serialize)]
pub struct ProjectsReport {
    config_dir: Option<PathBuf>,
    projects: Vec<KnownProject>,
}

#[derive(Serialize)]
struct KnownProject {
    name: String,
    root: PathBuf,
    exists: bool,
}

/// The registered projects, in the registry's order.
pub fn projects() -> Result<ProjectsReport, CliError> {
    let projects = target::registered()?
        .into_iter()
        .map(|root| KnownProject {
            name: Project::peek_display_name(&root),
            exists: root.is_dir(),
            root,
        })
        .collect();
    Ok(ProjectsReport { config_dir: postui_core::config_dir(), projects })
}

impl ProjectsReport {
    pub fn text(&self) -> String {
        let config_dir = match &self.config_dir {
            Some(dir) => dir.display().to_string(),
            None => "(none)".to_string(),
        };
        let rows: Vec<Vec<String>> = self
            .projects
            .iter()
            .map(|p| {
                let missing = if p.exists { "" } else { "(missing)" };
                vec![p.name.clone(), p.root.display().to_string(), missing.to_string()]
            })
            .collect();
        let listed = if rows.is_empty() { "  (none)\n".to_string() } else { table(&rows) };
        format!("config dir  {config_dir}\n\nprojects\n{listed}")
    }
}

/// `postui show`.
#[derive(Serialize)]
pub struct ShowReport {
    name: String,
    root: PathBuf,
    files: ShowFiles,
    environments: Vec<ShownEnv>,
    spaces: Vec<ShownSpace>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ShowFiles {
    project: PathBuf,
    variables: PathBuf,
}

#[derive(Serialize)]
struct ShownEnv {
    slug: String,
    name: String,
    file: PathBuf,
    /// The environment the TUI would open on.
    active: bool,
}

#[derive(Serialize)]
struct ShownSpace {
    slug: String,
    name: String,
    dir: PathBuf,
    requests: usize,
}

/// The project's name, files, environments (a broken inactive one is
/// listed all the same) and spaces, and the warnings opening it gave.
pub fn show(g: &Global) -> Result<ShowReport, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning().map(str::to_string));
    let environments = project
        .environments()
        .iter()
        .map(|slug| ShownEnv {
            slug: slug.clone(),
            name: project.env_name(slug),
            file: root.join("environments").join(format!("{slug}.toml")),
            active: project.active_env() == Some(slug.as_str()),
        })
        .collect();
    let spaces = project
        .spaces()
        .iter()
        .map(|slug| ShownSpace {
            slug: slug.clone(),
            name: project.space_name(slug),
            dir: root.join("requests").join(slug),
            requests: displayed(&project, slug).len(),
        })
        .collect();
    Ok(ShowReport {
        name: project.display_name(),
        files: ShowFiles {
            project: root.join("project.toml"),
            variables: root.join("variables.toml"),
        },
        root,
        environments,
        spaces,
        warnings,
    })
}

impl ShowReport {
    pub fn text(&self) -> String {
        let mut out = table(&[
            vec!["name".to_string(), self.name.clone()],
            vec!["root".to_string(), self.root.display().to_string()],
            vec!["project".to_string(), self.files.project.display().to_string()],
            vec!["variables".to_string(), self.files.variables.display().to_string()],
        ]);
        let envs: Vec<Vec<String>> = self
            .environments
            .iter()
            .map(|e| {
                let active = if e.active { "(active)" } else { "" };
                vec![
                    e.slug.clone(),
                    e.name.clone(),
                    active.to_string(),
                    e.file.display().to_string(),
                ]
            })
            .collect();
        out.push_str("\nenvironments\n");
        out.push_str(&if envs.is_empty() { "  (none)\n".to_string() } else { table(&envs) });
        let spaces: Vec<Vec<String>> = self
            .spaces
            .iter()
            .map(|s| {
                vec![
                    s.slug.clone(),
                    s.name.clone(),
                    format!("{} requests", s.requests),
                    s.dir.display().to_string(),
                ]
            })
            .collect();
        out.push_str("\nspaces\n");
        out.push_str(&if spaces.is_empty() { "  (none)\n".to_string() } else { table(&spaces) });
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}

/// A `warnings` section for `--text`; nothing when there are none.
fn warnings_text(warnings: &[String]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    let rows: Vec<Vec<String>> = warnings.iter().map(|w| vec![w.clone()]).collect();
    format!("\nwarnings\n{}", table(&rows))
}

/// `space`'s requests as full slugs, in the order the sidebar draws them.
fn displayed(project: &Project, space: &str) -> Vec<String> {
    order::displayed_slugs(project.requests(), order::space_order(project.meta(), space), space)
}

/// `postui requests`.
#[derive(Serialize)]
pub struct RequestsReport {
    requests: Vec<ListedRequest>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ListedRequest {
    slug: String,
    space: String,
    name: Option<String>,
    method: Option<Method>,
    url: Option<String>,
    file: PathBuf,
    error: Option<String>,
}

/// The requests of `space` (every space when `None`), spaces in project
/// order, each in sidebar order. A file that doesn't parse is listed with
/// its error.
pub fn requests(g: &Global, space: Option<&str>) -> Result<RequestsReport, CliError> {
    let root = target::resolve(g.project.as_deref())?;
    let (mut project, mut warnings) = open(&root)?;
    warnings.extend(project.listing_warning().map(str::to_string));
    let spaces: Vec<String> = match space {
        None => project.spaces().to_vec(),
        Some(wanted) if project.spaces().iter().any(|s| s == wanted) => vec![wanted.to_string()],
        Some(wanted) => {
            return Err(CliError::new(format!("no such space \"{wanted}\" in this project")));
        }
    };
    let mut requests = Vec::new();
    for space in spaces {
        for slug in displayed(&project, &space) {
            let (name, broken) = project
                .requests()
                .iter()
                .find(|l| l.slug == slug)
                .map(|l| (l.display_name().to_string(), l.broken.clone()))
                .unwrap_or_default();
            let parsed = match broken {
                Some(error) => Err(error),
                None => project.read_request(&slug).map_err(|e| e.to_string()),
            };
            let file = storage::request_path(&root, &slug);
            requests.push(match parsed {
                Ok(req) => ListedRequest {
                    slug,
                    space: space.clone(),
                    name: Some(name),
                    method: Some(req.method),
                    url: Some(req.url),
                    file,
                    error: None,
                },
                Err(error) => ListedRequest {
                    slug,
                    space: space.clone(),
                    name: None,
                    method: None,
                    url: None,
                    file,
                    error: Some(error),
                },
            });
        }
    }
    Ok(RequestsReport { requests, warnings })
}

impl RequestsReport {
    pub fn text(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .requests
            .iter()
            .map(|r| match &r.error {
                Some(error) => vec![r.slug.clone(), "(broken)".to_string(), error.clone()],
                None => vec![
                    r.slug.clone(),
                    r.method.map_or("", Method::as_str).to_string(),
                    r.url.clone().unwrap_or_default(),
                    r.name.clone().unwrap_or_default(),
                ],
            })
            .collect();
        let listed = if rows.is_empty() { "  (none)\n".to_string() } else { table(&rows) };
        format!("{listed}{}", warnings_text(&self.warnings))
    }
}
