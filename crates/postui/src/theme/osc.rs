//! Terminal color introspection via OSC escape sequences (OSC 10/11/4), so
//! `ThemeChoice::Terminal` can seed a generated palette from whatever colors
//! the user's real terminal is already configured with, instead of forcing
//! a hand-picked default on top of it.
//!
//! The wire protocol (writing queries, reading a raced reply with a
//! deadline) lives in [`OscQuery`]; the pure, TTY-free byte parsing lives in
//! [`parse_osc_response`] so it can be unit tested without a terminal.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Colors recovered from a terminal color query: background/foreground plus
/// the 16 ANSI slots (indices 0..=15), each `None` when the terminal didn't
/// answer (or answered with something unparseable) for that slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueriedColors {
    pub bg: Option<(u8, u8, u8)>,
    pub fg: Option<(u8, u8, u8)>,
    pub ansi: [Option<(u8, u8, u8)>; 16],
}

/// Abstraction over "ask the terminal what colors it's using", so
/// `Theme::from_environment` can be tested with a fake instead of a real
/// TTY.
pub trait TerminalPalette {
    fn query(&mut self) -> QueriedColors;
}

/// The real implementation: writes OSC queries to stdout and races a 150ms
/// deadline reading stdin for replies. Requires raw mode already be active
/// at the call site (see `main.rs`) — this type never touches raw mode
/// itself, since toggling it here would race whatever else the caller is
/// doing with the terminal.
pub struct OscQuery;

/// How long we wait for the terminal to answer before giving up and
/// treating it as silent. Chosen to be well under one frame's worth of
/// perceptible startup delay while still giving a real terminal (or tmux
/// passthrough) time to round-trip.
const QUERY_DEADLINE: Duration = Duration::from_millis(150);

impl TerminalPalette for OscQuery {
    fn query(&mut self) -> QueriedColors {
        query_via_stdio().unwrap_or_default()
    }
}

/// Writes the OSC 10/11/4 queries plus a DA1 fence, then reads whatever
/// comes back within the deadline. Any I/O error degrades to `None` (via
/// the caller's `unwrap_or_default`) rather than propagating — a silent or
/// misbehaving terminal must never block or crash startup.
fn query_via_stdio() -> std::io::Result<QueriedColors> {
    let mut stdout = std::io::stdout();
    let mut query = String::from("\x1b]10;?\x07\x1b]11;?\x07");
    for slot in [1u8, 2, 3, 4, 9, 10, 11, 12] {
        query.push_str(&format!("\x1b]4;{slot};?\x07"));
    }
    query.push_str("\x1b[c"); // DA1 fence: marks the end of the reply burst
    stdout.write_all(query.as_bytes())?;
    stdout.flush()?;

    let buf = read_until_da1_or_deadline()?;
    Ok(parse_osc_response(&buf))
}

/// Reads raw bytes from stdin until a DA1 reply (`\x1b[?...c`) is seen or
/// `QUERY_DEADLINE` elapses, whichever comes first. Raw mode must already
/// be active (enforced by the caller), or these bytes would echo to the
/// screen instead of coming back through stdin.
fn read_until_da1_or_deadline() -> std::io::Result<Vec<u8>> {
    let mut stdin = std::io::stdin();
    let deadline = Instant::now() + QUERY_DEADLINE;
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];

    while Instant::now() < deadline {
        match poll_read(&mut stdin, &mut byte) {
            Some(true) => {
                buf.push(byte[0]);
                if ends_with_da1(&buf) {
                    break;
                }
            }
            Some(false) => break, // EOF
            None => continue,     // nothing ready yet; keep polling until the deadline
        }
    }
    Ok(buf)
}

/// Non-blocking-ish single byte read: `Some(true)` if a byte was read,
/// `Some(false)` on EOF, `None` if nothing is available right now. Uses
/// `crossterm`'s event poll on stdin's readiness rather than raw termios
/// flags, keeping this file free of platform-specific code.
fn poll_read(stdin: &mut std::io::Stdin, byte: &mut [u8; 1]) -> Option<bool> {
    use ratatui::crossterm::event::poll;
    // We use crossterm's `poll` purely to learn whether stdin has bytes
    // ready, without letting it parse those bytes as key/mouse events (its
    // `read()` would swallow the escape bytes we need intact). The actual
    // read is a plain `std::io::Read` byte-at-a-time pull.
    if matches!(poll(Duration::from_millis(5)), Ok(true)) {
        match stdin.read(byte) {
            Ok(0) => Some(false),
            Ok(_) => Some(true),
            Err(_) => Some(false),
        }
    } else {
        None
    }
}

fn ends_with_da1(buf: &[u8]) -> bool {
    // DA1 replies look like "\x1b[?<params>c"; scan backward for the
    // shortest plausible match rather than a full parse.
    if !buf.ends_with(b"c") {
        return false;
    }
    if let Some(start) = find_last(buf, b"\x1b[?") {
        return start < buf.len() - 1; // there's at least "?...c" after the fence
    }
    false
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .rev()
        .find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// Pure parser for the byte stream a terminal sends back in response to our
/// OSC 10/11/4 queries. Exposed (not just `pub(crate)`) so tests can drive
/// it directly with canned bytes, with no TTY involved.
///
/// Recognizes `\x1b]{code};rgb:RRRR/GGGG/BBBB` replies, where `code` is
/// `10` (fg), `11` (bg), or `4;{slot}` (an ANSI color slot). Each channel is
/// 16-bit; we take the high byte. Anything else in the buffer (DA1 fence,
/// unrelated bytes, partial replies) is ignored.
pub fn parse_osc_response(buf: &[u8]) -> QueriedColors {
    let mut out = QueriedColors::default();
    let text = String::from_utf8_lossy(buf);
    for reply in text.split('\x1b').skip(1) {
        let Some(rest) = reply.strip_prefix(']') else {
            continue;
        };
        let rest = rest.trim_end_matches(['\x07', '\x1b']); // BEL or ST terminator
        let rest = rest.trim_end_matches('\\'); // ST is "\x1b\\"; the ESC was the split delimiter

        if let Some(rgb_str) = rest.strip_prefix("10;rgb:") {
            out.fg = parse_rgb(rgb_str);
        } else if let Some(rgb_str) = rest.strip_prefix("11;rgb:") {
            out.bg = parse_rgb(rgb_str);
        } else if let Some(slot_and_rgb) = rest.strip_prefix("4;")
            && let Some((slot, rgb_str)) = slot_and_rgb.split_once(";rgb:")
            && let Ok(slot) = slot.parse::<usize>()
            && slot < 16
        {
            out.ansi[slot] = parse_rgb(rgb_str);
        }
    }
    out
}

/// Parses `"RRRR/GGGG/BBBB"` (16-bit hex per channel, X11 `rgb:` syntax)
/// into 8-bit components by taking the high byte of each 16-bit value.
fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let mut parts = s.splitn(3, '/');
    let r = parts.next()?;
    let g = parts.next()?;
    let b = parts.next()?;
    let high_byte = |hex: &str| -> Option<u8> {
        let v = u16::from_str_radix(hex, 16).ok()?;
        Some(if hex.len() >= 3 {
            (v >> 8) as u8
        } else {
            // Some terminals answer with fewer than 4 hex digits per
            // channel; scale up so e.g. "ff" (8-bit) still lands correctly.
            v as u8
        })
    };
    Some((high_byte(r)?, high_byte(g)?, high_byte(b)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_osc11_bg_reply() {
        let q = parse_osc_response(b"\x1b]11;rgb:1e1e/2a2a/3939\x07\x1b[?6c");
        assert_eq!(q.bg, Some((0x1e, 0x2a, 0x39)));
    }

    #[test]
    fn parses_osc10_fg_reply() {
        let q = parse_osc_response(b"\x1b]10;rgb:d8d8/dede/e9e9\x07");
        assert_eq!(q.fg, Some((0xd8, 0xde, 0xe9)));
    }

    #[test]
    fn parses_osc4_slot() {
        let q = parse_osc_response(b"\x1b]4;4;rgb:0101/7878/d4d4\x07");
        assert_eq!(q.ansi[4], Some((0x01, 0x78, 0xd4)));
    }

    #[test]
    fn parses_multiple_replies_in_one_buffer() {
        let q = parse_osc_response(
            b"\x1b]11;rgb:1010/1414/2020\x07\x1b]4;1;rgb:f7f7/7676/8e8e\x07\x1b]4;2;rgb:9e9e/cece/6a6a\x07",
        );
        assert_eq!(q.bg, Some((0x10, 0x14, 0x20)));
        assert_eq!(q.ansi[1], Some((0xf7, 0x76, 0x8e)));
        assert_eq!(q.ansi[2], Some((0x9e, 0xce, 0x6a)));
    }

    #[test]
    fn empty_input_yields_all_none() {
        let q = parse_osc_response(b"");
        assert!(q.bg.is_none() && q.fg.is_none() && q.ansi.iter().all(Option::is_none));
    }

    #[test]
    fn garbage_input_yields_all_none() {
        let q = parse_osc_response(b"not an escape sequence at all");
        assert!(q.bg.is_none() && q.fg.is_none() && q.ansi.iter().all(Option::is_none));
    }

    #[test]
    fn st_terminated_reply_parses_same_as_bel() {
        let q = parse_osc_response(b"\x1b]11;rgb:1e1e/2a2a/3939\x1b\\");
        assert_eq!(q.bg, Some((0x1e, 0x2a, 0x39)));
    }
}
