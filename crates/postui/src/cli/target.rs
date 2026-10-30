//! Which project a command means (`-p`, spec 2026-10-28 §4): a directory,
//! else a registered project's name, else the current directory; and it
//! must hold a `project.toml`.

use super::CliError;
use postui_core::project::Project;
use std::path::{Path, PathBuf};

/// The project root `-p` names, as its canonical path. A value that is a
/// directory is that directory; any other value is matched against the
/// registered projects' names, exactly first, then ignoring ASCII case. No
/// value is the current directory. Whichever it is must be a project.
///
/// The root is canonical (`Path::canonicalize`: absolute, no `.` or `..`,
/// no trailing slash, symlinks followed), so every spelling of one
/// directory (`~/x`, `x/`, `../..` from inside it, its registered name, or
/// running from it) is one root, and every path built from it is clean.
pub fn resolve(project: Option<&str>) -> Result<PathBuf, CliError> {
    resolve_where(project, Project::is_project)
}

/// [`resolve`] for `edit end` and `edit status`: a directory with an edit
/// flag up is the one the flag is in, though the agent's edit removed its
/// `project.toml`.
pub fn resolve_flagged(project: Option<&str>) -> Result<PathBuf, CliError> {
    resolve_where(project, |root| Project::is_project(root) || Project::edit_flag_up(root))
}

/// The root `project` names, refused unless `takes` it.
fn resolve_where(project: Option<&str>, takes: fn(&Path) -> bool) -> Result<PathBuf, CliError> {
    let root = match project {
        None => std::env::current_dir()
            .and_then(|cwd| cwd.canonicalize())
            .map_err(|e| CliError::new(format!("can't read the current directory: {e}")))?,
        Some(s) => {
            let path = crate::config::expand_tilde(s);
            if path.is_dir() {
                path.canonicalize()
                    .map_err(|e| CliError::new(format!("can't resolve \"{s}\": {e}")))?
            } else {
                // A registered root that is gone stays as written, so the
                // error below names where it was.
                let root = by_name(s)?;
                root.canonicalize().unwrap_or(root)
            }
        }
    };
    if !takes(&root) {
        return Err(CliError::at(
            "not a postui project (no project.toml); `postui init` makes one, or pass -p <dir>",
            root.join("project.toml"),
        ));
    }
    Ok(root)
}

/// The one registered project named `name`.
fn by_name(name: &str) -> Result<PathBuf, CliError> {
    let named: Vec<(PathBuf, String)> = registered()?
        .into_iter()
        .map(|root| {
            let shown = Project::peek_display_name(&root);
            (root, shown)
        })
        .collect();
    let pick = |same: &dyn Fn(&str) -> bool| -> Vec<&Path> {
        named.iter().filter(|(_, shown)| same(shown)).map(|(root, _)| root.as_path()).collect()
    };
    let mut found = pick(&|shown| shown == name);
    if found.is_empty() {
        found = pick(&|shown| shown.eq_ignore_ascii_case(name));
    }
    match found.as_slice() {
        [] => Err(CliError::new(format!("no such directory or registered project \"{name}\""))),
        [root] => Ok(root.to_path_buf()),
        roots => {
            let listed: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
            Err(CliError::new(format!(
                "\"{name}\" names {} registered projects: {}",
                roots.len(),
                listed.join(", ")
            )))
        }
    }
}

/// The registered projects' roots, from `config.toml`. One that will not
/// parse is an error naming it, not an empty list: the registry is in it.
pub(super) fn registered() -> Result<Vec<PathBuf>, CliError> {
    let (_, loaded, _) = crate::config::Config::load(cfg!(target_os = "macos"));
    match loaded.config_error {
        None => Ok(loaded.registry.known),
        Some(error) => Err(match postui_core::config_dir() {
            Some(dir) => CliError::at(error, dir.join(crate::config::CONFIG_TOML)),
            None => CliError::new(error),
        }),
    }
}
