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

/// `editor_collapsed_to_chrome` is `true` exactly when the Editor pane has
/// nothing but chrome to show — its params/headers table is collapsed and
/// one of those two tabs is active (the Body tab always keeps the normal
/// split, table or no table). In that case the Editor pane shrinks to
/// [`editor::CHROME_HEIGHT`] and the Response pane takes every row that
/// frees up; otherwise the two keep today's fixed 50/50 split.
pub fn compute_layout(area: Rect, editor_collapsed_to_chrome: bool) -> AppLayout {
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
    let right_constraints = if editor_collapsed_to_chrome {
        [
            Constraint::Length(editor::CHROME_HEIGHT),
            Constraint::Min(0),
        ]
    } else {
        [Constraint::Percentage(50), Constraint::Percentage(50)]
    };
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints(right_constraints)
        .split(cols[2]);
    AppLayout {
        header: rows[0],
        body,
        sidebar: cols[0],
        gutter: cols[1],
        editor: right[0],
        response: right[1],
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
        let l = compute_layout(area, false);
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
        let expanded = compute_layout(area, false);
        let collapsed = compute_layout(area, true);
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
}
