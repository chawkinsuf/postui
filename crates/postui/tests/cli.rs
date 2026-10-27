//! The binary's command line, run for real: where usage goes and the exit
//! status a script sees (#144).

use std::process::Command;

fn postui(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_postui")).args(args).output().expect("the binary runs")
}

#[test]
fn help_prints_usage_to_stdout_and_succeeds() {
    for flag in ["--help", "-h"] {
        let out = postui(&[flag]);
        assert_eq!(out.status.code(), Some(0), "{flag}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.starts_with("usage: postui"), "{flag}: {stdout}");
        assert!(stdout.contains("--help"), "the usage lists --help: {stdout}");
        assert!(out.stderr.is_empty(), "{flag}");
    }
}

#[test]
fn a_bad_argument_prints_usage_to_stderr_and_fails() {
    for args in [&["-x"][..], &["-v"], &["--version", "/x"], &["a", "b"]] {
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
