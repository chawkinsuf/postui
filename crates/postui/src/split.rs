//! The main column's editor/response split state and the five-stop
//! split control's state transitions.
//!
//! The column has five settled states, named by the editor's share:
//! 100/0 (response minimized to its header strip), 75/25, 50/50, 25/75,
//! and 0/100 (editor minimized to its address-bar strip). The endpoint
//! states keep living in the two existing flags (`App::table_collapsed`
//! and `session.response.collapsed`); this module adds the in-between
//! ratio and the direct stop → state function behind the control's five
//! chips (see [`crate::paint::SplitControl`]) — one click reaches any
//! state, and the control lives in the editor pane's fixed tab-bar row
//! so it never moves out from under the pointer.

/// One of the five settled split states, named by the editor's share —
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

/// The editor's share of the column while both panes are visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitRatio {
    /// 75/25 — editor big, response quarter.
    EditorBig,
    /// 50/50.
    #[default]
    Even,
    /// 25/75 — editor quarter, response big.
    ResponseBig,
}

impl SplitRatio {
    /// The editor pane's share of the split as a fraction.
    pub fn editor_share(self) -> f32 {
        match self {
            SplitRatio::EditorBig => 0.75,
            SplitRatio::Even => 0.50,
            SplitRatio::ResponseBig => 0.25,
        }
    }
}

/// The whole split state: the two minimized endpoints plus the ratio the
/// column returns to while both panes are visible. At most one of the
/// minimized flags is set; `ratio` is sticky through the endpoint stops
/// so the collapse animation eases from the boundary the column actually
/// held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SplitState {
    pub editor_minimized: bool,
    pub response_minimized: bool,
    pub ratio: SplitRatio,
}

impl SplitState {
    /// The state after clicking `stop`'s chip: a direct jump — every
    /// stop is reachable from every state in one click.
    pub fn apply(self, stop: SplitStop) -> SplitState {
        let ratio = match stop {
            // Endpoints keep the ratio sticky: the collapse eases from
            // (and any later ratio chip is one click away regardless of)
            // where the boundary sat.
            SplitStop::EditorFull | SplitStop::ResponseFull => self.ratio,
            SplitStop::EditorBig => SplitRatio::EditorBig,
            SplitStop::Even => SplitRatio::Even,
            SplitStop::ResponseBig => SplitRatio::ResponseBig,
        };
        SplitState {
            editor_minimized: stop == SplitStop::ResponseFull,
            response_minimized: stop == SplitStop::EditorFull,
            ratio,
        }
    }

    /// The stop this state sits at — the control's lit chip. Every
    /// settled state is exactly one stop.
    pub fn stop(self) -> SplitStop {
        if self.response_minimized {
            SplitStop::EditorFull
        } else if self.editor_minimized {
            SplitStop::ResponseFull
        } else {
            match self.ratio {
                SplitRatio::EditorBig => SplitStop::EditorBig,
                SplitRatio::Even => SplitStop::Even,
                SplitRatio::ResponseBig => SplitStop::ResponseBig,
            }
        }
    }

    /// This state as the stable token stored in the project's
    /// `.local/state.toml` (`LocalState::main_split`), named by the
    /// editor's share of the column.
    pub fn to_token(self) -> &'static str {
        match self.stop() {
            SplitStop::EditorFull => "editor-full",
            SplitStop::EditorBig => "editor-big",
            SplitStop::Even => "even",
            SplitStop::ResponseBig => "response-big",
            SplitStop::ResponseFull => "response-full",
        }
    }

    /// Parses [`Self::to_token`]'s output; `None` for anything else, so a
    /// hand-edited or future token degrades to the default split.
    pub fn from_token(token: &str) -> Option<SplitState> {
        let mut s = SplitState::default();
        match token {
            "editor-full" => s.response_minimized = true,
            "response-full" => s.editor_minimized = true,
            "editor-big" => s.ratio = SplitRatio::EditorBig,
            "even" => {}
            "response-big" => s.ratio = SplitRatio::ResponseBig,
            _ => return None,
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SplitStop::*;

    /// Shorthand: a settled state from the editor's share in percent.
    fn at(editor_pct: u8) -> SplitState {
        match editor_pct {
            100 => SplitState {
                response_minimized: true,
                ..Default::default()
            },
            0 => SplitState {
                editor_minimized: true,
                ..Default::default()
            },
            75 => SplitState {
                ratio: SplitRatio::EditorBig,
                ..Default::default()
            },
            50 => SplitState::default(),
            25 => SplitState {
                ratio: SplitRatio::ResponseBig,
                ..Default::default()
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn every_stop_is_one_click_away_from_every_state() {
        for from in [0, 25, 50, 75, 100] {
            for (stop, want) in [
                (EditorFull, 100),
                (EditorBig, 75),
                (Even, 50),
                (ResponseBig, 25),
                (ResponseFull, 0),
            ] {
                let next = at(from).apply(stop);
                assert_eq!(
                    next.stop(),
                    stop,
                    "from editor {from}% via {stop:?}"
                );
                // Ratio stops land exactly; endpoints are checked below
                // (their ratio is sticky, so `at(want)` only matches when
                // coming from the default ratio).
                if !matches!(stop, EditorFull | ResponseFull) {
                    assert_eq!(next, at(want));
                }
            }
        }
    }

    #[test]
    fn endpoints_keep_the_ratio_sticky_for_the_collapse_anim() {
        let s = at(75).apply(EditorFull);
        assert!(s.response_minimized && !s.editor_minimized);
        assert_eq!(s.ratio, SplitRatio::EditorBig);
        let s = at(25).apply(ResponseFull);
        assert!(s.editor_minimized && !s.response_minimized);
        assert_eq!(s.ratio, SplitRatio::ResponseBig);
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
    fn clicking_the_current_stop_is_a_no_op() {
        for pct in [0, 25, 50, 75, 100] {
            let s = at(pct);
            assert_eq!(s.apply(s.stop()), s, "state at editor {pct}%");
        }
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
        assert_eq!(stops, SplitStop::ALL.to_vec());
    }

    #[test]
    fn tokens_round_trip_every_settled_state_and_reject_junk() {
        for pct in [0, 25, 50, 75, 100] {
            let s = at(pct);
            assert_eq!(
                SplitState::from_token(s.to_token()),
                Some(s),
                "state at editor {pct}%"
            );
        }
        assert_eq!(at(100).to_token(), "editor-full");
        assert_eq!(at(50).to_token(), "even");
        assert_eq!(SplitState::from_token("sideways"), None);
        assert_eq!(SplitState::from_token(""), None);
    }
}
