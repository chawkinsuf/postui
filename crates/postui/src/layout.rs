use crate::components::editor;
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

/// `collapse_t` is the eased `AnimKey::PaneCollapse` value (Task 14):
/// `0.0` is the Editor pane's normal 50/50 split with the Response pane;
/// `1.0` is the Editor pane collapsed to nothing but chrome — shrunk to
/// exactly [`editor::CHROME_HEIGHT`], with the Response pane taking every
/// row that frees up. Values strictly between the two interpolate the
/// Editor pane's height (rounded to whole rows — a `Rect` can't hold a
/// fractional one) between the two endpoints' actual split heights, so the
/// row boundary itself eases smoothly rather than snapping.
///
/// The two endpoints are each computed with `Layout::split` exactly as
/// before (once for the 50/50 split, once for the chrome/`Min(0)` split),
/// so `collapse_t <= 0.0` and `collapse_t >= 1.0` are byte-identical to the
/// old `editor_collapsed_to_chrome: bool` behavior — callers that only
/// ever need the settled state (every existing test) can keep passing
/// `0.0`/`1.0` unchanged.
pub fn compute_layout(area: Rect, collapse_t: f32) -> AppLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(crate::components::header_bar::HEADER_HEIGHT),
            Constraint::Min(0), // body
            Constraint::Length(crate::components::footer::FOOTER_HEIGHT),
        ])
        .split(area);
    let body = rows[1];
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(28),
            Constraint::Length(1), // painted gutter separating sidebar from main
            Constraint::Percentage(72),
        ])
        .split(body);
    let t = collapse_t.clamp(0.0, 1.0);
    let expanded = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(cols[2]);
    let (editor, response) = if t <= 0.0 {
        (expanded[0], expanded[1])
    } else {
        let collapsed = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(editor::CHROME_HEIGHT),
                Constraint::Min(0),
            ])
            .split(cols[2]);
        if t >= 1.0 {
            (collapsed[0], collapsed[1])
        } else {
            let editor_h = (expanded[0].height as f32
                + (collapsed[0].height as f32 - expanded[0].height as f32) * t)
                .round() as u16;
            let response_h = cols[2].height.saturating_sub(editor_h);
            let editor_rect = Rect::new(cols[2].x, cols[2].y, cols[2].width, editor_h);
            let response_rect = Rect::new(
                cols[2].x,
                cols[2].y.saturating_add(editor_h),
                cols[2].width,
                response_h,
            );
            (editor_rect, response_rect)
        }
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
        assert_eq!(
            seen,
            vec![PaneId::Sidebar, PaneId::Editor, PaneId::Response]
        );
        assert_eq!(p.next(), start);
        assert_eq!(start.prev(), PaneId::Response);
    }

    #[test]
    fn layout_partitions_area() {
        let area = Rect::new(0, 0, 120, 40);
        let l = compute_layout(area, 0.0);
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
        let expanded = compute_layout(area, 0.0);
        let collapsed = compute_layout(area, 1.0);
        assert_eq!(
            collapsed.editor.height,
            editor::CHROME_HEIGHT,
            "editor pane shrinks to exactly its chrome"
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
    fn mid_collapse_height_sits_strictly_between_both_endpoints() {
        let area = Rect::new(0, 0, 120, 40);
        let expanded = compute_layout(area, 0.0);
        let collapsed = compute_layout(area, 1.0);
        let mid = compute_layout(area, 0.5);
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
        let below = compute_layout(area, -0.5);
        let above = compute_layout(area, 1.5);
        assert_eq!(below.editor, compute_layout(area, 0.0).editor);
        assert_eq!(above.editor, compute_layout(area, 1.0).editor);
    }
}
