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

#[allow(dead_code)]
pub struct AppLayout {
    pub header: Rect,
    pub sidebar: Rect,
    pub editor: Rect,
    pub response: Rect,
    pub footer: Rect,
}

#[allow(dead_code)]
pub fn compute_layout(area: Rect) -> AppLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer hints
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(rows[1]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(cols[1]);
    AppLayout {
        header: rows[0],
        sidebar: cols[0],
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
        assert_eq!(seen, vec![PaneId::Sidebar, PaneId::Editor, PaneId::Response]);
        assert_eq!(p.next(), start);
        assert_eq!(start.prev(), PaneId::Response);
    }

    #[test]
    fn layout_partitions_area() {
        let area = Rect::new(0, 0, 120, 40);
        let l = compute_layout(area);
        assert_eq!(l.header.height, 1);
        assert_eq!(l.footer.height, 1);
        assert_eq!(l.header.y, 0);
        assert_eq!(l.footer.y, 39);
        // sidebar left of editor/response; editor above response
        assert!(l.sidebar.x < l.editor.x);
        assert_eq!(l.editor.x, l.response.x);
        assert!(l.editor.y < l.response.y);
        // body fills between header and footer
        assert_eq!(l.sidebar.y, 1);
        assert_eq!(l.sidebar.height, 38);
        assert_eq!(l.editor.height + l.response.height, 38);
        assert_eq!(l.sidebar.width + l.editor.width, 120);
    }
}
