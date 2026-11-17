//! The "describe a filter" shell-out: a configurable command (default
//! `claude -p`) that reads the whole prompt on stdin and prints a reply.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::SystemTime;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Whether `cmd`'s program (`program_name`) resolves on `PATH` (or is an
/// existing path).
pub fn program_available(cmd: &str) -> bool {
    let program = program_name(cmd);
    if program.is_empty() {
        return false;
    }
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// The program in `cmd`: its first word past any leading `NAME=value`
/// environment assignments — for toasts, menu hints, and the log, which
/// never sees the rest of the command (a key may be in it).
pub fn program_name(cmd: &str) -> &str {
    shell_words(cmd).into_iter().find(|word| !is_assignment(word)).unwrap_or("")
}

/// `cmd` split at whitespace the way a shell sees words: a quoted stretch
/// (`"a b"`, `'a b'`) stays inside its word, so `KEY="sk abc" llm` is two
/// words and `llm` is the program.
fn shell_words(cmd: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    for (i, c) in cmd.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                start.get_or_insert(i);
            }
            None if c.is_whitespace() => {
                if let Some(s) = start.take() {
                    words.push(&cmd[s..i]);
                }
            }
            None => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        words.push(&cmd[s..]);
    }
    words
}

/// `NAME=…` as a shell reads it: an identifier, then `=`. `env`, which
/// runs the command, takes any leading word with `=` as a setting; the two
/// differ for a word whose part before `=` is not an identifier, such as
/// a program path with `=` in it. That is left unhandled (the #314
/// paragraph of the jq-response-filter spec).
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
    })
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
        let last = self.stderr.lines().rev().find(|l| !l.trim().is_empty()).map(str::trim);
        Err(last.map(str::to_string).unwrap_or(self.status))
    }
}

/// One run of the AI command, shared by the task that runs it
/// (`run_command`) and the app, which ends it with `kill` from the UI
/// thread: on a cancel, and when the app quits, since a quit doesn't wait
/// for the task to be dropped.
#[derive(Default)]
pub struct CommandRun(Mutex<RunState>);

#[derive(Default)]
enum RunState {
    #[default]
    NotStarted,
    /// Started and not yet waited on, so its pid is still its own.
    Running(tokio::process::Child),
    /// Killed, or taken back to be waited on.
    Over,
}

impl CommandRun {
    /// Kills the command if it is running, and keeps it from starting if
    /// it hasn't yet. The kill is SIGKILL, sent at once: the command gets
    /// no chance to stop what it started itself, so a helper process it
    /// forked can outlive it (a cost of the `exec` design, in the #314
    /// paragraph of the jq-response-filter spec). A SIGTERM first would
    /// need a later SIGKILL for a command that ignores it, and a quit
    /// doesn't wait for anything after this call.
    pub fn kill(&self) {
        if let RunState::Running(mut child) = std::mem::replace(&mut *self.state(), RunState::Over)
        {
            let _ = child.start_kill();
        }
    }

    /// Whether the run is killed or has been taken back to be waited on.
    pub fn is_over(&self) -> bool {
        matches!(*self.state(), RunState::Over)
    }

    /// The running command's pid.
    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        match &*self.state() {
            RunState::Running(child) => child.id(),
            _ => None,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, RunState> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Runs `sh -c "exec env <cmd>"` with `stdin` piped in. The shell becomes
/// `env`, which takes `cmd`'s leading `NAME=value` words as its settings
/// and becomes the program in turn, so `CommandRun::kill` reaches the
/// command and not only the shell. `cmd` is not parsed, so in a list or a
/// pipeline only the first command gets `exec env` (the #314 paragraph of
/// the jq-response-filter spec). `Err` when the command could
/// not be started or waited on, or `run` was killed before it finished.
pub async fn run_command(
    cmd: String,
    stdin: String,
    run: &CommandRun,
) -> Result<CommandOutput, String> {
    if run.is_over() {
        return Err("cancelled".into());
    }
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(format!("exec env {cmd}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let pipe = child.stdin.take();
    // Both are piped above, so both are there.
    let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
        return Err("the command's output was not piped".into());
    };
    {
        let mut state = run.state();
        if matches!(*state, RunState::Over) {
            // Killed while it was being started.
            let _ = child.start_kill();
            return Err("cancelled".into());
        }
        *state = RunState::Running(child);
    }
    // Written while stdout and stderr are read, so a command that prints
    // before it has read all its input can't fill a pipe and stall both.
    let write = async move {
        if let Some(mut pipe) = pipe {
            // A child that exits early closes the pipe; that is reported
            // by its exit status below, not here.
            let _ = pipe.write_all(stdin.as_bytes()).await;
        }
        Ok::<_, std::io::Error>(())
    };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let read = tokio::try_join!(write, out.read_to_end(&mut stdout), err.read_to_end(&mut stderr))
        .map(|_| ());
    // Waited on only once out of the run, so `kill` never signals a pid
    // that has been reaped and may belong to another process by then.
    // From here, only dropping this future ends the command
    // (`kill_on_drop`), and a quit doesn't wait for that: a command that
    // has closed both streams and keeps running can outlive a quit, a cost
    // the #314 paragraph of the jq-response-filter spec accepts.
    let RunState::Running(mut child) = std::mem::replace(&mut *run.state(), RunState::Over) else {
        return Err("cancelled".into());
    };
    read.map_err(|e| e.to_string())?;
    let status = child.wait().await.map_err(|e| e.to_string())?;
    Ok(CommandOutput {
        success: status.success(),
        status: status.to_string(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Where `--debug-ai` writes: `ai.log` in the app's state directory
/// (`postui_core::state_dir`).
pub fn debug_log_path() -> Option<PathBuf> {
    Some(postui_core::state_dir()?.join("ai.log"))
}

/// The first half of a `--debug-ai` log entry, written before the command
/// runs so a reply that never comes (cancelled, superseded, hung) still
/// leaves its prompt behind: the request number, when, the command, and
/// the whole prompt. The number pairs it with its reply half, since a
/// superseded request's reply can land after the next request's prompt.
pub fn log_prompt(request: u64, when: SystemTime, cmd: &str, prompt: &str) -> String {
    let mut out = format!("{}--- prompt\n{prompt}", entry_header(request, when, cmd));
    if !prompt.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A whole entry for a request that sent nothing because the body was not
/// JSON, so the log's request numbers stay unbroken.
pub fn log_not_json(request: u64, when: SystemTime, cmd: &str) -> String {
    format!("{}--- not JSON: nothing sent\n\n", entry_header(request, when, cmd))
}

/// `=== #N <when>  <program>\n`. Only the program's name: the rest of the
/// command may carry a key (`KEY=… llm`, `llm --key …`), the log is plain
/// text, and the full command is in config.toml anyway.
fn entry_header(request: u64, when: SystemTime, cmd: &str) -> String {
    format!("=== #{request} {}  {}\n", utc_timestamp(when), program_name(cmd))
}

/// The second half, appended when the command finishes: what came back,
/// and the filter that was taken from it.
pub fn log_reply(request: u64, outcome: &Result<CommandOutput, String>) -> String {
    let mut out = format!("--- reply #{request}\n");
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
                out.push_str(&format!("--- filter: {}\n", filter.as_deref().unwrap_or("(none)")));
            }
        }
        Err(e) => out.push_str(&format!("--- could not run: {e}\n")),
    }
    out.push('\n');
    out
}

// `2026-10-14T16:44:58Z`, the edit flag's stamp too.
use postui_core::project::rfc3339 as utc_timestamp;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn program_available_looks_at_the_program_word() {
        assert!(program_available("sh -c 'echo hi'"));
        assert!(program_available("/bin/sh"));
        assert!(program_available("FOO=1 BAR_2=x sh"), "assignments are skipped");
        assert!(!program_available("definitely-not-a-program-42 --flag"));
        assert!(!program_available(""));
        assert!(!program_available("FOO=1"), "nothing but assignments");
        assert_eq!(program_name("A=1 llm --key k"), "llm");
        assert_eq!(program_name("1X=1 llm"), "1X=1", "not an identifier: a program");
    }

    async fn run(cmd: &str, stdin: &str) -> Result<CommandOutput, String> {
        run_command(cmd.into(), stdin.into(), &CommandRun::default()).await
    }

    #[tokio::test]
    async fn run_command_pipes_stdin_and_returns_stdout() {
        let out = run("cat", "hello").await.unwrap();
        assert_eq!(out.into_reply().unwrap(), "hello");
    }

    #[tokio::test]
    async fn run_command_keeps_leading_assignments() {
        for cmd in [
            "GREETING=\"hi there\" sh -c 'echo \"$GREETING\"'",
            "GREETING=hi\\ there sh -c 'echo \"$GREETING\"'",
            "GREETING=\"$(echo hi there)\" sh -c 'echo \"$GREETING\"'",
        ] {
            let out = run(cmd, "").await.unwrap();
            assert_eq!(out.into_reply().unwrap(), "hi there\n", "{cmd}");
        }
    }

    #[tokio::test]
    async fn run_command_keeps_redirections() {
        let out = run("sh -c 'echo oops >&2' 2>&1", "").await.unwrap();
        assert_eq!(out.into_reply().unwrap(), "oops\n");
    }

    /// The command reads its input only after it has printed more than a
    /// pipe holds, so the write and the reads have to go on together.
    #[tokio::test]
    async fn run_command_writes_and_reads_at_once() {
        let input = "x".repeat(256 * 1024);
        let fut = run("sh -c 'head -c 262144 /dev/zero >&2; cat'", &input);
        let out = tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("no stall between the stdin write and the reads")
            .unwrap();
        assert_eq!(out.stderr.len(), 256 * 1024);
        assert_eq!(out.stdout, input);
    }

    #[tokio::test]
    async fn run_command_reports_the_last_stderr_line_on_failure() {
        let out = run("sh -c 'echo one >&2; echo two >&2; exit 3'", "").await.unwrap();
        assert_eq!(out.stderr, "one\ntwo\n", "the whole stderr is kept");
        assert_eq!(out.into_reply().unwrap_err(), "two");
        let err = run("sh -c 'exit 4'", "").await.unwrap().into_reply().unwrap_err();
        assert!(err.contains("4"), "{err}");
    }

    /// The run ends only once every holder of its stdout has gone: had
    /// `sh` forked `sleep` rather than become it, the kill would end `sh`
    /// and leave `sleep` holding it.
    #[tokio::test]
    async fn kill_ends_the_command() {
        for cmd in ["sleep 30", "NAP=1 sleep 30", "NAP=a\\ b sleep 30"] {
            let run = std::sync::Arc::new(CommandRun::default());
            let task = tokio::spawn({
                let run = std::sync::Arc::clone(&run);
                async move { run_command(cmd.into(), String::new(), &run).await }
            });
            for _ in 0..2000 {
                if run.pid().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(run.pid().is_some(), "{cmd}: the command started");
            run.kill();
            let out = tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .expect("the run ends at the kill, not when sleep does")
                .unwrap();
            assert_eq!(out.unwrap_err(), "cancelled", "{cmd}");
            assert!(run.is_over());
        }
    }

    #[tokio::test]
    async fn a_run_killed_before_it_starts_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let run = CommandRun::default();
        run.kill();
        let cmd = format!("touch '{}'", marker.display());
        assert_eq!(run_command(cmd, String::new(), &run).await.unwrap_err(), "cancelled");
        assert!(!marker.exists(), "the command never ran");
    }

    #[test]
    fn timestamps_are_utc_iso() {
        assert_eq!(utc_timestamp(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
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
        let entry = log_prompt(7, SystemTime::UNIX_EPOCH, "claude -p", "PROMPT\nLINES\n")
            + &log_reply(7, &outcome);
        assert_eq!(
            entry,
            "\
=== #7 1970-01-01T00:00:00Z  claude
--- prompt
PROMPT
LINES
--- reply #7
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
        let entry = log_reply(2, &failed);
        assert!(
            entry.contains("--- stdout: (empty)\n--- stderr\nnot logged in\n--- exit status: 1\n"),
            "{entry}"
        );
        assert!(!entry.contains("filter:"), "{entry}");
        let entry = log_reply(3, &Err("no sh".into()));
        assert!(entry.contains("--- reply #3\n--- could not run: no sh\n"), "{entry}");
    }

    #[test]
    fn the_log_names_the_program_and_nothing_else_of_the_command() {
        let entry = log_prompt(
            1,
            SystemTime::UNIX_EPOCH,
            "API_KEY=\"sk-abc def\" llm --key sk-9 -m gpt",
            "p",
        );
        assert!(entry.starts_with("=== #1 1970-01-01T00:00:00Z  llm\n"), "{entry}");
        assert!(!entry.contains("sk-"), "{entry}");
        let entry = log_not_json(4, SystemTime::UNIX_EPOCH, "claude -p");
        assert_eq!(entry, "=== #4 1970-01-01T00:00:00Z  claude\n--- not JSON: nothing sent\n\n");
    }

    #[test]
    fn the_prompt_half_ends_in_a_newline_on_its_own() {
        assert_eq!(
            log_prompt(1, SystemTime::UNIX_EPOCH, "c", "p"),
            "=== #1 1970-01-01T00:00:00Z  c\n--- prompt\np\n"
        );
    }
}
