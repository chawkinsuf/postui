//! The "describe a filter" shell-out: a configurable command (default
//! `claude -p`) that reads the whole prompt on stdin and prints a reply.

use std::process::Stdio;
use tokio::io::AsyncWriteExt as _;

/// Whether `cmd`'s first word resolves on `PATH` (or is an existing path).
pub fn program_available(cmd: &str) -> bool {
    let Some(program) = cmd.split_whitespace().next() else {
        return false;
    };
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// The first word of `cmd` — the program name, for toasts and menu hints.
pub fn program_name(cmd: &str) -> &str {
    cmd.split_whitespace().next().unwrap_or("")
}

/// Runs `sh -c cmd` with `stdin` piped in; `Ok(stdout)` on exit 0, `Err(last
/// stderr line or status)` otherwise.
pub async fn run_command(cmd: String, stdin: String) -> Result<String, String> {
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut pipe) = child.stdin.take() {
        // A child that exits early closes the pipe; that is reported by
        // wait_with_output below, not here.
        let _ = pipe.write_all(stdin.as_bytes()).await;
        drop(pipe);
    }
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::trim);
    Err(last
        .map(str::to_string)
        .unwrap_or_else(|| out.status.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_available_looks_at_the_first_word() {
        assert!(program_available("sh -c 'echo hi'"));
        assert!(program_available("/bin/sh"));
        assert!(!program_available("definitely-not-a-program-42 --flag"));
        assert!(!program_available(""));
    }

    #[tokio::test]
    async fn run_command_pipes_stdin_and_returns_stdout() {
        let out = run_command("cat".into(), "hello".into()).await.unwrap();
        assert_eq!(out, "hello");
    }

    #[tokio::test]
    async fn run_command_reports_the_last_stderr_line_on_failure() {
        let err = run_command("echo one >&2; echo two >&2; exit 3".into(), String::new())
            .await
            .unwrap_err();
        assert_eq!(err, "two");
        let err = run_command("exit 4".into(), String::new())
            .await
            .unwrap_err();
        assert!(err.contains("4"), "{err}");
    }
}
