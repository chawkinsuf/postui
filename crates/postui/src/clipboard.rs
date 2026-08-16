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
/// any), the OSC 52 size threshold, and a lazily-initialized native
/// clipboard handle.
pub struct Clipboard {
    cmd: Option<String>,
    osc52_limit: usize,
    allow_arboard: bool,
    arboard: Option<Result<arboard::Clipboard, String>>,
}

impl Clipboard {
    pub fn new(settings: &UiSettings) -> Self {
        Self {
            cmd: settings.clipboard_cmd.clone(),
            osc52_limit: settings.osc52_limit,
            allow_arboard: true,
            arboard: None,
        }
    }

    /// Test-only constructor that lets tests disable the arboard tier so
    /// the cmd and OSC 52 tiers can be exercised deterministically without
    /// touching a real OS clipboard. Gated on the `test-util` feature
    /// (rather than plain `#[cfg(test)]`) because integration tests under
    /// `tests/` link against the crate as an ordinary dependency — `cfg(test)`
    /// never applies there. The crate's own `[dev-dependencies]` pulls
    /// itself back in with `test-util` enabled so `cargo test` gets this for
    /// free; a plain `cargo build`/`cargo build --release` does not enable
    /// the feature, so this stays unreachable outside tests.
    #[cfg(any(test, feature = "test-util"))]
    pub fn new_for_test(cmd: Option<String>, limit: usize, allow_arboard: bool) -> Self {
        Self {
            cmd,
            osc52_limit: limit,
            allow_arboard,
            arboard: None,
        }
    }

    pub fn copy(&mut self, text: &str) -> CopyResult {
        if let Some(cmd) = self.cmd.clone() {
            return Self::copy_via_cmd(&cmd, text);
        }

        if self.allow_arboard {
            let handle = self
                .arboard
                .get_or_insert_with(|| arboard::Clipboard::new().map_err(|e| e.to_string()));
            if let Ok(clipboard) = handle
                && clipboard.set_text(text).is_ok()
            {
                return CopyResult::Copied { via: "clipboard" };
            }
        }

        Self::copy_via_osc52(text, self.osc52_limit)
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
            return CopyResult::Failed(e.to_string());
        }

        match child.wait_with_output() {
            Ok(output) if output.status.success() => CopyResult::Copied {
                via: "clipboard_cmd",
            },
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

    fn copy_via_osc52(text: &str, limit: usize) -> CopyResult {
        if text.len() >= limit {
            return CopyResult::OscTooLarge;
        }

        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let sequence = format!("\x1b]52;c;{encoded}\x07");
        let mut stdout = std::io::stdout();
        match stdout
            .write_all(sequence.as_bytes())
            .and_then(|()| stdout.flush())
        {
            Ok(()) => CopyResult::Copied {
                via: "terminal (OSC 52)",
            },
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
        let mut clipboard = Clipboard::new_for_test(Some(cmd), 65536, false);

        let result = clipboard.copy("hello");

        assert!(matches!(
            result,
            CopyResult::Copied {
                via: "clipboard_cmd"
            }
        ));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello");
    }

    #[test]
    fn failing_cmd_fails_without_falling_through() {
        let mut clipboard = Clipboard::new_for_test(Some("false".to_string()), 65536, true);

        let result = clipboard.copy("hello");

        assert!(
            matches!(result, CopyResult::Failed(_)),
            "cmd is authoritative when set: {result:?}"
        );
    }

    #[test]
    fn osc52_threshold_boundary_at_limit_is_too_large() {
        let mut clipboard = Clipboard::new_for_test(None, 8, false);

        let result = clipboard.copy("12345678"); // 8 bytes, at the limit

        assert!(matches!(result, CopyResult::OscTooLarge));
    }

    #[test]
    fn osc52_threshold_boundary_below_limit_copies_via_terminal() {
        let mut clipboard = Clipboard::new_for_test(None, 8, false);

        let result = clipboard.copy("1234567"); // 7 bytes, below the limit

        assert!(matches!(
            result,
            CopyResult::Copied {
                via: "terminal (OSC 52)"
            }
        ));
    }
}
