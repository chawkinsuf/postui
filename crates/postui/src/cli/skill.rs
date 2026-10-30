//! `postui skill install|show`: the Claude Code skill that teaches an agent
//! the command line and the project files (spec 2026-10-28 §4, piece 3,
//! R12). Its source is `skills/postui/` at the repo root, for a manual
//! install; the binary carries a copy of it.

use super::CliError;
use super::out::fields;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The skill's files, as `skills/postui/` holds them. Read from outside the
/// crate's own folder, which `cargo package` would refuse: postui is built
/// from the repository (cargo-dist, `cargo install --git`) and never
/// published as a crate (docs/releasing.md), so the one copy stays where a
/// manual install finds it.
pub const FILES: [(&str, &str); 2] = [
    ("SKILL.md", include_str!("../../../../skills/postui/SKILL.md")),
    ("reference.md", include_str!("../../../../skills/postui/reference.md")),
];

/// Where the skill goes: under the user's home, or the current directory.
const SKILL_DIR: &str = ".claude/skills/postui";

/// `postui skill install`.
#[derive(Serialize)]
pub struct InstallReport {
    dir: PathBuf,
    files: Vec<Installed>,
}

#[derive(Serialize)]
struct Installed {
    file: &'static str,
    /// `installed` (new), `same` (identical, left alone), `replaced`
    /// (`--force`), or `differs` (left alone).
    status: &'static str,
}

/// Installs the skill into `~/.claude/skills/postui/`, or with
/// `into_project` into `./.claude/skills/postui/`. A file that is already
/// there and differs is left alone unless `force`, so a skill the user
/// edited is never lost unasked.
pub fn install(into_project: bool, force: bool) -> Result<InstallReport, CliError> {
    let base = if into_project {
        std::env::current_dir()
            .map_err(|e| CliError::new(format!("can't read the current directory: {e}")))?
    } else {
        directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .ok_or_else(|| CliError::new("can't find your home directory; try --project"))?
    };
    let dir = base.join(SKILL_DIR);
    let mut files = Vec::new();
    for (file, text) in FILES {
        let path = dir.join(file);
        let status = match read(&path)? {
            None => "installed",
            Some(there) if there == text.as_bytes() => "same",
            Some(_) if force => "replaced",
            Some(_) => "differs",
        };
        if matches!(status, "installed" | "replaced")
            && let Err(e) = crate::hostfs::write_user_file(&path, text.as_bytes())
        {
            // The files written before it are said, so a half install
            // isn't taken for none.
            let done: Vec<&str> = files
                .iter()
                .filter(|f: &&Installed| matches!(f.status, "installed" | "replaced"))
                .map(|f| f.file)
                .collect();
            let written = match done.as_slice() {
                [] => String::new(),
                done => format!(" ({} written)", done.join(", ")),
            };
            return Err(CliError::at(format!("can't write it: {e}{written}"), path));
        }
        files.push(Installed { file, status });
    }
    Ok(InstallReport { dir, files })
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, CliError> {
    crate::hostfs::read_user_file(path)
        .map_err(|e| CliError::at(format!("can't read it: {e}"), path.to_path_buf()))
}

impl InstallReport {
    fn left_differing(&self) -> bool {
        self.files.iter().any(|f| f.status == "differs")
    }

    /// 1 when a file was left differing, else 0.
    pub fn status(&self) -> ExitCode {
        if self.left_differing() { ExitCode::from(1) } else { ExitCode::SUCCESS }
    }

    pub fn text(&self) -> String {
        let mut rows = vec![("dir", self.dir.display().to_string())];
        rows.extend(self.files.iter().map(|f| (f.status, f.file.to_string())));
        let mut out = fields(&rows);
        if self.left_differing() {
            out.push_str("\n--force replaces the files that differ\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_lists_each_file_by_what_happened_to_it() {
        let report = InstallReport {
            dir: PathBuf::from("/h/.claude/skills/postui"),
            files: vec![
                Installed { file: "SKILL.md", status: "installed" },
                Installed { file: "reference.md", status: "differs" },
            ],
        };
        assert_eq!(
            report.text(),
            "dir        /h/.claude/skills/postui\ninstalled  SKILL.md\ndiffers    reference.md\n\n\
             --force replaces the files that differ\n"
        );
        assert_eq!(report.status(), ExitCode::from(1));
    }

    #[test]
    fn the_skill_names_itself_postui() {
        assert!(FILES[0].1.starts_with("---\nname: postui\ndescription: "));
    }
}
