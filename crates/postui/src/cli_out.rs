//! Prints that must not panic on a stream that can't be written, which
//! `print!`/`eprint!` do. Two kinds use it:
//!
//! - the command line's own text, before any terminal mode is set
//!   (`--help`, `--version`, `--setup`, the `--keydump` banner, a usage
//!   error; #162, #168): a failed write ends the run with a status. Once
//!   keydump's echo loop runs in raw mode, its write and terminal errors
//!   end the run as errors, as they always have;
//! - the app's panic hook, reporting a worker thread's panic while the TUI
//!   runs (#169): a panic inside the hook would abort, so [`print_err`]
//!   drops a failed write. Both modes pass the text through untouched.

use std::io::Write;
use std::process::ExitCode;

/// Writes `text` to stdout. `None` when it all went out; otherwise the
/// status to end the run with. A closed pipe (`postui --help | head`)
/// means the reader stopped, so it ends quietly and succeeds; any other
/// error is said on stderr and fails.
pub fn print_out(text: &str) -> Option<ExitCode> {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => None,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Some(ExitCode::SUCCESS),
        Err(e) => {
            print_err(&format!("postui: can't write to stdout: {e}\n"));
            Some(ExitCode::FAILURE)
        }
    }
}

/// Writes `text` to stderr with one `write_all`. A failure there has nowhere left
/// to be told, so it is dropped: the caller's exit status still says what
/// happened.
pub fn print_err(text: &str) {
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
}
