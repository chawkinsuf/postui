//! Tiered clipboard: an optional external `clipboard_cmd` takes priority at
//! any size, then the native OS clipboard via `arboard` (no size limit),
//! then an OSC 52 terminal escape sequence as a headless/SSH fallback, gated
//! by a size threshold below which nothing is silently truncated — payloads
//! at or over the threshold are not sent and not auto-saved.

use std::io::Write as _;
use std::process::{Command, Stdio};

use base64::Engine as _;

use crate::config::UiSettings;

/// Outcome of a [`Clipboard::copy`] attempt.
#[derive(Debug)]
pub enum CopyResult {
    /// Copy succeeded via the named tier: `"clipboard_cmd"`, `"clipboard"`,
    /// or `"terminal (OSC 52)"`.
    Copied { via: &'static str },
    /// The OSC 52 tier was reached but the payload was at or over the size
    /// threshold; nothing was sent.
    OscTooLarge,
    /// A tier that is authoritative when applicable (currently only
    /// `clipboard_cmd`) failed; the message is the cause.
    Failed(String),
}

/// A tiered clipboard writer. Holds the configured external command (if
/// any), the OSC 52 size threshold, and where the native and OSC 52 tiers
/// go.
pub struct Clipboard {
    cmd: Option<String>,
    osc52_limit: usize,
    host: Host,
    /// Test-injected `read()` result, so paste flows are testable without
    /// a real OS clipboard. See [`Clipboard::set_read_for_test`].
    #[cfg(any(test, feature = "test-util"))]
    test_read: Option<String>,
}

/// Where the native and OSC 52 tiers go.
enum Host {
    /// The OS clipboard through `arboard` (the handle is created on first
    /// use) and the real terminal on stdout.
    #[cfg_attr(test, expect(dead_code, reason = "no unit test may reach the host"))]
    Real(Option<Result<arboard::Clipboard, String>>),
    /// A test's clipboard: no OS clipboard, and OSC 52 sequences collect
    /// here instead of reaching the terminal that runs the tests, which
    /// would put the test's value on the developer's clipboard.
    #[cfg(any(test, feature = "test-util"))]
    Offline(Vec<u8>),
}

impl Clipboard {
    pub fn new(settings: &UiSettings) -> Self {
        Self {
            cmd: settings.clipboard_cmd.clone(),
            osc52_limit: settings.osc52_limit,
            // Every test App is built through here (`App::bare`), so a unit
            // test can't reach the host whichever path built its App. Keyed
            // on `cfg(test)`, not the `test-util` feature: `cargo test`
            // builds the real binary with `test-util` on too, and that
            // binary must keep its clipboard.
            #[cfg(test)]
            host: Host::Offline(Vec::new()),
            #[cfg(not(test))]
            host: Host::Real(None),
            #[cfg(any(test, feature = "test-util"))]
            test_read: None,
        }
    }

    /// Updates the settings-derived fields in place, keeping the lazily
    /// created native handle. What a live config reload must use instead
    /// of building a fresh `Clipboard`: dropping the old one drops
    /// `arboard`'s handle, and on X11 without a clipboard manager that
    /// destroys the selection-owning window — everything the user had
    /// copied out of postui would stop being pasteable the moment they
    /// pressed the reload key.
    pub fn reconfigure(&mut self, settings: &UiSettings) {
        self.cmd = settings.clipboard_cmd.clone();
        self.osc52_limit = settings.osc52_limit;
    }

    /// Test-only constructor for a clipboard that never touches the host:
    /// no OS clipboard, and OSC 52 sequences land in [`Self::osc52_sent`],
    /// so the cmd and OSC 52 tiers can be exercised deterministically.
    /// Gated on the `test-util` feature (rather than plain `#[cfg(test)]`)
    /// because integration tests under `tests/` link against the crate as
    /// an ordinary dependency — `cfg(test)` never applies there, so
    /// [`Self::new`] reaches the real host in them, and an integration test
    /// that copies must install this first. The crate's own
    /// `[dev-dependencies]` pulls itself back in with `test-util` enabled so
    /// `cargo test` gets this for free; a plain `cargo build`/`cargo build
    /// --release` does not enable the feature, so this stays unreachable
    /// outside tests.
    #[cfg(any(test, feature = "test-util"))]
    pub fn new_for_test(cmd: Option<String>, limit: usize) -> Self {
        Self { cmd, osc52_limit: limit, host: Host::Offline(Vec::new()), test_read: None }
    }

    /// The OSC 52 bytes a test's clipboard collected in place of the
    /// terminal; empty for a real one.
    #[cfg(any(test, feature = "test-util"))]
    pub fn osc52_sent(&self) -> &[u8] {
        match &self.host {
            Host::Real(_) => &[],
            Host::Offline(sent) => sent,
        }
    }

    /// Makes the next `read()` calls return `text` (tests only).
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_read_for_test(&mut self, text: &str) {
        self.test_read = Some(text.to_string());
    }

    /// Reads text from the OS clipboard — the arboard tier only. There is
    /// no external-command read (no `paste_cmd` config exists) and no
    /// OSC 52 read (terminals block clipboard reads almost universally).
    /// `Err` carries the cause for a toast.
    pub fn read(&mut self) -> Result<String, String> {
        #[cfg(any(test, feature = "test-util"))]
        if let Some(text) = self.test_read.clone() {
            return Ok(text);
        }
        match self.native() {
            Some(Ok(clipboard)) => clipboard.get_text().map_err(|e| e.to_string()),
            Some(Err(e)) => Err(e.clone()),
            None => Err("no OS clipboard available".to_string()),
        }
    }

    pub fn copy(&mut self, text: &str) -> CopyResult {
        if let Some(cmd) = self.cmd.clone() {
            return Self::copy_via_cmd(&cmd, text);
        }

        if let Some(Ok(clipboard)) = self.native()
            && clipboard.set_text(text).is_ok()
        {
            return CopyResult::Copied { via: "clipboard" };
        }

        self.copy_via_osc52(text)
    }

    /// The native clipboard handle, created on first use; `None` for a
    /// test's clipboard, which has no OS clipboard to reach.
    fn native(&mut self) -> Option<&mut Result<arboard::Clipboard, String>> {
        match &mut self.host {
            Host::Real(handle) => Some(
                handle.get_or_insert_with(|| arboard::Clipboard::new().map_err(|e| e.to_string())),
            ),
            #[cfg(any(test, feature = "test-util"))]
            Host::Offline(_) => None,
        }
    }

    fn copy_via_cmd(cmd: &str, text: &str) -> CopyResult {
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => return CopyResult::Failed(e.to_string()),
        };

        if let Some(mut stdin) = child.stdin.take()
            && let Err(e) = stdin.write_all(text.as_bytes())
        {
            // The child may already have exited (e.g. EPIPE from a command
            // that closed stdin without reading it); reap it here so it
            // doesn't linger as a zombie until the app exits.
            let _ = child.kill();
            let _ = child.wait();
            return CopyResult::Failed(e.to_string());
        }

        match child.wait_with_output() {
            Ok(output) if output.status.success() => CopyResult::Copied { via: "clipboard_cmd" },
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if stderr.is_empty() {
                    CopyResult::Failed(output.status.to_string())
                } else {
                    CopyResult::Failed(stderr)
                }
            }
            Err(e) => CopyResult::Failed(e.to_string()),
        }
    }

    fn copy_via_osc52(&mut self, text: &str) -> CopyResult {
        if text.len() >= self.osc52_limit {
            return CopyResult::OscTooLarge;
        }

        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let sequence = format!("\x1b]52;c;{encoded}\x07");
        let written = match &mut self.host {
            Host::Real(_) => {
                let mut stdout = std::io::stdout();
                stdout.write_all(sequence.as_bytes()).and_then(|()| stdout.flush())
            }
            #[cfg(any(test, feature = "test-util"))]
            Host::Offline(sent) => {
                sent.extend_from_slice(sequence.as_bytes());
                Ok(())
            }
        };
        match written {
            Ok(()) => CopyResult::Copied { via: "terminal (OSC 52)" },
            Err(e) => CopyResult::Failed(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cmd_tier_writes_to_target_and_reports_via_clipboard_cmd() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.txt");
        let cmd = format!("cat > {}", out.to_string_lossy());
        let mut clipboard = Clipboard::new_for_test(Some(cmd), 65536);

        let result = clipboard.copy("hello");

        assert!(matches!(result, CopyResult::Copied { via: "clipboard_cmd" }));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello");
    }

    #[test]
    fn failing_cmd_fails_without_falling_through() {
        let mut clipboard = Clipboard::new_for_test(Some("false".to_string()), 65536);

        let result = clipboard.copy("hello");

        assert!(
            matches!(result, CopyResult::Failed(_)),
            "cmd is authoritative when set: {result:?}"
        );
        assert!(clipboard.osc52_sent().is_empty(), "nothing went to the terminal tier");
    }

    #[test]
    fn cmd_that_exits_before_reading_stdin_fails_without_hanging() {
        // `exit 0` closes stdin immediately without reading it. A payload
        // larger than a pipe buffer forces `write_all` to hit EPIPE once
        // the child is gone. This exercises the write-error path in
        // `copy_via_cmd`, which now kills+waits the child before
        // returning — proving the call still terminates (rather than
        // hanging on a full pipe) and reports Failed. Full absence of a
        // zombie process isn't practical to assert deterministically from
        // a unit test (no access to the child's pid); verified by
        // inspection of the kill()+wait() added on the error path.
        let big = "x".repeat(4 * 1024 * 1024);
        let mut clipboard = Clipboard::new_for_test(Some("exit 0".to_string()), 65536);

        let result = clipboard.copy(&big);

        assert!(
            matches!(result, CopyResult::Failed(_)),
            "expected the broken pipe to surface as Failed: {result:?}"
        );
    }

    /// `App::bare` builds this one, so every test App starts with it: in a
    /// test it must reach neither the OS clipboard nor the terminal.
    #[test]
    fn the_settings_clipboard_stays_off_the_host_in_tests() {
        let mut clipboard = Clipboard::new(&UiSettings::default());

        let result = clipboard.copy("x");

        assert!(
            matches!(result, CopyResult::Copied { via: "terminal (OSC 52)" }),
            "skips the OS clipboard: {result:?}"
        );
        assert_eq!(clipboard.osc52_sent(), b"\x1b]52;c;eA==\x07", "and the terminal");
        assert!(clipboard.read().is_err(), "and never reads it");
    }

    #[test]
    fn osc52_threshold_boundary_at_limit_is_too_large() {
        let mut clipboard = Clipboard::new_for_test(None, 8);

        let result = clipboard.copy("12345678"); // 8 bytes, at the limit

        assert!(matches!(result, CopyResult::OscTooLarge));
        assert!(clipboard.osc52_sent().is_empty(), "nothing is sent");
    }

    #[test]
    fn osc52_threshold_boundary_below_limit_copies_via_terminal() {
        let mut clipboard = Clipboard::new_for_test(None, 8);

        let result = clipboard.copy("1234567"); // 7 bytes, below the limit

        assert!(matches!(result, CopyResult::Copied { via: "terminal (OSC 52)" }));
        assert_eq!(clipboard.osc52_sent(), b"\x1b]52;c;MTIzNDU2Nw==\x07");
    }
}
