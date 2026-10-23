use crate::components::editor;
use crate::split::SplitState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Sidebar,
    Editor,
    Response,
}

impl PaneId {
    pub fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::Editor,
            Self::Editor => Self::Response,
            Self::Response => Self::Sidebar,
        }
    }

    pub fn prev(self) -> Self {
        self.next().next() // 3-cycle: two nexts == one prev
    }
}

pub struct AppLayout {
    pub header: Rect,
    /// The full-width area between the header and footer, before it's
    /// split into `sidebar`/`gutter`/`editor`/`response` — what a
    /// full-frame screen (e.g. the Variable Manager) draws into instead of
    /// the three panes.
    pub body: Rect,
    pub sidebar: Rect,
    /// The 1-col painted gutter between the sidebar and the main panes,
    /// filled with `theme.page` by `ui::draw` — the surviving separator
    /// between them now that panes no longer paint a `│` border of their
    /// own.
    pub gutter: Rect,
    pub editor: Rect,
    pub response: Rect,
    pub footer: Rect,
}

/// `editor_share` is the Editor pane's fraction of the main column while
/// both panes are visible — the eased `AnimKey::SplitRatio` value, a free
/// share between 0 and 1 (see [`crate::split`]).
///
/// `collapse_t` is the eased `AnimKey::PaneCollapse` value (Task 14):
/// `0.0` is the Editor pane's normal `editor_share` split with the
/// Response pane; `1.0` is the Editor pane hidden — shrunk to exactly its
/// [`editor::COLLAPSED_HEIGHT`]-row strip (address bar + `› show` row +
/// one eave row for the split control),
/// with the Response pane taking every row that frees up. Values strictly
/// between the two interpolate the Editor pane's height (rounded to whole
/// rows — a `Rect` can't hold a fractional one) between the two
/// endpoints' actual split heights, so the row boundary itself eases
/// smoothly rather than snapping.
///
/// `response_t` is the eased `AnimKey::ResponseCollapse` value: `1.0` is
/// the Response pane hidden — shrunk to exactly its
/// [`crate::components::response::COLLAPSED_HEIGHT`]-row strip (the header
/// strip's first row), with the Editor pane taking every freed row.
/// Applied after (and overriding) `collapse_t`'s split: with both at `1.0`
/// the editor keeps the leftover rows as empty page below its strip.
///
/// `sidebar_cols` is the saved sidebar width in columns. `None` keeps the
/// default 28% split; `Some(w)` gives the sidebar `w` columns, squeezed by
/// [`clamp_sidebar_cols`] when the window can't hold them (the saved value
/// itself is never touched here).
pub fn compute_layout(
    area: Rect,
    collapse_t: f32,
    response_t: f32,
    editor_share: f32,
    sidebar_cols: Option<u16>,
) -> AppLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(crate::components::header_bar::HEADER_HEIGHT),
            Constraint::Min(0), // body
            Constraint::Length(crate::components::footer::FOOTER_HEIGHT),
        ])
        .split(area);
    let body = rows[1];
    let (sidebar_c, main_c) = match sidebar_cols {
        None => (Constraint::Percentage(28), Constraint::Percentage(72)),
        Some(w) => {
            (Constraint::Length(clamp_sidebar_cols(body.width, i32::from(w))), Constraint::Min(0))
        }
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            sidebar_c,
            Constraint::Length(1), // painted gutter separating sidebar from main
            main_c,
        ])
        .split(body);
    let t = collapse_t.clamp(0.0, 1.0);
    // The both-panes-visible split at `editor_share` — the endpoint every
    // collapse interpolation below starts from. Computed by hand rather
    // than `Constraint::Percentage` so the share (itself animated between
    // the 25/50/75 stops) can be any fraction. The share divides the
    // *content* rows — what's left after both panes' fixed chrome (the
    // editor's address bar + tab bar, the response's header strip) — so
    // "editor at 25%" means a quarter of the usable space for the table,
    // not a quarter minus the address bar.
    let expanded = {
        let editor_h = editor_rows_for(cols[2].height, editor_share);
        [
            Rect::new(cols[2].x, cols[2].y, cols[2].width, editor_h),
            Rect::new(
                cols[2].x,
                cols[2].y.saturating_add(editor_h),
                cols[2].width,
                cols[2].height - editor_h,
            ),
        ]
    };
    let (editor, response) = if t <= 0.0 {
        (expanded[0], expanded[1])
    } else {
        let collapsed = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(editor::COLLAPSED_HEIGHT), Constraint::Min(0)])
            .split(cols[2]);
        if t >= 1.0 {
            (collapsed[0], collapsed[1])
        } else {
            let editor_h = (expanded[0].height as f32
                + (collapsed[0].height as f32 - expanded[0].height as f32) * t)
                .round() as u16;
            let response_h = cols[2].height.saturating_sub(editor_h);
            let editor_rect = Rect::new(cols[2].x, cols[2].y, cols[2].width, editor_h);
            let response_rect =
                Rect::new(cols[2].x, cols[2].y.saturating_add(editor_h), cols[2].width, response_h);
            (editor_rect, response_rect)
        }
    };
    let rt = response_t.clamp(0.0, 1.0);
    let (editor, response) = if rt <= 0.0 {
        (editor, response)
    } else {
        let chrome = crate::components::response::COLLAPSED_HEIGHT.min(cols[2].height);
        let response_h = if rt >= 1.0 {
            chrome
        } else {
            (response.height as f32 + (chrome as f32 - response.height as f32) * rt).round() as u16
        };
        let editor_h = cols[2].height.saturating_sub(response_h);
        (
            Rect::new(cols[2].x, cols[2].y, cols[2].width, editor_h),
            Rect::new(cols[2].x, cols[2].y.saturating_add(editor_h), cols[2].width, response_h),
        )
    };
    AppLayout {
        header: rows[0],
        body,
        sidebar: cols[0],
        gutter: cols[1],
        editor,
        response,
        footer: rows[2],
    }
}

/// The sidebar's narrowest width in columns.
pub const SIDEBAR_MIN_COLS: u16 = 16;
/// The narrowest the main column (editor/response) may be squeezed to by
/// the sidebar.
pub const MAIN_MIN_COLS: u16 = 40;
/// One keyboard step of the sidebar width.
pub const SIDEBAR_STEP_COLS: u16 = 4;

/// Clamps a wanted sidebar width to what a `body_width`-wide body can hold:
/// at least [`SIDEBAR_MIN_COLS`] and at most what leaves the gutter and
/// [`MAIN_MIN_COLS`] for the main column. A body too narrow for even the
/// minimum squeezes the sidebar to whatever is left (possibly 0).
pub fn clamp_sidebar_cols(body_width: u16, want: i32) -> u16 {
    let max = body_width.saturating_sub(1 + MAIN_MIN_COLS);
    want.clamp(i32::from(SIDEBAR_MIN_COLS.min(max)), i32::from(max)) as u16
}

/// The inverse of the expanded split in [`compute_layout`]: the editor
/// share that gives the editor `editor_rows` of a `column_height`-row
/// column. The share divides the content rows (after both panes' chrome),
/// so a column with no content rows answers `0.5` rather than dividing by
/// zero.
pub fn editor_share_for(column_height: u16, editor_rows: u16) -> f32 {
    let content = column_height
        .saturating_sub(editor::CHROME_HEIGHT)
        .saturating_sub(crate::components::response::HEADER_STRIP_HEIGHT);
    if content == 0 {
        return 0.5;
    }
    let editor_content = editor_rows.saturating_sub(editor::CHROME_HEIGHT);
    (f32::from(editor_content) / f32::from(content)).clamp(0.0, 1.0)
}

/// The editor's rows in a `column_height`-row column at `editor_share`
/// with both panes showing: [`compute_layout`]'s expanded split, which
/// calls it, and the forward direction of [`editor_share_for`]. A resize
/// that counts rows reads it from the split's state, since a frame drawn
/// mid-ease shows rows the state has already left (review round 2 G6).
pub fn editor_rows_for(column_height: u16, editor_share: f32) -> u16 {
    let editor_chrome = editor::CHROME_HEIGHT.min(column_height);
    let response_chrome = crate::components::response::HEADER_STRIP_HEIGHT;
    let content = column_height.saturating_sub(editor_chrome).saturating_sub(response_chrome);
    let editor_h = editor_chrome + (content as f32 * editor_share.clamp(0.0, 1.0)).round() as u16;
    editor_h.min(column_height)
}

/// The split a drag of the editor/response boundary to `editor_rows` rows
/// of editor means. At or above the editor's chrome the editor is
/// minimized; within the response's header strip of the bottom the
/// response is. Both snaps keep `start.ratio`, so restoring lands where
/// the user left it. Anywhere between, both show at the matching share.
pub fn split_for_editor_rows(
    start: SplitState,
    column_height: u16,
    editor_rows: i32,
) -> SplitState {
    let column = i32::from(column_height);
    if editor_rows <= i32::from(editor::CHROME_HEIGHT) {
        return SplitState {
            editor_minimized: true,
            response_minimized: false,
            ratio: start.ratio,
        };
    }
    if column - editor_rows <= i32::from(crate::components::response::HEADER_STRIP_HEIGHT) {
        return SplitState {
            editor_minimized: false,
            response_minimized: true,
            ratio: start.ratio,
        };
    }
    SplitState {
        editor_minimized: false,
        response_minimized: false,
        ratio: editor_share_for(column_height, editor_rows as u16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_cycles_through_all_panes_and_back() {
        let start = PaneId::Sidebar;
        let mut p = start;
        let mut seen = vec![p];
        for _ in 0..2 {
            p = p.next();
            seen.push(p);
        }
        assert_eq!(seen, vec![PaneId::Sidebar, PaneId::Editor, PaneId::Response]);
        assert_eq!(p.next(), start);
        assert_eq!(start.prev(), PaneId::Response);
    }

    #[test]
    fn layout_partitions_area() {
        let area = Rect::new(0, 0, 120, 40);
        let l = compute_layout(area, 0.0, 0.0, 0.5, None);
        assert_eq!(l.header.height, 3);
        assert_eq!(l.footer.height, 3);
        assert_eq!(l.header.y, 0);
        assert_eq!(l.footer.y, 37);
        // sidebar, then the 1-col gutter, then editor/response; editor above response
        assert!(l.sidebar.x < l.gutter.x);
        assert!(l.gutter.x < l.editor.x);
        assert_eq!(l.gutter.width, 1);
        assert_eq!(l.editor.x, l.response.x);
        assert!(l.editor.y < l.response.y);
        // body fills between header and footer
        assert_eq!(l.sidebar.y, 3);
        assert_eq!(l.sidebar.height, 34);
        assert_eq!(l.editor.height + l.response.height, 34);
        assert_eq!(l.sidebar.width + l.gutter.width + l.editor.width, 120);
    }

    #[test]
    fn collapsed_editor_shrinks_to_chrome_and_response_takes_the_rest() {
        let area = Rect::new(0, 0, 120, 40);
        let expanded = compute_layout(area, 0.0, 0.0, 0.5, None);
        let collapsed = compute_layout(area, 1.0, 0.0, 0.5, None);
        assert_eq!(
            collapsed.editor.height,
            editor::COLLAPSED_HEIGHT,
            "editor pane shrinks to exactly its collapsed strip"
        );
        assert_eq!(
            collapsed.editor.height + collapsed.response.height,
            expanded.editor.height + expanded.response.height,
            "the two panes still exactly fill the same vertical span"
        );
        assert!(
            collapsed.response.height > expanded.response.height,
            "response pane reclaims every row the table gave up"
        );
    }

    #[test]
    fn collapsed_response_shrinks_to_its_header_and_editor_takes_the_rest() {
        let area = Rect::new(0, 0, 120, 40);
        let expanded = compute_layout(area, 0.0, 0.0, 0.5, None);
        let collapsed = compute_layout(area, 0.0, 1.0, 0.5, None);
        assert_eq!(
            collapsed.response.height,
            crate::components::response::COLLAPSED_HEIGHT,
            "response pane shrinks to exactly its one-row strip"
        );
        assert_eq!(
            collapsed.editor.height + collapsed.response.height,
            expanded.editor.height + expanded.response.height,
            "the two panes still exactly fill the same vertical span"
        );
        assert!(
            collapsed.editor.height > expanded.editor.height,
            "editor pane reclaims every row the response gave up"
        );
    }

    #[test]
    fn mid_collapse_height_sits_strictly_between_both_endpoints() {
        let area = Rect::new(0, 0, 120, 40);
        let expanded = compute_layout(area, 0.0, 0.0, 0.5, None);
        let collapsed = compute_layout(area, 1.0, 0.0, 0.5, None);
        let mid = compute_layout(area, 0.5, 0.0, 0.5, None);
        assert!(
            mid.editor.height < expanded.editor.height,
            "mid-collapse editor is shorter than fully expanded"
        );
        assert!(
            mid.editor.height > collapsed.editor.height,
            "mid-collapse editor is taller than fully collapsed"
        );
        assert_eq!(
            mid.editor.height + mid.response.height,
            expanded.editor.height + expanded.response.height,
            "still exactly fills the same vertical span mid-anim"
        );
    }

    #[test]
    fn collapse_t_clamps_outside_zero_one() {
        let area = Rect::new(0, 0, 120, 40);
        let below = compute_layout(area, -0.5, 0.0, 0.5, None);
        let above = compute_layout(area, 1.5, 0.0, 0.5, None);
        assert_eq!(below.editor, compute_layout(area, 0.0, 0.0, 0.5, None).editor);
        assert_eq!(above.editor, compute_layout(area, 1.0, 0.0, 0.5, None).editor);
    }

    #[test]
    fn mid_response_collapse_sits_strictly_between_both_endpoints() {
        let area = Rect::new(0, 0, 120, 40);
        let expanded = compute_layout(area, 0.0, 0.0, 0.5, None);
        let collapsed = compute_layout(area, 0.0, 1.0, 0.5, None);
        let mid = compute_layout(area, 0.0, 0.5, 0.5, None);
        assert!(mid.response.height < expanded.response.height);
        assert!(mid.response.height > collapsed.response.height);
        assert_eq!(
            mid.editor.height + mid.response.height,
            expanded.editor.height + expanded.response.height,
            "still exactly fills the same vertical span mid-anim"
        );
    }

    /// `editor_share` divides the column's *content* rows — what's left
    /// after both panes' fixed chrome — so 0.25 gives the editor its
    /// chrome plus a quarter of the usable space, not a quarter of the
    /// raw column with the address bar eating into it. The two panes
    /// always exactly fill the same span.
    #[test]
    fn editor_share_divides_the_content_rows_after_both_panes_chrome() {
        let area = Rect::new(0, 0, 120, 40);
        let small = compute_layout(area, 0.0, 0.0, 0.25, None);
        let even = compute_layout(area, 0.0, 0.0, 0.5, None);
        let big = compute_layout(area, 0.0, 0.0, 0.75, None);
        let column = even.editor.height + even.response.height;
        let content =
            column - editor::CHROME_HEIGHT - crate::components::response::HEADER_STRIP_HEIGHT;
        for (l, share) in [(&small, 0.25), (&even, 0.5), (&big, 0.75)] {
            assert_eq!(l.editor.height + l.response.height, column);
            assert_eq!(l.editor.y + l.editor.height, l.response.y);
            let want = editor::CHROME_HEIGHT as f32 + content as f32 * share;
            assert!(
                (l.editor.height as f32 - want).abs() <= 1.0,
                "share {share}: editor {} rows, wanted about {want}",
                l.editor.height
            );
        }
        assert!(small.editor.height < even.editor.height);
        assert!(even.editor.height < big.editor.height);
    }

    /// A share strictly between two stops (the ratio anim mid-flight)
    /// lands the boundary strictly between them too.
    #[test]
    fn mid_share_sits_strictly_between_the_stops() {
        let area = Rect::new(0, 0, 120, 40);
        let even = compute_layout(area, 0.0, 0.0, 0.5, None);
        let big = compute_layout(area, 0.0, 0.0, 0.75, None);
        let mid = compute_layout(area, 0.0, 0.0, 0.625, None);
        assert!(mid.editor.height > even.editor.height);
        assert!(mid.editor.height < big.editor.height);
    }

    /// The minimized strips ignore the share: a collapse animates from
    /// whatever ratio the column held, but the settled strip heights are
    /// the panes' fixed chrome.
    #[test]
    fn collapse_endpoints_keep_their_strip_heights_at_any_share() {
        let area = Rect::new(0, 0, 120, 40);
        for share in [0.25, 0.5, 0.75] {
            let editor_min = compute_layout(area, 1.0, 0.0, share, None);
            assert_eq!(editor_min.editor.height, editor::COLLAPSED_HEIGHT);
            let response_min = compute_layout(area, 0.0, 1.0, share, None);
            assert_eq!(response_min.response.height, crate::components::response::COLLAPSED_HEIGHT);
        }
    }

    /// Mid-collapse the boundary eases from the *share's* boundary, not
    /// the even split's: collapsing the editor from 75/25 must start high.
    #[test]
    fn mid_collapse_interpolates_from_the_shares_own_boundary() {
        let area = Rect::new(0, 0, 120, 40);
        let big = compute_layout(area, 0.0, 0.0, 0.75, None);
        let mid = compute_layout(area, 0.5, 0.0, 0.75, None);
        let collapsed = compute_layout(area, 1.0, 0.0, 0.75, None);
        assert!(mid.editor.height < big.editor.height);
        assert!(mid.editor.height > collapsed.editor.height);
        assert!(
            mid.editor.height > compute_layout(area, 0.5, 0.0, 0.5, None).editor.height,
            "the 75-share mid-collapse is taller than the even split's"
        );
    }

    /// A collapsed response wins the freed rows even while the editor is
    /// itself collapsed — the editor pane simply shows its strip atop
    /// empty page.
    #[test]
    fn response_collapse_takes_precedence_over_editor_collapse() {
        let area = Rect::new(0, 0, 120, 40);
        let both = compute_layout(area, 1.0, 1.0, 0.5, None);
        assert_eq!(both.response.height, crate::components::response::COLLAPSED_HEIGHT);
        assert_eq!(
            both.editor.height + both.response.height,
            compute_layout(area, 0.0, 0.0, 0.5, None).editor.height
                + compute_layout(area, 0.0, 0.0, 0.5, None).response.height
        );
    }

    #[test]
    fn default_sidebar_is_still_28_percent() {
        let area = Rect::new(0, 0, 120, 40);
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(crate::components::header_bar::HEADER_HEIGHT),
                Constraint::Min(0),
                Constraint::Length(crate::components::footer::FOOTER_HEIGHT),
            ])
            .split(area)[1];
        let old = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(28),
                Constraint::Length(1),
                Constraint::Percentage(72),
            ])
            .split(body);
        let l = compute_layout(area, 0.0, 0.0, 0.5, None);
        assert_eq!(l.sidebar.width, old[0].width);
        assert_eq!(l.editor.width, old[2].width);
    }

    #[test]
    fn saved_sidebar_width_is_used_as_columns() {
        for width in [120, 200] {
            let l = compute_layout(Rect::new(0, 0, width, 40), 0.0, 0.0, 0.5, Some(30));
            assert_eq!(l.sidebar.width, 30, "at {width} wide");
            assert_eq!(l.gutter.x, 30);
            assert_eq!(l.editor.x, 31);
            assert_eq!(l.sidebar.width + l.gutter.width + l.editor.width, width);
        }
    }

    #[test]
    fn saved_width_squeezes_on_a_narrow_window() {
        let at = |width| {
            compute_layout(Rect::new(0, 0, width, 40), 0.0, 0.0, 0.5, Some(100)).sidebar.width
        };
        assert_eq!(at(120), 79);
        assert_eq!(at(50), 9);
    }

    #[test]
    fn clamp_sidebar_cols_respects_both_limits() {
        assert_eq!(clamp_sidebar_cols(120, 5), 16);
        assert_eq!(clamp_sidebar_cols(120, 200), 79);
        assert_eq!(clamp_sidebar_cols(120, 30), 30);
        assert_eq!(clamp_sidebar_cols(30, 20), 0);
    }

    #[test]
    fn editor_share_for_inverts_the_layout() {
        for column_height in [34u16, 20] {
            // Screen height = column + the header and footer rows.
            let area = Rect::new(
                0,
                0,
                120,
                column_height
                    + crate::components::header_bar::HEADER_HEIGHT
                    + crate::components::footer::FOOTER_HEIGHT,
            );
            for share in [0.3, 0.6] {
                let h = compute_layout(area, 0.0, 0.0, share, None).editor.height;
                let back = editor_share_for(column_height, h);
                let again = compute_layout(area, 0.0, 0.0, back, None);
                assert_eq!(again.editor.height, h, "column {column_height}, share {share}");
            }
        }
    }

    /// Review round 2 G6: the forward direction gives the rows the layout
    /// draws for a share, so a resize can count from the split's state.
    #[test]
    fn editor_rows_for_agrees_with_the_layout() {
        let chrome = editor::CHROME_HEIGHT + crate::components::response::HEADER_STRIP_HEIGHT;
        for column_height in [34u16, 20, 13, chrome, chrome - 1, 0] {
            let area = Rect::new(
                0,
                0,
                120,
                column_height
                    + crate::components::header_bar::HEADER_HEIGHT
                    + crate::components::footer::FOOTER_HEIGHT,
            );
            for share in [0.0, 0.1, 0.25, 0.3, 0.5, 0.55, 0.6, 0.75, 1.0] {
                let drawn = compute_layout(area, 0.0, 0.0, share, None).editor.height;
                assert_eq!(
                    editor_rows_for(column_height, share),
                    drawn,
                    "column {column_height}, share {share}"
                );
            }
        }
    }

    #[test]
    fn editor_share_for_zero_content_is_finite() {
        let s = editor_share_for(
            editor::CHROME_HEIGHT + crate::components::response::HEADER_STRIP_HEIGHT,
            editor::CHROME_HEIGHT,
        );
        assert!(s.is_finite() && (0.0..=1.0).contains(&s), "{s}");
        let s = editor_share_for(0, 0);
        assert!(s.is_finite() && (0.0..=1.0).contains(&s), "{s}");
    }

    #[test]
    fn split_for_editor_rows_snaps_at_the_chrome_and_the_strip() {
        let start = SplitState { ratio: 0.5, ..Default::default() };
        let strip = crate::components::response::HEADER_STRIP_HEIGHT as i32;
        let at = |rows: i32| split_for_editor_rows(start, 34, rows);
        for rows in [editor::CHROME_HEIGHT as i32, -10] {
            let s = at(rows);
            assert!(s.editor_minimized && !s.response_minimized, "rows {rows}");
        }
        for rows in [34 - strip, 100] {
            let s = at(rows);
            assert!(s.response_minimized && !s.editor_minimized, "rows {rows}");
        }
        let s = at(20);
        assert!(!s.editor_minimized && !s.response_minimized);
        assert_eq!(s.ratio, editor_share_for(34, 20));
    }

    #[test]
    fn a_drag_into_a_snap_keeps_the_start_ratio() {
        let start = SplitState { ratio: 0.6, ..Default::default() };
        assert_eq!(split_for_editor_rows(start, 34, 1).ratio, 0.6);
        assert_eq!(split_for_editor_rows(start, 34, 100).ratio, 0.6);
    }
}
