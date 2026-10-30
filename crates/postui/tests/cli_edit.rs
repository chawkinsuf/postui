//! `postui edit begin|end|status`, run for real in a throwaway home: the
//! flag, its refusals, and a TUI's lock on the project it has open, which
//! this test process holds the way a TUI would (spec 2026-10-28 §4, piece
//! 3).

// Nothing here sends, so the shared test server goes unused.
#[allow(dead_code)]
mod common;

use common::*;
use postui_core::project::{EditFlag, Project, ProjectLock};
use std::path::Path;
use std::process::Output;

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn flag(root: &Path) -> Option<EditFlag> {
    Project::read_edit_flag(root).unwrap()
}

/// Runs `edit <args>` on `root`.
fn edit(home: &Home, root: &Path, args: &[&str]) -> Output {
    let mut line = vec!["edit"];
    line.extend_from_slice(args);
    line.extend_from_slice(&["-p", s(root)]);
    home.run(&line)
}

#[test]
fn begin_raises_the_flag_with_its_note() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let out = edit(&home, &root, &["begin", "--note", "add orders"]);
    assert_ok(&out, "begin");
    let up = flag(&root).expect("the flag is up");
    assert_eq!(up.note.as_deref(), Some("add orders"));
    assert!(up.age(std::time::SystemTime::now()).is_some(), "{}", up.started);
    assert_eq!(
        stdout(&out),
        format!("root  {}\nflag  .postui/editing.toml\nnote  add orders\n", root.display())
    );

    let json = json(&edit(&home, &root, &["status", "--json"]));
    assert_eq!(json["flag"]["note"], "add orders");
    assert_eq!(json["flag"]["started"], up.started.as_str());
    assert_eq!(json["tui_open"], false);
}

/// One edit at a time: a second `edit begin` refuses, and points at `edit
/// end --force` for a flag the user confirms is stuck.
#[test]
fn begin_refuses_a_flag_already_up() {
    let home = Home::new();
    let root = write_project(&home, "api");
    assert_ok(&edit(&home, &root, &["begin", "--note", "first"]), "first begin");
    let first = flag(&root).unwrap();

    let out = edit(&home, &root, &["begin"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.starts_with(&format!("postui: {}: ", root.join(".postui/editing.toml").display())));
    assert!(err.contains("already in progress (started just now: \"first\")"), "{err}");
    assert!(err.contains("postui edit end --force"), "{err}");
    assert_eq!(flag(&root), Some(first), "the refusal leaves the flag alone");

    // --json: the envelope on stdout, still 1.
    let out = edit(&home, &root, &["begin", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(json(&out)["error"].as_str().unwrap().contains("already in progress"));
}

/// A TUI with the project open (holding its lock, as this test process
/// does here) makes `edit begin` refuse and leave no flag. Once the TUI
/// lets go, it goes ahead.
#[test]
fn begin_refuses_while_a_tui_has_the_project_open() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let tui = ProjectLock::take(&root).unwrap().expect("free");
    assert_eq!(json(&edit(&home, &root, &["status", "--json"]))["tui_open"], true);

    let out = edit(&home, &root, &["begin"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("postui has this project open"), "{}", stderr(&out));
    assert_eq!(flag(&root), None, "no flag left behind");

    drop(tui);
    assert_eq!(json(&edit(&home, &root, &["status", "--json"]))["tui_open"], false);
    assert_ok(&edit(&home, &root, &["begin"]), "begin once postui let go");
}

#[test]
fn end_checks_first_and_keeps_the_flag_on_errors() {
    let home = Home::new();
    let root = write_project(&home, "api");
    assert_ok(&edit(&home, &root, &["begin"]), "begin");
    std::fs::write(root.join("requests/main/list.toml"), "url = [").unwrap();

    let out = edit(&home, &root, &["end"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    assert!(text.contains("requests/main/list.toml: "), "{text}");
    assert!(
        text.ends_with("flag  kept: fix the errors and run postui edit end again, or --force\n"),
        "{text}"
    );
    assert!(flag(&root).is_some());

    let report = json(&edit(&home, &root, &["end", "--json"]));
    assert_eq!(report["flag_removed"], false);
    assert_eq!(report["check"]["ok"], false);

    // Forced, the flag comes down, and the errors still say 1.
    let out = edit(&home, &root, &["end", "--force"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(stdout(&out).ends_with("flag  removed\n"), "{}", stdout(&out));
    assert_eq!(flag(&root), None);
}

#[test]
fn end_with_no_flag_says_it_was_overridden() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let out = edit(&home, &root, &["end"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no edit is in progress: the user overrode it"),
        "{}",
        stderr(&out)
    );

    assert_ok(&edit(&home, &root, &["begin"]), "begin");
    let out = edit(&home, &root, &["end"]);
    assert_ok(&out, "end");
    assert!(stdout(&out).ends_with("flag  removed\n"), "{}", stdout(&out));
}

/// A flag file that won't parse is up: `edit end` and `edit status` name
/// its error, and `edit end --force` takes it down.
#[test]
fn end_force_takes_down_a_broken_flag() {
    let home = Home::new();
    let root = write_project(&home, "api");
    std::fs::write(root.join(".postui/editing.toml"), "started = [").unwrap();
    for cmd in ["end", "status"] {
        let out = edit(&home, &root, &[cmd]);
        assert_eq!(out.status.code(), Some(2), "{cmd}");
        assert!(stderr(&out).contains(".postui/editing.toml"), "{}", stderr(&out));
    }
    assert!(Project::edit_flag_up(&root));
    assert_ok(&edit(&home, &root, &["end", "--force"]), "forced end");
    assert!(!Project::edit_flag_up(&root));
}

/// `status` is a read command: it writes nothing, not even a lock file.
#[test]
fn status_writes_nothing() {
    let home = Home::new();
    let root = write_project(&home, "api");
    let before = tree(&root);
    let out = edit(&home, &root, &["status"]);
    assert_ok(&out, "status");
    assert_eq!(
        stdout(&out),
        format!("root    {}\nflag    down\npostui  not open\n", root.display())
    );
    assert_eq!(tree(&root), before);

    assert_ok(&edit(&home, &root, &["begin", "--note", "x"]), "begin");
    let before = tree(&root);
    let text = stdout(&edit(&home, &root, &["status"]));
    assert!(text.contains("flag    up, started just now\nnote    x\n"), "{text}");
    assert_eq!(tree(&root), before);
}

#[test]
fn edit_needs_a_project() {
    let home = Home::new();
    std::fs::create_dir_all(home.path().join("bare")).unwrap();
    let out = edit(&home, &home.path().join("bare"), &["begin"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!home.path().join("bare/.postui").exists());
}
