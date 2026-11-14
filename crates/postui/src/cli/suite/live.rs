//! The live view: the suite's list drawn at a terminal as the run goes
//! (spec 2026-11-13 §5). Lines that are final print once and scroll away
//! like any output; the band under them (the first unfinished request down
//! to the end of the list, then the progress line) is redrawn in place by
//! moving the cursor up over it and clearing its lines. [`Board`] says which
//! lines are which, [`paint`](super::paint) makes them text. It never
//! enters raw mode, so keys typed during the run echo as in any command
//! (§5.5).

use super::super::read::warnings_text;
use super::board::Board;
use super::outline::Line;
use super::paint::{
    Badge, Seg, Tone, error_under, fit, header, line_row, name_column, paint, progress, totals,
};
use super::{Plan, SuiteReport, SuiteResult, Watch};
use crossterm::cursor::{Hide, MoveToNextLine, MoveToPreviousLine, Show};
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What the finish says of `-o`'s file.
pub(in crate::cli) enum FileNote {
    /// No `-o`, or it could not be written (a warning says so).
    None,
    Written(PathBuf),
    /// The run was cancelled, so the file was not written (spec §5.4).
    Cancelled(PathBuf),
}

/// The live view on `out`, a [`Watch`] of the run. Dropping it shows the
/// cursor again, so a panic or an early return doesn't leave it hidden.
pub(in crate::cli) struct Live<W: Write> {
    out: W,
    color: bool,
    /// The terminal's (columns, rows), read at each redraw so a resize is
    /// picked up (spec §5.2).
    size: Box<dyn Fn() -> (u16, u16)>,
    board: Board,
    name_col: usize,
    total: usize,
    /// When the run began, for the progress line's wall time.
    began: Instant,
    /// The spinner's frame.
    frame: usize,
    /// Lines the band took at the last redraw: how far up the next starts.
    drawn: usize,
    /// The cursor is hidden and is to be shown again.
    hidden: bool,
    /// Each list line printed for good, unpainted, in the order printed.
    #[cfg(test)]
    finals: Vec<String>,
}

impl<W: Write> Live<W> {
    pub(in crate::cli) fn new(out: W, color: bool, size: Box<dyn Fn() -> (u16, u16)>) -> Live<W> {
        Live {
            out,
            color,
            size,
            board: Board::new(Vec::new()),
            name_col: 0,
            total: 0,
            began: Instant::now(),
            frame: 0,
            drawn: 0,
            hidden: false,
            #[cfg(test)]
            finals: Vec::new(),
        }
    }

    /// The lines the band may use: the terminal's height less the progress
    /// line, the blank line over it, and the one line the cursor sits on
    /// after the band.
    fn budget(&self) -> usize {
        usize::from((self.size)().1).saturating_sub(3)
    }

    /// Ends the view: a cancelled run's unfinished rows are skipped, every
    /// line still in the band prints for good, then the totals line, what
    /// became of `-o`'s file, the warnings, and the cursor comes back
    /// (spec §5.3, §5.4).
    pub(in crate::cli) fn finish(&mut self, report: &SuiteReport, file: FileNote) {
        let cancelled = report.cancelled();
        if cancelled {
            self.board.skip_unfinished();
        }
        let counts = self.board.counts();
        let badge = if cancelled {
            Badge::Cancelled
        } else if counts.failed == 0 {
            Badge::Ok
        } else {
            Badge::Fail
        };
        let end = totals(
            badge,
            self.total,
            counts.passed,
            counts.failed,
            counts.skipped,
            Duration::from_millis(report.totals.elapsed_ms),
        );
        self.redraw(Some(&end));
        let mut text = match file {
            FileNote::None => String::new(),
            FileNote::Written(path) => format!("written to {}\n", path.display()),
            FileNote::Cancelled(path) => format!("not written to {}: cancelled\n", path.display()),
        };
        text.push_str(&warnings_text(&report.warnings));
        let mut buf = text.into_bytes();
        if self.hidden {
            let _ = queue!(buf, Show);
            self.hidden = false;
        }
        self.write(&buf);
    }

    /// Line `at` of the list as segments.
    fn row(&self, at: usize, now: Instant) -> Vec<Seg> {
        let line = &self.board.lines()[at];
        let state = match line {
            Line::Request { index, .. } => Some(self.board.state(*index)),
            Line::Space { .. } | Line::Folder { .. } => None,
        };
        line_row(line, state, self.name_col, now, self.frame)
    }

    /// Wipes the band, prints the lines that just became final, whole and
    /// with a `!` row's further lines under it, then draws the band again,
    /// or `end` in its place when the run is over.
    fn redraw(&mut self, end: Option<&[Seg]>) {
        let mut buf = Vec::new();
        wipe(&mut buf, self.drawn);
        let now = Instant::now();
        // Before the band (R14): the band and `may_start` count from the
        // mark this moves.
        for at in self.board.take_final() {
            let segs = self.row(at, now);
            put(&mut buf, &paint(&segs, self.color));
            if let Line::Request { index, depth, .. } = &self.board.lines()[at] {
                for under in error_under(self.board.state(*index), *depth) {
                    let under = [Seg { text: under, tone: Some(Tone::NotSent), shrink: 0 }];
                    put(&mut buf, &paint(&under, self.color));
                }
            }
            #[cfg(test)]
            self.finals.push(paint(&segs, false));
        }
        self.drawn = match end {
            Some(end) => {
                put(&mut buf, "");
                put(&mut buf, &paint(end, self.color));
                0
            }
            None => self.band(&mut buf, now),
        };
        self.write(&buf);
    }

    /// Draws the band into `buf`, each line clipped to the terminal's width
    /// so it takes one line, and gives how many lines it took: never more
    /// than the terminal is high less one, or the next redraw couldn't
    /// reach its top (spec §5.2).
    fn band(&self, buf: &mut Vec<u8>, now: Instant) -> usize {
        let (cols, rows) = (self.size)();
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        let fitted = |mut segs: Vec<Seg>| {
            fit(&mut segs, cols);
            paint(&segs, self.color)
        };
        let (range, more) = self.board.band(rows.saturating_sub(3));
        let mut lines: Vec<String> = range.map(|at| fitted(self.row(at, now))).collect();
        let mut more = (more > 0).then(|| {
            fitted(vec![Seg { text: format!("… {more} more"), tone: Some(Tone::Dim), shrink: 0 }])
        });
        let mut blank = true;
        // The progress line.
        let counts = self.board.counts();
        let mut under = Some(fitted(progress(
            counts.done,
            self.total,
            counts.passed,
            counts.failed,
            now.saturating_duration_since(self.began),
        )));
        // R13: on a terminal too short for it all, the blank line gives
        // way first, then the progress line, then `… N more`, then the
        // band's own lines past the room (one is always drawn).
        let room = rows.saturating_sub(1).max(1);
        let height =
            |lines: &[String], more: &Option<String>, blank: bool, under: &Option<String>| {
                lines.len()
                    + usize::from(more.is_some())
                    + usize::from(blank)
                    + usize::from(under.is_some())
            };
        if height(&lines, &more, blank, &under) > room {
            blank = false;
        }
        if height(&lines, &more, blank, &under) > room {
            under = None;
        }
        if height(&lines, &more, blank, &under) > room {
            more = None;
        }
        lines.truncate(room);
        let drawn = height(&lines, &more, blank, &under);
        for line in lines.iter().chain(&more) {
            put(buf, line);
        }
        if blank {
            put(buf, "");
        }
        if let Some(under) = &under {
            put(buf, under);
        }
        drawn
    }

    /// Writes `buf` out in one go. The view has nowhere to report a failed
    /// write to the terminal, so it carries on; the run's result stands.
    fn write(&mut self, buf: &[u8]) {
        let _ = self.out.write_all(buf);
        let _ = self.out.flush();
    }
}

/// Wipes the `up` lines above the cursor, and the line it is on, and leaves
/// the cursor where the first of them was. A line at a time, not one clear to
/// the end of the screen: from the top-left corner, where a band as high as
/// the screen starts, tmux (`scroll-on-clear`) takes that for clearing the
/// screen and pushes it into scrollback first, leaving a copy of the band
/// there. The picker wipes its form the same way.
pub(super) fn wipe(buf: &mut Vec<u8>, up: usize) {
    buf.push(b'\r');
    let up = u16::try_from(up).unwrap_or(u16::MAX);
    if up > 0 {
        let _ = queue!(buf, MoveToPreviousLine(up));
        for _ in 0..up {
            let _ = queue!(buf, Clear(ClearType::CurrentLine), MoveToNextLine(1));
        }
    }
    let _ = queue!(buf, Clear(ClearType::CurrentLine));
    if up > 0 {
        let _ = queue!(buf, MoveToPreviousLine(up));
    }
}

/// `line` and the line end the view uses.
pub(super) fn put(buf: &mut Vec<u8>, line: &str) {
    buf.extend_from_slice(line.as_bytes());
    buf.extend_from_slice(b"\r\n");
}

impl<W: Write> Watch for Live<W> {
    /// The header and a blank line, and the cursor hidden for the run.
    fn begin(&mut self, plan: &Plan) {
        self.board = Board::new(plan.lines.clone());
        self.name_col = name_column(&plan.lines);
        self.total = plan.total;
        self.began = Instant::now();
        let mut buf = Vec::new();
        put(&mut buf, &paint(&header(plan), self.color));
        put(&mut buf, "");
        let _ = queue!(buf, Hide);
        self.hidden = true;
        self.write(&buf);
    }

    fn may_start(&self, index: usize) -> bool {
        self.board.may_start(index, self.budget())
    }

    fn started(&mut self, index: usize, url: Option<&str>) {
        self.board.start(index, Instant::now(), url.map(str::to_string));
    }

    /// Redrawn at once (R14), so the final mark is current when the run
    /// asks [`Watch::may_start`] next. A request that never went out is
    /// finished without having started (R15).
    fn finished(&mut self, index: usize, result: &SuiteResult) {
        self.board.finish(index, result.clone());
        self.redraw(None);
    }

    fn ticks(&self) -> bool {
        true
    }

    fn tick(&mut self) {
        self.frame += 1;
        self.redraw(None);
    }
}

impl<W: Write> Drop for Live<W> {
    fn drop(&mut self) {
        if self.hidden {
            let _ = queue!(self.out, Show);
            let _ = self.out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::suite::Totals;
    use postui_core::model::Method;
    use std::collections::VecDeque;

    /// Request `index`'s name: fixed width, so no name holds another.
    fn name(index: usize) -> String {
        format!("req-{index:02}")
    }

    /// A space line, then `n` requests.
    fn lines(n: usize) -> Vec<Line> {
        let requests = (0..n).map(|index| Line::Request {
            depth: 1,
            index,
            method: Some(Method::Get),
            name: name(index),
        });
        std::iter::once(Line::Space { name: "main".into() }).chain(requests).collect()
    }

    fn plan(n: usize, jobs: usize) -> Plan {
        Plan { env: None, space: None, changed: Vec::new(), jobs, lines: lines(n), total: n }
    }

    fn ok(index: usize) -> SuiteResult {
        SuiteResult {
            request: format!("main/{}", name(index)),
            method: Some(Method::Get),
            url: Some(format!("http://x/{index}")),
            status: Some(200),
            elapsed_ms: Some(120),
            size: Some(512),
            error: None,
        }
    }

    fn report(results: Vec<SuiteResult>, cancelled: bool, warnings: Vec<String>) -> SuiteReport {
        let passed = results.iter().filter(|r| r.passed()).count();
        SuiteReport {
            environment: None,
            space: None,
            jobs: 1,
            totals: Totals {
                requests: results.len(),
                passed,
                failed: results.len() - passed,
                elapsed_ms: 0,
            },
            results,
            warnings,
            output_file: None,
            write_failed: false,
            cancelled,
        }
    }

    fn live(out: &mut Vec<u8>, color: bool, cols: u16, rows: u16) -> Live<&mut Vec<u8>> {
        Live::new(out, color, Box::new(move || (cols, rows)))
    }

    /// The finals line of request `index` that passed, as `ok` gives it.
    fn passed_row(index: usize) -> String {
        format!("  ✓ GET    {}  200  120 ms   512 B  http://x/{index}", name(index))
    }

    /// Runs `n` requests as the run loop does, up to `jobs` at once while
    /// the live view lets them start, finishing the newest in flight and
    /// the oldest in turn; `after` is told after every event.
    fn drive(
        live: &mut Live<&mut Vec<u8>>,
        n: usize,
        jobs: usize,
        after: &mut dyn FnMut(&Live<&mut Vec<u8>>),
    ) -> Vec<SuiteResult> {
        let mut flight = VecDeque::new();
        let mut next = 0;
        let mut results = Vec::new();
        loop {
            while flight.len() < jobs && next < n && (flight.is_empty() || live.may_start(next)) {
                live.started(next, Some(&format!("http://x/{next}")));
                flight.push_back(next);
                next += 1;
                after(live);
            }
            let newest_first = results.len() % 2 == 0;
            let Some(index) = (if newest_first { flight.pop_back() } else { flight.pop_front() })
            else {
                break;
            };
            live.tick();
            after(live);
            live.finished(index, &ok(index));
            results.push(ok(index));
            after(live);
        }
        results.sort_by(|a, b| a.request.cmp(&b.request));
        results
    }

    /// Each line once, in list order, unpainted.
    fn each_line_once(finals: &[String], n: usize) {
        assert_eq!(finals.len(), n + 1, "{finals:#?}");
        assert_eq!(finals[0], "main");
        for index in 0..n {
            assert!(finals[index + 1].contains(&name(index)), "{index}: {finals:#?}");
        }
    }

    /// Spec 2026-11-13 §5.2, §5.3: rows print for good in sidebar order
    /// whatever order they finished in, each once; the finish ends with the
    /// totals and gives the cursor back.
    #[test]
    fn live_prints_every_row_once_in_order_by_the_finish() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 24);
        live.begin(&plan(3, 3));
        for index in 0..3 {
            live.started(index, Some(&format!("http://x/{index}")));
        }
        for index in [2, 0, 1] {
            live.finished(index, &ok(index));
        }
        live.finish(&report((0..3).map(ok).collect(), false, Vec::new()), FileNote::None);
        let finals = live.finals.clone();
        drop(live);
        assert_eq!(finals, ["main".to_string(), passed_row(0), passed_row(1), passed_row(2)]);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.ends_with("  OK   3 requests · 3 passed · 0 failed · 0 ms\r\n\x1b[?25h"),
            "{text:?}"
        );
        assert!(text.starts_with("postui suite  env (none)  space all · 3 requests · 3 at a time"));
        // Hidden once, shown once: dropping after the finish adds nothing.
        assert_eq!(text.matches("\x1b[?25l").count(), 1);
        assert_eq!(text.matches("\x1b[?25h").count(), 1);
    }

    /// Spec 2026-11-13 §5.4: a cancelled run's unfinished rows are skipped,
    /// the badge says so, and the finish says `-o` was not written, then the
    /// warnings.
    #[test]
    fn live_finish_says_a_cancelled_file_was_not_written() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 24);
        live.begin(&plan(3, 1));
        live.started(0, Some("http://x/0"));
        live.finished(0, &ok(0));
        live.started(1, Some("http://x/1"));
        let report = report(vec![ok(0)], true, vec!["a warning".into()]);
        live.finish(&report, FileNote::Cancelled(PathBuf::from("/tmp/r.txt")));
        let finals = live.finals.clone();
        drop(live);
        each_line_once(&finals, 3);
        assert_eq!(finals[2], "  – GET    req-01  skipped");
        assert_eq!(finals[3], "  – GET    req-02  skipped");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(" CANCELLED "), "{text:?}");
        assert!(text.contains("3 requests · 1 passed · 0 failed · 2 skipped"), "{text:?}");
        let note = text.find("not written to /tmp/r.txt: cancelled\n").expect(&text);
        let warnings = text.find("\nwarnings\n  a warning\n").expect(&text);
        assert!(note < warnings);
        assert!(!text.contains("written to /tmp/r.txt\n"));
    }

    /// `written to <file>` after a run that wrote `-o`.
    #[test]
    fn live_finish_names_the_written_file() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 24);
        live.begin(&plan(0, 1));
        live.finish(&report(Vec::new(), false, Vec::new()), FileNote::Written("/tmp/r.txt".into()));
        drop(live);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains(
                "  OK   0 requests · 0 passed · 0 failed · 0 ms\r\nwritten to /tmp/r.txt\n"
            )
        );
    }

    /// Review focus 5: an empty suite prints the header, its space line and
    /// ` OK  0 requests`.
    #[test]
    fn an_empty_suite_is_ok_with_nothing() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 24);
        live.begin(&plan(0, 1));
        live.finish(&report(Vec::new(), false, Vec::new()), FileNote::None);
        assert_eq!(live.finals, ["main"]);
        drop(live);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("main\r\n\r\n  OK   0 requests"), "{text:?}");
    }

    /// Spec 2026-11-13 §5.2: the band never takes more lines than the
    /// terminal is high minus one, so the redraw can always reach its top.
    #[test]
    fn live_never_draws_taller_than_the_screen() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 6);
        live.begin(&plan(20, 8));
        let mut most = 0;
        let results = drive(&mut live, 20, 8, &mut |live| {
            assert!(live.drawn <= 5, "{}", live.drawn);
            most = most.max(live.drawn);
        });
        assert!(most > 0);
        live.finish(&report(results, false, Vec::new()), FileNote::None);
        each_line_once(&live.finals, 20);
    }

    /// R13: on a terminal of 4, 3 or 2 rows the blank line, then the
    /// progress line, then `… N more` give way, then band rows; one request
    /// still runs, and every row still prints for good once.
    #[test]
    fn a_tiny_terminal_still_draws_within_it() {
        for rows in [4, 3, 2] {
            let mut out = Vec::new();
            let mut live = live(&mut out, false, 80, rows);
            live.begin(&plan(20, 8));
            let room = usize::from(rows) - 1;
            let results = drive(&mut live, 20, 8, &mut |live| {
                assert!(live.drawn <= room, "{rows} rows: drew {}", live.drawn);
            });
            live.finish(&report(results, false, Vec::new()), FileNote::None);
            each_line_once(&live.finals, 20);
        }
    }

    /// Every band line is one line however narrow the terminal (review
    /// focus 1): nothing wider than the columns is drawn in the band.
    #[test]
    fn band_lines_fit_a_narrow_terminal() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 20, 24);
        live.begin(&plan(3, 3));
        for index in 0..3 {
            live.started(index, Some("https://example.com/a/very/long/path"));
        }
        live.finished(1, &ok(1));
        drop(live);
        let text = String::from_utf8(out).unwrap();
        // The band is what follows the last wipe: its last line clear, then
        // the move back up over the wiped lines.
        let rest = text.rsplit("\x1b[2K").next().unwrap();
        let band =
            rest.strip_prefix("\x1b[").map_or(rest, |code| &code[code.find('F').unwrap() + 1..]);
        assert!(band.contains("1 of 3"), "{band:?}");
        for line in band.split("\r\n") {
            assert!(crate::ui::cells(line) <= 20, "{line:?}");
        }
    }

    /// The band is wiped a line at a time, never with a clear to the end of
    /// the screen: a band as high as the screen starts in its top-left
    /// corner, and tmux pushes a screen cleared from there into scrollback
    /// (`scroll-on-clear`), leaving a copy of the band behind.
    #[test]
    fn the_band_is_wiped_line_by_line() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 6);
        live.begin(&plan(20, 1));
        live.started(0, None);
        live.tick();
        assert_eq!(live.drawn, 5);
        let from = live.out.len();
        live.tick();
        let wipe = format!("\r\x1b[5F{}\x1b[2K\x1b[5F", "\x1b[2K\x1b[1E".repeat(5));
        assert!(live.out[from..].starts_with(wipe.as_bytes()), "{:?}", &live.out[from..]);
        live.finished(0, &ok(0));
        live.finish(&report(vec![ok(0)], true, Vec::new()), FileNote::None);
        drop(live);
        assert!(!String::from_utf8(out).unwrap().contains("\x1b[J"));
    }

    /// R15: a request refused before it went out is finished without ever
    /// starting; its row is final like any other.
    #[test]
    fn a_row_finished_without_starting_is_final() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 24);
        live.begin(&plan(2, 1));
        let mut refused = ok(0);
        (refused.status, refused.elapsed_ms, refused.size) = (None, None, None);
        refused.error = Some("{{id}} has no value\nsecond line".into());
        live.finished(0, &refused);
        assert_eq!(live.finals, ["main", "  ! GET    req-00  {{id}} has no value"]);
        drop(live);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("{{id}} has no value\r\n      second line\r\n"), "{text:?}");
    }

    /// Spec 2026-11-13 §4: without color the live view writes no SGR code,
    /// only the cursor's; with it, the rows are colored.
    #[test]
    fn no_color_writes_only_cursor_codes() {
        let run = |color: bool| {
            let mut out = Vec::new();
            let mut live = live(&mut out, color, 80, 24);
            live.begin(&plan(2, 2));
            live.started(0, Some("http://x/0"));
            live.started(1, Some("http://x/1"));
            live.tick();
            live.finished(1, &ok(1));
            live.finished(0, &ok(0));
            live.finish(&report(vec![ok(0), ok(1)], false, Vec::new()), FileNote::None);
            drop(live);
            String::from_utf8(out).unwrap()
        };
        let plain = run(false);
        // Hide and show the cursor, move it up or down lines, clear a line.
        for code in plain.split("\x1b[").skip(1) {
            let rest = code.trim_start_matches(|c: char| c.is_ascii_digit());
            assert!(code.starts_with("?25") || rest.starts_with(['F', 'E', 'K']), "{code:?}");
        }
        assert!(run(true).contains("\x1b[32m✓\x1b[0m"));
    }

    /// Spec 2026-11-13 §5.5: the cursor comes back however the view ends.
    #[test]
    fn dropping_mid_run_shows_the_cursor() {
        let mut out = Vec::new();
        let mut live = live(&mut out, false, 80, 24);
        live.begin(&plan(2, 1));
        live.started(0, None);
        live.tick();
        drop(live);
        let text = String::from_utf8(out).unwrap();
        assert!(text.ends_with("\x1b[?25h"), "{text:?}");
    }
}
