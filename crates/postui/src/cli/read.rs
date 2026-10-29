//! The read commands' reports: what each one finds, as the `Serialize`
//! shape its JSON takes, and its `--text` rendering. Every project is
//! opened read-only ([`open`]), so nothing here can write (R16).

use super::out::table;
use super::{CliError, Global, target};
use postui_core::project::{Project, Warning};
use serde::Serialize;
use std::path::PathBuf;

/// Opens `root` read-only. A refused open is the error, naming the file
/// that refused it.
fn open(root: PathBuf) -> Result<(Project, Vec<Warning>), CliError> {
    Project::open_read_only(root).map_err(|e| {
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

/// `postui show`: for now the project's name and root (#214's next step
/// adds its files, environments and spaces).
#[derive(Serialize)]
pub struct ShowReport {
    name: String,
    root: PathBuf,
}

pub fn show(g: &Global) -> Result<ShowReport, CliError> {
    let (project, _warnings) = open(target::resolve(g.project.as_deref())?)?;
    Ok(ShowReport { name: project.display_name(), root: project.root().to_path_buf() })
}

impl ShowReport {
    pub fn text(&self) -> String {
        table(&[
            vec!["name".to_string(), self.name.clone()],
            vec!["root".to_string(), self.root.display().to_string()],
        ])
    }
}
