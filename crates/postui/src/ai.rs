//! The "describe a filter" shell-out: a configurable command (default
//! `claude -p`) that reads the whole prompt on stdin and prints a reply.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::SystemTime;
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

/// Everything a finished command left behind.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub success: bool,
    /// `exit status: 0`, `signal: 9 (SIGKILL)`, …
    pub status: String,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// `Ok(stdout)` on exit 0, `Err(last stderr line or status)` otherwise.
    pub fn into_reply(self) -> Result<String, String> {
        if self.success {
            return Ok(self.stdout);
        }
        let last = self
            .stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(str::trim);
        Err(last.map(str::to_string).unwrap_or(self.status))
    }
}

/// Runs `sh -c cmd` with `stdin` piped in. `Err` only when the command
/// could not be started or waited on.
pub async fn run_command(cmd: String, stdin: String) -> Result<CommandOutput, String> {
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
    Ok(CommandOutput {
        success: out.status.success(),
        status: out.status.to_string(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Where `--debug-ai` writes: `$XDG_STATE_HOME/postui/ai.log` on Linux,
/// the platform's local data directory elsewhere.
pub fn debug_log_path() -> Option<PathBuf> {
    let dirs = directories::BaseDirs::new()?;
    let base = dirs.state_dir().unwrap_or(dirs.data_local_dir());
    Some(base.join("postui").join("ai.log"))
}

/// One `--debug-ai` log entry: when, the command, the whole prompt, what
/// came back, and the filter that was taken from it.
pub fn log_entry(
    when: SystemTime,
    cmd: &str,
    prompt: &str,
    outcome: &Result<CommandOutput, String>,
) -> String {
    let mut out = format!("=== {}  {cmd}\n--- prompt\n{prompt}", utc_timestamp(when));
    if !prompt.ends_with('\n') {
        out.push('\n');
    }
    match outcome {
        Ok(output) => {
            for (name, text) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
                if text.is_empty() {
                    out.push_str(&format!("--- {name}: (empty)\n"));
                } else {
                    out.push_str(&format!("--- {name}\n{text}"));
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
            out.push_str(&format!("--- {}\n", output.status));
            if output.success {
                let filter = postui_core::jq::ai::extract_filter(&output.stdout);
                out.push_str(&format!(
                    "--- filter: {}\n",
                    filter.as_deref().unwrap_or("(none)")
                ));
            }
        }
        Err(e) => out.push_str(&format!("--- could not run: {e}\n")),
    }
    out.push('\n');
    out
}

/// `2026-10-13T16:44:58Z`.
fn utc_timestamp(when: SystemTime) -> String {
    let secs = when
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        assert_eq!(out.into_reply().unwrap(), "hello");
    }

    #[tokio::test]
    async fn run_command_reports_the_last_stderr_line_on_failure() {
        let out = run_command("echo one >&2; echo two >&2; exit 3".into(), String::new())
            .await
            .unwrap();
        assert_eq!(out.stderr, "one\ntwo\n", "the whole stderr is kept");
        assert_eq!(out.into_reply().unwrap_err(), "two");
        let err = run_command("exit 4".into(), String::new())
            .await
            .unwrap()
            .into_reply()
            .unwrap_err();
        assert!(err.contains("4"), "{err}");
    }

    #[test]
    fn timestamps_are_utc_iso() {
        assert_eq!(
            utc_timestamp(SystemTime::UNIX_EPOCH),
            "1970-01-01T00:00:00Z"
        );
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_996_298);
        assert_eq!(utc_timestamp(when), "2026-10-14T16:44:58Z");
        let leap = SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc_timestamp(leap), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn a_log_entry_holds_the_prompt_the_raw_reply_and_the_filter_taken() {
        let outcome = Ok(CommandOutput {
            success: true,
            status: "exit status: 0".into(),
            stdout: "`.a | length`\n".into(),
            stderr: String::new(),
        });
        let entry = log_entry(
            SystemTime::UNIX_EPOCH,
            "claude -p",
            "PROMPT\nLINES\n",
            &outcome,
        );
        assert_eq!(
            entry,
            "\
=== 1970-01-01T00:00:00Z  claude -p
--- prompt
PROMPT
LINES
--- stdout
`.a | length`
--- stderr: (empty)
--- exit status: 0
--- filter: .a | length

"
        );
    }

    #[test]
    fn a_log_entry_records_failures() {
        let failed = Ok(CommandOutput {
            success: false,
            status: "exit status: 1".into(),
            stdout: String::new(),
            stderr: "not logged in".into(),
        });
        let entry = log_entry(SystemTime::UNIX_EPOCH, "c", "p", &failed);
        assert!(
            entry.contains("--- stdout: (empty)\n--- stderr\nnot logged in\n--- exit status: 1\n"),
            "{entry}"
        );
        assert!(!entry.contains("filter:"), "{entry}");
        let entry = log_entry(SystemTime::UNIX_EPOCH, "c", "p", &Err("no sh".into()));
        assert!(entry.contains("--- could not run: no sh\n"), "{entry}");
    }
}
