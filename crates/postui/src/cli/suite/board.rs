//! The live view's model: which lines of the suite's list are final, which
//! are in the band that is redrawn, and when a new request may start (spec
//! 2026-11-13 §5.2). It draws nothing; `live` asks it what to draw.
//!
//! Every budget is the number of lines the band may use, its own `… N more`
//! line counted inside it.

use super::SuiteResult;
use super::outline::Line;
use super::paint::RowState;
use std::ops::Range;
use std::time::Instant;

/// How the finished requests came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Counts {
    /// Requests with a result (passed or failed), not counting skipped.
    pub done: usize,
    pub passed: usize,
    /// Done and not passed, a request that never went out included.
    pub failed: usize,
    pub skipped: usize,
}

pub(super) struct Board {
    lines: Vec<Line>,
    /// Each request's state, by request index.
    states: Vec<RowState>,
    /// The line each request is on, by request index.
    row_of: Vec<usize>,
    /// Lines before this one are final: printed for good.
    flushed: usize,
    /// The newest request started (they start in order), if any.
    newest: Option<usize>,
}

impl Board {
    pub(super) fn new(lines: Vec<Line>) -> Board {
        let mut row_of = Vec::new();
        for (at, line) in lines.iter().enumerate() {
            if let Line::Request { index, .. } = line {
                debug_assert_eq!(*index, row_of.len(), "requests are numbered in order");
                row_of.push(at);
            }
        }
        let states = row_of.iter().map(|_| RowState::Waiting).collect();
        Board { lines, states, row_of, flushed: 0, newest: None }
    }

    pub(super) fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub(super) fn state(&self, index: usize) -> &RowState {
        &self.states[index]
    }

    /// Request `index` is in flight; `url` is its masked URL when it has one.
    pub(super) fn start(&mut self, index: usize, now: Instant, url: Option<String>) {
        self.states[index] = RowState::Running { since: now, url };
        self.newest = self.newest.max(Some(index));
    }

    pub(super) fn finish(&mut self, index: usize, result: SuiteResult) {
        self.states[index] = RowState::Done(result);
        self.newest = self.newest.max(Some(index));
    }

    /// The run ended early: everything not finished is skipped.
    pub(super) fn skip_unfinished(&mut self) {
        for state in &mut self.states {
            if matches!(state, RowState::Waiting | RowState::Running { .. }) {
                *state = RowState::Skipped;
            }
        }
    }

    /// The lines that just became final: from the last flushed line up to
    /// the first line holding a request not yet done or skipped, or to the
    /// end when there is none. Moves the flushed mark forward.
    pub(super) fn take_final(&mut self) -> Range<usize> {
        let open = self
            .states
            .iter()
            .position(|s| !matches!(s, RowState::Done(_) | RowState::Skipped))
            .map_or(self.lines.len(), |index| self.row_of[index]);
        let from = self.flushed;
        self.flushed = self.flushed.max(open);
        from..self.flushed
    }

    /// The lines to draw, from the flushed mark, and how many waiting
    /// request lines were left out. Every line through the newest started
    /// request's is in; after that lines are added while the count stays
    /// within `budget`, one line kept back for `… N more` whenever a request
    /// line would be left out.
    pub(super) fn band(&self, budget: usize) -> (Range<usize>, usize) {
        let from = self.flushed;
        let all = self.lines.len();
        let required = self.newest.map_or(from, |index| self.row_of[index] + 1).max(from);
        let left_out = |end: usize| self.lines[end..].iter().filter(|l| is_request(l)).count();
        if all - from <= budget {
            return (from..all, 0);
        }
        let end = required.max(from + budget.saturating_sub(1)).min(all);
        let more = left_out(end);
        if more == 0 {
            // Only headers are left over: no `… N more` line to keep room for.
            return (from..required.max(from + budget).min(all), 0);
        }
        (from..end, more)
    }

    /// Whether request `index` may start with a band of `budget` lines:
    /// always when nothing is in flight (one always runs), else only while
    /// the band through its line, and the progress line's share, still fits.
    pub(super) fn may_start(&self, index: usize, budget: usize) -> bool {
        let running = self.states.iter().any(|s| matches!(s, RowState::Running { .. }));
        !running || self.row_of[index] + 1 - self.flushed < budget
    }

    pub(super) fn counts(&self) -> Counts {
        let mut counts = Counts { done: 0, passed: 0, failed: 0, skipped: 0 };
        for state in &self.states {
            match state {
                RowState::Done(r) => {
                    counts.done += 1;
                    if r.passed() {
                        counts.passed += 1;
                    } else {
                        counts.failed += 1;
                    }
                }
                RowState::Skipped => counts.skipped += 1,
                RowState::Waiting | RowState::Running { .. } => {}
            }
        }
        counts
    }
}

fn is_request(line: &Line) -> bool {
    matches!(line, Line::Request { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postui_core::model::Method;

    fn request(index: usize) -> Line {
        Line::Request { depth: 1, index, method: Some(Method::Get), name: format!("r{index}") }
    }

    /// `n` requests and nothing else.
    fn requests(n: usize) -> Board {
        Board::new((0..n).map(request).collect())
    }

    fn result(status: Option<u16>) -> SuiteResult {
        SuiteResult {
            request: "x".into(),
            method: Some(Method::Get),
            url: None,
            status,
            elapsed_ms: None,
            size: None,
            error: None,
        }
    }

    fn run(board: &mut Board, index: usize) {
        board.start(index, Instant::now(), None);
    }

    /// Spec 2026-11-13 §5.2: everything above the first unfinished request
    /// is final, and is handed out once.
    #[test]
    fn rows_above_the_first_unfinished_are_final() {
        let mut board = Board::new(vec![
            Line::Space { name: "main".into() },
            request(0),
            request(1),
            request(2),
        ]);
        run(&mut board, 0);
        run(&mut board, 1);
        board.finish(1, result(Some(200)));
        assert_eq!(board.take_final(), 0..1);
        board.finish(0, result(Some(200)));
        assert_eq!(board.take_final(), 1..3);
        assert!(board.take_final().is_empty());
        run(&mut board, 2);
        board.finish(2, result(Some(200)));
        assert_eq!(board.take_final(), 3..4);
    }

    /// A slow request holds the finished rows after it in the band.
    #[test]
    fn out_of_order_finishes_wait_in_the_band() {
        let mut board = requests(3);
        for index in 0..3 {
            run(&mut board, index);
        }
        board.finish(2, result(Some(200)));
        board.finish(1, result(Some(200)));
        assert!(board.take_final().is_empty());
        assert_eq!(board.band(10), (0..3, 0));
        board.finish(0, result(Some(200)));
        assert_eq!(board.take_final(), 0..3);
    }

    /// Waiting rows that don't fit are left out, one line kept for
    /// `… N more`.
    #[test]
    fn band_leaves_out_waiting_rows_that_dont_fit() {
        let mut board = requests(10);
        run(&mut board, 0);
        assert_eq!(board.band(4), (0..3, 7));
        // Everything fits: nothing is left out, no `… N more` line.
        assert_eq!(board.band(10), (0..10, 0));
        // The band starts at the flushed mark.
        board.finish(0, result(Some(200)));
        assert_eq!(board.take_final(), 0..1);
        assert_eq!(board.band(4), (1..4, 6));
    }

    /// Lines through the newest started request are always in, whatever the
    /// budget says.
    #[test]
    fn the_band_always_holds_the_started_rows() {
        let mut board = requests(6);
        for index in 0..4 {
            run(&mut board, index);
        }
        assert_eq!(board.band(2), (0..4, 2));
    }

    /// Trailing headers with no request after them are not "more".
    #[test]
    fn a_trailing_empty_space_is_not_more() {
        let mut board =
            Board::new(vec![request(0), request(1), Line::Space { name: "empty".into() }]);
        run(&mut board, 0);
        assert_eq!(board.band(3), (0..3, 0));
        // Too tight for the second request: it is the one "more".
        assert_eq!(board.band(2), (0..1, 1));
        // Only headers beyond the band: no line is kept back for "more".
        let mut board = Board::new(vec![
            request(0),
            Line::Space { name: "a".into() },
            Line::Space { name: "b".into() },
        ]);
        run(&mut board, 0);
        assert_eq!(board.band(2), (0..2, 0));
    }

    /// The band's budget, plus the progress line, holds a new start back.
    #[test]
    fn the_throttle_holds_new_starts_at_the_budget() {
        let mut board = requests(3);
        run(&mut board, 0);
        assert!(board.may_start(1, 3));
        run(&mut board, 1);
        board.finish(1, result(Some(200)));
        assert!(!board.may_start(2, 3));
        assert!(board.may_start(2, 4));
        // Once 0 lands nothing is in flight: one always runs.
        board.finish(0, result(Some(200)));
        assert!(board.may_start(2, 0));
    }

    /// Review focus 3: a 5-row terminal leaves a budget of 2 or less; one
    /// request still runs.
    #[test]
    fn a_short_terminal_still_runs_one() {
        let mut board = requests(4);
        assert!(board.may_start(0, 0));
        run(&mut board, 0);
        assert!(!board.may_start(1, 0));
        assert!(!board.may_start(1, 2));
        board.finish(0, result(Some(200)));
        assert!(board.may_start(1, 0));
    }

    /// Ctrl+C before anything started: every request is skipped and the
    /// whole list is final.
    #[test]
    fn skip_with_nothing_started() {
        let mut board =
            Board::new(vec![Line::Space { name: "main".into() }, request(0), request(1)]);
        board.skip_unfinished();
        assert!(matches!(board.state(0), RowState::Skipped));
        assert_eq!(board.take_final(), 0..3);
        let counts = board.counts();
        assert_eq!((counts.skipped, counts.done), (2, 0));
    }

    /// Running rows are skipped too; finished ones stay as they are.
    #[test]
    fn skipping_keeps_what_finished() {
        let mut board = requests(3);
        run(&mut board, 0);
        run(&mut board, 1);
        board.finish(0, result(Some(200)));
        board.skip_unfinished();
        assert!(matches!(board.state(0), RowState::Done(_)));
        assert!(matches!(board.state(1), RowState::Skipped));
        assert!(matches!(board.state(2), RowState::Skipped));
        assert_eq!(board.take_final(), 0..3);
    }

    /// Review focus 5: an empty suite has nothing to hold back.
    #[test]
    fn an_empty_board_is_all_final() {
        let mut board = Board::new(vec![]);
        assert_eq!(board.take_final(), 0..0);
        assert_eq!(board.band(5), (0..0, 0));
        assert!(board.lines().is_empty());
    }

    #[test]
    fn counts_track_passes_and_failures() {
        let mut board = requests(4);
        for (index, status) in [(0, Some(200)), (1, Some(404)), (2, None)] {
            run(&mut board, index);
            board.finish(index, result(status));
        }
        let counts = board.counts();
        assert_eq!((counts.done, counts.passed, counts.failed, counts.skipped), (3, 1, 2, 0));
    }

    /// R11: a running row keeps the URL it was started with.
    #[test]
    fn a_started_row_keeps_its_url() {
        let mut board = requests(1);
        board.start(0, Instant::now(), Some("https://x/y".into()));
        assert!(
            matches!(board.state(0), RowState::Running { url: Some(u), .. } if u == "https://x/y")
        );
    }
}
