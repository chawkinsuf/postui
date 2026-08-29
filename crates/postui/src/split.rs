//! The main column's editor/response split state and the transition
//! logic behind each pane's minimize / half / expand buttons.
//!
//! The column has five settled states, named by the editor's share:
//! 100/0 (response minimized to its header strip), 75/25, 50/50, 25/75,
//! and 0/100 (editor minimized to its address-bar strip). The endpoint
//! states keep living in the two existing flags (`App::table_collapsed`
//! and `session.response.collapsed`); this module adds the in-between
//! ratio and the pure button → next-state function both panes' button
//! clusters share.

/// Which pane a split button belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitPane {
    Editor,
    Response,
}

/// One of the three buttons in a pane's cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitButton {
    /// Shrink this pane to its chrome strip (0%).
    Minimize,
    /// Toggle this pane between 25% and 50%; from an endpoint it goes to
    /// whichever of the two is closest (0% → 25%, 100% → 50%), and from
    /// 75% down to 50%.
    Half,
    /// Give this pane the whole column (the other pane keeps its strip).
    Expand,
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
/// minimized flags is set; `ratio` is sticky through a minimize/expand so
/// re-opening lands where the user left the split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SplitState {
    pub editor_minimized: bool,
    pub response_minimized: bool,
    pub ratio: SplitRatio,
}

impl SplitState {
    /// The state after pressing `button` on `pane`'s cluster.
    pub fn apply(self, pane: SplitPane, button: SplitButton) -> SplitState {
        // The pane's own quarter share (25% for it, 75% for the other).
        let quarter = match pane {
            SplitPane::Editor => SplitRatio::ResponseBig,
            SplitPane::Response => SplitRatio::EditorBig,
        };
        let (pane_minimized, pane_full) = match pane {
            SplitPane::Editor => (self.editor_minimized, self.response_minimized),
            SplitPane::Response => (self.response_minimized, self.editor_minimized),
        };
        match button {
            SplitButton::Minimize => SplitState {
                editor_minimized: pane == SplitPane::Editor,
                response_minimized: pane == SplitPane::Response,
                ratio: self.ratio,
            },
            SplitButton::Expand => SplitState {
                editor_minimized: pane == SplitPane::Response,
                response_minimized: pane == SplitPane::Editor,
                ratio: self.ratio,
            },
            SplitButton::Half => SplitState {
                editor_minimized: false,
                response_minimized: false,
                ratio: if pane_minimized {
                    quarter // 0% → the closest half stop, 25%
                } else if pane_full || self.ratio != SplitRatio::Even {
                    // 100% → 50%; 25% toggles up to 50%; 75% → 50%.
                    SplitRatio::Even
                } else {
                    quarter // 50% toggles down to 25%
                },
            },
        }
    }

    /// This state as the stable token stored in the project's
    /// `.local/state.toml` (`LocalState::main_split`), named by the
    /// editor's share of the column.
    pub fn to_token(self) -> &'static str {
        if self.response_minimized {
            "editor-full"
        } else if self.editor_minimized {
            "response-full"
        } else {
            match self.ratio {
                SplitRatio::EditorBig => "editor-big",
                SplitRatio::Even => "even",
                SplitRatio::ResponseBig => "response-big",
            }
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

    /// Which of `pane`'s buttons should render lit: the one describing
    /// the pane's current share. `None` while the pane sits at 75% —
    /// that share is only ever reached from the *other* pane's cluster.
    pub fn active_button(self, pane: SplitPane) -> Option<SplitButton> {
        let (pane_minimized, pane_full) = match pane {
            SplitPane::Editor => (self.editor_minimized, self.response_minimized),
            SplitPane::Response => (self.response_minimized, self.editor_minimized),
        };
        if pane_minimized {
            Some(SplitButton::Minimize)
        } else if pane_full {
            Some(SplitButton::Expand)
        } else {
            match (pane, self.ratio) {
                (SplitPane::Editor, SplitRatio::EditorBig)
                | (SplitPane::Response, SplitRatio::ResponseBig) => None,
                _ => Some(SplitButton::Half),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SplitButton::*;
    use SplitPane::*;
    use SplitRatio::*;

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
                ratio: EditorBig,
                ..Default::default()
            },
            50 => SplitState::default(),
            25 => SplitState {
                ratio: ResponseBig,
                ..Default::default()
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn minimize_shrinks_its_pane_to_the_strip_and_frees_the_other() {
        let s = at(0).apply(Response, Minimize);
        assert!(s.response_minimized);
        assert!(!s.editor_minimized, "the editor never stays minimized too");

        let s = at(100).apply(Editor, Minimize);
        assert!(s.editor_minimized);
        assert!(!s.response_minimized);
    }

    #[test]
    fn expand_gives_its_pane_the_column_by_minimizing_the_other() {
        let s = at(50).apply(Response, Expand);
        assert!(s.editor_minimized);
        assert!(!s.response_minimized);

        let s = at(25).apply(Editor, Expand);
        assert!(s.response_minimized);
        assert!(!s.editor_minimized);
    }

    #[test]
    fn minimize_and_expand_leave_the_ratio_sticky_for_reopening() {
        let s = at(75).apply(Response, Minimize);
        assert_eq!(s.ratio, EditorBig);
        let s = at(25).apply(Response, Expand);
        assert_eq!(s.ratio, ResponseBig);
    }

    #[test]
    fn half_toggles_between_quarter_and_even() {
        // `at()` names the *editor's* share: the response at a quarter is
        // at(75), the editor at a quarter is at(25).
        assert_eq!(at(50).apply(Response, Half), at(75));
        assert_eq!(at(75).apply(Response, Half), at(50));
        assert_eq!(at(50).apply(Editor, Half), at(25));
        assert_eq!(at(25).apply(Editor, Half), at(50));
    }

    #[test]
    fn half_from_an_endpoint_goes_to_the_closest_of_quarter_and_even() {
        // Pane at 0% → 25% (its quarter share).
        assert_eq!(at(100).apply(Response, Half).ratio, EditorBig);
        assert!(!at(100).apply(Response, Half).response_minimized);
        assert_eq!(at(0).apply(Editor, Half).ratio, ResponseBig);
        assert!(!at(0).apply(Editor, Half).editor_minimized);

        // Pane at 100% → 50%.
        assert_eq!(at(0).apply(Response, Half), at(50));
        assert_eq!(at(100).apply(Editor, Half), at(50));
    }

    #[test]
    fn half_from_three_quarters_settles_at_even() {
        // The pane is at 75%: closest of {25, 50} is 50.
        assert_eq!(at(25).apply(Response, Half), at(50));
        assert_eq!(at(75).apply(Editor, Half), at(50));
    }

    #[test]
    fn minimize_and_expand_on_an_already_settled_state_are_no_ops() {
        assert_eq!(at(100).apply(Response, Minimize), at(100));
        assert_eq!(at(0).apply(Response, Expand), at(0));
        assert_eq!(at(0).apply(Editor, Minimize), at(0));
        assert_eq!(at(100).apply(Editor, Expand), at(100));
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

    #[test]
    fn active_button_names_the_panes_current_share() {
        assert_eq!(at(100).active_button(Response), Some(Minimize));
        assert_eq!(at(100).active_button(Editor), Some(Expand));
        assert_eq!(at(0).active_button(Response), Some(Expand));
        assert_eq!(at(0).active_button(Editor), Some(Minimize));

        // 25% and 50% both read as "half" for that pane.
        assert_eq!(at(50).active_button(Response), Some(Half));
        assert_eq!(at(50).active_button(Editor), Some(Half));
        assert_eq!(at(75).active_button(Response), Some(Half));
        assert_eq!(at(25).active_button(Editor), Some(Half));

        // A pane at 75% lights nothing — that share belongs to the other
        // pane's half button.
        assert_eq!(at(25).active_button(Response), None);
        assert_eq!(at(75).active_button(Editor), None);
    }
}
