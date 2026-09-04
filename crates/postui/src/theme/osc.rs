//! Terminal color introspection via OSC escape sequences (OSC 10/11/4), so
//! the theme registry's Terminal entry can seed a generated palette from
//! whatever colors the user's real terminal is already configured with,
//! instead of forcing a hand-picked default on top of it.
//!
//! The wire protocol (writing queries, reading a raced reply with a
//! deadline) lives in [`OscQuery`]; the pure, TTY-free byte parsing lives in
//! [`parse_osc_response`] so it can be unit tested without a terminal.

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::AsFd;
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
/// `seeds_from_queried` can be tested with a fake instead of a real
/// TTY.
pub trait TerminalPalette {
    fn query(&mut self) -> QueriedColors;
}

/// The real implementation: writes OSC queries to stdout and races a 600ms
/// deadline reading stdin for replies. Requires raw mode already be active
/// at the call site (see `main.rs`) — this type never touches raw mode
/// itself, since toggling it here would race whatever else the caller is
/// doing with the terminal.
pub struct OscQuery;

/// How long we wait for the terminal to answer before giving up and
/// treating it as silent — at startup and, via `drain_until_da1_fence`, at
/// teardown. The read loop exits early on the DA1 fence, so a
/// responsive terminal never waits this long — the deadline only bites for
/// terminals that stay silent. 150ms proved too tight in practice: a
/// terminal answering late (observed on macOS under load) caused a
/// nondeterministic fallback to the built-in dark seeds between launches.
pub const QUERY_DEADLINE: Duration = Duration::from_millis(600);

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

    let mut stdin = std::io::stdin();
    let deadline = Instant::now() + QUERY_DEADLINE;
    let buf = read_until_da1_or_deadline(&mut stdin, deadline);
    Ok(parse_osc_response(&buf))
}

/// Unbuffered stdin: each `read` is one `read(2)` on fd 0. `std::io::Stdin`
/// goes through a shared 8 KiB `BufReader`, which on the first byte would
/// slurp everything the tty holds into a buffer `poll(2)` can't see — the
/// fence loop below would then wait on a fd that looks idle while the DA1
/// reply sits in userspace, and whatever followed the reply would vanish
/// with the process instead of reaching the shell.
pub struct RawStdin;

impl Read for RawStdin {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        rustix::io::read(std::io::stdin(), buf).map_err(std::io::Error::from)
    }
}

impl AsFd for RawStdin {
    fn as_fd(&self) -> rustix::fd::BorrowedFd<'_> {
        // SAFETY: fd 0 is open for the life of the process (std owns it and
        // never closes it), so a borrow bounded by `&self` cannot dangle.
        unsafe { rustix::fd::BorrowedFd::borrow_raw(0) }
    }
}

/// Writes a DA1 query (`\x1b[c`) to `out` and then consumes `input` up to
/// and including the terminal's reply, or until `deadline` if none comes.
/// Returns the consumed bytes.
///
/// This is the teardown counterpart of the startup query above, used after
/// the mouse-disable sequence has been written: a terminal answers DA1 only
/// once it has processed everything written before the query, so any mouse
/// reports it emitted before honouring the disable necessarily land in
/// `input` ahead of the reply. Consuming through the reply therefore
/// swallows exactly the reports that would otherwise be typed at the shell,
/// however slowly the terminal gets to the disable — where a fixed "wait
/// for N ms of quiet" guess fails as soon as the terminal is slower than
/// N. Bytes after the reply are left alone: the terminal emitted them with
/// mouse reporting already off, so they are real input for whoever reads
/// the tty next. Raw mode must still be on when this runs, and the caller
/// must have stopped crossterm's own reader (dropped the `EventStream`)
/// first, or the two would race over the same fd.
pub fn drain_until_da1_fence<W: Write, S: Read + AsFd>(
    out: &mut W,
    input: &mut S,
    deadline: Instant,
) -> Vec<u8> {
    if out.write_all(b"\x1b[c").and_then(|()| out.flush()).is_err() {
        return Vec::new();
    }
    read_until_da1_or_deadline(input, deadline)
}

/// Reads raw bytes from `source` until a DA1 reply (`\x1b[?...c`) is seen or
/// `deadline` elapses, whichever comes first. Raw mode must already be
/// active at the real call site (enforced by the caller in `main.rs`), or
/// these bytes would echo to the screen instead of coming back through
/// stdin.
///
/// Generic over the byte source (rather than hardcoding `std::io::Stdin`)
/// so it can be driven end-to-end in tests against a real OS pipe, with no
/// TTY involved — a plain `#[test]` can write canned reply bytes into the
/// write end and assert this function (and the `poll`/`read` syscalls it
/// actually issues) recovers them.
///
/// Deliberately uses raw `poll(2)`/`read(2)` via `rustix` rather than
/// `ratatui::crossterm::event::poll`/`read`: on unix, crossterm's `poll`
/// itself performs a `read(2)` on the fd into its own internal event
/// parser (to decide whether a full event is available), which would
/// silently consume the OSC/DA1 reply bytes before this function ever saw
/// them. Calling this before `EventStream::new()` exists (as `App::new`
/// does) also means crossterm's internal reader hasn't been constructed
/// yet, so there is no possibility of two readers racing over the same fd.
fn read_until_da1_or_deadline<S: Read + AsFd>(source: &mut S, deadline: Instant) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        // Poll in short slices (capped at the remaining deadline) rather
        // than one long wait, so a deadline shorter than the slice still
        // gets honored promptly.
        let wait = (deadline - now).min(Duration::from_millis(5));
        let Ok(timeout) = Timespec::try_from(wait) else {
            break;
        };

        let ready = {
            let mut fds = [PollFd::new(source, PollFlags::IN)];
            matches!(poll(&mut fds, Some(&timeout)), Ok(n) if n > 0)
                && fds[0].revents().contains(PollFlags::IN)
        };
        if !ready {
            continue; // nothing ready yet; keep polling until the deadline
        }

        match source.read(&mut byte) {
            Ok(0) => break, // EOF
            Ok(_) => {
                buf.push(byte[0]);
                if ends_with_da1(&buf) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    buf
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
        // Terminated by BEL ("\x07") or ST ("\x1b\\"); either way the ESC
        // that starts the terminator was already consumed as our split
        // delimiter above, so only a trailing BEL (never a trailing `\`)
        // can still be attached to `rest` here.
        let rest = rest.trim_end_matches('\x07');

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

    #[test]
    fn out_of_range_osc4_slot_is_ignored() {
        let q = parse_osc_response(b"\x1b]4;99;rgb:ffff/ffff/ffff\x07");
        assert!(q.ansi.iter().all(Option::is_none));
    }

    // --- read-loop tests: exercise `read_until_da1_or_deadline` end-to-end
    // against a real OS pipe (real `poll(2)`/`read(2)` syscalls), so the
    // layer that previously silently lost every reply to crossterm's
    // internal reader is actually covered, not just `parse_osc_response`.

    #[test]
    fn read_loop_recovers_a_reply_written_to_the_pipe() {
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        let reply = b"\x1b]11;rgb:1e1e/2a2a/3939\x07\x1b[?6c";
        writer.write_all(reply).unwrap();
        // The DA1 fence is already in `reply`, so the loop should break on
        // its own well before this deadline — this just bounds the test.
        let deadline = Instant::now() + Duration::from_secs(2);

        let buf = read_until_da1_or_deadline(&mut reader, deadline);

        assert_eq!(buf, reply);
        let q = parse_osc_response(&buf);
        assert_eq!(q.bg, Some((0x1e, 0x2a, 0x39)));
    }

    #[test]
    fn read_loop_recovers_a_reply_written_after_a_delay() {
        // Confirms the loop actually waits/polls for readiness rather than
        // only succeeding when bytes are already buffered before the first
        // poll — a delayed writer exercises the same `poll` return-to-loop
        // path a slow real terminal would.
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        let reply = b"\x1b]4;4;rgb:0101/7878/d4d4\x07\x1b[?6c".to_vec();
        let reply_for_thread = reply.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            writer.write_all(&reply_for_thread).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);

        let buf = read_until_da1_or_deadline(&mut reader, deadline);
        handle.join().unwrap();

        assert_eq!(buf, reply);
        let q = parse_osc_response(&buf);
        assert_eq!(q.ansi[4], Some((0x01, 0x78, 0xd4)));
    }

    #[test]
    fn fence_drain_swallows_bytes_ahead_of_the_da1_and_leaves_later_ones() {
        // Models teardown on a terminal that honours the mouse-disable late:
        // two SGR motion reports emitted before it got there, then its DA1
        // answer to our fence, then a keystroke typed after. Everything up
        // to and including the fence must be consumed (those bytes would
        // otherwise be typed at the shell); what follows is left for it.
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        writer
            .write_all(b"\x1b[<35;57;31M\x1b[<35;58;28M\x1b[?62;22c")
            .unwrap();
        let mut out = Vec::new();

        let drained = drain_until_da1_fence(
            &mut out,
            &mut reader,
            Instant::now() + Duration::from_secs(2),
        );

        assert_eq!(out, b"\x1b[c", "the fence query is what gets written");
        assert_eq!(drained, b"\x1b[<35;57;31M\x1b[<35;58;28M\x1b[?62;22c");
        writer.write_all(b"x").unwrap();
        let mut rest = [0u8; 8];
        let n = reader.read(&mut rest).unwrap();
        assert_eq!(&rest[..n], b"x", "bytes after the fence are not touched");
    }

    #[test]
    fn read_loop_returns_whatever_arrived_when_the_deadline_expires_first() {
        // A silent source (nothing written before the deadline) must not
        // hang — the loop returns (empty, here) once `deadline` passes.
        let (mut reader, _writer) = std::io::pipe().unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);

        let started = Instant::now();
        let buf = read_until_da1_or_deadline(&mut reader, deadline);
        let elapsed = started.elapsed();

        assert!(buf.is_empty());
        assert!(
            elapsed < Duration::from_millis(500),
            "deadline should be honored promptly, took {elapsed:?}"
        );
        let q = parse_osc_response(&buf);
        assert!(q.bg.is_none() && q.fg.is_none() && q.ansi.iter().all(Option::is_none));
    }
}
