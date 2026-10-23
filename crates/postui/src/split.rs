//! The main column's editor/response split state and the five-stop
//! split control's state transitions.
//!
//! The editor's share of the column is a free `f32`. Five of its values
//! are named stops, in the keyboard cycle: 100/0 (response minimized to
//! its header strip), 75/25, 50/50, 25/75, and 0/100 (editor minimized
//! to its address-bar strip). The endpoint states keep living in the two
//! existing flags (`App::table_collapsed` and
//! `session.response.collapsed`); this module adds the ratio between
//! them and the direct stop → state function behind the control's two
//! end chips (see [`crate::paint::SplitControl`]), which live in the
//! editor pane's fixed tab-bar row so they never move out from under the
//! pointer.

/// One of the five named split stops, named by the editor's share —
/// and one chip of the split control, in on-screen order (the boundary
/// slides down left to right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitStop {
    /// 100/0 — response minimized to its header strip.
    EditorFull,
    /// 75/25.
    EditorBig,
    /// 50/50.
    Even,
    /// 25/75.
    ResponseBig,
    /// 0/100 — editor minimized to its address-bar strip.
    ResponseFull,
}

impl SplitStop {
    /// Every stop in on-screen (editor share descending) order.
    pub const ALL: [SplitStop; 5] = [
        SplitStop::EditorFull,
        SplitStop::EditorBig,
        SplitStop::Even,
        SplitStop::ResponseBig,
        SplitStop::ResponseFull,
    ];

    /// The editor's share of the column at this stop.
    pub fn share(self) -> f32 {
        match self {
            SplitStop::EditorFull => 1.0,
            SplitStop::EditorBig => 0.75,
            SplitStop::Even => 0.5,
            SplitStop::ResponseBig => 0.25,
            SplitStop::ResponseFull => 0.0,
        }
    }

    /// The next stop in on-screen order, wrapping — the keyboard cycle
    /// behind `Action::CycleSplit`: one key walks the boundary down the
    /// column and back around to the top.
    pub fn next(self) -> SplitStop {
        let i = SplitStop::ALL.iter().position(|s| *s == self).unwrap();
        SplitStop::ALL[(i + 1) % SplitStop::ALL.len()]
    }

    /// The previous stop in on-screen order, wrapping — `next`'s mirror,
    /// behind `Action::CycleSplitBack` (shift+alt+w).
    pub fn prev(self) -> SplitStop {
        let i = SplitStop::ALL.iter().position(|s| *s == self).unwrap();
        SplitStop::ALL[(i + SplitStop::ALL.len() - 1) % SplitStop::ALL.len()]
    }
}

/// How close a share must sit to a stop's to count as being at it.
const STOP_TOLERANCE: f32 = 1e-3;

/// The whole split state: the two minimized endpoints plus the editor's
/// share the column returns to while both panes are visible. At most one
/// of the minimized flags is set; `ratio` is sticky through the endpoint
/// stops so the collapse animation eases from the boundary the column
/// actually held, and a restore brings that share back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitState {
    pub editor_minimized: bool,
    pub response_minimized: bool,
    /// The editor's share of the content rows while both panes show.
    pub ratio: f32,
}

impl Default for SplitState {
    fn default() -> Self {
        SplitState { editor_minimized: false, response_minimized: false, ratio: 0.5 }
    }
}

impl SplitState {
    /// The stop this state sits at — the control's lit chip — or `None`
    /// when a dragged share sits between stops. The end stops are the
    /// minimized flags, never a share: a free share at 1.0 or 0.0 (a
    /// hand-written token) still shows both panes, so lighting an end chip
    /// for it would offer "restore" where its click minimizes.
    pub fn stop(self) -> Option<SplitStop> {
        if self.response_minimized {
            return Some(SplitStop::EditorFull);
        }
        if self.editor_minimized {
            return Some(SplitStop::ResponseFull);
        }
        [SplitStop::EditorBig, SplitStop::Even, SplitStop::ResponseBig]
            .into_iter()
            .find(|s| (s.share() - self.ratio).abs() < STOP_TOLERANCE)
    }

    /// The state after clicking `stop`'s chip: a direct jump — every
    /// stop is reachable from every state in one click. The middle stops
    /// set the ratio; an end stop minimizes and keeps it sticky. Clicking
    /// the end that is already lit restores the split instead (both flags
    /// clear, the ratio as it was).
    pub fn apply(self, stop: SplitStop) -> SplitState {
        match stop {
            SplitStop::EditorFull => SplitState {
                editor_minimized: false,
                response_minimized: !self.response_minimized,
                ratio: self.ratio,
            },
            SplitStop::ResponseFull => SplitState {
                editor_minimized: !self.editor_minimized,
                response_minimized: false,
                ratio: self.ratio,
            },
            middle => SplitState {
                editor_minimized: false,
                response_minimized: false,
                ratio: middle.share(),
            },
        }
    }

    /// The state after one keyboard cycle press. At a stop, the next (or
    /// previous) stop, wrapping. Between stops, the nearest stop in that
    /// direction: forward moves the boundary down the column, so it takes
    /// the first stop with a smaller share than where the column sits.
    pub fn cycled(self, forward: bool) -> SplitState {
        let target = match self.stop() {
            Some(stop) if forward => stop.next(),
            Some(stop) => stop.prev(),
            None => {
                // Only an unflagged state gets here (`stop` names every
                // flagged one), and its minimized ends lie beyond any free
                // share, even one of 1.0 or 0.0: rank them past the range
                // so either way always finds a stop.
                let rank = |s: SplitStop| match s {
                    SplitStop::EditorFull => f32::INFINITY,
                    SplitStop::ResponseFull => f32::NEG_INFINITY,
                    middle => middle.share(),
                };
                let found = if forward {
                    SplitStop::ALL.into_iter().find(|s| rank(*s) < self.ratio)
                } else {
                    SplitStop::ALL.into_iter().rev().find(|s| rank(*s) > self.ratio)
                };
                found.expect("a free share has a minimized end each way")
            }
        };
        self.apply(target)
    }

    /// This state as the stable token stored in the project's
    /// `.local/state.toml` (`LocalState::main_split`): the editor's exact
    /// share when both panes show, else which pane is minimized plus the
    /// share it restores to.
    pub fn to_token(self) -> String {
        if self.response_minimized {
            format!("editor-full@{:.4}", self.ratio)
        } else if self.editor_minimized {
            format!("response-full@{:.4}", self.ratio)
        } else {
            format!("{:.4}", self.ratio)
        }
    }

    /// Parses [`Self::to_token`]'s output and the older stop names
    /// (`even`, `editor-big`, `response-big`, `editor-full`,
    /// `response-full`); `None` for anything else, so a hand-edited or
    /// future token degrades to the default split.
    pub fn from_token(token: &str) -> Option<SplitState> {
        let mut s = SplitState::default();
        match token {
            "even" => return Some(s),
            "editor-big" => s.ratio = 0.75,
            "response-big" => s.ratio = 0.25,
            "editor-full" => s.response_minimized = true,
            "response-full" => s.editor_minimized = true,
            _ => {
                let (share, minimized) = if let Some(rest) = token.strip_prefix("editor-full@") {
                    (rest, Some(true))
                } else if let Some(rest) = token.strip_prefix("response-full@") {
                    (rest, Some(false))
                } else {
                    (token, None)
                };
                let share: f32 = share.parse().ok()?;
                if !share.is_finite() || !(0.0..=1.0).contains(&share) {
                    return None;
                }
                s.ratio = share;
                match minimized {
                    Some(true) => s.response_minimized = true,
                    Some(false) => s.editor_minimized = true,
                    None => {}
                }
            }
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SplitStop::*;

    /// Shorthand: a settled state from the editor's share in percent. The
    /// ends are the minimized states with the default sticky ratio.
    fn at(editor_pct: u8) -> SplitState {
        match editor_pct {
            100 => SplitState::default().apply(EditorFull),
            0 => SplitState::default().apply(ResponseFull),
            25 | 50 | 75 => {
                SplitState { ratio: f32::from(editor_pct) / 100.0, ..Default::default() }
            }
            _ => unreachable!(),
        }
    }

    fn ratio(r: f32) -> SplitState {
        SplitState { ratio: r, ..Default::default() }
    }

    #[test]
    fn every_stop_lands_from_every_state() {
        for from in [0, 25, 50, 75, 100] {
            for stop in SplitStop::ALL {
                let start = at(from);
                let next = start.apply(stop);
                // Re-clicking the lit end restores the split instead.
                let relit = start.stop() == Some(stop) && matches!(stop, EditorFull | ResponseFull);
                if relit {
                    assert_eq!(next.stop(), Some(Even), "restore from editor {from}%");
                } else {
                    assert_eq!(next.stop(), Some(stop), "from editor {from}% via {stop:?}");
                }
            }
        }
    }

    #[test]
    fn reclicking_the_lit_end_restores_the_split() {
        let s = SplitState { response_minimized: true, ratio: 0.6, ..Default::default() };
        assert_eq!(s.apply(EditorFull), ratio(0.6));
        let s = SplitState { editor_minimized: true, ratio: 0.6, ..Default::default() };
        assert_eq!(s.apply(ResponseFull), ratio(0.6));
    }

    #[test]
    fn reclicking_a_lit_middle_stop_is_a_no_op() {
        assert_eq!(at(50).apply(Even), at(50));
    }

    #[test]
    fn endpoints_keep_the_ratio_sticky_for_the_collapse_anim() {
        let s = at(75).apply(EditorFull);
        assert!(s.response_minimized && !s.editor_minimized);
        assert_eq!(s.ratio, 0.75);
        let s = at(25).apply(ResponseFull);
        assert!(s.editor_minimized && !s.response_minimized);
        assert_eq!(s.ratio, 0.25);
    }

    #[test]
    fn at_most_one_minimized_flag_after_any_stop() {
        for from in [0, 100] {
            for stop in SplitStop::ALL {
                let s = at(from).apply(stop);
                assert!(
                    !(s.editor_minimized && s.response_minimized),
                    "from editor {from}% via {stop:?}"
                );
            }
        }
    }

    #[test]
    fn stop_is_none_between_stops() {
        assert_eq!(ratio(0.6).stop(), None);
        assert_eq!(ratio(0.75).stop(), Some(EditorBig));
        assert_eq!(ratio(0.7505).stop(), Some(EditorBig));
    }

    #[test]
    fn cycled_from_between_stops_goes_to_the_nearest_stop_that_way() {
        assert_eq!(ratio(0.6).cycled(true), at(50));
        assert_eq!(ratio(0.6).cycled(false), at(75));
        let s = ratio(0.1).cycled(true);
        assert_eq!(s, SplitState { editor_minimized: true, ratio: 0.1, ..Default::default() });
        let s = ratio(0.9).cycled(false);
        assert_eq!(s, SplitState { response_minimized: true, ratio: 0.9, ..Default::default() });
    }

    /// Review F6: the end stops are the minimized flags, never a share. A
    /// free share at either end (a token of `1` or `0`) shows both panes,
    /// so no end chip is lit, and that end's chip minimizes.
    #[test]
    fn an_unflagged_share_at_an_end_is_no_stop() {
        assert_eq!(ratio(1.0).stop(), None);
        assert_eq!(ratio(0.0).stop(), None);
        assert_eq!(ratio(0.9995).stop(), None);
        assert_eq!(
            ratio(1.0).apply(EditorFull),
            SplitState { response_minimized: true, ratio: 1.0, ..Default::default() }
        );
        assert_eq!(
            ratio(0.0).apply(ResponseFull),
            SplitState { editor_minimized: true, ratio: 0.0, ..Default::default() }
        );
    }

    /// From a free share at an end, the cycle still goes to the nearest
    /// stop each way: the minimized end lies beyond the share, the middle
    /// stops the other way.
    #[test]
    fn cycled_from_an_unflagged_end_share() {
        assert_eq!(ratio(1.0).cycled(true), at(75));
        assert_eq!(
            ratio(1.0).cycled(false),
            SplitState { response_minimized: true, ratio: 1.0, ..Default::default() }
        );
        assert_eq!(ratio(0.0).cycled(false), at(25));
        assert_eq!(
            ratio(0.0).cycled(true),
            SplitState { editor_minimized: true, ratio: 0.0, ..Default::default() }
        );
    }

    #[test]
    fn cycled_at_a_stop_walks_and_wraps() {
        // Compared by stop: the sticky ratio a minimized end keeps depends
        // on the walk, so whole states legitimately differ from `at(..)`.
        let walk = |start: SplitState, forward: bool| {
            let mut s = start;
            let mut seen = Vec::new();
            for _ in 0..5 {
                s = s.cycled(forward);
                seen.push(s.stop());
            }
            seen
        };
        assert_eq!(
            walk(at(100), true),
            [Some(EditorBig), Some(Even), Some(ResponseBig), Some(ResponseFull), Some(EditorFull)]
        );
        assert_eq!(
            walk(at(100), false),
            [Some(ResponseFull), Some(ResponseBig), Some(Even), Some(EditorBig), Some(EditorFull)]
        );
    }

    #[test]
    fn next_walks_the_stops_in_screen_order_and_wraps() {
        let mut s = EditorFull;
        let mut seen = vec![s];
        for _ in 0..4 {
            s = s.next();
            seen.push(s);
        }
        assert_eq!(seen, SplitStop::ALL.to_vec());
        assert_eq!(s.next(), EditorFull, "wraps back to the top");
    }

    #[test]
    fn prev_mirrors_next_on_every_stop() {
        for s in SplitStop::ALL {
            assert_eq!(s.next().prev(), s);
            assert_eq!(s.prev().next(), s);
        }
        assert_eq!(EditorFull.prev(), ResponseFull, "wraps back to the bottom");
    }

    #[test]
    fn stop_names_every_settled_state_exactly_once() {
        let stops: Vec<_> = [100, 75, 50, 25, 0].map(at).map(SplitState::stop).into();
        assert_eq!(stops, SplitStop::ALL.map(Some).to_vec());
    }

    #[test]
    fn tokens_round_trip_free_shares_and_minimized_states() {
        assert_eq!(ratio(0.6).to_token(), "0.6000");
        assert_eq!(SplitState::from_token("0.6000"), Some(ratio(0.6)));
        let s = SplitState { response_minimized: true, ratio: 0.6, ..Default::default() };
        assert_eq!(s.to_token(), "editor-full@0.6000");
        assert_eq!(SplitState::from_token("editor-full@0.6000"), Some(s));
        let s = SplitState { editor_minimized: true, ratio: 0.6, ..Default::default() };
        assert_eq!(s.to_token(), "response-full@0.6000");
        assert_eq!(SplitState::from_token("response-full@0.6000"), Some(s));
    }

    #[test]
    fn old_tokens_still_load() {
        assert_eq!(SplitState::from_token("even"), Some(ratio(0.5)));
        assert_eq!(SplitState::from_token("editor-big"), Some(ratio(0.75)));
        assert_eq!(SplitState::from_token("response-big"), Some(ratio(0.25)));
        assert_eq!(
            SplitState::from_token("editor-full"),
            Some(SplitState { response_minimized: true, ..Default::default() })
        );
        assert_eq!(
            SplitState::from_token("response-full"),
            Some(SplitState { editor_minimized: true, ..Default::default() })
        );
    }

    #[test]
    fn junk_tokens_are_rejected() {
        for junk in ["sideways", "", "1.5", "-0.1", "NaN", "editor-full@x", "inf"] {
            assert_eq!(SplitState::from_token(junk), None, "{junk:?}");
        }
    }
}
