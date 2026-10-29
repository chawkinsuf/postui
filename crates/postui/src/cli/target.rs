//! Which project a command means (`-p`, spec 2026-10-28 §4): a directory,
//! else a registered project's name, else the current directory; and it
//! must hold a `project.toml`.

use super::CliError;
use postui_core::project::Project;
use std::path::{Path, PathBuf};

/// The project root `-p` names, absolute. A value that is a directory is
/// that directory (`~/x`, a relative path and a trailing slash all come to
/// the same root); any other value is matched against the registered
/// projects' names, exactly first, then ignoring ASCII case. No value is
/// the current directory. Whichever it is must be a project.
pub fn resolve(project: Option<&str>) -> Result<PathBuf, CliError> {
    let root = match project {
        None => std::env::current_dir()
            .map_err(|e| CliError::new(format!("can't read the current directory: {e}")))?,
        Some(s) => {
            let path = crate::config::expand_tilde(s);
            if path.is_dir() {
                let absolute = std::path::absolute(&path)
                    .map_err(|e| CliError::new(format!("can't resolve \"{s}\": {e}")))?;
                // `absolute` keeps a trailing slash and inner `.`s; the
                // components drop them.
                absolute.components().collect()
            } else {
                by_name(s)?
            }
        }
    };
    if !Project::is_project(&root) {
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
