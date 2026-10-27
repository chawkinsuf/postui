//! The binary's command line, run for real: where usage goes and the exit
//! status a script sees (#144).

use std::process::{Command, Stdio};

/// Runs the binary in a throwaway home with stdin closed, so a case that
/// regressed into starting the app fails here instead of touching the
/// real config or waiting on a terminal.
fn postui(args: &[&str]) -> std::process::Output {
    let home = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_postui"))
        .args(args)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs")
}

#[test]
fn help_prints_usage_to_stdout_and_succeeds() {
    for args in [&["--help"][..], &["-h"], &["my-api", "-h"], &["--debug-ai", "--help"]] {
        let out = postui(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.starts_with("usage: postui"), "{args:?}: {stdout}");
        assert!(stdout.contains("--help"), "the usage lists --help: {stdout}");
        assert!(out.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn a_bad_argument_prints_usage_to_stderr_and_fails() {
    for args in [
        &["-x"][..],
        &["-v"],
        &["--version", "/x"],
        &["a", "b"],
        &["--keydump=abc"],
        &["--debug-ai", "--debug-ai"],
    ] {
        let out = postui(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with("usage: postui"), "{args:?}: {stderr}");
    }
}

#[test]
fn version_prints_to_stdout_and_succeeds() {
    let out = postui(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, format!("postui {}\n", env!("CARGO_PKG_VERSION")));
}
