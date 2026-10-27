//! The binary's command line, run for real: where usage goes and the exit
//! status a script sees (#144).

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// Runs the binary in a throwaway home with stdin closed, so a case that
/// regressed into starting the app can't touch the real config. (It could
/// still open `/dev/tty` from an interactive shell; in CI there is none.)
fn postui<S: AsRef<OsStr>>(args: &[S]) -> std::process::Output {
    postui_to(args, Stdio::piped())
}

/// [`postui`] with its stdout sent to `stdout`.
fn postui_to<S: AsRef<OsStr>>(args: &[S], stdout: Stdio) -> std::process::Output {
    let home = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_postui"))
        .args(args)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .stdin(Stdio::null())
        .stdout(stdout)
        .output()
        .expect("the binary runs")
}

/// The flags that only print, each run on its own.
const PRINTERS: [&str; 3] = ["--help", "--version", "--setup"];

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
        &["-x", "--help"],
    ] {
        let out = postui(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with("postui: "), "the reason first: {stderr}");
        assert!(stderr.contains("\n\nusage: postui"), "{args:?}: {stderr}");
    }
}

#[test]
fn version_prints_to_stdout_and_succeeds() {
    let out = postui(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, format!("postui {}\n", env!("CARGO_PKG_VERSION")));
}

/// #161: an argument that isn't UTF-8 is read, not a panic. (A lone one is
/// the directory and would start the app, so the parse's unit test covers
/// that; here it sits on lines that are mistakes anyway.)
#[cfg(unix)]
#[test]
fn a_non_utf8_argument_is_read_not_a_panic() {
    use std::os::unix::ffi::OsStrExt;
    let raw = OsStr::from_bytes(b"\xff");
    let dashed = OsStr::from_bytes(b"-\xff");
    for (args, reason) in [
        (&[raw, OsStr::new("b")][..], "postui: one directory at most; 'b' is a second\n"),
        (&[dashed], "postui: unknown option '-\u{FFFD}'\n"),
    ] {
        let out = postui(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.starts_with(reason), "{args:?}: {stderr}");
    }
    let out = postui(&[raw, OsStr::new("--help")]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("usage: postui"));
}

/// #162: a reader that stopped (`postui --help | head`) is not an error:
/// the printers end quietly and succeed.
#[test]
fn printing_into_a_closed_pipe_succeeds_quietly() {
    for flag in PRINTERS {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let out = postui_to(&[flag], writer.into());
        assert_eq!(out.status.code(), Some(0), "{flag}");
        assert!(out.stderr.is_empty(), "{flag}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// #162: any other write error is said on stderr and fails, not a panic.
#[cfg(target_os = "linux")]
#[test]
fn printing_to_a_full_device_fails_with_a_message() {
    for flag in PRINTERS {
        let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
        let out = postui_to(&[flag], full.into());
        assert_eq!(out.status.code(), Some(1), "{flag}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr, "postui: can't write to stdout: No space left on device (os error 28)\n",
            "{flag}"
        );
    }
}
