use super::{Component, DrawCtx, pane_block};
use crate::action::Action;
use crate::theme::Theme;
use postui_core::storage::RequestListing;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// One line of the sidebar listing: either a directory heading (not
/// selectable) or a request (selectable, possibly broken).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Dir(String),
    Request {
        slug: String,
        broken: Option<String>,
    },
}

#[derive(Default)]
pub struct Sidebar {
    pub rows: Vec<Row>,
    /// Always indexes a `Row::Request` when `rows` contains one; meaningless
    /// (and never read) while `rows` has no request rows at all.
    pub selected: usize,
    /// Index of the first row drawn; `draw` keeps `selected` inside the
    /// visible window by adjusting this each time it renders.
    pub scroll: usize,
    /// The slug currently loaded in the editor, kept in sync by
    /// `App::update` after every action.
    pub open_slug: Option<String>,
    /// Whether the open request has unsaved changes, likewise kept in sync
    /// by `App::update`.
    pub open_dirty: bool,
}

impl Sidebar {
    /// Rebuilds `rows` from a fresh listing: top-level requests first (no
    /// `Dir` row), then each directory's requests grouped behind a `Dir`
    /// row for its (immediate) parent path. `listing` is assumed sorted by
    /// slug (as `storage::list_requests` returns it), so requests sharing a
    /// directory are already contiguous.
    pub fn refresh(&mut self, listing: Vec<RequestListing>) {
        let prev_selected_slug = self.selected_slug();

        let (top, nested): (Vec<_>, Vec<_>) =
            listing.into_iter().partition(|l| !l.slug.contains('/'));

        let mut rows = Vec::new();
        for l in top {
            rows.push(Row::Request {
                slug: l.slug,
                broken: l.broken,
            });
        }
        let mut current_dir: Option<String> = None;
        for l in nested {
            let dir = l
                .slug
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default();
            if current_dir.as_deref() != Some(dir.as_str()) {
                rows.push(Row::Dir(dir.clone()));
                current_dir = Some(dir);
            }
            rows.push(Row::Request {
                slug: l.slug,
                broken: l.broken,
            });
        }
        self.rows = rows;

        self.selected = prev_selected_slug
            .and_then(|slug| {
                self.rows
                    .iter()
                    .position(|r| matches!(r, Row::Request { slug: s, .. } if *s == slug))
            })
            .or_else(|| self.first_request_index())
            .unwrap_or(0);
        self.scroll = 0;
    }

    /// Selects the request row for `slug`, if one exists. A no-op (leaves
    /// the current selection untouched) when `slug` isn't in `rows`.
    pub fn select_slug(&mut self, slug: &str) {
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Request { slug: s, .. } if s == slug))
        {
            self.selected = i;
        }
    }

    fn first_request_index(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Request { .. }))
    }

    fn request_indices(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| matches!(r, Row::Request { .. }).then_some(i))
            .collect()
    }

    /// The slug of the currently selected request row, or `None` if the
    /// sidebar is empty or the selection is (transiently) on a `Dir` row.
    pub fn selected_slug(&self) -> Option<String> {
        match self.rows.get(self.selected) {
            Some(Row::Request { slug, .. }) => Some(slug.clone()),
            _ => None,
        }
    }

    fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Moves `selected` by `delta` request rows (skipping `Dir` rows
    /// entirely), clamped to the first/last request row.
    fn move_selection(&mut self, delta: i32) {
        let idxs = self.request_indices();
        if idxs.is_empty() {
            return;
        }
        let pos = idxs.iter().position(|&i| i == self.selected).unwrap_or(0);
        let new_pos = (pos as i32 + delta).clamp(0, idxs.len() as i32 - 1) as usize;
        self.selected = idxs[new_pos];
    }
}

impl Component for Sidebar {
    fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match ev.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                Some(Action::Render)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                Some(Action::Render)
            }
            KeyCode::Enter => match self.selected_row()? {
                Row::Request { slug, broken: None } => Some(Action::OpenRequest(slug.clone())),
                Row::Request {
                    slug,
                    broken: Some(_),
                } => Some(Action::ShowRequestError(slug.clone())),
                Row::Dir(_) => None,
            },
            KeyCode::Char('n') => Some(Action::PromptNewRequest),
            KeyCode::Char('r') => matches!(self.selected_row()?, Row::Request { .. })
                .then_some(Action::PromptRenameRequest),
            KeyCode::Char('d') => matches!(self.selected_row()?, Row::Request { .. })
                .then_some(Action::ConfirmDeleteRequest),
            _ => None,
        }
    }

    fn handle_scroll(&mut self, delta: i16) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len().saturating_sub(1);
        self.scroll = (self.scroll as i32 + delta as i32).clamp(0, max as i32) as usize;
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Requests", ctx);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.rows.is_empty() {
            let empty = Paragraph::new(vec![Line::raw(""), Line::raw("No requests yet.")])
                .style(Style::default().fg(ctx.theme.text_muted))
                .centered();
            frame.render_widget(empty, inner);
            return;
        }

        let visible_height = inner.height as usize;
        if visible_height > 0 {
            if self.selected < self.scroll {
                self.scroll = self.selected;
            } else if self.selected >= self.scroll + visible_height {
                self.scroll = self.selected + 1 - visible_height;
            }
            let max_scroll = self.rows.len().saturating_sub(visible_height);
            self.scroll = self.scroll.min(max_scroll);
        }

        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(visible_height.max(1))
            .map(|(i, row)| self.render_row(i, row, ctx))
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

impl Sidebar {
    fn render_row(&self, idx: usize, row: &Row, ctx: &DrawCtx) -> Line<'static> {
        let theme: &Theme = ctx.theme;
        match row {
            Row::Dir(name) => {
                Line::styled(format!("▸ {name}/"), Style::default().fg(theme.text_muted))
            }
            Row::Request { slug, broken } => {
                let is_selected = idx == self.selected;
                let marker_style = if is_selected {
                    if ctx.focused {
                        Style::default().fg(theme.accent).bold()
                    } else {
                        Style::default().fg(theme.text_muted)
                    }
                } else {
                    Style::default().fg(theme.text_muted)
                };
                let text_style = if is_selected && ctx.focused {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text)
                };

                let marker = if is_selected { "\u{203a} " } else { "  " };
                let basename = slug.rsplit('/').next().unwrap_or(slug.as_str());
                let indent = if slug.contains('/') { "  " } else { "" };

                let mut spans = vec![Span::styled(marker, marker_style)];
                if !indent.is_empty() {
                    spans.push(Span::raw(indent));
                }
                if broken.is_some() {
                    spans.push(Span::styled("\u{2717} ", Style::default().fg(theme.error)));
                } else if self.open_slug.as_deref() == Some(slug.as_str()) && self.open_dirty {
                    spans.push(Span::styled("\u{25cf} ", Style::default().fg(theme.accent)));
                } else {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(basename.to_string(), text_style));
                Line::from(spans)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn listing(slugs: &[&str]) -> Vec<RequestListing> {
        slugs
            .iter()
            .map(|s| RequestListing {
                slug: s.to_string(),
                broken: None,
            })
            .collect()
    }

    #[test]
    fn refresh_groups_top_level_before_directories() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["auth/login", "ping"]));
        assert_eq!(
            s.rows,
            vec![
                Row::Request {
                    slug: "ping".into(),
                    broken: None
                },
                Row::Dir("auth".into()),
                Row::Request {
                    slug: "auth/login".into(),
                    broken: None
                },
            ]
        );
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn navigation_skips_dir_rows_and_clamps() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["auth/login", "ping"]));
        assert_eq!(s.selected_slug().as_deref(), Some("ping"));
        s.handle_key(key(KeyCode::Char('j')));
        assert_eq!(
            s.selected_slug().as_deref(),
            Some("auth/login"),
            "Dir row skipped"
        );
        s.handle_key(key(KeyCode::Char('j')));
        assert_eq!(
            s.selected_slug().as_deref(),
            Some("auth/login"),
            "clamped at the end"
        );
        s.handle_key(key(KeyCode::Char('k')));
        assert_eq!(s.selected_slug().as_deref(), Some("ping"));
        s.handle_key(key(KeyCode::Char('k')));
        assert_eq!(
            s.selected_slug().as_deref(),
            Some("ping"),
            "clamped at the start"
        );
    }

    #[test]
    fn enter_on_healthy_and_broken_rows() {
        let mut s = Sidebar {
            rows: vec![
                Row::Request {
                    slug: "ok".into(),
                    broken: None,
                },
                Row::Request {
                    slug: "bad".into(),
                    broken: Some("boom".into()),
                },
            ],
            ..Sidebar::default()
        };
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(Action::OpenRequest("ok".into()))
        );
        s.selected = 1;
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(Action::ShowRequestError("bad".into()))
        );
    }
}
