//! The "describe a filter" shell-out: a configurable command (default
//! `claude -p`) that reads the whole prompt on stdin and prints a reply.

use rustix::process::{Pid, Signal};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
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

/// `NAME=…` as a shell reads it: an identifier, then `=`.
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
    /// The pid of `sh`, which is also its process group's id.
    Running(Pid),
    /// Killed, or waited on.
    Over,
}

impl RunState {
    /// The run is over, and its group leaves `HANGUP_GROUP`.
    fn end(&mut self) {
        if let RunState::Running(pid) = *self {
            let raw = pid.as_raw_pid();
            let _ = HANGUP_GROUP.compare_exchange(raw, 0, Ordering::Relaxed, Ordering::Relaxed);
        }
        *self = RunState::Over;
    }
}

impl CommandRun {
    /// Kills the command if it is running, and keeps it from starting if
    /// it hasn't yet. The whole process group: `sh` may have forked the
    /// command rather than exec'd it, and the command may fork too.
    pub fn kill(&self) {
        let mut state = self.state();
        if let RunState::Running(pid) = *state {
            let _ = rustix::process::kill_process_group(pid, Signal::KILL);
        }
        state.end();
    }

    /// Whether the run is killed or has been waited on.
    pub fn is_over(&self) -> bool {
        matches!(*self.state(), RunState::Over)
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        matches!(*self.state(), RunState::Running(_))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, RunState> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Runs `sh -c cmd` with `stdin` piped in, in a process group of its own
/// (see `CommandRun::kill`). `Err` when the command could not be started
/// or waited on, or `run` was killed before it started.
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
        .arg(&cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    // `id` is `None` only once the child has been waited on.
    let Some(pid) = child.id().and_then(|id| Pid::from_raw(i32::try_from(id).ok()?)) else {
        return Err("the command has no pid".into());
    };
    {
        let mut state = run.state();
        if matches!(*state, RunState::Over) {
            // Killed while it was being started.
            let _ = rustix::process::kill_process_group(pid, Signal::KILL);
            return Err("cancelled".into());
        }
        *state = RunState::Running(pid);
        HANGUP_GROUP.store(pid.as_raw_pid(), Ordering::Relaxed);
    }
    if let Some(mut pipe) = child.stdin.take() {
        // A child that exits early closes the pipe; that is reported by
        // its exit status below, not here.
        let _ = pipe.write_all(stdin.as_bytes()).await;
        drop(pipe);
    }
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let read = match (child.stdout.take(), child.stderr.take()) {
        (Some(mut out), Some(mut err)) => {
            tokio::try_join!(out.read_to_end(&mut stdout), err.read_to_end(&mut stderr)).map(|_| ())
        }
        _ => Ok(()),
    };
    // `sh` is reaped only here, under the run's lock, and the run is over
    // from then: a `kill` signals the group only while `sh`, running or a
    // zombie, still holds its id.
    let status = loop {
        {
            let mut state = run.state();
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    state.end();
                    break Ok(status);
                }
                Err(e) => {
                    state.end();
                    break Err(e);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    read.map_err(|e| e.to_string())?;
    let status = status.map_err(|e| e.to_string())?;
    Ok(CommandOutput {
        success: status.success(),
        status: status.to_string(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// The process group of the AI command running now, or 0: what the hangup
/// handler kills. Kept here as well as in the run because a signal handler
/// can't take the run's lock. One at a time: a new request ends the last
/// one first.
static HANGUP_GROUP: AtomicI32 = AtomicI32::new(0);

/// Installs the hangup (SIGHUP) handler, when the hangup has its default
/// action: a hangup that is ignored (`nohup`) stays ignored. The terminal
/// sends its hangup only to its foreground group, and the AI command runs
/// in a group of its own (`run_command`), so the app passes it on.
pub fn catch_hangup() {
    // SAFETY: plain sigaction(2) calls on zeroed structs; `on_hangup` only
    // makes async-signal-safe calls.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut old) != 0
            || old.sa_sigaction != libc::SIG_DFL
        {
            return;
        }
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_hangup as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGHUP, &action, std::ptr::null_mut());
    }
}

/// Kills the AI command's group, then puts the hangup's default action
/// back and raises it again: SIGHUP stays blocked until this returns, and
/// then ends the app as it would have with no handler. Only an atomic load,
/// kill(2), signal(2) and raise(3), which are async-signal-safe.
extern "C" fn on_hangup(_: libc::c_int) {
    if let Some(group) = Pid::from_raw(HANGUP_GROUP.load(Ordering::Relaxed)) {
        let _ = rustix::process::kill_process_group(group, Signal::KILL);
    }
    // SAFETY: both are on POSIX's async-signal-safe list.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_DFL);
        libc::raise(libc::SIGHUP);
    }
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
    async fn run_command_reports_the_last_stderr_line_on_failure() {
        let out = run("echo one >&2; echo two >&2; exit 3", "").await.unwrap();
        assert_eq!(out.stderr, "one\ntwo\n", "the whole stderr is kept");
        assert_eq!(out.into_reply().unwrap_err(), "two");
        let err = run("exit 4", "").await.unwrap().into_reply().unwrap_err();
        assert!(err.contains("4"), "{err}");
    }

    /// `sh` forks `sleep` here, since a command follows it. The run ends
    /// only once every holder of its stdout has gone, `sleep` included.
    #[tokio::test]
    async fn kill_ends_what_sh_forked_too() {
        let run = std::sync::Arc::new(CommandRun::default());
        let task = tokio::spawn({
            let run = std::sync::Arc::clone(&run);
            async move { run_command("sleep 30; echo .x".into(), String::new(), &run).await }
        });
        for _ in 0..2000 {
            if run.is_running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(run.is_running(), "the command started");
        run.kill();
        let out = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the run ends at the kill, not when sleep does")
            .unwrap()
            .unwrap();
        assert!(!out.success);
        assert!(run.is_over());
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

    /// What the hangup tests run, in a process of their own since a hangup
    /// ends the process it reaches: this test binary run again with
    /// `POSTUI_HANGUP_CHILD` set (`hangup_child_process`). Without it, this
    /// test does nothing.
    #[test]
    fn hangup_child() {
        let Ok(mode) = std::env::var("POSTUI_HANGUP_CHILD") else {
            return;
        };
        // The pipe to the parent test, on an fd every process this one
        // starts inherits (`dup2` leaves close-on-exec off), so the parent
        // reads to its end only once they have all ended.
        assert_eq!(unsafe { libc::dup2(1, 9) }, 9);
        if mode == "ignored" {
            unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
            catch_hangup();
            assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
            println!("survived");
            return;
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        if mode == "slot" {
            let group = || HANGUP_GROUP.load(Ordering::Relaxed);
            runtime.block_on(run("true", "")).unwrap();
            assert_eq!(group(), 0, "a run that ended leaves the slot");
            let run = std::sync::Arc::new(CommandRun::default());
            let task = runtime.spawn({
                let run = std::sync::Arc::clone(&run);
                async move { run_command("sleep 30".into(), String::new(), &run).await }
            });
            wait_until_running(&run);
            assert_ne!(group(), 0, "a running command is in the slot");
            run.kill();
            assert_eq!(group(), 0, "a killed run leaves the slot");
            runtime.block_on(task).unwrap().unwrap();
            println!("checked");
            return;
        }
        catch_hangup();
        let _task = (mode == "run").then(|| {
            let run = std::sync::Arc::new(CommandRun::default());
            let task = runtime.spawn({
                let run = std::sync::Arc::clone(&run);
                // `sh` forks `sleep`, since a command follows it.
                async move { run_command("sleep 30; echo .x".into(), String::new(), &run).await }
            });
            wait_until_running(&run);
            task
        });
        println!("ready");
        std::thread::sleep(Duration::from_secs(30));
    }

    fn wait_until_running(run: &CommandRun) {
        for _ in 0..2000 {
            if run.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(run.is_running(), "the command started");
    }

    /// The child's stdout, a line at a time; the channel closes at its end.
    fn hangup_child_process(
        mode: &str,
    ) -> (std::process::Child, std::sync::mpsc::Receiver<String>) {
        use std::io::BufRead as _;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["ai::tests::hangup_child", "--exact", "--nocapture", "--test-threads=1"])
            .env("POSTUI_HANGUP_CHILD", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        (child, rx)
    }

    /// Sends the child a hangup once it is ready, and checks that it and
    /// everything it started end (its stdout reaches its end) and that
    /// the hangup is what ended it.
    fn hang_up_on(mode: &str) {
        use std::os::unix::process::ExitStatusExt as _;
        use std::sync::mpsc::RecvTimeoutError;
        let (mut child, lines) = hangup_child_process(mode);
        loop {
            let line = lines.recv_timeout(Duration::from_secs(20)).expect("the child got ready");
            // The harness's `test … ... ` goes before it on the line.
            if line.ends_with("ready") {
                break;
            }
        }
        rustix::process::kill_process(Pid::from_child(&child), Signal::HUP).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match lines.recv_timeout(left) {
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    panic!("something the child started outlived the hangup");
                }
            }
        }
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGHUP), "{status}");
    }

    #[test]
    fn a_hangup_kills_the_running_command_and_ends_the_app_as_a_hangup() {
        hang_up_on("run");
    }

    #[test]
    fn a_hangup_with_no_command_running_ends_the_app_as_a_hangup() {
        hang_up_on("idle");
    }

    #[test]
    fn an_ignored_hangup_stays_ignored() {
        let (mut child, lines) = hangup_child_process("ignored");
        let status = child.wait().unwrap();
        assert!(status.success(), "{status}");
        assert!(lines.iter().any(|line| line.ends_with("survived")));
    }

    /// In a process of its own, since every run in this one shares the slot.
    #[test]
    fn the_hangup_slot_holds_the_running_command_only() {
        let (mut child, lines) = hangup_child_process("slot");
        let status = child.wait().unwrap();
        assert!(status.success(), "{status}");
        assert!(lines.iter().any(|line| line.ends_with("checked")));
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
