//! `postui skill install|show`, run for real in a throwaway home (spec
//! 2026-10-28 §4, piece 3, R12).

// Nothing here sends, or opens a project.
#[allow(dead_code)]
mod common;

use common::*;
use std::path::{Path, PathBuf};

/// `skills/postui/` in the repo.
fn source(file: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/postui");
    std::fs::read_to_string(dir.join(file)).unwrap()
}

fn installed(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(".claude/skills/postui").join(file)).unwrap()
}

#[test]
fn install_into_the_project_then_same_then_differs_then_force() {
    let home = Home::new();
    let cwd: PathBuf = home.path().join("repo");
    std::fs::create_dir_all(&cwd).unwrap();
    let install = |extra: &[&str]| {
        let mut args = vec!["skill", "install", "--project"];
        args.extend_from_slice(extra);
        home.run_in(&cwd, &args, |_| {})
    };

    let out = install(&[]);
    assert_ok(&out, "first install");
    let dir = cwd.join(".claude/skills/postui");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("dir        {}\ninstalled  SKILL.md\ninstalled  reference.md\n", dir.display())
    );
    assert_eq!(installed(&cwd, "SKILL.md"), source("SKILL.md"));
    assert_eq!(installed(&cwd, "reference.md"), source("reference.md"));
    assert!(!home.path().join(".claude").exists(), "--project stays out of the home");

    let out = install(&[]);
    assert_ok(&out, "second install");
    assert!(String::from_utf8_lossy(&out.stdout).contains("same  SKILL.md\n"));

    std::fs::write(dir.join("SKILL.md"), "my own notes").unwrap();
    let out = install(&[]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("differs  SKILL.md\nsame     reference.md\n"), "{text}");
    assert!(text.ends_with("--force replaces the files that differ\n"), "{text}");
    assert_eq!(installed(&cwd, "SKILL.md"), "my own notes", "left alone");

    let out = install(&["--force", "--json"]);
    assert_ok(&out, "forced install");
    let report = json(&out);
    assert_eq!(report["files"][0]["status"], "replaced");
    assert_eq!(report["files"][1]["status"], "same");
    assert_eq!(installed(&cwd, "SKILL.md"), source("SKILL.md"));
}

#[test]
fn install_goes_into_the_home_by_default() {
    let home = Home::new();
    assert_ok(&home.run(&["skill", "install"]), "install");
    assert_eq!(installed(home.path(), "SKILL.md"), source("SKILL.md"));
    assert_ok(&home.run(&["skill", "install", "--user"]), "install --user");
}

#[test]
fn show_prints_the_skill_as_the_repo_has_it() {
    let home = Home::new();
    let out = home.run(&["skill", "show"]);
    assert_ok(&out, "show");
    assert_eq!(String::from_utf8_lossy(&out.stdout), source("SKILL.md"));
}
